// SPDX-License-Identifier: GPL-3.0-only

//! Presentation-space state (PMGPI): save / restore, reset, attribute
//! bundles, `GpiQueryPS` / `GpiSetPS`.
//!
//! - `GpiSavePS` pushes a copy of the whole presentation-space state (every
//!   attribute, the current position, transforms, page viewport, viewing
//!   limits, graphics field, clip path, the selected clip region, colour
//!   table, logical fonts) and returns its depth (1 = the first save).
//!   Copying the whole `PresentationSpace` rather than a field list means
//!   attributes added later are saved too. Not saved: the window, device
//!   context, bitmap, creator and open area / path brackets.
//!   The clip region is saved as its handle *and* its contents: restoring
//!   puts the contents back into that handle, so the common Save →
//!   `GpiIntersectClipRectangle` → draw → Restore sequence gets its old
//!   clip back (the reference only says "references … must not be
//!   changed"; Warpine's choice). At most `MAX_SAVED_PS` levels
//!   (`PMERR_INSUFFICIENT_MEMORY` beyond, Warpine's bound).
//! - `GpiRestorePS(lPSid)`: a positive id is an absolute depth, a negative
//!   one counts back from the most recent (-1); 0 or a missing level fails
//!   with `PMERR_INV_ID`. The levels above the restored one are discarded
//!   and their clip regions destroyed, as is the clip region selected
//!   before the restore when nothing kept references it (unverified: the
//!   reference speaks of "clip regions selected into discarded
//!   presentation spaces").
//! - `GpiResetPS`: `GRES_ATTRS` resets the attributes, model transform,
//!   current position (0, 0), ends open area / path brackets, removes the
//!   clip path and viewing limits; `GRES_SEGMENTS` also releases the clip
//!   region (deselects it — it is the application's again, unverified)
//!   and resets the default view matrix, viewing transform and graphics
//!   field; `GRES_ALL` also deletes the logical fonts and resets the
//!   colour table (the *GPI Reference* wording).
//! - `GpiSetAttrs` / `GpiQueryAttrs` for `PRIM_LINE` / `CHAR` / `MARKER` /
//!   `AREA` / `IMAGE` bundles (`<pmgpi.h>` layouts). Warpine keeps one
//!   colour, background colour, mix and background mix for all primitive
//!   types (`GpiSetColor` sets them all in PM too; `GpiSetAttrs` for one
//!   type sets the shared value — an approximation). The character
//!   bundle's mode (`usPrecision`), angle, shear, direction, text alignment
//!   (one USHORT: horizontal `TA_*` in the low byte, vertical in the high
//!   byte), extra and break extra are the text attributes of
//!   `GpiSetCharMode` … `GpiSetCharBreakExtra` (`PresentationSpace::text`)
//!   and are checked as those calls check them. The attributes Warpine
//!   does not model (line ends and joins, marker set / symbol / box,
//!   pattern set) are accepted and ignored, and reported as default.
//! - `GpiQueryPS` returns the options (`PU_*`, `GPIF_*`, `GPIT_*`,
//!   `GPIA_*`) and the page size; `GpiSetPS` changes units, format and size
//!   and resets the presentation space (`GRES_ALL`, or `GRES_SEGMENTS` with
//!   `PS_NORESET`); its codepage becomes the process codepage.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_area::PathState;
use super::pm_text::{
    TextAttrs, CHDIRN_BOTTOMTOP, CHDIRN_DEFAULT, CHDIRN_LEFTRIGHT, CM_DEFAULT, CM_MODE1, CM_MODE3,
    PMERR_INV_CHAR_ALIGN_ATTR, PMERR_INV_CHAR_DIRECTION_ATTR, PMERR_INV_CHAR_MODE_ATTR, PMERR_INV_CHAR_SHEAR_ATTR,
    TA_NORMAL_HORIZ, TA_NORMAL_VERT, TA_STANDARD_HORIZ, TA_STANDARD_VERT,
};
use super::pm_types::{PresentationSpace, WindowManager};
use super::pm_xform::{pels_per_unit, CoordState};
use super::Loader;
use crate::gui::region::Region;
use crate::gui::surface::Mix;

/// Most states `GpiSavePS` keeps per presentation space.
pub const MAX_SAVED_PS: usize = 256;

/// A state saved by `GpiSavePS`.
#[derive(Clone)]
pub struct SavedPs {
    /// The presentation space as it was (empty brackets and stack).
    pub ps: PresentationSpace,
    /// The contents of its clip region (`ps.clip_region`) at that time.
    pub clip_region: Option<Arc<Region>>,
}

/// The reset level of `GpiResetPS` options (`GRES_*`, one of them).
fn reset_level(options: u32) -> Option<u32> {
    match options {
        GRES_ATTRS | GRES_SEGMENTS | GRES_ALL => Some(options),
        _ => None,
    }
}

// Bundle field offsets (<pmgpi.h>): every bundle starts with lColor,
// lBackColor, usMixMode, usBackMixMode.
const B_COLOR: u32 = 0;
const B_BACK_COLOR: u32 = 4;
const B_MIX: u32 = 8;
const B_BACK_MIX: u32 = 10;
// LINEBUNDLE
const LB_WIDTH: u32 = 12;
const LB_GEOM_WIDTH: u32 = 16;
const LB_TYPE: u32 = 20;
// CHARBUNDLE
const CB_SET: u32 = 12;
const CB_PRECISION: u32 = 14;
const CB_CELL: u32 = 16;
const CB_ANGLE: u32 = 24;
const CB_SHEAR: u32 = 32;
const CB_DIRECTION: u32 = 40;
const CB_TEXT_ALIGN: u32 = 42;
const CB_EXTRA: u32 = 44;
const CB_BREAK_EXTRA: u32 = 48;
// AREABUNDLE
const AB_SYMBOL: u32 = 14;
const AB_REF_POINT: u32 = 16;

/// The attribute bits defined for primitive type `prim`; `None` for an
/// invalid type.
fn defined_bits(prim: u32) -> Option<u32> {
    Some(match prim {
        PRIM_LINE => LBB_COLOR | LBB_BACK_COLOR | LBB_MIX_MODE | LBB_BACK_MIX_MODE | LBB_WIDTH
            | LBB_GEOM_WIDTH | LBB_TYPE | LBB_END | LBB_JOIN,
        PRIM_CHAR => CBB_COLOR | CBB_BACK_COLOR | CBB_MIX_MODE | CBB_BACK_MIX_MODE | CBB_SET | CBB_MODE
            | CBB_BOX | CBB_ANGLE | CBB_SHEAR | CBB_DIRECTION | CBB_TEXT_ALIGN | CBB_EXTRA | CBB_BREAK_EXTRA,
        PRIM_MARKER => MBB_COLOR | MBB_BACK_COLOR | MBB_MIX_MODE | MBB_BACK_MIX_MODE | MBB_SET | MBB_SYMBOL | MBB_BOX,
        PRIM_AREA => ABB_COLOR | ABB_BACK_COLOR | ABB_MIX_MODE | ABB_BACK_MIX_MODE | ABB_SET | ABB_SYMBOL
            | ABB_REF_POINT,
        PRIM_IMAGE => IBB_COLOR | IBB_BACK_COLOR | IBB_MIX_MODE | IBB_BACK_MIX_MODE,
        _ => return None,
    })
}

/// One attribute of a bundle that Warpine keeps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Attr {
    Color,
    BackColor,
    Mix,
    BackMix,
    LineWidth,
    GeomWidth,
    LineType,
    CharSet,
    CharBox,
    CharMode,
    CharAngle,
    CharShear,
    CharDirection,
    CharAlign,
    CharExtra,
    CharBreakExtra,
    Pattern,
    RefPoint,
}

/// What the attribute bit `bit` of a `prim` bundle is and its offset;
/// `None` for the attributes Warpine does not keep. The four common bits
/// are equal in every bundle (`*BB_COLOR` = 1 …).
fn attr_of(prim: u32, bit: u32) -> Option<(Attr, u32)> {
    Some(match (prim, bit) {
        (_, LBB_COLOR) => (Attr::Color, B_COLOR),
        (_, LBB_BACK_COLOR) => (Attr::BackColor, B_BACK_COLOR),
        (_, LBB_MIX_MODE) => (Attr::Mix, B_MIX),
        (_, LBB_BACK_MIX_MODE) => (Attr::BackMix, B_BACK_MIX),
        (PRIM_LINE, LBB_WIDTH) => (Attr::LineWidth, LB_WIDTH),
        (PRIM_LINE, LBB_GEOM_WIDTH) => (Attr::GeomWidth, LB_GEOM_WIDTH),
        (PRIM_LINE, LBB_TYPE) => (Attr::LineType, LB_TYPE),
        (PRIM_CHAR, CBB_SET) => (Attr::CharSet, CB_SET),
        (PRIM_CHAR, CBB_BOX) => (Attr::CharBox, CB_CELL),
        (PRIM_CHAR, CBB_MODE) => (Attr::CharMode, CB_PRECISION),
        (PRIM_CHAR, CBB_ANGLE) => (Attr::CharAngle, CB_ANGLE),
        (PRIM_CHAR, CBB_SHEAR) => (Attr::CharShear, CB_SHEAR),
        (PRIM_CHAR, CBB_DIRECTION) => (Attr::CharDirection, CB_DIRECTION),
        (PRIM_CHAR, CBB_TEXT_ALIGN) => (Attr::CharAlign, CB_TEXT_ALIGN),
        (PRIM_CHAR, CBB_EXTRA) => (Attr::CharExtra, CB_EXTRA),
        (PRIM_CHAR, CBB_BREAK_EXTRA) => (Attr::CharBreakExtra, CB_BREAK_EXTRA),
        (PRIM_AREA, ABB_SYMBOL) => (Attr::Pattern, AB_SYMBOL),
        (PRIM_AREA, ABB_REF_POINT) => (Attr::RefPoint, AB_REF_POINT),
        _ => return None,
    })
}

/// FIXED of a whole number of pels (character box cells).
fn fixed_of(v: i32) -> u32 {
    (v.clamp(i16::MIN as i32, i16::MAX as i32) << 16) as u32
}

impl WindowManager {
    /// Destroy the regions in `candidates` that `hps` no longer selects or
    /// keeps on its save stack.
    fn drop_unused_clip_regions(&mut self, hps: u32, candidates: &[u32]) {
        let Some(ps) = self.ps_map.get(&hps) else { return };
        let in_use: Vec<u32> = std::iter::once(ps.clip_region)
            .chain(ps.saved.iter().map(|s| s.ps.clip_region)).collect();
        for &h in candidates {
            if h != 0 && !in_use.contains(&h) && self.clip_owner(h).is_none() {
                self.regions.remove(h);
            }
        }
    }
}

impl Loader {
    /// The PMGPI presentation-space state calls by ordinal; `None` for other
    /// ordinals.
    pub(super) fn handle_psstate_call(&self, tid: u32, ordinal: u32, arg: &dyn Fn(u64) -> u32) -> Option<u32> {
        Some(match ordinal {
            501 => {
                // GpiSavePS(HPS hps) → LONG
                self.gpi_save_ps(tid, arg(4))
            }
            499 => {
                // GpiRestorePS(HPS hps, LONG lPSid) → BOOL
                self.gpi_restore_ps(tid, arg(4), arg(8) as i32)
            }
            498 => {
                // GpiResetPS(HPS hps, ULONG flOptions) → BOOL
                let (hps, options) = (arg(4), arg(8));
                let Some(level) = reset_level(options) else { return Some(self.state_fail(tid, PMERR_INV_RESET_OPTIONS)) };
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if !wm.ps_map.contains_key(&hps) { drop(wm); return Some(self.state_fail(tid, PMERR_INV_HPS)); }
                reset_ps(&mut wm, hps, level);
                1
            }
            588 => {
                // GpiSetAttrs(HPS hps, LONG lPrimType, ULONG flAttrMask, ULONG flDefMask, PBUNDLE ppbunAttrs) → BOOL
                self.gpi_set_attrs(tid, arg(4), arg(8), arg(12), arg(16), arg(20))
            }
            583 => {
                // GpiQueryAttrs(HPS hps, LONG lPrimType, ULONG flAttrMask, PBUNDLE ppbunAttrs) → LONG
                self.gpi_query_attrs(tid, arg(4), arg(8), arg(12), arg(16))
            }
            471 => {
                // GpiQueryPS(HPS hps, PSIZEL psizlSize) → ULONG
                let (hps, psizl) = (arg(4), arg(8));
                let Some((options, size)) = self.shared.window_mgr.lock_or_recover().ps_map.get(&hps)
                    .map(|ps| (ps.coords.options, ps.coords.page_size()))
                else { return Some(self.state_fail(tid, PMERR_INV_HPS)) };
                if psizl != 0 && (self.guest_write::<i32>(psizl, size.0).is_none()
                    || self.guest_write::<i32>(psizl.wrapping_add(4), size.1).is_none())
                {
                    return Some(self.state_fail(tid, PMERR_INV_PS_SIZE));
                }
                options
            }
            539 => {
                // GpiSetPS(HPS hps, PSIZEL psizlsize, ULONG flOptions) → BOOL
                self.gpi_set_ps(tid, arg(4), arg(8), arg(12))
            }
            _ => return None,
        })
    }

    /// Record `code` and return FALSE / `GPI_ERROR`.
    fn state_fail(&self, tid: u32, code: u32) -> u32 {
        self.pm_error(tid, code);
        GPI_ERROR
    }

    fn gpi_save_ps(&self, tid: u32, hps: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(ps) = wm.ps_map.get(&hps) else { drop(wm); return self.state_fail(tid, PMERR_INV_HPS) };
        if ps.saved.len() >= MAX_SAVED_PS { drop(wm); return self.state_fail(tid, PMERR_INSUFFICIENT_MEMORY); }
        let clip_region = (ps.clip_region != 0).then(|| wm.regions.get(ps.clip_region).cloned()).flatten();
        let ps = wm.ps_map.get_mut(&hps).expect("checked above");
        // Copy without the stack and the brackets.
        let saved = std::mem::take(&mut ps.saved);
        let (area, path) = (ps.area.take(), ps.path.take());
        let snapshot = SavedPs { ps: ps.clone(), clip_region };
        (ps.saved, ps.area, ps.path) = (saved, area, path);
        ps.saved.push(snapshot);
        ps.saved.len() as u32
    }

    fn gpi_restore_ps(&self, tid: u32, hps: u32, id: i32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(ps) = wm.ps_map.get_mut(&hps) else { drop(wm); return self.state_fail(tid, PMERR_INV_HPS) };
        let depth = ps.saved.len() as i64;
        let level = match id {
            0 => None,
            i if i > 0 => (i as i64 <= depth).then_some(i as i64),
            i => (depth + 1 + i as i64 >= 1).then_some(depth + 1 + i as i64),
        };
        let Some(level) = level else { drop(wm); return self.state_fail(tid, PMERR_INV_ID) };
        let index = (level - 1) as usize;
        // The restored level and the ones above it leave the stack.
        let mut popped: Vec<SavedPs> = ps.saved.drain(index..).collect();
        let SavedPs { ps: mut restored, clip_region } = popped.remove(0);
        let before = ps.clip_region;
        // Keep what is not part of the saved state.
        restored.hps = ps.hps;
        restored.hwnd = ps.hwnd;
        restored.hdc = ps.hdc;
        restored.bitmap = ps.bitmap;
        restored.origin = ps.origin;
        restored.area = ps.area.take();
        restored.path = ps.path.take();
        restored.saved = std::mem::take(&mut ps.saved);
        *ps = restored;
        // Put the clip region's saved contents back (in its old handle, or
        // a new one when that was destroyed meanwhile).
        let handle = ps.clip_region;
        if handle != 0 {
            match clip_region {
                Some(content) if wm.regions.get(handle).is_some() => wm.regions.set(handle, (*content).clone()),
                Some(content) => {
                    let h = wm.regions.create((*content).clone()).unwrap_or(0);
                    if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.clip_region = h; }
                }
                None => if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.clip_region = 0; },
            }
        }
        let mut discarded: Vec<u32> = popped.iter().map(|s| s.ps.clip_region).collect();
        discarded.push(before);
        wm.drop_unused_clip_regions(hps, &discarded);
        wm.refresh_clip(hps);
        1
    }

    fn gpi_set_ps(&self, tid: u32, hps: u32, psizl: u32, options: u32) -> u32 {
        let Some((cx, cy)) = self.read_pointl(psizl).filter(|&(cx, cy)| cx >= 0 && cy >= 0)
            else { return self.state_fail(tid, PMERR_INV_PS_SIZE) };
        let units = options & PS_UNITS;
        let format = options & PS_FORMAT;
        if pels_per_unit(units).is_none() || !matches!(format, GPIF_DEFAULT | GPIF_SHORT | GPIF_LONG) {
            return self.state_fail(tid, PMERR_INV_OR_INCOMPAT_OPTIONS);
        }
        let process_cp = self.shared.active_codepage.load(Ordering::Relaxed);
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(ps) = wm.ps_map.get(&hps) else { drop(wm); return self.state_fail(tid, PMERR_INV_HPS) };
        // Type, mode and association stay; units and format change.
        let kept = ps.coords.options & (PS_TYPE | PS_MODE | PS_ASSOCIATE);
        reset_ps(&mut wm, hps, if options & PS_NORESET != 0 { GRES_SEGMENTS } else { GRES_ALL });
        if let Some(ps) = wm.ps_map.get_mut(&hps) {
            ps.coords = CoordState::with_page(kept | units | format, (cx, cy));
            ps.codepage = process_cp;
        }
        wm.refresh_clip(hps);
        1
    }

    fn gpi_set_attrs(&self, tid: u32, hps: u32, prim: u32, mask: u32, defaults: u32, bundle: u32) -> u32 {
        let Some(defined) = defined_bits(prim) else { return self.state_fail(tid, PMERR_INV_PRIMITIVE_TYPE) };
        if mask & !defined != 0 { return self.state_fail(tid, PMERR_UNSUPPORTED_ATTR); }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(ps) = wm.ps_map.get(&hps) else { drop(wm); return self.state_fail(tid, PMERR_INV_HPS) };
        let fresh = PresentationSpace::fresh(0, 0, 0, Arc::clone(&ps.sys_colors));
        // Read and check everything first: a failing call changes nothing.
        let mut changes: Vec<(Attr, Value)> = Vec::new();
        for bit in (0..u32::BITS).map(|i| 1u32 << i).filter(|b| mask & b != 0) {
            let Some((attr, off)) = attr_of(prim, bit) else { continue };
            let at = bundle.wrapping_add(off);
            let value = if defaults & bit != 0 {
                default_value(&fresh, attr)
            } else {
                match self.read_attr(attr, at) {
                    Some(v) => v,
                    None => { drop(wm); return self.state_fail(tid, PMERR_INV_COORDINATE); }
                }
            };
            if let Err(e) = check_value(attr, value) { drop(wm); return self.state_fail(tid, e); }
            changes.push((attr, value));
        }
        let ps = wm.ps_map.get_mut(&hps).expect("checked above");
        for (attr, value) in changes { apply_value(ps, attr, value); }
        1
    }

    fn gpi_query_attrs(&self, tid: u32, hps: u32, prim: u32, mask: u32, bundle: u32) -> u32 {
        let alt_error = GPI_ALTERROR as u32;
        let Some(defined) = defined_bits(prim) else { self.pm_error(tid, PMERR_INV_PRIMITIVE_TYPE); return alt_error };
        if mask & !defined != 0 { self.pm_error(tid, PMERR_UNSUPPORTED_ATTR); return alt_error; }
        let values: Vec<(u32, Attr, u32, Value, bool)> = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(ps) = wm.ps_map.get(&hps) else { drop(wm); self.pm_error(tid, PMERR_INV_HPS); return alt_error };
            let fresh = PresentationSpace::fresh(0, 0, 0, Arc::clone(&ps.sys_colors));
            (0..u32::BITS).map(|i| 1u32 << i).filter(|b| mask & b != 0).filter_map(|bit| {
                let (attr, off) = attr_of(prim, bit)?;
                let v = current_value(ps, attr);
                Some((bit, attr, off, v, v == default_value(&fresh, attr)))
            }).collect()
        };
        // Attributes Warpine does not keep are always at their default.
        let mut def_mask = mask;
        for (bit, attr, off, value, is_default) in values {
            if !is_default { def_mask &= !bit; }
            if !self.write_attr(attr, bundle.wrapping_add(off), value) { return alt_error; }
        }
        def_mask
    }

    fn read_attr(&self, attr: Attr, at: u32) -> Option<Value> {
        Some(match attr {
            Attr::Mix | Attr::BackMix | Attr::LineType | Attr::CharSet | Attr::Pattern
            | Attr::CharMode | Attr::CharDirection | Attr::CharAlign =>
                Value::One(self.guest_read::<u16>(at)? as u32),
            Attr::CharBox | Attr::RefPoint | Attr::CharAngle | Attr::CharShear =>
                Value::Two(self.guest_read::<i32>(at)?, self.guest_read::<i32>(at.wrapping_add(4))?),
            _ => Value::One(self.guest_read::<u32>(at)?),
        })
    }

    fn write_attr(&self, attr: Attr, at: u32, value: Value) -> bool {
        match (attr, value) {
            (Attr::Mix | Attr::BackMix | Attr::LineType | Attr::CharSet | Attr::Pattern
            | Attr::CharMode | Attr::CharDirection | Attr::CharAlign, Value::One(v)) =>
                self.guest_write::<u16>(at, v as u16).is_some(),
            (_, Value::One(v)) => self.guest_write::<u32>(at, v).is_some(),
            (_, Value::Two(a, b)) =>
                self.guest_write::<i32>(at, a).is_some() && self.guest_write::<i32>(at.wrapping_add(4), b).is_some(),
        }
    }
}

/// An attribute value as a bundle holds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Value {
    One(u32),
    Two(i32, i32),
}

fn current_value(ps: &PresentationSpace, attr: Attr) -> Value {
    match attr {
        Attr::Color => Value::One(ps.color_attr),
        Attr::BackColor => Value::One(ps.back_color_attr),
        Attr::Mix => Value::One(ps.mix_mode),
        Attr::BackMix => Value::One(ps.back_mix),
        Attr::LineWidth => Value::One(ps.line_width),
        Attr::GeomWidth => Value::One(ps.line_width_geom as u32),
        Attr::LineType => Value::One(ps.line_type),
        Attr::CharSet => Value::One(ps.char_set),
        Attr::CharBox => Value::Two(fixed_of(ps.char_box.0) as i32, fixed_of(ps.char_box.1) as i32),
        Attr::CharMode => Value::One(ps.text.mode),
        Attr::CharAngle => Value::Two(ps.text.angle.0, ps.text.angle.1),
        Attr::CharShear => Value::Two(ps.text.shear.0, ps.text.shear.1),
        Attr::CharDirection => Value::One(ps.text.direction),
        Attr::CharAlign => Value::One(ps.text.align.0 | ps.text.align.1),
        Attr::CharExtra => Value::One(ps.text.char_extra as u32),
        Attr::CharBreakExtra => Value::One(ps.text.break_extra as u32),
        Attr::Pattern => Value::One(ps.pattern),
        Attr::RefPoint => Value::Two(ps.pattern_ref.0, ps.pattern_ref.1),
    }
}

fn default_value(fresh: &PresentationSpace, attr: Attr) -> Value {
    current_value(fresh, attr)
}

/// Validate a value as the matching `GpiSet*` call would.
fn check_value(attr: Attr, value: Value) -> Result<(), u32> {
    let v = match value {
        // GpiSetCharShear: the vertical strokes cannot run horizontally.
        Value::Two(_, 0) if attr == Attr::CharShear => return Err(PMERR_INV_CHAR_SHEAR_ATTR),
        Value::Two(..) => return Ok(()),
        Value::One(v) => v,
    };
    match attr {
        Attr::CharMode if v > CM_MODE3 => Err(PMERR_INV_CHAR_MODE_ATTR),
        Attr::CharDirection if v > CHDIRN_BOTTOMTOP => Err(PMERR_INV_CHAR_DIRECTION_ATTR),
        Attr::CharAlign if char_align(v).is_none() => Err(PMERR_INV_CHAR_ALIGN_ATTR),
        Attr::Mix if Mix::from_fm(v).is_none() => Err(PMERR_INV_MIX_ATTR),
        Attr::BackMix if Mix::from_fm(v).is_none() && v != BM_SRCTRANSPARENT && v != BM_DESTTRANSPARENT =>
            Err(PMERR_INV_BACKGROUND_MIX_ATTR),
        Attr::LineType if v > LINETYPE_ALTERNATE => Err(PMERR_INV_LINE_TYPE_ATTR),
        Attr::LineWidth | Attr::GeomWidth if (v as i32) < 0 => Err(PMERR_INV_LINE_WIDTH_ATTR),
        Attr::Pattern if super::pm_shapes::pattern_rows(v).is_none() => Err(PMERR_INV_PATTERN_ATTR),
        _ => Ok(()),
    }
}

fn apply_value(ps: &mut PresentationSpace, attr: Attr, value: Value) {
    match (attr, value) {
        (Attr::Color, Value::One(v)) => ps.set_color(v),
        (Attr::BackColor, Value::One(v)) => ps.set_back_color(v),
        (Attr::Mix, Value::One(v)) => ps.mix_mode = v,
        (Attr::BackMix, Value::One(v)) => ps.back_mix = v,
        (Attr::LineWidth, Value::One(v)) => ps.line_width = v,
        (Attr::GeomWidth, Value::One(v)) => ps.line_width_geom = v as i32,
        (Attr::LineType, Value::One(v)) => ps.line_type = v,
        (Attr::CharSet, Value::One(v)) => ps.char_set = v,
        // Like GpiSetCharBox: FIXED cell, whole world units.
        (Attr::CharBox, Value::Two(cx, cy)) => ps.char_box = (cx >> 16, cy >> 16),
        // The *_DEFAULT values select the defaults, as in the GpiSetChar* calls.
        (Attr::CharMode, Value::One(v)) => ps.text.mode = if v == CM_DEFAULT { CM_MODE1 } else { v },
        (Attr::CharAngle, Value::Two(x, y)) =>
            ps.text.angle = if (x, y) == (0, 0) { TextAttrs::default().angle } else { (x, y) },
        (Attr::CharShear, Value::Two(x, y)) => ps.text.shear = (x, y),
        (Attr::CharDirection, Value::One(v)) =>
            ps.text.direction = if v == CHDIRN_DEFAULT { CHDIRN_LEFTRIGHT } else { v },
        (Attr::CharAlign, Value::One(v)) => if let Some(a) = char_align(v) { ps.text.align = a },
        (Attr::CharExtra, Value::One(v)) => ps.text.char_extra = v as i32,
        (Attr::CharBreakExtra, Value::One(v)) => ps.text.break_extra = v as i32,
        (Attr::Pattern, Value::One(v)) => ps.pattern = v,
        (Attr::RefPoint, Value::Two(x, y)) => ps.pattern_ref = (x, y),
        _ => {}
    }
}

/// The `(horizontal, vertical)` `TA_*` pair of a CHARBUNDLE `usTextAlign`
/// (horizontal in the low byte, vertical in the high byte); `None` when
/// either is invalid. 0 (`TA_DEFAULT` in each byte) selects the normal
/// alignment, unverified.
fn char_align(v: u32) -> Option<(u32, u32)> {
    let h = match v & 0xFF { 0 => TA_NORMAL_HORIZ, h => h };
    let vt = match v & 0xFF00 { 0 => TA_NORMAL_VERT, vt => vt };
    ((TA_NORMAL_HORIZ..=TA_STANDARD_HORIZ).contains(&h) && (TA_NORMAL_VERT..=TA_STANDARD_VERT).contains(&vt)
        && v >> 16 == 0).then_some((h, vt))
}

/// `GpiResetPS` at `level` (`GRES_*`) of an existing presentation space.
fn reset_ps(wm: &mut WindowManager, hps: u32, level: u32) {
    let Some(ps) = wm.ps_map.get_mut(&hps) else { return };
    let fresh = PresentationSpace::fresh(ps.hps, ps.hwnd, ps.codepage, Arc::clone(&ps.sys_colors));
    let old = std::mem::replace(ps, fresh);
    // Never reset: the window, DC, bitmap, creator, save stack and page.
    ps.hdc = old.hdc;
    ps.bitmap = old.bitmap;
    ps.origin = old.origin;
    ps.saved = old.saved;
    ps.coords.options = old.coords.options;
    ps.coords.page_size = old.coords.page_size;
    ps.coords.viewport = old.coords.viewport;
    // Open brackets end; a defined path stays (unverified).
    ps.path = old.path.filter(|p| !matches!(p, PathState::Open(_)));
    // GRES_SEGMENTS and GRES_ALL release the clip region: it is the
    // application's again (not destroyed).
    if level == GRES_ATTRS {
        ps.clip_region = old.clip_region;
        ps.coords.default_view = old.coords.default_view;
        ps.coords.viewing = old.coords.viewing;
        ps.coords.graphics_field = old.coords.graphics_field;
    }
    if level != GRES_ALL {
        ps.color_table = old.color_table;
        ps.rgb_mode = old.rgb_mode;
        ps.log_fonts = old.log_fonts;
    }
    ps.refresh_colors();
    wm.refresh_clip(hps);
}

#[cfg(test)]
mod tests {
    use super::super::constants::*;
    use super::super::mutex_ext::MutexExt;
    use super::super::pm_region::testing::*;

    // PMGPI ordinals (doc/os2_ordinals.md).
    const SAVE_PS: u32 = 501;
    const RESTORE_PS: u32 = 499;
    const RESET_PS: u32 = 498;
    const SET_ATTRS: u32 = 588;
    const QUERY_ATTRS: u32 = 583;
    const QUERY_PS: u32 = 471;
    const SET_PS: u32 = 539;
    const SET_COLOR: u32 = 517;
    const QUERY_COLOR: u32 = 437;
    const SET_MIX: u32 = 537;
    const QUERY_MIX: u32 = 467;
    const MOVE: u32 = 404;
    const QUERY_POS: u32 = 441;
    const SET_MODEL: u32 = 538;
    const QUERY_MODEL: u32 = 468;
    const INTERSECT_CLIP: u32 = 396;
    const QUERY_CLIP_BOX: u32 = 435;
    const QUERY_CLIP_REGION: u32 = 436;
    const SET_LINE_TYPE: u32 = 530;
    const QUERY_LINE_TYPE: u32 = 459;
    const BEGIN_AREA: u32 = 352;
    const QUERY_CP: u32 = 440;

    const PT: u32 = 0x5000;
    const BUN: u32 = 0x5100;
    const MAT: u32 = 0x5200;
    const RC: u32 = 0x5300;
    const ONE: i32 = 0x1_0000;

    fn pos(l: &super::Loader, hps: u32) -> (i32, i32) {
        gpi(l, QUERY_POS, &[hps, PT]);
        (l.guest_read::<i32>(PT).unwrap(), l.guest_read::<i32>(PT + 4).unwrap())
    }

    #[test]
    fn save_and_restore_relative_and_absolute() {
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        assert_eq!(gpi(&l, SAVE_PS, &[hps]), 1, "first level");
        gpi(&l, SET_COLOR, &[hps, CLR_BLUE]);
        put_points(&l, PT, &[(7, 8)]);
        gpi(&l, MOVE, &[hps, PT]);
        assert_eq!(gpi(&l, SAVE_PS, &[hps]), 2);
        gpi(&l, SET_COLOR, &[hps, CLR_GREEN]);
        assert_eq!(gpi(&l, SAVE_PS, &[hps]), 3);
        gpi(&l, SET_MIX, &[hps, FM_XOR]);
        // -1: the most recent.
        assert_eq!(gpi(&l, RESTORE_PS, &[hps, -1i32 as u32]), 1);
        assert_eq!(gpi(&l, QUERY_COLOR, &[hps]), CLR_GREEN);
        assert_eq!(gpi(&l, QUERY_MIX, &[hps]), FM_OVERPAINT);
        // Absolute 1 discards level 2 too.
        assert_eq!(gpi(&l, RESTORE_PS, &[hps, 1]), 1);
        assert_eq!(gpi(&l, QUERY_COLOR, &[hps]), CLR_RED);
        assert_eq!(pos(&l, hps), (0, 0));
        assert_eq!(gpi(&l, RESTORE_PS, &[hps, 1]), 0, "nothing left");
        assert_eq!(last_error(&l), PMERR_INV_ID);
        assert_eq!(gpi(&l, SAVE_PS, &[hps]), 1, "the stack is empty again");
        assert_eq!(gpi(&l, RESTORE_PS, &[hps, 0]), 0);
        assert_eq!(last_error(&l), PMERR_INV_ID);
        assert_eq!(gpi(&l, RESTORE_PS, &[hps, -2i32 as u32]), 0, "only one level");
        assert_eq!(gpi(&l, RESTORE_PS, &[hps, 2]), 0);
        assert_eq!(gpi(&l, RESTORE_PS, &[hps, -1i32 as u32]), 1);
        assert_eq!(gpi(&l, SAVE_PS, &[0xBAD]), 0);
        assert_eq!(last_error(&l), PMERR_INV_HPS);
    }

    #[test]
    fn save_restore_covers_transforms_and_bounds_the_stack() {
        let (l, _, hps) = setup();
        assert_eq!(gpi(&l, SAVE_PS, &[hps]), 1);
        for (i, v) in [2 * ONE, 0, 0, 0, 2 * ONE, 0, 0, 0, 1].iter().enumerate() {
            l.guest_write::<i32>(MAT + 4 * i as u32, *v).unwrap();
        }
        gpi(&l, SET_MODEL, &[hps, 9, MAT, TRANSFORM_REPLACE]);
        gpi(&l, RESTORE_PS, &[hps, -1i32 as u32]);
        gpi(&l, QUERY_MODEL, &[hps, 1, MAT]);
        assert_eq!(l.guest_read::<i32>(MAT).unwrap(), ONE, "the model transform came back");
        for i in 0..super::MAX_SAVED_PS { assert_eq!(gpi(&l, SAVE_PS, &[hps]), i as u32 + 1); }
        assert_eq!(gpi(&l, SAVE_PS, &[hps]), 0);
        assert_eq!(last_error(&l), PMERR_INSUFFICIENT_MEMORY);
    }

    /// Save → intersect the clip → restore gives the old clip back, and a
    /// clip region created after the save is destroyed by the restore.
    #[test]
    fn restore_brings_back_the_clip_region() {
        let (l, _, hps) = setup();
        put_rects(&l, RC, &[(0, 0, 99, 49)]);
        assert_ne!(gpi(&l, INTERSECT_CLIP, &[hps, RC]), 0);
        let region = gpi(&l, QUERY_CLIP_REGION, &[hps]);
        assert_ne!(region, 0);
        gpi(&l, SAVE_PS, &[hps]);
        put_rects(&l, RC, &[(10, 10, 19, 19)]);
        gpi(&l, INTERSECT_CLIP, &[hps, RC]);
        gpi(&l, QUERY_CLIP_BOX, &[hps, RC]);
        assert_eq!(get_rect(&l, RC), (10, 10, 19, 19));
        gpi(&l, RESTORE_PS, &[hps, -1i32 as u32]);
        assert_eq!(gpi(&l, QUERY_CLIP_REGION, &[hps]), region, "the same handle");
        gpi(&l, QUERY_CLIP_BOX, &[hps, RC]);
        assert_eq!(get_rect(&l, RC), (0, 0, 99, 49), "with its old contents");

        // No clip region at save time: the one made afterwards goes away.
        let (l, _, hps) = setup();
        gpi(&l, SAVE_PS, &[hps]);
        put_rects(&l, RC, &[(10, 10, 19, 19)]);
        gpi(&l, INTERSECT_CLIP, &[hps, RC]);
        let made = gpi(&l, QUERY_CLIP_REGION, &[hps]);
        gpi(&l, RESTORE_PS, &[hps, 1]);
        assert_eq!(gpi(&l, QUERY_CLIP_REGION, &[hps]), 0);
        assert!(l.shared.window_mgr.lock_or_recover().regions.get(made).is_none(), "destroyed");
    }

    #[test]
    fn reset_levels() {
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        gpi(&l, SET_LINE_TYPE, &[hps, LINETYPE_DOT]);
        put_points(&l, PT, &[(5, 5)]);
        gpi(&l, MOVE, &[hps, PT]);
        for (i, v) in [3 * ONE].iter().enumerate() { l.guest_write::<i32>(MAT + 4 * i as u32, *v).unwrap(); }
        gpi(&l, SET_MODEL, &[hps, 1, MAT, TRANSFORM_REPLACE]);
        put_rects(&l, RC, &[(0, 0, 9, 9)]);
        gpi(&l, INTERSECT_CLIP, &[hps, RC]);
        assert_eq!(gpi(&l, BEGIN_AREA, &[hps, 0]), 1);
        assert_eq!(gpi(&l, RESET_PS, &[hps, GRES_ATTRS]), 1);
        assert_eq!(gpi(&l, QUERY_COLOR, &[hps]), CLR_DEFAULT as u32);
        assert_eq!(gpi(&l, QUERY_LINE_TYPE, &[hps]), LINETYPE_DEFAULT);
        assert_eq!(pos(&l, hps), (0, 0));
        gpi(&l, QUERY_MODEL, &[hps, 1, MAT]);
        assert_eq!(l.guest_read::<i32>(MAT).unwrap(), ONE, "model transform reset");
        assert!(l.shared.window_mgr.lock_or_recover().ps_map[&hps].area.is_none(), "area ended");
        assert_ne!(gpi(&l, QUERY_CLIP_REGION, &[hps]), 0, "GRES_ATTRS keeps the clip region");
        assert_eq!(gpi(&l, RESET_PS, &[hps, GRES_SEGMENTS]), 1);
        assert_eq!(gpi(&l, QUERY_CLIP_REGION, &[hps]), 0, "released");
        assert_eq!(gpi(&l, RESET_PS, &[hps, GRES_ALL]), 1);
        assert_eq!(gpi(&l, RESET_PS, &[hps, 3]), 0);
        assert_eq!(last_error(&l), PMERR_INV_RESET_OPTIONS);
        assert_eq!(gpi(&l, RESET_PS, &[0xBAD, GRES_ALL]), 0);
    }

    #[test]
    fn set_and_query_attribute_bundles() {
        let (l, _, hps) = setup();
        // LINEBUNDLE: colour, mix, type.
        l.guest_write::<u32>(BUN, CLR_BLUE).unwrap();
        l.guest_write::<u16>(BUN + 8, FM_XOR as u16).unwrap();
        l.guest_write::<u16>(BUN + 20, LINETYPE_DASHDOT as u16).unwrap();
        let mask = LBB_COLOR | LBB_MIX_MODE | LBB_TYPE;
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_LINE, mask, 0, BUN]), 1);
        assert_eq!(gpi(&l, QUERY_COLOR, &[hps]), CLR_BLUE);
        assert_eq!(gpi(&l, QUERY_MIX, &[hps]), FM_XOR);
        assert_eq!(gpi(&l, QUERY_LINE_TYPE, &[hps]), LINETYPE_DASHDOT);
        // Query: none of them default, the background colour is.
        for i in 0..8 { l.guest_write::<u32>(BUN + 4 * i, 0).unwrap(); }
        let defs = gpi(&l, QUERY_ATTRS, &[hps, PRIM_LINE, mask | LBB_BACK_COLOR | LBB_END, BUN]);
        assert_eq!(defs, LBB_BACK_COLOR | LBB_END);
        assert_eq!(l.guest_read::<u32>(BUN).unwrap(), CLR_BLUE);
        assert_eq!(l.guest_read::<u16>(BUN + 20).unwrap(), LINETYPE_DASHDOT as u16);
        // The default mask resets without reading the bundle.
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_LINE, LBB_COLOR | LBB_TYPE, LBB_COLOR | LBB_TYPE, 0]), 1);
        assert_eq!(gpi(&l, QUERY_COLOR, &[hps]), CLR_DEFAULT as u32);
        assert_eq!(gpi(&l, QUERY_LINE_TYPE, &[hps]), LINETYPE_DEFAULT);
        // AREABUNDLE: pattern and reference point.
        l.guest_write::<u16>(BUN + 14, PATSYM_HALFTONE as u16).unwrap();
        put_points(&l, BUN + 16, &[(3, 4)]);
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_AREA, ABB_SYMBOL | ABB_REF_POINT, 0, BUN]), 1);
        {
            let wm = l.shared.window_mgr.lock_or_recover();
            assert_eq!((wm.ps_map[&hps].pattern, wm.ps_map[&hps].pattern_ref), (PATSYM_HALFTONE, (3, 4)));
        }
        // CHARBUNDLE: set and box.
        l.guest_write::<u16>(BUN + 12, 5).unwrap();
        put_points(&l, BUN + 16, &[(20 * ONE, 30 * ONE)]);
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_CHAR, CBB_SET | CBB_BOX | CBB_ANGLE, 0, BUN]), 1);
        assert_eq!(gpi(&l, QUERY_ATTRS, &[hps, PRIM_CHAR, CBB_BOX, BUN]), 0);
        assert_eq!(l.guest_read::<i32>(BUN + 20).unwrap(), 30 * ONE);
        // Errors leave everything unchanged.
        l.guest_write::<u16>(BUN + 8, 99).unwrap();
        l.guest_write::<u32>(BUN, CLR_RED).unwrap();
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_IMAGE, IBB_COLOR | IBB_MIX_MODE, 0, BUN]), 0);
        assert_eq!(last_error(&l), PMERR_INV_MIX_ATTR);
        assert_eq!(gpi(&l, QUERY_COLOR, &[hps]), CLR_DEFAULT as u32, "nothing was set");
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, 9, 1, 0, BUN]), 0);
        assert_eq!(last_error(&l), PMERR_INV_PRIMITIVE_TYPE);
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_IMAGE, 0x10, 0, BUN]), 0);
        assert_eq!(last_error(&l), PMERR_UNSUPPORTED_ATTR);
        assert_eq!(gpi(&l, QUERY_ATTRS, &[hps, 0, 1, BUN]), GPI_ALTERROR as u32);
        assert_eq!(gpi(&l, QUERY_ATTRS, &[0xBAD, PRIM_LINE, 1, BUN]), GPI_ALTERROR as u32);
        // A bundle off the end of guest memory.
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_LINE, LBB_COLOR, 0, u32::MAX - 1]), 0);
    }

    #[test]
    fn character_bundle_text_attributes_both_ways() {
        use super::super::pm_text::*;
        let (l, _, hps) = setup();
        let text = |l: &super::super::Loader| l.shared.window_mgr.lock_or_recover().ps_map[&hps].text;
        let all = CBB_MODE | CBB_ANGLE | CBB_SHEAR | CBB_DIRECTION | CBB_TEXT_ALIGN | CBB_EXTRA | CBB_BREAK_EXTRA;
        // A fresh presentation space reports every one as default.
        for i in 0..13 { l.guest_write::<u32>(BUN + 4 * i, 0xDEAD_BEEF).unwrap(); }
        assert_eq!(gpi(&l, QUERY_ATTRS, &[hps, PRIM_CHAR, all, BUN]), all);
        assert_eq!(l.guest_read::<u16>(BUN + 14).unwrap(), CM_MODE1 as u16);
        assert_eq!(l.guest_read::<i32>(BUN + 24).unwrap(), 1);
        assert_eq!(l.guest_read::<i32>(BUN + 28).unwrap(), 0);
        assert_eq!(l.guest_read::<i32>(BUN + 36).unwrap(), 1, "shear (0, 1)");
        assert_eq!(l.guest_read::<u16>(BUN + 40).unwrap(), CHDIRN_LEFTRIGHT as u16);
        assert_eq!(l.guest_read::<u16>(BUN + 42).unwrap(), (TA_NORMAL_HORIZ | TA_NORMAL_VERT) as u16);
        assert_eq!(l.guest_read::<i32>(BUN + 44).unwrap(), 0);
        // Set every one.
        l.guest_write::<u16>(BUN + 14, CM_MODE3 as u16).unwrap();
        put_points(&l, BUN + 24, &[(1, 1), (1, 2)]);
        l.guest_write::<u16>(BUN + 40, CHDIRN_TOPBOTTOM as u16).unwrap();
        l.guest_write::<u16>(BUN + 42, (TA_CENTER | TA_HALF) as u16).unwrap();
        l.guest_write::<i32>(BUN + 44, 3 << 16).unwrap();
        l.guest_write::<i32>(BUN + 48, -(2 << 16)).unwrap();
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_CHAR, all, 0, BUN]), 1);
        let t = text(&l);
        assert_eq!((t.mode, t.angle, t.shear, t.direction), (CM_MODE3, (1, 1), (1, 2), CHDIRN_TOPBOTTOM));
        assert_eq!((t.align, t.char_extra, t.break_extra), ((TA_CENTER, TA_HALF), 3 << 16, -(2 << 16)));
        // The query reads them back and no bit is default.
        for i in 0..13 { l.guest_write::<u32>(BUN + 4 * i, 0).unwrap(); }
        assert_eq!(gpi(&l, QUERY_ATTRS, &[hps, PRIM_CHAR, all, BUN]), 0);
        assert_eq!(l.guest_read::<u16>(BUN + 14).unwrap(), CM_MODE3 as u16);
        assert_eq!((l.guest_read::<i32>(BUN + 24).unwrap(), l.guest_read::<i32>(BUN + 36).unwrap()), (1, 2));
        assert_eq!(l.guest_read::<u16>(BUN + 42).unwrap(), (TA_CENTER | TA_HALF) as u16);
        assert_eq!(l.guest_read::<i32>(BUN + 48).unwrap(), -(2 << 16));
        // The single-attribute calls see the bundle's values.
        assert_eq!(gpi(&l, 431, &[hps]), CHDIRN_TOPBOTTOM, "GpiQueryCharDirection");
        assert_eq!(gpi(&l, 432, &[hps]), CM_MODE3, "GpiQueryCharMode");
        // Invalid values fail as the GpiSetChar* calls do, changing nothing.
        let bad: [(u32, u32, u32, u32); 4] = [
            (CBB_SHEAR, 36, 0, PMERR_INV_CHAR_SHEAR_ATTR),
            (CBB_MODE, 14, 4, PMERR_INV_CHAR_MODE_ATTR),
            (CBB_DIRECTION, 40, 5, PMERR_INV_CHAR_DIRECTION_ATTR),
            (CBB_TEXT_ALIGN, 42, 0x0706, PMERR_INV_CHAR_ALIGN_ATTR),
        ];
        for (bit, off, v, err) in bad {
            let before = text(&l);
            if bit == CBB_SHEAR { l.guest_write::<i32>(BUN + off, v as i32).unwrap(); }
            else { l.guest_write::<u16>(BUN + off, v as u16).unwrap(); }
            assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_CHAR, bit | CBB_EXTRA, 0, BUN]), 0);
            assert_eq!(last_error(&l), err);
            assert_eq!(text(&l), before);
        }
        // Zero values select the defaults; the default mask resets.
        for i in 0..13 { l.guest_write::<u32>(BUN + 4 * i, 0).unwrap(); }
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_CHAR, CBB_MODE | CBB_ANGLE | CBB_DIRECTION | CBB_TEXT_ALIGN, 0, BUN]), 1);
        let t = text(&l);
        assert_eq!((t.mode, t.angle, t.direction, t.align), (CM_MODE1, (1, 0), CHDIRN_LEFTRIGHT, (TA_NORMAL_HORIZ, TA_NORMAL_VERT)));
        assert_eq!(gpi(&l, SET_ATTRS, &[hps, PRIM_CHAR, all, all, 0]), 1);
        assert_eq!(text(&l), TextAttrs::default());
    }

    #[test]
    fn query_and_set_ps() {
        let (l, _, hps) = setup();
        let (dw, dh) = desktop_size();
        assert_eq!(gpi(&l, QUERY_PS, &[hps, PT]), PU_PELS);
        assert_eq!((l.guest_read::<i32>(PT).unwrap(), l.guest_read::<i32>(PT + 4).unwrap()), (dw as i32, dh as i32));
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        put_points(&l, PT, &[(2000, 1000)]);
        assert_eq!(gpi(&l, SET_PS, &[hps, PT, PU_LOMETRIC | GPIF_LONG]), 1);
        assert_eq!(gpi(&l, QUERY_COLOR, &[hps]), CLR_DEFAULT as u32, "reset");
        assert_eq!(gpi(&l, QUERY_PS, &[hps, PT]), PU_LOMETRIC | GPIF_LONG);
        assert_eq!(l.guest_read::<i32>(PT).unwrap(), 2000);
        assert_eq!(gpi(&l, QUERY_CP, &[hps]), 437, "the process codepage");
        put_points(&l, PT, &[(-1, 0)]);
        assert_eq!(gpi(&l, SET_PS, &[hps, PT, PU_PELS]), 0);
        assert_eq!(last_error(&l), PMERR_INV_PS_SIZE);
        put_points(&l, PT, &[(0, 0)]);
        assert_eq!(gpi(&l, SET_PS, &[hps, PT, 0x20]), 0);
        assert_eq!(last_error(&l), PMERR_INV_OR_INCOMPAT_OPTIONS);
        assert_eq!(gpi(&l, QUERY_PS, &[0xBAD, PT]), 0);
    }
}
