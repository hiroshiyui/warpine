// SPDX-License-Identifier: GPL-3.0-only

//! The frame of a windowed VIO session: a text-mode program shown in a
//! window on the PM desktop, as OS/2 runs a window-compatible program in an
//! "OS/2 window" session.
//!
//! The frame belongs to the host, not to a guest message queue: it has a
//! title bar, a system-menu icon and a border, its client is the text screen
//! (`cols × 8` by `rows × 16` pels, drawn by `gui::vio_window`), keys typed
//! while it is active go to the VIO program's keyboard queue, and closing it
//! ends the program. It is not sizeable (no `FCF_SIZEBORDER` / maximize);
//! it follows the screen size when the program changes mode (`VioSetMode`).

use super::constants::{
    default_frame_pos, desktop_size, frame_border_width, CHROME_TITLE_H, FCF_BORDER, FCF_SYSMENU,
    FCF_TITLEBAR, HWND_DESKTOP,
};
use super::pm_types::WindowManager;

/// Width and height in pels of one text cell.
pub const VIO_CELL_W: i32 = 8;
pub const VIO_CELL_H: i32 = 16;

/// The frame-creation flags of a VIO window.
pub const VIO_FRAME_FLAGS: u32 = FCF_TITLEBAR | FCF_SYSMENU | FCF_BORDER;

/// The size of a VIO frame for a `cols × rows` text screen: the cells, the
/// border on the left, right and bottom, and the title bar on top (which the
/// top border is part of).
pub fn vio_frame_size(cols: u16, rows: u16) -> (i32, i32) {
    let bw = frame_border_width(VIO_FRAME_FLAGS);
    (cols as i32 * VIO_CELL_W + 2 * bw, rows as i32 * VIO_CELL_H + CHROME_TITLE_H + bw)
}

impl WindowManager {
    /// Window class of a VIO frame and its client.
    pub const VIO_CLASS: &'static str = "#VioWindow";

    /// Create the VIO frame for a `cols × rows` screen titled `title`, on top
    /// of the desktop and active, and remember it as `vio_frame`. Returns
    /// the frame.
    pub fn create_vio_frame(&mut self, title: &str, cols: u16, rows: u16) -> u32 {
        let frame = self.create_window(Self::VIO_CLASS.to_string(), HWND_DESKTOP, 0);
        let client = self.create_window(Self::VIO_CLASS.to_string(), frame, 0);
        self.frame_to_client.insert(frame, client);
        let (cx, cy) = vio_frame_size(cols, rows);
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
        frame
    }

    /// Fit the VIO frame to a `cols × rows` screen, keeping its top-left
    /// corner where it is. Returns whether the size changed.
    pub fn fit_vio_frame(&mut self, cols: u16, rows: u16) -> bool {
        let frame = self.vio_frame;
        let (cx, cy) = vio_frame_size(cols, rows);
        let Some(w) = self.get_window_mut(frame) else { return false };
        if (w.cx, w.cy) == (cx, cy) { return false; }
        // PM y counts from the bottom: keep the top edge in place.
        w.y += w.cy - cy;
        (w.cx, w.cy) = (cx, cy);
        let (scx, scy) = self.layout_frame(frame);
        self.resize_surface(frame, scx, scy);
        self.mark_all_dirty();
        true
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
}
