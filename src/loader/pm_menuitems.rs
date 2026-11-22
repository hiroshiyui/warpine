// SPDX-License-Identifier: GPL-3.0-or-later

//! The menu item messages of a menu window (`#Menu`): `MM_QUERYITEMCOUNT`,
//! `MM_ISITEMVALID`, `MM_QUERYITEMATTR` / `MM_SETITEMATTR` (and so the
//! `WinCheckMenuItem` / `WinEnableMenuItem` / `WinIsMenuItem*` macros),
//! `MM_QUERYITEMTEXT` / `MM_QUERYITEMTEXTLENGTH` / `MM_SETITEMTEXT`,
//! `MM_ITEMPOSITIONFROMID` / `MM_ITEMIDFROMPOSITION`, `MM_QUERYITEM`,
//! `MM_INSERTITEM`, `MM_DELETEITEM` / `MM_REMOVEITEM` (PM Reference, the
//! `MM_*` pages). The messages that take `fIncludeSubmenus` search the
//! pull-downs too when it is TRUE.
//!
//! Warpine's pull-downs are not windows of their own: an item's submenu is
//! its `children`. So `MM_QUERYITEM` gives the menu's own handle as
//! `hwndSubMenu` of an item with a submenu — messages sent to it with
//! `fIncludeSubmenus` reach the submenu's items — and `MM_INSERTITEM` with
//! a `hwndSubMenu` copies that menu's items as the new item's submenu.
//! TODO(unverified): both are Warpine's choices.
//!
//! **The system menu** (`FID_SYSMENU`). The renderer draws a frame's system
//! menu (`gui/system_menu.rs`); the `FID_SYSMENU` window exists so that a
//! program can change it. It is made when a program first asks for it
//! (`WinWindowFromID(frame, FID_SYSMENU)`), as an invisible `#Menu` holding
//! one `SC_SYSMENU` item whose submenu has the system commands
//! (`MIS_SYSCOMMAND`). What a program disables or checks there shows in
//! the system menu, and a disabled Close turns `SC_CLOSE` away — from the
//! menu, Alt+F4 and the close button alike. TODO(unverified): the
//! reference does not describe the system menu's items; Warpine's are the
//! ones its renderer shows, and the window being made on demand.

use super::constants::*;
use super::pm_types::{MenuItem, WindowManager};
use super::{ApiResult, Loader, MutexExt};

/// `MENUITEM` (pack 2): iPosition, afStyle, afAttribute, id, hwndSubMenu,
/// hItem.
const MENUITEM_STYLE: u32 = 2;
const MENUITEM_ATTR: u32 = 4;
const MENUITEM_ID: u32 = 6;
const MENUITEM_SUBMENU: u32 = 8;
const MENUITEM_SIZE: usize = 16;
/// `MIT_END` / `MIT_NONE` / `MIT_ERROR` (-1) as a reply.
const MIT_ERROR_REPLY: u32 = 0xFFFF_FFFF;
/// `MIT_END`: insert at the end.
const MIT_END: i16 = -1;

/// Where item `id` is: the indices from the top level down; only the top
/// level unless `deep`.
pub fn item_path(items: &[MenuItem], id: u16, deep: bool) -> Option<Vec<usize>> {
    for (i, item) in items.iter().enumerate() {
        if item.id == id {
            return Some(vec![i]);
        }
        if deep && let Some(mut p) = item_path(&item.children, id, true) {
            p.insert(0, i);
            return Some(p);
        }
    }
    None
}

/// The item at `path`.
pub fn item_at<'a>(items: &'a mut [MenuItem], path: &[usize]) -> Option<&'a mut MenuItem> {
    let (&first, rest) = path.split_first()?;
    let item = items.get_mut(first)?;
    if rest.is_empty() { Some(item) } else { item_at(&mut item.children, rest) }
}

/// The list holding the item at `path`, and its index there.
fn list_of<'a>(items: &'a mut Vec<MenuItem>, path: &[usize]) -> Option<(&'a mut Vec<MenuItem>, usize)> {
    let (&last, parents) = path.split_last()?;
    let mut list = items;
    for &i in parents {
        list = &mut list.get_mut(i)?.children;
    }
    Some((list, last))
}

/// `MM_SETITEMATTR`: the `mask` bits of item `id` become those of `data`.
pub fn set_attr(items: &mut [MenuItem], id: u16, deep: bool, mask: u16, data: u16) -> bool {
    let Some(path) = item_path(items, id, deep) else { return false };
    let Some(item) = item_at(items, &path) else { return false };
    item.attr = (item.attr & !mask) | (data & mask);
    true
}

/// `MM_ISITEMVALID`: the item exists and can be chosen — it is enabled, and
/// so is the bar item whose pull-down holds it.
pub fn is_valid(items: &[MenuItem], id: u16, deep: bool) -> bool {
    let Some(path) = item_path(items, id, deep) else { return false };
    let mut list = items;
    for &i in &path {
        let Some(item) = list.get(i) else { return false };
        if item.attr & MIA_DISABLED != 0 {
            return false;
        }
        list = &item.children;
    }
    true
}

/// The system menu `FID_SYSMENU` holds: one `SC_SYSMENU` item whose
/// submenu is the system commands of a frame with `frame_flags`.
pub fn system_menu_items(frame_flags: u32) -> Vec<MenuItem> {
    let children = crate::gui::system_menu::frame_system_menu_items(frame_flags, 0).into_iter().map(|p| {
        if p.separator {
            MenuItem { id: 0xFFFF, style: MIS_SEPARATOR, attr: 0, text: String::new(), children: vec![], image: None }
        } else {
            MenuItem { id: p.id as u16, style: MIS_TEXT | MIS_SYSCOMMAND, attr: 0, text: p.text, children: vec![],
                       image: None }
        }
    }).collect();
    vec![MenuItem { id: SC_SYSMENU as u16, style: MIS_SUBMENU | MIS_BITMAP, attr: 0, text: String::new(), children,
                    image: None }]
}

impl WindowManager {
    /// The `FID_SYSMENU` window of frame `frame` (made now if the frame has
    /// a system menu and no such window yet).
    pub fn system_menu_window(&mut self, frame: u32) -> Option<u32> {
        if let Some(h) = self.find_child_by_id(frame, FID_SYSMENU) {
            return Some(h);
        }
        let flags = self.get_window(frame).filter(|w| w.frame_flags & FCF_SYSMENU != 0)?.frame_flags;
        let h = self.create_window("#Menu".to_string(), frame, 0);
        let w = self.get_window_mut(h)?;
        w.id = FID_SYSMENU;
        w.owner = frame;
        w.visible = false;
        w.menu_items = system_menu_items(flags);
        Some(h)
    }

    /// The attribute bits a program set on system command `cmd` in the
    /// `FID_SYSMENU` window of `frame` (0 without that window).
    pub fn system_menu_attr(&self, frame: u32, cmd: u32) -> u16 {
        let Some(menu) = self.find_child_by_id(frame, FID_SYSMENU) else { return 0 };
        let Some(w) = self.get_window(menu) else { return 0 };
        let Some(path) = item_path(&w.menu_items, cmd as u16, true) else { return 0 };
        let mut list = &w.menu_items[..];
        let mut attr = 0;
        for &i in &path {
            let Some(item) = list.get(i) else { return 0 };
            attr = item.attr;
            list = &item.children;
        }
        attr
    }

    /// Whether `SC_CLOSE` is turned away: Close is disabled in the system
    /// menu of the frame of `hwnd`.
    pub fn close_disabled(&self, hwnd: u32) -> bool {
        let frame = self.client_to_frame(hwnd);
        self.system_menu_attr(frame, SC_CLOSE) & MIA_DISABLED != 0
    }
}

impl Loader {
    /// An `MM_*` item message to menu `menu` (see the module documentation);
    /// `None` for other messages.
    pub(super) fn menu_item_message(&self, menu: u32, msg: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let id = mp1 as u16;
        let deep = (mp1 >> 16) as u16 != 0;
        let ok = |b: bool| Some(ApiResult::Normal(b as u32));
        let cp = self.window_codepage(menu);
        let result = match msg {
            MM_QUERYITEMCOUNT => {
                let wm = self.shared.window_mgr.lock_or_recover();
                return Some(ApiResult::Normal(wm.get_window(menu).map_or(0, |w| w.menu_items.len() as u32)));
            }
            MM_ISITEMVALID => {
                let wm = self.shared.window_mgr.lock_or_recover();
                return ok(wm.get_window(menu).is_some_and(|w| is_valid(&w.menu_items, id, deep)));
            }
            MM_QUERYITEMATTR => {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let attr = wm.get_window_mut(menu).and_then(|w| {
                    let path = item_path(&w.menu_items, id, deep)?;
                    item_at(&mut w.menu_items, &path).map(|i| i.attr)
                }).unwrap_or(0);
                return Some(ApiResult::Normal((attr & mp2 as u16) as u32));
            }
            MM_SETITEMATTR => {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.get_window_mut(menu).is_some_and(|w| set_attr(&mut w.menu_items, id, deep, mp2 as u16, (mp2 >> 16) as u16))
            }
            MM_QUERYITEMTEXTLENGTH | MM_QUERYITEMTEXT => {
                let text = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    wm.get_window_mut(menu).and_then(|w| {
                        let path = item_path(&w.menu_items, id, true)?;
                        item_at(&mut w.menu_items, &path).filter(|i| i.style & (MIS_BITMAP | MIS_SEPARATOR | MIS_OWNERDRAW) == 0)
                            .map(|i| i.text.clone())
                    })
                };
                let Some(bytes) = text.map(|t| super::codepage::cp_encode(&t, cp)) else { return Some(ApiResult::Normal(0)) };
                if msg == MM_QUERYITEMTEXTLENGTH {
                    return Some(ApiResult::Normal(bytes.len() as u32));
                }
                // At most cchMax - 1 bytes and the NUL; 0 copies nothing.
                let max = (mp1 >> 16) as u16 as i16;
                if max <= 0 { return Some(ApiResult::Normal(0)); }
                let n = bytes.len().min(max as usize - 1);
                let mut out = bytes[..n].to_vec();
                out.push(0);
                if self.guest_write_bytes(mp2, &out).is_none() { return Some(ApiResult::Normal(0)); }
                return Some(ApiResult::Normal(n as u32));
            }
            MM_SETITEMTEXT => {
                if mp2 == 0 { return ok(false); }
                let text = self.read_guest_string_cp(mp2, cp);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.get_window_mut(menu).is_some_and(|w| {
                    let Some(path) = item_path(&w.menu_items, id, true) else { return false };
                    item_at(&mut w.menu_items, &path).map(|i| i.text = text).is_some()
                })
            }
            MM_ITEMPOSITIONFROMID => {
                let wm = self.shared.window_mgr.lock_or_recover();
                let pos = wm.get_window(menu).and_then(|w| item_path(&w.menu_items, id, deep)).and_then(|p| p.last().copied());
                return Some(ApiResult::Normal(pos.map_or(MIT_ERROR_REPLY, |p| p as u32)));
            }
            MM_ITEMIDFROMPOSITION => {
                let wm = self.shared.window_mgr.lock_or_recover();
                let at = mp1 as u16 as i16;
                let item = wm.get_window(menu).and_then(|w| usize::try_from(at).ok().and_then(|i| w.menu_items.get(i)));
                return Some(ApiResult::Normal(item.map_or(MIT_ERROR_REPLY, |i| i.id as u32)));
            }
            MM_QUERYITEM => {
                let found = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    wm.get_window_mut(menu).and_then(|w| {
                        let path = item_path(&w.menu_items, id, deep)?;
                        let pos = *path.last()?;
                        item_at(&mut w.menu_items, &path).map(|i| (pos, i.style, i.attr, i.id, !i.children.is_empty()))
                    })
                };
                let Some((pos, style, attr, item_id, sub)) = found else { return ok(false) };
                let mut b = Vec::with_capacity(MENUITEM_SIZE);
                for v in [pos as u16, style, attr, item_id] { b.extend_from_slice(&v.to_le_bytes()); }
                b.extend_from_slice(&(if sub { menu } else { 0 }).to_le_bytes());
                b.extend_from_slice(&0u32.to_le_bytes());
                self.guest_write_bytes(mp2, &b).is_some()
            }
            MM_INSERTITEM => return Some(ApiResult::Normal(self.menu_insert_item(menu, mp1, mp2, cp))),
            MM_DELETEITEM | MM_REMOVEITEM => {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let left = wm.get_window_mut(menu).map_or(0, |w| {
                    if let Some(path) = item_path(&w.menu_items, id, deep)
                        && let Some((list, i)) = list_of(&mut w.menu_items, &path) {
                        list.remove(i);
                    }
                    w.menu_items.len() as u32
                });
                let frame = wm.find_frame_for_hwnd(menu);
                wm.mark_dirty(frame);
                return Some(ApiResult::Normal(left));
            }
            _ => return None,
        };
        if result {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let frame = wm.find_frame_for_hwnd(menu);
            wm.mark_dirty(frame);
        }
        ok(result)
    }

    /// `MM_INSERTITEM`: the item `MENUITEM` at `pitem` describes, with text
    /// `ptext` for an `MIS_TEXT` item, at its `iPosition` (`MIT_END`: last).
    /// Returns its index, or `MIT_ERROR`.
    fn menu_insert_item(&self, menu: u32, pitem: u32, ptext: u32, cp: u32) -> u32 {
        let Some(raw) = self.guest_read_bytes(pitem, MENUITEM_SIZE) else { return MIT_ERROR_REPLY };
        let word = |o: u32| u16::from_le_bytes([raw[o as usize], raw[o as usize + 1]]);
        let pos = word(0) as i16;
        let (style, attr, id) = (word(MENUITEM_STYLE), word(MENUITEM_ATTR), word(MENUITEM_ID));
        let sub = u32::from_le_bytes(raw[MENUITEM_SUBMENU as usize..MENUITEM_SUBMENU as usize + 4].try_into().unwrap_or([0; 4]));
        let text = if style & MIS_TEXT != 0 || style & (MIS_BITMAP | MIS_SEPARATOR | MIS_OWNERDRAW) == 0 {
            if ptext == 0 { String::new() } else { self.read_guest_string_cp(ptext, cp) }
        } else { String::new() };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let children = if sub != 0 && sub != menu {
            wm.get_window(sub).filter(|w| w.class_name == "#Menu").map(|w| w.menu_items.clone()).unwrap_or_default()
        } else { Vec::new() };
        let Some(w) = wm.get_window_mut(menu) else { return MIT_ERROR_REPLY };
        let at = if pos == MIT_END || pos < 0 { w.menu_items.len() } else { (pos as usize).min(w.menu_items.len()) };
        w.menu_items.insert(at, MenuItem { id, style, attr, text, children, image: None });
        let frame = wm.find_frame_for_hwnd(menu);
        wm.mark_dirty(frame);
        at as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u16, attr: u16, children: Vec<MenuItem>) -> MenuItem {
        MenuItem { id, style: MIS_TEXT | if children.is_empty() { 0 } else { MIS_SUBMENU }, attr,
                   text: format!("~Item {id}"), children, image: None }
    }

    fn bar() -> Vec<MenuItem> {
        vec![item(1, 0, vec![item(11, 0, vec![]), item(12, 0, vec![])]), item(2, MIA_DISABLED, vec![item(21, 0, vec![])])]
    }

    #[test]
    fn items_are_found_on_the_bar_and_in_submenus_when_asked() {
        let b = bar();
        assert_eq!(item_path(&b, 2, false), Some(vec![1]));
        assert_eq!(item_path(&b, 12, false), None, "not without fIncludeSubmenus");
        assert_eq!(item_path(&b, 12, true), Some(vec![0, 1]));
        assert_eq!(item_path(&b, 99, true), None);
    }

    #[test]
    fn attributes_are_set_under_their_mask_and_validity_follows_the_bar_item() {
        let mut b = bar();
        assert!(set_attr(&mut b, 11, true, MIA_CHECKED | MIA_DISABLED, MIA_CHECKED));
        assert_eq!(b[0].children[0].attr, MIA_CHECKED);
        assert!(set_attr(&mut b, 11, true, MIA_CHECKED, 0));
        assert_eq!(b[0].children[0].attr, 0, "only the masked bits change");
        assert!(!set_attr(&mut b, 11, false, MIA_CHECKED, MIA_CHECKED), "not found without submenus");
        assert!(is_valid(&b, 12, true));
        assert!(!is_valid(&b, 21, true), "its bar item is disabled");
        assert!(!is_valid(&b, 99, true));
    }

    #[test]
    fn the_system_menu_holds_the_system_commands_under_sc_sysmenu() {
        let items = system_menu_items(FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINMAX);
        assert_eq!((items.len(), items[0].id as u32), (1, SC_SYSMENU));
        let ids: Vec<u32> = items[0].children.iter().filter(|c| c.style & MIS_SEPARATOR == 0).map(|c| c.id as u32).collect();
        assert!(ids.contains(&SC_CLOSE) && ids.contains(&SC_MOVE) && ids.contains(&SC_TASKMANAGER), "{ids:?}");
        assert!(items[0].children.iter().all(|c| c.style & MIS_SEPARATOR != 0 || c.style & MIS_SYSCOMMAND != 0));
    }

    /// A frame with a menu bar of [`bar`], on a mock loader.
    fn fixture() -> (Loader, u32, u32) {
        let l = Loader::new_mock();
        let (frame, menu) = {
            let mut wm = l.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            wm.get_window_mut(frame).unwrap().frame_flags = FCF_TITLEBAR | FCF_SYSMENU;
            let menu = wm.create_window("#Menu".into(), frame, 0);
            let m = wm.get_window_mut(menu).unwrap();
            (m.id, m.menu_items) = (FID_MENU, bar());
            (frame, menu)
        };
        (l, frame, menu)
    }

    fn send(l: &Loader, menu: u32, msg: u32, mp1: u32, mp2: u32) -> u32 {
        match l.menu_item_message(menu, msg, mp1, mp2) { Some(ApiResult::Normal(v)) => v, other => panic!("{other:?}") }
    }

    #[test]
    fn the_item_messages_answer_as_documented() {
        let (l, _, menu) = fixture();
        let deep = 1 << 16;
        assert_eq!(send(&l, menu, MM_QUERYITEMCOUNT, 0, 0), 2);
        // WinCheckMenuItem / WinIsMenuItemChecked / WinEnableMenuItem.
        assert_eq!(send(&l, menu, MM_SETITEMATTR, 12 | deep, MIA_CHECKED as u32 | (MIA_CHECKED as u32) << 16), 1);
        assert_eq!(send(&l, menu, MM_QUERYITEMATTR, 12 | deep, MIA_CHECKED as u32), MIA_CHECKED as u32);
        assert_eq!(send(&l, menu, MM_SETITEMATTR, 12 | deep, MIA_DISABLED as u32 | (MIA_DISABLED as u32) << 16), 1);
        assert_eq!(send(&l, menu, MM_ISITEMVALID, 12 | deep, 0), 0);
        assert_eq!(send(&l, menu, MM_SETITEMATTR, 99 | deep, MIA_CHECKED as u32), 0, "no such item");
        // Positions and ids.
        assert_eq!(send(&l, menu, MM_ITEMPOSITIONFROMID, 12 | deep, 0), 1);
        assert_eq!(send(&l, menu, MM_ITEMPOSITIONFROMID, 12, 0), MIT_ERROR_REPLY);
        assert_eq!(send(&l, menu, MM_ITEMIDFROMPOSITION, 1, 0), 2);
        assert_eq!(send(&l, menu, MM_ITEMIDFROMPOSITION, 7, 0), MIT_ERROR_REPLY);
        // Text.
        let buf = 0x0010_0000;
        assert_eq!(send(&l, menu, MM_QUERYITEMTEXTLENGTH, 11, 0), 8);
        assert_eq!(send(&l, menu, MM_QUERYITEMTEXT, 11 | 5 << 16, buf), 4, "cut to cchMax - 1");
        assert_eq!(l.guest_read_bytes(buf, 5).unwrap(), b"~Ite\0");
        l.guest_write_bytes(buf, b"~Renamed\0").unwrap();
        assert_eq!(send(&l, menu, MM_SETITEMTEXT, 11, buf), 1);
        assert_eq!(send(&l, menu, MM_QUERYITEMTEXTLENGTH, 11, 0), 8);
        // MM_QUERYITEM: an item with a submenu names the menu itself.
        assert_eq!(send(&l, menu, MM_QUERYITEM, 1, buf), 1);
        let mi = l.guest_read_bytes(buf, MENUITEM_SIZE).unwrap();
        assert_eq!((u16::from_le_bytes([mi[6], mi[7]]), u32::from_le_bytes(mi[8..12].try_into().unwrap())), (1, menu));
    }

    #[test]
    fn items_are_inserted_and_deleted() {
        let (l, _, menu) = fixture();
        let (pitem, ptext) = (0x0010_0000, 0x0010_0100);
        let mut mi = Vec::new();
        for v in [MIT_END as u16, MIS_TEXT, 0, 30] { mi.extend_from_slice(&v.to_le_bytes()); }
        mi.extend_from_slice(&[0; 8]);
        l.guest_write_bytes(pitem, &mi).unwrap();
        l.guest_write_bytes(ptext, b"~Help\0").unwrap();
        assert_eq!(send(&l, menu, MM_INSERTITEM, pitem, ptext), 2, "appended");
        mi[0..2].copy_from_slice(&0u16.to_le_bytes());
        mi[6..8].copy_from_slice(&31u16.to_le_bytes());
        l.guest_write_bytes(pitem, &mi).unwrap();
        assert_eq!(send(&l, menu, MM_INSERTITEM, pitem, ptext), 0, "at the front");
        assert_eq!(send(&l, menu, MM_ITEMIDFROMPOSITION, 0, 0), 31);
        assert_eq!(send(&l, menu, MM_DELETEITEM, 11 | 1 << 16, 0), 4, "items left on the bar");
        assert_eq!(send(&l, menu, MM_ITEMPOSITIONFROMID, 11 | 1 << 16, 0), MIT_ERROR_REPLY);
        assert_eq!(send(&l, menu, MM_REMOVEITEM, 31, 0), 3);
    }

    #[test]
    fn a_disabled_close_in_the_system_menu_turns_sc_close_away() {
        let (l, frame, _) = fixture();
        let sys = l.shared.window_mgr.lock_or_recover().system_menu_window(frame).expect("made on demand");
        assert_eq!(l.shared.window_mgr.lock_or_recover().system_menu_window(frame), Some(sys), "once");
        assert!(!l.shared.window_mgr.lock_or_recover().close_disabled(frame));
        send(&l, sys, MM_SETITEMATTR, SC_CLOSE | 1 << 16, MIA_DISABLED as u32 | (MIA_DISABLED as u32) << 16);
        let wm = l.shared.window_mgr.lock_or_recover();
        assert!(wm.close_disabled(frame));
        assert_eq!(wm.system_menu_attr(frame, SC_CLOSE) & MIA_DISABLED, MIA_DISABLED);
        assert!(!wm.get_window(sys).unwrap().visible);
    }
}
