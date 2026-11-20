// SPDX-License-Identifier: GPL-3.0-or-later
//
// PMWIN utility calls (ADR-0086): national-language string helpers,
// dialog helpers, presentation spaces for painting, the clipboard's owner,
// viewer and format queries, waits, pointers from bitmaps, error
// information, libraries, user message types and min / max positions. `handle_pmwin_call` hands them here.
// Signatures from Open Watcom's `pmwin.h` and the Toolkit's (ADR-0072).

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_win::Got;
use super::pm_types::PsOrigin;
use super::ApiResult;
use crate::gui::surface::{DrawOp, Mix};

/// The longest string the string helpers look at.
const MAX_HELPER_STRING: u32 = 64 * 1024;
/// The most points one WinMapDlgPoints call converts.
const MAX_DLG_POINTS: u32 = 64 * 1024;
/// The largest pointer WinCreatePointer makes, in pels each way.
const MAX_POINTER_SIZE: u32 = 256;
/// The most user messages and user data types registered.
const MAX_USER_TYPES: usize = 4096;
/// The most fields of one user data type.
const MAX_USER_TYPE_FIELDS: i32 = 256;
/// The fixed part of an ERRINFO (five ULONGs).
const ERRINFO_SIZE: u32 = 20;
/// White, as a pel of a bitmap's surface.
const RGB_WHITE: u32 = 0x00FF_FFFF;
/// `<pmwin.h>`: WinDrawPointer options, the first user data type.
const DP_HALFTONED: u32 = 0x0001;
const DP_INVERTED: u32 = 0x0002;
const DTYP_USER: i32 = 16384;
/// The start of WinStoreWindowPos's record (Warpine's own layout).
const WINPOS_MAGIC: &[u8; 8] = b"WRPWPOS1";
/// The ULONGs of the record after its start.
const WINPOS_FIELDS: usize = 11;
/// `<pmerr.h>`: a bitmap handle that is no bitmap.
const PMERR_INV_HBITMAP: u32 = 0x207B;

impl super::Loader {
    /// A PMWIN utility call, or `None` when `ordinal` is not one.
    pub(super) fn pmwin_util_call(&self, vcpu_id: u32, ordinal: u32, read_stack: &dyn Fn(u64) -> u32) -> Option<ApiResult> {
        Some(match ordinal {
            893 => {
                // WinUpper(HAB hab, ULONG idcp, ULONG idcc, PSZ psz) -> ULONG: the string in
                // upper case, in place; its length.
                let (cp, psz) = (self.helper_cp(vcpu_id, read_stack(8)), read_stack(16));
                let len = self.helper_strlen(psz);
                if len > 0 { self.map_case_guest_buf(psz, len, cp); }
                ApiResult::Normal(len)
            }
            894 => {
                // WinUpperChar(HAB hab, ULONG idcp, ULONG idcc, ULONG c) -> ULONG
                let cp = self.helper_cp(vcpu_id, read_stack(8));
                ApiResult::Normal(upper_char(read_stack(16), cp))
            }
            791 => {
                // WinNextChar(HAB hab, ULONG idcp, ULONG idcc, PSZ psz) -> PSZ: the next
                // character (a DBCS pair counts as one); the null stays.
                let (cp, psz) = (self.helper_cp(vcpu_id, read_stack(8)), read_stack(16));
                ApiResult::Normal(self.next_char(psz, cp))
            }
            795 => {
                // WinPrevChar(HAB hab, ULONG idcp, ULONG idcc, PSZ pszStart, PSZ psz) -> PSZ
                let cp = self.helper_cp(vcpu_id, read_stack(8));
                let (start, at) = (read_stack(16), read_stack(20));
                if at <= start { return Some(ApiResult::Normal(start)); }
                let (mut prev, mut p) = (start, start);
                while p < at && p - start < MAX_HELPER_STRING {
                    prev = p;
                    let next = self.next_char(p, cp);
                    if next == p { break; }
                    p = next;
                }
                ApiResult::Normal(prev)
            }
            708 => {
                // WinCompareStrings(HAB hab, ULONG idcp, ULONG idcc, PSZ psz1, PSZ psz2,
                // ULONG reserved) -> WCS_EQ / WCS_LT / WCS_GT by the codepage's collating
                // sequence (DosQueryCollate's).
                let cp = self.helper_cp(vcpu_id, read_stack(8));
                let (a, b) = (read_stack(16), read_stack(20));
                if a == 0 || b == 0 { return Some(ApiResult::Normal(WCS_ERROR)); }
                let table = super::codepage::collate_table(cp);
                let key = |p: u32| -> Vec<u8> { self.helper_bytes(p).iter().map(|&c| table[c as usize]).collect() };
                ApiResult::Normal(match key(a).cmp(&key(b)) {
                    std::cmp::Ordering::Equal => WCS_EQ,
                    std::cmp::Ordering::Less => WCS_LT,
                    std::cmp::Ordering::Greater => WCS_GT,
                })
            }
            711 => {
                // WinCpTranslateChar(HAB hab, ULONG cpSrc, UCHAR chSrc, ULONG cpDst) -> UCHAR
                let (src, ch, dst) = (read_stack(8), read_stack(12) as u8, read_stack(16));
                let text = super::codepage::cp_decode(&[ch], src);
                ApiResult::Normal(super::codepage::cp_encode(&text, dst).first().copied().unwrap_or(0) as u32)
            }
            712 => {
                // WinCpTranslateString(HAB hab, ULONG cpSrc, PSZ pszSrc, ULONG cpDst,
                // ULONG cchDestMax, PSZ pchDest) -> BOOL: at most cchDestMax bytes, the null
                // included.
                let (src, psz, dst, max, out) = (read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24));
                if psz == 0 || out == 0 || max == 0 { return Some(self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS)); }
                let text = super::codepage::cp_decode(&self.helper_bytes(psz), src);
                let mut bytes = super::codepage::cp_encode(&text, dst);
                bytes.truncate(max as usize - 1);
                bytes.push(0);
                ApiResult::Normal(self.guest_write_bytes(out, &bytes).is_some() as u32)
            }
            750 => {
                // WinGetCurrentTime(HAB hab) -> ULONG: milliseconds, on the clock of the
                // messages' time stamps.
                ApiResult::Normal(super::stubs::ms_count())
            }
            922 => {
                // WinCreateDlg(HWND hwndParent, HWND hwndOwner, PFNWP pfnDlgProc,
                // PDLGTEMPLATE pdlgt, PVOID pCreateParams) -> HWND: a modeless dialog from a
                // template in memory.
                let pdlgt = read_stack(16);
                if pdlgt == 0 { return Some(self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS)); }
                ApiResult::Normal(self.load_dialog_template(vcpu_id, read_stack(4), read_stack(8), read_stack(12),
                                                            pdlgt, read_stack(20)))
            }
            787 => {
                // WinMapDlgPoints(HWND hwndDlg, PPOINTL prgwptl, ULONG cwpt, BOOL fCalcWindowCoords)
                // -> BOOL: dialog units to window coordinates, or back.
                let (hwnd, pts, n, to_window) = (read_stack(4), read_stack(8), read_stack(12), read_stack(16) != 0);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return Some(ApiResult::Normal(0)); }
                if n > MAX_DLG_POINTS || (n > 0 && pts == 0) { return Some(self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS)); }
                let metrics = crate::gui::font::metrics(crate::gui::font::FontSpec::SYSTEM);
                for k in 0..n {
                    let at = pts + k * 8;
                    let (Some(x), Some(y)) = (self.guest_read::<i32>(at), self.guest_read::<i32>(at + 4)) else {
                        return Some(ApiResult::Normal(0));
                    };
                    let (x, y) = if to_window {
                        (dlg_units_to_px(x, metrics.avg_char_width, DLG_UNITS_PER_CHAR_W),
                         dlg_units_to_px(y, metrics.line_height, DLG_UNITS_PER_CHAR_H))
                    } else {
                        (px_to_dlg_units(x, metrics.avg_char_width, DLG_UNITS_PER_CHAR_W),
                         px_to_dlg_units(y, metrics.line_height, DLG_UNITS_PER_CHAR_H))
                    };
                    self.guest_write::<i32>(at, x);
                    self.guest_write::<i32>(at + 4, y);
                }
                ApiResult::Normal(1)
            }
            816 => {
                // WinQueryDlgItemTextLength(HWND hwndDlg, ULONG idItem) -> LONG
                let (hwnd, id) = (read_stack(4), read_stack(8));
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return Some(ApiResult::Normal(0)); }
                let item = self.shared.window_mgr.lock_or_recover().find_child_by_id(hwnd, id);
                match item {
                    Some(h) => self.win_query_window_text_length(vcpu_id, h),
                    None => self.pm_fail(vcpu_id, PMERR_INVALID_HWND),
                }
            }
            914 => {
                // WinGetDlgMsg(HWND hwndDlg, PQMSG pqmsg) -> BOOL: the next message of a
                // dialog's own loop; FALSE once the dialog is dismissed (or WM_QUIT).
                let (hwnd, pqmsg) = (read_stack(4), read_stack(8));
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return Some(ApiResult::Normal(0)); }
                if !self.has_msg_queue(vcpu_id) { return Some(self.pm_fail(vcpu_id, PMERR_NO_MSG_QUEUE)); }
                let dismissed = self.shared.window_mgr.lock_or_recover().get_window(hwnd).is_none_or(|w| w.dialog_dismissed);
                if dismissed { return Some(ApiResult::Normal(0)); }
                match self.next_message(vcpu_id, pqmsg, 0, 0, 0, PM_REMOVE, true, true) {
                    Some(Got::Sent(s)) => self.run_sent(vcpu_id, s, super::pm_remote::AfterSent::Rerun(ordinal)),
                    Some(Got::HitTest(a)) => self.hit_test_chain(vcpu_id, a, super::pm_remote::AfterSent::Rerun(ordinal)),
                    Some(Got::Message(msg)) => ApiResult::Normal((msg != WM_QUIT) as u32),
                    None => ApiResult::Normal(0),
                }
            }
            890 => {
                // WinTrackRect(HWND hwnd, HPS hps, PTRACKINFO pti) -> BOOL: track a rectangle
                // with the pointer or keyboard until accepted (TRUE) or cancelled (pm_track.rs).
                self.win_track_rect(vcpu_id, ordinal, read_stack(0), read_stack(4), read_stack(8), read_stack(12))
            }
            882 => {
                // WinShowTrackRect(HWND hwnd, BOOL fShow) -> BOOL: hide or show the outline.
                self.win_show_track_rect(vcpu_id, read_stack(4), read_stack(8) != 0)
            }
            759 => {
                // WinGetScreenPS(HWND hwndDesktop) -> HPS: a presentation space for the
                // whole screen, freed by WinReleasePS.
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let hps = wm.create_ps(HWND_DESKTOP);
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.origin = PsOrigin::Cached; }
                ApiResult::Normal(hps)
            }
            749 => {
                // WinGetClipPS(HWND hwnd, HWND hwndClip, ULONG fl) -> HPS: as WinGetPS; the
                // window is clipped to itself, the PSF_* flags otherwise ignored.
                let (hwnd, fl) = (read_stack(4), read_stack(12));
                if fl & !(PSF_LOCKWINDOWUPDATE | PSF_CLIPUPWARDS | PSF_CLIPDOWNWARDS | PSF_CLIPSIBLINGS
                          | PSF_CLIPCHILDREN | PSF_PARENTCLIP) != 0 {
                    return Some(self.pm_fail(vcpu_id, PMERR_INVALID_FLAG));
                }
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return Some(ApiResult::Normal(0)); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if wm.get_window(hwnd).is_none() { return Some(ApiResult::Normal(0)); }
                let hps = wm.create_ps(hwnd);
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.origin = PsOrigin::Cached; }
                ApiResult::Normal(hps)
            }
            767 => {
                // WinInvertRect(HPS hps, PRECTL prcl) -> BOOL: every pel inverted.
                let (hps, prcl) = (read_stack(4), read_stack(8));
                let r = |i: u32| self.guest_read::<i32>(prcl.wrapping_add(4 * i));
                let (Some(x1), Some(y1), Some(x2), Some(y2)) = (r(0), r(1), r(2), r(3)) else { return Some(ApiResult::Normal(0)) };
                let Some(t) = self.ps_target(hps) else { return Some(ApiResult::Normal(0)) };
                self.draw_ps(&t, DrawOp::Box {
                    x1, y1, x2: x2 - 1, y2: y2 - 1, color: 0, fill: true, mix: Mix::Invert,
                });
                ApiResult::Normal(1)
            }
            786 => {
                // WinMakeRect(HAB hab, PRECTL pwrc) -> BOOL: a WRECT and a RECTL are the same
                // in 32-bit PM, so nothing changes.
                ApiResult::Normal((read_stack(8) != 0) as u32)
            }
            785 => {
                // WinMakePoints(HAB hab, PPOINTL pwpt, ULONG cwpt) -> BOOL: likewise.
                ApiResult::Normal((read_stack(8) != 0 || read_stack(12) == 0) as u32)
            }
            774 => {
                // WinIsWindowShowing(HWND hwnd) -> BOOL: visible, not minimized, with a
                // part inside its parents (other windows covering it are not considered).
                let hwnd = read_stack(4);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return Some(ApiResult::Normal(0)); }
                ApiResult::Normal(self.shared.window_mgr.lock_or_recover().is_window_showing(hwnd) as u32)
            }
            855 => {
                // WinSetClipbrdOwner(HAB hab, HWND hwnd) -> BOOL
                let hwnd = read_stack(8);
                if !self.pm_check_hwnd(vcpu_id, hwnd, true) { return Some(ApiResult::Normal(0)); }
                self.shared.window_mgr.lock_or_recover().clipboard_owner = hwnd;
                ApiResult::Normal(1)
            }
            808 => {
                // WinQueryClipbrdOwner(HAB hab) -> HWND
                ApiResult::Normal(self.shared.window_mgr.lock_or_recover().clipboard_owner)
            }
            856 => {
                // WinSetClipbrdViewer(HAB hab, HWND hwnd) -> BOOL: hwnd is posted
                // WM_DRAWCLIPBOARD when the clipboard is closed after a change.
                let hwnd = read_stack(8);
                if !self.pm_check_hwnd(vcpu_id, hwnd, true) { return Some(ApiResult::Normal(0)); }
                self.shared.window_mgr.lock_or_recover().clipboard_viewer = hwnd;
                ApiResult::Normal(1)
            }
            809 => {
                // WinQueryClipbrdViewer(HAB hab) -> HWND
                ApiResult::Normal(self.shared.window_mgr.lock_or_recover().clipboard_viewer)
            }
            739 => {
                // WinEnumClipbrdFmts(HAB hab, ULONG fmt) -> ULONG: the format after fmt (0:
                // the first), in ascending order; 0 after the last.
                let after = read_stack(8);
                let wm = self.shared.window_mgr.lock_or_recover();
                ApiResult::Normal(wm.clipboard.keys().copied().filter(|&f| f > after).min().unwrap_or(0))
            }
            807 => {
                // WinQueryClipbrdFmtInfo(HAB hab, ULONG fmt, PULONG prgfFmtInfo) -> BOOL:
                // whether the format is there, and the CFI_* flags it was set with.
                let (fmt, out) = (read_stack(8), read_stack(12));
                let wm = self.shared.window_mgr.lock_or_recover();
                if !wm.clipboard.contains_key(&fmt) { return Some(ApiResult::Normal(0)); }
                let info = wm.clipboard_info.get(&fmt).copied().unwrap_or(0);
                drop(wm);
                if out != 0 { self.guest_write::<u32>(out, info); }
                ApiResult::Normal(1)
            }
            980 => {
                // WinWaitMuxWaitSem(HMUX hmux, ULONG ulTimeout, PULONG pulUser) -> APIRET:
                // as DosWaitMuxWaitSem (no sent messages to process meanwhile, as
                // WinWaitEventSem).
                ApiResult::Normal(self.dos_wait_mux_wait_sem(vcpu_id, read_stack(4), read_stack(8), read_stack(12)))
            }
            998 => {
                // WinCheckInput(HAB hab) -> BOOL: input waits in the system queue.
                ApiResult::Normal((self.shared.window_mgr.lock_or_recover().pending_input() > 0) as u32)
            }
            771 => {
                // WinIsThreadActive(HAB hab) -> BOOL: the active window is this thread's.
                let wm = self.shared.window_mgr.lock_or_recover();
                let active = wm.active_window();
                let mine = wm.thread_queue(vcpu_id);
                ApiResult::Normal((active != 0 && mine.is_some() && wm.find_hmq_for_hwnd(active) == mine) as u32)
            }
            717 => {
                // WinCreatePointer(HWND hwndDesktop, HBITMAP hbmPointer, BOOL fPointer,
                // LONG xHotspot, LONG yHotspot) -> HPOINTER: from a two-tone bitmap twice
                // the pointer's height (AND mask above, XOR mask below).
                let info = [read_stack(12), read_stack(16), read_stack(20), read_stack(8), 0, 0, 0];
                self.create_pointer_from(vcpu_id, info)
            }
            942 => {
                // WinCreatePointerIndirect(HWND hwndDesktop, PPOINTERINFO pptri) -> HPOINTER:
                // the masks, and a colour bitmap when hbmColor is given.
                let p = read_stack(8);
                let mut info = [0u32; 7];
                for (k, v) in info.iter_mut().enumerate() {
                    match self.guest_read::<u32>(p.wrapping_add(4 * k as u32)) {
                        Some(x) if p != 0 => *v = x,
                        _ => return Some(self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS)),
                    }
                }
                self.create_pointer_from(vcpu_id, info)
            }
            822 => {
                // WinQueryPointerInfo(HPOINTER hptr, PPOINTERINFO pptri) -> BOOL: what it was
                // made from; for other pointers fPointer and the hot spot, no bitmaps.
                let (hptr, p) = (read_stack(4), read_stack(8));
                let mgr = self.shared.pointers.lock_or_recover();
                let info = match (mgr.info(hptr), mgr.shape(hptr)) {
                    (Some(i), _) => i,
                    (None, Some(crate::gui::pointer::PointerShape::Image(img))) => {
                        [1, img.hot_x as u32, (img.height as i32 - 1 - img.hot_y) as u32, 0, 0, 0, 0]
                    }
                    (None, Some(_)) => [1, 0, 0, 0, 0, 0, 0],
                    (None, None) => { drop(mgr); return Some(self.pm_fail(vcpu_id, PMERR_INVALID_HPTR)); }
                };
                drop(mgr);
                let bytes: Vec<u8> = info.iter().flat_map(|v| v.to_le_bytes()).collect();
                ApiResult::Normal(self.guest_write_bytes(p, &bytes).is_some() as u32)
            }
            732 => {
                // WinDrawPointer(HPS hps, LONG x, LONG y, HPOINTER hptr, ULONG fs) -> BOOL:
                // its image with its lower-left corner at (x, y); DP_HALFTONED every other
                // pel, DP_INVERTED the colours inverted. A system pointer draws nothing.
                let (hps, x, y, hptr, fs) = (read_stack(4), read_stack(8) as i32, read_stack(12) as i32, read_stack(16), read_stack(20));
                let shape = self.shared.pointers.lock_or_recover().shape(hptr);
                let Some(shape) = shape else { return Some(self.pm_fail(vcpu_id, PMERR_INVALID_HPTR)) };
                let Some(t) = self.ps_target(hps) else { return Some(ApiResult::Normal(0)) };
                if let crate::gui::pointer::PointerShape::Image(img) = shape {
                    let (w, h) = (img.width as i32, img.height as i32);
                    for row in 0..h {
                        for col in 0..w {
                            if fs & DP_HALFTONED != 0 && (row + col) % 2 == 1 { continue; }
                            let at = ((row * w + col) * 4) as usize;
                            let Some(px) = img.rgba.get(at..at + 4) else { continue };
                            if px[3] == 0 { continue; }
                            let mut color = (px[0] as u32) << 16 | (px[1] as u32) << 8 | px[2] as u32;
                            if fs & DP_INVERTED != 0 { color ^= 0x00FF_FFFF; }
                            self.draw_ps(&t, DrawOp::Pel { x: x + col, y: y + (h - 1 - row), color, mix: Mix::Overpaint });
                        }
                    }
                }
                ApiResult::Normal(1)
            }
            751 => {
                // WinGetErrorInfo(HAB hab) -> PERRINFO: the thread's last error (taken, as
                // WinGetLastError does), with one message; NULL when there is none.
                let id = self.shared.pm_errors.take(vcpu_id);
                if id == 0 { return Some(ApiResult::Normal(0)); }
                ApiResult::Normal(self.error_info_block(id).unwrap_or(0))
            }
            748 => {
                // WinFreeErrorInfo(PERRINFO perrinfo) -> BOOL: only a block WinGetErrorInfo gave.
                let p = read_stack(4);
                let ours = self.shared.window_mgr.lock_or_recover().error_infos.remove(&p);
                if ours { self.shared.mem_mgr.lock_or_recover().free(p); }
                ApiResult::Normal(ours as u32)
            }
            977 => {
                // WinSetErrorInfo(ERRORID errorcode, ULONG fsErrorInfo, ...) -> ERRORID: the
                // thread's last error becomes errorcode (the SEI_* options and the further
                // arguments only concern IBM's debugging aids).
                let id = read_stack(4);
                self.shared.pm_errors.set_id(vcpu_id, id);
                ApiResult::Normal(id)
            }
            777 => {
                // WinLoadLibrary(HAB hab, PSZ pszLibname) -> HLIB: as DosLoadModule, the
                // handle returned (0 on failure).
                self.win_load_library(vcpu_id, read_stack(8))
            }
            986 => {
                // WinLoadProcedure(HAB hab, HLIB hlib, PSZ pszProcname) -> PFNWP
                let Some(scratch) = self.alloc_zeroed(4) else { return Some(ApiResult::Normal(0)) };
                let rc = self.dos_query_proc_addr(read_stack(8), 0, read_stack(12), scratch);
                let pfn = if rc == 0 { self.guest_read::<u32>(scratch).unwrap_or(0) } else { 0 };
                self.shared.mem_mgr.lock_or_recover().free(scratch);
                ApiResult::Normal(pfn)
            }
            722 => {
                // WinDeleteLibrary(HAB hab, HLIB hlib) -> BOOL: as DosFreeModule.
                let hlib = read_stack(8);
                match self.dos_free_module(hlib) {
                    ApiResult::Normal(rc) => ApiResult::Normal((rc == 0) as u32),
                    ApiResult::CallGuest { addr, hmod, object_bases, .. } => {
                        self.shared.dll_mgr.lock_or_recover().pending_free.insert(hmod, object_bases);
                        let step = super::pm_hook::CallStep::hook(addr, vec![hmod, 1], false);
                        self.start_chain(super::pm_hook::CallChain::new(vcpu_id, vec![step], false,
                            super::pm_hook::ChainThen::FreeLibrary { hmod }))
                    }
                    other => other,
                }
            }
            987 => {
                // WinDeleteProcedure(HAB hab, PFNWP wndproc) -> BOOL: nothing to free.
                ApiResult::Normal((read_stack(8) != 0) as u32)
            }
            846 => {
                // WinRegisterUserMsg(HAB hab, ULONG msgid, LONG datatype1, LONG dir1,
                // LONG datatype2, LONG dir2, LONG datatyper) -> BOOL: recorded.
                let msg = read_stack(8);
                if msg == 0 { return Some(self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS)); }
                let types = [read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)];
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if wm.user_msgs.len() >= MAX_USER_TYPES && !wm.user_msgs.contains_key(&msg) {
                    drop(wm);
                    return Some(self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS));
                }
                wm.user_msgs.insert(msg, types);
                ApiResult::Normal(1)
            }
            845 => {
                // WinRegisterUserDatatype(HAB hab, LONG datatype, LONG count, PLONG types)
                // -> BOOL: recorded.
                let (dt, n, p) = (read_stack(8) as i32, read_stack(12) as i32, read_stack(16));
                if dt < DTYP_USER || n <= 0 || n > MAX_USER_TYPE_FIELDS || p == 0 {
                    return Some(self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS));
                }
                let Some(fields) = (0..n as u32).map(|k| self.guest_read::<i32>(p + 4 * k)).collect::<Option<Vec<i32>>>() else {
                    return Some(self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS));
                };
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if wm.user_datatypes.len() >= MAX_USER_TYPES && !wm.user_datatypes.contains_key(&dt) {
                    drop(wm);
                    return Some(self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS));
                }
                wm.user_datatypes.insert(dt, fields);
                ApiResult::Normal(1)
            }
            754 => {
                // WinGetMaxPosition(HWND hwnd, PSWP pswp) -> BOOL: where a frame goes when
                // maximized.
                let (hwnd, pswp) = (read_stack(4), read_stack(8));
                let wm = self.shared.window_mgr.lock_or_recover();
                let Some(w) = wm.get_window(hwnd).filter(|_| wm.frame_to_client.contains_key(&hwnd)) else {
                    return Some(ApiResult::Normal(0));
                };
                let border = frame_border_width(w.frame_flags);
                let (dw, dh) = desktop_size();
                drop(wm);
                let r = super::pm_minmax::WinRect { x: -border, y: -border, cx: dw as i32 + 2 * border, cy: dh as i32 + 2 * border };
                ApiResult::Normal(self.write_min_max_swp(pswp, hwnd, SWP_MAXIMIZE | SWP_MOVE | SWP_SIZE, r) as u32)
            }
            755 => {
                // WinGetMinPosition(HWND hwnd, PSWP pswp, PPOINTL pptl) -> BOOL: where a
                // frame goes when minimized: at pptl when given, else its remembered or
                // first free icon position.
                let (hwnd, pswp, pptl) = (read_stack(4), read_stack(8), read_stack(12));
                let wm = self.shared.window_mgr.lock_or_recover();
                if !wm.frame_to_client.contains_key(&hwnd) { return Some(ApiResult::Normal(0)); }
                let remembered = match (wm.word(hwnd, QWS_XMINIMIZE), wm.word(hwnd, QWS_YMINIMIZE)) {
                    (Some(x), Some(y)) if (x, y) != (-1, -1) => Some((x, y)),
                    _ => None,
                };
                let given = (pptl != 0).then(|| (self.guest_read::<i32>(pptl), self.guest_read::<i32>(pptl + 4)));
                let (x, y) = match given {
                    Some((Some(x), Some(y))) => (x, y),
                    Some(_) => return Some(ApiResult::Normal(0)),
                    None => remembered.unwrap_or_else(|| wm.icon_position(hwnd)),
                };
                drop(wm);
                let r = super::pm_minmax::WinRect { x, y, cx: MINIMIZED_CX, cy: MINIMIZED_CY };
                ApiResult::Normal(self.write_min_max_swp(pswp, hwnd, SWP_MINIMIZE | SWP_MOVE | SWP_SIZE, r) as u32)
            }
            _ => return None,
        })
    }

    /// WinStoreWindowPos(PSZ pszAppName, PSZ pszKeyName, HWND hwnd) -> BOOL: the
    /// window's position, size and state (and a frame's normal and icon
    /// positions) in the user profile, as Warpine's own record (OS/2's
    /// layout is not public; its presentation parameters are not kept).
    pub fn win_store_window_pos(&self, tid: u32, papp: u32, pkey: u32, hwnd: u32) -> u32 {
        let (app, key) = (self.helper_bytes(papp), self.helper_bytes(pkey));
        if app.is_empty() || key.is_empty() { return self.pm_fail_bool(tid, PMERR_INVALID_PARAMETERS); }
        if !self.pm_check_hwnd(tid, hwnd, false) { return 0; }
        let record = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window(hwnd) else { return 0 };
            let state = if w.style & WS_MINIMIZED != 0 { 1 } else if w.style & WS_MAXIMIZED != 0 { 2 } else { 0 };
            let word = |i: i32, d: i32| if state == 0 { d } else { wm.word(hwnd, i).unwrap_or(d) };
            let fields = [w.x, w.y, w.cx, w.cy, state,
                          word(QWS_XRESTORE, w.x), word(QWS_YRESTORE, w.y), word(QWS_CXRESTORE, w.cx), word(QWS_CYRESTORE, w.cy),
                          wm.word(hwnd, QWS_XMINIMIZE).unwrap_or(-1), wm.word(hwnd, QWS_YMINIMIZE).unwrap_or(-1)];
            let mut r = WINPOS_MAGIC.to_vec();
            r.extend(fields.iter().flat_map(|v| v.to_le_bytes()));
            r
        };
        let profiles = self.shared.profiles.lock_or_recover();
        let dm = self.shared.drive_mgr.lock_or_recover();
        profiles.write(&dm, HINI_USERPROFILE, &app, Some(&key), Some(&record)).is_ok() as u32
    }

    /// WinRestoreWindowPos(PSZ pszAppName, PSZ pszKeyName, HWND hwnd) -> BOOL: what
    /// WinStoreWindowPos kept, applied: position and size, and a frame's
    /// state (made at once, without asking its client with WM_MINMAXFRAME,
    /// unverified). FALSE when nothing was kept under the names.
    pub fn win_restore_window_pos(&self, tid: u32, papp: u32, pkey: u32, hwnd: u32) -> u32 {
        let (app, key) = (self.helper_bytes(papp), self.helper_bytes(pkey));
        if !self.pm_check_hwnd(tid, hwnd, false) { return 0; }
        let record = {
            let profiles = self.shared.profiles.lock_or_recover();
            let dm = self.shared.drive_mgr.lock_or_recover();
            profiles.query_value(&dm, HINI_USERPROFILE, &app, &key).ok()
        };
        let Some(f) = record.as_deref().and_then(parse_window_pos) else { return 0 };
        let [x, y, cx, cy, state, rx, ry, rcx, rcy, mx, my] = f;
        let op = match state { 1 => SWP_MINIMIZE, 2 => SWP_MAXIMIZE, _ => SWP_RESTORE };
        let is_frame = self.shared.window_mgr.lock_or_recover().frame_to_client.contains_key(&hwnd);
        if !is_frame {
            self.set_window_pos(hwnd, 0, x, y, cx.max(0), cy.max(0), SWP_MOVE | SWP_SIZE);
            return 1;
        }
        let (px, py, pcx, pcy) = if state == 0 { (x, y, cx, cy) } else { (rx, ry, rcx, rcy) };
        // The normal position first; then the state, which returns to it.
        if let Some((o, target)) = self.set_window_pos(hwnd, 0, px, py, pcx.max(0), pcy.max(0), SWP_RESTORE | SWP_MOVE | SWP_SIZE) {
            self.shared.window_mgr.lock_or_recover().finish_minmax(hwnd, o, target);
        }
        if state != 0 {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            wm.set_word(hwnd, QWS_XMINIMIZE, mx);
            wm.set_word(hwnd, QWS_YMINIMIZE, my);
            drop(wm);
            if let Some((o, target)) = self.set_window_pos(hwnd, 0, 0, 0, 0, 0, op) {
                self.shared.window_mgr.lock_or_recover().finish_minmax(hwnd, o, target);
            }
        }
        1
    }

    /// FALSE with PM error `code` (for calls returning a BOOL).
    fn pm_fail_bool(&self, tid: u32, code: u32) -> u32 {
        self.shared.pm_errors.set(tid, code);
        0
    }

    /// Write an SWP: `fl`, the size and position of `r`, `hwnd`.
    fn write_min_max_swp(&self, pswp: u32, hwnd: u32, fl: u32, r: super::pm_minmax::WinRect) -> bool {
        let words = [fl, r.cy as u32, r.cx as u32, r.y as u32, r.x as u32, 0, hwnd, 0, 0];
        let bytes: Vec<u8> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
        pswp != 0 && self.guest_write_bytes(pswp, &bytes).is_some()
    }

    /// A pointer from POINTERINFO `info` (fPointer, xHotspot, yHotspot,
    /// hbmPointer, hbmColor, …): the masks' bitmap (white pels are 1 bits,
    /// unverified) and, when given, the colour bitmap of the same width
    /// and half the height.
    fn create_pointer_from(&self, vcpu_id: u32, info: [u32; 7]) -> ApiResult {
        let pels = |hbm: u32| -> Option<(u32, u32, Vec<u32>)> {
            let wm = self.shared.window_mgr.lock_or_recover();
            let s = wm.bitmaps.get(&hbm)?.surface.lock_or_recover();
            Some((s.width, s.height, s.buffer.clone()))
        };
        let Some((w, h2, mask)) = pels(info[3]) else { return self.pm_fail(vcpu_id, PMERR_INV_HBITMAP) };
        let h = h2 / 2;
        if w == 0 || h == 0 || h2 % 2 != 0 || w > MAX_POINTER_SIZE || h > MAX_POINTER_SIZE {
            return self.pm_fail(vcpu_id, PMERR_INVALID_PARAMETERS);
        }
        let color = match info[4] {
            0 => None,
            hbm => match pels(hbm) {
                Some((cw, ch, c)) if (cw, ch) == (w, h) => Some(c),
                _ => return self.pm_fail(vcpu_id, PMERR_INV_HBITMAP),
            },
        };
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            for col in 0..w {
                let at = |r: u32| (r * w + col) as usize;
                let (and, xor) = (mask[at(row)] & RGB_WHITE != 0, mask[at(h + row)] & RGB_WHITE != 0);
                let px: [u8; 4] = match (and, xor) {
                    (true, false) => [0, 0, 0, 0],
                    // An inverted pel: black (a host cursor cannot invert).
                    (true, true) => [0, 0, 0, 0xFF],
                    (false, _) => {
                        let c = match &color { Some(c) => c[at(row)], None if xor => RGB_WHITE, None => 0 };
                        [(c >> 16) as u8, (c >> 8) as u8, c as u8, 0xFF]
                    }
                };
                rgba.extend(px);
            }
        }
        // The hot spot counts from the bottom-left pel, the image from the top.
        let hot_x = (info[1] as i32).clamp(0, w as i32 - 1);
        let hot_y = (h as i32 - 1 - info[2] as i32).clamp(0, h as i32 - 1);
        let image = crate::gui::pointer::PointerImage { width: w, height: h, hot_x, hot_y, rgba };
        let mut mgr = self.shared.pointers.lock_or_recover();
        let hptr = mgr.create(crate::gui::pointer::PointerShape::Image(std::sync::Arc::new(image)), vcpu_id);
        mgr.set_info(hptr, info);
        ApiResult::Normal(hptr)
    }

    /// An ERRINFO block in new memory for error `id`: the fixed part, one
    /// 16-bit message offset (unverified width) and the message.
    fn error_info_block(&self, id: u32) -> Option<u32> {
        let message = format!("PM error {:#06x}\0", id & 0xFFFF);
        let offs_at = ERRINFO_SIZE;
        let msg_at = offs_at + 4;
        let size = msg_at + message.len() as u32;
        let p = self.alloc_zeroed(size)?;
        let words = [ERRINFO_SIZE, id, 1, offs_at, 0];
        let mut bytes: Vec<u8> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
        bytes.extend_from_slice(&(msg_at as u16).to_le_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(message.as_bytes());
        self.guest_write_bytes(p, &bytes)?;
        self.shared.window_mgr.lock_or_recover().error_infos.insert(p);
        Some(p)
    }

    /// WinLoadLibrary: DosLoadModule into a scratch word; the handle, after
    /// the module's `_DLL_InitTerm` calls when it has some.
    fn win_load_library(&self, vcpu_id: u32, psz: u32) -> ApiResult {
        let Some(scratch) = self.alloc_zeroed(4) else { return ApiResult::Normal(0) };
        let then = |hmod| super::pm_hook::ChainThen::LoadLibrary { hmod, scratch };
        match self.dos_load_module(0, 0, psz, scratch) {
            ApiResult::Normal(rc) => {
                let h = if rc == 0 { self.guest_read::<u32>(scratch).unwrap_or(0) } else { 0 };
                self.shared.mem_mgr.lock_or_recover().free(scratch);
                ApiResult::Normal(h)
            }
            ApiResult::CallChain(mut chain) => {
                if let super::pm_hook::ChainThen::LoadModule { hmod, .. } = chain.then { chain.then = then(hmod); }
                ApiResult::CallChain(chain)
            }
            ApiResult::CallGuest { addr, hmod, .. } => {
                let step = super::pm_hook::CallStep::hook(addr, vec![hmod, 0], false);
                self.start_chain(super::pm_hook::CallChain::new(vcpu_id, vec![step], false, then(hmod)))
            }
            other => other,
        }
    }

    /// The end of WinLoadLibrary's `_DLL_InitTerm` chain: the handle, or 0.
    pub(super) fn load_library_done(&self, hmod: u32, scratch: u32, chain: &super::pm_hook::CallChain) -> u32 {
        let rc = self.dll_inits_done(hmod, scratch, chain);
        self.shared.mem_mgr.lock_or_recover().free(scratch);
        if rc == 0 { hmod } else { 0 }
    }

    /// The end of WinDeleteLibrary's `_DLL_InitTerm(hmod, 1)`: free the
    /// module's pages.
    pub(super) fn free_library_done(&self, hmod: u32) -> u32 {
        let bases = self.shared.dll_mgr.lock_or_recover().pending_free.remove(&hmod).unwrap_or_default();
        let mut mem = self.shared.mem_mgr.lock_or_recover();
        for base in bases { mem.free(base); }
        1
    }

    /// The codepage of a string helper: `idcp`, or the thread's queue's
    /// when 0.
    fn helper_cp(&self, vcpu_id: u32, idcp: u32) -> u32 {
        if idcp == 0 { self.thread_codepage(vcpu_id) } else { idcp }
    }

    /// The bytes of the string at `p` (without its null), at most
    /// `MAX_HELPER_STRING`.
    fn helper_bytes(&self, p: u32) -> Vec<u8> {
        let mut out = Vec::new();
        if p == 0 { return out; }
        while (out.len() as u32) < MAX_HELPER_STRING {
            match self.guest_read::<u8>(p.wrapping_add(out.len() as u32)) {
                Some(0) | None => break,
                Some(b) => out.push(b),
            }
        }
        out
    }

    fn helper_strlen(&self, p: u32) -> u32 { self.helper_bytes(p).len() as u32 }

    /// The character after the one at `p`: two bytes for a DBCS lead byte
    /// with its trail byte, one otherwise; `p` itself at the null.
    fn next_char(&self, p: u32, cp: u32) -> u32 {
        match self.guest_read::<u8>(p) {
            Some(0) | None => p,
            Some(b) if super::locale::is_dbcs_lead_byte(b, cp) && self.guest_read::<u8>(p.wrapping_add(1)).is_some_and(|t| t != 0) => p + 2,
            Some(_) => p + 1,
        }
    }
}

/// The fields of a WinStoreWindowPos record: x, y, cx, cy, state (0
/// normal, 1 minimized, 2 maximized), the normal x, y, cx, cy and the
/// icon x, y.
fn parse_window_pos(record: &[u8]) -> Option<[i32; WINPOS_FIELDS]> {
    let body = record.strip_prefix(WINPOS_MAGIC.as_slice())?;
    if body.len() != WINPOS_FIELDS * 4 { return None; }
    let mut out = [0i32; WINPOS_FIELDS];
    for (k, v) in out.iter_mut().enumerate() {
        *v = i32::from_le_bytes(body[k * 4..k * 4 + 4].try_into().ok()?);
    }
    Some(out)
}

/// A character in upper case: a byte, or (in a DBCS codepage) a pair with
/// the lead byte low and the trail byte high (unverified).
fn upper_char(c: u32, cp: u32) -> u32 {
    if c <= 0xFF { return super::codepage::cp_map_case_upper(c as u8, cp) as u32; }
    let (lead, trail) = ((c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8);
    let ch = super::codepage::decode_dbcs(lead, trail, cp);
    let mut upper = ch.to_uppercase();
    let (Some(u), None) = (upper.next(), upper.next()) else { return c };
    let bytes = super::codepage::cp_encode(&u.to_string(), cp);
    match bytes.as_slice() {
        [l, t] => (*l as u32) | ((*t as u32) << 8),
        _ => c,
    }
}

/// Dialog units to pels: `units` × `per_char_px` / `units_per_char`, rounded.
fn dlg_units_to_px(units: i32, per_char_px: i32, units_per_char: i32) -> i32 {
    ((units as i64 * per_char_px as i64 + units_per_char as i64 / 2).div_euclid(units_per_char as i64)) as i32
}

/// Pels to dialog units, rounded.
fn px_to_dlg_units(px: i32, per_char_px: i32, units_per_char: i32) -> i32 {
    if per_char_px <= 0 { return 0; }
    ((px as i64 * units_per_char as i64 + per_char_px as i64 / 2).div_euclid(per_char_px as i64)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_units_convert_both_ways() {
        assert_eq!(dlg_units_to_px(8, 7, 4), 14);
        assert_eq!(px_to_dlg_units(14, 7, 4), 8);
        assert_eq!(dlg_units_to_px(-4, 16, 8), -8);
        assert_eq!(px_to_dlg_units(5, 0, 4), 0, "no metrics, no units");
    }

    #[test]
    fn window_position_records_are_checked() {
        let mut r = WINPOS_MAGIC.to_vec();
        for v in 1..=WINPOS_FIELDS as i32 { r.extend(v.to_le_bytes()); }
        assert_eq!(parse_window_pos(&r).map(|f| f[4]), Some(5));
        assert_eq!(parse_window_pos(&r[..r.len() - 1]), None, "too short");
        assert_eq!(parse_window_pos(b"something else"), None);
    }

    #[test]
    fn characters_go_to_upper_case() {
        assert_eq!(upper_char(b'a' as u32, 850), b'A' as u32);
        assert_eq!(upper_char(0x82, 850), 0x90, "é to É in codepage 850");
        assert_eq!(upper_char(b'1' as u32, 437), b'1' as u32);
    }
}
