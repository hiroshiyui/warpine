// SPDX-License-Identifier: GPL-3.0-or-later

//! Minimizing, maximizing and restoring frame windows.
//!
//! A frame changes state through `WM_SYSCOMMAND` (`SC_MINIMIZE` /
//! `SC_MAXIMIZE` / `SC_RESTORE`: its minimize and maximize buttons, the
//! system accelerators Alt+F9 / Alt+F10 / Alt+F5, title-bar double clicks)
//! when it has those buttons (`FID_MINMAX`), or through `WinSetWindowPos`
//! with `SWP_MINIMIZE` / `SWP_MAXIMIZE` / `SWP_RESTORE`.
//!
//! The change runs in two steps so that the frame's client can be asked
//! first: [`WindowManager::begin_minmax`] works out the new position, and
//! records the normal position in the frame's window words (`QWS_XRESTORE`
//! ... `QWS_CYRESTORE`) and the icon position (`QWS_XMINIMIZE` /
//! `QWS_YMINIMIZE`); then `WM_MINMAXFRAME` goes to the client, and unless
//! it answers TRUE, [`WindowManager::finish_minmax`] sets `WS_MINIMIZED` /
//! `WS_MAXIMIZED`, moves and sizes the frame, hides (or shows again) the
//! frames it owns and tells the client its new size with `WM_SIZE`.
//!
//! A maximized frame covers its parent, the desktop, with its border just
//! outside it. A minimized frame becomes a small title bar
//! (`MINIMIZED_CX` × `MINIMIZED_CY`) in the icon area along the bottom of
//! the desktop: the first slot no other minimized frame occupies, or the
//! position it was minimized to before.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::{OS2Message, WindowManager};

/// Position and size of a window, in desktop coordinates (bottom-left origin).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinRect {
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
}

impl WinRect {
    fn overlaps(&self, o: &WinRect) -> bool {
        self.x < o.x + o.cx && o.x < self.x + self.cx && self.y < o.y + o.cy && o.y < self.y + self.cy
    }
}

/// The `SWP_*` flags that change a frame's state.
pub const STATE_FLAGS: u32 = SWP_MINIMIZE | SWP_MAXIMIZE | SWP_RESTORE;

/// Size of an `SWP` structure: fl, cy, cx, y, x, hwndInsertBehind, hwnd,
/// ulReserved1, ulReserved2.
pub const SWP_STRUCT_SIZE: u32 = 36;

/// Icon slots looked at for a free one before falling back to the first.
const MAX_ICON_SLOTS: i32 = 1024;

/// The state change `WinSetWindowPos` flags `fl` ask for: one of
/// `SWP_MINIMIZE` / `SWP_MAXIMIZE` / `SWP_RESTORE`. The three exclude each
/// other, so flags asking for more than one ask for none.
pub fn state_change(fl: u32) -> Option<u32> {
    let s = fl & STATE_FLAGS;
    matches!(s, SWP_MINIMIZE | SWP_MAXIMIZE | SWP_RESTORE).then_some(s)
}

/// The state change a system command (`SC_*`) stands for.
pub fn syscommand_state(sc: u32) -> Option<u32> {
    match sc {
        SC_MINIMIZE => Some(SWP_MINIMIZE),
        SC_MAXIMIZE => Some(SWP_MAXIMIZE),
        SC_RESTORE => Some(SWP_RESTORE),
        _ => None,
    }
}

/// The `SWP` sent with `WM_MINMAXFRAME`, as nine ULONGs: the state flag
/// plus `SWP_MOVE | SWP_SIZE`, and the position and size `frame` takes.
pub fn minmax_swp(frame: u32, op: u32, target: WinRect) -> [u32; 9] {
    [op | SWP_MOVE | SWP_SIZE, target.cy as u32, target.cx as u32, target.y as u32,
     target.x as u32, 0, frame, 0, 0]
}

/// `MPFROM2SHORT(cx, cy)`: a size as `WM_SIZE` carries it.
pub fn pack_size((cx, cy): (i32, i32)) -> u32 {
    (cx as u32 & 0xFFFF) | ((cy as u32 & 0xFFFF) << 16)
}

/// The style bit of a state change.
fn state_style(op: u32) -> u32 {
    match op {
        SWP_MINIMIZE => WS_MINIMIZED,
        SWP_MAXIMIZE => WS_MAXIMIZED,
        _ => 0,
    }
}

impl WindowManager {
    /// Whether `frame` has minimize, maximize or hide buttons (`FID_MINMAX`),
    /// so that the system commands minimize, maximize and restore apply to
    /// it. That the hide button belongs to the `FID_MINMAX` control (it
    /// takes the minimize button's place, `FCF_HIDEMAX` = hide + maximize)
    /// is assumed; the PM Reference does not say.
    pub fn has_minmax_buttons(&self, frame: u32) -> bool {
        self.get_window(frame).is_some_and(|w| {
            w.frame_flags & (FCF_MINBUTTON | FCF_MAXBUTTON | FCF_HIDEBUTTON) != 0
        })
    }

    /// `WS_MINIMIZED` / `WS_MAXIMIZED` of `hwnd` (0 when neither, or unknown).
    pub fn minmax_state(&self, hwnd: u32) -> u32 {
        self.get_window(hwnd).map_or(0, |w| w.style & (WS_MINIMIZED | WS_MAXIMIZED))
    }

    /// WinQueryWindowUShort: window word `index` of `hwnd`. `QWS_ID` is the
    /// window's identity. A frame that was never minimized reports -1
    /// (0xFFFF) as its icon position.
    pub fn query_window_ushort(&self, hwnd: u32, index: i32) -> u16 {
        let Some(w) = self.get_window(hwnd) else { return 0 };
        if index == QWS_ID {
            return w.id as u16;
        }
        match w.window_ushort.get(&index) {
            Some(&v) => v,
            None if matches!(index, QWS_XMINIMIZE | QWS_YMINIMIZE)
                && self.frame_to_client.contains_key(&hwnd) => 0xFFFF,
            None => 0,
        }
    }

    /// WinQueryWindowULong: window word `index` of `hwnd`. `QWL_STYLE` is
    /// the style with `WS_VISIBLE` as the window is now, `QWL_HMQ` its
    /// message queue.
    pub fn query_window_ulong(&self, hwnd: u32, index: i32) -> u32 {
        let Some(w) = self.get_window(hwnd) else { return 0 };
        match index {
            QWL_STYLE => (w.style & !WS_VISIBLE) | if w.visible { WS_VISIBLE } else { 0 },
            QWL_HMQ => w.hmq,
            _ => w.window_ulong.get(&index).copied().unwrap_or(0),
        }
    }

    /// A signed window word of `frame`, if it was ever set.
    pub(super) fn word(&self, frame: u32, index: i32) -> Option<i32> {
        self.get_window(frame)?.window_ushort.get(&index).map(|&v| v as i16 as i32)
    }

    pub(super) fn set_word(&mut self, frame: u32, index: i32, value: i32) {
        if let Some(w) = self.get_window_mut(frame) {
            w.window_ushort.insert(index, value as i16 as u16);
        }
    }

    /// Where a frame minimized now goes: the first slot of the icon area
    /// (rows of `MINIMIZED_CX` × `MINIMIZED_CY` slots from the bottom-left
    /// corner of the desktop) that no other minimized frame overlaps.
    pub fn icon_position(&self, frame: u32) -> (i32, i32) {
        let (dw, _) = desktop_size();
        let (step_x, step_y) = (MINIMIZED_CX + ICON_AREA_GAP, MINIMIZED_CY + ICON_AREA_GAP);
        let cols = ((dw as i32 - ICON_AREA_GAP) / step_x).max(1);
        let taken: Vec<WinRect> = self.windows()
            .filter(|w| w.handle != frame && w.style & WS_MINIMIZED != 0)
            .map(|w| WinRect { x: w.x, y: w.y, cx: w.cx, cy: w.cy })
            .collect();
        (0..MAX_ICON_SLOTS).map(|i| {
            (ICON_AREA_GAP + (i % cols) * step_x, ICON_AREA_GAP + (i / cols) * step_y)
        }).find(|&(x, y)| {
            let slot = WinRect { x, y, cx: MINIMIZED_CX, cy: MINIMIZED_CY };
            !taken.iter().any(|r| r.overlaps(&slot))
        }).unwrap_or((ICON_AREA_GAP, ICON_AREA_GAP))
    }

    /// First step of minimizing, maximizing or restoring standard frame
    /// `frame` (`op`: one of `SWP_MINIMIZE` / `SWP_MAXIMIZE` /
    /// `SWP_RESTORE`): the position and size it is to take, or `None` when
    /// the change does nothing (a frame already in that state, a frame
    /// neither minimized nor maximized to restore, not a standard frame).
    ///
    /// Leaving the normal state records the frame's position and size in
    /// its restore window words; minimizing records the icon position.
    /// `pos` / `size` (`SWP_MOVE` / `SWP_SIZE`) replace the remembered
    /// position and size when restoring, and place the icon when minimizing.
    pub fn begin_minmax(&mut self, frame: u32, op: u32, pos: Option<(i32, i32)>,
                        size: Option<(i32, i32)>) -> Option<WinRect> {
        if !self.frame_to_client.contains_key(&frame) { return None; }
        let w = self.get_window(frame)?;
        let state = w.style & (WS_MINIMIZED | WS_MAXIMIZED);
        let current = WinRect { x: w.x, y: w.y, cx: w.cx, cy: w.cy };
        let border = frame_border_width(w.frame_flags);
        let unchanged = match op {
            SWP_MINIMIZE => state & WS_MINIMIZED != 0,
            SWP_MAXIMIZE => state & WS_MAXIMIZED != 0,
            SWP_RESTORE => state == 0,
            _ => true,
        };
        if unchanged { return None; }
        if state == 0 {
            for (index, v) in [(QWS_XRESTORE, current.x), (QWS_YRESTORE, current.y),
                               (QWS_CXRESTORE, current.cx), (QWS_CYRESTORE, current.cy)] {
                self.set_word(frame, index, v);
            }
        }
        Some(match op {
            SWP_MAXIMIZE => {
                let (dw, dh) = desktop_size();
                WinRect { x: -border, y: -border, cx: dw as i32 + 2 * border, cy: dh as i32 + 2 * border }
            }
            SWP_MINIMIZE => {
                let remembered = match (self.word(frame, QWS_XMINIMIZE), self.word(frame, QWS_YMINIMIZE)) {
                    (Some(x), Some(y)) if (x, y) != (-1, -1) => Some((x, y)),
                    _ => None,
                };
                let (x, y) = pos.or(remembered).unwrap_or_else(|| self.icon_position(frame));
                self.set_word(frame, QWS_XMINIMIZE, x);
                self.set_word(frame, QWS_YMINIMIZE, y);
                WinRect { x, y, cx: MINIMIZED_CX, cy: MINIMIZED_CY }
            }
            _ => {
                let word = |i, d| self.word(frame, i).unwrap_or(d);
                let (x, y) = pos.unwrap_or((word(QWS_XRESTORE, current.x), word(QWS_YRESTORE, current.y)));
                let (cx, cy) = size.unwrap_or((word(QWS_CXRESTORE, current.cx), word(QWS_CYRESTORE, current.cy)));
                WinRect { x, y, cx: cx.max(0), cy: cy.max(0) }
            }
        })
    }

    /// Second step (unless the client answered `WM_MINMAXFRAME` with TRUE):
    /// set the state style, move and size `frame` to `target`, hide the
    /// frames it (or its client) owns when minimizing and show them again
    /// when it leaves the minimized state, and post `WM_SIZE` (old and new
    /// client size) and `WM_PAINT` to the client. A minimized frame that
    /// was the active window passes activation on ([`Self::activate_next_frame`]).
    pub fn finish_minmax(&mut self, frame: u32, op: u32, target: WinRect) {
        let Some(client) = self.frame_to_client.get(&frame).copied() else { return };
        let old = self.get_window(client).map_or((0, 0), |w| (w.cx, w.cy));
        let Some(w) = self.get_window_mut(frame) else { return };
        w.style = (w.style & !(WS_MINIMIZED | WS_MAXIMIZED)) | state_style(op);
        (w.x, w.y, w.cx, w.cy) = (target.x, target.y, target.cx, target.cy);
        let surface = self.layout_frame(frame);
        self.resize_surface(frame, surface.0, surface.1);
        // The client's size: the surface less any frame scroll bars.
        let new = self.get_window(client).map_or(surface, |w| (w.cx, w.cy));
        self.sync_owned_frames(frame);
        self.mark_all_dirty();
        self.post_size(client, old, new);
        if op == SWP_MINIMIZE {
            self.activate_next_frame(frame);
        }
    }

    /// Post `msg` to `hwnd` on the queue of its thread.
    pub fn post_msg(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) {
        if let Some(mq_arc) = self.find_hmq_for_hwnd(hwnd).and_then(|h| self.get_mq(h)) {
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(OS2Message { hwnd, msg, mp1, mp2, time: 0, x: 0, y: 0 });
            mq.cond.notify_one();
        }
    }

    /// Tell `hwnd` it changed size from `old` to `new` (`WM_SIZE`: mp1 the
    /// old, mp2 the new size, `MPFROM2SHORT(cx, cy)`) and have it repaint
    /// (`WM_PAINT`). PM sends `WM_SIZE`; Warpine posts it, like every frame
    /// size change, so that it reaches the window procedure on its own thread.
    pub fn post_size(&mut self, hwnd: u32, old: (i32, i32), new: (i32, i32)) {
        self.post_msg(hwnd, WM_SIZE, pack_size(old), pack_size(new));
        // All of it, or what the new size uncovers (CS_SIZEREDRAW, pm_paint.rs).
        self.invalidate_resized(hwnd, old, new);
    }

    /// The window a frame's messages are delivered to: a standard frame's
    /// client (the frame shares the client's window procedure in Warpine),
    /// otherwise the window itself (a dialog).
    pub fn frame_msg_target(&self, frame: u32) -> u32 {
        self.frame_to_client.get(&frame).copied().unwrap_or(frame)
    }

    /// Hide or show again the frames `frame` (or its client) owns: they are
    /// hidden while `frame` is minimized or hidden, and the ones hidden that
    /// way are shown again once it is visible and not minimized (PM Guide,
    /// "Windows": "When the frame window is minimized or hidden, it hides all
    /// the windows it owns. Owned windows hidden this way are restored when
    /// the frame window is restored.").
    pub fn sync_owned_frames(&mut self, frame: u32) {
        let Some(w) = self.get_window(frame) else { return };
        let shown = w.visible && w.style & WS_MINIMIZED == 0;
        if shown {
            if let Some(owned) = self.hidden_with_owner.remove(&frame) {
                for h in owned {
                    if let Some(w) = self.get_window_mut(h) { w.visible = true; }
                }
                self.mark_all_dirty();
            }
            return;
        }
        let client = self.frame_to_client.get(&frame).copied().unwrap_or(frame);
        let owned: Vec<u32> = self.windows()
            .filter(|w| (w.owner == frame || w.owner == client) && w.visible && w.handle != frame)
            .map(|w| w.handle)
            .filter(|&h| self.is_frame(h))
            .collect();
        if owned.is_empty() { return; }
        for &h in &owned {
            if let Some(w) = self.get_window_mut(h) { w.visible = false; }
        }
        self.hidden_with_owner.entry(frame).or_default().extend(owned);
        self.mark_all_dirty();
    }

    /// When `frame` is the active window, make the next frame in Z-order
    /// (topmost first) that is visible, enabled and not minimized the
    /// active window instead: raised to the top, with `WM_ACTIVATE` FALSE
    /// posted to `frame` first and then `WM_ACTIVATE` TRUE to the new one
    /// (mp2 = the other frame). `frame` stays active when there is no such
    /// frame. Returns the newly active frame.
    ///
    /// The PM Guide ("Windows") states this for a destroyed active window
    /// ("the next window, as determined by the Alt+Esc key combination");
    /// that minimizing or hiding the active window does the same is
    /// assumed, not documented.
    pub fn activate_next_frame(&mut self, frame: u32) -> Option<u32> {
        if self.focused_hwnd != frame { return None; }
        let next = self.z_order.iter().rev().copied().find(|&h| {
            h != frame && self.is_frame(h) && self.get_window(h).is_some_and(|w| {
                w.visible && w.style & (WS_DISABLED | WS_MINIMIZED) == 0
            })
        })?;
        self.focused_hwnd = next;
        self.z_push_top(next);
        self.post_msg(self.frame_msg_target(frame), WM_ACTIVATE, 0, next);
        self.post_msg(self.frame_msg_target(next), WM_ACTIVATE, 1, frame);
        Some(next)
    }

    /// The desktop server made `new_active`, another program's frame, the
    /// active window (ADR-0052): `frame`, active here until now, is not —
    /// `WM_ACTIVATE` FALSE to it, and its title bar drawn inactive.
    ///
    /// `seq` is the number of this program's report that made `frame`
    /// active (ADR-0108). A program that has reported since, or activated
    /// the frame again and not yet reported it, keeps it: the server takes
    /// its newer report, so this `Deactivate` is stale.
    pub fn deactivate_frame(&mut self, frame: u32, new_active: u32, seq: u32) {
        if self.focused_hwnd != frame || frame == 0 { return; }
        if self.activation_changed() || self.activation_sent.seq != seq { return; }
        self.focused_hwnd = 0;
        self.post_msg(self.frame_msg_target(frame), WM_ACTIVATE, 0, new_active);
        self.mark_dirty(frame);
    }

    /// Make `frame` the active frame on purpose (`WinSetActiveWindow`, a
    /// click, a switch, `SWP_ACTIVATE`): the next report of the active
    /// frame to the desktop server is a new one even when `frame` was
    /// already active here, since the server may have given activation to
    /// another program since this program's last report (ADR-0108).
    pub fn claim_activation(&mut self, frame: u32) {
        self.focused_hwnd = frame;
        self.activation_claimed = true;
    }

    /// Whether the active frame changed, or was claimed, since the last
    /// report to the desktop server.
    pub fn activation_changed(&self) -> bool {
        self.activation_claimed || self.focused_hwnd != self.activation_sent.frame
    }

    /// The report of the active frame to send the desktop server now: the
    /// last one again, or, after a change, a new one with the next number.
    pub fn activation_report(&mut self) -> crate::desktop::proto::Activation {
        if self.activation_changed() {
            self.activation_sent = crate::desktop::proto::Activation {
                frame: self.focused_hwnd,
                seq: self.activation_sent.seq.wrapping_add(1),
            };
            self.activation_claimed = false;
        }
        self.activation_sent
    }

    /// The `Report` for the desktop server, when anything changed since
    /// the last one: the active frame's report and the frames API calls
    /// raised or lowered (ADR-0108).
    pub fn take_report(&mut self) -> Option<crate::desktop::proto::WinCall> {
        if !self.activation_changed() && self.reported_raised.is_empty() && self.reported_lowered.is_empty() {
            return None;
        }
        Some(crate::desktop::proto::WinCall::Report {
            active: self.activation_report(),
            raised: std::mem::take(&mut self.reported_raised),
            lowered: std::mem::take(&mut self.reported_lowered),
        })
    }

    /// Hide `frame` (`WM_SYSCOMMAND(SC_HIDE)`, its hide button): the frames
    /// it owns are hidden with it and, if it was the active window, another
    /// frame becomes active. Returns whether it was visible.
    pub fn hide_frame(&mut self, frame: u32) -> bool {
        let Some(w) = self.get_window_mut(frame) else { return false };
        let was_visible = std::mem::replace(&mut w.visible, false);
        self.mark_all_dirty();
        self.frame_visibility_changed(frame);
        was_visible
    }

    /// After `WinShowWindow` / `WinSetWindowPos(SWP_SHOW | SWP_HIDE)` on
    /// `hwnd`: a frame hides or shows again the frames it owns, and a hidden
    /// frame that was active passes activation on. Other windows: nothing.
    pub fn frame_visibility_changed(&mut self, hwnd: u32) {
        if !self.is_frame(hwnd) { return; }
        self.sync_owned_frames(hwnd);
        if self.get_window(hwnd).is_some_and(|w| !w.visible) {
            self.activate_next_frame(hwnd);
        }
    }

    /// The `fl` of the `SWP` that `WinQueryWindowPos` returns for `hwnd`:
    /// `SWP_MOVE | SWP_SIZE`, `SWP_MINIMIZE` or `SWP_MAXIMIZE` for a
    /// minimized or maximized window, and for a frame `SWP_ACTIVATE` when it
    /// is the active window, else `SWP_DEACTIVATE`; all other bits zero
    /// (PM Reference, `WinQueryWindowPos`). Which windows the activation
    /// bits are reported for is not documented; Warpine reports them for
    /// frames only.
    pub fn query_swp_flags(&self, hwnd: u32) -> u32 {
        let state = self.minmax_state(hwnd);
        let mut fl = SWP_MOVE | SWP_SIZE;
        if state & WS_MINIMIZED != 0 {
            fl |= SWP_MINIMIZE;
        } else if state & WS_MAXIMIZED != 0 {
            fl |= SWP_MAXIMIZE;
        }
        if self.is_frame(hwnd) {
            fl |= if self.focused_hwnd == hwnd { SWP_ACTIVATE } else { SWP_DEACTIVATE };
        }
        fl
    }

    /// Minimize, maximize or restore `frame` at once, without asking its
    /// client. Returns whether anything changed.
    pub fn set_minmax(&mut self, frame: u32, op: u32) -> bool {
        match self.begin_minmax(frame, op, None, None) {
            Some(target) => { self.finish_minmax(frame, op, target); true }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HMQ_CP: u32 = 437;

    /// A 640×480 standard frame at (100, 200) with min/max buttons and a
    /// sizing border, on a queue. Returns `(wm, frame, client)`.
    fn frame() -> (WindowManager, u32, u32) {
        let mut wm = WindowManager::new();
        let hmq = wm.create_mq(HMQ_CP);
        let frame = wm.create_window("App".into(), 1, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        wm.frame_to_client.insert(frame, client);
        let w = wm.get_window_mut(frame).unwrap();
        (w.x, w.y, w.cx, w.cy) = (100, 200, 640, 480);
        w.frame_flags = FCF_TITLEBAR | FCF_SIZEBORDER | FCF_MINBUTTON | FCF_MAXBUTTON;
        w.visible = true;
        wm.layout_frame(frame);
        wm.frame_surface(frame);
        (wm, frame, client)
    }

    fn rect(wm: &WindowManager, h: u32) -> WinRect {
        let w = wm.get_window(h).unwrap();
        WinRect { x: w.x, y: w.y, cx: w.cx, cy: w.cy }
    }

    /// The messages queued for `client`, removed from the queue.
    fn drain(wm: &WindowManager, client: u32) -> Vec<(u32, u32, u32)> {
        let mq = wm.get_mq(wm.find_hmq_for_hwnd(client).unwrap()).unwrap();
        let mut mq = mq.lock_or_recover();
        mq.messages.drain(..).filter(|m| m.hwnd == client).map(|m| (m.msg, m.mp1, m.mp2)).collect()
    }

    #[test]
    fn state_flags_exclude_each_other() {
        assert_eq!(state_change(SWP_MINIMIZE | SWP_MOVE), Some(SWP_MINIMIZE));
        assert_eq!(state_change(SWP_RESTORE), Some(SWP_RESTORE));
        assert_eq!(state_change(SWP_MINIMIZE | SWP_MAXIMIZE), None);
        assert_eq!(state_change(SWP_MOVE | SWP_SIZE), None);
        assert_eq!(syscommand_state(SC_MAXIMIZE), Some(SWP_MAXIMIZE));
        assert_eq!(syscommand_state(SC_CLOSE), None);
    }

    #[test]
    fn maximize_covers_the_desktop_with_the_border_outside() {
        let (mut wm, frame, client) = frame();
        drain(&wm, client);
        let (dw, dh) = desktop_size();
        let b = CHROME_BORDER_W;
        let target = wm.begin_minmax(frame, SWP_MAXIMIZE, None, None).unwrap();
        assert_eq!(target, WinRect { x: -b, y: -b, cx: dw as i32 + 2 * b, cy: dh as i32 + 2 * b });
        // The window words are set before the change; the state is not.
        assert_eq!(wm.query_window_ushort(frame, QWS_XRESTORE), 100);
        assert_eq!(wm.query_window_ushort(frame, QWS_CYRESTORE), 480);
        assert_eq!(wm.minmax_state(frame), 0);
        wm.finish_minmax(frame, SWP_MAXIMIZE, target);
        assert_eq!(wm.minmax_state(frame), WS_MAXIMIZED);
        assert_eq!(rect(&wm, frame), target);
        let client_size = (target.cx, target.cy - CHROME_TITLE_H);
        assert_eq!((rect(&wm, client).cx, rect(&wm, client).cy), client_size);
        let size = |(cx, cy): (i32, i32)| cx as u32 | ((cy as u32) << 16);
        assert_eq!(drain(&wm, client), vec![(WM_SIZE, size((640, 460)), size(client_size)), (WM_PAINT, 0, 0)]);
        let s = wm.surfaces[&frame].lock_or_recover();
        assert_eq!((s.width as i32, s.height as i32), client_size);
        drop(s);
        // Maximizing again does nothing.
        assert_eq!(wm.begin_minmax(frame, SWP_MAXIMIZE, None, None), None);
    }

    #[test]
    fn restore_returns_to_the_remembered_position() {
        let (mut wm, frame, client) = frame();
        assert!(!wm.set_minmax(frame, SWP_RESTORE), "a normal frame is not restored");
        assert!(wm.set_minmax(frame, SWP_MAXIMIZE));
        drain(&wm, client);
        assert!(wm.set_minmax(frame, SWP_RESTORE));
        assert_eq!(wm.minmax_state(frame), 0);
        assert_eq!(rect(&wm, frame), WinRect { x: 100, y: 200, cx: 640, cy: 480 });
        assert_eq!(drain(&wm, client)[0].0, WM_SIZE);
    }

    #[test]
    fn restore_words_can_be_changed_and_moved_or_sized() {
        let (mut wm, frame, _) = frame();
        assert!(wm.set_minmax(frame, SWP_MINIMIZE));
        // WinSetWindowUShort alters where the frame is restored to.
        wm.get_window_mut(frame).unwrap().window_ushort.insert(QWS_CXRESTORE, 300);
        wm.get_window_mut(frame).unwrap().window_ushort.insert(QWS_XRESTORE, (-20i16) as u16);
        let t = wm.begin_minmax(frame, SWP_RESTORE, None, None).unwrap();
        assert_eq!(t, WinRect { x: -20, y: 200, cx: 300, cy: 480 });
        // SWP_MOVE / SWP_SIZE win over the window words.
        let t = wm.begin_minmax(frame, SWP_RESTORE, Some((5, 6)), Some((70, 80))).unwrap();
        assert_eq!(t, WinRect { x: 5, y: 6, cx: 70, cy: 80 });
    }

    #[test]
    fn minimize_goes_to_the_first_free_icon_slot_and_remembers_it() {
        let (mut wm, frame, client) = frame();
        assert_eq!(wm.query_window_ushort(frame, QWS_XMINIMIZE), 0xFFFF, "never minimized");
        assert_eq!(wm.query_window_ushort(client, QWS_XMINIMIZE), 0, "not a frame");
        assert!(wm.set_minmax(frame, SWP_MINIMIZE));
        assert_eq!(wm.minmax_state(frame), WS_MINIMIZED);
        let icon = WinRect { x: ICON_AREA_GAP, y: ICON_AREA_GAP, cx: MINIMIZED_CX, cy: MINIMIZED_CY };
        assert_eq!(rect(&wm, frame), icon);
        assert_eq!(wm.query_window_ushort(frame, QWS_XMINIMIZE), ICON_AREA_GAP as u16);
        assert!(!wm.set_minmax(frame, SWP_MINIMIZE), "already minimized");

        // A second frame takes the next slot.
        let f3 = {
            let hmq = wm.find_hmq_for_hwnd(client).unwrap();
            let f3 = wm.create_window("App".into(), 1, hmq);
            let c3 = wm.create_window("App".into(), f3, hmq);
            wm.frame_to_client.insert(f3, c3);
            let w = wm.get_window_mut(f3).unwrap();
            (w.x, w.y, w.cx, w.cy) = (300, 300, 200, 200);
            f3
        };
        assert!(wm.set_minmax(f3, SWP_MINIMIZE));
        assert_eq!((rect(&wm, f3).x, rect(&wm, f3).y), (2 * ICON_AREA_GAP + MINIMIZED_CX, ICON_AREA_GAP));

        // Restored and minimized again: back to its own icon position, even
        // though the first slot is free now.
        assert!(wm.set_minmax(f3, SWP_RESTORE));
        assert_eq!(rect(&wm, f3), WinRect { x: 300, y: 300, cx: 200, cy: 200 });
        assert!(wm.set_minmax(frame, SWP_RESTORE));
        assert!(wm.set_minmax(f3, SWP_MINIMIZE));
        assert_eq!(rect(&wm, f3).x, 2 * ICON_AREA_GAP + MINIMIZED_CX);
        // An explicit position places the icon.
        assert!(wm.set_minmax(f3, SWP_RESTORE));
        assert_eq!(wm.begin_minmax(f3, SWP_MINIMIZE, Some((500, 9)), None).map(|r| (r.x, r.y)), Some((500, 9)));
    }

    #[test]
    fn maximize_from_minimized_keeps_the_normal_position() {
        let (mut wm, frame, _) = frame();
        assert!(wm.set_minmax(frame, SWP_MINIMIZE));
        assert!(wm.set_minmax(frame, SWP_MAXIMIZE));
        assert_eq!(wm.minmax_state(frame), WS_MAXIMIZED);
        assert!(wm.set_minmax(frame, SWP_RESTORE));
        assert_eq!(rect(&wm, frame), WinRect { x: 100, y: 200, cx: 640, cy: 480 });
    }

    #[test]
    fn minimizing_hides_owned_frames_until_restored() {
        let (mut wm, frame, client) = frame();
        let hmq = wm.find_hmq_for_hwnd(client).unwrap();
        let dlg = wm.create_window(WindowManager::DIALOG_CLASS.into(), 1, hmq);
        let hidden_dlg = wm.create_window(WindowManager::DIALOG_CLASS.into(), 1, hmq);
        let control = wm.create_window("WC_BUTTON".into(), client, hmq);
        for (h, visible) in [(dlg, true), (hidden_dlg, false), (control, true)] {
            let w = wm.get_window_mut(h).unwrap();
            w.owner = client;
            w.visible = visible;
        }
        assert!(wm.set_minmax(frame, SWP_MINIMIZE));
        assert!(!wm.get_window(dlg).unwrap().visible);
        assert!(wm.get_window(control).unwrap().visible, "only frames are hidden");
        assert!(wm.set_minmax(frame, SWP_MAXIMIZE));
        assert!(wm.get_window(dlg).unwrap().visible, "shown again");
        assert!(!wm.get_window(hidden_dlg).unwrap().visible, "was hidden before");
    }

    #[test]
    fn only_standard_frames_change_state() {
        let (mut wm, _, client) = frame();
        assert_eq!(wm.begin_minmax(client, SWP_MAXIMIZE, None, None), None);
        assert_eq!(wm.begin_minmax(0xDEAD, SWP_MAXIMIZE, None, None), None);
    }

    #[test]
    fn minmax_buttons_enable_the_system_commands() {
        let (mut wm, frame, _) = frame();
        assert!(wm.has_minmax_buttons(frame));
        wm.get_window_mut(frame).unwrap().frame_flags = FCF_TITLEBAR | FCF_MAXBUTTON;
        assert!(wm.has_minmax_buttons(frame));
        wm.get_window_mut(frame).unwrap().frame_flags = FCF_TITLEBAR | FCF_SYSMENU;
        assert!(!wm.has_minmax_buttons(frame));
    }

    /// Another visible standard frame on `frame`'s queue, at the bottom of
    /// the Z-order. Returns `(frame, client)`.
    fn second_frame(wm: &mut WindowManager, frame: u32) -> (u32, u32) {
        let hmq = wm.find_hmq_for_hwnd(frame).unwrap();
        let f = wm.create_window("App".into(), 1, hmq);
        let c = wm.create_window("App".into(), f, hmq);
        wm.frame_to_client.insert(f, c);
        let w = wm.get_window_mut(f).unwrap();
        (w.x, w.y, w.cx, w.cy) = (10, 10, 200, 200);
        w.visible = true;
        wm.z_push_bottom(f);
        (f, c)
    }

    #[test]
    fn minimizing_the_active_frame_activates_the_next_one() {
        let (mut wm, frame, client) = frame();
        let (f2, _) = second_frame(&mut wm, frame);
        let (f3, _) = second_frame(&mut wm, frame);
        wm.z_push_top(frame);
        wm.focused_hwnd = frame;
        // Z-order bottom to top: f3, f2, frame. f2 is disabled, so f3 is next.
        wm.get_window_mut(f2).unwrap().style |= WS_DISABLED;
        drain(&wm, client);
        assert!(wm.set_minmax(frame, SWP_MINIMIZE));
        assert_eq!(wm.focused_hwnd, f3);
        assert_eq!(wm.z_order.last(), Some(&f3), "raised");
        let c3 = wm.frame_to_client[&f3];
        let mq = wm.get_mq(wm.find_hmq_for_hwnd(client).unwrap()).unwrap();
        let acts: Vec<_> = mq.lock_or_recover().messages.iter().filter(|m| m.msg == WM_ACTIVATE)
            .map(|m| (m.hwnd, m.mp1, m.mp2)).collect();
        assert_eq!(acts, vec![(client, 0, f3), (c3, 1, frame)], "deactivation first");
        // Minimizing a frame that is not active changes nothing.
        wm.get_window_mut(f2).unwrap().style &= !WS_DISABLED;
        assert!(wm.set_minmax(f2, SWP_MINIMIZE));
        assert_eq!(wm.focused_hwnd, f3);
        // The last frame that is not minimized stays active when minimized.
        assert!(wm.set_minmax(f3, SWP_MINIMIZE));
        assert_eq!(wm.focused_hwnd, f3);
    }

    /// ADR-0108: reports of the active frame are numbered; a frame
    /// activated again on purpose is a new report; a Deactivate answering
    /// an older report, or arriving before a claim was reported, is stale.
    #[test]
    fn reports_of_the_active_frame_and_stale_deactivations() {
        use crate::desktop::proto::Activation;
        let (mut wm, frame, client) = frame();
        wm.z_push_top(frame);
        wm.focused_hwnd = frame;
        assert!(wm.activation_changed());
        assert_eq!(wm.activation_report(), Activation { frame, seq: 1 });
        assert!(!wm.activation_changed());
        assert_eq!(wm.activation_report(), Activation { frame, seq: 1 }, "repeated, not renumbered");
        // Activated again before the server's Deactivate (for report 1)
        // was read: a claim, so that Deactivate is stale.
        assert!(wm.set_active_window(frame));
        assert!(wm.activation_changed(), "claimed");
        wm.deactivate_frame(frame, 0x20_1000, 1);
        assert_eq!(wm.focused_hwnd, frame, "stale before the claim is reported");
        assert_eq!(wm.activation_report(), Activation { frame, seq: 2 });
        wm.deactivate_frame(frame, 0x20_1000, 1);
        assert_eq!(wm.focused_hwnd, frame, "stale: it answers report 1");
        assert!(drain(&wm, client).iter().all(|m| m.0 != WM_ACTIVATE || m.1 != 0), "no WM_ACTIVATE FALSE");
        // The Deactivate for the newest report is taken.
        wm.deactivate_frame(frame, 0x20_1000, 2);
        assert_eq!(wm.focused_hwnd, 0);
        assert_eq!(drain(&wm, client), vec![(WM_ACTIVATE, 0, 0x20_1000)]);
        assert_eq!(wm.activation_report(), Activation { frame: 0, seq: 3 });
        // Numbers wrap around.
        wm.activation_sent.seq = u32::MAX;
        wm.claim_activation(frame);
        assert_eq!(wm.activation_report(), Activation { frame, seq: 0 });
    }

    #[test]
    fn hiding_hides_owned_frames_and_passes_activation_on() {
        let (mut wm, frame, client) = frame();
        let (f2, _) = second_frame(&mut wm, frame);
        wm.z_push_top(frame);
        wm.focused_hwnd = frame;
        let hmq = wm.find_hmq_for_hwnd(client).unwrap();
        let dlg = wm.create_window(WindowManager::DIALOG_CLASS.into(), 1, hmq);
        let w = wm.get_window_mut(dlg).unwrap();
        (w.owner, w.visible) = (client, true);
        assert!(wm.hide_frame(frame));
        assert!(!wm.get_window(dlg).unwrap().visible, "owned frame hidden with it");
        assert_eq!(wm.focused_hwnd, f2);
        assert!(!wm.hide_frame(frame), "already hidden");
        // Shown again: the owned frame comes back.
        wm.get_window_mut(frame).unwrap().visible = true;
        wm.frame_visibility_changed(frame);
        assert!(wm.get_window(dlg).unwrap().visible);
        // Hidden while minimized, then shown: still minimized, owned frame
        // stays hidden until restored.
        assert!(wm.set_minmax(frame, SWP_MINIMIZE));
        assert!(!wm.get_window(dlg).unwrap().visible);
        wm.hide_frame(frame);
        wm.get_window_mut(frame).unwrap().visible = true;
        wm.frame_visibility_changed(frame);
        assert!(!wm.get_window(dlg).unwrap().visible);
        assert!(wm.set_minmax(frame, SWP_RESTORE));
        assert!(wm.get_window(dlg).unwrap().visible);
    }

    #[test]
    fn query_swp_flags_report_state_and_activation() {
        let (mut wm, frame, client) = frame();
        wm.focused_hwnd = frame;
        assert_eq!(wm.query_swp_flags(frame), SWP_MOVE | SWP_SIZE | SWP_ACTIVATE);
        assert_eq!(wm.query_swp_flags(client), SWP_MOVE | SWP_SIZE);
        wm.set_minmax(frame, SWP_MINIMIZE);
        assert_eq!(wm.query_swp_flags(frame) & (SWP_MINIMIZE | SWP_MAXIMIZE), SWP_MINIMIZE);
        wm.set_minmax(frame, SWP_MAXIMIZE);
        assert_eq!(wm.query_swp_flags(frame) & (SWP_MINIMIZE | SWP_MAXIMIZE), SWP_MAXIMIZE);
        wm.focused_hwnd = 0;
        assert_eq!(wm.query_swp_flags(frame), SWP_MOVE | SWP_SIZE | SWP_MAXIMIZE | SWP_DEACTIVATE);
    }

    #[test]
    fn hide_button_counts_as_a_minmax_control() {
        let (mut wm, frame, _) = frame();
        wm.get_window_mut(frame).unwrap().frame_flags = FCF_TITLEBAR | FCF_HIDEMAX;
        assert!(wm.has_minmax_buttons(frame));
        assert_eq!(FCF_HIDEMAX, FCF_HIDEBUTTON | FCF_MAXBUTTON);
        assert_eq!(pack_size((0x1_2345, -1)), 0xFFFF_2345);
    }

    #[test]
    fn minmax_swp_layout() {
        let t = WinRect { x: -2, y: 4, cx: 10, cy: 20 };
        assert_eq!(minmax_swp(7, SWP_MAXIMIZE, t),
                   [SWP_MAXIMIZE | SWP_MOVE | SWP_SIZE, 20, 10, 4, (-2i32) as u32, 0, 7, 0, 0]);
        assert_eq!(minmax_swp(7, 0, t).len() as u32 * 4, SWP_STRUCT_SIZE);
    }
}
