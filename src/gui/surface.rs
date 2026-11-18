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
use super::region::{self, FillRule, Region};
use super::font::{self, CjkRegion, FontSpec};
use super::glyph_transform;
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

/// Most glyphs one [`DrawOp::GlyphRun`] draws (the GPI string limit).
pub const MAX_RUN_GLYPHS: usize = 4096;

/// A solid line style for [`DrawOp::Line`].
pub const SOLID_LINE: u16 = 0xFFFF;

/// Widest stroke drawn, in pels. Widths come from the guest
/// (`GpiSetLineWidth`); this bounds the work per pel of a line.
pub const MAX_STROKE_WIDTH: u32 = 32;

/// Most points in one [`DrawOp::Shape`]; longer paths are cut off. Arcs
/// are generated with far fewer.
pub const MAX_SHAPE_POINTS: usize = 8192;

/// Widest boundary brush of a [`DrawOp::Figures`] (the geometric width of
/// `GpiStrokePath`), in pels; bounds the work per pel of the outline.
pub const MAX_BOUNDARY_WIDTH: u32 = 64;

/// Shape coordinates are clamped to ± this, far outside any surface, so the
/// rasteriser's arithmetic cannot overflow.
const SHAPE_COORD_LIMIT: i32 = 1 << 24;

/// An 8 × 8 area pattern repeated across the surface. Row `r` is the PM row
/// `origin.1 + r` (mod 8), bit 7 of a row its leftmost pel (`origin.0`).
/// Set bits take the foreground colour, clear bits the background colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub rows: [u8; 8],
    pub fg: u32,
    pub bg: u32,
    pub origin: (i32, i32),
}

impl Pattern {
    /// Every pel set.
    pub const SOLID_ROWS: [u8; 8] = [0xFF; 8];

    /// A solid pattern of `color`.
    pub fn solid(color: u32) -> Pattern {
        Pattern { rows: Self::SOLID_ROWS, fg: color, bg: color, origin: (0, 0) }
    }

    pub fn is_solid(&self) -> bool {
        self.rows == Self::SOLID_ROWS
    }

    /// Whether the pattern is set at PM `(x, y)`.
    pub fn bit(&self, x: i32, y: i32) -> bool {
        let r = (y as i64 - self.origin.1 as i64).rem_euclid(8) as usize;
        let c = (x as i64 - self.origin.0 as i64).rem_euclid(8);
        self.rows[r] & (0x80 >> c) != 0
    }

    /// The pattern's colour at PM `(x, y)`.
    pub fn color_at(&self, x: i32, y: i32) -> u32 {
        if self.bit(x, y) { self.fg } else { self.bg }
    }
}

/// How the inside of a [`DrawOp::Shape`] is painted: pattern pels that are
/// set with `mix` and the foreground colour, clear ones with `back_mix` and
/// the background colour (`None` leaves them alone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AreaFill {
    pub pattern: Pattern,
    pub mix: Mix,
    pub back_mix: Option<Mix>,
}

/// How the outline of a [`DrawOp::Shape`] is drawn: `width` pels wide (a
/// square brush centred on the path), with the 16-pel repeating `style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stroke {
    pub color: u32,
    pub mix: Mix,
    pub style: u16,
    pub width: u32,
}

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
    /// built-in controls place labels), with CJK characters from the `cjk`
    /// regional face (the codepage of the window's queue). Anti-aliased,
    /// overpaint.
    Text { x: i32, y: i32, text: String, color: u32, cjk: CjkRegion },
    /// Text in `font` with its baseline at `y` (the GPI convention).
    TextFont { x: i32, y: i32, text: String, color: u32, font: FontSpec, mix: Mix },
    /// Characters in `font` placed one by one (GPI text with increments,
    /// extra spacing, a direction, alignment, an angle or a shear): each
    /// glyph's origin on its baseline at PM `(x, y)`, its shape mapped by
    /// `matrix` (glyph-local `(u, v)`, y up → PM offsets; see
    /// [`glyph_transform::Matrix`]). With [`glyph_transform::IDENTITY`] the
    /// glyphs are the upright ones [`DrawOp::TextFont`] draws. At most
    /// [`MAX_RUN_GLYPHS`] glyphs are drawn.
    GlyphRun { glyphs: Vec<(char, i32, i32)>, color: u32, font: FontSpec, mix: Mix,
               matrix: glyph_transform::Matrix },
    /// One pel.
    Pel { x: i32, y: i32, color: u32, mix: Mix },
    /// A path through `points` (closed back to the first when `closed`),
    /// its inside painted with `fill` and its outline with `stroke`. Each
    /// pel is painted once: outline pels by the stroke, the inside (pel
    /// centres inside the polygon, even-odd, plus the outline's pels when
    /// there is no stroke) by the fill. Used for arcs, patterned areas and
    /// wide lines; a closed rectangle covers the same pels as [`DrawOp::Box`].
    Shape { points: Vec<(i32, i32)>, closed: bool, fill: Option<AreaFill>, stroke: Option<Stroke> },
    /// Several figures drawn as one area (GPI areas, paths, polygons):
    /// each a list of points, outlined back to its start when its flag is
    /// set. The inside of all of them together (each closed for the
    /// purpose) by `rule` (`None`: no inside), plus the pels a
    /// `boundary`-wide square brush covers along the outlines (0: none, and
    /// the inside leaves out the bottom and right edges), is painted with
    /// `fill`; the outlines with `stroke`. Each pel is painted once, the
    /// stroke winning. At most [`MAX_SHAPE_POINTS`] points in all.
    Figures {
        figures: Vec<(Vec<(i32, i32)>, bool)>,
        rule: Option<FillRule>,
        boundary: u32,
        fill: Option<AreaFill>,
        stroke: Option<Stroke>,
    },
    /// Raster-operation blit onto the `w` × `h` area with lower-left corner
    /// `(x, y)`: each pel becomes [`rop3`]`(rop, P, S, D)` with P the
    /// `pattern`'s colour at that pel. `src`
    /// holds the source pels bottom row first (`src[row * w + col]`); a
    /// `None` pel (outside the source) leaves the destination unchanged.
    /// Without `src`, S is 0 (for operations that ignore the source).
    Blit { x: i32, y: i32, w: u32, h: u32, src: Option<Vec<Option<u32>>>, rop: u8, pattern: Pattern },
    /// Fill the whole surface with [`SURFACE_BLANK`].
    Clear,
}

/// The pixels of one frame window's client area (0x00RRGGBB, top row first).
pub struct Surface {
    pub buffer: Vec<u32>,
    pub width:  u32,
    pub height: u32,
    /// Bumped by every change to the pixels or size (`draw`, `put_row`,
    /// `resize`), so a compositor that caches the surface (one texture per
    /// window) can tell a moved window from a redrawn one. Writes to
    /// `buffer` from outside these methods do not bump it.
    generation: u64,
    /// Screen-space rectangle (top-first rows) covering every pel changed
    /// since the last [`Surface::take_changes`]; may be larger than what
    /// actually changed (text marks whole rows), never smaller.
    changed: Rect,
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
            buffer: vec![SURFACE_BLANK; n], width, height, generation: 0, changed: Rect::EMPTY,
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
        self.generation += 1;
        self.changed = self.bounds();
    }

    /// The area changed since the last call (screen coordinates, inside the
    /// surface), and start a new one. A compositor that keeps a copy of the
    /// surface (the layered compositor's frame textures) refreshes only this
    /// part; there is one such consumer at a time.
    pub fn take_changes(&mut self) -> Rect {
        std::mem::replace(&mut self.changed, Rect::EMPTY)
    }

    fn note_change(&mut self, r: Rect) {
        self.changed = self.changed.union(&r.intersect(&self.bounds()));
    }

    /// Screen rows a string with its baseline on PM row `y` may touch, as a
    /// full-width band: one line height below the baseline and two above it
    /// (accents and fallback glyphs can be taller than the face's ascent).
    fn text_rows(&self, y: i32, spec: FontSpec) -> Rect {
        let y = y.clamp(-TEXT_COORD_LIMIT, TEXT_COORD_LIMIT);
        let baseline = self.height as i32 - y;
        let lh = font::metrics(spec).line_height.max(1);
        Rect::new(0, baseline - 2 * lh, self.width as i32, baseline + lh)
    }

    /// A rectangle (screen coordinates, not yet clipped) that contains every
    /// pel `op` can change.
    fn op_bounds(&self, op: &DrawOp) -> Rect {
        match *op {
            DrawOp::Box { x1, y1, x2, y2, .. } => {
                let (x1, y1, x2, y2) = (self.clamp(x1), self.clamp(y1), self.clamp(x2), self.clamp(y2));
                Rect::new(x1.min(x2), self.row(y1.max(y2)), x1.max(x2) + 1, self.row(y1.min(y2)) + 1)
            }
            DrawOp::Line { x1, y1, x2, y2, .. } => match self.clip_line(x1, y1, x2, y2) {
                Some((x1, y1, x2, y2)) =>
                    Rect::new(x1.min(x2), self.row(y1.max(y2)), x1.max(x2) + 1, self.row(y1.min(y2)) + 1),
                None => Rect::EMPTY,
            },
            DrawOp::Text { y, cjk, .. } => {
                let spec = FontSpec { cjk, ..FontSpec::SYSTEM };
                self.text_rows(y.saturating_add(font::metrics(spec).descent), spec)
            }
            DrawOp::TextFont { y, font, .. } => self.text_rows(y, font),
            DrawOp::GlyphRun { ref glyphs, font, matrix, .. } => self.glyph_run_area(glyphs, font, matrix),
            DrawOp::Pel { x, y, .. } => match self.index(x, y) {
                Some(_) => Rect::at(x, self.row(y), 1, 1),
                None => Rect::EMPTY,
            },
            DrawOp::Blit { x, y, w, h, .. } => {
                // i64: the guest supplies all four; clamp to just outside.
                let lim = |v: i64| v.clamp(-1, MAX_SURFACE_DIM as i64 + 1) as i32;
                let (x, y, w, h) = (x as i64, y as i64, w as i64, h as i64);
                let height = self.height as i64;
                Rect::new(lim(x), lim(height - (y + h)), lim(x + w), lim(height - y))
            }
            DrawOp::Shape { ref points, stroke, fill, closed } =>
                self.shape_area(&[(points, closed)], stroke, Self::shape_boundary(fill, stroke)),
            DrawOp::Figures { ref figures, boundary, stroke, .. } =>
                self.shape_area(&Self::figure_slices(figures), stroke, boundary),
            DrawOp::Clear => self.bounds(),
        }
    }

    /// The change counter (see the field); differs whenever the pixels or
    /// the size may have changed since it was last read.
    pub fn generation(&self) -> u64 {
        self.generation
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
        self.clip_line_margin(x1, y1, x2, y2, 0)
    }

    /// Overwrite PM row `y` from column 0 with `pels` (clipped).
    pub fn put_row(&mut self, y: i32, pels: &[u32]) {
        if let Some(i) = self.index(0, y) {
            self.generation += 1;
            self.note_change(Rect::at(0, self.row(y), self.width as i32, 1));
            let n = pels.len().min(self.width as usize);
            for (d, &p) in self.buffer[i..i + n].iter_mut().zip(pels) {
                *d = p & RGB_MASK;
            }
        }
    }

    /// Overwrite PM row `y` from column `x` with `pels`, clipped to the
    /// surface (the VIO window writes its text screen this way).
    pub fn put_span(&mut self, x: i32, y: i32, pels: &[u32]) {
        if y < 0 || y >= self.height as i32 || x >= self.width as i32 { return; }
        let skip = (-x).max(0) as usize;
        if skip >= pels.len() { return; }
        let x0 = x.max(0);
        let n = (pels.len() - skip).min((self.width as i32 - x0) as usize);
        let Some(i) = self.index(x0, y) else { return };
        self.generation += 1;
        self.note_change(Rect::at(x0, self.row(y), n as i32, 1));
        for (d, &p) in self.buffer[i..i + n].iter_mut().zip(&pels[skip..]) {
            *d = p & RGB_MASK;
        }
    }

    /// `WinScrollWindow`: fill the PM rectangle `x0..x1` × `y0..y1`
    /// (exclusive at the right and top, clipped to the surface) with the
    /// pels `dx` to the left and `dy` below each of them, as they were
    /// before the call; a pel whose source is off the surface stays.
    pub fn scroll(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, dx: i32, dy: i32) {
        let (w, h) = (self.width as i32, self.height as i32);
        let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(w), y1.min(h));
        if x0 >= x1 || y0 >= y1 { return; }
        // The sources, read before anything is written (the areas overlap).
        let src: Vec<Vec<Option<u32>>> = (y0..y1).map(|y| {
            (x0..x1).map(|x| self.pel(x.saturating_sub(dx), y.saturating_sub(dy))).collect()
        }).collect();
        for (y, row) in (y0..y1).zip(src) {
            for (x, p) in (x0..x1).zip(row) {
                if let (Some(p), Some(i)) = (p, self.index(x, y)) {
                    self.buffer[i] = p;
                }
            }
        }
        self.generation += 1;
        self.note_change(Rect::new(x0, self.row(y1 - 1), x1, self.row(y0) + 1));
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
        let touched = self.op_bounds(&op);
        self.apply(&op);
        self.note_change(touched);
        self.generation += 1;
        #[cfg(test)]
        if let Some(rec) = &mut self.record {
            rec.push(op);
        }
    }

    /// Apply `op`, keeping only the pels inside `clip` (PM coordinates of
    /// the surface; `None` = unclipped, as fast as [`Surface::draw`]).
    ///
    /// Every operation is clipped the same way: the area it can touch
    /// (`op_bounds`) is saved, the operation applied, and the pels outside
    /// the clip region put back. So mixes, patterns, text and blits clip
    /// pel-exactly without each primitive knowing about regions. When the
    /// clip holds the whole area the operation is applied as is; when it
    /// misses it, nothing is drawn.
    pub fn draw_clipped(&mut self, op: DrawOp, clip: Option<&Region>) {
        let Some(clip) = clip else { return self.draw(op) };
        let touched = self.op_bounds(&op).intersect(&self.bounds());
        if touched.is_empty() { return self.draw(op); }
        // The touched area in PM coordinates (exclusive at the right / top).
        let pm = region::RRect::new(touched.x0, self.height as i32 - touched.y1,
                                    touched.x1, self.height as i32 - touched.y0);
        match clip.overlap(pm) {
            region::Overlap::Inside => return self.draw(op),
            region::Overlap::Outside => return,
            region::Overlap::Partial => {}
        }
        let (w, tw) = (self.width as usize, touched.width() as usize);
        let saved: Vec<u32> = (touched.y0..touched.y1)
            .flat_map(|sy| self.buffer[sy as usize * w + touched.x0 as usize..][..tw].iter().copied())
            .collect();
        self.draw(op);
        for (r, sy) in (touched.y0..touched.y1).enumerate() {
            let y = self.height as i32 - 1 - sy;
            let row = &mut self.buffer[sy as usize * w..][touched.x0 as usize..touched.x1 as usize];
            let old = &saved[r * tw..][..tw];
            // Put back every pel of the row outside the clip's spans.
            let mut x = touched.x0;
            for &(s, e) in clip.spans_at(y).iter().chain(std::iter::once(&(touched.x1, touched.x1))) {
                let s = s.clamp(touched.x0, touched.x1);
                if s > x {
                    let (a, b) = ((x - touched.x0) as usize, (s - touched.x0) as usize);
                    row[a..b].copy_from_slice(&old[a..b]);
                }
                x = x.max(e.clamp(touched.x0, touched.x1));
            }
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
            DrawOp::Text { x, y, ref text, color, cjk } => {
                let spec = FontSpec { cjk, ..FontSpec::SYSTEM };
                let descent = font::metrics(spec).descent;
                self.text(x, y.saturating_add(descent), text, color, spec, Mix::Overpaint);
            }
            DrawOp::TextFont { x, y, ref text, color, font, mix } => {
                self.text(x, y, text, color, font, mix);
            }
            DrawOp::GlyphRun { ref glyphs, color, font, mix, matrix } => {
                self.glyph_run(glyphs, color, font, mix, matrix);
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
                        let (px, py) = ((x as i64 + c) as i32, (y as i64 + r) as i32);
                        if let Some(i) = self.index(px, py) {
                            self.buffer[i] = rop3(rop, pattern.color_at(px, py), s, self.buffer[i]);
                        }
                    }
                }
            }
            DrawOp::Shape { ref points, closed, fill, stroke } => {
                let rule = closed.then_some(FillRule::Alternate);
                self.figures(&[(points, closed)], rule, region::Edges::Centres,
                             Self::shape_boundary(fill, stroke), fill, stroke);
            }
            DrawOp::Figures { ref figures, rule, boundary, fill, stroke } => {
                let edges = if boundary == 0 { region::Edges::TopLeft } else { region::Edges::Centres };
                self.figures(&Self::figure_slices(figures), rule, edges, boundary, fill, stroke);
            }
            DrawOp::Clear => self.buffer.fill(SURFACE_BLANK),
        }
    }

    /// The figures of a shape as used: at most [`MAX_SHAPE_POINTS`] points
    /// in all, each clamped to ±[`SHAPE_COORD_LIMIT`].
    fn shape_figures(figures: &[(&[(i32, i32)], bool)]) -> Vec<(Vec<(i32, i32)>, bool)> {
        let lim = |v: i32| v.clamp(-SHAPE_COORD_LIMIT, SHAPE_COORD_LIMIT);
        let mut left = MAX_SHAPE_POINTS;
        let mut out = Vec::new();
        for (pts, closed) in figures {
            if left == 0 { break; }
            let take: Vec<(i32, i32)> = pts.iter().take(left).map(|&(x, y)| (lim(x), lim(y))).collect();
            left -= take.len();
            out.push((take, *closed));
        }
        out
    }

    /// Screen rectangle (inside the surface) a shape can touch: its points'
    /// bounding box grown by the reach of the stroke or the boundary brush.
    fn shape_area(&self, figures: &[(&[(i32, i32)], bool)], stroke: Option<Stroke>, boundary: u32) -> Rect {
        let reach = |w: u32| w.clamp(1, MAX_BOUNDARY_WIDTH) as i32 / 2 + 1;
        let reach = stroke.map_or(0, |s| reach(s.width.min(MAX_STROKE_WIDTH)))
            .max(if boundary > 0 { reach(boundary) } else { 0 });
        let mut it = figures.iter().flat_map(|(p, _)| p.iter().copied()).take(MAX_SHAPE_POINTS);
        let lim = |v: i32| v.clamp(-SHAPE_COORD_LIMIT, SHAPE_COORD_LIMIT);
        let Some((x, y)) = it.next() else { return Rect::EMPTY };
        let (mut x0, mut x1, mut y0, mut y1) = (lim(x), lim(x), lim(y), lim(y));
        for (x, y) in it {
            let (x, y) = (lim(x), lim(y));
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        Rect::new(x0 - reach, self.row(y1) - reach, x1 + 1 + reach, self.row(y0) + 1 + reach)
            .intersect(&self.bounds())
    }

    /// Rasterise a [`DrawOp::Figures`] (and a [`DrawOp::Shape`], one
    /// figure): mark its pels in a mask over the area it can touch
    /// (1 = inside, 2 = outline), then paint each marked pel once.
    #[allow(clippy::too_many_arguments)]
    fn figures(&mut self, figures: &[(&[(i32, i32)], bool)], rule: Option<FillRule>, edges: region::Edges,
               boundary: u32, fill: Option<AreaFill>, stroke: Option<Stroke>) {
        const INSIDE: u8 = 1;
        const OUTLINE: u8 = 2;
        let boundary = boundary.min(MAX_BOUNDARY_WIDTH);
        let area = self.shape_area(figures, stroke, boundary);
        if area.is_empty() || (fill.is_none() && stroke.is_none()) { return; }
        let figs = Self::shape_figures(figures);
        let (aw, ah) = (area.width() as usize, area.height() as usize);
        let mut mask = vec![0u8; aw * ah];
        let height = self.height as i32;
        // Mark PM (x, y) if it lies in the area.
        let mut mark = |x: i32, y: i32, v: u8| {
            let sy = height - 1 - y;
            if x >= area.x0 && x < area.x1 && sy >= area.y0 && sy < area.y1 {
                let m = &mut mask[(sy - area.y0) as usize * aw + (x - area.x0) as usize];
                *m = (*m).max(v);
            }
        };

        // Inside: pel centres between edge crossings on each row, by the
        // rule (every figure closed for the purpose).
        if fill.is_some() && let Some(rule) = rule {
            let segs = region::fill_segments(figs.iter().map(|(p, _)| p.as_slice()));
            let mut spans = Vec::new();
            for sy in area.y0..area.y1 {
                region::scan_row(&segs, rule, edges, height - 1 - sy, &mut spans);
                for &(a, b) in &spans {
                    for x in a.max(area.x0)..=b.min(area.x1 - 1) { mark(x, height - 1 - sy, INSIDE); }
                }
            }
        }

        // Outlines: Bresenham along each segment, stamping the brush; the
        // style runs on across segments. The boundary brush marks inside
        // pels (a filled shape includes its boundary).
        let mut passes: Vec<(u8, i32, u16)> = Vec::new();
        if let Some(s) = stroke { passes.push((OUTLINE, s.width.clamp(1, MAX_STROKE_WIDTH) as i32, s.style)); }
        if fill.is_some() && boundary > 0 { passes.push((INSIDE, boundary as i32, SOLID_LINE)); }
        for (value, width, style) in passes {
            let (lo, hi) = (-(width - 1) / 2, width / 2);
            let margin = width;
            let bounds = (-1 - margin, -1 - margin, self.width as i32 + margin, self.height as i32 + margin);
            let mut n = 0u32;
            for (pts, closed) in &figs {
                for seg in region::outline_segments(pts, *closed) {
                    region::walk_segment(seg, bounds, |x, y| {
                        if style & (0x8000 >> (n % 16)) != 0 {
                            for oy in lo..=hi { for ox in lo..=hi { mark(x + ox, y + oy, value); } }
                        }
                        n += 1;
                    });
                    // The next segment starts on this end point; do not count it twice.
                    n = n.wrapping_sub(1);
                }
            }
        }

        let w = self.width as usize;
        for (i, &m) in mask.iter().enumerate() {
            if m == 0 { continue; }
            let (x, sy) = (area.x0 + (i % aw) as i32, area.y0 + (i / aw) as i32);
            let d = &mut self.buffer[sy as usize * w + x as usize];
            match (m, stroke, fill) {
                (OUTLINE, Some(s), _) => *d = s.mix.apply(s.color, *d),
                (_, _, Some(f)) => {
                    let y = height - 1 - sy;
                    if f.pattern.bit(x, y) {
                        *d = f.mix.apply(f.pattern.fg, *d);
                    } else if let Some(bm) = f.back_mix {
                        *d = bm.apply(f.pattern.bg, *d);
                    }
                }
                _ => {}
            }
        }
    }

    /// [`Surface::clip_line`] against the surface grown by `margin` pels on
    /// every side, so a wide brush centred just outside still reaches in.
    fn clip_line_margin(&self, x1: i32, y1: i32, x2: i32, y2: i32, margin: i32)
                        -> Option<(i32, i32, i32, i32)> {
        let bounds = (-1 - margin, -1 - margin, self.width as i32 + margin, self.height as i32 + margin);
        region::clip_segment(((x1, y1), (x2, y2)), bounds).map(|((a, b), (c, d))| (a, b, c, d))
    }

    /// The boundary brush of a [`DrawOp::Shape`]: without a stroke, a
    /// filled shape includes its one-pel outline.
    fn shape_boundary(fill: Option<AreaFill>, stroke: Option<Stroke>) -> u32 {
        u32::from(fill.is_some() && stroke.is_none())
    }

    fn figure_slices(figures: &[(Vec<(i32, i32)>, bool)]) -> Vec<(&[(i32, i32)], bool)> {
        figures.iter().map(|(p, c)| (p.as_slice(), *c)).collect()
    }

    /// Draw `text` with its baseline on PM row `y`.
    /// Screen rectangle (inside the surface) a [`DrawOp::GlyphRun`] can
    /// touch: the box of the glyph origins grown by two line heights (one
    /// glyph's reach in any direction) scaled by the matrix.
    fn glyph_run_area(&self, glyphs: &[(char, i32, i32)], spec: FontSpec, m: glyph_transform::Matrix) -> Rect {
        if !glyph_transform::drawable(m) { return Rect::EMPTY; }
        let lim = |v: i32| v.clamp(-TEXT_COORD_LIMIT, TEXT_COORD_LIMIT);
        let mut it = glyphs.iter().take(MAX_RUN_GLYPHS).map(|&(_, x, y)| (lim(x), lim(y)));
        let Some((x, y)) = it.next() else { return Rect::EMPTY };
        let (mut x0, mut x1, mut y0, mut y1) = (x, x, y, y);
        for (x, y) in it {
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        let norm = (m[0].abs() + m[1].abs()).max(m[2].abs() + m[3].abs());
        let reach = (2.0 * font::metrics(spec).line_height.max(1) as f32 * norm).ceil() as i32 + 2;
        Rect::new(x0 - reach, self.row(y1) - reach, x1 + 1 + reach, self.row(y0) + 1 + reach)
            .intersect(&self.bounds())
    }

    fn glyph_run(&mut self, glyphs: &[(char, i32, i32)], color: u32, spec: FontSpec, mix: Mix,
                 m: glyph_transform::Matrix) {
        if self.width == 0 || self.height == 0 { return; }
        let (w, h) = (self.width as usize, self.height as usize);
        let paint = |p: &mut u32, cov: u8| {
            if mix == Mix::Overpaint {
                *p = font::blend(*p, color, cov);
            } else if cov >= MIX_TEXT_COVERAGE {
                *p = mix.apply(color, *p);
            }
        };
        let mut one = [0u8; 4];
        for &(ch, x, y) in glyphs.iter().take(MAX_RUN_GLYPHS) {
            let (x, y) = (x.clamp(-TEXT_COORD_LIMIT, TEXT_COORD_LIMIT), y.clamp(-TEXT_COORD_LIMIT, TEXT_COORD_LIMIT));
            let baseline = self.height as i32 - y;
            if m == glyph_transform::IDENTITY {
                let text = ch.encode_utf8(&mut one);
                font::draw_text_with(&mut self.buffer, w, h, x, baseline, text, spec, paint);
            } else {
                glyph_transform::draw_glyph(&mut self.buffer, w, h, x, baseline, ch, spec, m, paint);
            }
        }
    }

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

    #[test]
    fn put_span_writes_a_clipped_run_of_pels() {
        const W: u32 = 0x00FF_FFFF; // a blank pel, as row_pels reports it
        let mut s = Surface::new(8, 4);
        s.put_span(2, 1, &[0xFF_11_22_33, 0x00_44_55_66]);
        assert_eq!(s.row_pels(1).unwrap()[1..5], [W, 0x11_22_33, 0x44_55_66, W]);
        // Clipped at both ends, and outside the surface nothing happens.
        s.put_span(-1, 0, &[1, 2, 3]);
        assert_eq!(s.row_pels(0).unwrap()[..3], [2, 3, W]);
        s.put_span(6, 2, &[7, 8, 9]);
        assert_eq!(s.row_pels(2).unwrap()[6..], [7, 8]);
        let generation = s.generation;
        s.put_span(0, 4, &[1]);
        s.put_span(8, 0, &[1]);
        s.put_span(-3, 0, &[1, 2]);
        assert_eq!(s.generation, generation, "nothing written");
    }

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
            DrawOp::Text { x: hi, y: hi, text: "x".into(), color: 0, cjk: CjkRegion::Auto },
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
        s.draw(DrawOp::Blit { x: 2, y: 0, w: 3, h: 2, src: Some(src), rop: 0xCC, pattern: Pattern::solid(0) });
        assert_eq!(s.pel(2, 0), Some(1));
        assert_eq!(s.pel(3, 0), Some(2));
        assert_eq!(s.pel(3, 1), Some(5));
        assert_eq!(s.pel(2, 1), Some(4));
        assert_eq!(s.pel(1, 0), Some(WHITE), "left of the area");
        // No source: pattern copy, and a mismatched source is ignored.
        s.draw(DrawOp::Blit { x: -1, y: 3, w: 9, h: 9, src: None, rop: 0xF0, pattern: Pattern::solid(RED) });
        assert_eq!(s.row_pels(3), Some(vec![RED; 4]));
        s.draw(DrawOp::Blit { x: 0, y: 0, w: 2, h: 2, src: Some(vec![Some(7)]), rop: 0xCC, pattern: Pattern::solid(0) });
        assert_eq!(s.pel(0, 0), Some(WHITE));
        // Guest extremes do not overflow.
        s.draw(DrawOp::Blit { x: i32::MIN, y: i32::MAX, w: u32::MAX, h: 1, src: None, rop: 0xF0, pattern: Pattern::solid(0) });
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
        s.draw(DrawOp::Text { x: 0, y, text: "Tg".into(), color: 0, cjk: CjkRegion::Auto });
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

    #[test]
    fn generation_changes_with_pixels_and_size_only() {
        let mut s = Surface::new(4, 4);
        let g0 = s.generation();
        s.draw(DrawOp::Pel { x: 1, y: 1, color: RED, mix: Mix::Overpaint });
        let g1 = s.generation();
        assert_ne!(g1, g0, "drawing changes it");
        let _ = (s.pel(1, 1), s.row_pels(1));
        assert_eq!(s.generation(), g1, "reading does not");
        s.put_row(0, &[RED]);
        let g2 = s.generation();
        assert_ne!(g2, g1, "put_row changes it");
        s.put_row(99, &[RED]);
        assert_eq!(s.generation(), g2, "a row outside the surface does not");
        s.resize(4, 4);
        assert_eq!(s.generation(), g2, "resizing to the same size does not");
        s.resize(5, 4);
        assert_ne!(s.generation(), g2, "a real resize does");
    }

    /// Every pel an operation changes lies inside what `take_changes`
    /// reports, and the report starts afresh after each call.
    #[test]
    fn changes_cover_every_changed_pel() {
        let ops = vec![
            DrawOp::Box { x1: 3, y1: 4, x2: 20, y2: 9, color: RED, fill: true, mix: Mix::Overpaint },
            DrawOp::Box { x1: 30, y1: 20, x2: 2, y2: 28, color: RED, fill: false, mix: Mix::Xor },
            DrawOp::Line { x1: -50, y1: 3, x2: 70, y2: 37, color: RED, mix: Mix::Overpaint, style: SOLID_LINE },
            DrawOp::Line { x1: 5, y1: 30, x2: 5, y2: 2, color: RED, mix: Mix::Invert, style: SOLID_LINE },
            DrawOp::Text { x: 2, y: 10, text: "Hég".into(), color: RED, cjk: CjkRegion::Auto },
            DrawOp::TextFont { x: 30, y: 25, text: "Åy".into(), color: RED, font: FontSpec::SYSTEM, mix: Mix::Overpaint },
            DrawOp::Pel { x: 63, y: 0, color: RED, mix: Mix::Overpaint },
            DrawOp::Pel { x: 64, y: 0, color: RED, mix: Mix::Overpaint },
            DrawOp::Blit { x: 50, y: 30, w: 30, h: 30, src: None, rop: 0x55, pattern: Pattern::solid(0) },
            DrawOp::Blit { x: i32::MIN, y: i32::MAX, w: u32::MAX, h: 3, src: None, rop: 0xFF, pattern: Pattern::solid(0) },
        ];
        for op in ops {
            let mut s = Surface::new(64, 40);
            for (i, p) in s.buffer.iter_mut().enumerate() { *p = (i as u32).wrapping_mul(2654435761) & RGB_MASK; }
            let before = s.buffer.clone();
            s.take_changes();
            s.draw(op.clone());
            let changed = s.take_changes();
            for (i, (a, b)) in before.iter().zip(&s.buffer).enumerate() {
                if a != b {
                    let (x, y) = ((i % 64) as i32, (i / 64) as i32);
                    assert!(changed.contains(&Rect::at(x, y, 1, 1)), "{op:?}: ({x}, {y}) outside {changed:?}");
                }
            }
            assert!(Rect::at(0, 0, 64, 40).contains(&changed), "{op:?}: {changed:?} is clipped");
            assert_eq!(s.take_changes(), Rect::EMPTY, "taken");
        }
        let mut s = Surface::new(8, 8);
        s.put_row(2, &[RED]);
        assert_eq!(s.take_changes(), Rect::at(0, 5, 8, 1), "PM row 2 is screen row 5");
        s.resize(9, 8);
        assert_eq!(s.take_changes(), Rect::at(0, 0, 9, 8), "a resize changes everything");
        s.draw(DrawOp::Pel { x: 1, y: 1, color: RED, mix: Mix::Overpaint });
        s.draw(DrawOp::Pel { x: 3, y: 2, color: RED, mix: Mix::Overpaint });
        assert_eq!(s.take_changes(), Rect::new(1, 5, 4, 7), "changes accumulate");
    }

    #[test]
    fn scroll_moves_pels_and_records_the_change() {
        let mut s = numbered(16, 12);
        let before = s.buffer.clone();
        let at = |p: &[u32], x: i32, y: i32| p[((11 - y) * 16 + x) as usize];
        s.take_changes();
        let g = s.generation();
        // Up 2 and left 3 over the overlapping area x 2..10, y 4..9.
        s.scroll(2, 4, 10, 9, -3, 2);
        for y in 0..12 {
            for x in 0..16 {
                let want = if (2..10).contains(&x) && (4..9).contains(&y) {
                    at(&before, x + 3, y - 2)
                } else {
                    at(&before, x, y)
                };
                assert_eq!(s.pel(x, y), Some(want), "({x}, {y})");
            }
        }
        assert!(s.generation() > g);
        assert_eq!(s.take_changes(), Rect::new(2, 3, 10, 8), "PM rows 4..9 are screen rows 3..8");
        // Sources off the surface leave the destination; the destination is clipped.
        let mut s = numbered(4, 4);
        let before = s.buffer.clone();
        s.scroll(-10, -10, 100, 100, 3, 0);
        assert_eq!(s.pel(3, 0), Some(at4(&before, 0, 0)));
        assert_eq!(s.pel(2, 0), Some(at4(&before, 2, 0)), "source off the surface");
        s.scroll(0, 0, 4, 4, i32::MIN, i32::MAX);
        s.scroll(5, 5, 9, 9, 1, 1);
    }

    fn at4(p: &[u32], x: i32, y: i32) -> u32 {
        p[((3 - y) * 4 + x) as usize]
    }

    fn rect_points(x1: i32, y1: i32, x2: i32, y2: i32) -> Vec<(i32, i32)> {
        vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)]
    }

    fn solid_fill(color: u32, mix: Mix) -> Option<AreaFill> {
        Some(AreaFill { pattern: Pattern::solid(color), mix, back_mix: None })
    }

    fn stroke(color: u32, width: u32) -> Option<Stroke> {
        Some(Stroke { color, mix: Mix::Overpaint, style: SOLID_LINE, width })
    }

    #[test]
    fn closed_rectangles_cover_the_same_pels_as_boxes() {
        for (fill, outline) in [(true, false), (false, true)] {
            let mut a = Surface::new(40, 30);
            a.draw(DrawOp::Box { x1: 3, y1: 4, x2: 20, y2: 15, color: RED, fill, mix: Mix::Overpaint });
            let mut b = Surface::new(40, 30);
            b.draw(DrawOp::Shape {
                points: rect_points(3, 4, 20, 15), closed: true,
                fill: if fill { solid_fill(RED, Mix::Overpaint) } else { None },
                stroke: if outline { stroke(RED, 1) } else { None },
            });
            assert_eq!(a.buffer, b.buffer, "fill {fill}, outline {outline}");
        }
    }

    #[test]
    fn shapes_paint_each_pel_once() {
        let mut s = Surface::new(40, 30);
        for (i, p) in s.buffer.iter_mut().enumerate() { *p = (i as u32).wrapping_mul(2654435761) & RGB_MASK; }
        let before = s.buffer.clone();
        let op = DrawOp::Shape {
            points: vec![(5, 5), (30, 8), (18, 25)], closed: true,
            fill: solid_fill(0x00_FF00FF, Mix::Xor),
            stroke: Some(Stroke { color: 0x00_00FFFF, mix: Mix::Xor, style: SOLID_LINE, width: 3 }),
        };
        s.draw(op.clone());
        assert_ne!(s.buffer, before);
        s.draw(op);
        assert_eq!(s.buffer, before, "XOR twice restores every pel");
    }

    #[test]
    fn pattern_fills_leave_or_paint_the_clear_pels() {
        let checker = Pattern { rows: [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55],
                                fg: RED, bg: 0x00_0000FF, origin: (0, 0) };
        for back_mix in [None, Some(Mix::Overpaint)] {
            let mut s = Surface::new(16, 16);
            s.draw(DrawOp::Shape { points: rect_points(0, 0, 15, 15), closed: true,
                                   fill: Some(AreaFill { pattern: checker, mix: Mix::Overpaint, back_mix }),
                                   stroke: None });
            assert_eq!(s.pel(0, 0), Some(RED), "row 0 starts with a set bit");
            assert_eq!(s.pel(1, 1), Some(RED), "row 1 = 0x55");
            let clear = if back_mix.is_some() { 0x00_0000FF } else { SURFACE_BLANK & RGB_MASK };
            assert_eq!(s.pel(1, 0), Some(clear), "clear bit with back mix {back_mix:?}");
            assert_eq!(s.pel(9, 8), Some(clear), "the pattern repeats every 8 pels");
        }
        // The origin shifts the pattern.
        let shifted = Pattern { origin: (1, 0), ..checker };
        assert!(shifted.bit(1, 0) && !shifted.bit(0, 0) && shifted.bit(-1, 0));
        assert!(Pattern::solid(RED).is_solid() && !checker.is_solid());
    }

    #[test]
    fn wide_strokes_are_centred_on_the_path() {
        let mut s = Surface::new(30, 20);
        s.draw(DrawOp::Shape { points: vec![(5, 10), (20, 10)], closed: false, fill: None, stroke: stroke(RED, 3) });
        for y in 9..=11 { assert_eq!(s.pel(12, y), Some(RED), "row {y} inside the 3-pel width"); }
        assert_ne!(s.pel(12, 8), Some(RED));
        assert_ne!(s.pel(12, 12), Some(RED));
        assert_eq!(s.pel(4, 10), Some(RED), "the brush reaches past the end point");
        // Width 2 covers the path and one pel on one side.
        let mut s = Surface::new(30, 20);
        s.draw(DrawOp::Shape { points: vec![(5, 10), (20, 10)], closed: false, fill: None, stroke: stroke(RED, 2) });
        let rows: Vec<i32> = (5..15).filter(|&y| s.pel(12, y) == Some(RED)).collect();
        assert_eq!(rows.len(), 2);
        assert!(rows.contains(&10));
    }

    #[test]
    fn line_styles_run_on_across_shape_segments() {
        let mut s = Surface::new(40, 10);
        // 0xF0F0: 4 on, 4 off. A corner at x = 6 sits inside an "on" run.
        let dashed = Some(Stroke { color: RED, mix: Mix::Overpaint, style: 0xF0F0, width: 1 });
        s.draw(DrawOp::Shape { points: vec![(0, 5), (6, 5), (13, 5)], closed: false, fill: None, stroke: dashed });
        let on: Vec<i32> = (0..14).filter(|&x| s.pel(x, 5) == Some(RED)).collect();
        assert_eq!(on, vec![0, 1, 2, 3, 8, 9, 10, 11], "one continuous dash pattern");
    }

    #[test]
    fn shapes_with_extreme_coordinates_are_safe() {
        let mut s = Surface::new(20, 20);
        s.draw(DrawOp::Shape { points: vec![(i32::MIN, i32::MIN), (i32::MAX, 0), (0, i32::MAX)], closed: true,
                               fill: solid_fill(RED, Mix::Overpaint),
                               stroke: Some(Stroke { color: RED, mix: Mix::Xor, style: SOLID_LINE, width: u32::MAX }) });
        s.draw(DrawOp::Shape { points: Vec::new(), closed: true, fill: solid_fill(RED, Mix::Overpaint), stroke: None });
        s.draw(DrawOp::Shape { points: vec![(5, 5)], closed: false, fill: None, stroke: stroke(RED, 1) });
        assert_eq!(s.pel(5, 5), Some(RED), "a one-point path draws its point");
        let many: Vec<(i32, i32)> = (0..MAX_SHAPE_POINTS as i32 * 2).map(|i| (i % 20, i % 7)).collect();
        s.draw(DrawOp::Shape { points: many, closed: true, fill: solid_fill(RED, Mix::Xor), stroke: None });
    }

    #[test]
    fn shape_changes_are_tracked() {
        let mut s = Surface::new(64, 40);
        s.take_changes();
        let before = s.buffer.clone();
        s.draw(DrawOp::Shape { points: vec![(-10, 3), (70, 30), (20, 45)], closed: true,
                               fill: solid_fill(RED, Mix::Overpaint), stroke: stroke(RED, 5) });
        let changed = s.take_changes();
        for (i, (a, b)) in before.iter().zip(&s.buffer).enumerate() {
            if a != b {
                assert!(changed.contains(&Rect::at((i % 64) as i32, (i / 64) as i32, 1, 1)));
            }
        }
    }

    #[test]
    fn clipped_drawing_keeps_only_the_region() {
        use crate::gui::region::{RRect, Region};
        let clip = Region::from_rects(&[RRect::new(2, 2, 6, 6), RRect::new(10, 0, 12, 20)]).unwrap();
        let mut s = Surface::new(20, 20);
        s.draw_clipped(fill(0, 0, 19, 19, RED, Mix::Overpaint), Some(&clip));
        assert_eq!(count(&s, RED), 16 + 40);
        assert_eq!(s.pel(2, 2), Some(RED));
        assert_eq!(s.pel(6, 2), Some(WHITE), "right edge of the clip is out");
        assert_eq!(s.pel(11, 19), Some(RED));
        // Mixes: pels outside are left exactly as they were.
        s.draw_clipped(fill(0, 0, 19, 19, RED, Mix::Xor), Some(&clip));
        assert_eq!(count(&s, RED), 0);
        assert_eq!(count(&s, 0), 56, "RED XOR RED");
        // Text and lines clip too; no clip at all draws everything.
        let mut t = Surface::new(20, 20);
        t.draw_clipped(DrawOp::Line { x1: 0, y1: 4, x2: 19, y2: 4, color: RED, mix: Mix::Overpaint, style: SOLID_LINE },
                       Some(&clip));
        assert_eq!(count(&t, RED), 4 + 2);
        t.draw_clipped(fill(0, 0, 19, 19, RED, Mix::Overpaint), None);
        assert_eq!(count(&t, RED), 400);
        // A clip that misses the operation draws nothing.
        let far = Region::rect(RRect::new(100, 100, 120, 120));
        let mut u = Surface::new(20, 20);
        u.draw_clipped(fill(0, 0, 19, 19, RED, Mix::Overpaint), Some(&far));
        assert_eq!(count(&u, RED), 0);
    }

    #[test]
    fn figures_fill_by_rule_with_or_without_edges() {
        let square = |x0: i32, y0: i32, x1: i32, y1: i32| (vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)], true);
        let solid = Some(AreaFill { pattern: Pattern::solid(RED), mix: Mix::Overpaint, back_mix: None });
        let figs = vec![square(0, 0, 9, 9), square(5, 5, 14, 14)];
        let mut alt = Surface::new(20, 20);
        alt.draw(DrawOp::Figures { figures: figs.clone(), rule: Some(FillRule::Alternate), boundary: 1,
                                   fill: solid, stroke: None });
        assert_eq!(alt.pel(7, 7), Some(WHITE), "overlap outside");
        assert_eq!(alt.pel(2, 2), Some(RED));
        let mut wind = Surface::new(20, 20);
        wind.draw(DrawOp::Figures { figures: figs, rule: Some(FillRule::Winding), boundary: 1,
                                    fill: solid, stroke: None });
        assert_eq!(wind.pel(7, 7), Some(RED), "overlap inside");
        // Exclusive: no bottom and right edges.
        let mut ex = Surface::new(20, 20);
        ex.draw(DrawOp::Figures { figures: vec![square(0, 0, 9, 9)], rule: Some(FillRule::Alternate),
                                  boundary: 0, fill: solid, stroke: None });
        assert_eq!(count(&ex, RED), 81);
        assert_eq!(ex.pel(0, 9), Some(RED));
        assert_eq!(ex.pel(9, 5), Some(WHITE));
        assert_eq!(ex.pel(5, 0), Some(WHITE));
        // A wide boundary without an inside (a stroked path).
        let mut st = Surface::new(20, 20);
        st.draw(DrawOp::Figures { figures: vec![(vec![(2, 10), (17, 10)], false)], rule: None,
                                  boundary: 3, fill: solid, stroke: None });
        assert_eq!(count(&st, RED), 18 * 3, "square brush: one pel past each end");
        // The boundary brush is bounded.
        let mut big = Surface::new(20, 20);
        big.draw(DrawOp::Figures { figures: vec![(vec![(10, 10)], false)], rule: None,
                                   boundary: u32::MAX, fill: solid, stroke: None });
        assert_eq!(count(&big, RED), 400, "64 pels wide covers all");
    }
}
