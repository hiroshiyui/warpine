// SPDX-License-Identifier: GPL-3.0-or-later

//! Outline faces read with `skrifa`, measured and rasterised the way
//! `ab_glyph` did (ADR-0119).
//!
//! Warpine drew its unhinted glyphs (CJK faces, transformed text) and took
//! its metrics through `ab_glyph`, whose parser `ttf-parser` is no longer
//! maintained (RUSTSEC-2026-0192). Hinted Latin text already came from
//! `skrifa`; this module gives the rest the same subset of `ab_glyph`'s
//! calls, on `skrifa`, with `ab_glyph`'s arithmetic, so that every glyph
//! keeps its pels:
//!
//! * a pixel size `px` is the face's height (ascent − descent), so the
//!   scale factor is `px / height` both ways ([`Face::scale_factor`]);
//! * an outline is its contours as lines and curves in font units (y up),
//!   each contour closed with a line back to its start, and its bounds are
//!   those of all its points, control points included ([`Face::outline`]);
//! * [`Outlined`] places an outline at a pixel position with the same
//!   floor / ceil bounds and fills it with `ab_glyph_rasterizer`.

use ab_glyph_rasterizer::Rasterizer;
pub use ab_glyph_rasterizer::{point, Point};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::MetadataProvider;

/// A glyph of a face; 0 is "no glyph".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct GlyphId(pub u16);

/// A rectangle by two corners. For an outline's bounds in font units,
/// `min` holds the left and the top (largest y), `max` the right and the
/// bottom, so that scaling with y flipped gives pixel rows growing down.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub min: Point,
    pub max: Point,
}

impl Rect {
    pub fn width(&self) -> f32 { self.max.x - self.min.x }
    pub fn height(&self) -> f32 { self.max.y - self.min.y }
}

/// A piece of a contour, in font units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutlineCurve {
    Line(Point, Point),
    Quad(Point, Point, Point),
    Cubic(Point, Point, Point, Point),
}

/// A glyph's contours and their bounds (see [`Rect`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub bounds: Rect,
    pub curves: Vec<OutlineCurve>,
}

/// Font units to pixels, horizontally and vertically.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PxScaleFactor {
    pub horizontal: f32,
    pub vertical: f32,
}

/// A parsed face and the metrics Warpine uses.
pub struct Face {
    font: skrifa::FontRef<'static>,
    charmap: skrifa::charmap::Charmap<'static>,
    upem: f32,
    ascent: f32,
    descent: f32,
}

impl Face {
    /// The face in `data`, or `None` if it cannot be read.
    pub fn new(data: &'static [u8]) -> Option<Face> {
        let font = skrifa::FontRef::new(data).ok()?;
        let m = font.metrics(Size::unscaled(), LocationRef::default());
        Some(Face { charmap: font.charmap(), upem: f32::from(m.units_per_em), ascent: m.ascent, descent: m.descent, font })
    }

    /// The `skrifa` view of the face.
    pub fn font(&self) -> &skrifa::FontRef<'static> {
        &self.font
    }

    /// The glyph for `ch`; `GlyphId(0)` if the face has none.
    pub fn glyph_id(&self, ch: char) -> GlyphId {
        self.charmap.map(ch).and_then(|g| u16::try_from(g.to_u32()).ok()).map_or(GlyphId(0), GlyphId)
    }

    pub fn units_per_em(&self) -> Option<f32> {
        (self.upem > 0.0).then_some(self.upem)
    }

    /// Ascent − descent, in font units.
    pub fn height_unscaled(&self) -> f32 {
        self.ascent - self.descent
    }

    /// Font units to pixels for a face `px` pixels high.
    pub fn scale_factor(&self, px: f32) -> PxScaleFactor {
        let f = px / self.height_unscaled();
        PxScaleFactor { horizontal: f, vertical: f }
    }

    /// The ascent of a face `px` pixels high, in pixels.
    pub fn ascent(&self, px: f32) -> f32 {
        self.ascent * self.scale_factor(px).vertical
    }

    /// The advance of glyph `id` in a face `px` pixels high, in pixels.
    pub fn h_advance(&self, id: GlyphId, px: f32) -> f32 {
        let metrics = self.font.glyph_metrics(Size::unscaled(), LocationRef::default());
        let units = metrics.advance_width(skrifa::GlyphId::new(u32::from(id.0))).unwrap_or(0.0);
        units * self.scale_factor(px).horizontal
    }

    /// The contours of glyph `id` in font units; `None` without any (a
    /// space).
    pub fn outline(&self, id: GlyphId) -> Option<Outline> {
        let outlines = self.font.outline_glyphs();
        let glyph = outlines.get(skrifa::GlyphId::new(u32::from(id.0)))?;
        let mut pen = CurvePen::default();
        glyph.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).ok()?;
        pen.finish()
    }

    /// Glyph `id` of a face `px` pixels high with its origin at `position`
    /// (pixels, y down), ready to rasterise.
    pub fn outlined(&self, id: GlyphId, px: f32, position: Point) -> Option<Outlined> {
        Some(Outlined::new(self.outline(id)?, self.scale_factor(px), position))
    }
}

/// Collects a contour as [`OutlineCurve`]s and the bounds of its points.
#[derive(Default)]
struct CurvePen {
    last: Point,
    last_move: Option<Point>,
    curves: Vec<OutlineCurve>,
    /// x min, y min, x max, y max of every point.
    bbox: Option<(f32, f32, f32, f32)>,
}

impl CurvePen {
    fn extend(&mut self, p: Point) {
        self.bbox = Some(match self.bbox {
            None => (p.x, p.y, p.x, p.y),
            Some((x0, y0, x1, y1)) => (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)),
        });
    }

    /// The outline, its bounds truncated to whole font units as a face's
    /// glyph box is; `None` for a glyph without contours.
    fn finish(self) -> Option<Outline> {
        let (x0, y0, x1, y1) = self.bbox?;
        if self.curves.is_empty() { return None; }
        let t = |v: f32| v as i16 as f32;
        Some(Outline { bounds: Rect { min: point(t(x0), t(y1)), max: point(t(x1), t(y0)) }, curves: self.curves })
    }
}

impl OutlinePen for CurvePen {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = point(x, y);
        self.extend(p);
        self.last = p;
        self.last_move = Some(p);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = point(x, y);
        self.extend(p);
        self.curves.push(OutlineCurve::Line(self.last, p));
        self.last = p;
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (c, p) = (point(cx, cy), point(x, y));
        self.extend(c);
        self.extend(p);
        self.curves.push(OutlineCurve::Quad(self.last, c, p));
        self.last = p;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, p) = (point(cx0, cy0), point(cx1, cy1), point(x, y));
        self.extend(c0);
        self.extend(c1);
        self.extend(p);
        self.curves.push(OutlineCurve::Cubic(self.last, c0, c1, p));
        self.last = p;
    }

    fn close(&mut self) {
        if let Some(m) = self.last_move.take()
            && m != self.last {
            self.curves.push(OutlineCurve::Line(self.last, m));
            self.last = m;
        }
    }
}

/// An outline placed at a pixel position, with its pixel bounds.
pub struct Outlined {
    outline: Outline,
    scale: PxScaleFactor,
    position: Point,
    px_bounds: Rect,
}

impl Outlined {
    /// `outline` scaled by `scale` with its origin at `position` (pixels,
    /// y down).
    pub fn new(outline: Outline, scale: PxScaleFactor, position: Point) -> Outlined {
        let Rect { min, max } = outline.bounds;
        // The subpixel fraction goes into the rounding, so that glyphs at
        // the same fraction get the same pels.
        let (xt, xf) = (position.x.trunc(), position.x.fract());
        let (yt, yf) = (position.y.trunc(), position.y.fract());
        let px_bounds = Rect {
            min: point((min.x * scale.horizontal + xf).floor() + xt, (min.y * -scale.vertical + yf).floor() + yt),
            max: point((max.x * scale.horizontal + xf).ceil() + xt, (max.y * -scale.vertical + yf).ceil() + yt),
        };
        Outlined { outline, scale, position, px_bounds }
    }

    /// The pels it covers, whole pixels (y down).
    pub fn px_bounds(&self) -> Rect {
        self.px_bounds
    }

    /// Call `o(x, y, coverage)` for every pel of [`Self::px_bounds`]
    /// (relative to its corner), coverage 0.0 to 1.0.
    pub fn draw<O: FnMut(u32, u32, f32)>(&self, o: O) {
        let (h, v) = (self.scale.horizontal, -self.scale.vertical);
        let offset = point(self.position.x - self.px_bounds.min.x, self.position.y - self.px_bounds.min.y);
        let (w, ht) = (self.px_bounds.width() as usize, self.px_bounds.height() as usize);
        let up = |p: &Point| point(p.x * h + offset.x, p.y * v + offset.y);
        let mut r = Rasterizer::new(w, ht);
        for curve in &self.outline.curves {
            match curve {
                OutlineCurve::Line(a, b) => r.draw_line(up(a), up(b)),
                OutlineCurve::Quad(a, b, c) => r.draw_quad(up(a), up(b), up(c)),
                OutlineCurve::Cubic(a, b, c, d) => r.draw_cubic(up(a), up(b), up(c), up(d)),
            }
        }
        r.for_each_pixel_2d(o);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contours_close_and_bounds_take_every_point() {
        let mut pen = CurvePen::default();
        pen.move_to(0.0, 0.0);
        pen.line_to(10.0, 0.0);
        pen.quad_to(20.0, 30.0, 10.0, 20.0);
        pen.close();
        let o = pen.finish().unwrap();
        assert_eq!(o.curves.len(), 3, "closed with a line back to the start");
        assert_eq!(o.curves[2], OutlineCurve::Line(point(10.0, 20.0), point(0.0, 0.0)));
        // The control point (20, 30) counts: min = (left, top), max = (right, bottom).
        assert_eq!(o.bounds, Rect { min: point(0.0, 30.0), max: point(20.0, 0.0) });
        assert!(CurvePen::default().finish().is_none(), "no contours, no outline");
    }

    #[test]
    fn a_square_fills_its_pixel_bounds() {
        let o = Outline {
            bounds: Rect { min: point(0.0, 10.0), max: point(10.0, 0.0) },
            curves: vec![OutlineCurve::Line(point(0.0, 0.0), point(0.0, 10.0)),
                         OutlineCurve::Line(point(0.0, 10.0), point(10.0, 10.0)),
                         OutlineCurve::Line(point(10.0, 10.0), point(10.0, 0.0)),
                         OutlineCurve::Line(point(10.0, 0.0), point(0.0, 0.0))],
        };
        let g = Outlined::new(o, PxScaleFactor { horizontal: 0.5, vertical: 0.5 }, point(2.0, 8.0));
        let b = g.px_bounds();
        assert_eq!((b.min.x, b.min.y, b.max.x, b.max.y), (2.0, 3.0, 7.0, 8.0), "5 × 5 above the baseline");
        let mut full = 0;
        g.draw(|_, _, c| if c > 0.99 { full += 1 });
        assert_eq!(full, 25);
    }
}
