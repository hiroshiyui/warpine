// SPDX-License-Identifier: GPL-3.0-or-later

//! GPI coordinate spaces and transforms (PMGPI): pure matrix maths.
//!
//! A presentation space maps its coordinates in four steps (*GPI Guide*,
//! "Coordinate Spaces and Transformations"; `GpiConvert`'s `CVTC_*`):
//!
//! ```text
//! world ──model transform──▶ model ──viewing transform──▶ default page
//!       ──default view matrix──▶ page ──page viewport──▶ device (pels)
//! ```
//!
//! - The model transform (`GpiSetModelTransformMatrix`), the viewing
//!   transform (`GpiSetViewingTransformMatrix`) and the default view matrix
//!   (`GpiSetDefaultViewMatrix`) are `MATRIXLF`s ([`MatrixLf`]): elements
//!   1, 2, 4, 5 FIXED 16.16, 7 and 8 LONG translations, 3, 6, 9 fixed at
//!   0, 0, 1; a point (x, y) maps to (M11·x + M21·y + M31, M12·x + M22·y +
//!   M32). They combine with `TRANSFORM_REPLACE` / `ADD` (existing first,
//!   then the new one) / `PREEMPT` (the new one first) and are stored as
//!   MATRIXLFs, so a composition is rounded to FIXED like the reference's
//!   ("can exceed fixed-point limits and cause an error": a result that does
//!   not fit fails with `PMERR_COORDINATE_OVERFLOW`, Warpine's choice of
//!   code).
//! - The page viewport maps the presentation page (its size from
//!   `GpiCreatePS` / `GpiSetPS`, in page units `PU_*`) to device pels. The
//!   default viewport gives each page unit its physical size at Warpine's
//!   96-dpi display (`PU_LOMETRIC` 0.1 mm = 96/254 pel …); `PU_PELS` and
//!   `PU_ARBITRARY` ("initially pels") map one unit to one pel. A page size
//!   of 0 is the device's: the desktop. `GpiSetPageViewport` replaces it by
//!   a device rectangle (exclusive at the right and top — unverified, the
//!   reference does not say), scaling the page into it.
//! - [`CoordState::world_to_device`] composes the chain into an [`Xform`]
//!   with an identity fast path: with the default state every point is
//!   passed through untouched, so drawing stays bit-for-bit as before.
//!
//! Device coordinates are rounded half away from zero and clamped to
//! ±[`COORD_LIMIT`] (2²⁷, the range of a `GPIF_LONG` presentation space and
//! of regions), so no guest matrix can make the rasterisers overflow.

use super::constants::*;
use super::pm_rect::Rectl;
use crate::gui::region::{RRect, REGION_COORD_LIMIT};

/// Device (and converted) coordinates are clamped to ± this.
pub const COORD_LIMIT: i32 = REGION_COORD_LIMIT;
/// 1.0 in FIXED 16.16.
const FIXED_ONE: f64 = 65536.0;
/// Resolution of Warpine's display, dots per inch (`DevQueryCaps` reports
/// the same 3780 pels per metre).
pub const DEVICE_DPI: f64 = 96.0;
/// Millimetres per inch.
const MM_PER_INCH: f64 = 25.4;
/// Twips per inch (a twip is 1/20 point, 1/1440 inch).
const TWIPS_PER_INCH: f64 = 1440.0;
/// Number of elements of a `MATRIXLF`.
pub const MATRIX_ELEMENTS: usize = 9;
/// Determinants smaller than this make a transform singular (no inverse).
const MIN_DET: f64 = 1e-12;

/// A 2-D affine map in the `MATRIXLF` convention:
/// `(a·x + c·y + e, b·x + d·y + f)` (a = M11, b = M12, c = M21, d = M22,
/// e = M31, f = M32).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn translate(tx: f64, ty: f64) -> Affine {
        Affine { e: tx, f: ty, ..Affine::IDENTITY }
    }

    pub fn scale(sx: f64, sy: f64) -> Affine {
        Affine { a: sx, d: sy, ..Affine::IDENTITY }
    }

    /// Rotation by `degrees` counter-clockwise about the origin. Multiples
    /// of 90° are exact.
    pub fn rotate(degrees: f64) -> Affine {
        let quarter = degrees / 90.0;
        let (sin, cos) = if quarter.fract() == 0.0 && quarter.is_finite() {
            match (quarter as i64).rem_euclid(4) {
                0 => (0.0, 1.0),
                1 => (1.0, 0.0),
                2 => (0.0, -1.0),
                _ => (-1.0, 0.0),
            }
        } else {
            degrees.to_radians().sin_cos()
        };
        Affine { a: cos, b: sin, c: -sin, d: cos, e: 0.0, f: 0.0 }
    }

    /// `self` about the point `(cx, cy)` instead of the origin.
    pub fn about(self, (cx, cy): (f64, f64)) -> Affine {
        Affine::translate(-cx, -cy).then(&self).then(&Affine::translate(cx, cy))
    }

    /// The map that applies `self` first, then `next`.
    pub fn then(&self, next: &Affine) -> Affine {
        Affine {
            a: self.a * next.a + self.b * next.c,
            b: self.a * next.b + self.b * next.d,
            c: self.c * next.a + self.d * next.c,
            d: self.c * next.b + self.d * next.d,
            e: self.e * next.a + self.f * next.c + next.e,
            f: self.e * next.b + self.f * next.d + next.f,
        }
    }

    pub fn apply(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// The linear part applied to a vector (no translation).
    pub fn apply_vector(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (self.a * x + self.c * y, self.b * x + self.d * y)
    }

    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// The inverse map; `None` when singular or not finite.
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.det();
        if !det.is_finite() || det.abs() < MIN_DET { return None; }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        let inv = Affine { a, b, c, d, e: -(self.e * a + self.f * c), f: -(self.e * b + self.f * d) };
        inv.is_finite().then_some(inv)
    }

    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f].iter().all(|v| v.is_finite())
    }

    pub fn is_identity(&self) -> bool {
        *self == Affine::IDENTITY
    }

    /// Whether horizontal and vertical lines stay horizontal and vertical
    /// (no rotation or shear): a rectangle maps to a rectangle.
    pub fn axis_aligned(&self) -> bool {
        self.b == 0.0 && self.c == 0.0
    }
}

/// A `MATRIXLF` as stored: FIXED scale/rotation elements, LONG translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatrixLf {
    pub m11: i32,
    pub m12: i32,
    pub m21: i32,
    pub m22: i32,
    pub m31: i32,
    pub m32: i32,
}

impl Default for MatrixLf {
    fn default() -> Self {
        MatrixLf::IDENTITY
    }
}

/// A FIXED value of `v`; `None` when it does not fit.
fn to_fixed(v: f64) -> Option<i32> {
    let r = (v * FIXED_ONE).round();
    (r.is_finite() && r >= i32::MIN as f64 && r <= i32::MAX as f64).then_some(r as i32)
}

/// A LONG translation of `v`; `None` outside ±[`COORD_LIMIT`].
fn to_long(v: f64) -> Option<i32> {
    let r = v.round();
    (r.is_finite() && r.abs() <= COORD_LIMIT as f64).then_some(r as i32)
}

impl MatrixLf {
    pub const IDENTITY: MatrixLf =
        MatrixLf { m11: FIXED_ONE as i32, m12: 0, m21: 0, m22: FIXED_ONE as i32, m31: 0, m32: 0 };

    /// The matrix of the first `elements.len()` (≤ 9) elements of a
    /// `MATRIXLF` array, the rest from the identity ("If lCount is less than
    /// 9, remaining elements default to the corresponding elements of the
    /// identity matrix"). Elements 3, 6 and 9, when given, must be 0, 0, 1
    /// (`PMERR_INV_MATRIX_ELEMENT`).
    pub fn from_elements(elements: &[i32]) -> Result<MatrixLf, u32> {
        let mut all = MatrixLf::IDENTITY.elements();
        if elements.len() > MATRIX_ELEMENTS { return Err(PMERR_INV_LENGTH_OR_COUNT); }
        all[..elements.len()].copy_from_slice(elements);
        if all[2] != 0 || all[5] != 0 || all[8] != 1 { return Err(PMERR_INV_MATRIX_ELEMENT); }
        Ok(MatrixLf { m11: all[0], m12: all[1], m21: all[3], m22: all[4], m31: all[6], m32: all[7] })
    }

    /// The nine elements of the `MATRIXLF`, in row order.
    pub fn elements(&self) -> [i32; MATRIX_ELEMENTS] {
        [self.m11, self.m12, 0, self.m21, self.m22, 0, self.m31, self.m32, 1]
    }

    pub fn affine(&self) -> Affine {
        Affine {
            a: self.m11 as f64 / FIXED_ONE,
            b: self.m12 as f64 / FIXED_ONE,
            c: self.m21 as f64 / FIXED_ONE,
            d: self.m22 as f64 / FIXED_ONE,
            e: self.m31 as f64,
            f: self.m32 as f64,
        }
    }

    /// `m` rounded to a MATRIXLF; `PMERR_COORDINATE_OVERFLOW` when an
    /// element does not fit (a FIXED beyond ±32768, a translation beyond
    /// ±[`COORD_LIMIT`]).
    pub fn from_affine(m: &Affine) -> Result<MatrixLf, u32> {
        let fx = |v| to_fixed(v).ok_or(PMERR_COORDINATE_OVERFLOW);
        let lg = |v| to_long(v).ok_or(PMERR_COORDINATE_OVERFLOW);
        Ok(MatrixLf { m11: fx(m.a)?, m12: fx(m.b)?, m21: fx(m.c)?, m22: fx(m.d)?, m31: lg(m.e)?, m32: lg(m.f)? })
    }

    /// Combine `new` into `self` by `option` (`TRANSFORM_*`).
    pub fn combine(&self, new: &Affine, option: u32) -> Result<MatrixLf, u32> {
        let old = self.affine();
        let m = match option {
            TRANSFORM_REPLACE => *new,
            TRANSFORM_ADD => old.then(new),
            TRANSFORM_PREEMPT => new.then(&old),
            _ => return Err(PMERR_INV_TRANSFORM_TYPE),
        };
        MatrixLf::from_affine(&m)
    }
}

/// A device coordinate: rounded half away from zero, clamped to
/// ±[`COORD_LIMIT`] (NaN → 0).
pub fn device_coord(v: f64) -> i32 {
    if v.is_nan() { return 0; }
    v.round().clamp(-(COORD_LIMIT as f64), COORD_LIMIT as f64) as i32
}

/// A converted coordinate, `None` beyond ±[`COORD_LIMIT`] (GpiConvert's
/// `PMERR_COORDINATE_OVERFLOW`).
pub fn checked_coord(v: f64) -> Option<i32> {
    to_long(v)
}

/// The world → device map of a presentation space, with an identity fast
/// path: when `identity` is set, points pass through unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub m: Affine,
    pub identity: bool,
}

impl Default for Xform {
    fn default() -> Self {
        Xform::IDENTITY
    }
}

impl Xform {
    pub const IDENTITY: Xform = Xform { m: Affine::IDENTITY, identity: true };

    pub fn new(m: Affine) -> Xform {
        Xform { m, identity: m.is_identity() }
    }

    /// A world point in device pels.
    pub fn pt(&self, p: (i32, i32)) -> (i32, i32) {
        if self.identity { return p; }
        let (x, y) = self.m.apply((p.0 as f64, p.1 as f64));
        (device_coord(x), device_coord(y))
    }

    /// World points in device pels.
    pub fn pts(&self, pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
        if self.identity { return pts.to_vec(); }
        pts.iter().map(|&p| self.pt(p)).collect()
    }

    /// A device point in world coordinates; `None` when the map is singular.
    pub fn inverse_pt(&self, p: (i32, i32)) -> Option<(i32, i32)> {
        if self.identity { return Some(p); }
        let (x, y) = self.m.inverse()?.apply((p.0 as f64, p.1 as f64));
        Some((device_coord(x), device_coord(y)))
    }

    pub fn axis_aligned(&self) -> bool {
        self.identity || self.m.axis_aligned()
    }

    /// The device bounding box (inclusive) of the world rectangle with
    /// inclusive corners `(x0, y0)`, `(x1, y1)`: the corners are points, as
    /// for `GpiBox`, so a ×2 scale maps 10..19 to 20..38 (Warpine's reading
    /// of "the boundaries are part of the interior"; unverified).
    pub fn bounds(&self, (x0, y0): (i32, i32), (x1, y1): (i32, i32)) -> (i32, i32, i32, i32) {
        if self.identity { return (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)); }
        bounding_box(&[self.pt((x0, y0)), self.pt((x1, y0)), self.pt((x1, y1)), self.pt((x0, y1))])
    }

    /// The world bounding box (inclusive) of the inclusive device rectangle
    /// `(x0, y0)`–`(x1, y1)`, the inverse of [`Xform::bounds`]; `None` when
    /// the map is singular.
    pub fn inverse_bounds(&self, (x0, y0): (i32, i32), (x1, y1): (i32, i32)) -> Option<(i32, i32, i32, i32)> {
        if self.identity { return Some((x0, y0, x1, y1)); }
        let inv = Xform::new(self.m.inverse()?);
        Some(inv.bounds((x0, y0), (x1, y1)))
    }
}

/// The inclusive bounding box of `pts` (at least one point).
pub fn bounding_box(pts: &[(i32, i32)]) -> (i32, i32, i32, i32) {
    let xs = pts.iter().map(|p| p.0);
    let ys = pts.iter().map(|p| p.1);
    (xs.clone().min().unwrap_or(0), ys.clone().min().unwrap_or(0), xs.max().unwrap_or(0), ys.max().unwrap_or(0))
}

/// Pels per page unit (`PU_*`) at [`DEVICE_DPI`]; `None` for an invalid
/// unit.
pub fn pels_per_unit(units: u32) -> Option<f64> {
    Some(match units {
        PU_PELS | PU_ARBITRARY => 1.0,
        PU_LOMETRIC => DEVICE_DPI / (MM_PER_INCH * 10.0),
        PU_HIMETRIC => DEVICE_DPI / (MM_PER_INCH * 100.0),
        PU_LOENGLISH => DEVICE_DPI / 100.0,
        PU_HIENGLISH => DEVICE_DPI / 1000.0,
        PU_TWIPS => DEVICE_DPI / TWIPS_PER_INCH,
        _ => return None,
    })
}

/// The coordinate state of a presentation space: page units and size,
/// page viewport, the three transforms, viewing limits and graphics field.
#[derive(Clone, Debug, PartialEq)]
pub struct CoordState {
    /// `flOptions` of `GpiCreatePS` / `GpiSetPS` (`PS_UNITS`, `PS_FORMAT`,
    /// `PS_TYPE`, `PS_ASSOCIATE` fields), reported by `GpiQueryPS`.
    pub options: u32,
    /// Page size in page units as given; 0 = the device's.
    pub page_size: (i32, i32),
    /// `GpiSetPageViewport` (device, exclusive at the right and top);
    /// `None` = the default viewport.
    pub viewport: Option<Rectl>,
    pub model: MatrixLf,
    pub viewing: MatrixLf,
    pub default_view: MatrixLf,
    /// `GpiSetViewingLimits` (model space, inclusive); `None` = widest.
    pub viewing_limits: Option<Rectl>,
    /// `GpiSetGraphicsField` (page space, inclusive); `None` = none.
    pub graphics_field: Option<Rectl>,
}

impl Default for CoordState {
    /// A `PU_PELS` page the size of the device, identity transforms.
    fn default() -> Self {
        CoordState {
            options: PU_PELS,
            page_size: (0, 0),
            viewport: None,
            model: MatrixLf::IDENTITY,
            viewing: MatrixLf::IDENTITY,
            default_view: MatrixLf::IDENTITY,
            viewing_limits: None,
            graphics_field: None,
        }
    }
}

/// The widest viewing limits (`GpiQueryViewingLimits` with none set).
pub const WIDEST_LIMITS: Rectl =
    Rectl { x_left: -COORD_LIMIT, y_bottom: -COORD_LIMIT, x_right: COORD_LIMIT, y_top: COORD_LIMIT };

impl CoordState {
    /// The state for `GpiCreatePS` / `GpiSetPS` options and page size.
    pub fn with_page(options: u32, page_size: (i32, i32)) -> CoordState {
        CoordState { options, page_size, ..CoordState::default() }
    }

    /// The page units (`PU_*`); an invalid value counts as `PU_PELS`
    /// (Warpine's leniency: older Warpine accepted any options).
    pub fn units(&self) -> u32 {
        let u = self.options & PS_UNITS;
        if pels_per_unit(u).is_some() { u } else { PU_PELS }
    }

    fn ppu(&self) -> f64 {
        pels_per_unit(self.units()).unwrap_or(1.0)
    }

    /// The page size in page units: as given, or (for a 0 dimension) the
    /// desktop's size in those units.
    pub fn page_size(&self) -> (i32, i32) {
        let (dw, dh) = desktop_size();
        let ppu = self.ppu();
        let dim = |given: i32, device: u32| {
            if given > 0 { given } else { device_coord(device as f64 / ppu).max(1) }
        };
        (dim(self.page_size.0, dw), dim(self.page_size.1, dh))
    }

    /// The default page viewport: the page at its physical size, at the
    /// bottom-left of the device.
    pub fn default_viewport(&self) -> Rectl {
        let (cx, cy) = self.page_size();
        let ppu = self.ppu();
        Rectl::new(0, 0, device_coord(cx as f64 * ppu), device_coord(cy as f64 * ppu))
    }

    /// The page viewport in force.
    pub fn page_viewport(&self) -> Rectl {
        self.viewport.unwrap_or_else(|| self.default_viewport())
    }

    /// Page → device. The default viewport scales by the unit size exactly
    /// (so `PU_PELS` is the identity); a set one by its size over the page
    /// size.
    pub fn page_to_device(&self) -> Affine {
        match self.viewport {
            None => {
                let ppu = self.ppu();
                Affine::scale(ppu, ppu)
            }
            Some(vp) => {
                let (cx, cy) = self.page_size();
                let sx = (vp.x_right - vp.x_left) as f64 / cx.max(1) as f64;
                let sy = (vp.y_top - vp.y_bottom) as f64 / cy.max(1) as f64;
                Affine { a: sx, d: sy, e: vp.x_left as f64, f: vp.y_bottom as f64, ..Affine::IDENTITY }
            }
        }
    }

    /// The four steps world → model → default page → page → device.
    pub fn chain(&self) -> [Affine; 4] {
        [self.model.affine(), self.viewing.affine(), self.default_view.affine(), self.page_to_device()]
    }

    /// The map from coordinate space `from` to `to` (`CVTC_*`):
    /// `PMERR_INV_COORD_SPACE` for an invalid space,
    /// `PMERR_COORDINATE_OVERFLOW` when going back needs the inverse of a
    /// singular transform.
    pub fn convert(&self, from: u32, to: u32) -> Result<Affine, u32> {
        let valid = CVTC_WORLD..=CVTC_DEVICE;
        if !valid.contains(&from) || !valid.contains(&to) { return Err(PMERR_INV_COORD_SPACE); }
        let chain = self.chain();
        let (lo, hi) = (from.min(to) as usize - 1, from.max(to) as usize - 1);
        let forward = chain[lo..hi].iter().fold(Affine::IDENTITY, |acc, m| acc.then(m));
        if from <= to { Ok(forward) } else { forward.inverse().ok_or(PMERR_COORDINATE_OVERFLOW) }
    }

    /// The composed world → device map.
    pub fn world_to_device(&self) -> Xform {
        if self.model == MatrixLf::IDENTITY && self.viewing == MatrixLf::IDENTITY
            && self.default_view == MatrixLf::IDENTITY && self.viewport.is_none()
            && self.ppu() == 1.0
        {
            return Xform::IDENTITY;
        }
        Xform::new(self.chain().iter().fold(Affine::IDENTITY, |acc, m| acc.then(m)))
    }

    /// The device area drawing is limited to by the viewing limits (model
    /// space) and the graphics field (page space), exclusive at the right
    /// and top; `None` when neither is set. A rotated limit is replaced by
    /// its bounding box (Warpine's approximation).
    pub fn device_limits(&self) -> Option<RRect> {
        let mut out: Option<RRect> = None;
        let mut limit = |r: Rectl, from: u32| {
            let Ok(m) = self.convert(from, CVTC_DEVICE) else { return };
            let x = Xform::new(m);
            let (x0, y0, x1, y1) = x.bounds((r.x_left, r.y_bottom), (r.x_right, r.y_top));
            let rect = RRect::new(x0, y0, x1.saturating_add(1), y1.saturating_add(1));
            out = Some(match out {
                None => rect,
                Some(o) => RRect::new(o.x0.max(rect.x0), o.y0.max(rect.y0), o.x1.min(rect.x1).max(o.x0.max(rect.x0)),
                                      o.y1.min(rect.y1).max(o.y0.max(rect.y0))),
            });
        };
        if let Some(r) = self.viewing_limits { limit(r, CVTC_MODEL); }
        if let Some(r) = self.graphics_field { limit(r, CVTC_PAGE); }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: i32 = 65536;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn affine_composition_applies_in_order() {
        let t = Affine::translate(10.0, 0.0);
        let s = Affine::scale(2.0, 3.0);
        // Translate first, then scale: (1, 1) → (11, 1) → (22, 3).
        assert!(close(t.then(&s).apply((1.0, 1.0)), (22.0, 3.0)));
        // Scale first, then translate: (1, 1) → (2, 3) → (12, 3).
        assert!(close(s.then(&t).apply((1.0, 1.0)), (12.0, 3.0)));
        assert!(Affine::IDENTITY.then(&s) == s && s.then(&Affine::IDENTITY) == s);
    }

    #[test]
    fn rotation_is_counter_clockwise_and_exact_at_quarters() {
        let r = Affine::rotate(90.0);
        assert_eq!(r.apply((1.0, 0.0)), (0.0, 1.0));
        assert_eq!(Affine::rotate(180.0).apply((1.0, 2.0)), (-1.0, -2.0));
        assert_eq!(Affine::rotate(-90.0).apply((1.0, 0.0)), (0.0, -1.0));
        assert_eq!(Affine::rotate(450.0), r);
        let r30 = Affine::rotate(30.0).apply((1.0, 0.0));
        assert!(close(r30, (3f64.sqrt() / 2.0, 0.5)));
        // About a centre.
        let rc = Affine::rotate(90.0).about((10.0, 10.0));
        assert!(close(rc.apply((20.0, 10.0)), (10.0, 20.0)));
    }

    #[test]
    fn inverse_undoes_and_singular_has_none() {
        let m = Affine { a: 2.0, b: 1.0, c: -1.0, d: 3.0, e: 5.0, f: -7.0 };
        let inv = m.inverse().unwrap();
        assert!(close(inv.apply(m.apply((3.0, 4.0))), (3.0, 4.0)));
        assert!(close(m.then(&inv).apply((9.0, -2.0)), (9.0, -2.0)));
        assert_eq!(Affine::scale(0.0, 1.0).inverse(), None);
        assert_eq!(Affine { a: f64::NAN, ..Affine::IDENTITY }.inverse(), None);
    }

    #[test]
    fn matrixlf_elements_round_trip_and_validate() {
        let m = MatrixLf::from_elements(&[2 * ONE, 0, 0, 0, ONE / 2, 0, 30, -40, 1]).unwrap();
        assert_eq!(m.elements(), [2 * ONE, 0, 0, 0, ONE / 2, 0, 30, -40, 1]);
        assert_eq!(m.affine().apply((10.0, 10.0)), (50.0, -35.0));
        // Fewer elements: the rest from the identity; none: the identity.
        assert_eq!(MatrixLf::from_elements(&[3 * ONE]).unwrap().elements(), [3 * ONE, 0, 0, 0, ONE, 0, 0, 0, 1]);
        assert_eq!(MatrixLf::from_elements(&[]).unwrap(), MatrixLf::IDENTITY);
        // Elements 3, 6, 9 must be 0, 0, 1.
        assert_eq!(MatrixLf::from_elements(&[ONE, 0, 1]), Err(PMERR_INV_MATRIX_ELEMENT));
        assert_eq!(MatrixLf::from_elements(&[ONE, 0, 0, 0, ONE, 0, 0, 0, 2]), Err(PMERR_INV_MATRIX_ELEMENT));
        assert_eq!(MatrixLf::from_elements(&[0; 10]), Err(PMERR_INV_LENGTH_OR_COUNT));
    }

    #[test]
    fn matrixlf_combine_replace_add_preempt() {
        let t = MatrixLf::from_affine(&Affine::translate(10.0, 0.0)).unwrap();
        let s = Affine::scale(2.0, 2.0);
        // ADD: existing (translate) first, then the new (scale).
        let add = t.combine(&s, TRANSFORM_ADD).unwrap();
        assert_eq!(add.affine().apply((1.0, 1.0)), (22.0, 2.0));
        // PREEMPT: the new first.
        let pre = t.combine(&s, TRANSFORM_PREEMPT).unwrap();
        assert_eq!(pre.affine().apply((1.0, 1.0)), (12.0, 2.0));
        assert_eq!(t.combine(&s, TRANSFORM_REPLACE).unwrap().affine(), s);
        assert_eq!(t.combine(&s, 3), Err(PMERR_INV_TRANSFORM_TYPE));
    }

    #[test]
    fn matrixlf_overflow_is_an_error() {
        let big = MatrixLf::from_affine(&Affine::scale(30000.0, 1.0)).unwrap();
        assert_eq!(big.combine(&Affine::scale(30000.0, 1.0), TRANSFORM_ADD), Err(PMERR_COORDINATE_OVERFLOW));
        assert_eq!(MatrixLf::from_affine(&Affine::translate(1e12, 0.0)), Err(PMERR_COORDINATE_OVERFLOW));
        assert_eq!(MatrixLf::from_affine(&Affine { a: f64::INFINITY, ..Affine::IDENTITY }),
                   Err(PMERR_COORDINATE_OVERFLOW));
        // FIXED rounding: 1/3 is stored to the nearest 1/65536.
        let third = MatrixLf::from_affine(&Affine::scale(1.0 / 3.0, 1.0)).unwrap();
        assert_eq!(third.m11, 21845);
    }

    #[test]
    fn device_coords_round_and_clamp() {
        assert_eq!(device_coord(2.5), 3);
        assert_eq!(device_coord(-2.5), -3);
        assert_eq!(device_coord(1e30), COORD_LIMIT);
        assert_eq!(device_coord(-1e30), -COORD_LIMIT);
        assert_eq!(device_coord(f64::NAN), 0);
        assert_eq!(checked_coord(1e10), None);
        assert_eq!(checked_coord(-12.4), Some(-12));
    }

    #[test]
    fn xform_identity_fast_path_passes_points_through() {
        let x = CoordState::default().world_to_device();
        assert!(x.identity);
        // Even extreme guest values are untouched (the rasterisers clip them).
        assert_eq!(x.pt((i32::MAX, i32::MIN)), (i32::MAX, i32::MIN));
        let scaled = Xform::new(Affine::scale(2.0, 2.0));
        assert_eq!(scaled.pt((i32::MAX, 3)), (COORD_LIMIT, 6), "clamped when transformed");
        assert_eq!(scaled.inverse_pt((10, 7)), Some((5, 4)));
        assert_eq!(Xform::new(Affine::scale(0.0, 1.0)).inverse_pt((1, 1)), None);
        let rot = Xform::new(Affine::rotate(90.0));
        assert!(!rot.axis_aligned() && scaled.axis_aligned());
        assert_eq!(rot.bounds((0, 0), (10, 5)), (-5, 0, 0, 10));
        assert_eq!(scaled.bounds((1, 1), (4, 2)), (2, 2, 8, 4), "corners are points");
        assert_eq!(scaled.inverse_bounds((2, 2), (8, 4)), Some((1, 1, 4, 2)));
    }

    #[test]
    fn page_units_map_to_device_pels() {
        // 96 dpi: 1 inch = 96 pels.
        for (units, per_inch) in [(PU_LOMETRIC, 254.0), (PU_HIMETRIC, 2540.0), (PU_LOENGLISH, 100.0),
                                  (PU_HIENGLISH, 1000.0), (PU_TWIPS, 1440.0), (PU_PELS, 96.0),
                                  (PU_ARBITRARY, 96.0)] {
            let c = CoordState::with_page(units, (0, 0));
            let x = c.world_to_device();
            assert_eq!(x.pt((per_inch as i32, 0)), (96, 0), "units {units:#x}");
            assert_eq!(x.pt((0, 2 * per_inch as i32)), (0, 192), "units {units:#x}");
            assert_eq!(x.identity, units == PU_PELS || units == PU_ARBITRARY);
        }
        // An invalid unit counts as pels.
        assert!(CoordState::with_page(0x00FC, (0, 0)).world_to_device().identity);
    }

    #[test]
    fn default_viewport_and_page_size() {
        let c = CoordState::with_page(PU_LOMETRIC, (1000, 500));
        assert_eq!(c.page_size(), (1000, 500));
        // 100 mm × 50 mm at 96 dpi.
        assert_eq!(c.default_viewport(), Rectl::new(0, 0, 378, 189));
        let pels = CoordState::with_page(PU_PELS, (300, 200));
        assert_eq!(pels.default_viewport(), Rectl::new(0, 0, 300, 200));
        // Size 0: the desktop.
        let (dw, dh) = desktop_size();
        assert_eq!(CoordState::default().page_size(), (dw as i32, dh as i32));
    }

    #[test]
    fn set_page_viewport_scales_the_page() {
        let mut c = CoordState::with_page(PU_PELS, (100, 50));
        c.viewport = Some(Rectl::new(10, 20, 210, 70));
        let x = c.world_to_device();
        assert_eq!(x.pt((0, 0)), (10, 20));
        assert_eq!(x.pt((100, 50)), (210, 70));
        assert_eq!(x.pt((50, 25)), (110, 45));
    }

    #[test]
    fn convert_between_every_pair_of_spaces() {
        let mut c = CoordState::with_page(PU_PELS, (0, 0));
        c.model = MatrixLf::from_affine(&Affine::translate(5.0, 0.0)).unwrap();
        c.viewing = MatrixLf::from_affine(&Affine::scale(2.0, 2.0)).unwrap();
        c.default_view = MatrixLf::from_affine(&Affine::translate(0.0, 7.0)).unwrap();
        c.viewport = Some(Rectl::new(1, 1, 1 + c.page_size().0, 1 + c.page_size().1));
        let p = (3.0, 4.0);
        let w2m = c.convert(CVTC_WORLD, CVTC_MODEL).unwrap().apply(p);
        assert_eq!(w2m, (8.0, 4.0));
        let w2d = c.convert(CVTC_WORLD, CVTC_DEFAULTPAGE).unwrap().apply(p);
        assert_eq!(w2d, (16.0, 8.0));
        assert_eq!(c.convert(CVTC_WORLD, CVTC_PAGE).unwrap().apply(p), (16.0, 15.0));
        assert_eq!(c.convert(CVTC_WORLD, CVTC_DEVICE).unwrap().apply(p), (17.0, 16.0));
        // Backwards is the inverse; the same space is the identity.
        assert!(close(c.convert(CVTC_DEVICE, CVTC_WORLD).unwrap().apply((17.0, 16.0)), p));
        assert_eq!(c.convert(CVTC_PAGE, CVTC_PAGE).unwrap(), Affine::IDENTITY);
        assert_eq!(c.convert(0, CVTC_PAGE), Err(PMERR_INV_COORD_SPACE));
        assert_eq!(c.convert(CVTC_WORLD, 6), Err(PMERR_INV_COORD_SPACE));
        // A singular transform has no way back.
        c.model = MatrixLf::from_affine(&Affine::scale(0.0, 1.0)).unwrap();
        assert_eq!(c.convert(CVTC_DEVICE, CVTC_WORLD), Err(PMERR_COORDINATE_OVERFLOW));
    }

    #[test]
    fn viewing_limits_and_graphics_field_limit_the_device() {
        let mut c = CoordState::default();
        assert_eq!(c.device_limits(), None);
        c.viewing_limits = Some(Rectl::new(10, 10, 49, 29));
        assert_eq!(c.device_limits(), Some(RRect::new(10, 10, 50, 30)));
        c.graphics_field = Some(Rectl::new(20, 0, 99, 99));
        assert_eq!(c.device_limits(), Some(RRect::new(20, 10, 50, 30)));
        // Limits in model space follow the later transforms.
        c.graphics_field = None;
        c.viewing = MatrixLf::from_affine(&Affine::scale(2.0, 2.0)).unwrap();
        assert_eq!(c.device_limits(), Some(RRect::new(20, 20, 99, 59)));
    }
}
