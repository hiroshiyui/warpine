// SPDX-License-Identifier: GPL-3.0-or-later

//! GPI regions and clipping (PMGPI).
//!
//! A region (`HRGN`) is a [`Region`] (`gui/region.rs`: y-bands of x spans,
//! PM coordinates, exclusive at the right and top) kept in
//! [`RegionTable`] (`WindowManager::regions`). Regions are in device
//! coordinates, the pels of the surface a presentation space draws into
//! (`GpiCreateRegion`, `GpiPtInRegion`, `GpiPaintRegion` …). The clip
//! rectangle calls (`GpiIntersectClipRectangle`,
//! `GpiExcludeClipRectangle`), `GpiOffsetClipRegion` and `GpiQueryClipBox`
//! take or report world coordinates, mapped through the PS's transforms
//! (`pm_xform.rs`; a rotated rectangle becomes its upright bounding box,
//! Warpine's approximation). The viewing limits and the graphics field
//! clip too (`CoordState::device_limits`).
//!
//! - `GpiCreateRegion` / `GpiSetRegion` take the union of `RECTL`s (left
//!   and bottom edges in, right and top out); `GpiCombineRegion`
//!   (`CRGN_OR` / `COPY` / `XOR` / `AND` / `DIFF`; the destination may be a
//!   source) and the queries (`GpiQueryRegionBox`, `GpiQueryRegionRects`
//!   with `RGNRECT` paging and the four `RECTDIR_*` orders,
//!   `GpiPtInRegion`, `GpiRectInRegion`, `GpiEqualRegion`) report
//!   `RGN_NULL` / `RGN_RECT` / `RGN_COMPLEX` (one rectangle is `RGN_RECT`).
//! - `GpiPaintRegion` fills the region with the area pattern, both its
//!   colours combined by the foreground mix ("mixing is controlled by the
//!   area foreground mix only"); `GpiFrameRegion` paints the pels less than
//!   `cx` / `cy` inside the region's boundary.
//! - The clip region: `GpiSetClipRegion` selects a region (the old one's
//!   handle is returned and is the application's again); while selected,
//!   only the clip calls may change it and every other region call fails
//!   with `PMERR_REGION_IS_CLIP_REGION` (another PS selecting it:
//!   `PMERR_HRGN_BUSY`). `GpiIntersectClipRectangle` /
//!   `GpiExcludeClipRectangle` take an inclusive rectangle ("the boundaries
//!   are part of the interior") and create a clip region when there is
//!   none; `GpiOffsetClipRegion` moves it; `GpiQueryClipBox` reports the
//!   inclusive bounds of everything clipping the PS (clip region, clip path
//!   and the surface), `right < left` and `top < bottom` when nothing is
//!   left. The clip region of a PS is destroyed with it.
//! - Drawing through a PS with a clip region or clip path
//!   (`PresentationSpace::clip`) passes it to `Surface::draw_clipped`.
//!
//! Bounds against the guest: at most `MAX_REGION_RECTS` rectangles per
//! region (a result that would need more fails with
//! `PMERR_INSUFFICIENT_MEMORY`), `MAX_REGIONS` regions, coordinates within
//! ±2²⁷ (`PMERR_INV_COORDINATE` beyond).

use std::collections::HashMap;
use std::sync::Arc;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_rect::{Rectl, EMPTY_RECTL};
use super::pm_types::{DrawTarget, WindowManager};
use super::{ApiResult, Loader};
use crate::gui::region::{
    Complexity, Overlap, RRect, RectOrder, Region, SetOp, MAX_REGION_RECTS, REGION_COORD_LIMIT,
};
use crate::gui::surface::AreaFill;
use crate::gui::DrawOp;

/// Most regions a process may hold at once.
pub const MAX_REGIONS: usize = 16384;

/// First region handle (handles are never reused before they wrap).
const HRGN_FIRST: u32 = 0x0001_0000;

/// The regions of the process, by handle.
#[derive(Default)]
pub struct RegionTable {
    map: HashMap<u32, Arc<Region>>,
    next: u32,
}

impl RegionTable {
    /// Register `region`; `None` when the table is full.
    pub fn create(&mut self, region: Region) -> Option<u32> {
        if self.map.len() >= MAX_REGIONS { return None; }
        loop {
            self.next = self.next.wrapping_add(1).max(HRGN_FIRST);
            if self.next != HRGN_ERROR && !self.map.contains_key(&self.next) { break; }
        }
        self.map.insert(self.next, Arc::new(region));
        Some(self.next)
    }

    pub fn get(&self, h: u32) -> Option<&Arc<Region>> {
        self.map.get(&h)
    }

    pub fn set(&mut self, h: u32, region: Region) {
        if let Some(r) = self.map.get_mut(&h) { *r = Arc::new(region); }
    }

    pub fn remove(&mut self, h: u32) -> bool {
        self.map.remove(&h).is_some()
    }

}

/// `RGN_*` of a complexity.
pub fn rgn_value(c: Complexity) -> u32 {
    match c {
        Complexity::Null => RGN_NULL,
        Complexity::Rect => RGN_RECT,
        Complexity::Complex => RGN_COMPLEX,
    }
}

fn set_op(mode: u32) -> Option<SetOp> {
    Some(match mode {
        CRGN_OR => SetOp::Or,
        CRGN_COPY => SetOp::Copy,
        CRGN_XOR => SetOp::Xor,
        CRGN_AND => SetOp::And,
        CRGN_DIFF => SetOp::Diff,
        _ => return None,
    })
}

fn in_range(v: i32) -> bool {
    (-REGION_COORD_LIMIT..=REGION_COORD_LIMIT).contains(&v)
}

/// A region-call `RECTL` (exclusive at the right and top) as a rectangle;
/// `PMERR_INV_RECT` when inverted, `PMERR_INV_COORDINATE` out of range.
fn region_rect(r: Rectl) -> Result<RRect, u32> {
    if ![r.x_left, r.y_bottom, r.x_right, r.y_top].into_iter().all(in_range) { return Err(PMERR_INV_COORDINATE); }
    if r.x_right < r.x_left || r.y_top < r.y_bottom { return Err(PMERR_INV_RECT); }
    Ok(RRect::new(r.x_left, r.y_bottom, r.x_right, r.y_top))
}

/// An inclusive (clip) `RECTL` as a rectangle exclusive at the right and top.
fn inclusive_rect(r: Rectl) -> Result<RRect, u32> {
    if ![r.x_left, r.y_bottom, r.x_right, r.y_top].into_iter().all(in_range) { return Err(PMERR_INV_COORDINATE); }
    if r.x_right < r.x_left || r.y_top < r.y_bottom { return Err(PMERR_INV_RECT); }
    Ok(RRect::new(r.x_left, r.y_bottom, r.x_right + 1, r.y_top + 1))
}

fn to_rectl(r: RRect) -> Rectl {
    Rectl::new(r.x0, r.y0, r.x1, r.y1)
}

impl WindowManager {
    /// Destroy presentation space `hps` and the clip region it holds.
    pub fn remove_ps(&mut self, hps: u32) {
        if let Some(ps) = self.ps_map.remove(&hps) && ps.clip_region != 0 {
            self.regions.remove(ps.clip_region);
        }
    }

    /// The PS that has `hrgn` as its clip region, if any.
    pub fn clip_owner(&self, hrgn: u32) -> Option<u32> {
        self.ps_map.values().find(|ps| ps.clip_region == hrgn).map(|ps| ps.hps)
    }

    /// Recompute what drawing through `hps` is clipped to: its clip region
    /// and its clip path together.
    pub fn refresh_clip(&mut self, hps: u32) {
        let Some(ps) = self.ps_map.get(&hps) else { return };
        let region = (ps.clip_region != 0).then(|| self.regions.get(ps.clip_region).cloned()).flatten();
        let clip = match (region, ps.clip_path.clone()) {
            (None, None) => None,
            (Some(r), None) | (None, Some(r)) => Some(r),
            (Some(a), Some(b)) => Some(Region::combine(&a, &b, SetOp::And).map(Arc::new).unwrap_or(a)),
        };
        // Viewing limits and graphics field (pm_xform.rs), and the update
        // region of a WinBeginPaint PS.
        let mut clip = clip;
        let limits = [ps.coords.device_limits().map(|r| Arc::new(Region::rect(r))), ps.paint_clip.clone()];
        for limit in limits {
            clip = match (clip, limit) {
                (clip, None) => clip,
                (None, Some(r)) => Some(r),
                (Some(c), Some(r)) => Some(Region::combine(&c, &r, SetOp::And).map(Arc::new).unwrap_or(c)),
            };
        }
        if let Some(ps) = self.ps_map.get_mut(&hps) { ps.clip = clip; }
    }

    /// Everything clipping `hps`: its clip and the extent of its surface.
    pub fn effective_clip(&mut self, hps: u32) -> Region {
        let Some(ps) = self.ps_map.get(&hps) else { return Region::empty() };
        let clip = ps.clip.clone();
        let target = self.ps_draw_target(hps).filter(|t| *t != DrawTarget::None);
        // A client's extent, a child's part inside its ancestors, else its
        // surface's.
        let visible = match target.and_then(|t| self.target_visible(t)) {
            Some(v) => Region::rect(v),
            None => target.and_then(|t| self.target_surface(t))
                .map(|s| { let s = s.lock_or_recover(); Region::rect(RRect::new(0, 0, s.width as i32, s.height as i32)) })
                .unwrap_or_else(Region::infinite),
        };
        match clip {
            Some(c) => Region::combine(&c, &visible, SetOp::And).unwrap_or_else(|| (*c).clone()),
            None => visible,
        }
    }
}

impl Loader {
    /// The PMGPI region and clip calls by ordinal; `None` for other ordinals.
    pub(super) fn handle_region_call(&self, tid: u32, ordinal: u32, arg: &dyn Fn(u64) -> u32) -> Option<u32> {
        Some(match ordinal {
            370 => {
                // GpiCreateRegion(HPS hps, LONG lCount, PRECTL arclRectangles) → HRGN
                self.gpi_create_region(tid, arg(4), arg(8) as i32, arg(12))
            }
            611 => {
                // GpiDestroyRegion(HPS hps, HRGN hrgn) → BOOL
                self.gpi_destroy_region(tid, arg(4), arg(8))
            }
            546 => {
                // GpiSetRegion(HPS hps, HRGN hrgn, LONG lcount, PRECTL arclRectangles) → BOOL
                self.gpi_set_region(tid, arg(4), arg(8), arg(12) as i32, arg(16))
            }
            362 => {
                // GpiCombineRegion(HPS hps, HRGN hrgnDest, HRGN hrgnSrc1, HRGN hrgnSrc2, LONG lMode) → LONG
                self.gpi_combine_region(tid, arg(4), arg(8), arg(12), arg(16), arg(20))
            }
            481 => {
                // GpiQueryRegionBox(HPS hps, HRGN hrgn, PRECTL prclBound) → LONG
                self.gpi_query_region_box(tid, arg(4), arg(8), arg(12))
            }
            587 => {
                // GpiQueryRegionRects(HPS hps, HRGN hrgn, PRECTL prclBound, PRGNRECT prgnrcControl, PRECTL prclRect) → BOOL
                self.gpi_query_region_rects(tid, arg(4), arg(8), arg(12), arg(16), arg(20))
            }
            419 => {
                // GpiPtInRegion(HPS hps, HRGN hrgn, PPOINTL pptlPoint) → LONG
                self.gpi_pt_in_region(tid, arg(4), arg(8), arg(12))
            }
            494 => {
                // GpiRectInRegion(HPS hps, HRGN hrgn, PRECTL prclRect) → LONG
                self.gpi_rect_in_region(tid, arg(4), arg(8), arg(12))
            }
            407 => {
                // GpiOffsetRegion(HPS hps, HRGN hrgn, PPOINTL pptlOffset) → BOOL
                self.gpi_offset_region(tid, arg(4), arg(8), arg(12))
            }
            388 => {
                // GpiEqualRegion(HPS hps, HRGN hrgnSrc1, HRGN hrgnSrc2) → LONG
                self.gpi_equal_region(tid, arg(4), arg(8), arg(12))
            }
            409 => {
                // GpiPaintRegion(HPS hps, HRGN hrgn) → LONG
                self.gpi_paint_region(tid, arg(4), arg(8), None)
            }
            617 => {
                // GpiFrameRegion(HPS hps, HRGN hrgn, PSIZEL thickness) → LONG
                self.gpi_paint_region(tid, arg(4), arg(8), Some(arg(12)))
            }
            516 => {
                // GpiSetClipRegion(HPS hps, HRGN hrgn, PHRGN phrgnOld) → LONG
                self.gpi_set_clip_region(tid, arg(4), arg(8), arg(12))
            }
            436 => {
                // GpiQueryClipRegion(HPS hps) → HRGN
                let wm = self.shared.window_mgr.lock_or_recover();
                match wm.ps_map.get(&arg(4)) {
                    Some(ps) => ps.clip_region,
                    None => { drop(wm); self.pm_error(tid, PMERR_INV_HPS); HRGN_ERROR }
                }
            }
            396 => {
                // GpiIntersectClipRectangle(HPS hps, PRECTL prclRectangle) → LONG
                self.gpi_clip_rectangle(tid, arg(4), arg(8), SetOp::And)
            }
            391 => {
                // GpiExcludeClipRectangle(HPS hps, PRECTL prclRectangle) → LONG
                self.gpi_clip_rectangle(tid, arg(4), arg(8), SetOp::Diff)
            }
            405 => {
                // GpiOffsetClipRegion(HPS hps, PPOINTL pptlPoint) → LONG
                self.gpi_offset_clip_region(tid, arg(4), arg(8))
            }
            435 => {
                // GpiQueryClipBox(HPS hps, PRECTL prclBound) → LONG
                self.gpi_query_clip_box(tid, arg(4), arg(8))
            }
            _ => return None,
        })
    }

    /// Whether `hps` is a presentation space; if not, `PMERR_INV_HPS`.
    pub(super) fn check_hps(&self, tid: u32, hps: u32) -> bool {
        let ok = self.shared.window_mgr.lock_or_recover().ps_map.contains_key(&hps);
        if !ok { self.pm_error(tid, PMERR_INV_HPS); }
        ok
    }

    /// Region `hrgn` for a region call through `hps`: `PMERR_INV_HPS`,
    /// `PMERR_INV_HRGN` or (selected as a clip region)
    /// `PMERR_REGION_IS_CLIP_REGION` when it cannot be used.
    fn usable_region(&self, tid: u32, hps: u32, hrgn: u32) -> Option<Arc<Region>> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let err = if !wm.ps_map.contains_key(&hps) {
            PMERR_INV_HPS
        } else if let Some(r) = wm.regions.get(hrgn) {
            if wm.clip_owner(hrgn).is_none() { return Some(r.clone()); }
            PMERR_REGION_IS_CLIP_REGION
        } else {
            PMERR_INV_HRGN
        };
        drop(wm);
        self.pm_error(tid, err);
        None
    }

    /// `count` RECTLs at `ptr` as one region.
    fn read_region(&self, count: i32, ptr: u32) -> Result<Region, u32> {
        if !(0..=MAX_REGION_RECTS as i32).contains(&count) { return Err(PMERR_INV_LENGTH_OR_COUNT); }
        let rects = (0..count as u32)
            .map(|i| self.read_rectl(ptr.wrapping_add(i * super::pm_rect::RECTL_SIZE))
                .ok_or(PMERR_INV_COORDINATE).and_then(region_rect))
            .collect::<Result<Vec<_>, u32>>()?;
        Region::from_rects(&rects).ok_or(PMERR_INSUFFICIENT_MEMORY)
    }

    fn gpi_create_region(&self, tid: u32, hps: u32, count: i32, ptr: u32) -> u32 {
        if !self.check_hps(tid, hps) { return RGN_ERROR; }
        let region = match self.read_region(count, ptr) {
            Ok(r) => r,
            Err(e) => { self.pm_error(tid, e); return RGN_ERROR; }
        };
        match self.shared.window_mgr.lock_or_recover().regions.create(region) {
            Some(h) => h,
            None => { self.pm_error(tid, PMERR_INSUFFICIENT_MEMORY); RGN_ERROR }
        }
    }

    fn gpi_destroy_region(&self, tid: u32, hps: u32, hrgn: u32) -> u32 {
        if !self.check_hps(tid, hps) { return 0; }
        if hrgn == 0 { return 1; }
        if self.usable_region(tid, hps, hrgn).is_none() { return 0; }
        self.shared.window_mgr.lock_or_recover().regions.remove(hrgn);
        1
    }

    fn gpi_set_region(&self, tid: u32, hps: u32, hrgn: u32, count: i32, ptr: u32) -> u32 {
        if self.usable_region(tid, hps, hrgn).is_none() { return 0; }
        match self.read_region(count, ptr) {
            Ok(r) => { self.shared.window_mgr.lock_or_recover().regions.set(hrgn, r); 1 }
            Err(e) => { self.pm_error(tid, e); 0 }
        }
    }

    fn gpi_combine_region(&self, tid: u32, hps: u32, dest: u32, src1: u32, src2: u32, mode: u32) -> u32 {
        let Some(op) = set_op(mode) else {
            if self.check_hps(tid, hps) { self.pm_error(tid, PMERR_INV_REGION_MIX_MODE); }
            return RGN_ERROR;
        };
        if self.usable_region(tid, hps, dest).is_none() { return RGN_ERROR; }
        let Some(a) = self.usable_region(tid, hps, src1) else { return RGN_ERROR };
        // CRGN_COPY ignores the second source.
        let b = if op == SetOp::Copy { Arc::new(Region::empty()) } else {
            let Some(b) = self.usable_region(tid, hps, src2) else { return RGN_ERROR };
            b
        };
        let Some(out) = Region::combine(&a, &b, op) else {
            self.pm_error(tid, PMERR_INSUFFICIENT_MEMORY);
            return RGN_ERROR;
        };
        let c = rgn_value(out.complexity());
        self.shared.window_mgr.lock_or_recover().regions.set(dest, out);
        c
    }

    fn gpi_query_region_box(&self, tid: u32, hps: u32, hrgn: u32, prcl: u32) -> u32 {
        let Some(r) = self.usable_region(tid, hps, hrgn) else { return RGN_ERROR };
        // A null region: left = right and bottom = top.
        let b = r.bounds().map_or(EMPTY_RECTL, to_rectl);
        if !self.write_rectl(prcl, b) { self.pm_error(tid, PMERR_INV_COORDINATE); return RGN_ERROR; }
        rgn_value(r.complexity())
    }

    /// GpiQueryRegionRects: the region's rectangles (each cut to
    /// `prclBound` when given) from number `ircStart` (1-based) on, at most
    /// `crc` of them, in `ulDirection` order; `crcReturned` tells how many
    /// were written. With no `prclRect` only the count is returned.
    fn gpi_query_region_rects(&self, tid: u32, hps: u32, hrgn: u32, pbound: u32, pctl: u32, prects: u32) -> u32 {
        let Some(r) = self.usable_region(tid, hps, hrgn) else { return 0 };
        let read = |i: u32| self.guest_read::<u32>(pctl.wrapping_add(4 * i));
        let (Some(start), Some(crc), Some(dir)) = (read(0), read(1), read(3)) else {
            return self.fail_with(tid, PMERR_INV_REGION_CONTROL);
        };
        let order = match dir {
            RECTDIR_LFRT_TOPBOT => RectOrder::LeftRightTopBottom,
            RECTDIR_RTLF_TOPBOT => RectOrder::RightLeftTopBottom,
            RECTDIR_LFRT_BOTTOP => RectOrder::LeftRightBottomTop,
            RECTDIR_RTLF_BOTTOP => RectOrder::RightLeftBottomTop,
            _ => return self.fail_with(tid, PMERR_INV_REGION_CONTROL),
        };
        if start == 0 { return self.fail_with(tid, PMERR_INV_REGION_CONTROL); }
        let bound = if pbound == 0 { None } else {
            match self.read_rectl(pbound).ok_or(PMERR_INV_COORDINATE).and_then(region_rect) {
                Ok(b) => Some(Region::rect(b)),
                Err(e) => return self.fail_with(tid, e),
            }
        };
        let region = match bound {
            Some(b) => Region::combine(&r, &b, SetOp::And).unwrap_or_default(),
            None => (*r).clone(),
        };
        let rects = region.rects(order);
        let rest = rects.get(start as usize - 1..).unwrap_or(&[]);
        let n = if prects == 0 { rest.len() } else { rest.len().min(crc as usize) };
        if prects != 0 {
            for (i, rc) in rest[..n].iter().enumerate() {
                let at = prects.wrapping_add(i as u32 * super::pm_rect::RECTL_SIZE);
                if !self.write_rectl(at, to_rectl(*rc)) { return self.fail_with(tid, PMERR_INV_COORDINATE); }
            }
        }
        if self.guest_write::<u32>(pctl.wrapping_add(8), n as u32).is_none() {
            return self.fail_with(tid, PMERR_INV_REGION_CONTROL);
        }
        1
    }

    fn fail_with(&self, tid: u32, code: u32) -> u32 {
        self.pm_error(tid, code);
        0
    }

    fn gpi_pt_in_region(&self, tid: u32, hps: u32, hrgn: u32, pptl: u32) -> u32 {
        let Some(r) = self.usable_region(tid, hps, hrgn) else { return PRGN_ERROR };
        let Some((x, y)) = self.read_pointl(pptl) else { return self.fail_with(tid, PMERR_INV_COORDINATE) };
        if r.contains(x, y) { PRGN_INSIDE } else { PRGN_OUTSIDE }
    }

    fn gpi_rect_in_region(&self, tid: u32, hps: u32, hrgn: u32, prcl: u32) -> u32 {
        let Some(r) = self.usable_region(tid, hps, hrgn) else { return RRGN_ERROR };
        let rect = match self.read_rectl(prcl).ok_or(PMERR_INV_COORDINATE).and_then(region_rect) {
            Ok(rc) => rc,
            Err(e) => return self.fail_with(tid, e),
        };
        match r.overlap(rect) {
            Overlap::Outside => RRGN_OUTSIDE,
            Overlap::Partial => RRGN_PARTIAL,
            Overlap::Inside => RRGN_INSIDE,
        }
    }

    fn gpi_offset_region(&self, tid: u32, hps: u32, hrgn: u32, pptl: u32) -> u32 {
        let Some(r) = self.usable_region(tid, hps, hrgn) else { return 0 };
        let Some((dx, dy)) = self.read_pointl(pptl) else { return self.fail_with(tid, PMERR_INV_COORDINATE) };
        let mut moved = (*r).clone();
        moved.offset(dx, dy);
        self.shared.window_mgr.lock_or_recover().regions.set(hrgn, moved);
        1
    }

    fn gpi_equal_region(&self, tid: u32, hps: u32, a: u32, b: u32) -> u32 {
        let Some(ra) = self.usable_region(tid, hps, a) else { return EQRGN_ERROR };
        let Some(rb) = self.usable_region(tid, hps, b) else { return EQRGN_ERROR };
        if ra == rb { EQRGN_EQUAL } else { EQRGN_NOTEQUAL }
    }

    /// GpiPaintRegion (`thickness` = 0) / GpiFrameRegion (`thickness` a
    /// PSIZEL): paint the region, or its frame, with the area pattern.
    fn gpi_paint_region(&self, tid: u32, hps: u32, hrgn: u32, thickness: Option<u32>) -> u32 {
        let Some(r) = self.usable_region(tid, hps, hrgn) else { return GPI_ERROR };
        let region = match thickness {
            None => (*r).clone(),
            Some(p) => {
                let Some((cx, cy)) = self.read_pointl(p) else { return self.fail_with(tid, PMERR_INV_COORDINATE) };
                if cx < 0 || cy < 0 { return self.fail_with(tid, PMERR_INV_COORDINATE); }
                match r.frame(cx, cy) {
                    Some(f) => f,
                    None => return self.fail_with(tid, PMERR_INSUFFICIENT_MEMORY),
                }
            }
        };
        let Some(t) = self.ps_target(hps) else { return GPI_ERROR };
        if self.in_bracket(hps) { return GPI_OK; }
        let fill = AreaFill { back_mix: Some(t.mix), ..t.fill() };
        self.paint_region(&t, &region, fill);
        GPI_OK
    }

    /// Fill every pel of `region` with `fill`, through the PS's clip.
    pub(super) fn paint_region(&self, t: &super::pm_gpi::PsTarget, region: &Region, fill: AreaFill) {
        for rc in region.rects(RectOrder::LeftRightBottomTop) {
            let (x1, y1, x2, y2) = (rc.x0, rc.y0, rc.x1 - 1, rc.y1 - 1);
            let op = if fill.pattern.is_solid() {
                DrawOp::Box { x1, y1, x2, y2, color: fill.pattern.fg, fill: true, mix: fill.mix }
            } else {
                DrawOp::Shape { points: vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)], closed: true,
                                fill: Some(fill), stroke: None }
            };
            self.draw_ps(t, op);
        }
    }

    fn gpi_set_clip_region(&self, tid: u32, hps: u32, hrgn: u32, pold: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let err = match wm.ps_map.get(&hps) {
            None => Some(PMERR_INV_HPS),
            Some(_) if hrgn != 0 && wm.regions.get(hrgn).is_none() => Some(PMERR_INV_HRGN),
            Some(_) if hrgn != 0 && wm.clip_owner(hrgn).is_some_and(|o| o != hps) => Some(PMERR_HRGN_BUSY),
            Some(_) => None,
        };
        if let Some(e) = err {
            drop(wm);
            if pold != 0 { self.guest_write::<u32>(pold, HRGN_ERROR); }
            return self.fail_with(tid, e);
        }
        let ps = wm.ps_map.get_mut(&hps).expect("checked above");
        let old = std::mem::replace(&mut ps.clip_region, hrgn);
        let old = if old == hrgn { 0 } else { old };
        wm.refresh_clip(hps);
        let c = rgn_value(wm.effective_clip(hps).complexity());
        drop(wm);
        if pold != 0 { self.guest_write::<u32>(pold, old); }
        c
    }

    /// GpiIntersectClipRectangle (`op` = And) / GpiExcludeClipRectangle
    /// (`op` = Diff): change the clip region by an inclusive rectangle,
    /// creating one (everything) when the PS has none.
    fn gpi_clip_rectangle(&self, tid: u32, hps: u32, prcl: u32, op: SetOp) -> u32 {
        if !self.check_hps(tid, hps) { return RGN_ERROR; }
        // World coordinates: the device bounding box of the mapped corners.
        let xform = self.ps_xform(hps);
        let to_device = |r: Rectl| -> Result<Rectl, u32> {
            if xform.identity { return Ok(r); }
            if r.x_right < r.x_left || r.y_top < r.y_bottom { return Err(PMERR_INV_RECT); }
            let (x0, y0, x1, y1) = xform.bounds((r.x_left, r.y_bottom), (r.x_right, r.y_top));
            Ok(Rectl::new(x0, y0, x1, y1))
        };
        let rect = match self.read_rectl(prcl).ok_or(PMERR_INV_COORDINATE).and_then(to_device).and_then(inclusive_rect) {
            Ok(r) => Region::rect(r),
            Err(e) => { self.pm_error(tid, e); return RGN_ERROR; }
        };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let current = wm.ps_map[&hps].clip_region;
        let base = wm.regions.get(current).cloned().unwrap_or_else(|| Arc::new(Region::infinite()));
        let Some(next) = Region::combine(&base, &rect, op) else {
            drop(wm);
            self.pm_error(tid, PMERR_INSUFFICIENT_MEMORY);
            return RGN_ERROR;
        };
        if current != 0 && wm.regions.get(current).is_some() {
            wm.regions.set(current, next);
        } else {
            let Some(h) = wm.regions.create(next) else {
                drop(wm);
                self.pm_error(tid, PMERR_INSUFFICIENT_MEMORY);
                return RGN_ERROR;
            };
            if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.clip_region = h; }
        }
        wm.refresh_clip(hps);
        rgn_value(wm.effective_clip(hps).complexity())
    }

    fn gpi_offset_clip_region(&self, tid: u32, hps: u32, pptl: u32) -> u32 {
        if !self.check_hps(tid, hps) { return RGN_ERROR; }
        let Some((dx, dy)) = self.read_pointl(pptl) else {
            self.pm_error(tid, PMERR_COORDINATE_OVERFLOW);
            return RGN_ERROR;
        };
        // A world offset: the device displacement of the mapped vector.
        let xform = self.ps_xform(hps);
        let (dx, dy) = if xform.identity { (dx, dy) } else {
            let (vx, vy) = xform.m.apply_vector((dx as f64, dy as f64));
            (super::pm_xform::device_coord(vx), super::pm_xform::device_coord(vy))
        };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let current = wm.ps_map[&hps].clip_region;
        if let Some(r) = wm.regions.get(current) {
            let mut moved = (**r).clone();
            moved.offset(dx, dy);
            wm.regions.set(current, moved);
            wm.refresh_clip(hps);
        }
        rgn_value(wm.effective_clip(hps).complexity())
    }

    fn gpi_query_clip_box(&self, tid: u32, hps: u32, prcl: u32) -> u32 {
        if !self.check_hps(tid, hps) { return RGN_ERROR; }
        let clip = self.shared.window_mgr.lock_or_recover().effective_clip(hps);
        // Inclusive bounds; nothing left: right < left, top < bottom.
        let b = clip.bounds().map_or(Rectl::new(0, 0, -1, -1), |r| Rectl::new(r.x0, r.y0, r.x1 - 1, r.y1 - 1));
        // In world coordinates: the bounding box of the device box mapped
        // back (left as device pels when the transform is singular).
        let xform = self.ps_xform(hps);
        let b = if b.x_right < b.x_left { b } else {
            xform.inverse_bounds((b.x_left, b.y_bottom), (b.x_right, b.y_top))
                .map_or(b, |(x0, y0, x1, y1)| Rectl::new(x0, y0, x1, y1))
        };
        if !self.write_rectl(prcl, b) { self.pm_error(tid, PMERR_COORDINATE_OVERFLOW); return RGN_ERROR; }
        rgn_value(clip.complexity())
    }

    // ── Window update regions (PMWIN) ───────────────────────────────────────

    /// A region handle a window call may use (`PMERR_HRGN_BUSY` when it is
    /// a clip region, `PMERR_INV_HRGN` when unknown); `Ok(None)` for
    /// NULLHANDLE.
    fn window_region(&self, tid: u32, hrgn: u32) -> Result<Option<Arc<Region>>, ()> {
        if hrgn == 0 { return Ok(None); }
        let wm = self.shared.window_mgr.lock_or_recover();
        let err = match wm.regions.get(hrgn) {
            Some(_) if wm.clip_owner(hrgn).is_some() => PMERR_HRGN_BUSY,
            Some(r) => return Ok(Some(r.clone())),
            None => PMERR_INV_HRGN,
        };
        drop(wm);
        self.pm_error(tid, err);
        Err(())
    }

    /// WinInvalidateRegion(HWND hwnd, HRGN hrgn, BOOL fIncludeChildren) →
    /// BOOL: add the region (window coordinates; NULLHANDLE = the whole
    /// window) to the update region (`pm_paint.rs`).
    pub(super) fn win_invalidate_region(&self, tid: u32, hwnd: u32, hrgn: u32, include: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let Ok(region) = self.window_region(tid, hrgn) else { return ApiResult::Normal(0) };
        if region.as_ref().is_some_and(|r| r.is_empty()) { return ApiResult::Normal(1); }
        self.shared.window_mgr.lock_or_recover().invalidate_area(hwnd, region.as_deref(), include != 0);
        ApiResult::Normal(1)
    }

    /// WinValidateRegion(HWND hwnd, HRGN hrgn, BOOL fIncludeClippedChildren)
    /// → BOOL: remove the region from the update region.
    pub(super) fn win_validate_region(&self, tid: u32, hwnd: u32, hrgn: u32, include: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let Ok(region) = self.window_region(tid, hrgn) else { return ApiResult::Normal(0) };
        self.shared.window_mgr.lock_or_recover().validate_area(hwnd, region.as_deref(), include != 0);
        ApiResult::Normal(1)
    }

    /// WinQueryUpdateRegion(HWND hwnd, HRGN hrgn) → LONG: set the region to
    /// the window's update region (window coordinates).
    pub(super) fn win_query_update_region(&self, tid: u32, hwnd: u32, hrgn: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(RGN_ERROR); }
        let Ok(Some(_)) = self.window_region(tid, hrgn) else {
            if hrgn == 0 { self.pm_error(tid, PMERR_INV_HRGN); }
            return ApiResult::Normal(RGN_ERROR);
        };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let region = wm.update_region(hwnd);
        let c = rgn_value(region.complexity());
        wm.regions.set(hrgn, region);
        ApiResult::Normal(c)
    }

    /// WinExcludeUpdateRegion(HPS hps, HWND hwnd) → LONG: take the window's
    /// update region out of the PS's clip region (creating one).
    pub(super) fn win_exclude_update_region(&self, tid: u32, hps: u32, hwnd: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(RGN_ERROR); }
        if !self.check_hps(tid, hps) { return ApiResult::Normal(RGN_ERROR); }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let update = wm.update_region(hwnd);
        let current = wm.ps_map[&hps].clip_region;
        if update.is_empty() {
            return ApiResult::Normal(rgn_value(wm.effective_clip(hps).complexity()));
        }
        let base = wm.regions.get(current).cloned().unwrap_or_else(|| Arc::new(Region::infinite()));
        let Some(next) = Region::combine(&base, &update, SetOp::Diff) else {
            return ApiResult::Normal(RGN_ERROR);
        };
        if wm.regions.get(current).is_some() {
            wm.regions.set(current, next);
        } else {
            let Some(h) = wm.regions.create(next) else { return ApiResult::Normal(RGN_ERROR) };
            if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.clip_region = h; }
        }
        wm.refresh_clip(hps);
        ApiResult::Normal(rgn_value(wm.effective_clip(hps).complexity()))
    }

    /// The `hrgnUpdate` of WinScrollWindow: checked before scrolling
    /// (`Err` fails the call), set to the uncovered rectangles after.
    pub(super) fn scroll_update_region(&self, tid: u32, hrgn: u32) -> Result<(), ()> {
        self.window_region(tid, hrgn).map(|_| ())
    }

    /// Set region `hrgn` (if any) to the union of `rects`.
    pub(super) fn set_region_rects(&self, hrgn: u32, rects: &[Rectl]) {
        if hrgn == 0 { return; }
        let rects: Vec<RRect> = rects.iter().map(|r| RRect::new(r.x_left, r.y_bottom, r.x_right, r.y_top)).collect();
        if let Some(r) = Region::from_rects(&rects) {
            self.shared.window_mgr.lock_or_recover().regions.set(hrgn, r);
        }
    }
}

/// Helpers for the region, clip, area and path tests.
#[cfg(test)]
pub(super) mod testing {
    use super::super::mutex_ext::MutexExt;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::{ApiResult, Loader};

    pub const ARGS: u32 = 0x1000;
    pub const CLR_RED: u32 = 2;
    pub const CLR_BLUE: u32 = 1;
    pub const CLR_GREEN: u32 = 4;

    fn call(loader: &Loader, args: &[u32], f: impl FnOnce(&mut MockVcpu) -> ApiResult) -> u32 {
        let mut vcpu = MockVcpu::new();
        vcpu.regs.rsp = ARGS as u64;
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(ARGS + 4 + i as u32 * 4, a).unwrap();
        }
        match f(&mut vcpu) {
            ApiResult::Normal(v) => v,
            other => panic!("unexpected {other:?}"),
        }
    }

    pub fn gpi(loader: &Loader, ordinal: u32, args: &[u32]) -> u32 {
        call(loader, args, |v| loader.handle_pmgpi_call(v, 0, ordinal))
    }

    pub fn win(loader: &Loader, ordinal: u32, args: &[u32]) -> u32 {
        call(loader, args, |v| loader.handle_pmwin_call(v, 0, ordinal))
    }

    /// The code of the thread's last PM error (`WinGetLastError`), reset.
    pub fn last_error(loader: &Loader) -> u32 {
        win(loader, 753, &[0]) & 0xFFFF
    }

    /// A queue and a 200×100 window with a presentation space:
    /// `(loader, hwnd, hps)`.
    pub fn setup() -> (Loader, u32, u32) {
        let loader = Loader::new_mock();
        let hmq = win(&loader, 716, &[0, 0]); // WinCreateMsgQueue
        let (hwnd, hps) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hwnd = wm.create_window("Test".into(), 0, hmq);
            let w = wm.get_window_mut(hwnd).unwrap();
            (w.cx, w.cy, w.visible) = (200, 100, true);
            (hwnd, wm.create_ps(hwnd))
        };
        (loader, hwnd, hps)
    }

    /// Write `RECTL`s at `at`.
    pub fn put_rects(loader: &Loader, at: u32, rects: &[(i32, i32, i32, i32)]) {
        for (i, &(a, b, c, d)) in rects.iter().enumerate() {
            for (k, v) in [a, b, c, d].into_iter().enumerate() {
                loader.guest_write::<i32>(at + 16 * i as u32 + 4 * k as u32, v).unwrap();
            }
        }
    }

    /// Write `POINTL`s at `at`.
    pub fn put_points(loader: &Loader, at: u32, pts: &[(i32, i32)]) {
        for (i, &(x, y)) in pts.iter().enumerate() {
            loader.guest_write::<i32>(at + 8 * i as u32, x).unwrap();
            loader.guest_write::<i32>(at + 8 * i as u32 + 4, y).unwrap();
        }
    }

    pub fn get_rect(loader: &Loader, at: u32) -> (i32, i32, i32, i32) {
        let r = loader.read_rectl(at).unwrap();
        (r.x_left, r.y_bottom, r.x_right, r.y_top)
    }

    /// GpiQueryPel at `(x, y)`.
    pub fn pel(loader: &Loader, hps: u32, x: i32, y: i32) -> u32 {
        const AT: u32 = 0x1F00;
        put_points(loader, AT, &[(x, y)]);
        gpi(loader, 476, &[hps, AT])
    }

    /// GpiMove to `p`.
    pub fn move_to(loader: &Loader, hps: u32, p: (i32, i32)) {
        const AT: u32 = 0x1F10;
        put_points(loader, AT, &[p]);
        gpi(loader, 404, &[hps, AT]);
    }

    /// GpiCreateRegion of `rects`.
    pub fn region(loader: &Loader, hps: u32, rects: &[(i32, i32, i32, i32)]) -> u32 {
        const AT: u32 = 0x5000;
        put_rects(loader, AT, rects);
        gpi(loader, 370, &[hps, rects.len() as u32, AT])
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::super::constants::*;

    // PMGPI ordinals (doc/os2_ordinals.md).
    const CREATE: u32 = 370;
    const DESTROY: u32 = 611;
    const SET: u32 = 546;
    const COMBINE: u32 = 362;
    const BOX: u32 = 481;
    const RECTS: u32 = 587;
    const PT_IN: u32 = 419;
    const RECT_IN: u32 = 494;
    const OFFSET: u32 = 407;
    const EQUAL: u32 = 388;
    const PAINT: u32 = 409;
    const FRAME: u32 = 617;
    const SET_CLIP: u32 = 516;
    const QUERY_CLIP: u32 = 436;
    const INTERSECT_CLIP: u32 = 396;
    const EXCLUDE_CLIP: u32 = 391;
    const OFFSET_CLIP: u32 = 405;
    const CLIP_BOX: u32 = 435;
    const SET_COLOR: u32 = 517;
    const GPI_BOX: u32 = 356;
    const OUT: u32 = 0x6000;
    const CTL: u32 = 0x6100;
    const PT: u32 = 0x6200;

    #[test]
    fn create_query_and_destroy_regions() {
        let (l, _, hps) = setup();
        let r = region(&l, hps, &[(10, 10, 20, 20), (15, 15, 30, 25)]);
        assert_ne!(r, RGN_ERROR);
        assert_eq!(gpi(&l, BOX, &[hps, r, OUT]), RGN_COMPLEX);
        assert_eq!(get_rect(&l, OUT), (10, 10, 30, 25));
        // Left / bottom in, right / top out.
        put_points(&l, PT, &[(10, 10)]);
        assert_eq!(gpi(&l, PT_IN, &[hps, r, PT]), PRGN_INSIDE);
        put_points(&l, PT, &[(20, 12)]);
        assert_eq!(gpi(&l, PT_IN, &[hps, r, PT]), PRGN_OUTSIDE);
        put_rects(&l, PT, &[(11, 11, 14, 14)]);
        assert_eq!(gpi(&l, RECT_IN, &[hps, r, PT]), RRGN_INSIDE);
        put_rects(&l, PT, &[(0, 0, 12, 12)]);
        assert_eq!(gpi(&l, RECT_IN, &[hps, r, PT]), RRGN_PARTIAL);
        put_rects(&l, PT, &[(40, 40, 50, 50)]);
        assert_eq!(gpi(&l, RECT_IN, &[hps, r, PT]), RRGN_OUTSIDE);
        // An empty region: left = right, bottom = top.
        let e = region(&l, hps, &[]);
        assert_eq!(gpi(&l, BOX, &[hps, e, OUT]), RGN_NULL);
        assert_eq!(get_rect(&l, OUT), (0, 0, 0, 0));
        // GpiSetRegion replaces the contents.
        put_rects(&l, PT, &[(0, 0, 5, 5)]);
        assert_eq!(gpi(&l, SET, &[hps, e, 1, PT]), 1);
        assert_eq!(gpi(&l, BOX, &[hps, e, OUT]), RGN_RECT);
        assert_eq!(gpi(&l, DESTROY, &[hps, e]), 1);
        assert_eq!(gpi(&l, BOX, &[hps, e, OUT]), RGN_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_HRGN);
        assert_eq!(gpi(&l, DESTROY, &[hps, 0]), 1, "NULLHANDLE is no error");
    }

    #[test]
    fn bad_arguments_fail_with_their_errors() {
        let (l, _, hps) = setup();
        assert_eq!(region(&l, 0xDEAD, &[(0, 0, 1, 1)]), RGN_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_HPS);
        assert_eq!(region(&l, hps, &[(10, 0, 5, 5)]), RGN_ERROR, "right left of left");
        assert_eq!(last_error(&l), PMERR_INV_RECT);
        assert_eq!(region(&l, hps, &[(0, 0, 1 << 28, 5)]), RGN_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_COORDINATE);
        assert_eq!(gpi(&l, CREATE, &[hps, (-1i32) as u32, 0x5000]), RGN_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_LENGTH_OR_COUNT);
        assert_eq!(gpi(&l, CREATE, &[hps, 1_000_000, 0x5000]), RGN_ERROR, "count bounded");
        assert_eq!(last_error(&l), PMERR_INV_LENGTH_OR_COUNT);
        let a = region(&l, hps, &[(0, 0, 5, 5)]);
        assert_eq!(gpi(&l, COMBINE, &[hps, a, a, a, 3]), RGN_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_REGION_MIX_MODE);
    }

    #[test]
    fn combine_offset_and_equal() {
        let (l, _, hps) = setup();
        let a = region(&l, hps, &[(0, 0, 10, 10)]);
        let b = region(&l, hps, &[(5, 0, 15, 10)]);
        let d = region(&l, hps, &[]);
        for (mode, want, bx) in [
            (CRGN_OR, RGN_RECT, (0, 0, 15, 10)), (CRGN_AND, RGN_RECT, (5, 0, 10, 10)),
            (CRGN_DIFF, RGN_RECT, (0, 0, 5, 10)), (CRGN_XOR, RGN_COMPLEX, (0, 0, 15, 10)),
            (CRGN_COPY, RGN_RECT, (0, 0, 10, 10)),
        ] {
            assert_eq!(gpi(&l, COMBINE, &[hps, d, a, b, mode]), want, "mode {mode}");
            gpi(&l, BOX, &[hps, d, OUT]);
            assert_eq!(get_rect(&l, OUT), bx, "mode {mode}");
        }
        // The destination may be a source.
        assert_eq!(gpi(&l, COMBINE, &[hps, a, a, b, CRGN_AND]), RGN_RECT);
        gpi(&l, BOX, &[hps, a, OUT]);
        assert_eq!(get_rect(&l, OUT), (5, 0, 10, 10));
        put_points(&l, PT, &[(-5, 0)]);
        assert_eq!(gpi(&l, OFFSET, &[hps, b, PT]), 1);
        assert_eq!(gpi(&l, EQUAL, &[hps, a, b]), EQRGN_NOTEQUAL);
        let c = region(&l, hps, &[(0, 0, 10, 10)]);
        assert_eq!(gpi(&l, EQUAL, &[hps, b, c]), EQRGN_EQUAL);
    }

    #[test]
    fn query_region_rects_pages_and_orders() {
        let (l, _, hps) = setup();
        let r = region(&l, hps, &[(0, 0, 2, 2), (4, 0, 6, 2), (0, 4, 2, 6)]);
        let ctl = |start: u32, crc: u32, dir: u32| {
            for (i, v) in [start, crc, 0, dir].into_iter().enumerate() {
                l.guest_write::<u32>(CTL + 4 * i as u32, v).unwrap();
            }
        };
        let returned = || l.guest_read::<u32>(CTL + 8).unwrap();
        ctl(1, 10, RECTDIR_LFRT_TOPBOT);
        assert_eq!(gpi(&l, RECTS, &[hps, r, 0, CTL, OUT]), 1);
        assert_eq!(returned(), 3);
        assert_eq!(get_rect(&l, OUT), (0, 4, 2, 6), "top band first");
        assert_eq!(get_rect(&l, OUT + 32), (4, 0, 6, 2));
        // Paging: from the second, at most one.
        ctl(2, 1, RECTDIR_LFRT_TOPBOT);
        assert_eq!(gpi(&l, RECTS, &[hps, r, 0, CTL, OUT]), 1);
        assert_eq!((returned(), get_rect(&l, OUT)), (1, (0, 0, 2, 2)));
        // Count only, and cut to a bound.
        put_rects(&l, PT, &[(1, 1, 5, 5)]);
        ctl(1, 0, RECTDIR_LFRT_BOTTOP);
        assert_eq!(gpi(&l, RECTS, &[hps, r, PT, CTL, 0]), 1);
        assert_eq!(returned(), 3);
        ctl(1, 3, RECTDIR_RTLF_BOTTOP);
        gpi(&l, RECTS, &[hps, r, PT, CTL, OUT]);
        assert_eq!(get_rect(&l, OUT), (4, 1, 5, 2));
        ctl(1, 3, 9);
        assert_eq!(gpi(&l, RECTS, &[hps, r, 0, CTL, OUT]), 0);
        assert_eq!(last_error(&l), PMERR_INV_REGION_CONTROL);
    }

    #[test]
    fn clip_region_ownership() {
        let (l, hwnd, hps) = setup();
        let r = region(&l, hps, &[(0, 0, 50, 50)]);
        l.guest_write::<u32>(OUT, 0x1234).unwrap();
        assert_eq!(gpi(&l, SET_CLIP, &[hps, r, OUT]), RGN_RECT);
        assert_eq!(l.guest_read::<u32>(OUT), Some(0), "no old clip region");
        assert_eq!(gpi(&l, QUERY_CLIP, &[hps]), r);
        // While selected, other region calls refuse it.
        assert_eq!(gpi(&l, BOX, &[hps, r, OUT]), RGN_ERROR);
        assert_eq!(last_error(&l), PMERR_REGION_IS_CLIP_REGION);
        assert_eq!(gpi(&l, DESTROY, &[hps, r]), 0);
        assert_eq!(last_error(&l), PMERR_REGION_IS_CLIP_REGION);
        // Another PS cannot select it.
        let hps2 = l.shared.window_mgr.lock().unwrap().create_ps(hwnd);
        assert_eq!(gpi(&l, SET_CLIP, &[hps2, r, OUT]), RGN_ERROR);
        assert_eq!(last_error(&l), PMERR_HRGN_BUSY);
        assert_eq!(l.guest_read::<u32>(OUT), Some(HRGN_ERROR));
        // Deselecting returns it; it is usable again.
        assert_eq!(gpi(&l, SET_CLIP, &[hps, 0, OUT]), RGN_RECT, "the window is the clip");
        assert_eq!(l.guest_read::<u32>(OUT), Some(r));
        assert_eq!(gpi(&l, BOX, &[hps, r, OUT]), RGN_RECT);
        // Destroying a PS destroys its clip region.
        assert_eq!(gpi(&l, SET_CLIP, &[hps2, r, OUT]), RGN_RECT);
        gpi(&l, 379, &[hps2]); // GpiDestroyPS
        assert_eq!(gpi(&l, BOX, &[hps, r, OUT]), RGN_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_HRGN);
    }

    #[test]
    fn clipping_limits_drawing() {
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        // Draw only inside (20,20)–(39,39) less (25,25)–(29,29).
        put_rects(&l, PT, &[(20, 20, 39, 39)]);
        assert_eq!(gpi(&l, INTERSECT_CLIP, &[hps, PT]), RGN_RECT);
        put_rects(&l, PT, &[(25, 25, 29, 29)]);
        assert_eq!(gpi(&l, EXCLUDE_CLIP, &[hps, PT]), RGN_COMPLEX);
        assert_eq!(gpi(&l, CLIP_BOX, &[hps, OUT]), RGN_COMPLEX);
        assert_eq!(get_rect(&l, OUT), (20, 20, 39, 39), "inclusive");
        assert_ne!(gpi(&l, QUERY_CLIP, &[hps]), 0, "a clip region was created");
        move_to(&l, hps, (0, 0));
        put_points(&l, PT, &[(99, 99)]);
        assert_eq!(gpi(&l, GPI_BOX, &[hps, DRO_FILL, PT, 0, 0]), GPI_OK);
        assert_eq!(pel(&l, hps, 20, 20), CLR_RED);
        assert_eq!(pel(&l, hps, 39, 39), CLR_RED, "the clip rectangle's edges are inside");
        assert_eq!(pel(&l, hps, 19, 30), CLR_BACKGROUND);
        assert_eq!(pel(&l, hps, 40, 30), CLR_BACKGROUND);
        assert_eq!(pel(&l, hps, 27, 27), CLR_BACKGROUND, "excluded");
        assert_eq!(pel(&l, hps, 24, 27), CLR_RED);
        // WinFillRect is clipped too.
        put_rects(&l, PT, &[(0, 0, 200, 100)]);
        assert_eq!(win(&l, 743, &[hps, PT, CLR_BLUE]), 1);
        assert_eq!(pel(&l, hps, 30, 21), CLR_BLUE);
        assert_eq!(pel(&l, hps, 10, 10), CLR_BACKGROUND);
        // Moving the clip region.
        put_points(&l, PT, &[(100, 0)]);
        assert_eq!(gpi(&l, OFFSET_CLIP, &[hps, PT]), RGN_COMPLEX);
        gpi(&l, CLIP_BOX, &[hps, OUT]);
        assert_eq!(get_rect(&l, OUT), (120, 20, 139, 39));
        // Nothing left: right < left.
        put_rects(&l, PT, &[(0, 0, 5, 5)]);
        assert_eq!(gpi(&l, INTERSECT_CLIP, &[hps, PT]), RGN_NULL);
        gpi(&l, CLIP_BOX, &[hps, OUT]);
        let (x0, y0, x1, y1) = get_rect(&l, OUT);
        assert!(x1 < x0 && y1 < y0);
        // Without a clip region the clip box is the window.
        gpi(&l, SET_CLIP, &[hps, 0, OUT]);
        assert_eq!(gpi(&l, CLIP_BOX, &[hps, OUT]), RGN_RECT);
        assert_eq!(get_rect(&l, OUT), (0, 0, 199, 99));
    }

    #[test]
    fn paint_and_frame_region() {
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_GREEN]);
        let r = region(&l, hps, &[(10, 10, 20, 20), (30, 10, 40, 20)]);
        assert_eq!(gpi(&l, PAINT, &[hps, r]), GPI_OK);
        assert_eq!(pel(&l, hps, 10, 10), CLR_GREEN);
        assert_eq!(pel(&l, hps, 19, 19), CLR_GREEN);
        assert_eq!(pel(&l, hps, 20, 19), CLR_BACKGROUND, "right edge out");
        assert_eq!(pel(&l, hps, 35, 15), CLR_GREEN);
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        let f = region(&l, hps, &[(50, 50, 70, 70)]);
        put_points(&l, PT, &[(2, 3)]);
        assert_eq!(gpi(&l, FRAME, &[hps, f, PT]), GPI_OK);
        assert_eq!(pel(&l, hps, 50, 60), CLR_RED);
        assert_eq!(pel(&l, hps, 51, 60), CLR_RED);
        assert_eq!(pel(&l, hps, 52, 60), CLR_BACKGROUND, "cx = 2 pels wide");
        assert_eq!(pel(&l, hps, 60, 67), CLR_RED);
        assert_eq!(pel(&l, hps, 60, 66), CLR_BACKGROUND, "cy = 3 pels high");
        assert_eq!(pel(&l, hps, 69, 69), CLR_RED);
        assert_eq!(pel(&l, hps, 70, 69), CLR_BACKGROUND);
        put_points(&l, PT, &[(-1, 3)]);
        assert_eq!(gpi(&l, FRAME, &[hps, f, PT]), GPI_ERROR);
    }

    #[test]
    fn window_update_regions() {
        let (l, hwnd, hps) = setup();
        let r = region(&l, hps, &[(10, 10, 20, 20)]);
        // WinInvalidateRegion adds the region's bounds to the update area.
        assert_eq!(win(&l, 766, &[hwnd, r, 0]), 1);
        let q = region(&l, hps, &[]);
        assert_eq!(win(&l, 832, &[hwnd, q]), RGN_RECT);
        assert_eq!(gpi(&l, BOX, &[hps, q, OUT]), RGN_RECT);
        assert_eq!(get_rect(&l, OUT), (10, 10, 20, 20));
        // WinExcludeUpdateRegion keeps drawing out of it.
        assert_eq!(win(&l, 742, &[hps, hwnd]), RGN_COMPLEX);
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        move_to(&l, hps, (0, 0));
        put_points(&l, PT, &[(50, 50)]);
        gpi(&l, GPI_BOX, &[hps, DRO_FILL, PT, 0, 0]);
        assert_eq!(pel(&l, hps, 15, 15), CLR_BACKGROUND);
        assert_eq!(pel(&l, hps, 25, 15), CLR_RED);
        // WinValidateRegion empties it again.
        assert_eq!(win(&l, 896, &[hwnd, r, 0]), 1);
        assert_eq!(win(&l, 832, &[hwnd, q]), RGN_NULL);
        // A clip region is busy for window calls.
        let clip = gpi(&l, QUERY_CLIP, &[hps]);
        assert_eq!(win(&l, 766, &[hwnd, clip, 0]), 0);
        assert_eq!(last_error(&l), PMERR_HRGN_BUSY);
        // WinScrollWindow reports the uncovered area in hrgnUpdate.
        let u = region(&l, hps, &[]);
        assert_eq!(win(&l, 849, &[hwnd, 10, 0, 0, 0, u, 0, 0]), RGN_RECT);
        gpi(&l, BOX, &[hps, u, OUT]);
        assert_eq!(get_rect(&l, OUT), (0, 0, 10, 100));
    }
}
