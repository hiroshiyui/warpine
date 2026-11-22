// SPDX-License-Identifier: GPL-3.0-or-later
//
// OS/2 bitmap formats: BITMAPINFOHEADER / BITMAPINFOHEADER2 parsing, the
// pel data of uncompressed bitmaps (1, 4, 8, 24 and 32 bits per pel) and of
// run-length encoded ones (BCA_RLE4 / RLE8 / RLE24), and RT_BITMAP
// resources (BITMAPFILEHEADER / BITMAPARRAYFILEHEADER).
//
// Layouts are from the OS/2 Warp 4 Toolkit header <pmgpi.h>:
//
//   BITMAPINFOHEADER  (cbFix = 12): ULONG cbFix; USHORT cx, cy, cPlanes, cBitCount
//   BITMAPINFOHEADER2 (cbFix 16..64): ULONG cbFix, cx, cy; USHORT cPlanes, cBitCount;
//       ULONG ulCompression (16), cbImage, cxResolution, cyResolution,
//       cclrUsed (32), …  — fields past cbFix are absent and read as 0
//   colour table after the header: RGB (3 bytes) entries after a 12-byte
//       header, RGB2 (4 bytes: blue, green, red, options) otherwise
//   pel data: rows bottom-up, each padded to a 4-byte boundary; 24-bit pels
//       are blue, green, red
//   RLE data (cbImage bytes, cbImage at offset 20): a stream from the bottom
//       row up of byte pairs — (n > 0, value): n pels of value (RLE4: two
//       alternating nibbles; RLE24: the value is three bytes, blue, green,
//       red); (0, 0) end of row; (0, 1) end of bitmap; (0, 2, dx, dy) move
//       right dx and up dy (skipped pels keep their value); (0, n ≥ 3): n
//       literal pels follow, padded to a 16-bit boundary
//
// The codec works on byte slices only, so the fuzz target `bitmap` can
// include it alone (fuzz/, ADR-0112); `pm_bitmap.rs` builds surfaces and
// the GPI / DEV entry points on it.

use super::constants::{BCA_RLE24, BCA_RLE4, BCA_RLE8, BCA_UNCOMP, BFT_BITMAPARRAY, BFT_BMAP};
use crate::gui::surface::MAX_SURFACE_DIM;

/// cbFix of the old 16-bit-field BITMAPINFOHEADER.
pub const OLD_HEADER_LEN: u32 = 12;
/// Shortest and longest cbFix accepted for BITMAPINFOHEADER2.
pub const MIN_HEADER2_LEN: u32 = 16;
pub const MAX_HEADER2_LEN: u32 = 64;
/// Offsets in BITMAPINFOHEADER2.
const H2_COMPRESSION: u32 = 16;
const H2_IMAGE_LEN: u32 = 20;
const H2_CLR_USED: u32 = 32;
/// Size of BITMAPFILEHEADER up to the info header, and of the
/// BITMAPARRAYFILEHEADER fields before its embedded file header.
pub(crate) const FILE_HEADER_LEN: usize = 14;
pub(crate) const ARRAY_HEADER_LEN: usize = 14;

/// The format of bitmap data: size, depth and colour table.
#[derive(Clone, Debug, PartialEq)]
pub struct BitmapFormat {
    pub cx: u32,
    pub cy: u32,
    pub planes: u16,
    pub bit_count: u16,
    /// cbFix of the header it was read from (12 = old layout).
    pub header_len: u32,
    /// Colour table as 0x00RRGGBB, for 1/4/8-bit formats.
    pub palette: Vec<u32>,
    /// `BCA_UNCOMP` or one of the RLE formats.
    pub compression: u32,
    /// cbImage: bytes of compressed data (0 when unknown).
    pub image_len: u32,
}

impl BitmapFormat {
    /// An uncompressed format.
    pub fn uncompressed(cx: u32, cy: u32, bit_count: u16, header_len: u32, palette: Vec<u32>) -> Self {
        BitmapFormat { cx, cy, planes: 1, bit_count, header_len, palette,
                       compression: BCA_UNCOMP, image_len: 0 }
    }

    /// Bytes of pel data for `rows` rows: padded rows when uncompressed,
    /// cbImage when compressed (`None` if it is missing or larger than any
    /// RLE stream of this size can usefully be: [`max_rle_len`]).
    pub fn data_len(&self, rows: u32) -> Option<usize> {
        if self.compression == BCA_UNCOMP {
            return Some(self.row_bytes() * rows as usize);
        }
        let n = self.image_len as usize;
        (n > 0 && n <= max_rle_len(self.cx, rows)).then_some(n)
    }

    /// Bytes per row of pel data (padded to 4 bytes).
    pub fn row_bytes(&self) -> usize {
        (self.cx as usize * self.bit_count as usize).div_ceil(32) * 4
    }

    /// Bytes of one colour-table entry in this header layout.
    pub fn entry_len(&self) -> usize {
        if self.header_len == OLD_HEADER_LEN { 3 } else { 4 }
    }

    /// Number of colour-table entries a format of this depth uses.
    pub fn palette_len(bit_count: u16) -> usize {
        if bit_count <= 8 { 1 << bit_count } else { 0 }
    }
}

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

/// The cbFix of a header starting with `first4` bytes, if it is valid.
pub fn header_len(first4: u32) -> Option<u32> {
    (first4 == OLD_HEADER_LEN || (MIN_HEADER2_LEN..=MAX_HEADER2_LEN).contains(&first4))
        .then_some(first4)
}

/// The longest RLE stream accepted for `cx` × `rows` pels: every pel as a
/// one-pel run of the widest kind (2 + 3 bytes), every row ended, padded.
pub fn max_rle_len(cx: u32, rows: u32) -> usize {
    (cx as usize * 5 + 4).saturating_mul(rows as usize).saturating_add(4)
}

/// Whether `compression` can be decoded for `bit_count`-bit pels.
fn compression_supported(compression: u32, bit_count: u16) -> bool {
    matches!((compression, bit_count), (BCA_UNCOMP, _) | (BCA_RLE8, 8) | (BCA_RLE4, 4) | (BCA_RLE24, 24))
}

/// Parse a bitmap header from `b` (the header followed by its colour table,
/// when `with_palette`). Only 1/4/8/24/32-bit, single-plane formats up to
/// [`MAX_SURFACE_DIM`] are accepted, uncompressed or RLE4 / RLE8 / RLE24.
pub fn parse_header(b: &[u8], with_palette: bool) -> Option<BitmapFormat> {
    let len = header_len(u32_at(b, 0)?)?;
    let field = |off: u32, size: u32| (off + size <= len).then_some(());
    let (cx, cy, planes, bit_count) = if len == OLD_HEADER_LEN {
        (u16_at(b, 4)? as u32, u16_at(b, 6)? as u32, u16_at(b, 8)?, u16_at(b, 10)?)
    } else {
        (u32_at(b, 4)?, u32_at(b, 8)?, u16_at(b, 12)?, u16_at(b, 14)?)
    };
    let compression = field(H2_COMPRESSION, 4)
        .and_then(|_| u32_at(b, H2_COMPRESSION as usize)).unwrap_or(BCA_UNCOMP);
    let image_len = field(H2_IMAGE_LEN, 4)
        .and_then(|_| u32_at(b, H2_IMAGE_LEN as usize)).unwrap_or(0);
    let clr_used = field(H2_CLR_USED, 4)
        .and_then(|_| u32_at(b, H2_CLR_USED as usize)).unwrap_or(0);
    let valid = planes == 1 && matches!(bit_count, 1 | 4 | 8 | 24 | 32)
        && compression_supported(compression, bit_count)
        && cx <= MAX_SURFACE_DIM && cy <= MAX_SURFACE_DIM;
    if !valid { return None; }
    let mut fmt = BitmapFormat { cx, cy, planes, bit_count, header_len: len, palette: Vec::new(),
                                 compression, image_len };
    if with_palette && bit_count <= 8 {
        let max = BitmapFormat::palette_len(bit_count);
        let n = if clr_used != 0 && (clr_used as usize) < max { clr_used as usize } else { max };
        let el = fmt.entry_len();
        for i in 0..n {
            let at = len as usize + i * el;
            let e = b.get(at..at + 3)?;
            fmt.palette.push((e[2] as u32) << 16 | (e[1] as u32) << 8 | e[0] as u32);
        }
    }
    Some(fmt)
}

/// Bytes a header of `len` plus its colour table take for `bit_count`.
pub fn info_len(len: u32, bit_count: u16) -> usize {
    let el = if len == OLD_HEADER_LEN { 3 } else { 4 };
    len as usize + BitmapFormat::palette_len(bit_count) * el
}

/// Decode one row of pel data into 0x00RRGGBB pels.
pub fn decode_row(fmt: &BitmapFormat, row: &[u8]) -> Vec<u32> {
    let pal = |i: usize| fmt.palette.get(i).copied().unwrap_or(0);
    (0..fmt.cx as usize).map(|x| match fmt.bit_count {
        1 => pal(((row.get(x / 8).copied().unwrap_or(0) >> (7 - x % 8)) & 1) as usize),
        4 => {
            let b = row.get(x / 2).copied().unwrap_or(0);
            pal((if x % 2 == 0 { b >> 4 } else { b & 0x0F }) as usize)
        }
        8 => pal(row.get(x).copied().unwrap_or(0) as usize),
        _ => {
            let step = fmt.bit_count as usize / 8;
            match row.get(x * step..x * step + 3) {
                Some(p) => (p[2] as u32) << 16 | (p[1] as u32) << 8 | p[0] as u32,
                None => 0,
            }
        }
    }).collect()
}

/// Encode 0x00RRGGBB `pels` as one row of `fmt` (1/4/8-bit pels take the
/// nearest colour of `fmt.palette`).
pub fn encode_row(fmt: &BitmapFormat, pels: &[u32]) -> Vec<u8> {
    let mut row = vec![0u8; fmt.row_bytes()];
    for (x, &p) in pels.iter().take(fmt.cx as usize).enumerate() {
        match fmt.bit_count {
            1 => row[x / 8] |= (nearest(&fmt.palette, p) as u8 & 1) << (7 - x % 8),
            4 => row[x / 2] |= (nearest(&fmt.palette, p) as u8 & 0x0F) << if x % 2 == 0 { 4 } else { 0 },
            8 => row[x] = nearest(&fmt.palette, p) as u8,
            _ => {
                let step = fmt.bit_count as usize / 8;
                row[x * step..x * step + 3].copy_from_slice(&[p as u8, (p >> 8) as u8, (p >> 16) as u8]);
            }
        }
    }
    row
}

/// Index of the palette colour nearest `rgb`.
pub fn nearest(palette: &[u32], rgb: u32) -> usize {
    let dist = |c: u32| -> i64 {
        (0..3).map(|i| {
            let d = ((c >> (8 * i)) & 0xFF) as i64 - ((rgb >> (8 * i)) & 0xFF) as i64;
            d * d
        }).sum()
    };
    palette.iter().enumerate().min_by_key(|&(i, &c)| (dist(c), i)).map_or(0, |(i, _)| i)
}

/// The 16 colours of the classic VGA palette (black first, white last).
const VGA_16: [u32; 16] = [
    0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xC0C0C0,
    0x808080, 0xFF0000, 0x00FF00, 0xFFFF00, 0x0000FF, 0xFF00FF, 0x00FFFF, 0xFFFFFF,
];

/// The colour table `GpiQueryBitmapBits` reports for `bit_count`: black and
/// white for 1 bit, the 16 VGA colours for 4 bits, and for 8 bits those 16
/// followed by a 6×6×6 colour cube and a grey ramp. (The PM default colour
/// table is not used: it has no pure black.)
pub fn standard_palette(bit_count: u16) -> Vec<u32> {
    let defaults = VGA_16.to_vec();
    match bit_count {
        1 => vec![0x0000_0000, 0x00FF_FFFF],
        4 => defaults,
        8 => {
            const LEVELS: [u32; 6] = [0x00, 0x33, 0x66, 0x99, 0xCC, 0xFF];
            let mut p = defaults;
            for r in LEVELS { for g in LEVELS { for b in LEVELS { p.push(r << 16 | g << 8 | b); } } }
            let grey = 256 - p.len();
            p.extend((0..grey as u32).map(|i| {
                let v = (i + 1) * 255 / (grey as u32 + 1);
                v << 16 | v << 8 | v
            }));
            p
        }
        _ => Vec::new(),
    }
}

/// Decode `rows` rows of bottom-up pel data, uncompressed or RLE. A `None`
/// pel was not given by the data (skipped by an RLE delta or end-of-row)
/// and keeps whatever the destination has.
pub fn decode_rows(fmt: &BitmapFormat, data: &[u8], rows: u32) -> Vec<Vec<Option<u32>>> {
    if fmt.compression != BCA_UNCOMP {
        return decode_rle(fmt, data, rows);
    }
    let rb = fmt.row_bytes();
    (0..rows as usize).map(|y| {
        let row = data.get(y * rb..(y + 1) * rb).unwrap_or(&[]);
        decode_row(fmt, row).into_iter().map(Some).collect()
    }).collect()
}

/// Decode an RLE4 / RLE8 / RLE24 stream (see the top of this file) into
/// `rows` rows of `fmt.cx` pels. Runs past the end of a row are cut off; a
/// truncated stream leaves the rest undecoded.
fn decode_rle(fmt: &BitmapFormat, data: &[u8], rows: u32) -> Vec<Vec<Option<u32>>> {
    let (w, h) = (fmt.cx as usize, rows as usize);
    let mut out = vec![vec![None; w]; h];
    let pal = |i: u8| fmt.palette.get(i as usize).copied().unwrap_or(0);
    let bgr = |b: &[u8]| (b[2] as u32) << 16 | (b[1] as u32) << 8 | b[0] as u32;
    let (mut x, mut y, mut i) = (0usize, 0usize, 0usize);
    let mut put = |x: &mut usize, y: usize, c: u32| {
        if *x < w && y < h { out[y][*x] = Some(c); }
        *x = x.saturating_add(1);
    };
    while y < h && i + 1 < data.len() {
        let (n, v) = (data[i] as usize, data[i + 1]);
        i += 2;
        if n > 0 {
            // A run of n pels.
            match fmt.compression {
                BCA_RLE8 => for _ in 0..n { put(&mut x, y, pal(v)); },
                BCA_RLE4 => for k in 0..n { put(&mut x, y, pal(if k % 2 == 0 { v >> 4 } else { v & 0x0F })); },
                _ => {
                    // RLE24: v is blue, the next two bytes green and red.
                    let Some(rest) = data.get(i..i + 2) else { break };
                    let c = bgr(&[v, rest[0], rest[1]]);
                    i += 2;
                    for _ in 0..n { put(&mut x, y, c); }
                }
            }
            continue;
        }
        match v {
            0 => { x = 0; y += 1; }
            1 => break,
            2 => {
                let Some(d) = data.get(i..i + 2) else { break };
                x = x.saturating_add(d[0] as usize);
                y += d[1] as usize;
                i += 2;
            }
            count => {
                // `count` literal pels, padded to a 16-bit boundary.
                let count = count as usize;
                let len = match fmt.compression {
                    BCA_RLE8 => count,
                    BCA_RLE4 => count.div_ceil(2),
                    _ => count * 3,
                };
                let Some(lit) = data.get(i..i + len) else { break };
                for k in 0..count {
                    let c = match fmt.compression {
                        BCA_RLE8 => pal(lit[k]),
                        BCA_RLE4 => pal(if k % 2 == 0 { lit[k / 2] >> 4 } else { lit[k / 2] & 0x0F }),
                        _ => bgr(&lit[k * 3..k * 3 + 3]),
                    };
                    put(&mut x, y, c);
                }
                i += len + len % 2;
            }
        }
    }
    out
}

/// An RT_BITMAP resource: a bitmap file ("BM") or a bitmap array ("BA")
/// whose first entry is used. Returns the format and its pel data, which
/// holds every row when uncompressed.
pub fn bitmap_resource_parts(res: &[u8]) -> Option<(BitmapFormat, &[u8])> {
    let base = match u16_at(res, 0)? {
        BFT_BMAP => 0,
        BFT_BITMAPARRAY => ARRAY_HEADER_LEN,
        _ => return None,
    };
    if u16_at(res, base)? != BFT_BMAP { return None; }
    // offBits counts from the start of the whole file (the resource).
    let off_bits = u32_at(res, base + 10)? as usize;
    let fmt = parse_header(res.get(base + FILE_HEADER_LEN..)?, true)?;
    let data = res.get(off_bits..)?;
    if fmt.compression == BCA_UNCOMP && data.len() < fmt.row_bytes() * fmt.cy as usize { return None; }
    Some((fmt, data))
}
