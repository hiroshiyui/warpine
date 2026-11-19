// SPDX-License-Identifier: GPL-3.0-only

//! Painting styles and painting control: class styles (`CS_SIZEREDRAW`,
//! `CS_SYNCPAINT`, `CS_CLIPCHILDREN` …), `WS_SYNCPAINT`,
//! `WM_ERASEBACKGROUND` and `WinLockWindowUpdate`.
//!
//! * **Class styles.** `CS_SYNCPAINT`, `CS_SAVEBITS`, `CS_PARENTCLIP`,
//!   `CS_CLIPSIBLINGS` and `CS_CLIPCHILDREN` have the values of the
//!   `WS_*` styles of the same names; a window of a class registered with
//!   them gets those window styles (the PM Reference, WinRegisterClass).
//!   `WS_CLIPCHILDREN` keeps an invalidated window's children valid
//!   (`pm_paint.rs`). `WS_CLIPSIBLINGS`, `WS_PARENTCLIP` and `WS_SAVEBITS`
//!   are kept but change nothing: Warpine draws every window of a frame
//!   into the frame's one surface without clipping a window to itself, its
//!   siblings or its parent (ADR-0011), and composites frames itself, so
//!   there is nothing to save under a window.
//! * **`CS_SIZEREDRAW`**: a window whose class has it is invalidated as a
//!   whole whenever its size changes; any other window only gets what the
//!   new size uncovers (nothing when it shrinks) — its pels keep their
//!   place at the bottom left, as in PM. Built-in controls and dialogs
//!   repaint as a whole (unverified: which of PM's preregistered classes
//!   have the style).
//! * **`WS_SYNCPAINT`** (and `CS_SYNCPAINT`): PM sends `WM_PAINT` at once
//!   when such a window is invalidated. Warpine makes the call when the
//!   invalidating API returns to the window's thread, before its result
//!   reaches the caller — through the owner-call chain of `pm_owner.rs` —
//!   and keeps the queued `WM_PAINT`, which the window's `WinBeginPaint`
//!   withdraws.
//! * **`WM_ERASEBACKGROUND`**: when a standard frame repaints — it is
//!   invalidated, shown, or its client changes size — the frame sends its
//!   client `WM_ERASEBACKGROUND` (mp1 = a presentation space on the client,
//!   mp2 = the `RECTL` to erase, the client's update rectangle); TRUE has
//!   the frame fill the rectangle with `SYSCLR_WINDOW`, FALSE leaves it to
//!   the client (`WinDefWindowProc` returns FALSE). Like `WS_SYNCPAINT` it
//!   is made before the client's `WM_PAINT` is read, through the owner-call
//!   chain. Invalidating the client alone sends nothing, as in PM.
//! * **`WinLockWindowUpdate`** locks one window (or the desktop): drawing
//!   into its frame's surface is dropped until it is unlocked, then the
//!   frame (every frame for the desktop) is invalidated with its children
//!   so that what was not drawn is painted. Warpine locks the whole frame
//!   the window draws into — siblings and parent included — since windows
//!   share their frame's surface.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_owner::{OwnerCall, OwnerCallKind, OwnerSetup};
use super::pm_paint::rect_difference;
use super::pm_rect::Rectl;
use super::pm_types::{PsOrigin, WindowManager};
use super::{ApiResult, Loader, Painter};

// ── <pmwin.h> ────────────────────────────────────────────────────────────────

/// `mp1` = HPS, `mp2` = PRECTL; reply TRUE to have the frame erase.
pub const WM_ERASEBACKGROUND: u32 = 0x004f;

// Kept in the window style; nothing to do in Warpine (see above).
#[cfg_attr(not(test), allow(dead_code))]
pub const WS_CLIPSIBLINGS: u32 = 0x1000_0000;
#[cfg_attr(not(test), allow(dead_code))]
pub const WS_PARENTCLIP: u32 = 0x0800_0000;
#[cfg_attr(not(test), allow(dead_code))]
pub const WS_SAVEBITS: u32 = 0x0400_0000;
pub const WS_SYNCPAINT: u32 = 0x0200_0000;

/// Not a window style (`WM_MOVE` notification is not implemented).
#[cfg_attr(not(test), allow(dead_code))]
pub const CS_MOVENOTIFY: u32 = 0x0000_0001;
pub const CS_SIZEREDRAW: u32 = 0x0000_0004;
pub const CS_SYNCPAINT: u32 = 0x0200_0000;
pub const CS_SAVEBITS: u32 = 0x0400_0000;
pub const CS_PARENTCLIP: u32 = 0x0800_0000;
pub const CS_CLIPSIBLINGS: u32 = 0x1000_0000;
pub const CS_CLIPCHILDREN: u32 = 0x2000_0000;

/// The class styles that are window styles too.
const CS_WINDOW_STYLES: u32 = CS_SYNCPAINT | CS_SAVEBITS | CS_PARENTCLIP | CS_CLIPSIBLINGS | CS_CLIPCHILDREN;

/// Words of the `RECTL` passed with `WM_ERASEBACKGROUND`.
const RECTL_WORDS: usize = 4;

impl WindowManager {
    /// The window styles windows of class `class` get from its class style.
    pub fn class_window_style(&self, class: &str) -> u32 {
        self.get_class(class).map_or(0, |c| c.style & CS_WINDOW_STYLES)
    }

    /// Whether `hwnd` repaints as a whole when its size changes.
    fn size_redraw(&self, hwnd: u32) -> bool {
        let Some(w) = self.get_window(hwnd) else { return false };
        w.pfn_wp == 0 || self.is_dialog(hwnd)
            || self.get_class(&w.class_name).is_some_and(|c| c.style & CS_SIZEREDRAW != 0)
    }

    /// `hwnd` changed size from `old` to `new`: invalidate all of it
    /// (`CS_SIZEREDRAW`, or it had no size) or what the new size uncovers.
    /// A frame's client also gets `WM_ERASEBACKGROUND` (the frame repaints).
    pub fn invalidate_resized(&mut self, hwnd: u32, old: (i32, i32), new: (i32, i32)) {
        let target = self.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
        if self.size_redraw(target) || old.0 <= 0 || old.1 <= 0 {
            self.invalidate_window_only(target);
        } else {
            let (o, n) = (Rectl::new(0, 0, old.0, old.1), Rectl::new(0, 0, new.0, new.1));
            for part in rect_difference(&n, &o) {
                self.invalidate(target, Some(part), false);
            }
        }
        let frame = self.client_to_frame(target);
        if frame != target && self.is_frame(frame) {
            self.queue_erase_background(target);
        }
    }

    /// Have the frame of `client` send it `WM_ERASEBACKGROUND` before it
    /// paints.
    pub fn queue_erase_background(&mut self, client: u32) {
        if self.get_window(client).is_some_and(|w| w.pfn_wp != 0) {
            self.queue_owner_call(OwnerCall { control: client, kind: OwnerCallKind::EraseBackground });
        }
    }

    /// A `WS_SYNCPAINT` window `hwnd` with a window procedure was
    /// invalidated: have it paint before the invalidating API returns.
    pub fn queue_sync_paint(&mut self, hwnd: u32) {
        let sync = self.get_window(hwnd).is_some_and(|w| w.pfn_wp != 0 && w.style & WS_SYNCPAINT != 0);
        if sync {
            self.queue_owner_call(OwnerCall { control: hwnd, kind: OwnerCallKind::SyncPaint });
        }
    }

    /// Whether drawing into `frame`'s surface is suppressed by
    /// `WinLockWindowUpdate`.
    pub fn is_update_locked(&self, frame: u32) -> bool {
        match self.update_locked {
            0 => false,
            HWND_DESKTOP => true,
            locked => self.is_window(locked) && self.find_frame_for_hwnd(locked) == frame,
        }
    }

    /// The frames to repaint when `locked` is unlocked.
    fn locked_frames(&self, locked: u32) -> Vec<u32> {
        match locked {
            0 => Vec::new(),
            HWND_DESKTOP => self.z_order.clone(),
            h if self.is_window(h) => vec![self.find_frame_for_hwnd(h)],
            _ => Vec::new(),
        }
    }
}

impl Loader {
    /// WinLockWindowUpdate(HWND hwndDesktop, HWND hwndLockUpdate) -> BOOL:
    /// lock `lock` (NULLHANDLE unlocks). FALSE while another window is
    /// locked (unverified: whether PM sets an error).
    pub(super) fn win_lock_window_update(&self, tid: u32, desktop: u32, lock: u32) -> ApiResult {
        if !self.check_desktop_arg(tid, desktop) { return ApiResult::Normal(0); }
        if lock != 0 && lock != HWND_DESKTOP && !self.pm_check_hwnd(tid, lock, false) {
            return ApiResult::Normal(0);
        }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let current = wm.update_locked;
        let current = if current == HWND_DESKTOP || wm.is_window(current) { current } else { 0 };
        if lock != 0 && current != 0 && current != lock {
            return ApiResult::Normal(0);
        }
        wm.update_locked = lock;
        if lock == 0 {
            for frame in wm.locked_frames(current) {
                wm.invalidate(frame, None, true);
            }
            wm.mark_all_dirty();
        }
        ApiResult::Normal(1)
    }

    /// `WM_ERASEBACKGROUND` to `client` with a presentation space on it and
    /// its update rectangle; skipped when it has nothing to paint any more.
    pub(super) fn erase_background_setup(&self, client: u32) -> OwnerSetup {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(w) = wm.get_window(client) else { return OwnerSetup::Skip };
        let (wnd_proc, update) = (w.pfn_wp, super::pm_paint::bounds_of(&w.update));
        if wnd_proc == 0 || update.is_empty() {
            return OwnerSetup::Skip;
        }
        let hps = wm.create_ps(client);
        if let Some(ps) = wm.ps_map.get_mut(&hps) {
            ps.origin = PsOrigin::Cached;
        }
        let block = vec![update.x_left as u32, update.y_bottom as u32, update.x_right as u32, update.y_top as u32];
        OwnerSetup::Call { wnd_proc, owner: client, msg: WM_ERASEBACKGROUND, mp1: hps, mp2: 0, block, hps }
    }

    /// `WM_ERASEBACKGROUND` returned `result`: TRUE fills the rectangle (as
    /// the client may have changed it, within the client) with
    /// `SYSCLR_WINDOW`. The presentation space goes.
    pub(super) fn erase_background_finish(&self, client: u32, hps: u32, result: u32, block: &[u32]) {
        let (frame, origin, size, color) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if hps != 0 { wm.remove_ps(hps); }
            let (ax, ay, cx, cy) = wm.get_abs_rect_in_frame(client);
            let color = wm.sys_colors.get(SYSCLR_WINDOW).unwrap_or(super::CONTROL_FIELD_COLOR);
            (wm.find_frame_for_hwnd(client), (ax, ay), (cx, cy), color)
        };
        if result == 0 || block.len() < RECTL_WORDS {
            return;
        }
        let r = Rectl::new(block[0] as i32, block[1] as i32, block[2] as i32, block[3] as i32);
        let Some(r) = r.intersect(&Rectl::new(0, 0, size.0, size.1)) else { return };
        if r.is_empty() { return; }
        Painter::new(self, frame, client).fill(
            (origin.0 + r.x_left, origin.1 + r.y_bottom, origin.0 + r.x_right - 1, origin.1 + r.y_top - 1), color);
    }

    /// `WM_PAINT` sent to `WS_SYNCPAINT` window `hwnd` while it has
    /// something to paint.
    pub(super) fn sync_paint_setup(&self, hwnd: u32) -> OwnerSetup {
        let wm = self.shared.window_mgr.lock_or_recover();
        match wm.get_window(hwnd) {
            Some(w) if w.pfn_wp != 0 && !w.update.is_empty() => OwnerSetup::Call {
                wnd_proc: w.pfn_wp, owner: hwnd, msg: WM_PAINT, mp1: 0, mp2: 0, block: Vec::new(), hps: 0,
            },
            _ => OwnerSetup::Skip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_types::OS2Message;
    use super::super::vm_backend::mock::MockVcpu;
    use crate::gui::surface::DrawOp;

    const ORD_WIN_CREATE_MSG_QUEUE: u32 = 716;
    const ORD_WIN_LOCK_WINDOW_UPDATE: u32 = 784;
    const ORD_WIN_INVALIDATE_RECT: u32 = 765;
    const PROC: u32 = 0x5000;

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

    /// A queue and a 200×100 standard frame with a client of class `class`
    /// (procedure `PROC`), registered with `class_style`.
    fn setup(class_style: u32) -> (Loader, u32, u32, u32) {
        let loader = Loader::new_mock();
        let hmq = call(&loader, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        let (frame, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.register_class("App".into(), PROC, class_style);
            let frame = wm.create_window("App".into(), 0, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            for h in [frame, client] {
                let w = wm.get_window_mut(h).unwrap();
                (w.cx, w.cy, w.visible) = (200, 100, true);
            }
            (frame, client)
        };
        (loader, hmq, frame, client)
    }

    fn clear(loader: &Loader, hmq: u32, client: u32) {
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        wm.begin_paint(client);
        wm.owner_calls.clear();
        wm.get_mq(hmq).unwrap().lock_or_recover().messages.clear();
    }

    fn update(loader: &Loader, hwnd: u32) -> Rectl {
        loader.shared.window_mgr.lock_or_recover().update_rect(hwnd)
    }

    fn calls(loader: &Loader) -> Vec<OwnerCall> {
        loader.shared.window_mgr.lock_or_recover().owner_calls.clone()
    }

    #[test]
    fn class_styles_become_window_styles() {
        let (loader, ..) = setup(CS_SIZEREDRAW | CS_SYNCPAINT | CS_CLIPCHILDREN | CS_MOVENOTIFY);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.class_window_style("App"), WS_SYNCPAINT | WS_CLIPCHILDREN);
        assert_eq!(wm.class_window_style("Unknown"), 0);
        assert_eq!(CS_CLIPSIBLINGS, WS_CLIPSIBLINGS);
        assert_eq!((CS_PARENTCLIP, CS_SAVEBITS), (WS_PARENTCLIP, WS_SAVEBITS));
    }

    #[test]
    fn size_redraw_invalidates_all_or_only_the_uncovered_part() {
        for (style, grown, shrunk) in [
            (CS_SIZEREDRAW, Rectl::new(0, 0, 300, 100), Rectl::new(0, 0, 150, 100)),
            (0, Rectl::new(200, 0, 300, 100), Rectl::new(0, 0, 0, 0)),
        ] {
            let (loader, hmq, _frame, client) = setup(style);
            let resize = |old: (i32, i32), new: (i32, i32)| {
                let mut wm = loader.shared.window_mgr.lock_or_recover();
                wm.get_window_mut(client).unwrap().cx = new.0;
                wm.post_size(client, old, new);
            };
            clear(&loader, hmq, client);
            resize((200, 100), (300, 100));
            assert_eq!(update(&loader, client), grown, "style {style:#x}: grown");
            clear(&loader, hmq, client);
            resize((300, 100), (150, 100));
            let r = update(&loader, client);
            assert!(r.is_empty() == shrunk.is_empty() && (r.is_empty() || r == shrunk), "style {style:#x}: shrunk {r:?}");
        }
    }

    #[test]
    fn a_frame_repaint_erases_the_clients_background_first() {
        let (loader, hmq, frame, client) = setup(0);
        clear(&loader, hmq, client);
        // Invalidating the client alone sends nothing.
        assert_eq!(call(&loader, ORD_WIN_INVALIDATE_RECT, &[client, 0, 0]), 1);
        assert!(calls(&loader).is_empty());
        assert_eq!(call(&loader, ORD_WIN_INVALIDATE_RECT, &[frame, 0, 0]), 1);
        assert_eq!(calls(&loader), vec![OwnerCall { control: client, kind: OwnerCallKind::EraseBackground }]);
        let setup = loader.erase_background_setup(client);
        let OwnerSetup::Call { wnd_proc, owner, msg, mp1, block, hps, .. } = setup else { panic!("{setup:?}") };
        assert_eq!((wnd_proc, owner, msg, mp1 == hps, block), (PROC, client, WM_ERASEBACKGROUND, true, vec![0, 0, 200, 100]));
        assert!(loader.shared.window_mgr.lock_or_recover().ps_map.contains_key(&hps));
        // TRUE: the frame fills the rectangle with SYSCLR_WINDOW.
        loader.erase_background_finish(client, hps, 1, &[10, 10, 20, 30]);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.ps_map.contains_key(&hps));
        let s = wm.surfaces.get(&frame).unwrap().clone();
        let color = wm.sys_colors.get(SYSCLR_WINDOW).unwrap();
        drop(wm);
        let s = s.lock_or_recover();
        assert_eq!((s.pel(10, 10), s.pel(19, 29)), (Some(color), Some(color)));
        drop(s);
        // Nothing left to paint: no call.
        clear(&loader, hmq, client);
        assert_eq!(loader.erase_background_setup(client), OwnerSetup::Skip);
    }

    #[test]
    fn erase_false_leaves_the_pels() {
        let (loader, hmq, frame, client) = setup(0);
        clear(&loader, hmq, client);
        loader.draw(frame, DrawOp::Box { x1: 0, y1: 0, x2: 199, y2: 99, color: 0x123456, fill: true,
                                         mix: crate::gui::Mix::Overpaint });
        loader.erase_background_finish(client, 0, 0, &[0, 0, 200, 100]);
        let s = loader.shared.window_mgr.lock_or_recover().surfaces.get(&frame).unwrap().clone();
        assert_eq!(s.lock_or_recover().pel(5, 5), Some(0x123456));
    }

    #[test]
    fn sync_paint_windows_paint_before_the_api_returns() {
        let (loader, hmq, _frame, client) = setup(CS_SYNCPAINT);
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let style = wm.class_window_style("App");
            wm.get_window_mut(client).unwrap().style |= style;
        }
        clear(&loader, hmq, client);
        assert_eq!(call(&loader, ORD_WIN_INVALIDATE_RECT, &[client, 0, 0]), 1);
        assert_eq!(calls(&loader), vec![OwnerCall { control: client, kind: OwnerCallKind::SyncPaint }]);
        let OwnerSetup::Call { msg, owner, .. } = loader.sync_paint_setup(client) else { panic!() };
        assert_eq!((msg, owner), (WM_PAINT, client));
        // The queued WM_PAINT stays until the window paints.
        let wm = loader.shared.window_mgr.lock_or_recover();
        let queued: Vec<OS2Message> = wm.get_mq(hmq).unwrap().lock_or_recover().messages.iter().cloned().collect();
        assert!(queued.iter().any(|m| m.msg == WM_PAINT && m.hwnd == client));
        drop(wm);
        clear(&loader, hmq, client);
        assert_eq!(loader.sync_paint_setup(client), OwnerSetup::Skip);
    }

    #[test]
    fn locked_windows_draw_nothing_until_unlocked() {
        let (loader, hmq, frame, client) = setup(0);
        let other = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let f2 = wm.create_window("App".into(), 0, hmq);
            let c2 = wm.create_window("App".into(), f2, hmq);
            wm.frame_to_client.insert(f2, c2);
            f2
        };
        clear(&loader, hmq, client);
        let fill = |f: u32, color: u32| loader.draw(f, DrawOp::Box { x1: 0, y1: 0, x2: 9, y2: 9, color,
                                                                    fill: true, mix: crate::gui::Mix::Overpaint });
        fill(frame, 0x0000FF);
        assert_eq!(call(&loader, ORD_WIN_LOCK_WINDOW_UPDATE, &[HWND_DESKTOP, client]), 1);
        assert_eq!(call(&loader, ORD_WIN_LOCK_WINDOW_UPDATE, &[HWND_DESKTOP, other]), 0, "one lock at a time");
        assert_eq!(call(&loader, ORD_WIN_LOCK_WINDOW_UPDATE, &[HWND_DESKTOP, client]), 1, "locking it again");
        fill(frame, 0xFF0000);
        let pel = |f: u32| loader.shared.window_mgr.lock_or_recover().surfaces.get(&f).unwrap().lock_or_recover().pel(1, 1);
        assert_eq!(pel(frame), Some(0x0000FF), "drawing dropped while locked");
        fill(other, 0x00FF00);
        assert_eq!(pel(other), Some(0x00FF00), "other frames draw");
        assert_eq!(call(&loader, ORD_WIN_LOCK_WINDOW_UPDATE, &[HWND_DESKTOP, 0]), 1);
        assert_eq!(update(&loader, client), Rectl::new(0, 0, 200, 100), "unlocked: repainted");
        fill(frame, 0xFF0000);
        assert_eq!(pel(frame), Some(0xFF0000));
        // The desktop locks every frame.
        assert_eq!(call(&loader, ORD_WIN_LOCK_WINDOW_UPDATE, &[HWND_DESKTOP, HWND_DESKTOP]), 1);
        fill(other, 0x111111);
        assert_eq!(pel(other), Some(0x00FF00));
        assert_eq!(call(&loader, ORD_WIN_LOCK_WINDOW_UPDATE, &[HWND_DESKTOP, 0]), 1);
        assert_eq!(call(&loader, ORD_WIN_LOCK_WINDOW_UPDATE, &[HWND_DESKTOP, 0xBAD]), 0);
    }
}
