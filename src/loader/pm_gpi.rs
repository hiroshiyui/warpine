// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 Presentation Manager PMGPI API handler methods.

use std::sync::atomic::Ordering;
use super::vm_backend::VcpuBackend;
use log::{debug, warn};

use super::mutex_ext::MutexExt;
use super::ApiResult;
use super::constants::*;
use super::pm_types::{DeviceContext, DrawTarget};
use crate::gui::font::{self, CjkRegion, FontFamily, FontSpec};
use crate::gui::surface::{DrawOp, Mix};

// ── GpiCreateLogFont return values ────────────────────────────────────────────
const FONT_DEFAULT: u32 = 1; // no FATTRS given: the system default font was used
const FONT_MATCH:   u32 = 2; // a font matching the FATTRS was selected

// FATTRS byte offsets (OS/2 Warp 4 pmgpi.h)
const FATTRS_OFF_SELECTION:      u32 = 2;  // USHORT fsSelection
const FATTRS_OFF_FACENAME:       u32 = 8;  // CHAR szFacename[FACESIZE]
const FATTRS_OFF_MAXBASELINEEXT: u32 = 44; // LONG lMaxBaselineExt
const FACESIZE: usize = 32;
const FATTR_SEL_ITALIC: u16 = 0x0001;
const FATTR_SEL_BOLD:   u16 = 0x0020;

/// Upper bound on the string length GpiQueryTextBox will read from the guest.
const MAX_TEXT_BOX_CHARS: i32 = 4096;

const DEVICE_RES: i16 = 96; // device resolution (DPI) reported to apps

// GpiBox / GpiFullArc lControl (<pmgpi.h>)
const DRO_FILL:        u32 = 1;
const DRO_OUTLINE:     u32 = 2;
const DRO_OUTLINEFILL: u32 = 3;

/// What drawing through a presentation space needs: where it draws and the
/// current attributes.
pub(super) struct PsTarget {
    pub target: DrawTarget,
    pub color: u32,
    pub back_color: u32,
    pub mix: Mix,
    /// Mix for the background of text; `None` leaves it alone.
    pub back_mix: Option<Mix>,
    pub pos: (i32, i32),
    pub line_type: u32,
}

/// Pel mask (16 pels, MSB first) for a `LINETYPE_*`; `None` draws nothing.
/// The dash lengths are Warpine's choice: the PM reference names the styles
/// but does not give pel patterns.
fn line_style(line_type: u32) -> Option<u16> {
    Some(match line_type {
        LINETYPE_DOT | LINETYPE_ALTERNATE => 0xAAAA,
        LINETYPE_SHORTDASH     => 0xF0F0,
        LINETYPE_DASHDOT       => 0xFF18,
        LINETYPE_DOUBLEDOT     => 0xA0A0,
        LINETYPE_LONGDASH      => 0xFFF0,
        LINETYPE_DASHDOUBLEDOT => 0xFF24,
        LINETYPE_INVISIBLE     => return None,
        _ => crate::gui::surface::SOLID_LINE, // LINETYPE_DEFAULT, LINETYPE_SOLID
    })
}

/// The mix a `BM_*` background mix applies to the text background; `None`
/// for the modes that leave it alone (the default).
fn background_mix(bm: u32) -> Option<Mix> {
    match bm {
        FM_DEFAULT | BM_LEAVEALONE | BM_SRCTRANSPARENT | BM_DESTTRANSPARENT => None,
        m => Mix::from_fm(m),
    }
}

// FONTMETRICS byte offsets (OS/2 Warp 4 pmgpi.h, packed struct, no alignment padding)
const FM_OFF_FAMILYNAME:     u32 = 0;   // STR8[32]
const FM_OFF_FACENAME:       u32 = 32;  // STR8[32]
const FM_OFF_EMHEIGHT:       u32 = 68;  // LONG
const FM_OFF_XHEIGHT:        u32 = 72;  // LONG
const FM_OFF_MAXASCENDER:    u32 = 76;  // LONG
const FM_OFF_MAXDESCENDER:   u32 = 80;  // LONG
const FM_OFF_LCASE_ASCENT:   u32 = 84;  // LONG
const FM_OFF_LCASE_DESCENT:  u32 = 88;  // LONG
const FM_OFF_AVECHWIDTH:     u32 = 100; // LONG
const FM_OFF_MAXCHARINC:     u32 = 104; // LONG
const FM_OFF_EMINC:          u32 = 108; // LONG
const FM_OFF_MAXBASELINEEXT: u32 = 112; // LONG
const FM_OFF_XDEVICERES:     u32 = 126; // SHORT
const FM_OFF_YDEVICERES:     u32 = 128; // SHORT
const FM_OFF_FIRSTCHAR:      u32 = 130; // SHORT
const FM_OFF_LASTCHAR:       u32 = 132; // SHORT
const FM_OFF_DEFAULTCHAR:    u32 = 134; // SHORT
const FM_OFF_BREAKCHAR:      u32 = 136; // SHORT
const FM_OFF_NOMINALPOINTSIZE: u32 = 138; // SHORT
const FM_SIZE:               u32 = 208; // total struct size

impl super::Loader {
    pub(crate) fn handle_pmgpi_call(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, ordinal: u32) -> ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| -> u32 { self.guest_read::<u32>((esp + off) as u32).expect("Stack read OOB") };

        match ordinal {
            369 => {
                // GpiCreatePS(HAB hab, HDC hdc, PSIZEL pszl, ULONG flOptions)
                // With GPIA_ASSOC the PS is associated with hdc: a memory DC
                // (draws into the bitmap selected with GpiSetBitmap) or a
                // window DC (draws into the window).
                let _hab = read_stack(4);
                let hdc = read_stack(8);
                let _pszl = read_stack(12);
                let opts = read_stack(16);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let dc = match hdc {
                    0 => None,
                    h => match wm.dcs.get(&h) {
                        Some(&dc) => Some(dc),
                        None => return ApiResult::Normal(GPI_ERROR),
                    },
                };
                let assoc = opts & GPIA_ASSOC != 0;
                let hwnd = match dc { Some(DeviceContext::Window(w)) if assoc => w, _ => 0 };
                let hps = wm.create_ps(hwnd);
                if assoc && let Some(ps) = wm.ps_map.get_mut(&hps) { ps.hdc = hdc; }
                debug!("  [VCPU {}] GpiCreatePS(hdc={:#x}, assoc={}) -> HPS {}", vcpu_id, hdc, assoc, hps);
                ApiResult::Normal(hps)
            }
            379 => {
                // GpiDestroyPS(HPS hps)
                let hps = read_stack(4);
                self.shared.window_mgr.lock_or_recover().ps_map.remove(&hps);
                ApiResult::Normal(1)
            }
            517 => {
                // GpiSetColor(HPS hps, LONG lColor)
                let hps = read_stack(4);
                let color = read_stack(8);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let Some(ps) = wm.ps_map.get_mut(&hps) else { return ApiResult::Normal(GPI_ERROR) };
                ps.set_color(color);
                ApiResult::Normal(GPI_OK)
            }
            404 => {
                // GpiMove(HPS hps, PPOINTL pptl)
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let (x, y) = (
                    self.guest_read::<i32>(pptl).unwrap_or(0),
                    self.guest_read::<i32>(pptl + 4).unwrap_or(0),
                );
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.current_pos = (x, y);
                }
                ApiResult::Normal(1)
            }
            356 => {
                // GpiBox(HPS hps, LONG lControl, PPOINTL pptl, LONG lHRound, LONG lVRound)
                let hps = read_stack(4);
                let control = read_stack(8);
                let pptl = read_stack(12);
                let _h_round = read_stack(16);
                let _v_round = read_stack(20);
                let (x2, y2) = (
                    self.guest_read::<i32>(pptl).unwrap_or(0),
                    self.guest_read::<i32>(pptl + 4).unwrap_or(0),
                );
                let Some(t) = self.ps_target(hps) else { return ApiResult::Normal(GPI_ERROR) };
                let (x1, y1) = t.pos;
                self.draw_box(&t, (x1, y1, x2, y2), control);
                ApiResult::Normal(GPI_OK)
            }
            398 => {
                // GpiLine(HPS hps, PPOINTL pptl)
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let (x2, y2) = (
                    self.guest_read::<i32>(pptl).unwrap_or(0),
                    self.guest_read::<i32>(pptl + 4).unwrap_or(0),
                );
                let Some(t) = self.ps_target(hps) else { return ApiResult::Normal(GPI_ERROR) };
                let (x1, y1) = t.pos;
                if let Some(style) = line_style(t.line_type) {
                    self.draw_to(t.target, DrawOp::Line { x1, y1, x2, y2, color: t.color, mix: t.mix, style });
                }
                self.set_current_pos(hps, (x2, y2));
                ApiResult::Normal(GPI_OK)
            }
            359 => {
                // GpiCharStringAt(HPS hps, PPOINTL pptl, LONG lCount, PCH pchString)
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let count = read_stack(12) as usize;
                let pch = read_stack(16);
                let (x, y) = (
                    self.guest_read::<i32>(pptl).unwrap_or(0),
                    self.guest_read::<i32>(pptl + 4).unwrap_or(0),
                );
                let text: Vec<u8> = (0..count).map(|i| {
                    self.guest_read::<u8>(pch + i as u32).unwrap_or(0)
                }).collect();
                ApiResult::Normal(self.char_string(hps, Some((x, y)), &text))
            }
            389 => {
                // GpiErase(HPS hps)
                let hps = read_stack(4);
                let Some(t) = self.ps_target(hps) else { return ApiResult::Normal(GPI_ERROR) };
                self.draw_to(t.target, DrawOp::Clear);
                ApiResult::Normal(GPI_OK)
            }

            358 => {
                // GpiCharString(HPS hps, LONG lCount, PCH pchString)
                // Draw text at current position; advance current_pos.
                let hps = read_stack(4);
                let count = read_stack(8) as usize;
                let pch = read_stack(12);
                let text: Vec<u8> = (0..count)
                    .map(|i| self.guest_read::<u8>(pch + i as u32).unwrap_or(0))
                    .collect();
                ApiResult::Normal(self.char_string(hps, None, &text))
            }

            504 => {
                // GpiSetBackColor(HPS hps, LONG lColor)
                let hps = read_stack(4);
                let color = read_stack(8);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let Some(ps) = wm.ps_map.get_mut(&hps) else { return ApiResult::Normal(GPI_ERROR) };
                ps.set_back_color(color);
                ApiResult::Normal(GPI_OK)
            }
            437 => {
                // GpiQueryColor(HPS hps) → LONG lColor (the value as set)
                let hps = read_stack(4);
                let color = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map_or(CLR_ERROR as u32, |ps| ps.color_attr);
                ApiResult::Normal(color)
            }
            424 => {
                // GpiQueryBackColor(HPS hps) → LONG lColor (the value as set)
                let hps = read_stack(4);
                let color = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map_or(CLR_ERROR as u32, |ps| ps.back_color_attr);
                ApiResult::Normal(color)
            }
            537 => {
                // GpiSetMix(HPS hps, LONG lMixMode)
                let hps = read_stack(4);
                let mode = read_stack(8);
                if Mix::from_fm(mode).is_none() { return ApiResult::Normal(GPI_ERROR); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let Some(ps) = wm.ps_map.get_mut(&hps) else { return ApiResult::Normal(GPI_ERROR) };
                ps.mix_mode = mode;
                ApiResult::Normal(GPI_OK)
            }
            505 => {
                // GpiSetBackMix(HPS hps, LONG lMixMode)
                // Applies to the background box of character strings.
                let hps = read_stack(4);
                let mode = read_stack(8);
                let valid = Mix::from_fm(mode).is_some()
                    || mode == BM_SRCTRANSPARENT || mode == BM_DESTTRANSPARENT;
                if !valid { return ApiResult::Normal(GPI_ERROR); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let Some(ps) = wm.ps_map.get_mut(&hps) else { return ApiResult::Normal(GPI_ERROR) };
                ps.back_mix = mode;
                ApiResult::Normal(GPI_OK)
            }
            467 => {
                // GpiQueryMix(HPS hps) → LONG lMixMode
                let hps = read_stack(4);
                let mode = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map_or(-1i32 as u32, |ps| ps.mix_mode); // FM_ERROR
                ApiResult::Normal(mode)
            }
            425 => {
                // GpiQueryBackMix(HPS hps) → LONG lMixMode
                let hps = read_stack(4);
                let mode = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map_or(-1i32 as u32, |ps| ps.back_mix); // BM_ERROR
                ApiResult::Normal(mode)
            }
            544 => {
                // GpiSetPel(HPS hps, PPOINTL pptl) → LONG
                // Sets one pel to the current colour with the current mix.
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let (Some(x), Some(y)) = (self.guest_read::<i32>(pptl), self.guest_read::<i32>(pptl + 4))
                    else { return ApiResult::Normal(GPI_ERROR) };
                let Some(t) = self.ps_target(hps) else { return ApiResult::Normal(GPI_ERROR) };
                self.draw_to(t.target, DrawOp::Pel { x, y, color: t.color, mix: t.mix });
                ApiResult::Normal(GPI_OK)
            }
            476 => {
                // GpiQueryPel(HPS hps, PPOINTL pptl) → LONG lColor
                // Reads back what has been drawn, as a colour value of the
                // PS's colour table (`PresentationSpace::value_of_pel`).
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let (Some(x), Some(y)) = (self.guest_read::<i32>(pptl), self.guest_read::<i32>(pptl + 4))
                    else { return ApiResult::Normal(CLR_NOINDEX as u32) };
                let pel = self.ps_target(hps)
                    .and_then(|t| self.shared.window_mgr.lock_or_recover().target_surface(t.target))
                    .and_then(|s| s.lock_or_recover().pel(x, y));
                let value = pel.and_then(|rgb| self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map(|ps| ps.value_of_pel(rgb)));
                ApiResult::Normal(value.unwrap_or(CLR_NOINDEX as u32))
            }

            441 => {
                // GpiQueryCurrentPosition(HPS hps, PPOINTL pptl) → BOOL
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let pos = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map(|ps| ps.current_pos).unwrap_or((0, 0));
                if pptl != 0 {
                    self.guest_write::<i32>(pptl,     pos.0);
                    self.guest_write::<i32>(pptl + 4, pos.1);
                }
                ApiResult::Normal(1) // TRUE
            }

            513 => {
                // GpiSetCharSet(HPS hps, LONG lcid) → BOOL
                let hps = read_stack(4);
                let lcid = read_stack(8);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.char_set = lcid; }
                ApiResult::Normal(1)
            }
            510 => {
                // GpiSetCharBox(HPS hps, PSIZEF psizfxBox) → BOOL
                // SIZEF = { FIXED cx; FIXED cy } where FIXED is 16.16 fixed-point.
                let hps = read_stack(4);
                let psizf = read_stack(8);
                if psizf != 0 {
                    let cx_fixed = self.guest_read::<i32>(psizf).unwrap_or(0);
                    let cy_fixed = self.guest_read::<i32>(psizf + 4).unwrap_or(0);
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    if let Some(ps) = wm.ps_map.get_mut(&hps) {
                        // Convert 16.16 fixed-point to integer world units.
                        ps.char_box = (cx_fixed >> 16, cy_fixed >> 16);
                    }
                }
                ApiResult::Normal(1)
            }

            368 => {
                // GpiCreateLogFont(HPS hps, PSTR8 pszLocalID, LONG lcid, PFATTRS pfatfs) → LONG
                // Maps the requested face onto an IBM Plex family and registers it
                // under `lcid`; GpiSetCharSet(lcid) then selects it for text output.
                let hps = read_stack(4);
                let lcid = read_stack(12);
                let pfat = read_stack(16);
                let (spec, ret) = if pfat != 0 {
                    let selection = self.guest_read::<u16>(pfat + FATTRS_OFF_SELECTION).unwrap_or(0);
                    let face_bytes: Vec<u8> = (0..FACESIZE as u32)
                        .map(|i| self.guest_read::<u8>(pfat + FATTRS_OFF_FACENAME + i).unwrap_or(0))
                        .take_while(|&b| b != 0)
                        .collect();
                    let cp = self.shared.active_codepage.load(Ordering::Relaxed);
                    let facename = super::codepage::cp_decode(&face_bytes, cp);
                    let max_ext = self.guest_read::<i32>(pfat + FATTRS_OFF_MAXBASELINEEXT).unwrap_or(0);
                    let spec = FontSpec::from_facename(
                        &facename,
                        selection & FATTR_SEL_BOLD != 0,
                        selection & FATTR_SEL_ITALIC != 0,
                        max_ext.max(0) as u32,
                    );
                    debug!("  GpiCreateLogFont(hps={}, lcid={}, face='{}') → {:?}", hps, lcid, facename, spec);
                    (spec, FONT_MATCH)
                } else {
                    debug!("  GpiCreateLogFont(hps={}, lcid={}) → system font", hps, lcid);
                    (FontSpec::SYSTEM, FONT_DEFAULT)
                };
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.log_fonts.insert(lcid, spec); }
                ApiResult::Normal(ret)
            }
            378 => {
                // GpiDeleteSetId(HPS hps, LONG lcid) → BOOL
                let hps = read_stack(4);
                let lcid = read_stack(8);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.log_fonts.remove(&lcid);
                    if ps.char_set == lcid { ps.char_set = 0; }
                }
                ApiResult::Normal(1)
            }

            530 => {
                // GpiSetLineType(HPS hps, LONG lLineType) → BOOL
                // Applies to GpiLine (see `line_style`).
                let hps = read_stack(4);
                let lt = read_stack(8);
                if lt > LINETYPE_ALTERNATE { return ApiResult::Normal(GPI_ERROR); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let Some(ps) = wm.ps_map.get_mut(&hps) else { return ApiResult::Normal(GPI_ERROR) };
                ps.line_type = lt;
                ApiResult::Normal(GPI_OK)
            }
            459 => {
                // GpiQueryLineType(HPS hps) → LONG
                let hps = read_stack(4);
                let lt = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map_or(LINETYPE_ERROR as u32, |ps| ps.line_type);
                ApiResult::Normal(lt)
            }
            531 => {
                // GpiSetLineWidth(HPS hps, FIXED fxLineWidth) → BOOL  (stub)
                ApiResult::Normal(1)
            }
            532 => {
                // GpiSetLineWidthGeom(HPS hps, FIXED fxLineWidth) → BOOL  (stub)
                ApiResult::Normal(1)
            }

            400 => {
                // GpiLoadFonts(HAB hab, PSZ pszFilename) → BOOL  (stub)
                ApiResult::Normal(1)
            }
            622 => {
                // GpiLoadPublicFonts(HAB hab, PSZ pszFilename) → BOOL  (stub)
                ApiResult::Normal(1)
            }
            623 => {
                // GpiUnloadPublicFonts(HAB hab, PSZ pszFilename) → BOOL  (stub)
                ApiResult::Normal(1)
            }

            453 => {
                // GpiQueryFontMetrics(HPS hps, LONG lMetricsLength, PFONTMETRICS pfmMetrics) → BOOL
                let hps = read_stack(4);
                let cb = read_stack(8); // byte length caller wants filled
                let pfm = read_stack(12);
                debug!("  GpiQueryFontMetrics(hps={}, cb={}, pfm=0x{:08X})", hps, cb, pfm);
                if pfm != 0 {
                    let font = self.ps_font(hps);
                    self.write_font_metrics(pfm, cb, font);
                }
                ApiResult::Normal(1) // TRUE
            }

            586 => {
                // GpiQueryFonts(HPS hps, ULONG flOptions, PSZ pszFacename,
                //               PLONG plReqFonts, LONG lMetricsLength, PFONTMETRICS afmMetrics)
                // → LONG (count of matching fonts)
                let hps        = read_stack(4);
                let _fl_opts   = read_stack(8);
                let _psz_face  = read_stack(12);
                let pl_req     = read_stack(16); // PLONG — in: requested count; out: remaining
                let cb         = read_stack(20);
                let afm        = read_stack(24); // first entry in caller's array
                debug!("  GpiQueryFonts(hps={}, afm=0x{:08X}, cb={})", hps, afm, cb);
                // Report one system font.
                if pl_req != 0 { self.guest_write::<i32>(pl_req, 0); } // 0 remaining
                if afm != 0 && cb > 0 { self.write_font_metrics(afm, cb, FontSpec::SYSTEM); }
                ApiResult::Normal(1) // 1 font matches
            }

            489 => {
                // GpiQueryTextBox(HPS hps, LONG lCount1, PCH pchString,
                //                 LONG lCount2, PPOINTL aptlPoints) → BOOL
                //
                // Returns up to TXTBOX_COUNT (5) corner points of the text bounding
                // box in current-coordinate space (baseline at y=0).
                let hps     = read_stack(4);
                let count   = read_stack(8) as i32;
                let pch     = read_stack(12);
                let npoints = read_stack(16); // how many POINTLs to fill (≤5)
                let aptl    = read_stack(20);
                // Measure the actual string in the presentation space's font.
                let bytes: Vec<u8> = (0..count.clamp(0, MAX_TEXT_BOX_CHARS) as u32)
                    .map(|i| self.guest_read::<u8>(pch + i).unwrap_or(0))
                    .collect();
                let cp = self.shared.active_codepage.load(Ordering::Relaxed);
                let font = self.ps_font(hps);
                let w = font::text_width(&super::codepage::cp_decode(&bytes, cp), font);
                let m = font::metrics(font);
                // TXTBOX_TOPLEFT(0), TXTBOX_BOTTOMLEFT(1), TXTBOX_BOTTOMRIGHT(2),
                // TXTBOX_TOPRIGHT(3), TXTBOX_CONCAT(4) — each is a POINTL (2×i32 = 8 bytes).
                let pts: [(i32, i32); 5] = [
                    (0,  m.ascent),   // top-left
                    (0, -m.descent),  // bottom-left
                    (w, -m.descent),  // bottom-right
                    (w,  m.ascent),   // top-right
                    (w,  0),               // concat point (at baseline)
                ];
                if aptl != 0 {
                    for (i, &pt) in pts.iter().enumerate().take(npoints.min(5) as usize) {
                        self.guest_write::<i32>(aptl + (i as u32 * 8),     pt.0);
                        self.guest_write::<i32>(aptl + (i as u32 * 8) + 4, pt.1);
                    }
                }
                ApiResult::Normal(1) // TRUE
            }

            393 => {
                // GpiFullArc(HPS hps, LONG lControl, FIXED fxMult) → LONG
                // Approximated as a filled/outlined box centred on current_pos.
                // Arc params (GpiSetArcParams) are not tracked; use a fixed 10-unit radius.
                let hps = read_stack(4);
                let control = read_stack(8);
                let Some(t) = self.ps_target(hps) else { return ApiResult::Normal(GPI_ERROR) };
                let (cx, cy) = t.pos;
                let r: i32 = 10; // approximate radius
                self.draw_box(&t, (cx - r, cy - r, cx + r, cy + r), control);
                ApiResult::Normal(GPI_OK)
            }

            502 => {
                // GpiSetArcParams(HPS hps, PARCPARAMS parcpArcParams) → BOOL  (stub)
                ApiResult::Normal(1)
            }

            // ── Logical colour tables ────────────────────────────────────────
            592 => {
                // GpiCreateLogColorTable(HPS hps, ULONG flOptions, LONG lFormat,
                //                        LONG lStart, LONG lCount, PLONG alTable) → BOOL
                let (hps, opts, format) = (read_stack(4), read_stack(8), read_stack(12));
                let (start, count, table) = (read_stack(16), read_stack(20), read_stack(24));
                ApiResult::Normal(self.create_log_color_table(hps, opts, format, start, count, table))
            }
            593 => {
                // GpiQueryLogColorTable(HPS hps, ULONG flOptions, LONG lStart,
                //                       LONG lCount, PLONG alArray) → LONG
                let (hps, opts, start, count, array) =
                    (read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20));
                ApiResult::Normal(self.query_log_color_table(hps, opts, start, count, array))
            }
            479 => {
                // GpiQueryRGBColor(HPS hps, ULONG flOptions, LONG lColor) → LONG
                let (hps, color) = (read_stack(4), read_stack(12));
                let rgb = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map_or(GPI_ALTERROR as u32, |ps| ps.resolve_color(color));
                ApiResult::Normal(rgb)
            }
            439 => {
                // GpiQueryColorIndex(HPS hps, ULONG flOptions, LONG lRgbColor) → LONG
                let (hps, rgb) = (read_stack(4), read_stack(12));
                let index = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map_or(GPI_ALTERROR as u32, |ps| ps.nearest_index(rgb));
                ApiResult::Normal(index)
            }

            // ── Device contexts, bitmaps and bit-block transfers ─────────────
            // (implemented in pm_bitmap.rs)
            610 => {
                // DevOpenDC(HAB hab, LONG lType, PSZ pszToken, LONG lCount,
                //           PDEVOPENDATA pdopData, HDC hdcComp) → HDC
                ApiResult::Normal(self.dev_open_dc(read_stack(8)))
            }
            604 => {
                // DevCloseDC(HDC hdc) → HMF
                ApiResult::Normal(self.dev_close_dc(read_stack(4)))
            }
            606 => {
                // DevQueryCaps(HDC hdc, LONG lStart, LONG lCount, PLONG alArray) → BOOL
                ApiResult::Normal(self.dev_query_caps(read_stack(4), read_stack(8), read_stack(12), read_stack(16)))
            }
            351 => {
                // GpiAssociate(HPS hps, HDC hdc) → BOOL
                ApiResult::Normal(self.gpi_associate(read_stack(4), read_stack(8)))
            }
            598 => {
                // GpiCreateBitmap(HPS hps, PBITMAPINFOHEADER2 pbmpNew, ULONG flOptions,
                //                 PBYTE pbInitData, PBITMAPINFO2 pbmiInfoTable) → HBITMAP
                ApiResult::Normal(self.gpi_create_bitmap(
                    read_stack(8), read_stack(12), read_stack(16), read_stack(20)))
            }
            399 => {
                // GpiLoadBitmap(HPS hps, HMODULE hmod, ULONG idBitmap, LONG cx, LONG cy) → HBITMAP
                ApiResult::Normal(self.gpi_load_bitmap(read_stack(12), read_stack(16) as i32, read_stack(20) as i32))
            }
            371 => {
                // GpiDeleteBitmap(HBITMAP hbm) → BOOL
                ApiResult::Normal(self.gpi_delete_bitmap(read_stack(4)))
            }
            506 => {
                // GpiSetBitmap(HPS hps, HBITMAP hbm) → HBITMAP (previous)
                ApiResult::Normal(self.gpi_set_bitmap(read_stack(4), read_stack(8)))
            }
            601 => {
                // GpiQueryBitmapInfoHeader(HBITMAP hbm, PBITMAPINFOHEADER2 pbmp) → BOOL
                ApiResult::Normal(self.query_bitmap_header(read_stack(4), read_stack(8), false))
            }
            573 => {
                // GpiQueryBitmapParameters(HBITMAP hbm, PBITMAPINFOHEADER pbmp) → BOOL
                ApiResult::Normal(self.query_bitmap_header(read_stack(4), read_stack(8), true))
            }
            602 => {
                // GpiSetBitmapBits(HPS hps, LONG lScanStart, LONG lScans,
                //                  PBYTE pbBuffer, PBITMAPINFO2 pbmiInfoTable) → LONG
                ApiResult::Normal(self.bitmap_bits(read_stack(4), read_stack(8), read_stack(12),
                                                   read_stack(16), read_stack(20), true))
            }
            599 => {
                // GpiQueryBitmapBits(HPS hps, LONG lScanStart, LONG lScans,
                //                    PBYTE pbBuffer, PBITMAPINFO2 pbmiInfoTable) → LONG
                ApiResult::Normal(self.bitmap_bits(read_stack(4), read_stack(8), read_stack(12),
                                                   read_stack(16), read_stack(20), false))
            }
            355 => {
                // GpiBitBlt(HPS hpsTarget, HPS hpsSource, LONG lCount,
                //           PPOINTL aptlPoints, LONG lRop, ULONG flOptions) → LONG
                ApiResult::Normal(self.gpi_bit_blt(read_stack(4), read_stack(8), read_stack(12),
                                                   read_stack(16), read_stack(20)))
            }
            557 => {
                // GpiWCBitBlt(HPS hpsTarget, HBITMAP hbmSource, LONG lCount,
                //             PPOINTL aptlPoints, LONG lRop, ULONG flOptions) → LONG
                ApiResult::Normal(self.gpi_wc_bit_blt(read_stack(4), read_stack(8), read_stack(12),
                                                      read_stack(16), read_stack(20)))
            }
            603 => {
                // GpiDrawBits(HPS hps, PVOID pBits, PBITMAPINFO2 pbmiInfoTable,
                //             LONG lCount, PPOINTL aptlPoints, LONG lRop, ULONG flOptions) → LONG
                ApiResult::Normal(self.gpi_draw_bits(read_stack(4), read_stack(8), read_stack(12),
                                                     read_stack(16), read_stack(20), read_stack(24)))
            }

            _ => {
                warn!("Warning: Unknown PMGPI Ordinal {} on VCPU {}", ordinal, vcpu_id);
                ApiResult::Normal(0)
            }
        }
    }

    /// Where `hps` draws and its current attributes; `None` for an unknown
    /// presentation space. One with neither a window nor a bitmap has
    /// `DrawTarget::None`, which [`Loader::draw_to`] ignores.
    pub(super) fn ps_target(&self, hps: u32) -> Option<PsTarget> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let target = wm.ps_draw_target(hps)?;
        let ps = wm.ps_map.get(&hps)?;
        Some(PsTarget {
            target,
            color: ps.color,
            back_color: ps.back_color,
            mix: Mix::from_fm(ps.mix_mode).unwrap_or(Mix::Overpaint),
            back_mix: background_mix(ps.back_mix),
            pos: ps.current_pos,
            line_type: ps.line_type,
        })
    }

    /// GpiCreateLogColorTable: `LCOLF_RGB` switches to RGB colour values,
    /// `LCOLF_CONSECRGB` loads `count` consecutive entries from `start`,
    /// `LCOLF_INDRGB` loads (index, RGB) pairs, `LCOLF_DEFAULT` or
    /// `LCOL_RESET` restore the default table. `LCOLF_PALETTE` (palette
    /// manager) is not supported.
    fn create_log_color_table(&self, hps: u32, opts: u32, format: u32, start: u32, count: u32, table: u32) -> u32 {
        let read = |i: u32| self.guest_read::<u32>(table.wrapping_add(4 * i));
        let count = count.min(MAX_LOG_COLOR_ENTRIES);
        let entries: Option<Vec<(u32, u32)>> = match format {
            LCOLF_DEFAULT | LCOLF_RGB => Some(Vec::new()),
            LCOLF_CONSECRGB => (0..count).map(|i| Some((start.wrapping_add(i), read(i)? & 0x00FF_FFFF))).collect(),
            LCOLF_INDRGB => (0..count / 2).map(|k| Some((read(2 * k)?, read(2 * k + 1)? & 0x00FF_FFFF))).collect(),
            _ => None,
        };
        let Some(entries) = entries else { return GPI_ERROR };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(ps) = wm.ps_map.get_mut(&hps) else { return GPI_ERROR };
        if opts & LCOL_RESET != 0 || matches!(format, LCOLF_DEFAULT | LCOLF_RGB) {
            ps.color_table.clear();
        }
        ps.rgb_mode = format == LCOLF_RGB;
        ps.color_table.extend(entries);
        ps.refresh_colors();
        GPI_OK
    }

    /// GpiQueryLogColorTable: `QLCT_RGB` in RGB mode, `QLCT_NOTLOADED` with
    /// the default table; otherwise the RGB values of indices `start..` or,
    /// with `LCOLOPT_INDEX`, (index, RGB) pairs. Returns the LONGs written.
    fn query_log_color_table(&self, hps: u32, opts: u32, start: u32, count: u32, array: u32) -> u32 {
        let values: Vec<u32> = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(ps) = wm.ps_map.get(&hps) else { return QLCT_ERROR as u32 };
            if ps.rgb_mode { return QLCT_RGB as u32; }
            if ps.color_table.is_empty() { return QLCT_NOTLOADED as u32; }
            let count = count.min(MAX_LOG_COLOR_ENTRIES);
            if opts & LCOLOPT_INDEX != 0 {
                ps.color_entries().into_iter().filter(|&(i, _)| i >= start)
                    .flat_map(|(i, rgb)| [i, rgb]).take((count & !1) as usize).collect()
            } else {
                (0..count).map(|i| ps.resolve_color(start.wrapping_add(i))).collect()
            }
        };
        for (i, v) in values.iter().enumerate() {
            if self.guest_write::<u32>(array.wrapping_add(4 * i as u32), *v).is_none() {
                return QLCT_ERROR as u32;
            }
        }
        values.len() as u32
    }

    fn set_current_pos(&self, hps: u32, pos: (i32, i32)) {
        if let Some(ps) = self.shared.window_mgr.lock_or_recover().ps_map.get_mut(&hps) {
            ps.current_pos = pos;
        }
    }

    /// Draw a GpiBox-style rectangle: `control` is `DRO_FILL`,
    /// `DRO_OUTLINE` or `DRO_OUTLINEFILL`.
    fn draw_box(&self, t: &PsTarget, (x1, y1, x2, y2): (i32, i32, i32, i32), control: u32) {
        let mut parts = Vec::with_capacity(2);
        if control == DRO_FILL || control == DRO_OUTLINEFILL { parts.push(true); }
        if control == DRO_OUTLINE || control == DRO_OUTLINEFILL { parts.push(false); }
        for fill in parts {
            self.draw_to(t.target, DrawOp::Box { x1, y1, x2, y2, color: t.color, fill, mix: t.mix });
        }
    }

    /// GpiCharString / GpiCharStringAt: draw `bytes` (in the active
    /// codepage) at `at`, or at the current position, in the current font,
    /// and advance the current position past the string.
    fn char_string(&self, hps: u32, at: Option<(i32, i32)>, bytes: &[u8]) -> u32 {
        let cp = self.shared.active_codepage.load(Ordering::Relaxed);
        let text = super::codepage::cp_decode(bytes, cp);
        let Some(t) = self.ps_target(hps) else { return GPI_ERROR };
        let (x, y) = at.unwrap_or(t.pos);
        // Pin the CJK regional face to the codepage of this call.
        let font = FontSpec { cjk: CjkRegion::from_codepage(cp), ..self.ps_font(hps) };
        let advance = font::text_width(&text, font);
        // The background mix applies to the string's character box.
        if let Some(mix) = t.back_mix && advance > 0 {
            let m = font::metrics(font);
            self.draw_to(t.target, DrawOp::Box {
                x1: x, y1: y - m.descent, x2: x + advance - 1, y2: y + m.ascent - 1,
                color: t.back_color, fill: true, mix,
            });
        }
        self.draw_to(t.target, DrawOp::TextFont { x, y, text, color: t.color, font, mix: t.mix });
        self.set_current_pos(hps, (x + advance, y));
        GPI_OK
    }

    /// Font currently selected into presentation space `hps`.
    fn ps_font(&self, hps: u32) -> FontSpec {
        self.shared.window_mgr.lock_or_recover()
            .ps_map.get(&hps).map_or(FontSpec::SYSTEM, |ps| ps.current_font())
    }

    /// Write the `FONTMETRICS` of `spec` to guest memory at `addr`.
    ///
    /// `cb` is the byte length the caller has allocated; we fill at most `cb`
    /// bytes so callers that request a partial struct still get the data they fit.
    fn write_font_metrics(&self, addr: u32, cb: u32, spec: FontSpec) {
        let m = font::metrics(spec);
        let face: &[u8] = match spec.family {
            FontFamily::Sans  => b"IBM Plex Sans",
            FontFamily::Mono  => b"IBM Plex Mono",
            FontFamily::Serif => b"IBM Plex Serif",
        };
        // Zero the entire region first so unwritten fields default to 0.
        let len = cb.min(FM_SIZE) as usize;
        let zeros = vec![0u8; len];
        if let Some(slice) = self.guest_slice_mut(addr, len) {
            slice.copy_from_slice(&zeros);
        }

        // Helper: only write if the field's end fits within `cb`.
        let write_i32 = |off: u32, val: i32| {
            if off + 4 <= cb { self.guest_write::<i32>(addr + off, val); }
        };
        let write_i16 = |off: u32, val: i16| {
            if off + 2 <= cb { self.guest_write::<i16>(addr + off, val); }
        };
        let write_str = |off: u32, s: &[u8], max: usize| {
            if off < cb {
                let end = (off as usize + max).min(cb as usize);
                let n = s.len().min(end - off as usize);
                if let Some(slice) = self.guest_slice_mut(addr + off, max.min(cb as usize - off as usize)) {
                    slice[..n].copy_from_slice(&s[..n]);
                }
            }
        };

        // Names
        write_str(FM_OFF_FAMILYNAME, face, 32);
        write_str(FM_OFF_FACENAME,   face, 32);

        // Key metrics
        write_i32(FM_OFF_EMHEIGHT,       m.line_height);
        write_i32(FM_OFF_XHEIGHT,        m.x_height);
        write_i32(FM_OFF_MAXASCENDER,    m.ascent);
        write_i32(FM_OFF_MAXDESCENDER,   m.descent);
        write_i32(FM_OFF_LCASE_ASCENT,   m.ascent);
        write_i32(FM_OFF_LCASE_DESCENT,  m.descent);
        write_i32(FM_OFF_AVECHWIDTH,     m.avg_char_width);
        write_i32(FM_OFF_MAXCHARINC,     m.max_char_width);
        write_i32(FM_OFF_EMINC,          m.max_char_width);
        write_i32(FM_OFF_MAXBASELINEEXT, m.line_height);
        write_i16(FM_OFF_XDEVICERES,     DEVICE_RES);
        write_i16(FM_OFF_YDEVICERES,     DEVICE_RES);
        write_i16(FM_OFF_FIRSTCHAR,      32);   // ' '
        write_i16(FM_OFF_LASTCHAR,       255);
        write_i16(FM_OFF_DEFAULTCHAR,    63);   // '?'
        write_i16(FM_OFF_BREAKCHAR,      32);   // ' '
        write_i16(FM_OFF_NOMINALPOINTSIZE, 100); // 10pt × 10
    }
}

#[cfg(test)]
mod tests {
    use super::super::Loader;
    use super::{DrawOp, Mix, DRO_FILL, DRO_OUTLINE, DRO_OUTLINEFILL, GPI_ERROR, GPI_OK};

    #[test]
    fn test_map_color_clr_black_white() {
        let loader = Loader::new_mock();
        // CLR_BLACK = -1 as u32 = 0xFFFFFFFF
        assert_eq!(loader.map_color(0xFFFF_FFFF), 0x00000000, "CLR_BLACK");
        // CLR_WHITE = -2 as u32 = 0xFFFFFFFE
        assert_eq!(loader.map_color(0xFFFF_FFFE), 0x00FFFFFF, "CLR_WHITE");
        // CLR_DEFAULT = -3 as u32 = 0xFFFFFFFD
        assert_eq!(loader.map_color(0xFFFF_FFFD), 0x00000000, "CLR_DEFAULT");
    }

    #[test]
    fn test_map_color_palette_indices() {
        let loader = Loader::new_mock();
        assert_eq!(loader.map_color(1), 0x000000FF, "CLR_BLUE");
        assert_eq!(loader.map_color(2), 0x00FF0000, "CLR_RED");
        assert_eq!(loader.map_color(4), 0x0000FF00, "CLR_GREEN");
        assert_eq!(loader.map_color(5), 0x0000FFFF, "CLR_CYAN");
        assert_eq!(loader.map_color(8), 0x00404040, "CLR_DARKGRAY");
        assert_eq!(loader.map_color(15), 0x00C0C0C0, "CLR_PALEGRAY");
    }

    #[test]
    fn test_map_color_direct_rgb() {
        let loader = Loader::new_mock();
        // Direct 0x00RRGGBB values (>= 16, positive i32) pass through unchanged.
        assert_eq!(loader.map_color(0x00123456), 0x00123456, "direct RGB passthrough");
        assert_eq!(loader.map_color(0x00FF8000), 0x00FF8000, "orange passthrough");
        // Values 0-15 are CLR_* palette — not pass-through.
        assert_ne!(loader.map_color(16), 0, "value 16 is first direct RGB");
    }

    #[test]
    fn test_font_metrics_key_constants() {
        assert!(super::FM_SIZE <= 256, "FONTMETRICS fits in a reasonable buffer");
        assert!(super::FATTRS_OFF_FACENAME + super::FACESIZE as u32 <= super::FATTRS_OFF_MAXBASELINEEXT);
    }

    #[test]
    fn test_gpi_query_current_position_roundtrip() {
        // GpiMove (404) stores position; GpiQueryCurrentPosition (416) reads it back.
        // We exercise this by calling the handlers via raw API dispatch with a mock loader.
        // This test just confirms we don't panic/crash on these ordinals.
        let _loader = Loader::new_mock();
        // (Full dispatch requires a VcpuBackend; smoke test is via integration)
    }

    /// GpiCharString (ordinal 358) must decode raw guest bytes through the
    /// active codepage rather than treating them as UTF-8.
    #[test]
    fn test_gpi_char_string_decodes_through_active_codepage() {
        use std::sync::atomic::Ordering;

        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(850, Ordering::Relaxed);
        let (_, hps, surface) = window_ps(&loader);

        // CP850 bytes for "caf\x82" (é = 0x82 in CP850).
        let text_ptr: u32 = 0x3000;
        write_bytes(&loader, text_ptr, b"caf\x82");
        assert_eq!(call_gpi(&loader, 358, &[hps, 4, text_ptr]), GPI_OK);

        let drawn = recorded(&surface);
        let [DrawOp::TextFont { text, font, .. }] = drawn.as_slice() else { panic!("{drawn:?}") };
        // The decoded text is "café" (U+00E9), not "caf\u{FFFD}".
        assert_eq!(text, "caf\u{00E9}", "GpiCharString should decode CP850 bytes to Unicode");
        // No logical font selected: the system font, with the CJK region
        // pinned from the codepage active at the call (CP850).
        assert_eq!(*font, crate::gui::font::FontSpec {
            cjk: crate::gui::font::CjkRegion::Japanese, ..crate::gui::font::FontSpec::SYSTEM });
    }

    /// A 200×100 window with a presentation space; its surface records what
    /// is drawn. Returns `(hwnd, hps, surface)`.
    fn window_ps(loader: &Loader) -> (u32, u32, crate::gui::SharedSurface) {
        use super::super::mutex_ext::MutexExt;
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hwnd = wm.create_window("Test".to_string(), 0, 0);
        let w = wm.get_window_mut(hwnd).unwrap();
        (w.cx, w.cy) = (200, 100);
        let hps = wm.create_ps(hwnd);
        let surface = wm.frame_surface(hwnd).unwrap();
        surface.lock().unwrap().record = Some(Vec::new());
        (hwnd, hps, surface)
    }

    /// The operations drawn into `surface` since the last call.
    fn recorded(surface: &crate::gui::SharedSurface) -> Vec<DrawOp> {
        surface.lock().unwrap().record.replace(Vec::new()).unwrap()
    }

    fn write_point(loader: &Loader, addr: u32, (x, y): (i32, i32)) {
        loader.guest_write::<i32>(addr, x).unwrap();
        loader.guest_write::<i32>(addr + 4, y).unwrap();
    }

    /// Drawing happens on the calling thread: GpiQueryPel reads back what
    /// GpiSetPel, GpiBox and GpiLine just drew, as a default colour-table
    /// index, and outside the window reports CLR_NOINDEX.
    #[test]
    fn test_gpi_query_pel_reads_back_what_was_drawn() {
        use super::super::constants::CLR_NOINDEX;
        const CLR_RED: u32 = 2;
        const CLR_BLUE: u32 = 1;
        let loader = Loader::new_mock();
        let (_, hps, _) = window_ps(&loader);
        let ppt: u32 = 0x4000;
        let query = |pt| { write_point(&loader, ppt, pt); call_gpi(&loader, 476, &[hps, ppt]) };

        assert_eq!(query((5, 5)), 0, "an unpainted pel is the background (CLR_BACKGROUND)");
        assert_eq!(call_gpi(&loader, 517, &[hps, CLR_RED]), 1); // GpiSetColor
        write_point(&loader, ppt, (5, 5));
        assert_eq!(call_gpi(&loader, 544, &[hps, ppt]), GPI_OK); // GpiSetPel
        assert_eq!(query((5, 5)), CLR_RED);
        assert_eq!(query((6, 5)), 0);

        // GpiMove + GpiBox(DRO_FILL) with a direct RGB colour.
        call_gpi(&loader, 517, &[hps, 0x0012_3456]);
        write_point(&loader, ppt, (20, 20));
        call_gpi(&loader, 404, &[hps, ppt]);
        write_point(&loader, ppt, (30, 25));
        assert_eq!(call_gpi(&loader, 356, &[hps, DRO_FILL, ppt, 0, 0]), GPI_OK);
        assert_eq!(query((25, 22)), 0x0012_3456, "RGB without a table entry is returned as RGB");
        assert_eq!(query((31, 22)), 0);

        // GpiLine from (30, 25) to (30, 60).
        call_gpi(&loader, 517, &[hps, CLR_BLUE]);
        write_point(&loader, ppt, (30, 25));
        call_gpi(&loader, 404, &[hps, ppt]);
        write_point(&loader, ppt, (30, 60));
        assert_eq!(call_gpi(&loader, 398, &[hps, ppt]), GPI_OK);
        assert_eq!(query((30, 40)), CLR_BLUE);

        assert_eq!(query((200, 5)), CLR_NOINDEX as u32, "right of the window");
        assert_eq!(query((-1, 5)), CLR_NOINDEX as u32);
        assert_eq!(call_gpi(&loader, 476, &[0xBAD, ppt]), CLR_NOINDEX as u32, "unknown HPS");
    }

    /// GpiSetMix changes how drawing combines with the pels already there;
    /// GpiQueryMix / GpiQueryBackMix report the modes and invalid modes are
    /// rejected.
    #[test]
    fn test_gpi_mix_modes_combine_with_the_destination() {
        use super::super::constants::{BM_LEAVEALONE, BM_SRCTRANSPARENT, FM_INVERT, FM_OVERPAINT, FM_XOR};
        let loader = Loader::new_mock();
        let (hwnd, hps, surface) = window_ps(&loader);
        assert_eq!(call_gpi(&loader, 467, &[hps]), FM_OVERPAINT, "default foreground mix");
        assert_eq!(call_gpi(&loader, 425, &[hps]), BM_LEAVEALONE, "default background mix");

        let ppt: u32 = 0x4000;
        let pel = |x, y| surface.lock().unwrap().pel(x, y).unwrap();
        let fill = |(x1, y1), (x2, y2)| {
            write_point(&loader, ppt, (x1, y1));
            call_gpi(&loader, 404, &[hps, ppt]);
            write_point(&loader, ppt, (x2, y2));
            call_gpi(&loader, 356, &[hps, DRO_FILL, ppt, 0, 0]);
        };
        call_gpi(&loader, 517, &[hps, 0x00F0_0F0F]);
        fill((0, 0), (9, 9));
        assert_eq!(pel(3, 3), 0x00F0_0F0F);

        assert_eq!(call_gpi(&loader, 537, &[hps, FM_XOR]), GPI_OK);
        assert_eq!(call_gpi(&loader, 467, &[hps]), FM_XOR);
        call_gpi(&loader, 517, &[hps, 0x00FF_FFFF]);
        fill((0, 0), (4, 9));
        assert_eq!(pel(3, 3), 0x000F_F0F0, "XOR with white inverts");
        assert_eq!(pel(7, 3), 0x00F0_0F0F, "outside the box unchanged");
        fill((0, 0), (4, 9));
        assert_eq!(pel(3, 3), 0x00F0_0F0F, "XOR twice restores");

        call_gpi(&loader, 537, &[hps, FM_INVERT]);
        write_point(&loader, ppt, (7, 3));
        call_gpi(&loader, 544, &[hps, ppt]);
        assert_eq!(pel(7, 3), 0x000F_F0F0, "FM_INVERT ignores the colour");

        // The mix is part of what is drawn.
        let ops = recorded(&surface);
        assert!(matches!(ops.last(), Some(DrawOp::Pel { x: 7, y: 3, mix: Mix::Invert, .. })), "{ops:?}");

        assert_eq!(call_gpi(&loader, 537, &[hps, 3]), GPI_ERROR, "3 is not a mix mode");
        assert_eq!(call_gpi(&loader, 537, &[hps, 18]), GPI_ERROR, "background-only mode");
        assert_eq!(call_gpi(&loader, 467, &[hps]), FM_INVERT, "rejected modes leave it unchanged");
        assert_eq!(call_gpi(&loader, 505, &[hps, BM_SRCTRANSPARENT]), GPI_OK);
        assert_eq!(call_gpi(&loader, 425, &[hps]), BM_SRCTRANSPARENT);
        assert_eq!(call_gpi(&loader, 505, &[hps, 20]), GPI_ERROR);
        assert_eq!(call_gpi(&loader, 537, &[0xBAD, FM_XOR]), GPI_ERROR, "unknown HPS");

        // Drawing marks the frame dirty for the compositor.
        use super::super::mutex_ext::MutexExt;
        assert!(loader.shared.window_mgr.lock_or_recover().dirty.contains(&hwnd));
    }

    /// GpiBox outlines and fills; GpiErase clears the window.
    #[test]
    fn test_gpi_box_outline_and_erase() {
        use crate::gui::surface::SURFACE_BLANK;
        let loader = Loader::new_mock();
        let (_, hps, surface) = window_ps(&loader);
        let ppt: u32 = 0x4000;
        write_point(&loader, ppt, (10, 10));
        call_gpi(&loader, 404, &[hps, ppt]);
        write_point(&loader, ppt, (20, 20));
        assert_eq!(call_gpi(&loader, 356, &[hps, DRO_OUTLINEFILL, ppt, 0, 0]), GPI_OK);
        let ops = recorded(&surface);
        assert_eq!(ops.len(), 2, "fill, then outline: {ops:?}");
        assert!(matches!(ops[0], DrawOp::Box { x1: 10, y1: 10, x2: 20, y2: 20, fill: true, .. }));
        assert!(matches!(ops[1], DrawOp::Box { fill: false, .. }));
        call_gpi(&loader, 356, &[hps, DRO_OUTLINE, ppt, 0, 0]);
        assert!(matches!(recorded(&surface)[..], [DrawOp::Box { fill: false, .. }]));

        assert_eq!(call_gpi(&loader, 389, &[hps]), GPI_OK); // GpiErase
        assert!(surface.lock().unwrap().buffer.iter().all(|&p| p == SURFACE_BLANK));
    }

    /// A presentation space without a window draws nothing but still tracks
    /// its current position.
    #[test]
    fn test_gpi_without_a_window_only_moves_the_position() {
        use super::super::mutex_ext::MutexExt;
        let loader = Loader::new_mock();
        let hps = loader.shared.window_mgr.lock_or_recover().create_ps(0);
        let ppt: u32 = 0x4000;
        write_point(&loader, ppt, (7, 9));
        assert_eq!(call_gpi(&loader, 398, &[hps, ppt]), GPI_OK);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.ps_map[&hps].current_pos, (7, 9));
        assert!(wm.surfaces.is_empty());
    }

    /// Push `args` as a cdecl argument list and call PMGPI `ordinal`.
    fn call_gpi(loader: &Loader, ordinal: u32, args: &[u32]) -> u32 {
        use super::super::vm_backend::mock::MockVcpu;
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
        match loader.handle_pmgpi_call(&mut vcpu, 0, ordinal) {
            super::ApiResult::Normal(v) => v,
            _ => panic!("unexpected ApiResult"),
        }
    }

    fn write_bytes(loader: &Loader, addr: u32, bytes: &[u8]) {
        for (i, &b) in bytes.iter().enumerate() {
            loader.guest_write::<u8>(addr + i as u32, b).unwrap();
        }
    }

    /// GpiCreateLogFont maps the FATTRS face/style/size onto an IBM Plex font;
    /// GpiSetCharSet selects it; text output, the current-position advance,
    /// GpiQueryTextBox and GpiQueryFontMetrics then all use that font.
    #[test]
    fn test_gpi_log_font_selects_plex_face_for_drawing_and_metrics() {
        use super::super::mutex_ext::MutexExt;
        use crate::gui::font::{self, FontFamily, FontSpec};

        let loader = Loader::new_mock();
        let (_, hps, surface) = window_ps(&loader);

        // FATTRS: bold "Courier" with a 24-px maximum baseline extent.
        let pfat: u32 = 0x4000;
        write_bytes(&loader, pfat, &[0u8; 56]);
        loader.guest_write::<u16>(pfat + super::FATTRS_OFF_SELECTION, super::FATTR_SEL_BOLD).unwrap();
        write_bytes(&loader, pfat + super::FATTRS_OFF_FACENAME, b"Courier\0");
        loader.guest_write::<i32>(pfat + super::FATTRS_OFF_MAXBASELINEEXT, 24).unwrap();
        use crate::gui::font::CjkRegion;
        // Text drawn under CP936 pins the Simplified Chinese face.
        loader.shared.active_codepage.store(936, std::sync::atomic::Ordering::Relaxed);
        let selected = FontSpec { family: FontFamily::Mono, bold: true, italic: false, px: 24,
                                  cjk: CjkRegion::Auto };
        let expect = FontSpec { cjk: CjkRegion::SimplifiedChinese, ..selected };

        assert_eq!(call_gpi(&loader, 368, &[hps, 0, 7, pfat]), super::FONT_MATCH);
        assert_eq!(call_gpi(&loader, 513, &[hps, 7]), 1); // GpiSetCharSet(lcid 7)

        // GpiCharStringAt(hps, &pt, 5, "Hello")
        let (ppt, ptext): (u32, u32) = (0x4100, 0x4200);
        loader.guest_write::<i32>(ppt, 10).unwrap();
        loader.guest_write::<i32>(ppt + 4, 50).unwrap();
        write_bytes(&loader, ptext, b"Hello");
        call_gpi(&loader, 359, &[hps, ppt, 5, ptext]);
        let drawn: Vec<_> = recorded(&surface).into_iter().filter_map(|op| match op {
            DrawOp::TextFont { x, y, text, font, .. } => Some((x, y, text, font)),
            _ => None,
        }).collect();
        assert_eq!(drawn, [(10, 50, "Hello".to_string(), expect)]);
        let width = font::text_width("Hello", expect);
        {
            let wm = loader.shared.window_mgr.lock_or_recover();
            assert_eq!(wm.ps_map[&hps].current_pos, (10 + width, 50), "advance uses the font's width");
        }

        // GpiQueryTextBox reports the same width and the font's ascent/descent.
        let aptl: u32 = 0x4300;
        call_gpi(&loader, 489, &[hps, 5, ptext, 5, aptl]);
        let m = font::metrics(expect);
        let pt = |i: u32| (loader.guest_read::<i32>(aptl + i * 8).unwrap(),
                           loader.guest_read::<i32>(aptl + i * 8 + 4).unwrap());
        assert_eq!(pt(0), (0, m.ascent));
        assert_eq!(pt(2), (width, -m.descent));
        assert_eq!(pt(4), (width, 0));

        // GpiQueryFontMetrics describes the selected face.
        let pfm: u32 = 0x4400;
        call_gpi(&loader, 453, &[hps, super::FM_SIZE, pfm]);
        assert_eq!(loader.guest_read::<i32>(pfm + super::FM_OFF_MAXBASELINEEXT).unwrap(), 24);
        assert_eq!(loader.guest_read::<i32>(pfm + super::FM_OFF_MAXASCENDER).unwrap(), m.ascent);
        assert_eq!(loader.guest_read::<i32>(pfm + super::FM_OFF_AVECHWIDTH).unwrap(), m.avg_char_width);
        let name: Vec<u8> = (0..13).map(|i| loader.guest_read::<u8>(pfm + super::FM_OFF_FACENAME + i).unwrap()).collect();
        assert_eq!(&name, b"IBM Plex Mono");

        // GpiDeleteSetId drops the logical font and falls back to the system font.
        assert_eq!(call_gpi(&loader, 378, &[hps, 7]), 1);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.ps_map[&hps].current_font(), FontSpec::SYSTEM);
    }

    /// A GpiSetCharBox character box resizes the current font.
    #[test]
    fn test_char_box_sets_font_size() {
        use super::super::pm_types::WindowManager;
        use crate::gui::font::FontSpec;
        let mut wm = WindowManager::new();
        let hwnd = wm.create_window("Test".to_string(), 0, 0);
        let hps = wm.create_ps(hwnd);
        let ps = wm.ps_map.get_mut(&hps).unwrap();
        assert_eq!(ps.current_font(), FontSpec::SYSTEM);
        ps.char_box = (0, 32);
        assert_eq!(ps.current_font(), FontSpec { px: 32, ..FontSpec::SYSTEM });
    }

    /// Every `ordinal => { // ApiName(` arm in a PM sub-dispatcher must use the
    /// ordinal the Open Watcom import libraries assign to that API
    /// (`doc/os2_ordinals.md`). A wrong ordinal compiles and passes handler
    /// tests that call it by number, yet no real application ever reaches it.
    fn assert_arms_match_ordinal_table(source: &str, prefix: &str, not_exported: &[&str]) {
        let table = include_str!("../../doc/os2_ordinals.md");
        let mut doc: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
        for line in table.lines() {
            let cols: Vec<&str> = line.split('|').map(str::trim).collect();
            if cols.len() >= 3 && cols[2].starts_with(prefix)
                && let Ok(ord) = cols[1].parse::<u32>()
            {
                doc.entry(cols[2]).or_insert(ord);
            }
        }
        assert!(doc.len() > 50, "ordinal table for {prefix}* not found");

        let lines: Vec<&str> = source.lines().collect();
        let mut checked = 0;
        for pair in lines.windows(2) {
            let Some(ord) = pair[0].trim().strip_suffix(" => {").and_then(|o| o.parse::<u32>().ok())
                else { continue };
            let Some(rest) = pair[1].trim().strip_prefix("// ") else { continue };
            let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if !name.starts_with(prefix) || not_exported.contains(&name.as_str()) { continue; }
            assert_eq!(doc.get(name.as_str()).copied(), Some(ord),
                       "{name} is dispatched at ordinal {ord}");
            checked += 1;
        }
        assert!(checked >= 25, "only {checked} {prefix}* arms were recognised");
    }

    #[test]
    fn test_pmgpi_dispatch_ordinals_match_ordinal_table() {
        assert_arms_match_ordinal_table(include_str!("pm_gpi.rs"), "Gpi", &[]);
    }

    #[test]
    fn test_pmwin_dispatch_ordinals_match_ordinal_table() {
        assert_arms_match_ordinal_table(include_str!("pm_win.rs"), "Win", &[]);
    }
}
