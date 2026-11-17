// SPDX-License-Identifier: GPL-3.0-only

//! The PM text cursor: `WinCreateCursor` (715), `WinShowCursor` (880),
//! `WinDestroyCursor` (725) and `WinQueryCursorInfo` (812) — PM Reference /
//! EDM2.
//!
//! Each message queue has at most one cursor (`WindowManager::cursors`,
//! by queue); creating one replaces the queue's earlier cursor. A new
//! cursor is hidden: `WinShowCursor(TRUE)` counts a hide level down to 0,
//! where it shows, and `WinShowCursor(FALSE)` counts it up again; showing a
//! visible cursor fails. Its rectangle is in window coordinates, a zero
//! width or height is the nominal border (`SV_CXBORDER` / `SV_CYBORDER`), and
//! it is clipped to the window and to the clip rectangle if one is given.
//! `CURSOR_SETPOS` moves the existing cursor and ignores everything else.
//!
//! The cursor is not drawn into the window's surface: the compositors lay
//! it over the frame's pixels as they put them on the screen (see
//! `gui::cursor`), inverted solid, halftone or as a frame. A
//! `CURSOR_FLASH` cursor blinks at the `SV_CURSORRATE` rate (the renderer
//! toggles `WindowManager::cursor_blink_off`; not in scripted sessions).

use super::constants::*;
use super::pm_types::WindowManager;
use super::{ApiResult, Loader, MutexExt};
use crate::gui::cursor::{CursorKind, CursorMark};

/// The flags `WinCreateCursor` accepts.
const CURSOR_FLAGS: u32 = CURSOR_HALFTONE | CURSOR_FRAME | CURSOR_FLASH | CURSOR_SETPOS
    | CURSOR_BIDI_FIRST | CURSOR_BIDI_LAST;
/// Nominal border width / height in pels (`SV_CXBORDER` / `SV_CYBORDER`).
const NOMINAL_BORDER: i32 = 1;
/// Largest cursor side accepted, in pels; larger values are cut to it.
const MAX_CURSOR_DIM: i32 = 1 << 15;

/// One queue's cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmCursor {
    pub hwnd: u32,
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    /// `CURSOR_*` flags as created (never `CURSOR_SETPOS`).
    pub fs: u32,
    /// Clip rectangle in window coordinates (xLeft, yBottom, xRight, yTop);
    /// `None`: the window.
    pub clip: Option<(i32, i32, i32, i32)>,
    /// 0 = shown; each `WinShowCursor(FALSE)` adds one.
    pub hide_level: u32,
}

impl PmCursor {
    /// What the cursor looks like.
    fn kind(&self) -> CursorKind {
        if self.fs & CURSOR_FRAME != 0 {
            CursorKind::Frame
        } else if self.fs & CURSOR_HALFTONE != 0 {
            CursorKind::Halftone
        } else {
            CursorKind::Solid
        }
    }
}

impl WindowManager {
    /// The cursors to show now, by the frame whose surface they lie on:
    /// shown, on a visible window, and not in the off phase of a flash.
    /// Rectangles are in the frame surface's PM coordinates.
    pub fn cursor_marks(&self) -> Vec<(u32, CursorMark)> {
        let mut out: Vec<(u32, CursorMark)> = self.cursors.values()
            .filter(|c| c.hide_level == 0 && !(self.cursor_blink_off && c.fs & CURSOR_FLASH != 0))
            .filter(|c| self.is_window_visible(c.hwnd))
            .filter_map(|c| {
                let frame = self.find_frame_for_hwnd(c.hwnd);
                if !self.surfaces.contains_key(&frame) {
                    return None;
                }
                let (ax, ay, wcx, wcy) = self.get_abs_rect_in_frame(c.hwnd);
                let rect = (ax + c.x, ay + c.y, ax + c.x + c.cx, ay + c.y + c.cy);
                let window = (ax, ay, ax + wcx, ay + wcy);
                let clip = match c.clip {
                    Some((l, b, r, t)) => (window.0.max(ax + l), window.1.max(ay + b),
                                           window.2.min(ax + r), window.3.min(ay + t)),
                    None => window,
                };
                Some((frame, CursorMark { rect, clip, kind: c.kind() }))
            })
            .collect();
        out.sort_by_key(|(frame, _)| *frame);
        out
    }

    /// One step of the cursor flash: switch the blink phase and have the
    /// windows of flashing, shown cursors redrawn. Returns whether any
    /// cursor flashes.
    pub fn flash_cursors(&mut self) -> bool {
        self.cursor_blink_off = !self.cursor_blink_off;
        let flashing: Vec<u32> = self.cursors.values()
            .filter(|c| c.fs & CURSOR_FLASH != 0 && c.hide_level == 0)
            .map(|c| c.hwnd)
            .collect();
        for &hwnd in &flashing {
            self.mark_dirty(hwnd);
        }
        !flashing.is_empty()
    }
}

impl Loader {
    /// The calling thread's queue, or a `PMERR_NO_MSG_QUEUE` failure.
    fn cursor_queue(&self, tid: u32) -> Result<u32, ApiResult> {
        self.shared.window_mgr.lock_or_recover().thread_queue(tid)
            .ok_or_else(|| self.pm_fail(tid, PMERR_NO_MSG_QUEUE))
    }

    /// WinCreateCursor(hwnd, x, y, cx, cy, fs, prclClip).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn win_create_cursor(&self, tid: u32, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32),
                                    fs: u32, prcl_clip: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        if fs & !CURSOR_FLAGS != 0 {
            return self.pm_fail(tid, PMERR_INVALID_FLAG);
        }
        let hmq = match self.cursor_queue(tid) { Ok(h) => h, Err(e) => return e };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if fs & CURSOR_SETPOS != 0 {
            // Move the queue's cursor; it must be this window's.
            let Some(c) = wm.cursors.get_mut(&hmq).filter(|c| c.hwnd == hwnd) else {
                return ApiResult::Normal(0);
            };
            (c.x, c.y) = (x, y);
            wm.mark_dirty(hwnd);
            return ApiResult::Normal(1);
        }
        let clip = if prcl_clip == 0 {
            None
        } else {
            let read = |i: u32| self.guest_read::<i32>(prcl_clip.wrapping_add(i * 4));
            match (read(0), read(1), read(2), read(3)) {
                (Some(l), Some(b), Some(r), Some(t)) => Some((l, b, r, t)),
                _ => return ApiResult::Normal(0),
            }
        };
        let side = |v: i32| if v == 0 { NOMINAL_BORDER } else { v.clamp(-MAX_CURSOR_DIM, MAX_CURSOR_DIM) };
        let (cx, cy) = (side(cx), side(cy));
        // A negative extent runs left / down from the point.
        let (x, cx) = if cx < 0 { (x + cx, -cx) } else { (x, cx) };
        let (y, cy) = if cy < 0 { (y + cy, -cy) } else { (y, cy) };
        if let Some(old) = wm.cursors.insert(hmq, PmCursor { hwnd, x, y, cx, cy, fs, clip, hide_level: 1 }) {
            wm.mark_dirty(old.hwnd);
        }
        ApiResult::Normal(1)
    }

    /// WinShowCursor(hwnd, fShow).
    pub(super) fn win_show_cursor(&self, tid: u32, hwnd: u32, show: bool) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let hmq = match self.cursor_queue(tid) { Ok(h) => h, Err(e) => return e };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(c) = wm.cursors.get_mut(&hmq).filter(|c| c.hwnd == hwnd) else {
            return ApiResult::Normal(0);
        };
        if show {
            if c.hide_level == 0 {
                return ApiResult::Normal(0);
            }
            c.hide_level -= 1;
        } else {
            c.hide_level = c.hide_level.saturating_add(1);
        }
        wm.mark_dirty(hwnd);
        ApiResult::Normal(1)
    }

    /// WinDestroyCursor(hwnd): destroys the queue's cursor if it is
    /// `hwnd`'s.
    pub(super) fn win_destroy_cursor(&self, tid: u32, hwnd: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let hmq = match self.cursor_queue(tid) { Ok(h) => h, Err(e) => return e };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if wm.cursors.get(&hmq).is_none_or(|c| c.hwnd != hwnd) {
            return ApiResult::Normal(0);
        }
        wm.cursors.remove(&hmq);
        wm.mark_dirty(hwnd);
        ApiResult::Normal(1)
    }

    /// WinQueryCursorInfo(hwndDesktop, pCursorInfo): the queue's cursor as
    /// created (`CURSORINFO`); the clip rectangle is the window's when none
    /// was given. FALSE, writing nothing, without a cursor.
    pub(super) fn win_query_cursor_info(&self, tid: u32, hwnd_desktop: u32, pci: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd_desktop, true) { return ApiResult::Normal(0); }
        let hmq = match self.cursor_queue(tid) { Ok(h) => h, Err(e) => return e };
        let (c, window) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(c) = wm.cursors.get(&hmq).cloned() else { return ApiResult::Normal(0) };
            let window = wm.get_window(c.hwnd).map_or((0, 0), |w| (w.cx, w.cy));
            (c, window)
        };
        if pci == 0 {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        let (l, b, r, t) = c.clip.unwrap_or((0, 0, window.0, window.1));
        let fields = [c.hwnd, c.x as u32, c.y as u32, c.cx as u32, c.cy as u32, c.fs,
                      l as u32, b as u32, r as u32, t as u32];
        for (i, v) in fields.into_iter().enumerate() {
            if self.guest_write::<u32>(pci.wrapping_add(i as u32 * 4), v).is_none() {
                return ApiResult::Normal(0);
            }
        }
        ApiResult::Normal(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::vm_backend::mock::MockVcpu;

    const ORD_WIN_CREATE_CURSOR: u32 = 715;
    const ORD_WIN_DESTROY_CURSOR: u32 = 725;
    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_QUERY_CURSOR_INFO: u32 = 812;
    const ORD_WIN_SHOW_CURSOR: u32 = 880;
    /// Guest address of the structures the tests pass.
    const BUF: u32 = 0x0030_0000;

    fn call(l: &Loader, tid: u32, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        for (i, &a) in args.iter().enumerate() {
            l.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
        match l.handle_pmwin_call(&mut vcpu, tid, ordinal) {
            ApiResult::Normal(v) => v,
            other => panic!("PMWIN ordinal {ordinal} returned {other:?}"),
        }
    }

    /// A frame (400 x 300) with a client and a child window at (50, 40)
    /// 100 x 20 in it; thread 0 has the queue.
    fn setup() -> (Loader, u32, u32, u32) {
        let l = Loader::new_mock();
        let (hmq, client, child) = {
            let mut wm = l.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            let child = wm.create_window("App".into(), client, hmq);
            for (h, rect) in [(frame, (0, 0, 400, 300)), (client, (0, 0, 400, 280)), (child, (50, 40, 100, 20))] {
                let w = wm.get_window_mut(h).unwrap();
                (w.x, w.y, w.cx, w.cy) = rect;
                w.visible = true;
            }
            wm.frame_surface(frame);
            wm.resize_surface(frame, 400, 280);
            (hmq, client, child)
        };
        (l, hmq, client, child)
    }

    fn create(l: &Loader, hwnd: u32, r: (i32, i32, i32, i32), fs: u32, clip: u32) -> u32 {
        call(l, 0, ORD_WIN_CREATE_CURSOR, &[hwnd, r.0 as u32, r.1 as u32, r.2 as u32, r.3 as u32, fs, clip])
    }

    #[test]
    fn a_new_cursor_is_hidden_until_shown_and_counts_hide_levels() {
        let (l, _, client, _) = setup();
        assert_eq!(create(&l, client, (10, 20, 2, 16), CURSOR_SOLID, 0), 1);
        let marks = || l.shared.window_mgr.lock_or_recover().cursor_marks();
        assert!(marks().is_empty(), "created hidden");
        assert_eq!(call(&l, 0, ORD_WIN_SHOW_CURSOR, &[client, 1]), 1);
        let m = marks();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].1.rect, (10, 20, 12, 36));
        assert_eq!(m[0].1.kind, CursorKind::Solid);
        assert_eq!(call(&l, 0, ORD_WIN_SHOW_CURSOR, &[client, 1]), 0, "already visible");
        call(&l, 0, ORD_WIN_SHOW_CURSOR, &[client, 0]);
        call(&l, 0, ORD_WIN_SHOW_CURSOR, &[client, 0]);
        call(&l, 0, ORD_WIN_SHOW_CURSOR, &[client, 1]);
        assert!(marks().is_empty(), "hidden twice, shown once");
        call(&l, 0, ORD_WIN_SHOW_CURSOR, &[client, 1]);
        assert_eq!(marks().len(), 1);
    }

    #[test]
    fn one_cursor_per_queue_moved_with_setpos_and_destroyed_by_its_window() {
        let (l, hmq, client, child) = setup();
        create(&l, client, (1, 1, 0, 0), CURSOR_FRAME, 0);
        {
            let wm = l.shared.window_mgr.lock_or_recover();
            let c = &wm.cursors[&hmq];
            assert_eq!((c.cx, c.cy), (1, 1), "zero means the nominal border");
        }
        // A new cursor replaces it.
        create(&l, child, (5, 2, 3, 12), CURSOR_HALFTONE | CURSOR_FLASH, 0);
        assert_eq!(l.shared.window_mgr.lock_or_recover().cursors.len(), 1);
        assert_eq!(call(&l, 0, ORD_WIN_SHOW_CURSOR, &[client, 1]), 0, "not the client's any more");
        assert_eq!(create(&l, client, (9, 9, 0, 0), CURSOR_SETPOS, 0), 0, "SETPOS for another window");
        assert_eq!(create(&l, child, (7, 3, 50, 50), CURSOR_SETPOS, 0), 1);
        {
            let wm = l.shared.window_mgr.lock_or_recover();
            let c = &wm.cursors[&hmq];
            assert_eq!((c.x, c.y, c.cx, c.cy, c.fs), (7, 3, 3, 12, CURSOR_HALFTONE | CURSOR_FLASH),
                       "SETPOS moves only");
        }
        assert_eq!(call(&l, 0, ORD_WIN_DESTROY_CURSOR, &[client]), 0, "not the client's");
        assert_eq!(call(&l, 0, ORD_WIN_DESTROY_CURSOR, &[child]), 1);
        assert!(l.shared.window_mgr.lock_or_recover().cursors.is_empty());
        assert_eq!(call(&l, 0, ORD_WIN_DESTROY_CURSOR, &[child]), 0, "gone");
    }

    #[test]
    fn query_cursor_info_reports_the_cursor_as_created() {
        let (l, _, _, child) = setup();
        assert_eq!(call(&l, 0, ORD_WIN_QUERY_CURSOR_INFO, &[HWND_DESKTOP, BUF]), 0, "no cursor");
        for (i, v) in [2i32, 1, 30, 15].into_iter().enumerate() {
            l.guest_write::<i32>(BUF + 0x100 + i as u32 * 4, v).unwrap();
        }
        create(&l, child, (4, 2, 2, 14), CURSOR_FLASH, BUF + 0x100);
        assert_eq!(call(&l, 0, ORD_WIN_QUERY_CURSOR_INFO, &[HWND_DESKTOP, BUF]), 1);
        let got: Vec<u32> = (0..CURSORINFO_SIZE / 4).map(|i| l.guest_read::<u32>(BUF + i * 4).unwrap()).collect();
        assert_eq!(got, vec![child, 4, 2, 2, 14, CURSOR_FLASH, 2, 1, 30, 15]);
        // Without a clip rectangle: the window's.
        create(&l, child, (4, 2, 2, 14), CURSOR_SOLID, 0);
        call(&l, 0, ORD_WIN_QUERY_CURSOR_INFO, &[HWND_DESKTOP, BUF]);
        assert_eq!(l.guest_read::<u32>(BUF + 32), Some(100));
        assert_eq!(l.guest_read::<u32>(BUF + 36), Some(20));
    }

    #[test]
    fn marks_are_in_frame_surface_coordinates_and_clipped() {
        let (l, _, _, child) = setup();
        for (i, v) in [2i32, 1, 30, 15].into_iter().enumerate() {
            l.guest_write::<i32>(BUF + i as u32 * 4, v).unwrap();
        }
        create(&l, child, (4, 2, 2, 14), CURSOR_FRAME, BUF);
        call(&l, 0, ORD_WIN_SHOW_CURSOR, &[child, 1]);
        let marks = l.shared.window_mgr.lock_or_recover().cursor_marks();
        let m = marks[0].1;
        assert_eq!(m.rect, (54, 42, 56, 56), "child origin (50, 40) added");
        assert_eq!(m.clip, (52, 41, 80, 55), "the clip rectangle inside the child");
        assert_eq!(m.kind, CursorKind::Frame);
        // A hidden window hides its cursor.
        l.shared.window_mgr.lock_or_recover().get_window_mut(child).unwrap().visible = false;
        assert!(l.shared.window_mgr.lock_or_recover().cursor_marks().is_empty());
    }

    #[test]
    fn flashing_cursors_blink_and_steady_ones_do_not() {
        let (l, _, client, _) = setup();
        create(&l, client, (1, 1, 2, 2), CURSOR_SOLID, 0);
        call(&l, 0, ORD_WIN_SHOW_CURSOR, &[client, 1]);
        let mut wm = l.shared.window_mgr.lock_or_recover();
        assert!(!wm.flash_cursors(), "a steady cursor");
        assert_eq!(wm.cursor_marks().len(), 1, "shows in either phase");
        drop(wm);
        create(&l, client, (1, 1, 2, 2), CURSOR_FLASH, 0);
        call(&l, 0, ORD_WIN_SHOW_CURSOR, &[client, 1]);
        let mut wm = l.shared.window_mgr.lock_or_recover();
        let phase = |wm: &WindowManager| wm.cursor_marks().len();
        let before = phase(&wm);
        wm.dirty.clear();
        assert!(wm.flash_cursors());
        assert!(wm.dirty.contains(&client), "redrawn");
        assert_ne!(phase(&wm), before);
        wm.flash_cursors();
        assert_eq!(phase(&wm), before);
    }

    #[test]
    fn errors_and_cleanup() {
        let (l, hmq, client, child) = setup();
        let last_error = |l: &Loader| call(l, 0, ORD_WIN_GET_LAST_ERROR, &[0]) & 0xFFFF;
        assert_eq!(create(&l, 0xDEAD, (0, 0, 1, 1), 0, 0), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_HWND);
        assert_eq!(create(&l, client, (0, 0, 1, 1), 0x10, 0), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_FLAG);
        assert_eq!(call(&l, 7, ORD_WIN_CREATE_CURSOR, &[client, 0, 0, 1, 1, 0, 0]), 0);
        assert_eq!(last_error_of(&l, 7), PMERR_NO_MSG_QUEUE);
        // Destroying the window destroys its cursor.
        create(&l, child, (0, 0, 1, 1), 0, 0);
        l.shared.window_mgr.lock_or_recover().destroy_window(child);
        assert!(!l.shared.window_mgr.lock_or_recover().cursors.contains_key(&hmq));
        // Destroying the queue too.
        create(&l, client, (0, 0, 1, 1), 0, 0);
        assert!(l.shared.window_mgr.lock_or_recover().destroy_mq(0, hmq));
        assert!(l.shared.window_mgr.lock_or_recover().cursors.is_empty());
    }

    fn last_error_of(l: &Loader, tid: u32) -> u32 {
        call(l, tid, ORD_WIN_GET_LAST_ERROR, &[0]) & 0xFFFF
    }
}
