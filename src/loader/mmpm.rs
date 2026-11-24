// SPDX-License-Identifier: GPL-3.0-or-later
//
// MMPM/2 (MultiMedia Presentation Manager/2) emulation.
//
// Implements MDM.DLL ordinals:
//   1 = mciSendCommand(usDeviceID, usMessage, ulParam1, ulParam2)
//   2 = mciSendString(pszCommandBuf, pszReturnString, cbReturnString, hwndCallback)
//   3 = mciGetErrorString(ulError, pszBuffer, usLength)
// (the ordinals of Open Watcom's mmpm2.lib; MDM has no ordinal 4 there)
//
// Audio output goes through `SharedState::audio` (`gui::audio`): PCM data is
// handed to the main-thread loop, which plays it with SDL3 audio streams;
// this module never calls SDL. WAV files are parsed here (`parse_wav`).
// DosBeep tone synthesis also lives here.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use log::{debug, warn};

use crate::gui::audio::{AudioService, PcmSpec, SampleFormat, WaitEnd};

// Values below are from the OS/2 Toolkit headers <mcios2.h> / <meerror.h>.

// ── MCI message IDs ───────────────────────────────────────────────────────────

pub const MCI_OPEN:   u16 = 1;
pub const MCI_CLOSE:  u16 = 2;
pub const MCI_PLAY:   u16 = 4;
pub const MCI_SEEK:   u16 = 5;
pub const MCI_STOP:   u16 = 6;
pub const MCI_STATUS: u16 = 10;
pub const MCI_SET:    u16 = 12;
pub const MCI_RECORD: u16 = 14;

// ── MCI ulParam1 flags (common) ───────────────────────────────────────────────

/// Post MM_MCINOTIFY to hwndCallback when the command completes (non-blocking).
pub const MCI_NOTIFY: u32 = 0x0000_0001;
/// Block until command completes.
const MCI_WAIT: u32 = 0x0000_0002;
/// ulFrom field in MCI_PLAY_PARMS is valid — start playback from this position.
const MCI_FROM: u32 = 0x0000_0004;

// MCI_OPEN ulParam1 flags. pszDeviceType is always used: a string, or with
// MCI_OPEN_TYPE_ID a device type in its low word.
const MCI_OPEN_ELEMENT: u32 = 0x0000_0100;
const MCI_OPEN_ALIAS:   u32 = 0x0000_0200;
const MCI_OPEN_TYPE_ID: u32 = 0x0000_1000;
/// Device type of waveform audio (`MCI_DEVTYPE_WAVEFORM_AUDIO`).
const MCI_DEVTYPE_WAVEFORM_AUDIO: u32 = 7;

// MCI_SET ulParam1 flags
/// `ulAudio` (channel) in MCI_SET_PARMS is valid.
pub const MCI_SET_AUDIO:  u32 = 0x0000_1000;
/// `ulLevel` (volume 0–100) in MCI_SET_PARMS is valid.
pub const MCI_SET_VOLUME: u32 = 0x0008_0000;

// MCI_SET_PARMS.ulAudio channels
const MCI_SET_AUDIO_ALL:   u32 = 0;
#[allow(dead_code)]
const MCI_SET_AUDIO_LEFT:  u32 = 1;
#[allow(dead_code)]
const MCI_SET_AUDIO_RIGHT: u32 = 2;

// MCI_SET_PARMS offsets: hwndCallback, ulTimeFormat, ulSpeedFormat, ulAudio, ulLevel…
const SET_PARMS_AUDIO: u32 = 12;
const SET_PARMS_LEVEL: u32 = 16;
// MCI_OPEN_PARMS offsets: hwndCallback, usDeviceID, usReserved0, pszDeviceType,
// pszElementName, pszAlias.
const OPEN_PARMS_DEVICE_ID: u32 = 4;
const OPEN_PARMS_TYPE:      u32 = 8;
const OPEN_PARMS_ELEMENT:   u32 = 12;
const OPEN_PARMS_ALIAS:     u32 = 16;

// MCI_STATUS item codes (MCI_STATUS_PARMS.ulItem)
const MCI_STATUS_LENGTH:   u32 = 2;
const MCI_STATUS_MODE:     u32 = 3;
const MCI_STATUS_POSITION: u32 = 5;

// MCI_STATUS_MODE values
const MCI_MODE_NOT_READY: u32 = 1;
const MCI_MODE_PLAY:      u32 = 3;
const MCI_MODE_STOP:      u32 = 4;

// ── MMPM/2 window messages ────────────────────────────────────────────────────

/// MM_MCINOTIFY — posted to hwndCallback when an MCI command with MCI_NOTIFY
/// completes: mp1 = MPFROM2SHORT(notify code, the command's usUserParm),
/// mp2 = MPFROM2SHORT(device ID, the MCI message that completed).
pub const MM_MCINOTIFY: u32 = 0x0500;

/// `MM_MCINOTIFY`'s mp1: the notify code and the user parameter.
pub fn notify_mp1(code: u32, user_parm: u16) -> u32 {
    (code & 0xFFFF) | (user_parm as u32) << 16
}

/// `MM_MCINOTIFY`'s mp2: the device and the message that completed.
pub fn notify_mp2(device_id: u32, message: u16) -> u32 {
    (device_id & 0xFFFF) | (message as u32) << 16
}

/// Notify codes for MM_MCINOTIFY.
pub const MCI_NOTIFY_SUCCESSFUL: u32 = 0;
pub const MCI_NOTIFY_SUPERSEDED: u32 = 1;
pub const MCI_NOTIFY_ABORTED:    u32 = 2;
pub const MCI_NOTIFY_ERROR:      u32 = 3;

// ── MCI error codes (<meerror.h>) ─────────────────────────────────────────────

pub const MCIERR_SUCCESS:              u32 = 0;
const MCIERR_BASE:                     u32 = 5000;
pub const MCIERR_INVALID_DEVICE_ID:    u32 = MCIERR_BASE + 1;
pub const MCIERR_INVALID_DEVICE_NAME:  u32 = MCIERR_BASE + 7;
pub const MCIERR_CANNOT_LOAD_DRIVER:   u32 = MCIERR_BASE + 10;
pub const MCIERR_UNSUPPORTED_FUNCTION: u32 = MCIERR_BASE + 18;
pub const MCIERR_FILE_NOT_FOUND:       u32 = MCIERR_BASE + 19;
pub const MCIERR_OUTOFRANGE:           u32 = MCIERR_BASE + 26;
pub const MCIERR_INVALID_BUFFER:       u32 = MCIERR_BASE + 50;
pub const MCIERR_INVALID_FLAG:         u32 = MCIERR_BASE + 54;

/// The longest text `mciGetErrorString` returns, its NUL included (IBM's
/// Multimedia Programming Reference: "The maximum string length returned
/// is 128 bytes").
pub const MCI_ERROR_STRING_MAX: usize = 128;
/// `mciGetErrorString`'s text for `MCIERR_SUCCESS` (Warpine's wording).
const MCI_NO_ERROR_TEXT: &str = "No error";

/// The text of an MCI error code (the low word of `mciGetErrorString`'s
/// `ulError`), or `None` for a code `<meerror.h>` does not define.
pub fn mci_error_text(code: u32) -> Option<&'static str> {
    if code == MCIERR_SUCCESS {
        return Some(MCI_NO_ERROR_TEXT);
    }
    let n = code.checked_sub(MCIERR_BASE)?;
    super::mmpm_errtext::MCI_ERROR_TEXTS.iter().find(|(c, _)| *c == n).map(|(_, t)| *t)
}

// ── MCI device state ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MciMode { NotReady, Stopped, Playing }

pub struct MciDevice {
    /// Device type string, e.g. "waveaudio"
    pub device_type: String,
    /// Element name (filename in OS/2 path form)
    pub element_name: String,
    /// Alias given by `open … alias <name>` (empty if none).
    pub alias: String,
    /// Current playback mode
    pub mode: MciMode,
    /// The voice playing this device's audio (`AudioService`), if any.
    pub voice: Option<u32>,
    /// Decoded PCM data of the element (empty if none was loaded).
    pub pcm: Vec<u8>,
    /// Format of `pcm`.
    pub spec: PcmSpec,
    /// Playback start position in bytes (set by MCI_SEEK or MCI_FROM flag).
    /// The next MCI_PLAY queues audio starting from this byte offset.
    pub current_position: u32,
    /// Volume level 0–100 (default 100), the playing voice's gain.
    pub volume: u8,
    /// Window handle to receive MM_MCINOTIFY when MCI_NOTIFY play completes.
    pub notify_hwnd: u32,
    /// Set to `true` by mci_stop() to abort an in-flight notify watcher thread.
    pub notify_cancel: Option<Arc<AtomicBool>>,
}

impl Drop for MciDevice {
    fn drop(&mut self) {
        // Cancel any in-flight notify watcher. (Closing the device stops its
        // voice first: `mci_close`.)
        if let Some(ref cancel) = self.notify_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The default format of a device opened without an element.
const DEFAULT_SPEC: PcmSpec = PcmSpec { freq: 44100, channels: 2, format: SampleFormat::S16 };

/// MCI device type names a string command can open directly (`open
/// waveaudio alias w`), optionally with a two-digit device number
/// (`waveaudio01`). Returns the type without the number.
const MCI_DEVICE_TYPE_NAMES: &[&str] = &[
    "waveaudio", "sequencer", "cdaudio", "cdxa", "videodisc", "digitalvideo",
    "ampmix", "speaker", "headphone", "videotape", "overlay", "animation",
];

fn mci_device_name(word: &str) -> Option<&'static str> {
    MCI_DEVICE_TYPE_NAMES.iter().copied().find(|&t| {
        word.strip_prefix(t).is_some_and(|rest| rest.is_empty() || (rest.len() == 2 && rest.bytes().all(|b| b.is_ascii_digit())))
    })
}

/// The gain for an MCI volume level (0–100).
fn gain(volume: u8) -> f32 {
    volume as f32 / 100.0
}

/// Whether a device is playing: it has a voice that has not finished.
fn device_mode(dev: &MciDevice, audio: &AudioService) -> MciMode {
    match dev.voice {
        Some(v) if audio.is_playing(v) => MciMode::Playing,
        _ if dev.mode == MciMode::NotReady => MciMode::NotReady,
        _ => MciMode::Stopped,
    }
}

// ── MmpmManager ───────────────────────────────────────────────────────────────

pub struct MmpmManager {
    pub devices: HashMap<u16, MciDevice>,
    pub next_device_id: u16,
    /// The system values of `mciSetSysValue` / `mciQuerySysValue`.
    pub sys: MciSysValues,
}

/// The MCI system values (`MSV_*`, <mcios2.h>), per process: Warpine keeps
/// what a program sets and reports it; playback does not act on them yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MciSysValues {
    pub closed_caption: bool,
    /// 0 to 100.
    pub master_volume: u32,
    pub headphones: bool,
    pub speakers: bool,
    /// Where temporary multimedia files go.
    pub work_path: Vec<u8>,
    pub qos_value: u32,
    pub qos_error_flag: u32,
}

impl Default for MciSysValues {
    fn default() -> Self {
        MciSysValues {
            closed_caption: false, master_volume: MAX_MASTER_VOLUME, headphones: false, speakers: true,
            work_path: DEFAULT_WORK_PATH.to_vec(), qos_value: 0, qos_error_flag: 0,
        }
    }
}

/// `MSV_*` system values (<mcios2.h>).
pub const MSV_CLOSEDCAPTION: u32 = 0;
pub const MSV_MASTERVOLUME: u32 = 1;
pub const MSV_HEADPHONES: u32 = 2;
pub const MSV_SPEAKERS: u32 = 3;
pub const MSV_WORKPATH: u32 = 4;
pub const MSV_SYSQOSVALUE: u32 = 5;
pub const MSV_SYSQOSERRORFLAG: u32 = 6;
const MAX_MASTER_VOLUME: u32 = 100;
/// `CCHMAXPATH`: the room `MSV_WORKPATH`'s buffer has, its NUL included.
const CCHMAXPATH: usize = 260;
/// The work path before a program sets one (Warpine's choice: drive C:'s
/// root, which every session has).
const DEFAULT_WORK_PATH: &[u8] = b"C:\\";

impl Default for MmpmManager {
    fn default() -> Self { Self::new() }
}

impl MmpmManager {
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
            next_device_id: 1,
            sys: MciSysValues::default(),
        }
    }
}

// ── DosBeep tone synthesis ────────────────────────────────────────────────────

/// Sample rate of DosBeep tones.
const BEEP_RATE: u32 = 44100;
/// DosBeep amplitude (fraction of full scale).
const BEEP_AMPLITUDE: f32 = 0.30;
/// Extra time allowed for MCI playback to drain after its nominal length.
const PLAY_SLACK_MS: u64 = 500;
/// Extra time allowed for a tone to drain after its nominal duration.
const BEEP_SLACK_MS: u64 = 500;

/// A `duration_ms` sine tone at `freq_hz` as mono 16-bit PCM.
fn beep_pcm(freq_hz: u32, duration_ms: u32) -> Vec<u8> {
    let frames = (BEEP_RATE as u64 * duration_ms as u64 / 1000) as usize;
    let amp = i16::MAX as f32 * BEEP_AMPLITUDE;
    (0..frames).flat_map(|i| {
        let t = i as f32 / BEEP_RATE as f32;
        ((amp * (2.0 * std::f32::consts::PI * freq_hz as f32 * t).sin()) as i16).to_le_bytes()
    }).collect()
}

/// How often a wait for playback looks whether its call should stop.
const STOP_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// Wait for `voice` to finish, at most `max`, looking every
/// [`STOP_POLL`] whether the call should stop (`stop`: a signal, a kill,
/// the process end; then `WaitEnd::Cancelled`).
pub fn wait_or_stop(audio: &AudioService, voice: u32, max: std::time::Duration, stop: &dyn Fn() -> bool) -> WaitEnd {
    let deadline = std::time::Instant::now() + max;
    loop {
        let now = std::time::Instant::now();
        if now >= deadline { return WaitEnd::TimedOut; }
        match audio.wait(voice, None, (deadline - now).min(STOP_POLL)) {
            WaitEnd::TimedOut if stop() => return WaitEnd::Cancelled,
            WaitEnd::TimedOut => {}
            end => return end,
        }
    }
}

/// Play a sine-wave tone and wait until it has finished (DosBeep), or
/// until `stop` says the call should stop (the tone is cut short then).
/// Silent, and immediate, when no audio backend is attached.
pub fn beep_tone(audio: &AudioService, freq_hz: u32, duration_ms: u32, stop: &dyn Fn() -> bool) {
    if freq_hz == 0 || duration_ms == 0 {
        return;
    }
    let spec = PcmSpec { freq: BEEP_RATE, channels: 1, format: SampleFormat::S16 };
    if let Some(voice) = audio.play(spec, beep_pcm(freq_hz, duration_ms), 1.0) {
        let limit = std::time::Duration::from_millis(duration_ms as u64 + BEEP_SLACK_MS);
        if matches!(wait_or_stop(audio, voice, limit, stop), WaitEnd::TimedOut | WaitEnd::Cancelled) {
            audio.stop(voice);
        }
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

/// Convert a time position in milliseconds to a byte offset in the WAV data,
/// aligned to a frame boundary (frame = channels × bytes-per-sample).
fn ms_to_byte_offset(ms: u32, spec: PcmSpec) -> u32 {
    let frames = spec.freq as u64 * ms as u64 / 1000;
    (frames * spec.bytes_per_frame() as u64).min(u32::MAX as u64) as u32
}

/// The inverse: a byte offset in milliseconds.
fn byte_offset_to_ms(bytes: u32, spec: PcmSpec) -> u32 {
    let bps = spec.bytes_per_sec() as u64;
    (bytes as u64 * 1000).checked_div(bps).map_or(0, |ms| ms.min(u32::MAX as u64) as u32)
}

// ── WAV files (parsed in wav.rs, fuzzed) ──────────────────────────────────────

pub use super::wav::parse_wav;

// ── impl Loader ───────────────────────────────────────────────────────────────

impl super::Loader {
    /// MDM ordinal 1: `mciSendCommand(usDeviceID, usMessage, ulParam1,
    /// pParam2, usUserParm)`
    ///
    /// `pParam2` is a guest pointer to the message-specific parameter
    /// block; `usUserParm` comes back in `MM_MCINOTIFY`.
    pub fn mci_send_command(
        &self,
        device_id: u32,
        message: u32,
        param1: u32,
        param2: u32,
        user_parm: u32,
    ) -> u32 {
        let msg = message as u16;
        debug!(
            "  mciSendCommand(devID={}, msg=0x{:04X}, p1=0x{:08X}, p2=0x{:08X})",
            device_id, message, param1, param2
        );

        match msg {
            MCI_OPEN   => self.mci_open(param1, param2),
            MCI_CLOSE  => self.mci_close(device_id),
            MCI_PLAY   => self.mci_play(device_id, param1, param2, user_parm as u16),
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
        user_parm: u32,
    ) -> u32 {
        // cbReturnString is a USHORT: only the low half of its stack slot
        // is defined.
        let cb_ret = cb_ret & 0xFFFF;
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
                let mut alias = String::new();
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
                        "alias" => {
                            alias = tokens.get(i + 1).copied().unwrap_or("").to_string();
                            i += 2;
                        }
                        // Flags without an argument.
                        "wait" | "notify" | "shareable" => i += 1,
                        // `open waveaudio …` names a device, not a file.
                        word if i == 1 && mci_device_name(word).is_some() => {
                            device_type = mci_device_name(word).unwrap_or_default().to_string();
                            i += 1;
                        }
                        _ => {
                            if element_name.is_empty() {
                                element_name = tokens[i].to_string();
                            }
                            i += 1;
                        }
                    }
                }
                let rc = self.mci_open_internal(&device_type, &element_name, &alias);
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
                self.mci_play_with_notify(dev_id as u32, flags, user_parm as u16, hwnd_callback)
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
                    self.mci_set_volume(dev_id as u32, level)
                } else {
                    MCIERR_UNSUPPORTED_FUNCTION
                }
            }

            "status" => {
                let dev_id = self.mci_resolve_alias(tokens.get(1).copied().unwrap_or(""));
                let item_str = tokens.get(2).copied().unwrap_or("");
                let mode = self.shared.mmpm_mgr.lock().unwrap()
                    .devices.get(&dev_id).map(|d| device_mode(d, &self.shared.audio));
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

    /// MDM ordinal 3: `mciGetErrorString(ulError, pszBuffer, usLength)`.
    ///
    /// The low word of `ulError` is the error code, the high word a device
    /// ID (0 for none; an ID that is not open is `MCIERR_INVALID_DEVICE_ID`).
    /// The text and its NUL go to the buffer, at most `usLength` bytes of
    /// them: a short buffer gets the text cut, without a NUL, as IBM's
    /// Reference describes. Warpine has no device-dependent texts.
    pub fn mci_get_error_string(&self, ul_error: u32, psz_buffer: u32, us_length: u32) -> u32 {
        let code = ul_error & 0xFFFF;
        let device = (ul_error >> 16) as u16;
        debug!("  mciGetErrorString({code}, device {device})");
        if device != 0 && !self.shared.mmpm_mgr.lock().unwrap().devices.contains_key(&device) {
            return MCIERR_INVALID_DEVICE_ID;
        }
        let Some(text) = mci_error_text(code) else { return MCIERR_OUTOFRANGE };
        if psz_buffer == 0 {
            return MCIERR_INVALID_BUFFER;
        }
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        bytes.truncate((us_length & 0xFFFF) as usize);
        match self.guest_write_bytes(psz_buffer, &bytes) {
            Some(()) => MCIERR_SUCCESS,
            None => MCIERR_INVALID_BUFFER,
        }
    }

    /// MDM ordinal 16: `mciGetDeviceID(pszName)`: the device opened under
    /// the alias `pszName` (case does not matter), 0 when there is none.
    pub fn mci_get_device_id(&self, psz_name: u32) -> u32 {
        let Some(name) = self.mci_guest_string(psz_name) else { return 0 };
        let mgr = self.shared.mmpm_mgr.lock().unwrap();
        mgr.devices.iter().find(|(_, d)| !d.alias.is_empty() && d.alias.eq_ignore_ascii_case(&name))
            .map_or(0, |(&id, _)| u32::from(id))
    }

    /// MDM ordinal 11: `mciQuerySysValue(iSysValue, pValue)`: the value to
    /// `*pValue` (a ULONG, or for `MSV_WORKPATH` a `CCHMAXPATH` buffer);
    /// `MCIERR_SUCCESS`, or `MCIERR_INVALID_FLAG` for an unknown value or
    /// a bad pointer.
    pub fn mci_query_sys_value(&self, which: u32, p_value: u32) -> u32 {
        let sys = self.shared.mmpm_mgr.lock().unwrap().sys.clone();
        let written = match which & 0xFFFF {
            MSV_WORKPATH => {
                let mut path = sys.work_path.clone();
                path.truncate(CCHMAXPATH - 1);
                path.push(0);
                self.guest_write_bytes(p_value, &path)
            }
            v => {
                let value = match v {
                    MSV_CLOSEDCAPTION => u32::from(sys.closed_caption),
                    MSV_MASTERVOLUME => sys.master_volume,
                    MSV_HEADPHONES => u32::from(sys.headphones),
                    MSV_SPEAKERS => u32::from(sys.speakers),
                    MSV_SYSQOSVALUE => sys.qos_value,
                    MSV_SYSQOSERRORFLAG => sys.qos_error_flag,
                    _ => return MCIERR_INVALID_FLAG,
                };
                self.guest_write::<u32>(p_value, value)
            }
        };
        if written.is_some() { MCIERR_SUCCESS } else { MCIERR_INVALID_FLAG }
    }

    /// MDM ordinal 10: `mciSetSysValue(iSysValue, pValue)`: set a value from
    /// `*pValue` (a ULONG, or for `MSV_WORKPATH` a string); TRUE, or FALSE
    /// for an unknown value, a bad pointer, a volume above 100 or a path that
    /// is empty or does not fit `CCHMAXPATH`.
    pub fn mci_set_sys_value(&self, which: u32, p_value: u32) -> u32 {
        const TRUE: u32 = 1;
        const FALSE: u32 = 0;
        if which & 0xFFFF == MSV_WORKPATH {
            let Some(path) = self.mci_guest_bytes(p_value, CCHMAXPATH) else { return FALSE };
            if path.is_empty() { return FALSE; }
            self.shared.mmpm_mgr.lock().unwrap().sys.work_path = path;
            return TRUE;
        }
        let Some(value) = self.guest_read::<u32>(p_value) else { return FALSE };
        let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
        let sys = &mut mgr.sys;
        match which & 0xFFFF {
            MSV_CLOSEDCAPTION => sys.closed_caption = value != 0,
            MSV_MASTERVOLUME if value <= MAX_MASTER_VOLUME => sys.master_volume = value,
            MSV_HEADPHONES => sys.headphones = value != 0,
            MSV_SPEAKERS => sys.speakers = value != 0,
            MSV_SYSQOSVALUE => sys.qos_value = value,
            MSV_SYSQOSERRORFLAG => sys.qos_error_flag = value,
            _ => return FALSE,
        }
        TRUE
    }

    /// The bytes of the NUL-terminated guest string at `p`, if it ends
    /// within `limit` bytes.
    fn mci_guest_bytes(&self, p: u32, limit: usize) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for i in 0..limit as u32 {
            match self.guest_read::<u8>(p.checked_add(i)?)? { 0 => return Some(out), b => out.push(b) }
        }
        None
    }

    fn mci_guest_string(&self, p: u32) -> Option<String> {
        self.mci_guest_bytes(p, CCHMAXPATH).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    // ── Internal MCI helpers ──────────────────────────────────────────────────

    fn mci_open(&self, param1: u32, param2: u32) -> u32 {
        if param2 == 0 {
            return MCIERR_CANNOT_LOAD_DRIVER;
        }
        let psz_type = self.guest_read::<u32>(param2 + OPEN_PARMS_TYPE).unwrap_or(0);
        let psz_elem = if param1 & MCI_OPEN_ELEMENT != 0 {
            self.guest_read::<u32>(param2 + OPEN_PARMS_ELEMENT).unwrap_or(0)
        } else {
            0
        };
        let psz_alias = if param1 & MCI_OPEN_ALIAS != 0 {
            self.guest_read::<u32>(param2 + OPEN_PARMS_ALIAS).unwrap_or(0)
        } else {
            0
        };

        let device_type = if param1 & MCI_OPEN_TYPE_ID != 0 {
            match psz_type & 0xFFFF {
                MCI_DEVTYPE_WAVEFORM_AUDIO => "waveaudio".to_string(),
                other => format!("device type {other}"),
            }
        } else if psz_type != 0 {
            self.read_guest_string(psz_type).to_ascii_lowercase()
        } else {
            "waveaudio".to_string()
        };
        let alias = if psz_alias != 0 { self.read_guest_string(psz_alias) } else { String::new() };
        let element_name = if psz_elem != 0 {
            self.read_guest_string(psz_elem)
        } else {
            String::new()
        };

        let rc = self.mci_open_internal(&device_type, &element_name, &alias);
        if rc == MCIERR_SUCCESS {
            let id = self.shared.mmpm_mgr.lock().unwrap().next_device_id - 1;
            self.guest_write::<u16>(param2 + OPEN_PARMS_DEVICE_ID, id);
        }
        rc
    }

    fn mci_open_internal(&self, device_type: &str, element_name: &str, alias: &str) -> u32 {
        if device_type != "waveaudio" {
            warn!("mciOpen: unsupported device type '{}'", device_type);
            return MCIERR_INVALID_DEVICE_NAME;
        }

        let (spec, pcm) = if !element_name.is_empty() {
            match self.load_wav_via_vfs(element_name) {
                Ok(data) => data,
                Err(e) => {
                    warn!("mciOpen: failed to load WAV '{}': {}", element_name, e);
                    return MCIERR_FILE_NOT_FOUND;
                }
            }
        } else {
            // Empty waveaudio device — no WAV data
            (DEFAULT_SPEC, Vec::new())
        };

        let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
        let id = mgr.next_device_id;
        mgr.next_device_id = mgr.next_device_id.wrapping_add(1).max(1);
        mgr.devices.insert(id, MciDevice {
            device_type: device_type.to_string(),
            element_name: element_name.to_string(),
            alias: alias.to_string(),
            mode: MciMode::Stopped,
            voice: None,
            pcm,
            spec,
            current_position: 0,
            volume: 100,
            notify_hwnd: 0,
            notify_cancel: std::option::Option::None,
        });
        debug!("mciOpen: opened device ID {} for '{}'", id, element_name);
        MCIERR_SUCCESS
    }

    fn mci_close(&self, device_id: u32) -> u32 {
        let removed = self.shared.mmpm_mgr.lock().unwrap().devices.remove(&(device_id as u16));
        if let Some(dev) = removed {
            if let Some(v) = dev.voice { self.shared.audio.stop(v); }
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
    fn mci_play(&self, device_id: u32, param1: u32, param2: u32, user_parm: u16) -> u32 {
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
        self.mci_play_with_notify(device_id, param1, user_parm, hwnd_callback)
    }

    /// Core play implementation; `hwnd_callback` is extracted by the caller.
    ///
    /// The PCM from the current position is handed to the audio service; with
    /// `MCI_WAIT` the call blocks until it has played, with `MCI_NOTIFY` a
    /// watcher thread posts `MM_MCINOTIFY` when it finishes (or
    /// `MCI_NOTIFY_SUPERSEDED` when stopped / replaced first).
    fn mci_play_with_notify(
        &self,
        device_id: u32,
        param1: u32,
        user_parm: u16,
        hwnd_callback: u32,
    ) -> u32 {
        let audio = &self.shared.audio;
        // Take what to play, and replace any earlier playback, under the lock.
        let (pcm, spec, volume, old_voice) = {
            let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
            let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) else {
                return MCIERR_INVALID_DEVICE_ID;
            };
            if dev.pcm.is_empty() {
                return MCIERR_CANNOT_LOAD_DRIVER;
            }
            if let Some(ref old_cancel) = dev.notify_cancel {
                old_cancel.store(true, Ordering::Relaxed);
            }
            dev.notify_cancel = None;
            let start = (dev.current_position as usize).min(dev.pcm.len());
            (dev.pcm[start..].to_vec(), dev.spec, dev.volume, dev.voice.take())
        };
        if let Some(v) = old_voice { audio.stop(v); }
        if pcm.is_empty() {
            return MCIERR_SUCCESS; // nothing to play (seeked past end)
        }
        let len = pcm.len() as u64;
        let Some(voice) = audio.play(spec, pcm, gain(volume)) else {
            warn!("mciPlay: no audio output");
            return MCIERR_CANNOT_LOAD_DRIVER;
        };
        {
            let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
            if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
                dev.voice = Some(voice);
                dev.mode = MciMode::Playing;
                dev.notify_hwnd = hwnd_callback;
            }
        }
        // The longest the voice can take, plus slack.
        let max_wait = std::time::Duration::from_millis(
            (len * 1000).checked_div(spec.bytes_per_sec() as u64).unwrap_or(0) + PLAY_SLACK_MS);

        if param1 & MCI_WAIT != 0 {
            match wait_or_stop(audio, voice, max_wait, &|| self.interrupted() || self.shutting_down()) {
                WaitEnd::TimedOut => audio.stop(voice),
                // A signal or a kill ends the wait; the sound plays on, as
                // without MCI_WAIT (Warpine's choice).
                WaitEnd::Cancelled => return MCIERR_SUCCESS,
                _ => {}
            }
            self.mci_stop(device_id);
            if param1 & MCI_NOTIFY != 0 && hwnd_callback != 0 {
                post_pm_msg(&self.shared, hwnd_callback, MM_MCINOTIFY,
                            notify_mp1(MCI_NOTIFY_SUCCESSFUL, user_parm), notify_mp2(device_id, MCI_PLAY));
            }
        } else if param1 & MCI_NOTIFY != 0 && hwnd_callback != 0 {
            let cancel = Arc::new(AtomicBool::new(false));
            let cancel_clone = Arc::clone(&cancel);
            {
                let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
                if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
                    dev.notify_cancel = Some(cancel);
                }
            }
            let shared = Arc::clone(&self.shared);
            std::thread::spawn(move || {
                let code = match shared.audio.wait(voice, Some(&cancel_clone), max_wait) {
                    WaitEnd::Cancelled => MCI_NOTIFY_SUPERSEDED,
                    WaitEnd::Finished | WaitEnd::TimedOut => {
                        shared.audio.stop(voice);
                        let mut mgr = shared.mmpm_mgr.lock().unwrap();
                        if let Some(dev) = mgr.devices.get_mut(&(device_id as u16))
                            && dev.voice == Some(voice) {
                            dev.voice = None;
                            dev.mode = MciMode::Stopped;
                            dev.notify_cancel = None;
                        }
                        MCI_NOTIFY_SUCCESSFUL
                    }
                };
                post_pm_msg(&shared, hwnd_callback, MM_MCINOTIFY, notify_mp1(code, user_parm), notify_mp2(device_id, MCI_PLAY));
            });
        }

        debug!("mciPlay: device {} playing voice {}", device_id, voice);
        MCIERR_SUCCESS
    }

    fn mci_stop(&self, device_id: u32) -> u32 {
        let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
        if let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) {
            // Cancel any in-flight notify watcher.
            if let Some(ref cancel) = dev.notify_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            dev.notify_cancel = None;
            if let Some(v) = dev.voice.take() {
                self.shared.audio.stop(v);
            }
            dev.mode = MciMode::Stopped;
            debug!("mciStop: device {} stopped", device_id);
            MCIERR_SUCCESS
        } else {
            MCIERR_INVALID_DEVICE_ID
        }
    }

    /// Set the volume (0–100) of a device, and of its playing voice.
    fn mci_set_volume(&self, device_id: u32, level: u32) -> u32 {
        let mut mgr = self.shared.mmpm_mgr.lock().unwrap();
        let Some(dev) = mgr.devices.get_mut(&(device_id as u16)) else {
            return MCIERR_INVALID_DEVICE_ID;
        };
        dev.volume = clamp_volume(level);
        if let Some(v) = dev.voice {
            self.shared.audio.set_gain(v, gain(dev.volume));
        }
        debug!("mciSet: device {} volume → {}%", device_id, dev.volume);
        MCIERR_SUCCESS
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
            let byte_offset = ms_to_byte_offset(pos_ms, dev.spec);
            dev.current_position = byte_offset.min(dev.pcm.len() as u32);
            debug!("mciSeek: device {} → {}ms (byte {})", device_id, pos_ms, dev.current_position);
            MCIERR_SUCCESS
        } else {
            MCIERR_INVALID_DEVICE_ID
        }
    }

    /// MCI_RECORD — audio recording stub.
    ///
    /// Recording is not implemented; returns MCIERR_UNSUPPORTED_FUNCTION.
    fn mci_record(&self, device_id: u32, _param1: u32, _param2: u32) -> u32 {
        warn!("mciRecord: device {} — recording not implemented", device_id);
        MCIERR_UNSUPPORTED_FUNCTION
    }

    /// MCI_SET — configure device parameters.
    ///
    /// Currently handles `MCI_SET_AUDIO | MCI_SET_VOLUME` to set playback volume.
    ///
    /// MCI_SET_PARMS (guest, param2):
    ///   +0   hwndCallback  (u32)
    ///   +4   ulTimeFormat  (u32)
    ///   +8   ulSpeedFormat (u32)
    ///   +12  ulAudio       (u32) — channel: MCI_SET_AUDIO_ALL=0, LEFT=1, RIGHT=2
    ///   +16  ulLevel       (u32) — volume 0–100 with MCI_SET_VOLUME
    fn mci_set(&self, device_id: u32, param1: u32, param2: u32) -> u32 {
        debug!("  mciSet(devID={}, flags=0x{:08X})", device_id, param1);
        if param2 == 0 {
            return MCIERR_SUCCESS;
        }

        if !self.shared.mmpm_mgr.lock().unwrap().devices.contains_key(&(device_id as u16)) {
            return MCIERR_INVALID_DEVICE_ID;
        }

        if param1 & MCI_SET_AUDIO != 0 && param1 & MCI_SET_VOLUME != 0 {
            let _channel = self.guest_read::<u32>(param2 + SET_PARMS_AUDIO).unwrap_or(MCI_SET_AUDIO_ALL);
            let level    = self.guest_read::<u32>(param2 + SET_PARMS_LEVEL).unwrap_or(100);
            return self.mci_set_volume(device_id, level);
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
                MCI_STATUS_MODE => match device_mode(dev, &self.shared.audio) {
                    MciMode::Playing  => MCI_MODE_PLAY,
                    MciMode::Stopped  => MCI_MODE_STOP,
                    MciMode::NotReady => MCI_MODE_NOT_READY,
                },
                MCI_STATUS_LENGTH => byte_offset_to_ms(dev.pcm.len() as u32, dev.spec),
                MCI_STATUS_POSITION => byte_offset_to_ms(dev.current_position, dev.spec),
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
            if dev.alias.eq_ignore_ascii_case(alias) || dev.element_name.eq_ignore_ascii_case(alias) {
                return id;
            }
        }
        0
    }

    /// Load a WAV file through the VFS and decode it (`parse_wav`).
    fn load_wav_via_vfs(&self, os2_path: &str) -> Result<(PcmSpec, Vec<u8>), String> {
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

        parse_wav(&file_bytes)
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
        // <mcios2.h>
        assert_eq!(MCI_MODE_STOP, 4);
        assert_eq!(MCI_MODE_PLAY, 3);
        assert_eq!((MCI_PLAY, MCI_STOP, MCI_STATUS, MCI_SET), (4, 6, 10, 12));
        assert_ne!(MCI_MODE_NOT_READY, MCI_MODE_STOP);
        assert_ne!(MCI_MODE_NOT_READY, MCI_MODE_PLAY);
    }

    #[test]
    fn test_mci_device_drop_without_audio() {
        // A device that never played drops without touching audio.
        let dev = MciDevice {
            device_type: "waveaudio".to_string(),
            element_name: String::new(),
            alias: String::new(),
            mode: MciMode::Stopped,
            voice: None,
            pcm: Vec::new(),
            spec: DEFAULT_SPEC,
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
            alias: String::new(),
            mode: MciMode::Playing,
            voice: None,
            pcm: Vec::new(),
            spec: DEFAULT_SPEC,
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
        let audio = AudioService::default();
        beep_tone(&audio, 0, 1000, &|| false);
        beep_tone(&audio, 440, 0, &|| false);
        beep_tone(&audio, 0, 0, &|| false);
        // No backend: returns at once.
        let start = std::time::Instant::now();
        beep_tone(&audio, 440, 5000, &|| false);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_wait_for_playback_stops_when_its_call_should() {
        // An attached backend that never drains: the voice plays on.
        let audio = AudioService::default();
        audio.attach(Box::new(|| {}));
        let spec = PcmSpec { freq: BEEP_RATE, channels: 1, format: SampleFormat::S16 };
        let voice = audio.play(spec, beep_pcm(440, 5000), 1.0).expect("a voice");
        let start = std::time::Instant::now();
        let asked = std::cell::Cell::new(0);
        let end = wait_or_stop(&audio, voice, std::time::Duration::from_secs(10), &|| {
            asked.set(asked.get() + 1);
            asked.get() >= 2
        });
        assert_eq!(end, WaitEnd::Cancelled);
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "stopped after two slices");
        assert_eq!(wait_or_stop(&audio, voice, std::time::Duration::from_millis(150), &|| false), WaitEnd::TimedOut);
        audio.stop(voice);
    }

    #[test]
    fn mm_mcinotify_carries_the_user_parameter_and_the_message() {
        assert_eq!(notify_mp1(MCI_NOTIFY_SUPERSEDED, 0xBEEF), 0xBEEF_0001);
        assert_eq!(notify_mp2(7, MCI_PLAY), 0x0004_0007);
    }

    #[test]
    fn test_mm_mcinotify_value() {
        // <mcios2.h>
        assert_eq!(MM_MCINOTIFY, 0x0500);
        assert_eq!(MCIERR_INVALID_DEVICE_ID, 5001);
    }

    #[test]
    fn test_mci_notify_codes_are_distinct() {
        let codes = [
            MCI_NOTIFY_SUCCESSFUL,
            MCI_NOTIFY_SUPERSEDED,
            MCI_NOTIFY_ABORTED,
            MCI_NOTIFY_ERROR,
        ];
        // Consecutive codes from 0 (<mcios2.h>), not bit flags.
        assert_eq!(codes, [0, 1, 2, 3]);
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
        let offset = ms_to_byte_offset(10, DEFAULT_SPEC);
        assert_eq!(offset, 1764);
        assert_eq!(byte_offset_to_ms(176_400, DEFAULT_SPEC), 1000);
    }

    #[test]
    fn test_ms_to_byte_offset_zero() {
        assert_eq!(ms_to_byte_offset(0, DEFAULT_SPEC), 0);
        assert_eq!(byte_offset_to_ms(5, PcmSpec { freq: 0, ..DEFAULT_SPEC }), 0);
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

    /// A RIFF WAVE file with one `fmt ` chunk (`tag`, `channels`, `freq`,
    /// `bits`), an odd-sized chunk to skip, and `data`.
    fn wav(tag: u16, channels: u16, freq: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend(tag.to_le_bytes());
        fmt.extend(channels.to_le_bytes());
        fmt.extend(freq.to_le_bytes());
        fmt.extend((freq * channels as u32 * bits as u32 / 8).to_le_bytes());
        fmt.extend((channels * bits / 8).to_le_bytes());
        fmt.extend(bits.to_le_bytes());
        let mut body = b"WAVE".to_vec();
        for (id, chunk) in [(b"fmt ", fmt.as_slice()), (b"LIST", &[7u8][..]), (b"data", data)] {
            body.extend(id);
            body.extend((chunk.len() as u32).to_le_bytes());
            body.extend(chunk);
            if chunk.len() % 2 == 1 { body.push(0); }
        }
        let mut f = b"RIFF".to_vec();
        f.extend((body.len() as u32).to_le_bytes());
        f.extend(body);
        f
    }

    #[test]
    fn test_parse_wav_formats() {
        let (spec, pcm) = parse_wav(&wav(1, 2, 22050, 16, &[1, 2, 3, 4, 5, 6, 7])).unwrap();
        assert_eq!(spec, PcmSpec { freq: 22050, channels: 2, format: SampleFormat::S16 });
        assert_eq!(pcm, vec![1, 2, 3, 4], "whole frames only");
        let (spec, pcm) = parse_wav(&wav(1, 1, 8000, 8, &[0x80, 0xFF])).unwrap();
        assert_eq!((spec.format, pcm), (SampleFormat::U8, vec![0x80, 0xFF]));
        let (spec, pcm) = parse_wav(&wav(1, 1, 8000, 24, &[1, 2, 3])).unwrap();
        assert_eq!((spec.format, pcm), (SampleFormat::S32, vec![0, 1, 2, 3]), "24-bit widened");
        let (spec, _) = parse_wav(&wav(3, 1, 8000, 32, &[0; 4])).unwrap();
        assert_eq!(spec.format, SampleFormat::F32);
    }

    #[test]
    fn test_parse_wav_rejects_bad_files() {
        assert!(parse_wav(b"RIFX0000WAVE").is_err());
        assert!(parse_wav(&wav(2, 1, 8000, 4, &[0; 4])).is_err(), "MS ADPCM");
        assert!(parse_wav(&wav(1, 0, 8000, 16, &[0; 4])).is_err(), "no channels");
        let mut no_data = wav(1, 1, 8000, 16, &[]);
        no_data.truncate(no_data.len() - 8);
        assert!(parse_wav(&no_data).is_err());
        let mut huge = wav(1, 1, 8000, 16, &[0; 4]);
        let n = huge.len();
        huge[n - 8..n - 4].copy_from_slice(&u32::MAX.to_le_bytes()); // data length past the end
        assert_eq!(parse_wav(&huge).unwrap().1.len(), 4, "clamped to the file");
    }

    #[test]
    fn test_beep_pcm_length_and_amplitude() {
        let pcm = beep_pcm(440, 100);
        assert_eq!(pcm.len(), 4410 * 2);
        let peak = pcm.as_chunks::<2>().0.iter().map(|s| i16::from_le_bytes(*s).unsigned_abs()).max().unwrap();
        assert!(peak > 9000 && peak < 10000, "about 30% of full scale: {peak}");
    }

    /// MCI playback through the audio service: play hands the PCM from the
    /// seek position to the service, the mode follows the voice, volume
    /// changes reach the playing voice, stop ends it.
    #[test]
    fn test_mci_play_through_the_audio_service() {
        use crate::gui::audio::AudioCommand;
        let loader = super::super::Loader::new_mock();
        let audio = &loader.shared.audio;
        audio.attach(Box::new(|| {}));
        let id = {
            let mut mgr = loader.shared.mmpm_mgr.lock().unwrap();
            mgr.devices.insert(7, MciDevice {
                device_type: "waveaudio".into(), element_name: "t.wav".into(), alias: "t".into(),
                mode: MciMode::Stopped, voice: None,
                pcm: (0..=255u8).collect(), spec: PcmSpec { freq: 1000, channels: 1, format: SampleFormat::U8 },
                current_position: 0, volume: 100, notify_hwnd: 0, notify_cancel: None,
            });
            7u32
        };
        assert_eq!(loader.mci_resolve_alias("T"), 7, "aliases are case-insensitive");
        assert_eq!(loader.mci_resolve_alias("7"), 7);
        assert_eq!(loader.mci_seek_to(id, 100), MCIERR_SUCCESS); // 100 ms = 100 bytes
        assert_eq!(loader.mci_play_with_notify(id, 0, 0, 0), MCIERR_SUCCESS);
        let cmds = audio.take_commands();
        let [AudioCommand::Play { voice, data, gain, .. }] = cmds.as_slice() else { panic!("{cmds:?}") };
        assert_eq!((data.len(), data[0], *gain), (156, 100, 1.0));
        let voice = *voice;
        let mode = |l: &super::super::Loader| {
            let mgr = l.shared.mmpm_mgr.lock().unwrap();
            device_mode(&mgr.devices[&7], &l.shared.audio)
        };
        assert_eq!(mode(&loader), MciMode::Playing);
        assert_eq!(loader.mci_set_volume(id, 50), MCIERR_SUCCESS);
        assert_eq!(audio.take_commands(), vec![AudioCommand::SetGain { voice, gain: 0.5 }]);
        audio.finish(voice);
        assert_eq!(mode(&loader), MciMode::Stopped, "the voice ended");
        assert_eq!(loader.mci_stop(id), MCIERR_SUCCESS);
        assert_eq!(loader.mci_close(id), MCIERR_SUCCESS);
        assert_eq!(loader.mci_close(id), MCIERR_INVALID_DEVICE_ID);
        // Without a backend, play fails cleanly.
        audio.detach();
        loader.shared.mmpm_mgr.lock().unwrap().devices.insert(8, MciDevice {
            device_type: "waveaudio".into(), element_name: String::new(), alias: String::new(),
            mode: MciMode::Stopped,
            voice: None, pcm: vec![0; 4], spec: DEFAULT_SPEC, current_position: 0, volume: 100,
            notify_hwnd: 0, notify_cancel: None,
        });
        assert_eq!(loader.mci_play_with_notify(8, 0, 0, 0), MCIERR_CANNOT_LOAD_DRIVER);
    }

    /// `open … alias <name> wait` registers the alias (and `wait`, which
    /// takes no argument, does not swallow the next word); the return
    /// buffer size is a USHORT, so the high half of its slot is ignored.
    #[test]
    fn test_mci_send_string_alias_and_flags() {
        let loader = super::super::Loader::new_mock();
        let (cmd, ret) = (0x5000u32, 0x5100u32);
        let send = |text: &str| {
            loader.guest_write_bytes(cmd, text.as_bytes()).unwrap();
            loader.guest_write::<u8>(cmd + text.len() as u32, 0).unwrap();
            // cbReturnString 16 with garbage in the high half of the slot.
            loader.mci_send_string(cmd, ret, 0xABCD_0010, 0, 0)
        };
        assert_eq!(send("open type waveaudio wait alias Chime"), MCIERR_SUCCESS);
        assert_eq!(send("status chime mode"), MCIERR_SUCCESS);
        assert_eq!(loader.read_guest_string(ret), "stopped");
        assert_eq!(send("set chime audio volume to 30"), MCIERR_SUCCESS);
        assert_eq!(send("status nosuch mode"), MCIERR_INVALID_DEVICE_ID);
        assert_eq!(send("close chime"), MCIERR_SUCCESS);
        assert_eq!(send("close chime"), MCIERR_INVALID_DEVICE_ID);
    }

    /// mciSendCommand with the <mcios2.h> layouts: MCI_OPEN by device type
    /// ID with an alias returns the device ID in MCI_OPEN_PARMS, MCI_SET
    /// reads the volume from ulLevel (+16), MCI_STATUS answers the mode.
    #[test]
    fn test_mci_send_command_uses_toolkit_layouts() {
        let loader = super::super::Loader::new_mock();
        let (open, set, status, alias) = (0x5000u32, 0x5100u32, 0x5200u32, 0x5300u32);
        loader.guest_write_bytes(alias, b"bell\0").unwrap();
        loader.guest_write::<u32>(open + OPEN_PARMS_TYPE, MCI_DEVTYPE_WAVEFORM_AUDIO).unwrap();
        loader.guest_write::<u32>(open + OPEN_PARMS_ALIAS, alias).unwrap();
        assert_eq!(loader.mci_send_command(0, MCI_OPEN as u32, MCI_OPEN_TYPE_ID | MCI_OPEN_ALIAS | MCI_WAIT, open, 0),
                   MCIERR_SUCCESS);
        let id = loader.guest_read::<u16>(open + OPEN_PARMS_DEVICE_ID).unwrap() as u32;
        assert_ne!(id, 0);
        assert_eq!(loader.mci_resolve_alias("bell") as u32, id);

        loader.guest_write::<u32>(set + SET_PARMS_AUDIO, MCI_SET_AUDIO_ALL).unwrap();
        loader.guest_write::<u32>(set + SET_PARMS_LEVEL, 40).unwrap();
        assert_eq!(loader.mci_send_command(id, MCI_SET as u32, MCI_SET_AUDIO | MCI_SET_VOLUME, set, 0), MCIERR_SUCCESS);
        assert_eq!(loader.shared.mmpm_mgr.lock().unwrap().devices[&(id as u16)].volume, 40);

        loader.guest_write::<u32>(status + 8, MCI_STATUS_MODE).unwrap();
        assert_eq!(loader.mci_send_command(id, MCI_STATUS as u32, 0, status, 0), MCIERR_SUCCESS);
        assert_eq!(loader.guest_read::<u32>(status + 4), Some(MCI_MODE_STOP));
        assert_eq!(loader.mci_send_command(id, MCI_CLOSE as u32, 0, 0, 0), MCIERR_SUCCESS);
        assert_eq!(loader.mci_send_command(id, MCI_PLAY as u32, 0, 0, 0), MCIERR_INVALID_DEVICE_ID);
    }

    /// `open waveaudio alias w` opens the device by name (no file); a word
    /// that is not a device name is still the element.
    #[test]
    fn test_mci_open_by_device_name() {
        assert_eq!(mci_device_name("waveaudio"), Some("waveaudio"));
        assert_eq!(mci_device_name("waveaudio01"), Some("waveaudio"));
        assert_eq!(mci_device_name("waveaudio1x"), None);
        assert_eq!(mci_device_name("tone.wav"), None);
        let loader = super::super::Loader::new_mock();
        let (cmd, ret) = (0x5000u32, 0x5100u32);
        let send = |text: &str| {
            loader.guest_write_bytes(cmd, text.as_bytes()).unwrap();
            loader.guest_write::<u8>(cmd + text.len() as u32, 0).unwrap();
            loader.mci_send_string(cmd, ret, 16, 0, 0)
        };
        assert_eq!(send("open waveaudio alias wtest"), MCIERR_SUCCESS);
        assert_eq!(send("close wtest"), MCIERR_SUCCESS);
        assert_eq!(send("open nosuch.wav alias x"), MCIERR_FILE_NOT_FOUND, "an element that does not exist");
    }

    /// Every MCI error code has a text that fits the documented 128 bytes;
    /// codes outside `<meerror.h>` have none.
    #[test]
    fn test_mci_error_texts() {
        assert_eq!(mci_error_text(MCIERR_SUCCESS), Some("No error"));
        assert_eq!(mci_error_text(MCIERR_INVALID_DEVICE_ID), Some("Invalid device ID"));
        assert_eq!(mci_error_text(MCIERR_FILE_NOT_FOUND), Some("File not found"));
        assert_eq!(mci_error_text(MCIERR_BASE + 36), None, "a gap in <meerror.h>");
        assert_eq!(mci_error_text(MCIERR_BASE + 256), None, "the custom driver base");
        assert_eq!(mci_error_text(42), None);
        let texts = super::super::mmpm_errtext::MCI_ERROR_TEXTS;
        assert!(texts.windows(2).all(|w| w[0].0 < w[1].0), "in code order, no duplicates");
        assert!(texts.iter().all(|(_, t)| !t.is_empty() && t.len() < MCI_ERROR_STRING_MAX));
    }

    /// `mciGetErrorString`: the text with its NUL, a short buffer cut
    /// without one, and the documented refusals.
    #[test]
    fn test_mci_get_error_string() {
        let loader = super::super::Loader::new_mock();
        let buf = 0x0020_0000;
        let read = |n: usize| {
            (0..n as u32).map(|i| loader.guest_read::<u8>(buf + i).unwrap()).collect::<Vec<u8>>()
        };
        assert_eq!(loader.mci_get_error_string(MCIERR_FILE_NOT_FOUND, buf, 128), MCIERR_SUCCESS);
        assert_eq!(read(15), b"File not found\0");
        loader.guest_write_bytes(buf, &[0xAA; 8]).unwrap();
        assert_eq!(loader.mci_get_error_string(MCIERR_FILE_NOT_FOUND, buf, 4), MCIERR_SUCCESS);
        assert_eq!(read(5), b"File\xAA", "four bytes, no NUL");
        assert_eq!(loader.mci_get_error_string(MCIERR_BASE + 36, buf, 128), MCIERR_OUTOFRANGE);
        assert_eq!(loader.mci_get_error_string(MCIERR_FILE_NOT_FOUND, 0, 128), MCIERR_INVALID_BUFFER);
        // The high word names a device: one that is not open is refused.
        assert_eq!(loader.mci_get_error_string((9 << 16) | MCIERR_FILE_NOT_FOUND, buf, 128), MCIERR_INVALID_DEVICE_ID);
        loader.shared.mmpm_mgr.lock().unwrap().devices.insert(9, MciDevice {
            device_type: "waveaudio".into(), element_name: String::new(), alias: String::new(),
            mode: MciMode::Stopped, voice: None, pcm: Vec::new(),
            spec: PcmSpec { freq: 1000, channels: 1, format: SampleFormat::U8 },
            current_position: 0, volume: 100, notify_hwnd: 0, notify_cancel: None,
        });
        assert_eq!(loader.mci_get_error_string((9 << 16) | MCIERR_FILE_NOT_FOUND, buf, 128), MCIERR_SUCCESS);
    }

    /// mciGetDeviceID finds an alias; the system values are kept and checked.
    #[test]
    fn test_mci_device_ids_and_system_values() {
        let loader = super::super::Loader::new_mock();
        let p = 0x0020_0000;
        loader.shared.mmpm_mgr.lock().unwrap().devices.insert(5, MciDevice {
            device_type: "waveaudio".into(), element_name: "x.wav".into(), alias: "Tune".into(),
            mode: MciMode::Stopped, voice: None, pcm: Vec::new(),
            spec: PcmSpec { freq: 1000, channels: 1, format: SampleFormat::U8 },
            current_position: 0, volume: 100, notify_hwnd: 0, notify_cancel: None,
        });
        loader.guest_write_bytes(p, b"TUNE\0").unwrap();
        assert_eq!(loader.mci_get_device_id(p), 5);
        loader.guest_write_bytes(p, b"x.wav\0").unwrap();
        assert_eq!(loader.mci_get_device_id(p), 0, "an element name is no alias");
        loader.guest_write::<u32>(p, 101).unwrap();
        assert_eq!(loader.mci_set_sys_value(MSV_MASTERVOLUME, p), 0, "above 100");
        loader.guest_write::<u32>(p, 30).unwrap();
        assert_eq!(loader.mci_set_sys_value(MSV_MASTERVOLUME, p), 1);
        assert_eq!(loader.mci_query_sys_value(MSV_MASTERVOLUME, p), MCIERR_SUCCESS);
        assert_eq!(loader.guest_read::<u32>(p), Some(30));
        loader.guest_write_bytes(p, b"D:\\TMP\0").unwrap();
        assert_eq!(loader.mci_set_sys_value(MSV_WORKPATH, p), 1);
        assert_eq!(loader.mci_query_sys_value(MSV_WORKPATH, p + 0x100), MCIERR_SUCCESS);
        assert_eq!(loader.guest_read_bytes(p + 0x100, 7).unwrap(), b"D:\\TMP\0");
        assert_eq!(loader.mci_query_sys_value(99, p), MCIERR_INVALID_FLAG);
        assert_eq!(loader.mci_set_sys_value(99, p), 0);
    }
}
