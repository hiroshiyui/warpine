// SPDX-License-Identifier: GPL-3.0-or-later
//
// The RT_POINTER decoder (`WinLoadPointer`, `pm_pointer.rs`): a pointer or
// icon file, or a bitmap array of them, into an RGBA image with a hot spot.
// A program's resources are untrusted, so the decoder is pure and fuzzed
// (fuzz/fuzz_targets/pointer.rs, ADR-0112), included there by #[path] with
// bitmap_codec.rs and shims for the constants and `PointerImage`.

use super::bitmap_codec::{decode_rows, info_len, parse_header, BitmapFormat};
use super::constants::{BFT_BITMAPARRAY, MAX_POINTER_DIM, POINTER_SIZE};
use crate::gui::pointer::PointerImage;

/// File-header types (`BFT_*`, <pmgpi.h>).
pub(crate) const BFT_ICON: u16 = 0x4349;
pub(crate) const BFT_POINTER: u16 = 0x5450;
pub(crate) const BFT_COLORICON: u16 = 0x4943;
pub(crate) const BFT_COLORPOINTER: u16 = 0x5043;

/// Offsets in BITMAPFILEHEADER(2): hot spot, offBits, and the info header.
pub(crate) const FH_HOTSPOT_X: usize = 6;
pub(crate) const FH_HOTSPOT_Y: usize = 8;
pub(crate) const FH_OFF_BITS: usize = 10;
pub(crate) const FH_INFO: usize = 14;
/// BITMAPARRAYFILEHEADER(2): offNext and the embedded file header (the
/// display size between them is not used: entries are chosen by size).
pub(crate) const AH_OFF_NEXT: usize = 6;
pub(crate) const AH_FILE_HEADER: usize = 14;
/// At most this many bitmap-array entries are looked at.
pub(crate) const MAX_ARRAY_ENTRIES: usize = 64;

pub(crate) fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off.checked_add(2)?)?.try_into().ok()?))
}

pub(crate) fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off.checked_add(4)?)?.try_into().ok()?))
}

/// One decoded pointer of a resource.
#[derive(Debug)]
struct Candidate {
    image: PointerImage,
    color: bool,
}

/// The pel rows (bottom-up) of the bitmap whose file header is at `fh`,
/// with format `fmt`, `rows` rows; `None` if the data is not all there.
fn bitmap_rows(res: &[u8], fh: usize, fmt: &BitmapFormat, rows: u32) -> Option<Vec<Vec<Option<u32>>>> {
    let off = u32_at(res, fh + FH_OFF_BITS)? as usize;
    let len = fmt.data_len(rows)?;
    let data = res.get(off..off.checked_add(len)?)?;
    Some(decode_rows(fmt, data, rows))
}

/// Decode the pointer or icon whose (first) file header is at `fh`.
fn decode_entry(res: &[u8], fh: usize) -> Option<(PointerImage, bool)> {
    let color = match u16_at(res, fh)? {
        BFT_POINTER | BFT_ICON => false,
        BFT_COLORPOINTER | BFT_COLORICON => true,
        _ => return None,
    };
    let mut mask = parse_header(res.get(fh + FH_INFO..)?, false)?;
    let (w, h2) = (mask.cx, mask.cy);
    let h = h2 / 2;
    let valid = mask.bit_count == 1 && w > 0 && h > 0 && h2 % 2 == 0
        && w <= MAX_POINTER_DIM && h <= MAX_POINTER_DIM;
    if !valid { return None; }
    // Decode the masks to their bits, not to colours.
    mask.palette = vec![0, 1];
    let mask_rows = bitmap_rows(res, fh, &mask, h2)?;
    let colors = if color {
        let fh2 = fh + FH_INFO + info_len(mask.header_len, mask.bit_count);
        let cfmt = parse_header(res.get(fh2 + FH_INFO..)?, true)?;
        if (cfmt.cx, cfmt.cy) != (w, h) { return None; }
        Some(bitmap_rows(res, fh2, &cfmt, h)?)
    } else {
        None
    };
    let bit = |row: &Vec<Option<u32>>, x: usize| row.get(x).copied().flatten().unwrap_or(0) & 1;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    // Output rows top first: PM row h-1 down to 0.
    for y in (0..h as usize).rev() {
        let (xor_row, and_row) = (&mask_rows[y], &mask_rows[y + h as usize]);
        for x in 0..w as usize {
            let (and, xor) = (bit(and_row, x), bit(xor_row, x));
            let px: [u8; 4] = match (and, xor) {
                (1, 0) => [0, 0, 0, 0],
                // Inverted screen: black (a colour cursor cannot invert).
                (1, _) => [0, 0, 0, 0xFF],
                _ => {
                    let c = match &colors {
                        Some(rows) => rows[y].get(x).copied().flatten().unwrap_or(0),
                        None if xor == 1 => 0x00FF_FFFF,
                        None => 0,
                    };
                    [(c >> 16) as u8, (c >> 8) as u8, c as u8, 0xFF]
                }
            };
            rgba.extend(px);
        }
    }
    // The hot spot counts from the bottom-left pel; the image from the top.
    let hx = u16_at(res, fh + FH_HOTSPOT_X)? as i16 as i32;
    let hy = u16_at(res, fh + FH_HOTSPOT_Y)? as i16 as i32;
    let hot_x = hx.clamp(0, w as i32 - 1);
    let hot_y = (h as i32 - 1 - hy).clamp(0, h as i32 - 1);
    Some((PointerImage { width: w, height: h, hot_x, hot_y, rgba }, color))
}

/// Decode an RT_POINTER resource (a pointer or icon file, or a bitmap
/// array of them) into the image to show: from an array, the first entry
/// of the display's pointer size (`POINTER_SIZE`), a colour one before a
/// black-and-white one, else the first entry.
pub fn decode_pointer_resource(res: &[u8]) -> Option<PointerImage> {
    let mut candidates = Vec::new();
    if u16_at(res, 0)? == BFT_BITMAPARRAY {
        let mut at = 0usize;
        for _ in 0..MAX_ARRAY_ENTRIES {
            if u16_at(res, at) != Some(BFT_BITMAPARRAY) { break; }
            if let Some((image, color)) = decode_entry(res, at + AH_FILE_HEADER) {
                candidates.push(Candidate { image, color });
            }
            // Entries only go forward, so a loop cannot be followed; a
            // missing next entry ends the array.
            let Some(next) = u32_at(res, at + AH_OFF_NEXT) else { break };
            let next = next as usize;
            if next <= at { break; }
            at = next;
        }
    } else {
        let (image, color) = decode_entry(res, 0)?;
        candidates.push(Candidate { image, color });
    }
    let score = |c: &Candidate| {
        let device_size = c.image.width == POINTER_SIZE && c.image.height == POINTER_SIZE;
        (device_size, c.color)
    };
    let mut best: Option<Candidate> = None;
    for c in candidates {
        if best.as_ref().is_none_or(|b| score(&c) > score(b)) {
            best = Some(c);
        }
    }
    best.map(|b| b.image)
}
