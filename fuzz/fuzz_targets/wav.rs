// SPDX-License-Identifier: GPL-3.0-or-later
//! The WAV parser (src/loader/wav.rs): the files a program hands MMPM/2's
//! waveaudio device are untrusted. A RIFF WAVE file becomes its format and
//! whole frames of PCM.
#![no_main]

use libfuzzer_sys::fuzz_target;

/// What wav.rs takes from src/gui/audio.rs: the same types, with the same
/// sizes. The parser is included at the root, so its `crate::` is the root.
mod gui {
    pub mod audio {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum SampleFormat { U8, S16, S32, F32 }

        impl SampleFormat {
            pub fn bytes(self) -> u32 {
                match self { SampleFormat::U8 => 1, SampleFormat::S16 => 2, SampleFormat::S32 | SampleFormat::F32 => 4 }
            }
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct PcmSpec {
            pub freq: u32,
            pub channels: u8,
            pub format: SampleFormat,
        }

        impl PcmSpec {
            pub fn bytes_per_frame(&self) -> u32 {
                self.format.bytes() * self.channels.max(1) as u32
            }
        }
    }
}

#[path = "../../src/loader/wav.rs"]
#[allow(dead_code)]
mod wav;

fuzz_target!(|data: &[u8]| {
    if let Ok((spec, pcm)) = wav::parse_wav(data) {
        assert!(spec.freq > 0 && spec.channels > 0);
        assert_eq!(pcm.len() % spec.bytes_per_frame() as usize, 0, "whole frames only");
        // 24-bit samples widen to 32: at most 4/3 of the input.
        assert!(pcm.len() <= data.len() * 4 / 3 + 4);
    }
});
