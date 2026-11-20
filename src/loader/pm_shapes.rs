// SPDX-License-Identifier: GPL-3.0-or-later
//
// Geometry for GPI arcs and the area pattern table.
//
// Arcs (`GpiFullArc`, `GpiPartialArc`, `GpiPointArc`) are drawn on the
// ellipse obtained by transforming the unit circle with the presentation
// space's arc parameters (`ARCPARAMS` in <pmgpi.h>, set by
// `GpiSetArcParams`):
//
//     x = xc + m·(P·cos t + R·sin t)
//     y = yc + m·(S·cos t + Q·sin t)
//
// where m is the arc multiplier (a FIXED 16.16 value) and t runs
// counter-clockwise from the x axis (in degrees at the API). The default
// parameters (1, 1, 0, 0) give a circle of radius m. Arcs come out as
// polylines with about one point every `ARC_STEP` pels, which the surface
// rasterises (`DrawOp::Shape`).
//
// The 8 × 8 pel patterns of the `PATSYM_*` symbols are Warpine's own: the PM
// reference names and pictures them but does not give their bits.

use super::constants::*;
use super::pm_xform::Affine;

/// 1.0 as a FIXED (16.16) value.
pub const FIXED_ONE: i32 = 0x0001_0000;

/// Largest number of points on a full ellipse.
pub const MAX_ARC_POINTS: usize = 2048;
/// Fewest points on a full ellipse.
const MIN_ARC_POINTS: usize = 16;
/// Approximate distance between successive arc points, in pels.
const ARC_STEP: f64 = 2.0;
/// Arc coordinates are clamped to ± this before rounding to pels.
const ARC_COORD_LIMIT: f64 = (1 << 24) as f64;

/// A FIXED (16.16) value as a number.
pub fn fixed(fx: u32) -> f64 {
    fx as i32 as f64 / FIXED_ONE as f64
}

/// The arc parameters of a presentation space (`ARCPARAMS`: lP, lQ, lR, lS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArcParams {
    pub p: i32,
    pub q: i32,
    pub r: i32,
    pub s: i32,
}

impl Default for ArcParams {
    /// A unit circle.
    fn default() -> Self {
        ArcParams { p: 1, q: 1, r: 0, s: 0 }
    }
}

impl ArcParams {
    /// The parameters as a linear map.
    fn lin(&self) -> Lin {
        Lin { p: self.p as f64, q: self.q as f64, r: self.r as f64, s: self.s as f64 }
    }
}

/// A linear map `(u, v) → (p·u + r·v, s·u + q·v)`: the arc parameters,
/// possibly followed by the linear part of a presentation space's world →
/// device transform (so arcs are generated in device pels, `*_mapped`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lin {
    pub p: f64,
    pub q: f64,
    pub r: f64,
    pub s: f64,
}

impl Lin {
    /// Apply the transform to `(u, v)` (no translation).
    fn map(&self, (u, v): (f64, f64)) -> (f64, f64) {
        (self.p * u + self.r * v, self.s * u + self.q * v)
    }

    fn det(&self) -> f64 {
        self.p * self.q - self.r * self.s
    }

    /// The inverse transform, if the matrix is invertible.
    fn unmap(&self, (x, y): (f64, f64)) -> Option<(f64, f64)> {
        let d = self.det();
        (d != 0.0).then(|| ((self.q * x - self.r * y) / d, (-self.s * x + self.p * y) / d))
    }

    /// The longest semi-axis bound: how far a unit-circle point can land.
    fn reach(&self) -> f64 {
        let col1 = self.p.hypot(self.s);
        let col2 = self.r.hypot(self.q);
        col1 + col2
    }

    /// `self` followed by the linear part of `m`.
    fn then(&self, m: &Affine) -> Lin {
        Lin {
            p: m.a * self.p + m.c * self.s,
            r: m.a * self.r + m.c * self.q,
            s: m.b * self.p + m.d * self.s,
            q: m.b * self.r + m.d * self.q,
        }
    }
}

fn pel((x, y): (f64, f64)) -> (i32, i32) {
    let c = |v: f64| v.clamp(-ARC_COORD_LIMIT, ARC_COORD_LIMIT).round() as i32;
    (c(x), c(y))
}

/// Points for `fraction` of a full turn on a curve of the given reach.
fn point_count(reach: f64, fraction: f64) -> usize {
    let full = (std::f64::consts::TAU * reach.abs() / ARC_STEP).ceil();
    let full = if full.is_finite() { full as usize } else { MAX_ARC_POINTS };
    let full = full.clamp(MIN_ARC_POINTS, MAX_ARC_POINTS);
    ((full as f64 * fraction.abs()).ceil() as usize).clamp(1, MAX_ARC_POINTS)
}

/// Drop points equal to their predecessor.
fn dedup(mut pts: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    pts.dedup();
    pts
}

/// The ellipse of `GpiFullArc`: centre `c`, multiplier `mult`, as a closed
/// polygon (the first point is not repeated at the end).
pub fn full_arc(c: (i32, i32), params: ArcParams, mult: f64) -> Vec<(i32, i32)> {
    full_arc_lin((c.0 as f64, c.1 as f64), params.lin(), mult)
}

/// [`full_arc`] in device pels: centre and ellipse mapped by `m`.
pub fn full_arc_mapped(m: &Affine, c: (i32, i32), params: ArcParams, mult: f64) -> Vec<(i32, i32)> {
    full_arc_lin(m.apply((c.0 as f64, c.1 as f64)), params.lin().then(m), mult)
}

fn full_arc_lin((cx, cy): (f64, f64), lin: Lin, mult: f64) -> Vec<(i32, i32)> {
    let n = point_count(lin.reach() * mult, 1.0);
    dedup((0..n).map(|i| {
        let t = std::f64::consts::TAU * i as f64 / n as f64;
        let (x, y) = lin.map((mult * t.cos(), mult * t.sin()));
        pel((cx + x, cy + y))
    }).collect())
}

/// The arc of `GpiPartialArc`: from `start` degrees, `sweep` degrees
/// counter-clockwise (capped at a full turn), first point to last.
pub fn partial_arc(c: (i32, i32), params: ArcParams, mult: f64, start: f64, sweep: f64) -> Vec<(i32, i32)> {
    partial_arc_lin((c.0 as f64, c.1 as f64), params.lin(), mult, start, sweep)
}

/// [`partial_arc`] in device pels: centre and ellipse mapped by `m`.
pub fn partial_arc_mapped(m: &Affine, c: (i32, i32), params: ArcParams, mult: f64, start: f64, sweep: f64)
    -> Vec<(i32, i32)>
{
    partial_arc_lin(m.apply((c.0 as f64, c.1 as f64)), params.lin().then(m), mult, start, sweep)
}

fn partial_arc_lin((cx, cy): (f64, f64), lin: Lin, mult: f64, start: f64, sweep: f64) -> Vec<(i32, i32)> {
    let sweep = sweep.clamp(0.0, 360.0);
    let n = point_count(lin.reach() * mult, sweep / 360.0);
    dedup((0..=n).map(|i| {
        let t = (start + sweep * i as f64 / n as f64).to_radians();
        let (x, y) = lin.map((mult * t.cos(), mult * t.sin()));
        pel((cx + x, cy + y))
    }).collect())
}

/// The arc of `GpiPointArc`: from `p0` through `p1` to `p2`, on an ellipse
/// of the shape the arc parameters give. In the parameters' unit space the
/// three points lie on a circle, which is found and mapped back. When the
/// points are collinear (or the parameters are singular) there is no such
/// arc and the result is the straight path `p0` → `p1` → `p2` — Warpine's
/// choice; the first and last points are always `p0` and `p2`.
pub fn point_arc(p0: (i32, i32), p1: (i32, i32), p2: (i32, i32), params: ArcParams) -> Vec<(i32, i32)> {
    point_arc_lin(p0, p1, p2, params.lin())
}

/// [`point_arc`] in device pels: `p0`, `p1`, `p2` are already device
/// points and the ellipse shape is the arc parameters followed by the
/// linear part of `m` (an affine map takes the world ellipse through the
/// three world points to the device ellipse through their images).
pub fn point_arc_mapped(m: &Affine, p0: (i32, i32), p1: (i32, i32), p2: (i32, i32), params: ArcParams)
    -> Vec<(i32, i32)>
{
    point_arc_lin(p0, p1, p2, params.lin().then(m))
}

fn point_arc_lin(p0: (i32, i32), p1: (i32, i32), p2: (i32, i32), params: Lin) -> Vec<(i32, i32)> {
    let straight = || dedup(vec![p0, p1, p2]);
    let f = |p: (i32, i32)| (p.0 as f64, p.1 as f64);
    let (Some(a), Some(b), Some(c)) = (params.unmap(f(p0)), params.unmap(f(p1)), params.unmap(f(p2)))
        else { return straight() };
    // Circumcentre of a, b, c.
    let d = 2.0 * (a.0 * (b.1 - c.1) + b.0 * (c.1 - a.1) + c.0 * (a.1 - b.1));
    if d.abs() < 1e-9 { return straight(); }
    let sq = |p: (f64, f64)| p.0 * p.0 + p.1 * p.1;
    let ux = (sq(a) * (b.1 - c.1) + sq(b) * (c.1 - a.1) + sq(c) * (a.1 - b.1)) / d;
    let uy = (sq(a) * (c.0 - b.0) + sq(b) * (a.0 - c.0) + sq(c) * (b.0 - a.0)) / d;
    let rho = (a.0 - ux).hypot(a.1 - uy);
    if !rho.is_finite() { return straight(); }
    let angle = |p: (f64, f64)| (p.1 - uy).atan2(p.0 - ux);
    let tau = std::f64::consts::TAU;
    let (t0, t1, t2) = (angle(a), angle(b), angle(c));
    let ccw = (t2 - t0).rem_euclid(tau);
    // Go the way that passes through p1.
    let sweep = if (t1 - t0).rem_euclid(tau) <= ccw { ccw } else { ccw - tau };
    let n = point_count(params.reach() * rho, sweep / tau).max(2);
    let mut pts: Vec<(i32, i32)> = (0..=n).map(|i| {
        let t = t0 + sweep * i as f64 / n as f64;
        pel(params.map((ux + rho * t.cos(), uy + rho * t.sin())))
    }).collect();
    pts[0] = p0;
    pts[n] = p2;
    dedup(pts)
}

// ── Curves: fillets and splines ──────────────────────────────────────────────

/// Most points one fillet or spline is flattened into.
const MAX_CURVE_POINTS: usize = 256;
/// Fewest points per curve.
const MIN_CURVE_POINTS: usize = 4;

fn fpt(p: (i32, i32)) -> (f64, f64) {
    (p.0 as f64, p.1 as f64)
}

/// Points for a curve whose control polygon is `len` pels long.
fn curve_points(len: f64) -> usize {
    let n = (len / ARC_STEP).ceil();
    if n.is_finite() { (n as usize).clamp(MIN_CURVE_POINTS, MAX_CURVE_POINTS) } else { MAX_CURVE_POINTS }
}

fn dist_f(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// The conic from `a` to `c` with control point `b` and sharpness `w`
/// (`GpiPolyFilletSharp`): the rational quadratic Bézier with weight `w`
/// on `b`. Its point at t = ½ is D = (W + w·B) / (1 + w), W the midpoint of
/// AC, so WD / DB = w — the reference's definition of sharpness (w > 1 a
/// hyperbola, 1 a parabola, < 1 an ellipse). Points after `a`, ending
/// exactly on `c`.
pub fn conic(a: (i32, i32), b: (i32, i32), c: (i32, i32), w: f64) -> Vec<(i32, i32)> {
    let (fa, fb, fc) = (fpt(a), fpt(b), fpt(c));
    let n = curve_points(dist_f(fa, fb) + dist_f(fb, fc));
    let mut pts: Vec<(i32, i32)> = (1..=n).map(|i| {
        let t = i as f64 / n as f64;
        let (k0, k1, k2) = ((1.0 - t) * (1.0 - t), 2.0 * w * t * (1.0 - t), t * t);
        let d = k0 + k1 + k2;
        pel(((k0 * fa.0 + k1 * fb.0 + k2 * fc.0) / d, (k0 * fa.1 + k1 * fb.1 + k2 * fc.1) / d))
    }).collect();
    if let Some(last) = pts.last_mut() { *last = c; }
    dedup(pts)
}

/// The cubic Bézier spline from `p0` with control points `c1`, `c2` to
/// `p3` (`GpiPolySpline`); points after `p0`, ending exactly on `p3`.
pub fn cubic(p0: (i32, i32), c1: (i32, i32), c2: (i32, i32), p3: (i32, i32)) -> Vec<(i32, i32)> {
    let (a, b, c, d) = (fpt(p0), fpt(c1), fpt(c2), fpt(p3));
    let n = curve_points(dist_f(a, b) + dist_f(b, c) + dist_f(c, d));
    let mut pts: Vec<(i32, i32)> = (1..=n).map(|i| {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let (k0, k1, k2, k3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        pel((k0 * a.0 + k1 * b.0 + k2 * c.0 + k3 * d.0, k0 * a.1 + k1 * b.1 + k2 * c.1 + k3 * d.1))
    }).collect();
    if let Some(last) = pts.last_mut() { *last = p3; }
    dedup(pts)
}

/// The curve of `GpiPolyFillet` from the current position `from` through
/// the imaginary polyline `from`, `pts[0]`, …: with one point a straight
/// line; with two a fillet from `from` to `pts[1]` tangent to both lines;
/// with more, the inner lines are split at their midpoints and a fillet
/// joins each pair of split points around the corner between them. The
/// fillets are parabolic (sharpness 1) — the reference does not give their
/// sharpness, so this is Warpine's choice. Points after `from`.
pub fn poly_fillet(from: (i32, i32), pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    match pts.len() {
        0 => return Vec::new(),
        1 => return vec![pts[0]],
        _ => {}
    }
    let mid = |a: (i32, i32), b: (i32, i32)| pel(((a.0 as f64 + b.0 as f64) / 2.0, (a.1 as f64 + b.1 as f64) / 2.0));
    let mut out = Vec::new();
    let mut start = from;
    let corners = pts.len() - 1; // pts[0] .. pts[n-2] are the control points
    for i in 0..corners {
        let end = if i + 1 == corners { pts[i + 1] } else { mid(pts[i], pts[i + 1]) };
        out.extend(conic(start, pts[i], end, 1.0));
        start = end;
    }
    out
}

/// Bayer 8 × 8 ordered-dither thresholds (0..64), used for the
/// `PATSYM_DENSE*` densities.
const BAYER_8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// Pels (of 64) set by `PATSYM_DENSE1` (densest) … `PATSYM_DENSE8`.
const DENSE_LEVELS: [u8; 8] = [56, 48, 40, 32, 24, 16, 8, 4];

fn dither(level: u8) -> [u8; 8] {
    let mut rows = [0u8; 8];
    for (row, thresholds) in rows.iter_mut().zip(&BAYER_8) {
        for (c, &t) in thresholds.iter().enumerate() {
            if t < level { *row |= 0x80 >> c; }
        }
    }
    rows
}

/// Diagonal lines 4 pels apart, `thick` pels wide; rising to the right
/// (bottom-left to top-right) or falling.
fn diagonal(rising: bool, thick: u32) -> [u8; 8] {
    let mut rows = [0u8; 8];
    for (r, row) in rows.iter_mut().enumerate() {
        for c in 0..8i32 {
            let k = if rising { c - r as i32 } else { c + r as i32 };
            if (k.rem_euclid(4) as u32) < thick { *row |= 0x80 >> c; }
        }
    }
    rows
}

fn or(a: [u8; 8], b: [u8; 8]) -> [u8; 8] {
    std::array::from_fn(|i| a[i] | b[i])
}

/// The 8 × 8 bits of pattern symbol `symbol` (`PATSYM_*`), row 0 first
/// (the bottom row of each repeat); `None` for values that are not pattern
/// symbols.
pub fn pattern_rows(symbol: u32) -> Option<[u8; 8]> {
    const VERT: [u8; 8] = [0x88; 8];
    const HORIZ: [u8; 8] = [0xFF, 0, 0, 0, 0xFF, 0, 0, 0];
    Some(match symbol {
        PATSYM_DEFAULT | PATSYM_SOLID => [0xFF; 8],
        PATSYM_DENSE1..=PATSYM_DENSE8 => dither(DENSE_LEVELS[(symbol - PATSYM_DENSE1) as usize]),
        PATSYM_VERT => VERT,
        PATSYM_HORIZ => HORIZ,
        PATSYM_DIAG1 => diagonal(true, 1),
        PATSYM_DIAG2 => diagonal(true, 2),
        PATSYM_DIAG3 => diagonal(false, 1),
        PATSYM_DIAG4 => diagonal(false, 2),
        PATSYM_NOSHADE | PATSYM_BLANK => [0; 8],
        PATSYM_HALFTONE => [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55],
        PATSYM_HATCH => or(VERT, HORIZ),
        PATSYM_DIAGHATCH => or(diagonal(true, 1), diagonal(false, 1)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(p: (i32, i32), c: (i32, i32)) -> f64 {
        ((p.0 - c.0) as f64).hypot((p.1 - c.1) as f64)
    }

    #[test]
    fn fixed_values() {
        assert_eq!(fixed(FIXED_ONE as u32), 1.0);
        assert_eq!(fixed(0x0002_8000), 2.5);
        assert_eq!(fixed((-FIXED_ONE) as u32), -1.0);
    }

    #[test]
    fn full_arc_with_default_params_is_a_circle() {
        let pts = full_arc((100, 50), ArcParams::default(), 20.0);
        assert!(pts.len() >= MIN_ARC_POINTS);
        for &p in &pts {
            assert!((dist(p, (100, 50)) - 20.0).abs() <= 0.75, "{p:?} on the circle");
        }
        assert!(pts.contains(&(120, 50)) && pts.contains(&(80, 50)), "right and left extremes");
        assert!(pts.contains(&(100, 70)) && pts.contains(&(100, 30)));
    }

    #[test]
    fn arc_params_shape_the_ellipse() {
        // P = 3, Q = 1: three times as wide as high.
        let pts = full_arc((0, 0), ArcParams { p: 3, q: 1, r: 0, s: 0 }, 10.0);
        let xmax = pts.iter().map(|p| p.0).max().unwrap();
        let ymax = pts.iter().map(|p| p.1).max().unwrap();
        assert_eq!((xmax, ymax), (30, 10));
        // A singular matrix degenerates to a line, without panicking.
        let flat = full_arc((0, 0), ArcParams { p: 1, q: 0, r: 0, s: 0 }, 10.0);
        assert!(flat.iter().all(|p| p.1 == 0));
    }

    #[test]
    fn partial_arcs_run_counter_clockwise_from_the_start_angle() {
        let pts = partial_arc((0, 0), ArcParams::default(), 10.0, 0.0, 90.0);
        assert_eq!(pts.first(), Some(&(10, 0)));
        assert_eq!(pts.last(), Some(&(0, 10)));
        assert!(pts.iter().all(|p| p.0 >= 0 && p.1 >= 0), "first quadrant only");
        let half = partial_arc((0, 0), ArcParams::default(), 10.0, 90.0, 180.0);
        assert_eq!((half.first(), half.last()), (Some(&(0, 10)), Some(&(0, -10))));
        assert!(half.iter().all(|p| p.0 <= 0), "left half");
        // The sweep is capped at a full turn; a zero sweep is one point.
        assert_eq!(partial_arc((0, 0), ArcParams::default(), 10.0, 0.0, 0.0), vec![(10, 0)]);
        let over = partial_arc((0, 0), ArcParams::default(), 10.0, 0.0, 1000.0);
        assert_eq!(over.last(), Some(&(10, 0)));
    }

    #[test]
    fn point_arcs_pass_through_all_three_points() {
        // Circle of radius 10 about the origin: (10,0) → (0,10) → (-10,0).
        let pts = point_arc((10, 0), (0, 10), (-10, 0), ArcParams::default());
        assert_eq!((pts.first(), pts.last()), (Some(&(10, 0)), Some(&(-10, 0))));
        assert!(pts.contains(&(0, 10)), "through the middle point");
        assert!(pts.iter().all(|p| p.1 >= 0), "the upper half, not the lower");
        // The same end points with the middle point below go the other way.
        let lower = point_arc((10, 0), (0, -10), (-10, 0), ArcParams::default());
        assert!(lower.iter().all(|p| p.1 <= 0));
        assert!(lower.contains(&(0, -10)));
        // Collinear points give the straight path.
        assert_eq!(point_arc((0, 0), (5, 5), (10, 10), ArcParams::default()), vec![(0, 0), (5, 5), (10, 10)]);
        // An ellipse twice as wide: points on it are honoured.
        let wide = ArcParams { p: 2, q: 1, r: 0, s: 0 };
        let e = point_arc((20, 0), (0, 10), (-20, 0), wide);
        assert!(e.contains(&(0, 10)));
        for &p in &e {
            let (u, v) = (p.0 as f64 / 20.0, p.1 as f64 / 10.0);
            assert!((u.hypot(v) - 1.0).abs() < 0.08, "{p:?} on the ellipse");
        }
        // Singular parameters fall back to straight lines.
        assert_eq!(point_arc((0, 0), (1, 2), (3, 0), ArcParams { p: 0, q: 0, r: 0, s: 0 }).len(), 3);
    }

    #[test]
    fn extreme_multipliers_are_bounded() {
        let pts = full_arc((0, 0), ArcParams { p: i32::MAX, q: i32::MIN, r: i32::MAX, s: 1 }, 32767.0);
        assert!(pts.len() <= MAX_ARC_POINTS);
        assert!(pts.iter().all(|p| p.0.abs() <= 1 << 24 && p.1.abs() <= 1 << 24));
        let nan = full_arc((0, 0), ArcParams::default(), f64::NAN);
        assert!(nan.len() <= MAX_ARC_POINTS);
        assert!(partial_arc((0, 0), ArcParams::default(), 1e12, 0.0, 360.0).len() <= MAX_ARC_POINTS + 1);
    }

    #[test]
    fn pattern_symbols() {
        assert_eq!(pattern_rows(PATSYM_SOLID), Some([0xFF; 8]));
        assert_eq!(pattern_rows(PATSYM_DEFAULT), pattern_rows(PATSYM_SOLID));
        assert_eq!(pattern_rows(PATSYM_BLANK), Some([0; 8]));
        assert_eq!(pattern_rows(PATSYM_NOSHADE), Some([0; 8]));
        let ones = |s: u32| pattern_rows(s).unwrap().iter().map(|r| r.count_ones()).sum::<u32>();
        // DENSE1 is the densest, each level sparser than the one before.
        for s in PATSYM_DENSE1..PATSYM_DENSE8 {
            assert!(ones(s) > ones(s + 1), "PATSYM_DENSE{} denser than the next", s);
        }
        assert_eq!(ones(PATSYM_DENSE1 + 3), 32, "PATSYM_DENSE4 is half");
        assert_eq!(ones(PATSYM_HALFTONE), 32);
        assert_eq!(pattern_rows(PATSYM_VERT).unwrap(), [0x88; 8]);
        assert_eq!(pattern_rows(PATSYM_HORIZ).unwrap()[0], 0xFF);
        assert_ne!(pattern_rows(PATSYM_DIAG1), pattern_rows(PATSYM_DIAG3), "rising vs falling");
        assert!(ones(PATSYM_DIAG2) > ones(PATSYM_DIAG1));
        assert_eq!(ones(PATSYM_HATCH), 8 * 2 + 2 * 8 - 4);
        for bad in [20, 63, 65, 0xFFFF_FFFF] {
            assert_eq!(pattern_rows(bad), None, "{bad}");
        }
    }

    #[test]
    fn curves_end_on_their_end_points() {
        let c = cubic((0, 0), (0, 100), (100, 100), (100, 0));
        assert_eq!(*c.last().unwrap(), (100, 0));
        assert!(c.contains(&(50, 75)), "the cubic's midpoint");
        assert!(c.len() >= MIN_CURVE_POINTS && c.len() <= MAX_CURVE_POINTS);
        // Sharpness 1: a parabola through (W + B) / 2.
        let p = conic((0, 0), (50, 100), (100, 0), 1.0);
        assert!(p.contains(&(50, 50)));
        // Sharpness 3: D = (W + 3B) / 4.
        let h = conic((0, 0), (50, 100), (100, 0), 3.0);
        assert!(h.contains(&(50, 75)));
        // Sharpness 0: the straight chord.
        assert!(conic((0, 0), (50, 100), (100, 0), 0.0).iter().all(|&(_, y)| y == 0));
        // A fillet chain ends on the last point, through split midpoints.
        let f = poly_fillet((0, 0), &[(0, 100), (100, 100), (100, 0)]);
        assert_eq!(*f.last().unwrap(), (100, 0));
        assert!(f.contains(&(50, 100)), "the midpoint of the inner line");
        assert_eq!(poly_fillet((0, 0), &[(5, 5)]), vec![(5, 5)]);
        assert!(poly_fillet((0, 0), &[]).is_empty());
        // Huge coordinates stay bounded.
        let far = cubic((i32::MIN, 0), (i32::MAX, 0), (0, i32::MAX), (0, 0));
        assert!(far.len() <= MAX_CURVE_POINTS);
    }
}
