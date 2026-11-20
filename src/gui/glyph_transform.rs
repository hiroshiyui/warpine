// SPDX-License-Identifier: GPL-3.0-or-later

//! Glyphs drawn under a linear transform: GPI character angle and shear.
//!
//! An outline glyph is transformed in font units before it is rasterised
//! (ab_glyph's own rasteriser, through [`ab_glyph::OutlinedGlyph`]), so a
//! rotated or sheared character is as smooth as an upright one. The GNU
//! Unifont bitmap fallbacks are mapped pel by pel (nearest pel through the
//! inverse transform). Upright text never comes here: it keeps the cached
//! rasters of `font.rs`.
//!
//! Transformed rasters are cached per thread like the upright ones, keyed by
//! the character, the font and the matrix, and the cache is bounded
//! ([`TRANSFORMED_CACHE_MAX`]).

use std::cell::RefCell;
use std::collections::HashMap;

use ab_glyph::{point, OutlineCurve, OutlinedGlyph, Point, PxScale, Rect};

use super::font::{
    glyph_shape, FontSpec, GlyphShape, UNIFONT_CELL_H, UNIFONT_DESCENT, UNIFONT_NARROW_W, UNIFONT_WIDE_W,
};

/// A 2×2 linear map `[a, b, c, d]`: glyph-local `(u, v)` (u along the
/// baseline, v up) → `(a·u + b·v, c·u + d·v)` in PM space (y up).
pub type Matrix = [f32; 4];

/// The upright transform.
pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0];

/// Largest magnitude of a matrix entry drawn; larger ones (an extreme
/// shear) draw nothing, so a glyph's raster stays small.
pub const MAX_MATRIX_ENTRY: f32 = 16.0;

/// Largest raster (pels) one transformed glyph may have.
const MAX_RASTER_PELS: usize = 1 << 20;

/// Upper bound on cached transformed glyphs per thread; cleared when full.
pub const TRANSFORMED_CACHE_MAX: usize = 1024;

/// A coverage bitmap placed relative to the glyph origin on the baseline
/// (screen orientation: `top` is the offset of the first row, negative =
/// above the baseline).
#[derive(Clone, Debug, PartialEq)]
struct Coverage {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    coverage: Vec<u8>,
}

type Key = (char, FontSpec, [u32; 4]);

thread_local! {
    static CACHE: RefCell<HashMap<Key, Option<Coverage>>> = RefCell::new(HashMap::new());
}

/// Whether `m` can be drawn: finite, entries within ±[`MAX_MATRIX_ENTRY`]
/// and not singular.
pub fn drawable(m: Matrix) -> bool {
    m.iter().all(|v| v.is_finite() && v.abs() <= MAX_MATRIX_ENTRY)
        && (m[0] * m[3] - m[1] * m[2]).abs() > f32::EPSILON
}

/// Apply `m` to glyph-local `(u, v)`.
fn apply(m: Matrix, (u, v): (f32, f32)) -> (f32, f32) {
    (m[0] * u + m[1] * v, m[2] * u + m[3] * v)
}

/// Draw `ch` in `spec` transformed by `m`, its origin at screen `(x,
/// baseline)` of a top-left-origin buffer `buf_w` × `buf_h`; `paint` gets
/// each covered pel and its coverage (never 0). Pels outside the buffer are
/// clipped. A matrix that is not [`drawable`] draws nothing.
#[allow(clippy::too_many_arguments)]
pub fn draw_glyph(buf: &mut [u32], buf_w: usize, buf_h: usize, x: i32, baseline: i32,
                  ch: char, spec: FontSpec, m: Matrix, mut paint: impl FnMut(&mut u32, u8)) {
    if !drawable(m) { return; }
    let buf_h = buf_h.min(buf.len() / buf_w.max(1));
    let key = (ch, spec, m.map(f32::to_bits));
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= TRANSFORMED_CACHE_MAX && !cache.contains_key(&key) {
            cache.clear();
        }
        let Some(c) = cache.entry(key).or_insert_with(|| rasterize(ch, spec, m)) else { return };
        for row in 0..c.height {
            let py = baseline as i64 + c.top as i64 + row as i64;
            if py < 0 || py >= buf_h as i64 { continue; }
            for col in 0..c.width {
                let px = x as i64 + c.left as i64 + col as i64;
                let cov = c.coverage[row * c.width + col];
                if cov != 0 && px >= 0 && px < buf_w as i64 {
                    paint(&mut buf[py as usize * buf_w + px as usize], cov);
                }
            }
        }
    });
}

/// Number of entries in this thread's transformed-glyph cache.
#[cfg(test)]
fn cache_len() -> usize {
    CACHE.with(|c| c.borrow().len())
}

fn rasterize(ch: char, spec: FontSpec, m: Matrix) -> Option<Coverage> {
    match glyph_shape(ch, spec) {
        GlyphShape::Outline { outline, scale } => rasterize_outline(outline, scale, m),
        GlyphShape::Narrow(rows) => rasterize_bitmap(UNIFONT_NARROW_W, m, |col, row| {
            rows[row as usize] & (0x80 >> col) != 0
        }),
        GlyphShape::Wide(bytes) => rasterize_bitmap(UNIFONT_WIDE_W, m, |col, row| {
            let bits = u16::from_be_bytes([bytes[row as usize * 2], bytes[row as usize * 2 + 1]]);
            bits & (0x8000 >> col) != 0
        }),
        GlyphShape::Blank => None,
    }
}

/// Transform the outline's curves in font units (y up) and rasterise them
/// at the glyph origin (0, 0).
fn rasterize_outline(mut outline: ab_glyph::Outline, scale: ab_glyph::PxScaleFactor, m: Matrix)
                     -> Option<Coverage> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut map = |p: &mut Point| {
        let (x, y) = apply(m, (p.x, p.y));
        *p = point(x, y);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    };
    for curve in &mut outline.curves {
        match curve {
            OutlineCurve::Line(a, b) => { map(a); map(b); }
            OutlineCurve::Quad(a, b, c) => { map(a); map(b); map(c); }
            OutlineCurve::Cubic(a, b, c, d) => { map(a); map(b); map(c); map(d); }
        }
    }
    if x0 > x1 || y0 > y1 { return None; }
    // ab_glyph keeps the top (largest y) in `min.y`: its pixel rows grow down.
    outline.bounds = Rect { min: point(x0, y1), max: point(x1, y0) };
    let glyph = ab_glyph::GlyphId(0).with_scale_and_position(PxScale::from(1.0), point(0.0, 0.0));
    let outlined = OutlinedGlyph::new(glyph, outline, scale);
    let b = outlined.px_bounds();
    let (width, height) = (b.width() as usize, b.height() as usize);
    if width == 0 || height == 0 || width.saturating_mul(height) > MAX_RASTER_PELS { return None; }
    let mut coverage = vec![0u8; width * height];
    outlined.draw(|x, y, c| {
        if let Some(p) = coverage.get_mut(y as usize * width + x as usize) {
            *p = super::font::contrast_coverage(c);
        }
    });
    Some(Coverage { left: b.min.x as i32, top: b.min.y as i32, width, height, coverage })
}

/// Map a Unifont bitmap `cell_w` × 16 (baseline [`UNIFONT_DESCENT`] above
/// its bottom) through `m`: each destination pel whose centre falls, through
/// the inverse transform, in a set source pel is fully covered.
fn rasterize_bitmap(cell_w: i32, m: Matrix, set: impl Fn(i32, i32) -> bool) -> Option<Coverage> {
    let det = m[0] * m[3] - m[1] * m[2];
    let inv = [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det];
    let (bottom, top) = (-UNIFONT_DESCENT as f32, (UNIFONT_CELL_H - UNIFONT_DESCENT) as f32);
    let corners = [(0.0, bottom), (cell_w as f32, bottom), (0.0, top), (cell_w as f32, top)]
        .map(|p| apply(m, p));
    let fold = |f: fn(f32, f32) -> f32, init: f32, sel: fn(&(f32, f32)) -> f32|
        corners.iter().map(sel).fold(init, f);
    // PM bounds (y up), then screen rows (y down).
    let (px0, px1) = (fold(f32::min, f32::MAX, |p| p.0).floor() as i32, fold(f32::max, f32::MIN, |p| p.0).ceil() as i32);
    let (py0, py1) = (fold(f32::min, f32::MAX, |p| p.1).floor() as i32, fold(f32::max, f32::MIN, |p| p.1).ceil() as i32);
    let (width, height) = ((px1 - px0).max(0) as usize, (py1 - py0).max(0) as usize);
    if width == 0 || height == 0 || width.saturating_mul(height) > MAX_RASTER_PELS { return None; }
    let mut coverage = vec![0u8; width * height];
    for row in 0..height {
        // Screen row `row` of the raster is PM y from the top down.
        let y = py1 as f32 - row as f32 - 0.5;
        for col in 0..width {
            let x = px0 as f32 + col as f32 + 0.5;
            let (u, v) = apply(inv, (x, y));
            let (c, r) = (u.floor() as i32, (top - v).floor() as i32);
            if (0..cell_w).contains(&c) && (0..UNIFONT_CELL_H).contains(&r) && set(c, r) {
                coverage[row * width + col] = 255;
            }
        }
    }
    Some(Coverage { left: px0, top: -py1, width, height, coverage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::font::{self, FontSpec};

    const W: usize = 120;
    const H: usize = 120;
    const BG: u32 = 0x00FF_FFFF;

    fn draw(ch: char, m: Matrix) -> Vec<(i32, i32)> {
        let mut buf = vec![BG; W * H];
        draw_glyph(&mut buf, W, H, 60, 60, ch, FontSpec { px: 32, ..FontSpec::SYSTEM }, m,
                   |p, c| *p = font::blend(*p, 0, c));
        buf.iter().enumerate().filter(|&(_, &p)| p != BG)
            .map(|(i, _)| ((i % W) as i32 - 60, 60 - (i / W) as i32)).collect()
    }

    fn extent(pels: &[(i32, i32)]) -> (i32, i32, i32, i32) {
        let xs = pels.iter().map(|p| p.0);
        let ys = pels.iter().map(|p| p.1);
        (xs.clone().min().unwrap(), ys.clone().min().unwrap(), xs.max().unwrap(), ys.max().unwrap())
    }

    /// The identity transform covers the pels of the upright path, within
    /// a pel: upright Latin text is hinted (ADR-0047), transformed text
    /// cannot be.
    #[test]
    fn identity_matches_upright_rendering() {
        let spec = FontSpec { px: 32, ..FontSpec::SYSTEM };
        let mut upright = vec![BG; W * H];
        font::draw_text(&mut upright, W, H, 60, 60, "H", 0, spec);
        let upright: Vec<(i32, i32)> = upright.iter().enumerate().filter(|&(_, &p)| p != BG)
            .map(|(i, _)| ((i % W) as i32 - 60, 60 - (i / W) as i32)).collect();
        let (a, b) = (extent(&upright), extent(&draw('H', IDENTITY)));
        for (u, t) in [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)] {
            assert!((u - t).abs() <= 1, "upright {a:?}, transformed {b:?}");
        }
    }

    /// A quarter turn counter-clockwise puts an "I" (tall, narrow) lying on
    /// its side to the left of the origin: wide and low.
    #[test]
    fn rotation_turns_the_glyph() {
        let upright = extent(&draw('I', IDENTITY));
        assert!(upright.3 - upright.1 > 2 * (upright.2 - upright.0), "{upright:?}");
        assert!(upright.1 >= -1, "upright I sits on the baseline");
        // (u, v) → (−v, u): up becomes left.
        let turned = extent(&draw('I', [0.0, -1.0, 1.0, 0.0]));
        assert!(turned.2 - turned.0 > 2 * (turned.3 - turned.1), "{turned:?}");
        assert!(turned.2 <= 1, "the glyph lies left of the origin: {turned:?}");
        // Half a turn: upside down, below the baseline.
        let flipped = extent(&draw('I', [-1.0, 0.0, 0.0, -1.0]));
        assert!(flipped.3 <= 1, "{flipped:?}");
    }

    /// A shear leans the top of an "I" to the right.
    #[test]
    fn shear_leans_the_glyph() {
        let pels = draw('I', [1.0, 0.5, 0.0, 1.0]);
        let (_, y0, _, y1) = extent(&pels);
        let mean_x = |y: i32| {
            let row: Vec<i32> = pels.iter().filter(|p| p.1 == y).map(|p| p.0).collect();
            row.iter().sum::<i32>() as f32 / row.len() as f32
        };
        // Half the height between the two rows sampled.
        let lean = (y1 - y0 - 2) as f32 / 2.0;
        assert!(mean_x(y1 - 1) > mean_x(y0 + 1) + lean - 1.5, "top leans right");
    }

    /// Unifont fallback glyphs are mapped too, and degenerate or extreme
    /// matrices draw nothing.
    #[test]
    fn bitmap_glyphs_and_bad_matrices() {
        let upright = draw('ᚠ', IDENTITY);
        let turned = draw('ᚠ', [0.0, -1.0, 1.0, 0.0]);
        assert!(!upright.is_empty() && !turned.is_empty());
        assert_eq!(upright.len(), turned.len(), "a quarter turn maps pels one to one");
        assert!(draw('H', [0.0; 4]).is_empty());
        assert!(draw('H', [1.0, 100.0, 0.0, 1.0]).is_empty());
        assert!(draw('H', [f32::NAN, 0.0, 0.0, 1.0]).is_empty());
        // Off the buffer: clipped, no panic.
        let mut buf = vec![BG; 16];
        draw_glyph(&mut buf, 4, 4, -500, 9000, 'H', FontSpec::SYSTEM, [0.0, -1.0, 1.0, 0.0], |p, _| *p = 0);
    }

    #[test]
    fn cache_is_bounded() {
        let spec = FontSpec::SYSTEM;
        let mut buf = vec![BG; 16];
        for i in 0..(TRANSFORMED_CACHE_MAX + 10) {
            let a = 1.0 + i as f32 / 4096.0;
            draw_glyph(&mut buf, 4, 4, 0, 0, '.', spec, [a, 0.0, 0.0, 1.0], |_, _| {});
            assert!(cache_len() <= TRANSFORMED_CACHE_MAX);
        }
    }
}
