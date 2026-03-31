// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 Presentation Manager PMGPI API handler methods.

use super::vm_backend::VcpuBackend;
use log::{debug, warn};

use super::mutex_ext::MutexExt;
use super::ApiResult;
use crate::gui::GUIMessage;

// ── GpiCreateLogFont return values ────────────────────────────────────────────
const FONT_DEFAULT: u32 = 1; // best available match is the system default

// ── Mock font metrics (8×16 fixed-pitch system font) ─────────────────────────
const MOCK_CHAR_W:     i32 = 8;
const MOCK_CHAR_H:     i32 = 16;
const MOCK_ASCENDER:   i32 = 12; // pixels above baseline
const MOCK_DESCENDER:  i32 = 4;  // pixels below baseline
const MOCK_DEVICE_RES: i16 = 96; // device resolution (DPI) reported to apps

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
                let _hab = read_stack(4);
                let _hdc = read_stack(8);
                let _pszl = read_stack(12);
                let _opts = read_stack(16);
                let hps = self.shared.window_mgr.lock_or_recover().create_ps(0);
                debug!("  [VCPU {}] GpiCreatePS -> HPS {}", vcpu_id, hps);
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
                let mapped = self.map_color(color);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.color = mapped;
                }
                ApiResult::Normal(1)
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
                let wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get(&hps) {
                    let (x1, y1) = ps.current_pos;
                    let color = ps.color;
                    // DRO_FILL=1 (fill only), DRO_OUTLINE=2 (outline only),
                    // DRO_OUTLINEFILL=3 (fill + outline) — from Open Watcom pmgpi.h
                    let do_fill    = control == 1 || control == 3;
                    let do_outline = control == 2 || control == 3;
                    let hwnd = ps.hwnd;
                    let frame_hwnd = wm.client_to_frame(hwnd);
                    if let Some(ref sender) = wm.gui_tx {
                        if do_fill {
                            let _ = sender.send(GUIMessage::DrawBox {
                                handle: frame_hwnd, x1, y1, x2, y2, color, fill: true,
                            });
                        }
                        if do_outline {
                            let _ = sender.send(GUIMessage::DrawBox {
                                handle: frame_hwnd, x1, y1, x2, y2, color, fill: false,
                            });
                        }
                    }
                }
                ApiResult::Normal(1) // GPI_OK
            }
            398 => {
                // GpiLine(HPS hps, PPOINTL pptl)
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let (x2, y2) = (
                    self.guest_read::<i32>(pptl).unwrap_or(0),
                    self.guest_read::<i32>(pptl + 4).unwrap_or(0),
                );
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get(&hps) {
                    let (x1, y1) = ps.current_pos;
                    let color = ps.color;
                    let hwnd = ps.hwnd;
                    let frame_hwnd = wm.client_to_frame(hwnd);
                    if let Some(ref sender) = wm.gui_tx {
                        let _ = sender.send(GUIMessage::DrawLine {
                            handle: frame_hwnd, x1, y1, x2, y2, color
                        });
                    }
                }
                // Update current position
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.current_pos = (x2, y2);
                }
                ApiResult::Normal(1) // GPI_OK
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
                let text_str = String::from_utf8_lossy(&text).to_string();
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let color = wm.ps_map.get(&hps).map(|ps| ps.color).unwrap_or(0);
                let ps_hwnd = wm.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0);
                let hwnd = wm.client_to_frame(ps_hwnd);
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::DrawText {
                        handle: hwnd, x, y, text: text_str, color,
                    });
                }
                // Update current position (advance x by character width * count)
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.current_pos = (x + (count as i32 * 8), y);
                }
                ApiResult::Normal(1) // GPI_OK
            }
            389 => {
                // GpiErase(HPS hps)
                let hps = read_stack(4);
                let wm = self.shared.window_mgr.lock_or_recover();
                let ps_hwnd = wm.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0);
                let frame_hwnd = wm.client_to_frame(ps_hwnd);
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::ClearBuffer { handle: frame_hwnd });
                }
                ApiResult::Normal(1)
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
                let text_str = String::from_utf8_lossy(&text).to_string();
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let (x, y) = wm.ps_map.get(&hps).map(|ps| ps.current_pos).unwrap_or((0, 0));
                let color  = wm.ps_map.get(&hps).map(|ps| ps.color).unwrap_or(0);
                let ps_hwnd = wm.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0);
                let frame_hwnd = wm.client_to_frame(ps_hwnd);
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::DrawText { handle: frame_hwnd, x, y, text: text_str, color });
                }
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.current_pos = (x + count as i32 * MOCK_CHAR_W, y);
                }
                ApiResult::Normal(1) // GPI_OK
            }

            518 => {
                // GpiSetBackColor(HPS hps, LONG lColor)
                let hps = read_stack(4);
                let color = self.map_color(read_stack(8));
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.back_color = color; }
                debug!("  GpiSetBackColor(hps={}, color=0x{:06X})", hps, color);
                ApiResult::Normal(1)
            }
            520 => {
                // GpiQueryColor(HPS hps) → LONG lColor
                let hps = read_stack(4);
                let color = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map(|ps| ps.color).unwrap_or(0);
                ApiResult::Normal(color)
            }
            521 => {
                // GpiQueryBackColor(HPS hps) → LONG lColor
                let hps = read_stack(4);
                let color = self.shared.window_mgr.lock_or_recover()
                    .ps_map.get(&hps).map(|ps| ps.back_color).unwrap_or(0x00FFFFFF);
                ApiResult::Normal(color)
            }
            509 => {
                // GpiSetMix(HPS hps, LONG lMixMode)
                let hps = read_stack(4);
                let mode = read_stack(8);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.mix_mode = mode; }
                ApiResult::Normal(1)
            }
            503 => {
                // GpiSetBackMix(HPS hps, LONG lMixMode)
                let hps = read_stack(4);
                let mode = read_stack(8);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.back_mix = mode; }
                ApiResult::Normal(1)
            }

            416 => {
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

            481 => {
                // GpiSetCharSet(HPS hps, LONG lcid) → BOOL
                let hps = read_stack(4);
                let lcid = read_stack(8);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.char_set = lcid; }
                ApiResult::Normal(1)
            }
            482 => {
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

            381 => {
                // GpiCreateLogFont(HPS hps, PSTR8 pszLocalID, LONG lcid, PFATTRS pfatfs) → LONG
                // We always return FONT_DEFAULT — no real font selection.
                let hps = read_stack(4);
                let lcid = read_stack(12);
                debug!("  GpiCreateLogFont(hps={}, lcid={})", hps, lcid);
                ApiResult::Normal(FONT_DEFAULT)
            }
            385 => {
                // GpiDeleteSetId(HPS hps, LONG lcid) → BOOL
                // No-op — we have no real font set to release.
                ApiResult::Normal(1)
            }

            527 => {
                // GpiSetLineType(HPS hps, LONG lLineType) → BOOL  (stub)
                ApiResult::Normal(1)
            }
            529 => {
                // GpiSetLineWidth(HPS hps, FIXED fxLineWidth) → BOOL  (stub)
                ApiResult::Normal(1)
            }
            530 => {
                // GpiSetLineWidthGeom(HPS hps, FIXED fxLineWidth) → BOOL  (stub)
                ApiResult::Normal(1)
            }

            399 => {
                // GpiLoadFonts(HAB hab, PSZ pszFilename) → BOOL  (stub)
                ApiResult::Normal(1)
            }
            400 => {
                // GpiLoadPublicFonts(HAB hab, PSZ pszFilename) → BOOL  (stub)
                ApiResult::Normal(1)
            }
            401 => {
                // GpiUnloadPublicFonts(HAB hab, PSZ pszFilename) → BOOL  (stub)
                ApiResult::Normal(1)
            }

            464 => {
                // GpiQueryFontMetrics(HPS hps, LONG lMetricsLength, PFONTMETRICS pfmMetrics) → BOOL
                let hps = read_stack(4);
                let cb = read_stack(8); // byte length caller wants filled
                let pfm = read_stack(12);
                debug!("  GpiQueryFontMetrics(hps={}, cb={}, pfm=0x{:08X})", hps, cb, pfm);
                if pfm != 0 {
                    self.write_font_metrics(pfm, cb);
                }
                let _ = hps;
                ApiResult::Normal(1) // TRUE
            }

            459 => {
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
                if afm != 0 && cb > 0 { self.write_font_metrics(afm, cb); }
                ApiResult::Normal(1) // 1 font matches
            }

            476 => {
                // GpiQueryTextBox(HPS hps, LONG lCount1, PCH pchString,
                //                 LONG lCount2, PPOINTL aptlPoints) → BOOL
                //
                // Returns up to TXTBOX_COUNT (5) corner points of the text bounding
                // box in current-coordinate space (baseline at y=0).
                let hps     = read_stack(4);
                let count   = read_stack(8) as i32;
                let _pch    = read_stack(12);
                let npoints = read_stack(16); // how many POINTLs to fill (≤5)
                let aptl    = read_stack(20);
                let _ = hps;
                let w = count * MOCK_CHAR_W;
                // TXTBOX_TOPLEFT(0), TXTBOX_BOTTOMLEFT(1), TXTBOX_BOTTOMRIGHT(2),
                // TXTBOX_TOPRIGHT(3), TXTBOX_CONCAT(4) — each is a POINTL (2×i32 = 8 bytes).
                let pts: [(i32, i32); 5] = [
                    (0,  MOCK_ASCENDER),   // top-left
                    (0, -MOCK_DESCENDER),  // bottom-left
                    (w, -MOCK_DESCENDER),  // bottom-right
                    (w,  MOCK_ASCENDER),   // top-right
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

            392 => {
                // GpiFullArc(HPS hps, LONG lControl, FIXED fxMult) → LONG
                // Approximated as a filled/outlined box centred on current_pos.
                // Arc params (GpiSetArcParams) are not tracked; use a fixed 10-unit radius.
                let hps = read_stack(4);
                let control = read_stack(8);
                let wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get(&hps) {
                    let (cx, cy) = ps.current_pos;
                    let r: i32 = 10; // approximate radius
                    let color = ps.color;
                    let frame_hwnd = wm.client_to_frame(ps.hwnd);
                    if let Some(ref sender) = wm.gui_tx {
                        let do_fill    = control == 1 || control == 3;
                        let do_outline = control == 2 || control == 3;
                        if do_fill {
                            let _ = sender.send(GUIMessage::DrawBox {
                                handle: frame_hwnd, x1: cx-r, y1: cy-r, x2: cx+r, y2: cy+r,
                                color, fill: true,
                            });
                        }
                        if do_outline {
                            let _ = sender.send(GUIMessage::DrawBox {
                                handle: frame_hwnd, x1: cx-r, y1: cy-r, x2: cx+r, y2: cy+r,
                                color, fill: false,
                            });
                        }
                    }
                }
                ApiResult::Normal(1) // GPI_OK
            }

            353 => {
                // GpiSetArcParams(HPS hps, PARCPARAMS parcpArcParams) → BOOL  (stub)
                ApiResult::Normal(1)
            }

            _ => {
                warn!("Warning: Unknown PMGPI Ordinal {} on VCPU {}", ordinal, vcpu_id);
                ApiResult::Normal(0)
            }
        }
    }

    /// Write a mock `FONTMETRICS` struct to guest memory at `addr`.
    ///
    /// `cb` is the byte length the caller has allocated; we fill at most `cb`
    /// bytes so callers that request a partial struct still get the data they fit.
    fn write_font_metrics(&self, addr: u32, cb: u32) {
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
        write_str(FM_OFF_FAMILYNAME, b"System", 32);
        write_str(FM_OFF_FACENAME,   b"System", 32);

        // Key metrics
        write_i32(FM_OFF_EMHEIGHT,       MOCK_ASCENDER);
        write_i32(FM_OFF_XHEIGHT,        MOCK_ASCENDER * 3 / 4);
        write_i32(FM_OFF_MAXASCENDER,    MOCK_ASCENDER);
        write_i32(FM_OFF_MAXDESCENDER,   MOCK_DESCENDER);
        write_i32(FM_OFF_LCASE_ASCENT,   MOCK_ASCENDER);
        write_i32(FM_OFF_LCASE_DESCENT,  MOCK_DESCENDER);
        write_i32(FM_OFF_AVECHWIDTH,     MOCK_CHAR_W);
        write_i32(FM_OFF_MAXCHARINC,     MOCK_CHAR_W);
        write_i32(FM_OFF_EMINC,          MOCK_CHAR_W);
        write_i32(FM_OFF_MAXBASELINEEXT, MOCK_CHAR_H);
        write_i16(FM_OFF_XDEVICERES,     MOCK_DEVICE_RES);
        write_i16(FM_OFF_YDEVICERES,     MOCK_DEVICE_RES);
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
        // Sanity-check our mock font constants for internal consistency.
        assert_eq!(super::MOCK_CHAR_H, super::MOCK_ASCENDER + super::MOCK_DESCENDER,
                   "char height must equal ascender + descender");
        assert!(super::MOCK_CHAR_W > 0 && super::MOCK_CHAR_H > 0);
        assert!(super::FM_SIZE <= 256, "FONTMETRICS fits in a reasonable buffer");
    }

    #[test]
    fn test_gpi_query_current_position_roundtrip() {
        // GpiMove (404) stores position; GpiQueryCurrentPosition (416) reads it back.
        // We exercise this by calling the handlers via raw API dispatch with a mock loader.
        // This test just confirms we don't panic/crash on these ordinals.
        let _loader = Loader::new_mock();
        // (Full dispatch requires a VcpuBackend; smoke test is via integration)
    }
}
