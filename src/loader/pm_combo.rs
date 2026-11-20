// SPDX-License-Identifier: GPL-3.0-or-later

//! Combination boxes (`WC_COMBOBOX`): an entry field and a list box, the
//! built-in controls of `pm_controls.rs`, as children of the combination
//! box (ids `CBID_EDIT` and `CBID_LIST`, owned by it).
//!
//! * `CBS_SIMPLE`: the list is always shown below the entry field.
//! * `CBS_DROPDOWN`: the list drops down below the entry field when the
//!   arrow button right of it is clicked, or on Alt+Down / F4.
//! * `CBS_DROPDOWNLIST`: as `CBS_DROPDOWN`, with a read-only entry field
//!   that shows the selected item; a click on it drops the list too.
//!
//! The combination box's window covers the entry field and the space the
//! list drops into, as in PM dialog templates (whose combination box height
//! includes the list). The dropped list is a child window: it lies over the
//! controls below it in its parent — painted after them
//! (`WindowManager::builtin_control_descendants` puts it last, and a
//! control repainted under it repaints it again) and hit first — but is
//! clipped to the parent window like any child (PM's list box is a child of
//! the combination box too; whether PM lets it reach outside the parent is
//! unverified). While the list is hidden the area below the entry field
//! belongs to whatever lies there: the combination box is transparent to
//! the pointer outside its entry row, and hiding the list invalidates its
//! area in the parent.
//!
//! The parts report to the combination box (`WM_CONTROL` from its
//! children), which tells its owner: `EN_CHANGE` → `CBN_EFCHANGE`,
//! `LN_SELECT` → `CBN_LBSELECT` (the entry field takes the item's text),
//! `LN_ENTER` → `CBN_ENTER`; `CBN_SHOWLIST` before the list drops down.
//! mp2 of every `CBN_*` is the combination box. Keys the entry field does
//! not use come to the combination box: Alt+Down / F4 drop or close the
//! list, Up / Down / PageUp / PageDown move the selection, Enter closes the
//! list and sends `CBN_ENTER` (and, with the list closed, goes on to the
//! owner so a dialog's default button works — unverified), Esc closes the
//! list (or goes on to the owner). An item clicked in the dropped list is
//! selected and the list closes when the button is released.
//!
//! Messages: `CBM_SHOWLIST`, `CBM_HILITE` (the entry field's text is shown
//! highlighted), `CBM_ISLISTSHOWING`; `LM_*` go to the list box (selecting
//! an item also sets the entry field's text) and `EM_*` to the entry field.
//! The window text of the combination box is its entry field's
//! (`WindowManager::text_window`). The keyboard focus of a combination box
//! is its entry field's (`WindowManager::focus_window_for`).

use super::constants::*;
use super::pm_types::WindowManager;
use super::{control_box, ApiResult, Loader, MutexExt, Painter, CONTROL_FACE_COLOR,
            CONTROL_LINE_H, CONTROL_TEXT_COLOR};

pub(super) const CLASS_COMBOBOX: &str = "WC_COMBOBOX";

/// Height of a combination box's entry field (and arrow button).
pub(super) const COMBO_ENTRY_H: i32 = CONTROL_LINE_H + 6;
/// Width of a drop-down combination box's arrow button.
pub(super) const COMBO_ARROW_W: i32 = 18;
/// Height of a dropped list when the combination box leaves it no room.
const COMBO_MIN_LIST_H: i32 = 4 * CONTROL_LINE_H + 4;

/// The parts and state of a combination box.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ComboState {
    /// The entry field (`CBID_EDIT`).
    pub entry: u32,
    /// The list box (`CBID_LIST`).
    pub list: u32,
    /// The drop-down list is showing.
    pub showing: bool,
    /// `CBM_HILITE`: the entry field is highlighted.
    pub hilite: bool,
}

/// The `CBS_*` kind of a combination box style.
fn kind(style: u32) -> u32 {
    if style & CBS_DROPDOWNLIST != 0 {
        CBS_DROPDOWNLIST
    } else if style & CBS_DROPDOWN != 0 {
        CBS_DROPDOWN
    } else {
        CBS_SIMPLE
    }
}

/// Whether a combination box of `style` has a drop-down list.
pub(super) fn drops_down(style: u32) -> bool {
    kind(style) != CBS_SIMPLE
}

/// Where the parts of a combination box `cx` × `cy` of `style` go, relative
/// to it: `(entry, list, arrow)` as `(x, y, cx, cy)`; the arrow is empty
/// for `CBS_SIMPLE`.
pub(super) fn combo_layout(style: u32, cx: i32, cy: i32) -> [(i32, i32, i32, i32); 3] {
    let arrow_w = if drops_down(style) { COMBO_ARROW_W.min(cx) } else { 0 };
    let entry_h = COMBO_ENTRY_H.min(cy.max(0)).max(0);
    let top = cy - entry_h;
    // A drop-down list gets some room even when the box leaves it none.
    let list_h = if drops_down(style) { top.max(COMBO_MIN_LIST_H) } else { top.max(0) };
    [
        (0, top, cx - arrow_w, entry_h),
        (0, top - list_h, cx, list_h),
        (cx - arrow_w, top, arrow_w, entry_h),
    ]
}

impl WindowManager {
    /// Whether `hwnd` is a combination box.
    pub fn is_combo(&self, hwnd: u32) -> bool {
        self.get_window(hwnd).is_some_and(|w| w.class_name == CLASS_COMBOBOX)
    }

    /// The combination box `hwnd` is a part of, if it is one.
    pub fn combo_of(&self, hwnd: u32) -> Option<u32> {
        let parent = self.get_window(hwnd)?.parent;
        let c = self.get_window(parent)?.combo?;
        (c.entry == hwnd || c.list == hwnd).then_some(parent)
    }

    /// Whether `hwnd` is the list of a drop-down combination box that is
    /// showing it.
    pub fn is_dropped_list(&self, hwnd: u32) -> bool {
        self.combo_of(hwnd).and_then(|c| self.get_window(c))
            .is_some_and(|w| w.combo.is_some_and(|s| s.list == hwnd && s.showing) && drops_down(w.style))
    }

    /// Create the entry field and list box of new combination box `hwnd`
    /// (its style, size and owner set) and place them.
    pub fn build_combo(&mut self, hwnd: u32) {
        let Some((style, hmq)) = self.get_window(hwnd).map(|w| (w.style, w.hmq)) else { return };
        if self.get_window(hwnd).is_some_and(|w| w.combo.is_some()) { return; }
        let entry = self.create_window("WC_ENTRYFIELD".into(), hwnd, hmq);
        let list = self.create_window("WC_LISTBOX".into(), hwnd, hmq);
        let entry_style = WS_VISIBLE | (style & WS_TABSTOP)
            | if kind(style) == CBS_DROPDOWNLIST { ES_READONLY } else { 0 };
        let list_style = if drops_down(style) { 0 } else { WS_VISIBLE };
        for (h, id, s) in [(entry, CBID_EDIT, entry_style), (list, CBID_LIST, list_style)] {
            if let Some(w) = self.get_window_mut(h) {
                (w.id, w.style, w.owner, w.visible) = (id, s, hwnd, s & WS_VISIBLE != 0);
            }
        }
        if let Some(w) = self.get_window_mut(hwnd) {
            w.combo = Some(ComboState { entry, list, ..Default::default() });
        }
        self.layout_combo(hwnd);
    }

    /// Create the parts of a new combination box or spin button `hwnd`
    /// (nothing for other windows).
    pub fn build_combo_or_spin(&mut self, hwnd: u32) {
        match self.get_window(hwnd).map(|w| w.class_name.as_str()) {
            Some(CLASS_COMBOBOX) => self.build_combo(hwnd),
            Some(super::pm_spin::CLASS_SPINBUTTON) => self.build_spin(hwnd),
            _ => {}
        }
    }

    /// Place the parts of combination box `hwnd` for its size.
    pub fn layout_combo(&mut self, hwnd: u32) {
        let Some((style, cx, cy, c)) = self.get_window(hwnd).and_then(|w| Some((w.style, w.cx, w.cy, w.combo?)))
        else { return };
        let [entry, list, _] = combo_layout(style, cx, cy);
        for (h, (x, y, w, hgt)) in [(c.entry, entry), (c.list, list)] {
            if let Some(win) = self.get_window_mut(h) {
                (win.x, win.y, win.cx, win.cy) = (x, y, w, hgt);
            }
        }
    }

    /// Visible dropped lists of `hwnd`'s frame that overlap `hwnd`: a
    /// control repainted under one must have the list painted over it again.
    pub fn dropped_lists_over(&self, hwnd: u32) -> Vec<u32> {
        if self.is_dropped_list(hwnd) || self.combo_of(hwnd).is_some() || self.is_combo(hwnd) {
            return Vec::new();
        }
        let frame = self.find_frame_for_hwnd(hwnd);
        let Some(root) = self.control_root(frame) else { return Vec::new() };
        let (x, y, cx, cy) = self.get_abs_rect_in_frame(hwnd);
        self.builtin_control_descendants(root).into_iter().filter(|&h| {
            if !self.is_dropped_list(h) { return false; }
            let (lx, ly, lcx, lcy) = self.get_abs_rect_in_frame(h);
            lx < x + cx && x < lx + lcx && ly < y + cy && y < ly + lcy
        }).collect()
    }
}

impl Loader {
    /// `WinCreateWindow` made combination box or spin button `hwnd`: create
    /// its parts; visible ones paint themselves when first dispatched.
    pub(super) fn build_compound_control(&self, hwnd: u32) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        wm.build_combo_or_spin(hwnd);
        let visible = wm.get_window(hwnd).is_some_and(|w| w.visible);
        let parts = wm.get_window(hwnd).map(|w| w.children.clone()).unwrap_or_default();
        if visible {
            for part in parts {
                if wm.get_window(part).is_some_and(|w| w.visible) {
                    wm.invalidate_window_only(part);
                }
            }
        }
    }

    /// The text of the field of a combination box or spin button was set
    /// (`WinSetWindowText` / `WinSetDlgItemText` on the control): the caret
    /// goes to its end and the field is repainted.
    pub(super) fn set_field_text_done(&self, field: u32) {
        if let Some(w) = self.shared.window_mgr.lock_or_recover().get_window_mut(field) {
            let n = w.text.chars().count();
            (w.edit_anchor, w.edit_caret, w.edit_scroll) = (n, n, 0);
        }
        self.repaint_control(field);
    }

    /// Messages to combination box `hwnd`; `None` for messages it leaves to
    /// the generic handling.
    pub(super) fn combo_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let (style, c) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let w = wm.get_window(hwnd)?;
            (w.style, w.combo?)
        };
        Some(match msg {
            WM_BUTTON1DOWN | WM_BUTTON1DBLCLK => {
                // The arrow button (the rest of the box is its parts').
                if style & WS_DISABLED == 0 && drops_down(style) {
                    self.set_focus_window(c.entry);
                    self.combo_show_list(hwnd, !c.showing);
                }
                ApiResult::Normal(1)
            }
            WM_CONTROL => {
                self.combo_child_notify(hwnd, mp1 & 0xFFFF, mp1 >> 16);
                ApiResult::Normal(0)
            }
            WM_CHAR => ApiResult::Normal(self.combo_char(hwnd, mp1, mp2) as u32),
            CBM_SHOWLIST => ApiResult::Normal(self.combo_show_list(hwnd, mp1 & 0xFFFF != 0) as u32),
            CBM_ISLISTSHOWING => ApiResult::Normal((c.showing || !drops_down(style)) as u32),
            CBM_HILITE => {
                let on = mp1 & 0xFFFF != 0;
                let changed = on != c.hilite;
                if changed {
                    if let Some(s) = self.shared.window_mgr.lock_or_recover()
                        .get_window_mut(hwnd).and_then(|w| w.combo.as_mut()) {
                        s.hilite = on;
                    }
                    self.repaint_control(c.entry);
                }
                ApiResult::Normal(changed as u32)
            }
            LM_INSERTITEM | LM_QUERYITEMCOUNT | LM_SETTOPINDEX | LM_DELETEITEM | LM_SELECTITEM
            | LM_QUERYSELECTION | LM_SETITEMTEXT | LM_QUERYITEMTEXTLENGTH | LM_QUERYITEMTEXT
            | LM_SETITEMHANDLE | LM_QUERYITEMHANDLE | LM_SEARCHSTRING | LM_SETITEMHEIGHT
            | LM_QUERYTOPINDEX | LM_DELETEALL => {
                let r = self.dispatch_builtin_control(c.list, msg, mp1, mp2);
                if msg == LM_SELECTITEM && matches!(r, ApiResult::Normal(1)) {
                    self.combo_take_selection(hwnd);
                }
                r
            }
            EM_QUERYCHANGED..=EM_SETINSERTMODE => self.dispatch_builtin_control(c.entry, msg, mp1, mp2),
            _ => return None,
        })
    }

    /// `WM_CONTROL(code)` from part `id` of combination box `hwnd`.
    fn combo_child_notify(&self, hwnd: u32, id: u32, code: u32) {
        match (id, code) {
            (CBID_EDIT, EN_CHANGE) => self.notify_combo_owner(hwnd, CBN_EFCHANGE),
            (CBID_EDIT, EN_SCROLL) => self.notify_combo_owner(hwnd, CBN_EFSCROLL),
            (CBID_EDIT, EN_MEMERROR) => self.notify_combo_owner(hwnd, CBN_MEMERROR),
            // The focus left the combination box: the list closes.
            (CBID_EDIT, EN_KILLFOCUS) => {
                let focus = self.shared.window_mgr.lock_or_recover().focus_hwnd;
                let inside = self.shared.window_mgr.lock_or_recover().combo_of(focus) == Some(hwnd);
                if !inside {
                    self.combo_show_list(hwnd, false);
                }
            }
            (CBID_LIST, LN_SELECT) => {
                self.combo_take_selection(hwnd);
                self.notify_combo_owner(hwnd, CBN_LBSELECT);
            }
            (CBID_LIST, LN_SCROLL) => self.notify_combo_owner(hwnd, CBN_LBSCROLL),
            (CBID_LIST, LN_ENTER) => {
                self.combo_show_list(hwnd, false);
                self.notify_combo_owner(hwnd, CBN_ENTER);
            }
            _ => {}
        }
    }

    /// Post `WM_CONTROL(code)` from combination box `hwnd` to its owner.
    fn notify_combo_owner(&self, hwnd: u32, code: u32) {
        self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, code, hwnd);
    }

    /// Put the text of the list's selected item into the entry field of
    /// combination box `hwnd` (all of it selected, as PM shows a picked
    /// item — unverified), telling the owner `CBN_EFCHANGE` when it changed.
    pub(super) fn combo_take_selection(&self, hwnd: u32) {
        let changed = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(c) = wm.get_window(hwnd).and_then(|w| w.combo) else { return };
            let Some(text) = wm.get_window(c.list)
                .and_then(|l| l.listbox_sel.next_after(None).and_then(|i| l.listbox_items.get(i).cloned()))
            else { return };
            let Some(e) = wm.get_window_mut(c.entry) else { return };
            let changed = e.text != text;
            let n = text.chars().count();
            e.text = text;
            (e.edit_anchor, e.edit_caret, e.edit_scroll) = (0, n, 0);
            changed.then_some(c.entry)
        };
        if let Some(entry) = changed {
            self.repaint_control(entry);
            self.notify_combo_owner(hwnd, CBN_EFCHANGE);
        }
    }

    /// Show (`show`) or hide the list of drop-down combination box `hwnd`.
    /// Returns whether its state changed (never for `CBS_SIMPLE`).
    pub(super) fn combo_show_list(&self, hwnd: u32, show: bool) -> bool {
        let (c, parent, (x, y)) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window(hwnd) else { return false };
            let Some(c) = w.combo else { return false };
            if !drops_down(w.style) || c.showing == show {
                return false;
            }
            (c, w.parent, (w.x, w.y))
        };
        if show {
            // "The list box is about to be displayed."
            self.notify_combo_owner(hwnd, CBN_SHOWLIST);
        }
        let list_rect = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if let Some(s) = wm.get_window_mut(hwnd).and_then(|w| w.combo.as_mut()) {
                s.showing = show;
            }
            let Some(l) = wm.get_window_mut(c.list) else { return true };
            l.visible = show;
            if show {
                // The cursor on the selected item, scrolled into view.
                if let Some(i) = l.listbox_sel.next_after(None) {
                    l.top_row = i;
                }
            }
            (l.x, l.y, l.cx, l.cy)
        };
        if show {
            self.repaint_control(c.list);
        } else {
            // What lay under the list shows again.
            let (lx, ly, lcx, lcy) = list_rect;
            let r = super::pm_rect::Rectl::new(x + lx, y + ly, x + lx + lcx, y + ly + lcy);
            self.shared.window_mgr.lock_or_recover().invalidate(parent, Some(r), true);
        }
        true
    }

    /// A key the entry field of combination box `hwnd` passed on. Returns
    /// whether the combination box used it; others go on to its owner.
    fn combo_char(&self, hwnd: u32, mp1: u32, mp2: u32) -> bool {
        let (style, c) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            match wm.get_window(hwnd) {
                Some(w) if w.combo.is_some() => (w.style, w.combo.unwrap_or_default()),
                _ => return false,
            }
        };
        let fs = mp1 & 0xFFFF;
        let up = fs & KC_KEYUP != 0;
        let vk = if fs & KC_VIRTUALKEY != 0 { mp2 >> WM_CHAR_VKEY_SHIFT } else { 0 };
        let alt = fs & KC_ALT != 0;
        let used = match vk {
            VK_DOWN if alt => { if !up && drops_down(style) { self.combo_show_list(hwnd, !c.showing); } true }
            VK_F4 => { if !up && drops_down(style) { self.combo_show_list(hwnd, !c.showing); } true }
            VK_UP | VK_DOWN | VK_PAGEUP | VK_PAGEDOWN if !alt => {
                // The list moves its selection; the entry field follows.
                self.dispatch_builtin_control(c.list, WM_CHAR, mp1, mp2);
                if !up { self.combo_take_selection(hwnd); }
                true
            }
            VK_NEWLINE | VK_ENTER => {
                if !up {
                    self.notify_combo_owner(hwnd, CBN_ENTER);
                }
                if c.showing {
                    if !up { self.combo_show_list(hwnd, false); }
                    true
                } else {
                    false
                }
            }
            VK_ESC if c.showing => { if !up { self.combo_show_list(hwnd, false); } true }
            _ => false,
        };
        if !used {
            let owner = self.shared.window_mgr.lock_or_recover().notify_target(hwnd);
            if owner != 0 && owner != hwnd {
                self.post_control_message(owner, WM_CHAR, mp1, mp2);
            }
        }
        used
    }

    /// A part of a combination box was pressed with button 1 (`hwnd`, after
    /// the part's own handling): the read-only field of a
    /// `CBS_DROPDOWNLIST` drops or closes the list.
    pub(super) fn combo_part_pressed(&self, hwnd: u32) {
        let target = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.combo_of(hwnd).and_then(|combo| {
                let w = wm.get_window(combo)?;
                let c = w.combo?;
                (c.entry == hwnd && kind(w.style) == CBS_DROPDOWNLIST).then_some((combo, c.showing))
            })
        };
        if let Some((combo, showing)) = target {
            self.combo_show_list(combo, !showing);
        }
    }

    /// Button 1 was released on a part of a combination box: an item picked
    /// in a dropped list closes it.
    pub(super) fn combo_part_released(&self, hwnd: u32) {
        let combo = {
            let wm = self.shared.window_mgr.lock_or_recover();
            if !wm.is_dropped_list(hwnd) { return; }
            wm.combo_of(hwnd)
        };
        if let Some(combo) = combo {
            self.combo_show_list(combo, false);
        }
    }

    /// WC_COMBOBOX: the arrow button of a drop-down combination box (its
    /// parts paint themselves).
    pub(super) fn paint_combo(&self, p: &Painter, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32)) {
        let Some(style) = self.shared.window_mgr.lock_or_recover().get_window(hwnd).map(|w| w.style) else { return };
        let [_, _, (ax, ay, aw, ah)] = combo_layout(style, cx, cy);
        if aw <= 0 || ah <= 0 {
            return;
        }
        let r = (x + ax, y + ay, aw, ah);
        control_box(p, (r.0, r.1, aw - 1, ah - 1), CONTROL_FACE_COLOR);
        let color = if style & WS_DISABLED != 0 { super::CONTROL_BORDER_COLOR } else { CONTROL_TEXT_COLOR };
        super::pm_scrollbar::draw_arrow(p, r, true, false, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::pm_types::OS2Message;

    struct Fx {
        loader: Loader,
        hmq: u32,
        client: u32,
        frame: u32,
    }

    fn fixture() -> Fx {
        let loader = Loader::new_mock();
        let (hmq, frame, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            wm.register_class("App".into(), 0x5000, 0);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            for h in [frame, client] {
                let w = wm.get_window_mut(h).unwrap();
                (w.cx, w.cy, w.visible) = (400, 300, true);
            }
            (hmq, frame, client)
        };
        Fx { loader, hmq, client, frame }
    }

    impl Fx {
        fn combo(&self, style: u32, rect: (i32, i32, i32, i32)) -> (u32, ComboState) {
            let mut wm = self.loader.shared.window_mgr.lock_or_recover();
            let h = wm.create_window(CLASS_COMBOBOX.into(), self.client, self.hmq);
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy) = rect;
            (w.style, w.id, w.owner, w.visible) = (style | WS_VISIBLE, 42, self.client, true);
            wm.build_combo(h);
            let c = wm.get_window(h).unwrap().combo.unwrap();
            (h, c)
        }
        fn send(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(hwnd, msg, mp1, mp2) {
                ApiResult::Normal(v) => v,
                other => panic!("unexpected {other:?}"),
            }
        }
        fn insert(&self, hwnd: u32, items: &[&str]) {
            for (i, t) in items.iter().enumerate() {
                let at = 0x0008_0000 + 64 * i as u32;
                self.loader.guest_write_bytes(at, format!("{t}\0").as_bytes()).unwrap();
                self.send(hwnd, LM_INSERTITEM, LIT_END, at);
            }
        }
        fn posted(&self) -> Vec<OS2Message> {
            let mq = self.loader.shared.window_mgr.lock_or_recover().get_mq(self.hmq).unwrap();
            mq.lock_or_recover().messages.drain(..).collect()
        }
        /// Deliver the posted messages for built-in windows (as WinDispatchMsg
        /// would) and return those for the client.
        fn pump(&self) -> Vec<(u32, u32)> {
            let mut out = Vec::new();
            for _ in 0..8 {
                let msgs = self.posted();
                if msgs.is_empty() { break; }
                for m in msgs {
                    if m.hwnd == self.client {
                        if m.msg == WM_CONTROL { out.push((m.mp1 >> 16, m.mp2)); }
                    } else {
                        self.loader.dispatch_builtin_control(m.hwnd, m.msg, m.mp1, m.mp2);
                    }
                }
            }
            out
        }
        fn text(&self, hwnd: u32) -> String {
            let wm = self.loader.shared.window_mgr.lock_or_recover();
            wm.get_window(wm.text_window(hwnd)).unwrap().text.clone()
        }
        fn visible(&self, hwnd: u32) -> bool {
            self.loader.shared.window_mgr.lock_or_recover().get_window(hwnd).unwrap().visible
        }
        fn vkey(&self, hwnd: u32, vk: u32, kc: u32) -> u32 {
            self.send(hwnd, WM_CHAR, KC_VIRTUALKEY | KC_SCANCODE | kc, vk << WM_CHAR_VKEY_SHIFT)
        }
    }

    fn pos(x: i32, y: i32) -> u32 { (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16) }

    #[test]
    fn the_layout_puts_the_entry_field_on_top_and_the_list_below() {
        let [entry, list, arrow] = combo_layout(CBS_DROPDOWN, 120, 100);
        assert_eq!(entry, (0, 100 - COMBO_ENTRY_H, 120 - COMBO_ARROW_W, COMBO_ENTRY_H));
        assert_eq!(arrow, (120 - COMBO_ARROW_W, 100 - COMBO_ENTRY_H, COMBO_ARROW_W, COMBO_ENTRY_H));
        assert_eq!(list, (0, 0, 120, 100 - COMBO_ENTRY_H));
        let [entry, _, arrow] = combo_layout(CBS_SIMPLE, 120, 100);
        assert_eq!((entry.2, arrow.2), (120, 0), "no arrow for CBS_SIMPLE");
        // A drop-down box with no room for its list drops it below itself.
        let [_, list, _] = combo_layout(CBS_DROPDOWNLIST, 120, COMBO_ENTRY_H);
        assert_eq!((list.1, list.3), (-COMBO_MIN_LIST_H, COMBO_MIN_LIST_H));
    }

    #[test]
    fn a_combination_box_is_built_from_an_entry_field_and_a_list_box() {
        let fx = fixture();
        let (combo, c) = fx.combo(CBS_DROPDOWN | WS_TABSTOP, (10, 10, 120, 100));
        let wm = fx.loader.shared.window_mgr.lock_or_recover();
        let (e, l) = (wm.get_window(c.entry).unwrap(), wm.get_window(c.list).unwrap());
        assert_eq!((e.class_name.as_str(), e.id, e.owner), ("WC_ENTRYFIELD", CBID_EDIT, combo));
        assert_eq!((l.class_name.as_str(), l.id, l.owner), ("WC_LISTBOX", CBID_LIST, combo));
        assert!(e.visible && !l.visible, "the drop-down list starts hidden");
        assert!(e.style & WS_TABSTOP != 0, "the field is the tab stop");
        assert_eq!(wm.focus_window_for(combo), c.entry, "the field takes the focus");
        assert_eq!(wm.focus_window_for(c.list), c.entry);
        drop(wm);
        let (_, s) = fx.combo(CBS_SIMPLE, (200, 10, 120, 100));
        let (_, d) = fx.combo(CBS_DROPDOWNLIST, (200, 150, 120, 100));
        let wm = fx.loader.shared.window_mgr.lock_or_recover();
        assert!(wm.get_window(s.list).unwrap().visible, "CBS_SIMPLE shows its list");
        assert!(wm.get_window(d.entry).unwrap().style & ES_READONLY != 0);
    }

    #[test]
    fn showlist_drops_the_list_notifies_and_hides_it_again() {
        let fx = fixture();
        let (combo, c) = fx.combo(CBS_DROPDOWN, (10, 10, 120, 100));
        assert_eq!(fx.send(combo, CBM_ISLISTSHOWING, 0, 0), 0);
        assert_eq!(fx.send(combo, CBM_SHOWLIST, 1, 0), 1);
        assert_eq!(fx.send(combo, CBM_SHOWLIST, 1, 0), 0, "no change");
        assert_eq!(fx.send(combo, CBM_ISLISTSHOWING, 0, 0), 1);
        assert!(fx.visible(c.list));
        assert_eq!(fx.pump(), vec![(CBN_SHOWLIST, combo)]);
        assert!(fx.loader.shared.window_mgr.lock_or_recover().is_dropped_list(c.list));
        assert_eq!(fx.send(combo, CBM_SHOWLIST, 0, 0), 1);
        assert!(!fx.visible(c.list));
        // The parent repaints what the list covered.
        let update = fx.loader.shared.window_mgr.lock_or_recover().update_rect(fx.client);
        assert!(!update.is_empty());
        let (simple, _) = fx.combo(CBS_SIMPLE, (200, 10, 120, 100));
        assert_eq!(fx.send(simple, CBM_SHOWLIST, 1, 0), 0, "no effect on CBS_SIMPLE");
        assert_eq!(fx.send(simple, CBM_ISLISTSHOWING, 0, 0), 1);
    }

    #[test]
    fn list_messages_go_to_the_list_and_a_selection_fills_the_entry_field() {
        let fx = fixture();
        let (combo, c) = fx.combo(CBS_DROPDOWN, (10, 10, 120, 100));
        fx.insert(combo, &["Red", "Green", "Blue"]);
        assert_eq!(fx.send(combo, LM_QUERYITEMCOUNT, 0, 0), 3);
        assert_eq!(fx.send(c.list, LM_QUERYITEMCOUNT, 0, 0), 3);
        fx.pump();
        assert_eq!(fx.send(combo, LM_SELECTITEM, 1, 1), 1);
        assert_eq!(fx.text(combo), "Green", "at once, before any notification");
        let notes = fx.pump();
        assert!(notes.contains(&(CBN_LBSELECT, combo)), "{notes:?}");
        assert!(notes.contains(&(CBN_EFCHANGE, combo)), "{notes:?}");
        // EM_* go to the entry field.
        assert_eq!(fx.send(combo, EM_SETSEL, 2 << 16, 0), 1);
        assert_eq!(fx.send(combo, EM_QUERYSEL, 0, 0), 2 << 16);
    }

    #[test]
    fn keys_drop_the_list_move_the_selection_and_enter_closes_it() {
        let fx = fixture();
        let (combo, c) = fx.combo(CBS_DROPDOWN, (10, 10, 120, 100));
        fx.insert(combo, &["Red", "Green", "Blue"]);
        fx.pump();
        // Alt+Down in the entry field reaches the combination box.
        fx.vkey(c.entry, VK_DOWN, KC_ALT);
        fx.pump();
        assert!(fx.visible(c.list), "Alt+Down drops the list");
        fx.vkey(combo, VK_DOWN, 0);
        fx.vkey(combo, VK_DOWN, 0);
        assert_eq!(fx.text(combo), "Green");
        fx.pump();
        fx.vkey(combo, VK_NEWLINE, 0);
        assert!(!fx.visible(c.list), "Enter closes the list");
        assert!(fx.pump().contains(&(CBN_ENTER, combo)));
        fx.vkey(combo, VK_F4, 0);
        assert!(fx.visible(c.list), "F4 drops it");
        fx.vkey(combo, VK_ESC, 0);
        assert!(!fx.visible(c.list), "Esc closes it");
    }

    #[test]
    fn clicking_the_arrow_drops_the_list_and_a_picked_item_closes_it() {
        let fx = fixture();
        let (combo, c) = fx.combo(CBS_DROPDOWN, (10, 10, 120, 100));
        fx.insert(combo, &["Red", "Green", "Blue"]);
        fx.pump();
        // The pointer finds the arrow in the entry row, the field left of it.
        let (ax, ay, _, _) = fx.loader.shared.window_mgr.lock_or_recover().get_abs_rect_in_frame(combo);
        let wm = fx.loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.control_at(fx.frame, ax + 115, ay + 95).map(|h| h.0), Some(combo));
        assert_eq!(wm.control_at(fx.frame, ax + 5, ay + 95).map(|h| h.0), Some(c.entry));
        assert_eq!(wm.control_at(fx.frame, ax + 5, ay + 20), None, "transparent below while closed");
        drop(wm);
        fx.send(combo, WM_BUTTON1DOWN, pos(115, 95), 0);
        assert!(fx.visible(c.list));
        let wm = fx.loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.control_at(fx.frame, ax + 5, ay + 20).map(|h| h.0), Some(c.list));
        assert_eq!(*wm.builtin_control_descendants(fx.client).last().unwrap(), c.list, "painted last");
        drop(wm);
        let lcy = 100 - COMBO_ENTRY_H;
        // The second row of the list.
        let y = lcy - 2 - CONTROL_LINE_H - CONTROL_LINE_H / 2;
        fx.send(c.list, WM_BUTTON1DOWN, pos(5, y), 0);
        fx.send(c.list, WM_BUTTON1UP, pos(5, y), 0);
        assert!(!fx.visible(c.list), "released on an item: closed");
        let notes = fx.pump();
        assert_eq!(fx.text(combo), "Green");
        assert!(notes.contains(&(CBN_LBSELECT, combo)), "{notes:?}");
        let focus = fx.loader.shared.window_mgr.lock_or_recover().focus_hwnd;
        assert_eq!(focus, c.entry, "the list never takes the focus");
    }

    #[test]
    fn a_dropdownlist_field_is_read_only_and_a_click_on_it_drops_the_list() {
        let fx = fixture();
        let (combo, c) = fx.combo(CBS_DROPDOWNLIST, (10, 10, 120, 100));
        fx.insert(combo, &["One", "Two"]);
        fx.send(c.entry, WM_BUTTON1DOWN, pos(5, 5), 0);
        assert!(fx.visible(c.list));
        fx.send(c.entry, WM_CHAR, KC_CHAR, b'x' as u32);
        assert_eq!(fx.text(combo), "");
    }

    #[test]
    fn typing_in_the_field_tells_the_owner_cbn_efchange_and_the_window_text_is_the_fields() {
        let fx = fixture();
        let (combo, c) = fx.combo(CBS_DROPDOWN, (10, 10, 120, 100));
        fx.loader.set_focus_window(combo);
        fx.send(c.entry, WM_CHAR, KC_CHAR, b'a' as u32);
        assert_eq!(fx.text(combo), "a");
        assert!(fx.pump().contains(&(CBN_EFCHANGE, combo)));
        assert_eq!(fx.send(combo, CBM_HILITE, 1, 0), 1);
        assert_eq!(fx.send(combo, CBM_HILITE, 1, 0), 0);
    }

    #[test]
    fn losing_the_focus_closes_the_list() {
        let fx = fixture();
        let (combo, c) = fx.combo(CBS_DROPDOWN, (10, 10, 120, 100));
        fx.loader.set_focus_window(combo);
        fx.send(combo, CBM_SHOWLIST, 1, 0);
        fx.pump();
        fx.loader.set_focus_window(0);
        fx.pump();
        assert!(!fx.visible(c.list));
    }

    #[test]
    fn a_control_repainted_under_the_dropped_list_repaints_the_list() {
        let fx = fixture();
        let (combo, c) = fx.combo(CBS_DROPDOWN, (10, 100, 120, 100));
        let below = {
            let mut wm = fx.loader.shared.window_mgr.lock_or_recover();
            let h = wm.create_window("WC_BUTTON".into(), fx.client, fx.hmq);
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy, w.visible) = (10, 110, 80, 30, true);
            h
        };
        fx.send(combo, CBM_SHOWLIST, 1, 0);
        assert_eq!(fx.loader.shared.window_mgr.lock_or_recover().dropped_lists_over(below), vec![c.list]);
        assert!(fx.loader.shared.window_mgr.lock_or_recover().dropped_lists_over(c.list).is_empty());
    }
}
