// SPDX-License-Identifier: GPL-3.0-or-later
//
// The WAV parser of MMPM/2's waveaudio device (`mmpm.rs`): the files a
// program names are untrusted, so the parser is pure and fuzzed
// (fuzz/fuzz_targets/wav.rs, ADR-0112), included there by #[path] with a
// shim for the PCM types it returns.

use crate::gui::audio::{PcmSpec, SampleFormat};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// Offset of the sub-format GUID's first two bytes in an extensible `fmt `.
const FMT_EXT_SUBFORMAT: usize = 24;

/// Parse a RIFF WAVE file: uncompressed PCM (8, 16, 24 or 32 bits; 24-bit
/// samples are widened to 32) or 32-bit IEEE float, also inside
/// `WAVE_FORMAT_EXTENSIBLE`. Compressed formats (ADPCM, µ-law…) are refused.
pub fn parse_wav(bytes: &[u8]) -> Result<(PcmSpec, Vec<u8>), String> {
    let u16_at = |b: &[u8], o: usize| b.get(o..o + 2).map(|v| u16::from_le_bytes([v[0], v[1]]));
    let u32_at = |b: &[u8], o: usize| b.get(o..o + 4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a RIFF WAVE file".into());
    }
    let (mut fmt, mut data) = (None, None);
    let mut at = 12;
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), u32_at(bytes, at + 4)) {
        let body = at + 8;
        let end = body.saturating_add(len as usize).min(bytes.len());
        match id {
            b"fmt " => fmt = Some(&bytes[body..end]),
            b"data" => data = Some(&bytes[body..end]),
            _ => {}
        }
        at = body.saturating_add(len as usize + (len as usize & 1)); // chunks are word-aligned
    }
    let fmt = fmt.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    let mut tag = u16_at(fmt, 0).ok_or("short fmt chunk")?;
    let channels = u16_at(fmt, 2).ok_or("short fmt chunk")?;
    let freq = u32_at(fmt, 4).ok_or("short fmt chunk")?;
    let bits = u16_at(fmt, 14).ok_or("short fmt chunk")?;
    if tag == WAVE_FORMAT_EXTENSIBLE {
        tag = u16_at(fmt, FMT_EXT_SUBFORMAT).ok_or("short extensible fmt chunk")?;
    }
    if channels == 0 || channels > u8::MAX as u16 || freq == 0 {
        return Err(format!("bad format: {channels} channels at {freq} Hz"));
    }
    let (format, pcm) = match (tag, bits) {
        (WAVE_FORMAT_PCM, 8) => (SampleFormat::U8, data.to_vec()),
        (WAVE_FORMAT_PCM, 16) => (SampleFormat::S16, data.to_vec()),
        (WAVE_FORMAT_PCM, 24) => (SampleFormat::S32, data.as_chunks::<3>().0.iter()
            .flat_map(|s| [0, s[0], s[1], s[2]]).collect()),
        (WAVE_FORMAT_PCM, 32) => (SampleFormat::S32, data.to_vec()),
        (WAVE_FORMAT_IEEE_FLOAT, 32) => (SampleFormat::F32, data.to_vec()),
        _ => return Err(format!("unsupported WAV encoding {tag:#06x}, {bits} bits")),
    };
    let spec = PcmSpec { freq, channels: channels as u8, format };
    // Whole frames only.
    let n = pcm.len() - pcm.len() % spec.bytes_per_frame() as usize;
    Ok((spec, pcm[..n].to_vec()))
}
