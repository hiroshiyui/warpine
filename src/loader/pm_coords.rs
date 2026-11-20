// SPDX-License-Identifier: GPL-3.0-or-later

//! GPI transform and coordinate-space calls (PMGPI).
//!
//! The matrix maths and the coordinate state live in `pm_xform.rs`; this
//! module reads and writes the guest structures (`MATRIXLF`, `POINTL`,
//! `RECTL`) and keeps `PresentationSpace::coords`:
//!
//! - `GpiSetModelTransformMatrix` / `GpiQueryModelTransformMatrix`,
//!   `GpiSetViewingTransformMatrix` / `GpiQueryViewingTransformMatrix`,
//!   `GpiSetDefaultViewMatrix` / `GpiQueryDefaultViewMatrix` (not inside an
//!   area or path bracket): `lCount` elements (0–9, the rest from the
//!   identity), `TRANSFORM_REPLACE` / `ADD` / `PREEMPT`.
//! - `GpiTranslate`, `GpiScale`, `GpiRotate` update a `MATRIXLF` in guest
//!   memory (the presentation space is only checked); `TRANSFORM_PREEMPT`
//!   is accepted as for the Set calls (unverified: the reference lists
//!   REPLACE and ADD).
//! - `GpiConvert` (`CVTC_*`) and `GpiConvertWithMatrix` convert points in
//!   place; results beyond ±2²⁷ fail with `PMERR_COORDINATE_OVERFLOW` and
//!   leave the points unchanged.
//! - `GpiSetPageViewport` / `GpiQueryPageViewport` (device rectangle,
//!   exclusive at the right and top — unverified), `GpiSetViewingLimits` /
//!   `GpiQueryViewingLimits` (model space, inclusive; the widest limits by
//!   default), `GpiSetGraphicsField` / `GpiQueryGraphicsField` (page space,
//!   inclusive; a NULL rectangle removes it, and the query reports
//!   (0, 0, 0, 0) without one — unverified). Limits and field clip drawing
//!   (`WindowManager::refresh_clip`).
//!
//! Where the transform is applied: each drawing primitive maps its world
//! points to device pels when it is called (`PsTarget::dev`, figures of
//! area and path brackets are stored in device pels); the current position
//! stays in world coordinates.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_rect::Rectl;
use super::pm_xform::{self, Affine, CoordState, MatrixLf, Xform, MATRIX_ELEMENTS};
use super::Loader;

/// Most points one `GpiConvert` / `GpiConvertWithMatrix` call converts
/// (bounds what a call reads from the guest).
const MAX_CONVERT_POINTS: i32 = 8192;
/// Size of a POINTL in guest memory.
const POINTL_SIZE: u32 = 8;
/// Size of one `MATRIXLF` element.
const ELEMENT_SIZE: u32 = 4;

/// Which matrix of a presentation space a Set / Query call is about.
#[derive(Clone, Copy)]
enum Which {
    Model,
    Viewing,
    DefaultView,
}

fn matrix_of(c: &mut CoordState, which: Which) -> &mut MatrixLf {
    match which {
        Which::Model => &mut c.model,
        Which::Viewing => &mut c.viewing,
        Which::DefaultView => &mut c.default_view,
    }
}

impl Loader {
    /// The world → device map of presentation space `hps` (identity for
    /// an unknown one).
    pub(super) fn ps_xform(&self, hps: u32) -> Xform {
        self.shared.window_mgr.lock_or_recover().ps_map.get(&hps)
            .map_or(Xform::IDENTITY, |ps| ps.coords.world_to_device())
    }

    /// The PMGPI transform and coordinate calls by ordinal; `None` for
    /// other ordinals.
    pub(super) fn handle_coords_call(&self, tid: u32, ordinal: u32, arg: &dyn Fn(u64) -> u32) -> Option<u32> {
        Some(match ordinal {
            538 => {
                // GpiSetModelTransformMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray, LONG lOptions) → BOOL
                self.set_matrix(tid, arg(4), arg(8) as i32, arg(12), arg(16), Which::Model)
            }
            468 => {
                // GpiQueryModelTransformMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray) → BOOL
                self.query_matrix(tid, arg(4), arg(8) as i32, arg(12), Which::Model)
            }
            553 => {
                // GpiSetViewingTransformMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray, LONG lOptions) → BOOL
                self.set_matrix(tid, arg(4), arg(8) as i32, arg(12), arg(16), Which::Viewing)
            }
            491 => {
                // GpiQueryViewingTransformMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray) → BOOL
                self.query_matrix(tid, arg(4), arg(8) as i32, arg(12), Which::Viewing)
            }
            520 => {
                // GpiSetDefaultViewMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray, LONG lOptions) → BOOL
                self.set_matrix(tid, arg(4), arg(8) as i32, arg(12), arg(16), Which::DefaultView)
            }
            443 => {
                // GpiQueryDefaultViewMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray) → BOOL
                self.query_matrix(tid, arg(4), arg(8) as i32, arg(12), Which::DefaultView)
            }
            564 => {
                // GpiTranslate(HPS hps, PMATRIXLF pmatlfArray, LONG lOptions, PPOINTL pptlTranslation) → BOOL
                let (hps, pm, opts, pptl) = (arg(4), arg(8), arg(12), arg(16));
                let Some((tx, ty)) = self.read_pointl(pptl) else { return Some(self.coords_fail(tid, PMERR_INV_COORDINATE)) };
                self.update_guest_matrix(tid, hps, pm, opts, Affine::translate(tx as f64, ty as f64))
            }
            565 => {
                // GpiScale(HPS hps, PMATRIXLF pmatlfArray, LONG lOptions, PFIXED afxScale, PPOINTL pptlCenter) → BOOL
                let (hps, pm, opts, pfx, pptl) = (arg(4), arg(8), arg(12), arg(16), arg(20));
                let (Some(sx), Some(sy)) = (self.guest_read::<u32>(pfx), self.guest_read::<u32>(pfx.wrapping_add(4)))
                    else { return Some(self.coords_fail(tid, PMERR_INV_COORDINATE)) };
                let Some((cx, cy)) = self.read_pointl(pptl) else { return Some(self.coords_fail(tid, PMERR_INV_COORDINATE)) };
                let m = Affine::scale(super::pm_shapes::fixed(sx), super::pm_shapes::fixed(sy)).about((cx as f64, cy as f64));
                self.update_guest_matrix(tid, hps, pm, opts, m)
            }
            566 => {
                // GpiRotate(HPS hps, PMATRIXLF pmatlfArray, LONG lOptions, FIXED fxAngle, PPOINTL pptlCenter) → BOOL
                let (hps, pm, opts, angle, pptl) = (arg(4), arg(8), arg(12), arg(16), arg(20));
                let Some((cx, cy)) = self.read_pointl(pptl) else { return Some(self.coords_fail(tid, PMERR_INV_COORDINATE)) };
                let m = Affine::rotate(super::pm_shapes::fixed(angle)).about((cx as f64, cy as f64));
                self.update_guest_matrix(tid, hps, pm, opts, m)
            }
            364 => {
                // GpiConvert(HPS hps, LONG lSrc, LONG lTarg, LONG lCount, PPOINTL aptlPoints) → BOOL
                let (hps, src, targ, count, aptl) = (arg(4), arg(8), arg(12), arg(16) as i32, arg(20));
                let map = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let Some(ps) = wm.ps_map.get(&hps) else { drop(wm); return Some(self.coords_fail(tid, PMERR_INV_HPS)) };
                    ps.coords.convert(src, targ)
                };
                match map {
                    Ok(m) => self.convert_points(tid, count, aptl, &m),
                    Err(e) => self.coords_fail(tid, e),
                }
            }
            618 => {
                // GpiConvertWithMatrix(HPS hps, LONG lCountp, PPOINTL aptlPoints, LONG lCount, PMATRIXLF pmatlfArray) → BOOL
                let (hps, npts, aptl, count, pm) = (arg(4), arg(8) as i32, arg(12), arg(16) as i32, arg(20));
                if !self.check_hps(tid, hps) { return Some(GPI_ERROR); }
                match self.read_matrix(count, pm) {
                    Ok(m) => self.convert_points(tid, npts, aptl, &m.affine()),
                    Err(e) => self.coords_fail(tid, e),
                }
            }
            540 => {
                // GpiSetPageViewport(HPS hps, PRECTL prclViewport) → BOOL
                let (hps, prcl) = (arg(4), arg(8));
                let Some(r) = self.read_rectl(prcl) else { return Some(self.coords_fail(tid, PMERR_INV_PAGE_VIEWPORT)) };
                if !rect_in_range(r) { return Some(self.coords_fail(tid, PMERR_INV_COORDINATE)); }
                if r.x_right <= r.x_left || r.y_top <= r.y_bottom {
                    return Some(self.coords_fail(tid, PMERR_INV_PAGE_VIEWPORT));
                }
                self.update_coords(tid, hps, |c| { c.viewport = Some(r); Ok(()) })
            }
            472 => {
                // GpiQueryPageViewport(HPS hps, PRECTL prclViewport) → BOOL
                self.query_rect(tid, arg(4), arg(8), |c| c.page_viewport())
            }
            552 => {
                // GpiSetViewingLimits(HPS hps, PRECTL prclLimits) → BOOL
                let (hps, prcl) = (arg(4), arg(8));
                let Some(r) = self.read_rectl(prcl) else { return Some(self.coords_fail(tid, PMERR_INV_VIEWING_LIMITS)) };
                if !rect_in_range(r) || r.x_right < r.x_left || r.y_top < r.y_bottom {
                    return Some(self.coords_fail(tid, PMERR_INV_VIEWING_LIMITS));
                }
                let limits = (r != pm_xform::WIDEST_LIMITS).then_some(r);
                self.update_coords(tid, hps, |c| { c.viewing_limits = limits; Ok(()) })
            }
            490 => {
                // GpiQueryViewingLimits(HPS hps, PRECTL prclLimits) → BOOL
                self.query_rect(tid, arg(4), arg(8), |c| c.viewing_limits.unwrap_or(pm_xform::WIDEST_LIMITS))
            }
            526 => {
                // GpiSetGraphicsField(HPS hps, PRECTL prclField) → BOOL
                let (hps, prcl) = (arg(4), arg(8));
                let field = if prcl == 0 { None } else {
                    let Some(r) = self.read_rectl(prcl) else { return Some(self.coords_fail(tid, PMERR_INV_GRAPHICS_FIELD)) };
                    if !rect_in_range(r) || r.x_right < r.x_left || r.y_top < r.y_bottom {
                        return Some(self.coords_fail(tid, PMERR_INV_GRAPHICS_FIELD));
                    }
                    Some(r)
                };
                self.update_coords(tid, hps, |c| { c.graphics_field = field; Ok(()) })
            }
            454 => {
                // GpiQueryGraphicsField(HPS hps, PRECTL prclField) → BOOL
                self.query_rect(tid, arg(4), arg(8), |c| c.graphics_field.unwrap_or_default())
            }
            _ => return None,
        })
    }

    /// Record `code` and return FALSE (`GPI_ERROR`).
    fn coords_fail(&self, tid: u32, code: u32) -> u32 {
        self.pm_error(tid, code);
        GPI_ERROR
    }

    /// Change the coordinate state of `hps` with `f` and refresh its clip
    /// (limits follow the transforms); TRUE, or FALSE with the error.
    fn update_coords(&self, tid: u32, hps: u32, f: impl FnOnce(&mut CoordState) -> Result<(), u32>) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(ps) = wm.ps_map.get_mut(&hps) else { drop(wm); return self.coords_fail(tid, PMERR_INV_HPS) };
        let mut next = ps.coords.clone();
        if let Err(e) = f(&mut next) {
            drop(wm);
            return self.coords_fail(tid, e);
        }
        ps.coords = next;
        wm.refresh_clip(hps);
        1
    }

    fn query_rect(&self, tid: u32, hps: u32, prcl: u32, f: impl FnOnce(&CoordState) -> Rectl) -> u32 {
        let r = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(ps) = wm.ps_map.get(&hps) else { drop(wm); return self.coords_fail(tid, PMERR_INV_HPS) };
            f(&ps.coords)
        };
        if self.write_rectl(prcl, r) { 1 } else { self.coords_fail(tid, PMERR_INV_COORDINATE) }
    }

    /// `count` (0–9) `MATRIXLF` elements at `ptr` as a matrix.
    fn read_matrix(&self, count: i32, ptr: u32) -> Result<MatrixLf, u32> {
        if !(0..=MATRIX_ELEMENTS as i32).contains(&count) { return Err(PMERR_INV_LENGTH_OR_COUNT); }
        let elements = (0..count as u32)
            .map(|i| self.guest_read::<i32>(ptr.wrapping_add(i * ELEMENT_SIZE)))
            .collect::<Option<Vec<i32>>>()
            .ok_or(PMERR_INV_MATRIX_ELEMENT)?;
        MatrixLf::from_elements(&elements)
    }

    /// Write the first `count` elements of `m` at `ptr`.
    fn write_matrix(&self, m: &MatrixLf, count: usize, ptr: u32) -> bool {
        m.elements().iter().take(count).enumerate()
            .all(|(i, &v)| self.guest_write::<i32>(ptr.wrapping_add(i as u32 * ELEMENT_SIZE), v).is_some())
    }

    fn set_matrix(&self, tid: u32, hps: u32, count: i32, ptr: u32, opts: u32, which: Which) -> u32 {
        if !self.check_hps(tid, hps) { return GPI_ERROR; }
        if opts > TRANSFORM_PREEMPT { return self.coords_fail(tid, PMERR_INV_TRANSFORM_TYPE); }
        if let Which::DefaultView = which {
            // "cannot be called inside a path or area bracket".
            let wm = self.shared.window_mgr.lock_or_recover();
            let ps = &wm.ps_map[&hps];
            let err = if ps.area.is_some() { Some(PMERR_INV_IN_AREA) }
                else if matches!(ps.path, Some(super::pm_area::PathState::Open(_))) { Some(PMERR_INV_IN_PATH) }
                else { None };
            drop(wm);
            if let Some(e) = err { return self.coords_fail(tid, e); }
        }
        let new = match self.read_matrix(count, ptr) {
            Ok(m) => m.affine(),
            Err(e) => return self.coords_fail(tid, e),
        };
        self.update_coords(tid, hps, |c| {
            let m = matrix_of(c, which);
            *m = m.combine(&new, opts)?;
            Ok(())
        })
    }

    fn query_matrix(&self, tid: u32, hps: u32, count: i32, ptr: u32, which: Which) -> u32 {
        if !(0..=MATRIX_ELEMENTS as i32).contains(&count) { return self.coords_fail(tid, PMERR_INV_LENGTH_OR_COUNT); }
        let m = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(ps) = wm.ps_map.get_mut(&hps) else { drop(wm); return self.coords_fail(tid, PMERR_INV_HPS) };
            *matrix_of(&mut ps.coords, which)
        };
        if self.write_matrix(&m, count as usize, ptr) { 1 } else { self.coords_fail(tid, PMERR_INV_MATRIX_ELEMENT) }
    }

    /// GpiTranslate / GpiScale / GpiRotate: combine `new` into the full
    /// `MATRIXLF` at `ptr` by `opts` and write it back.
    fn update_guest_matrix(&self, tid: u32, hps: u32, ptr: u32, opts: u32, new: Affine) -> u32 {
        if !self.check_hps(tid, hps) { return GPI_ERROR; }
        if opts > TRANSFORM_PREEMPT { return self.coords_fail(tid, PMERR_INV_TRANSFORM_TYPE); }
        // REPLACE does not read the old matrix.
        let old = if opts == TRANSFORM_REPLACE { Ok(MatrixLf::IDENTITY) } else { self.read_matrix(MATRIX_ELEMENTS as i32, ptr) };
        let result = old.and_then(|old| old.combine(&new, opts));
        match result {
            Ok(m) if self.write_matrix(&m, MATRIX_ELEMENTS, ptr) => 1,
            Ok(_) => self.coords_fail(tid, PMERR_INV_MATRIX_ELEMENT),
            Err(e) => self.coords_fail(tid, e),
        }
    }

    /// Convert `count` POINTLs at `aptl` in place by `m`.
    fn convert_points(&self, tid: u32, count: i32, aptl: u32, m: &Affine) -> u32 {
        if !(0..=MAX_CONVERT_POINTS).contains(&count) { return self.coords_fail(tid, PMERR_INV_LENGTH_OR_COUNT); }
        let Some(pts) = (0..count as u32).map(|i| self.read_pointl(aptl.wrapping_add(i * POINTL_SIZE)))
            .collect::<Option<Vec<_>>>()
        else { return self.coords_fail(tid, PMERR_INV_COORDINATE) };
        let converted: Option<Vec<(i32, i32)>> = pts.iter().map(|&(x, y)| {
            let (cx, cy) = m.apply((x as f64, y as f64));
            Some((pm_xform::checked_coord(cx)?, pm_xform::checked_coord(cy)?))
        }).collect();
        let Some(converted) = converted else { return self.coords_fail(tid, PMERR_COORDINATE_OVERFLOW) };
        for (i, &(x, y)) in converted.iter().enumerate() {
            let at = aptl.wrapping_add(i as u32 * POINTL_SIZE);
            if self.guest_write::<i32>(at, x).is_none() || self.guest_write::<i32>(at.wrapping_add(4), y).is_none() {
                return self.coords_fail(tid, PMERR_INV_COORDINATE);
            }
        }
        1
    }
}

/// Whether every coordinate of `r` is within ±2²⁷.
fn rect_in_range(r: Rectl) -> bool {
    [r.x_left, r.y_bottom, r.x_right, r.y_top].iter().all(|v| v.abs() <= pm_xform::COORD_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::super::constants::*;
    use super::super::pm_region::testing::*;

    // PMGPI ordinals (doc/os2_ordinals.md).
    const CONVERT: u32 = 364;
    const CONVERT_WITH_MATRIX: u32 = 618;
    const SET_MODEL: u32 = 538;
    const QUERY_MODEL: u32 = 468;
    const SET_VIEWING: u32 = 553;
    const QUERY_VIEWING: u32 = 491;
    const SET_DEFAULT_VIEW: u32 = 520;
    const QUERY_DEFAULT_VIEW: u32 = 443;
    const TRANSLATE: u32 = 564;
    const SCALE: u32 = 565;
    const ROTATE: u32 = 566;
    const SET_PAGE_VIEWPORT: u32 = 540;
    const QUERY_PAGE_VIEWPORT: u32 = 472;
    const SET_VIEWING_LIMITS: u32 = 552;
    const QUERY_VIEWING_LIMITS: u32 = 490;
    const SET_GRAPHICS_FIELD: u32 = 526;
    const QUERY_GRAPHICS_FIELD: u32 = 454;
    const BEGIN_AREA: u32 = 352;

    const MAT: u32 = 0x5000;
    const PTS: u32 = 0x5100;
    const RC: u32 = 0x5200;
    const FX: u32 = 0x5300;
    const ONE: i32 = 0x1_0000;

    fn put_matrix(l: &super::Loader, m: &[i32]) {
        for (i, &v) in m.iter().enumerate() { l.guest_write::<i32>(MAT + 4 * i as u32, v).unwrap(); }
    }

    fn get_matrix(l: &super::Loader) -> [i32; 9] {
        std::array::from_fn(|i| l.guest_read::<i32>(MAT + 4 * i as u32).unwrap())
    }

    fn get_points(l: &super::Loader, n: u32) -> Vec<(i32, i32)> {
        (0..n).map(|i| (l.guest_read::<i32>(PTS + 8 * i).unwrap(), l.guest_read::<i32>(PTS + 8 * i + 4).unwrap())).collect()
    }

    #[test]
    fn model_transform_set_query_and_combine() {
        let (l, _, hps) = setup();
        put_matrix(&l, &[0; 9]);
        assert_eq!(gpi(&l, QUERY_MODEL, &[hps, 9, MAT]), 1);
        assert_eq!(get_matrix(&l), [ONE, 0, 0, 0, ONE, 0, 0, 0, 1], "identity by default");
        // Scale ×2 (two elements given), then ADD a translation.
        put_matrix(&l, &[2 * ONE, 0]);
        assert_eq!(gpi(&l, SET_MODEL, &[hps, 1, MAT, TRANSFORM_REPLACE]), 1);
        put_matrix(&l, &[ONE, 0, 0, 0, ONE, 0, 10, 20, 1]);
        assert_eq!(gpi(&l, SET_MODEL, &[hps, 9, MAT, TRANSFORM_ADD]), 1);
        assert_eq!(gpi(&l, QUERY_MODEL, &[hps, 9, MAT]), 1);
        assert_eq!(get_matrix(&l), [2 * ONE, 0, 0, 0, ONE, 0, 10, 20, 1]);
        // PREEMPT the same translation: it is scaled too.
        put_matrix(&l, &[ONE, 0, 0, 0, ONE, 0, 10, 20, 1]);
        assert_eq!(gpi(&l, SET_MODEL, &[hps, 9, MAT, TRANSFORM_PREEMPT]), 1);
        gpi(&l, QUERY_MODEL, &[hps, 9, MAT]);
        assert_eq!(get_matrix(&l)[6..8], [30, 40]);
        // A partial query writes only lCount elements.
        put_matrix(&l, &[-7; 9]);
        assert_eq!(gpi(&l, QUERY_MODEL, &[hps, 2, MAT]), 1);
        assert_eq!(get_matrix(&l)[..3], [2 * ONE, 0, -7]);
        // Errors.
        put_matrix(&l, &[ONE, 0, 5]);
        assert_eq!(gpi(&l, SET_MODEL, &[hps, 3, MAT, TRANSFORM_REPLACE]), 0);
        assert_eq!(last_error(&l), PMERR_INV_MATRIX_ELEMENT);
        assert_eq!(gpi(&l, SET_MODEL, &[hps, 10, MAT, TRANSFORM_REPLACE]), 0);
        assert_eq!(last_error(&l), PMERR_INV_LENGTH_OR_COUNT);
        assert_eq!(gpi(&l, SET_MODEL, &[hps, 0, MAT, 3]), 0);
        assert_eq!(last_error(&l), PMERR_INV_TRANSFORM_TYPE);
        assert_eq!(gpi(&l, SET_MODEL, &[0xBAD, 0, MAT, 0]), 0);
        assert_eq!(last_error(&l), PMERR_INV_HPS);
        assert_eq!(gpi(&l, QUERY_MODEL, &[hps, -1i32 as u32, MAT]), 0);
        // Overflow of the combined FIXED scale.
        put_matrix(&l, &[30000 * ONE, 0]);
        assert_eq!(gpi(&l, SET_MODEL, &[hps, 1, MAT, TRANSFORM_REPLACE]), 1);
        assert_eq!(gpi(&l, SET_MODEL, &[hps, 1, MAT, TRANSFORM_ADD]), 0);
        assert_eq!(last_error(&l), PMERR_COORDINATE_OVERFLOW);
        // lCount 0 replaces with the identity.
        assert_eq!(gpi(&l, SET_MODEL, &[hps, 0, 0, TRANSFORM_REPLACE]), 1);
        gpi(&l, QUERY_MODEL, &[hps, 9, MAT]);
        assert_eq!(get_matrix(&l), [ONE, 0, 0, 0, ONE, 0, 0, 0, 1]);
    }

    #[test]
    fn viewing_and_default_view_matrices() {
        let (l, _, hps) = setup();
        put_matrix(&l, &[ONE, 0, 0, 0, ONE, 0, 3, 4, 1]);
        assert_eq!(gpi(&l, SET_VIEWING, &[hps, 9, MAT, TRANSFORM_REPLACE]), 1);
        put_matrix(&l, &[ONE / 2, 0, 0, 0, ONE / 2, 0, 0, 0, 1]);
        assert_eq!(gpi(&l, SET_DEFAULT_VIEW, &[hps, 9, MAT, TRANSFORM_REPLACE]), 1);
        gpi(&l, QUERY_VIEWING, &[hps, 9, MAT]);
        assert_eq!(get_matrix(&l)[6..8], [3, 4]);
        gpi(&l, QUERY_DEFAULT_VIEW, &[hps, 9, MAT]);
        assert_eq!(get_matrix(&l)[0], ONE / 2);
        // World (10, 20) → model (10, 20) → default page (13, 24) → page (6.5, 12) → device.
        put_points(&l, PTS, &[(10, 20)]);
        assert_eq!(gpi(&l, CONVERT, &[hps, CVTC_WORLD, CVTC_DEVICE, 1, PTS]), 1);
        assert_eq!(get_points(&l, 1), [(7, 12)]);
        // Not inside an area bracket.
        assert_eq!(gpi(&l, BEGIN_AREA, &[hps, 0]), 1);
        assert_eq!(gpi(&l, SET_DEFAULT_VIEW, &[hps, 0, 0, TRANSFORM_REPLACE]), 0);
        assert_eq!(last_error(&l), PMERR_INV_IN_AREA);
    }

    #[test]
    fn translate_scale_rotate_update_a_guest_matrix() {
        let (l, _, hps) = setup();
        put_points(&l, PTS, &[(5, 6)]);
        put_matrix(&l, &[0; 9]); // REPLACE ignores the old contents
        assert_eq!(gpi(&l, TRANSLATE, &[hps, MAT, TRANSFORM_REPLACE, PTS]), 1);
        assert_eq!(get_matrix(&l), [ONE, 0, 0, 0, ONE, 0, 5, 6, 1]);
        // Scale ×3 about (0, 0) after it: the translation is scaled too.
        l.guest_write::<i32>(FX, 3 * ONE).unwrap();
        l.guest_write::<i32>(FX + 4, 3 * ONE).unwrap();
        put_points(&l, PTS + 64, &[(0, 0)]);
        assert_eq!(gpi(&l, SCALE, &[hps, MAT, TRANSFORM_ADD, FX, PTS + 64]), 1);
        assert_eq!(get_matrix(&l), [3 * ONE, 0, 0, 0, 3 * ONE, 0, 15, 18, 1]);
        // Rotate 90° about (10, 0), replacing: (20, 0) → (10, 10).
        put_points(&l, PTS + 64, &[(10, 0)]);
        assert_eq!(gpi(&l, ROTATE, &[hps, MAT, TRANSFORM_REPLACE, (90 << 16) as u32, PTS + 64]), 1);
        assert_eq!(get_matrix(&l), [0, ONE, 0, -ONE, 0, 0, 10, -10, 1]);
        // ADD reads the old matrix, which must be valid.
        put_matrix(&l, &[ONE, 0, 0, 0, ONE, 0, 0, 0, 7]);
        assert_eq!(gpi(&l, TRANSLATE, &[hps, MAT, TRANSFORM_ADD, PTS]), 0);
        assert_eq!(last_error(&l), PMERR_INV_MATRIX_ELEMENT);
        assert_eq!(gpi(&l, TRANSLATE, &[hps, MAT, 9, PTS]), 0);
        assert_eq!(last_error(&l), PMERR_INV_TRANSFORM_TYPE);
        assert_eq!(gpi(&l, TRANSLATE, &[0xBAD, MAT, 0, PTS]), 0);
        // A matrix off the end of guest memory fails without writing.
        assert_eq!(gpi(&l, TRANSLATE, &[hps, u32::MAX - 8, TRANSFORM_ADD, PTS]), 0);
    }

    #[test]
    fn convert_round_trips_and_checks() {
        let (l, _, hps) = setup();
        put_matrix(&l, &[2 * ONE, 0, 0, 0, 2 * ONE, 0, 100, 0, 1]);
        gpi(&l, SET_MODEL, &[hps, 9, MAT, TRANSFORM_REPLACE]);
        put_points(&l, PTS, &[(1, 2), (-3, 4)]);
        assert_eq!(gpi(&l, CONVERT, &[hps, CVTC_WORLD, CVTC_DEVICE, 2, PTS]), 1);
        assert_eq!(get_points(&l, 2), [(102, 4), (94, 8)]);
        assert_eq!(gpi(&l, CONVERT, &[hps, CVTC_DEVICE, CVTC_WORLD, 2, PTS]), 1);
        assert_eq!(get_points(&l, 2), [(1, 2), (-3, 4)]);
        assert_eq!(gpi(&l, CONVERT, &[hps, CVTC_MODEL, CVTC_PAGE, 0, PTS]), 1, "no points");
        assert_eq!(gpi(&l, CONVERT, &[hps, 0, CVTC_PAGE, 1, PTS]), 0);
        assert_eq!(last_error(&l), PMERR_INV_COORD_SPACE);
        assert_eq!(gpi(&l, CONVERT, &[hps, CVTC_WORLD, CVTC_PAGE, -1i32 as u32, PTS]), 0);
        assert_eq!(last_error(&l), PMERR_INV_LENGTH_OR_COUNT);
        // An overflowing result fails and leaves the points alone.
        put_points(&l, PTS, &[(100_000_000, 0)]);
        assert_eq!(gpi(&l, CONVERT, &[hps, CVTC_WORLD, CVTC_DEVICE, 1, PTS]), 0);
        assert_eq!(last_error(&l), PMERR_COORDINATE_OVERFLOW);
        assert_eq!(get_points(&l, 1), [(100_000_000, 0)]);
        // A singular transform has no inverse.
        put_matrix(&l, &[0, 0]);
        gpi(&l, SET_MODEL, &[hps, 2, MAT, TRANSFORM_REPLACE]);
        assert_eq!(gpi(&l, CONVERT, &[hps, CVTC_DEVICE, CVTC_WORLD, 1, PTS]), 0);
        assert_eq!(last_error(&l), PMERR_COORDINATE_OVERFLOW);
        // Points running off guest memory.
        assert_eq!(gpi(&l, CONVERT, &[hps, CVTC_WORLD, CVTC_MODEL, 2, u32::MAX - 8]), 0);
        // GpiConvertWithMatrix uses the given matrix only.
        put_matrix(&l, &[ONE, 0, 0, 0, -ONE, 0, 0, 50, 1]);
        put_points(&l, PTS, &[(7, 10)]);
        assert_eq!(gpi(&l, CONVERT_WITH_MATRIX, &[hps, 1, PTS, 9, MAT]), 1);
        assert_eq!(get_points(&l, 1), [(7, 40)]);
        assert_eq!(gpi(&l, CONVERT_WITH_MATRIX, &[hps, 1, PTS, 12, MAT]), 0);
    }

    #[test]
    fn page_viewport_viewing_limits_and_graphics_field() {
        let (l, _, hps) = setup();
        let (dw, dh) = desktop_size();
        assert_eq!(gpi(&l, QUERY_PAGE_VIEWPORT, &[hps, RC]), 1);
        assert_eq!(get_rect(&l, RC), (0, 0, dw as i32, dh as i32), "a pel page of the desktop's size");
        put_rects(&l, RC, &[(10, 10, 10, 50)]);
        assert_eq!(gpi(&l, SET_PAGE_VIEWPORT, &[hps, RC]), 0, "empty");
        assert_eq!(last_error(&l), PMERR_INV_PAGE_VIEWPORT);
        // Twice the page: everything doubles.
        put_rects(&l, RC, &[(0, 0, 2 * dw as i32, 2 * dh as i32)]);
        assert_eq!(gpi(&l, SET_PAGE_VIEWPORT, &[hps, RC]), 1);
        put_points(&l, PTS, &[(5, 7)]);
        gpi(&l, CONVERT, &[hps, CVTC_PAGE, CVTC_DEVICE, 1, PTS]);
        assert_eq!(get_points(&l, 1), [(10, 14)]);

        assert_eq!(gpi(&l, QUERY_VIEWING_LIMITS, &[hps, RC]), 1);
        let w = super::pm_xform::COORD_LIMIT;
        assert_eq!(get_rect(&l, RC), (-w, -w, w, w), "widest");
        put_rects(&l, RC, &[(5, 5, 1, 9)]);
        assert_eq!(gpi(&l, SET_VIEWING_LIMITS, &[hps, RC]), 0);
        assert_eq!(last_error(&l), PMERR_INV_VIEWING_LIMITS);
        put_rects(&l, RC, &[(1, 2, 3, 4)]);
        assert_eq!(gpi(&l, SET_VIEWING_LIMITS, &[hps, RC]), 1);
        put_rects(&l, RC, &[(0, 0, 0, 0)]);
        gpi(&l, QUERY_VIEWING_LIMITS, &[hps, RC]);
        assert_eq!(get_rect(&l, RC), (1, 2, 3, 4));

        assert_eq!(gpi(&l, QUERY_GRAPHICS_FIELD, &[hps, RC]), 1);
        assert_eq!(get_rect(&l, RC), (0, 0, 0, 0), "none");
        put_rects(&l, RC, &[(0, 0, 9, 9)]);
        assert_eq!(gpi(&l, SET_GRAPHICS_FIELD, &[hps, RC]), 1);
        assert_eq!(gpi(&l, SET_GRAPHICS_FIELD, &[hps, 0]), 1, "NULL removes it");
        gpi(&l, QUERY_GRAPHICS_FIELD, &[hps, RC]);
        assert_eq!(get_rect(&l, RC), (0, 0, 0, 0));
        assert_eq!(gpi(&l, QUERY_PAGE_VIEWPORT, &[0xBAD, RC]), 0);
    }

    // ── Primitives under transforms ─────────────────────────────────────────

    const SET_COLOR: u32 = 517;
    const MOVE: u32 = 404;
    const LINE: u32 = 398;
    const BOX: u32 = 356;
    const SET_PEL: u32 = 544;
    const QUERY_PEL: u32 = 476;
    const FULL_ARC: u32 = 393;
    const POLY_LINE: u32 = 415;
    const CHAR_STRING_AT: u32 = 359;
    const QUERY_POS: u32 = 441;
    const END_AREA: u32 = 385;
    const INTERSECT_CLIP: u32 = 396;
    const QUERY_CLIP_BOX: u32 = 435;
    const POLY_SPLINE: u32 = 417;

    /// A model transform from `[M11, M12, M21, M22]` (as numbers) and a
    /// translation.
    fn set_model(l: &super::Loader, hps: u32, m: [f64; 4], t: (i32, i32)) {
        let fx = |v: f64| (v * ONE as f64).round() as i32;
        put_matrix(l, &[fx(m[0]), fx(m[1]), 0, fx(m[2]), fx(m[3]), 0, t.0, t.1, 1]);
        assert_eq!(gpi(l, SET_MODEL, &[hps, 9, MAT, TRANSFORM_REPLACE]), 1);
    }

    /// The RGB value of device pel (x, y) of the window behind `hwnd`.
    fn device_pel(l: &super::Loader, hwnd: u32, x: i32, y: i32) -> Option<u32> {
        use super::super::mutex_ext::MutexExt;
        let s = l.shared.window_mgr.lock_or_recover().frame_surface(hwnd).unwrap();
        s.lock_or_recover().pel(x, y)
    }

    const RED_RGB: u32 = 0x00FF_0000;

    fn move_to(l: &super::Loader, hps: u32, p: (i32, i32)) {
        put_points(l, PTS, &[p]);
        assert_eq!(gpi(l, MOVE, &[hps, PTS]), 1);
    }

    #[test]
    fn lines_and_pels_are_transformed_and_the_position_stays_in_world() {
        let (l, hwnd, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        set_model(&l, hps, [2.0, 0.0, 0.0, 2.0], (10, 5));
        move_to(&l, hps, (0, 0));
        put_points(&l, PTS, &[(20, 0)]);
        assert_eq!(gpi(&l, LINE, &[hps, PTS]), 1);
        // World (0,0)–(20,0) → device (10,5)–(50,5).
        assert_eq!(device_pel(&l, hwnd, 10, 5), Some(RED_RGB));
        assert_eq!(device_pel(&l, hwnd, 50, 5), Some(RED_RGB));
        assert_ne!(device_pel(&l, hwnd, 51, 5), Some(RED_RGB));
        assert_ne!(device_pel(&l, hwnd, 20, 0), Some(RED_RGB), "not drawn in world pels");
        gpi(&l, QUERY_POS, &[hps, PTS]);
        assert_eq!(get_points(&l, 1), [(20, 0)], "the current position is in world coordinates");
        // GpiSetPel / GpiQueryPel take world points.
        put_points(&l, PTS, &[(30, 30)]);
        assert_eq!(gpi(&l, SET_PEL, &[hps, PTS]), 1);
        assert_eq!(device_pel(&l, hwnd, 70, 65), Some(RED_RGB));
        assert_eq!(gpi(&l, QUERY_PEL, &[hps, PTS]), CLR_RED);
        // A polyline maps every point.
        move_to(&l, hps, (0, 10));
        put_points(&l, PTS, &[(5, 10), (5, 20)]);
        assert_eq!(gpi(&l, POLY_LINE, &[hps, 2, PTS]), 1);
        assert_eq!(device_pel(&l, hwnd, 20, 45), Some(RED_RGB), "on the vertical leg (20, 25..45)");
    }

    #[test]
    fn boxes_scale_and_rotate() {
        let (l, hwnd, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        // Non-uniform scale: the box stays a box.
        set_model(&l, hps, [3.0, 0.0, 0.0, 1.0], (0, 0));
        move_to(&l, hps, (2, 2));
        put_points(&l, PTS, &[(10, 10)]);
        assert_eq!(gpi(&l, BOX, &[hps, DRO_FILL, PTS, 0, 0]), 1);
        assert_eq!(device_pel(&l, hwnd, 6, 2), Some(RED_RGB));
        assert_eq!(device_pel(&l, hwnd, 30, 10), Some(RED_RGB));
        assert_ne!(device_pel(&l, hwnd, 31, 10), Some(RED_RGB));
        assert_ne!(device_pel(&l, hwnd, 30, 11), Some(RED_RGB));
        // Rotated 45° about the origin, moved to (100, 20): a diamond.
        let c = std::f64::consts::FRAC_1_SQRT_2;
        set_model(&l, hps, [c, c, -c, c], (100, 20));
        move_to(&l, hps, (0, 0));
        put_points(&l, PTS, &[(40, 40)]);
        assert_eq!(gpi(&l, BOX, &[hps, DRO_FILL, PTS, 0, 0]), 1);
        // Corners: (100,20) (128,48) (100,77) (72,48).
        assert_eq!(device_pel(&l, hwnd, 100, 48), Some(RED_RGB), "centre");
        assert_eq!(device_pel(&l, hwnd, 100, 22), Some(RED_RGB), "near the bottom corner");
        assert_ne!(device_pel(&l, hwnd, 80, 25), Some(RED_RGB), "outside the diamond, inside its bounding box");
        assert_ne!(device_pel(&l, hwnd, 125, 72), Some(RED_RGB));
    }

    #[test]
    fn arcs_follow_non_uniform_scales() {
        let (l, hwnd, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        set_model(&l, hps, [4.0, 0.0, 0.0, 1.0], (0, 0));
        move_to(&l, hps, (25, 50));
        assert_eq!(gpi(&l, FULL_ARC, &[hps, DRO_OUTLINE, (20 << 16) as u32]), 1);
        // A circle of radius 20 at world (25, 50) → an 80 × 20 ellipse at (100, 50).
        assert_eq!(device_pel(&l, hwnd, 180, 50), Some(RED_RGB), "right end");
        assert_eq!(device_pel(&l, hwnd, 20, 50), Some(RED_RGB), "left end");
        assert_eq!(device_pel(&l, hwnd, 100, 70), Some(RED_RGB), "top");
        assert_ne!(device_pel(&l, hwnd, 100, 50), Some(RED_RGB), "outline only");
    }

    #[test]
    fn text_starts_at_the_mapped_point() {
        use super::super::mutex_ext::MutexExt;
        use crate::gui::DrawOp;
        let (l, hwnd, hps) = setup();
        let surface = l.shared.window_mgr.lock_or_recover().frame_surface(hwnd).unwrap();
        surface.lock().unwrap().record = Some(Vec::new());
        set_model(&l, hps, [2.0, 0.0, 0.0, 2.0], (0, 0));
        l.guest_write::<u8>(PTS + 64, b'A').unwrap();
        put_points(&l, PTS, &[(10, 20)]);
        assert_eq!(gpi(&l, CHAR_STRING_AT, &[hps, PTS, 1, PTS + 64]), 1);
        let ops = surface.lock().unwrap().record.take().unwrap();
        let [DrawOp::TextFont { x, y, .. }] = ops.as_slice() else { panic!("{ops:?}") };
        assert_eq!((*x, *y), (20, 40));
        // The advance (device pels) comes back as world units: half of it.
        let advance = crate::gui::font::text_width("A", crate::gui::font::FontSpec::SYSTEM);
        gpi(&l, QUERY_POS, &[hps, PTS]);
        let (px, py) = get_points(&l, 1)[0];
        assert_eq!(py, 20);
        assert!((px - (10 + advance / 2)).abs() <= 1, "{px} vs {}", 10 + advance / 2);
    }

    #[test]
    fn areas_and_curves_are_built_in_device_pels() {
        let (l, hwnd, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        set_model(&l, hps, [2.0, 0.0, 0.0, 2.0], (0, 0));
        move_to(&l, hps, (5, 5));
        assert_eq!(gpi(&l, BEGIN_AREA, &[hps, 0]), 1);
        put_points(&l, PTS, &[(25, 5), (25, 25)]);
        assert_eq!(gpi(&l, POLY_LINE, &[hps, 2, PTS]), 1);
        assert_eq!(gpi(&l, END_AREA, &[hps]), 1);
        // Triangle (10,10) (50,10) (50,50) in device pels.
        assert_eq!(device_pel(&l, hwnd, 45, 20), Some(RED_RGB));
        assert_ne!(device_pel(&l, hwnd, 20, 45), Some(RED_RGB));
        gpi(&l, QUERY_POS, &[hps, PTS]);
        assert_eq!(get_points(&l, 1), [(5, 5)], "back at the figure's start, in world coordinates");
        // A spline from the current position, flattened in device pels.
        move_to(&l, hps, (0, 40));
        put_points(&l, PTS, &[(10, 40), (20, 40), (30, 40)]);
        assert_eq!(gpi(&l, POLY_SPLINE, &[hps, 3, PTS]), 1);
        assert_eq!(device_pel(&l, hwnd, 60, 80), Some(RED_RGB));
        assert_eq!(device_pel(&l, hwnd, 30, 80), Some(RED_RGB));
        gpi(&l, QUERY_POS, &[hps, PTS]);
        assert_eq!(get_points(&l, 1), [(30, 40)]);
    }

    #[test]
    fn metric_page_units_draw_at_physical_size() {
        use super::super::mutex_ext::MutexExt;
        let (l, hwnd, _) = setup();
        // A PS in PU_LOENGLISH (0.01 inch): one inch is 96 pels.
        let hps = {
            let mut wm = l.shared.window_mgr.lock_or_recover();
            let hps = wm.create_ps(hwnd);
            wm.ps_map.get_mut(&hps).unwrap().coords = super::CoordState::with_page(PU_LOENGLISH, (0, 0));
            hps
        };
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        move_to(&l, hps, (0, 50));
        put_points(&l, PTS, &[(100, 50)]);
        assert_eq!(gpi(&l, LINE, &[hps, PTS]), 1);
        assert_eq!(device_pel(&l, hwnd, 96, 48), Some(RED_RGB), "one inch along, half an inch up");
        assert_ne!(device_pel(&l, hwnd, 98, 48), Some(RED_RGB));
        put_points(&l, PTS, &[(100, 100)]);
        assert_eq!(gpi(&l, CONVERT, &[hps, CVTC_WORLD, CVTC_DEVICE, 1, PTS]), 1);
        assert_eq!(get_points(&l, 1), [(96, 96)]);
    }

    #[test]
    fn clip_rectangles_and_clip_box_are_in_world_coordinates() {
        let (l, hwnd, hps) = setup();
        set_model(&l, hps, [2.0, 0.0, 0.0, 2.0], (0, 0));
        put_rects(&l, RC, &[(5, 5, 14, 9)]);
        assert_ne!(gpi(&l, INTERSECT_CLIP, &[hps, RC]), 0);
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        move_to(&l, hps, (0, 0));
        put_points(&l, PTS, &[(49, 49)]);
        gpi(&l, BOX, &[hps, DRO_FILL, PTS, 0, 0]);
        // World 5..=14 × 5..=9 → device 10..=28 × 10..=18 (corners are points).
        assert_eq!(device_pel(&l, hwnd, 10, 10), Some(RED_RGB));
        assert_eq!(device_pel(&l, hwnd, 28, 18), Some(RED_RGB));
        assert_ne!(device_pel(&l, hwnd, 29, 18), Some(RED_RGB));
        assert_ne!(device_pel(&l, hwnd, 28, 19), Some(RED_RGB));
        assert_ne!(device_pel(&l, hwnd, 9, 10), Some(RED_RGB));
        put_rects(&l, RC, &[(0, 0, 0, 0)]);
        gpi(&l, QUERY_CLIP_BOX, &[hps, RC]);
        assert_eq!(get_rect(&l, RC), (5, 5, 14, 9));
    }

    /// The shape generators mapped by the identity give the plain shapes.
    #[test]
    fn mapped_arcs_with_the_identity_equal_the_plain_ones() {
        use super::super::pm_shapes::*;
        let id = super::Affine::IDENTITY;
        let p = ArcParams { p: 3, q: 2, r: 1, s: 0 };
        assert_eq!(full_arc_mapped(&id, (10, 20), p, 4.5), full_arc((10, 20), p, 4.5));
        assert_eq!(partial_arc_mapped(&id, (10, 20), p, 4.5, 30.0, 200.0), partial_arc((10, 20), p, 4.5, 30.0, 200.0));
        assert_eq!(point_arc_mapped(&id, (0, 0), (10, 10), (20, 0), p), point_arc((0, 0), (10, 10), (20, 0), p));
        // Scaled ×10, the arc is generated in device pels: dense and smooth.
        let s = super::Affine::scale(10.0, 10.0);
        let big = full_arc_mapped(&s, (0, 0), ArcParams::default(), 5.0);
        assert!(big.len() > full_arc((0, 0), ArcParams::default(), 5.0).len() * 5);
        assert!(big.iter().all(|&(x, y)| (((x * x + y * y) as f64).sqrt() - 50.0).abs() <= 1.0));
    }

    /// Viewing limits clip what is drawn.
    #[test]
    fn viewing_limits_clip_drawing() {
        let (l, _, hps) = setup();
        put_rects(&l, RC, &[(10, 10, 19, 19)]);
        assert_eq!(gpi(&l, SET_VIEWING_LIMITS, &[hps, RC]), 1);
        gpi(&l, 517, &[hps, CLR_RED]); // GpiSetColor
        put_points(&l, PTS, &[(0, 0)]);
        gpi(&l, 404, &[hps, PTS]); // GpiMove
        put_points(&l, PTS, &[(50, 50)]);
        gpi(&l, 356, &[hps, DRO_FILL, PTS, 0, 0]); // GpiBox
        let pel = |x, y| { put_points(&l, PTS, &[(x, y)]); gpi(&l, 476, &[hps, PTS]) };
        assert_eq!(pel(15, 15), CLR_RED);
        assert_eq!(pel(19, 19), CLR_RED, "the limits are inclusive");
        assert_eq!(pel(20, 15), 0);
        assert_eq!(pel(5, 5), 0);
    }
}
