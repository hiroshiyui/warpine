// SPDX-License-Identifier: GPL-3.0-only
//
// Audio output for MMPM/2 and DosBeep.
//
// SDL3 audio belongs to the main thread: in the safe sdl3-rs API the `Sdl`
// context is created there and `AudioSubsystem` / `AudioStream` are neither
// `Send` nor `Sync`. The guest's API threads therefore do not touch SDL.
// They hand PCM data to the `AudioService` in `SharedState` and get back a
// voice number; the main-thread loop (PM desktop or VGA text window) owns an
// `SdlAudio` that drains the commands into SDL audio streams and reports how
// much of each voice is still queued. Callers that must block (`MCI_WAIT`,
// DosBeep) or post a completion message (`MCI_NOTIFY`) wait on the service.
//
// Without a main-thread loop (`WARPINE_HEADLESS=1`, where the vCPU runs on
// the main thread) no backend is attached and `play` returns `None`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use log::warn;

use crate::loader::mutex_ext::MutexExt;

/// PCM sample formats (little-endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat { U8, S16, S32, F32 }

impl SampleFormat {
    pub fn bytes(self) -> u32 {
        match self { SampleFormat::U8 => 1, SampleFormat::S16 => 2, SampleFormat::S32 | SampleFormat::F32 => 4 }
    }
}

/// The format of a block of PCM data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmSpec {
    pub freq: u32,
    pub channels: u8,
    pub format: SampleFormat,
}

impl PcmSpec {
    pub fn bytes_per_frame(&self) -> u32 {
        self.format.bytes() * self.channels.max(1) as u32
    }

    pub fn bytes_per_sec(&self) -> u32 {
        self.freq.saturating_mul(self.bytes_per_frame())
    }
}

/// A request from an API thread to the main-thread audio backend.
#[derive(Debug, PartialEq)]
pub enum AudioCommand {
    Play { voice: u32, spec: PcmSpec, data: Vec<u8>, gain: f32 },
    Stop { voice: u32 },
    SetGain { voice: u32, gain: f32 },
}

/// How a wait for a voice ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitEnd { Finished, Cancelled, TimedOut }

/// How often a waiter re-checks its cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(50);

pub type WakeFn = Box<dyn Fn() + Send + Sync>;

/// The thread-safe side of audio output; see the module comment.
#[derive(Default)]
pub struct AudioService {
    commands: Mutex<VecDeque<AudioCommand>>,
    /// Voice → bytes still queued. A voice that is not here has finished
    /// (or was stopped).
    voices: Mutex<HashMap<u32, u32>>,
    finished: Condvar,
    next_voice: AtomicU32,
    attached: AtomicBool,
    wake: Mutex<Option<WakeFn>>,
}

impl AudioService {
    /// A backend is now serving commands; `wake` interrupts its loop's wait.
    pub fn attach(&self, wake: WakeFn) {
        *self.wake.lock_or_recover() = Some(wake);
        self.attached.store(true, Ordering::Release);
    }

    /// The backend is gone: every voice ends.
    pub fn detach(&self) {
        self.attached.store(false, Ordering::Release);
        *self.wake.lock_or_recover() = None;
        self.voices.lock_or_recover().clear();
        self.commands.lock_or_recover().clear();
        self.finished.notify_all();
    }

    /// Whether a backend is attached.
    pub fn available(&self) -> bool {
        self.attached.load(Ordering::Acquire)
    }

    fn send(&self, cmd: AudioCommand) {
        self.commands.lock_or_recover().push_back(cmd);
        if let Some(wake) = &*self.wake.lock_or_recover() {
            wake();
        }
    }

    /// Queue `data` for playback; the voice that plays it, or `None` when
    /// there is no backend or no data.
    pub fn play(&self, spec: PcmSpec, data: Vec<u8>, gain: f32) -> Option<u32> {
        if !self.available() || data.is_empty() { return None; }
        let voice = self.next_voice.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        self.voices.lock_or_recover().insert(voice, data.len() as u32);
        self.send(AudioCommand::Play { voice, spec, data, gain });
        Some(voice)
    }

    /// Stop `voice` at once.
    pub fn stop(&self, voice: u32) {
        self.voices.lock_or_recover().remove(&voice);
        self.finished.notify_all();
        self.send(AudioCommand::Stop { voice });
    }

    /// Change the volume of a playing voice (1.0 = as recorded).
    pub fn set_gain(&self, voice: u32, gain: f32) {
        if self.is_playing(voice) {
            self.send(AudioCommand::SetGain { voice, gain });
        }
    }

    pub fn is_playing(&self, voice: u32) -> bool {
        self.voices.lock_or_recover().contains_key(&voice)
    }

    /// Wait until `voice` finishes, `cancel` is set or `timeout` passes.
    pub fn wait(&self, voice: u32, cancel: Option<&AtomicBool>, timeout: Duration) -> WaitEnd {
        let deadline = Instant::now() + timeout;
        let mut voices = self.voices.lock_or_recover();
        loop {
            if !voices.contains_key(&voice) { return WaitEnd::Finished; }
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { return WaitEnd::Cancelled; }
            let now = Instant::now();
            if now >= deadline { return WaitEnd::TimedOut; }
            let step = (deadline - now).min(CANCEL_POLL);
            voices = self.finished.wait_timeout(voices, step)
                .unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    // ── Backend side ─────────────────────────────────────────────────────────

    /// Take the queued commands.
    pub fn take_commands(&self) -> Vec<AudioCommand> {
        self.commands.lock_or_recover().drain(..).collect()
    }

    /// Record how many bytes of `voice` are still queued.
    pub fn update(&self, voice: u32, queued: u32) {
        if let Some(q) = self.voices.lock_or_recover().get_mut(&voice) {
            *q = queued;
        }
    }

    /// `voice` has finished playing (or could not be played).
    pub fn finish(&self, voice: u32) {
        self.voices.lock_or_recover().remove(&voice);
        self.finished.notify_all();
    }

    /// Whether any voice is playing (the backend should keep pumping).
    pub fn active(&self) -> bool {
        !self.voices.lock_or_recover().is_empty()
    }
}

/// How long the main loop may wait between pumps while audio plays, so
/// completion is noticed promptly.
pub const AUDIO_PUMP_INTERVAL: Duration = Duration::from_millis(10);

/// The SDL3 audio backend, owned by the main-thread loop.
pub struct SdlAudio {
    audio: Option<sdl3::AudioSubsystem>,
    streams: HashMap<u32, sdl3::audio::AudioStreamOwner>,
    attached: bool,
}

impl SdlAudio {
    /// Open the audio subsystem. Without audio (no device, unusable
    /// `SDL_AUDIO_DRIVER`) playback requests later fail gracefully.
    pub fn new(sdl: &sdl3::Sdl) -> Self {
        let audio = sdl.audio()
            .map_err(|e| warn!("[audio] SDL audio unavailable ({e}); MMPM/2 and DosBeep are silent"))
            .ok();
        SdlAudio { audio, streams: HashMap::new(), attached: false }
    }

    /// Start serving `service` (once): from now on `play` succeeds, and
    /// `wake` interrupts the owning loop's wait when a command arrives.
    pub fn attach(&mut self, service: &AudioService, wake: WakeFn) {
        if self.audio.is_some() && !self.attached {
            service.attach(wake);
            self.attached = true;
        }
    }

    /// Carry out queued commands and report each voice's progress.
    pub fn pump(&mut self, service: &AudioService) {
        let Some(audio) = &self.audio else { return };
        for cmd in service.take_commands() {
            match cmd {
                AudioCommand::Play { voice, spec, data, gain } => {
                    match open_stream(audio, spec, &data, gain) {
                        Ok(stream) => { self.streams.insert(voice, stream); }
                        Err(e) => {
                            warn!("[audio] cannot play voice {voice}: {e}");
                            service.finish(voice);
                        }
                    }
                }
                AudioCommand::Stop { voice } => { self.streams.remove(&voice); }
                AudioCommand::SetGain { voice, gain } => {
                    if let Some(s) = self.streams.get(&voice) { let _ = s.set_gain(gain); }
                }
            }
        }
        self.streams.retain(|&voice, stream| {
            let queued = stream.queued_bytes().unwrap_or(0);
            if queued > 0 && service.is_playing(voice) {
                service.update(voice, queued as u32);
                true
            } else {
                service.finish(voice);
                false
            }
        });
    }

    /// Whether streams are playing (pump at least every
    /// [`AUDIO_PUMP_INTERVAL`]).
    pub fn busy(&self) -> bool {
        !self.streams.is_empty()
    }
}

fn open_stream(audio: &sdl3::AudioSubsystem, spec: PcmSpec, data: &[u8], gain: f32)
               -> Result<sdl3::audio::AudioStreamOwner, sdl3::Error> {
    use sdl3::audio::{AudioFormat, AudioSpec};
    let format = match spec.format {
        SampleFormat::U8 => AudioFormat::U8,
        SampleFormat::S16 => AudioFormat::S16LE,
        SampleFormat::S32 => AudioFormat::S32LE,
        SampleFormat::F32 => AudioFormat::F32LE,
    };
    let spec = AudioSpec {
        freq: Some(spec.freq as i32), channels: Some(spec.channels as i32), format: Some(format),
    };
    // The stream converts from the data's format to the device's.
    let stream = audio.open_playback_device(&spec)?.open_device_stream(Some(&spec))?;
    stream.put_data(data)?;
    stream.flush()?;
    stream.set_gain(gain)?;
    stream.resume()?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const SPEC: PcmSpec = PcmSpec { freq: 8000, channels: 1, format: SampleFormat::S16 };

    #[test]
    fn spec_sizes() {
        assert_eq!(SPEC.bytes_per_frame(), 2);
        assert_eq!(SPEC.bytes_per_sec(), 16000);
        let st = PcmSpec { freq: 44100, channels: 2, format: SampleFormat::F32 };
        assert_eq!(st.bytes_per_frame(), 8);
    }

    #[test]
    fn without_a_backend_nothing_plays() {
        let s = AudioService::default();
        assert!(!s.available());
        assert_eq!(s.play(SPEC, vec![0; 4], 1.0), None);
        assert!(s.take_commands().is_empty());
    }

    #[test]
    fn commands_flow_to_the_backend_and_wake_it() {
        let s = AudioService::default();
        let woken = Arc::new(AtomicU32::new(0));
        let w = woken.clone();
        s.attach(Box::new(move || { w.fetch_add(1, Ordering::Relaxed); }));
        let v = s.play(SPEC, vec![1, 2], 0.5).unwrap();
        s.set_gain(v, 0.25);
        assert_eq!(woken.load(Ordering::Relaxed), 2);
        assert_eq!(s.take_commands(), vec![
            AudioCommand::Play { voice: v, spec: SPEC, data: vec![1, 2], gain: 0.5 },
            AudioCommand::SetGain { voice: v, gain: 0.25 },
        ]);
        assert!(s.play(SPEC, Vec::new(), 1.0).is_none(), "no data, no voice");
        assert!(s.is_playing(v));
        s.update(v, 1);
        s.finish(v);
        assert!(!s.is_playing(v));
        s.set_gain(v, 1.0);
        assert!(s.take_commands().is_empty(), "no commands for finished voices");
    }

    #[test]
    fn waits_end_on_finish_cancel_or_timeout() {
        let s = Arc::new(AudioService::default());
        s.attach(Box::new(|| {}));
        let v = s.play(SPEC, vec![0; 8], 1.0).unwrap();
        assert_eq!(s.wait(v, None, Duration::from_millis(20)), WaitEnd::TimedOut);
        let cancel = AtomicBool::new(true);
        assert_eq!(s.wait(v, Some(&cancel), Duration::from_secs(5)), WaitEnd::Cancelled);
        let s2 = s.clone();
        let t = std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(30)); s2.finish(v); });
        assert_eq!(s.wait(v, None, Duration::from_secs(5)), WaitEnd::Finished);
        t.join().unwrap();
        // Stopping and detaching end voices too.
        let v = s.play(SPEC, vec![0; 8], 1.0).unwrap();
        s.stop(v);
        assert_eq!(s.wait(v, None, Duration::from_secs(1)), WaitEnd::Finished);
        let v = s.play(SPEC, vec![0; 8], 1.0).unwrap();
        s.detach();
        assert!(!s.is_playing(v) && !s.available());
    }
}
