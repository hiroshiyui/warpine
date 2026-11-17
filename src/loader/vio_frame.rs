// SPDX-License-Identifier: GPL-3.0-only

//! The frame of a windowed VIO session: a text-mode program shown in a
//! window on the PM desktop, as OS/2 runs a window-compatible program in an
//! "OS/2 window" session.
//!
//! The frame belongs to the host, not to a guest message queue: it has a
//! title bar, a system-menu icon, minimize and maximize buttons and a sizing
//! border; its client is the text screen (`cols × 8` by `rows × 16` pels,
//! drawn by `gui::vio_window`), keys typed while it is active go to the VIO
//! program's keyboard queue, and closing it ends the program.
//!
//! The window is a view on the program's logical video buffer (LVB): IBM's
//! `MouGetPtrPos` reference says "For a text window (VIO) application, the
//! text window is a view on the larger logical video buffer (LVB)". It can
//! be made smaller than the screen (scroll bars then appear, drawn by
//! `gui::vio_window`) but not larger ([`vio_size_limits`]; Warpine's
//! choice). Maximizing fits it to the whole screen, up to the size of the
//! desktop ([`WindowManager::vio_maximized_rect`]). It follows the screen
//! size when the program changes mode (`VioSetMode`).
//!
//! The frame has no message queue, so its system commands are carried out
//! here at once ([`WindowManager::vio_minmax`]) rather than posted.

use super::constants::{
    default_frame_pos, desktop_size, frame_border_width, CHROME_TITLE_H, FCF_MAXBUTTON,
    FCF_MINBUTTON, FCF_SIZEBORDER, FCF_SYSMENU, FCF_TITLEBAR, HWND_DESKTOP, QWS_CXRESTORE,
    QWS_CYRESTORE, QWS_XRESTORE, QWS_YRESTORE, SWP_MAXIMIZE, SWP_RESTORE, WS_MAXIMIZED,
    WS_MINIMIZED,
};
use super::pm_minmax::WinRect;
use super::pm_types::WindowManager;

/// Width and height in pels of one text cell.
pub const VIO_CELL_W: i32 = 8;
pub const VIO_CELL_H: i32 = 16;

/// The frame-creation flags of a VIO window.
pub const VIO_FRAME_FLAGS: u32 =
    FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINBUTTON | FCF_MAXBUTTON;

/// Thickness of the VIO window's scroll bars (Warpine's own look).
pub const VIO_SCROLLBAR_W: i32 = 16;

/// Smallest frame width: room for the system-menu icon, the three title-bar
/// buttons and a little title (Warpine's choice; the same width as a
/// minimized frame).
pub const VIO_MIN_FRAME_CX: i32 = 160;
/// Smallest client height: a vertical scroll bar's two arrows and its thumb
/// (Warpine's choice).
pub const VIO_MIN_CLIENT_CY: i32 = 3 * VIO_SCROLLBAR_W;

/// The size of a VIO frame for a `cols × rows` text screen: the cells, the
/// border on the left, right and bottom, and the title bar on top (which the
/// top border is part of).
pub fn vio_frame_size(cols: u16, rows: u16) -> (i32, i32) {
    let bw = frame_border_width(VIO_FRAME_FLAGS);
    (cols as i32 * VIO_CELL_W + 2 * bw, rows as i32 * VIO_CELL_H + CHROME_TITLE_H + bw)
}

/// The client area (text view plus scroll bars) of a VIO frame `cx × cy`.
pub fn vio_client_size(cx: i32, cy: i32) -> (i32, i32) {
    let bw = frame_border_width(VIO_FRAME_FLAGS);
    ((cx - 2 * bw).max(0), (cy - CHROME_TITLE_H - bw).max(0))
}

/// Which scroll bars a client of `client_w × client_h` pels showing a
/// `cols × rows` screen needs: `(vertical, horizontal)`. A bar is needed for
/// a direction the view cannot show whole; the room one bar takes may make
/// the other one needed too.
pub fn vio_scroll_bars(client_w: i32, client_h: i32, cols: u16, rows: u16) -> (bool, bool) {
    let (tw, th) = (cols as i32 * VIO_CELL_W, rows as i32 * VIO_CELL_H);
    let (mut vbar, mut hbar) = (false, false);
    // Two rounds settle it: a bar can only make the other one needed.
    for _ in 0..2 {
        let view_w = client_w - if vbar { VIO_SCROLLBAR_W } else { 0 };
        let view_h = client_h - if hbar { VIO_SCROLLBAR_W } else { 0 };
        hbar |= view_w < tw;
        vbar |= view_h < th;
    }
    (vbar, hbar)
}

/// Smallest and largest size `(cx, cy)` of the frame of a `cols × rows`
/// screen. The largest shows the whole screen plus room for a scroll bar
/// in each direction; [`clamp_vio_size`] takes that room back where no bar
/// is needed, so the window never shows more than the whole screen
/// (Warpine's choice: what OS/2 does with a window larger than its LVB is
/// not documented).
pub fn vio_size_limits(cols: u16, rows: u16) -> ((i32, i32), (i32, i32)) {
    let full = vio_frame_size(cols, rows);
    let max = (full.0 + VIO_SCROLLBAR_W, full.1 + VIO_SCROLLBAR_W);
    let bw = frame_border_width(VIO_FRAME_FLAGS);
    let min = (VIO_MIN_FRAME_CX.min(full.0), (VIO_MIN_CLIENT_CY + CHROME_TITLE_H + bw).min(full.1));
    (min, max)
}

/// The frame size `(cx, cy)` of a `cols × rows` screen kept within
/// [`vio_size_limits`] and no wider (taller) than the whole screen unless a
/// vertical (horizontal) scroll bar takes the extra room.
pub fn clamp_vio_size(cx: i32, cy: i32, cols: u16, rows: u16) -> (i32, i32) {
    let (min, max) = vio_size_limits(cols, rows);
    let (cx, cy) = (cx.clamp(min.0, max.0), cy.clamp(min.1, max.1));
    let (cw, ch) = vio_client_size(cx, cy);
    let (vbar, hbar) = vio_scroll_bars(cw, ch, cols, rows);
    let full = vio_frame_size(cols, rows);
    (if vbar { cx } else { cx.min(full.0) }.max(min.0), if hbar { cy } else { cy.min(full.1) }.max(min.1))
}

/// The frame size for a `cols × rows` screen that also fits on the desktop
/// (its border may lie just outside it, as a maximized frame's does): the
/// whole screen if there is room, else as much as fits with scroll bars.
fn fit_on_desktop(cols: u16, rows: u16) -> (i32, i32) {
    let (dw, dh) = desktop_size();
    let bw = frame_border_width(VIO_FRAME_FLAGS);
    let (_, max) = vio_size_limits(cols, rows);
    clamp_vio_size(max.0.min(dw as i32 + 2 * bw), max.1.min(dh as i32 + 2 * bw), cols, rows)
}

impl WindowManager {
    /// Window class of a VIO frame and its client.
    pub const VIO_CLASS: &'static str = "#VioWindow";

    /// Create the VIO frame for a `cols × rows` screen titled `title`, on top
    /// of the desktop and active, and remember it as `vio_frame`. It shows
    /// the whole screen if the desktop has room. Returns the frame.
    pub fn create_vio_frame(&mut self, title: &str, cols: u16, rows: u16) -> u32 {
        let frame = self.create_window(Self::VIO_CLASS.to_string(), HWND_DESKTOP, 0);
        let client = self.create_window(Self::VIO_CLASS.to_string(), frame, 0);
        self.frame_to_client.insert(frame, client);
        let (cx, cy) = fit_on_desktop(cols, rows);
        let (x, y) = default_frame_pos(0, cx, cy, desktop_size());
        if let Some(w) = self.get_window_mut(frame) {
            (w.x, w.y, w.cx, w.cy) = (x, y, cx, cy);
            w.frame_flags = VIO_FRAME_FLAGS;
            w.visible = true;
            w.text = title.to_string();
        }
        if let Some(w) = self.get_window_mut(client) {
            w.visible = true;
        }
        self.layout_frame(frame);
        self.z_push_top(frame);
        self.focused_hwnd = frame;
        self.frame_surface(frame);
        self.vio_frame = frame;
        self.vio_screen = (cols, rows);
        frame
    }

    /// Fit the VIO frame to a `cols × rows` screen after a mode change,
    /// keeping its top-left corner where it is: a normal frame shows the
    /// whole new screen (as far as the desktop has room), a maximized one is
    /// maximized again for it, a minimized one is restored to that size
    /// later. Returns whether the screen size changed.
    pub fn fit_vio_frame(&mut self, cols: u16, rows: u16) -> bool {
        if self.vio_screen == (cols, rows) { return false; }
        self.vio_screen = (cols, rows);
        let frame = self.vio_frame;
        let Some(w) = self.get_window(frame) else { return true };
        let state = w.style & (WS_MINIMIZED | WS_MAXIMIZED);
        let (cx, cy) = fit_on_desktop(cols, rows);
        if state & WS_MINIMIZED != 0 {
            // Restoring brings it back at the new size, top edge in place.
            let top = self.restore_word(frame, QWS_YRESTORE, w.y) + self.restore_word(frame, QWS_CYRESTORE, w.cy);
            self.set_restore_word(frame, QWS_CXRESTORE, cx);
            self.set_restore_word(frame, QWS_CYRESTORE, cy);
            self.set_restore_word(frame, QWS_YRESTORE, top - cy);
            return true;
        }
        let target = if state & WS_MAXIMIZED != 0 {
            self.vio_maximized_rect()
        } else {
            // PM y counts from the bottom: keep the top edge in place.
            WinRect { x: w.x, y: w.y + w.cy - cy, cx, cy }
        };
        self.set_vio_rect(target);
        true
    }

    /// Move and size the VIO frame to `r` and lay it out again.
    pub fn set_vio_rect(&mut self, r: WinRect) {
        let frame = self.vio_frame;
        let Some(w) = self.get_window_mut(frame) else { return };
        (w.x, w.y, w.cx, w.cy) = (r.x, r.y, r.cx, r.cy);
        let (scx, scy) = self.layout_frame(frame);
        self.resize_surface(frame, scx, scy);
        self.mark_all_dirty();
    }

    /// Where a maximized VIO frame goes: the whole screen, up to the desktop
    /// size (border outside the desktop as for other maximized frames), with
    /// its top-left corner where the normal frame's is as far as that keeps
    /// it on the desktop (Warpine's choice: OS/2's placement is not
    /// documented).
    pub fn vio_maximized_rect(&self) -> WinRect {
        let frame = self.vio_frame;
        let (cols, rows) = self.vio_screen;
        let (cx, cy) = fit_on_desktop(cols, rows);
        let (dw, dh) = desktop_size();
        let (dw, dh) = (dw as i32, dh as i32);
        let bw = frame_border_width(VIO_FRAME_FLAGS);
        let (x, y, ncy) = self.get_window(frame).map_or((0, 0, cy), |w| {
            (self.restore_word(frame, QWS_XRESTORE, w.x), self.restore_word(frame, QWS_YRESTORE, w.y),
             self.restore_word(frame, QWS_CYRESTORE, w.cy))
        });
        let top = y + ncy;
        WinRect {
            x: x.min(dw + bw - cx).max(-bw),
            y: (top - cy).min(dh + bw - cy).max(-bw),
            cx, cy,
        }
    }

    /// Size `cx × cy` clamped for `frame` when it is the VIO frame
    /// ([`clamp_vio_size`] of its screen); `None` for other frames.
    pub fn vio_clamp_size(&self, frame: u32, cx: i32, cy: i32) -> Option<(i32, i32)> {
        (frame != 0 && frame == self.vio_frame)
            .then(|| clamp_vio_size(cx, cy, self.vio_screen.0, self.vio_screen.1))
    }

    /// Minimize, maximize or restore the VIO frame at once (`op`: one of
    /// `SWP_MINIMIZE` / `SWP_MAXIMIZE` / `SWP_RESTORE`); it has no client to
    /// ask with `WM_MINMAXFRAME`. Maximizing fits it to the screen
    /// ([`Self::vio_maximized_rect`]); restoring keeps the size within the
    /// limits of the current screen. Returns whether anything changed.
    pub fn vio_minmax(&mut self, op: u32) -> bool {
        let frame = self.vio_frame;
        let Some(mut target) = self.begin_minmax(frame, op, None, None) else { return false };
        match op {
            SWP_MAXIMIZE => target = self.vio_maximized_rect(),
            SWP_RESTORE => {
                let (cols, rows) = self.vio_screen;
                let top = target.y + target.cy;
                (target.cx, target.cy) = clamp_vio_size(target.cx, target.cy, cols, rows);
                target.y = top - target.cy;
            }
            _ => {}
        }
        self.finish_minmax(frame, op, target);
        true
    }

    fn restore_word(&self, frame: u32, index: i32, default: i32) -> i32 {
        self.get_window(frame)
            .and_then(|w| w.window_ushort.get(&index))
            .map_or(default, |&v| v as i16 as i32)
    }

    fn set_restore_word(&mut self, frame: u32, index: i32, value: i32) {
        if let Some(w) = self.get_window_mut(frame) {
            w.window_ushort.insert(index, value as i16 as u16);
        }
    }

    /// Whether `hwnd` is the VIO frame or its client.
    pub fn is_vio_window(&self, hwnd: u32) -> bool {
        self.vio_frame != 0
            && (hwnd == self.vio_frame || self.frame_to_client.get(&self.vio_frame) == Some(&hwnd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::constants::SWP_MINIMIZE;

    fn rect(wm: &WindowManager) -> WinRect {
        let w = wm.get_window(wm.vio_frame).unwrap();
        WinRect { x: w.x, y: w.y, cx: w.cx, cy: w.cy }
    }

    #[test]
    fn a_vio_frame_holds_the_text_screen_inside_its_chrome() {
        let mut wm = WindowManager::new();
        let frame = wm.create_vio_frame("Warpine — 4os2.exe", 80, 25);
        let bw = frame_border_width(VIO_FRAME_FLAGS);
        let w = wm.get_window(frame).unwrap();
        assert_eq!((w.cx, w.cy), (640 + 2 * bw, 400 + CHROME_TITLE_H + bw));
        assert_eq!(w.text, "Warpine — 4os2.exe");
        assert!(w.visible);
        assert_eq!((wm.vio_frame, wm.focused_hwnd), (frame, frame));
        assert_eq!(wm.z_order.last(), Some(&frame));
        let client = wm.frame_to_client[&frame];
        assert!(wm.is_vio_window(frame) && wm.is_vio_window(client));
        assert!(!wm.is_vio_window(0));
        // The surface covers the frame below its title bar.
        let s = wm.frame_surface(frame).unwrap();
        let s = s.lock().unwrap();
        assert_eq!((s.width, s.height), ((640 + 2 * bw) as u32, (400 + bw) as u32));
    }

    #[test]
    fn the_vio_frame_has_window_buttons_and_a_sizing_border() {
        for f in [FCF_MINBUTTON, FCF_MAXBUTTON, FCF_SIZEBORDER, FCF_SYSMENU, FCF_TITLEBAR] {
            assert_ne!(VIO_FRAME_FLAGS & f, 0, "{f:#x}");
        }
        let mut wm = WindowManager::new();
        let frame = wm.create_vio_frame("t", 80, 25);
        assert!(wm.has_minmax_buttons(frame));
    }

    #[test]
    fn a_screen_larger_than_the_desktop_starts_in_a_window_that_fits() {
        let mut wm = WindowManager::new();
        let (_, dh) = desktop_size();
        // 80 × 60 cells = 960 pels of text: taller than the default desktop.
        wm.create_vio_frame("t", 80, 60);
        let r = rect(&wm);
        let bw = frame_border_width(VIO_FRAME_FLAGS);
        assert_eq!(r.cy, dh as i32 + 2 * bw, "{r:?}");
        assert_eq!(r.cx, vio_frame_size(80, 60).0 + VIO_SCROLLBAR_W, "the width and a vertical bar");
        let (cw, ch) = vio_client_size(r.cx, r.cy);
        assert_eq!(vio_scroll_bars(cw, ch, 80, 60), (true, false), "only a vertical scroll bar");
    }

    #[test]
    fn the_vio_frame_follows_a_mode_change_with_its_top_edge_fixed() {
        let mut wm = WindowManager::new();
        let frame = wm.create_vio_frame("t", 80, 25);
        let top = |wm: &WindowManager| wm.get_window(frame).map(|w| w.y + w.cy).unwrap();
        let before = top(&wm);
        assert!(!wm.fit_vio_frame(80, 25), "unchanged");
        assert!(wm.fit_vio_frame(80, 43));
        assert_eq!(wm.get_window(frame).unwrap().cy, vio_frame_size(80, 43).1);
        assert_eq!(top(&wm), before);
        let s = wm.frame_surface(frame).unwrap();
        assert_eq!(s.lock().unwrap().height, (43 * 16 + frame_border_width(VIO_FRAME_FLAGS)) as u32);
    }

    #[test]
    fn a_resized_frame_is_kept_until_the_mode_changes() {
        let mut wm = WindowManager::new();
        wm.create_vio_frame("t", 80, 25);
        let mut r = rect(&wm);
        r.cx -= 100;
        wm.set_vio_rect(r);
        assert!(!wm.fit_vio_frame(80, 25), "same screen: the user's size stays");
        assert_eq!(rect(&wm).cx, r.cx);
    }

    #[test]
    fn scroll_bars_appear_where_the_view_is_too_small() {
        assert_eq!(vio_scroll_bars(640, 400, 80, 25), (false, false));
        assert_eq!(vio_scroll_bars(600, 400, 80, 25), (true, true), "the horizontal bar takes height");
        assert_eq!(vio_scroll_bars(640 + VIO_SCROLLBAR_W, 300, 80, 25), (true, false));
        assert_eq!(vio_scroll_bars(640, 300, 80, 25), (true, true));
    }

    #[test]
    fn size_limits_never_exceed_the_whole_screen() {
        let ((min_cx, min_cy), max) = vio_size_limits(80, 25);
        let full = vio_frame_size(80, 25);
        assert_eq!(max, (full.0 + VIO_SCROLLBAR_W, full.1 + VIO_SCROLLBAR_W));
        assert_eq!(min_cx, VIO_MIN_FRAME_CX);
        assert_eq!(min_cy, VIO_MIN_CLIENT_CY + CHROME_TITLE_H + frame_border_width(VIO_FRAME_FLAGS));
        assert_eq!(clamp_vio_size(5000, 5000, 80, 25), full, "no larger than the whole screen");
        // Shorter than the screen: the vertical bar may take extra width.
        assert_eq!(clamp_vio_size(5000, 300, 80, 25), (full.0 + VIO_SCROLLBAR_W, 300));
        assert_eq!(clamp_vio_size(full.0 + 8, full.1, 80, 25), full, "no bar, no extra room");
        assert_eq!(clamp_vio_size(0, 0, 80, 25), (min_cx, min_cy));
        assert_eq!(clamp_vio_size(300, 200, 80, 25), (300, 200));
        // A tiny screen: the minimum is never above the maximum.
        let (min, max) = vio_size_limits(4, 1);
        assert!(min.0 <= max.0 && min.1 <= max.1);
        let mut wm = WindowManager::new();
        let frame = wm.create_vio_frame("t", 80, 25);
        assert_eq!(wm.vio_clamp_size(frame, 5000, 5000), Some(full));
        assert_eq!(wm.vio_clamp_size(frame + 1, 5000, 5000), None);
    }

    #[test]
    fn maximize_fits_the_whole_screen_and_restore_brings_the_old_size_back() {
        let mut wm = WindowManager::new();
        let frame = wm.create_vio_frame("t", 80, 25);
        let full = rect(&wm);
        let small = WinRect { x: 300, y: 400, cx: 400, cy: 200 };
        wm.set_vio_rect(small);
        assert!(wm.vio_minmax(SWP_MAXIMIZE));
        assert_eq!(wm.minmax_state(frame), WS_MAXIMIZED);
        let max = rect(&wm);
        assert_eq!((max.cx, max.cy), (full.cx, full.cy), "the whole 80 × 25 screen");
        let (dw, dh) = desktop_size();
        assert!(max.x + max.cx <= dw as i32 + 2 && max.y + max.cy <= dh as i32 + 2, "{max:?}");
        assert_eq!(max.y + max.cy, small.y + small.cy, "the top edge stays where there is room");
        assert!(!wm.vio_minmax(SWP_MAXIMIZE), "already maximized");
        assert!(wm.vio_minmax(SWP_RESTORE));
        assert_eq!(rect(&wm), small);
        assert_eq!(wm.minmax_state(frame), 0);
    }

    #[test]
    fn maximizing_a_screen_larger_than_the_desktop_covers_the_desktop() {
        let mut wm = WindowManager::new();
        wm.create_vio_frame("t", 132, 60);
        assert!(wm.vio_minmax(SWP_MAXIMIZE));
        let (dw, dh) = desktop_size();
        let bw = frame_border_width(VIO_FRAME_FLAGS);
        assert_eq!(rect(&wm), WinRect { x: -bw, y: -bw, cx: dw as i32 + 2 * bw, cy: dh as i32 + 2 * bw });
    }

    #[test]
    fn minimize_and_restore_the_vio_frame() {
        let mut wm = WindowManager::new();
        let frame = wm.create_vio_frame("t", 80, 25);
        let normal = rect(&wm);
        assert!(wm.vio_minmax(SWP_MINIMIZE));
        assert_eq!(wm.minmax_state(frame), WS_MINIMIZED);
        assert_ne!(rect(&wm), normal);
        // A mode change while minimized takes effect on restore.
        assert!(wm.fit_vio_frame(80, 43));
        assert!(wm.vio_minmax(SWP_RESTORE));
        let r = rect(&wm);
        assert_eq!((r.cx, r.cy), vio_frame_size(80, 43));
        assert_eq!(r.y + r.cy, normal.y + normal.cy, "top edge kept");
    }

    #[test]
    fn a_maximized_frame_is_maximized_again_after_a_mode_change() {
        let mut wm = WindowManager::new();
        let frame = wm.create_vio_frame("t", 80, 25);
        wm.vio_minmax(SWP_MAXIMIZE);
        assert!(wm.fit_vio_frame(80, 30));
        assert_eq!(wm.minmax_state(frame), WS_MAXIMIZED);
        let r = rect(&wm);
        assert_eq!((r.cx, r.cy), vio_frame_size(80, 30));
    }
}
