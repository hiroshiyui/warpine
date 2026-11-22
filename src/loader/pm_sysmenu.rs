// SPDX-License-Identifier: GPL-3.0-or-later

//! The frame's system menu, keyboard move / size, and pop-up menus.
//!
//! `WM_SYSCOMMAND` (PM Reference, frame control processing): `SC_SYSMENU`
//! "sends MM_STARTMENUMODE to the FID_SYSMENU control", `SC_MOVE` /
//! `SC_SIZE` "send WM_TRACKFRAME to the frame window", whose frame control
//! processing tracks the frame with `WinTrackRect`. Warpine has no
//! `FID_SYSMENU` window: the system menu is a pop-up drawn by the renderer
//! (`gui/system_menu.rs`, `gui/popup_menu.rs`) and the keyboard tracking
//! runs on the GUI thread (`gui/frame_track.rs`), so `WinDefWindowProc` /
//! `WinDefDlgProc` hand both to the GUI thread (`GUIMessage::SystemMenu` /
//! `GUIMessage::TrackFrame`). The renderer posts the item chosen in the
//! menu as `WM_SYSCOMMAND(SC_*, MPFROM2SHORT(CMDSRC_MENU, fPointer))`, which
//! the client may handle itself or pass on to `WinDefWindowProc`.
//!
//! `WinPopupMenu` shows the top-level items of a menu window at a point of
//! its parent window and returns at once ("the function returns as soon as
//! the menu is invoked"); a chosen item is posted to the owner as
//! `WM_COMMAND` (`WM_SYSCOMMAND` for `MIS_SYSCOMMAND`, `WM_HELP` for
//! `MIS_HELP`) with `CMDSRC_MENU`.
//!
//! Unverified (no source): `(x, y)` of `WinPopupMenu` as the menu's
//! bottom-left corner; `PU_POSITIONONITEM` centring the item's row on
//! `y`; the menu kept on the desktop with or without `PU_HCONSTRAIN` /
//! `PU_VCONSTRAIN`; no `WM_INITMENU` / `WM_MENUSELECT` / `WM_MENUEND` to
//! the owner (only `WM_INITMENU` for `FID_MENU` is documented to reach the
//! client); `WM_TRACKFRAME` / `WM_QUERYTRACKINFO` are not sent.

use super::constants::{
    FCF_SYSMENU, HWND_DESKTOP, PMERR_INVALID_HWND, PU_POSITIONONITEM, PU_SELECTITEM, SC_MOVE, SC_SIZE,
    SC_SYSMENU, WS_MAXIMIZED, WS_MINIMIZED, frame_border_width,
};
use super::mutex_ext::MutexExt;
use super::{ApiResult, Loader};
use crate::gui::message::PopupRequest;
use crate::gui::system_menu::can_track;
use crate::gui::GUIMessage;

impl Loader {
    /// The part of `WM_SYSCOMMAND` default processing that needs the GUI
    /// thread, for a frame client or dialog `hwnd`: `SC_SYSMENU` opens the
    /// system menu of a frame that has one (`FCF_SYSMENU`), `SC_MOVE` /
    /// `SC_SIZE` start a keyboard move / size when the frame's state and
    /// sizing border allow it. `mp2` is `MPFROM2SHORT(source, fPointer)`.
    /// Returns whether `cmd` was one of them.
    pub(super) fn frame_syscommand(&self, hwnd: u32, cmd: u32, mp2: u32) -> bool {
        if !matches!(cmd, SC_SYSMENU | SC_MOVE | SC_SIZE) { return false; }
        let wm = self.shared.window_mgr.lock_or_recover();
        let frame = wm.client_to_frame(hwnd);
        let (Some(w), Some(tx)) = (wm.get_window(frame), wm.gui_tx.as_ref()) else { return true };
        if !wm.is_frame(frame) { return true; }
        let state = w.style & (WS_MINIMIZED | WS_MAXIMIZED);
        let msg = match cmd {
            SC_SYSMENU if w.frame_flags & FCF_SYSMENU != 0 => {
                GUIMessage::SystemMenu { frame, keyboard: mp2 >> 16 == 0 }
            }
            SC_MOVE | SC_SIZE if can_track(w.frame_flags, state, cmd == SC_SIZE) && !w.immovable => {
                GUIMessage::TrackFrame { frame, size: cmd == SC_SIZE }
            }
            _ => return true,
        };
        let _ = tx.send(msg);
        true
    }

    /// `WinPopupMenu(hwndParent, hwndOwner, hwndMenu, x, y, idItem, fs)`:
    /// show the menu window `hwnd_menu` (`WinLoadMenu` / `WinCreateMenu`)
    /// with `(x, y)` in `parent`'s coordinates. FALSE for a window that is
    /// not a menu or has no items, or without a PM desktop.
    pub(super) fn win_popup_menu(&self, vcpu_id: u32, args: [u32; 7]) -> ApiResult {
        let [parent, owner, hwnd_menu, x, y, id_item, fs] = args;
        for h in [owner, hwnd_menu] {
            if !self.pm_check_hwnd(vcpu_id, h, false) { return ApiResult::Normal(0); }
        }
        let wm = self.shared.window_mgr.lock_or_recover();
        let origin = if parent == HWND_DESKTOP || parent == 0 {
            Some((0, 0))
        } else {
            let frame = wm.find_frame_for_hwnd(parent);
            wm.get_window(frame).filter(|_| wm.is_frame(frame)).map(|f| {
                // A window's position within its frame's surface, which
                // starts inside the frame's left border.
                let (px, py, _, _) = wm.get_abs_rect_in_frame(parent);
                (f.x + frame_border_width(f.frame_flags) + px, f.y + py)
            })
        };
        let Some((ox, oy)) = origin else {
            drop(wm);
            return self.pm_fail(vcpu_id, PMERR_INVALID_HWND);
        };
        let items = match wm.get_window(hwnd_menu) {
            Some(m) if !m.menu_items.is_empty() => m.menu_items.clone(),
            _ => return ApiResult::Normal(0),
        };
        let Some(tx) = wm.gui_tx.as_ref() else { return ApiResult::Normal(0) };
        let item = (fs & (PU_POSITIONONITEM | PU_SELECTITEM) != 0).then_some(id_item & 0xFFFF);
        let request = PopupRequest {
            owner, items, x: ox + x as i32, y: oy + y as i32,
            position_on_item: fs & PU_POSITIONONITEM != 0, item,
        };
        ApiResult::Normal(tx.send(GUIMessage::PopupMenu(request)).is_ok() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::super::constants::*;
    use super::super::mutex_ext::MutexExt;
    use super::super::pm_types::MenuItem;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::{ApiResult, Loader};
    use crate::gui::{create_gui_channel, GUIMessage, GUIReceiver};

    const ORD_WIN_DEF_WINDOW_PROC: u32 = 911;
    const ORD_WIN_POPUP_MENU: u32 = 937;
    const STD: u32 = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINBUTTON | FCF_MAXBUTTON;

    fn call(loader: &Loader, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
        match loader.handle_pmwin_call(&mut vcpu, 0, ordinal) {
            ApiResult::Normal(v) => v,
            other => panic!("PMWIN ordinal {} returned {:?}", ordinal, other),
        }
    }

    /// A frame with `flags` at (100, 50), 300 × 200, and its client, with a
    /// GUI channel: (frame, client, receiver).
    fn frame(loader: &Loader, flags: u32) -> (u32, u32, GUIReceiver) {
        let (tx, rx) = create_gui_channel();
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        wm.gui_tx = Some(tx);
        let hmq = wm.create_mq(437);
        wm.tid_to_hmq.insert(0, hmq);
        wm.register_class("App".into(), 0x1234, 0);
        let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        wm.frame_to_client.insert(frame, client);
        let f = wm.get_window_mut(frame).unwrap();
        (f.x, f.y, f.cx, f.cy, f.frame_flags) = (100, 50, 300, 200, flags);
        (frame, client, rx)
    }

    fn messages(rx: &GUIReceiver) -> Vec<String> {
        rx.try_iter().map(|m| match m {
            GUIMessage::SystemMenu { frame, keyboard } => format!("menu {frame} {keyboard}"),
            GUIMessage::TrackFrame { frame, size } => format!("track {frame} {size}"),
            GUIMessage::PopupMenu(r) => format!("popup {} {} {} {:?}", r.owner, r.x, r.y, r.item),
            _ => "other".into(),
        }).collect()
    }

    #[test]
    fn sc_sysmenu_opens_the_frames_system_menu() {
        let loader = Loader::new_mock();
        let (frame, client, rx) = frame(&loader, STD);
        call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_SYSMENU, CMDSRC_ACCELERATOR]);
        call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_SYSMENU, CMDSRC_OTHER | 1 << 16]);
        assert_eq!(messages(&rx), vec![format!("menu {frame} true"), format!("menu {frame} false")]);
        // A frame without a system menu opens none.
        loader.shared.window_mgr.lock_or_recover().get_window_mut(frame).unwrap().frame_flags = FCF_TITLEBAR;
        call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_SYSMENU, 0]);
        assert!(messages(&rx).is_empty());
    }

    #[test]
    fn sc_move_and_sc_size_start_keyboard_tracking_when_allowed() {
        let loader = Loader::new_mock();
        let (frame, client, rx) = frame(&loader, STD);
        call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_MOVE, CMDSRC_ACCELERATOR]);
        call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_SIZE, CMDSRC_MENU]);
        assert_eq!(messages(&rx), vec![format!("track {frame} false"), format!("track {frame} true")]);
        // Maximized: neither.
        loader.shared.window_mgr.lock_or_recover().get_window_mut(frame).unwrap().style |= WS_MAXIMIZED;
        call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_MOVE, 0]);
        call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_SIZE, 0]);
        assert!(messages(&rx).is_empty());
        // No sizing border: Move only.
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let f = wm.get_window_mut(frame).unwrap();
            f.style &= !WS_MAXIMIZED;
            f.frame_flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_BORDER;
        }
        call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_SIZE, 0]);
        call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_MOVE, 0]);
        assert_eq!(messages(&rx), vec![format!("track {frame} false")]);
    }

    #[test]
    fn dialogs_open_their_system_menu_through_win_def_dlg_proc() {
        use super::super::pm_types::WindowManager;
        let loader = Loader::new_mock();
        let (_, _, rx) = frame(&loader, STD);
        let dlg = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.tid_to_hmq[&0];
            let dlg = wm.create_window(WindowManager::DIALOG_CLASS.into(), HWND_DESKTOP, hmq);
            wm.get_window_mut(dlg).unwrap().frame_flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_DLGBORDER;
            dlg
        };
        loader.def_dlg_proc(dlg, WM_SYSCOMMAND, SC_SYSMENU, CMDSRC_ACCELERATOR);
        loader.def_dlg_proc(dlg, WM_SYSCOMMAND, SC_SIZE, CMDSRC_ACCELERATOR);
        loader.def_dlg_proc(dlg, WM_SYSCOMMAND, SC_MOVE, CMDSRC_ACCELERATOR);
        assert_eq!(messages(&rx), vec![format!("menu {dlg} true"), format!("track {dlg} false")],
                   "a dialog border is no sizing border");
    }

    #[test]
    fn popup_menu_shows_a_menus_items_at_a_point_of_its_parent() {
        let loader = Loader::new_mock();
        let (frame, client, rx) = frame(&loader, STD);
        let menu = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.tid_to_hmq[&0];
            let menu = wm.create_window("#Menu".into(), HWND_DESKTOP, hmq);
            wm.get_window_mut(menu).unwrap().menu_items = vec![
                MenuItem { id: 7, style: 0, attr: 0, text: "~Open".into(), children: vec![], image: None },
                MenuItem { id: 8, style: 0, attr: 0, text: "~Save".into(), children: vec![], image: None },
            ];
            menu
        };
        let border = frame_border_width(STD) as u32;
        assert_eq!(call(&loader, ORD_WIN_POPUP_MENU, &[client, client, menu, 10, 20, 8, PU_SELECTITEM | PU_KEYBOARD]), 1);
        assert_eq!(call(&loader, ORD_WIN_POPUP_MENU, &[HWND_DESKTOP, client, menu, 500, 400, 0, PU_NONE]), 1);
        assert_eq!(messages(&rx), vec![
            format!("popup {client} {} 70 Some(8)", 110 + border),
            format!("popup {client} 500 400 None"),
        ]);
        let _ = frame;
        // Not a menu (no items): FALSE.
        assert_eq!(call(&loader, ORD_WIN_POPUP_MENU, &[client, client, client, 0, 0, 0, 0]), 0);
        assert!(messages(&rx).is_empty());
    }
}
