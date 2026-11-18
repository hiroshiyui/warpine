// SPDX-License-Identifier: GPL-3.0-only

//! `WinMessageBox` as a PM dialog.
//!
//! The message box is an ordinary modal dialog built in memory: one
//! `WC_STATIC` per line of text and a row of push buttons, run by the same
//! modal loop as `WinDlgBox` (`ApiResult::DlgRunLoop`). It has no dialog
//! procedure; its messages go to `WinDefDlgProc` (see
//! [`Loader::procless_dialog_message`]), so a push button's `WM_COMMAND`
//! ends it with the button's `MBID_*` value. Other windows keep running
//! while it is open, and only its owner's frame is disabled.
//!
//! An `MB_ICON*` style puts a pictogram left of the text (painted with the
//! dialog face, [`Loader::paint_message_box_icon`]); `MB_HELP` adds a Help
//! push button (`BS_HELP`) whose `WM_HELP` goes to the box and leaves it
//! open; Close from the title bar ends the box with `MBID_CANCEL` whatever
//! its buttons.
//!
//! The layout — sizes, margins, where the text wraps — and the pictograms
//! are Warpine's own.

use super::constants::*;
use super::pm_edit::EditBuffer;
use super::{ApiResult, Loader, MutexExt, Painter};
use crate::gui::surface::{DrawOp, Pattern};

/// Margin around the text and the buttons inside the dialog border.
const MSGBOX_MARGIN: i32 = 12;
/// Widest line of text before it wraps.
const MSGBOX_TEXT_MAX_W: i32 = 400;
/// Narrowest content area (text or buttons).
const MSGBOX_MIN_CONTENT_W: i32 = 200;
/// Push-button size and the gap between buttons.
const MSGBOX_BUTTON_W: i32 = 80;
const MSGBOX_BUTTON_H: i32 = 26;
const MSGBOX_BUTTON_GAP: i32 = 8;
/// Width and height of the icon pictogram, and the gap between it and the
/// text.
pub const MSGBOX_ICON_SIZE: i32 = 32;
const MSGBOX_ICON_GAP: i32 = 12;
/// Mask of the button-set value in `flStyle` (`MB_OK` ... `MB_ENTERCANCEL`).
const MB_BUTTON_SET_MASK: u32 = 0x000F;
/// Mask of the icon choice in `flStyle` (`MB_ICONQUESTION` ... `MB_CUSTOMICON`).
const MB_ICON_MASK: u32 = 0x00F0;
/// Mask and shift of the default-button choice (`MB_DEFBUTTON1/2/3`).
const MB_DEFBUTTON_MASK: u32 = 0x0300;
const MB_DEFBUTTON_SHIFT: u32 = 8;

/// The buttons of a message box with style `style`, left to right: the
/// `MBID_*` value each returns and its label. `MB_HELP` adds Help last
/// (`MBID_HELP`; it sends `WM_HELP` instead of ending the box).
pub fn buttons(style: u32) -> Vec<(u32, &'static str)> {
    let set: &[(u32, &'static str)] = match style & MB_BUTTON_SET_MASK {
        MB_OKCANCEL => &[(MBID_OK, "OK"), (MBID_CANCEL, "Cancel")],
        MB_RETRYCANCEL => &[(MBID_RETRY, "Retry"), (MBID_CANCEL, "Cancel")],
        MB_ABORTRETRYIGNORE => &[(MBID_ABORT, "Abort"), (MBID_RETRY, "Retry"), (MBID_IGNORE, "Ignore")],
        MB_YESNO => &[(MBID_YES, "Yes"), (MBID_NO, "No")],
        MB_YESNOCANCEL => &[(MBID_YES, "Yes"), (MBID_NO, "No"), (MBID_CANCEL, "Cancel")],
        MB_CANCEL => &[(MBID_CANCEL, "Cancel")],
        MB_ENTER => &[(MBID_ENTER, "Enter")],
        MB_ENTERCANCEL => &[(MBID_ENTER, "Enter"), (MBID_CANCEL, "Cancel")],
        // MB_OK and values outside the documented set.
        _ => &[(MBID_OK, "OK")],
    };
    let mut v = set.to_vec();
    if style & MB_HELP != 0 {
        v.push((MBID_HELP, "Help"));
    }
    v
}

/// Index of the default button (`MB_DEFBUTTON1/2/3`) among `count`
/// buttons; a choice past the last button falls back to the first.
pub fn default_button(style: u32, count: usize) -> usize {
    let i = ((style & MB_DEFBUTTON_MASK) >> MB_DEFBUTTON_SHIFT) as usize;
    if i < count { i } else { 0 }
}

/// The icon a message box with style `style` shows, as the system pointer
/// of that icon (`SPTR_ICON*`); 0 for `MB_NOICON`, `MB_CUSTOMICON` (no
/// application icon reaches `WinMessageBox`) and undocumented values.
pub fn icon(style: u32) -> u32 {
    match style & MB_ICON_MASK {
        MB_ICONHAND => SPTR_ICONERROR,
        MB_ICONASTERISK => SPTR_ICONINFORMATION,
        MB_ICONEXCLAMATION => SPTR_ICONWARNING,
        MB_ICONQUESTION => SPTR_ICONQUESTION,
        _ => 0,
    }
}

/// Bottom-left corner of the icon in a message box `cy` high (dialog
/// coordinates, border included): the top-left corner of the content.
pub fn icon_origin(cy: i32) -> (i32, i32) {
    let border = CHROME_BORDER_W;
    (border + MSGBOX_MARGIN, cy - CHROME_TITLE_H - border - MSGBOX_MARGIN - MSGBOX_ICON_SIZE)
}

/// Where everything goes in a message box. Coordinates are PM's: relative
/// to the dialog's bottom-left corner, border included, y up.
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    pub cx: i32,
    pub cy: i32,
    /// The icon (`SPTR_ICON*`) and its bottom-left corner;
    /// `MSGBOX_ICON_SIZE` square.
    pub icon: Option<(u32, i32, i32)>,
    /// Each line of text and the bottom-left of its row.
    pub rows: Vec<(String, i32, i32)>,
    /// Each button: `(MBID_*, label, x, y)`; all `MSGBOX_BUTTON_W` ×
    /// `MSGBOX_BUTTON_H`.
    pub buttons: Vec<(u32, &'static str, i32, i32)>,
}

/// Lay out a message box with `text`, the icon and the buttons of `style`,
/// text measured with `measure` in rows `line_h` high. The text breaks at
/// its line breaks and wraps at word boundaries past `MSGBOX_TEXT_MAX_W`;
/// at most `max_rows` rows are kept (the rest is cut). An icon goes in the
/// top-left corner with the text to its right, centred against it when
/// shorter. Buttons are centred below.
pub fn layout(text: &str, style: u32, measure: impl Fn(&str) -> i32 + Copy + 'static,
              line_h: i32, max_rows: usize) -> Layout {
    let mut buf = EditBuffer::new(text, 0, true);
    buf.set_wrap(MSGBOX_TEXT_MAX_W, measure);
    let lines: Vec<String> = buf.lines().into_iter().take(max_rows.max(1))
        .map(|r| buf.slice(r).trim_end().to_string()).collect();
    let text_w = lines.iter().map(|l| measure(l)).max().unwrap_or(0);
    let set = buttons(style);
    let n = set.len() as i32;
    let buttons_w = n * MSGBOX_BUTTON_W + (n - 1) * MSGBOX_BUTTON_GAP;
    let border = CHROME_BORDER_W;
    let sptr = icon(style);
    let icon_w = if sptr != 0 { MSGBOX_ICON_SIZE + MSGBOX_ICON_GAP } else { 0 };
    let content_w = (icon_w + text_w).max(buttons_w).max(MSGBOX_MIN_CONTENT_W);
    let cx = content_w + 2 * (MSGBOX_MARGIN + border);
    let text_bottom = border + 2 * MSGBOX_MARGIN + MSGBOX_BUTTON_H;
    let text_h = lines.len() as i32 * line_h;
    let content_h = if sptr != 0 { text_h.max(MSGBOX_ICON_SIZE) } else { text_h };
    let cy = text_bottom + content_h + MSGBOX_MARGIN + CHROME_TITLE_H + border;
    let text_top = text_bottom + content_h - (content_h - text_h) / 2;
    let text_x = border + MSGBOX_MARGIN + icon_w;
    let rows = lines.into_iter().enumerate()
        .map(|(i, l)| (l, text_x, text_top - (i as i32 + 1) * line_h))
        .collect();
    let icon = (sptr != 0).then(|| {
        let (x, y) = icon_origin(cy);
        (sptr, x, y)
    });
    let left = (cx - buttons_w) / 2;
    let buttons = set.iter().enumerate()
        .map(|(i, &(id, label))| (id, label, left + i as i32 * (MSGBOX_BUTTON_W + MSGBOX_BUTTON_GAP),
                                  border + MSGBOX_MARGIN))
        .collect();
    Layout { cx, cy, icon, rows, buttons }
}

// Pictogram colours (Warpine's own).
const ICON_RED: u32 = 0x00D0_1010;
const ICON_RED_DARK: u32 = 0x0080_0000;
const ICON_BLUE: u32 = 0x0020_50C0;
const ICON_BLUE_DARK: u32 = 0x0010_2870;
const ICON_YELLOW: u32 = 0x00F0_C820;
const ICON_YELLOW_DARK: u32 = 0x0080_6000;
const ICON_WHITE: u32 = 0x00FF_FFFF;
const ICON_BLACK: u32 = 0x0000_0000;
const ICON_GREY: u32 = 0x0080_8080;
const ICON_TAN: u32 = 0x00E8_C060;
const ICON_TAN_DARK: u32 = 0x0080_6010;

/// The marks drawn on the information, warning and question pictograms
/// (`#` = set), Warpine's own.
const GLYPH_I: [&str; 17] = [
    "....###...", "...#####..", "....###...", "..........", "..........",
    "..#####...", "....###...", "....###...", "....###...", "....###...",
    "....###...", "....###...", "....###...", "....###...", "....###...",
    "..#######.", "..#######.",
];
const GLYPH_EXCLAMATION: [&str; 15] = [
    "...####...", "...####...", "...####...", "...####...", "....##....",
    "....##....", "....##....", "....##....", "....##....", "..........",
    "..........", "...####...", "...####...", "...####...", "..........",
];
const GLYPH_QUESTION: [&str; 17] = [
    "..######..", ".########.", "###....###", "##......##", "........##",
    ".......###", "......###.", ".....###..", "....###...", "....##....",
    "....##....", "....##....", "..........", "..........", "....##....",
    "...####...", "....##....",
];

/// The `MSGBOX_ICON_SIZE`² pels of the pictogram for `sptr`
/// (`SPTR_ICON*`, and the object icons `SPTR_APPICON`, `SPTR_FILE`,
/// `SPTR_FOLDER`, `SPTR_MULTFILE`, `SPTR_PROGRAM`), top row first; `None`
/// = transparent. `None` overall for anything else. Error: a red circle
/// with a red line across it; information: a white "i" on a blue disc;
/// warning: a black "!" on a yellow triangle; question: a white "?" on a
/// blue disc; see [`object_pels`] for the others.
pub fn icon_pels(sptr: u32) -> Option<Vec<Option<u32>>> {
    if let Some(pels) = object_pels(sptr) {
        return Some(pels);
    }
    let n = MSGBOX_ICON_SIZE;
    let mut pels = vec![None; (n * n) as usize];
    let centre = n as f32 / 2.0;
    // Distance of pel (x, y)'s centre from the icon's centre.
    let dist = |x: i32, y: i32| ((x as f32 + 0.5 - centre).powi(2) + (y as f32 + 0.5 - centre).powi(2)).sqrt();
    let outer = centre - 0.5;
    let disc = |pels: &mut [Option<u32>], fill: u32, edge: u32| {
        for y in 0..n {
            for x in 0..n {
                let d = dist(x, y);
                if d <= outer {
                    pels[(y * n + x) as usize] = Some(if d > outer - 1.5 { edge } else { fill });
                }
            }
        }
    };
    let glyph = |pels: &mut [Option<u32>], rows: &[&str], top: i32, color: u32| {
        let left = (n - rows[0].len() as i32) / 2;
        for (dy, row) in rows.iter().enumerate() {
            for (dx, c) in row.bytes().enumerate() {
                if c == b'#' {
                    pels[((top + dy as i32) * n + left + dx as i32) as usize] = Some(color);
                }
            }
        }
    };
    match sptr {
        SPTR_ICONERROR => {
            // A red ring (white inside) crossed by a red bar from the top
            // left to the bottom right.
            disc(&mut pels, ICON_RED, ICON_RED_DARK);
            let inner = outer - 5.0;
            for y in 0..n {
                for x in 0..n {
                    let across = ((x - y) as f32).abs() / std::f32::consts::SQRT_2;
                    if dist(x, y) <= inner && across > 2.5 {
                        pels[(y * n + x) as usize] = Some(ICON_WHITE);
                    }
                }
            }
        }
        SPTR_ICONINFORMATION => {
            disc(&mut pels, ICON_BLUE, ICON_BLUE_DARK);
            glyph(&mut pels, &GLYPH_I, 7, ICON_WHITE);
        }
        SPTR_ICONQUESTION => {
            disc(&mut pels, ICON_BLUE, ICON_BLUE_DARK);
            glyph(&mut pels, &GLYPH_QUESTION, 7, ICON_WHITE);
        }
        SPTR_ICONWARNING => {
            // A triangle standing on the bottom row, apex in the top row.
            for y in 0..n {
                let half = (y as f32 + 0.5) / 2.0 + 0.5;
                for x in 0..n {
                    let off = (x as f32 + 0.5 - centre).abs();
                    if off <= half {
                        let edge = off > half - 1.5 || y == n - 1;
                        pels[(y * n + x) as usize] = Some(if edge { ICON_YELLOW_DARK } else { ICON_YELLOW });
                    }
                }
            }
            glyph(&mut pels, &GLYPH_EXCLAMATION, 12, ICON_BLACK);
        }
        _ => return None,
    }
    Some(pels)
}

/// Warpine's own pictograms of the object icons, top row first: a file is
/// a white page with a folded corner and grey lines, several files two
/// pages, a folder a tan folder with a tab, a program (and the default
/// application icon) a window with a blue title bar.
fn object_pels(sptr: u32) -> Option<Vec<Option<u32>>> {
    let n = MSGBOX_ICON_SIZE;
    let mut pels = vec![None; (n * n) as usize];
    // Fill the inclusive box (x1, y1)-(x2, y2), outlined in `edge`.
    let boxed = |pels: &mut [Option<u32>], (x1, y1, x2, y2): (i32, i32, i32, i32), fill: u32, edge: u32| {
        for y in y1..=y2 {
            for x in x1..=x2 {
                let on_edge = x == x1 || x == x2 || y == y1 || y == y2;
                pels[(y * n + x) as usize] = Some(if on_edge { edge } else { fill });
            }
        }
    };
    // A page at (x1, y1)-(x2, y2) with its top-right corner folded.
    let page = |pels: &mut [Option<u32>], (x1, y1, x2, y2): (i32, i32, i32, i32)| {
        boxed(pels, (x1, y1, x2, y2), ICON_WHITE, ICON_BLACK);
        let fold = 6;
        for d in 0..fold {
            for x in x2 - fold + 1 + d..=x2 {
                pels[((y1 + d) * n + x) as usize] = None;
            }
            pels[((y1 + d) * n + x2 - fold + 1 + d) as usize] = Some(ICON_BLACK);
            pels[((y1 + fold) * n + x2 - d) as usize] = Some(ICON_BLACK);
            pels[((y1 + d) * n + x2 - fold + 1) as usize] = Some(ICON_BLACK);
        }
        for y in (y1 + fold + 3..y2 - 2).step_by(3) {
            for x in x1 + 3..x2 - 2 {
                pels[(y * n + x) as usize] = Some(ICON_GREY);
            }
        }
    };
    match sptr {
        SPTR_FILE => page(&mut pels, (7, 2, 24, 29)),
        SPTR_MULTFILE => {
            page(&mut pels, (11, 1, 27, 24));
            page(&mut pels, (4, 7, 20, 30));
        }
        SPTR_FOLDER => {
            boxed(&mut pels, (3, 6, 13, 10), ICON_TAN, ICON_TAN_DARK);
            boxed(&mut pels, (3, 9, 28, 26), ICON_TAN, ICON_TAN_DARK);
        }
        SPTR_PROGRAM | SPTR_APPICON => {
            boxed(&mut pels, (3, 5, 28, 26), ICON_WHITE, ICON_BLACK);
            boxed(&mut pels, (3, 5, 28, 10), ICON_BLUE, ICON_BLACK);
        }
        _ => return None,
    }
    Some(pels)
}

impl Loader {
    /// WinMessageBox: create the message box as a modal dialog owned by
    /// `owner`, centred on the desktop, with the focus on its default button,
    /// and run it (`ApiResult::DlgRunLoop`; the result is the `MBID_*` of
    /// the button pressed). `MBID_ERROR` when the thread has no message
    /// queue.
    pub(super) fn message_box(&self, vcpu_id: u32, owner: u32, caption: String, text: &str,
                              style: u32) -> ApiResult {
        let line_h = super::CONTROL_LINE_H;
        let (dw, dh) = desktop_size();
        let max_rows = ((dh as i32 - 2 * MSGBOX_BUTTON_H - CHROME_TITLE_H) / line_h).max(1) as usize;
        // The text is drawn in the CJK face of the thread's queue codepage
        // (the box's controls belong to that queue); measure it in the same.
        let face = super::pm_controls::system_font(self.thread_codepage(vcpu_id));
        let lay = layout(text, style, super::pm_controls::measure_in(face), line_h, max_rows);
        let (hwnd_dlg, hmq, focus) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(hmq) = wm.thread_queue(vcpu_id) else {
                return ApiResult::Normal(MBID_ERROR);
            };
            let h = wm.create_window(super::pm_types::WindowManager::DIALOG_CLASS.to_string(), HWND_DESKTOP, hmq);
            if let Some(w) = wm.get_window_mut(h) {
                w.owner = owner;
                w.visible = true;
                w.text = caption;
                w.frame_flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_DLGBORDER;
                w.cx = lay.cx;
                w.cy = lay.cy;
                w.x = (dw as i32 - lay.cx) / 2;
                w.y = (dh as i32 - lay.cy) / 2;
                w.msgbox_icon = lay.icon.map_or(0, |i| i.0);
                w.message_box = true;
            }
            let add = |wm: &mut super::pm_types::WindowManager, class: &str, text: &str,
                           (x, y, cx, cy): (i32, i32, i32, i32), id: u32, style: u32| {
                let c = wm.create_window(class.to_string(), h, hmq);
                if let Some(w) = wm.get_window_mut(c) {
                    (w.x, w.y, w.cx, w.cy) = (x, y, cx, cy);
                    w.text = text.to_string();
                    w.id = id;
                    w.style = style | WS_VISIBLE;
                    w.visible = true;
                    w.owner = h;
                }
                c
            };
            for (line, x, y) in &lay.rows {
                // From the row's left edge to the right margin.
                let text_w = lay.cx - CHROME_BORDER_W - MSGBOX_MARGIN - x;
                // Static text has no id of its own (-1, as in templates).
                add(&mut wm, "WC_STATIC", line, (*x, *y, text_w, line_h), 0xFFFF, SS_TEXT);
            }
            let default = default_button(style, lay.buttons.len());
            let mut focus = 0;
            for (i, &(id, label, x, y)) in lay.buttons.iter().enumerate() {
                let mut bstyle = BS_PUSHBUTTON | WS_TABSTOP;
                if i == 0 { bstyle |= WS_GROUP; }
                if i == default { bstyle |= BS_DEFAULT; }
                // Help sends WM_HELP to the box instead of a command.
                if id == MBID_HELP { bstyle |= BS_HELP; }
                let b = add(&mut wm, "WC_BUTTON", label, (x, y, MSGBOX_BUTTON_W, MSGBOX_BUTTON_H), id, bstyle);
                if i == default { focus = b; }
            }
            wm.z_push_top(h);
            wm.focused_hwnd = h;
            wm.focus_hwnd = focus;
            wm.begin_modal_dialog(h, true);
            (h, hmq, focus)
        };
        self.queue_dialog_start(hwnd_dlg, hmq, focus, 0);
        ApiResult::DlgRunLoop { dlg_proc: 0, hwnd_dlg, hmq }
    }

    /// Paint a message box's icon (`OS2Window::msgbox_icon`) onto the face
    /// of dialog `dlg`, whose painter is `d`; nothing for other dialogs.
    pub(super) fn paint_message_box_icon(&self, d: &Painter, dlg: u32) {
        let (sptr, cy, border) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window(dlg) else { return };
            (w.msgbox_icon, w.cy, frame_border_width(w.frame_flags))
        };
        let Some(pels) = icon_pels(sptr) else { return };
        let (x, y) = icon_origin(cy);
        let n = MSGBOX_ICON_SIZE as usize;
        // The blit takes the bottom row first.
        let src = pels.chunks(n).rev().flatten().copied().collect();
        // The surface starts inside the left border (as the controls do).
        d.op(DrawOp::Blit {
            x: x - border, y, w: n as u32, h: n as u32, src: Some(src),
            rop: ROP_SRCCOPY as u8, pattern: Pattern::solid(0),
        });
    }

    /// A message for a dialog without a dialog procedure (a message box):
    /// `WinDefDlgProc`, except that
    /// - a command ends it only when one of its controls has that id (a push
    ///   button), so Esc (`WM_COMMAND(DID_CANCEL)`) ends a message box only
    ///   when it has a Cancel button (TODO(unverified): what PM does on Esc
    ///   in a box without one);
    /// - closing it (`WM_CLOSE`: the close button, the system menu's Close,
    ///   Alt+F4) ends it with `MBID_CANCEL` whether or not it has a Cancel
    ///   button, as documented for `WinMessageBox`;
    /// - `WM_HELP` (its Help button) leaves it open and goes on to the parent
    ///   unless that is the desktop, as the default window procedure does —
    ///   and a message box's parent is the desktop.
    pub(super) fn procless_dialog_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> ApiResult {
        match msg {
            WM_COMMAND => {
                let id = mp1 & 0xFFFF;
                let known = self.shared.window_mgr.lock_or_recover().find_child_by_id(hwnd, id).is_some();
                if known {
                    self.dismiss_dialog(hwnd, id);
                }
                ApiResult::Normal(0)
            }
            WM_CLOSE => {
                self.dismiss_dialog(hwnd, MBID_CANCEL);
                ApiResult::Normal(0)
            }
            WM_HELP => {
                let parent = self.shared.window_mgr.lock_or_recover().get_window(hwnd).map_or(0, |w| w.parent);
                if parent != 0 && parent != HWND_DESKTOP {
                    // TODO(unverified): PM sends it; Warpine posts it.
                    self.post_to_window(parent, WM_HELP, mp1, mp2);
                }
                ApiResult::Normal(0)
            }
            _ => self.def_dlg_proc(hwnd, msg, mp1, mp2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character 10 px wide.
    fn mono(s: &str) -> i32 {
        s.chars().count() as i32 * 10
    }

    #[test]
    fn button_sets_follow_the_style() {
        let ids = |style| buttons(style).iter().map(|b| b.0).collect::<Vec<_>>();
        assert_eq!(ids(MB_OK), [MBID_OK]);
        assert_eq!(ids(MB_OKCANCEL | MB_ICONHAND), [MBID_OK, MBID_CANCEL]);
        assert_eq!(ids(MB_ABORTRETRYIGNORE), [MBID_ABORT, MBID_RETRY, MBID_IGNORE]);
        assert_eq!(ids(MB_RETRYCANCEL | MB_ICONEXCLAMATION), [MBID_RETRY, MBID_CANCEL], "icon bits do not matter");
        assert_eq!(ids(MB_YESNO), [MBID_YES, MBID_NO]);
        assert_eq!(ids(MB_CANCEL), [MBID_CANCEL]);
        assert_eq!(ids(MB_ENTER), [MBID_ENTER]);
        assert_eq!(ids(MB_YESNOCANCEL | MB_DEFBUTTON2), [MBID_YES, MBID_NO, MBID_CANCEL]);
        assert_eq!(ids(MB_ENTERCANCEL), [MBID_ENTER, MBID_CANCEL]);
        assert_eq!(ids(0x000F), [MBID_OK], "an undocumented set falls back to OK");
    }

    #[test]
    fn mb_help_adds_a_help_button_last() {
        assert_eq!(buttons(MB_OK | MB_HELP), [(MBID_OK, "OK"), (MBID_HELP, "Help")]);
        assert_eq!(buttons(MB_YESNOCANCEL | MB_HELP).last(), Some(&(MBID_HELP, "Help")));
        assert_eq!(buttons(MB_YESNOCANCEL | MB_HELP).len(), 4);
        // MB_DEFBUTTON counts it like the other buttons.
        assert_eq!(default_button(MB_OK | MB_HELP | MB_DEFBUTTON2, 2), 1);
    }

    #[test]
    fn the_default_button_is_chosen_by_mb_defbutton() {
        assert_eq!(default_button(MB_YESNOCANCEL, 3), 0);
        assert_eq!(default_button(MB_YESNOCANCEL | MB_DEFBUTTON2, 3), 1);
        assert_eq!(default_button(MB_YESNOCANCEL | MB_DEFBUTTON3, 3), 2);
        assert_eq!(default_button(MB_OK | MB_DEFBUTTON3, 1), 0, "past the last button");
    }

    #[test]
    fn icons_follow_the_style_and_its_aliases() {
        // The aliases of <pmwin.h>.
        assert_eq!((MB_ERROR, MB_CRITICAL, MB_CUACRITICAL), (MB_ICONHAND, MB_ICONHAND, MB_ICONHAND));
        assert_eq!((MB_WARNING, MB_CUAWARNING), (MB_ICONEXCLAMATION, MB_ICONEXCLAMATION));
        assert_eq!((MB_INFORMATION, MB_QUERY), (MB_ICONASTERISK, MB_ICONQUESTION));
        assert_eq!(MB_CUANOTIFICATION, MB_NOICON);

        assert_eq!(icon(MB_OK | MB_ERROR), SPTR_ICONERROR);
        assert_eq!(icon(MB_YESNO | MB_ICONHAND | MB_DEFBUTTON2 | MB_HELP), SPTR_ICONERROR);
        assert_eq!(icon(MB_INFORMATION), SPTR_ICONINFORMATION);
        assert_eq!(icon(MB_ICONASTERISK), SPTR_ICONINFORMATION);
        assert_eq!(icon(MB_WARNING), SPTR_ICONWARNING);
        assert_eq!(icon(MB_ICONEXCLAMATION), SPTR_ICONWARNING);
        assert_eq!(icon(MB_QUERY), SPTR_ICONQUESTION);
        assert_eq!(icon(MB_NOICON | MB_OKCANCEL), 0);
        assert_eq!(icon(MB_CUSTOMICON), 0, "no application icon in WinMessageBox");
        assert_eq!(icon(0x0050), 0, "undocumented");
    }

    #[test]
    fn short_text_gets_the_minimum_width_and_centred_buttons() {
        let lay = layout("Saved.", MB_YESNO, mono, 16, 50);
        let border = CHROME_BORDER_W;
        assert_eq!(lay.icon, None);
        assert_eq!(lay.cx, MSGBOX_MIN_CONTENT_W + 2 * (MSGBOX_MARGIN + border));
        assert_eq!(lay.rows, vec![("Saved.".to_string(), border + MSGBOX_MARGIN,
                                   border + 2 * MSGBOX_MARGIN + MSGBOX_BUTTON_H)]);
        let (yes, no) = (&lay.buttons[0], &lay.buttons[1]);
        assert_eq!((yes.0, no.0), (MBID_YES, MBID_NO));
        assert_eq!(no.2 - yes.2, MSGBOX_BUTTON_W + MSGBOX_BUTTON_GAP);
        let right_gap = lay.cx - (no.2 + MSGBOX_BUTTON_W);
        assert!((yes.2 - right_gap).abs() <= 1, "centred: {yes:?} {right_gap}");
        assert_eq!(yes.3, border + MSGBOX_MARGIN);
        // Title bar, text, buttons and margins stack up to the height.
        assert_eq!(lay.cy, border + 3 * MSGBOX_MARGIN + MSGBOX_BUTTON_H + 16 + CHROME_TITLE_H + border);
    }

    #[test]
    fn an_icon_goes_top_left_with_the_text_right_of_it() {
        let border = CHROME_BORDER_W;
        let plain = layout("Disk full.", MB_OK, mono, 16, 50);
        let lay = layout("Disk full.", MB_OK | MB_ERROR, mono, 16, 50);
        let (sptr, ix, iy) = lay.icon.unwrap();
        assert_eq!(sptr, SPTR_ICONERROR);
        assert_eq!((ix, iy), icon_origin(lay.cy));
        assert_eq!(ix, border + MSGBOX_MARGIN);
        // The icon's top is a margin below the title bar.
        assert_eq!(iy + MSGBOX_ICON_SIZE, lay.cy - CHROME_TITLE_H - border - MSGBOX_MARGIN);
        // and a margin above the buttons.
        assert_eq!(iy, lay.buttons[0].3 + MSGBOX_BUTTON_H + MSGBOX_MARGIN);
        // One row, taller with the icon, centred against it.
        assert_eq!(lay.cy - plain.cy, MSGBOX_ICON_SIZE - 16);
        let (_, tx, ty) = &lay.rows[0];
        assert_eq!(*tx, ix + MSGBOX_ICON_SIZE + MSGBOX_ICON_GAP);
        assert_eq!(*ty, iy + (MSGBOX_ICON_SIZE - 16) / 2);

        // Long text: the icon widens the box and the rows start at its top.
        let long = "word ".repeat(30);
        let lay = layout(&format!("{long}\n{long}"), MB_OK | MB_WARNING, mono, 16, 50);
        let widest = lay.rows.iter().map(|r| mono(&r.0)).max().unwrap();
        assert_eq!(lay.cx, MSGBOX_ICON_SIZE + MSGBOX_ICON_GAP + widest + 2 * (MSGBOX_MARGIN + border));
        let (_, _, iy) = lay.icon.unwrap();
        assert_eq!(lay.rows[0].2 + 16, iy + MSGBOX_ICON_SIZE, "first row level with the icon's top");
        assert!(lay.rows.iter().all(|r| r.1 == border + MSGBOX_MARGIN + MSGBOX_ICON_SIZE + MSGBOX_ICON_GAP));
    }

    #[test]
    fn each_icon_has_its_own_pictogram() {
        let n = MSGBOX_ICON_SIZE as usize;
        let at = |p: &[Option<u32>], x: usize, y: usize| p[y * n + x];
        let all: Vec<Vec<Option<u32>>> = [SPTR_ICONERROR, SPTR_ICONINFORMATION, SPTR_ICONWARNING, SPTR_ICONQUESTION]
            .iter().map(|&s| icon_pels(s).expect("a pictogram")).collect();
        for p in &all {
            assert_eq!(p.len(), n * n);
            assert_eq!(at(p, 0, 0), None, "transparent corners");
            assert_eq!(at(p, n - 1, 0), None);
        }
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i], all[j], "pictograms {i} and {j} differ");
            }
        }
        let [error, info, warning, question] = &all[..] else { unreachable!() };
        // Error: red ring, red bar through the centre, white either side of it.
        assert_eq!(at(error, 16, 16), Some(ICON_RED));
        assert_eq!(at(error, 8, 8), Some(ICON_RED), "the bar runs top left to bottom right");
        assert_eq!(at(error, 22, 10), Some(ICON_WHITE));
        assert_eq!(at(error, 10, 22), Some(ICON_WHITE));
        assert_eq!(at(error, 16, 2), Some(ICON_RED), "the ring");
        // Information and question: white marks on a blue disc.
        assert_eq!(at(info, 16, 7), Some(ICON_WHITE), "the dot of the i");
        assert_eq!(at(info, 16, 18), Some(ICON_WHITE), "its stem");
        assert_eq!(at(info, 8, 16), Some(ICON_BLUE));
        assert_eq!(at(question, 16, 8), Some(ICON_WHITE));
        assert_eq!(at(question, 8, 16), Some(ICON_BLUE));
        // Warning: a yellow triangle (point at the top) with a black "!".
        assert_eq!(at(warning, 16, 0), Some(ICON_YELLOW_DARK));
        assert_eq!(at(warning, 2, 3), None, "outside the triangle");
        assert_eq!(at(warning, 16, 14), Some(ICON_BLACK));
        assert_eq!(at(warning, 9, 26), Some(ICON_YELLOW));
        assert_eq!(icon_pels(0), None);
        assert_eq!(icon_pels(SPTR_ICONWARNING + 1), None);
    }

    #[test]
    fn object_icons_have_pictograms() {
        let n = MSGBOX_ICON_SIZE as usize;
        let at = |p: &[Option<u32>], x: usize, y: usize| p[y * n + x];
        let file = icon_pels(SPTR_FILE).expect("file");
        let folder = icon_pels(SPTR_FOLDER).expect("folder");
        let multi = icon_pels(SPTR_MULTFILE).expect("several files");
        let program = icon_pels(SPTR_PROGRAM).expect("program");
        assert_eq!(icon_pels(SPTR_APPICON), Some(program.clone()));
        assert_eq!(at(&file, 10, 19), Some(ICON_WHITE), "a white page");
        assert_eq!(at(&file, 24, 2), None, "its top-right corner folded away");
        assert_eq!(at(&file, 0, 0), None);
        assert_eq!(at(&folder, 16, 18), Some(ICON_TAN));
        assert_eq!(at(&folder, 20, 7), None, "only the tab rises at the left");
        assert_eq!(at(&program, 16, 7), Some(ICON_BLUE), "the title bar");
        assert_eq!(at(&program, 16, 18), Some(ICON_WHITE));
        assert_ne!(file, multi);
        assert_eq!(icon_pels(SPTR_ARROW), None);
    }

    #[test]
    fn long_text_wraps_and_line_breaks_start_new_rows() {
        let long = "word ".repeat(30);
        let lay = layout(&format!("First line\n{long}"), MB_OK, mono, 16, 50);
        let texts: Vec<&str> = lay.rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(texts[0], "First line");
        assert!(texts.len() >= 4, "{texts:?}");
        assert!(texts.iter().all(|t| mono(t) <= MSGBOX_TEXT_MAX_W), "{texts:?}");
        let widest = texts.iter().map(|t| mono(t)).max().unwrap();
        assert_eq!(lay.cx, widest + 2 * (MSGBOX_MARGIN + CHROME_BORDER_W), "as wide as the widest row");
        // Rows run downwards from the top, 16 px apart.
        assert!(lay.rows.windows(2).all(|w| w[0].2 - w[1].2 == 16));
        // Too many rows are cut.
        assert_eq!(layout(&"x\n".repeat(100), MB_OK, mono, 16, 5).rows.len(), 5);
    }

    /// A loader with a message queue on thread 0 and an owner frame.
    fn with_owner() -> (Loader, u32) {
        let loader = Loader::new_mock();
        let frame = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), 0, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            wm.z_push_top(frame);
            frame
        };
        (loader, frame)
    }

    fn open(loader: &Loader, owner: u32, style: u32) -> u32 {
        match loader.message_box(0, owner, "Title".into(), "Really?", style) {
            ApiResult::DlgRunLoop { hwnd_dlg, .. } => hwnd_dlg,
            other => panic!("expected the dialog loop, got {other:?}"),
        }
    }

    fn result(loader: &Loader, dlg: u32) -> (bool, u32) {
        loader.shared.window_mgr.lock_or_recover().get_window(dlg)
            .map(|w| (w.dialog_dismissed, w.dialog_result)).unwrap()
    }

    /// Messages waiting in thread 0's queue, removed.
    fn take_messages(loader: &Loader) -> Vec<(u32, u32, u32, u32)> {
        let wm = loader.shared.window_mgr.lock_or_recover();
        let mq = wm.get_mq(wm.thread_queue(0).unwrap()).unwrap();
        let mut mq = mq.lock_or_recover();
        mq.messages.drain(..).map(|m| (m.hwnd, m.msg, m.mp1, m.mp2)).collect()
    }

    #[test]
    fn a_message_box_is_a_modal_dialog_with_its_buttons() {
        let (loader, frame) = with_owner();
        let dlg = open(&loader, frame, MB_YESNOCANCEL | MB_DEFBUTTON2);
        let wm = loader.shared.window_mgr.lock_or_recover();
        let d = wm.get_window(dlg).unwrap();
        assert_eq!((d.class_name.as_str(), d.text.as_str(), d.pfn_wp), ("#Dialog", "Title", 0));
        assert_eq!(d.msgbox_icon, 0, "no icon");
        assert_eq!(wm.focused_hwnd, dlg, "the active window");
        let no = wm.find_child_by_id(dlg, MBID_NO).unwrap();
        assert_eq!(wm.focus_hwnd, no, "the focus is on the default button");
        assert_ne!(wm.get_window(no).unwrap().style & BS_DEFAULT, 0);
        assert!(wm.find_child_by_id(dlg, MBID_YES).is_some() && wm.find_child_by_id(dlg, MBID_CANCEL).is_some());
        assert!(wm.find_child_by_id(dlg, MBID_HELP).is_none(), "no Help without MB_HELP");
        assert_ne!(wm.get_window(frame).unwrap().style & WS_DISABLED, 0, "the owner is disabled");
        let label = d.children.iter().filter_map(|&c| wm.get_window(c))
            .find(|w| w.class_name == "WC_STATIC").map(|w| w.text.clone());
        assert_eq!(label.as_deref(), Some("Really?"));
    }

    #[test]
    fn button_commands_end_it_and_esc_cancels_only_with_a_cancel_button() {
        let (loader, frame) = with_owner();
        let dlg = open(&loader, frame, MB_YESNO);
        loader.procless_dialog_message(dlg, WM_COMMAND, DID_CANCEL, 0);
        loader.procless_dialog_message(dlg, WM_COMMAND, MBID_OK, 0);
        assert!(!result(&loader, dlg).0, "no Cancel or OK button: still open");
        loader.procless_dialog_message(dlg, WM_COMMAND, MBID_NO, 0);
        assert_eq!(result(&loader, dlg), (true, MBID_NO));
        assert_eq!(loader.shared.window_mgr.lock_or_recover().finish_modal_dialog(dlg), MBID_NO);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.is_window(dlg), "destroyed when it ends");
        assert_eq!(wm.get_window(frame).unwrap().style & WS_DISABLED, 0, "the owner is enabled again");
        drop(wm);

        let dlg = open(&loader, frame, MB_OKCANCEL);
        loader.procless_dialog_message(dlg, WM_COMMAND, DID_CANCEL, 0);
        assert_eq!(result(&loader, dlg), (true, MBID_CANCEL), "Esc is Cancel");
    }

    #[test]
    fn close_ends_it_with_mbid_cancel_whatever_its_buttons() {
        let (loader, frame) = with_owner();
        for style in [MB_YESNO, MB_OK | MB_HELP, MB_ENTER, MB_OKCANCEL] {
            let dlg = open(&loader, frame, style);
            loader.procless_dialog_message(dlg, WM_CLOSE, 0, 0);
            assert_eq!(result(&loader, dlg), (true, MBID_CANCEL), "style {style:#x}");
            loader.shared.window_mgr.lock_or_recover().finish_modal_dialog(dlg);
        }
        // The close button / system-menu Close arrive as WM_SYSCOMMAND(SC_CLOSE),
        // which becomes WM_CLOSE.
        let dlg = open(&loader, frame, MB_YESNO);
        take_messages(&loader);
        loader.procless_dialog_message(dlg, WM_SYSCOMMAND, SC_CLOSE, 0);
        let msgs = take_messages(&loader);
        assert!(msgs.iter().any(|m| (m.0, m.1) == (dlg, WM_CLOSE)), "{msgs:?}");
        loader.procless_dialog_message(dlg, WM_CLOSE, 0, 0);
        assert_eq!(result(&loader, dlg), (true, MBID_CANCEL));
    }

    #[test]
    fn the_help_button_sends_wm_help_to_the_box_which_stays_open() {
        let (loader, frame) = with_owner();
        let dlg = open(&loader, frame, MB_YESNO | MB_HELP);
        let help = {
            let wm = loader.shared.window_mgr.lock_or_recover();
            let help = wm.find_child_by_id(dlg, MBID_HELP).expect("a Help button");
            let w = wm.get_window(help).unwrap();
            assert_eq!((w.text.as_str(), w.class_name.as_str()), ("Help", "WC_BUTTON"));
            assert_eq!(w.style & BS_PRIMARYSTYLES, BS_PUSHBUTTON);
            assert_ne!(w.style & BS_HELP, 0);
            assert_eq!(w.style & BS_DEFAULT, 0, "not the default button");
            help
        };
        take_messages(&loader);
        loader.click_button(help, true);
        let msgs = take_messages(&loader);
        assert_eq!(msgs, [(dlg, WM_HELP, MBID_HELP, (1 << 16) | CMDSRC_PUSHBUTTON)],
                   "WM_HELP to the box with the button id, pushed with the pointer");
        assert!(matches!(loader.procless_dialog_message(dlg, WM_HELP, MBID_HELP, CMDSRC_PUSHBUTTON),
                         ApiResult::Normal(0)));
        assert!(!result(&loader, dlg).0, "still open");
        assert!(take_messages(&loader).is_empty(), "the desktop parent gets nothing");
        // The other buttons still end it.
        loader.procless_dialog_message(dlg, WM_COMMAND, MBID_YES, 0);
        assert_eq!(result(&loader, dlg), (true, MBID_YES));
    }

    #[test]
    fn the_icon_is_painted_left_of_the_text() {
        let (loader, frame) = with_owner();
        let dlg = open(&loader, frame, MB_OK | MB_ERROR);
        let (cy, label_x) = {
            let wm = loader.shared.window_mgr.lock_or_recover();
            let d = wm.get_window(dlg).unwrap();
            assert_eq!(d.msgbox_icon, SPTR_ICONERROR);
            let label = d.children.iter().filter_map(|&c| wm.get_window(c))
                .find(|w| w.class_name == "WC_STATIC").unwrap();
            (d.cy, label.x)
        };
        let (ix, iy) = icon_origin(cy);
        assert!(label_x >= ix + MSGBOX_ICON_SIZE, "the text is right of the icon");
        loader.dispatch_builtin_control(dlg, WM_PAINT, 0, 0);
        let surface = loader.shared.window_mgr.lock_or_recover().frame_surface(dlg).unwrap();
        let s = surface.lock_or_recover();
        let sx = ix - CHROME_BORDER_W;
        let pels = icon_pels(SPTR_ICONERROR).unwrap();
        let n = MSGBOX_ICON_SIZE;
        for (x, y) in [(16, 16), (22, 10), (10, 22), (16, 2)] {
            // Top-row-first pel (x, y) is PM y = iy + n - 1 - y.
            assert_eq!(s.pel(sx + x, iy + n - 1 - y), pels[(y * n + x) as usize], "pel ({x}, {y})");
        }
        assert_eq!(s.pel(sx, iy + n - 1), Some(super::super::CONTROL_FACE_COLOR),
                   "a transparent corner shows the face");
    }

    #[test]
    fn without_a_message_queue_it_fails() {
        let loader = Loader::new_mock();
        assert!(matches!(loader.message_box(0, 0, String::new(), "x", MB_OK),
                         ApiResult::Normal(MBID_ERROR)));
    }
}
