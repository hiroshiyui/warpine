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

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::lx::header::AppType;
use crate::loader::constants::{
    frame_border_width, CHROME_TITLE_H, SC_CLOSE, SC_HIDE, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE,
    SC_RESTORE, SC_SIZE, WS_MAXIMIZED, WS_MINIMIZED,
};
use crate::loader::moucalls::{self, MouButton};
use crate::loader::pm_minmax::WinRect;
use crate::loader::vio_frame::{clamp_vio_size, vio_client_size, VIO_CELL_H, VIO_CELL_W, VIO_SCROLLBAR_W};
use crate::loader::{MutexExt, SharedState};
use super::popup_menu::PopupItem;
use super::text_renderer::{render_cells, VgaTextBuffer};
use super::vio_view::{follow_cursor, paste_keys, rect_text, BarHit, CellRect, ScrollBar, VioLayout};

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

/// The VIO window's system menu for a frame in `state` (`WS_MINIMIZED` /
/// `WS_MAXIMIZED`), in the order of the GG24-3732 screenshot. Restore,
/// Move, Size, Minimize and Maximize are enabled as for PM frames (Restore
/// only when minimized or maximized, Move unless maximized, Size only in
/// the normal state). Hide is greyed: there is no window list to bring
/// the window back. Help and Font Size are greyed (not implemented), and
/// Scroll too (its behaviour is not documented). Mark is checked in Mark
/// mode; Copy needs a marked rectangle, Paste clipboard text (these
/// conditions are Warpine's choice).
pub fn system_menu_items(state: u32, marking: bool, has_mark: bool, can_paste: bool) -> Vec<PopupItem> {
    let normal = state & (WS_MINIMIZED | WS_MAXIMIZED) == 0;
    vec![
        PopupItem::new(SC_RESTORE, "~Restore", "Alt+F5", !normal),
        PopupItem::new(SC_MOVE, "~Move", "Alt+F7", state & WS_MAXIMIZED == 0),
        PopupItem::new(SC_SIZE, "~Size", "Alt+F8", normal),
        PopupItem::new(SC_MINIMIZE, "Mi~nimize", "Alt+F9", state & WS_MINIMIZED == 0),
        PopupItem::new(SC_MAXIMIZE, "Ma~ximize", "Alt+F10", state & WS_MAXIMIZED == 0),
        PopupItem::new(SC_HIDE, "~Hide", "Alt+F11", false),
        PopupItem::separator(),
        PopupItem::new(SC_CLOSE, "~Close", "Alt+F4", true),
        PopupItem::separator(),
        PopupItem::new(CMD_HELP, "H~elp...", "", false),
        PopupItem::new(CMD_FONT_SIZE, "~Font Size...", "", false),
        PopupItem::new(CMD_SCROLL, "Scro~ll", "", false),
        PopupItem::new(CMD_MARK, "M~ark", "", true).checked(marking),
        PopupItem::new(CMD_COPY, "C~opy", "", has_mark),
        PopupItem::new(CMD_COPY_ALL, "Cop~y All", "", true),
        PopupItem::new(CMD_PASTE, "~Paste", "", can_paste),
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

/// A key that works a keyboard move or size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKey { Left, Right, Up, Down, Enter, Esc }

/// A keyboard move or size (system menu Move / Size), from the frame's
/// position and size `orig`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Track {
    size: bool,
    orig: WinRect,
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
    /// Mark mode, and the marked rectangle's anchor and moving corner.
    marking: bool,
    mark: Option<((i32, i32), (i32, i32))>,
    drag: Option<Drag>,
    /// The frame's title outside Mark mode.
    title: Option<String>,
    track: Option<Track>,
}

impl VioWindow {
    /// The window for `frame` (`WindowManager::create_vio_frame`).
    pub fn new(frame: u32) -> Self {
        VioWindow {
            frame, shown: None, blink_epoch: Instant::now(), pixels: Vec::new(), client_px: Vec::new(),
            origin: (0, 0), layout: None, last_cursor: None, marking: false, mark: None, drag: None,
            title: None, track: None,
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
        self.marking
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
        let (surface, border, client) = {
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
            (surface, border, client)
        };
        let layout = VioLayout::new(client.0, client.1, buf.cols, buf.rows);
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
        let (w, h) = (buf.cols as usize * VIO_CELL_W as usize, buf.rows as usize * VIO_CELL_H as usize);
        self.pixels.resize(w * h, 0);
        render_cells(&buf, blink_on, &mut self.pixels, w, h);
        if let Some(r) = self.mark() {
            invert_cells(&mut self.pixels, w, r);
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

    /// The window was closed (close button, system-menu double click): the
    /// program ends, as when its session is closed on OS/2.
    pub fn close(&self, shared: &SharedState) {
        shared.exit_requested.store(true, Ordering::Relaxed);
        shared.kbd_cond.notify_all();
        shared.mou_cond.notify_all();
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
        if self.marking && let Some(cell) = layout.cell_at(self.origin, cx, cy) {
            self.mark = Some((cell, cell));
            self.drag = Some(Drag::Mark);
            return true;
        }
        false
    }

    /// The pointer moved to desktop `(x, os2_y)` with button 1 held: a mark
    /// follows it (up to the view's edge; dragging past it does not scroll,
    /// Warpine's simplification), a thumb drag scrolls. Returns whether the
    /// window holds the button.
    pub fn drag_to(&mut self, shared: &SharedState, x: i32, os2_y: i32) -> bool {
        let Some(drag) = self.drag else { return false };
        let (Some((cx, cy)), Some(layout)) = (self.client_point(shared, x, os2_y), self.layout) else { return true };
        match drag {
            Drag::Mark => {
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
        self.drag.take().is_some()
    }

    /// Enter or leave Mark mode. The title shows "Marking: " in front while
    /// marking; leaving drops the marked rectangle.
    pub fn set_marking(&mut self, shared: &SharedState, on: bool) {
        let mut wm = shared.window_mgr.lock_or_recover();
        let Some(f) = wm.get_window_mut(self.frame) else { return };
        if on && !self.marking {
            self.title = Some(f.text.clone());
            f.text = format!("{MARKING_PREFIX}{}", f.text);
        } else if !on && self.marking && let Some(t) = self.title.take() {
            f.text = t;
        }
        self.marking = on;
        if !on {
            self.mark = None;
            if self.drag == Some(Drag::Mark) { self.drag = None; }
        }
        wm.mark_dirty(self.frame);
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
    /// (system menu Move / Size): the arrow keys move it, or move its right
    /// and bottom edges, a cell at a time; Enter keeps the result, Esc puts
    /// the window back. PM lets the user pick the edges to size and move
    /// the pointer with the keys; Warpine's keyboard size moves the right
    /// and bottom edges only (simplification). Returns whether it started.
    pub fn start_track(&mut self, shared: &SharedState, size: bool) -> bool {
        let wm = shared.window_mgr.lock_or_recover();
        let Some(f) = wm.get_window(self.frame) else { return false };
        let state = f.style & (WS_MINIMIZED | WS_MAXIMIZED);
        let allowed = if size { state == 0 } else { state & WS_MAXIMIZED == 0 };
        if allowed {
            self.track = Some(Track { size, orig: WinRect { x: f.x, y: f.y, cx: f.cx, cy: f.cy } });
        }
        allowed
    }

    /// Work a keyboard move or size with `key`. Returns whether one is in
    /// progress (the key was used).
    pub fn track_key(&mut self, shared: &SharedState, key: TrackKey) -> bool {
        let Some(track) = self.track else { return false };
        let mut wm = shared.window_mgr.lock_or_recover();
        let Some(f) = wm.get_window(self.frame) else { self.track = None; return true };
        let mut r = WinRect { x: f.x, y: f.y, cx: f.cx, cy: f.cy };
        let (dx, dy) = match key {
            TrackKey::Left => (-VIO_CELL_W, 0),
            TrackKey::Right => (VIO_CELL_W, 0),
            TrackKey::Up => (0, VIO_CELL_H),
            TrackKey::Down => (0, -VIO_CELL_H),
            TrackKey::Enter => { self.track = None; return true; }
            TrackKey::Esc => {
                self.track = None;
                wm.set_vio_rect(track.orig);
                return true;
            }
        };
        if track.size {
            let (cols, rows) = wm.vio_screen;
            let top = r.y + r.cy;
            (r.cx, r.cy) = clamp_vio_size(r.cx + dx, r.cy - dy, cols, rows);
            r.y = top - r.cy;
            wm.set_vio_rect(r);
        } else {
            wm.move_frame(self.frame, r.x + dx, r.y + dy);
        }
        true
    }
}

/// Invert the cells of `rect` in `pixels`, a rendered screen `w` pels wide.
fn invert_cells(pixels: &mut [u32], w: usize, rect: CellRect) {
    let h = pixels.len() / w.max(1);
    let (cw, ch) = (VIO_CELL_W as usize, VIO_CELL_H as usize);
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
    let (sx, sy) = (origin.0.max(0) as usize * VIO_CELL_W as usize, origin.1.max(0) as usize * VIO_CELL_H as usize);
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
    use crate::loader::constants::{SWP_MAXIMIZE, SWP_MINIMIZE};

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
    fn closing_the_window_ends_the_program() {
        let (shared, vio) = setup();
        vio.close(&shared);
        assert!(shared.exit_requested.load(Ordering::Relaxed));
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
    fn paste_types_the_text_into_the_keyboard_queue() {
        let (shared, vio) = setup();
        vio.paste(&shared, "ab\r\nc");
        let q: Vec<(u8, u8)> = shared.kbd_queue.lock_or_recover().iter().map(|k| (k.ch, k.scan)).collect();
        assert_eq!(q, vec![(b'a', 0), (b'b', 0), (0x0D, 0x1C), (b'c', 0)]);
    }

    #[test]
    fn system_menu_items_follow_the_window_state() {
        let enabled = |items: &[PopupItem], id: u32| items.iter().find(|i| i.id == id).unwrap().enabled;
        let normal = system_menu_items(0, false, false, false);
        let ids: Vec<u32> = normal.iter().filter(|i| !i.separator).map(|i| i.id).collect();
        assert_eq!(ids, vec![SC_RESTORE, SC_MOVE, SC_SIZE, SC_MINIMIZE, SC_MAXIMIZE, SC_HIDE, SC_CLOSE,
                             CMD_HELP, CMD_FONT_SIZE, CMD_SCROLL, CMD_MARK, CMD_COPY, CMD_COPY_ALL, CMD_PASTE]);
        assert_eq!(normal.iter().filter(|i| i.separator).count(), 2);
        assert!(!enabled(&normal, SC_RESTORE) && enabled(&normal, SC_MOVE) && enabled(&normal, SC_SIZE));
        assert!(enabled(&normal, SC_MINIMIZE) && enabled(&normal, SC_MAXIMIZE));
        for id in [SC_HIDE, CMD_HELP, CMD_FONT_SIZE, CMD_SCROLL, CMD_COPY, CMD_PASTE] {
            assert!(!enabled(&normal, id), "{id:#x} greyed");
        }
        assert!(enabled(&normal, SC_CLOSE) && enabled(&normal, CMD_MARK) && enabled(&normal, CMD_COPY_ALL));
        let max = system_menu_items(WS_MAXIMIZED, true, true, true);
        assert!(enabled(&max, SC_RESTORE) && !enabled(&max, SC_MOVE) && !enabled(&max, SC_SIZE));
        assert!(enabled(&max, SC_MINIMIZE) && !enabled(&max, SC_MAXIMIZE));
        assert!(enabled(&max, CMD_COPY) && enabled(&max, CMD_PASTE));
        assert!(max.iter().find(|i| i.id == CMD_MARK).unwrap().checked);
        let min = system_menu_items(WS_MINIMIZED, false, false, false);
        assert!(enabled(&min, SC_RESTORE) && enabled(&min, SC_MOVE) && !enabled(&min, SC_SIZE));
        assert!(!enabled(&min, SC_MINIMIZE) && enabled(&min, SC_MAXIMIZE));
    }

    #[test]
    fn system_menu_mnemonics_are_unique() {
        let items = system_menu_items(0, false, false, false);
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
        // Size: the full screen cannot grow, but it shrinks a cell at a time.
        assert!(vio.start_track(&shared, true));
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
        let layout = VioLayout { cols: 2, rows: 1, view_w: 20, view_h: 20, vbar: true, hbar: true };
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
