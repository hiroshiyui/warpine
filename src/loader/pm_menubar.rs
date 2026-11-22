// SPDX-License-Identifier: GPL-3.0-or-later

//! Keyboard menu mode of a frame's menu bar (`FID_MENU`).
//!
//! F10 is a system accelerator for `WM_SYSCOMMAND(SC_APPMENU)`, which the
//! frame's default processing answers by starting menu mode on its menu bar
//! (PM Reference: `SC_APPMENU` "sends MM_STARTMENUMODE to the FID_MENU
//! control"); a guest may also send `MM_STARTMENUMODE` itself. Pressing and
//! releasing Alt with no other key in between does the same.
//!
//! In menu mode the keyboard belongs to the menu: keystrokes for the frame
//! are taken out of the queue where accelerators are translated
//! ([`Loader::menu_mode_message`]) and drive the menu instead (delivered as
//! `WM_NULL`). Left / Right move along the bar; Down or Enter drops the
//! highlighted item's pull-down; Up / Down / Home / End move in it; Enter
//! chooses; a mnemonic letter chooses its item; Esc closes the pull-down,
//! then leaves; F10, Alt or a click elsewhere leaves. A chosen item is
//! posted to the client as `WM_COMMAND` (`WM_SYSCOMMAND` for
//! `MIS_SYSCOMMAND`, `WM_HELP` for `MIS_HELP`) with
//! `MPFROM2SHORT(CMDSRC_MENU, FALSE)`.
//!
//! TODO(unverified): PM documents the keys in its user guides only loosely.
//! Which items the arrows skip (separators, disabled items), that F10 only
//! highlights the first item without dropping its pull-down, and that the
//! Alt key's release is what starts menu mode are Warpine's choices.
//!
//! The client hears what the menu does (PM Reference: the menu control
//! "sends" these to its owner, and a frame passes `FID_MENU`'s to
//! `FID_CLIENT`), from the mouse as from the keyboard, as owner calls
//! (`pm_owner.rs`): `WM_INITMENU(FID_MENU, menu)` when menu mode starts and
//! `WM_INITMENU(item, menu)` before a pull-down drops, so the client can
//! change its items first; `WM_MENUSELECT(MPFROM2SHORT(item, FALSE), menu)`
//! when the highlight moves to an item, and with TRUE when one is chosen —
//! the command is posted only when the client returns TRUE, which
//! `WinDefWindowProc` does; `WM_MENUEND(FID_MENU, menu)` when menu mode
//! ends. An `MIS_OWNERDRAW` item gets `WM_DRAWITEM` again when the
//! keyboard highlight comes to it or leaves it, with `MIA_HILITED` in
//! `fsAttribute` and the opposite in `fsAttributeOld`. TODO(unverified):
//! which identity `WM_INITMENU` / `WM_MENUEND` carry for the bar and its
//! pull-downs, and that pull-downs, which are not windows in Warpine, give
//! the menu bar's handle.
//!
//! State: the open pull-down stays where the mouse code keeps it
//! (`window_ulong[-1]`, 1-based); the keyboard state is in
//! `window_ulong[-2]` ([`MenuMode::encode`]), which the renderer draws.

use super::constants::*;
use super::pm_owner::{OwnerCall, OwnerCallKind, OwnerSetup};
use super::pm_types::{MenuItem, OS2Message};
use super::{Loader, MutexExt};

/// `window_ulong` key of the open pull-down (1-based item, 0 = none).
pub const OPEN_ITEM_KEY: i32 = -1;
/// `window_ulong` key of the keyboard menu mode ([`MenuMode::encode`]).
pub const MENU_MODE_KEY: i32 = -2;

/// Keyboard menu mode: the highlighted bar item and, while its pull-down
/// is dropped, the highlighted row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuMode {
    pub item: usize,
    pub row: Option<usize>,
}

impl MenuMode {
    /// As stored: the 1-based item in the low word, the 1-based row (0 = no
    /// pull-down) in the high word; 0 = not in menu mode.
    pub fn encode(self) -> u32 {
        (self.item as u32 + 1) & 0xFFFF | (self.row.map_or(0, |r| r as u32 + 1) << 16)
    }

    pub fn decode(v: u32) -> Option<MenuMode> {
        let item = (v & 0xFFFF).checked_sub(1)? as usize;
        let row = (v >> 16).checked_sub(1).map(|r| r as usize);
        Some(MenuMode { item, row })
    }
}

/// Flag of [`draw_state`]: the bar item is highlighted, its pull-down not
/// dropped.
const DRAW_BAR_ONLY: u32 = 0x8000_0000;

/// What the renderer draws of a menu bar, from its open pull-down `open`
/// (`window_ulong[-1]`) and its keyboard menu mode `mode` (`[-2]`): the
/// highlighted item (1-based) in the low word, the highlighted row
/// (1-based) above it, and [`DRAW_BAR_ONLY`] when no pull-down shows.
pub fn draw_state(open: u32, mode: u32) -> u32 {
    match MenuMode::decode(mode) {
        Some(MenuMode { item, row: None }) => (item as u32 + 1) & 0xFFFF | DRAW_BAR_ONLY,
        Some(MenuMode { row: Some(r), .. }) => open & 0xFFFF | ((r as u32 + 1) & 0x7FFF) << 16,
        None => open & 0xFFFF,
    }
}

/// The highlighted bar item of a [`draw_state`] (0-based).
pub fn drawn_item(state: u32) -> Option<usize> {
    (state & 0xFFFF).checked_sub(1).map(|i| i as usize)
}

/// The bar item whose pull-down shows (0-based).
pub fn drawn_pulldown(state: u32) -> Option<usize> {
    drawn_item(state).filter(|_| state & DRAW_BAR_ONLY == 0)
}

/// The highlighted pull-down row (0-based).
pub fn drawn_row(state: u32) -> Option<usize> {
    ((state >> 16) & 0x7FFF).checked_sub(1).map(|r| r as usize)
}

/// What a key does in menu mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// Stay in menu mode, in this state.
    Stay(MenuMode),
    /// Leave menu mode.
    Leave,
    /// Leave menu mode, choosing this item.
    Choose(MenuItemRef),
}

/// A chosen item: its id and style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItemRef {
    pub id: u16,
    pub style: u16,
}

fn selectable(item: &MenuItem) -> bool {
    item.style & MIS_SEPARATOR == 0 && item.attr & MIA_DISABLED == 0
}

/// The selectable item after (`step` 1) or before (`step` -1) `from` in
/// `items`, wrapping round; `from` itself if no other is.
fn step_in(items: &[MenuItem], from: usize, step: isize) -> usize {
    let n = items.len() as isize;
    (1..=n).map(|k| (from as isize + step * k).rem_euclid(n) as usize)
        .find(|&i| selectable(&items[i]))
        .unwrap_or(from)
}

fn first_selectable(items: &[MenuItem]) -> Option<usize> {
    items.iter().position(selectable)
}

fn mnemonic_of(items: &[MenuItem], ch: char) -> Option<usize> {
    items.iter().position(|i| selectable(i)
        && super::pm_dialog::mnemonic(&i.text).is_some_and(|m| m.eq_ignore_ascii_case(&ch)))
}

/// Enter on bar item `item`: drop its pull-down at its first row, or choose
/// it when it has none.
fn enter_item(items: &[MenuItem], item: usize) -> MenuKey {
    let it = &items[item];
    if it.children.is_empty() {
        return MenuKey::Choose(MenuItemRef { id: it.id, style: it.style });
    }
    MenuKey::Stay(MenuMode { item, row: first_selectable(&it.children) })
}

/// The effect of virtual key `vk` (0 for a character) or character `ch`
/// pressed in menu mode `mode` on the menu bar `items`.
pub fn menu_key(items: &[MenuItem], mode: MenuMode, vk: u32, ch: Option<char>) -> MenuKey {
    let Some(bar) = items.get(mode.item) else { return MenuKey::Leave };
    // Moving along the bar keeps a dropped pull-down dropped.
    let along = |step| {
        let item = step_in(items, mode.item, step);
        match mode.row {
            Some(_) => MenuKey::Stay(MenuMode { item, row: first_selectable(&items[item].children) }),
            None => MenuKey::Stay(MenuMode { item, row: None }),
        }
    };
    match (mode.row, vk) {
        (_, VK_F10) => MenuKey::Leave,
        (_, VK_LEFT) => along(-1),
        (_, VK_RIGHT) => along(1),
        (None, VK_ESC) => MenuKey::Leave,
        (None, VK_DOWN | VK_ENTER | VK_NEWLINE) => enter_item(items, mode.item),
        (Some(_), VK_ESC) => MenuKey::Stay(MenuMode { row: None, ..mode }),
        (Some(row), VK_DOWN | VK_UP) => {
            let row = step_in(&bar.children, row, if vk == VK_DOWN { 1 } else { -1 });
            MenuKey::Stay(MenuMode { row: Some(row), ..mode })
        }
        (Some(_), VK_HOME) => MenuKey::Stay(MenuMode { row: first_selectable(&bar.children), ..mode }),
        (Some(_), VK_END) => MenuKey::Stay(MenuMode {
            row: bar.children.iter().rposition(selectable).or(mode.row), ..mode
        }),
        (Some(row), VK_ENTER | VK_NEWLINE) => match bar.children.get(row) {
            Some(c) if selectable(c) => MenuKey::Choose(MenuItemRef { id: c.id, style: c.style }),
            _ => MenuKey::Stay(mode),
        },
        _ => {
            let Some(ch) = ch else { return MenuKey::Stay(mode) };
            match mode.row {
                None => mnemonic_of(items, ch).map_or(MenuKey::Stay(mode), |i| enter_item(items, i)),
                Some(_) => mnemonic_of(&bar.children, ch).map_or(MenuKey::Stay(mode), |r| {
                    let c = &bar.children[r];
                    MenuKey::Choose(MenuItemRef { id: c.id, style: c.style })
                }),
            }
        }
    }
}

/// The message a chosen menu item posts to the client: `(msg, mp1, mp2)`.
pub fn chosen_message(item: MenuItemRef) -> (u32, u32, u32) {
    let msg = if item.style & MIS_SYSCOMMAND != 0 {
        WM_SYSCOMMAND
    } else if item.style & MIS_HELP != 0 {
        WM_HELP
    } else {
        WM_COMMAND
    };
    (msg, item.id as u32, super::pm_dialog::command_mp2(CMDSRC_MENU, false))
}

/// The item the keyboard highlights in `mode`: the row of the dropped
/// pull-down, else the bar item.
fn highlighted(items: &[MenuItem], mode: MenuMode) -> Option<&MenuItem> {
    let bar = items.get(mode.item)?;
    match mode.row { Some(r) => bar.children.get(r), None => Some(bar) }
}

/// The owner calls a change of menu mode from `old` to `new` makes on menu
/// `menu_id`'s `items` (see the module documentation).
pub fn transition_calls(items: &[MenuItem], menu_id: u16, old: Option<MenuMode>, new: Option<MenuMode>)
        -> Vec<OwnerCallKind> {
    let mut calls = Vec::new();
    if old.is_none() && new.is_some() {
        calls.push(OwnerCallKind::MenuInit { id: menu_id });
    }
    if let Some(n) = new && n.row.is_some() && old.is_none_or(|o| o.row.is_none() || o.item != n.item)
        && let Some(bar) = items.get(n.item) {
        calls.push(OwnerCallKind::MenuInit { id: bar.id });
    }
    let before = old.and_then(|m| highlighted(items, m));
    let after = new.and_then(|m| highlighted(items, m));
    if before.map(|i| i.id) != after.map(|i| i.id) || old.map(|m| (m.item, m.row)) != new.map(|m| (m.item, m.row)) {
        let owner_drawn = |i: &&MenuItem| i.style & MIS_OWNERDRAW != 0;
        if let Some(i) = before.filter(owner_drawn).filter(|_| old.is_some_and(|m| m.row.is_some())) {
            calls.push(OwnerCallKind::DrawMenuItem { id: i.id, hilite: Some(false) });
        }
        if let Some(i) = after {
            if new.is_some_and(|m| m.row.is_some()) && owner_drawn(&i) {
                calls.push(OwnerCallKind::DrawMenuItem { id: i.id, hilite: Some(true) });
            }
            calls.push(OwnerCallKind::MenuSelect { id: i.id, post: None });
        }
    }
    if old.is_some() && new.is_none() {
        calls.push(OwnerCallKind::MenuEnd { id: menu_id });
    }
    calls
}

impl Loader {
    /// The client a menu's notifications go to: the client of its frame
    /// (or its owner), and its window procedure.
    fn menu_owner(&self, menu: u32) -> (u32, u32) {
        let wm = self.shared.window_mgr.lock_or_recover();
        let owner = wm.notify_target(menu);
        let owner = wm.frame_to_client.get(&owner).copied().unwrap_or(owner);
        (owner, wm.get_window(owner).map_or(0, |o| o.pfn_wp))
    }

    /// `WM_INITMENU` / `WM_MENUSELECT` / `WM_MENUEND` for menu `call.control`.
    /// Without a guest procedure to ask, a chosen item's command is posted
    /// at once (as `WinDefWindowProc`'s TRUE would have it).
    pub(super) fn menu_notify_setup(&self, call: &OwnerCall) -> OwnerSetup {
        let menu = call.control;
        let (owner, wnd_proc) = self.menu_owner(menu);
        if wnd_proc == 0 || owner == 0 {
            if let OwnerCallKind::MenuSelect { post: Some((msg, mp1, mp2)), .. } = call.kind {
                self.post_menu_command(menu, msg, mp1, mp2);
            }
            return OwnerSetup::Skip;
        }
        let (msg, mp1) = match call.kind {
            OwnerCallKind::MenuInit { id } => (WM_INITMENU, id as u32),
            OwnerCallKind::MenuSelect { id, post } => (WM_MENUSELECT, id as u32 | (post.is_some() as u32) << 16),
            OwnerCallKind::MenuEnd { id } => (WM_MENUEND, id as u32),
            _ => return OwnerSetup::Skip,
        };
        OwnerSetup::Call { wnd_proc, owner, msg, mp1, mp2: menu, block: Vec::new(), hps: 0 }
    }

    /// Post a chosen item's command to the client of menu `menu`.
    pub(super) fn post_menu_command(&self, menu: u32, msg: u32, mp1: u32, mp2: u32) {
        let (owner, _) = self.menu_owner(menu);
        if owner != 0 {
            self.post_to_window(owner, msg, mp1, mp2);
        }
    }

    /// A click dropped the pull-down of bar item `id` (`was_open`: another
    /// was open): `WM_INITMENU` for the bar first if it was not.
    pub(super) fn mouse_menu_opened(&self, menu: u32, was_open: bool, id: u16) {
        let menu_id = self.shared.window_mgr.lock_or_recover().get_window(menu).map_or(0, |w| w.id as u16);
        let mut calls = Vec::new();
        if !was_open { calls.push(OwnerCallKind::MenuInit { id: menu_id }); }
        calls.push(OwnerCallKind::MenuInit { id });
        self.queue_menu_calls(menu, calls);
    }

    /// A click closed the pull-down without choosing.
    pub(super) fn mouse_menu_closed(&self, menu: u32) {
        let menu_id = self.shared.window_mgr.lock_or_recover().get_window(menu).map_or(0, |w| w.id as u16);
        self.queue_menu_calls(menu, vec![OwnerCallKind::MenuEnd { id: menu_id }]);
    }

    /// A click chose pull-down item `id` of `style`.
    pub(super) fn mouse_menu_chose(&self, menu: u32, id: u16, style: u16) {
        let (msg, mp1, _) = chosen_message(MenuItemRef { id, style });
        let post = (msg, mp1, super::pm_dialog::command_mp2(CMDSRC_MENU, true));
        let menu_id = self.shared.window_mgr.lock_or_recover().get_window(menu).map_or(0, |w| w.id as u16);
        self.queue_menu_calls(menu, vec![OwnerCallKind::MenuSelect { id, post: Some(post) },
                                         OwnerCallKind::MenuEnd { id: menu_id }]);
    }

    /// Queue the owner calls of `kinds` for menu `menu`.
    fn queue_menu_calls(&self, menu: u32, kinds: Vec<OwnerCallKind>) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        for kind in kinds {
            wm.queue_owner_call(OwnerCall { control: menu, kind });
        }
    }

    /// The menu bar (`FID_MENU`) of `frame`, if it has one with items.
    fn frame_menu_bar(&self, frame: u32) -> Option<u32> {
        let wm = self.shared.window_mgr.lock_or_recover();
        wm.find_child_by_id(frame, FID_MENU)
            .filter(|&m| wm.get_window(m).is_some_and(|w| w.class_name == "#Menu" && !w.menu_items.is_empty()))
    }

    /// Whether `menu` is in keyboard menu mode.
    fn menu_mode(&self, menu: u32) -> Option<MenuMode> {
        let wm = self.shared.window_mgr.lock_or_recover();
        wm.get_window(menu).and_then(|w| w.window_ulong.get(&MENU_MODE_KEY).copied()).and_then(MenuMode::decode)
    }

    /// Put `menu` into `mode` (`None`: out of menu mode), with the
    /// pull-down `mode` drops, and redraw it.
    fn set_menu_mode(&self, menu: u32, mode: Option<MenuMode>) {
        let old = self.menu_mode(menu);
        let calls = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.get_window(menu).map(|w| transition_calls(&w.menu_items, w.id as u16, old, mode)).unwrap_or_default()
        };
        self.queue_menu_calls(menu, calls);
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window_mut(menu) else { return };
            match mode {
                Some(m) => {
                    w.window_ulong.insert(MENU_MODE_KEY, m.encode());
                    match m.row {
                        Some(_) => { w.window_ulong.insert(OPEN_ITEM_KEY, m.item as u32 + 1); }
                        None => { w.window_ulong.remove(&OPEN_ITEM_KEY); }
                    }
                }
                None => {
                    w.window_ulong.remove(&MENU_MODE_KEY);
                    w.window_ulong.remove(&OPEN_ITEM_KEY);
                }
            }
            let frame = wm.find_frame_for_hwnd(menu);
            wm.mark_dirty(frame);
        }
    }

    /// `MM_STARTMENUMODE` to menu `menu` (or `SC_APPMENU` to its frame):
    /// highlight the first item; `show` drops its pull-down too.
    pub(super) fn start_menu_mode(&self, menu: u32, show: bool) -> bool {
        let items = {
            let wm = self.shared.window_mgr.lock_or_recover();
            match wm.get_window(menu) { Some(w) => w.menu_items.clone(), None => return false }
        };
        let Some(item) = first_selectable(&items) else { return false };
        let row = if show { first_selectable(&items[item].children) } else { None };
        self.set_menu_mode(menu, Some(MenuMode { item, row }));
        true
    }

    /// `WM_SYSCOMMAND(SC_APPMENU)` for frame client or frame `hwnd`: menu
    /// mode on the frame's menu bar.
    pub(super) fn app_menu(&self, hwnd: u32) {
        let frame = self.shared.window_mgr.lock_or_recover().client_to_frame(hwnd);
        if let Some(menu) = self.frame_menu_bar(frame) {
            self.start_menu_mode(menu, false);
        }
    }

    /// Leave menu mode on `menu` (`MM_ENDMENUMODE`, a click on the menu).
    pub(super) fn end_menu_mode(&self, menu: u32) {
        if self.menu_mode(menu).is_some() {
            self.set_menu_mode(menu, None);
        }
    }

    /// A message read from the queue, before accelerators: in menu mode a
    /// key for the menu's frame drives the menu, and a click elsewhere
    /// leaves it; a lone Alt press and release asks for menu mode. Returns
    /// whether `msg` was replaced (by `WM_NULL`, or the chosen item's
    /// message to the client).
    pub(crate) fn menu_mode_message(&self, msg: &mut OS2Message) -> bool {
        let is_down = (WM_BUTTON1DOWN..=WM_BUTTON3DBLCLK).contains(&msg.msg);
        if msg.msg != WM_CHAR && !is_down {
            return false;
        }
        let (frame, client) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let frame = wm.find_frame_for_hwnd(msg.hwnd);
            (frame, wm.frame_to_client.get(&frame).copied().unwrap_or(frame))
        };
        let Some(menu) = self.frame_menu_bar(frame) else { return false };
        let mode = self.menu_mode(menu);
        if is_down {
            if mode.is_some() && msg.hwnd != menu {
                self.set_menu_mode(menu, None);
            }
            return false;
        }
        let fs = msg.mp1 & 0xFFFF;
        let vk = if fs & KC_VIRTUALKEY != 0 { msg.mp2 >> WM_CHAR_VKEY_SHIFT } else { 0 };
        let up = fs & KC_KEYUP != 0;
        // Alt pressed and released alone.
        if vk == VK_ALT {
            let lone = {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if up { std::mem::take(&mut wm.alt_alone) } else { wm.alt_alone = true; false }
            };
            if !lone {
                // The menu takes Alt in menu mode; otherwise it goes on.
                return mode.is_some() && swallow(msg);
            }
            if mode.is_some() {
                self.set_menu_mode(menu, None);
                return swallow(msg);
            }
            // As F10: WM_SYSCOMMAND(SC_APPMENU) to the client.
            *msg = OS2Message { hwnd: client, msg: WM_SYSCOMMAND, mp1: SC_APPMENU,
                                mp2: CMDSRC_ACCELERATOR, time: msg.time, x: 0, y: 0 };
            return true;
        }
        if !up {
            self.shared.window_mgr.lock_or_recover().alt_alone = false;
        }
        let Some(mode) = mode else { return false };
        if up {
            return swallow(msg);
        }
        let ch = if fs & KC_CHAR != 0 { char::from_u32(msg.mp2 & 0xFF) } else { None };
        let items = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.get_window(menu).map(|w| w.menu_items.clone()).unwrap_or_default()
        };
        match menu_key(&items, mode, vk, ch) {
            MenuKey::Stay(m) => self.set_menu_mode(menu, Some(m)),
            MenuKey::Leave => self.set_menu_mode(menu, None),
            MenuKey::Choose(item) => {
                // WM_MENUSELECT(TRUE) first; its reply decides the post.
                self.queue_menu_calls(menu, vec![OwnerCallKind::MenuSelect { id: item.id, post: Some(chosen_message(item)) }]);
                self.set_menu_mode(menu, None);
            }
        }
        swallow(msg)
    }
}

/// Replace `msg` by `WM_NULL`: the menu took it.
fn swallow(msg: &mut OS2Message) -> bool {
    msg.msg = WM_NULL;
    (msg.mp1, msg.mp2) = (0, 0);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u16, text: &str, children: Vec<MenuItem>) -> MenuItem {
        MenuItem { id, style: MIS_TEXT | if children.is_empty() { 0 } else { MIS_SUBMENU }, attr: 0,
                   text: text.into(), children, image: None }
    }

    fn bar() -> Vec<MenuItem> {
        let sep = MenuItem { style: MIS_SEPARATOR, ..item(0, "", vec![]) };
        let mut disabled = item(13, "~Disabled", vec![]);
        disabled.attr = MIA_DISABLED;
        vec![
            item(1, "~File", vec![item(11, "~Open", vec![]), sep, disabled, item(14, "E~xit", vec![])]),
            item(2, "~Edit", vec![item(21, "~Copy", vec![])]),
            item(3, "~Help", vec![]),
        ]
    }

    fn at(item: usize, row: Option<usize>) -> MenuMode { MenuMode { item, row } }

    #[test]
    fn the_drawn_state_shows_the_highlight_and_the_pull_down() {
        let s = draw_state(2, 0);
        assert_eq!((drawn_item(s), drawn_pulldown(s), drawn_row(s)), (Some(1), Some(1), None), "a click");
        let s = draw_state(0, at(1, None).encode());
        assert_eq!((drawn_item(s), drawn_pulldown(s), drawn_row(s)), (Some(1), None, None), "bar only");
        let s = draw_state(1, at(0, Some(3)).encode());
        assert_eq!((drawn_item(s), drawn_pulldown(s), drawn_row(s)), (Some(0), Some(0), Some(3)));
        assert_eq!(drawn_item(draw_state(0, 0)), None);
    }

    #[test]
    fn the_state_round_trips() {
        for m in [at(0, None), at(2, Some(0)), at(5, Some(7))] {
            assert_eq!(MenuMode::decode(m.encode()), Some(m));
        }
        assert_eq!(MenuMode::decode(0), None);
    }

    #[test]
    fn arrows_move_along_the_bar_and_in_the_pull_down() {
        let b = bar();
        assert_eq!(menu_key(&b, at(0, None), VK_RIGHT, None), MenuKey::Stay(at(1, None)));
        assert_eq!(menu_key(&b, at(0, None), VK_LEFT, None), MenuKey::Stay(at(2, None)), "wraps");
        assert_eq!(menu_key(&b, at(0, None), VK_DOWN, None), MenuKey::Stay(at(0, Some(0))));
        assert_eq!(menu_key(&b, at(0, Some(0)), VK_DOWN, None), MenuKey::Stay(at(0, Some(3))),
                   "the separator and the disabled item are skipped");
        assert_eq!(menu_key(&b, at(0, Some(3)), VK_DOWN, None), MenuKey::Stay(at(0, Some(0))), "wraps");
        assert_eq!(menu_key(&b, at(0, Some(0)), VK_UP, None), MenuKey::Stay(at(0, Some(3))));
        assert_eq!(menu_key(&b, at(0, Some(0)), VK_END, None), MenuKey::Stay(at(0, Some(3))));
        assert_eq!(menu_key(&b, at(0, Some(3)), VK_HOME, None), MenuKey::Stay(at(0, Some(0))));
        assert_eq!(menu_key(&b, at(0, Some(3)), VK_RIGHT, None), MenuKey::Stay(at(1, Some(0))),
                   "the next pull-down drops");
    }

    #[test]
    fn enter_mnemonics_and_escape() {
        let b = bar();
        let chose = |id, style| MenuKey::Choose(MenuItemRef { id, style });
        assert_eq!(menu_key(&b, at(0, Some(3)), VK_ENTER, None), chose(14, MIS_TEXT));
        assert_eq!(menu_key(&b, at(2, None), VK_ENTER, None), chose(3, MIS_TEXT), "an item without a pull-down");
        assert_eq!(menu_key(&b, at(0, None), 0, Some('e')), MenuKey::Stay(at(1, Some(0))));
        assert_eq!(menu_key(&b, at(0, Some(0)), 0, Some('X')), chose(14, MIS_TEXT));
        assert_eq!(menu_key(&b, at(0, Some(0)), 0, Some('d')), MenuKey::Stay(at(0, Some(0))),
                   "a disabled item's mnemonic does nothing");
        assert_eq!(menu_key(&b, at(0, Some(0)), VK_ESC, None), MenuKey::Stay(at(0, None)));
        assert_eq!(menu_key(&b, at(0, None), VK_ESC, None), MenuKey::Leave);
        assert_eq!(menu_key(&b, at(0, Some(0)), VK_F10, None), MenuKey::Leave);
    }

    #[test]
    fn chosen_items_post_their_message() {
        let mp2 = super::super::pm_dialog::command_mp2(CMDSRC_MENU, false);
        assert_eq!(chosen_message(MenuItemRef { id: 5, style: MIS_TEXT }), (WM_COMMAND, 5, mp2));
        assert_eq!(chosen_message(MenuItemRef { id: 5, style: MIS_SYSCOMMAND }).0, WM_SYSCOMMAND);
        assert_eq!(chosen_message(MenuItemRef { id: 5, style: MIS_HELP }).0, WM_HELP);
    }

    // ── On a live window manager ────────────────────────────────────────

    struct Fx { l: Loader, client: u32, menu: u32 }

    fn fx() -> Fx {
        let l = Loader::new_mock();
        let (client, menu) = {
            let mut wm = l.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            let menu = wm.create_window("#Menu".into(), frame, 0);
            let m = wm.get_window_mut(menu).unwrap();
            m.id = FID_MENU;
            m.menu_items = bar();
            wm.get_window_mut(frame).unwrap().menu_hwnd = menu;
            (client, menu)
        };
        Fx { l, client, menu }
    }

    impl Fx {
        /// A key for the client as it leaves the queue: (msg, mp1) delivered.
        fn key(&self, vk: u32, up: bool) -> (u32, u32, u32) {
            let fs = KC_VIRTUALKEY | if up { KC_KEYUP } else { 0 };
            let mut m = OS2Message { hwnd: self.client, msg: WM_CHAR, mp1: fs, mp2: vk << WM_CHAR_VKEY_SHIFT,
                                     time: 0, x: 0, y: 0 };
            self.l.translate_queued_accelerator(&mut m);
            (m.hwnd, m.msg, m.mp1)
        }
        fn mode(&self) -> Option<MenuMode> { self.l.menu_mode(self.menu) }
    }

    #[test]
    fn f10_starts_menu_mode_and_enter_chooses() {
        let f = fx();
        assert_eq!(f.key(VK_F10, false), (f.client, WM_SYSCOMMAND, SC_APPMENU));
        f.l.def_window_proc(1, f.client, WM_SYSCOMMAND, SC_APPMENU, CMDSRC_ACCELERATOR);
        assert_eq!(f.mode(), Some(at(0, None)), "the first item highlighted");
        assert_eq!(f.key(VK_F10, true).1, WM_NULL, "the menu takes the release");
        assert_eq!(f.key(VK_DOWN, false).1, WM_NULL);
        assert_eq!(f.mode(), Some(at(0, Some(0))));
        let open = f.l.shared.window_mgr.lock_or_recover().get_window(f.menu)
            .and_then(|w| w.window_ulong.get(&OPEN_ITEM_KEY).copied());
        assert_eq!(open, Some(1), "the pull-down is dropped");
        f.key(VK_DOWN, false);
        assert_eq!(f.key(VK_ENTER, false).1, WM_NULL, "the menu takes Enter");
        assert_eq!(f.mode(), None);
        // No window procedure to ask: WM_MENUSELECT's TRUE is assumed and
        // Exit's command posted.
        f.l.drain_owner_calls(|_| 1);
        let posted: Vec<_> = {
            let wm = f.l.shared.window_mgr.lock_or_recover();
            let mq = wm.get_mq(wm.find_hmq_for_hwnd(f.client).unwrap()).unwrap();
            let mq = mq.lock_or_recover();
            mq.messages.iter().map(|m| (m.hwnd, m.msg, m.mp1)).collect()
        };
        assert!(posted.contains(&(f.client, WM_COMMAND, 14)), "Exit chosen: {posted:?}");
        let open = f.l.shared.window_mgr.lock_or_recover().get_window(f.menu)
            .and_then(|w| w.window_ulong.get(&OPEN_ITEM_KEY).copied());
        assert_eq!(open, None, "and the pull-down closed");
        assert_eq!(f.key(VK_DOWN, false).1, WM_CHAR, "keys go to the client again");
    }

    #[test]
    fn the_client_hears_init_select_and_end_and_its_reply_decides_the_post() {
        let f = fx();
        f.l.shared.window_mgr.lock_or_recover().get_window_mut(f.client).unwrap().pfn_wp = 0x5000;
        f.l.start_menu_mode(f.menu, false);
        f.key(VK_DOWN, false);
        f.key(VK_ENTER, false);
        let fid = FID_MENU & 0xFFFF;
        let made = f.l.drain_owner_calls(|msg| (msg != WM_MENUSELECT) as u32);
        let msgs: Vec<_> = made.iter().map(|&(o, m, mp1, mp2)| { assert_eq!((o, mp2), (f.client, f.menu)); (m, mp1) }).collect();
        assert_eq!(msgs, vec![
            (WM_INITMENU, fid), (WM_MENUSELECT, 1),           // menu mode: File highlighted
            (WM_INITMENU, 1), (WM_MENUSELECT, 11),             // File's pull-down: Open
            (WM_MENUSELECT, 11 | 1 << 16), (WM_MENUEND, fid),  // chosen
        ]);
        let posted = {
            let wm = f.l.shared.window_mgr.lock_or_recover();
            let mq = wm.get_mq(wm.find_hmq_for_hwnd(f.client).unwrap()).unwrap();
            let mq = mq.lock_or_recover();
            mq.messages.iter().any(|m| m.msg == WM_COMMAND)
        };
        assert!(!posted, "the client returned FALSE: nothing posted");
    }

    #[test]
    fn owner_drawn_rows_are_drawn_again_when_the_highlight_moves() {
        let mut b = bar();
        b[0].children[0].style |= MIS_OWNERDRAW;
        let calls = transition_calls(&b, 0x8005, Some(at(0, None)), Some(at(0, Some(0))));
        assert!(calls.contains(&OwnerCallKind::DrawMenuItem { id: 11, hilite: Some(true) }), "{calls:?}");
        let calls = transition_calls(&b, 0x8005, Some(at(0, Some(0))), Some(at(0, Some(3))));
        assert!(calls.contains(&OwnerCallKind::DrawMenuItem { id: 11, hilite: Some(false) }), "{calls:?}");
        assert!(calls.contains(&OwnerCallKind::MenuSelect { id: 14, post: None }));
    }

    #[test]
    fn a_lone_alt_toggles_menu_mode_but_alt_with_a_key_does_not() {
        let f = fx();
        assert_eq!(f.key(VK_ALT, false).1, WM_CHAR);
        assert_eq!(f.key(VK_ALT, true), (f.client, WM_SYSCOMMAND, SC_APPMENU));
        f.l.def_window_proc(1, f.client, WM_SYSCOMMAND, SC_APPMENU, CMDSRC_ACCELERATOR);
        assert!(f.mode().is_some());
        f.key(VK_ALT, false);
        assert_eq!(f.key(VK_ALT, true).1, WM_NULL);
        assert_eq!(f.mode(), None, "Alt again leaves");
        f.key(VK_ALT, false);
        f.key(VK_F4, false);
        assert_eq!(f.key(VK_ALT, true).1, WM_CHAR, "Alt+F4 is no lone Alt");
        // MM_STARTMENUMODE with fShowSubmenu, then a click elsewhere leaves.
        f.l.dispatch_builtin_control(f.menu, MM_STARTMENUMODE, 1, 0);
        assert_eq!(f.mode(), Some(at(0, Some(0))));
        let mut click = OS2Message { hwnd: f.client, msg: WM_BUTTON1DOWN, mp1: 0, mp2: 0, time: 0, x: 0, y: 0 };
        f.l.translate_queued_accelerator(&mut click);
        assert_eq!((click.msg, f.mode()), (WM_BUTTON1DOWN, None));
    }
}
