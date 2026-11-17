// SPDX-License-Identifier: GPL-3.0-only

//! Window tree and window-word calls that were missing from PMWIN:
//! `WinIsWindowVisible`, `WinQueryWindowTextLength`, `WinSetParent`,
//! `WinSetOwner`, `WinQueryWindowPtr` and `WinSetWindowPtr`.
//!
//! Behaviour follows the PM Reference pages of each call:
//! - `WinIsWindowVisible`: TRUE when the window "and all its parents have
//!   the WS_VISIBLE style bit set on" — the style bits only, not whether the
//!   window is obscured.
//! - `WinQueryWindowTextLength`: the text length "excluding any null
//!   termination character", in the bytes `WinQueryWindowText` would write
//!   (the window queue's codepage).
//! - `WinSetParent`: the new parent "cannot be a descendant of hwnd";
//!   `HWND_DESKTOP` makes a main window, `HWND_OBJECT` an object window.
//! - `WinSetOwner`: NULLHANDLE disowns the window.
//! - `WinQueryWindowPtr` / `WinSetWindowPtr`: the application's window words
//!   ("zero through cbWindowData - 4") or `QWP_PFNWP`, the window procedure.
//!
//! Unverified (no source): an `HWND_OBJECT` ancestor makes a window not
//! visible (object windows are never shown); `WinSetParent` with a
//! descendant fails with `PMERR_INVALID_HWND`; `WinSetWindowPtr(QWP_PFNWP)`
//! replaces the window procedure as `WinSubclassWindow` does; the window
//! position stays relative to the new parent (the reference does not say).

use super::constants::{
    HWND_DESKTOP, HWND_OBJECT, PMERR_INVALID_HWND, PMERR_PARAMETER_OUT_OF_RANGE, QWP_PFNWP,
};
use super::mutex_ext::MutexExt;
use super::pm_types::WindowManager;
use super::ApiResult;

/// Whether `hwnd` stands for the desktop as a parent: `HWND_DESKTOP`, or 0
/// for windows created before a parent was recorded.
fn is_desktop(hwnd: u32) -> bool {
    hwnd == 0 || hwnd == HWND_DESKTOP
}

impl WindowManager {
    /// `WinIsWindowVisible`: `hwnd` and every parent up to the desktop have
    /// `WS_VISIBLE` (`OS2Window::visible`). The desktop itself is visible;
    /// an unknown window or one under `HWND_OBJECT` is not.
    pub fn is_window_visible(&self, hwnd: u32) -> bool {
        let mut h = hwnd;
        // A tree is never deeper than the number of windows.
        for _ in 0..=self.windows().count() {
            if is_desktop(h) { return true; }
            match self.get_window(h) {
                Some(w) if w.visible => h = w.parent,
                _ => return false,
            }
        }
        false
    }

    /// `WinSetParent`: move `hwnd` under `new_parent` (`HWND_DESKTOP`,
    /// `HWND_OBJECT` or a window), on top of its new siblings. A main
    /// window that is a frame or dialog enters the Z-order (raised if
    /// visible); one that stops being a main window leaves it. Returns
    /// `false` when `new_parent` is `hwnd` or one of its descendants, or
    /// either is unknown.
    pub fn set_parent(&mut self, hwnd: u32, new_parent: u32) -> bool {
        let Some(old_parent) = self.get_window(hwnd).map(|w| w.parent) else { return false };
        let to_window = !is_desktop(new_parent) && new_parent != HWND_OBJECT;
        if to_window && (!self.is_window(new_parent) || self.is_same_or_descendant(new_parent, hwnd)) {
            return false;
        }
        if let Some(p) = self.get_window_mut(old_parent) {
            p.children.retain(|&c| c != hwnd);
        }
        if let Some(p) = self.get_window_mut(new_parent) {
            // The end of `children` is the top of the siblings (QW_TOP).
            p.children.push(hwnd);
        }
        let visible = {
            let w = self.get_window_mut(hwnd).expect("checked above");
            w.parent = new_parent;
            w.visible
        };
        let main_frame = self.is_frame(hwnd) || self.is_dialog(hwnd);
        if main_frame && is_desktop(new_parent) {
            if visible { self.z_push_top(hwnd); }
        } else if self.z_order.contains(&hwnd) {
            self.z_remove(hwnd);
        }
        self.mark_all_dirty();
        true
    }
}

impl super::Loader {
    /// WinIsWindowVisible(HWND hwnd) -> BOOL
    pub(super) fn win_is_window_visible(&self, tid: u32, hwnd: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        ApiResult::Normal(self.shared.window_mgr.lock_or_recover().is_window_visible(hwnd) as u32)
    }

    /// WinQueryWindowTextLength(HWND hwnd) -> LONG: the bytes of the window
    /// text in its queue's codepage, without the terminating null.
    pub(super) fn win_query_window_text_length(&self, tid: u32, hwnd: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let cp = self.window_codepage(hwnd);
        let text = self.shared.window_mgr.lock_or_recover().get_window(hwnd)
            .map(|w| w.text.clone()).unwrap_or_default();
        ApiResult::Normal(super::codepage::cp_encode(&text, cp).len() as u32)
    }

    /// WinSetParent(HWND hwnd, HWND hwndNewParent, BOOL fRedraw) -> BOOL.
    /// Warpine always redraws (the frames are composited), so `fRedraw`
    /// only matters to PM's device updates.
    pub(super) fn win_set_parent(&self, tid: u32, hwnd: u32, new_parent: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) || !self.pm_check_hwnd(tid, new_parent, false) {
            return ApiResult::Normal(0);
        }
        if !self.shared.window_mgr.lock_or_recover().set_parent(hwnd, new_parent) {
            // A descendant (or the window itself) as the new parent.
            return self.pm_fail(tid, PMERR_INVALID_HWND);
        }
        ApiResult::Normal(1)
    }

    /// WinSetOwner(HWND hwnd, HWND hwndNewOwner) -> BOOL; NULLHANDLE
    /// disowns the window.
    pub(super) fn win_set_owner(&self, tid: u32, hwnd: u32, new_owner: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) || !self.pm_check_hwnd(tid, new_owner, true) {
            return ApiResult::Normal(0);
        }
        if let Some(w) = self.shared.window_mgr.lock_or_recover().get_window_mut(hwnd) {
            w.owner = new_owner;
        }
        ApiResult::Normal(1)
    }

    /// Whether `index` is valid for `WinQueryWindowPtr` / `WinSetWindowPtr`
    /// on `hwnd`: `QWP_PFNWP`, or a pointer inside the application's window
    /// words (the other reserved negative indices are not pointers).
    fn pm_check_window_ptr(&self, tid: u32, hwnd: u32, index: i32) -> bool {
        if index == QWP_PFNWP { return self.pm_check_hwnd(tid, hwnd, false); }
        if index < 0 {
            if !self.pm_check_hwnd(tid, hwnd, false) { return false; }
            self.pm_error(tid, PMERR_PARAMETER_OUT_OF_RANGE);
            return false;
        }
        self.pm_check_window_word(tid, hwnd, index, 4)
    }

    /// WinQueryWindowPtr(HWND hwnd, LONG index) -> PVOID; NULL on error.
    /// The application's words are shared with `WinQueryWindowULong`.
    pub(super) fn win_query_window_ptr(&self, tid: u32, hwnd: u32, index: i32) -> ApiResult {
        if !self.pm_check_window_ptr(tid, hwnd, index) { return ApiResult::Normal(0); }
        let wm = self.shared.window_mgr.lock_or_recover();
        let Some(w) = wm.get_window(hwnd) else { return ApiResult::Normal(0) };
        ApiResult::Normal(if index == QWP_PFNWP {
            w.pfn_wp
        } else {
            w.window_ulong.get(&index).copied().unwrap_or(0)
        })
    }

    /// WinSetWindowPtr(HWND hwnd, LONG index, PVOID p) -> BOOL.
    /// `QWP_PFNWP` replaces the window procedure.
    pub(super) fn win_set_window_ptr(&self, tid: u32, hwnd: u32, index: i32, p: u32) -> ApiResult {
        if !self.pm_check_window_ptr(tid, hwnd, index) { return ApiResult::Normal(0); }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(w) = wm.get_window_mut(hwnd) else { return ApiResult::Normal(0) };
        if index == QWP_PFNWP {
            w.pfn_wp = p;
        } else {
            w.window_ulong.insert(index, p);
        }
        ApiResult::Normal(1)
    }
}

#[cfg(test)]
mod tests {
    use super::super::constants::*;
    use super::super::mutex_ext::MutexExt;
    use super::super::pm_error::error_id;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::{ApiResult, Loader};

    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_IS_WINDOW_VISIBLE: u32 = 775;
    const ORD_WIN_QUERY_WINDOW: u32 = 834;
    const ORD_WIN_QUERY_WINDOW_PTR: u32 = 839;
    const ORD_WIN_QUERY_WINDOW_TEXT_LENGTH: u32 = 842;
    const ORD_WIN_QUERY_WINDOW_ULONG: u32 = 843;
    const ORD_WIN_SET_OWNER: u32 = 864;
    const ORD_WIN_SET_PARENT: u32 = 865;
    const ORD_WIN_SET_WINDOW_PTR: u32 = 876;

    /// A handle that is no window.
    const BAD_HWND: u32 = 0xDEAD;

    fn call(loader: &Loader, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
        match loader.handle_pmwin_call(&mut vcpu, 0, ordinal) {
            ApiResult::Normal(v) => v,
            other => panic!("PMWIN ordinal {} returned {:?}", ordinal, other),
        }
    }

    fn last_error(loader: &Loader) -> u32 {
        call(loader, ORD_WIN_GET_LAST_ERROR, &[MOCK_HAB])
    }

    /// A frame on the desktop with a client, a child of the client and a
    /// second frame: (frame, client, child, frame2).
    fn tree(loader: &Loader) -> (u32, u32, u32, u32) {
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hmq = wm.create_mq(850);
        wm.tid_to_hmq.insert(0, hmq);
        wm.register_class("App".into(), 0x1234, 0);
        let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        let child = wm.create_window("App".into(), client, hmq);
        let frame2 = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client2 = wm.create_window("App".into(), frame2, hmq);
        wm.frame_to_client.insert(frame, client);
        wm.frame_to_client.insert(frame2, client2);
        for h in [frame, client, child, frame2, client2] {
            wm.get_window_mut(h).unwrap().visible = true;
        }
        wm.z_push_top(frame);
        wm.z_push_top(frame2);
        (frame, client, child, frame2)
    }

    #[test]
    fn a_window_is_visible_only_when_all_its_parents_are() {
        let loader = Loader::new_mock();
        let (frame, client, child, _) = tree(&loader);
        assert_eq!(call(&loader, ORD_WIN_IS_WINDOW_VISIBLE, &[child]), 1);
        loader.shared.window_mgr.lock_or_recover().get_window_mut(frame).unwrap().visible = false;
        assert_eq!(call(&loader, ORD_WIN_IS_WINDOW_VISIBLE, &[child]), 0, "the frame is hidden");
        assert_eq!(call(&loader, ORD_WIN_IS_WINDOW_VISIBLE, &[client]), 0);
        loader.shared.window_mgr.lock_or_recover().get_window_mut(frame).unwrap().visible = true;
        loader.shared.window_mgr.lock_or_recover().get_window_mut(child).unwrap().visible = false;
        assert_eq!(call(&loader, ORD_WIN_IS_WINDOW_VISIBLE, &[child]), 0, "its own bit");
        assert_eq!(call(&loader, ORD_WIN_IS_WINDOW_VISIBLE, &[client]), 1);
        assert_eq!(call(&loader, ORD_WIN_IS_WINDOW_VISIBLE, &[BAD_HWND]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
    }

    #[test]
    fn text_length_counts_bytes_without_the_null() {
        let loader = Loader::new_mock();
        let (frame, _, child, _) = tree(&loader);
        loader.shared.window_mgr.lock_or_recover().get_window_mut(frame).unwrap().text = "Grüße".into();
        // CP850: one byte per character.
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_TEXT_LENGTH, &[frame]), 5);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_TEXT_LENGTH, &[child]), 0, "no text");
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_TEXT_LENGTH, &[BAD_HWND]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
    }

    #[test]
    fn set_parent_moves_a_window_in_the_tree_and_the_z_order() {
        let loader = Loader::new_mock();
        let (frame, client, child, frame2) = tree(&loader);
        let client2 = loader.shared.window_mgr.lock_or_recover().frame_to_client[&frame2];
        // A child moves to another client, on top of its new siblings.
        assert_eq!(call(&loader, ORD_WIN_SET_PARENT, &[child, client2, 1]), 1);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW, &[child, QW_PARENT as u32]), client2);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW, &[client2, QW_TOP as u32]), child);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW, &[client, QW_TOP as u32]), 0, "left the old parent");
        // A frame made a child leaves the Z-order; back on the desktop it is a
        // main window again, raised.
        assert_eq!(call(&loader, ORD_WIN_SET_PARENT, &[frame2, client, 0]), 1);
        assert!(!loader.shared.window_mgr.lock_or_recover().z_order.contains(&frame2));
        assert_eq!(call(&loader, ORD_WIN_SET_PARENT, &[frame2, HWND_DESKTOP, 1]), 1);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().z_order.last(), Some(&frame2));
        // An object window is never visible.
        assert_eq!(call(&loader, ORD_WIN_SET_PARENT, &[frame2, HWND_OBJECT, 1]), 1);
        assert!(!loader.shared.window_mgr.lock_or_recover().z_order.contains(&frame2));
        assert_eq!(call(&loader, ORD_WIN_IS_WINDOW_VISIBLE, &[frame2]), 0);
        // Not under itself or a descendant; not under a bad handle.
        assert_eq!(call(&loader, ORD_WIN_SET_PARENT, &[frame, client, 1]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
        assert_eq!(call(&loader, ORD_WIN_SET_PARENT, &[frame, frame, 1]), 0);
        assert_eq!(call(&loader, ORD_WIN_SET_PARENT, &[frame, BAD_HWND, 1]), 0);
        assert_eq!(call(&loader, ORD_WIN_SET_PARENT, &[BAD_HWND, HWND_DESKTOP, 1]), 0);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW, &[frame, QW_PARENT as u32]), HWND_DESKTOP, "unchanged");
    }

    #[test]
    fn set_owner_changes_and_clears_the_owner() {
        let loader = Loader::new_mock();
        let (frame, _, child, frame2) = tree(&loader);
        assert_eq!(call(&loader, ORD_WIN_SET_OWNER, &[frame2, frame]), 1);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW, &[frame2, QW_OWNER as u32]), frame);
        assert_eq!(call(&loader, ORD_WIN_SET_OWNER, &[frame2, 0]), 1, "disowned");
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW, &[frame2, QW_OWNER as u32]), 0);
        assert_eq!(call(&loader, ORD_WIN_SET_OWNER, &[child, BAD_HWND]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
        assert_eq!(call(&loader, ORD_WIN_SET_OWNER, &[BAD_HWND, frame]), 0);
    }

    #[test]
    fn window_ptr_shares_the_window_words_and_reaches_the_window_procedure() {
        let loader = Loader::new_mock();
        let (_, client, _, _) = tree(&loader);
        loader.shared.window_mgr.lock_or_recover().set_class_window_data("App", 8);
        assert_eq!(call(&loader, ORD_WIN_SET_WINDOW_PTR, &[client, 4, 0x0012_3400]), 1);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_PTR, &[client, 4]), 0x0012_3400);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_ULONG, &[client, 4]), 0x0012_3400, "same words");
        // QWP_PFNWP: the window procedure.
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_PTR, &[client, QWP_PFNWP as u32]), 0x1234);
        assert_eq!(call(&loader, ORD_WIN_SET_WINDOW_PTR, &[client, QWP_PFNWP as u32, 0x5678]), 1);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().get_window(client).unwrap().pfn_wp, 0x5678);
        // Past cbWindowData - 4, or another reserved index: out of range.
        for index in [5i32, 8, QWL_STYLE] {
            assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_PTR, &[client, index as u32]), 0, "{index}");
            assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
            assert_eq!(call(&loader, ORD_WIN_SET_WINDOW_PTR, &[client, index as u32, 1]), 0, "{index}");
            assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
        }
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_PTR, &[BAD_HWND, 0]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
        assert_eq!(call(&loader, ORD_WIN_SET_WINDOW_PTR, &[BAD_HWND, QWP_PFNWP as u32, 1]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
    }
}
