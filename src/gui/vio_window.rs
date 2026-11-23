// SPDX-License-Identifier: GPL-3.0-or-later

//! A VIO program in a window on the PM desktop (a windowed VIO session).
//!
//! On OS/2 a window-compatible text-mode program runs either full screen or
//! in an "OS/2 window" on the PM desktop; the starter chooses (the LX module
//! flags only say what the program allows). Warpine runs it in a PM window
//! by default and in the separate 640 × 400 text window ("full screen")
//! when `WARPINE_VIO=fullscreen` or the program is not window-compatible.
//!
//! [`VioWindow`] lives on the GUI thread: each pass it snapshots the VIO
//! buffer and, when the screen, the view or the cursor blink phase changed,
//! draws the visible part of the cells ([`render_cells`], the text window's
//! renderer) into the frame's surface inside its border, with scroll bars
//! when the window is smaller than the screen (`vio_view.rs`). The frame
//! itself is made by `WindowManager::create_vio_frame` (`loader/vio_frame.rs`).
//!
//! The window's system menu (`system_menu_items`, shown by the renderer as
//! a [`PopupMenu`](super::popup_menu::PopupMenu)) follows the GG24-3732
//! screenshot of an OS/2 window's system menu, without "Window list".
//! Mark / Copy / Copy All / Paste follow GG24-3731 ("Click on the system
//! icon and select 'Mark'. Select an area within the window with the mouse
//! pointer. Click on the system icon and select 'Copy'"; "Select 'Copy all'
//! to copy the screen contents to the clipboard"; Paste: "The text is
//! copied from the clipboard character-by-character into the input area").
//!
//! The rest of the menu is Warpine's design (recorded as unverified):
//! Font Size offers cell sizes drawn from the 8 × 16 glyphs
//! (`VIO_FONT_SIZES`) in a pop-up list rather than OS/2's dialog; Scroll
//! is a mode, like Mark, in which the arrow and page keys scroll the view;
//! in Mark mode the keys mark too (Shift extends, Enter copies) and a mark
//! dragged past the view's edge scrolls it on; Hide hides the frame, which
//! the window list (Ctrl+Esc, its switch-list entry) brings back; Help
//! lists the window's keys in a pop-up panel.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::lx::header::AppType;
use crate::loader::constants::{frame_border_width, CHROME_TITLE_H, WS_MAXIMIZED, WS_MINIMIZED};
use crate::loader::moucalls::{self, MouButton};
use crate::loader::pm_minmax::WinRect;
use crate::loader::constants::{SC_HIDE, SC_TASKMANAGER};
use crate::loader::vio_frame::{
    clamp_vio_size, vio_client_size, CellSize, VIO_FONT_SIZES, VIO_FRAME_FLAGS, VIO_SCROLLBAR_W,
};
use crate::loader::{MutexExt, SharedState};
use super::frame_track::{KeyTrack, TrackOutcome};
pub use super::frame_track::TrackKey;
use super::popup_menu::PopupItem;
use super::system_menu::{can_track, window_items};
use super::text_renderer::{render_cells_sized, VgaTextBuffer};
pub use super::vio_view::ViewKey;
use super::vio_view::{follow_cursor, mark_key, paste_keys, rect_text, BarHit, CellRect, ScrollBar, VioLayout};

/// Environment variable choosing how a VIO program is shown:
/// `window` (the default) or `fullscreen`.
pub const VIO_ENV: &str = "WARPINE_VIO";

/// Half period of the cursor blink, as in the text window.
pub const CURSOR_BLINK: Duration = Duration::from_millis(500);

/// How often the GUI loop looks at the VIO buffer at most: the program
/// writes it without waking the loop.
pub const VIO_POLL: Duration = Duration::from_millis(30);

/// Title prefix while the window is in Mark mode (Warpine's wording).
pub const MARKING_PREFIX: &str = "Marking: ";
/// Title prefix while the window is in Scroll mode (Warpine's wording).
pub const SCROLLING_PREFIX: &str = "Scrolling: ";

/// How often a mark held past the view's edge scrolls it on by a cell
/// (Warpine's choice).
pub const AUTOSCROLL_RATE: Duration = Duration::from_millis(60);

/// Colour of the area right of / below the screen and of the corner
/// between two scroll bars.
const VIO_BLANK: u32 = 0x0000_0000;
const VIO_CORNER: u32 = 0x00CC_CCCC;

/// System-menu items of the VIO window. The system commands use their
/// `SC_*` values; the VIO items have no documented identity, so these
/// values are Warpine's.
pub const CMD_HELP: u32 = 0x0101;
pub const CMD_FONT_SIZE: u32 = 0x0102;
pub const CMD_SCROLL: u32 = 0x0103;
pub const CMD_MARK: u32 = 0x0104;
pub const CMD_COPY: u32 = 0x0105;
pub const CMD_COPY_ALL: u32 = 0x0106;
pub const CMD_PASTE: u32 = 0x0107;
/// The lines of the Help panel: choosing one only closes it.
pub const CMD_HELP_LINE: u32 = 0x0110;
/// The Font Size list's items: `CMD_FONT_FIRST + n` picks
/// `VIO_FONT_SIZES[n]`.
pub const CMD_FONT_FIRST: u32 = 0x0120;

/// The VIO window's modes: Mark and Scroll take the arrow keys (and, in
/// Mark mode, the mouse) from the program until left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VioMode {
    #[default]
    Normal,
    /// Marking a rectangle of cells (system menu Mark).
    Mark,
    /// Scrolling the view with the keyboard (system menu Scroll).
    Scroll,
}

impl VioMode {
    /// What the title shows in front while in this mode.
    fn title_prefix(self) -> &'static str {
        match self {
            VioMode::Normal => "",
            VioMode::Mark => MARKING_PREFIX,
            VioMode::Scroll => SCROLLING_PREFIX,
        }
    }
}

/// The VIO window's system menu for a frame in `state` (`WS_MINIMIZED` /
/// `WS_MAXIMIZED`), in the order of the GG24-3732 screenshot. Restore,
/// Move, Size, Minimize and Maximize are enabled as for PM frames (Restore
/// only when minimized or maximized, Move unless maximized, Size only in
/// the normal state); Hide always (the window list brings the window
/// back); Window list always. Mark and Scroll are checked in their modes;
/// Copy needs a marked rectangle, Paste clipboard text (these conditions
/// are Warpine's choice).
pub fn system_menu_items(state: u32, mode: VioMode, has_mark: bool, can_paste: bool) -> Vec<PopupItem> {
    // The window items as for a PM frame with the VIO frame's flags; it
    // has no hide button, but Hide works from the menu and Alt+F11.
    let mut items = window_items(VIO_FRAME_FLAGS, state);
    for item in items.iter_mut().filter(|i| i.id == SC_HIDE) {
        item.enabled = true;
    }
    items.extend([
        PopupItem::separator(),
        PopupItem::new(SC_TASKMANAGER, "~Window list", "Ctrl+Esc", true),
        PopupItem::separator(),
        PopupItem::new(CMD_HELP, "H~elp...", "", true),
        PopupItem::new(CMD_FONT_SIZE, "~Font Size...", "", true),
        PopupItem::new(CMD_SCROLL, "Scro~ll", "", true).checked(mode == VioMode::Scroll),
        PopupItem::new(CMD_MARK, "M~ark", "", true).checked(mode == VioMode::Mark),
        PopupItem::new(CMD_COPY, "C~opy", "", has_mark),
        PopupItem::new(CMD_COPY_ALL, "Cop~y All", "", true),
        PopupItem::new(CMD_PASTE, "~Paste", "", can_paste),
    ]);
    items
}

/// The Font Size list: each of `VIO_FONT_SIZES` as "width × height", the
/// one in use (`current`) checked.
pub fn font_size_items(current: CellSize) -> Vec<PopupItem> {
    VIO_FONT_SIZES.iter().enumerate().map(|(n, &(w, h))| {
        PopupItem::new(CMD_FONT_FIRST + n as u32, &format!("{w} × {h}"), "", true).checked((w, h) == current)
    }).collect()
}

/// The cell size a Font Size item `id` picks.
pub fn font_size_of(id: u32) -> Option<CellSize> {
    VIO_FONT_SIZES.get(id.checked_sub(CMD_FONT_FIRST)? as usize).copied()
}

/// The Help panel: what the window's keys do (Warpine's own text).
pub fn help_items() -> Vec<PopupItem> {
    let line = |text: &str, keys: &str| PopupItem::new(CMD_HELP_LINE, text, keys, true);
    vec![
        line("System menu", "Alt+Space"),
        line("Window list", "Ctrl+Esc"),
        line("Restore", "Alt+F5"),
        line("Move, Size", "Alt+F7, Alt+F8"),
        line("Minimize, Maximize", "Alt+F9, Alt+F10"),
        line("Hide", "Alt+F11"),
        line("Close", "Alt+F4"),
        line("Full screen", "Alt+Home"),
        PopupItem::separator(),
        line("Mark: drag, or move with", "arrows, PgUp, PgDn, Home, End"),
        line("Extend the mark", "Shift+key"),
        line("Copy the mark", "Enter"),
        line("Scroll: scroll the view with", "arrows, PgUp, PgDn, Home, End"),
        line("Top, bottom of the screen", "Ctrl+Home, Ctrl+End"),
        line("Leave Mark or Scroll", "Esc"),
    ]
}

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
    layout: VioLayout,
    origin: (i32, i32),
    mark: Option<CellRect>,
}

/// A drag with button 1 that the window itself handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    /// Marking a rectangle of cells.
    Mark,
    /// Dragging a scroll bar's thumb, held `offset` pels into it.
    Thumb { vertical: bool, offset: i32 },
}

/// The VIO program's text screen in its frame on the PM desktop.
pub struct VioWindow {
    frame: u32,
    shown: Option<Shown>,
    blink_epoch: Instant,
    /// The whole screen rendered (`cols × 8` by `rows × 16` pels).
    pixels: Vec<u32>,
    /// The client area composed from the visible part and the scroll bars.
    client_px: Vec<u32>,
    /// First column and row the view shows.
    origin: (i32, i32),
    /// The layout of the last update; `None` before it or while minimized.
    layout: Option<VioLayout>,
    /// Where the cursor was at the last update.
    last_cursor: Option<(i32, i32)>,
    /// Mark or Scroll mode, and the marked rectangle's anchor and moving
    /// corner.
    mode: VioMode,
    mark: Option<((i32, i32), (i32, i32))>,
    drag: Option<Drag>,
    /// Where a mark drag last was, from the client's top-left, and when it
    /// last scrolled the view on (autoscroll).
    drag_point: Option<(i32, i32)>,
    last_autoscroll: Option<Instant>,
    /// The frame's title outside Mark and Scroll mode.
    title: Option<String>,
    track: Option<KeyTrack>,
}

impl VioWindow {
    /// The window for `frame` (`WindowManager::create_vio_frame`).
    pub fn new(frame: u32) -> Self {
        VioWindow {
            frame, shown: None, blink_epoch: Instant::now(), pixels: Vec::new(), client_px: Vec::new(),
            origin: (0, 0), layout: None, last_cursor: None, mode: VioMode::Normal, mark: None, drag: None,
            drag_point: None, last_autoscroll: None, title: None, track: None,
        }
    }

    /// The frame it draws into.
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// The first column and row of the screen the view shows: add it to a
    /// cell position within the view to get the cell of the screen.
    pub fn view_origin(&self) -> (u16, u16) {
        (self.origin.0 as u16, self.origin.1 as u16)
    }

    /// How the client is divided between the text view and the scroll bars
    /// (as of the last update); `None` while minimized. Its `cell_at`
    /// gives the screen cell under a client point, scroll position included.
    pub fn layout(&self) -> Option<VioLayout> {
        self.layout
    }

    /// Whether the window is in Mark mode.
    pub fn marking(&self) -> bool {
        self.mode == VioMode::Mark
    }

    /// The window's mode.
    pub fn mode(&self) -> VioMode {
        self.mode
    }

    /// The marked rectangle, if any.
    pub fn mark(&self) -> Option<CellRect> {
        self.mark.map(|(a, b)| CellRect::spanning(a, b))
    }

    /// Whether a keyboard move or size is in progress.
    pub fn tracking(&self) -> bool {
        self.track.is_some()
    }

    /// Whether the window holds button 1 (a mark or thumb drag).
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Bring the frame up to date with the VIO buffer: resize it after a
    /// mode change and redraw the view when the screen, the view or the
    /// cursor blink phase changed. When the cursor moved out of the view,
    /// the view scrolls to it. Returns whether anything was drawn.
    pub fn update(&mut self, shared: &SharedState) -> bool {
        let buf = VgaTextBuffer::snapshot(shared);
        let blink_on = (self.blink_epoch.elapsed().as_millis() / CURSOR_BLINK.as_millis()).is_multiple_of(2);
        let cursor_shown = buf.cursor_visible && blink_on;

        // Lock order: window manager, then surface.
        let (surface, border, client, cell) = {
            let mut wm = shared.window_mgr.lock_or_recover();
            if wm.vio_frame != self.frame { return false; }
            wm.fit_vio_frame(buf.cols, buf.rows);
            let Some(f) = wm.get_window(self.frame) else { return false };
            let minimized = f.style & WS_MINIMIZED != 0;
            let (border, client) = (frame_border_width(f.frame_flags), vio_client_size(f.cx, f.cy));
            if minimized || client.0 == 0 || client.1 == 0 {
                self.layout = None;
                self.shown = None;
                return false;
            }
            let Some(surface) = wm.frame_surface(self.frame) else { return false };
            (surface, border, client, wm.vio_cell)
        };
        let layout = VioLayout::new(client.0, client.1, buf.cols, buf.rows, cell);
        let cursor = (buf.cursor_col as i32, buf.cursor_row as i32);
        self.origin = if self.layout != Some(layout) || self.last_cursor != Some(cursor) {
            follow_cursor(&layout, self.origin, cursor)
        } else {
            layout.clamp_origin(self.origin)
        };
        self.layout = Some(layout);
        self.last_cursor = Some(cursor);

        let shown = Shown {
            cols: buf.cols,
            rows: buf.rows,
            cells: buf.cells.clone(),
            cursor: cursor_shown.then_some((buf.cursor_row, buf.cursor_col, buf.cursor_start, buf.cursor_end)),
            layout,
            origin: self.origin,
            mark: self.mark(),
        };
        if self.shown.as_ref() == Some(&shown) {
            return false;
        }
        let cell_px = (cell.0 as usize, cell.1 as usize);
        let (w, h) = (buf.cols as usize * cell_px.0, buf.rows as usize * cell_px.1);
        self.pixels.resize(w * h, 0);
        render_cells_sized(&buf, blink_on, &mut self.pixels, w, h, cell_px);
        if let Some(r) = self.mark() {
            invert_cells(&mut self.pixels, w, r, cell_px);
        }
        let (cw, ch) = (client.0 as usize, client.1 as usize);
        compose_client(&mut self.client_px, (cw, ch), &self.pixels, (w, h), &layout, self.origin);

        {
            let mut s = surface.lock_or_recover();
            // The client starts at the top of the surface (just below the
            // title bar), inside the left border; PM rows count from the bottom.
            let top = s.height as i32 - 1;
            for (r, row) in self.client_px.chunks(cw).enumerate() {
                s.put_span(border, top - r as i32, row);
            }
        }
        shared.window_mgr.lock_or_recover().mark_dirty(self.frame);
        self.shown = Some(shown);
        true
    }

    /// The screen cell at (`local_x`, `local_y`), pels right of and below
    /// the top-left corner of the text view, as `(row, col)` — the view's
    /// scroll position included (`VioLayout::cell_at`); `None` outside the
    /// view (on a scroll bar, say) or before the first update.
    pub fn cell_at(&self, local_x: i32, local_y: i32) -> Option<(u16, u16)> {
        let layout = self.shown.as_ref()?.layout;
        let (col, row) = layout.cell_at(self.origin, local_x, local_y)?;
        Some((row as u16, col as u16))
    }

    /// The cell nearest to (`local_x`, `local_y`) (see [`Self::cell_at`]),
    /// for a drag that leaves the view.
    fn nearest_cell(&self, local_x: i32, local_y: i32) -> Option<(u16, u16)> {
        let layout = self.shown.as_ref()?.layout;
        let (col, row) = layout.cell_clamped(self.origin, local_x, local_y);
        Some((row as u16, col as u16))
    }

    /// Feed a pointer event at desktop point (`x`, `y`) (SDL coordinates,
    /// `desk_h` the desktop height) to the program's mouse (MOUCALLS):
    /// motion, or `button` going down or up. Over the cells, with the VIO
    /// frame on top there, the event reports the cell under the pointer;
    /// while a button is held the nearest cell, so the release is not lost.
    pub fn pointer_event(&self, shared: &SharedState, desk_h: i32, x: i32, y: i32,
                         button: Option<(MouButton, bool)>) {
        let (local, on_top) = {
            let wm = shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window(self.frame) else { return };
            let bw = frame_border_width(w.frame_flags);
            let top = desk_h - (w.y + w.cy) + CHROME_TITLE_H;
            let on_top = wm.z_hit_test(x, desk_h - 1 - y).is_some_and(|h| wm.is_vio_window(h));
            ((x - (w.x + bw), y - top), on_top)
        };
        let cell = match self.cell_at(local.0, local.1) {
            Some(cell) if on_top => Some(cell),
            _ if moucalls::any_button_down(shared) => self.nearest_cell(local.0, local.1),
            _ => None,
        };
        let Some((row, col)) = cell else { return };
        match button {
            Some((b, down)) => moucalls::mouse_button(shared, row, col, b, down),
            None => moucalls::mouse_moved(shared, row, col),
        }
    }

    /// Desktop point `(x, os2_y)` (bottom-left origin) relative to the
    /// client's top-left corner.
    fn client_point(&self, shared: &SharedState, x: i32, os2_y: i32) -> Option<(i32, i32)> {
        let wm = shared.window_mgr.lock_or_recover();
        let f = wm.get_window(self.frame)?;
        let border = frame_border_width(f.frame_flags);
        let client_top = f.y + f.cy - CHROME_TITLE_H - 1;
        Some((x - f.x - border, client_top - os2_y))
    }

    /// The scroll bars of the current view.
    fn bars(&self) -> [Option<ScrollBar>; 2] {
        match &self.layout {
            Some(l) => [l.vertical_bar(self.origin), l.horizontal_bar(self.origin)],
            None => [None, None],
        }
    }

    /// Scroll the view so that it starts at `origin` (clamped).
    pub fn scroll_to(&mut self, origin: (i32, i32)) {
        if let Some(l) = &self.layout {
            self.origin = l.clamp_origin(origin);
        }
    }

    /// Button 1 went down at desktop `(x, os2_y)` inside the client. A
    /// click on a scroll bar scrolls (arrows: a row or column; track: a
    /// page; thumb: starts a drag); in Mark mode a click on the text starts
    /// marking. Returns whether the window took the click; other clicks are
    /// left to the caller.
    pub fn press(&mut self, shared: &SharedState, x: i32, os2_y: i32) -> bool {
        let Some((cx, cy)) = self.client_point(shared, x, os2_y) else { return false };
        let Some(layout) = self.layout else { return false };
        for bar in self.bars().into_iter().flatten() {
            let Some(hit) = bar.hit(cx, cy) else { continue };
            match hit {
                BarHit::Thumb { offset } => self.drag = Some(Drag::Thumb { vertical: bar.vertical, offset }),
                hit => {
                    let pos = bar.scrolled(hit);
                    self.scroll_to(if bar.vertical { (self.origin.0, pos) } else { (pos, self.origin.1) });
                }
            }
            return true;
        }
        if layout.vbar && layout.hbar && cx >= layout.view_w && cy >= layout.view_h {
            return true; // the corner between the bars
        }
        if self.mode == VioMode::Mark && let Some(cell) = layout.cell_at(self.origin, cx, cy) {
            self.mark = Some((cell, cell));
            self.drag = Some(Drag::Mark);
            self.drag_point = Some((cx, cy));
            return true;
        }
        false
    }

    /// The pointer moved to desktop `(x, os2_y)` with button 1 held: a mark
    /// follows it up to the view's edge (held past it, the view scrolls on:
    /// [`Self::autoscroll`]), a thumb drag scrolls. Returns whether the
    /// window holds the button.
    pub fn drag_to(&mut self, shared: &SharedState, x: i32, os2_y: i32) -> bool {
        let Some(drag) = self.drag else { return false };
        let (Some((cx, cy)), Some(layout)) = (self.client_point(shared, x, os2_y), self.layout) else { return true };
        match drag {
            Drag::Mark => {
                self.drag_point = Some((cx, cy));
                if let Some((anchor, _)) = self.mark {
                    self.mark = Some((anchor, layout.cell_clamped(self.origin, cx, cy)));
                }
            }
            Drag::Thumb { vertical, offset } => {
                let bar = if vertical { layout.vertical_bar(self.origin) } else { layout.horizontal_bar(self.origin) };
                if let Some(bar) = bar {
                    let along = if vertical { cy } else { cx };
                    let pos = bar.pos_for_thumb(along - offset - bar.start());
                    self.scroll_to(if vertical { (self.origin.0, pos) } else { (pos, self.origin.1) });
                }
            }
        }
        true
    }

    /// Button 1 came up: ends a drag. Returns whether there was one.
    pub fn release(&mut self) -> bool {
        self.drag_point = None;
        self.drag.take().is_some()
    }

    /// A mark drag held past the view's edge: every [`AUTOSCROLL_RATE`]
    /// the view scrolls a cell on toward the pointer and the mark follows
    /// to the cell beyond the edge. Called each pass of the GUI loop;
    /// returns whether the view scrolled.
    pub fn autoscroll(&mut self, now: Instant) -> bool {
        let (Some(Drag::Mark), Some((x, y)), Some(layout), Some((anchor, _))) =
            (self.drag, self.drag_point, self.layout, self.mark) else { return false };
        let inside = x >= 0 && y >= 0 && x < layout.view_w && y < layout.view_h;
        if inside || self.last_autoscroll.is_some_and(|t| now.duration_since(t) < AUTOSCROLL_RATE) {
            return false;
        }
        let corner = layout.cell_beyond(self.origin, x, y);
        let origin = follow_cursor(&layout, self.origin, corner);
        // One cell a step in each direction the pointer is past the edge.
        let step = |o: i32, n: i32| o + (n - o).signum();
        let origin = layout.clamp_origin((step(self.origin.0, origin.0), step(self.origin.1, origin.1)));
        self.last_autoscroll = Some(now);
        if origin == self.origin { return false; }
        self.origin = origin;
        self.mark = Some((anchor, layout.cell_clamped(self.origin, x, y)));
        true
    }

    /// Enter or leave Mark mode. The title shows "Marking: " in front while
    /// marking; leaving drops the marked rectangle.
    pub fn set_marking(&mut self, shared: &SharedState, on: bool) {
        if on {
            self.set_mode(shared, VioMode::Mark);
        } else if self.mode == VioMode::Mark {
            self.set_mode(shared, VioMode::Normal);
        }
    }

    /// Enter or leave Scroll mode. The title shows "Scrolling: " in front
    /// meanwhile.
    pub fn set_scrolling(&mut self, shared: &SharedState, on: bool) {
        if on {
            self.set_mode(shared, VioMode::Scroll);
        } else if self.mode == VioMode::Scroll {
            self.set_mode(shared, VioMode::Normal);
        }
    }

    /// Switch to `mode` (one mode at a time): the title gets the new
    /// mode's prefix; leaving Mark mode drops the marked rectangle.
    fn set_mode(&mut self, shared: &SharedState, mode: VioMode) {
        if mode == self.mode { return; }
        let mut wm = shared.window_mgr.lock_or_recover();
        let Some(f) = wm.get_window_mut(self.frame) else { return };
        let plain = self.title.take().unwrap_or_else(|| f.text.clone());
        f.text = format!("{}{plain}", mode.title_prefix());
        if mode != VioMode::Normal {
            self.title = Some(plain);
        }
        if self.mode == VioMode::Mark {
            self.mark = None;
            if self.drag == Some(Drag::Mark) {
                self.drag = None;
                self.drag_point = None;
            }
        }
        self.mode = mode;
        wm.mark_dirty(self.frame);
    }

    /// A key in Mark mode (see `vio_view::mark_key`): the mark moves or,
    /// with `extend` (Shift), grows from its anchor; the view follows its
    /// moving corner. The first key starts at the text cursor.
    pub fn mark_key(&mut self, shared: &SharedState, key: ViewKey, extend: bool) {
        let Some(layout) = self.layout else { return };
        let buf = VgaTextBuffer::snapshot(shared);
        let start = layout.clamp_cell((buf.cursor_col as i32, buf.cursor_row as i32));
        let mark = mark_key(&layout, self.mark, start, key, extend);
        self.mark = Some(mark);
        self.origin = follow_cursor(&layout, self.origin, mark.1);
    }

    /// A key in Scroll mode: the view scrolls (`VioLayout::scrolled_origin`).
    pub fn scroll_key(&mut self, key: ViewKey) {
        if let Some(layout) = self.layout {
            self.origin = layout.scrolled_origin(self.origin, key);
        }
    }

    /// Hide the window (system menu Hide, Alt+F11): the frame is hidden and,
    /// with no other frame of this program to take it, nothing here is
    /// active; the window list brings it back. The desktop server is told.
    pub fn hide(&mut self, shared: &SharedState) {
        {
            let mut wm = shared.window_mgr.lock_or_recover();
            wm.hide_frame(self.frame);
            if wm.focused_hwnd == self.frame {
                wm.claim_activation(0);
            }
        }
        crate::loader::pm_switch::report_host_change(shared);
    }

    /// The text of the marked rectangle, leaving Mark mode (Copy); `None`
    /// when nothing is marked.
    pub fn copy_mark(&mut self, shared: &SharedState) -> Option<String> {
        let rect = self.mark()?;
        let buf = VgaTextBuffer::snapshot(shared);
        let text = rect_text(&buf.cells, &buf.cell_kind, buf.cols, rect);
        self.set_marking(shared, false);
        Some(text)
    }

    /// The text of the whole screen (Copy All).
    pub fn copy_all(&self, shared: &SharedState) -> String {
        let buf = VgaTextBuffer::snapshot(shared);
        rect_text(&buf.cells, &buf.cell_kind, buf.cols, CellRect::screen(buf.cols, buf.rows))
    }

    /// Type `text` into the program (Paste): its characters go into the
    /// keyboard queue, encoded in the active codepage.
    pub fn paste(&self, shared: &SharedState, text: &str) {
        let keys = paste_keys(text, shared.active_codepage.load(Ordering::Relaxed));
        if keys.is_empty() { return; }
        shared.kbd_queue.lock_or_recover().extend(keys);
        shared.kbd_cond.notify_all();
    }

    /// Start moving (`size` false) or sizing the window with the keyboard
    /// (system menu Move / Size): the arrow keys move it, or the edges they
    /// pick (`frame_track.rs`), a cell at a time; Enter keeps the result,
    /// Esc puts the window back. Returns whether it started.
    pub fn start_track(&mut self, shared: &SharedState, size: bool) -> bool {
        let wm = shared.window_mgr.lock_or_recover();
        let Some(f) = wm.get_window(self.frame) else { return false };
        let allowed = can_track(f.frame_flags, f.style & (WS_MINIMIZED | WS_MAXIMIZED), size);
        if allowed {
            let orig = WinRect { x: f.x, y: f.y, cx: f.cx, cy: f.cy };
            self.track = Some(KeyTrack::new(orig, size, wm.vio_cell));
        }
        allowed
    }

    /// Work a keyboard move or size with `key`. Returns whether one is in
    /// progress (the key was used).
    pub fn track_key(&mut self, shared: &SharedState, key: TrackKey) -> bool {
        let Some(track) = self.track.as_mut() else { return false };
        let mut wm = shared.window_mgr.lock_or_recover();
        let Some(f) = wm.get_window(self.frame) else { self.track = None; return true };
        let current = WinRect { x: f.x, y: f.y, cx: f.cx, cy: f.cy };
        let ((cols, rows), cell) = (wm.vio_screen, wm.vio_cell);
        match track.key(current, key, |cx, cy| clamp_vio_size(cx, cy, cols, rows, cell)) {
            TrackOutcome::Continue(None) => {}
            TrackOutcome::Continue(Some(r)) if track.size => wm.set_vio_rect(r),
            TrackOutcome::Continue(Some(r)) => wm.move_frame(self.frame, r.x, r.y),
            TrackOutcome::Accept => self.track = None,
            TrackOutcome::Cancel(orig) => {
                self.track = None;
                wm.set_vio_rect(orig);
            }
        }
        true
    }
}

/// Invert the cells of `rect` in `pixels`, a rendered screen `w` pels wide
/// of `cw × ch` cells.
fn invert_cells(pixels: &mut [u32], w: usize, rect: CellRect, (cw, ch): (usize, usize)) {
    let h = pixels.len() / w.max(1);
    let x0 = rect.col0.max(0) as usize * cw;
    let x1 = ((rect.col1 + 1).max(0) as usize * cw).min(w);
    let y0 = rect.row0.max(0) as usize * ch;
    let y1 = ((rect.row1 + 1).max(0) as usize * ch).min(h);
    for y in y0..y1 {
        for p in &mut pixels[y * w + x0.min(x1)..y * w + x1] {
            *p ^= 0x00FF_FFFF;
        }
    }
}

/// Compose the client area (`cw × ch`) into `out`: the part of the rendered
/// screen (`pixels`, `w × h`) that the view starting at cell `origin`
/// shows, blank beyond the screen's edge, and the scroll bars of `layout`.
fn compose_client(out: &mut Vec<u32>, (cw, ch): (usize, usize), pixels: &[u32], (w, h): (usize, usize),
                  layout: &VioLayout, origin: (i32, i32)) {
    out.clear();
    out.resize(cw * ch, VIO_BLANK);
    let (sx, sy) = (origin.0.max(0) as usize * layout.cell.0 as usize, origin.1.max(0) as usize * layout.cell.1 as usize);
    let (vw, vh) = (layout.view_w.max(0) as usize, layout.view_h.max(0) as usize);
    for r in 0..vh.min(ch) {
        let src_y = sy + r;
        if src_y >= h { break; }
        let n = vw.min(cw).min(w.saturating_sub(sx));
        out[r * cw..r * cw + n].copy_from_slice(&pixels[src_y * w + sx..src_y * w + sx + n]);
    }
    for bar in [layout.vertical_bar(origin), layout.horizontal_bar(origin)].into_iter().flatten() {
        bar.draw(out, cw);
    }
    if layout.vbar && layout.hbar {
        let r = super::damage::Rect::at(layout.view_w, layout.view_h, VIO_SCROLLBAR_W, VIO_SCROLLBAR_W);
        super::damage::fill_clipped(out, cw, r, r, VIO_CORNER);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::constants::{
        SC_CLOSE, SC_HIDE, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE, SC_SIZE, SWP_MAXIMIZE, SWP_MINIMIZE,
    };

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
    fn cells_and_pointer_events_map_to_text_cells() {
        let shared = crate::loader::Loader::new_mock().get_shared();
        shared.console_mgr.lock_or_recover().enable_sdl_mode();
        shared.console_mgr.lock_or_recover().cursor_visible = false;
        let frame = shared.window_mgr.lock_or_recover().create_vio_frame("t", 80, 25);
        let mut vio = VioWindow::new(frame);
        assert_eq!(vio.cell_at(0, 0), None, "nothing shown yet");
        vio.update(&shared);
        assert_eq!(vio.cell_at(0, 0), Some((0, 0)));
        assert_eq!(vio.cell_at(8 * 79 + 7, 16 * 24 + 15), Some((24, 79)));
        assert_eq!(vio.cell_at(17, 33), Some((2, 2)));
        assert_eq!(vio.cell_at(-1, 0), None);
        assert_eq!(vio.cell_at(640, 0), None);
        assert_eq!(vio.cell_at(0, 400), None);
        assert_eq!(vio.nearest_cell(-5, 1000), Some((24, 0)));

        // Desktop coordinates: the cells start inside the border, below the title bar.
        let desk_h = crate::loader::constants::desktop_size().1 as i32;
        let (x0, y0) = {
            let wm = shared.window_mgr.lock_or_recover();
            let w = wm.get_window(frame).unwrap();
            (w.x + frame_border_width(w.frame_flags), desk_h - (w.y + w.cy) + CHROME_TITLE_H)
        };
        shared.mou_mgr.lock_or_recover().open(25, 80);
        let (x, y) = (x0 + 8 * 10 + 3, y0 + 16 * 5 + 2);
        vio.pointer_event(&shared, desk_h, x, y, None);
        vio.pointer_event(&shared, desk_h, x, y, Some((MouButton::B1, true)));
        // A drag out of the window keeps reporting the nearest cell.
        vio.pointer_event(&shared, desk_h, x0 - 50, y, None);
        vio.pointer_event(&shared, desk_h, x0 - 50, y, Some((MouButton::B1, false)));
        // Over the title bar without a button: not the program's.
        vio.pointer_event(&shared, desk_h, x0 + 20, y0 - 5, None);
        let mut mou = shared.mou_mgr.lock_or_recover();
        let got: Vec<_> = std::iter::from_fn(|| mou.read()).map(|e| (e.fs, e.row, e.col)).collect();
        use crate::loader::constants::{MOUSE_BN1_DOWN, MOUSE_MOTION, MOUSE_MOTION_WITH_BN1_DOWN};
        assert_eq!(got, vec![
            (MOUSE_MOTION, 5, 10),
            (MOUSE_BN1_DOWN, 5, 10),
            (MOUSE_MOTION_WITH_BN1_DOWN, 5, 0),
            (0, 5, 0),
        ]);
    }

    fn setup() -> (std::sync::Arc<SharedState>, VioWindow) {
        let shared = crate::loader::Loader::new_mock().get_shared();
        shared.console_mgr.lock_or_recover().enable_sdl_mode();
        // Hide the cursor so the blink phase does not matter.
        shared.console_mgr.lock_or_recover().cursor_visible = false;
        let frame = shared.window_mgr.lock_or_recover().create_vio_frame("t", 80, 25);
        (shared, VioWindow::new(frame))
    }

    /// The program's mouse sees screen cells: with the view scrolled, the
    /// cell under a point is offset by the view's origin.
    #[test]
    fn pointer_cells_include_the_scroll_position() {
        let (shared, mut vio) = setup();
        vio.update(&shared);
        assert_eq!(vio.cell_at(0, 0), Some((0, 0)));
        vio.origin = (10, 3);
        assert_eq!(vio.cell_at(0, 0), Some((3, 10)));
        assert_eq!(vio.cell_at(8 * 2 + 1, 16 + 1), Some((4, 12)));
    }

    #[test]
    fn update_draws_the_screen_into_the_frame_and_only_on_change() {
        let (shared, mut vio) = setup();
        let frame = vio.frame();
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

    /// Make the VIO frame show a `cw × ch` client.
    fn resize_client(shared: &SharedState, cw: i32, ch: i32) {
        let mut wm = shared.window_mgr.lock_or_recover();
        let f = wm.get_window(wm.vio_frame).unwrap();
        let bw = frame_border_width(f.frame_flags);
        let r = WinRect { x: f.x, y: f.y, cx: cw + 2 * bw, cy: ch + CHROME_TITLE_H + bw };
        wm.set_vio_rect(r);
    }

    /// The desktop point at client `(x, y)` (from the client's top-left).
    fn desktop(shared: &SharedState, x: i32, y: i32) -> (i32, i32) {
        let wm = shared.window_mgr.lock_or_recover();
        let f = wm.get_window(wm.vio_frame).unwrap();
        let bw = frame_border_width(f.frame_flags);
        (f.x + bw + x, f.y + f.cy - CHROME_TITLE_H - 1 - y)
    }

    #[test]
    fn a_small_window_shows_scroll_bars_that_scroll_the_view() {
        let (shared, mut vio) = setup();
        resize_client(&shared, 320 + 16, 160 + 16);
        vio.update(&shared);
        assert_eq!(vio.view_origin(), (0, 0));
        // The down arrow of the vertical bar: one row.
        let (x, y) = desktop(&shared, 328, 155);
        assert!(vio.press(&shared, x, y));
        assert!(!vio.release(), "an arrow is not a drag");
        assert_eq!(vio.view_origin(), (0, 1));
        // The right part of the horizontal track: a page (39 columns).
        let (x, y) = desktop(&shared, 300, 165);
        assert!(vio.press(&shared, x, y));
        assert_eq!(vio.view_origin(), (39, 1));
        // The view's pels now come from column 39.
        {
            let mut con = shared.console_mgr.lock_or_recover();
            let cols = con.cols as usize;
            con.buffer[cols + 39] = ('#', 0x0F);
        }
        assert!(vio.update(&shared));
        let surface = shared.window_mgr.lock_or_recover().frame_surface(vio.frame()).unwrap();
        let s = surface.lock().unwrap();
        let bw = 2;
        let white = (0..16).flat_map(|r| s.row_pels(s.height as i32 - 1 - r).unwrap()
            [bw..bw + 8].to_vec()).filter(|&p| p == 0xFF_FF_FF).count();
        assert!(white > 10, "column 39 of row 1 is the top-left cell: {white}");
        // A click on the text is not the window's outside Mark mode.
        drop(s);
        let (x, y) = desktop(&shared, 10, 10);
        assert!(!vio.press(&shared, x, y));
    }

    #[test]
    fn dragging_the_thumb_scrolls() {
        let (shared, mut vio) = setup();
        resize_client(&shared, 320 + 16, 160 + 16);
        vio.update(&shared);
        let bar = vio.layout.unwrap().vertical_bar((0, 0)).unwrap();
        let (t0, _) = bar.thumb();
        let (x, y) = desktop(&shared, 328, t0 + 2);
        assert!(vio.press(&shared, x, y));
        assert!(vio.dragging());
        let (x, y) = desktop(&shared, 328, 1000);
        assert!(vio.drag_to(&shared, x, y));
        assert_eq!(vio.view_origin(), (0, 15), "dragged to the end");
        assert!(vio.release());
        assert!(!vio.drag_to(&shared, x, y));
    }

    #[test]
    fn the_view_scrolls_to_the_cursor_when_it_moves() {
        let (shared, mut vio) = setup();
        resize_client(&shared, 320 + 16, 160 + 16);
        vio.update(&shared);
        {
            let mut con = shared.console_mgr.lock_or_recover();
            (con.cursor_row, con.cursor_col) = (24, 70);
        }
        vio.update(&shared);
        assert_eq!(vio.view_origin(), (31, 15), "just far enough to show (70, 24)");
        // Scrolled away by the user, the view stays while the cursor does.
        vio.scroll_to((0, 0));
        vio.update(&shared);
        assert_eq!(vio.view_origin(), (0, 0));
    }

    #[test]
    fn mark_mode_marks_copies_and_restores_the_title() {
        let (shared, mut vio) = setup();
        {
            let mut con = shared.console_mgr.lock_or_recover();
            let cols = con.cols as usize;
            for (i, c) in "hello".chars().enumerate() { con.buffer[cols + 2 + i] = (c, 7); }
            for (i, c) in "world  x".chars().enumerate() { con.buffer[2 * cols + 2 + i] = (c, 7); }
        }
        vio.update(&shared);
        let frame = vio.frame();
        let title = |s: &SharedState| s.window_mgr.lock_or_recover().get_window(frame).unwrap().text.clone();
        vio.set_marking(&shared, true);
        assert_eq!(title(&shared), "Marking: t");
        // Drag from (2, 1) to (6, 2).
        let (x, y) = desktop(&shared, 2 * 8 + 3, 16 + 3);
        assert!(vio.press(&shared, x, y));
        let (x, y) = desktop(&shared, 6 * 8 + 3, 2 * 16 + 3);
        assert!(vio.drag_to(&shared, x, y));
        assert!(vio.release());
        assert_eq!(vio.mark(), Some(CellRect { col0: 2, row0: 1, col1: 6, row1: 2 }));
        // The marked cells are drawn inverted: blank cells become white.
        assert!(vio.update(&shared));
        let surface = shared.window_mgr.lock_or_recover().frame_surface(vio.frame()).unwrap();
        {
            let s = surface.lock().unwrap();
            let row = s.row_pels(s.height as i32 - 1 - 2 * 16).unwrap();
            assert_eq!(row[2 + 6 * 8 + 7], 0xFF_FF_FF, "inside the mark");
            assert_eq!(row[2 + 7 * 8], 0, "outside it");
        }
        assert_eq!(vio.copy_mark(&shared).as_deref(), Some("hello\r\nworld"));
        assert!(!vio.marking() && vio.mark().is_none());
        assert_eq!(title(&shared), "t");
        assert_eq!(vio.copy_mark(&shared), None, "nothing marked");
        // Copy All: the whole screen, trailing blanks trimmed.
        let all = vio.copy_all(&shared);
        assert!(all.starts_with("\r\n  hello\r\n  world  x\r\n"), "{all:?}");
        assert_eq!(all.matches("\r\n").count(), 24);
    }

    #[test]
    fn keys_mark_from_the_cursor_and_shift_extends_the_mark() {
        let (shared, mut vio) = setup();
        {
            let mut con = shared.console_mgr.lock_or_recover();
            let cols = con.cols as usize;
            for (i, c) in "abc".chars().enumerate() { con.buffer[cols + 4 + i] = (c, 7); }
            (con.cursor_row, con.cursor_col) = (1, 3);
        }
        vio.update(&shared);
        vio.set_marking(&shared, true);
        vio.mark_key(&shared, ViewKey::Right, false);
        assert_eq!(vio.mark(), Some(CellRect { col0: 4, row0: 1, col1: 4, row1: 1 }), "one cell right of the cursor");
        vio.mark_key(&shared, ViewKey::Right, true);
        vio.mark_key(&shared, ViewKey::Right, true);
        assert_eq!(vio.copy_mark(&shared).as_deref(), Some("abc"));
    }

    #[test]
    fn a_keyboard_mark_scrolls_the_view_to_its_moving_corner() {
        let (shared, mut vio) = setup();
        resize_client(&shared, 320 + 16, 160 + 16);
        vio.update(&shared);
        vio.set_marking(&shared, true);
        vio.mark_key(&shared, ViewKey::Bottom, true);
        assert_eq!(vio.view_origin(), (40, 15), "the screen's last cell is in view");
    }

    #[test]
    fn a_mark_held_past_the_view_scrolls_it_on() {
        let (shared, mut vio) = setup();
        resize_client(&shared, 320 + 16, 160 + 16);
        vio.update(&shared);
        vio.set_marking(&shared, true);
        let (x, y) = desktop(&shared, 10, 10);
        assert!(vio.press(&shared, x, y));
        // Below the view (on the horizontal bar and beyond).
        let (x, y) = desktop(&shared, 10, 200);
        assert!(vio.drag_to(&shared, x, y));
        let t0 = Instant::now();
        assert!(vio.autoscroll(t0));
        assert_eq!(vio.view_origin(), (0, 1));
        assert!(!vio.autoscroll(t0 + AUTOSCROLL_RATE / 2), "not yet");
        assert!(vio.autoscroll(t0 + AUTOSCROLL_RATE));
        assert_eq!(vio.view_origin(), (0, 2));
        assert_eq!(vio.mark().map(|r| r.row1), Some(2 + 9), "the mark reaches the view's last row");
        // Back inside: no more scrolling; released: none either.
        let (x, y) = desktop(&shared, 10, 50);
        vio.drag_to(&shared, x, y);
        assert!(!vio.autoscroll(t0 + 4 * AUTOSCROLL_RATE));
        let (x, y) = desktop(&shared, 10, 200);
        vio.drag_to(&shared, x, y);
        vio.release();
        assert!(!vio.autoscroll(t0 + 8 * AUTOSCROLL_RATE));
    }

    #[test]
    fn scroll_mode_scrolls_with_keys_and_shows_in_the_title() {
        let (shared, mut vio) = setup();
        resize_client(&shared, 320 + 16, 160 + 16);
        vio.update(&shared);
        let frame = vio.frame();
        let title = |s: &SharedState| s.window_mgr.lock_or_recover().get_window(frame).unwrap().text.clone();
        vio.set_scrolling(&shared, true);
        assert_eq!((vio.mode(), title(&shared).as_str()), (VioMode::Scroll, "Scrolling: t"));
        vio.scroll_key(ViewKey::PageDown);
        vio.scroll_key(ViewKey::Right);
        assert_eq!(vio.view_origin(), (1, 10));
        // Mark mode replaces Scroll mode, and the title follows.
        vio.set_marking(&shared, true);
        assert_eq!((vio.mode(), title(&shared).as_str()), (VioMode::Mark, "Marking: t"));
        vio.set_scrolling(&shared, false);
        assert_eq!(vio.mode(), VioMode::Mark, "leaving a mode not in force changes nothing");
        vio.set_marking(&shared, false);
        assert_eq!((vio.mode(), title(&shared).as_str()), (VioMode::Normal, "t"));
    }

    #[test]
    fn another_font_size_draws_the_cells_at_that_size() {
        let (shared, mut vio) = setup();
        {
            let mut con = shared.console_mgr.lock_or_recover();
            con.buffer[0] = ('X', 0x07);
        }
        vio.update(&shared);
        let frame = vio.frame();
        assert!(shared.window_mgr.lock_or_recover().set_vio_cell((16, 32)));
        assert!(vio.update(&shared));
        assert_eq!(vio.layout().unwrap().cell, (16, 32));
        let (fcx, fcy) = {
            let wm = shared.window_mgr.lock_or_recover();
            let f = wm.get_window(frame).unwrap();
            (f.cx, f.cy)
        };
        let (cw, ch) = vio_client_size(fcx, fcy);
        let (dw, dh) = crate::loader::constants::desktop_size();
        assert!(cw <= dw as i32 + 2 && ch <= dh as i32 + 2, "fitted on the desktop: {cw} × {ch}");
        // The X fills twice the pels both ways, in the first 16 × 32 cell.
        let surface = shared.window_mgr.lock_or_recover().frame_surface(frame).unwrap();
        let s = surface.lock().unwrap();
        let lit = |w: usize, h: usize| (0..h as i32).flat_map(|r| s.row_pels(s.height as i32 - 1 - r).unwrap()
            [2..2 + w].to_vec()).filter(|&p| p == 0xAA_AA_AA).count();
        let x_pels = crate::gui::text_renderer::get_glyph_for_char('X').iter().map(|b| b.count_ones() as usize).sum::<usize>();
        assert_eq!(lit(16, 32), 4 * x_pels);
        // The pointer maps to the bigger cells.
        assert_eq!(vio.cell_at(17, 33), Some((1, 1)));
    }

    #[test]
    fn hide_hides_the_frame_and_the_window_list_brings_it_back() {
        let (shared, mut vio) = setup();
        let frame = vio.frame();
        shared.window_mgr.lock_or_recover().add_vio_switch(437);
        vio.hide(&shared);
        {
            let wm = shared.window_mgr.lock_or_recover();
            assert!(!wm.get_window(frame).unwrap().visible);
            assert_eq!(wm.focused_hwnd, 0, "nothing here is active");
        }
        let entries = crate::loader::pm_switch::window_list_entries(&shared, 437);
        assert_eq!(entries, vec![(frame, "t".to_string())]);
        assert!(crate::loader::pm_switch::host_switch_to(&shared, frame));
        let wm = shared.window_mgr.lock_or_recover();
        assert!(wm.get_window(frame).unwrap().visible);
        assert_eq!(wm.focused_hwnd, frame);
        assert_eq!(wm.z_order.last(), Some(&frame));
    }

    #[test]
    fn paste_types_the_text_into_the_keyboard_queue() {
        let (shared, vio) = setup();
        vio.paste(&shared, "ab\r\nc");
        let q: Vec<(u8, u8)> = shared.kbd_queue.lock_or_recover().iter().map(|k| (k.ch, k.scan)).collect();
        assert_eq!(q, vec![(b'a', 0), (b'b', 0), (0x0D, 0x1C), (b'c', 0)]);
    }

    #[test]
    fn system_menu_items_follow_the_window_state() {
        let enabled = |items: &[PopupItem], id: u32| items.iter().find(|i| i.id == id).unwrap().enabled;
        let checked = |items: &[PopupItem], id: u32| items.iter().find(|i| i.id == id).unwrap().checked;
        let normal = system_menu_items(0, VioMode::Normal, false, false);
        let ids: Vec<u32> = normal.iter().filter(|i| !i.separator).map(|i| i.id).collect();
        assert_eq!(ids, vec![SC_RESTORE, SC_MOVE, SC_SIZE, SC_MINIMIZE, SC_MAXIMIZE, SC_HIDE, SC_CLOSE,
                             SC_TASKMANAGER, CMD_HELP, CMD_FONT_SIZE, CMD_SCROLL, CMD_MARK, CMD_COPY,
                             CMD_COPY_ALL, CMD_PASTE]);
        assert_eq!(normal.iter().filter(|i| i.separator).count(), 3);
        assert!(!enabled(&normal, SC_RESTORE) && enabled(&normal, SC_MOVE) && enabled(&normal, SC_SIZE));
        assert!(enabled(&normal, SC_MINIMIZE) && enabled(&normal, SC_MAXIMIZE));
        for id in [SC_HIDE, SC_TASKMANAGER, CMD_HELP, CMD_FONT_SIZE, CMD_SCROLL, SC_CLOSE, CMD_MARK, CMD_COPY_ALL] {
            assert!(enabled(&normal, id), "{id:#x} enabled");
        }
        assert!(!enabled(&normal, CMD_COPY) && !enabled(&normal, CMD_PASTE));
        assert!(!checked(&normal, CMD_MARK) && !checked(&normal, CMD_SCROLL));
        let max = system_menu_items(WS_MAXIMIZED, VioMode::Mark, true, true);
        assert!(enabled(&max, SC_RESTORE) && !enabled(&max, SC_MOVE) && !enabled(&max, SC_SIZE));
        assert!(enabled(&max, SC_MINIMIZE) && !enabled(&max, SC_MAXIMIZE));
        assert!(enabled(&max, CMD_COPY) && enabled(&max, CMD_PASTE));
        assert!(checked(&max, CMD_MARK) && !checked(&max, CMD_SCROLL));
        let min = system_menu_items(WS_MINIMIZED, VioMode::Scroll, false, false);
        assert!(enabled(&min, SC_RESTORE) && enabled(&min, SC_MOVE) && !enabled(&min, SC_SIZE));
        assert!(!enabled(&min, SC_MINIMIZE) && enabled(&min, SC_MAXIMIZE));
        assert!(checked(&min, CMD_SCROLL) && !checked(&min, CMD_MARK));
    }

    #[test]
    fn font_size_items_offer_every_size_and_check_the_current_one() {
        use crate::loader::vio_frame::VIO_DEFAULT_CELL;
        let items = font_size_items(VIO_DEFAULT_CELL);
        assert_eq!(items.len(), VIO_FONT_SIZES.len());
        assert_eq!(items.iter().filter(|i| i.checked).count(), 1);
        let current = items.iter().find(|i| i.checked).unwrap();
        assert_eq!(current.text, "8 × 16");
        assert_eq!(font_size_of(current.id), Some(VIO_DEFAULT_CELL));
        for (n, item) in items.iter().enumerate() {
            assert_eq!(font_size_of(item.id), Some(VIO_FONT_SIZES[n]));
        }
        assert_eq!(font_size_of(CMD_FONT_FIRST + VIO_FONT_SIZES.len() as u32), None);
        assert_eq!(font_size_of(CMD_PASTE), None);
    }

    #[test]
    fn the_help_panel_names_the_keys_of_every_item_it_mentions() {
        let items = help_items();
        assert!(items.iter().filter(|i| !i.separator).all(|i| i.id == CMD_HELP_LINE && i.enabled && !i.accel.is_empty()));
        for key in ["Alt+F11", "Ctrl+Esc", "Shift+key", "Enter", "Esc"] {
            assert!(items.iter().any(|i| i.accel == key || i.accel.contains(key)), "{key}");
        }
    }

    #[test]
    fn system_menu_mnemonics_are_unique() {
        let items = system_menu_items(0, VioMode::Normal, false, false);
        let mut letters: Vec<char> = items.iter().filter(|i| !i.separator).map(|i| {
            let at = i.text.find('~').unwrap_or_else(|| panic!("{} has no mnemonic", i.text));
            i.text[at + 1..].chars().next().unwrap().to_ascii_lowercase()
        }).collect();
        let n = letters.len();
        letters.sort();
        letters.dedup();
        assert_eq!(letters.len(), n, "{letters:?}");
    }

    #[test]
    fn keyboard_move_and_size() {
        let (shared, mut vio) = setup();
        let rect = |s: &SharedState| {
            let wm = s.window_mgr.lock_or_recover();
            let f = wm.get_window(wm.vio_frame).unwrap();
            WinRect { x: f.x, y: f.y, cx: f.cx, cy: f.cy }
        };
        let start = rect(&shared);
        assert!(!vio.track_key(&shared, TrackKey::Left), "no move in progress");
        assert!(vio.start_track(&shared, false));
        vio.track_key(&shared, TrackKey::Right);
        vio.track_key(&shared, TrackKey::Down);
        assert_eq!(rect(&shared), WinRect { x: start.x + 8, y: start.y - 16, ..start });
        vio.track_key(&shared, TrackKey::Esc);
        assert_eq!(rect(&shared), start, "Esc puts it back");
        assert!(!vio.tracking());
        // Size: the first arrows pick the right and bottom edges; the full
        // screen cannot grow, but it shrinks a cell at a time.
        assert!(vio.start_track(&shared, true));
        vio.track_key(&shared, TrackKey::Right);
        vio.track_key(&shared, TrackKey::Down);
        assert_eq!(rect(&shared), start, "picking edges moves nothing");
        vio.track_key(&shared, TrackKey::Right);
        assert_eq!(rect(&shared), start, "not larger than the screen");
        vio.track_key(&shared, TrackKey::Left);
        vio.track_key(&shared, TrackKey::Up);
        let r = rect(&shared);
        assert_eq!((r.cx, r.cy), (start.cx - 8, start.cy - 16));
        assert_eq!(r.y + r.cy, start.y + start.cy, "the top edge stays");
        vio.track_key(&shared, TrackKey::Enter);
        assert!(!vio.tracking());
        assert_eq!(rect(&shared), r, "Enter keeps it");
        // Maximized: no keyboard move or size.
        shared.window_mgr.lock_or_recover().vio_minmax(SWP_MAXIMIZE);
        assert!(!vio.start_track(&shared, false) && !vio.start_track(&shared, true));
        shared.window_mgr.lock_or_recover().vio_minmax(SWP_MINIMIZE);
        assert!(vio.start_track(&shared, false), "a minimized window can be moved");
    }

    #[test]
    fn a_minimized_window_draws_nothing() {
        let (shared, mut vio) = setup();
        vio.update(&shared);
        shared.window_mgr.lock_or_recover().vio_minmax(SWP_MINIMIZE);
        assert!(!vio.update(&shared));
        let (x, y) = desktop(&shared, 1, 1);
        assert!(!vio.press(&shared, x, y));
    }

    #[test]
    fn compose_puts_the_view_blank_space_and_the_corner_in_place() {
        // A 2 × 1 screen in a client larger than it, with both bars forced.
        let layout = VioLayout { cols: 2, rows: 1, view_w: 20, view_h: 20, vbar: true, hbar: true, cell: (8, 16) };
        let (w, h) = (16usize, 16usize);
        let pixels = vec![0x11u32; w * h];
        let mut out = Vec::new();
        compose_client(&mut out, (36, 36), &pixels, (w, h), &layout, (0, 0));
        assert_eq!(out[0], 0x11, "the screen");
        assert_eq!(out[18], VIO_BLANK, "right of the screen");
        assert_eq!(out[17 * 36], VIO_BLANK, "below the screen");
        assert_eq!(out[30 * 36 + 30], VIO_CORNER, "the corner");
    }
}
