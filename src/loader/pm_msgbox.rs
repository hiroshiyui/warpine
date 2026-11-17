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
//! The layout — sizes, margins, where the text wraps — is Warpine's own.

use super::constants::*;
use super::pm_edit::{EditBuffer, Measure};
use super::{ApiResult, Loader, MutexExt};

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
/// Mask of the button-set value in `flStyle` (`MB_OK` ... `MB_ENTERCANCEL`).
const MB_BUTTON_SET_MASK: u32 = 0x000F;
/// Mask and shift of the default-button choice (`MB_DEFBUTTON1/2/3`).
const MB_DEFBUTTON_MASK: u32 = 0x0300;
const MB_DEFBUTTON_SHIFT: u32 = 8;

/// The buttons of a message box with style `style`, left to right: the
/// `MBID_*` value each returns and its label.
pub fn buttons(style: u32) -> &'static [(u32, &'static str)] {
    match style & MB_BUTTON_SET_MASK {
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
    }
}

/// Index of the default button (`MB_DEFBUTTON1/2/3`) among `count`
/// buttons; a choice past the last button falls back to the first.
pub fn default_button(style: u32, count: usize) -> usize {
    let i = ((style & MB_DEFBUTTON_MASK) >> MB_DEFBUTTON_SHIFT) as usize;
    if i < count { i } else { 0 }
}

/// Where everything goes in a message box. Coordinates are PM's: relative
/// to the dialog's bottom-left corner, border included, y up.
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    pub cx: i32,
    pub cy: i32,
    /// Each line of text and the bottom-left of its row.
    pub rows: Vec<(String, i32, i32)>,
    /// Each button: `(MBID_*, label, x, y)`; all `MSGBOX_BUTTON_W` ×
    /// `MSGBOX_BUTTON_H`.
    pub buttons: Vec<(u32, &'static str, i32, i32)>,
}

/// Lay out a message box with `text` and the buttons of `style`, text
/// measured with `measure` in rows `line_h` high. The text breaks at its
/// line breaks and wraps at word boundaries past `MSGBOX_TEXT_MAX_W`; at
/// most `max_rows` rows are kept (the rest is cut). Buttons are centred
/// below it.
pub fn layout(text: &str, style: u32, measure: Measure, line_h: i32, max_rows: usize) -> Layout {
    let mut buf = EditBuffer::new(text, 0, true);
    buf.set_wrap(MSGBOX_TEXT_MAX_W, measure);
    let lines: Vec<String> = buf.lines().into_iter().take(max_rows.max(1))
        .map(|r| buf.slice(r).trim_end().to_string()).collect();
    let text_w = lines.iter().map(|l| measure(l)).max().unwrap_or(0);
    let set = buttons(style);
    let n = set.len() as i32;
    let buttons_w = n * MSGBOX_BUTTON_W + (n - 1) * MSGBOX_BUTTON_GAP;
    let border = CHROME_BORDER_W;
    let content_w = text_w.max(buttons_w).max(MSGBOX_MIN_CONTENT_W);
    let cx = content_w + 2 * (MSGBOX_MARGIN + border);
    let text_bottom = border + 2 * MSGBOX_MARGIN + MSGBOX_BUTTON_H;
    let text_h = lines.len() as i32 * line_h;
    let cy = text_bottom + text_h + MSGBOX_MARGIN + CHROME_TITLE_H + border;
    let rows = lines.into_iter().enumerate()
        .map(|(i, l)| (l, border + MSGBOX_MARGIN, text_bottom + text_h - (i as i32 + 1) * line_h))
        .collect();
    let left = (cx - buttons_w) / 2;
    let buttons = set.iter().enumerate()
        .map(|(i, &(id, label))| (id, label, left + i as i32 * (MSGBOX_BUTTON_W + MSGBOX_BUTTON_GAP),
                                  border + MSGBOX_MARGIN))
        .collect();
    Layout { cx, cy, rows, buttons }
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
        let lay = layout(text, style, super::pm_controls::system_width, line_h, max_rows);
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
            let text_w = lay.cx - 2 * (CHROME_BORDER_W + MSGBOX_MARGIN);
            for (line, x, y) in &lay.rows {
                // Static text has no id of its own (-1, as in templates).
                add(&mut wm, "WC_STATIC", line, (*x, *y, text_w, line_h), 0xFFFF, SS_TEXT);
            }
            let default = default_button(style, lay.buttons.len());
            let mut focus = 0;
            for (i, &(id, label, x, y)) in lay.buttons.iter().enumerate() {
                let mut bstyle = BS_PUSHBUTTON | WS_TABSTOP;
                if i == 0 { bstyle |= WS_GROUP; }
                if i == default { bstyle |= BS_DEFAULT; }
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

    /// A message for a dialog without a dialog procedure (a message box):
    /// `WinDefDlgProc`, except that a command ends it only when one of its
    /// controls has that id (a push button), and closing it counts as
    /// `DID_CANCEL` under the same rule — so Esc and the close button end a
    /// message box only when it has a Cancel button
    /// (TODO(unverified): what PM does for a box without one).
    pub(super) fn procless_dialog_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> ApiResult {
        let command = match msg {
            WM_COMMAND => Some(mp1 & 0xFFFF),
            WM_CLOSE => Some(DID_CANCEL),
            _ => None,
        };
        match command {
            Some(id) => {
                let known = self.shared.window_mgr.lock_or_recover().find_child_by_id(hwnd, id).is_some();
                if known {
                    self.dismiss_dialog(hwnd, id);
                }
                ApiResult::Normal(0)
            }
            None => self.def_dlg_proc(hwnd, msg, mp1, mp2),
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
    fn the_default_button_is_chosen_by_mb_defbutton() {
        assert_eq!(default_button(MB_YESNOCANCEL, 3), 0);
        assert_eq!(default_button(MB_YESNOCANCEL | MB_DEFBUTTON2, 3), 1);
        assert_eq!(default_button(MB_YESNOCANCEL | MB_DEFBUTTON3, 3), 2);
        assert_eq!(default_button(MB_OK | MB_DEFBUTTON3, 1), 0, "past the last button");
    }

    #[test]
    fn short_text_gets_the_minimum_width_and_centred_buttons() {
        let lay = layout("Saved.", MB_YESNO, mono, 16, 50);
        let border = CHROME_BORDER_W;
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

    #[test]
    fn a_message_box_is_a_modal_dialog_with_its_buttons() {
        let (loader, frame) = with_owner();
        let dlg = open(&loader, frame, MB_YESNOCANCEL | MB_DEFBUTTON2);
        let wm = loader.shared.window_mgr.lock_or_recover();
        let d = wm.get_window(dlg).unwrap();
        assert_eq!((d.class_name.as_str(), d.text.as_str(), d.pfn_wp), ("#Dialog", "Title", 0));
        assert_eq!(wm.focused_hwnd, dlg, "the active window");
        let no = wm.find_child_by_id(dlg, MBID_NO).unwrap();
        assert_eq!(wm.focus_hwnd, no, "the focus is on the default button");
        assert_ne!(wm.get_window(no).unwrap().style & BS_DEFAULT, 0);
        assert!(wm.find_child_by_id(dlg, MBID_YES).is_some() && wm.find_child_by_id(dlg, MBID_CANCEL).is_some());
        assert_ne!(wm.get_window(frame).unwrap().style & WS_DISABLED, 0, "the owner is disabled");
        let label = d.children.iter().filter_map(|&c| wm.get_window(c))
            .find(|w| w.class_name == "WC_STATIC").map(|w| w.text.clone());
        assert_eq!(label.as_deref(), Some("Really?"));
    }

    #[test]
    fn button_commands_end_it_and_cancel_only_with_a_cancel_button() {
        let (loader, frame) = with_owner();
        let result = |dlg: u32| loader.shared.window_mgr.lock_or_recover().get_window(dlg)
            .map(|w| (w.dialog_dismissed, w.dialog_result)).unwrap();

        let dlg = open(&loader, frame, MB_YESNO);
        loader.procless_dialog_message(dlg, WM_CLOSE, 0, 0);
        loader.procless_dialog_message(dlg, WM_COMMAND, DID_CANCEL, 0);
        loader.procless_dialog_message(dlg, WM_COMMAND, MBID_OK, 0);
        assert!(!result(dlg).0, "no Cancel or OK button: still open");
        loader.procless_dialog_message(dlg, WM_COMMAND, MBID_NO, 0);
        assert_eq!(result(dlg), (true, MBID_NO));
        assert_eq!(loader.shared.window_mgr.lock_or_recover().finish_modal_dialog(dlg), MBID_NO);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.is_window(dlg), "destroyed when it ends");
        assert_eq!(wm.get_window(frame).unwrap().style & WS_DISABLED, 0, "the owner is enabled again");
        drop(wm);

        let dlg = open(&loader, frame, MB_OKCANCEL);
        loader.procless_dialog_message(dlg, WM_CLOSE, 0, 0);
        assert_eq!(result(dlg), (true, MBID_CANCEL), "closing is Cancel");
    }

    #[test]
    fn without_a_message_queue_it_fails() {
        let loader = Loader::new_mock();
        assert!(matches!(loader.message_box(0, 0, String::new(), "x", MB_OK),
                         ApiResult::Normal(MBID_ERROR)));
    }
}
