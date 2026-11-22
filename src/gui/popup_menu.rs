// SPDX-License-Identifier: GPL-3.0-or-later

//! A pop-up menu drawn by the renderer as an overlay on the desktop: a list
//! of items (id, text with a `~` mnemonic, accelerator text, enabled,
//! checked, separators), chosen with the mouse or the keyboard. The VIO
//! window's system menu uses it; it does not depend on a guest menu window,
//! so frames whose menus Warpine draws itself (a PM frame's system menu) can
//! use it too.
//!
//! It looks like the pull-downs of the menu bar (`draw_menu_pulldown` in
//! `sdl_renderer.rs`). Keyboard handling is modelled on PM menus: Up / Down
//! move the selection, Home / End go to the first / last item, Enter
//! chooses, Esc closes, a letter chooses the item with that mnemonic.
//! Which items the arrows skip (separators and disabled items) and that a
//! click on a disabled item keeps the menu open are Warpine's choices.

use super::damage::{fill_clipped, Rect};
use super::font::{self, FontSpec};
use crate::loader::constants::MENU_DROPDOWN_ITEM_H;
use crate::loader::pm_types::menu_label;

/// Menu background, edge, text and highlight colours (shared with the menu
/// bar and its pull-downs).
pub(super) const MENU_BG_COLOR: u32 = 0x00D4_D0C8;
pub(super) const MENU_EDGE_COLOR: u32 = 0x0080_8080;
pub(super) const MENU_TEXT_COLOR: u32 = 0x0000_0000;
pub(super) const MENU_HILITE_COLOR: u32 = 0x0000_0080;
pub(super) const MENU_HILITE_TEXT: u32 = 0x00FF_FFFF;
/// Text of a disabled (greyed) item.
pub(super) const MENU_DISABLED_TEXT: u32 = 0x0080_8080;

/// Height of an item row and of a separator row.
pub const POPUP_ITEM_H: i32 = MENU_DROPDOWN_ITEM_H;
pub const POPUP_SEPARATOR_H: i32 = 8;
/// Left margin, holding the check mark of a checked item.
pub const POPUP_CHECK_W: i32 = 16;
/// Right margin, and the gap between an item's text and its accelerator.
pub const POPUP_RIGHT_PAD: i32 = 8;
pub const POPUP_ACCEL_GAP: i32 = 24;
/// Width of the menu's edge.
const POPUP_EDGE: i32 = 1;

/// One item of a [`PopupMenu`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopupItem {
    /// What choosing the item reports.
    pub id: u32,
    /// Label; `~` marks the mnemonic letter that follows it.
    pub text: String,
    /// Accelerator shown right-aligned ("Alt+F4"), or empty.
    pub accel: String,
    pub enabled: bool,
    pub checked: bool,
    pub separator: bool,
}

impl PopupItem {
    /// An item that can be chosen when `enabled`.
    pub fn new(id: u32, text: &str, accel: &str, enabled: bool) -> PopupItem {
        PopupItem { id, text: text.into(), accel: accel.into(), enabled, checked: false, separator: false }
    }

    /// A horizontal rule between groups of items.
    pub fn separator() -> PopupItem {
        PopupItem { id: 0, text: String::new(), accel: String::new(), enabled: false, checked: false,
                    separator: true }
    }

    /// The item with a check mark when `checked`.
    pub fn checked(self, checked: bool) -> PopupItem {
        PopupItem { checked, ..self }
    }

    /// Whether the item can be selected and chosen.
    fn selectable(&self) -> bool {
        self.enabled && !self.separator
    }

    /// The mnemonic letter (after `~`), lowercase.
    fn mnemonic(&self) -> Option<char> {
        let mut chars = self.text.chars();
        while let Some(c) = chars.next() {
            if c == '~' {
                return chars.next().map(|m| m.to_lowercase().next().unwrap_or(m));
            }
        }
        None
    }

    fn height(&self) -> i32 {
        if self.separator { POPUP_SEPARATOR_H } else { POPUP_ITEM_H }
    }
}

/// A key that works an open pop-up menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKey { Up, Down, Home, End, Enter, Esc, Char(char) }

/// What an input did to the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupAction {
    /// The menu stays open (its look may have changed).
    Stay,
    /// The menu closes without a choice.
    Close,
    /// The item with this id was chosen; the menu closes.
    Choose(u32),
}

/// An open pop-up menu, placed on the desktop (screen coordinates).
pub struct PopupMenu {
    items: Vec<PopupItem>,
    /// Where it is on the desktop.
    rect: Rect,
    /// The highlighted item.
    selected: Option<usize>,
    /// The item button 1 went down on (chosen when it comes up over it).
    pressed: bool,
    /// The menu as last drawn (`rect`-sized), and whether it is out of date.
    pixels: Vec<u32>,
    stale: bool,
}

impl PopupMenu {
    /// Open a menu of `items` with its top-left corner at screen `(x, y)`,
    /// moved as needed to lie on a `desktop` (width, height) desktop. With
    /// `select_first` (opened from the keyboard) the first item that can be
    /// chosen is highlighted.
    pub fn new(items: Vec<PopupItem>, x: i32, y: i32, desktop: (i32, i32), select_first: bool) -> PopupMenu {
        let spec = FontSpec::SYSTEM;
        let text_w = items.iter().filter(|i| !i.separator)
            .map(|i| font::text_width(&menu_label(&i.text), spec)).max().unwrap_or(0);
        let accel_w = items.iter().map(|i| font::text_width(&i.accel, spec)).max().unwrap_or(0);
        let w = 2 * POPUP_EDGE + POPUP_CHECK_W + text_w
            + if accel_w > 0 { POPUP_ACCEL_GAP + accel_w } else { 0 } + POPUP_RIGHT_PAD;
        let h = 2 * POPUP_EDGE + items.iter().map(PopupItem::height).sum::<i32>();
        let x = x.min(desktop.0 - w).max(0);
        let y = y.min(desktop.1 - h).max(0);
        let selected = if select_first { items.iter().position(PopupItem::selectable) } else { None };
        PopupMenu { items, rect: Rect::at(x, y, w, h), selected, pressed: false, pixels: Vec::new(), stale: true }
    }

    /// Where the menu is on the desktop.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn items(&self) -> &[PopupItem] {
        &self.items
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Top of item `index` relative to the menu's top edge.
    fn item_top(&self, index: usize) -> i32 {
        POPUP_EDGE + self.items[..index].iter().map(PopupItem::height).sum::<i32>()
    }

    /// The index of the item `id` (not a separator), if any.
    fn index_of(&self, id: u32) -> Option<usize> {
        self.items.iter().position(|i| !i.separator && i.id == id)
    }

    /// The middle of item `id`'s row, relative to the menu's top edge.
    pub fn item_middle(&self, id: u32) -> Option<i32> {
        self.index_of(id).map(|i| self.item_top(i) + self.items[i].height() / 2)
    }

    /// Highlight item `id` when it can be chosen.
    pub fn highlight(&mut self, id: u32) {
        if let Some(i) = self.index_of(id).filter(|&i| self.items[i].selectable()) {
            self.select(Some(i));
        }
    }

    /// The item under screen point `(x, y)`, if any.
    pub fn item_at(&self, x: i32, y: i32) -> Option<usize> {
        if !self.rect.contains(&Rect::at(x, y, 1, 1)) { return None; }
        let mut top = self.rect.y0 + POPUP_EDGE;
        for (i, item) in self.items.iter().enumerate() {
            if y >= top && y < top + item.height() { return Some(i); }
            top += item.height();
        }
        None
    }

    /// Whether screen point `(x, y)` is on the menu.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.rect.contains(&Rect::at(x, y, 1, 1))
    }

    fn select(&mut self, index: Option<usize>) -> bool {
        let changed = self.selected != index;
        self.selected = index;
        self.stale |= changed;
        changed
    }

    /// The next item that can be chosen after (`step` 1) or before
    /// (`step` -1) the selection, wrapping around.
    fn step(&self, step: isize) -> Option<usize> {
        let n = self.items.len() as isize;
        if n == 0 { return None; }
        let start = self.selected.map_or(if step > 0 { -1 } else { n }, |s| s as isize);
        (1..=n).map(|k| (start + k * step).rem_euclid(n) as usize)
            .find(|&i| self.items[i].selectable())
    }

    /// Work the menu with key `key`.
    pub fn key(&mut self, key: PopupKey) -> PopupAction {
        match key {
            PopupKey::Esc => PopupAction::Close,
            PopupKey::Enter => match self.selected {
                Some(i) if self.items[i].selectable() => PopupAction::Choose(self.items[i].id),
                _ => PopupAction::Stay,
            },
            PopupKey::Down | PopupKey::Up => {
                let next = self.step(if key == PopupKey::Down { 1 } else { -1 });
                self.select(next.or(self.selected));
                PopupAction::Stay
            }
            PopupKey::Home | PopupKey::End => {
                let found = if key == PopupKey::Home {
                    self.items.iter().position(PopupItem::selectable)
                } else {
                    self.items.iter().rposition(PopupItem::selectable)
                };
                self.select(found.or(self.selected));
                PopupAction::Stay
            }
            PopupKey::Char(c) => {
                let c = c.to_lowercase().next().unwrap_or(c);
                match self.items.iter().position(|i| !i.separator && i.mnemonic() == Some(c)) {
                    Some(i) if self.items[i].enabled => PopupAction::Choose(self.items[i].id),
                    _ => PopupAction::Stay,
                }
            }
        }
    }

    /// The pointer moved to screen `(x, y)`: highlight the item under it.
    /// Returns whether the look changed.
    pub fn hover(&mut self, x: i32, y: i32) -> bool {
        match self.item_at(x, y) {
            Some(i) if self.items[i].selectable() => self.select(Some(i)),
            Some(_) => self.select(None),
            None if self.pressed => self.select(None),
            None => false,
        }
    }

    /// Button 1 went down at screen `(x, y)`: on the menu it highlights the
    /// item there; elsewhere the menu closes (`Close`).
    pub fn press(&mut self, x: i32, y: i32) -> PopupAction {
        if !self.contains(x, y) { return PopupAction::Close; }
        self.pressed = true;
        self.hover(x, y);
        PopupAction::Stay
    }

    /// Button 1 came up at screen `(x, y)`: an item that can be chosen
    /// under the pointer is chosen, anything else keeps the menu open.
    pub fn release(&mut self, x: i32, y: i32) -> PopupAction {
        self.pressed = false;
        match self.item_at(x, y) {
            Some(i) if self.items[i].selectable() => PopupAction::Choose(self.items[i].id),
            _ => PopupAction::Stay,
        }
    }

    /// Whether button 1 went down on the menu and is still held.
    pub fn is_pressed(&self) -> bool {
        self.pressed
    }

    /// The menu's pixels (`rect`-sized, rows from the top), redrawn if its
    /// look changed since the last call.
    pub fn pixels(&mut self) -> &[u32] {
        if std::mem::take(&mut self.stale) || self.pixels.is_empty() {
            self.render();
        }
        &self.pixels
    }

    fn render(&mut self) {
        let (w, h) = (self.rect.width(), self.rect.height());
        let (bw, bh) = (w as usize, h as usize);
        let mut px = vec![MENU_EDGE_COLOR; bw * bh];
        let fill = |px: &mut Vec<u32>, r: Rect, c: u32| fill_clipped(px, bw, r, r, c);
        fill(&mut px, Rect::new(POPUP_EDGE, POPUP_EDGE, w - POPUP_EDGE, h - POPUP_EDGE), MENU_BG_COLOR);
        let spec = FontSpec::SYSTEM;
        let ascent = font::metrics(spec).ascent;
        for (i, item) in self.items.iter().enumerate() {
            let top = self.item_top(i);
            if item.separator {
                let y = top + POPUP_SEPARATOR_H / 2;
                fill(&mut px, Rect::new(POPUP_EDGE + 2, y, w - POPUP_EDGE - 2, y + 1), MENU_EDGE_COLOR);
                continue;
            }
            let hilite = self.selected == Some(i);
            if hilite {
                fill(&mut px, Rect::new(POPUP_EDGE, top, w - POPUP_EDGE, top + POPUP_ITEM_H), MENU_HILITE_COLOR);
            }
            let color = if !item.enabled { MENU_DISABLED_TEXT }
                else if hilite { MENU_HILITE_TEXT } else { MENU_TEXT_COLOR };
            if item.checked {
                draw_check(&mut px, bw, POPUP_EDGE + 4, top + POPUP_ITEM_H / 2, color);
            }
            let baseline = top + (POPUP_ITEM_H - font::metrics(spec).line_height) / 2 + ascent;
            font::draw_text(&mut px, bw, bh, POPUP_EDGE + POPUP_CHECK_W, baseline,
                            &menu_label(&item.text), color, spec);
            if !item.accel.is_empty() {
                let ax = w - POPUP_EDGE - POPUP_RIGHT_PAD - font::text_width(&item.accel, spec);
                font::draw_text(&mut px, bw, bh, ax, baseline, &item.accel, color, spec);
            }
        }
        self.pixels = px;
    }
}

/// A check mark 8 pels wide whose middle left is at `(x, y)`.
pub(super) fn draw_check(px: &mut [u32], w: usize, x: i32, y: i32, color: u32) {
    let mut dot = |x: i32, y: i32| {
        let r = Rect::at(x, y, 2, 2);
        fill_clipped(px, w, r, r, color);
    };
    for i in 0..3 { dot(x + i, y + i - 1); }
    for i in 0..5 { dot(x + 3 + i, y + 1 - i); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<PopupItem> {
        vec![
            PopupItem::new(1, "~Restore", "Alt+F5", false),
            PopupItem::new(2, "~Move", "Alt+F7", true),
            PopupItem::separator(),
            PopupItem::new(3, "~Close", "Alt+F4", true),
            PopupItem::new(4, "M~ark", "", true).checked(true),
        ]
    }

    fn menu() -> PopupMenu {
        PopupMenu::new(items(), 100, 50, (1024, 768), true)
    }

    #[test]
    fn the_menu_is_as_large_as_its_items_and_stays_on_the_desktop() {
        let m = menu();
        let r = m.rect();
        assert_eq!((r.x0, r.y0), (100, 50));
        assert_eq!(r.height(), 2 + 4 * POPUP_ITEM_H + POPUP_SEPARATOR_H);
        assert!(r.width() > POPUP_CHECK_W + POPUP_ACCEL_GAP);
        // Opened near the bottom-right corner: moved onto the desktop.
        let m = PopupMenu::new(items(), 1020, 760, (1024, 768), false);
        assert_eq!((m.rect().x1, m.rect().y1), (1024, 768));
        assert_eq!(m.selected(), None, "opened with the mouse: nothing highlighted");
    }

    #[test]
    fn arrows_skip_separators_and_disabled_items_and_wrap() {
        let mut m = menu();
        assert_eq!(m.selected(), Some(1), "the first item that can be chosen");
        assert_eq!(m.key(PopupKey::Down), PopupAction::Stay);
        assert_eq!(m.selected(), Some(3), "past the separator");
        m.key(PopupKey::Down);
        assert_eq!(m.selected(), Some(4));
        m.key(PopupKey::Down);
        assert_eq!(m.selected(), Some(1), "wrapped, past the disabled Restore");
        m.key(PopupKey::Up);
        assert_eq!(m.selected(), Some(4));
        m.key(PopupKey::Home);
        assert_eq!(m.selected(), Some(1));
        m.key(PopupKey::End);
        assert_eq!(m.selected(), Some(4));
        assert_eq!(m.key(PopupKey::Enter), PopupAction::Choose(4));
        assert_eq!(m.key(PopupKey::Esc), PopupAction::Close);
    }

    #[test]
    fn mnemonics_choose_enabled_items() {
        let mut m = menu();
        assert_eq!(m.key(PopupKey::Char('a')), PopupAction::Choose(4));
        assert_eq!(m.key(PopupKey::Char('C')), PopupAction::Choose(3), "case does not matter");
        assert_eq!(m.key(PopupKey::Char('r')), PopupAction::Stay, "Restore is disabled");
        assert_eq!(m.key(PopupKey::Char('z')), PopupAction::Stay);
    }

    #[test]
    fn clicks_choose_on_release_over_an_enabled_item() {
        let mut m = menu();
        let r = m.rect();
        let row = |i: i32| r.y0 + 1 + i * POPUP_ITEM_H + POPUP_ITEM_H / 2;
        assert_eq!(m.item_at(r.x0 + 5, row(0)), Some(0));
        assert_eq!(m.item_at(r.x0 + 5, r.y0 + 1 + 2 * POPUP_ITEM_H + 2), Some(2), "the separator");
        assert_eq!(m.item_at(r.x0 - 1, row(0)), None);
        assert_eq!(m.press(r.x0 + 5, row(1)), PopupAction::Stay);
        assert!(m.is_pressed());
        assert_eq!(m.selected(), Some(1));
        assert_eq!(m.release(r.x0 + 5, row(1)), PopupAction::Choose(2));
        assert!(!m.is_pressed());
        // A disabled item or the separator keeps the menu open.
        assert_eq!(m.release(r.x0 + 5, row(0)), PopupAction::Stay);
        // A press outside closes it.
        assert_eq!(m.press(r.x1 + 3, row(0)), PopupAction::Close);
    }

    #[test]
    fn items_can_be_found_and_highlighted_by_id() {
        let mut m = PopupMenu::new(items(), 0, 0, (1024, 768), false);
        assert_eq!(m.item_middle(2), Some(1 + POPUP_ITEM_H + POPUP_ITEM_H / 2));
        assert_eq!(m.item_middle(3), Some(1 + 2 * POPUP_ITEM_H + POPUP_SEPARATOR_H + POPUP_ITEM_H / 2));
        assert_eq!(m.item_middle(99), None);
        m.highlight(1);
        assert_eq!(m.selected(), None, "Restore is disabled");
        m.highlight(3);
        assert_eq!(m.selected(), Some(3));
        assert_eq!(m.key(PopupKey::Enter), PopupAction::Choose(3));
    }

    #[test]
    fn hovering_highlights_the_item_under_the_pointer() {
        let mut m = PopupMenu::new(items(), 0, 0, (1024, 768), false);
        let r = m.rect();
        assert!(m.hover(r.x0 + 5, r.y0 + 1 + POPUP_ITEM_H + 3));
        assert_eq!(m.selected(), Some(1));
        assert!(!m.hover(r.x0 + 5, r.y0 + 1 + POPUP_ITEM_H + 4), "same item");
        assert!(m.hover(r.x0 + 5, r.y0 + 2), "a disabled item clears the highlight");
        assert_eq!(m.selected(), None);
    }

    #[test]
    fn the_menu_draws_highlight_greyed_text_and_check_marks() {
        let mut m = menu();
        let r = m.rect();
        let w = r.width() as usize;
        let px = m.pixels().to_vec();
        assert_eq!(px.len(), w * r.height() as usize);
        let at = |x: i32, y: i32| px[y as usize * w + x as usize];
        assert_eq!(at(0, 0), MENU_EDGE_COLOR);
        let row_y = |i: i32| 1 + i * POPUP_ITEM_H;
        assert_eq!(at(w as i32 - 3, row_y(1) + 1), MENU_HILITE_COLOR, "Move is highlighted");
        assert_eq!(at(w as i32 - 3, row_y(0) + 1), MENU_BG_COLOR);
        let row = |i: i32| (row_y(i)..row_y(i) + POPUP_ITEM_H)
            .flat_map(|y| (0..w as i32).map(move |x| (x, y))).map(|(x, y)| at(x, y)).collect::<Vec<_>>();
        assert!(row(0).contains(&MENU_DISABLED_TEXT), "Restore is greyed");
        assert!(!row(1).contains(&MENU_TEXT_COLOR), "highlighted text is white");
        // Mark (row 4, after the separator) has a check mark in its margin.
        let mark_top = row_y(3) + POPUP_SEPARATOR_H;
        let check = (mark_top..mark_top + POPUP_ITEM_H)
            .any(|y| (1..POPUP_CHECK_W).any(|x| at(x, y) == MENU_TEXT_COLOR));
        assert!(check, "check mark drawn");
        // The look is redrawn only after a change.
        m.key(PopupKey::Down);
        assert_ne!(m.pixels(), &px[..]);
    }
}
