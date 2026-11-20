// SPDX-License-Identifier: GPL-3.0-or-later

//! Selection model of the built-in list box (WC_LISTBOX).
//!
//! Pure: no window manager, no painting. A list box selects in one of three
//! modes, chosen by its style:
//!
//! * **single** (default) — at most one item is selected; every click or
//!   cursor move selects the item it lands on.
//! * **multiple** (`LS_MULTIPLESEL`) — a click or Space toggles one item and
//!   leaves the others alone; the cursor keys only move the cursor.
//! * **extended** (`LS_EXTENDEDSEL`) — a click or cursor move selects just
//!   that item; with Shift held it selects the range from the anchor (the
//!   last item clicked without Shift) to it, and a Ctrl+click toggles one
//!   item, keeping the rest. Dragging extends the range.
//!
//! The *cursor* is the item the keyboard acts on; in single mode it is the
//! selected item.

use std::collections::BTreeSet;

use super::constants::{LS_EXTENDEDSEL, LS_MULTIPLESEL};

/// How a list box selects, from its `LS_*` style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMode {
    Single,
    Multiple,
    Extended,
}

impl ListMode {
    /// The mode of a list box with window style `style`. `LS_MULTIPLESEL`
    /// wins when both bits are set.
    pub fn from_style(style: u32) -> Self {
        if style & LS_MULTIPLESEL != 0 {
            ListMode::Multiple
        } else if style & LS_EXTENDEDSEL != 0 {
            ListMode::Extended
        } else {
            ListMode::Single
        }
    }
}

/// The selected items, cursor and range anchor of a list box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListSelection {
    selected: BTreeSet<usize>,
    cursor: Option<usize>,
    anchor: Option<usize>,
}

impl ListSelection {
    pub fn is_selected(&self, item: usize) -> bool {
        self.selected.contains(&item)
    }

    /// The item with the cursor, if any.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// The first selected item after `after` (`None` = from the top).
    pub fn next_after(&self, after: Option<usize>) -> Option<usize> {
        match after {
            None => self.selected.first().copied(),
            Some(a) => self.selected.range(a + 1..).next().copied(),
        }
    }

    /// Replace the selection with `items`; returns whether it changed.
    fn set(&mut self, items: impl IntoIterator<Item = usize>) -> bool {
        let new: BTreeSet<usize> = items.into_iter().collect();
        std::mem::replace(&mut self.selected, new) != self.selected
    }

    /// Flip `item`'s selection.
    fn toggle(&mut self, item: usize) {
        if !self.selected.remove(&item) {
            self.selected.insert(item);
        }
    }

    /// The range from the anchor (or `item` without one) to `item`.
    fn range_to(&self, item: usize) -> std::ops::RangeInclusive<usize> {
        let anchor = self.anchor.unwrap_or(item);
        anchor.min(item)..=anchor.max(item)
    }

    /// Deselect everything (`LM_SELECTITEM(LIT_NONE)`); in single mode the
    /// cursor goes too. Returns whether the selection changed.
    pub fn clear(&mut self, mode: ListMode) -> bool {
        if mode == ListMode::Single {
            self.cursor = None;
        }
        self.set([])
    }

    /// `LM_SELECTITEM(item, on)`: select or deselect one item. In single
    /// mode selecting replaces the selection and moves the cursor there;
    /// otherwise the other items keep their state. Returns whether the
    /// selection changed.
    pub fn select(&mut self, item: usize, on: bool, mode: ListMode) -> bool {
        match (on, mode) {
            (true, ListMode::Single) => {
                self.cursor = Some(item);
                self.anchor = Some(item);
                self.set([item])
            }
            (true, _) => self.selected.insert(item),
            (false, _) => {
                let removed = self.selected.remove(&item);
                if removed && mode == ListMode::Single {
                    self.cursor = None;
                }
                removed
            }
        }
    }

    /// A click on `item` with Shift / Ctrl as given. Returns whether the
    /// selection changed.
    pub fn click(&mut self, item: usize, mode: ListMode, shift: bool, ctrl: bool) -> bool {
        self.cursor = Some(item);
        match mode {
            ListMode::Single => {
                self.anchor = Some(item);
                self.set([item])
            }
            ListMode::Multiple => {
                self.toggle(item);
                true
            }
            ListMode::Extended if shift => self.set(self.range_to(item)),
            ListMode::Extended if ctrl => {
                self.anchor = Some(item);
                self.toggle(item);
                true
            }
            ListMode::Extended => {
                self.anchor = Some(item);
                self.set([item])
            }
        }
    }

    /// The pointer dragged onto `item` with button 1 held: single mode
    /// selects it, extended mode selects the range from the anchor, multiple
    /// mode only moves the cursor. Returns whether the selection changed.
    pub fn drag_to(&mut self, item: usize, mode: ListMode) -> bool {
        self.cursor = Some(item);
        match mode {
            ListMode::Single => {
                self.anchor = Some(item);
                self.set([item])
            }
            ListMode::Multiple => false,
            ListMode::Extended => self.set(self.range_to(item)),
        }
    }

    /// A cursor key moved the cursor to `item` (Shift as given): single
    /// mode selects it, extended mode selects it alone (or, with Shift, the
    /// range from the anchor), multiple mode leaves the selection alone.
    /// Returns whether the selection changed.
    pub fn move_cursor(&mut self, item: usize, mode: ListMode, shift: bool) -> bool {
        self.cursor = Some(item);
        match mode {
            ListMode::Multiple => false,
            ListMode::Extended if shift => self.set(self.range_to(item)),
            _ => {
                self.anchor = Some(item);
                self.set([item])
            }
        }
    }

    /// Space: toggle the cursor item in multiple and extended mode (a
    /// no-op in single mode or without a cursor). Returns whether the
    /// selection changed.
    pub fn toggle_cursor(&mut self, mode: ListMode) -> bool {
        match (mode, self.cursor) {
            (ListMode::Single, _) | (_, None) => false,
            (_, Some(item)) => {
                self.anchor = Some(item);
                self.toggle(item);
                true
            }
        }
    }

    /// Shift every index at or after `index` by one: an item was inserted
    /// there, so the items after it keep their state.
    pub fn inserted(&mut self, index: usize) {
        let shift = |i: usize| if i >= index { i + 1 } else { i };
        self.selected = self.selected.iter().map(|&i| shift(i)).collect();
        self.cursor = self.cursor.map(shift);
        self.anchor = self.anchor.map(shift);
    }

    /// Item `index` was removed: it loses its state and the items after it
    /// move up.
    pub fn removed(&mut self, index: usize) {
        let shift = |i: usize| match i.cmp(&index) {
            std::cmp::Ordering::Less => Some(i),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(i - 1),
        };
        self.selected = self.selected.iter().filter_map(|&i| shift(i)).collect();
        self.cursor = self.cursor.and_then(shift);
        self.anchor = self.anchor.and_then(shift);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(s: &ListSelection) -> Vec<usize> {
        s.selected.iter().copied().collect()
    }

    #[test]
    fn mode_follows_the_style_bits() {
        assert_eq!(ListMode::from_style(0), ListMode::Single);
        assert_eq!(ListMode::from_style(LS_MULTIPLESEL), ListMode::Multiple);
        assert_eq!(ListMode::from_style(LS_EXTENDEDSEL), ListMode::Extended);
        assert_eq!(ListMode::from_style(LS_MULTIPLESEL | LS_EXTENDEDSEL), ListMode::Multiple);
    }

    #[test]
    fn single_mode_keeps_one_item_with_the_cursor_on_it() {
        let mut s = ListSelection::default();
        let m = ListMode::Single;
        assert!(s.click(2, m, false, false));
        assert!(!s.click(2, m, true, true), "the same item again changes nothing");
        assert!(s.move_cursor(3, m, true));
        assert_eq!((selected(&s), s.cursor()), (vec![3], Some(3)));
        assert!(s.select(1, true, m));
        assert_eq!((selected(&s), s.cursor()), (vec![1], Some(1)));
        assert!(!s.select(0, false, m), "deselecting another item");
        assert!(s.select(1, false, m));
        assert_eq!((selected(&s), s.cursor()), (vec![], None));
        assert!(!s.toggle_cursor(m));
    }

    #[test]
    fn multiple_mode_toggles_items_and_keys_only_move_the_cursor() {
        let mut s = ListSelection::default();
        let m = ListMode::Multiple;
        s.click(1, m, false, false);
        s.click(3, m, false, false);
        assert_eq!(selected(&s), [1, 3]);
        s.click(1, m, false, false);
        assert_eq!(selected(&s), [3]);
        assert!(!s.move_cursor(0, m, false));
        assert_eq!((selected(&s), s.cursor()), (vec![3], Some(0)));
        assert!(s.toggle_cursor(m));
        assert_eq!(selected(&s), [0, 3]);
        assert!(!s.drag_to(2, m));
        assert!(s.select(2, true, m));
        assert!(!s.select(2, true, m));
        assert_eq!(selected(&s), [0, 2, 3]);
        assert!(s.clear(m));
        assert_eq!((selected(&s), s.cursor()), (vec![], Some(2)), "the cursor stays");
    }

    #[test]
    fn extended_mode_selects_ranges_from_the_anchor() {
        let mut s = ListSelection::default();
        let m = ListMode::Extended;
        s.click(2, m, false, false);
        s.click(5, m, true, false);
        assert_eq!(selected(&s), [2, 3, 4, 5]);
        s.click(0, m, true, false);
        assert_eq!(selected(&s), [0, 1, 2], "the anchor stays at the plain click");
        s.click(4, m, false, true);
        assert_eq!(selected(&s), [0, 1, 2, 4], "Ctrl+click adds an item");
        s.click(1, m, false, true);
        assert_eq!(selected(&s), [0, 2, 4], "and removes one");
        s.click(3, m, false, false);
        assert_eq!(selected(&s), [3]);
        s.drag_to(5, m);
        assert_eq!(selected(&s), [3, 4, 5]);
        s.move_cursor(6, m, true);
        assert_eq!(selected(&s), [3, 4, 5, 6]);
        s.move_cursor(1, m, false);
        assert_eq!((selected(&s), s.cursor()), (vec![1], Some(1)));
    }

    #[test]
    fn next_after_walks_the_selection_in_order() {
        let mut s = ListSelection::default();
        for i in [4, 1, 7] {
            s.select(i, true, ListMode::Multiple);
        }
        assert_eq!(s.next_after(None), Some(1));
        assert_eq!(s.next_after(Some(1)), Some(4));
        assert_eq!(s.next_after(Some(5)), Some(7));
        assert_eq!(s.next_after(Some(7)), None);
    }

    #[test]
    fn inserting_and_removing_items_keep_states_on_their_items() {
        let mut s = ListSelection::default();
        let m = ListMode::Extended;
        s.click(1, m, false, false);
        s.click(3, m, true, false);
        s.inserted(2);
        assert_eq!((selected(&s), s.cursor(), s.anchor), (vec![1, 3, 4], Some(4), Some(1)));
        s.removed(1);
        assert_eq!((selected(&s), s.cursor(), s.anchor), (vec![2, 3], Some(3), None));
        s.removed(3);
        assert_eq!((selected(&s), s.cursor()), (vec![2], None));
    }
}
