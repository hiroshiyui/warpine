// SPDX-License-Identifier: GPL-3.0-only

//! A VIO program in a window on the PM desktop (a windowed VIO session).
//!
//! On OS/2 a window-compatible text-mode program runs either full screen or
//! in an "OS/2 window" on the PM desktop; the starter chooses (the LX module
//! flags only say what the program allows). Warpine runs it in a PM window
//! by default and in the separate 640 × 400 text window ("full screen")
//! when `WARPINE_VIO=fullscreen` or the program is not window-compatible.
//!
//! [`VioWindow`] lives on the GUI thread: each pass it snapshots the VIO
//! buffer and, when the screen or the cursor blink phase changed, draws the
//! cells ([`render_cells`], the text window's renderer) into the frame's
//! surface inside its border. The frame itself is made by
//! `WindowManager::create_vio_frame` (`loader/vio_frame.rs`).

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::lx::header::AppType;
use crate::loader::constants::frame_border_width;
use crate::loader::{MutexExt, SharedState};
use super::text_renderer::{render_cells, VgaTextBuffer};

/// Environment variable choosing how a VIO program is shown:
/// `window` (the default) or `fullscreen`.
pub const VIO_ENV: &str = "WARPINE_VIO";

/// Half period of the cursor blink, as in the text window.
pub const CURSOR_BLINK: Duration = Duration::from_millis(500);

/// How often the GUI loop looks at the VIO buffer at most: the program
/// writes it without waking the loop.
pub const VIO_POLL: Duration = Duration::from_millis(30);

/// How a VIO program is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VioDisplay {
    /// A frame on the PM desktop (an "OS/2 window" session).
    Window,
    /// The separate text window (an "OS/2 full screen" session).
    FullScreen,
}

/// Choose the display of a VIO program of type `app_type`, given the value
/// of [`VIO_ENV`]. A program that is not window-compatible always runs full
/// screen, as OS/2 does; otherwise `fullscreen` asks for the text window and
/// anything else (or nothing) for a PM window. Returns the choice and, for
/// a request that cannot be honoured or is not understood, a warning.
pub fn choose_display(app_type: AppType, env: Option<&str>) -> (VioDisplay, Option<String>) {
    let wanted = match env.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        None | Some("") | Some("window") => VioDisplay::Window,
        Some("fullscreen") => VioDisplay::FullScreen,
        Some(other) => return (choose_display(app_type, None).0,
                               Some(format!("{VIO_ENV}={other} is not window or fullscreen; ignored"))),
    };
    if app_type == AppType::NotWindowCompatible {
        let warning = (wanted == VioDisplay::Window && env.is_some()).then(||
            "the program is not window-compatible; running it full screen".to_string());
        return (VioDisplay::FullScreen, warning);
    }
    (wanted, None)
}

/// What the window last showed, to redraw only on a change.
#[derive(PartialEq)]
struct Shown {
    cols: u16,
    rows: u16,
    cells: Vec<(char, u8)>,
    cursor: Option<(u16, u16, u8, u8)>,
}

/// The VIO program's text screen in its frame on the PM desktop.
pub struct VioWindow {
    frame: u32,
    shown: Option<Shown>,
    blink_epoch: Instant,
    pixels: Vec<u32>,
}

impl VioWindow {
    /// The window for `frame` (`WindowManager::create_vio_frame`).
    pub fn new(frame: u32) -> Self {
        VioWindow { frame, shown: None, blink_epoch: Instant::now(), pixels: Vec::new() }
    }

    /// The frame it draws into.
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// Bring the frame up to date with the VIO buffer: resize it after a
    /// mode change and redraw the cells when the screen or the cursor
    /// blink phase changed. Returns whether anything was drawn.
    pub fn update(&mut self, shared: &SharedState) -> bool {
        let buf = VgaTextBuffer::snapshot(shared);
        let blink_on = (self.blink_epoch.elapsed().as_millis() / CURSOR_BLINK.as_millis()).is_multiple_of(2);
        let cursor_shown = buf.cursor_visible && blink_on;
        let shown = Shown {
            cols: buf.cols,
            rows: buf.rows,
            cells: buf.cells.clone(),
            cursor: cursor_shown.then_some((buf.cursor_row, buf.cursor_col, buf.cursor_start, buf.cursor_end)),
        };
        if self.shown.as_ref() == Some(&shown) {
            return false;
        }
        let (w, h) = (buf.cols as usize * 8, buf.rows as usize * 16);
        self.pixels.resize(w * h, 0);
        render_cells(&buf, blink_on, &mut self.pixels, w, h);

        // Lock order: window manager, then surface.
        let (surface, border) = {
            let mut wm = shared.window_mgr.lock_or_recover();
            if wm.vio_frame != self.frame { return false; }
            wm.fit_vio_frame(buf.cols, buf.rows);
            let border = wm.get_window(self.frame).map_or(0, |f| frame_border_width(f.frame_flags));
            let Some(surface) = wm.frame_surface(self.frame) else { return false };
            (surface, border)
        };
        {
            let mut s = surface.lock_or_recover();
            // The cells start at the top of the surface (just below the
            // title bar), inside the left border; PM rows count from the bottom.
            let top = s.height as i32 - 1;
            for (r, row) in self.pixels.chunks(w).enumerate() {
                s.put_span(border, top - r as i32, row);
            }
        }
        shared.window_mgr.lock_or_recover().mark_dirty(self.frame);
        self.shown = Some(shown);
        true
    }

    /// The window was closed (close button, system-menu double click): the
    /// program ends, as when its session is closed on OS/2.
    pub fn close(&self, shared: &SharedState) {
        shared.exit_requested.store(true, Ordering::Relaxed);
        shared.kbd_cond.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_is_the_default_unless_the_program_or_the_user_says_otherwise() {
        use AppType::*;
        assert_eq!(choose_display(WindowCompatible, None), (VioDisplay::Window, None));
        assert_eq!(choose_display(Unspecified, None), (VioDisplay::Window, None));
        assert_eq!(choose_display(WindowCompatible, Some("window")), (VioDisplay::Window, None));
        assert_eq!(choose_display(WindowCompatible, Some(" FullScreen ")), (VioDisplay::FullScreen, None));
        // A program that cannot run in a window runs full screen.
        assert_eq!(choose_display(NotWindowCompatible, None), (VioDisplay::FullScreen, None));
        let (d, warning) = choose_display(NotWindowCompatible, Some("window"));
        assert_eq!(d, VioDisplay::FullScreen);
        assert!(warning.is_some());
        // An unknown value is reported and ignored.
        let (d, warning) = choose_display(WindowCompatible, Some("tiny"));
        assert_eq!(d, VioDisplay::Window);
        assert!(warning.unwrap().contains("tiny"));
    }

    #[test]
    fn closing_the_window_ends_the_program() {
        let shared = crate::loader::Loader::new_mock().get_shared();
        let frame = shared.window_mgr.lock_or_recover().create_vio_frame("t", 80, 25);
        VioWindow::new(frame).close(&shared);
        assert!(shared.exit_requested.load(Ordering::Relaxed));
    }

    #[test]
    fn update_draws_the_screen_into_the_frame_and_only_on_change() {
        let shared = crate::loader::Loader::new_mock().get_shared();
        shared.console_mgr.lock_or_recover().enable_sdl_mode();
        let frame = shared.window_mgr.lock_or_recover().create_vio_frame("t", 80, 25);
        let mut vio = VioWindow::new(frame);
        // Hide the cursor so the blink phase does not matter.
        shared.console_mgr.lock_or_recover().cursor_visible = false;
        assert!(vio.update(&shared), "first update draws");
        assert!(!vio.update(&shared), "nothing changed");
        let surface = shared.window_mgr.lock_or_recover().frame_surface(frame).unwrap();
        let border = frame_border_width(crate::loader::vio_frame::VIO_FRAME_FLAGS);
        // An empty screen is black cells inside the border.
        let top = {
            let s = surface.lock().unwrap();
            s.row_pels(s.height as i32 - 1).unwrap()
        };
        assert!(top[border as usize..border as usize + 640].iter().all(|&p| p == 0));
        // A character written to the buffer is drawn.
        {
            let mut con = shared.console_mgr.lock_or_recover();
            con.buffer[0] = ('X', 0x07);
        }
        assert!(vio.update(&shared));
        let s = surface.lock().unwrap();
        let lit = (0..16).flat_map(|r| s.row_pels(s.height as i32 - 1 - r).unwrap()
            [border as usize..border as usize + 8].to_vec()).filter(|&p| p == 0xAA_AA_AA).count();
        assert!(lit > 10, "the X is drawn in light grey: {lit} pels");
    }
}
