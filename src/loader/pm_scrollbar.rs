// SPDX-License-Identifier: GPL-3.0-or-later

//! The scroll-bar control (`WC_SCROLLBAR`): its range, slider position and
//! thumb size (`SBM_SETSCROLLBAR`, `SBM_SETPOS`, `SBM_QUERYPOS`,
//! `SBM_QUERYRANGE`, `SBM_SETTHUMBSIZE`, `SBCDATA` control data), its
//! painting, and the pointer: the arrows, the shaft on either side of the
//! slider and the slider itself, reported to the owner as `WM_VSCROLL`
//! (`SBS_VERT`) or `WM_HSCROLL` (`SBS_HORZ`) with mp1 = MPFROMSHORT(id) and
//! mp2 = MPFROM2SHORT(slider position, `SB_*` command) — PM Reference,
//! "Scroll Bar Control Window Processing", `WM_VSCROLL`, `SBM_*`.
//!
//! The scroll bar does not move its slider for line and page commands: the
//! owner scrolls its view and sets the new position with `SBM_SETPOS`. A
//! dragged slider follows the pointer (`SB_SLIDERTRACK` on every change, the
//! final `SB_SLIDERPOSITION` on release); line and page scrolling ends with
//! `SB_ENDSCROLL` on release, slider dragging does not (EDM2 `WM_VSCROLL`).
//! While an arrow or the shaft stays pressed under the pointer, the command
//! repeats on the `WM_TIMER(TID_SCROLL)` drag-scroll timer, first after
//! `SV_FIRSTSCROLLRATE`, then every `SV_SCROLLRATE`. Without
//! `SBS_AUTOTRACK` the slider stays where it is while it is dragged and an
//! outline follows the pointer; with it the slider itself follows.
//!
//! A focused scroll bar takes the keyboard: the arrows along its axis send
//! the line commands, Page Up / Page Down the page commands, and the key's
//! release `SB_ENDSCROLL`; other keys go to its owner. TODO(unverified):
//! the reference names no keys, nor whether Home / End scroll, nor whether a
//! horizontal bar pages with Page Up / Page Down (Warpine's choice: yes).
//!
//! Geometry runs along the bar's axis from its "first" end (top of a
//! vertical bar, left of a horizontal one): an arrow button at each end, as
//! long as the bar is broad (at most half its length), and the shaft
//! between them, in which the slider sits in proportion to its position.

use super::constants::*;
use super::{control_box, ApiResult, Loader, MutexExt, Painter, CONTROL_FACE_COLOR,
            CONTROL_PRESSED_COLOR, CONTROL_SCROLLBAR_COLOR, CONTROL_TEXT_COLOR};
use crate::gui::theme;

pub(super) const CLASS_SCROLLBAR: &str = "WC_SCROLLBAR";

/// Shortest slider drawn for a proportional thumb size (`SBM_SETTHUMBSIZE`).
/// TODO(unverified): the PM references give no minimum.
const MIN_THUMB_LEN: i32 = 8;
/// Colour of the arrow glyphs of a disabled scroll bar.
const DISABLED_ARROW_COLOR: u32 = 0x0080_8080;
/// Gap between an arrow button's edge and its triangle.
const ARROW_GLYPH_INSET: i32 = 4;

/// A part of a scroll bar under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The arrow at the first end (up / left): `SB_LINEUP` / `SB_LINELEFT`.
    LineUp,
    /// The arrow at the last end (down / right).
    LineDown,
    /// The shaft between the first arrow and the slider.
    PageUp,
    /// The shaft between the slider and the last arrow.
    PageDown,
    /// The slider (thumb).
    Thumb,
}

impl Part {
    /// The `SB_*` command a press on this part sends (the slider has none).
    pub fn command(self) -> Option<u32> {
        match self {
            Part::LineUp => Some(SB_LINEUP),
            Part::LineDown => Some(SB_LINEDOWN),
            Part::PageUp => Some(SB_PAGEUP),
            Part::PageDown => Some(SB_PAGEDOWN),
            Part::Thumb => None,
        }
    }
}

/// The part held with button 1 and, while the slider is dragged, where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub part: Part,
    /// Pointer offset from the slider's start, along the axis, at the press.
    pub grab: i32,
    /// Slider position the drag has reached (`SB_SLIDERTRACK`).
    pub track: i16,
    /// The pointer is over the held arrow (drawn pressed).
    pub over: bool,
}

/// Range, position and thumb size of a scroll bar, and the part held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScrollBarState {
    pub pos: i16,
    pub first: i16,
    pub last: i16,
    /// `SBM_SETTHUMBSIZE`: (visible, total); `None` = a square slider.
    pub thumb_size: Option<(u16, u16)>,
    pub held: Option<Held>,
}

impl ScrollBarState {
    /// SBM_SETSCROLLBAR: the range `first`..=`last` and the slider
    /// position, moved into the range. A first bound below 0 becomes 0, a
    /// last bound below 0 or the first bound the higher of 0 and the first
    /// (EDM2 `SBM_SETSCROLLBAR`).
    pub fn set_range(&mut self, pos: i16, first: i16, last: i16) {
        self.first = first.max(0);
        self.last = last.max(self.first);
        self.set_pos(pos);
    }

    /// SBM_SETPOS: a position outside the range moves to its nearest end.
    pub fn set_pos(&mut self, pos: i16) {
        self.pos = pos.clamp(self.first, self.last);
    }

    /// Whether the scroll bar can be used: it has a range, and the visible
    /// part (`SBM_SETTHUMBSIZE`) is less than the whole.
    pub fn enabled(&self) -> bool {
        self.last > self.first && self.thumb_size.is_none_or(|(v, t)| v < t)
    }
}

/// Where the parts of a scroll bar `len` pels long and `breadth` broad lie,
/// along its axis from the first end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub len: i32,
    /// Length of each arrow button.
    pub arrow: i32,
    /// Slider length (0: no slider, the bar is disabled).
    pub thumb: i32,
    /// Slider start, from the first end.
    pub thumb_at: i32,
}

impl Geometry {
    /// The layout of `s` drawn `len` × `breadth`, the slider at `pos`.
    pub fn new(s: &ScrollBarState, len: i32, breadth: i32, pos: i16) -> Geometry {
        let len = len.max(0);
        let arrow = breadth.max(0).min(len / 2);
        let shaft = len - 2 * arrow;
        let thumb = if !s.enabled() {
            0
        } else if let Some((visible, total)) = s.thumb_size {
            (shaft as i64 * visible as i64 / (total as i64).max(1)) as i32
        } else {
            breadth
        }.max(if s.enabled() { MIN_THUMB_LEN } else { 0 }).min(shaft);
        let span = (s.last as i32 - s.first as i32).max(1);
        let at = (pos.clamp(s.first, s.last) as i32 - s.first as i32) as i64;
        let thumb_at = arrow + ((shaft - thumb) as i64 * at / span as i64) as i32;
        Geometry { len, arrow, thumb, thumb_at }
    }

    /// The part at `t` pels from the first end; `None` off the bar, and on
    /// the shaft of a bar without a slider.
    pub fn part_at(&self, t: i32) -> Option<Part> {
        if t < 0 || t >= self.len {
            None
        } else if t < self.arrow {
            Some(Part::LineUp)
        } else if t >= self.len - self.arrow {
            Some(Part::LineDown)
        } else if self.thumb == 0 {
            None
        } else if t < self.thumb_at {
            Some(Part::PageUp)
        } else if t < self.thumb_at + self.thumb {
            Some(Part::Thumb)
        } else {
            Some(Part::PageDown)
        }
    }

    /// The slider position for a slider starting at `start` (moved into
    /// the shaft), rounded to the nearest position of `s`'s range.
    pub fn pos_for(&self, s: &ScrollBarState, start: i32) -> i16 {
        let room = self.len - 2 * self.arrow - self.thumb;
        if room <= 0 {
            return s.first;
        }
        let off = (start - self.arrow).clamp(0, room) as i64;
        let span = s.last as i64 - s.first as i64;
        (s.first as i64 + (off * span + room as i64 / 2) / room as i64) as i16
    }
}

/// Whether a scroll bar of `style` is vertical.
pub fn is_vertical(style: u32) -> bool {
    style & SBS_VERT != 0
}

/// `(length, breadth)` of a `cx` × `cy` scroll bar of `style`.
fn axis_size(style: u32, cx: i32, cy: i32) -> (i32, i32) {
    if is_vertical(style) { (cy, cx) } else { (cx, cy) }
}

/// Distance of the pointer at `(x, y)` (relative to the bar's bottom-left)
/// from the first end of a scroll bar `cx` × `cy` of `style`.
fn along(style: u32, cy: i32, (x, y): (i32, i32)) -> i32 {
    if is_vertical(style) { cy - 1 - y } else { x }
}

/// Signed x / y of a mouse message's mp1.
fn mouse_pos(mp1: u32) -> (i32, i32) {
    ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32)
}

/// `MPFROM2SHORT(slider, command)`: mp2 of a scroll notification.
pub fn scroll_mp2(slider: i16, command: u32) -> u32 {
    (slider as u16 as u32) | (command << 16)
}

/// The notification message of a scroll bar of `style`.
pub fn scroll_msg(style: u32) -> u32 {
    if is_vertical(style) { WM_VSCROLL } else { WM_HSCROLL }
}

impl Loader {
    /// The scroll bar's style, size, state, and id, if `hwnd` still exists.
    fn scrollbar_info(&self, hwnd: u32) -> Option<(u32, i32, i32, ScrollBarState)> {
        let wm = self.shared.window_mgr.lock_or_recover();
        wm.get_window(hwnd).map(|w| (w.style, w.cx, w.cy, w.scrollbar.clone()))
    }

    fn update_scrollbar<R>(&self, hwnd: u32, f: impl FnOnce(&mut ScrollBarState) -> R) -> Option<R> {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        wm.get_window_mut(hwnd).map(|w| f(&mut w.scrollbar))
    }

    /// Post `WM_VSCROLL` / `WM_HSCROLL` with `command` and `slider` to the
    /// owner of scroll bar `hwnd` (its parent without one). A frame passes
    /// them to its client, as the frame window procedure does for its
    /// frame scroll bars (`FID_VERTSCROLL` / `FID_HORZSCROLL`).
    fn notify_scroll(&self, hwnd: u32, command: u32, slider: i16) {
        let (target, id, style) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window(hwnd) else { return };
            (wm.frame_msg_target(wm.notify_target(hwnd)), w.id, w.style)
        };
        if target == 0 || target == hwnd {
            return;
        }
        self.post_scroll_notification(target, scroll_msg(style), id & 0xFFFF, scroll_mp2(slider, command));
    }

    /// Queue a scroll notification for `target` in its message queue.
    fn post_scroll_notification(&self, target: u32, msg: u32, mp1: u32, mp2: u32) {
        let wm = self.shared.window_mgr.lock_or_recover();
        if let Some(mq_arc) = wm.find_hmq_for_hwnd(target).and_then(|hmq| wm.get_mq(hmq)) {
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(super::pm_types::OS2Message {
                hwnd: target, msg, mp1, mp2, time: 0, x: 0, y: 0,
            });
            mq.cond.notify_one();
        }
    }

    /// The `SBM_*` messages. Returns `None` for other messages.
    pub(super) fn scrollbar_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let short = |v: u32| v as u16 as i16;
        let result = match msg {
            // mp1 = slider position, mp2 = MPFROM2SHORT(first, last).
            SBM_SETSCROLLBAR => self.update_scrollbar(hwnd, |s| {
                s.set_range(short(mp1), short(mp2), short(mp2 >> 16));
            }).is_some() as u32,
            SBM_SETPOS => self.update_scrollbar(hwnd, |s| s.set_pos(short(mp1))).is_some() as u32,
            SBM_QUERYPOS => return Some(ApiResult::Normal(
                self.scrollbar_info(hwnd).map_or(0, |(_, _, _, s)| s.pos as u16 as u32))),
            // MRFROM2SHORT(first, last).
            SBM_QUERYRANGE => return Some(ApiResult::Normal(
                self.scrollbar_info(hwnd).map_or(0, |(_, _, _, s)| {
                    (s.first as u16 as u32) | ((s.last as u16 as u32) << 16)
                }))),
            // mp1 = MPFROM2SHORT(visible, total).
            SBM_SETTHUMBSIZE => self.update_scrollbar(hwnd, |s| {
                s.thumb_size = Some((mp1 as u16, (mp1 >> 16) as u16));
            }).is_some() as u32,
            _ => return None,
        };
        if result != 0 {
            self.repaint_control(hwnd);
        }
        Some(ApiResult::Normal(result))
    }

    /// Initial range, position and thumb size of a scroll bar created with
    /// `SBCDATA` control data at `pctl` (`cb` bytes long).
    pub(super) fn scrollbar_init(&self, hwnd: u32, pctl: u32) {
        if pctl == 0 {
            return;
        }
        let read = |off: u32| self.guest_read::<u16>(pctl.wrapping_add(off)).map(|v| v as i16);
        let Some(cb) = self.guest_read::<u16>(pctl) else { return };
        let (Some(first), Some(last), Some(pos)) =
            (read(SBCDATA_POS_FIRST), read(SBCDATA_POS_LAST), read(SBCDATA_POS_THUMB)) else { return };
        let size = if cb as u32 >= SBCDATA_SIZE {
            // A total of 0 sets no thumb size (TODO(unverified): the
            // references do not say what zeroed SBCDATA sizes mean).
            read(SBCDATA_C_VISIBLE).zip(read(SBCDATA_C_TOTAL))
                .filter(|&(_, t)| t != 0)
                .map(|(v, t)| (v as u16, t as u16))
        } else {
            None
        };
        self.update_scrollbar(hwnd, |s| {
            s.set_range(pos, first, last);
            if size.is_some() { s.thumb_size = size; }
        });
    }

    /// WM_BUTTON1DOWN on a scroll bar (`mp1` = pointer relative to it): an
    /// arrow or the shaft sends its command at once; the slider starts a
    /// drag. The scroll bar holds the pointer until the button comes up.
    pub(super) fn scrollbar_button_down(&self, hwnd: u32, mp1: u32) -> ApiResult {
        let Some((style, cx, cy, s)) = self.scrollbar_info(hwnd) else { return ApiResult::Normal(0) };
        let (len, breadth) = axis_size(style, cx, cy);
        let g = Geometry::new(&s, len, breadth, s.pos);
        let t = along(style, cy, mouse_pos(mp1));
        let Some(part) = g.part_at(t) else { return ApiResult::Normal(1) };
        if !s.enabled() {
            // A disabled bar does nothing, not even its arrows.
            return ApiResult::Normal(1);
        }
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            wm.pressed_control = hwnd;
            wm.drag_pointer = mouse_pos(mp1);
            if let Some(w) = wm.get_window_mut(hwnd) {
                w.scrollbar.held = Some(Held { part, grab: t - g.thumb_at, track: s.pos, over: true });
            }
        }
        self.repaint_control(hwnd);
        if let Some(cmd) = part.command() {
            self.notify_scroll(hwnd, cmd, 0);
        }
        ApiResult::Normal(1)
    }

    /// WM_MOUSEMOVE on a held scroll bar: a dragged slider follows the
    /// pointer, telling the owner `SB_SLIDERTRACK` whenever its position
    /// changes; a held arrow shows whether the pointer is still on it.
    pub(super) fn scrollbar_mouse_move(&self, hwnd: u32, mp1: u32) -> ApiResult {
        let Some((style, cx, cy, s)) = self.scrollbar_info(hwnd) else { return ApiResult::Normal(0) };
        let Some(held) = s.held else { return ApiResult::Normal(1) };
        self.shared.window_mgr.lock_or_recover().drag_pointer = mouse_pos(mp1);
        let (len, breadth) = axis_size(style, cx, cy);
        let g = Geometry::new(&s, len, breadth, s.pos);
        let t = along(style, cy, mouse_pos(mp1));
        if held.part == Part::Thumb {
            let track = g.pos_for(&s, t - held.grab);
            if track != held.track {
                self.update_scrollbar(hwnd, |s| if let Some(h) = s.held.as_mut() { h.track = track });
                self.repaint_control(hwnd);
                self.notify_scroll(hwnd, SB_SLIDERTRACK, track);
            }
        } else {
            let over = g.part_at(t) == Some(held.part);
            if over != held.over {
                self.update_scrollbar(hwnd, |s| if let Some(h) = s.held.as_mut() { h.over = over });
                self.repaint_control(hwnd);
            }
        }
        ApiResult::Normal(1)
    }

    /// WM_BUTTON1UP on a held scroll bar: a slider drag ends with
    /// `SB_SLIDERPOSITION` (the position reached), line and page scrolling
    /// with `SB_ENDSCROLL`.
    pub(super) fn scrollbar_button_up(&self, hwnd: u32) -> ApiResult {
        let held = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if wm.pressed_control == hwnd {
                wm.pressed_control = 0;
            }
            wm.get_window_mut(hwnd).and_then(|w| w.scrollbar.held.take())
        };
        let Some(held) = held else { return ApiResult::Normal(1) };
        self.repaint_control(hwnd);
        if held.part == Part::Thumb {
            self.notify_scroll(hwnd, SB_SLIDERPOSITION, held.track);
        } else {
            self.notify_scroll(hwnd, SB_ENDSCROLL, 0);
        }
        ApiResult::Normal(1)
    }

    /// WM_CHAR on a focused scroll bar: the arrows along its axis and Page
    /// Up / Page Down send their command, their release `SB_ENDSCROLL`.
    /// Other keys, and every key on a disabled bar, go to the owner.
    pub(super) fn scrollbar_char(&self, hwnd: u32, mp1: u32, mp2: u32) -> ApiResult {
        let Some((style, _, _, s)) = self.scrollbar_info(hwnd) else { return ApiResult::Normal(0) };
        let fs = mp1 & 0xFFFF;
        let vk = if fs & KC_VIRTUALKEY != 0 { mp2 >> WM_CHAR_VKEY_SHIFT } else { 0 };
        let command = key_command(is_vertical(style), vk)
            .filter(|_| fs & (KC_CTRL | KC_ALT) == 0 && s.enabled() && style & WS_DISABLED == 0);
        let Some(command) = command else {
            let owner = self.shared.window_mgr.lock_or_recover().notify_target(hwnd);
            if owner != 0 && owner != hwnd {
                self.post_control_message(owner, WM_CHAR, mp1, mp2);
            }
            return ApiResult::Normal(0);
        };
        let command = if fs & KC_KEYUP != 0 { SB_ENDSCROLL } else { command };
        self.notify_scroll(hwnd, command, 0);
        ApiResult::Normal(1)
    }

    /// WM_TIMER(TID_SCROLL) on a held scroll bar: the held arrow or shaft
    /// repeats its command while the pointer is on it (a shaft stops when
    /// the slider has reached the pointer).
    pub(super) fn scrollbar_repeat(&self, hwnd: u32) -> ApiResult {
        let Some((style, cx, cy, s)) = self.scrollbar_info(hwnd) else { return ApiResult::Normal(0) };
        let pointer = {
            let wm = self.shared.window_mgr.lock_or_recover();
            if wm.pressed_control != hwnd { return ApiResult::Normal(0); }
            wm.drag_pointer
        };
        let Some(held) = s.held else { return ApiResult::Normal(0) };
        let Some(cmd) = held.part.command() else { return ApiResult::Normal(0) };
        let (len, breadth) = axis_size(style, cx, cy);
        let g = Geometry::new(&s, len, breadth, s.pos);
        if g.part_at(along(style, cy, pointer)) == Some(held.part) {
            self.notify_scroll(hwnd, cmd, 0);
        }
        ApiResult::Normal(0)
    }

    /// WC_SCROLLBAR: the shaft, an arrow button at each end (drawn pressed
    /// while held under the pointer) and the slider at the position. While
    /// the slider is dragged, an `SBS_AUTOTRACK` bar draws it where the drag
    /// has taken it; another bar keeps it and draws an outline there. A
    /// disabled bar has grey arrows and no slider.
    pub(super) fn paint_scrollbar(&self, p: &Painter, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32)) {
        let Some((style, _, _, s)) = self.scrollbar_info(hwnd) else { return };
        let vertical = is_vertical(style);
        let (len, breadth) = axis_size(style, cx, cy);
        let track = match s.held {
            Some(Held { part: Part::Thumb, track, .. }) => Some(track),
            _ => None,
        };
        let autotrack = style & SBS_AUTOTRACK != 0;
        let pos = if autotrack { track.unwrap_or(s.pos) } else { s.pos };
        let g = Geometry::new(&s, len, breadth, pos);
        // The span `t0..t1` along the axis as a PM rectangle (x, y, w, h).
        let span = |t0: i32, t1: i32| if vertical {
            (x, y + cy - t1, cx, t1 - t0)
        } else {
            (x + t0, y, t1 - t0, cy)
        };
        // A raised button over `(rx, ry, w, h)`; `control_box` corners are
        // inclusive.
        let button = |(rx, ry, w, h): (i32, i32, i32, i32), pressed: bool| {
            if w > 0 && h > 0 {
                control_box(p, (rx, ry, w - 1, h - 1),
                            if pressed { CONTROL_PRESSED_COLOR } else { CONTROL_FACE_COLOR });
            }
        };
        // Modern (ADR-0118): a light track, flat arrows (a box only while
        // pressed) and a slimmer rounded thumb without an outline.
        let modern = !theme::current().bevelled();
        let shaft = if modern { theme::MODERN_TRACK_COLOR } else { CONTROL_SCROLLBAR_COLOR };
        p.fill((x, y, x + cx - 1, y + cy - 1), shaft);
        let pressed = |part: Part| s.held.is_some_and(|h| h.part == part && h.over);
        let glyph = if s.enabled() { CONTROL_TEXT_COLOR } else { DISABLED_ARROW_COLOR };
        for (part, t0) in [(Part::LineUp, 0), (Part::LineDown, len - g.arrow)] {
            let r = span(t0, t0 + g.arrow);
            if !modern || pressed(part) {
                button(r, pressed(part));
            }
            draw_arrow(p, r, vertical, part == Part::LineUp, glyph);
        }
        if g.thumb > 0 && modern {
            let (rx, ry, w, h) = span(g.thumb_at, g.thumb_at + g.thumb);
            let i = theme::MODERN_THUMB_INSET;
            let (ix, iy) = if vertical { (i, 0) } else { (0, i) };
            let color = if track.is_some() { theme::MODERN_THUMB_HELD_COLOR } else { theme::MODERN_THUMB_COLOR };
            if w - 2 * ix > 0 && h - 2 * iy > 0 {
                super::rounded_box(p, (rx + ix, ry + iy, w - 2 * ix - 1, h - 2 * iy - 1), Some(color), color);
            }
        } else if g.thumb > 0 {
            button(span(g.thumb_at, g.thumb_at + g.thumb), false);
        }
        // The tracking outline of a slider dragged without SBS_AUTOTRACK.
        if let Some(track) = track.filter(|_| !autotrack && g.thumb > 0) {
            let at = Geometry::new(&s, len, breadth, track).thumb_at;
            let (rx, ry, w, h) = span(at, at + g.thumb);
            let (r, t) = (rx + w - 1, ry + h - 1);
            for edge in [(rx, ry, r, ry), (rx, t, r, t), (rx, ry, rx, t), (r, ry, r, t)] {
                p.fill(edge, CONTROL_TEXT_COLOR);
            }
        }
    }
}

/// The `SB_*` command key `vk` sends on a vertical or horizontal bar.
pub fn key_command(vertical: bool, vk: u32) -> Option<u32> {
    match (vk, vertical) {
        (VK_UP, true) | (VK_LEFT, false) => Some(SB_LINEUP),
        (VK_DOWN, true) | (VK_RIGHT, false) => Some(SB_LINEDOWN),
        (VK_PAGEUP, _) => Some(SB_PAGEUP),
        (VK_PAGEDOWN, _) => Some(SB_PAGEDOWN),
        _ => None,
    }
}

/// A filled triangle in the button `(x, y, w, h)` pointing to the bar's
/// first end (`first`: up / left) or its last end.
pub(super) fn draw_arrow(p: &Painter, (x, y, w, h): (i32, i32, i32, i32), vertical: bool, first: bool, color: u32) {
    // As many rows as half its base: 1, 3, 5, ... pels wide.
    let height = ((w.min(h) - 2 * ARROW_GLYPH_INSET) / 2).max(1);
    let (mx, my) = (x + (w - 1) / 2, y + (h - 1) / 2);
    for i in 0..height {
        // Row `i` from the tip: 2i+1 pels wide.
        if vertical {
            let row = if first { my + height / 2 - i } else { my - height / 2 + i };
            p.fill((mx - i, row, mx + i, row), color);
        } else {
            let col = if first { mx - height / 2 + i } else { mx + height / 2 - i };
            p.fill((col, my - i, col, my + i), color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_types::OS2Message;

    fn state(pos: i16, first: i16, last: i16) -> ScrollBarState {
        let mut s = ScrollBarState::default();
        s.set_range(pos, first, last);
        s
    }

    #[test]
    fn the_range_and_position_are_kept_valid() {
        let s = state(150, 0, 100);
        assert_eq!((s.pos, s.first, s.last), (100, 0, 100), "a position past the end moves to it");
        let s = state(-5, -10, 20);
        assert_eq!((s.pos, s.first, s.last), (0, 0, 20), "a negative first bound becomes 0");
        let s = state(7, 30, 10);
        assert_eq!((s.pos, s.first, s.last), (30, 30, 30), "a last bound below the first takes it");
        let mut s = state(0, 0, 50);
        s.set_pos(25);
        assert_eq!(s.pos, 25);
        s.set_pos(-1);
        assert_eq!(s.pos, 0);
        assert!(s.enabled());
        assert!(!state(0, 5, 5).enabled(), "no range");
        s.thumb_size = Some((10, 10));
        assert!(!s.enabled(), "everything visible");
        s.thumb_size = Some((9, 10));
        assert!(s.enabled());
    }

    #[test]
    fn geometry_places_arrows_and_a_proportional_slider() {
        let s = state(0, 0, 100);
        let g = Geometry::new(&s, 200, 16, 0);
        assert_eq!((g.arrow, g.thumb, g.thumb_at), (16, 16, 16), "square slider at the top");
        assert_eq!(Geometry::new(&s, 200, 16, 100).thumb_at, 200 - 16 - 16, "at the bottom");
        assert_eq!(Geometry::new(&s, 200, 16, 50).thumb_at, 16 + (168 - 16) / 2);
        let mut half = s.clone();
        half.thumb_size = Some((50, 100));
        assert_eq!(Geometry::new(&half, 200, 16, 0).thumb, 84, "half the shaft");
        half.thumb_size = Some((1, 1000));
        assert_eq!(Geometry::new(&half, 200, 16, 0).thumb, MIN_THUMB_LEN);
        let short = Geometry::new(&s, 20, 16, 0);
        assert_eq!((short.arrow, short.thumb), (10, 0), "arrows share a short bar, no room for a slider");
        assert_eq!(Geometry::new(&state(0, 0, 0), 200, 16, 0).thumb, 0, "disabled: no slider");
    }

    #[test]
    fn parts_along_the_bar() {
        let s = state(50, 0, 100);
        let g = Geometry::new(&s, 200, 16, 50);
        assert_eq!(g.part_at(-1), None);
        assert_eq!(g.part_at(0), Some(Part::LineUp));
        assert_eq!(g.part_at(15), Some(Part::LineUp));
        assert_eq!(g.part_at(16), Some(Part::PageUp));
        assert_eq!(g.part_at(g.thumb_at), Some(Part::Thumb));
        assert_eq!(g.part_at(g.thumb_at + g.thumb), Some(Part::PageDown));
        assert_eq!(g.part_at(184), Some(Part::LineDown));
        assert_eq!(g.part_at(199), Some(Part::LineDown));
        assert_eq!(g.part_at(200), None);
        assert_eq!(Part::LineUp.command(), Some(SB_LINEUP));
        assert_eq!(Part::PageDown.command(), Some(SB_PAGEDOWN));
        assert_eq!(Part::Thumb.command(), None);
    }

    #[test]
    fn slider_positions_round_to_the_range() {
        let s = state(0, 10, 20);
        let g = Geometry::new(&s, 200, 16, 10);
        let room = 200 - 32 - 16;
        assert_eq!(g.pos_for(&s, 16), 10);
        assert_eq!(g.pos_for(&s, 16 + room), 20);
        assert_eq!(g.pos_for(&s, 16 + room / 2), 15);
        assert_eq!(g.pos_for(&s, -100), 10, "dragged past the top");
        assert_eq!(g.pos_for(&s, 1000), 20, "dragged past the bottom");
        // Every position maps back to itself.
        for pos in 10..=20 {
            assert_eq!(g.pos_for(&s, Geometry::new(&s, 200, 16, pos).thumb_at), pos);
        }
    }

    #[test]
    fn notification_layout() {
        assert_eq!(scroll_mp2(-2, SB_SLIDERTRACK), 0x0005_FFFE);
        assert_eq!(scroll_mp2(0, SB_ENDSCROLL), 7 << 16);
        assert_eq!(scroll_msg(SBS_VERT), WM_VSCROLL);
        assert_eq!(scroll_msg(SBS_HORZ), WM_HSCROLL);
    }

    struct Fixture {
        loader: Loader,
        hmq: u32,
        client: u32,
        vbar: u32,
        hbar: u32,
    }

    const VBAR_ID: u32 = 21;
    const HBAR_ID: u32 = 22;

    /// A client with a 16 × 200 vertical bar and a 200 × 16 horizontal
    /// bar, both with range 0..100.
    fn fixture() -> Fixture {
        let loader = Loader::new_mock();
        let (hmq, client, vbar, hbar) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            if let Some(w) = wm.get_window_mut(frame) { (w.cx, w.cy) = (400, 300); }
            if let Some(w) = wm.get_window_mut(client) { (w.cx, w.cy) = (400, 280); }
            let mut bar = |style: u32, id: u32, (x, y, cx, cy): (i32, i32, i32, i32)| {
                let h = wm.create_window(CLASS_SCROLLBAR.into(), client, hmq);
                let w = wm.get_window_mut(h).unwrap();
                (w.x, w.y, w.cx, w.cy) = (x, y, cx, cy);
                w.style = WS_VISIBLE | style;
                w.visible = true;
                w.id = id;
                w.owner = client;
                h
            };
            let vbar = bar(SBS_VERT, VBAR_ID, (300, 50, 16, 200));
            let hbar = bar(SBS_HORZ, HBAR_ID, (50, 10, 200, 16));
            (hmq, client, vbar, hbar)
        };
        let f = Fixture { loader, hmq, client, vbar, hbar };
        for bar in [f.vbar, f.hbar] {
            f.send(bar, SBM_SETSCROLLBAR, 0, 100 << 16);
        }
        f.drain();
        f
    }

    impl Fixture {
        fn send(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(hwnd, msg, mp1, mp2) {
                ApiResult::Normal(v) => v,
                _ => panic!("unexpected result"),
            }
        }
        fn drain(&self) -> Vec<OS2Message> {
            let wm = self.loader.shared.window_mgr.lock_or_recover();
            let mq = wm.get_mq(self.hmq).unwrap();
            let mut mq = mq.lock_or_recover();
            mq.messages.drain(..).collect()
        }
        /// The scroll notifications queued since the last call, as
        /// (msg, mp1, slider, command).
        fn scrolls(&self) -> Vec<(u32, u32, i16, u32)> {
            self.drain().into_iter()
                .filter(|m| m.msg == WM_VSCROLL || m.msg == WM_HSCROLL)
                .inspect(|m| assert_eq!(m.hwnd, self.client))
                .map(|m| (m.msg, m.mp1, m.mp2 as u16 as i16, m.mp2 >> 16))
                .collect()
        }
        fn at(x: i32, y: i32) -> u32 {
            (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16)
        }
    }

    #[test]
    fn sbm_messages_set_and_query_range_position_and_thumb_size() {
        let f = fixture();
        assert_eq!(f.send(f.vbar, SBM_SETSCROLLBAR, 30, (200 << 16) | 10), 1);
        assert_eq!(f.send(f.vbar, SBM_QUERYPOS, 0, 0), 30);
        assert_eq!(f.send(f.vbar, SBM_QUERYRANGE, 0, 0), (200 << 16) | 10);
        assert_eq!(f.send(f.vbar, SBM_SETPOS, 500, 0), 1);
        assert_eq!(f.send(f.vbar, SBM_QUERYPOS, 0, 0), 200, "clamped");
        assert_eq!(f.send(f.vbar, SBM_SETPOS, 5, 0), 1);
        assert_eq!(f.send(f.vbar, SBM_QUERYPOS, 0, 0), 10);
        assert_eq!(f.send(f.vbar, SBM_SETTHUMBSIZE, (100 << 16) | 25, 0), 1);
        let wm = f.loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.get_window(f.vbar).unwrap().scrollbar.thumb_size, Some((25, 100)));
    }

    #[test]
    fn arrows_and_shaft_send_line_and_page_commands_and_end_scroll() {
        let f = fixture();
        f.send(f.vbar, SBM_SETPOS, 50, 0);
        f.drain();
        let v = (WM_VSCROLL, VBAR_ID);
        // Top arrow (pointer y counts from the bottom).
        f.send(f.vbar, WM_BUTTON1DOWN, Fixture::at(8, 195), 0);
        assert_eq!(f.scrolls(), vec![(v.0, v.1, 0, SB_LINEUP)]);
        f.send(f.vbar, WM_BUTTON1UP, Fixture::at(8, 195), 0);
        assert_eq!(f.scrolls(), vec![(v.0, v.1, 0, SB_ENDSCROLL)]);
        // Bottom arrow, shaft above and below the slider.
        for (y, cmd) in [(3, SB_LINEDOWN), (170, SB_PAGEUP), (30, SB_PAGEDOWN)] {
            f.send(f.vbar, WM_BUTTON1DOWN, Fixture::at(8, y), 0);
            f.send(f.vbar, WM_BUTTON1UP, Fixture::at(8, y), 0);
            assert_eq!(f.scrolls(), vec![(v.0, v.1, 0, cmd), (v.0, v.1, 0, SB_ENDSCROLL)], "y {y}");
        }
        // The slider does not move by itself: the owner sets it.
        assert_eq!(f.send(f.vbar, SBM_QUERYPOS, 0, 0), 50);
        // Horizontal: the left arrow and the right shaft.
        f.send(f.hbar, WM_BUTTON1DOWN, Fixture::at(2, 8), 0);
        f.send(f.hbar, WM_BUTTON1UP, Fixture::at(2, 8), 0);
        f.send(f.hbar, WM_BUTTON1DOWN, Fixture::at(150, 8), 0);
        f.send(f.hbar, WM_BUTTON1UP, Fixture::at(150, 8), 0);
        let h = (WM_HSCROLL, HBAR_ID);
        assert_eq!(f.scrolls(), vec![(h.0, h.1, 0, SB_LINELEFT), (h.0, h.1, 0, SB_ENDSCROLL),
                                     (h.0, h.1, 0, SB_PAGERIGHT), (h.0, h.1, 0, SB_ENDSCROLL)]);
    }

    #[test]
    fn dragging_the_slider_tracks_and_ends_with_its_position() {
        let f = fixture();
        // The slider of a 0..100 bar at 0 sits right below the top arrow:
        // along 16..32, pointer y 183..168.
        f.send(f.vbar, WM_BUTTON1DOWN, Fixture::at(8, 176), 0);
        assert_eq!(f.scrolls(), vec![], "pressing the slider sends nothing");
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().pressed_control, f.vbar);
        // Half-way down the room of 152 pels: position 50.
        f.send(f.vbar, WM_MOUSEMOVE, Fixture::at(8, 176 - 76), 0);
        assert_eq!(f.scrolls(), vec![(WM_VSCROLL, VBAR_ID, 50, SB_SLIDERTRACK)]);
        f.send(f.vbar, WM_MOUSEMOVE, Fixture::at(20, 176 - 76), 0);
        assert_eq!(f.scrolls(), vec![], "no change, no notification");
        // Far below the bar: the end of the range.
        f.send(f.vbar, WM_MOUSEMOVE, Fixture::at(8, -500), 0);
        assert_eq!(f.scrolls(), vec![(WM_VSCROLL, VBAR_ID, 100, SB_SLIDERTRACK)]);
        f.send(f.vbar, WM_BUTTON1UP, Fixture::at(8, -500), 0);
        assert_eq!(f.scrolls(), vec![(WM_VSCROLL, VBAR_ID, 100, SB_SLIDERPOSITION)],
                   "no SB_ENDSCROLL after a slider drag");
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().pressed_control, 0);
        assert_eq!(f.send(f.vbar, SBM_QUERYPOS, 0, 0), 0, "the owner sets the position");
    }

    #[test]
    fn a_held_arrow_repeats_while_the_pointer_is_on_it() {
        let f = fixture();
        f.send(f.vbar, WM_BUTTON1DOWN, Fixture::at(8, 3), 0);
        f.drain();
        {
            let wm = f.loader.shared.window_mgr.lock_or_recover();
            assert_eq!(wm.post_drag_scroll(), Some(f.vbar), "the timer runs while it is held");
        }
        f.drain();
        f.send(f.vbar, WM_TIMER, TID_SCROLL, 0);
        assert_eq!(f.scrolls(), vec![(WM_VSCROLL, VBAR_ID, 0, SB_LINEDOWN)]);
        // Off the arrow: no repeat, and the arrow pops up.
        f.send(f.vbar, WM_MOUSEMOVE, Fixture::at(8, 100), 0);
        assert!(!f.loader.shared.window_mgr.lock_or_recover().get_window(f.vbar).unwrap()
            .scrollbar.held.unwrap().over);
        f.send(f.vbar, WM_TIMER, TID_SCROLL, 0);
        assert_eq!(f.scrolls(), vec![]);
        f.send(f.vbar, WM_BUTTON1UP, Fixture::at(8, 100), 0);
        assert_eq!(f.scrolls(), vec![(WM_VSCROLL, VBAR_ID, 0, SB_ENDSCROLL)]);
        f.send(f.vbar, WM_TIMER, TID_SCROLL, 0);
        assert_eq!(f.scrolls(), vec![], "released: no repeat");
    }

    #[test]
    fn a_disabled_scroll_bar_ignores_the_pointer() {
        let f = fixture();
        f.send(f.vbar, SBM_SETTHUMBSIZE, (100 << 16) | 100, 0);
        f.send(f.vbar, WM_BUTTON1DOWN, Fixture::at(8, 3), 0);
        f.send(f.vbar, WM_BUTTON1UP, Fixture::at(8, 3), 0);
        assert_eq!(f.scrolls(), vec![]);
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().pressed_control, 0);
    }

    #[test]
    fn painting_draws_arrows_and_the_slider_where_it_is_dragged() {
        let f = fixture();
        let frame = f.loader.shared.window_mgr.lock_or_recover().find_frame_for_hwnd(f.vbar);
        let pel = |x: i32, y: i32| {
            let wm = f.loader.shared.window_mgr.lock_or_recover();
            let s = wm.surfaces.get(&frame).unwrap().clone();
            drop(wm);
            let s = s.lock_or_recover();
            s.buffer[((s.height as i32 - 1 - y) * s.width as i32 + x) as usize]
        };
        f.loader.shared.window_mgr.lock_or_recover().resize_surface(frame, 400, 280);
        if let Some(w) = f.loader.shared.window_mgr.lock_or_recover().get_window_mut(f.vbar) { w.style |= SBS_AUTOTRACK; }
        f.send(f.vbar, WM_PAINT, 0, 0);
        // Bar at (300, 50) 16 x 200: slider right below the top arrow.
        assert_eq!(pel(300 + 8, 50 + 200 - 24), CONTROL_FACE_COLOR, "slider face");
        assert_eq!(pel(300 + 8, 50 + 100), CONTROL_SCROLLBAR_COLOR, "shaft");
        f.send(f.vbar, WM_BUTTON1DOWN, Fixture::at(8, 176), 0);
        f.send(f.vbar, WM_MOUSEMOVE, Fixture::at(8, 100), 0);
        assert_eq!(pel(300 + 8, 50 + 200 - 24), CONTROL_SCROLLBAR_COLOR, "the slider left");
        assert_eq!(pel(300 + 8, 50 + 100), CONTROL_FACE_COLOR, "and follows the pointer");
        // A held arrow is drawn pressed.
        f.send(f.vbar, WM_BUTTON1UP, Fixture::at(8, 100), 0);
        f.send(f.vbar, WM_BUTTON1DOWN, Fixture::at(8, 3), 0);
        assert_eq!(pel(300 + 2, 50 + 2), CONTROL_PRESSED_COLOR);
    }

    #[test]
    fn without_autotrack_an_outline_follows_the_pointer() {
        let f = fixture();
        let frame = f.loader.shared.window_mgr.lock_or_recover().find_frame_for_hwnd(f.vbar);
        let pel = |x: i32, y: i32| {
            let wm = f.loader.shared.window_mgr.lock_or_recover();
            let s = wm.surfaces.get(&frame).unwrap().clone();
            drop(wm);
            let s = s.lock_or_recover();
            s.buffer[((s.height as i32 - 1 - y) * s.width as i32 + x) as usize]
        };
        f.loader.shared.window_mgr.lock_or_recover().resize_surface(frame, 400, 280);
        f.send(f.vbar, WM_BUTTON1DOWN, Fixture::at(8, 176), 0);
        f.send(f.vbar, WM_MOUSEMOVE, Fixture::at(8, 100), 0);
        assert_eq!(f.scrolls().last().map(|s| s.3), Some(SB_SLIDERTRACK), "the owner still hears the track");
        assert_eq!(pel(300 + 8, 50 + 200 - 24), CONTROL_FACE_COLOR, "the slider stays");
        assert_eq!(pel(300, 50 + 100), CONTROL_TEXT_COLOR, "an outline at the pointer");
        assert_eq!(pel(300 + 8, 50 + 100), CONTROL_SCROLLBAR_COLOR, "the outline is hollow");
        f.send(f.vbar, WM_BUTTON1UP, Fixture::at(8, 100), 0);
        assert_eq!(pel(300, 50 + 100), CONTROL_SCROLLBAR_COLOR, "the outline goes on release");
    }

    #[test]
    fn a_focused_scroll_bar_scrolls_with_the_keyboard() {
        let f = fixture();
        let key = |bar: u32, vk: u32, up: bool| {
            let fs = KC_VIRTUALKEY | if up { KC_KEYUP } else { 0 };
            f.send(bar, WM_CHAR, fs, vk << WM_CHAR_VKEY_SHIFT)
        };
        assert_eq!(key(f.vbar, VK_DOWN, false), 1);
        key(f.vbar, VK_UP, false);
        key(f.vbar, VK_PAGEDOWN, false);
        key(f.vbar, VK_PAGEDOWN, true);
        assert_eq!(f.scrolls(), vec![(WM_VSCROLL, VBAR_ID, 0, SB_LINEDOWN), (WM_VSCROLL, VBAR_ID, 0, SB_LINEUP),
                                     (WM_VSCROLL, VBAR_ID, 0, SB_PAGEDOWN), (WM_VSCROLL, VBAR_ID, 0, SB_ENDSCROLL)]);
        key(f.hbar, VK_RIGHT, false);
        key(f.hbar, VK_LEFT, false);
        assert_eq!(f.scrolls(), vec![(WM_HSCROLL, HBAR_ID, 0, SB_LINERIGHT), (WM_HSCROLL, HBAR_ID, 0, SB_LINELEFT)]);
        // Keys across the axis, and others, go to the owner.
        assert_eq!(key(f.vbar, VK_LEFT, false), 0);
        let fwd: Vec<_> = f.drain().into_iter().map(|m| (m.hwnd, m.msg)).collect();
        assert_eq!(fwd, vec![(f.client, WM_CHAR)]);
        // A disabled bar scrolls nothing.
        f.send(f.vbar, SBM_SETTHUMBSIZE, (100 << 16) | 100, 0);
        key(f.vbar, VK_DOWN, false);
        assert_eq!(f.scrolls(), vec![]);
    }

    #[test]
    fn key_commands_follow_the_axis() {
        assert_eq!(key_command(true, VK_UP), Some(SB_LINEUP));
        assert_eq!(key_command(false, VK_UP), None);
        assert_eq!(key_command(false, VK_PAGEUP), Some(SB_PAGELEFT));
        assert_eq!(key_command(true, VK_HOME), None);
    }
}
