// SPDX-License-Identifier: GPL-3.0-only

//! Interactive behaviour of the built-in text controls: painting with caret,
//! selection highlight and scrolling; mouse and keyboard input; keyboard
//! focus; and the WM_CONTROL notifications sent to the owner.
//!
//! The editing arithmetic lives in [`super::pm_edit`]; this module applies it
//! to `OS2Window` state and draws the result. Messages reach these handlers
//! through `Loader::dispatch_builtin_control`.
//!
//! Coordinates: a control's rectangle `(x, y, cx, cy)` is in its frame's
//! client space with the origin at the bottom-left; mouse messages carry the
//! pointer position relative to the control's own bottom-left corner.

use super::constants::*;
use super::pm_edit::{self, EditBuffer, EditKey, CARET_W};
use super::pm_list::{ListMode, ListSelection};
use super::pm_types::OS2Message;
use super::{control_box, control_text, ApiResult, Loader, MutexExt, Painter,
            CONTROL_EMPHASIS_COLOR, CONTROL_FACE_COLOR, CONTROL_FIELD_COLOR, CONTROL_LINE_H,
            CONTROL_PRESSED_COLOR, CONTROL_TEXT_COLOR, CONTROL_TEXT_INSET};
use crate::gui::font::{self, FontSpec};
use crate::gui::surface::DrawOp;

pub(super) const CLASS_ENTRYFIELD: &str = "WC_ENTRYFIELD";
pub(super) const CLASS_MLE: &str = "WC_MLE";
pub(super) const CLASS_LISTBOX: &str = "WC_LISTBOX";

/// Colour of the caret bar.
const CONTROL_CARET_COLOR: u32 = 0x0000_0000;
/// Background of the selected list box row.
const CONTROL_SELECT_COLOR: u32 = 0x0000_0080;
/// Text colour on the selected list box row.
const CONTROL_SELECT_TEXT_COLOR: u32 = 0x00FF_FFFF;
/// Gap between a control's top border and its first text row.
const CONTROL_ROWS_TOP_INSET: i32 = 2;
/// Gap between a button's border and the focus mark drawn inside it.
const BUTTON_FOCUS_INSET: i32 = 3;
/// How far a held-down button's label moves right and down.
const BUTTON_PRESSED_SHIFT: i32 = 1;

/// WM_CHAR flags that give a key message a meaning beyond "a key moved":
/// a character, a virtual key, or a Ctrl/Alt chord.
const KEY_EVENT_KINDS: u32 = KC_CHAR | KC_VIRTUALKEY | KC_CTRL | KC_ALT;

/// The clipboard commands of the text controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClipboardCmd {
    Cut,
    Copy,
    Paste,
    Clear,
}

/// The CUA clipboard key a WM_CHAR of an entry field / MLE stands for:
/// Ctrl+Insert copies, Shift+Delete cuts, Shift+Insert pastes.
fn clipboard_key(class: &str, mp1: u32, mp2: u32) -> Option<ClipboardCmd> {
    if class == CLASS_LISTBOX || mp1 & KC_VIRTUALKEY == 0 {
        return None;
    }
    let mods = mp1 & (KC_SHIFT | KC_CTRL | KC_ALT);
    match (mp2 >> WM_CHAR_VKEY_SHIFT, mods) {
        (VK_INSERT, KC_CTRL) => Some(ClipboardCmd::Copy),
        (VK_DELETE, KC_SHIFT) => Some(ClipboardCmd::Cut),
        (VK_INSERT, KC_SHIFT) => Some(ClipboardCmd::Paste),
        _ => None,
    }
}

/// Controls that take keyboard focus and handle mouse and keyboard input here.
pub(super) fn is_input_control(class: &str) -> bool {
    matches!(class, CLASS_ENTRYFIELD | CLASS_MLE | CLASS_LISTBOX)
}

/// The text-editing controls: entry field and MLE.
pub(super) fn is_edit_control(class: &str) -> bool {
    matches!(class, CLASS_ENTRYFIELD | CLASS_MLE)
}

pub(super) fn system_width(s: &str) -> i32 {
    font::text_width(s, FontSpec::SYSTEM)
}

/// Number of whole text rows that fit in a control `cy` pixels high: rows run
/// from the top down and a row must end above the bottom border.
pub(super) fn control_visible_rows(cy: i32) -> usize {
    ((cy - CONTROL_ROWS_TOP_INSET - 1).max(0) / CONTROL_LINE_H) as usize
}

/// Bottom of visible row `row` (0 = top row), relative to the control's bottom.
pub(super) fn control_row_y(cy: i32, row: usize) -> i32 {
    cy - CONTROL_ROWS_TOP_INSET - CONTROL_LINE_H * (row as i32 + 1)
}

/// The visible row under `local_y` (pixels above the control's bottom edge),
/// or `None` in the margins above the first and below the last row.
pub(super) fn control_row_at(cy: i32, local_y: i32) -> Option<usize> {
    let top = cy - CONTROL_ROWS_TOP_INSET;
    if local_y < 0 || local_y >= top {
        return None;
    }
    let row = ((top - 1 - local_y) / CONTROL_LINE_H) as usize;
    (row < control_visible_rows(cy)).then_some(row)
}

/// First visible item of a list box: the stored scroll position, pulled back
/// so the view is never scrolled past the end of the list.
fn listbox_top(top_row: usize, count: usize, visible: usize) -> usize {
    top_row.min(count.saturating_sub(visible))
}

/// Signed x / y of a mouse message's mp1 (POINTS: two SHORTs).
fn mouse_pos(mp1: u32) -> (i32, i32) {
    ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32)
}

/// Draw one line of an edit control: `chars` clipped to `view_w` pixels with
/// its line box bottom-left at `(x, y)`, the characters `selected` (indices
/// into `chars`) highlighted, and the caret bar before `chars[caret]` when
/// `caret` is given. Highlight and caret are clipped vertically to
/// `(clip_lo, clip_hi)`.
fn draw_edit_line(p: &Painter, (x, y): (i32, i32), view_w: i32, chars: &[char],
                  selected: std::ops::Range<usize>, caret: Option<usize>,
                  (clip_lo, clip_hi): (i32, i32)) {
    let line: String = chars.iter().collect();
    let shown = font::fit_text(&line, view_w, p.font());
    if !shown.is_empty() {
        control_text(p, x, y, shown);
    }
    let prefix_w = |n: usize| system_width(&chars[..n.min(chars.len())].iter().collect::<String>());
    let (y1, y2) = (y.max(clip_lo), (y + CONTROL_LINE_H - 1).min(clip_hi));
    let shown_n = shown.chars().count();
    let (s, e) = (selected.start.min(shown_n), selected.end.min(shown_n));
    if s < e && y1 <= y2 {
        let (x1, x2) = (x + prefix_w(s), x + prefix_w(e));
        p.fill((x1, y1, x2 - 1, y2), CONTROL_SELECT_COLOR);
        p.op(DrawOp::Text { x: x1, y, text: chars[s..e].iter().collect(), color: CONTROL_SELECT_TEXT_COLOR, cjk: p.cjk });
    }
    if let Some(caret) = caret {
        let caret_x = x + prefix_w(caret);
        if y1 <= y2 {
            p.fill((caret_x, y1, caret_x + CARET_W - 1, y2), CONTROL_CARET_COLOR);
        }
    }
}

/// `range` shifted to start at `origin` and cut to what lies after it; empty
/// when it lies wholly before.
fn relative(range: std::ops::Range<usize>, origin: usize) -> std::ops::Range<usize> {
    range.start.saturating_sub(origin)..range.end.saturating_sub(origin)
}

/// The edit buffer of an entry field / MLE window, with its selection;
/// PageUp / PageDown move by the rows the window shows, and an MLE with
/// `MLS_WORDWRAP` wraps its lines to the width of its text area (less the
/// caret, so a caret after the last character of a full row stays inside).
fn edit_buffer(win: &super::pm_types::OS2Window, multiline: bool) -> EditBuffer {
    let mut buf = EditBuffer::with_selection(&win.text, win.edit_anchor, win.edit_caret, multiline);
    buf.set_page_rows(control_visible_rows(win.cy));
    if multiline && win.style & MLS_WORDWRAP != 0 {
        buf.set_wrap(win.cx - 2 * CONTROL_TEXT_INSET - CARET_W, system_width);
    }
    buf
}

/// Whether an entry field (`ES_READONLY`) / MLE (`MLS_READONLY`) is
/// read-only: its text can be selected and copied but not changed.
fn is_read_only(win: &super::pm_types::OS2Window) -> bool {
    let flag = if win.class_name == CLASS_MLE { MLS_READONLY } else { ES_READONLY };
    win.style & flag != 0
}

/// The read-only style bit of an edit control class.
pub(super) fn read_only_style(class: &str) -> u32 {
    if class == CLASS_MLE { MLS_READONLY } else { ES_READONLY }
}

/// Store the caret and selection of `buf` in its window.
fn store_selection(win: &mut super::pm_types::OS2Window, buf: &EditBuffer) {
    win.edit_caret = buf.caret();
    win.edit_anchor = buf.anchor();
}

/// The row index (counted from the first visible row `top`) under the
/// pointer at `local_y` of a control `cy` high: the margins inside the
/// control belong to the first / last visible row, and a pointer above or
/// below the control (while dragging) to the row just outside the view, so
/// that the view scrolls by a row per pointer move. `None` above the first
/// row of the content.
fn row_under_pointer(top: usize, cy: i32, local_y: i32) -> Option<usize> {
    let rows = control_visible_rows(cy);
    let row = match control_row_at(cy, local_y) {
        Some(row) => row as i64,
        None if local_y >= cy => -1,
        None if local_y < 0 => rows as i64,
        None if local_y >= control_row_y(cy, 0) => 0,
        None => rows as i64 - 1,
    };
    usize::try_from(top as i64 + row).ok()
}

/// The character index of an edit control under the pointer at
/// `(local_x, local_y)` (relative to the control, `cy` high): the row under
/// the pointer (see [`row_under_pointer`]; clamped to the text), then the
/// nearest caret position in it, allowing for the horizontal scroll of the
/// caret's line.
fn edit_pos_at(win: &super::pm_types::OS2Window, multiline: bool, cy: i32,
               (local_x, local_y): (i32, i32)) -> usize {
    let buf = edit_buffer(win, multiline);
    let lines = buf.lines();
    let (caret_line, _) = buf.caret_line();
    let line = if multiline {
        let top = win.top_row.min(lines.len() - 1);
        row_under_pointer(top, cy, local_y).unwrap_or(0).min(lines.len() - 1)
    } else {
        0
    };
    let chars = &buf.chars()[lines[line].clone()];
    // Only the caret's line is scrolled horizontally.
    let first = if line == caret_line { win.edit_scroll.min(chars.len()) } else { 0 };
    let col = first + pm_edit::caret_from_x(&chars[first..], local_x - CONTROL_TEXT_INSET, &system_width);
    buf.pos_in_line(line, col)
}

impl Loader {
    /// Queue `msg` for `hwnd` on its message queue.
    fn post_control_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) {
        let wm = self.shared.window_mgr.lock_or_recover();
        if let Some(mq_arc) = wm.find_hmq_for_hwnd(hwnd).and_then(|hmq| wm.get_mq(hmq)) {
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(OS2Message { hwnd, msg, mp1, mp2, time: 0, x: 0, y: 0 });
            mq.cond.notify_one();
        }
    }

    /// Post `WM_CONTROL` to the owner of control `hwnd`:
    /// mp1 = MPFROM2SHORT(control id, notify code), mp2 = `data`.
    fn notify_owner(&self, hwnd: u32, code: u32, data: u32) {
        self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, code, data);
    }

    /// Repaint built-in control `hwnd` into its frame's buffer and present it.
    /// Returns `false` (doing nothing) when `hwnd` is not a built-in `WC_*`
    /// control.
    pub(crate) fn repaint_control(&self, hwnd: u32) -> bool {
        let frame = {
            let wm = self.shared.window_mgr.lock_or_recover();
            match wm.get_window(hwnd) {
                Some(w) if w.pfn_wp == 0 && w.class_name.starts_with("WC_") => {
                    wm.find_frame_for_hwnd(hwnd)
                }
                _ => return false,
            }
        };
        self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
        self.shared.window_mgr.lock_or_recover().mark_dirty(frame);
        true
    }

    /// Make `hwnd` the focus window (0 = none), notify the owners of the
    /// controls that lost and gained it and repaint them. Returns the
    /// previous focus window.
    pub(crate) fn set_focus_window(&self, hwnd: u32) -> u32 {
        let old = self.shared.window_mgr.lock_or_recover().set_focus(hwnd);
        if old != hwnd {
            self.repaint_control(old);
            self.repaint_control(hwnd);
        }
        old
    }

    // ── Painting ─────────────────────────────────────────────────────────────

    /// Background behind a control: the dialog face inside a dialog, else
    /// the white of a client window.
    pub(super) fn control_background(&self, hwnd: u32) -> u32 {
        let wm = self.shared.window_mgr.lock_or_recover();
        if wm.is_dialog(wm.find_frame_for_hwnd(hwnd)) { CONTROL_FACE_COLOR } else { CONTROL_FIELD_COLOR }
    }

    /// WC_BUTTON: grey face with a border and a centred label. A push button
    /// is darker with its label shifted while held down, has a black outline
    /// when it is the default button (`BS_DEFAULT`) and a focus mark inside
    /// while it has the focus.
    pub(super) fn paint_button(&self, p: &Painter, hwnd: u32,
                               (x, y, cx, cy): (i32, i32, i32, i32), text: &str) {
        let (push, pressed, default, focused) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let style = wm.get_window(hwnd).map_or(0, |w| w.style);
            (wm.is_push_button(hwnd), wm.pressed_control == hwnd,
             style & BS_DEFAULT != 0, wm.focus_hwnd == hwnd)
        };
        let push = |flag: bool| push && flag;
        control_box(p, (x, y, cx, cy), if push(pressed) { CONTROL_PRESSED_COLOR } else { CONTROL_FACE_COLOR });
        if push(default) {
            p.op(DrawOp::Box { x1: x, y1: y, x2: x + cx, y2: y + cy, color: CONTROL_EMPHASIS_COLOR,
                               fill: false, mix: crate::gui::surface::Mix::Overpaint });
        }
        let max_w = cx - 2 * CONTROL_TEXT_INSET;
        let (shown, _) = super::pm_dialog::mnemonic_label(text);
        let w = font::text_width(font::fit_text(&shown, max_w, p.font()), p.font());
        let shift = if push(pressed) { BUTTON_PRESSED_SHIFT } else { 0 };
        super::control_label(p, x + (cx - w) / 2 + shift, y + (cy - CONTROL_LINE_H) / 2 - shift, text, max_w);
        if push(focused) {
            let i = BUTTON_FOCUS_INSET;
            p.op(DrawOp::Box { x1: x + i, y1: y + i, x2: x + cx - i, y2: y + cy - i,
                               color: CONTROL_EMPHASIS_COLOR, fill: false,
                               mix: crate::gui::surface::Mix::Overpaint });
        }
    }

    /// WC_ENTRYFIELD: white field with one vertically centred line of text,
    /// scrolled horizontally so the caret is visible; caret bar when focused.
    pub(super) fn paint_entry_field(&self, p: &Painter, hwnd: u32,
                                    (x, y, cx, cy): (i32, i32, i32, i32)) {
        let view_w = cx - 2 * CONTROL_TEXT_INSET;
        let (buf, first, focused, caret_on) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let focused = wm.focus_hwnd == hwnd;
            let Some(win) = wm.get_window_mut(hwnd) else { return };
            let buf = edit_buffer(win, false);
            let first = pm_edit::scroll_to_caret(
                buf.chars(), buf.caret(), win.edit_scroll, view_w - CARET_W, &system_width);
            store_selection(win, &buf);
            win.edit_scroll = first;
            (buf, first, focused, focused && !win.caret_off)
        };
        // The selection shows while the field has the focus.
        let selected = if focused { relative(buf.selection(), first) } else { 0..0 };
        control_box(p, (x, y, cx, cy), CONTROL_FIELD_COLOR);
        draw_edit_line(p, (x + CONTROL_TEXT_INSET, y + (cy - CONTROL_LINE_H) / 2),
                       view_w, &buf.chars()[first..], selected,
                       caret_on.then(|| buf.caret() - first), (y + 1, y + cy - 1));
    }

    /// WC_MLE: white field, one row per line from the top (per wrapped row
    /// with `MLS_WORDWRAP`). Scrolled vertically so the caret's row is
    /// visible; only the caret's row scrolls horizontally (with word wrap
    /// only when spaces hanging past the edge push the caret out of view).
    pub(super) fn paint_mle(&self, p: &Painter, hwnd: u32,
                            (x, y, cx, cy): (i32, i32, i32, i32)) {
        let view_w = cx - 2 * CONTROL_TEXT_INSET;
        let rows = control_visible_rows(cy);
        let (buf, top, first, focused, caret_on) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let focused = wm.focus_hwnd == hwnd;
            let Some(win) = wm.get_window_mut(hwnd) else { return };
            let buf = edit_buffer(win, true);
            let lines = buf.lines();
            let (caret_line, col) = buf.caret_line();
            let top = pm_edit::scroll_to_row(caret_line, win.top_row, rows, lines.len());
            let first = pm_edit::scroll_to_caret(
                &buf.chars()[lines[caret_line].clone()], col, win.edit_scroll,
                view_w - CARET_W, &system_width);
            store_selection(win, &buf);
            win.edit_scroll = first;
            win.top_row = top;
            (buf, top, first, focused, focused && !win.caret_off)
        };
        control_box(p, (x, y, cx, cy), CONTROL_FIELD_COLOR);
        let (caret_line, col) = buf.caret_line();
        let sel = if focused { buf.selection() } else { 0..0 };
        for (i, line) in buf.lines().into_iter().enumerate().skip(top).take(rows) {
            // The part of the selection in this line, relative to its start.
            let in_line = relative(sel.start.max(line.start)..sel.end.min(line.end), line.start);
            let in_line = if in_line.start < in_line.end { in_line } else { 0..0 };
            let chars = &buf.chars()[line];
            let (chars, caret, selected) = if i == caret_line {
                (&chars[first..], caret_on.then_some(col - first), relative(in_line, first))
            } else {
                (chars, None, in_line)
            };
            draw_edit_line(p, (x + CONTROL_TEXT_INSET, y + control_row_y(cy, i - top)),
                           view_w, chars, selected, caret, (y + 1, y + cy - 1));
        }
    }

    /// WC_LISTBOX: white field, one row per item from the first visible item;
    /// selected items are drawn highlighted and, in a list that selects
    /// several items, the cursor item is outlined while the list is focused.
    pub(super) fn paint_listbox(&self, p: &Painter, hwnd: u32,
                                (x, y, cx, cy): (i32, i32, i32, i32)) {
        let (items, sel, top_row, mode, focused) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(win) = wm.get_window(hwnd) else { return };
            (win.listbox_items.clone(), win.listbox_sel.clone(), win.top_row,
             ListMode::from_style(win.style), wm.focus_hwnd == hwnd)
        };
        let rows = control_visible_rows(cy);
        let top = listbox_top(top_row, items.len(), rows);
        control_box(p, (x, y, cx, cy), CONTROL_FIELD_COLOR);
        for (i, item) in items.iter().enumerate().skip(top).take(rows) {
            let row_y = y + control_row_y(cy, i - top);
            let selected = sel.is_selected(i);
            if selected {
                p.fill((x + 1, row_y, x + cx - 1, row_y + CONTROL_LINE_H - 1), CONTROL_SELECT_COLOR);
            }
            // A list that selects several items shows the cursor while focused.
            if focused && mode != ListMode::Single && sel.cursor() == Some(i) {
                p.op(DrawOp::Box { x1: x + 1, y1: row_y, x2: x + cx - 2, y2: row_y + CONTROL_LINE_H - 1,
                                   color: CONTROL_EMPHASIS_COLOR, fill: false,
                                   mix: crate::gui::surface::Mix::Overpaint });
            }
            let label = font::fit_text(item, cx - 2 * CONTROL_TEXT_INSET, p.font());
            if !label.is_empty() {
                p.op(DrawOp::Text {
                    x: x + CONTROL_TEXT_INSET, y: row_y, text: label.to_string(),
                    color: if selected { CONTROL_SELECT_TEXT_COLOR } else { CONTROL_TEXT_COLOR },
                    cjk: p.cjk,
                });
            }
        }
    }

    // ── Input ────────────────────────────────────────────────────────────────

    /// WM_BUTTON1DOWN on an entry field, MLE or list box: take the keyboard
    /// focus, then place the caret at / select the item under the pointer.
    /// `mp1` holds the pointer position relative to the control; with Shift
    /// held (`KC_SHIFT` in `fsInp`, the high word of `mp2`) an edit
    /// control's selection is extended to the pointer instead.
    pub(super) fn control_button_down(&self, hwnd: u32, class: &str, cy: i32, mp1: u32, mp2: u32) -> ApiResult {
        let (local_x, local_y) = mouse_pos(mp1);
        self.show_caret(hwnd);
        let extend = (mp2 >> 16) & KC_SHIFT != 0;
        let old_focus = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            match wm.get_window(hwnd) {
                Some(w) if w.style & WS_DISABLED == 0 => {}
                _ => return ApiResult::Normal(0),
            }
            wm.set_focus(hwnd)
        };
        if old_focus != hwnd {
            self.repaint_control(old_focus);
        }

        match class {
            CLASS_LISTBOX => {
                let item = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    wm.get_window(hwnd).and_then(|w| {
                        let count = w.listbox_items.len();
                        let top = listbox_top(w.top_row, count, control_visible_rows(cy));
                        control_row_at(cy, local_y).map(|row| top + row).filter(|&i| i < count)
                    })
                };
                if let Some(item) = item {
                    let ctrl = (mp2 >> 16) & KC_CTRL != 0;
                    self.listbox_update(hwnd, Some(item), |s, mode| s.click(item, mode, extend, ctrl));
                }
            }
            _ => {
                let multiline = class == CLASS_MLE;
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    let (line_before, _) = edit_buffer(win, multiline).caret_line();
                    let pos = edit_pos_at(win, multiline, cy, (local_x, local_y));
                    let mut buf = edit_buffer(win, multiline);
                    if extend { buf.extend_to(pos) } else { buf.set_caret(pos) }
                    store_selection(win, &buf);
                    if buf.caret_line().0 != line_before {
                        win.edit_scroll = 0;
                    }
                }
            }
        }
        // Held until released: dragging extends the selection.
        self.shared.window_mgr.lock_or_recover().pressed_control = hwnd;
        self.repaint_control(hwnd);
        ApiResult::Normal(1)
    }

    /// WM_BUTTON1DBLCLK on a list box: the second press of a double click
    /// selects like a press, then tells the owner `LN_ENTER` when an item is
    /// under the pointer.
    pub(super) fn listbox_double_click(&self, hwnd: u32, cy: i32, mp1: u32, mp2: u32) -> ApiResult {
        let result = self.control_button_down(hwnd, CLASS_LISTBOX, cy, mp1, mp2);
        let on_item = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.get_window(hwnd).is_some_and(|w| {
                let top = listbox_top(w.top_row, w.listbox_items.len(), control_visible_rows(cy));
                control_row_at(cy, mouse_pos(mp1).1)
                    .is_some_and(|row| top + row < w.listbox_items.len())
            })
        };
        if matches!(result, ApiResult::Normal(1)) && on_item {
            self.notify_owner(hwnd, LN_ENTER, hwnd);
        }
        result
    }

    /// WM_MOUSEMOVE on an entry field, MLE or list box held with button 1:
    /// extend the selection to the pointer / select the item under it.
    pub(super) fn control_mouse_move(&self, hwnd: u32, class: &str, cy: i32, mp1: u32) -> ApiResult {
        if self.shared.window_mgr.lock_or_recover().pressed_control != hwnd {
            return ApiResult::Normal(0);
        }
        let (local_x, local_y) = mouse_pos(mp1);
        if class == CLASS_LISTBOX {
            // Dragging above or below the list scrolls it an item at a time.
            let item = {
                let wm = self.shared.window_mgr.lock_or_recover();
                wm.get_window(hwnd).and_then(|w| {
                    let count = w.listbox_items.len();
                    let top = listbox_top(w.top_row, count, control_visible_rows(cy));
                    let last = count.checked_sub(1)?;
                    Some(row_under_pointer(top, cy, local_y).unwrap_or(0).min(last))
                })
            };
            if let Some(item) = item {
                self.listbox_update(hwnd, Some(item), |s, mode| s.drag_to(item, mode));
            }
            return ApiResult::Normal(1);
        }
        let multiline = class == CLASS_MLE;
        let changed = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(win) = wm.get_window_mut(hwnd) else { return ApiResult::Normal(0) };
            let pos = edit_pos_at(win, multiline, cy, (local_x, local_y));
            let mut buf = edit_buffer(win, multiline);
            let before = buf.caret();
            buf.extend_to(pos);
            store_selection(win, &buf);
            buf.caret() != before
        };
        if changed {
            self.repaint_control(hwnd);
        }
        ApiResult::Normal(1)
    }

    /// WM_BUTTON1UP on an entry field, MLE or list box: let go of it.
    pub(super) fn control_button_up(&self, hwnd: u32) -> ApiResult {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if wm.pressed_control == hwnd {
            wm.pressed_control = 0;
        }
        ApiResult::Normal(1)
    }

    /// WM_BUTTON1DOWN on a button: hold it down and take the focus (unless
    /// `BS_NOPOINTERFOCUS`). The release comes back to it even when the
    /// pointer has left it.
    pub(super) fn button_down(&self, hwnd: u32) -> ApiResult {
        let take_focus = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(style) = wm.get_window(hwnd).map(|w| w.style) else { return ApiResult::Normal(0) };
            if style & WS_DISABLED != 0 {
                return ApiResult::Normal(0);
            }
            wm.pressed_control = hwnd;
            style & BS_NOPOINTERFOCUS == 0
        };
        if take_focus {
            self.set_focus_window(hwnd);
        }
        self.repaint_control(hwnd);
        ApiResult::Normal(1)
    }

    /// WM_BUTTON1UP on a button: pop it up; released over the button (`mp1`
    /// = pointer position relative to it) it is clicked.
    pub(super) fn button_up(&self, hwnd: u32, cx: i32, cy: i32, mp1: u32) -> ApiResult {
        let was_down = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            std::mem::replace(&mut wm.pressed_control, 0) == hwnd
        };
        if !was_down {
            return ApiResult::Normal(0);
        }
        self.repaint_control(hwnd);
        let (lx, ly) = mouse_pos(mp1);
        if (0..cx).contains(&lx) && (0..cy).contains(&ly) {
            self.click_button(hwnd, true);
        }
        ApiResult::Normal(1)
    }

    /// WM_CHAR on a button: Space clicks it; other keys go to its owner.
    pub(super) fn button_char(&self, hwnd: u32, mp1: u32, mp2: u32) -> ApiResult {
        let fs = mp1 & 0xFFFF;
        let plain = fs & (KC_CTRL | KC_ALT) == 0;
        let space_key = fs & KC_VIRTUALKEY != 0 && mp2 >> WM_CHAR_VKEY_SHIFT == VK_SPACE;
        // The typed space arrives again as a character-only message.
        let space_char = fs & KEY_EVENT_KINDS == KC_CHAR && mp2 & 0xFFFF == b' ' as u32;
        if plain && (space_key || space_char) {
            if space_key && fs & KC_KEYUP == 0 {
                self.click_button(hwnd, false);
            }
            return ApiResult::Normal(1);
        }
        let owner = self.shared.window_mgr.lock_or_recover().notify_target(hwnd);
        if owner != 0 && owner != hwnd {
            self.post_control_message(owner, WM_CHAR, mp1, mp2);
        }
        ApiResult::Normal(0)
    }

    /// WM_TIMER(TID_CURSOR) on an entry field or MLE: the next phase of the
    /// caret blink. Only the focus window blinks; any other has its caret
    /// on for when it gets the focus.
    pub(super) fn edit_caret_blink(&self, hwnd: u32) -> ApiResult {
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let focused = wm.focus_hwnd == hwnd;
            let Some(win) = wm.get_window_mut(hwnd) else { return ApiResult::Normal(0) };
            win.caret_off = focused && !win.caret_off;
        }
        self.repaint_control(hwnd);
        ApiResult::Normal(0)
    }

    /// Keyboard or mouse input on a text control: its caret shows at once.
    fn show_caret(&self, hwnd: u32) {
        if let Some(w) = self.shared.window_mgr.lock_or_recover().get_window_mut(hwnd) {
            w.caret_off = false;
        }
    }

    /// WM_CHAR on an entry field, MLE or list box. Keys the control does not
    /// use are passed on to its owner, as PM controls do.
    pub(super) fn control_char(&self, hwnd: u32, class: &str, mp1: u32, mp2: u32) -> ApiResult {
        self.show_caret(hwnd);
        // Characters arrive in the codepage of the control's message queue.
        let codepage = self.window_codepage(hwnd);
        let enabled = self.shared.window_mgr.lock_or_recover()
            .get_window(hwnd).is_some_and(|w| w.style & WS_DISABLED == 0);
        let handled = enabled && if let Some(cmd) = clipboard_key(class, mp1, mp2) {
            // CUA clipboard keys; the release of the chord is swallowed too.
            if mp1 & KC_KEYUP == 0 {
                self.edit_clipboard(hwnd, class == CLASS_MLE, cmd);
            }
            true
        } else if class == CLASS_LISTBOX && mp1 & KC_VIRTUALKEY != 0
            && mp2 >> WM_CHAR_VKEY_SHIFT == VK_SPACE {
            self.listbox_space(hwnd, mp1 & KC_KEYUP != 0)
        } else { match pm_edit::key_from_wm_char(mp1, mp2, codepage) {
            Some((key, key_up)) if class == CLASS_LISTBOX => {
                self.listbox_key(hwnd, key, mp1 & KC_SHIFT != 0, key_up)
            }
            // Shift with a movement key extends the selection.
            Some((key, key_up)) => self.edit_key(hwnd, class == CLASS_MLE, key, mp1 & KC_SHIFT != 0, key_up),
            // The bare press/release of a printable key (its character
            // arrives separately as a character-only WM_CHAR) belongs to the
            // edit control too; it must not leak to the owner as a keystroke.
            None => class != CLASS_LISTBOX && mp1 & KEY_EVENT_KINDS == 0,
        } };
        if !handled {
            let owner = self.shared.window_mgr.lock_or_recover().notify_target(hwnd);
            if owner != 0 && owner != hwnd {
                self.post_control_message(owner, WM_CHAR, mp1, mp2);
            }
        }
        ApiResult::Normal(handled as u32)
    }

    /// Cut, copy, paste or clear in an entry field / MLE. Copied text goes to
    /// the PM clipboard (`CF_TEXT`) and the host clipboard; pasted text comes
    /// from it. The owner is notified when the text changed. Returns whether
    /// the text changed.
    pub(super) fn edit_clipboard(&self, hwnd: u32, multiline: bool, cmd: ClipboardCmd) -> bool {
        let changed = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let paste = wm.clipboard_text.clone();
            let Some(win) = wm.get_window_mut(hwnd) else { return false };
            // A read-only control only copies.
            let cmd = if is_read_only(win) && cmd != ClipboardCmd::Copy { return false } else { cmd };
            let mut buf = edit_buffer(win, multiline);
            let copied = matches!(cmd, ClipboardCmd::Cut | ClipboardCmd::Copy)
                .then(|| buf.selected_text()).filter(|t| !t.is_empty());
            let changed = match cmd {
                ClipboardCmd::Cut | ClipboardCmd::Clear => buf.delete_selection(),
                ClipboardCmd::Paste => buf.replace_selection(&paste),
                ClipboardCmd::Copy => false,
            };
            if changed {
                win.text = buf.text();
            }
            store_selection(win, &buf);
            if let Some(text) = &copied {
                wm.clipboard_text = text.clone();
                // A guest CF_TEXT handle no longer holds the clipboard text.
                wm.clipboard.insert(CF_TEXT, 0);
                if let Some(tx) = &wm.gui_tx {
                    let _ = tx.send(crate::gui::GUIMessage::SetClipboardText(text.clone()));
                }
            }
            changed
        };
        if changed {
            self.repaint_control(hwnd);
            if multiline {
                self.notify_owner(hwnd, MLN_CHANGE, 0);
            } else {
                self.notify_owner(hwnd, EN_CHANGE, hwnd);
            }
        }
        changed
    }

    /// EM_SETSEL (mp1 = MPFROM2SHORT(first, last)) / MLM_SETSEL (mp1 =
    /// anchor, mp2 = cursor): set the selection of an entry field / MLE;
    /// positions are characters, clamped to the text. Returns TRUE.
    pub(super) fn edit_set_selection(&self, hwnd: u32, multiline: bool, anchor: usize, caret: usize) -> ApiResult {
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(win) = wm.get_window_mut(hwnd) else { return ApiResult::Normal(0) };
            let mut buf = edit_buffer(win, multiline);
            buf.set_selection(anchor, caret);
            store_selection(win, &buf);
        }
        self.repaint_control(hwnd);
        ApiResult::Normal(1)
    }

    /// The selection of an entry field / MLE as `(anchor, caret)`, clamped.
    pub(super) fn edit_selection(&self, hwnd: u32, multiline: bool) -> (usize, usize) {
        let wm = self.shared.window_mgr.lock_or_recover();
        wm.get_window(hwnd).map_or((0, 0), |w| {
            let buf = edit_buffer(w, multiline);
            (buf.anchor(), buf.caret())
        })
    }

    /// Apply an editing key to an entry field / MLE. Returns whether the key
    /// belongs to the control (its key-up is then swallowed too).
    fn edit_key(&self, hwnd: u32, multiline: bool, key: EditKey, extend: bool, key_up: bool) -> bool {
        let changed = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(win) = wm.get_window_mut(hwnd) else { return false };
            let mut buf = edit_buffer(win, multiline);
            let (line_before, _) = buf.caret_line();
            let Some(changed) = buf.apply(key, extend, &system_width) else { return false };
            if key_up {
                return true;
            }
            // A read-only control takes the key but keeps its text and
            // selection when the key would change the text.
            if changed && is_read_only(win) {
                return true;
            }
            if changed {
                win.text = buf.text();
            }
            store_selection(win, &buf);
            let (line_after, _) = buf.caret_line();
            if line_after != line_before {
                win.edit_scroll = 0;
            }
            // A page key scrolls the view by as many lines as the caret moved.
            if matches!(key, EditKey::PageUp | EditKey::PageDown) {
                win.top_row = if line_after > line_before {
                    win.top_row + (line_after - line_before)
                } else {
                    win.top_row.saturating_sub(line_before - line_after)
                };
            }
            changed
        };
        self.repaint_control(hwnd);
        if changed {
            if multiline {
                self.notify_owner(hwnd, MLN_CHANGE, 0);
            } else {
                self.notify_owner(hwnd, EN_CHANGE, hwnd);
            }
        }
        true
    }

    /// Up/Down/PageUp/PageDown/Home/End move the cursor of a list box (and
    /// with it the selection, see [`ListSelection::move_cursor`]; Shift
    /// extends an extended selection); a page is the visible rows less one.
    /// Space (see [`Self::listbox_space`]) toggles the cursor item of a list
    /// that selects several items. Enter tells the owner `LN_ENTER` and is then passed on to it, so that
    /// in a dialog it still presses the default button
    /// (TODO(unverified): whether PM's list box keeps the key instead).
    fn listbox_key(&self, hwnd: u32, key: EditKey, shift: bool, key_up: bool) -> bool {
        let mode = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.get_window(hwnd).map_or(ListMode::Single, |w| ListMode::from_style(w.style))
        };
        let space = key == EditKey::Char(' ') && mode != ListMode::Single;
        if !space && !matches!(key, EditKey::Up | EditKey::Down | EditKey::PageUp | EditKey::PageDown
                                    | EditKey::Home | EditKey::End | EditKey::Enter) {
            return false;
        }
        if key == EditKey::Enter {
            if !key_up {
                self.notify_owner(hwnd, LN_ENTER, hwnd);
            }
            return false;
        }
        if key_up {
            return true;
        }
        if space {
            // The character of a Space key whose VK_SPACE already toggled.
            return true;
        }
        let target = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(win) = wm.get_window(hwnd) else { return false };
            let Some(last) = win.listbox_items.len().checked_sub(1) else { return true };
            let page = control_visible_rows(win.cy).saturating_sub(1).max(1);
            match (key, win.listbox_sel.cursor()) {
                (EditKey::End, _) => last,
                (EditKey::Up, Some(sel)) => sel.saturating_sub(1),
                (EditKey::Down, Some(sel)) => (sel + 1).min(last),
                (EditKey::PageUp, Some(sel)) => sel.saturating_sub(page),
                (EditKey::PageDown, Some(sel)) => (sel + page).min(last),
                // Home, or the first Up/Down in a list without a cursor.
                _ => 0,
            }
        };
        self.listbox_update(hwnd, Some(target), |s, mode| s.move_cursor(target, mode, shift));
        true
    }

    /// The Space key (`VK_SPACE`) on a list box: toggles the cursor item of a
    /// list that selects several items (its release and its character are
    /// swallowed too); a single-selection list leaves it to its owner.
    fn listbox_space(&self, hwnd: u32, key_up: bool) -> bool {
        let mode = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.get_window(hwnd).map_or(ListMode::Single, |w| ListMode::from_style(w.style))
        };
        if mode == ListMode::Single {
            return false;
        }
        if !key_up {
            self.listbox_update(hwnd, None, |s, mode| s.toggle_cursor(mode));
        }
        true
    }

    /// Change the selection of a list box with `change` (given its mode;
    /// returns whether the selection changed), scroll item `show` into view,
    /// repaint, and notify the owner with `LN_SELECT` if the selection
    /// changed. Returns `false` when `show` is out of range (nothing is
    /// changed then) or the window is gone.
    pub(super) fn listbox_update(&self, hwnd: u32, show: Option<usize>,
                                 change: impl FnOnce(&mut ListSelection, ListMode) -> bool) -> bool {
        let changed = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(win) = wm.get_window_mut(hwnd) else { return false };
            let count = win.listbox_items.len();
            if show.is_some_and(|i| i >= count) {
                return false;
            }
            if let Some(i) = show {
                win.top_row = pm_edit::scroll_to_row(
                    i, win.top_row, control_visible_rows(win.cy), count);
            }
            change(&mut win.listbox_sel, ListMode::from_style(win.style))
        };
        // The cursor outline may have moved even when the selection did not.
        self.repaint_control(hwnd);
        if changed {
            self.notify_owner(hwnd, LN_SELECT, hwnd);
        }
        true
    }

    /// LM_SELECTITEM: mp1 = item index (`LIT_NONE` deselects all), mp2 = TRUE
    /// to select / FALSE to deselect. In a list that selects one item,
    /// selecting replaces the selection; otherwise the other items keep
    /// theirs. Returns TRUE on success.
    pub(super) fn listbox_select_item(&self, hwnd: u32, mp1: u32, mp2: u32) -> ApiResult {
        // The index is a SHORT; MPFROMSHORT(-1) and MPFROMLONG(-1) both
        // sign-extend from the low word.
        let index = mp1 as u16 as i16 as i32;
        let ok = if index == LIT_NONE {
            self.listbox_update(hwnd, None, |s, mode| s.clear(mode))
        } else if index < 0 {
            false
        } else {
            let (item, on) = (index as usize, mp2 != 0);
            // Selecting scrolls the item into view; deselecting does not.
            let in_range = self.shared.window_mgr.lock_or_recover()
                .get_window(hwnd).is_some_and(|w| item < w.listbox_items.len());
            in_range && self.listbox_update(hwnd, on.then_some(item), |s, mode| s.select(item, on, mode))
        };
        ApiResult::Normal(ok as u32)
    }

    /// LM_DELETEITEM: mp1 = index of the item to remove. The selection
    /// follows the items after it; a removed selected item leaves none.
    /// Returns the number of items left (the count unchanged for an index
    /// out of range).
    pub(super) fn listbox_delete_item(&self, hwnd: u32, mp1: u32) -> ApiResult {
        let index = mp1 as u16 as i16;
        let left = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(win) = wm.get_window_mut(hwnd) else { return ApiResult::Normal(0) };
            let Ok(index) = usize::try_from(index) else { return ApiResult::Normal(win.listbox_items.len() as u32) };
            if index >= win.listbox_items.len() {
                return ApiResult::Normal(win.listbox_items.len() as u32);
            }
            win.listbox_items.remove(index);
            win.listbox_sel.removed(index);
            win.listbox_items.len()
        };
        self.repaint_control(hwnd);
        ApiResult::Normal(left as u32)
    }

    /// LM_DELETEALL: remove every item. Returns TRUE.
    pub(super) fn listbox_delete_all(&self, hwnd: u32) -> ApiResult {
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(win) = wm.get_window_mut(hwnd) else { return ApiResult::Normal(0) };
            win.listbox_items.clear();
            win.listbox_sel = ListSelection::default();
            win.top_row = 0;
        }
        self.repaint_control(hwnd);
        ApiResult::Normal(1)
    }

    /// EM_SETREADONLY / MLM_SETREADONLY: mp1 = TRUE makes the control
    /// read-only. Returns the previous state.
    /// MLM_SETWRAP: switch word wrap on or off and repaint. Returns TRUE.
    pub(super) fn mle_set_wrap(&self, hwnd: u32, wrap: bool) -> ApiResult {
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(win) = wm.get_window_mut(hwnd) else { return ApiResult::Normal(0) };
            if wrap { win.style |= MLS_WORDWRAP } else { win.style &= !MLS_WORDWRAP }
            win.edit_scroll = 0;
        }
        self.repaint_control(hwnd);
        ApiResult::Normal(1)
    }

    pub(super) fn edit_set_read_only(&self, hwnd: u32, read_only: bool) -> ApiResult {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(win) = wm.get_window_mut(hwnd) else { return ApiResult::Normal(0) };
        let flag = read_only_style(&win.class_name);
        let was = win.style & flag != 0;
        if read_only { win.style |= flag } else { win.style &= !flag }
        ApiResult::Normal(was as u32)
    }

    /// LM_QUERYSELECTION: mp1 = index to search after (`LIT_FIRST` = from the
    /// top; `LIT_CURSOR` asks for the cursor item instead). Returns the
    /// index of the next selected item, or `LIT_NONE`.
    pub(super) fn listbox_query_selection(&self, hwnd: u32, mp1: u32) -> ApiResult {
        let after = mp1 as u16 as i16 as i32;
        let wm = self.shared.window_mgr.lock_or_recover();
        let Some(sel) = wm.get_window(hwnd).map(|w| &w.listbox_sel) else {
            return ApiResult::Normal(LIT_NONE as u32);
        };
        let found = match after {
            LIT_CURSOR => sel.cursor(),
            a if a < 0 => sel.next_after(None),
            a => sel.next_after(Some(a as usize)),
        };
        ApiResult::Normal(found.map_or(LIT_NONE as u32, |i| i as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_CX: i32 = 640;
    const FRAME_CY: i32 = 480;

    struct Fixture {
        loader: Loader,
        client: u32,
        hmq: u32,
        frame: u32,
    }

    /// A std window (frame + client with a guest window procedure) whose
    /// surface records what the controls draw.
    fn fixture() -> Fixture {
        let loader = Loader::new_mock();
        let (client, hmq, frame) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(850);
            wm.tid_to_hmq.insert(0, hmq);
            wm.register_class("App".into(), 0x1234, 0);
            let frame = wm.create_window("App".into(), 0, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            for h in [frame, client] {
                let w = wm.get_window_mut(h).unwrap();
                (w.cx, w.cy, w.visible) = (FRAME_CX, FRAME_CY, true);
            }
            wm.z_push_top(frame);
            wm.focused_hwnd = frame;
            (client, hmq, frame)
        };
        Fixture { loader, client, hmq, frame }
    }

    impl Fixture {
        fn add(&self, class: &str, id: u32, rect: (i32, i32, i32, i32), text: &str) -> u32 {
            let mut wm = self.loader.shared.window_mgr.lock_or_recover();
            let h = wm.create_window(class.into(), self.client, self.hmq);
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy) = rect;
            (w.id, w.visible, w.text) = (id, true, text.to_string());
            h
        }

        fn with<R>(&self, hwnd: u32, f: impl FnOnce(&mut super::super::pm_types::OS2Window) -> R) -> R {
            f(self.loader.shared.window_mgr.lock_or_recover().get_window_mut(hwnd).unwrap())
        }

        fn text(&self, hwnd: u32) -> String { self.with(hwnd, |w| w.text.clone()) }
        fn caret(&self, hwnd: u32) -> usize { self.with(hwnd, |w| w.edit_caret) }
        fn focus(&self) -> u32 { self.loader.shared.window_mgr.lock_or_recover().focus_hwnd }

        fn send(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(hwnd, msg, mp1, mp2) {
                ApiResult::Normal(v) => v,
                _ => panic!("built-in controls answer synchronously"),
            }
        }

        fn click(&self, hwnd: u32, x: i32, y: i32) -> u32 {
            self.send(hwnd, WM_BUTTON1DOWN, (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16), 0)
        }

        fn vkey(&self, hwnd: u32, vk: u32) -> u32 {
            self.send(hwnd, WM_CHAR, KC_VIRTUALKEY | KC_SCANCODE, vk << WM_CHAR_VKEY_SHIFT)
        }

        /// Type `text` as WM_CHAR messages in the codepage of the queue.
        fn type_text(&self, hwnd: u32, text: &str) {
            let cp = self.loader.window_codepage(hwnd);
            for ch in text.chars() {
                let bytes = super::super::codepage::cp_encode(&ch.to_string(), cp);
                let chr = bytes.iter().rev().fold(0u32, |acc, &b| (acc << 8) | b as u32);
                assert_eq!(self.send(hwnd, WM_CHAR, KC_CHAR | KC_SCANCODE, chr), 1, "typing {ch:?}");
            }
        }

        /// Drain the message queue, returning `(hwnd, msg, mp1, mp2)` tuples.
        fn posted(&self) -> Vec<(u32, u32, u32, u32)> {
            let mq = self.loader.shared.window_mgr.lock_or_recover().get_mq(self.hmq).unwrap();
            mq.lock_or_recover().messages.drain(..).map(|m| (m.hwnd, m.msg, m.mp1, m.mp2)).collect()
        }

        /// Repaint `hwnd` and return the operations drawn into its frame's
        /// surface.
        fn paint(&self, hwnd: u32) -> Vec<DrawOp> {
            let surface = self.loader.shared.window_mgr.lock_or_recover()
                .frame_surface(self.frame).unwrap();
            surface.lock_or_recover().record = Some(Vec::new());
            self.send(hwnd, WM_PAINT, 0, 0);
            surface.lock_or_recover().record.take().unwrap()
        }

        fn drawn_texts(&self, hwnd: u32) -> Vec<(i32, i32, String, u32)> {
            self.paint(hwnd).into_iter().filter_map(|m| match m {
                DrawOp::Text { x, y, text, color, .. } => Some((x, y, text, color)),
                _ => None,
            }).collect()
        }

        /// Filled boxes of `color`: `(x1, y1, x2, y2)`.
        fn drawn_fills(&self, hwnd: u32, color: u32) -> Vec<(i32, i32, i32, i32)> {
            self.paint(hwnd).into_iter().filter_map(|m| match m {
                DrawOp::Box { x1, y1, x2, y2, color: c, fill: true, .. } if c == color => {
                    Some((x1, y1, x2, y2))
                }
                _ => None,
            }).collect()
        }

        /// The caret bar of an edit control: fills of the caret colour that
        /// are exactly `CARET_W` wide.
        fn caret_bars(&self, hwnd: u32) -> Vec<(i32, i32, i32, i32)> {
            self.drawn_fills(hwnd, CONTROL_CARET_COLOR).into_iter()
                .filter(|&(x1, _, x2, _)| x2 - x1 == CARET_W - 1).collect()
        }
    }

    fn control_msg(id: u32, code: u32) -> u32 { (code << 16) | id }

    const ENTRY_RECT: (i32, i32, i32, i32) = (10, 200, 200, 25);
    const ID_ENTRY: u32 = 103;
    const ID_LIST: u32 = 105;
    const ID_MLE: u32 = 106;

    /// Control text takes its CJK face from the codepage of its window's
    /// queue (WinSetCp), not from the process codepage.
    #[test]
    fn control_text_uses_the_cjk_face_of_the_queue_codepage() {
        use crate::gui::font::CjkRegion;
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "漢字");
        let list = fx.add(CLASS_LISTBOX, ID_LIST, (10, 100, 200, 80), "");
        fx.with(list, |w| w.listbox_items = vec!["한국어".into()]);
        let faces = |hwnd| -> Vec<CjkRegion> {
            fx.paint(hwnd).into_iter().filter_map(|op| match op {
                DrawOp::Text { cjk, .. } => Some(cjk),
                _ => None,
            }).collect()
        };
        for (cp, region) in [(949, CjkRegion::Korean), (950, CjkRegion::TraditionalChinese),
                             (936, CjkRegion::SimplifiedChinese), (932, CjkRegion::Japanese)] {
            let mq = fx.loader.shared.window_mgr.lock_or_recover().get_mq(fx.hmq).unwrap();
            mq.lock_or_recover().codepage = cp;
            for hwnd in [entry, list] {
                let f = faces(hwnd);
                assert!(!f.is_empty() && f.iter().all(|&r| r == region), "cp {cp}: {f:?}");
            }
        }
    }

    // ── Geometry ─────────────────────────────────────────────────────────────

    #[test]
    fn rows_run_top_down_and_only_whole_rows_are_visible() {
        // 100-px control, 16-px rows, 2-px top inset, 1-px bottom border → 6 rows.
        assert_eq!(control_visible_rows(100), 6);
        assert_eq!(control_row_y(100, 0), 100 - 2 - CONTROL_LINE_H);
        assert_eq!(control_row_y(100, 0) - control_row_y(100, 1), CONTROL_LINE_H);
        assert!(control_row_y(100, 5) > 0, "last row ends above the bottom border");
        // A control too short for a single row shows nothing.
        assert_eq!(control_visible_rows(10), 0);
        assert_eq!(control_visible_rows(0), 0);
        assert_eq!(control_visible_rows(-5), 0);
        assert_eq!(control_visible_rows(CONTROL_LINE_H + 3), 1);
        assert_eq!(control_visible_rows(CONTROL_LINE_H + 2), 0);
    }

    #[test]
    fn row_at_maps_pointer_y_to_rows() {
        assert_eq!(control_row_at(100, 97), Some(0), "top pixel of the first row");
        assert_eq!(control_row_at(100, 82), Some(0), "bottom pixel of the first row");
        assert_eq!(control_row_at(100, 81), Some(1));
        assert_eq!(control_row_at(100, 2), Some(5), "bottom pixel of the last row");
        assert_eq!(control_row_at(100, 1), None, "below the last whole row");
        assert_eq!(control_row_at(100, 98), None, "top margin");
        assert_eq!(control_row_at(100, 500), None);
        assert_eq!(control_row_at(100, -1), None);
        assert_eq!(control_row_at(10, 5), None, "no rows at all");
        // Every pixel of a row maps back to that row.
        for row in 0..control_visible_rows(100) {
            let y = control_row_y(100, row);
            assert_eq!(control_row_at(100, y), Some(row));
            assert_eq!(control_row_at(100, y + CONTROL_LINE_H - 1), Some(row));
        }
    }

    // ── Entry field ──────────────────────────────────────────────────────────

    #[test]
    fn entry_click_takes_focus_and_places_the_caret() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "hello");
        assert!(fx.caret_bars(entry).is_empty(), "no caret without focus");

        // Click far right of the text: caret at the end.
        assert_eq!(fx.click(entry, 190, 12), 1);
        assert_eq!(fx.focus(), entry);
        assert_eq!(fx.caret(entry), 5);

        // Click on the boundary between "he" and "llo".
        let x = CONTROL_TEXT_INSET + system_width("he");
        fx.click(entry, x, 12);
        assert_eq!(fx.caret(entry), 2);
        // Click in the left inset / left of the control's text: caret at 0.
        fx.click(entry, 0, 12);
        assert_eq!(fx.caret(entry), 0);

        // The caret is drawn where text_width says the prefix ends.
        fx.click(entry, x, 12);
        let (ex, ey, _, ecy) = ENTRY_RECT;
        let bars = fx.caret_bars(entry);
        assert_eq!(bars.len(), 1);
        let (x1, y1, x2, y2) = bars[0];
        assert_eq!(x1, ex + CONTROL_TEXT_INSET + system_width("he"));
        assert_eq!(x2 - x1 + 1, CARET_W);
        assert!(y1 > ey && y2 < ey + ecy, "caret stays inside the field");
    }

    #[test]
    fn entry_typing_and_editing_keys_update_the_window_text() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "");
        fx.click(entry, 5, 12);
        fx.type_text(entry, "helo");
        assert_eq!((fx.text(entry).as_str(), fx.caret(entry)), ("helo", 4));

        assert_eq!(fx.vkey(entry, VK_LEFT), 1);
        fx.type_text(entry, "l");
        assert_eq!((fx.text(entry).as_str(), fx.caret(entry)), ("hello", 4));

        fx.vkey(entry, VK_HOME);
        assert_eq!(fx.caret(entry), 0);
        fx.vkey(entry, VK_BACKSPACE);
        assert_eq!(fx.text(entry), "hello", "Backspace at the start does nothing");
        fx.vkey(entry, VK_DELETE);
        assert_eq!((fx.text(entry).as_str(), fx.caret(entry)), ("ello", 0));
        fx.vkey(entry, VK_END);
        assert_eq!(fx.caret(entry), 4);
        fx.vkey(entry, VK_DELETE);
        assert_eq!(fx.text(entry), "ello", "Delete at the end does nothing");
        fx.vkey(entry, VK_BACKSPACE);
        fx.vkey(entry, VK_RIGHT);
        assert_eq!((fx.text(entry).as_str(), fx.caret(entry)), ("ell", 3));

        // The edited text is what gets painted.
        let texts = fx.drawn_texts(entry);
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].2, "ell");
    }

    #[test]
    fn entry_notifies_owner_with_en_change_only_when_text_changes() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "ab");
        fx.click(entry, 190, 12);
        fx.posted();

        fx.type_text(entry, "c");
        assert_eq!(fx.posted(), vec![(fx.client, WM_CONTROL, control_msg(ID_ENTRY, EN_CHANGE), entry)]);
        fx.vkey(entry, VK_BACKSPACE);
        assert_eq!(fx.posted(), vec![(fx.client, WM_CONTROL, control_msg(ID_ENTRY, EN_CHANGE), entry)]);

        // Caret movement and no-op edits do not notify.
        fx.vkey(entry, VK_LEFT);
        fx.vkey(entry, VK_HOME);
        fx.vkey(entry, VK_BACKSPACE);
        fx.vkey(entry, VK_END);
        fx.vkey(entry, VK_DELETE);
        assert_eq!(fx.posted(), vec![]);

        // An explicit owner is notified instead of the parent.
        let owner = fx.loader.shared.window_mgr.lock_or_recover()
            .create_window("App".into(), 0, fx.hmq);
        fx.with(entry, |w| w.owner = owner);
        fx.type_text(entry, "!");
        assert_eq!(fx.posted(), vec![(owner, WM_CONTROL, control_msg(ID_ENTRY, EN_CHANGE), entry)]);
    }

    #[test]
    fn entry_passes_unused_keys_to_the_owner_and_swallows_its_own_key_ups() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "ab");
        fx.click(entry, 190, 12);
        fx.posted();

        // Enter, Up, F1 and Ctrl chords are not entry-field keys.
        let enter = (KC_VIRTUALKEY | KC_CHAR, 13 | (VK_NEWLINE << WM_CHAR_VKEY_SHIFT));
        let f1 = (KC_VIRTUALKEY, VK_F1 << WM_CHAR_VKEY_SHIFT);
        let up = (KC_VIRTUALKEY, VK_UP << WM_CHAR_VKEY_SHIFT);
        let ctrl_c = (KC_CHAR | KC_CTRL, b'c' as u32);
        for (mp1, mp2) in [enter, f1, up, ctrl_c] {
            assert_eq!(fx.send(entry, WM_CHAR, mp1, mp2), 0);
            assert_eq!(fx.posted(), vec![(fx.client, WM_CHAR, mp1, mp2)]);
        }
        assert_eq!((fx.text(entry).as_str(), fx.caret(entry)), ("ab", 2));

        // The bare press / release of a printable key (no character): consumed.
        assert_eq!(fx.send(entry, WM_CHAR, KC_SCANCODE | KC_SHIFT, 0), 1);
        assert_eq!(fx.send(entry, WM_CHAR, KC_SCANCODE | KC_KEYUP, 0), 1);
        // Key-up of a character or editing key: consumed, nothing happens.
        assert_eq!(fx.send(entry, WM_CHAR, KC_CHAR | KC_KEYUP, b'x' as u32), 1);
        assert_eq!(fx.send(entry, WM_CHAR, KC_VIRTUALKEY | KC_KEYUP,
                           VK_BACKSPACE << WM_CHAR_VKEY_SHIFT), 1);
        assert_eq!((fx.text(entry).as_str(), fx.caret(entry)), ("ab", 2));
        assert_eq!(fx.posted(), vec![]);
    }

    #[test]
    fn entry_scrolls_horizontally_to_keep_the_caret_visible() {
        let fx = fixture();
        let (ex, _, ecx, _) = ENTRY_RECT;
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "");
        fx.click(entry, 5, 12);
        let long = "The quick brown fox jumps over the lazy dog, twice over.";
        assert!(system_width(long) > ecx);
        fx.type_text(entry, long);

        let right_edge = ex + ecx - CONTROL_TEXT_INSET;
        let check_visible = |what: &str| {
            let bars = fx.caret_bars(entry);
            assert_eq!(bars.len(), 1, "{what}");
            let (x1, _, x2, _) = bars[0];
            assert!(x1 >= ex + CONTROL_TEXT_INSET && x2 < right_edge,
                    "{what}: caret {x1}..{x2} outside {}..{right_edge}", ex + CONTROL_TEXT_INSET);
        };
        check_visible("after typing past the right edge");
        let scroll = fx.with(entry, |w| w.edit_scroll);
        assert!(scroll > 0, "view scrolled right");
        // The drawn text is the tail of the string, clipped to the field.
        let texts = fx.drawn_texts(entry);
        let tail: String = long.chars().skip(scroll).collect();
        assert_eq!(texts[0].2, tail);
        assert!(system_width(&texts[0].2) <= ecx - 2 * CONTROL_TEXT_INSET);

        // Walk back to the start one key at a time.
        for i in 0..long.chars().count() {
            fx.vkey(entry, VK_LEFT);
            check_visible(&format!("after {} x Left", i + 1));
        }
        assert_eq!(fx.with(entry, |w| w.edit_scroll), 0);
        let texts = fx.drawn_texts(entry);
        assert!(long.starts_with(&texts[0].2) && texts[0].2.len() < long.len());

        // A click in a scrolled field accounts for the scroll offset.
        fx.vkey(entry, VK_END);
        let first = fx.with(entry, |w| w.edit_scroll);
        fx.click(entry, CONTROL_TEXT_INSET, 12);
        assert_eq!(fx.caret(entry), first, "click on the first visible character");
    }

    /// CP932 message queue (`WinSetCp`): typed double-byte characters arrive
    /// as lead|trail<<8, are stored as Unicode and edited per character.
    #[test]
    fn entry_edits_dbcs_text_per_character() {
        let fx = fixture();
        fx.loader.shared.window_mgr.lock_or_recover().get_mq(fx.hmq).unwrap().lock_or_recover().codepage = 932;
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "");
        fx.click(entry, 5, 12);
        fx.type_text(entry, "日本語abc");
        assert_eq!((fx.text(entry).as_str(), fx.caret(entry)), ("日本語abc", 6));
        fx.vkey(entry, VK_HOME);
        fx.vkey(entry, VK_RIGHT);
        fx.vkey(entry, VK_DELETE);
        assert_eq!((fx.text(entry).as_str(), fx.caret(entry)), ("日語abc", 1));
        fx.vkey(entry, VK_BACKSPACE);
        assert_eq!(fx.text(entry), "語abc");
        // Clicking in the middle of a wide glyph lands on a character boundary.
        fx.click(entry, CONTROL_TEXT_INSET + system_width("語") / 2 + 1, 12);
        assert_eq!(fx.caret(entry), 1);
        assert_eq!(fx.caret_bars(entry).len(), 1);
    }

    /// The guest replaces the text (WinSetWindowText) while the caret and
    /// scroll offset still describe the old, longer text.
    #[test]
    fn entry_survives_text_replaced_behind_its_back() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "");
        fx.click(entry, 5, 12);
        fx.type_text(entry, &"wide text ".repeat(10));
        assert!(fx.with(entry, |w| w.edit_scroll) > 0);

        fx.with(entry, |w| w.text = "日本".to_string());
        assert_eq!(fx.drawn_texts(entry)[0].2, "日本");
        assert_eq!(fx.caret(entry), 2, "caret clamped to the new text");
        fx.type_text(entry, "!");
        assert_eq!(fx.text(entry), "日本!");

        fx.with(entry, |w| { w.text.clear(); w.edit_caret = 99; w.edit_scroll = 99; });
        fx.click(entry, 100, 12);
        assert_eq!(fx.caret(entry), 0);
        fx.vkey(entry, VK_BACKSPACE);
        fx.vkey(entry, VK_DELETE);
        assert!(fx.drawn_texts(entry).is_empty());
        assert_eq!(fx.caret_bars(entry).len(), 1, "empty focused field still shows a caret");
    }

    #[test]
    fn disabled_controls_ignore_input() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "ab");
        fx.with(entry, |w| w.style |= WS_DISABLED);
        assert_eq!(fx.click(entry, 5, 12), 0);
        assert_eq!(fx.focus(), 0);
        assert_eq!(fx.send(entry, WM_CHAR, KC_CHAR, b'x' as u32), 0);
        assert_eq!(fx.text(entry), "ab");
    }

    #[test]
    fn focus_moves_the_caret_between_controls() {
        let fx = fixture();
        let a = fx.add(CLASS_ENTRYFIELD, 1, ENTRY_RECT, "a");
        let b = fx.add(CLASS_ENTRYFIELD, 2, (10, 100, 200, 25), "b");
        fx.click(a, 5, 12);
        assert_eq!(fx.caret_bars(a).len(), 1);
        fx.click(b, 5, 12);
        assert_eq!(fx.focus(), b);
        assert!(fx.caret_bars(a).is_empty(), "the old focus loses its caret");
        assert_eq!(fx.caret_bars(b).len(), 1);

        assert_eq!(fx.loader.set_focus_window(0), b);
        assert!(fx.caret_bars(b).is_empty());
    }

    // ── MLE ──────────────────────────────────────────────────────────────────

    const MLE_RECT: (i32, i32, i32, i32) = (10, 5, 200, 54); // 3 rows

    #[test]
    fn mle_enter_and_vertical_movement() {
        let fx = fixture();
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, "");
        fx.click(mle, 5, 40);
        fx.type_text(mle, "first");
        assert_eq!(fx.vkey(mle, VK_NEWLINE), 1);
        fx.type_text(mle, "second line");
        assert_eq!(fx.text(mle), "first\nsecond line");
        assert_eq!(fx.posted().iter().filter(|m| m.2 == control_msg(ID_MLE, MLN_CHANGE)).count(),
                   "first".len() + 1 + "second line".len());

        // Up from the end of the long line lands at the end of the short one.
        assert_eq!(fx.vkey(mle, VK_UP), 1);
        assert_eq!(fx.caret(mle), 5);
        assert_eq!(fx.vkey(mle, VK_UP), 1, "Up on the first line is consumed");
        assert_eq!(fx.caret(mle), 5);
        fx.vkey(mle, VK_HOME);
        fx.vkey(mle, VK_DOWN);
        assert_eq!(fx.caret(mle), 6, "start of the second line");
        fx.vkey(mle, VK_END);
        assert_eq!(fx.caret(mle), 17);
        // Backspace at the start of a line joins it with the previous one.
        fx.vkey(mle, VK_HOME);
        fx.vkey(mle, VK_BACKSPACE);
        assert_eq!((fx.text(mle).as_str(), fx.caret(mle)), ("firstsecond line", 5));
        assert_eq!(fx.posted().len(), 1, "one MLN_CHANGE, no stray messages");
    }

    #[test]
    fn mle_paints_one_row_per_line_with_caret_on_its_line() {
        let fx = fixture();
        let (mx, my, _, mcy) = MLE_RECT;
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, "Line one\r\nLine two");
        let texts = fx.drawn_texts(mle);
        assert_eq!(texts.iter().map(|t| t.2.as_str()).collect::<Vec<_>>(), ["Line one", "Line two"]);
        assert_eq!(texts[0].1, my + control_row_y(mcy, 0));
        assert_eq!(texts[1].1, my + control_row_y(mcy, 1));

        // Click on the second row, between "Line" and " two".
        let x = CONTROL_TEXT_INSET + system_width("Line");
        fx.click(mle, x, control_row_y(mcy, 1) + 3);
        assert_eq!(fx.focus(), mle);
        assert_eq!(fx.caret(mle), "Line one\r\n".len() + 4);
        let bars = fx.caret_bars(mle);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].0, mx + x);
        assert_eq!(bars[0].1, my + control_row_y(mcy, 1));

        // Clicks above the first row / below the last line clamp to them.
        fx.click(mle, 190, mcy - 1);
        assert_eq!(fx.caret(mle), 8, "end of the first line, before the CR");
        fx.click(mle, 0, 0);
        assert_eq!(fx.caret(mle), 10, "start of the last line");
    }

    #[test]
    fn mle_scrolls_vertically_to_the_caret_line() {
        let fx = fixture();
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, "");
        assert_eq!(control_visible_rows(MLE_RECT.3), 3);
        fx.click(mle, 5, 40);
        for i in 0..6 {
            fx.type_text(mle, &format!("line {i}"));
            if i < 5 { fx.vkey(mle, VK_NEWLINE); }
        }
        let shown = |fx: &Fixture| fx.drawn_texts(mle).into_iter().map(|t| t.2).collect::<Vec<_>>();
        assert_eq!(shown(&fx), ["line 3", "line 4", "line 5"]);
        assert_eq!(fx.caret_bars(mle).len(), 1);
        for _ in 0..4 { fx.vkey(mle, VK_UP); }
        assert_eq!(shown(&fx), ["line 1", "line 2", "line 3"]);
        fx.vkey(mle, VK_UP);
        assert_eq!(shown(&fx), ["line 0", "line 1", "line 2"]);
        // A click maps through the vertical scroll offset.
        for _ in 0..5 { fx.vkey(mle, VK_DOWN); }
        fx.click(mle, 0, control_row_y(MLE_RECT.3, 0) + 1);
        assert_eq!(EditBuffer::new(&fx.text(mle), fx.caret(mle), true).caret_line(), (3, 0));
    }

    #[test]
    fn mle_scrolls_only_the_caret_line_horizontally() {
        let fx = fixture();
        let (mx, _, mcx, _) = MLE_RECT;
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, "short\n");
        fx.click(mle, 0, 0);
        let long = "a line that is much wider than the multi-line entry field";
        fx.type_text(mle, long);
        let texts = fx.drawn_texts(mle);
        assert_eq!(texts[0].2, "short", "other lines stay at column 0");
        assert!(long.ends_with(&texts[1].2) && texts[1].2.len() < long.len());
        let (x1, _, x2, _) = fx.caret_bars(mle)[0];
        assert!(x1 >= mx + CONTROL_TEXT_INSET && x2 < mx + mcx - CONTROL_TEXT_INSET);
        // Leaving the line resets its scroll.
        fx.vkey(mle, VK_UP);
        assert_eq!(fx.with(mle, |w| w.edit_scroll), 0);
        assert!(long.starts_with(&fx.drawn_texts(mle)[1].2));
    }

    // ── List box ─────────────────────────────────────────────────────────────

    const LIST_RECT: (i32, i32, i32, i32) = (10, 60, 200, 54); // 3 rows

    fn list_with(fx: &Fixture, n: usize) -> u32 {
        let list = fx.add(CLASS_LISTBOX, ID_LIST, LIST_RECT, "");
        fx.with(list, |w| w.listbox_items = (0..n).map(|i| format!("Item {i}")).collect());
        list
    }

    fn selection(fx: &Fixture, list: u32) -> i32 {
        fx.send(list, LM_QUERYSELECTION, LIT_FIRST as u32, 0) as i32
    }

    #[test]
    fn listbox_select_item_and_query_selection() {
        let fx = fixture();
        let list = list_with(&fx, 3);
        assert_eq!(selection(&fx, list), LIT_NONE);

        assert_eq!(fx.send(list, LM_SELECTITEM, 1, 1), 1);
        assert_eq!(selection(&fx, list), 1);
        assert_eq!(fx.posted(), vec![(fx.client, WM_CONTROL, control_msg(ID_LIST, LN_SELECT), list)]);
        // Selecting the selected item again changes nothing: no notification.
        assert_eq!(fx.send(list, LM_SELECTITEM, 1, 1), 1);
        assert_eq!(fx.posted(), vec![]);

        // LM_QUERYSELECTION searches after the given index.
        assert_eq!(fx.send(list, LM_QUERYSELECTION, 0, 0), 1);
        assert_eq!(fx.send(list, LM_QUERYSELECTION, 1, 0) as i32, LIT_NONE);
        assert_eq!(fx.send(list, LM_QUERYSELECTION, 0xFFFF, 0), 1, "MPFROMSHORT(LIT_FIRST)");

        // Deselecting another item leaves the selection alone.
        assert_eq!(fx.send(list, LM_SELECTITEM, 2, 0), 1);
        assert_eq!(selection(&fx, list), 1);
        assert_eq!(fx.send(list, LM_SELECTITEM, 1, 0), 1);
        assert_eq!(selection(&fx, list), LIT_NONE);
        assert_eq!(fx.posted().len(), 1);

        // LIT_NONE deselects everything, as a SHORT or a LONG.
        fx.send(list, LM_SELECTITEM, 2, 1);
        assert_eq!(fx.send(list, LM_SELECTITEM, 0xFFFF, 0), 1);
        assert_eq!(selection(&fx, list), LIT_NONE);
        fx.send(list, LM_SELECTITEM, 2, 1);
        assert_eq!(fx.send(list, LM_SELECTITEM, LIT_NONE as u32, 1), 1);
        assert_eq!(selection(&fx, list), LIT_NONE);
    }

    #[test]
    fn listbox_rejects_out_of_range_indices() {
        let fx = fixture();
        let list = list_with(&fx, 3);
        fx.send(list, LM_SELECTITEM, 0, 1);
        fx.posted();
        for bad in [3u32, 100, 0x7FFF, 0xFFFE, 0x8000] {
            assert_eq!(fx.send(list, LM_SELECTITEM, bad, 1), 0, "index {bad:#x}");
            assert_eq!(fx.send(list, LM_SELECTITEM, bad, 0), 0, "index {bad:#x}");
        }
        assert_eq!(selection(&fx, list), 0, "selection untouched");
        assert_eq!(fx.posted(), vec![]);
        // Not a list box at all / unknown window.
        let empty = list_with(&fx, 0);
        assert_eq!(fx.send(empty, LM_SELECTITEM, 0, 1), 0);
        assert_eq!(selection(&fx, empty), LIT_NONE);
        assert_eq!(fx.send(0xDEAD, LM_SELECTITEM, 0, 1), 0);
    }

    #[test]
    fn listbox_click_selects_the_item_under_the_pointer() {
        let fx = fixture();
        let (lx, ly, lcx, lcy) = LIST_RECT;
        let list = list_with(&fx, 2);
        assert_eq!(fx.click(list, 50, control_row_y(lcy, 1) + 5), 1);
        assert_eq!(fx.focus(), list);
        assert_eq!(selection(&fx, list), 1);
        assert_eq!(fx.posted(), vec![(fx.client, WM_CONTROL, control_msg(ID_LIST, LN_SETFOCUS), list),
                                     (fx.client, WM_CONTROL, control_msg(ID_LIST, LN_SELECT), list)]);

        // The selected row is highlighted and its text drawn in the highlight colour.
        let fills = fx.drawn_fills(list, CONTROL_SELECT_COLOR);
        let row_y = ly + control_row_y(lcy, 1);
        assert_eq!(fills, vec![(lx + 1, row_y, lx + lcx - 1, row_y + CONTROL_LINE_H - 1)]);
        let texts = fx.drawn_texts(list);
        assert_eq!(texts[0].3, CONTROL_TEXT_COLOR);
        assert_eq!((texts[1].2.as_str(), texts[1].3), ("Item 1", CONTROL_SELECT_TEXT_COLOR));

        // A click on the empty third row or in the margins keeps the selection.
        fx.click(list, 50, control_row_y(lcy, 2) + 5);
        fx.click(list, 50, lcy - 1);
        fx.click(list, 50, 0);
        assert_eq!(selection(&fx, list), 1);
        assert_eq!(fx.posted(), vec![]);
    }

    #[test]
    fn listbox_enter_and_double_click_notify_ln_enter() {
        let fx = fixture();
        let lcy = LIST_RECT.3;
        let list = list_with(&fx, 2);
        let at = |row: usize| (50u32) | (((control_row_y(lcy, row) + 5) as u32) << 16);
        fx.click(list, 50, control_row_y(lcy, 0) + 5);
        fx.posted();
        fx.vkey(list, VK_ENTER);
        let posted = fx.posted();
        assert_eq!(posted[0], (fx.client, WM_CONTROL, control_msg(ID_LIST, LN_ENTER), list));
        assert_eq!((posted.len(), posted[1].1), (2, WM_CHAR), "then Enter goes on to the owner");

        // The second press of a double click selects, then reports LN_ENTER.
        assert_eq!(fx.send(list, WM_BUTTON1DBLCLK, at(1), 0), 1);
        assert_eq!(selection(&fx, list), 1);
        assert_eq!(fx.posted(), vec![(fx.client, WM_CONTROL, control_msg(ID_LIST, LN_SELECT), list),
                                     (fx.client, WM_CONTROL, control_msg(ID_LIST, LN_ENTER), list)]);
        fx.send(list, WM_BUTTON1UP, at(1), 0);
        // Not on an item: no LN_ENTER.
        fx.send(list, WM_BUTTON1DBLCLK, at(2), 0);
        assert_eq!(fx.posted(), vec![]);
        // A disabled list box ignores it.
        fx.with(list, |w| w.style |= WS_DISABLED);
        assert_eq!(fx.send(list, WM_BUTTON1DBLCLK, at(0), 0), 0);
        assert_eq!(fx.posted(), vec![]);
    }

    #[test]
    fn double_clicks_on_other_controls_act_as_presses() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "hello");
        let x = CONTROL_TEXT_INSET + system_width("he");
        fx.send(entry, WM_BUTTON1DBLCLK, x as u32 | (5 << 16), 0);
        assert_eq!(fx.focus(), entry);
        assert_eq!(fx.caret(entry), 2);
    }

    /// All selected items, walked with LM_QUERYSELECTION as an application does.
    fn all_selected(fx: &Fixture, list: u32) -> Vec<i32> {
        let mut out = Vec::new();
        let mut after = LIT_FIRST;
        loop {
            let i = fx.send(list, LM_QUERYSELECTION, after as u32 & 0xFFFF, 0) as i32;
            if i == LIT_NONE { return out; }
            out.push(i);
            after = i;
        }
    }

    fn press_row(fx: &Fixture, list: u32, row: usize, kc: u32) {
        let mp1 = 50 | (((control_row_y(LIST_RECT.3, row) + 5) as u32) << 16);
        fx.send(list, WM_BUTTON1DOWN, mp1, kc << 16);
        fx.send(list, WM_BUTTON1UP, mp1, kc << 16);
    }

    #[test]
    fn multiple_selection_list_toggles_items_by_click_and_space() {
        let fx = fixture();
        let list = list_with(&fx, 3);
        fx.with(list, |w| w.style |= LS_MULTIPLESEL);
        press_row(&fx, list, 0, 0);
        press_row(&fx, list, 2, 0);
        assert_eq!(all_selected(&fx, list), [0, 2]);
        press_row(&fx, list, 0, 0);
        assert_eq!(all_selected(&fx, list), [2]);
        let selects = fx.posted().iter().filter(|m| m.2 == control_msg(ID_LIST, LN_SELECT)).count();
        assert_eq!(selects, 3, "one LN_SELECT per toggle");

        // Arrow keys move only the cursor; Space toggles the cursor item.
        assert_eq!(fx.vkey(list, VK_UP), 1);
        // LIT_CURSOR asks for the cursor item, as a SHORT or a LONG.
        assert_eq!(fx.send(list, LM_QUERYSELECTION, LIT_CURSOR as u32 & 0xFFFF, 0), 0);
        assert_eq!(fx.send(list, LM_QUERYSELECTION, LIT_CURSOR as u32, 0), 0);
        assert_eq!(all_selected(&fx, list), [2]);
        assert_eq!(fx.vkey(list, VK_SPACE), 1);
        assert_eq!(all_selected(&fx, list), [0, 2]);
        // The typed character of the same key is swallowed, not toggled again.
        fx.type_text(list, " ");
        assert_eq!(all_selected(&fx, list), [0, 2]);
        assert!(fx.posted().iter().all(|m| m.1 != WM_CHAR), "Space is the list's own key");

        // LM_SELECTITEM adds and removes items without touching the others.
        assert_eq!(fx.send(list, LM_SELECTITEM, 1, 1), 1);
        assert_eq!(all_selected(&fx, list), [0, 1, 2]);
        assert_eq!(fx.send(list, LM_SELECTITEM, 0, 0), 1);
        assert_eq!(all_selected(&fx, list), [1, 2]);

        // Both selected rows are highlighted, and the cursor row is outlined.
        assert_eq!(fx.drawn_fills(list, CONTROL_SELECT_COLOR).len(), 2);
        let outlines = fx.paint(list).into_iter().filter(|op| matches!(op,
            DrawOp::Box { fill: false, color, .. } if *color == CONTROL_EMPHASIS_COLOR)).count();
        assert_eq!(outlines, 1);
        assert_eq!(fx.send(list, LM_SELECTITEM, LIT_NONE as u32, 0), 1);
        assert_eq!(all_selected(&fx, list), Vec::<i32>::new());
    }

    #[test]
    fn extended_selection_list_selects_ranges_with_shift_and_toggles_with_ctrl() {
        let fx = fixture();
        let list = list_with(&fx, 6);
        fx.with(list, |w| w.style |= LS_EXTENDEDSEL);
        press_row(&fx, list, 0, 0);
        press_row(&fx, list, 2, KC_SHIFT);
        assert_eq!(all_selected(&fx, list), [0, 1, 2]);
        press_row(&fx, list, 1, KC_CTRL);
        assert_eq!(all_selected(&fx, list), [0, 2]);
        press_row(&fx, list, 1, 0);
        assert_eq!(all_selected(&fx, list), [1]);
        // Shift+Down extends from the anchor and scrolls along.
        for _ in 0..4 {
            shift_vkey(&fx, list, VK_DOWN);
        }
        assert_eq!(all_selected(&fx, list), [1, 2, 3, 4, 5]);
        assert_eq!(fx.with(list, |w| w.top_row), 3);
        fx.vkey(list, VK_HOME);
        assert_eq!(all_selected(&fx, list), [0], "a plain move selects one item");
        // Dragging extends the range from the press.
        let at = |row: usize| 50 | (((control_row_y(LIST_RECT.3, row) + 5) as u32) << 16);
        fx.send(list, WM_BUTTON1DOWN, at(0), 0);
        fx.send(list, WM_MOUSEMOVE, at(2), 0);
        fx.send(list, WM_BUTTON1UP, at(2), 0);
        assert_eq!(all_selected(&fx, list), [0, 1, 2]);
    }

    #[test]
    fn listbox_arrow_keys_move_the_selection_and_scroll() {
        let fx = fixture();
        let list = list_with(&fx, 6);
        fx.click(list, 50, 0); // focus only: bottom margin
        assert_eq!(selection(&fx, list), LIT_NONE);
        let shown = |fx: &Fixture| fx.drawn_texts(list).into_iter().map(|t| t.2).collect::<Vec<_>>();

        assert_eq!(fx.vkey(list, VK_DOWN), 1);
        assert_eq!(selection(&fx, list), 0, "first Down selects the first item");
        fx.vkey(list, VK_UP);
        assert_eq!(selection(&fx, list), 0, "Up stops at the top");
        assert_eq!(fx.posted().iter().filter(|m| m.2 == control_msg(ID_LIST, LN_SELECT)).count(), 1);

        for _ in 0..3 { fx.vkey(list, VK_DOWN); }
        assert_eq!(selection(&fx, list), 3);
        assert_eq!(shown(&fx), ["Item 1", "Item 2", "Item 3"], "scrolled to keep it visible");
        assert_eq!(fx.drawn_fills(list, CONTROL_SELECT_COLOR).len(), 1);
        for _ in 0..9 { fx.vkey(list, VK_DOWN); }
        assert_eq!(selection(&fx, list), 5, "Down stops at the bottom");
        assert_eq!(shown(&fx), ["Item 3", "Item 4", "Item 5"]);
        fx.vkey(list, VK_HOME);
        assert_eq!(selection(&fx, list), 0);
        assert_eq!(shown(&fx), ["Item 0", "Item 1", "Item 2"]);
        fx.vkey(list, VK_END);
        assert_eq!(selection(&fx, list), 5);

        // A click maps through the scroll offset: top row is now item 3.
        fx.click(list, 50, control_row_y(LIST_RECT.3, 0) + 1);
        assert_eq!(selection(&fx, list), 3);

        // Other keys go to the owner; an empty list consumes arrows quietly.
        fx.posted();
        assert_eq!(fx.send(list, WM_CHAR, KC_CHAR, b'x' as u32), 0);
        assert_eq!(fx.posted(), vec![(fx.client, WM_CHAR, KC_CHAR, b'x' as u32)]);
        let empty = list_with(&fx, 0);
        assert_eq!(fx.vkey(empty, VK_DOWN), 1);
        assert_eq!(selection(&fx, empty), LIT_NONE);
        assert_eq!(fx.posted(), vec![]);
    }

    #[test]
    fn listbox_selection_follows_items_inserted_before_it() {
        let fx = fixture();
        let list = list_with(&fx, 3);
        fx.send(list, LM_SELECTITEM, 1, 1);
        // mp2 = 0: empty item text, no guest memory needed.
        assert_eq!(fx.send(list, LM_INSERTITEM, 0, 0), 0);
        assert_eq!(selection(&fx, list), 2, "same item, shifted down by the insert");
        assert_eq!(fx.send(list, LM_INSERTITEM, LIT_END, 0), 4);
        assert_eq!(selection(&fx, list), 2, "appending does not move it");
    }

    // ── selection and clipboard ──────────────────────────────────────────

    fn shift_vkey(fx: &Fixture, hwnd: u32, vk: u32) -> u32 {
        fx.send(hwnd, WM_CHAR, KC_VIRTUALKEY | KC_SCANCODE | KC_SHIFT, vk << WM_CHAR_VKEY_SHIFT)
    }

    fn sel(fx: &Fixture, hwnd: u32) -> (usize, usize) {
        fx.with(hwnd, |w| (w.edit_anchor, w.edit_caret))
    }

    /// x offset (relative to the control) of the caret position before `n`
    /// characters of `text`.
    fn x_of(text: &str, n: usize) -> i32 {
        CONTROL_TEXT_INSET + system_width(&text.chars().take(n).collect::<String>())
    }

    #[test]
    fn dragging_selects_and_the_release_lets_go() {
        let fx = fixture();
        let text = "drag over me";
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, text);
        fx.click(entry, x_of(text, 5), 12);
        assert_eq!(sel(&fx, entry), (5, 5));
        assert_eq!(fx.loader.shared.window_mgr.lock_or_recover().pressed_control, entry);
        let at = |x: i32| (x as u32 & 0xFFFF) | (12 << 16);
        fx.send(entry, WM_MOUSEMOVE, at(x_of(text, 9)), 0);
        assert_eq!(sel(&fx, entry), (5, 9));
        fx.send(entry, WM_MOUSEMOVE, at(x_of(text, 2)), 0);
        assert_eq!(sel(&fx, entry), (5, 2), "back past the anchor");
        fx.send(entry, WM_BUTTON1UP, at(x_of(text, 2)), 0);
        assert_eq!(fx.loader.shared.window_mgr.lock_or_recover().pressed_control, 0);
        // Moves after the release do nothing.
        fx.send(entry, WM_MOUSEMOVE, at(x_of(text, 11)), 0);
        assert_eq!(sel(&fx, entry), (5, 2));
        // A new click starts over.
        fx.click(entry, x_of(text, 7), 12);
        assert_eq!(sel(&fx, entry), (7, 7));
    }

    #[test]
    fn shift_arrows_select_and_typing_replaces_the_selection() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "abcdef");
        fx.click(entry, 5, 12);
        fx.vkey(entry, VK_HOME);
        for _ in 0..3 {
            shift_vkey(&fx, entry, VK_RIGHT);
        }
        assert_eq!(sel(&fx, entry), (0, 3));
        fx.type_text(entry, "Z");
        assert_eq!((fx.text(entry).as_str(), sel(&fx, entry)), ("Zdef", (1, 1)));
        assert!(fx.posted().iter().any(|&(h, m, mp1, _)| h == fx.client && m == WM_CONTROL
                                       && mp1 == (EN_CHANGE << 16) | ID_ENTRY));
    }

    #[test]
    fn the_selection_is_highlighted_while_focused() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "abcdef");
        fx.with(entry, |w| (w.edit_anchor, w.edit_caret) = (1, 4));
        assert!(fx.drawn_fills(entry, CONTROL_SELECT_COLOR).is_empty(), "not without the focus");
        fx.loader.set_focus_window(entry);
        let fills = fx.drawn_fills(entry, CONTROL_SELECT_COLOR);
        assert_eq!(fills.len(), 1);
        let (x, _, _, _) = ENTRY_RECT;
        let (x1, _, x2, _) = fills[0];
        assert_eq!((x1, x2 + 1), (x + x_of("abcdef", 1), x + x_of("abcdef", 4)));
        let texts = fx.drawn_texts(entry);
        assert!(texts.iter().any(|(_, _, t, c)| t == "bcd" && *c == CONTROL_SELECT_TEXT_COLOR));
    }

    #[test]
    fn mle_selection_spans_lines() {
        let fx = fixture();
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, "one\r\ntwo\r\nsix");
        fx.loader.set_focus_window(mle);
        fx.with(mle, |w| (w.edit_anchor, w.edit_caret) = (1, 7));
        // "ne" on the first line, "tw" on the second.
        assert_eq!(fx.drawn_fills(mle, CONTROL_SELECT_COLOR).len(), 2);
        let texts: Vec<String> = fx.drawn_texts(mle).into_iter()
            .filter(|(_, _, _, c)| *c == CONTROL_SELECT_TEXT_COLOR).map(|t| t.2).collect();
        assert_eq!(texts, ["ne", "tw"]);
    }

    #[test]
    fn clipboard_keys_and_messages_cut_copy_and_paste() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "copy me");
        fx.loader.set_focus_window(entry);
        let key = |mods: u32, vk: u32| fx.send(entry, WM_CHAR, KC_VIRTUALKEY | KC_SCANCODE | mods, vk << WM_CHAR_VKEY_SHIFT);
        let clip = || fx.loader.shared.window_mgr.lock_or_recover().clipboard_text.clone();

        fx.with(entry, |w| (w.edit_anchor, w.edit_caret) = (0, 4));
        assert_eq!(key(KC_CTRL, VK_INSERT), 1, "Ctrl+Insert copies");
        assert_eq!((clip().as_str(), fx.text(entry).as_str()), ("copy", "copy me"));
        assert_eq!(key(KC_SHIFT, VK_DELETE), 1, "Shift+Delete cuts");
        assert_eq!((clip().as_str(), fx.text(entry).as_str()), ("copy", " me"));
        fx.vkey(entry, VK_END);
        assert_eq!(key(KC_SHIFT, VK_INSERT), 1, "Shift+Insert pastes");
        assert_eq!(fx.text(entry), " mecopy");
        // The release of a chord is swallowed, not pasted again.
        assert_eq!(fx.send(entry, WM_CHAR, KC_VIRTUALKEY | KC_SHIFT | KC_KEYUP, VK_INSERT << WM_CHAR_VKEY_SHIFT), 1);
        assert_eq!(fx.text(entry), " mecopy");
        // Copying nothing leaves the clipboard alone.
        key(KC_CTRL, VK_INSERT);
        assert_eq!(clip(), "copy");

        // The same through messages; EM_SETSEL / EM_QUERYSEL.
        assert_eq!(fx.send(entry, EM_SETSEL, 1 | (3 << 16), 0), 1);
        assert_eq!(fx.send(entry, EM_QUERYSEL, 0, 0), 1 | (3 << 16));
        assert_eq!(fx.send(entry, EM_COPY, 0, 0), 0, "copying does not change the text");
        assert_eq!(clip(), "me");
        assert_eq!(fx.send(entry, EM_CLEAR, 0, 0), 1);
        assert_eq!(fx.text(entry), " copy");
        assert_eq!(fx.send(entry, EM_PASTE, 0, 0), 1);
        assert_eq!(fx.text(entry), " mecopy");
        assert_eq!(fx.send(entry, EM_SETSEL, 99, 0), 1);
        assert_eq!(fx.send(entry, EM_QUERYSEL, 0, 0), (7 << 16), "clamped, reported first..last");
    }

    #[test]
    fn mle_selection_messages_report_anchor_and_cursor() {
        let fx = fixture();
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, "one\r\ntwo");
        assert_eq!(fx.send(mle, MLM_SETSEL, 6, 1), 1);
        assert_eq!(fx.send(mle, MLM_QUERYSEL, MLFQS_ANCHORSEL, 0), 6);
        assert_eq!(fx.send(mle, MLM_QUERYSEL, MLFQS_CURSORSEL, 0), 1);
        assert_eq!(fx.send(mle, MLM_QUERYSEL, MLFQS_MINSEL, 0), 1);
        assert_eq!(fx.send(mle, MLM_QUERYSEL, MLFQS_MAXSEL, 0), 6);
        assert_eq!(fx.send(mle, MLM_QUERYSEL, MLFQS_MINMAXSEL, 0), 1 | (6 << 16));
        assert_eq!(fx.send(mle, MLM_CUT, 0, 0), 1);
        assert_eq!(fx.text(mle), "owo");
        assert_eq!(fx.loader.shared.window_mgr.lock_or_recover().clipboard_text, "ne\r\nt");
        assert!(fx.posted().iter().any(|&(_, m, mp1, _)| m == WM_CONTROL && mp1 >> 16 == MLN_CHANGE));
    }

    #[test]
    fn shift_click_extends_the_selection() {
        let fx = fixture();
        let text = "Hello world";
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, text);
        let y = ENTRY_RECT.3 / 2;
        fx.click(entry, x_of(text, 2), y);
        let at = |n: usize| (x_of(text, n) as u32 & 0xFFFF) | ((y as u32) << 16);
        fx.send(entry, WM_BUTTON1DOWN, at(7), KC_SHIFT << 16);
        fx.send(entry, WM_BUTTON1UP, at(7), KC_SHIFT << 16);
        assert_eq!(sel(&fx, entry), (2, 7), "anchor kept, caret at the pointer");
        // A plain click drops the selection.
        fx.send(entry, WM_BUTTON1DOWN, at(4), 0);
        assert_eq!(sel(&fx, entry), (4, 4));
    }

    // ── focus notifications ──────────────────────────────────────────────

    #[test]
    fn focus_changes_notify_the_owners() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "");
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, "");
        let list = fx.add(CLASS_LISTBOX, ID_LIST, LIST_RECT, "");
        let notes = |fx: &Fixture| fx.posted().into_iter()
            .filter(|m| m.1 == WM_CONTROL).map(|m| (m.0, m.2, m.3)).collect::<Vec<_>>();
        let c = fx.client;

        fx.click(entry, 5, 5);
        assert_eq!(notes(&fx), [(c, control_msg(ID_ENTRY, EN_SETFOCUS), entry)]);
        fx.click(entry, 5, 5);
        assert_eq!(notes(&fx), [], "no notification while the focus stays");
        fx.click(mle, 5, 5);
        assert_eq!(notes(&fx), [(c, control_msg(ID_ENTRY, EN_KILLFOCUS), entry),
                                (c, control_msg(ID_MLE, MLN_SETFOCUS), mle)]);
        fx.loader.set_focus_window(list);
        assert_eq!(notes(&fx), [(c, control_msg(ID_MLE, MLN_KILLFOCUS), mle),
                                (c, control_msg(ID_LIST, LN_SETFOCUS), list)]);
        fx.loader.set_focus_window(0);
        assert_eq!(notes(&fx), [(c, control_msg(ID_LIST, LN_KILLFOCUS), list)]);

        // Windows other than these controls have no focus notifications.
        let button = fx.add("WC_BUTTON", 7, (300, 10, 80, 30), "OK");
        fx.loader.set_focus_window(button);
        fx.loader.set_focus_window(fx.client);
        assert_eq!(notes(&fx), []);
    }

    // ── list box deletion ────────────────────────────────────────────────

    #[test]
    fn listbox_delete_item_keeps_the_selection_on_its_item() {
        let fx = fixture();
        let list = list_with(&fx, 5);
        fx.send(list, LM_SELECTITEM, 3, 1);
        fx.posted();
        assert_eq!(fx.send(list, LM_DELETEITEM, 1, 0), 4, "items left");
        assert_eq!(selection(&fx, list), 2, "the selected item moved up");
        assert_eq!(fx.with(list, |w| w.listbox_items.clone()), ["Item 0", "Item 2", "Item 3", "Item 4"]);
        assert_eq!(fx.send(list, LM_DELETEITEM, 3, 0), 3, "after the selection");
        assert_eq!(selection(&fx, list), 2);
        assert_eq!(fx.send(list, LM_DELETEITEM, 2, 0), 2, "the selected item");
        assert_eq!(selection(&fx, list), LIT_NONE);
        // Out of range: nothing removed.
        for bad in [2, 0xFFFF, 0x7FFF] {
            assert_eq!(fx.send(list, LM_DELETEITEM, bad, 0), 2, "index {bad:#x}");
        }
        assert_eq!(fx.posted(), [], "deleting sends no LN_SELECT");
        assert_eq!(fx.send(list, LM_QUERYITEMCOUNT, 0, 0), 2);
    }

    #[test]
    fn listbox_delete_all_empties_it() {
        let fx = fixture();
        let list = list_with(&fx, 6);
        fx.vkey(list, VK_END); // select and scroll
        assert_eq!(fx.send(list, LM_DELETEALL, 0, 0), 1);
        assert_eq!(fx.send(list, LM_QUERYITEMCOUNT, 0, 0), 0);
        assert_eq!(selection(&fx, list), LIT_NONE);
        assert_eq!(fx.with(list, |w| w.top_row), 0);
        assert!(fx.drawn_texts(list).is_empty());
        // Items added afterwards show from the top.
        fx.with(list, |w| w.listbox_items.push("New".into()));
        assert_eq!(fx.drawn_texts(list).into_iter().map(|t| t.2).collect::<Vec<_>>(), ["New"]);
    }

    // ── page keys ────────────────────────────────────────────────────────

    #[test]
    fn page_keys_move_a_list_selection_by_the_visible_rows_less_one() {
        let fx = fixture();
        let list = list_with(&fx, 8); // 3 rows visible: a page is 2 items
        fx.send(list, LM_SELECTITEM, 0, 1);
        assert_eq!(fx.vkey(list, VK_PAGEDOWN), 1);
        assert_eq!(selection(&fx, list), 2);
        fx.vkey(list, VK_PAGEDOWN);
        fx.vkey(list, VK_PAGEDOWN);
        fx.vkey(list, VK_PAGEDOWN);
        assert_eq!(selection(&fx, list), 7, "stops at the last item");
        fx.vkey(list, VK_PAGEUP);
        assert_eq!(selection(&fx, list), 5);
        for _ in 0..4 { fx.vkey(list, VK_PAGEUP); }
        assert_eq!(selection(&fx, list), 0);
    }

    #[test]
    fn page_keys_move_the_mle_caret_and_scroll_by_a_page() {
        let fx = fixture();
        let text = (0..10).map(|i| format!("Line {i}")).collect::<Vec<_>>().join("\r\n");
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, &text); // 3 rows: a page is 2 lines
        let line_of = |fx: &Fixture| fx.with(mle, |w| edit_buffer(w, true).caret_line().0);
        fx.paint(mle);
        assert_eq!(fx.vkey(mle, VK_PAGEDOWN), 1);
        assert_eq!(line_of(&fx), 2);
        assert_eq!(fx.with(mle, |w| w.top_row), 2, "the view scrolls with the caret");
        let shown = |fx: &Fixture| fx.drawn_texts(mle).into_iter().map(|t| t.2).collect::<Vec<_>>();
        assert_eq!(shown(&fx), ["Line 2", "Line 3", "Line 4"]);
        for _ in 0..6 { fx.vkey(mle, VK_PAGEDOWN); }
        assert_eq!(line_of(&fx), 9, "stops at the last line");
        assert_eq!(shown(&fx), ["Line 7", "Line 8", "Line 9"]);
        // Shift+PageUp selects back a page.
        shift_vkey(&fx, mle, VK_PAGEUP);
        assert_eq!(line_of(&fx), 7);
        assert!(fx.with(mle, |w| w.edit_anchor != w.edit_caret));
        // An entry field has no pages: the key goes to the owner.
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "x");
        fx.posted();
        assert_eq!(fx.vkey(entry, VK_PAGEDOWN), 0);
        assert!(fx.posted().iter().any(|m| m.0 == fx.client && m.1 == WM_CHAR));
    }

    // ── read-only ────────────────────────────────────────────────────────

    #[test]
    fn read_only_fields_select_and_copy_but_keep_their_text() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "Fixed");
        fx.with(entry, |w| w.style |= ES_READONLY);
        fx.click(entry, 1, 5);
        fx.posted();
        fx.type_text(entry, "xy");
        fx.vkey(entry, VK_DELETE);
        assert_eq!(fx.text(entry), "Fixed");
        assert_eq!(fx.caret(entry), 0, "typing does not move the caret");
        for _ in 0..3 { shift_vkey(&fx, entry, VK_RIGHT); }
        assert_eq!(sel(&fx, entry), (0, 3), "selection still works");
        fx.vkey(entry, VK_BACKSPACE);
        assert_eq!((fx.text(entry), sel(&fx, entry)), ("Fixed".to_string(), (0, 3)));
        assert_eq!(fx.send(entry, EM_COPY, 0, 0), 0, "copy changes no text");
        assert_eq!(fx.loader.shared.window_mgr.lock_or_recover().clipboard_text, "Fix");
        assert_eq!(fx.send(entry, EM_CUT, 0, 0), 0);
        assert_eq!(fx.send(entry, EM_PASTE, 0, 0), 0);
        assert_eq!(fx.send(entry, EM_CLEAR, 0, 0), 0);
        assert_eq!(fx.text(entry), "Fixed");
        assert!(fx.posted().iter().all(|m| m.1 != WM_CONTROL), "no EN_CHANGE");

        // EM_SETREADONLY returns the previous state; EM_QUERYREADONLY reports it.
        assert_eq!(fx.send(entry, EM_QUERYREADONLY, 0, 0), 1);
        assert_eq!(fx.send(entry, EM_SETREADONLY, 0, 0), 1);
        assert_eq!(fx.send(entry, EM_QUERYREADONLY, 0, 0), 0);
        assert_eq!(fx.send(entry, EM_SETREADONLY, 0, 0), 0);
        fx.type_text(entry, "!");
        assert_eq!(fx.text(entry), "!ed", "typing replaces the selected \"Fix\"");
    }

    #[test]
    fn read_only_mle_through_mlm_setreadonly() {
        let fx = fixture();
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, "ab\r\ncd");
        assert_eq!(fx.send(mle, MLM_QUERYREADONLY, 0, 0), 0);
        assert_eq!(fx.send(mle, MLM_SETREADONLY, 1, 0), 0);
        assert_eq!(fx.with(mle, |w| w.style & MLS_READONLY), MLS_READONLY);
        assert_eq!(fx.send(mle, MLM_QUERYREADONLY, 0, 0), 1);
        fx.click(mle, 1, MLE_RECT.3 - 5);
        fx.vkey(mle, VK_ENTER);
        fx.type_text(mle, "z");
        fx.vkey(mle, VK_DOWN);
        assert_eq!(fx.text(mle), "ab\r\ncd");
        assert_eq!(fx.caret(mle), 4, "movement still works");
        // The entry-field message does not apply to an MLE.
        assert_eq!(fx.send(mle, EM_SETREADONLY, 0, 0), 0);
        assert_eq!(fx.send(mle, MLM_QUERYREADONLY, 0, 0), 1);
    }

    // ── caret blink ──────────────────────────────────────────────────────

    #[test]
    fn tid_cursor_timers_blink_the_caret_and_input_shows_it() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "abc");
        let other = fx.add(CLASS_ENTRYFIELD, ID_ENTRY + 1, (10, 150, 200, 25), "x");
        fx.click(entry, 2, 5);
        let blink = |h: u32| fx.send(h, WM_TIMER, TID_CURSOR, 0);
        assert_eq!(fx.caret_bars(entry).len(), 1);
        blink(entry);
        assert!(fx.caret_bars(entry).is_empty(), "off phase");
        blink(entry);
        assert_eq!(fx.caret_bars(entry).len(), 1, "on again");
        blink(entry);
        fx.type_text(entry, "d");
        assert_eq!(fx.caret_bars(entry).len(), 1, "typing shows the caret at once");
        blink(entry);
        fx.vkey(entry, VK_LEFT);
        assert_eq!(fx.caret_bars(entry).len(), 1, "so does a movement key");
        // Only the focus window blinks; another keeps its caret on for later.
        blink(other);
        assert!(!fx.with(other, |w| w.caret_off));
        blink(entry);
        fx.click(other, 2, 5);
        blink(other);
        fx.click(entry, 2, 5);
        assert_eq!(fx.caret_bars(entry).len(), 1, "regaining the focus shows the caret");
        // Other timers are the owner's business, not the caret's.
        fx.send(entry, WM_TIMER, 1, 0);
        assert_eq!(fx.caret_bars(entry).len(), 1);
    }

    #[test]
    fn caret_blink_timers_go_to_a_focused_text_control_and_do_not_pile_up() {
        let fx = fixture();
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "abc");
        let list = list_with(&fx, 2);
        let post = || fx.loader.shared.window_mgr.lock_or_recover().post_caret_blink();
        assert_eq!(post(), None, "no focus window");
        fx.click(list, 50, 0);
        fx.posted();
        assert_eq!(post(), None, "a list box has no caret");
        fx.click(entry, 2, 5);
        fx.posted();
        assert_eq!(post(), Some(entry));
        assert_eq!(post(), Some(entry));
        assert_eq!(fx.posted(), vec![(entry, WM_TIMER, TID_CURSOR, 0)], "one pending at a time");
        fx.with(entry, |w| w.style |= WS_DISABLED);
        assert_eq!(post(), None, "a disabled field does not blink");
    }

    // ── word wrap ────────────────────────────────────────────────────────

    #[test]
    fn mle_word_wrap_paints_rows_that_fit_and_can_be_switched_off() {
        let fx = fixture();
        let (mx, _, mcx, mcy) = MLE_RECT;
        let text = "The quick brown fox jumps over the lazy dog";
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, text);
        let rows = |fx: &Fixture| fx.drawn_texts(mle).into_iter().map(|t| t.2).collect::<Vec<_>>();
        let one_clipped_row = |rows: Vec<String>| rows.len() == 1 && text.starts_with(rows[0].as_str());
        assert!(one_clipped_row(rows(&fx)), "no wrap by default");
        assert_eq!(fx.send(mle, MLM_QUERYWRAP, 0, 0), 0);

        assert_eq!(fx.send(mle, MLM_SETWRAP, 1, 0), 1);
        assert_eq!(fx.send(mle, MLM_QUERYWRAP, 0, 0), 1);
        let wrapped = rows(&fx);
        assert!(wrapped.len() > 1, "{wrapped:?}");
        assert_eq!(wrapped.concat(), text, "rows split the text without losing any of it");
        let room = mcx - 2 * CONTROL_TEXT_INSET - CARET_W;
        for row in &wrapped {
            assert!(system_width(row.trim_end()) <= room, "{row:?} is wider than {room}");
            assert!(row.ends_with(' ') || row.ends_with("dog"), "{row:?} breaks after a space");
        }
        let texts = fx.drawn_texts(mle);
        assert_eq!(texts[1].0, mx + CONTROL_TEXT_INSET, "every row starts at the left edge");
        assert_eq!(texts[1].1, MLE_RECT.1 + control_row_y(mcy, 1));

        // Down moves by rows; the caret is reported on its row.
        fx.click(mle, 1, control_row_y(mcy, 0) + 3);
        assert_eq!(fx.caret(mle), 0);
        fx.vkey(mle, VK_DOWN);
        assert_eq!(fx.caret(mle), wrapped[0].chars().count());
        assert_eq!(fx.with(mle, |w| edit_buffer(w, true).caret_line()), (1, 0));

        fx.send(mle, MLM_SETWRAP, 0, 0);
        assert!(one_clipped_row(rows(&fx)));
        // The entry-field and list-box classes ignore the MLE message.
        let entry = fx.add(CLASS_ENTRYFIELD, ID_ENTRY, ENTRY_RECT, "x");
        assert_eq!(fx.send(entry, MLM_SETWRAP, 1, 0), 0);
        assert_eq!(fx.with(entry, |w| w.style & MLS_WORDWRAP), 0);
    }

    #[test]
    fn mle_word_wrap_style_rewraps_as_text_is_typed() {
        let fx = fixture();
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, "");
        fx.with(mle, |w| w.style |= MLS_WORDWRAP);
        fx.click(mle, 1, control_row_y(MLE_RECT.3, 0) + 3);
        fx.type_text(mle, "aaaa bbbb cccc dddd eeee ffff gggg hhhh");
        let lines = fx.with(mle, |w| edit_buffer(w, true).lines().len());
        assert!(lines > 1, "typing past the edge starts a new row");
        assert_eq!(fx.with(mle, |w| w.edit_scroll), 0, "no horizontal scroll while wrapping");
        assert_eq!(fx.text(mle), "aaaa bbbb cccc dddd eeee ffff gggg hhhh", "no line breaks are inserted");
    }

    // ── dragging past the edge ───────────────────────────────────────────

    #[test]
    fn row_under_pointer_reaches_one_row_past_the_view() {
        let cy = MLE_RECT.3; // 3 rows
        assert_eq!(row_under_pointer(4, cy, control_row_y(cy, 0) + 1), Some(4));
        assert_eq!(row_under_pointer(4, cy, control_row_y(cy, 2) + 1), Some(6));
        assert_eq!(row_under_pointer(4, cy, cy - 1), Some(4), "top margin: first row");
        assert_eq!(row_under_pointer(4, cy, 0), Some(6), "bottom margin: last row");
        assert_eq!(row_under_pointer(4, cy, cy), Some(3), "above the control");
        assert_eq!(row_under_pointer(4, cy, -1), Some(7), "below the control");
        assert_eq!(row_under_pointer(0, cy, cy + 50), None, "nothing above the first row");
    }

    #[test]
    fn dragging_below_an_mle_scrolls_it_a_line_per_move() {
        let fx = fixture();
        let text = (0..8).map(|i| format!("L{i}")).collect::<Vec<_>>().join("\r\n");
        let mle = fx.add(CLASS_MLE, ID_MLE, MLE_RECT, &text);
        let line_of = |fx: &Fixture| fx.with(mle, |w| edit_buffer(w, true).caret_line().0);
        fx.click(mle, 2, control_row_y(MLE_RECT.3, 0) + 2);
        let below = (2u32 & 0xFFFF) | ((-5i32 as u32 & 0xFFFF) << 16);
        for want in 3..=7 {
            fx.send(mle, WM_MOUSEMOVE, below, 0);
            fx.paint(mle); // painting scrolls the view to the caret
            assert_eq!(line_of(&fx), want);
            assert_eq!(fx.with(mle, |w| w.top_row), want - 2);
        }
        fx.send(mle, WM_MOUSEMOVE, below, 0);
        assert_eq!(line_of(&fx), 7, "stops at the last line");
        // And back up above the control.
        let above = (2u32 & 0xFFFF) | ((MLE_RECT.3 as u32 + 3) << 16);
        fx.send(mle, WM_MOUSEMOVE, above, 0);
        fx.paint(mle);
        assert_eq!(line_of(&fx), 4);
        assert_eq!(fx.with(mle, |w| w.edit_anchor), 0, "the selection still starts at the click");
    }

    #[test]
    fn dragging_below_a_list_box_scrolls_it_an_item_per_move() {
        let fx = fixture();
        let list = list_with(&fx, 6);
        fx.click(list, 50, control_row_y(LIST_RECT.3, 0) + 2);
        assert_eq!(selection(&fx, list), 0);
        let below = 50 | ((-5i32 as u32 & 0xFFFF) << 16);
        for want in 3..=5 {
            fx.send(list, WM_MOUSEMOVE, below, 0);
            assert_eq!(selection(&fx, list), want);
        }
        fx.send(list, WM_MOUSEMOVE, below, 0);
        assert_eq!(selection(&fx, list), 5, "stops at the last item");
        fx.send(list, WM_BUTTON1UP, below, 0);
        let empty = fx.add(CLASS_LISTBOX, 9, (300, 60, 100, 54), "");
        fx.click(empty, 5, 5);
        assert_eq!(fx.send(empty, WM_MOUSEMOVE, below, 0), 1);
        assert_eq!(selection(&fx, empty), LIT_NONE, "an empty list has nothing to select");
    }
}
