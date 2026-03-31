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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use log::{debug, warn};

// ── MCI message IDs ───────────────────────────────────────────────────────────

pub const MCI_OPEN:   u16 = 0x0001;
pub const MCI_CLOSE:  u16 = 0x0002;
pub const MCI_PLAY:   u16 = 0x0006;
pub const MCI_STOP:   u16 = 0x0008;
pub const MCI_SEEK:   u16 = 0x0009;
pub const MCI_RECORD: u16 = 0x000B;
pub const MCI_SET:    u16 = 0x000D;
pub const MCI_STATUS: u16 = 0x001E;

// ── MCI ulParam1 flags ────────────────────────────────────────────────────────

/// Post MM_MCINOTIFY to hwndCallback when the command completes (non-blocking).
pub const MCI_NOTIFY: u32 = 0x0001;
/// Block until command completes.
const MCI_WAIT: u32 = 0x0002;
/// ulFrom field in MCI_PLAY_PARMS is valid — start playback from this position.
const MCI_FROM: u32 = 0x0040;

// MCI_OPEN ulParam1 flags
const MCI_OPEN_TYPE:    u32 = 0x2000;
const MCI_OPEN_ELEMENT: u32 = 0x8000;

// MCI_SET ulParam1 flags
/// `ulAudio` and `ulLevel` (volume) fields in MCI_SET_PARMS are valid.
pub const MCI_SET_AUDIO:  u32 = 0x0000_0800;
/// Volume level in MCI_SET_PARMS.ulLevel (0–100).
pub const MCI_SET_VOLUME: u32 = 0x0000_0400;

// MCI_AUDIO channel constants (MCI_SET_PARMS.ulAudio)
const MCI_AUDIO_ALL:   u32 = 0;
#[allow(dead_code)]
const MCI_AUDIO_LEFT:  u32 = 1;
#[allow(dead_code)]
const MCI_AUDIO_RIGHT: u32 = 2;

// MCI_STATUS item codes
const MCI_STATUS_POSITION: u32 = 0x0001;
const MCI_STATUS_LENGTH:   u32 = 0x0002;
const MCI_STATUS_MODE:     u32 = 0x0004;

// MCI mode values
const MCI_MODE_NOT_READY: u32 = 0x0524;
const MCI_MODE_STOP:      u32 = 0x0529;
const MCI_MODE_PLAY:      u32 = 0x0526;

// ── MMPM/2 window messages ────────────────────────────────────────────────────

/// MM_MCINOTIFY — posted to hwndCallback when an MCI command with MCI_NOTIFY
/// completes.  Defined in os2me.h as `WM_MMPMBASE + 2 = 0x0502`.
/// mp1 = USHORT usNotifyCode, mp2 = USHORT usDeviceID.
pub const MM_MCINOTIFY: u32 = 0x0502;

/// Notify codes for MM_MCINOTIFY.mp1 (bit flags per IBM MMPM/2 Programming Guide).
pub const MCI_NOTIFY_SUCCESSFUL: u32 = 0x0001;
pub const MCI_NOTIFY_SUPERSEDED: u32 = 0x0002;
pub const MCI_NOTIFY_ABORTED:    u32 = 0x0004;
pub const MCI_NOTIFY_ERROR:      u32 = 0x0008;

// ── MCI error codes ───────────────────────────────────────────────────────────

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
    /// Playback start position in bytes (set by MCI_SEEK or MCI_FROM flag).
    /// The next MCI_PLAY queues audio starting from this byte offset.
    pub current_position: u32,
    /// Volume level 0–100 (default 100).  Applied via SDL_MixAudioFormat.
    pub volume: u8,
    /// Window handle to receive MM_MCINOTIFY when MCI_NOTIFY play completes.
    pub notify_hwnd: u32,
    /// Set to `true` by mci_stop() to abort an in-flight notify watcher thread.
    pub notify_cancel: Option<Arc<AtomicBool>>,
}

// SAFETY: MciDevice holds raw SDL pointers but access is serialised by
// the Mutex<MmpmManager> in SharedState, so inter-thread aliasing cannot occur.
// Arc<AtomicBool> is unconditionally Send+Sync.
unsafe impl Send for MciDevice {}

impl Drop for MciDevice {
    fn drop(&mut self) {
        // Cancel any in-flight notify watcher.
        if let Some(ref cancel) = self.notify_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
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

impl Default for MmpmManager {
    fn default() -> Self { Self::new() }
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
        if sys::SDL_WasInit(sys::SDL_INIT_AUDIO) == 0
            && sys::SDL_InitSubSystem(sys::SDL_INIT_AUDIO) != 0 {
                warn!("DosBeep: SDL_InitSubSystem(AUDIO) failed");
                return;
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

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Post a PM message to the message queue belonging to `hwnd`.
///
/// Safe to call from any thread (e.g. an MCI notify watcher thread).
/// Silently does nothing if `hwnd` has no associated message queue.
fn post_pm_msg(
    shared: &Arc<super::SharedState>,
    hwnd: u32,
    msg: u32,
    mp1: u32,
    mp2: u32,
) {
    use super::mutex_ext::MutexExt as _;
    use super::pm_types::OS2Message;

    let wm = shared.window_mgr.lock_or_recover();
    let hmq = wm.find_hmq_for_hwnd(hwnd);
    if let Some(hmq) = hmq
        && let Some(mq_arc) = wm.get_mq(hmq) {
            // Release window_mgr before locking the inner MQ to respect lock order.
            drop(wm);
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(OS2Message { hwnd, msg, mp1, mp2, time: 0, x: 0, y: 0 });
            mq.cond.notify_one();
    }
}

/// Clamp a volume value to [0, 100].
fn clamp_volume(v: u32) -> u8 {
    v.min(100) as u8
}

/// Apply volume scaling to a PCM buffer using SDL_MixAudioFormat.
///
/// Returns a new `Vec<u8>` with scaled samples, or `None` if volume == 100
/// (caller should use the original buffer).
///
/// SAFETY: `src` must point to valid PCM data of `len` bytes in `format`.
unsafe fn apply_volume(
    src: *const u8,
    len: u32,
    format: u16,
    volume: u8,
) -> std::option::Option<Vec<u8>> {
    if volume >= 100 || len == 0 {
        return std::option::Option::None;
    }
    // SDL_MIX_MAXVOLUME = 128
    let sdl_vol = (volume as u32 * 128 / 100) as std::os::raw::c_int;
    let mut dst = vec![0u8; len as usize];
    unsafe {
        sdl2::sys::SDL_MixAudioFormat(
            dst.as_mut_ptr(),
            src,
            format as sdl2::sys::SDL_AudioFormat,
            len,
            sdl_vol,
        );
    }
    std::option::Option::Some(dst)
}

/// Convert a time position in milliseconds to a byte offset in the WAV data,
/// aligned to a frame boundary (frame = channels × bytes-per-sample).
fn ms_to_byte_offset(ms: u32, freq: i32, channels: u8, format: u16) -> u32 {
    let bits = (format as u32) & sdl2::sys::SDL_AUDIO_MASK_BITSIZE;
    let bytes_per_frame = ((bits / 8).max(1)) * channels as u32;
    let frames_per_ms = freq as u64 * ms as u64 / 1000;
    (frames_per_ms * bytes_per_frame as u64) as u32
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
            MCI_SEEK   => self.mci_seek(device_id, param1, param2),
            MCI_RECORD => self.mci_record(device_id, param1, param2),
            MCI_SET    => self.mci_set(device_id, param1, param2),
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
    ///   `play <alias> [wait] [notify]`
    ///   `stop <alias>`
    ///   `close <alias>`
    ///   `seek <alias> to <position_ms>`
    ///   `set <alias> audio volume to <level>`
    ///   `status <alias> mode`
    pub fn mci_send_string(
        &self,
        psz_cmd: u32,
        psz_ret: u32,
        cb_ret: u32,
        hwnd_callback: u32,
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
                let wait   = tokens.contains(&"wait");
                let notify = tokens.contains(&"notify");
                let mut flags = 0u32;
                if wait   { flags |= MCI_WAIT; }
                if notify { flags |= MCI_NOTIFY; }
                // Build a synthetic MCI_PLAY_PARMS on the host stack (no guest pointer needed
                // for the string path — pass hwnd_callback as a special param via notify_hwnd).
                self.mci_play_with_notify(dev_id as u32, flags, 0, hwnd_callback)
            }

            "stop" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                self.mci_stop(dev_id as u32)
            }

            "close" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                self.mci_close(dev_id as u32)
            }

            // seek <alias> to <position_ms>
            "seek" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                let pos_ms: u32 = tokens.get(3)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                self.mci_seek_to(dev_id as u32, pos_ms)
            }

            // set <alias> audio volume to <level>
            "set" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                // Locate "volume" keyword and read the value after "to"
                if let Some(vol_idx) = tokens.iter().position(|&t| t == "volume") {
                    let level: u32 = tokens.get(vol_idx + 2)
                        .or_else(|| tokens.get(vol_idx + 1))
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(100);
                    let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
                    if let Some(dev) = mgr.devices.get_mut(&dev_id) {
                        dev.volume = clamp_volume(level);
                        MCIERR_SUCCESS
                    } else {
                        MCIERR_INVALID_DEVICE_ID
                    }
                } else {
                    MCIERR_UNSUPPORTED_FUNCTION
                }
            }

            "status" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                let item_str = tokens.get(2).copied().unwrap_or("");
                let mode = self.shared.mmpm_mgr.lock().unwrap()
                    .devices.get(&dev_id).map(|d| d.mode);
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
            current_position: 0,
            volume: 100,
            notify_hwnd: 0,
            notify_cancel: std::option::Option::None,
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

    /// MCI_PLAY — dispatched from mci_send_command.
    ///
    /// Reads hwndCallback from MCI_PLAY_PARMS.hwndCallback at param2+0,
    /// and optional ulFrom at param2+4 when MCI_FROM flag is set.
    fn mci_play(&self, device_id: u32, param1: u32, param2: u32) -> u32 {
        // MCI_PLAY_PARMS: +0 hwndCallback, +4 ulFrom, +8 ulTo
        let hwnd_callback = if param2 != 0 {
            self.guest_read::<u32>(param2).unwrap_or(0)
        } else {
            0
        };
        // If MCI_FROM is set, seek to that position before playing.
        if param1 & MCI_FROM != 0 && param2 != 0 {
            let from_ms = self.guest_read::<u32>(param2 + 4).unwrap_or(0);
            let _ = self.mci_seek_to(device_id, from_ms);
        }
        self.mci_play_with_notify(device_id, param1, param2, hwnd_callback)
    }

    /// Core play implementation; `hwnd_callback` is extracted by the caller.
    fn mci_play_with_notify(
        &self,
        device_id: u32,
        param1: u32,
        _param2: u32,
        hwnd_callback: u32,
    ) -> u32 {
        use sdl2::sys as sys;
        use std::os::raw::{c_int, c_void};

        // Extract WAV data and playback parameters with lock held briefly, then drop.
        let (wav_buf, wav_len, freq, format, channels, start_pos, volume) = {
            let mgr = self.shared.mmpm_mgr.lock().unwrap();
            if let Some(dev) = mgr.devices.get(&(device_id as u16)) {
                if dev.wav_buf.is_null() {
                    return MCIERR_CANNOT_LOAD_DRIVER;
                }
                (
                    dev.wav_buf,
                    dev.wav_len,
                    dev.wav_freq,
                    dev.wav_format,
                    dev.wav_channels,
                    dev.current_position.min(dev.wav_len),
                    dev.volume,
                )
            } else {
                return MCIERR_INVALID_DEVICE_ID;
            }
        };

        // If there is already a notify watcher running (a prior non-blocking play),
        // cancel it with MCI_NOTIFY_SUPERSEDED before starting a new one.
        {
            let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
            if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
                if let Some(ref old_cancel) = dev.notify_cancel {
                    old_cancel.store(true, Ordering::Relaxed);
                }
                dev.notify_cancel = std::option::Option::None;
            }
        }

        let play_buf: *const u8;
        let play_len: u32;
        // Optionally volume-scaled copy.  Kept alive for the duration of SDL_QueueAudio.
        let _vol_buf: std::option::Option<Vec<u8>>;

        unsafe {
            // Apply seek offset.
            let raw_ptr = (wav_buf as *const u8).add(start_pos as usize);
            let raw_len = wav_len.saturating_sub(start_pos);

            // Apply volume.
            _vol_buf = apply_volume(raw_ptr, raw_len, format, volume);
            if let Some(ref vb) = _vol_buf {
                play_buf = vb.as_ptr();
                play_len = vb.len() as u32;
            } else {
                play_buf = raw_ptr;
                play_len = raw_len;
            }
        }

        if play_len == 0 {
            return MCIERR_SUCCESS; // nothing to play (seeked past end)
        }

        let audio_dev = unsafe {
            if sys::SDL_WasInit(sys::SDL_INIT_AUDIO) == 0
                && sys::SDL_InitSubSystem(sys::SDL_INIT_AUDIO) != 0 {
                    warn!("mciPlay: SDL_InitSubSystem(AUDIO) failed");
                    return MCIERR_CANNOT_LOAD_DRIVER;
            }

            let desired = sys::SDL_AudioSpec {
                freq,
                format:   format as sys::SDL_AudioFormat,
                channels,
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

            let needs_conv = obtained.freq != desired.freq
                || obtained.format != desired.format
                || obtained.channels != desired.channels;

            if needs_conv {
                if let Some(converted) = convert_audio(
                    play_buf as *mut sdl2::sys::Uint8, play_len,
                    desired.format, desired.channels, desired.freq,
                    obtained.format, obtained.channels, obtained.freq,
                ) {
                    let len = converted.len() as u32;
                    let ptr = Box::into_raw(converted.into_boxed_slice());
                    sys::SDL_QueueAudio(dev, (*ptr).as_ptr() as *const c_void, len);
                    // SDL_QueueAudio copies the data; safe to free.
                    drop(Box::from_raw(ptr));
                } else {
                    sys::SDL_QueueAudio(dev, play_buf as *const c_void, play_len);
                }
            } else {
                sys::SDL_QueueAudio(dev, play_buf as *const c_void, play_len);
            }

            sys::SDL_PauseAudioDevice(dev, 0);
            dev
        };

        // Update device state.
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
                dev.notify_hwnd = hwnd_callback;
            }
        }

        if param1 & MCI_WAIT != 0 {
            // Synchronous: wait until the queue drains.
            let bytes_per_sec = freq as u64 * channels as u64 * 2;
            let max_ms = if bytes_per_sec > 0 {
                play_len as u64 * 1000 / bytes_per_sec + 500
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
            // Notify after synchronous completion if MCI_NOTIFY also set.
            if param1 & MCI_NOTIFY != 0 && hwnd_callback != 0 {
                post_pm_msg(&self.shared, hwnd_callback,
                            MM_MCINOTIFY, MCI_NOTIFY_SUCCESSFUL, device_id);
            }
        } else if param1 & MCI_NOTIFY != 0 && hwnd_callback != 0 {
            // Asynchronous: spawn a watcher thread that posts MM_MCINOTIFY when done.
            let cancel = Arc::new(AtomicBool::new(false));
            let cancel_clone = Arc::clone(&cancel);
            {
                let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
                if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
                    dev.notify_cancel = std::option::Option::Some(cancel);
                }
            }

            let shared_clone = Arc::clone(&self.shared);
            let dev_id_u16 = device_id as u16;
            std::thread::spawn(move || {
                // Poll until playback finishes or is cancelled.
                let timeout = std::time::Instant::now()
                    + std::time::Duration::from_secs(3600);
                loop {
                    if cancel_clone.load(Ordering::Relaxed) {
                        post_pm_msg(&shared_clone, hwnd_callback,
                                    MM_MCINOTIFY, MCI_NOTIFY_SUPERSEDED, device_id);
                        return;
                    }
                    let queued = unsafe {
                        sdl2::sys::SDL_GetQueuedAudioSize(audio_dev)
                    };
                    if queued == 0 || std::time::Instant::now() > timeout {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }

                if !cancel_clone.load(Ordering::Relaxed) {
                    // Clean up device state.
                    {
                        let mut mgr = shared_clone.mmpm_mgr.lock().unwrap();
                        if let Some(dev) = mgr.devices.get_mut(&dev_id_u16) {
                            unsafe {
                                if dev.audio_dev != 0 {
                                    sdl2::sys::SDL_CloseAudioDevice(dev.audio_dev);
                                    dev.audio_dev = 0;
                                }
                            }
                            dev.mode = MciMode::Stopped;
                            dev.notify_cancel = std::option::Option::None;
                        }
                    }
                    post_pm_msg(&shared_clone, hwnd_callback,
                                MM_MCINOTIFY, MCI_NOTIFY_SUCCESSFUL, device_id);
                }
            });
        }

        debug!("mciPlay: device {} playing (pos={})", device_id, start_pos);
        MCIERR_SUCCESS
    }

    fn mci_stop(&self, device_id: u32) -> u32 {
        let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
        if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
            // Cancel any in-flight notify watcher.
            if let Some(ref cancel) = dev.notify_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            dev.notify_cancel = std::option::Option::None;
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

    /// MCI_SEEK — seek to a position in the audio stream.
    ///
    /// MCI_SEEK_PARMS (guest, param2):
    ///   +0  hwndCallback (u32)
    ///   +4  ulTo         (u32) — position in milliseconds (default time format)
    fn mci_seek(&self, device_id: u32, _param1: u32, param2: u32) -> u32 {
        let pos_ms = if param2 != 0 {
            self.guest_read::<u32>(param2 + 4).unwrap_or(0)
        } else {
            0
        };
        self.mci_seek_to(device_id, pos_ms)
    }

    /// Set `current_position` (in bytes) for the given device to `pos_ms` milliseconds.
    fn mci_seek_to(&self, device_id: u32, pos_ms: u32) -> u32 {
        let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
        if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
            let byte_offset = ms_to_byte_offset(pos_ms, dev.wav_freq, dev.wav_channels, dev.wav_format);
            dev.current_position = byte_offset.min(dev.wav_len);
            debug!("mciSeek: device {} → {}ms (byte {})", device_id, pos_ms, dev.current_position);
            MCIERR_SUCCESS
        } else {
            MCIERR_INVALID_DEVICE_ID
        }
    }

    /// MCI_RECORD — audio recording stub.
    ///
    /// SDL2 audio capture is not yet implemented; returns MCIERR_UNSUPPORTED_FUNCTION.
    fn mci_record(&self, device_id: u32, _param1: u32, _param2: u32) -> u32 {
        warn!("mciRecord: device {} — recording not implemented", device_id);
        MCIERR_UNSUPPORTED_FUNCTION
    }

    /// MCI_SET — configure device parameters.
    ///
    /// Currently handles `MCI_SET_AUDIO | MCI_SET_VOLUME` to set playback volume.
    ///
    /// MCI_SET_PARMS (guest, param2):
    ///   +0   hwndCallback (u32)
    ///   +4   ulTimeFormat  (u32)
    ///   +8   ulSpeedFormat (u32)
    ///   +12  fReplace      (u32)
    ///   +16  ulAudio       (u32) — channel: MCI_AUDIO_ALL=0, LEFT=1, RIGHT=2
    ///   +20  ulValue       (u32)
    ///   +24  ulLevel       (u32) — volume 0–100 when MCI_SET_VOLUME
    fn mci_set(&self, device_id: u32, param1: u32, param2: u32) -> u32 {
        debug!("  mciSet(devID={}, flags=0x{:08X})", device_id, param1);
        if param2 == 0 {
            return MCIERR_SUCCESS;
        }

        let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
        if !mgr.devices.contains_key(&(device_id as u16)) {
            return MCIERR_INVALID_DEVICE_ID;
        }

        if param1 & MCI_SET_AUDIO != 0 && param1 & MCI_SET_VOLUME != 0 {
            let _channel = self.guest_read::<u32>(param2 + 16).unwrap_or(MCI_AUDIO_ALL);
            let level    = self.guest_read::<u32>(param2 + 24).unwrap_or(100);
            if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
                dev.volume = clamp_volume(level);
                debug!("mciSet: device {} volume → {}%", device_id, dev.volume);
            }
        }
        // Other MCI_SET sub-functions (time format, speed) are accepted silently.
        MCIERR_SUCCESS
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
                MCI_STATUS_POSITION => {
                    // Convert current byte offset back to milliseconds.
                    let bits = (dev.wav_format as u32) & sdl2::sys::SDL_AUDIO_MASK_BITSIZE;
                    let bytes_per_frame = (bits / 8).max(1) * dev.wav_channels as u32;
                    let bytes_per_sec = dev.wav_freq as u32 * bytes_per_frame;
                    if bytes_per_sec > 0 {
                        dev.current_position / bytes_per_sec * 1000
                    } else {
                        0
                    }
                }
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
#[allow(clippy::too_many_arguments)]
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
        assert_ne!(MCI_SEEK, 0);
        assert_ne!(MCI_RECORD, 0);
        assert_ne!(MCI_SET, 0);
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
        // MciDevice with null ptrs must drop without panicking or calling SDL.
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
            current_position: 0,
            volume: 100,
            notify_hwnd: 0,
            notify_cancel: std::option::Option::None,
        };
        drop(dev);
    }

    #[test]
    fn test_mci_device_drop_with_cancel_flag() {
        // Dropping an MciDevice with an active cancel flag must set it to true.
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_probe = Arc::clone(&cancel);
        let dev = MciDevice {
            device_type: "waveaudio".to_string(),
            element_name: String::new(),
            mode: MciMode::Playing,
            audio_dev: 0,
            wav_buf: std::ptr::null_mut(),
            wav_len: 0,
            wav_freq: 44100,
            wav_format: sdl2::sys::AUDIO_S16LSB as u16,
            wav_channels: 2,
            current_position: 0,
            volume: 100,
            notify_hwnd: 1,
            notify_cancel: std::option::Option::Some(cancel),
        };
        assert!(!cancel_probe.load(Ordering::Relaxed));
        drop(dev);
        assert!(cancel_probe.load(Ordering::Relaxed), "cancel flag must be set on drop");
    }

    #[test]
    fn test_beep_tone_zero_inputs() {
        // Must return immediately without crashing.
        beep_tone(0, 1000);
        beep_tone(440, 0);
        beep_tone(0, 0);
    }

    #[test]
    fn test_mm_mcinotify_value() {
        // WM_MMPMBASE = 0x0500, MM_MCINOTIFY = WM_MMPMBASE + 2.
        assert_eq!(MM_MCINOTIFY, 0x0502);
    }

    #[test]
    fn test_mci_notify_codes_are_distinct_bit_flags() {
        let codes = [
            MCI_NOTIFY_SUCCESSFUL,
            MCI_NOTIFY_SUPERSEDED,
            MCI_NOTIFY_ABORTED,
            MCI_NOTIFY_ERROR,
        ];
        // All non-zero.
        for &c in &codes { assert_ne!(c, 0); }
        // All distinct.
        let mut seen = std::collections::HashSet::new();
        for &c in &codes { assert!(seen.insert(c)); }
        // All single bits (power-of-two).
        for &c in &codes { assert_eq!(c & (c - 1), 0, "0x{:04X} is not a power of two", c); }
    }

    #[test]
    fn test_clamp_volume() {
        assert_eq!(clamp_volume(0),   0);
        assert_eq!(clamp_volume(50),  50);
        assert_eq!(clamp_volume(100), 100);
        assert_eq!(clamp_volume(200), 100); // clamped
        assert_eq!(clamp_volume(u32::MAX), 100);
    }

    #[test]
    fn test_ms_to_byte_offset_basic() {
        // 44100 Hz, 2 ch, 16-bit → 176400 bytes/sec → 1764 bytes/10ms
        let fmt = sdl2::sys::AUDIO_S16LSB as u16;
        let offset = ms_to_byte_offset(10, 44100, 2, fmt);
        assert_eq!(offset, 1764);
    }

    #[test]
    fn test_ms_to_byte_offset_zero() {
        let fmt = sdl2::sys::AUDIO_S16LSB as u16;
        assert_eq!(ms_to_byte_offset(0, 44100, 2, fmt), 0);
    }

    #[test]
    fn test_mci_notify_flag_value() {
        // MCI_NOTIFY must be bit 0, MCI_WAIT must be bit 1.
        assert_eq!(MCI_NOTIFY, 0x0001);
        assert_eq!(MCI_WAIT,   0x0002);
    }

    #[test]
    fn test_mci_set_audio_volume_flags() {
        // Flags must be non-zero and non-overlapping.
        assert_ne!(MCI_SET_AUDIO, 0);
        assert_ne!(MCI_SET_VOLUME, 0);
        assert_eq!(MCI_SET_AUDIO & MCI_SET_VOLUME, 0);
    }
}
