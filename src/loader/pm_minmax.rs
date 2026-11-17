// SPDX-License-Identifier: GPL-3.0-only

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

/// The style bit of a state change.
fn state_style(op: u32) -> u32 {
    match op {
        SWP_MINIMIZE => WS_MINIMIZED,
        SWP_MAXIMIZE => WS_MAXIMIZED,
        _ => 0,
    }
}

impl WindowManager {
    /// Whether `frame` has minimize or maximize buttons (`FID_MINMAX`), so
    /// that the system commands minimize, maximize and restore apply to it.
    pub fn has_minmax_buttons(&self, frame: u32) -> bool {
        self.get_window(frame).is_some_and(|w| w.frame_flags & (FCF_MINBUTTON | FCF_MAXBUTTON) != 0)
    }

    /// `WS_MINIMIZED` / `WS_MAXIMIZED` of `hwnd` (0 when neither, or unknown).
    pub fn minmax_state(&self, hwnd: u32) -> u32 {
        self.get_window(hwnd).map_or(0, |w| w.style & (WS_MINIMIZED | WS_MAXIMIZED))
    }

    /// WinQueryWindowUShort: window word `index` of `hwnd`. A frame that
    /// was never minimized reports -1 (0xFFFF) as its icon position.
    pub fn query_window_ushort(&self, hwnd: u32, index: i32) -> u16 {
        let Some(w) = self.get_window(hwnd) else { return 0 };
        match w.window_ushort.get(&index) {
            Some(&v) => v,
            None if matches!(index, QWS_XMINIMIZE | QWS_YMINIMIZE)
                && self.frame_to_client.contains_key(&hwnd) => 0xFFFF,
            None => 0,
        }
    }

    /// A signed window word of `frame`, if it was ever set.
    fn word(&self, frame: u32, index: i32) -> Option<i32> {
        self.get_window(frame)?.window_ushort.get(&index).map(|&v| v as i16 as i32)
    }

    fn set_word(&mut self, frame: u32, index: i32, value: i32) {
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
    /// client size) and `WM_PAINT` to the client.
    pub fn finish_minmax(&mut self, frame: u32, op: u32, target: WinRect) {
        let Some(client) = self.frame_to_client.get(&frame).copied() else { return };
        let old = self.get_window(client).map_or((0, 0), |w| (w.cx, w.cy));
        let Some(w) = self.get_window_mut(frame) else { return };
        w.style = (w.style & !(WS_MINIMIZED | WS_MAXIMIZED)) | state_style(op);
        (w.x, w.y, w.cx, w.cy) = (target.x, target.y, target.cx, target.cy);
        let (cx, cy) = self.layout_frame(frame);
        self.resize_surface(frame, cx, cy);

        if op == SWP_MINIMIZE {
            let owned: Vec<u32> = self.windows()
                .filter(|w| (w.owner == frame || w.owner == client) && w.visible && w.handle != frame)
                .map(|w| w.handle)
                .filter(|&h| self.is_frame(h))
                .collect();
            for &h in &owned {
                if let Some(w) = self.get_window_mut(h) { w.visible = false; }
            }
            self.hidden_by_minimize.entry(frame).or_default().extend(owned);
        } else if let Some(owned) = self.hidden_by_minimize.remove(&frame) {
            for h in owned {
                if let Some(w) = self.get_window_mut(h) { w.visible = true; }
            }
        }
        self.mark_all_dirty();

        let pack = |(cx, cy): (i32, i32)| (cx as u32 & 0xFFFF) | ((cy as u32 & 0xFFFF) << 16);
        if let Some(mq_arc) = self.find_hmq_for_hwnd(client).and_then(|h| self.get_mq(h)) {
            let mut mq = mq_arc.lock_or_recover();
            for (msg, mp1, mp2) in [(WM_SIZE, pack(old), pack((cx, cy))), (WM_PAINT, 0, 0)] {
                mq.messages.push_back(OS2Message { hwnd: client, msg, mp1, mp2, time: 0, x: 0, y: 0 });
            }
            mq.cond.notify_one();
        }
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

    #[test]
    fn minmax_swp_layout() {
        let t = WinRect { x: -2, y: 4, cx: 10, cy: 20 };
        assert_eq!(minmax_swp(7, SWP_MAXIMIZE, t),
                   [SWP_MAXIMIZE | SWP_MOVE | SWP_SIZE, 20, 10, 4, (-2i32) as u32, 0, 7, 0, 0]);
        assert_eq!(minmax_swp(7, 0, t).len() as u32 * 4, SWP_STRUCT_SIZE);
    }
}
