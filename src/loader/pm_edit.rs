// SPDX-License-Identifier: GPL-3.0-only

//! Editing model shared by the built-in text controls (WC_ENTRYFIELD, WC_MLE)
//! and the scrolling arithmetic of WC_LISTBOX.
//!
//! Everything here is pure: no window manager, no guest memory, no renderer.
//! Text is edited as a sequence of Unicode scalar values, so caret positions
//! are character indices and a multi-byte character can never be split.
//! Pixel measurements are supplied by the caller as a `width` function (the
//! controls pass `font::text_width` of the proportional system font in the
//! CJK face their text is drawn in, `pm_controls::measure_in`).

use std::ops::Range;

use super::codepage::cp_decode;
use super::constants::*;

/// Width in pixels of the caret bar drawn in a focused text control.
pub const CARET_W: i32 = 1;

/// Lowest byte value that is a printable character rather than a control code.
const FIRST_PRINTABLE: u32 = 0x20;
/// ASCII DEL: sent as the character of the Delete key, never inserted.
const ASCII_DEL: u32 = 0x7F;
/// Mask of one byte of the WM_CHAR character word.
const CHAR_BYTE_MASK: u32 = 0xFF;
/// Mask of the WM_CHAR flag word (mp1) and character word (mp2).
const WORD_MASK: u32 = 0xFFFF;

/// A function giving the width in pixels of a string (shared by the copies
/// of a buffer; the controls measure with the face their text is drawn in).
pub type Measure = std::rc::Rc<dyn Fn(&str) -> i32>;

/// An editing action decoded from a WM_CHAR message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Up,
    Down,
    /// Up / down by a page: the visible rows less one (multi-line only).
    PageUp,
    PageDown,
    Enter,
}

/// Decode a WM_CHAR message (`mp1` = flags/repeat/scan code, `mp2` =
/// character/virtual key, see CHRMSG) into an editing action.
///
/// Returns `(key, is_key_up)`. `None` means the message is not an editing key
/// (function keys, Ctrl/Alt chords, control characters, undecodable bytes).
/// A double-byte character arrives with its lead byte in the low byte and its
/// trail byte in the high byte of the character word and is decoded through
/// `codepage`.
pub fn key_from_wm_char(mp1: u32, mp2: u32, codepage: u32) -> Option<(EditKey, bool)> {
    let flags = mp1 & WORD_MASK;
    let key_up = flags & KC_KEYUP != 0;
    if flags & (KC_CTRL | KC_ALT) != 0 {
        return None;
    }
    if flags & KC_VIRTUALKEY != 0 {
        let key = match mp2 >> WM_CHAR_VKEY_SHIFT {
            VK_BACKSPACE => Some(EditKey::Backspace),
            VK_DELETE => Some(EditKey::Delete),
            VK_LEFT => Some(EditKey::Left),
            VK_RIGHT => Some(EditKey::Right),
            VK_HOME => Some(EditKey::Home),
            VK_END => Some(EditKey::End),
            VK_UP => Some(EditKey::Up),
            VK_DOWN => Some(EditKey::Down),
            VK_PAGEUP => Some(EditKey::PageUp),
            VK_PAGEDOWN => Some(EditKey::PageDown),
            VK_NEWLINE | VK_ENTER => Some(EditKey::Enter),
            // Other virtual keys (VK_SPACE, ...) may still carry a character.
            _ => None,
        };
        if let Some(key) = key {
            return Some((key, key_up));
        }
    }
    if flags & KC_CHAR == 0 {
        return None;
    }
    let chr = mp2 & WORD_MASK;
    let (lead, trail) = (chr & CHAR_BYTE_MASK, chr >> 8);
    if lead < FIRST_PRINTABLE || lead == ASCII_DEL {
        return None;
    }
    let bytes = [lead as u8, trail as u8];
    let decoded = cp_decode(if trail == 0 { &bytes[..1] } else { &bytes[..] }, codepage);
    let mut chars = decoded.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if !ch.is_control() && ch != char::REPLACEMENT_CHARACTER => {
            Some((EditKey::Char(ch), key_up))
        }
        _ => None,
    }
}

/// The text, caret and selection of an entry field or MLE while it is
/// being edited.
///
/// In multi-line mode lines are separated by `\n` or `\r\n`; a `\r\n` pair is
/// treated as one unit, so the caret never rests between its two characters
/// and deleting a line break removes both.
///
/// The selection runs between the *anchor* and the caret (either may come
/// first); it is empty when they are equal. Moving with Shift held keeps the
/// anchor and so extends the selection; any other move or edit brings the
/// anchor to the caret.
///
/// With word wrap ([`Self::set_wrap`]) the *lines* of a multi-line buffer
/// are display rows: each line of the text is broken into rows that fit the
/// wrap width. A row ends after the spaces that follow its last word (they
/// may hang past the width); a word wider than the width is broken between
/// characters. Where a row is *soft-wrapped* (continues on the next row
/// without a line break) the caret position after its last character is the
/// start of the next row, so the caret can reach the row's end only up to
/// its last character.
#[derive(Clone)]
pub struct EditBuffer {
    chars: Vec<char>,
    caret: usize,
    anchor: usize,
    multiline: bool,
    /// Lines PageUp / PageDown move by.
    page_lines: usize,
    /// Word wrap: the row width in pixels and the function measuring text.
    wrap: Option<(i32, Measure)>,
}

impl std::fmt::Debug for EditBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditBuffer")
            .field("chars", &self.chars)
            .field("caret", &self.caret)
            .field("anchor", &self.anchor)
            .field("multiline", &self.multiline)
            .field("page_lines", &self.page_lines)
            .field("wrap_width", &self.wrap.as_ref().map(|(w, _)| *w))
            .finish()
    }
}

impl EditBuffer {
    /// Build a buffer from a window's text with no selection. `caret` is
    /// clamped to the text.
    pub fn new(text: &str, caret: usize, multiline: bool) -> Self {
        Self::with_selection(text, caret, caret, multiline)
    }

    /// Build a buffer with the selection from `anchor` to `caret` (both
    /// clamped to the text).
    pub fn with_selection(text: &str, anchor: usize, caret: usize, multiline: bool) -> Self {
        let mut buf = EditBuffer { chars: text.chars().collect(), caret: 0, anchor: 0, multiline, page_lines: 1, wrap: None };
        buf.set_selection(anchor, caret);
        buf
    }

    /// Make PageUp / PageDown move by a page of a view `rows` rows high:
    /// `rows - 1` lines, keeping one line of context, at least one.
    pub fn set_page_rows(&mut self, rows: usize) {
        self.page_lines = rows.saturating_sub(1).max(1);
    }

    /// The lines PageUp / PageDown move by.
    pub fn page_lines(&self) -> usize {
        self.page_lines
    }

    /// Wrap the lines of a multi-line buffer into rows `width` pixels wide,
    /// measured with `measure`. A single-line buffer is never wrapped.
    pub fn set_wrap(&mut self, width: i32, measure: impl Fn(&str) -> i32 + 'static) {
        if self.multiline {
            self.wrap = Some((width, std::rc::Rc::new(measure)));
        }
    }

    /// The selection anchor (the end of the selection the caret is not at).
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// The selected characters, `start..end`; empty when nothing is selected.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.caret
    }

    /// The selected text.
    pub fn selected_text(&self) -> String {
        self.slice(self.selection())
    }

    /// Select from `anchor` to `caret`, both clamped and kept out of the
    /// middle of a `\r\n` pair.
    pub fn set_selection(&mut self, anchor: usize, caret: usize) {
        self.set_caret(anchor);
        let anchor = self.caret;
        self.set_caret(caret);
        self.anchor = anchor;
    }

    /// Delete the selected text, leaving the caret where it began. Returns
    /// whether anything was deleted.
    pub fn delete_selection(&mut self) -> bool {
        let sel = self.selection();
        if sel.is_empty() {
            return false;
        }
        self.chars.drain(sel.clone());
        self.caret = sel.start;
        self.anchor = sel.start;
        true
    }

    /// Replace the selection with `text` (pasting): line breaks are kept in
    /// a multi-line buffer, while a single-line buffer takes the text up to
    /// the first line break; other control characters are dropped. The
    /// caret ends after the inserted text. Returns whether the text changed.
    pub fn replace_selection(&mut self, text: &str) -> bool {
        let mut changed = self.delete_selection();
        let text = if self.multiline { text } else { text.split(['\r', '\n']).next().unwrap_or("") };
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            let keep = !ch.is_control() || (self.multiline && (ch == '\n' || (ch == '\r' && chars.peek() == Some(&'\n'))));
            if keep {
                self.chars.insert(self.caret, ch);
                self.caret += 1;
                changed = true;
            }
        }
        self.anchor = self.caret;
        changed
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn chars(&self) -> &[char] {
        &self.chars
    }

    /// Caret position in characters, always in `0..=len`.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Move the caret to character index `pos`, clamped to the text and kept
    /// out of the middle of a `\r\n` pair, without a selection.
    pub fn set_caret(&mut self, pos: usize) {
        self.caret = pos.min(self.chars.len());
        if self.multiline && self.caret > 0
            && self.chars.get(self.caret) == Some(&'\n')
            && self.chars[self.caret - 1] == '\r'
        {
            self.caret -= 1;
        }
        self.anchor = self.caret;
    }

    /// Move the caret to `pos` keeping the anchor: extend the selection.
    pub fn extend_to(&mut self, pos: usize) {
        let anchor = self.anchor;
        self.set_caret(pos);
        self.anchor = anchor;
    }

    /// Length in characters of the line break starting at `i` (0 = none).
    fn break_len_at(&self, i: usize) -> usize {
        if !self.multiline {
            return 0;
        }
        match (self.chars.get(i), self.chars.get(i + 1)) {
            (Some('\n'), _) => 1,
            (Some('\r'), Some('\n')) => 2,
            _ => 0,
        }
    }

    /// Length in characters of the unit (character or line break) that ends
    /// just before the caret; 0 at the start of the text.
    fn unit_before_caret(&self) -> usize {
        match self.caret {
            0 => 0,
            c if self.multiline && c >= 2
                && self.chars[c - 1] == '\n' && self.chars[c - 2] == '\r' => 2,
            _ => 1,
        }
    }

    /// Length of the unit starting at the caret; 0 at the end of the text.
    fn unit_after_caret(&self) -> usize {
        if self.caret >= self.chars.len() { 0 } else { self.break_len_at(self.caret).max(1) }
    }

    /// Character ranges of the lines (display rows when word wrap is on),
    /// excluding their line breaks. There is always at least one line; a
    /// single-line buffer is one line throughout.
    pub fn lines(&self) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut push = |range: Range<usize>| match &self.wrap {
            Some((width, measure)) => out.extend(wrap_rows(&self.chars, range, *width, &**measure)),
            None => out.push(range),
        };
        let (mut start, mut i) = (0, 0);
        while i < self.chars.len() {
            match self.break_len_at(i) {
                0 => i += 1,
                n => {
                    push(start..i);
                    i += n;
                    start = i;
                }
            }
        }
        push(start..self.chars.len());
        out
    }

    /// The last caret position that belongs to line `line` of `lines`: its
    /// end, or its last character when the line is soft-wrapped (the end is
    /// then the start of the next line).
    fn line_caret_end(lines: &[Range<usize>], line: usize) -> usize {
        let range = &lines[line];
        match lines.get(line + 1) {
            Some(next) if next.start == range.end && !range.is_empty() => range.end - 1,
            _ => range.end,
        }
    }

    /// The text of a character range (as returned by [`Self::lines`]).
    pub fn slice(&self, range: Range<usize>) -> String {
        self.chars.get(range).map(|c| c.iter().collect()).unwrap_or_default()
    }

    /// `(line index, column)` of the caret.
    pub fn caret_line(&self) -> (usize, usize) {
        let lines = self.lines();
        let idx = lines.iter().rposition(|r| r.start <= self.caret).unwrap_or(0);
        (idx, self.caret - lines[idx].start)
    }

    /// Put the caret at `col` of line `line`; both are clamped. The
    /// selection is dropped.
    pub fn set_caret_in_line(&mut self, line: usize, col: usize) {
        let pos = self.pos_in_line(line, col);
        self.set_caret(pos);
    }

    /// The character index of `col` in line `line`, both clamped.
    pub fn pos_in_line(&self, line: usize, col: usize) -> usize {
        let lines = self.lines();
        let line = line.min(lines.len() - 1);
        (lines[line].start + col).min(Self::line_caret_end(&lines, line))
    }

    /// Insert a printable character at the caret. Control characters are
    /// rejected. Returns whether the text changed.
    pub fn insert(&mut self, ch: char) -> bool {
        if ch.is_control() {
            return false;
        }
        self.chars.insert(self.caret, ch);
        self.caret += 1;
        true
    }

    /// Break the line at the caret (multi-line buffers only).
    pub fn newline(&mut self) -> bool {
        if !self.multiline {
            return false;
        }
        self.chars.insert(self.caret, '\n');
        self.caret += 1;
        true
    }

    /// Delete the character (or line break) before the caret.
    pub fn backspace(&mut self) -> bool {
        let n = self.unit_before_caret();
        self.chars.drain(self.caret - n..self.caret);
        self.caret -= n;
        n > 0
    }

    /// Delete the character (or line break) at the caret.
    pub fn delete(&mut self) -> bool {
        let n = self.unit_after_caret();
        self.chars.drain(self.caret..self.caret + n);
        n > 0
    }

    pub fn move_left(&mut self) -> bool {
        let n = self.unit_before_caret();
        self.caret -= n;
        n > 0
    }

    pub fn move_right(&mut self) -> bool {
        let n = self.unit_after_caret();
        self.caret += n;
        n > 0
    }

    /// Move the caret to the start of its line.
    pub fn move_home(&mut self) -> bool {
        let (_, col) = self.caret_line();
        self.caret -= col;
        col > 0
    }

    /// Move the caret to the end of its line.
    pub fn move_end(&mut self) -> bool {
        let (line, _) = self.caret_line();
        let end = Self::line_caret_end(&self.lines(), line);
        let moved = end != self.caret;
        self.caret = end;
        moved
    }

    /// Move the caret `delta` lines up (negative) or down, to the column
    /// whose pixel position is closest to the caret's current one.
    pub fn move_vertical(&mut self, delta: i32, width: &dyn Fn(&str) -> i32) -> bool {
        let lines = self.lines();
        let (line, col) = self.caret_line();
        // Clamped to the first / last line, so a page move goes as far as it can.
        let target = (line as i64 + delta as i64).clamp(0, lines.len() as i64 - 1);
        if target == line as i64 {
            return false;
        }
        let from = &lines[line];
        let x = width(&self.slice(from.start..from.start + col));
        let target = target as usize;
        let to = lines[target].clone();
        self.caret = (to.start + caret_from_x(&self.chars[to], x, width))
            .min(Self::line_caret_end(&lines, target));
        true
    }

    /// Apply an editing key; `extend` (Shift held) makes a movement key
    /// extend the selection instead of dropping it. Returns `None` when the
    /// key does not apply to this kind of buffer (Enter/Up/Down in a
    /// single-line field), otherwise `Some(text_changed)`.
    ///
    /// With a selection, typing and Enter replace it, Backspace and Delete
    /// remove it, and Left / Right without Shift only drop it, leaving the
    /// caret at its start / end.
    pub fn apply(&mut self, key: EditKey, extend: bool, width: &dyn Fn(&str) -> i32) -> Option<bool> {
        if !self.multiline && matches!(key, EditKey::Enter | EditKey::Up | EditKey::Down
                                                | EditKey::PageUp | EditKey::PageDown) {
            return None;
        }
        let selected = self.has_selection();
        let anchor = self.anchor;
        let changed = match key {
            EditKey::Char(ch) => {
                if ch.is_control() { return Some(false); }
                self.delete_selection();
                self.insert(ch)
            }
            EditKey::Enter => { self.delete_selection(); self.newline() }
            EditKey::Backspace if selected => self.delete_selection(),
            EditKey::Delete if selected => self.delete_selection(),
            EditKey::Backspace => self.backspace(),
            EditKey::Delete => self.delete(),
            EditKey::Left | EditKey::Right if selected && !extend => {
                let sel = self.selection();
                self.caret = if key == EditKey::Left { sel.start } else { sel.end };
                false
            }
            EditKey::Left => { self.move_left(); false }
            EditKey::Right => { self.move_right(); false }
            EditKey::Home => { self.move_home(); false }
            EditKey::End => { self.move_end(); false }
            EditKey::Up => { self.move_vertical(-1, width); false }
            EditKey::Down => { self.move_vertical(1, width); false }
            EditKey::PageUp => { self.move_vertical(-(self.page_lines as i32), width); false }
            EditKey::PageDown => { self.move_vertical(self.page_lines as i32, width); false }
        };
        let moved = !matches!(key, EditKey::Char(_) | EditKey::Enter | EditKey::Backspace | EditKey::Delete);
        self.anchor = if moved && extend { anchor } else { self.caret };
        Some(changed)
    }
}

/// Break `chars[range]` (one line of text, without its line break) into rows
/// at most `width` pixels wide as measured by `measure`: a row breaks after
/// the spaces following the last word that fits (spaces may hang past the
/// width), or between characters when a single word is wider than the row.
/// Every row holds at least one character, and an empty line is one empty
/// row.
pub fn wrap_rows(chars: &[char], range: Range<usize>, width: i32,
                 measure: &dyn Fn(&str) -> i32) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut row_start = range.start;
    // Where the row may break: just after the last space run seen in it.
    let mut last_break = None;
    let mut i = range.start;
    while i < range.end {
        let ch = chars[i];
        if ch == ' ' {
            i += 1;
            last_break = Some(i);
            continue;
        }
        let row_w = measure(&chars[row_start..=i].iter().collect::<String>());
        if row_w > width && i > row_start {
            let brk = match last_break {
                Some(b) if b > row_start => b,
                _ => i,
            };
            rows.push(row_start..brk);
            row_start = brk;
            last_break = None;
            // Re-measure the moved word against the new row.
            i = brk;
            continue;
        }
        i += 1;
    }
    rows.push(row_start..range.end);
    rows
}

/// The caret index (0..=len) in `chars` nearest to pixel offset `x` from the
/// start of the text: a click on the left half of a character puts the caret
/// before it, on the right half after it.
pub fn caret_from_x(chars: &[char], x: i32, width: &dyn Fn(&str) -> i32) -> usize {
    let mut prefix = String::new();
    let mut left = 0;
    for (i, &ch) in chars.iter().enumerate() {
        prefix.push(ch);
        let right = width(&prefix);
        if 2 * x < left + right {
            return i;
        }
        left = right;
    }
    chars.len()
}

/// Horizontal scroll of one line of text: the index of the first visible
/// character such that the caret lies inside a view `view_w` pixels wide.
///
/// `first` is the current scroll position. It is kept when the caret is
/// already visible; moved left to the caret when the caret is before it
/// (revealing about a quarter of the view as context, so that deleting at the
/// left edge does not edit blind); and moved right just far enough when the
/// text between it and the caret no longer fits.
pub fn scroll_to_caret(chars: &[char], caret: usize, first: usize, view_w: i32,
                       width: &dyn Fn(&str) -> i32) -> usize {
    let caret = caret.min(chars.len());
    let span = |from: usize| width(&chars[from..caret].iter().collect::<String>());
    let mut first = first.min(chars.len());
    if caret <= first {
        first = caret;
        while first > 0 && span(first - 1) <= view_w / 4 {
            first -= 1;
        }
    }
    while first < caret && span(first) > view_w {
        first += 1;
    }
    first
}

/// Vertical scroll of a list of `count` rows shown `visible` at a time: the
/// new top row such that `row` is visible, moving the current `top` as little
/// as possible and never scrolling past the end of the list.
pub fn scroll_to_row(row: usize, top: usize, visible: usize, count: usize) -> usize {
    if visible == 0 {
        return row.min(count.saturating_sub(1));
    }
    let top = top.min(count.saturating_sub(visible));
    if row < top {
        row
    } else if row >= top + visible {
        row + 1 - visible
    } else {
        top
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::font::{self, FontSpec};

    /// Every character 10 px wide: makes pixel arithmetic exact in tests.
    fn mono(s: &str) -> i32 {
        s.chars().count() as i32 * 10
    }

    fn system(s: &str) -> i32 {
        font::text_width(s, FontSpec::SYSTEM)
    }

    fn wm_char(flags: u32, chr: u32, vk: u32) -> (u32, u32) {
        (flags, chr | (vk << WM_CHAR_VKEY_SHIFT))
    }

    // ── EditBuffer: single line ──────────────────────────────────────────────

    #[test]
    fn empty_buffer_edits_are_no_ops() {
        let mut b = EditBuffer::new("", 5, false);
        assert_eq!(b.caret(), 0, "caret is clamped to the text");
        assert!(!b.backspace());
        assert!(!b.delete());
        assert!(!b.move_left());
        assert!(!b.move_right());
        assert!(!b.move_home());
        assert!(!b.move_end());
        assert!(!b.move_vertical(1, &mono));
        assert_eq!(b.text(), "");
        assert_eq!(b.lines(), vec![0..0]);
        assert_eq!(b.caret_line(), (0, 0));
    }

    #[test]
    fn insert_at_start_middle_and_end() {
        let mut b = EditBuffer::new("bd", 0, false);
        assert!(b.insert('a'));
        assert_eq!((b.text().as_str(), b.caret()), ("abd", 1));
        b.set_caret(2);
        assert!(b.insert('c'));
        assert_eq!((b.text().as_str(), b.caret()), ("abcd", 3));
        b.set_caret(99);
        assert!(b.insert('e'));
        assert_eq!((b.text().as_str(), b.caret()), ("abcde", 5));
    }

    #[test]
    fn insert_rejects_control_characters() {
        let mut b = EditBuffer::new("ab", 1, false);
        for ch in ['\n', '\r', '\t', '\u{8}', '\u{7f}', '\0'] {
            assert!(!b.insert(ch));
        }
        assert_eq!((b.text().as_str(), b.caret()), ("ab", 1));
        assert!(!b.newline(), "a single-line field has no line breaks");
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn backspace_and_delete_at_both_ends() {
        let mut b = EditBuffer::new("abc", 0, false);
        assert!(!b.backspace(), "nothing before the caret");
        assert!(b.delete());
        assert_eq!((b.text().as_str(), b.caret()), ("bc", 0));
        b.set_caret(2);
        assert!(!b.delete(), "nothing after the caret");
        assert!(b.backspace());
        assert_eq!((b.text().as_str(), b.caret()), ("b", 1));
        assert!(b.backspace());
        assert!(!b.backspace());
        assert_eq!((b.text().as_str(), b.caret()), ("", 0));
    }

    #[test]
    fn caret_movement_stops_at_both_ends() {
        let mut b = EditBuffer::new("abc", 1, false);
        assert!(b.move_left());
        assert!(!b.move_left());
        assert_eq!(b.caret(), 0);
        assert!(b.move_end());
        assert!(!b.move_end());
        assert_eq!(b.caret(), 3);
        assert!(!b.move_right());
        assert!(b.move_home());
        assert!(!b.move_home());
        assert_eq!(b.caret(), 0);
        assert!(b.move_right());
        assert_eq!(b.caret(), 1);
    }

    /// Carets are character indices: editing multi-byte text (accented Latin,
    /// CJK, astral-plane emoji) never splits a character or panics.
    #[test]
    fn multibyte_text_is_edited_by_character() {
        let mut b = EditBuffer::new("aé日本😀z", 99, false);
        assert_eq!(b.caret(), 6);
        assert!(b.move_left());
        assert!(b.backspace()); // removes the emoji
        assert_eq!((b.text().as_str(), b.caret()), ("aé日本z", 4));
        assert!(b.backspace()); // removes 本
        assert!(b.insert('語'));
        assert_eq!((b.text().as_str(), b.caret()), ("aé日語z", 4));
        b.set_caret(1);
        assert!(b.delete()); // removes é
        assert_eq!((b.text().as_str(), b.caret()), ("a日語z", 1));
        assert!(b.insert('ß'));
        assert_eq!(b.text(), "aß日語z");
        // Every caret position yields valid UTF-8 on both sides.
        for c in 0..=b.chars().len() {
            b.set_caret(c);
            assert_eq!(b.slice(0..c) + &b.slice(c..b.chars().len()), b.text());
        }
    }

    #[test]
    fn single_line_treats_newlines_as_ordinary_text() {
        let mut b = EditBuffer::new("a\r\nb", 2, false);
        assert_eq!(b.caret(), 2, "no CRLF snapping in a single-line field");
        assert_eq!(b.lines(), vec![0..4]);
        assert_eq!(b.apply(EditKey::Enter, false, &mono), None);
        assert_eq!(b.apply(EditKey::Up, false, &mono), None);
        assert_eq!(b.apply(EditKey::Down, false, &mono), None);
        assert_eq!(b.text(), "a\r\nb");
    }

    // ── EditBuffer: multi-line ───────────────────────────────────────────────

    #[test]
    fn lines_split_on_lf_and_crlf() {
        let b = EditBuffer::new("one\ntwo\r\n\nfour", 0, true);
        let lines: Vec<String> = b.lines().into_iter().map(|r| b.slice(r)).collect();
        assert_eq!(lines, ["one", "two", "", "four"]);
        // A trailing break yields a final empty line; a lone CR is plain text.
        assert_eq!(EditBuffer::new("a\n", 0, true).lines(), vec![0..1, 2..2]);
        assert_eq!(EditBuffer::new("a\rb", 0, true).lines(), vec![0..3]);
    }

    #[test]
    fn newline_splits_the_line_and_backspace_joins_it() {
        let mut b = EditBuffer::new("hello", 2, true);
        assert_eq!(b.apply(EditKey::Enter, false, &mono), Some(true));
        assert_eq!((b.text().as_str(), b.caret()), ("he\nllo", 3));
        assert_eq!(b.caret_line(), (1, 0));
        assert!(b.backspace());
        assert_eq!((b.text().as_str(), b.caret()), ("hello", 2));
    }

    #[test]
    fn crlf_is_one_unit_for_movement_and_deletion() {
        let mut b = EditBuffer::new("ab\r\ncd", 3, true);
        assert_eq!(b.caret(), 2, "caret snaps out of the middle of CRLF");
        assert!(b.move_right());
        assert_eq!(b.caret(), 4);
        assert_eq!(b.caret_line(), (1, 0));
        assert!(b.move_left());
        assert_eq!(b.caret(), 2);
        assert!(b.delete());
        assert_eq!((b.text().as_str(), b.caret()), ("abcd", 2));

        let mut b = EditBuffer::new("ab\r\ncd", 4, true);
        assert!(b.backspace());
        assert_eq!((b.text().as_str(), b.caret()), ("abcd", 2));
    }

    #[test]
    fn home_and_end_are_line_relative() {
        let mut b = EditBuffer::new("one\r\ntwo\nthree", 6, true); // in "two"
        assert_eq!(b.caret_line(), (1, 1));
        assert!(b.move_home());
        assert_eq!(b.caret(), 5);
        assert!(b.move_end());
        assert_eq!(b.caret(), 8);
        // End on the CRLF line stops before the CR.
        b.set_caret(0);
        assert!(b.move_end());
        assert_eq!(b.caret(), 3);
    }

    #[test]
    fn vertical_movement_keeps_pixel_column_and_clamps_to_short_lines() {
        let mut b = EditBuffer::new("abcdef\nxy\n12345", 5, true);
        assert!(b.move_vertical(1, &mono));
        assert_eq!(b.caret_line(), (1, 2), "short line: caret goes to its end");
        assert!(b.move_vertical(1, &mono));
        assert_eq!(b.caret_line(), (2, 2));
        assert!(!b.move_vertical(1, &mono), "already on the last line");
        assert!(b.move_vertical(-2, &mono));
        assert_eq!(b.caret_line(), (0, 2));
        assert!(!b.move_vertical(-1, &mono), "already on the first line");
        assert_eq!(b.apply(EditKey::Down, false, &mono), Some(false));
        assert_eq!(b.caret_line(), (1, 2));
    }

    #[test]
    fn vertical_movement_uses_pixel_positions_for_proportional_text() {
        // "WWW" is far wider than "iii": moving down from the end of "WWW"
        // lands well past column 3 of the line of i's.
        let mut b = EditBuffer::new("WWW\niiiiiiiiiiii", 3, true);
        assert!(b.move_vertical(1, &system));
        let (line, col) = b.caret_line();
        assert_eq!(line, 1);
        assert!(col > 3, "caret column {col} follows pixels, not characters");
        let target = system("WWW");
        let here = system(&"i".repeat(col));
        assert!((here - target).abs() <= system("i"), "caret x {here} is near {target}");
    }

    #[test]
    fn set_caret_in_line_clamps_line_and_column() {
        let mut b = EditBuffer::new("ab\ncde", 0, true);
        b.set_caret_in_line(1, 2);
        assert_eq!(b.caret(), 5);
        b.set_caret_in_line(0, 99);
        assert_eq!(b.caret(), 2);
        b.set_caret_in_line(99, 99);
        assert_eq!(b.caret(), 6);
    }

    #[test]
    fn slice_with_out_of_range_bounds_is_empty() {
        let b = EditBuffer::new("abc", 0, false);
        assert_eq!(b.slice(2..9), "");
        assert_eq!(b.slice(1..3), "bc");
    }

    // ── caret_from_x ─────────────────────────────────────────────────────────

    #[test]
    fn caret_from_x_rounds_to_the_nearest_character_boundary() {
        let chars: Vec<char> = "abcd".chars().collect();
        assert_eq!(caret_from_x(&chars, -50, &mono), 0);
        assert_eq!(caret_from_x(&chars, 0, &mono), 0);
        assert_eq!(caret_from_x(&chars, 4, &mono), 0, "left half of 'a'");
        assert_eq!(caret_from_x(&chars, 5, &mono), 1, "right half of 'a'");
        assert_eq!(caret_from_x(&chars, 24, &mono), 2);
        assert_eq!(caret_from_x(&chars, 36, &mono), 4);
        assert_eq!(caret_from_x(&chars, 1000, &mono), 4, "past the end");
        assert_eq!(caret_from_x(&[], 10, &mono), 0, "empty text");
    }

    #[test]
    fn caret_from_x_inverts_text_width_for_mixed_width_text() {
        let chars: Vec<char> = "Wi日本é😀.".chars().collect();
        for caret in 0..=chars.len() {
            let x = system(&chars[..caret].iter().collect::<String>());
            assert_eq!(caret_from_x(&chars, x, &system), caret, "boundary {caret} at x={x}");
        }
    }

    // ── scroll_to_caret ──────────────────────────────────────────────────────

    #[test]
    fn scroll_stays_put_while_the_caret_is_visible() {
        let chars: Vec<char> = "0123456789".chars().collect();
        // 100-px view shows all ten 10-px characters.
        for caret in 0..=10 {
            assert_eq!(scroll_to_caret(&chars, caret, 0, 100, &mono), 0);
        }
        // Scrolled to 3 with the caret inside [3, 8]: unchanged.
        assert_eq!(scroll_to_caret(&chars, 5, 3, 50, &mono), 3);
        assert_eq!(scroll_to_caret(&chars, 8, 3, 50, &mono), 3);
        assert_eq!(scroll_to_caret(&[], 0, 0, 50, &mono), 0, "empty text");
        assert_eq!(scroll_to_caret(&[], 7, 9, 50, &mono), 0, "stale state is clamped");
    }

    #[test]
    fn scroll_follows_the_caret_past_the_right_edge() {
        let chars: Vec<char> = "0123456789abcdefghij".chars().collect();
        // Caret at the end of 20 chars in a 50-px view: 5 chars fit before it.
        assert_eq!(scroll_to_caret(&chars, 20, 0, 50, &mono), 15);
        // One step further right from a visible position scrolls by one.
        assert_eq!(scroll_to_caret(&chars, 9, 3, 50, &mono), 4);
    }

    #[test]
    fn scroll_reveals_context_when_the_caret_moves_left_of_the_view() {
        let chars: Vec<char> = "0123456789abcdefghij".chars().collect();
        // View of 80 px: a quarter (20 px = 2 chars) of context is revealed.
        assert_eq!(scroll_to_caret(&chars, 10, 15, 80, &mono), 8);
        // Caret exactly at the left edge (after a Backspace there): same.
        assert_eq!(scroll_to_caret(&chars, 15, 15, 80, &mono), 13);
        // Near the start there is less context to reveal.
        assert_eq!(scroll_to_caret(&chars, 1, 15, 80, &mono), 0);
        assert_eq!(scroll_to_caret(&chars, 0, 15, 80, &mono), 0);
    }

    #[test]
    fn scroll_keeps_the_caret_visible_in_proportional_and_cjk_text() {
        let text = "日本語のテキスト and some wide WWWWWW text あいうえお".repeat(3);
        let chars: Vec<char> = text.chars().collect();
        let view_w = 120;
        let mut first = 0;
        // Walk right to the end, then left to the start: the caret must stay
        // inside the view at every step.
        let positions = (0..=chars.len()).chain((0..chars.len()).rev());
        for caret in positions {
            first = scroll_to_caret(&chars, caret, first, view_w, &system);
            assert!(first <= caret);
            let x = system(&chars[first..caret].iter().collect::<String>());
            assert!(x <= view_w, "caret at {caret} is {x}px into a {view_w}px view");
        }
        assert_eq!(first, 0);
    }

    #[test]
    fn scroll_in_a_view_narrower_than_one_character_does_not_loop() {
        let chars: Vec<char> = "WWWW".chars().collect();
        assert_eq!(scroll_to_caret(&chars, 4, 0, 1, &mono), 4);
        assert_eq!(scroll_to_caret(&chars, 2, 0, 0, &mono), 2);
        assert_eq!(scroll_to_caret(&chars, 2, 0, -8, &mono), 2);
    }

    // ── scroll_to_row ────────────────────────────────────────────────────────

    #[test]
    fn row_scroll_moves_only_when_the_row_leaves_the_view() {
        // 10 rows, 4 visible.
        assert_eq!(scroll_to_row(0, 0, 4, 10), 0);
        assert_eq!(scroll_to_row(3, 0, 4, 10), 0);
        assert_eq!(scroll_to_row(4, 0, 4, 10), 1, "one below the view");
        assert_eq!(scroll_to_row(9, 0, 4, 10), 6);
        assert_eq!(scroll_to_row(2, 6, 4, 10), 2, "above the view");
        assert_eq!(scroll_to_row(7, 6, 4, 10), 6);
    }

    #[test]
    fn row_scroll_clamps_stale_top_and_handles_degenerate_views() {
        // Everything fits: never scrolled.
        assert_eq!(scroll_to_row(2, 5, 6, 3), 0);
        // Stale top beyond the end is pulled back so the view stays full.
        assert_eq!(scroll_to_row(9, 50, 4, 10), 6);
        // No rows visible / empty list.
        assert_eq!(scroll_to_row(3, 0, 0, 10), 3);
        assert_eq!(scroll_to_row(0, 0, 4, 0), 0);
        assert_eq!(scroll_to_row(5, 0, 0, 0), 0);
    }

    // ── key_from_wm_char ─────────────────────────────────────────────────────

    #[test]
    fn wm_char_virtual_keys_map_to_edit_keys() {
        for (vk, key) in [
            (VK_BACKSPACE, EditKey::Backspace), (VK_DELETE, EditKey::Delete),
            (VK_LEFT, EditKey::Left), (VK_RIGHT, EditKey::Right),
            (VK_HOME, EditKey::Home), (VK_END, EditKey::End),
            (VK_UP, EditKey::Up), (VK_DOWN, EditKey::Down),
            (VK_NEWLINE, EditKey::Enter), (VK_ENTER, EditKey::Enter),
        ] {
            let (mp1, mp2) = wm_char(KC_VIRTUALKEY | KC_SCANCODE, 0, vk);
            assert_eq!(key_from_wm_char(mp1, mp2, 437), Some((key, false)));
        }
        // Backspace and Enter also carry a control character; the virtual key wins.
        let (mp1, mp2) = wm_char(KC_VIRTUALKEY | KC_CHAR, 8, VK_BACKSPACE);
        assert_eq!(key_from_wm_char(mp1, mp2, 437), Some((EditKey::Backspace, false)));
        // Function keys are not editing keys.
        let (mp1, mp2) = wm_char(KC_VIRTUALKEY, 0, VK_F1);
        assert_eq!(key_from_wm_char(mp1, mp2, 437), None);
    }

    #[test]
    fn wm_char_characters_decode_through_the_codepage() {
        let (mp1, mp2) = wm_char(KC_CHAR | KC_SCANCODE, b'a' as u32, 0);
        assert_eq!(key_from_wm_char(mp1, mp2, 437), Some((EditKey::Char('a'), false)));
        // Space arrives with VK_SPACE and a character.
        let (mp1, mp2) = wm_char(KC_CHAR | KC_VIRTUALKEY, b' ' as u32, VK_SPACE);
        assert_eq!(key_from_wm_char(mp1, mp2, 437), Some((EditKey::Char(' '), false)));
        // 0x82 is é in CP850.
        let (mp1, mp2) = wm_char(KC_CHAR, 0x82, 0);
        assert_eq!(key_from_wm_char(mp1, mp2, 850), Some((EditKey::Char('é'), false)));
        // DBCS: lead byte low, trail byte high. 0x93 0xFA is 日 in CP932.
        let (mp1, mp2) = wm_char(KC_CHAR, 0x93 | (0xFA << 8), 0);
        assert_eq!(key_from_wm_char(mp1, mp2, 932), Some((EditKey::Char('日'), false)));
    }

    #[test]
    fn wm_char_non_editing_input_is_rejected() {
        // No KC_CHAR: the character word is not valid.
        assert_eq!(key_from_wm_char(KC_SCANCODE, b'a' as u32, 437), None);
        // Control characters (Tab, Esc, DEL) and NUL.
        for chr in [0u32, 9, 27, 0x7F] {
            assert_eq!(key_from_wm_char(KC_CHAR, chr, 437), None);
        }
        // Ctrl / Alt chords belong to accelerators.
        assert_eq!(key_from_wm_char(KC_CHAR | KC_CTRL, b'c' as u32, 437), None);
        assert_eq!(key_from_wm_char(KC_CHAR | KC_ALT, b'f' as u32, 437), None);
        // An orphan DBCS lead byte or an invalid pair does not decode.
        assert_eq!(key_from_wm_char(KC_CHAR, 0x93, 932), None);
        assert_eq!(key_from_wm_char(KC_CHAR, 0x93 | (0x20 << 8), 932), None);
        // Garbage in the unused upper bits of mp1 is ignored (flags are a word).
        assert_eq!(key_from_wm_char(KC_CHAR | (0x1E01 << 16), b'a' as u32, 437),
                   Some((EditKey::Char('a'), false)));
    }

    #[test]
    fn wm_char_key_up_is_reported() {
        let (mp1, mp2) = wm_char(KC_CHAR | KC_KEYUP, b'a' as u32, 0);
        assert_eq!(key_from_wm_char(mp1, mp2, 437), Some((EditKey::Char('a'), true)));
        let (mp1, mp2) = wm_char(KC_VIRTUALKEY | KC_KEYUP, 0, VK_LEFT);
        assert_eq!(key_from_wm_char(mp1, mp2, 437), Some((EditKey::Left, true)));
    }

    // ── selection ────────────────────────────────────────────────────────

    #[test]
    fn shift_moves_extend_and_plain_moves_collapse_the_selection() {
        let mut b = EditBuffer::new("hello world", 0, false);
        for _ in 0..5 {
            b.apply(EditKey::Right, true, &mono);
        }
        assert_eq!((b.selection(), b.selected_text().as_str()), (0..5, "hello"));
        b.apply(EditKey::Left, true, &mono);
        assert_eq!(b.selection(), 0..4, "Shift+Left shrinks it");
        b.apply(EditKey::Right, false, &mono);
        assert_eq!((b.caret(), b.has_selection()), (4, false), "Right drops it at its end");
        b.apply(EditKey::End, true, &mono);
        assert_eq!(b.selection(), 4..11);
        b.apply(EditKey::Left, false, &mono);
        assert_eq!((b.caret(), b.has_selection()), (4, false), "Left drops it at its start");
        b.apply(EditKey::Home, true, &mono);
        assert_eq!((b.anchor(), b.caret()), (4, 0), "the caret may lead");
        b.apply(EditKey::End, false, &mono);
        assert_eq!((b.caret(), b.has_selection()), (11, false));
    }

    #[test]
    fn edits_replace_or_remove_the_selection() {
        let sel = |a, c| EditBuffer::with_selection("abcdef", a, c, false);
        let mut b = sel(1, 4);
        assert_eq!(b.apply(EditKey::Char('X'), false, &mono), Some(true));
        assert_eq!((b.text().as_str(), b.caret(), b.has_selection()), ("aXef", 2, false));
        let mut b = sel(4, 1);
        b.apply(EditKey::Backspace, false, &mono);
        assert_eq!((b.text().as_str(), b.caret()), ("aef", 1), "removes only the selection");
        let mut b = sel(1, 4);
        b.apply(EditKey::Delete, false, &mono);
        assert_eq!(b.text(), "aef");
        // A control character neither inserts nor drops the selection.
        let mut b = sel(1, 4);
        assert_eq!(b.apply(EditKey::Char('\t'), false, &mono), Some(false));
        assert_eq!(b.selection(), 1..4);
        // Enter replaces it in an MLE.
        let mut b = EditBuffer::with_selection("abc\r\ndef", 2, 6, true);
        assert_eq!(b.selected_text(), "c\r\nd");
        b.apply(EditKey::Enter, false, &mono);
        assert_eq!(b.text(), "ab\nef");
    }

    #[test]
    fn selections_are_clamped_and_never_split_a_line_break() {
        let b = EditBuffer::with_selection("ab\r\ncd", 3, 99, true);
        assert_eq!((b.anchor(), b.caret()), (2, 6), "anchor out of the CRLF, caret clamped");
        let mut b = EditBuffer::new("ab\r\ncd", 0, true);
        b.extend_to(3);
        assert_eq!(b.selection(), 0..2);
        b.extend_to(4);
        assert_eq!(b.selected_text(), "ab\r\n");
        assert_eq!(b.pos_in_line(1, 99), 6);
        assert_eq!(b.pos_in_line(9, 1), 5, "line clamped to the last");
    }

    #[test]
    fn pasting_replaces_the_selection_and_filters_control_characters() {
        let mut b = EditBuffer::with_selection("one two", 4, 7, false);
        assert!(b.replace_selection("three\r\nfour"));
        assert_eq!((b.text().as_str(), b.caret()), ("one three", 9), "single line: up to the break");
        let mut b = EditBuffer::new("ab", 1, true);
        assert!(b.replace_selection("x\r\ny\tz\nw"));
        assert_eq!(b.text(), "ax\r\nyz\nwb", "line breaks kept, tab dropped");
        let mut b = EditBuffer::new("ab", 1, false);
        assert!(!b.replace_selection(""), "nothing to paste, nothing selected");
        assert!(!b.delete_selection());
    }

    #[test]
    fn page_keys_move_by_the_page_and_stop_at_the_ends() {
        let text = (0..8).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n");
        let mut b = EditBuffer::new(&text, 1, true); // line 0, column 1
        assert_eq!(b.page_lines(), 1, "default page");
        b.set_page_rows(4);
        assert_eq!(b.page_lines(), 3, "rows less one");
        assert_eq!(b.apply(EditKey::PageDown, false, &mono), Some(false));
        assert_eq!(b.caret_line(), (3, 1), "same column");
        b.apply(EditKey::PageDown, false, &mono);
        b.apply(EditKey::PageDown, false, &mono);
        assert_eq!(b.caret_line(), (7, 1), "clamped to the last line");
        b.apply(EditKey::PageUp, true, &mono);
        assert_eq!(b.caret_line(), (4, 1));
        assert_eq!(b.selected_text(), "4\nl5\nl6\nl", "Shift+PageUp selects back to column 1");
        b.set_page_rows(0);
        assert_eq!(b.page_lines(), 1, "a view with no rows still moves");
        // A single-line field has no pages.
        let mut f = EditBuffer::new("abc", 1, false);
        assert_eq!(f.apply(EditKey::PageDown, false, &mono), None);
        assert_eq!(f.apply(EditKey::PageUp, false, &mono), None);
    }

    #[test]
    fn page_keys_are_decoded_from_wm_char() {
        let vk = |v: u32| key_from_wm_char(KC_VIRTUALKEY | KC_SCANCODE, v << WM_CHAR_VKEY_SHIFT, 437);
        assert_eq!(vk(VK_PAGEUP), Some((EditKey::PageUp, false)));
        assert_eq!(vk(VK_PAGEDOWN), Some((EditKey::PageDown, false)));
    }

    // ── Word wrap ────────────────────────────────────────────────────────────

    fn rows_of(buf: &EditBuffer) -> Vec<String> {
        buf.lines().into_iter().map(|r| buf.slice(r)).collect()
    }

    fn wrapped(text: &str, caret: usize, width: i32) -> EditBuffer {
        let mut buf = EditBuffer::new(text, caret, true);
        buf.set_wrap(width, mono);
        buf
    }

    #[test]
    fn wrap_breaks_after_the_spaces_that_follow_the_last_fitting_word() {
        // 60 px = 6 characters; the space after "two" hangs.
        let buf = wrapped("one two three", 0, 60);
        assert_eq!(rows_of(&buf), ["one ", "two ", "three"]);
        let buf = wrapped("ab cd ef", 0, 60);
        assert_eq!(rows_of(&buf), ["ab cd ", "ef"]);
    }

    #[test]
    fn wrap_breaks_a_word_wider_than_the_row_between_characters() {
        let buf = wrapped("abcdefghij xy", 0, 40);
        assert_eq!(rows_of(&buf), ["abcd", "efgh", "ij ", "xy"]);
        // Even a row narrower than one character holds one character.
        assert_eq!(rows_of(&wrapped("abc", 0, 5)), ["a", "b", "c"]);
    }

    #[test]
    fn wrap_keeps_line_breaks_and_empty_lines() {
        let buf = wrapped("aaa bbb\r\n\nccc", 0, 40);
        assert_eq!(rows_of(&buf), ["aaa ", "bbb", "", "ccc"]);
    }

    #[test]
    fn single_line_buffers_ignore_wrap() {
        let mut buf = EditBuffer::new("one two three", 0, false);
        buf.set_wrap(40, mono);
        assert_eq!(rows_of(&buf), ["one two three"]);
    }

    #[test]
    fn caret_at_a_soft_wrap_belongs_to_the_next_row() {
        // Rows "one " 0..4, "two " 4..8, "three" 8..13.
        let mut buf = wrapped("one two three", 4, 60);
        assert_eq!(buf.caret_line(), (1, 0));
        // End stops before the hanging space, Home returns to the row start.
        buf.apply(EditKey::End, false, &mono);
        assert_eq!(buf.caret(), 7);
        assert_eq!(buf.caret_line(), (1, 3));
        buf.apply(EditKey::Home, false, &mono);
        assert_eq!(buf.caret(), 4);
        // On the last row End reaches the end of the text.
        buf.set_caret(9);
        buf.apply(EditKey::End, false, &mono);
        assert_eq!(buf.caret(), 13);
    }

    #[test]
    fn vertical_moves_and_clicks_step_through_wrapped_rows() {
        let mut buf = wrapped("one two three", 2, 60);
        buf.apply(EditKey::Down, false, &mono);
        assert_eq!(buf.caret(), 6, "same column on the next row");
        buf.set_caret(12);
        buf.apply(EditKey::Up, false, &mono);
        assert_eq!(buf.caret(), 7, "clamped before the hanging space, not onto the next row");
        assert_eq!(buf.caret_line(), (1, 3));
        assert_eq!(buf.pos_in_line(0, 99), 3);
        assert_eq!(buf.pos_in_line(2, 99), 13);
    }

    #[test]
    fn edits_rewrap_the_text() {
        let mut buf = wrapped("one two", 3, 60);
        assert_eq!(rows_of(&buf), ["one ", "two"]);
        buf.apply(EditKey::Backspace, false, &mono);
        buf.apply(EditKey::Backspace, false, &mono);
        assert_eq!(rows_of(&buf), ["o two"]);
    }
}
