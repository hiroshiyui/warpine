// SPDX-License-Identifier: GPL-3.0-only
//
// Per-frame drawing surfaces shared between the API threads and the GUI
// thread.
//
// PM drawing (GPI primitives, `WinFillRect`, built-in control painting) is
// rasterised on the calling API thread straight into the surface of the frame
// window it targets, so the result is visible to the application at once:
// `GpiQueryPel` reads it back and destination-dependent mix modes (XOR,
// invert…) combine with what is already there. The GUI thread only composites
// surfaces onto the desktop; the drawing call marks its frame dirty to tell it
// what changed.
//
// Locking: surfaces live in `WindowManager::surfaces` as `Arc<Mutex<Surface>>`.
// The window-manager lock may be held while a surface is locked, never the
// other way round, so drawing and compositing cannot deadlock.

use std::sync::{Arc, Mutex};

use super::damage::{Rect, fill_clipped};
use super::font::{self, FontSpec};
use crate::loader::constants::{
    FM_AND, FM_DEFAULT, FM_INVERT, FM_LEAVEALONE, FM_MASKSRCNOT, FM_MERGENOTSRC, FM_MERGESRCNOT,
    FM_NOTCOPYSRC, FM_NOTMASKSRC, FM_NOTMERGESRC, FM_NOTXORSRC, FM_ONE, FM_OR, FM_OVERPAINT,
    FM_SUBTRACT, FM_XOR, FM_ZERO,
};

/// Colour of a newly allocated (never painted) surface, and what
/// [`DrawOp::Clear`] fills it with.
pub const SURFACE_BLANK: u32 = 0xFFFF_FFFF;

/// Only the colour channels take part in raster operations.
const RGB_MASK: u32 = 0x00FF_FFFF;

/// Text drawn with a mix other than overpaint has no anti-aliasing: a pel is
/// part of a glyph when its coverage reaches this value (half).
const MIX_TEXT_COVERAGE: u8 = 128;

/// Largest surface width or height. Sizes come from the guest
/// (`WinSetWindowPos`); this caps the allocation at 256 MiB.
pub const MAX_SURFACE_DIM: u32 = 8192;

/// Text positions are clamped to ± this, far outside any surface, so that
/// glyph arithmetic on guest-supplied coordinates cannot overflow.
const TEXT_COORD_LIMIT: i32 = 1 << 24;

/// A solid line style for [`DrawOp::Line`].
pub const SOLID_LINE: u16 = 0xFFFF;

/// Ternary raster operation `rop` (`ROP_*`) on pattern `p`, source `s` and
/// destination `d` (0x00RRGGBB): bit `(P<<2)|(S<<1)|D` of `rop` is the result
/// bit for that combination, applied to every bit of the pels.
pub fn rop3(rop: u8, p: u32, s: u32, d: u32) -> u32 {
    let mut out = 0;
    for i in 0..8 {
        if rop & (1 << i) != 0 {
            let pick = |bit: u32, v: u32| if i & bit != 0 { v } else { !v };
            out |= pick(4, p) & pick(2, s) & pick(1, d);
        }
    }
    out & RGB_MASK
}

/// A GPI foreground mix: how the source colour S combines with the
/// destination pel D (`FM_*` in `<pmgpi.h>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mix {
    Or, Overpaint, Xor, LeaveAlone, And, Subtract, MaskSrcNot, Zero,
    NotMergeSrc, NotXorSrc, Invert, MergeSrcNot, NotCopySrc, MergeNotSrc,
    NotMaskSrc, One,
}

impl Mix {
    /// The mix for an `FM_*` value; `None` for values that are not mix modes.
    pub fn from_fm(fm: u32) -> Option<Mix> {
        Some(match fm {
            FM_DEFAULT | FM_OVERPAINT => Mix::Overpaint,
            FM_OR          => Mix::Or,
            FM_XOR         => Mix::Xor,
            FM_LEAVEALONE  => Mix::LeaveAlone,
            FM_AND         => Mix::And,
            FM_SUBTRACT    => Mix::Subtract,
            FM_MASKSRCNOT  => Mix::MaskSrcNot,
            FM_ZERO        => Mix::Zero,
            FM_NOTMERGESRC => Mix::NotMergeSrc,
            FM_NOTXORSRC   => Mix::NotXorSrc,
            FM_INVERT      => Mix::Invert,
            FM_MERGESRCNOT => Mix::MergeSrcNot,
            FM_NOTCOPYSRC  => Mix::NotCopySrc,
            FM_MERGENOTSRC => Mix::MergeNotSrc,
            FM_NOTMASKSRC  => Mix::NotMaskSrc,
            FM_ONE         => Mix::One,
            _ => return None,
        })
    }

    /// Combine source colour `s` with destination pel `d` (both 0x00RRGGBB).
    pub fn apply(self, s: u32, d: u32) -> u32 {
        let v = match self {
            Mix::Or          => s | d,
            Mix::Overpaint   => s,
            Mix::Xor         => s ^ d,
            Mix::LeaveAlone  => d,
            Mix::And         => s & d,
            Mix::Subtract    => !s & d,
            Mix::MaskSrcNot  => s & !d,
            Mix::Zero        => 0,
            Mix::NotMergeSrc => !(s | d),
            Mix::NotXorSrc   => !(s ^ d),
            Mix::Invert      => !d,
            Mix::MergeSrcNot => s | !d,
            Mix::NotCopySrc  => !s,
            Mix::MergeNotSrc => !s | d,
            Mix::NotMaskSrc  => !(s & d),
            Mix::One         => !0,
        };
        v & RGB_MASK
    }
}

/// One drawing operation. Coordinates are PM coordinates of the surface:
/// origin at the bottom-left, y growing upwards, rectangle corners inclusive.
#[derive(Clone, Debug, PartialEq)]
pub enum DrawOp {
    /// Rectangle with corners `(x1, y1)` and `(x2, y2)` in any order; filled
    /// or outlined.
    Box { x1: i32, y1: i32, x2: i32, y2: i32, color: u32, fill: bool, mix: Mix },
    /// Line from `(x1, y1)` to `(x2, y2)`, both end points included, drawn
    /// with the 16-pel repeating `style` mask (MSB first; [`SOLID_LINE`]).
    Line { x1: i32, y1: i32, x2: i32, y2: i32, color: u32, mix: Mix, style: u16 },
    /// System-font text whose 16-px line box has its bottom at `y` (how the
    /// built-in controls place labels). Anti-aliased, overpaint.
    Text { x: i32, y: i32, text: String, color: u32 },
    /// Text in `font` with its baseline at `y` (the GPI convention).
    TextFont { x: i32, y: i32, text: String, color: u32, font: FontSpec, mix: Mix },
    /// One pel.
    Pel { x: i32, y: i32, color: u32, mix: Mix },
    /// Raster-operation blit onto the `w` × `h` area with lower-left corner
    /// `(x, y)`: each pel becomes [`rop3`]`(rop, pattern, S, D)`. `src`
    /// holds the source pels bottom row first (`src[row * w + col]`); a
    /// `None` pel (outside the source) leaves the destination unchanged.
    /// Without `src`, S is 0 (for operations that ignore the source).
    Blit { x: i32, y: i32, w: u32, h: u32, src: Option<Vec<Option<u32>>>, rop: u8, pattern: u32 },
    /// Fill the whole surface with [`SURFACE_BLANK`].
    Clear,
}

/// The pixels of one frame window's client area (0x00RRGGBB, top row first).
pub struct Surface {
    pub buffer: Vec<u32>,
    pub width:  u32,
    pub height: u32,
    /// Test builds can record every operation applied, so tests can check
    /// what was drawn as well as the resulting pixels.
    #[cfg(test)]
    pub record: Option<Vec<DrawOp>>,
}

/// A surface shared between the drawing API threads and the compositor.
pub type SharedSurface = Arc<Mutex<Surface>>;

impl Surface {
    /// A blank surface; each dimension is capped at [`MAX_SURFACE_DIM`].
    pub fn new(width: u32, height: u32) -> Self {
        let (width, height) = (width.min(MAX_SURFACE_DIM), height.min(MAX_SURFACE_DIM));
        let n = width as usize * height as usize;
        Surface {
            buffer: vec![SURFACE_BLANK; n], width, height,
            #[cfg(test)]
            record: None,
        }
    }

    pub fn shared(width: u32, height: u32) -> SharedSurface {
        Arc::new(Mutex::new(Surface::new(width, height)))
    }

    /// Change the size, keeping the old content where it still fits.
    ///
    /// PM coordinates start at the bottom-left corner, so the content stays
    /// anchored there: the bottom rows are kept when the height changes, the
    /// left columns when the width changes. Areas the old surface did not
    /// cover are blank until the application repaints them; without this, an
    /// interactive resize would show a blank window until then.
    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.min(MAX_SURFACE_DIM), height.min(MAX_SURFACE_DIM));
        if (width, height) == (self.width, self.height) { return; }
        let mut next = vec![SURFACE_BLANK; width as usize * height as usize];
        let (ow, oh) = (self.width as i32, self.height as i32);
        // Old row r (from the top) holds PM y = oh-1-r; that y is new row
        // r + (height - oh).
        let shift = height as i32 - oh;
        super::damage::blit_clipped(&mut next, width as usize, &self.buffer, ow.max(1) as usize,
                                    0, shift, ow, oh, Rect::at(0, 0, width as i32, height as i32));
        self.buffer = next;
        self.width = width;
        self.height = height;
    }

    /// Screen row (top-first) of PM row `y`.
    fn row(&self, y: i32) -> i32 {
        self.height as i32 - 1 - y
    }

    fn bounds(&self) -> Rect {
        Rect::at(0, 0, self.width as i32, self.height as i32)
    }

    /// Apply `mix` with colour `color` to every pel of `r` (screen
    /// coordinates), clipped to the surface.
    fn mix_rect(&mut self, r: Rect, color: u32, mix: Mix) {
        let r = r.intersect(&self.bounds());
        if r.is_empty() { return; }
        if mix == Mix::Overpaint {
            fill_clipped(&mut self.buffer, self.width as usize, r, r, color & RGB_MASK);
            return;
        }
        let w = self.width as usize;
        for y in r.y0..r.y1 {
            let row = &mut self.buffer[y as usize * w..][r.x0 as usize..r.x1 as usize];
            for p in row {
                *p = mix.apply(color, *p);
            }
        }
    }

    /// Apply `mix` to the pel at PM `(x, y)`; outside the surface does nothing.
    fn mix_pel(&mut self, x: i32, y: i32, color: u32, mix: Mix) {
        if let Some(i) = self.index(x, y) {
            self.buffer[i] = mix.apply(color, self.buffer[i]);
        }
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height)
            .then(|| self.row(y) as usize * self.width as usize + x as usize)
    }

    /// Clamp a PM coordinate to just outside the surface on either side.
    /// Nothing visible changes (a clamped edge stays off the surface), and
    /// arithmetic on the result cannot overflow.
    fn clamp(&self, v: i32) -> i32 {
        v.clamp(-1, self.width.max(self.height) as i32 + 1)
    }

    /// Clip the line `(x1, y1)`–`(x2, y2)` to one pel outside the surface
    /// (Liang–Barsky), so drawing it costs at most the surface's size even
    /// for guest coordinates near the `i32` limits. Lines with both ends in
    /// that area are returned unchanged and drawn pel-exact; `None` when the
    /// line misses it.
    fn clip_line(&self, x1: i32, y1: i32, x2: i32, y2: i32) -> Option<(i32, i32, i32, i32)> {
        let (xmin, ymin) = (-1.0, -1.0);
        let (xmax, ymax) = (self.width as f64, self.height as f64);
        let inside = |x: i32, y: i32| {
            (xmin..=xmax).contains(&(x as f64)) && (ymin..=ymax).contains(&(y as f64))
        };
        if inside(x1, y1) && inside(x2, y2) {
            return Some((x1, y1, x2, y2));
        }
        let (fx, fy) = (x1 as f64, y1 as f64);
        let (dx, dy) = (x2 as f64 - fx, y2 as f64 - fy);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for (p, q) in [(-dx, fx - xmin), (dx, xmax - fx), (-dy, fy - ymin), (dy, ymax - fy)] {
            if p == 0.0 {
                if q < 0.0 { return None; }
                continue;
            }
            let r = q / p;
            if p < 0.0 {
                if r > t1 { return None; }
                t0 = t0.max(r);
            } else {
                if r < t0 { return None; }
                t1 = t1.min(r);
            }
        }
        let at = |t: f64| ((fx + t * dx).round() as i32, (fy + t * dy).round() as i32);
        let ((ax, ay), (bx, by)) = (at(t0), at(t1));
        Some((ax, ay, bx, by))
    }

    /// Overwrite PM row `y` from column 0 with `pels` (clipped).
    pub fn put_row(&mut self, y: i32, pels: &[u32]) {
        if let Some(i) = self.index(0, y) {
            let n = pels.len().min(self.width as usize);
            for (d, &p) in self.buffer[i..i + n].iter_mut().zip(pels) {
                *d = p & RGB_MASK;
            }
        }
    }

    /// PM row `y` (`width` pels); `None` outside the surface.
    pub fn row_pels(&self, y: i32) -> Option<Vec<u32>> {
        let i = self.index(0, y)?;
        Some(self.buffer[i..i + self.width as usize].iter().map(|p| p & RGB_MASK).collect())
    }

    /// The pel at PM `(x, y)` as 0x00RRGGBB; `None` outside the surface.
    pub fn pel(&self, x: i32, y: i32) -> Option<u32> {
        self.index(x, y).map(|i| self.buffer[i] & RGB_MASK)
    }

    /// Apply `op`.
    pub fn draw(&mut self, op: DrawOp) {
        self.apply(&op);
        #[cfg(test)]
        if let Some(rec) = &mut self.record {
            rec.push(op);
        }
    }

    fn apply(&mut self, op: &DrawOp) {
        match *op {
            DrawOp::Box { x1, y1, x2, y2, color, fill, mix } => {
                let (x1, y1, x2, y2) = (self.clamp(x1), self.clamp(y1), self.clamp(x2), self.clamp(y2));
                let (left, right) = (x1.min(x2), x1.max(x2));
                let (bottom, top) = (y1.min(y2), y1.max(y2));
                let (sy0, sy1) = (self.row(top), self.row(bottom) + 1);
                if fill {
                    self.mix_rect(Rect::new(left, sy0, right + 1, sy1), color, mix);
                } else {
                    // Top and bottom rows in full, the sides between them, so
                    // no pel is mixed twice (it matters for XOR).
                    self.mix_rect(Rect::new(left, sy0, right + 1, sy0 + 1), color, mix);
                    if sy1 - 1 > sy0 {
                        self.mix_rect(Rect::new(left, sy1 - 1, right + 1, sy1), color, mix);
                    }
                    if sy1 - 1 > sy0 + 1 {
                        self.mix_rect(Rect::new(left, sy0 + 1, left + 1, sy1 - 1), color, mix);
                        if right > left {
                            self.mix_rect(Rect::new(right, sy0 + 1, right + 1, sy1 - 1), color, mix);
                        }
                    }
                }
            }
            DrawOp::Line { x1, y1, x2, y2, color, mix, style } => {
                let Some((x1, y1, x2, y2)) = self.clip_line(x1, y1, x2, y2) else { return };
                // Bresenham; each pel is visited once.
                let (dx, dy) = ((x2 - x1).abs(), (y2 - y1).abs());
                let (sx, sy) = (if x1 < x2 { 1 } else { -1 }, if y1 < y2 { 1 } else { -1 });
                let (mut x, mut y, mut err) = (x1, y1, dx - dy);
                let mut n = 0u32;
                loop {
                    if style & (0x8000 >> (n % 16)) != 0 {
                        self.mix_pel(x, y, color, mix);
                    }
                    n += 1;
                    if x == x2 && y == y2 { break; }
                    let e2 = 2 * err;
                    if e2 > -dy { err -= dy; x += sx; }
                    if e2 < dx { err += dx; y += sy; }
                }
            }
            DrawOp::Text { x, y, ref text, color } => {
                let spec = FontSpec::SYSTEM;
                let descent = font::metrics(spec).descent;
                self.text(x, y.saturating_add(descent), text, color, spec, Mix::Overpaint);
            }
            DrawOp::TextFont { x, y, ref text, color, font, mix } => {
                self.text(x, y, text, color, font, mix);
            }
            DrawOp::Pel { x, y, color, mix } => self.mix_pel(x, y, color, mix),
            DrawOp::Blit { x, y, w, h, ref src, rop, pattern } => {
                if src.as_ref().is_some_and(|v| v.len() != w as usize * h as usize) { return; }
                // Only the part of the area on the surface (i64: x, y, w, h
                // come from the guest).
                let cols = (-(x as i64)).max(0)..(w as i64).min(self.width as i64 - x as i64);
                let rows = (-(y as i64)).max(0)..(h as i64).min(self.height as i64 - y as i64);
                for r in rows {
                    for c in cols.clone() {
                        let s = match src {
                            Some(v) => match v[r as usize * w as usize + c as usize] {
                                Some(s) => s,
                                None => continue,
                            },
                            None => 0,
                        };
                        if let Some(i) = self.index((x as i64 + c) as i32, (y as i64 + r) as i32) {
                            self.buffer[i] = rop3(rop, pattern, s, self.buffer[i]);
                        }
                    }
                }
            }
            DrawOp::Clear => self.buffer.fill(SURFACE_BLANK),
        }
    }

    /// Draw `text` with its baseline on PM row `y`.
    fn text(&mut self, x: i32, y: i32, text: &str, color: u32, spec: FontSpec, mix: Mix) {
        if self.width == 0 || self.height == 0 { return; }
        let (x, y) = (x.clamp(-TEXT_COORD_LIMIT, TEXT_COORD_LIMIT),
                      y.clamp(-TEXT_COORD_LIMIT, TEXT_COORD_LIMIT));
        // PM row `y` is screen row `height - 1 - y`; glyphs rest on that row,
        // so the baseline (the boundary under it) is screen y `height - y`.
        let baseline = self.height as i32 - y;
        let (w, h) = (self.width as usize, self.height as usize);
        if mix == Mix::Overpaint {
            font::draw_text(&mut self.buffer, w, h, x, baseline, text, color, spec);
        } else {
            font::draw_text_with(&mut self.buffer, w, h, x, baseline, text, spec, |p, cov| {
                if cov >= MIX_TEXT_COVERAGE {
                    *p = mix.apply(color, *p);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: u32 = 0x00FF_0000;
    const WHITE: u32 = 0x00FF_FFFF;

    fn count(s: &Surface, color: u32) -> usize {
        s.buffer.iter().filter(|&&p| p & RGB_MASK == color).count()
    }

    fn painted_rows(s: &Surface) -> (i32, i32) {
        let rows: Vec<i32> = s.buffer.iter().enumerate()
            .filter(|&(_, &p)| p & RGB_MASK != WHITE)
            .map(|(i, _)| (i as u32 / s.width) as i32).collect();
        (*rows.iter().min().unwrap(), *rows.iter().max().unwrap())
    }

    fn fill(x1: i32, y1: i32, x2: i32, y2: i32, color: u32, mix: Mix) -> DrawOp {
        DrawOp::Box { x1, y1, x2, y2, color, fill: true, mix }
    }

    #[test]
    fn mix_modes_follow_their_raster_operations() {
        let (s, d) = (0x00F0_F00F, 0x00CC_33AA);
        let cases = [
            (FM_OR, s | d), (FM_OVERPAINT, s), (FM_XOR, s ^ d), (FM_LEAVEALONE, d),
            (FM_AND, s & d), (FM_SUBTRACT, !s & d), (FM_MASKSRCNOT, s & !d), (FM_ZERO, 0),
            (FM_NOTMERGESRC, !(s | d)), (FM_NOTXORSRC, !(s ^ d)), (FM_INVERT, !d),
            (FM_MERGESRCNOT, s | !d), (FM_NOTCOPYSRC, !s), (FM_MERGENOTSRC, !s | d),
            (FM_NOTMASKSRC, !(s & d)), (FM_ONE, !0),
        ];
        for (fm, want) in cases {
            assert_eq!(Mix::from_fm(fm).unwrap().apply(s, d), want & RGB_MASK, "FM {fm}");
        }
        assert_eq!(Mix::from_fm(FM_DEFAULT), Some(Mix::Overpaint));
        assert_eq!(Mix::from_fm(3), None, "3 is not a mix mode");
        assert_eq!(Mix::from_fm(18), None, "BM_SRCTRANSPARENT is background-only");
    }

    #[test]
    fn filled_and_outlined_boxes() {
        let mut s = Surface::new(20, 20);
        s.draw(fill(5, 5, 14, 14, RED, Mix::Overpaint));
        assert_eq!(count(&s, RED), 100);
        let mut s = Surface::new(20, 20);
        s.draw(DrawOp::Box { x1: 14, y1: 14, x2: 5, y2: 5, color: RED, fill: false, mix: Mix::Overpaint });
        assert_eq!(count(&s, RED), 36, "corners in either order");
    }

    #[test]
    fn y_runs_upwards_from_the_bottom_row() {
        let mut s = Surface::new(10, 10);
        s.draw(fill(0, 0, 9, 0, RED, Mix::Overpaint));
        assert!(s.buffer[90..100].iter().all(|&p| p == RED), "PM y=0 is the bottom screen row");
        assert_eq!(s.pel(3, 0), Some(RED));
        assert_eq!(s.pel(3, 1), Some(WHITE));
    }

    /// The old renderer clamped coordinates, so a box entirely off the
    /// surface painted its edge; it must draw nothing.
    #[test]
    fn primitives_outside_the_surface_draw_nothing() {
        let mut s = Surface::new(10, 10);
        s.draw(fill(-20, -20, -5, -5, RED, Mix::Overpaint));
        s.draw(fill(30, 2, 40, 4, RED, Mix::Overpaint));
        s.draw(DrawOp::Line { x1: -5, y1: 20, x2: 20, y2: 20, color: RED, mix: Mix::Overpaint, style: SOLID_LINE });
        s.draw(DrawOp::Pel { x: 10, y: 0, color: RED, mix: Mix::Overpaint });
        assert_eq!(count(&s, RED), 0);
        // Partly inside: only the inside part.
        s.draw(fill(-5, -5, 1, 1, RED, Mix::Overpaint));
        assert_eq!(count(&s, RED), 4);
        assert_eq!(s.pel(-1, 0), None);
        assert_eq!(s.pel(0, 10), None);
    }

    /// Guest coordinates at the `i32` limits neither overflow nor take long:
    /// what lies on the surface is drawn, the rest is clipped.
    #[test]
    fn extreme_coordinates_are_clipped_safely() {
        let (lo, hi) = (i32::MIN, i32::MAX);
        let mut s = Surface::new(10, 10);
        let started = std::time::Instant::now();
        for op in [
            fill(lo, lo, hi, hi, RED, Mix::Overpaint),
            DrawOp::Box { x1: lo, y1: lo, x2: hi, y2: hi, color: RED, fill: false, mix: Mix::Xor },
            DrawOp::Line { x1: lo, y1: lo, x2: hi, y2: hi, color: 0, mix: Mix::Overpaint, style: SOLID_LINE },
            DrawOp::Line { x1: hi, y1: 5, x2: lo, y2: 5, color: 0, mix: Mix::Overpaint, style: SOLID_LINE },
            DrawOp::Line { x1: lo, y1: lo, x2: lo, y2: hi, color: 0, mix: Mix::Overpaint, style: SOLID_LINE },
            DrawOp::Pel { x: lo, y: hi, color: 0, mix: Mix::Overpaint },
            DrawOp::Text { x: hi, y: hi, text: "x".into(), color: 0 },
            DrawOp::TextFont { x: lo, y: lo, text: "x".into(), color: 0,
                               font: FontSpec::SYSTEM, mix: Mix::Xor },
        ] {
            s.draw(op);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(count(&s, RED), 100 - 10 - 9, "fill everywhere, then the horizontal line (y=5) and the diagonal");
        assert_eq!(s.pel(0, 0), Some(0), "the diagonal passes through the corner");
        assert_eq!(s.pel(9, 5), Some(0));
        assert_eq!(s.pel(-1, i32::MIN), None);
    }

    #[test]
    fn surfaces_are_capped_at_the_maximum_size() {
        let s = Surface::new(u32::MAX, 3);
        assert_eq!((s.width, s.height), (MAX_SURFACE_DIM, 3));
        let mut s = Surface::new(2, 2);
        s.resize(5, u32::MAX);
        assert_eq!((s.width, s.height), (5, MAX_SURFACE_DIM));
        assert_eq!(s.buffer.len(), 5 * MAX_SURFACE_DIM as usize);
    }

    #[test]
    fn rop3_matches_the_named_raster_operations() {
        use crate::loader::constants::*;
        let (p, s, d) = (0x00F0_F0F0u32, 0x00CC_CCCC, 0x00AA_AAAA);
        let r = |rop: u32| rop3(rop as u8, p, s, d);
        assert_eq!(r(ROP_SRCCOPY), s);
        assert_eq!(r(ROP_SRCPAINT), s | d);
        assert_eq!(r(ROP_SRCAND), s & d);
        assert_eq!(r(ROP_SRCINVERT), s ^ d);
        assert_eq!(r(ROP_NOTSRCCOPY), !s & RGB_MASK);
        assert_eq!(r(ROP_PATCOPY), p);
        assert_eq!(r(ROP_PATINVERT), p ^ d);
        assert_eq!(r(ROP_DSTINVERT), !d & RGB_MASK);
        assert_eq!(r(ROP_ZERO), 0);
        assert_eq!(r(ROP_ONE), RGB_MASK);
    }

    #[test]
    fn blit_applies_the_rop_inside_the_surface_only() {
        let mut s = Surface::new(4, 4);
        // A 3×2 source, bottom row first, with one hole.
        let src = vec![Some(1), Some(2), None, Some(4), Some(5), Some(6)];
        s.draw(DrawOp::Blit { x: 2, y: 0, w: 3, h: 2, src: Some(src), rop: 0xCC, pattern: 0 });
        assert_eq!(s.pel(2, 0), Some(1));
        assert_eq!(s.pel(3, 0), Some(2));
        assert_eq!(s.pel(3, 1), Some(5));
        assert_eq!(s.pel(2, 1), Some(4));
        assert_eq!(s.pel(1, 0), Some(WHITE), "left of the area");
        // No source: pattern copy, and a mismatched source is ignored.
        s.draw(DrawOp::Blit { x: -1, y: 3, w: 9, h: 9, src: None, rop: 0xF0, pattern: RED });
        assert_eq!(s.row_pels(3), Some(vec![RED; 4]));
        s.draw(DrawOp::Blit { x: 0, y: 0, w: 2, h: 2, src: Some(vec![Some(7)]), rop: 0xCC, pattern: 0 });
        assert_eq!(s.pel(0, 0), Some(WHITE));
        // Guest extremes do not overflow.
        s.draw(DrawOp::Blit { x: i32::MIN, y: i32::MAX, w: u32::MAX, h: 1, src: None, rop: 0xF0, pattern: 0 });
    }

    #[test]
    fn line_styles_skip_pels() {
        let mut s = Surface::new(16, 1);
        s.draw(DrawOp::Line { x1: 0, y1: 0, x2: 15, y2: 0, color: RED, mix: Mix::Overpaint, style: 0xAAAA });
        assert_eq!(count(&s, RED), 8);
        assert_eq!(s.pel(0, 0), Some(RED));
        assert_eq!(s.pel(1, 0), Some(WHITE));
    }

    #[test]
    fn rows_can_be_written_and_read() {
        let mut s = Surface::new(3, 2);
        s.put_row(1, &[1, 2, 3, 4]);
        assert_eq!(s.row_pels(1), Some(vec![1, 2, 3]));
        assert_eq!(s.pel(0, 1), Some(1));
        assert_eq!(s.row_pels(2), None);
        s.put_row(-1, &[9]);
    }

    #[test]
    fn lines_include_both_end_points() {
        for (x1, y1, x2, y2, n) in [(0, 5, 19, 5, 20), (5, 0, 5, 9, 10), (0, 0, 9, 9, 10), (9, 9, 0, 0, 10)] {
            let mut s = Surface::new(20, 10);
            s.draw(DrawOp::Line { x1, y1, x2, y2, color: RED, mix: Mix::Overpaint, style: SOLID_LINE });
            assert_eq!(count(&s, RED), n, "line {x1},{y1} → {x2},{y2}");
            assert_eq!(s.pel(x1, y1), Some(RED));
            assert_eq!(s.pel(x2, y2), Some(RED));
        }
    }

    /// XOR drawing twice restores the original pixels: outline corners and
    /// line pels are touched exactly once each.
    #[test]
    fn xor_twice_restores_the_surface() {
        let mut s = Surface::new(30, 30);
        s.draw(fill(0, 0, 29, 29, 0x0012_3456, Mix::Overpaint));
        let before = s.buffer.clone();
        let ops = [
            fill(2, 2, 20, 10, 0x00FF_00FF, Mix::Xor),
            DrawOp::Box { x1: 5, y1: 5, x2: 25, y2: 25, color: 0x0000_FFFF, fill: false, mix: Mix::Xor },
            DrawOp::Box { x1: 7, y1: 7, x2: 7, y2: 20, color: 0x0000_FFFF, fill: false, mix: Mix::Xor },
            DrawOp::Line { x1: 0, y1: 29, x2: 29, y2: 3, color: 0x00FF_FFFF, mix: Mix::Xor, style: SOLID_LINE },
            DrawOp::Pel { x: 4, y: 4, color: 0x0080_8080, mix: Mix::Xor },
            DrawOp::TextFont { x: 2, y: 12, text: "XOR".into(), color: 0x00FF_FFFF,
                               font: FontSpec::SYSTEM, mix: Mix::Xor },
        ];
        for op in ops.iter().chain(ops.iter()) {
            s.draw(op.clone());
        }
        assert_eq!(s.buffer.iter().map(|p| p & RGB_MASK).collect::<Vec<_>>(),
                   before.iter().map(|p| p & RGB_MASK).collect::<Vec<_>>());
    }

    #[test]
    fn invert_and_leave_alone_ignore_the_colour() {
        let mut s = Surface::new(4, 4);
        s.draw(fill(0, 0, 3, 3, 0x0010_2030, Mix::Overpaint));
        s.draw(fill(0, 0, 1, 1, RED, Mix::Invert));
        s.draw(fill(2, 2, 3, 3, RED, Mix::LeaveAlone));
        assert_eq!(s.pel(0, 0), Some(0x00EF_DFCF));
        assert_eq!(s.pel(3, 3), Some(0x0010_2030));
    }

    #[test]
    fn text_is_anti_aliased_only_when_overpainting() {
        let mut s = Surface::new(40, 30);
        s.draw(DrawOp::TextFont { x: 2, y: 8, text: "Wg".into(), color: 0,
                                  font: FontSpec::SYSTEM, mix: Mix::Overpaint });
        let shades = s.buffer.iter().filter(|&&p| p & RGB_MASK != 0 && p & RGB_MASK != WHITE).count();
        assert!(shades > 0, "overpainted text is blended");
        let mut s = Surface::new(40, 30);
        s.draw(DrawOp::TextFont { x: 2, y: 8, text: "Wg".into(), color: WHITE,
                                  font: FontSpec::SYSTEM, mix: Mix::Xor });
        assert!(s.buffer.iter().all(|&p| p & RGB_MASK == 0 || p & RGB_MASK == WHITE),
                "mixed text has whole pels only");
        assert!(count(&s, 0) > 0);
    }

    /// `Text` places the 16-px line box with its bottom at `y`; `TextFont`
    /// places the baseline there.
    #[test]
    fn text_line_box_and_baseline_placement() {
        let (w, h) = (32u32, 64u32);
        let y = 20;
        let mut s = Surface::new(w, h);
        s.draw(DrawOp::Text { x: 0, y, text: "Tg".into(), color: 0 });
        let (top, bot) = painted_rows(&s);
        assert!(top >= h as i32 - y - 16 && bot < h as i32 - y, "top={top} bot={bot}");

        let text_font = |text: &str| {
            let mut s = Surface::new(w, h);
            s.draw(DrawOp::TextFont { x: 0, y, text: text.into(), color: 0,
                                      font: FontSpec::SYSTEM, mix: Mix::Overpaint });
            painted_rows(&s)
        };
        // A capital rests on PM row `y`, i.e. screen row h - 1 - y…
        assert_eq!(text_font("T").1, h as i32 - 1 - y);
        // …while a descender reaches below it.
        assert!(text_font("g").1 > h as i32 - 1 - y);
        // And the text is right side up: the bar of a T is above its stem.
        let mut s = Surface::new(16, 32);
        s.draw(DrawOp::TextFont { x: 0, y: 8, text: "T".into(), color: 0,
                                  font: FontSpec::SYSTEM, mix: Mix::Overpaint });
        let (top, bot) = painted_rows(&s);
        let dark = |row: i32| (0..16).filter(|&x| s.buffer[(row * 16 + x) as usize] & 0xFF < 0x80).count();
        assert!(dark(top) > dark((top + bot) / 2));
    }

    #[test]
    fn clear_blanks_the_surface() {
        let mut s = Surface::new(5, 5);
        s.draw(fill(0, 0, 4, 4, RED, Mix::Overpaint));
        s.draw(DrawOp::Clear);
        assert!(s.buffer.iter().all(|&p| p == SURFACE_BLANK));
    }

    /// A surface whose pixel at (column c, row r from the top) is `100 * r + c`.
    fn numbered(w: u32, h: u32) -> Surface {
        let mut s = Surface::new(w, h);
        for (i, p) in s.buffer.iter_mut().enumerate() {
            *p = (100 * (i / w as usize) + i % w as usize) as u32;
        }
        s
    }

    #[test]
    fn resize_keeps_the_bottom_left_content() {
        // Grow 3×2 → 4×3: the old rows end up at the bottom, left-aligned.
        let mut s = numbered(3, 2);
        s.resize(4, 3);
        let b = SURFACE_BLANK;
        assert_eq!((s.width, s.height), (4, 3));
        assert_eq!(s.buffer, vec![b, b, b, b,
                                  0, 1, 2, b,
                                  100, 101, 102, b]);
        // Shrink 3×3 → 2×2: the bottom-left 2×2 survives.
        let mut s = numbered(3, 3);
        s.resize(2, 2);
        assert_eq!(s.buffer, vec![100, 101, 200, 201]);
        // Same size: untouched.
        let mut s = numbered(2, 2);
        s.resize(2, 2);
        assert_eq!(s.buffer, numbered(2, 2).buffer);
    }

    #[test]
    fn recording_keeps_the_applied_operations() {
        let mut s = Surface::new(4, 4);
        s.draw(DrawOp::Clear);
        s.record = Some(Vec::new());
        s.draw(DrawOp::Pel { x: 1, y: 1, color: RED, mix: Mix::Overpaint });
        assert_eq!(s.record.as_deref(), Some(&[DrawOp::Pel { x: 1, y: 1, color: RED, mix: Mix::Overpaint }][..]));
    }
}
