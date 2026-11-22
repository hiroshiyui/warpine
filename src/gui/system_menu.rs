// SPDX-License-Identifier: GPL-3.0-or-later

//! The system menu of a PM frame (`FCF_SYSMENU`, `FID_SYSMENU`) and the
//! items of pop-up menus (`WinPopupMenu`), as [`PopupItem`]s for the
//! renderer's [`PopupMenu`](super::popup_menu::PopupMenu).
//!
//! The system menu's items, their order, mnemonics and accelerator texts
//! follow IBM's screenshot of an OS/2 window's system menu (GG24-3732), as
//! the VIO window's menu does (`vio_window.rs`): Restore Alt+F5, Move
//! Alt+F7, Size Alt+F8, Minimize Alt+F9, Maximize Alt+F10, Hide Alt+F11,
//! Close Alt+F4, and Window list Ctrl+Esc. Choosing one posts
//! `WM_SYSCOMMAND(SC_*)` (PM Reference, WM_SYSCOMMAND: "the application can
//! simulate a system menu choice by sending this message").
//!
//! `WM_INITMENU` "If the identity is FID_SYSMENU the system menu state is
//! initialized according to the current state of the window" (PM
//! Reference): [`frame_system_menu_items`] computes that state. Which item
//! is enabled when is Warpine's reading: Restore when minimized or
//! maximized, Move unless maximized, Size only for a frame with a sizing
//! border in its normal state, Minimize / Maximize / Hide only for a frame
//! with that button and not already in that state, Close always, Window
//! list never (Warpine has no window list).

use super::popup_menu::PopupItem;
use crate::loader::constants::{
    FCF_HIDEBUTTON, FCF_MAXBUTTON, FCF_MINBUTTON, FCF_SIZEBORDER, MIA_CHECKED, MIA_DISABLED,
    MIS_SEPARATOR, MIS_SUBMENU, SC_CLOSE, SC_HIDE, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE,
    SC_SIZE, SC_TASKMANAGER, WS_MAXIMIZED, WS_MINIMIZED,
};
use crate::loader::pm_types::MenuItem;

/// Whether a window in `state` (`WS_MINIMIZED` / `WS_MAXIMIZED`) with
/// creation flags `frame_flags` can be moved (`size` false) or sized with
/// the keyboard: a maximized window cannot be moved, only a window with a
/// sizing border in its normal state can be sized.
pub fn can_track(frame_flags: u32, state: u32, size: bool) -> bool {
    if size {
        frame_flags & FCF_SIZEBORDER != 0 && state & (WS_MINIMIZED | WS_MAXIMIZED) == 0
    } else {
        state & WS_MAXIMIZED == 0
    }
}

/// The window items of a system menu, Restore to Close, for a frame with
/// creation flags `frame_flags` in `state`.
pub fn window_items(frame_flags: u32, state: u32) -> Vec<PopupItem> {
    let has = |fcf: u32| frame_flags & fcf != 0;
    let minimized = state & WS_MINIMIZED != 0;
    let maximized = state & WS_MAXIMIZED != 0;
    vec![
        PopupItem::new(SC_RESTORE, "~Restore", "Alt+F5", minimized || maximized),
        PopupItem::new(SC_MOVE, "~Move", "Alt+F7", can_track(frame_flags, state, false)),
        PopupItem::new(SC_SIZE, "~Size", "Alt+F8", can_track(frame_flags, state, true)),
        PopupItem::new(SC_MINIMIZE, "Mi~nimize", "Alt+F9", has(FCF_MINBUTTON) && !minimized),
        PopupItem::new(SC_MAXIMIZE, "Ma~ximize", "Alt+F10", has(FCF_MAXBUTTON) && !maximized),
        PopupItem::new(SC_HIDE, "~Hide", "Alt+F11", has(FCF_HIDEBUTTON)),
        PopupItem::separator(),
        PopupItem::new(SC_CLOSE, "~Close", "Alt+F4", true),
    ]
}

/// The system menu of a PM frame: [`window_items`] and Window list.
pub fn frame_system_menu_items(frame_flags: u32, state: u32) -> Vec<PopupItem> {
    let mut items = window_items(frame_flags, state);
    items.push(PopupItem::separator());
    items.push(PopupItem::new(SC_TASKMANAGER, "~Window list", "Ctrl+Esc", false));
    items
}

/// The top-level items of a menu (`WinPopupMenu`): separators, greyed
/// (`MIA_DISABLED`) and checked (`MIA_CHECKED`) items as the template says.
/// The accelerator text is what follows a tab in the item text. Submenus
/// cannot be opened yet: their items are greyed (Warpine's
/// simplification).
pub fn menu_popup_items(items: &[MenuItem]) -> Vec<PopupItem> {
    items.iter().map(|m| {
        if m.style & MIS_SEPARATOR != 0 {
            return PopupItem::separator();
        }
        let (text, accel) = m.text.split_once('\t').unwrap_or((&m.text, ""));
        let enabled = m.attr & MIA_DISABLED == 0 && m.style & MIS_SUBMENU == 0;
        PopupItem::new(m.id as u32, text, accel, enabled).checked(m.attr & MIA_CHECKED != 0)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::constants::{FCF_SYSMENU, FCF_TITLEBAR};

    const STD: u32 = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINBUTTON | FCF_MAXBUTTON;

    fn enabled(items: &[PopupItem], id: u32) -> bool {
        items.iter().find(|i| i.id == id && !i.separator).unwrap().enabled
    }

    #[test]
    fn the_system_menu_lists_the_window_commands_in_order() {
        let items = frame_system_menu_items(STD, 0);
        let ids: Vec<u32> = items.iter().filter(|i| !i.separator).map(|i| i.id).collect();
        assert_eq!(ids, vec![SC_RESTORE, SC_MOVE, SC_SIZE, SC_MINIMIZE, SC_MAXIMIZE, SC_HIDE, SC_CLOSE,
                             SC_TASKMANAGER]);
        assert_eq!(items.iter().filter(|i| i.separator).count(), 2);
        let close = items.iter().find(|i| i.id == SC_CLOSE).unwrap();
        assert_eq!((close.text.as_str(), close.accel.as_str()), ("~Close", "Alt+F4"));
    }

    #[test]
    fn items_follow_the_state_and_the_buttons() {
        let normal = frame_system_menu_items(STD, 0);
        assert!(!enabled(&normal, SC_RESTORE));
        for id in [SC_MOVE, SC_SIZE, SC_MINIMIZE, SC_MAXIMIZE, SC_CLOSE] {
            assert!(enabled(&normal, id), "{id:#x}");
        }
        assert!(!enabled(&normal, SC_HIDE) && !enabled(&normal, SC_TASKMANAGER));

        let max = frame_system_menu_items(STD, WS_MAXIMIZED);
        assert!(enabled(&max, SC_RESTORE) && enabled(&max, SC_MINIMIZE));
        assert!(!enabled(&max, SC_MOVE) && !enabled(&max, SC_SIZE) && !enabled(&max, SC_MAXIMIZE));

        let min = frame_system_menu_items(STD, WS_MINIMIZED);
        assert!(enabled(&min, SC_RESTORE) && enabled(&min, SC_MOVE) && enabled(&min, SC_MAXIMIZE));
        assert!(!enabled(&min, SC_SIZE) && !enabled(&min, SC_MINIMIZE));

        // No sizing border, no buttons: only Move and Close.
        let plain = frame_system_menu_items(FCF_TITLEBAR | FCF_SYSMENU, 0);
        let on: Vec<u32> = plain.iter().filter(|i| i.enabled).map(|i| i.id).collect();
        assert_eq!(on, vec![SC_MOVE, SC_CLOSE]);
        // A hide button enables Hide.
        assert!(enabled(&frame_system_menu_items(FCF_HIDEBUTTON, 0), SC_HIDE));
    }

    #[test]
    fn keyboard_tracking_needs_the_right_state() {
        assert!(can_track(STD, 0, false) && can_track(STD, 0, true));
        assert!(can_track(STD, WS_MINIMIZED, false) && !can_track(STD, WS_MINIMIZED, true));
        assert!(!can_track(STD, WS_MAXIMIZED, false) && !can_track(STD, WS_MAXIMIZED, true));
        assert!(!can_track(FCF_TITLEBAR, 0, true), "no Size without a sizing border");
    }

    #[test]
    fn mnemonics_are_unique() {
        let items = frame_system_menu_items(STD, 0);
        let mut letters: Vec<char> = items.iter().filter(|i| !i.separator).map(|i| {
            let at = i.text.find('~').unwrap();
            i.text[at + 1..].chars().next().unwrap().to_ascii_lowercase()
        }).collect();
        let n = letters.len();
        letters.sort();
        letters.dedup();
        assert_eq!(letters.len(), n);
    }

    #[test]
    fn menu_templates_become_popup_items() {
        let item = |id, style, attr, text: &str| MenuItem { id, style, attr, text: text.into(), children: vec![], image: None, row_h: 0 };
        let items = menu_popup_items(&[
            item(10, 0, 0, "~Open\tCtrl+O"),
            item(0, MIS_SEPARATOR, 0, ""),
            item(11, 0, MIA_DISABLED, "~Save"),
            item(12, 0, MIA_CHECKED, "~Wrap"),
            item(13, MIS_SUBMENU, 0, "~More"),
        ]);
        assert_eq!(items[0], PopupItem::new(10, "~Open", "Ctrl+O", true));
        assert!(items[1].separator);
        assert!(!items[2].enabled);
        assert!(items[3].checked && items[3].enabled);
        assert!(!items[4].enabled, "submenus are not opened yet");
    }
}
