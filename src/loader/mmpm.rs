// SPDX-License-Identifier: GPL-3.0-only
//
// MMPM/2 (MultiMedia Presentation Manager/2) emulation.
//
// Implements MDM.DLL ordinals:
//   1 = mciSendCommand(usDeviceID, usMessage, ulParam1, ulParam2)
//   2 = mciSendString(pszCommandBuf, pszReturnString, cbReturnString, hwndCallback)
//   3 = mciFreeBlock(pvBlock)
//   4 = mciGetLastError(usDeviceID, pszErrorBuf, cbErrorBuf)
//
// Audio backend: SDL2 audio queue (no callback; SDL_QueueAudio push model).
// DosBeep tone synthesis also lives here.
//
// NOTE: sdl2::sys exports `pub const None: u32 = 0` from C SDL2 bindings.
// This shadows Rust's `None` inside any scope where `use sdl2::sys::*` is active.
// Workaround: use `std::option::Option::None` for optional values, and use
// `if let Some(x) = ...` instead of `match ... { None => ... }` patterns.

use std::collections::HashMap;
use log::{debug, warn};

// ── MCI constants ─────────────────────────────────────────────────────────────

pub const MCI_OPEN:   u16 = 0x0001;
pub const MCI_CLOSE:  u16 = 0x0002;
pub const MCI_PLAY:   u16 = 0x0006;
pub const MCI_STOP:   u16 = 0x0008;
pub const MCI_STATUS: u16 = 0x001E;

// MCI_OPEN ulParam1 flags
const MCI_OPEN_TYPE:    u32 = 0x2000;
const MCI_OPEN_ELEMENT: u32 = 0x8000;

// MCI_PLAY ulParam1 flags
const MCI_WAIT: u32 = 0x0002;

// MCI_STATUS item codes
const MCI_STATUS_POSITION: u32 = 0x0001;
const MCI_STATUS_LENGTH:   u32 = 0x0002;
const MCI_STATUS_MODE:     u32 = 0x0004;

// MCI mode values
const MCI_MODE_NOT_READY: u32 = 0x0524;
const MCI_MODE_STOP:      u32 = 0x0529;
const MCI_MODE_PLAY:      u32 = 0x0526;

// MCI error codes
pub const MCIERR_SUCCESS:              u32 = 0;
pub const MCIERR_INVALID_DEVICE_ID:    u32 = 263;
pub const MCIERR_CANNOT_LOAD_DRIVER:   u32 = 262;
pub const MCIERR_UNSUPPORTED_FUNCTION: u32 = 269;
pub const MCIERR_INVALID_DEVICE_NAME:  u32 = 256;

// ── MCI device state ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MciMode { NotReady, Stopped, Playing }

pub struct MciDevice {
    /// Device type string, e.g. "waveaudio"
    pub device_type: String,
    /// Element name (filename in OS/2 path form)
    pub element_name: String,
    /// Current playback mode
    pub mode: MciMode,
    /// SDL2 AudioDeviceID — 0 means no device open
    pub audio_dev: u32,
    /// Decoded PCM data from SDL_LoadWAV_RW (null if not loaded)
    pub wav_buf: *mut sdl2::sys::Uint8,
    /// Length of wav_buf in bytes
    pub wav_len: u32,
    /// Audio format from SDL_LoadWAV_RW (freq, format, channels)
    pub wav_freq: i32,
    pub wav_format: u16,
    pub wav_channels: u8,
}

// SAFETY: MciDevice holds raw SDL pointers but access is serialised by
// the Mutex<MmpmManager> in SharedState, so inter-thread aliasing cannot occur.
unsafe impl Send for MciDevice {}

impl Drop for MciDevice {
    fn drop(&mut self) {
        unsafe {
            if self.audio_dev != 0 {
                sdl2::sys::SDL_CloseAudioDevice(self.audio_dev);
                self.audio_dev = 0;
            }
            if !self.wav_buf.is_null() {
                sdl2::sys::SDL_FreeWAV(self.wav_buf);
                self.wav_buf = std::ptr::null_mut();
            }
        }
    }
}

// ── MmpmManager ───────────────────────────────────────────────────────────────

pub struct MmpmManager {
    pub devices: HashMap<u16, MciDevice>,
    pub next_device_id: u16,
    pub last_error: u32,
}

impl MmpmManager {
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
            next_device_id: 1,
            last_error: 0,
        }
    }
}

// ── DosBeep tone synthesis ────────────────────────────────────────────────────

/// Generate a sine-wave tone using SDL2 audio.
///
/// Synchronous: blocks until the tone has finished playing.
/// Called from the VCPU thread; `SDL_InitSubSystem` / `SDL_OpenAudioDevice`
/// are safe to call from any thread on Linux after SDL has been initialised
/// on the main thread.
pub fn beep_tone(freq_hz: u32, duration_ms: u32) {
    use sdl2::sys as sys;
    use std::os::raw::{c_int, c_void};

    if freq_hz == 0 || duration_ms == 0 {
        return;
    }

    unsafe {
        // Ensure audio subsystem is initialised.
        if sys::SDL_WasInit(sys::SDL_INIT_AUDIO) == 0 {
            if sys::SDL_InitSubSystem(sys::SDL_INIT_AUDIO) != 0 {
                warn!("DosBeep: SDL_InitSubSystem(AUDIO) failed");
                return;
            }
        }

        let desired = sys::SDL_AudioSpec {
            freq:     44100,
            format:   sys::AUDIO_S16LSB as sys::SDL_AudioFormat,
            channels: 1,
            silence:  0,
            samples:  512,
            padding:  0,
            size:     0,
            callback: std::option::Option::None,
            userdata: std::ptr::null_mut(),
        };
        let mut obtained: sys::SDL_AudioSpec = std::mem::zeroed();
        let dev = sys::SDL_OpenAudioDevice(
            std::ptr::null(),
            0 as c_int,
            &desired,
            &mut obtained,
            (sys::SDL_AUDIO_ALLOW_FREQUENCY_CHANGE | sys::SDL_AUDIO_ALLOW_CHANNELS_CHANGE) as c_int,
        );
        if dev == 0 {
            warn!("DosBeep: SDL_OpenAudioDevice failed");
            return;
        }

        let rate = obtained.freq as u32;
        let chans = obtained.channels as usize;
        let num_frames = (rate * duration_ms / 1000) as usize;
        let amplitude = (i16::MAX as f32) * 0.30;

        let mut pcm: Vec<i16> = Vec::with_capacity(num_frames * chans);
        for i in 0..num_frames {
            let t = i as f32 / rate as f32;
            let v = (amplitude
                * (2.0 * std::f32::consts::PI * freq_hz as f32 * t).sin())
                as i16;
            for _ in 0..chans {
                pcm.push(v);
            }
        }

        let byte_len = (pcm.len() * std::mem::size_of::<i16>()) as u32;
        sys::SDL_QueueAudio(dev, pcm.as_ptr() as *const c_void, byte_len);
        sys::SDL_PauseAudioDevice(dev, 0);

        // Wait for the audio queue to drain fully.
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis((duration_ms + 200) as u64);
        while sys::SDL_GetQueuedAudioSize(dev) > 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        sys::SDL_CloseAudioDevice(dev);
    }
}

// ── impl Loader ───────────────────────────────────────────────────────────────

impl super::Loader {
    /// MDM ordinal 1: `mciSendCommand(usDeviceID, usMessage, ulParam1, ulParam2)`
    ///
    /// `ulParam2` is a guest pointer to the message-specific parameter block.
    pub fn mci_send_command(
        &self,
        device_id: u32,
        message: u32,
        param1: u32,
        param2: u32,
    ) -> u32 {
        let msg = message as u16;
        debug!(
            "  mciSendCommand(devID={}, msg=0x{:04X}, p1=0x{:08X}, p2=0x{:08X})",
            device_id, message, param1, param2
        );

        match msg {
            MCI_OPEN   => self.mci_open(param1, param2),
            MCI_CLOSE  => self.mci_close(device_id),
            MCI_PLAY   => self.mci_play(device_id, param1, param2),
            MCI_STOP   => self.mci_stop(device_id),
            MCI_STATUS => self.mci_status(device_id, param1, param2),
            _ => {
                warn!("mciSendCommand: unsupported message 0x{:04X}", message);
                MCIERR_UNSUPPORTED_FUNCTION
            }
        }
    }

    /// MDM ordinal 2: `mciSendString(pszCmd, pszRet, cbRet, hwnd)`
    ///
    /// Parses a simple MCI command string and dispatches to mci_send_command.
    /// Supported forms:
    ///   `open <file> type waveaudio alias <name>`
    ///   `play <alias> [wait]`
    ///   `stop <alias>`
    ///   `close <alias>`
    ///   `status <alias> mode`
    pub fn mci_send_string(
        &self,
        psz_cmd: u32,
        psz_ret: u32,
        cb_ret: u32,
        _hwnd: u32,
    ) -> u32 {
        let cmd = self.read_guest_string(psz_cmd).to_ascii_lowercase();
        debug!("  mciSendString('{}')", cmd);

        let write_ret = |s: &str| {
            if psz_ret != 0 && cb_ret > 0 {
                let bytes = s.as_bytes();
                let n = bytes.len().min(cb_ret as usize - 1);
                self.guest_write_bytes(psz_ret, &bytes[..n]);
                self.guest_write::<u8>(psz_ret + n as u32, 0);
            }
        };

        let tokens: Vec<&str> = cmd.split_whitespace().collect();
        if tokens.is_empty() {
            return MCIERR_UNSUPPORTED_FUNCTION;
        }

        match tokens[0] {
            "open" => {
                if tokens.len() < 2 {
                    return MCIERR_INVALID_DEVICE_NAME;
                }
                let mut device_type = "waveaudio".to_string();
                let mut element_name = String::new();
                let mut i = 1usize;
                while i < tokens.len() {
                    match tokens[i] {
                        "type" => {
                            if i + 1 < tokens.len() {
                                device_type = tokens[i + 1].to_string();
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "alias" | "wait" | "notify" => {
                            i += 2.min(tokens.len() - i);
                        }
                        _ => {
                            if element_name.is_empty() {
                                element_name = tokens[i].to_string();
                            }
                            i += 1;
                        }
                    }
                }
                let rc = self.mci_open_internal(&device_type, &element_name, 0);
                if rc == MCIERR_SUCCESS {
                    let id = self.shared.mmpm_mgr.lock().unwrap().next_device_id - 1;
                    write_ret(&id.to_string());
                }
                rc
            }

            "play" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                let wait = tokens.iter().any(|&t| t == "wait");
                let flags = if wait { MCI_WAIT } else { 0 };
                self.mci_play(dev_id as u32, flags, 0)
            }

            "stop" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                self.mci_stop(dev_id as u32)
            }

            "close" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                self.mci_close(dev_id as u32)
            }

            "status" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                let item_str = tokens.get(2).copied().unwrap_or("");
                let mode = self.shared.mmpm_mgr.lock().unwrap()
                    .devices.get(&(dev_id as u16)).map(|d| d.mode);
                let result = match (item_str, mode) {
                    ("mode", Some(MciMode::Playing))  => "playing",
                    ("mode", Some(MciMode::Stopped))  => "stopped",
                    ("mode", _)                       => "not ready",
                    _ => "",
                };
                write_ret(result);
                if mode.is_none() { MCIERR_INVALID_DEVICE_ID } else { MCIERR_SUCCESS }
            }

            _ => {
                warn!("mciSendString: unknown verb '{}'", tokens[0]);
                MCIERR_UNSUPPORTED_FUNCTION
            }
        }
    }

    /// MDM ordinal 3: `mciFreeBlock(pvBlock)` — no-op in our implementation.
    pub fn mci_free_block(&self, _pv: u32) -> u32 {
        debug!("  mciFreeBlock");
        MCIERR_SUCCESS
    }

    /// MDM ordinal 4: `mciGetLastError(usDeviceID, pszErrorBuf, cbErrorBuf)`
    pub fn mci_get_last_error(&self, _device_id: u32, psz_buf: u32, cb_buf: u32) -> u32 {
        let err = self.shared.mmpm_mgr.lock().unwrap().last_error;
        debug!("  mciGetLastError -> {}", err);
        if psz_buf != 0 && cb_buf > 0 {
            let msg = format!("Error {}", err);
            let bytes = msg.as_bytes();
            let n = bytes.len().min(cb_buf as usize - 1);
            self.guest_write_bytes(psz_buf, &bytes[..n]);
            self.guest_write::<u8>(psz_buf + n as u32, 0);
        }
        err
    }

    // ── Internal MCI helpers ──────────────────────────────────────────────────

    fn mci_open(&self, param1: u32, param2: u32) -> u32 {
        if param2 == 0 {
            return MCIERR_CANNOT_LOAD_DRIVER;
        }
        // MCI_OPEN_PARMS layout (guest):
        //  +0:  hwndCallback (u32)
        //  +4:  usDeviceID   (u16)  ← output
        //  +6:  usReserved   (u16)
        //  +8:  pszDeviceType (u32 ptr)
        //  +12: pszElementName (u32 ptr)
        let psz_type = if param1 & MCI_OPEN_TYPE != 0 {
            self.guest_read::<u32>(param2 + 8).unwrap_or(0)
        } else {
            0
        };
        let psz_elem = if param1 & MCI_OPEN_ELEMENT != 0 {
            self.guest_read::<u32>(param2 + 12).unwrap_or(0)
        } else {
            0
        };

        let device_type = if psz_type != 0 {
            self.read_guest_string(psz_type).to_ascii_lowercase()
        } else {
            "waveaudio".to_string()
        };
        let element_name = if psz_elem != 0 {
            self.read_guest_string(psz_elem)
        } else {
            String::new()
        };

        let rc = self.mci_open_internal(&device_type, &element_name, 0);
        if rc == MCIERR_SUCCESS {
            let id = self.shared.mmpm_mgr.lock().unwrap().next_device_id - 1;
            self.guest_write::<u16>(param2 + 4, id);
        }
        rc
    }

    fn mci_open_internal(&self, device_type: &str, element_name: &str, _flags: u32) -> u32 {
        if device_type != "waveaudio" {
            warn!("mciOpen: unsupported device type '{}'", device_type);
            return MCIERR_INVALID_DEVICE_NAME;
        }

        let (wav_buf, wav_len, wav_freq, wav_format, wav_channels) =
            if !element_name.is_empty() {
                match self.load_wav_via_vfs(element_name) {
                    Ok(data) => data,
                    Err(e) => {
                        warn!("mciOpen: failed to load WAV '{}': {}", element_name, e);
                        return MCIERR_CANNOT_LOAD_DRIVER;
                    }
                }
            } else {
                // Empty waveaudio device — no WAV data
                (
                    std::ptr::null_mut(),
                    0u32,
                    44100i32,
                    sdl2::sys::AUDIO_S16LSB as u16,
                    2u8,
                )
            };

        let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
        let id = mgr.next_device_id;
        mgr.next_device_id = mgr.next_device_id.wrapping_add(1).max(1);
        mgr.devices.insert(id, MciDevice {
            device_type: device_type.to_string(),
            element_name: element_name.to_string(),
            mode: MciMode::Stopped,
            audio_dev: 0,
            wav_buf,
            wav_len,
            wav_freq,
            wav_format,
            wav_channels,
        });
        debug!("mciOpen: opened device ID {} for '{}'", id, element_name);
        MCIERR_SUCCESS
    }

    fn mci_close(&self, device_id: u32) -> u32 {
        if self.shared.mmpm_mgr.lock().unwrap()
            .devices.remove(&(device_id as u16)).is_some()
        {
            debug!("mciClose: closed device {}", device_id);
            MCIERR_SUCCESS
        } else {
            MCIERR_INVALID_DEVICE_ID
        }
    }

    fn mci_play(&self, device_id: u32, param1: u32, _param2: u32) -> u32 {
        use sdl2::sys as sys;
        use std::os::raw::{c_int, c_void};

        // Extract WAV data with lock held briefly, then drop the lock
        let (wav_buf, wav_len, freq, format, channels) = {
            let mgr = self.shared.mmpm_mgr.lock().unwrap();
            if let Some(dev) = mgr.devices.get(&(device_id as u16)) {
                if dev.wav_buf.is_null() {
                    return MCIERR_CANNOT_LOAD_DRIVER;
                }
                (dev.wav_buf, dev.wav_len, dev.wav_freq, dev.wav_format, dev.wav_channels)
            } else {
                return MCIERR_INVALID_DEVICE_ID;
            }
        };

        let audio_dev = unsafe {
            if sys::SDL_WasInit(sys::SDL_INIT_AUDIO) == 0 {
                if sys::SDL_InitSubSystem(sys::SDL_INIT_AUDIO) != 0 {
                    warn!("mciPlay: SDL_InitSubSystem(AUDIO) failed");
                    return MCIERR_CANNOT_LOAD_DRIVER;
                }
            }

            let desired = sys::SDL_AudioSpec {
                freq:     freq,
                format:   format as sys::SDL_AudioFormat,
                channels: channels,
                silence:  0,
                samples:  1024,
                padding:  0,
                size:     0,
                callback: std::option::Option::None,
                userdata: std::ptr::null_mut(),
            };
            let mut obtained: sys::SDL_AudioSpec = std::mem::zeroed();
            let dev = sys::SDL_OpenAudioDevice(
                std::ptr::null(),
                0 as c_int,
                &desired,
                &mut obtained,
                sys::SDL_AUDIO_ALLOW_ANY_CHANGE as c_int,
            );
            if dev == 0 {
                warn!("mciPlay: SDL_OpenAudioDevice failed");
                return MCIERR_CANNOT_LOAD_DRIVER;
            }

            // Convert if format differs from what was decoded
            let needs_conv = obtained.freq != desired.freq
                || obtained.format != desired.format
                || obtained.channels != desired.channels;

            if needs_conv {
                if let Some(converted) = convert_audio(
                    wav_buf, wav_len,
                    desired.format, desired.channels, desired.freq,
                    obtained.format, obtained.channels, obtained.freq,
                ) {
                    let len = converted.len() as u32;
                    // Box the converted data so it stays alive during queuing
                    let ptr = Box::into_raw(converted.into_boxed_slice());
                    sys::SDL_QueueAudio(dev, (*ptr).as_ptr() as *const c_void, len);
                    // Leak intentionally — SDL2 copies the data in SDL_QueueAudio
                    drop(Box::from_raw(ptr));
                } else {
                    sys::SDL_QueueAudio(dev, wav_buf as *const c_void, wav_len);
                }
            } else {
                sys::SDL_QueueAudio(dev, wav_buf as *const c_void, wav_len);
            }

            sys::SDL_PauseAudioDevice(dev, 0);
            dev
        };

        // Update device state
        {
            let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
            if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
                unsafe {
                    if dev.audio_dev != 0 {
                        sdl2::sys::SDL_CloseAudioDevice(dev.audio_dev);
                    }
                }
                dev.audio_dev = audio_dev;
                dev.mode = MciMode::Playing;
            }
        }

        if param1 & MCI_WAIT != 0 {
            // Synchronous: wait until the queue drains
            let bytes_per_sec = freq as u64 * channels as u64 * 2;
            let max_ms = if bytes_per_sec > 0 {
                wav_len as u64 * 1000 / bytes_per_sec + 500
            } else {
                2000
            };
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(max_ms);
            while unsafe { sdl2::sys::SDL_GetQueuedAudioSize(audio_dev) } > 0
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            self.mci_stop(device_id);
        }

        debug!("mciPlay: device {} playing", device_id);
        MCIERR_SUCCESS
    }

    fn mci_stop(&self, device_id: u32) -> u32 {
        let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
        if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
            unsafe {
                if dev.audio_dev != 0 {
                    sdl2::sys::SDL_CloseAudioDevice(dev.audio_dev);
                    dev.audio_dev = 0;
                }
            }
            dev.mode = MciMode::Stopped;
            debug!("mciStop: device {} stopped", device_id);
            MCIERR_SUCCESS
        } else {
            MCIERR_INVALID_DEVICE_ID
        }
    }

    fn mci_status(&self, device_id: u32, _param1: u32, param2: u32) -> u32 {
        let mgr = self.shared.mmpm_mgr.lock().unwrap();
        if let Some(dev) = mgr.devices.get(&(device_id as u16)) {
            if param2 == 0 {
                return MCIERR_SUCCESS;
            }
            // MCI_STATUS_PARMS: +0 hwndCallback, +4 ulReturn (out), +8 ulItem
            let item = self.guest_read::<u32>(param2 + 8).unwrap_or(0);
            let val: u32 = match item {
                MCI_STATUS_MODE => match dev.mode {
                    MciMode::Playing  => MCI_MODE_PLAY,
                    MciMode::Stopped  => MCI_MODE_STOP,
                    MciMode::NotReady => MCI_MODE_NOT_READY,
                },
                MCI_STATUS_LENGTH => {
                    let bits = (dev.wav_format as u32)
                        & sdl2::sys::SDL_AUDIO_MASK_BITSIZE;
                    let bytes_per_frame = (bits / 8).max(1) * dev.wav_channels as u32;
                    let bytes_per_sec = dev.wav_freq as u32 * bytes_per_frame;
                    if bytes_per_sec > 0 {
                        dev.wav_len / bytes_per_sec * 1000
                    } else {
                        0
                    }
                }
                MCI_STATUS_POSITION => 0, // position tracking not implemented
                _ => 0,
            };
            self.guest_write::<u32>(param2 + 4, val);
            MCIERR_SUCCESS
        } else {
            MCIERR_INVALID_DEVICE_ID
        }
    }

    /// Resolve an alias or numeric device ID string to a device ID.
    fn mci_resolve_alias(&self, alias: &str) -> u16 {
        if let Ok(n) = alias.parse::<u16>() {
            return n;
        }
        let mgr = self.shared.mmpm_mgr.lock().unwrap();
        for (&id, dev) in &mgr.devices {
            if dev.element_name.eq_ignore_ascii_case(alias) {
                return id;
            }
        }
        0
    }

    /// Load a WAV file through the VFS and decode it with SDL_LoadWAV_RW.
    ///
    /// Returns `(audio_buf, audio_len, freq, format, channels)` on success.
    fn load_wav_via_vfs(
        &self,
        os2_path: &str,
    ) -> Result<(*mut sdl2::sys::Uint8, u32, i32, u16, u8), String> {
        use sdl2::sys as sys;
        use std::os::raw::c_void;
        use super::vfs::{ExistAction, FileAttribute, NewAction, OpenFlags, OpenMode, SharingMode};

        // Read file bytes through the VFS
        let file_bytes = {
            let mut dm = self.shared.drive_mgr.lock().unwrap();
            let open_flags = OpenFlags {
                exist_action: ExistAction::Open,
                new_action:   NewAction::Fail,
            };
            let (hf, _) = dm.open_file(
                os2_path,
                OpenMode::ReadOnly,
                SharingMode::DenyNone,
                open_flags,
                FileAttribute::NORMAL,
            ).map_err(|e| format!("{:?}", e))?;
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = dm.read_file(hf, &mut chunk).map_err(|e| format!("{:?}", e))?;
                if n == 0 { break; }
                buf.extend_from_slice(&chunk[..n]);
            }
            let _ = dm.close_file(hf);
            buf
        };

        if file_bytes.is_empty() {
            return Err("empty WAV file".to_string());
        }

        unsafe {
            let rw = sys::SDL_RWFromConstMem(
                file_bytes.as_ptr() as *const c_void,
                file_bytes.len() as std::os::raw::c_int,
            );
            if rw.is_null() {
                return Err("SDL_RWFromConstMem failed".to_string());
            }

            let mut wav_spec: sys::SDL_AudioSpec = std::mem::zeroed();
            let mut audio_buf: *mut sys::Uint8 = std::ptr::null_mut();
            let mut audio_len: sys::Uint32 = 0;

            // freesrc=1 → SDL frees the RWops after reading
            let ret = sys::SDL_LoadWAV_RW(
                rw, 1,
                &mut wav_spec,
                &mut audio_buf,
                &mut audio_len,
            );
            if ret.is_null() {
                return Err("SDL_LoadWAV_RW failed".to_string());
            }

            Ok((audio_buf, audio_len, wav_spec.freq, wav_spec.format, wav_spec.channels))
        }
    }
}

// ── Audio format conversion helper ───────────────────────────────────────────

/// Convert PCM audio between formats using SDL_BuildAudioCVT / SDL_ConvertAudio.
///
/// Returns converted samples as a `Vec<u8>`, or `None` if conversion not needed or fails.
///
/// SAFETY: `src` must point to valid PCM data of `src_len` bytes.
unsafe fn convert_audio(
    src: *mut sdl2::sys::Uint8,
    src_len: u32,
    src_fmt: u16,
    src_ch: u8,
    src_rate: i32,
    dst_fmt: u16,
    dst_ch: u8,
    dst_rate: i32,
) -> std::option::Option<Vec<u8>> {
    use sdl2::sys as sys;
    use std::os::raw::c_int;

    unsafe {
        let mut cvt: sys::SDL_AudioCVT = std::mem::zeroed();
        let built = sys::SDL_BuildAudioCVT(
            &mut cvt,
            src_fmt as sys::SDL_AudioFormat, src_ch, src_rate,
            dst_fmt as sys::SDL_AudioFormat, dst_ch, dst_rate,
        );
        if built <= 0 {
            return std::option::Option::None;
        }

        let buf_len = (src_len as f64 * cvt.len_ratio).ceil() as usize + 16;
        let mut buf: Vec<u8> = vec![0u8; buf_len];
        std::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), src_len as usize);

        cvt.buf = buf.as_mut_ptr();
        cvt.len = src_len as c_int;

        if sys::SDL_ConvertAudio(&mut cvt) != 0 {
            return std::option::Option::None;
        }

        buf.truncate(cvt.len_cvt as usize);
        std::option::Option::Some(buf)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mmpm_manager_new() {
        let mgr = MmpmManager::new();
        assert!(mgr.devices.is_empty());
        assert_eq!(mgr.next_device_id, 1);
        assert_eq!(mgr.last_error, 0);
    }

    #[test]
    fn test_mci_constants_nonzero() {
        assert_ne!(MCI_OPEN, 0);
        assert_ne!(MCI_CLOSE, 0);
        assert_ne!(MCI_PLAY, 0);
        assert_ne!(MCI_STOP, 0);
        assert_ne!(MCI_STATUS, 0);
    }

    #[test]
    fn test_mci_error_codes_distinct() {
        let codes = [
            MCIERR_SUCCESS,
            MCIERR_INVALID_DEVICE_ID,
            MCIERR_CANNOT_LOAD_DRIVER,
            MCIERR_UNSUPPORTED_FUNCTION,
            MCIERR_INVALID_DEVICE_NAME,
        ];
        assert_eq!(MCIERR_SUCCESS, 0);
        for &c in &codes[1..] {
            assert_ne!(c, 0, "error code should be non-zero");
        }
        let mut seen = std::collections::HashSet::new();
        for &c in &codes {
            assert!(seen.insert(c), "duplicate error code: {}", c);
        }
    }

    #[test]
    fn test_mci_mode_constants() {
        assert_eq!(MCI_MODE_STOP, 0x0529);
        assert_eq!(MCI_MODE_PLAY, 0x0526);
        assert_ne!(MCI_MODE_NOT_READY, MCI_MODE_STOP);
        assert_ne!(MCI_MODE_NOT_READY, MCI_MODE_PLAY);
    }

    #[test]
    fn test_mci_device_drop_null_ptrs() {
        // MciDevice with null ptrs must drop without panicking or calling SDL
        let dev = MciDevice {
            device_type: "waveaudio".to_string(),
            element_name: String::new(),
            mode: MciMode::Stopped,
            audio_dev: 0,
            wav_buf: std::ptr::null_mut(),
            wav_len: 0,
            wav_freq: 44100,
            wav_format: sdl2::sys::AUDIO_S16LSB as u16,
            wav_channels: 2,
        };
        drop(dev);
    }

    #[test]
    fn test_beep_tone_zero_inputs() {
        // Must return immediately without crashing
        beep_tone(0, 1000);
        beep_tone(440, 0);
        beep_tone(0, 0);
    }
}
