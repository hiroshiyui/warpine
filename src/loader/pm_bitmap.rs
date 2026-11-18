// SPDX-License-Identifier: GPL-3.0-only
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
// The codec works on byte slices. Below it, the GPI / DEV entry points for
// device contexts, bitmaps and bit-block transfers (dispatched from
// `pm_gpi.rs`, `WinDrawBitmap` from `pm_win.rs`) copy data in and out of
// guest memory with the bounds-checked helpers.

use super::constants;
use crate::gui::surface::{MAX_SURFACE_DIM, Surface};

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
const FILE_HEADER_LEN: usize = 14;
const ARRAY_HEADER_LEN: usize = 14;

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

/// Write decoded `rows` into `s` from PM row `start` up; `None` pels keep
/// their value.
pub fn put_rows(s: &mut Surface, start: i32, rows: &[Vec<Option<u32>>]) {
    for (i, row) in rows.iter().enumerate() {
        let y = start + i as i32;
        let Some(mut pels) = s.row_pels(y) else { continue };
        for (d, p) in pels.iter_mut().zip(row) {
            if let Some(p) = p { *d = *p; }
        }
        s.put_row(y, &pels);
    }
}

/// Decode `fmt.cy` rows of bottom-up pel data into a new surface.
pub fn decode_surface(fmt: &BitmapFormat, data: &[u8]) -> Surface {
    let mut s = Surface::new(fmt.cx, fmt.cy);
    put_rows(&mut s, 0, &decode_rows(fmt, data, fmt.cy));
    s
}

/// An RT_BITMAP resource: a bitmap file ("BM") or a bitmap array ("BA")
/// whose first entry is used. Returns the format and the decoded pels.
pub fn parse_bitmap_resource(res: &[u8]) -> Option<(BitmapFormat, Surface)> {
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
    let surface = decode_surface(&fmt, data);
    Some((fmt, surface))
}

// ── The GPI / DEV entry points (dispatched from pm_gpi.rs / pm_win.rs) ──────

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::{DeviceContext, DrawTarget};
use crate::gui::surface::{DrawOp, SharedSurface};

/// Guest coordinates for bit-block transfers are clamped to ± this, far
/// outside any surface, so the scaling arithmetic cannot overflow.
const BLT_COORD_LIMIT: i64 = 1 << 24;
/// Points per call: target rectangle, then source lower-left / rectangle.
const BLT_MAX_POINTS: u32 = 4;
/// Pels per metre at the 96-dpi resolution Warpine reports.
const PELS_PER_METRE_96DPI: u32 = 3780;
/// Colours of the 24-bit true-colour device.
const TRUE_COLORS: u32 = 1 << 24;
const MOUSE_BUTTONS: u32 = 3;
const TRUE_COLOR_BITS: u32 = 24;
/// The bitmap formats (planes, bits per pel) the display accepts, the
/// device's own first (`GpiQueryDeviceBitmapFormats`).
const DEVICE_BITMAP_FORMATS: [(u32, u32); 5] = [(1, 24), (1, 1), (1, 4), (1, 8), (1, 32)];

/// Where a blit takes its source pels from.
enum BlitSource<'a> {
    None,
    Shared(SharedSurface),
    Local(&'a Surface),
}

/// A rectangle as `(x0, y0, x1, y1)`, x1 / y1 exclusive.
pub(super) type Rect64 = (i64, i64, i64, i64);

/// Order the corners of `(x0, y0, x1, y1)` and clamp them to
/// ±`BLT_COORD_LIMIT`, so the scaling arithmetic cannot overflow.
fn normalized((x0, y0, x1, y1): Rect64) -> Rect64 {
    let c = |v: i64| v.clamp(-BLT_COORD_LIMIT, BLT_COORD_LIMIT);
    (c(x0.min(x1)), c(y0.min(y1)), c(x0.max(x1)), c(y0.max(y1)))
}

/// Whether ternary raster operation `rop` depends on the source.
fn rop_uses_source(rop: u8) -> bool {
    (rop >> 2) & 0x33 != rop & 0x33
}

impl super::Loader {
    /// Read `n` POINTLs at `aptl`, clamped to ±`BLT_COORD_LIMIT`.
    fn read_points(&self, aptl: u32, n: u32) -> Option<Vec<(i64, i64)>> {
        let c = |v: i32| (v as i64).clamp(-BLT_COORD_LIMIT, BLT_COORD_LIMIT);
        (0..n).map(|i| {
            let at = aptl.checked_add(8 * i)?;
            Some((c(self.guest_read::<i32>(at)?), c(self.guest_read::<i32>(at.checked_add(4)?)?)))
        }).collect()
    }

    /// Read a bitmap header (and with `with_palette`, its colour table).
    fn read_format(&self, addr: u32, with_palette: bool) -> Option<BitmapFormat> {
        let len = header_len(self.guest_read::<u32>(addr)?)?;
        let fmt = parse_header(&self.guest_read_bytes(addr, len as usize)?, false)?;
        if !with_palette || fmt.bit_count > 8 { return Some(fmt); }
        parse_header(&self.guest_read_bytes(addr, info_len(len, fmt.bit_count))?, true)
    }

    /// Write cx / cy / cPlanes / cBitCount into a header of `len` bytes at
    /// `addr` (old or new layout), zeroing the rest of it.
    fn write_header(&self, addr: u32, len: u32, (cx, cy): (u32, u32), planes: u16, bits: u16) -> Option<()> {
        self.guest_write_bytes(addr.checked_add(4)?, &vec![0; len as usize - 4])?;
        if len == OLD_HEADER_LEN {
            self.guest_write::<u16>(addr + 4, cx as u16)?;
            self.guest_write::<u16>(addr + 6, cy as u16)?;
            self.guest_write::<u16>(addr + 8, planes)?;
            self.guest_write::<u16>(addr + 10, bits)
        } else {
            self.guest_write::<u32>(addr + 4, cx)?;
            self.guest_write::<u32>(addr + 8, cy)?;
            self.guest_write::<u16>(addr + 12, planes)?;
            self.guest_write::<u16>(addr + 14, bits)
        }
    }

    // ── Device contexts ──────────────────────────────────────────────────────

    /// DevOpenDC: memory (`OD_MEMORY`) and information (`OD_INFO`) DCs.
    /// Printers (`OD_QUEUED`, `OD_DIRECT`: no printer driver exists,
    /// `PMERR_DRIVER_NOT_FOUND`; see `pmspl.rs`), metafiles and the rest
    /// (`PMERR_INV_DC_TYPE`) fail with `DEV_ERROR`.
    pub(super) fn dev_open_dc(&self, tid: u32, kind: u32) -> u32 {
        use super::pmspl::{OD_DIRECT, OD_QUEUED, PMERR_DRIVER_NOT_FOUND, PMERR_INV_DC_TYPE};
        let dc = match kind {
            OD_MEMORY => DeviceContext::Memory,
            OD_INFO => DeviceContext::Info,
            OD_QUEUED | OD_DIRECT => {
                log::info!("  DevOpenDC: printing is not supported (no printer)");
                self.pm_error(tid, PMERR_DRIVER_NOT_FOUND);
                return DEV_ERROR;
            }
            _ => {
                log::debug!("  DevOpenDC: device type {kind} not supported");
                self.pm_error(tid, PMERR_INV_DC_TYPE);
                return DEV_ERROR;
            }
        };
        self.shared.window_mgr.lock_or_recover().open_dc(dc)
    }

    /// DevCloseDC: presentation spaces still associated with it lose the
    /// association (and their bitmap).
    pub(super) fn dev_close_dc(&self, hdc: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(dc) = wm.dcs.remove(&hdc) else { return DEV_ERROR };
        for ps in wm.ps_map.values_mut().filter(|ps| ps.hdc == hdc) {
            ps.hdc = 0;
            ps.bitmap = 0;
            if matches!(dc, DeviceContext::Window(_)) { ps.hwnd = 0; }
        }
        DEV_OK
    }

    /// The `CAPS_*` value Warpine's display reports (a 24-bit true-colour
    /// device the size of the desktop at 96 dpi); 0 for the rest.
    fn device_cap(&self, index: u32) -> u32 {
        let (w, h) = constants::desktop_size();
        let m = crate::gui::font::metrics(crate::gui::font::FontSpec::SYSTEM);
        let (char_w, char_h) = (m.avg_char_width.max(1) as u32, m.line_height.max(1) as u32);
        match index {
            CAPS_WIDTH => w,
            CAPS_HEIGHT => h,
            CAPS_WIDTH_IN_CHARS => w / char_w,
            CAPS_HEIGHT_IN_CHARS => h / char_h,
            CAPS_HORIZONTAL_RESOLUTION | CAPS_VERTICAL_RESOLUTION => PELS_PER_METRE_96DPI,
            CAPS_CHAR_WIDTH => char_w,
            CAPS_CHAR_HEIGHT => char_h,
            CAPS_COLORS | CAPS_PHYS_COLORS => TRUE_COLORS,
            CAPS_COLOR_PLANES => 1,
            CAPS_COLOR_BITCOUNT => TRUE_COLOR_BITS,
            CAPS_MOUSE_BUTTONS => MOUSE_BUTTONS,
            CAPS_BITMAP_FORMATS => DEVICE_BITMAP_FORMATS.len() as u32,
            CAPS_FAMILY => 0,
            _ => 0,
        }
    }

    /// DevQueryCaps: `count` CAPS_* values from index `start` into `array`.
    pub(super) fn dev_query_caps(&self, hdc: u32, start: u32, count: u32, array: u32) -> u32 {
        if !self.shared.window_mgr.lock_or_recover().dcs.contains_key(&hdc) { return GPI_ERROR; }
        for i in 0..count.min(CAPS_COUNT) {
            let at = array.wrapping_add(4 * i);
            if self.guest_write::<u32>(at, self.device_cap(start.wrapping_add(i))).is_none() {
                return GPI_ERROR;
            }
        }
        GPI_OK
    }

    /// GpiQueryDeviceBitmapFormats: up to `count` LONGs of (planes, bits per
    /// pel) pairs, the device's preferred format first.
    pub(super) fn gpi_query_device_bitmap_formats(&self, hps: u32, count: u32, array: u32) -> u32 {
        if (count as i32) < 0 || !self.shared.window_mgr.lock_or_recover().ps_map.contains_key(&hps) {
            return GPI_ERROR;
        }
        let values = DEVICE_BITMAP_FORMATS.iter().flat_map(|&(planes, bits)| [planes, bits]);
        for (i, v) in values.take(count as usize).enumerate() {
            if self.guest_write::<u32>(array.wrapping_add(4 * i as u32), v).is_none() { return GPI_ERROR; }
        }
        GPI_OK
    }

    /// GpiAssociate: associate `hps` with `hdc`, or (hdc 0) dissociate it.
    pub(super) fn gpi_associate(&self, hps: u32, hdc: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let dc = match hdc {
            0 => None,
            h => match wm.dcs.get(&h) { Some(&dc) => Some(dc), None => return GPI_ERROR },
        };
        let Some(old_hdc) = wm.ps_map.get(&hps).map(|ps| ps.hdc) else { return GPI_ERROR };
        let old = wm.dcs.get(&old_hdc).copied();
        let ps = wm.ps_map.get_mut(&hps).expect("checked above");
        if matches!(old, Some(DeviceContext::Window(_))) { ps.hwnd = 0; }
        ps.bitmap = 0;
        ps.hdc = hdc;
        if let Some(DeviceContext::Window(w)) = dc { ps.hwnd = w; }
        GPI_OK
    }

    // ── Bitmaps ──────────────────────────────────────────────────────────────

    /// GpiCreateBitmap: a bitmap of the size and format in the header at
    /// `pbmp`, with `CBM_INIT` initialised from `init` in the format at `pbmi`.
    pub(super) fn gpi_create_bitmap(&self, pbmp: u32, opts: u32, init: u32, pbmi: u32) -> u32 {
        let Some(fmt) = self.read_format(pbmp, false) else { return GPI_ERROR };
        if fmt.cx == 0 || fmt.cy == 0 { return GPI_ERROR; }
        let mut surface = Surface::new(fmt.cx, fmt.cy);
        if opts & CBM_INIT != 0 && init != 0 {
            let Some(src_fmt) = self.read_format(pbmi, true) else { return GPI_ERROR };
            let Some(len) = src_fmt.data_len(src_fmt.cy) else { return GPI_ERROR };
            let Some(data) = self.guest_read_bytes(init, len) else { return GPI_ERROR };
            put_rows(&mut surface, 0, &decode_rows(&src_fmt, &data, src_fmt.cy.min(fmt.cy)));
        }
        self.shared.window_mgr.lock_or_recover().create_bitmap(surface, fmt.bit_count, fmt.planes)
    }

    /// GpiLoadBitmap: the RT_BITMAP resource `id` of module `hmod` (0 = the
    /// executable, else a loaded DLL), stretched to `cx` × `cy` when both are
    /// non-zero.
    pub(super) fn gpi_load_bitmap(&self, hmod: u32, id: u32, cx: i32, cy: i32) -> u32 {
        let Ok(id) = u16::try_from(id) else { return GPI_ERROR };
        let res = self.shared.resource_mgr.lock_or_recover().find(hmod, crate::lx::header::RT_BITMAP, id);
        let Some((addr, size)) = res else { return GPI_ERROR };
        let Some(bytes) = self.guest_read_bytes(addr, size as usize) else { return GPI_ERROR };
        let Some((fmt, mut surface)) = parse_bitmap_resource(&bytes) else { return GPI_ERROR };
        if cx > 0 && cy > 0 && (cx as u32, cy as u32) != (fmt.cx, fmt.cy) {
            surface = stretched(&surface, cx as u32, cy as u32);
        }
        self.shared.window_mgr.lock_or_recover().create_bitmap(surface, fmt.bit_count, fmt.planes)
    }

    /// GpiDeleteBitmap: fails while the bitmap is selected into a PS.
    pub(super) fn gpi_delete_bitmap(&self, hbm: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if wm.bitmap_owner(hbm).is_some() { return GPI_ERROR; }
        wm.bitmaps.remove(&hbm).map_or(GPI_ERROR, |_| GPI_OK)
    }

    /// GpiSetBitmap: select `hbm` (0 deselects) into a PS associated with a
    /// memory DC; returns the previously selected bitmap. A bitmap can be
    /// selected into one PS at a time.
    pub(super) fn gpi_set_bitmap(&self, hps: u32, hbm: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(hdc) = wm.ps_map.get(&hps).map(|ps| ps.hdc) else { return HBM_ERROR };
        if wm.dcs.get(&hdc) != Some(&DeviceContext::Memory) { return HBM_ERROR; }
        if hbm != 0 {
            if !wm.bitmaps.contains_key(&hbm) { return HBM_ERROR; }
            if wm.bitmap_owner(hbm).is_some_and(|o| o != hps) { return HBM_ERROR; }
        }
        let ps = wm.ps_map.get_mut(&hps).expect("checked above");
        std::mem::replace(&mut ps.bitmap, hbm)
    }

    /// GpiQueryBitmapInfoHeader (`old` = false) / GpiQueryBitmapParameters
    /// (`old` = true, the 12-byte BITMAPINFOHEADER).
    pub(super) fn query_bitmap_header(&self, hbm: u32, pbmp: u32, old: bool) -> u32 {
        let info = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.bitmaps.get(&hbm).map(|b| {
                let s = b.surface.lock_or_recover();
                ((s.width, s.height), b.planes, b.bit_count)
            })
        };
        let Some((size, planes, bits)) = info else { return GPI_ERROR };
        let len = if old {
            OLD_HEADER_LEN
        } else {
            match self.guest_read::<u32>(pbmp).and_then(header_len) { Some(l) => l, None => return GPI_ERROR }
        };
        if old && self.guest_write::<u32>(pbmp, OLD_HEADER_LEN).is_none() { return GPI_ERROR; }
        self.write_header(pbmp, len, size, planes, bits).map_or(GPI_ERROR, |_| GPI_OK)
    }

    /// GpiSetBitmapBits (`set`) / GpiQueryBitmapBits: copy `scans` rows from
    /// row `start` between the bitmap selected into `hps` and `buf`, in the
    /// format described at `pbmi`. A query fills in that header's size and
    /// colour table. Returns the rows copied, or `BMB_ERROR`.
    pub(super) fn bitmap_bits(&self, hps: u32, start: u32, scans: u32, buf: u32, pbmi: u32, set: bool) -> u32 {
        let err = BMB_ERROR as u32;
        let surface = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(hbm) = wm.ps_map.get(&hps).map(|ps| ps.bitmap) else { return err };
            match wm.bitmaps.get(&hbm) { Some(b) => b.surface.clone(), None => return err }
        };
        let (bw, bh) = { let s = surface.lock_or_recover(); (s.width, s.height) };
        let (Ok(start_i), Ok(_)) = (i32::try_from(start), i32::try_from(scans)) else { return err };
        let n = scans.min(bh.saturating_sub(start));
        let fmt = if set {
            match self.read_format(pbmi, true) { Some(f) => f, None => return err }
        } else {
            let Some(len) = self.guest_read::<u32>(pbmi).and_then(header_len) else { return err };
            let bits_at = if len == OLD_HEADER_LEN { 10 } else { 14 };
            let Some(bits) = self.guest_read::<u16>(pbmi.wrapping_add(bits_at)) else { return err };
            if !matches!(bits, 1 | 4 | 8 | 24 | 32) { return err; }
            BitmapFormat::uncompressed(bw, bh, bits, len, standard_palette(bits))
        };
        if set {
            let Some(len) = fmt.data_len(n) else { return err };
            let Some(data) = self.guest_read_bytes(buf, len) else { return err };
            put_rows(&mut surface.lock_or_recover(), start_i, &decode_rows(&fmt, &data, n));
        } else {
            // Always returned uncompressed; the header says so (BCA_UNCOMP).
            if self.write_header(pbmi, fmt.header_len, (bw, bh), 1, fmt.bit_count).is_none() { return err; }
            let el = fmt.entry_len();
            let mut table = Vec::with_capacity(fmt.palette.len() * el);
            for &c in &fmt.palette {
                table.extend_from_slice(&[c as u8, (c >> 8) as u8, (c >> 16) as u8, 0][..el]);
            }
            if self.guest_write_bytes(pbmi.wrapping_add(fmt.header_len), &table).is_none() { return err; }
            if buf != 0 {
                let out: Vec<u8> = {
                    let s = surface.lock_or_recover();
                    (0..n as i32).flat_map(|i| encode_row(&fmt, &s.row_pels(start_i + i).unwrap_or_default()))
                        .collect()
                };
                if self.guest_write_bytes(buf, &out).is_none() { return err; }
            }
        }
        n
    }

    // ── Bit-block transfers ──────────────────────────────────────────────────

    /// Transfer `src_rect` of `src` onto `target` (both exclusive at the top
    /// and right) of the surface `hps` draws into, with raster operation
    /// `rop`; the pattern is the PS's area pattern (`GpiSetPattern`) in its
    /// foreground and background colours. Different sizes are
    /// stretched or compressed by nearest-pel sampling.
    fn blit(&self, hps: u32, target: Rect64, src: BlitSource, src_rect: Rect64, rop: u32) -> u32 {
        let Ok(rop) = u8::try_from(rop) else { return GPI_ERROR };
        let Some(pt) = self.ps_target(hps) else { return GPI_ERROR };
        if matches!(src, BlitSource::None) && rop_uses_source(rop) { return GPI_ERROR; }
        let Some(surface) = self.shared.window_mgr.lock_or_recover().target_surface(pt.target)
            else { return GPI_OK };
        let (w, h) = { let s = surface.lock_or_recover(); (s.width as i64, s.height as i64) };
        let (tx0, ty0, tx1, ty1) = normalized(target);
        let (sx0, sy0, sx1, sy1) = normalized(src_rect);
        let (tw, th, sw, sh) = (tx1 - tx0, ty1 - ty0, sx1 - sx0, sy1 - sy0);
        let (cx0, cy0, cx1, cy1) = (tx0.max(0), ty0.max(0), tx1.min(w), ty1.min(h));
        if tw <= 0 || th <= 0 || cx0 >= cx1 || cy0 >= cy1 { return GPI_OK; }
        let sample = |s: &Surface| -> Vec<Option<u32>> {
            (cy0..cy1).flat_map(|y| (cx0..cx1).map(move |x| (x, y))).map(|(x, y)| {
                let sx = sx0 + (x - tx0) * sw / tw;
                let sy = sy0 + (y - ty0) * sh / th;
                s.pel(i32::try_from(sx).ok()?, i32::try_from(sy).ok()?)
            }).collect()
        };
        let pels = match src {
            BlitSource::None => None,
            BlitSource::Shared(s) => Some(sample(&s.lock_or_recover())),
            BlitSource::Local(s) => Some(sample(s)),
        };
        self.draw_ps(&pt, DrawOp::Blit {
            x: cx0 as i32, y: cy0 as i32, w: (cx1 - cx0) as u32, h: (cy1 - cy0) as u32,
            src: pels, rop, pattern: pt.pattern,
        });
        GPI_OK
    }

    /// The inclusive device rectangle of an inclusive world target
    /// rectangle (GpiWCBitBlt, GpiDrawBits), normalised: its corners mapped
    /// by the PS's transforms. A rotated or sheared target becomes its
    /// upright bounding box ("if any shear or rotation has occurred … the
    /// result is a rectangle that bounds the transformed shape"); the
    /// image itself is not rotated (Warpine's approximation).
    fn world_target(&self, hps: u32, rect: Rect64) -> Rect64 {
        let (x0, y0, x1, y1) = normalized(rect);
        let xform = self.ps_xform(hps);
        if xform.identity { return (x0, y0, x1, y1); }
        let (a, b, c, d) = xform.bounds((x0 as i32, y0 as i32), (x1 as i32, y1 as i32));
        normalized((a as i64, b as i64, c as i64, d as i64))
    }

    /// The surface of bitmap `hbm`.
    fn bitmap_surface(&self, hbm: u32) -> Option<SharedSurface> {
        self.shared.window_mgr.lock_or_recover().bitmaps.get(&hbm).map(|b| b.surface.clone())
    }

    /// GpiBitBlt: 2 points (target only, for operations without a source),
    /// 3 (plus the source's lower-left corner, same size) or 4 (plus the
    /// source rectangle; stretched). Device coordinates, rectangles
    /// exclusive at the top and right.
    pub(super) fn gpi_bit_blt(&self, hps_t: u32, hps_s: u32, count: u32, aptl: u32, rop: u32) -> u32 {
        if !(2..=BLT_MAX_POINTS).contains(&count) { return GPI_ERROR; }
        let Some(p) = self.read_points(aptl, count) else { return GPI_ERROR };
        let target = normalized((p[0].0, p[0].1, p[1].0, p[1].1));
        let src_rect = match count {
            2 => target,
            3 => (p[2].0, p[2].1, p[2].0 + target.2 - target.0, p[2].1 + target.3 - target.1),
            _ => (p[2].0, p[2].1, p[3].0, p[3].1),
        };
        let src = if count == 2 {
            BlitSource::None
        } else {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            match wm.ps_draw_target(hps_s) {
                Some(t) if t != DrawTarget::None => match wm.target_surface(t) {
                    Some(s) => BlitSource::Shared(s),
                    None => return GPI_ERROR,
                },
                _ => return GPI_ERROR,
            }
        };
        self.blit(hps_t, target, src, src_rect, rop)
    }

    /// GpiWCBitBlt: like GpiBitBlt from bitmap `hbm`, but the target
    /// rectangle is in world coordinates and inclusive.
    pub(super) fn gpi_wc_bit_blt(&self, hps: u32, hbm: u32, count: u32, aptl: u32, rop: u32) -> u32 {
        if !(3..=BLT_MAX_POINTS).contains(&count) { return GPI_ERROR; }
        let Some(p) = self.read_points(aptl, count) else { return GPI_ERROR };
        let Some(surface) = self.bitmap_surface(hbm) else { return GPI_ERROR };
        let (x0, y0, x1, y1) = self.world_target(hps, (p[0].0, p[0].1, p[1].0, p[1].1));
        let target = (x0, y0, x1 + 1, y1 + 1);
        let src_rect = if count == 3 {
            (p[2].0, p[2].1, p[2].0 + target.2 - x0, p[2].1 + target.3 - y0)
        } else {
            (p[2].0, p[2].1, p[3].0, p[3].1)
        };
        self.blit(hps, target, BlitSource::Shared(surface), src_rect, rop)
    }

    /// GpiDrawBits: draw bitmap data at `bits` (format at `pbmi`) like
    /// GpiWCBitBlt — target inclusive, source exclusive.
    pub(super) fn gpi_draw_bits(&self, hps: u32, bits: u32, pbmi: u32, count: u32, aptl: u32, rop: u32) -> u32 {
        if !(3..=BLT_MAX_POINTS).contains(&count) { return GPI_ERROR; }
        let Some(p) = self.read_points(aptl, count) else { return GPI_ERROR };
        let Some(fmt) = self.read_format(pbmi, true) else { return GPI_ERROR };
        let Some(len) = fmt.data_len(fmt.cy) else { return GPI_ERROR };
        let Some(data) = self.guest_read_bytes(bits, len) else { return GPI_ERROR };
        let image = decode_surface(&fmt, &data);
        let (x0, y0, x1, y1) = self.world_target(hps, (p[0].0, p[0].1, p[1].0, p[1].1));
        let target = (x0, y0, x1 + 1, y1 + 1);
        let src_rect = if count == 3 {
            (p[2].0, p[2].1, p[2].0 + target.2 - x0, p[2].1 + target.3 - y0)
        } else {
            (p[2].0, p[2].1, p[3].0, p[3].1)
        };
        self.blit(hps, target, BlitSource::Local(&image), src_rect, rop)
    }

    /// WinDrawBitmap: draw `src` of bitmap `hbm` (the whole bitmap when
    /// `None`) at `dst`; both exclusive. Returns TRUE / FALSE.
    pub(super) fn draw_bitmap(&self, hps: u32, hbm: u32, src: Option<Rect64>, dst: Rect64, rop: u32) -> u32 {
        let Some(surface) = self.bitmap_surface(hbm) else { return GPI_ERROR };
        let (w, h) = { let s = surface.lock_or_recover(); (s.width as i64, s.height as i64) };
        let src = src.unwrap_or((0, 0, w, h));
        self.blit(hps, dst, BlitSource::Shared(surface), src, rop)
    }
}

/// `src` resampled (nearest pel) to `w` × `h`.
fn stretched(src: &Surface, w: u32, h: u32) -> Surface {
    let mut out = Surface::new(w, h);
    let (w, h) = (out.width as i64, out.height as i64);
    let (sw, sh) = (src.width as i64, src.height as i64);
    for y in 0..h {
        let row: Vec<u32> = (0..w).map(|x| src.pel((x * sw / w) as i32, (y * sh / h) as i32).unwrap_or(0)).collect();
        out.put_row(y as i32, &row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header2(cx: u32, cy: u32, bits: u16, palette: &[u32]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(MIN_HEADER2_LEN.to_le_bytes());
        b.extend(cx.to_le_bytes());
        b.extend(cy.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(bits.to_le_bytes());
        for &c in palette {
            b.extend([c as u8, (c >> 8) as u8, (c >> 16) as u8, 0]);
        }
        b
    }

    #[test]
    fn headers_of_both_layouts_parse() {
        let h = header2(3, 2, 4, &[0x0011_2233; 16]);
        let f = parse_header(&h, true).unwrap();
        assert_eq!((f.cx, f.cy, f.bit_count, f.header_len), (3, 2, 4, 16));
        assert_eq!(f.palette, vec![0x0011_2233; 16]);
        assert_eq!(f.row_bytes(), 4);

        let mut old = Vec::new();
        old.extend(12u32.to_le_bytes());
        for v in [33u16, 5, 1, 1] { old.extend(v.to_le_bytes()); }
        old.extend([0, 0, 0, 0xFF, 0xFF, 0xFF]);
        let f = parse_header(&old, true).unwrap();
        assert_eq!((f.cx, f.cy, f.bit_count, f.entry_len()), (33, 5, 1, 3));
        assert_eq!(f.palette, vec![0, 0x00FF_FFFF]);
        assert_eq!(f.row_bytes(), 8, "33 bits round up to two ULONGs");
    }

    #[test]
    fn unsupported_headers_are_rejected() {
        assert!(parse_header(&header2(2, 2, 16, &[]), false).is_none(), "16 bits");
        assert!(parse_header(&header2(MAX_SURFACE_DIM + 1, 1, 24, &[]), false).is_none());
        let mut h = header2(2, 2, 24, &[]);
        h[12] = 2; // two planes
        assert!(parse_header(&h, false).is_none());
        let mut h = header2(2, 2, 24, &[]);
        h[0] = 13; // bad cbFix
        assert!(parse_header(&h, false).is_none());
        assert!(parse_header(&header2(2, 2, 8, &[1, 2]), true).is_none(), "palette cut short");
        assert!(parse_header(&[1, 2], false).is_none());
    }

    #[test]
    fn rows_round_trip_in_every_depth() {
        let pels = [0x0000_0000, 0x00FF_FFFF, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF];
        for bits in [1u16, 4, 8, 24, 32] {
            let palette = standard_palette(bits);
            let fmt = BitmapFormat::uncompressed(5, 1, bits, 16, palette);
            let row = encode_row(&fmt, &pels);
            assert_eq!(row.len(), fmt.row_bytes());
            let back = decode_row(&fmt, &row);
            let want: Vec<u32> = if bits == 1 {
                vec![0, 0x00FF_FFFF, 0, 0, 0] // nearest of black / white
            } else { pels.to_vec() };
            assert_eq!(back, want, "{bits} bits");
        }
    }

    #[test]
    fn standard_palettes_have_the_right_sizes() {
        assert_eq!(standard_palette(1).len(), 2);
        assert_eq!(standard_palette(4).len(), 16);
        assert_eq!(standard_palette(8).len(), 256);
        assert!(standard_palette(24).is_empty());
    }

    /// A "BM" file and the same file inside a "BA" array decode to the same
    /// pels; offBits counts from the start of the resource.
    #[test]
    fn bitmap_resources_decode() {
        // 2×2, 24 bits: bottom row red, blue; top row green, white.
        let info = header2(2, 2, 24, &[]);
        let rows: [[u8; 8]; 2] = [[0, 0, 255, 255, 0, 0, 0, 0], [0, 255, 0, 255, 255, 255, 0, 0]];
        let file = |prefix: usize| {
            let off_bits = (prefix + FILE_HEADER_LEN + info.len()) as u32;
            let mut f = Vec::new();
            f.extend(BFT_BMAP.to_le_bytes());
            f.extend(0u32.to_le_bytes());
            f.extend([0u8; 4]);
            f.extend(off_bits.to_le_bytes());
            f.extend(&info);
            f.extend(rows.concat());
            f
        };
        let mut array = Vec::new();
        array.extend(BFT_BITMAPARRAY.to_le_bytes());
        array.extend([0u8; 12]);
        array.extend(file(ARRAY_HEADER_LEN));
        for res in [file(0), array] {
            let (fmt, s) = parse_bitmap_resource(&res).unwrap();
            assert_eq!((fmt.cx, fmt.cy), (2, 2));
            assert_eq!(s.row_pels(0), Some(vec![0x00FF_0000, 0x0000_00FF]));
            assert_eq!(s.row_pels(1), Some(vec![0x0000_FF00, 0x00FF_FFFF]));
        }
        assert!(parse_bitmap_resource(b"XX").is_none());
        assert!(parse_bitmap_resource(&file(0)[..40]).is_none(), "truncated pel data");
    }
}

/// The entry points, called through the PMGPI / PMWIN dispatchers as a guest
/// would call them.
#[cfg(test)]
mod rle_tests {
    use super::*;

    const PAL: [u32; 4] = [0x000000, 0xFF0000, 0x00FF00, 0x0000FF];

    fn fmt(cx: u32, bits: u16, compression: u32) -> BitmapFormat {
        BitmapFormat { compression, image_len: 0, ..BitmapFormat::uncompressed(cx, 0, bits, 16, PAL.to_vec()) }
    }

    fn rows(f: &BitmapFormat, data: &[u8], n: u32) -> Vec<Vec<Option<u32>>> {
        decode_rows(f, data, n)
    }

    const K: Option<u32> = Some(0x000000);
    const R: Option<u32> = Some(0xFF0000);
    const G: Option<u32> = Some(0x00FF00);
    const B: Option<u32> = Some(0x0000FF);

    #[test]
    fn rle8_runs_end_of_line_and_end_of_bitmap() {
        let f = fmt(5, 8, BCA_RLE8);
        // Row 0: 3 red, 2 green. Row 1: 1 blue, end of line. End.
        let data = [3, 1, 2, 2, 0, 0, 1, 3, 0, 0, 0, 1];
        assert_eq!(rows(&f, &data, 3), vec![
            vec![R, R, R, G, G],
            vec![B, None, None, None, None],
            vec![None; 5],
        ]);
    }

    #[test]
    fn rle8_delta_and_padded_literals() {
        let f = fmt(6, 8, BCA_RLE8);
        // Delta right 2, up 1; then 3 literal pels (odd: one pad byte);
        // then a run of 1 black.
        let data = [0, 2, 2, 1, 0, 3, 1, 2, 3, 0xEE, 1, 0, 0, 1];
        assert_eq!(rows(&f, &data, 2), vec![
            vec![None; 6],
            vec![None, None, R, G, B, K],
        ]);
    }

    #[test]
    fn rle4_alternates_nibbles() {
        let f = fmt(5, 4, BCA_RLE4);
        // Run of 5 from 0x12: 1,2,1,2,1. Then end of line; 3 literal
        // nibbles 0x30 0x10 → 3,0,1 (2 bytes, already even).
        let data = [5, 0x12, 0, 0, 0, 3, 0x30, 0x10, 0, 1];
        assert_eq!(rows(&f, &data, 2), vec![
            vec![R, G, R, G, R],
            vec![B, K, R, None, None],
        ]);
    }

    #[test]
    fn rle24_values_are_blue_green_red() {
        let f = fmt(4, 24, BCA_RLE24);
        // Run of 2 of (b=0x30, g=0x20, r=0x10); 3 literal pels (9 bytes,
        // one pad byte).
        let data = [2, 0x30, 0x20, 0x10, 0, 3, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 1];
        let out = rows(&f, &data, 2);
        assert_eq!(out[0][..2], [Some(0x102030), Some(0x102030)]);
        assert_eq!(out[0][2..], [Some(0x030201), Some(0x060504)], "the third pel falls off the row");
    }

    #[test]
    fn malformed_streams_are_safe() {
        let f = fmt(4, 8, BCA_RLE8);
        for data in [&[255u8, 1][..], &[0, 2][..], &[0, 200, 1][..], &[0, 2, 255, 255, 3, 1][..], &[7][..], &[][..]] {
            let out = rows(&f, data, 3);
            assert_eq!(out.len(), 3);
            assert!(out.iter().all(|r| r.len() == 4));
        }
        // A run longer than the row is cut off at its end.
        assert_eq!(rows(&f, &[255, 1], 1), vec![vec![R, R, R, R]]);
    }

    #[test]
    fn headers_accept_matching_rle_formats_only() {
        let header = |bits: u16, compression: u32, image_len: u32| {
            let mut b = Vec::new();
            b.extend(24u32.to_le_bytes());
            b.extend(4u32.to_le_bytes());
            b.extend(2u32.to_le_bytes());
            b.extend(1u16.to_le_bytes());
            b.extend(bits.to_le_bytes());
            b.extend(compression.to_le_bytes());
            b.extend(image_len.to_le_bytes());
            b
        };
        let f = parse_header(&header(8, BCA_RLE8, 10), false).expect("RLE8 at 8 bits");
        assert_eq!((f.compression, f.image_len, f.data_len(2)), (BCA_RLE8, 10, Some(10)));
        assert!(parse_header(&header(4, BCA_RLE4, 10), false).is_some());
        assert!(parse_header(&header(24, BCA_RLE24, 10), false).is_some());
        assert!(parse_header(&header(4, BCA_RLE8, 10), false).is_none(), "RLE8 needs 8 bits");
        assert!(parse_header(&header(8, 3, 10), false).is_none(), "BCA_HUFFMAN1D");
        let missing = parse_header(&header(8, BCA_RLE8, 0), false).unwrap();
        assert_eq!(missing.data_len(2), None, "compressed data needs cbImage");
        let huge = parse_header(&header(8, BCA_RLE8, u32::MAX), false).unwrap();
        assert_eq!(huge.data_len(2), None, "cbImage beyond any useful stream");
        let plain = parse_header(&header(8, BCA_UNCOMP, 0), false).unwrap();
        assert_eq!(plain.data_len(2), Some(8), "uncompressed: padded rows");
    }
}

#[cfg(test)]
mod api_tests {
    use super::super::constants::*;
    use super::super::mutex_ext::MutexExt;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::{ApiResult, Loader};

    const ESP: u32 = 0x1000;
    const PTS: u32 = 0x4000;  // POINTL arrays
    const HDR: u32 = 0x4100;  // bitmap headers
    const DATA: u32 = 0x4400; // pel data
    const TBL: u32 = 0x4800;  // colour tables / CAPS arrays

    fn call(loader: &Loader, gpi: bool, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        vcpu.regs.rsp = ESP as u64;
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(ESP + 4 + i as u32 * 4, a).unwrap();
        }
        let r = if gpi {
            loader.handle_pmgpi_call(&mut vcpu, 0, ordinal)
        } else {
            loader.handle_pmwin_call(&mut vcpu, 0, ordinal)
        };
        match r { ApiResult::Normal(v) => v, _ => panic!("unexpected ApiResult") }
    }
    fn gpi(l: &Loader, ord: u32, args: &[u32]) -> u32 { call(l, true, ord, args) }
    fn win(l: &Loader, ord: u32, args: &[u32]) -> u32 { call(l, false, ord, args) }

    fn points(l: &Loader, pts: &[(i32, i32)]) -> u32 {
        for (i, &(x, y)) in pts.iter().enumerate() {
            l.guest_write::<i32>(PTS + 8 * i as u32, x).unwrap();
            l.guest_write::<i32>(PTS + 8 * i as u32 + 4, y).unwrap();
        }
        PTS
    }

    /// A 16-byte BITMAPINFOHEADER2 at HDR.
    fn header(l: &Loader, cx: u32, cy: u32, bits: u16) -> u32 {
        l.guest_write::<u32>(HDR, 16).unwrap();
        l.guest_write::<u32>(HDR + 4, cx).unwrap();
        l.guest_write::<u32>(HDR + 8, cy).unwrap();
        l.guest_write::<u16>(HDR + 12, 1).unwrap();
        l.guest_write::<u16>(HDR + 14, bits).unwrap();
        HDR
    }

    /// A window with a WinGetPS presentation space.
    fn window_ps(l: &Loader) -> (u32, u32) {
        let hwnd = {
            let mut wm = l.shared.window_mgr.lock_or_recover();
            let h = wm.create_window("Test".into(), 0, 0);
            let w = wm.get_window_mut(h).unwrap();
            (w.cx, w.cy) = (64, 32);
            h
        };
        (hwnd, win(l, 757, &[hwnd]))
    }

    /// A memory DC, a PS associated with it and an 8×4 24-bit bitmap
    /// selected into it: `(hdc, hps, hbm)`.
    fn memory_ps(l: &Loader) -> (u32, u32, u32) {
        let hdc = gpi(l, 610, &[0, OD_MEMORY, 0, 0, 0, 0]);
        assert_ne!(hdc, DEV_ERROR);
        let hps = gpi(l, 369, &[0, hdc, 0, GPIA_ASSOC]);
        assert_ne!(hps, 0);
        let hbm = gpi(l, 598, &[hps, header(l, 8, 4, 24), 0, 0, 0]);
        assert_ne!(hbm, 0);
        assert_eq!(gpi(l, 506, &[hps, hbm]), 0, "nothing was selected before");
        (hdc, hps, hbm)
    }

    fn pel(l: &Loader, hps: u32, x: i32, y: i32) -> u32 {
        gpi(l, 476, &[hps, points(l, &[(x, y)])])
    }

    fn fill(l: &Loader, hps: u32, color: u32, (x1, y1, x2, y2): (i32, i32, i32, i32)) {
        gpi(l, 517, &[hps, color]);
        gpi(l, 404, &[hps, points(l, &[(x1, y1)])]);
        gpi(l, 356, &[hps, 1, points(l, &[(x2, y2)]), 0, 0]); // DRO_FILL
    }

    const CLR_BLUE: u32 = 1;
    const CLR_RED: u32 = 2;

    /// Drawing through a memory PS lands in its bitmap, GpiBitBlt copies it
    /// to a window, and GpiQueryBitmapBits returns it bottom row first.
    #[test]
    fn memory_ps_draws_into_its_bitmap_and_blits_to_a_window() {
        let l = Loader::new_mock();
        let (_, mps, hbm) = memory_ps(&l);
        fill(&l, mps, CLR_RED, (0, 0, 7, 3));
        fill(&l, mps, CLR_BLUE, (0, 0, 3, 1));
        assert_eq!(pel(&l, mps, 1, 1), CLR_BLUE);
        assert_eq!(pel(&l, mps, 5, 2), CLR_RED);
        assert_eq!(pel(&l, mps, 8, 0), CLR_NOINDEX as u32, "outside the 8×4 bitmap");

        // Copy the bitmap to window (10, 10) with three points.
        let (hwnd, wps) = window_ps(&l);
        let p = points(&l, &[(10, 10), (18, 14), (0, 0)]);
        assert_eq!(gpi(&l, 355, &[wps, mps, 3, p, ROP_SRCCOPY, 0]), GPI_OK);
        assert_eq!(pel(&l, wps, 11, 11), CLR_BLUE);
        assert_eq!(pel(&l, wps, 17, 13), CLR_RED);
        assert_eq!(pel(&l, wps, 18, 13), 0, "target is exclusive at the right");
        assert!(l.shared.window_mgr.lock_or_recover().dirty.contains(&hwnd));

        // Read the bitmap back as 24-bit rows: row 0 starts blue, blue…
        header(&l, 0, 0, 24);
        assert_eq!(gpi(&l, 599, &[mps, 0, 4, DATA, HDR]), 4);
        assert_eq!(l.guest_read::<u32>(HDR + 4), Some(8), "cx filled in");
        assert_eq!(l.guest_read::<u32>(HDR + 8), Some(4), "cy filled in");
        let row0 = l.guest_read_bytes(DATA, 24).unwrap();
        assert_eq!(&row0[..6], &[0xFF, 0, 0, 0xFF, 0, 0], "blue as B, G, R");
        assert_eq!(&row0[12..15], &[0, 0, 0xFF], "red");

        // …and write a green row 3 with GpiSetBitmapBits.
        let green: Vec<u8> = (0..8).flat_map(|_| [0u8, 0xFF, 0]).collect();
        l.guest_write_bytes(DATA, &green).unwrap();
        header(&l, 8, 1, 24);
        assert_eq!(gpi(&l, 602, &[mps, 3, 1, DATA, HDR]), 1);
        assert_eq!(gpi(&l, 476, &[mps, points(&l, &[(4, 3)])]), CLR_GREEN_INDEX);
        assert_eq!(gpi(&l, 602, &[mps, 4, 1, DATA, HDR]), 0, "past the last row");

        // Header queries report the size and format the bitmap was made with.
        l.guest_write::<u32>(HDR, 16).unwrap();
        assert_eq!(gpi(&l, 601, &[hbm, HDR]), GPI_OK);
        assert_eq!((l.guest_read::<u32>(HDR + 4), l.guest_read::<u16>(HDR + 14)), (Some(8), Some(24)));
        assert_eq!(gpi(&l, 573, &[hbm, HDR]), GPI_OK);
        assert_eq!((l.guest_read::<u32>(HDR), l.guest_read::<u16>(HDR + 4)), (Some(12), Some(8)));
    }

    const CLR_GREEN_INDEX: u32 = 4;

    #[test]
    fn bitmap_selection_rules() {
        let l = Loader::new_mock();
        let (hdc, mps, hbm) = memory_ps(&l);
        let (_, wps) = window_ps(&l);
        assert_eq!(gpi(&l, 506, &[wps, hbm]), HBM_ERROR, "window PS has no memory DC");
        let other = gpi(&l, 369, &[0, hdc, 0, GPIA_ASSOC]);
        assert_eq!(gpi(&l, 506, &[other, hbm]), HBM_ERROR, "already selected elsewhere");
        assert_eq!(gpi(&l, 506, &[mps, 0xBAD]), HBM_ERROR, "unknown bitmap");
        assert_eq!(gpi(&l, 371, &[hbm]), GPI_ERROR, "cannot delete a selected bitmap");
        assert_eq!(gpi(&l, 506, &[mps, 0]), hbm, "deselect returns it");
        assert_eq!(gpi(&l, 371, &[hbm]), GPI_OK);
        assert_eq!(gpi(&l, 371, &[hbm]), GPI_ERROR, "already deleted");
        assert_eq!(gpi(&l, 369, &[0, 0xBAD, 0, GPIA_ASSOC]), 0, "unknown DC");
        assert_eq!(gpi(&l, 604, &[hdc]), DEV_OK);
        assert_eq!(gpi(&l, 604, &[hdc]), DEV_ERROR);
        assert_eq!(gpi(&l, 610, &[0, 2 /* OD_QUEUED */, 0, 0, 0, 0]), DEV_ERROR, "no printers");
        assert_eq!(l.shared.pm_errors.take(0) & 0xFFFF, super::super::pmspl::PMERR_DRIVER_NOT_FOUND);
        assert_eq!(gpi(&l, 610, &[0, 5 /* OD_DIRECT */, 0, 0, 0, 0]), DEV_ERROR, "no printers");
        assert_eq!(l.shared.pm_errors.take(0) & 0xFFFF, super::super::pmspl::PMERR_DRIVER_NOT_FOUND);
        assert_eq!(gpi(&l, 610, &[0, 7 /* OD_METAFILE */, 0, 0, 0, 0]), DEV_ERROR);
        assert_eq!(l.shared.pm_errors.take(0) & 0xFFFF, super::super::pmspl::PMERR_INV_DC_TYPE);
        // Unsupported formats are refused.
        assert_eq!(gpi(&l, 598, &[0, header(&l, 4, 4, 16), 0, 0, 0]), 0);
        assert_eq!(gpi(&l, 598, &[0, header(&l, 0, 4, 24), 0, 0, 0]), 0);
    }

    /// GpiDrawBits and GpiWCBitBlt targets are world coordinates (mapped by
    /// the model transform); GpiBitBlt targets are device coordinates.
    #[test]
    fn world_targets_follow_the_transforms() {
        let l = Loader::new_mock();
        let (hwnd, wps) = window_ps(&l);
        // 2×2, 1 bit: palette black, black; all pels black.
        header(&l, 2, 2, 1);
        l.guest_write_bytes(HDR + 16, &[0; 8]).unwrap();
        l.guest_write_bytes(DATA, &[0xC0, 0, 0, 0, 0xC0, 0, 0, 0]).unwrap();
        let hbm = gpi(&l, 598, &[0, HDR, CBM_INIT, DATA, HDR]);
        // Model transform: scale ×2, move by (10, 0).
        let m: [i32; 9] = [2 << 16, 0, 0, 0, 2 << 16, 0, 10, 0, 1];
        for (i, v) in m.iter().enumerate() { l.guest_write::<i32>(TBL + 4 * i as u32, *v).unwrap(); }
        assert_eq!(gpi(&l, 538, &[wps, 9, TBL, 0]), 1); // GpiSetModelTransformMatrix
        // World (0,0)–(1,1) inclusive → device (10,0)–(12,2): the corners are points.
        let p = points(&l, &[(0, 0), (1, 1), (0, 0), (2, 2)]);
        assert_eq!(gpi(&l, 603, &[wps, DATA, HDR, 4, p, ROP_SRCCOPY, 0]), GPI_OK);
        let surface = l.shared.window_mgr.lock_or_recover().frame_surface(hwnd).unwrap();
        let black = |x, y| surface.lock_or_recover().pel(x, y) == Some(0);
        assert!(black(10, 0) && black(12, 2));
        assert!(!black(13, 2) && !black(12, 3) && !black(9, 0));
        // GpiWCBitBlt: world (5,5)–(6,6) → device (20,10)–(22,12).
        let p = points(&l, &[(5, 5), (6, 6), (0, 0), (2, 2)]);
        assert_eq!(gpi(&l, 557, &[wps, hbm, 4, p, ROP_SRCCOPY, 0]), GPI_OK);
        assert!(black(20, 10) && black(22, 12) && !black(23, 12));
        // GpiBitBlt: device coordinates, the transform does not apply.
        let p = points(&l, &[(40, 20), (42, 22)]);
        assert_eq!(gpi(&l, 355, &[wps, 0, 2, p, ROP_ZERO, 0]), GPI_OK);
        assert!(black(40, 20) && black(41, 21) && !black(42, 21));
    }

    /// CBM_INIT from 1-bit data with a colour table; GpiDrawBits and a
    /// stretched four-point GpiBitBlt; operations without a source.
    #[test]
    fn initialised_bitmaps_stretching_and_raster_operations() {
        let l = Loader::new_mock();
        let (_, wps) = window_ps(&l);
        // 2×2, 1 bit: palette black, white; rows (bottom first) 10, 01.
        header(&l, 2, 2, 1);
        l.guest_write_bytes(HDR + 16, &[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0]).unwrap();
        l.guest_write_bytes(DATA, &[0x80, 0, 0, 0, 0x40, 0, 0, 0]).unwrap();
        let hbm = gpi(&l, 598, &[0, HDR, CBM_INIT, DATA, HDR]);
        assert_ne!(hbm, 0);

        // GpiDrawBits the same data, scaled ×4 into (0,0)–(7,7) inclusive.
        let p = points(&l, &[(0, 0), (7, 7), (0, 0), (2, 2)]);
        assert_eq!(gpi(&l, 603, &[wps, DATA, HDR, 4, p, ROP_SRCCOPY, 0]), GPI_OK);
        assert_eq!(pel(&l, wps, 0, 0), 0, "white bottom-left (CLR_BACKGROUND)");
        assert_eq!(pel(&l, wps, 3, 3), 0);
        assert_eq!(pel(&l, wps, 4, 0), CLR_BLACK as u32);
        assert_eq!(pel(&l, wps, 7, 7), 0);
        assert_eq!(pel(&l, wps, 0, 7), CLR_BLACK as u32);

        // DSTINVERT needs no source; a source operation without one fails.
        let p = points(&l, &[(0, 0), (1, 1)]);
        assert_eq!(gpi(&l, 355, &[wps, 0, 2, p, ROP_DSTINVERT, 0]), GPI_OK);
        assert_eq!(pel(&l, wps, 0, 0), CLR_BLACK as u32, "white inverted");
        assert_eq!(gpi(&l, 355, &[wps, 0, 2, p, ROP_SRCCOPY, 0]), GPI_ERROR);
        assert_eq!(gpi(&l, 355, &[wps, 0, 5, p, ROP_SRCCOPY, 0]), GPI_ERROR, "bad count");

        // WinDrawBitmap at (20, 20), inverted.
        let p = points(&l, &[(20, 20)]);
        assert_eq!(win(&l, 730, &[wps, hbm, 0, p, 0, 0, DBM_INVERT]), 1);
        assert_eq!(pel(&l, wps, 20, 20), CLR_BLACK as u32, "white → black");
        assert_eq!(pel(&l, wps, 21, 20), 0, "black → white");
        assert_eq!(win(&l, 730, &[wps, 0xBAD, 0, p, 0, 0, 0]), 0);

        // GpiWCBitBlt: inclusive target, guest extremes are harmless.
        let p = points(&l, &[(i32::MIN, i32::MIN), (i32::MAX, i32::MAX), (0, 0), (2, 2)]);
        assert_eq!(gpi(&l, 557, &[wps, hbm, 4, p, ROP_SRCCOPY, 0]), GPI_OK);
    }

    #[test]
    fn logical_color_tables() {
        let l = Loader::new_mock();
        let (_, wps) = window_ps(&l);
        assert_eq!(gpi(&l, 593, &[wps, 0, 0, 4, TBL]), QLCT_NOTLOADED as u32);
        // Index 1 redefined as orange.
        l.guest_write::<u32>(TBL, 0x00FF_8000).unwrap();
        assert_eq!(gpi(&l, 592, &[wps, 0, LCOLF_CONSECRGB, 1, 1, TBL]), GPI_OK);
        assert_eq!(gpi(&l, 479, &[wps, 0, 1]), 0x00FF_8000, "GpiQueryRGBColor");
        assert_eq!(gpi(&l, 439, &[wps, 0, 0x00FF_7F00]), 1, "nearest index");
        gpi(&l, 517, &[wps, 1]);
        assert_eq!(gpi(&l, 437, &[wps]), 1, "GpiQueryColor reports the index set");
        gpi(&l, 544, &[wps, points(&l, &[(2, 2)])]);
        assert_eq!(pel(&l, wps, 2, 2), 1, "drawn in orange, read back as index 1");
        assert_eq!(gpi(&l, 593, &[wps, LCOLOPT_INDEX, 1, 2, TBL]), 2);
        assert_eq!((l.guest_read::<u32>(TBL), l.guest_read::<u32>(TBL + 4)), (Some(1), Some(0x00FF_8000)));

        // (index, RGB) pairs.
        for (i, v) in [20u32, 0x0012_3456].into_iter().enumerate() {
            l.guest_write::<u32>(TBL + 4 * i as u32, v).unwrap();
        }
        assert_eq!(gpi(&l, 592, &[wps, 0, LCOLF_INDRGB, 0, 2, TBL]), GPI_OK);
        assert_eq!(gpi(&l, 479, &[wps, 0, 20]), 0x0012_3456);

        // RGB mode: colours are RGB values and GpiQueryPel answers in RGB.
        assert_eq!(gpi(&l, 592, &[wps, 0, LCOLF_RGB, 0, 0, 0]), GPI_OK);
        assert_eq!(gpi(&l, 593, &[wps, 0, 0, 4, TBL]), QLCT_RGB as u32);
        gpi(&l, 517, &[wps, 0x0000_00FF]);
        gpi(&l, 544, &[wps, points(&l, &[(3, 3)])]);
        assert_eq!(pel(&l, wps, 3, 3), 0x0000_00FF);
        assert_eq!(pel(&l, wps, 2, 2), 0x00FF_8000);

        // Back to the default table: the orange index is gone.
        assert_eq!(gpi(&l, 592, &[wps, LCOL_RESET, LCOLF_DEFAULT, 0, 0, 0]), GPI_OK);
        assert_eq!(gpi(&l, 479, &[wps, 0, 1]), 0x0000_00FF, "CLR_BLUE again");
        assert_eq!(gpi(&l, 592, &[wps, 0, 4 /* LCOLF_PALETTE */, 0, 0, 0]), GPI_ERROR);
        assert_eq!(gpi(&l, 479, &[0xBAD, 0, 1]), GPI_ALTERROR as u32);
    }

    #[test]
    fn line_types_and_text_background() {
        use crate::gui::surface::DrawOp;
        let l = Loader::new_mock();
        let (hwnd, wps) = window_ps(&l);
        let surface = l.shared.window_mgr.lock_or_recover().frame_surface(hwnd).unwrap();
        assert_eq!(gpi(&l, 459, &[wps]), LINETYPE_DEFAULT);
        assert_eq!(gpi(&l, 530, &[wps, LINETYPE_DOT]), GPI_OK);
        assert_eq!(gpi(&l, 459, &[wps]), LINETYPE_DOT);
        gpi(&l, 517, &[wps, CLR_RED]);
        gpi(&l, 404, &[wps, points(&l, &[(0, 5)])]);
        gpi(&l, 398, &[wps, points(&l, &[(15, 5)])]);
        let red = (0..16).filter(|&x| pel(&l, wps, x, 5) == CLR_RED).count();
        assert_eq!(red, 8, "every other pel");
        assert_eq!(gpi(&l, 530, &[wps, LINETYPE_INVISIBLE]), GPI_OK);
        gpi(&l, 398, &[wps, points(&l, &[(15, 20)])]);
        assert_eq!(pel(&l, wps, 15, 20), 0, "invisible lines draw nothing");
        assert_eq!(gpi(&l, 530, &[wps, 10]), GPI_ERROR);

        // BM_OVERPAINT fills the string's box with the background colour first.
        surface.lock().unwrap().record = Some(Vec::new());
        gpi(&l, 504, &[wps, CLR_BLUE]);
        gpi(&l, 505, &[wps, FM_OVERPAINT]);
        l.guest_write_bytes(DATA, b"Hi").unwrap();
        gpi(&l, 359, &[wps, points(&l, &[(2, 10)]), 2, DATA]);
        let ops = surface.lock().unwrap().record.take().unwrap();
        assert!(matches!(ops[..], [DrawOp::Box { color: 0x0000_00FF, fill: true, .. }, DrawOp::TextFont { .. }]),
                "{ops:?}");
        assert_eq!(gpi(&l, 424, &[wps]), CLR_BLUE, "GpiQueryBackColor reports the index set");
        // The default (leave alone) draws no box.
        gpi(&l, 505, &[wps, BM_LEAVEALONE]);
        surface.lock().unwrap().record = Some(Vec::new());
        gpi(&l, 359, &[wps, points(&l, &[(2, 10)]), 2, DATA]);
        assert!(matches!(surface.lock().unwrap().record.take().unwrap()[..], [DrawOp::TextFont { .. }]));
    }

    #[test]
    fn device_caps_describe_a_true_colour_display() {
        let l = Loader::new_mock();
        let hdc = gpi(&l, 610, &[0, OD_INFO, 0, 0, 0, 0]);
        assert_eq!(gpi(&l, 606, &[hdc, CAPS_WIDTH, 2, TBL]), GPI_OK);
        let (w, h) = desktop_size();
        assert_eq!((l.guest_read::<u32>(TBL), l.guest_read::<u32>(TBL + 4)), (Some(w), Some(h)));
        assert_eq!(gpi(&l, 606, &[hdc, CAPS_COLOR_PLANES, 2, TBL]), GPI_OK);
        assert_eq!((l.guest_read::<u32>(TBL), l.guest_read::<u32>(TBL + 4)), (Some(1), Some(24)));
        assert_eq!(gpi(&l, 606, &[0xBAD, 0, 1, TBL]), GPI_ERROR);
    }

    /// WinBeginPaint frees its presentation space at WinEndPaint and reports
    /// the update rectangle (then empty); WinGetPS / WinReleasePS pair;
    /// a PS on a window DC draws into the window.
    #[test]
    fn paint_and_cached_presentation_spaces() {
        let l = Loader::new_mock();
        let (hwnd, wps) = window_ps(&l);
        let count = || l.shared.window_mgr.lock_or_recover().ps_map.len();
        let before = count();
        for _ in 0..3 {
            win(&l, 765, &[hwnd, 0, 0]); // WinInvalidateRect: the whole window
            let hps = win(&l, 703, &[hwnd, 0, TBL]);
            assert_eq!(l.guest_read::<i32>(TBL + 8), Some(64), "update rect xRight");
            assert_eq!(l.guest_read::<i32>(TBL + 12), Some(32), "update rect yTop");
            win(&l, 738, &[hps]);
        }
        assert_eq!(count(), before, "no presentation space leaks per paint");
        let hps = win(&l, 703, &[hwnd, 0, TBL]);
        assert_eq!(l.guest_read::<i32>(TBL + 8), Some(0), "nothing left to paint");
        win(&l, 738, &[hps]);
        assert_eq!(win(&l, 703, &[hwnd, wps, 0]), wps, "a given PS is used as is");
        win(&l, 738, &[wps]);
        assert_eq!(win(&l, 848, &[wps]), 1);
        assert_eq!(win(&l, 848, &[wps]), 0, "already released");
        assert_eq!(win(&l, 757, &[0xBAD]), 0);

        let hdc = win(&l, 794, &[hwnd]);
        assert_ne!(hdc, 0);
        assert_eq!(win(&l, 794, &[hwnd]), hdc, "one DC per window");
        assert_eq!(win(&l, 835, &[hwnd]), hdc);
        let hps = gpi(&l, 369, &[0, hdc, 0, GPIA_ASSOC]);
        fill(&l, hps, CLR_RED, (0, 0, 3, 3));
        let surface = l.shared.window_mgr.lock_or_recover().frame_surface(hwnd).unwrap();
        assert_eq!(surface.lock().unwrap().pel(1, 1), Some(0x00FF_0000));
        assert_eq!(gpi(&l, 351, &[hps, 0]), GPI_OK, "dissociate");
        fill(&l, hps, CLR_BLUE, (0, 0, 3, 3));
        assert_eq!(surface.lock().unwrap().pel(1, 1), Some(0x00FF_0000), "no longer draws");
    }

    /// GpiSetBitmapBits decodes RLE8 data (cbImage bytes); GpiQueryBitmapBits
    /// returns it uncompressed.
    #[test]
    fn set_bitmap_bits_decodes_rle8() {
        let l = Loader::new_mock();
        let (_, hps, _) = memory_ps(&l);
        // 8 bpp RLE8 header with cbImage `len` and a table whose index 0 is
        // black and 1 white.
        let rle8_header = |len: u32| {
            let h = header(&l, 8, 4, 8);
            l.guest_write::<u32>(h, 24).unwrap();
            l.guest_write::<u32>(h + 16, BCA_RLE8).unwrap();
            l.guest_write::<u32>(h + 20, len).unwrap();
            for (i, c) in [0x000000u32, 0xFFFFFF].iter().enumerate() {
                l.guest_write::<u32>(h + 24 + 4 * i as u32, *c).unwrap();
            }
            h
        };
        let stream = [8u8, 1, 0, 0, 3, 0, 0, 1];
        let h = rle8_header(stream.len() as u32);
        // The rest of the 256-entry table reads as zeros (black).
        l.guest_write_bytes(DATA, &stream).unwrap();
        assert_eq!(gpi(&l, 602, &[hps, 0, 2, DATA, h]), 2, "two scans set");
        // Read back as 24-bit: row 0 all white, row 1 three black then the
        // pels it did not cover (still the bitmap's initial white).
        // A 24-byte query header whose compression field still says RLE8.
        let q = header(&l, 0, 0, 24);
        l.guest_write::<u32>(q, 24).unwrap();
        assert_eq!(l.guest_read::<u32>(q + 16).unwrap(), BCA_RLE8);
        assert_eq!(gpi(&l, 599, &[hps, 0, 2, DATA, q]), 2);
        let px = |row: u32, x: u32| l.guest_read::<u32>(DATA + row * 24 + x * 3).unwrap() & 0x00FF_FFFF;
        assert_eq!(px(0, 0), 0xFFFFFF);
        assert_eq!(px(0, 7), 0xFFFFFF);
        assert_eq!(px(1, 2), 0x000000);
        assert_eq!(l.guest_read::<u32>(q + 16).unwrap(), BCA_UNCOMP, "returned uncompressed");
        // Without cbImage the compressed data cannot be read.
        let h = rle8_header(0);
        assert_eq!(gpi(&l, 602, &[hps, 0, 2, DATA, h]), BMB_ERROR as u32);
    }

    /// GpiQueryDeviceBitmapFormats lists (planes, bits) pairs, 24-bit first;
    /// DevQueryCaps reports how many there are.
    #[test]
    fn device_bitmap_formats() {
        let l = Loader::new_mock();
        let (_, hps) = window_ps(&l);
        assert_eq!(gpi(&l, 445, &[hps, 4, TBL]), GPI_OK);
        let v: Vec<u32> = (0..4).map(|i| l.guest_read::<u32>(TBL + 4 * i).unwrap()).collect();
        assert_eq!(v, [1, 24, 1, 1]);
        l.guest_write::<u32>(TBL + 40, 0xDEAD).unwrap();
        assert_eq!(gpi(&l, 445, &[hps, 100, TBL]), GPI_OK, "more than there are");
        assert_eq!(l.guest_read::<u32>(TBL + 40).unwrap(), 0xDEAD, "only 5 pairs written");
        assert_eq!(gpi(&l, 445, &[hps, -1i32 as u32, TBL]), GPI_ERROR);
        assert_eq!(gpi(&l, 445, &[0xBAD, 2, TBL]), GPI_ERROR);
        let hdc = gpi(&l, 610, &[0, OD_INFO, 0, 0, 0, 0]);
        assert_eq!(gpi(&l, 606, &[hdc, CAPS_BITMAP_FORMATS, 1, TBL]), GPI_OK);
        assert_eq!(l.guest_read::<u32>(TBL).unwrap(), 5);
    }

    /// GpiLoadBitmap loads from the module named by hmod: NULLHANDLE is the
    /// executable, a DLL handle that DLL's resources.
    #[test]
    fn load_bitmap_from_the_named_module() {
        use super::super::managers::EXE_RESOURCE_MODULE;
        let l = Loader::new_mock();
        let (_, hps) = window_ps(&l);
        // A 1×1 24-bit "BM" resource of colour (b, g, r) at `at`.
        let bm = |at: u32, bgr: [u8; 3]| {
            let mut f = Vec::new();
            f.extend(BFT_BMAP.to_le_bytes());
            f.extend(0u32.to_le_bytes());
            f.extend([0u8; 4]);
            f.extend(30u32.to_le_bytes());
            f.extend(16u32.to_le_bytes());
            f.extend(1u32.to_le_bytes());
            f.extend(1u32.to_le_bytes());
            f.extend(1u16.to_le_bytes());
            f.extend(24u16.to_le_bytes());
            f.extend(bgr);
            f.push(0);
            l.guest_write_bytes(at, &f).unwrap();
            f.len() as u32
        };
        let dll = 9;
        {
            let red = bm(DATA, [0, 0, 0xFF]);
            let green = bm(DATA + 0x100, [0, 0xFF, 0]);
            let mut res = l.shared.resource_mgr.lock_or_recover();
            res.add(EXE_RESOURCE_MODULE, crate::lx::header::RT_BITMAP, 1, DATA, red);
            res.add(dll, crate::lx::header::RT_BITMAP, 1, DATA + 0x100, green);
        }
        let pel = |hbm: u32| {
            let wm = l.shared.window_mgr.lock_or_recover();
            let s = wm.bitmaps[&hbm].surface.lock_or_recover();
            s.pel(0, 0)
        };
        let from_exe = gpi(&l, 399, &[hps, 0, 1, 0, 0]);
        assert_ne!(from_exe, GPI_ERROR);
        assert_eq!(pel(from_exe), Some(0x00FF_0000));
        let from_dll = gpi(&l, 399, &[hps, dll, 1, 0, 0]);
        assert_ne!(from_dll, GPI_ERROR);
        assert_eq!(pel(from_dll), Some(0x0000_FF00), "the DLL's bitmap, not the executable's");
        assert_eq!(gpi(&l, 399, &[hps, 10, 1, 0, 0]), GPI_ERROR, "a module without it");
    }
}

