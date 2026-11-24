// SPDX-License-Identifier: GPL-3.0-or-later
//
// The VIO screen's rectangle and string operations, its pop-up screen and
// the logical keyboard's state: what `VioWrtCharStr`, `VioWrtNChar`,
// `VioReadCharStr`, `VioScrollUp` / `Dn` / `Lf` / `Rt`, `VioPopUp` /
// `VioEndPopUp` and the KBD calls of `kbdcalls.rs` work on.
//
// Semantics after IBM's Control Program Programming Reference (Toolkit 4.5):
// a string write or read reaching the end of a line goes on at the start of
// the next one and stops at the end of the screen; a scroll's corner or line
// count beyond the screen is taken as the screen's last row or column (or
// the area's height / width), and 0 lines scroll nothing.

#![forbid(unsafe_code)]

use std::io::{self, Write};

use super::console::VioManager;

/// The direction of a `VioScroll*` call: the side where blank lines come in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    /// `VioScrollUp`: lines move up, blanks enter at the bottom.
    Up,
    /// `VioScrollDn`: lines move down, blanks enter at the top.
    Down,
    /// `VioScrollLf`: columns move left, blanks enter at the right.
    Left,
    /// `VioScrollRt`: columns move right, blanks enter at the left.
    Right,
}

/// The screen as `VioPopUp` found it, put back by `VioEndPopUp`.
#[derive(Debug, Clone)]
pub struct SavedScreen {
    buffer: Vec<(char, u8)>,
    raw_bytes: Vec<u8>,
    cursor: (u16, u16),
}

/// The hardware settings `VioSetState` records and `VioGetState` reports
/// (Warpine's renderer does not act on them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VioState {
    /// The 16 palette registers (type 0), the EGA / VGA text-mode defaults.
    pub palette: [u16; 16],
    /// The overscan (border) colour (type 1).
    pub overscan: u16,
    /// Blink (0) or bright backgrounds (1) (type 2): 1, which the renderer
    /// draws.
    pub intensity: u16,
}

impl Default for VioState {
    fn default() -> Self {
        VioState {
            palette: [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x14, 0x07, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F],
            overscan: 0,
            intensity: 1,
        }
    }
}

/// The logical keyboard's state (`KbdSetStatus` / `KbdGetStatus`,
/// `KbdSetCp` / `KbdGetCp`) and a key `KbdPeek` read from the host terminal
/// but left for the next `KbdCharIn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KbdState {
    /// `KBDINFO.fsMask`: echo and input modes. The default input mode is
    /// ASCII (KBD reference, `KbdStringIn`); echo on is Warpine's choice
    /// (the reference gives no default; unverified), what `KbdStringIn`
    /// always did.
    pub mask: u16,
    /// `KBDINFO.chTurnAround`: the character that ends `KbdStringIn`.
    pub turnaround: u16,
    /// `KBDINFO.fsInterim`.
    pub interim: u16,
    /// `KBDINFO.fsState`: the shift state.
    pub shift: u16,
    /// The keyboard codepage of `KbdSetCp`; 0 is the default table.
    pub codepage: u16,
    /// `(character, scan code)` of a key `KbdPeek` took from the terminal.
    pub peeked: Option<(u8, u8)>,
    /// The logical keyboard handles `KbdOpen` gave out (0, the default
    /// keyboard, is always valid).
    pub handles: Vec<u16>,
}

impl KbdState {
    /// Whether `hkbd` names the default keyboard or one `KbdOpen` gave out.
    pub fn valid(&self, hkbd: u32) -> bool {
        hkbd == 0 || u16::try_from(hkbd).is_ok_and(|h| self.handles.contains(&h))
    }

    /// A new logical keyboard handle: the lowest number above 0 not in use.
    pub fn open(&mut self) -> Option<u16> {
        let h = (1..=u16::MAX).find(|h| !self.handles.contains(h))?;
        self.handles.push(h);
        Some(h)
    }

    /// Close a handle `KbdOpen` gave out; false for any other.
    pub fn close(&mut self, hkbd: u32) -> bool {
        let before = self.handles.len();
        self.handles.retain(|&h| u32::from(h) != hkbd);
        self.handles.len() != before
    }
}

/// `KBDINFO.fsMask` bits (<bsesub.h>).
pub const KEYBOARD_ECHO_ON: u16 = 0x0001;
pub const KEYBOARD_ECHO_OFF: u16 = 0x0002;
pub const KEYBOARD_BINARY_MODE: u16 = 0x0004;
pub const KEYBOARD_ASCII_MODE: u16 = 0x0008;
/// The carriage return that ends `KbdStringIn` by default.
const DEFAULT_TURNAROUND: u16 = 0x0D;

impl Default for KbdState {
    fn default() -> Self {
        KbdState {
            mask: KEYBOARD_ECHO_ON | KEYBOARD_ASCII_MODE,
            turnaround: DEFAULT_TURNAROUND,
            interim: 0,
            shift: 0,
            codepage: 0,
            peeked: None,
            handles: Vec::new(),
        }
    }
}

/// `KBDSTF_*` shift-state bits (<bsesub.h>).
const KBDSTF_RIGHTSHIFT: u16 = 0x0001;
const KBDSTF_LEFTSHIFT: u16 = 0x0002;
const KBDSTF_CONTROL: u16 = 0x0004;
const KBDSTF_ALT: u16 = 0x0008;
const KBDSTF_CAPSLOCK_ON: u16 = 0x0040;

/// The US layout's scan codes (set 1) with their unshifted and shifted
/// characters: what `KbdXlate` translates (Warpine has no keyboard
/// layouts; the US one is its choice).
const US_KEYS: &[(u8, u8, u8)] = &[
    (0x01, 0x1B, 0x1B), (0x0E, 0x08, 0x08), (0x0F, b'\t', 0), (0x1C, b'\r', b'\r'), (0x39, b' ', b' '),
    (0x02, b'1', b'!'), (0x03, b'2', b'@'), (0x04, b'3', b'#'), (0x05, b'4', b'$'), (0x06, b'5', b'%'),
    (0x07, b'6', b'^'), (0x08, b'7', b'&'), (0x09, b'8', b'*'), (0x0A, b'9', b'('), (0x0B, b'0', b')'),
    (0x0C, b'-', b'_'), (0x0D, b'=', b'+'),
    (0x10, b'q', b'Q'), (0x11, b'w', b'W'), (0x12, b'e', b'E'), (0x13, b'r', b'R'), (0x14, b't', b'T'),
    (0x15, b'y', b'Y'), (0x16, b'u', b'U'), (0x17, b'i', b'I'), (0x18, b'o', b'O'), (0x19, b'p', b'P'),
    (0x1A, b'[', b'{'), (0x1B, b']', b'}'),
    (0x1E, b'a', b'A'), (0x1F, b's', b'S'), (0x20, b'd', b'D'), (0x21, b'f', b'F'), (0x22, b'g', b'G'),
    (0x23, b'h', b'H'), (0x24, b'j', b'J'), (0x25, b'k', b'K'), (0x26, b'l', b'L'), (0x27, b';', b':'),
    (0x28, b'\'', b'"'), (0x29, b'`', b'~'), (0x2B, b'\\', b'|'),
    (0x2C, b'z', b'Z'), (0x2D, b'x', b'X'), (0x2E, b'c', b'C'), (0x2F, b'v', b'V'), (0x30, b'b', b'B'),
    (0x31, b'n', b'N'), (0x32, b'm', b'M'), (0x33, b',', b'<'), (0x34, b'.', b'>'), (0x35, b'/', b'?'),
];

/// `KbdXlate`: the character of scan code `scan` in shift state `state`
/// (US layout): Shift, Caps Lock (letters only) and Ctrl (letters become
/// 1–26) apply; Alt and keys without a character give 0, the extended-key
/// form. `None` for a scan code the table does not know.
pub fn translate_scan(scan: u8, state: u16) -> Option<u8> {
    let &(_, plain, shifted) = US_KEYS.iter().find(|k| k.0 == scan)?;
    if state & KBDSTF_ALT != 0 { return Some(0); }
    let shift = state & (KBDSTF_LEFTSHIFT | KBDSTF_RIGHTSHIFT) != 0;
    let letter = plain.is_ascii_lowercase();
    let upper = shift ^ (letter && state & KBDSTF_CAPSLOCK_ON != 0);
    if state & KBDSTF_CONTROL != 0 {
        return Some(if letter { plain - b'a' + 1 } else { 0 });
    }
    Some(if upper { shifted } else { plain })
}

impl VioManager {
    /// The index of cell (row, col), or `None` off the screen.
    pub fn cell_index(&self, row: u16, col: u16) -> Option<usize> {
        (row < self.rows && col < self.cols).then(|| row as usize * self.cols as usize + col as usize)
    }

    /// Write `bytes` from (row, col) on, keeping each cell's attribute,
    /// going on at the next line and stopping at the end of the screen
    /// (`VioWrtCharStr`, `VioWrtNChar`). `cp` decodes the bytes.
    pub fn write_chars(&mut self, row: u16, col: u16, bytes: &[u8], cp: u32) {
        let Some(start) = self.cell_index(row, col) else { return };
        let end = (start + bytes.len()).min(self.buffer.len());
        for (idx, &raw) in (start..end).zip(bytes) {
            self.buffer[idx].0 = Self::decode_vio_byte(raw, cp);
            self.raw_bytes[idx] = raw;
        }
        self.repaint_cells(start, end);
    }

    /// The guest bytes of up to `max` cells from (row, col) on, going on at
    /// the next line and stopping at the end of the screen
    /// (`VioReadCharStr`).
    pub fn read_chars(&self, row: u16, col: u16, max: usize) -> Vec<u8> {
        let Some(start) = self.cell_index(row, col) else { return Vec::new() };
        let end = (start + max).min(self.raw_bytes.len());
        self.raw_bytes[start..end].to_vec()
    }

    /// Scroll the area from (top, left) to (bottom, right) by `lines` in
    /// `dir`, filling what comes in with `fill`. Corners and counts beyond
    /// the screen are clamped; 0 lines scroll nothing; as many lines as the
    /// area has, or more, fill it.
    pub fn scroll_rect(&mut self, (top, left, bottom, right): (u16, u16, u16, u16), dir: ScrollDir,
                       lines: u16, fill: (char, u8), fill_raw: u8) {
        if self.rows == 0 || self.cols == 0 || lines == 0 { return; }
        let bottom = bottom.min(self.rows - 1);
        let right = right.min(self.cols - 1);
        if top > bottom || left > right { return; }
        let extent = match dir {
            ScrollDir::Up | ScrollDir::Down => bottom - top + 1,
            ScrollDir::Left | ScrollDir::Right => right - left + 1,
        };
        let n = lines.min(extent);
        let cols = self.cols as usize;
        let at = |r: u16, c: u16| r as usize * cols + c as usize;
        // The cell each target cell takes its content from, or None to fill.
        let source = |r: u16, c: u16| -> Option<(u16, u16)> {
            match dir {
                ScrollDir::Up => (r + n <= bottom).then_some((r + n, c)),
                ScrollDir::Down => (r >= top + n).then(|| (r - n, c)),
                ScrollDir::Left => (c + n <= right).then_some((r, c + n)),
                ScrollDir::Right => (c >= left + n).then(|| (r, c - n)),
            }
        };
        let rows: Vec<u16> = if dir == ScrollDir::Down { (top..=bottom).rev().collect() } else { (top..=bottom).collect() };
        let columns: Vec<u16> = if dir == ScrollDir::Right { (left..=right).rev().collect() } else { (left..=right).collect() };
        for &r in &rows {
            for &c in &columns {
                let (cell, raw) = match source(r, c) {
                    Some((sr, sc)) => (self.buffer[at(sr, sc)], self.raw_bytes[at(sr, sc)]),
                    None => (fill, fill_raw),
                };
                self.buffer[at(r, c)] = cell;
                self.raw_bytes[at(r, c)] = raw;
            }
        }
        for r in top..=bottom {
            self.repaint_cells(at(r, left), at(r, right) + 1);
        }
    }

    /// Save the screen for a pop-up; a non-transparent one starts blank
    /// (light grey on black) with the cursor at the top left.
    pub fn begin_popup(&mut self, transparent: bool) -> SavedScreen {
        let saved = SavedScreen {
            buffer: self.buffer.clone(),
            raw_bytes: self.raw_bytes.clone(),
            cursor: (self.cursor_row, self.cursor_col),
        };
        if !transparent {
            self.clear_screen(super::ansi::DEFAULT_ATTR);
        }
        saved
    }

    /// Put back the screen a pop-up saved (if the screen kept its size).
    pub fn end_popup(&mut self, saved: SavedScreen) {
        if saved.buffer.len() == self.buffer.len() {
            self.buffer = saved.buffer;
            self.raw_bytes = saved.raw_bytes;
        }
        let len = self.buffer.len();
        self.repaint_cells(0, len);
        self.set_cursor_pos(saved.cursor.0, saved.cursor.1);
    }

    /// Show cells `start..end` on the host terminal (terminal mode; the SDL
    /// renderer reads the buffer itself).
    fn repaint_cells(&self, start: usize, end: usize) {
        if self.sdl_mode || start >= end || self.cols == 0 { return; }
        let cols = self.cols as usize;
        let mut out = String::new();
        let mut attr = None;
        let end = end.min(self.buffer.len());
        for (idx, &(ch, a)) in self.buffer.iter().enumerate().take(end).skip(start) {
            if idx == start || idx % cols == 0 {
                out.push_str(&format!("\x1b[{};{}H", idx / cols + 1, idx % cols + 1));
            }
            if attr != Some(a) {
                out.push_str(&super::console::ansi_attr_escape(a));
                attr = Some(a);
            }
            out.push(ch);
        }
        out.push_str(&format!("\x1b[0m\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1));
        let mut stdout = io::stdout();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4×3 SDL-mode screen whose cells read "abcd" / "efgh" / "ijkl".
    fn screen() -> VioManager {
        let mut v = VioManager::new();
        v.sdl_mode = true;
        v.rows = 3;
        v.cols = 4;
        v.buffer = "abcdefghijkl".chars().map(|c| (c, 0x07)).collect();
        v.raw_bytes = b"abcdefghijkl".to_vec();
        v
    }

    fn text(v: &VioManager) -> String {
        v.buffer.iter().map(|c| c.0).collect()
    }

    #[test]
    fn strings_wrap_and_stop_at_the_end_of_the_screen() {
        let mut v = screen();
        v.buffer[3].1 = 0x1F;
        v.write_chars(0, 2, b"XYZ", 437);
        assert_eq!(text(&v), "abXYZfghijkl");
        assert_eq!(v.buffer[3], ('Y', 0x1F), "the attribute stays");
        v.write_chars(2, 2, b"0123", 437);
        assert_eq!(&text(&v)[8..], "ij01", "stops at the end of the screen");
        assert_eq!(v.read_chars(0, 3, 3), b"YZf");
        assert_eq!(v.read_chars(2, 2, 10), b"01");
        assert!(v.read_chars(3, 0, 4).is_empty(), "off the screen");
    }

    #[test]
    fn scrolls_move_inside_the_rectangle_only() {
        let fill = ('.', 0x07);
        let mut v = screen();
        v.scroll_rect((0, 1, 1, 2), ScrollDir::Up, 1, fill, b'.');
        assert_eq!(text(&v), "afgde..hijkl");
        let mut v = screen();
        v.scroll_rect((0, 1, 1, 2), ScrollDir::Down, 1, fill, b'.');
        assert_eq!(text(&v), "a..debchijkl");
        let mut v = screen();
        v.scroll_rect((0, 0, 0, 3), ScrollDir::Left, 1, fill, b'.');
        assert_eq!(text(&v), "bcd.efghijkl");
        let mut v = screen();
        v.scroll_rect((1, 0, 1, 3), ScrollDir::Right, 2, fill, b'.');
        assert_eq!(text(&v), "abcd..efijkl");
        assert_eq!(&v.raw_bytes[4..8], b"..ef");
    }

    #[test]
    fn scroll_limits_clamp_and_zero_lines_do_nothing() {
        let fill = ('.', 0x07);
        let mut v = screen();
        v.scroll_rect((0, 0, 0xFFFF, 0xFFFF), ScrollDir::Up, 0xFFFF, fill, b'.');
        assert_eq!(text(&v), "............", "the whole screen filled");
        let mut v = screen();
        v.scroll_rect((0, 0, 0xFFFF, 0xFFFF), ScrollDir::Up, 0, fill, b'.');
        assert_eq!(text(&v), "abcdefghijkl", "0 lines scroll nothing");
        v.scroll_rect((2, 0, 1, 3), ScrollDir::Up, 1, fill, b'.');
        assert_eq!(text(&v), "abcdefghijkl", "an empty area");
    }

    #[test]
    fn a_popup_saves_and_restores_the_screen() {
        let mut v = screen();
        v.cursor_row = 1;
        v.cursor_col = 2;
        let saved = v.begin_popup(false);
        assert_eq!(text(&v), "            ");
        assert_eq!((v.cursor_row, v.cursor_col), (0, 0));
        v.write_chars(0, 0, b"pop", 437);
        v.end_popup(saved);
        assert_eq!(text(&v), "abcdefghijkl");
        assert_eq!((v.cursor_row, v.cursor_col), (1, 2));
        let saved = v.begin_popup(true);
        assert_eq!(text(&v), "abcdefghijkl", "a transparent pop-up keeps the screen");
        v.end_popup(saved);
    }

    #[test]
    fn keyboard_state_defaults() {
        let k = KbdState::default();
        assert_eq!(k.mask, KEYBOARD_ECHO_ON | KEYBOARD_ASCII_MODE);
        assert_eq!(k.turnaround, 0x0D);
        assert_eq!(k.peeked, None);
    }

    #[test]
    fn scan_codes_translate_with_the_shift_state() {
        assert_eq!(translate_scan(0x1E, 0), Some(b'a'));
        assert_eq!(translate_scan(0x1E, KBDSTF_LEFTSHIFT), Some(b'A'));
        assert_eq!(translate_scan(0x1E, KBDSTF_CAPSLOCK_ON), Some(b'A'));
        assert_eq!(translate_scan(0x1E, KBDSTF_CAPSLOCK_ON | KBDSTF_RIGHTSHIFT), Some(b'a'));
        assert_eq!(translate_scan(0x02, KBDSTF_CAPSLOCK_ON), Some(b'1'), "Caps Lock only changes letters");
        assert_eq!(translate_scan(0x02, KBDSTF_LEFTSHIFT), Some(b'!'));
        assert_eq!(translate_scan(0x2E, KBDSTF_CONTROL), Some(3), "Ctrl+C");
        assert_eq!(translate_scan(0x1E, KBDSTF_ALT), Some(0), "Alt: an extended key");
        assert_eq!(translate_scan(0x3B, 0), None, "F1 is not in the table");
    }

    #[test]
    fn logical_keyboard_handles() {
        let mut k = KbdState::default();
        assert!(k.valid(0));
        assert!(!k.valid(1));
        assert_eq!(k.open(), Some(1));
        assert_eq!(k.open(), Some(2));
        assert!(k.valid(2));
        assert!(k.close(1));
        assert!(!k.close(1), "closed already");
        assert!(!k.close(0), "the default keyboard is not closed");
        assert_eq!(k.open(), Some(1), "the lowest free number");
        assert!(!k.valid(0x1_0001));
    }
}
