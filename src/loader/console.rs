// SPDX-License-Identifier: GPL-3.0-or-later
//
// Console manager for OS/2 VIO (Video I/O) subsystem.
// Maintains a screen buffer and cursor state, outputs via ANSI escape sequences.

use std::io::{self, Write};

use super::ansi::{AnsiParser, Csi, Feed, Rendition, DEFAULT_ATTR, ESC};

/// Tab stops of VioWrtTTY output: "standard 8-byte-wide fields".
const TAB_WIDTH: u16 = 8;
/// ANSI `ESC[=7h` / `ESC[=7l`: wrap at the end of a line on / off.
const ANSI_MODE_WRAP: u16 = 7;
/// ANSI `ESC[=nh` display modes 0–3: 40 × 25 (0, 1) and 80 × 25 (2, 3) text.
const ANSI_MODE_LAST_TEXT: u16 = 3;
const ANSI_MODE_FIRST_80_COLS: u16 = 2;
const ANSI_MODE_ROWS: u16 = 25;
/// ANSI `ESC[2J`: clear the screen.
const ANSI_ED_SCREEN: u16 = 2;
/// Glyph shown on a terminal for an ESC written as a character (CP437 0x1B).
const ESC_GLYPH: char = '\u{2190}';

/// Convert a CP437 byte to its Unicode character.
/// Bytes 0x00–0x7F are ASCII-identical. Bytes 0x80–0xFF map to CP437-specific glyphs
/// (box-drawing, accented letters, Greek, math symbols, etc.).
pub fn cp437_to_char(byte: u8) -> char {
    if byte < 0x80 {
        return byte as char;
    }
    // CP437 high bytes (0x80–0xFF) → Unicode code points
    const CP437_HIGH: [char; 128] = [
        // 0x80–0x8F
        'Ç','ü','é','â','ä','à','å','ç','ê','ë','è','ï','î','ì','Ä','Å',
        // 0x90–0x9F
        'É','æ','Æ','ô','ö','ò','û','ù','ÿ','Ö','Ü','¢','£','¥','₧','ƒ',
        // 0xA0–0xAF
        'á','í','ó','ú','ñ','Ñ','ª','º','¿','⌐','¬','½','¼','¡','«','»',
        // 0xB0–0xBF
        '░','▒','▓','│','┤','╡','╢','╖','╕','╣','║','╗','╝','╜','╛','┐',
        // 0xC0–0xCF
        '└','┴','┬','├','─','┼','╞','╟','╚','╔','╩','╦','╠','═','╬','╧',
        // 0xD0–0xDF
        '╨','╤','╥','╙','╘','╒','╓','╫','╪','┘','┌','█','▄','▌','▐','▀',
        // 0xE0–0xEF
        'α','ß','Γ','π','Σ','σ','µ','τ','Φ','Θ','Ω','δ','∞','φ','ε','∩',
        // 0xF0–0xFF
        '≡','±','≥','≤','⌠','⌡','÷','≈','°','∙','·','√','ⁿ','²','■','\u{00A0}',
    ];
    CP437_HIGH[(byte - 0x80) as usize]
}

/// OS/2 VIO attribute byte: bits 0-2 = fg color, 3 = fg bright, 4-6 = bg color, 7 = blink/bg bright.
/// Maps to standard 16-color CGA palette.
const CGA_TO_ANSI_FG: [u8; 8] = [30, 34, 32, 36, 31, 35, 33, 37]; // black, blue, green, cyan, red, magenta, brown, white
const CGA_TO_ANSI_BG: [u8; 8] = [40, 44, 42, 46, 41, 45, 43, 47];

pub struct VioManager {
    /// Screen buffer: (Unicode codepoint, attribute) pairs, row-major.
    /// Characters are decoded from the guest's active codepage at write time.
    pub buffer: Vec<(char, u8)>,
    /// Raw guest bytes parallel to `buffer`, used by the DBCS annotation pass.
    /// Stores the original byte written by the guest for each cell (0x20 for
    /// synthesised fill cells such as scroll fill and resize fill).
    pub raw_bytes: Vec<u8>,
    pub rows: u16,
    pub cols: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    /// Cursor shape: first scan line (0 = top of cell).
    pub cursor_start: u8,
    /// Cursor shape: last scan line (15 = bottom of cell for 8×16 font).
    pub cursor_end: u8,
    /// ANSI support of TTY output (`VioSetAnsi`); on by default.
    pub ansi_mode: bool,
    /// Escape-sequence state, kept between writes.
    ansi: AnsiParser,
    /// Graphics rendition set by ANSI `ESC[...m`; its attribute colours
    /// `write_tty` output ("the current attribute defined by ANSI or the
    /// default value of 7", VioWrtTTY reference).
    pub rendition: Rendition,
    /// Cursor saved by `ESC[s` (one level; nested saves are not supported).
    saved_cursor: Option<(u16, u16)>,
    /// Wrap at the end of a line (ANSI mode 7; on by default).
    pub wrap: bool,
    pub codepage: u16,
    /// When `true`, all ANSI escape output is suppressed; the SDL text
    /// renderer reads the buffer directly.  Also forces rows/cols to 80×25
    /// and prevents `enable_raw_mode` from touching termios.
    pub sdl_mode: bool,
    /// Whether terminal raw mode has been activated.
    raw_mode_active: bool,
    /// Original termios saved for restore.
    original_termios: Option<libc::termios>,
    /// Pending LF byte after CR→CRLF translation for DosRead on stdin.
    pub stdin_pending_lf: bool,
    /// Number of characters typed (echoed) since the last CR/LF in cooked-mode
    /// DosRead(fd=0).  Used to prevent backspace from erasing beyond the start
    /// of the current input line (e.g. into the shell prompt).
    pub stdin_cooked_chars: i32,
}

impl Default for VioManager {
    fn default() -> Self { Self::new() }
}

impl VioManager {
    pub fn new() -> Self {
        let (rows, cols) = Self::detect_terminal_size();
        let size = rows as usize * cols as usize;
        VioManager {
            buffer: vec![(' ', 0x07); size], // space with light gray on black
            raw_bytes: vec![b' '; size],
            rows,
            cols,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            cursor_start: 14,
            cursor_end: 15,
            ansi_mode: true,
            ansi: AnsiParser::new(),
            rendition: Rendition::default(),
            saved_cursor: None,
            wrap: true,
            codepage: 437,
            sdl_mode: false,
            raw_mode_active: false,
            original_termios: None,
            stdin_pending_lf: false,
            stdin_cooked_chars: 0,
        }
    }

    /// Switch to SDL text-mode: initialise to 80×25, suppress ANSI output.
    pub fn enable_sdl_mode(&mut self) {
        self.sdl_mode = true;
        // Start at the standard 80×25 text mode; VioSetMode may later change
        // the dimensions and the SDL window will resize to match.
        self.resize(25, 80);
    }

    /// Resize the virtual screen to `new_rows` × `new_cols`.
    ///
    /// Preserves as much of the existing buffer content as possible (top-left
    /// content is kept; new cells are initialised to space/attribute 0x07).
    /// Called by `VioSetMode` and `enable_sdl_mode`.
    pub fn resize(&mut self, new_rows: u16, new_cols: u16) {
        if self.rows == new_rows && self.cols == new_cols { return; }
        let new_size = new_rows as usize * new_cols as usize;
        let mut new_buf: Vec<(char, u8)> = vec![(' ', 0x07); new_size];
        let mut new_raw: Vec<u8> = vec![b' '; new_size];
        // Copy the top-left region that fits in both old and new dimensions.
        let copy_rows = self.rows.min(new_rows) as usize;
        let copy_cols = self.cols.min(new_cols) as usize;
        for r in 0..copy_rows {
            for c in 0..copy_cols {
                let src = r * self.cols as usize + c;
                let dst = r * new_cols as usize + c;
                new_buf[dst] = self.buffer[src];
                new_raw[dst] = self.raw_bytes[src];
            }
        }
        self.rows = new_rows;
        self.cols = new_cols;
        self.buffer = new_buf;
        self.raw_bytes = new_raw;
        // Clamp cursor to new boundaries.
        self.cursor_row = self.cursor_row.min(new_rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(new_cols.saturating_sub(1));
    }

    /// Set cursor scan-line shape (for `VioSetCurType`).
    pub fn set_cursor_shape(&mut self, start: u8, end: u8) {
        self.cursor_start = start;
        self.cursor_end = end;
    }

    /// Detect terminal size, defaulting to 25x80.
    fn detect_terminal_size() -> (u16, u16) {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
        if ret == 0 && ws.ws_row > 0 && ws.ws_col > 0 {
            (ws.ws_row, ws.ws_col)
        } else {
            (25, 80)
        }
    }

    /// Enable terminal raw mode for keyboard input.
    /// No-op in SDL text mode (keyboard events come from SDL instead).
    pub fn enable_raw_mode(&mut self) {
        if self.raw_mode_active || self.sdl_mode { return; }
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut termios) } == 0 {
            self.original_termios = Some(termios);
            let mut raw = termios;
            // Disable canonical mode, echo, and signal generation
            raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
            // Disable input processing
            raw.c_iflag &= !(libc::IXON | libc::ICRNL | libc::BRKINT | libc::INPCK | libc::ISTRIP);
            // Disable output processing
            raw.c_oflag &= !libc::OPOST;
            // Read returns after 1 byte, with 100ms timeout
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 1; // 100ms timeout
            unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw); }
            self.raw_mode_active = true;
        }
    }

    /// Restore terminal to original mode.
    pub fn disable_raw_mode(&mut self) {
        if let Some(orig) = self.original_termios.as_ref() {
            // Restore saved termios first
            unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, orig); }
            self.raw_mode_active = false;
        }
        // As a safety net, force-enable critical cooked-mode flags if raw mode
        // was ever activated.  Guard is intentional: a child warpine process
        // running headless (stdout piped) never sets raw mode, so calling
        // TCSAFLUSH here would flush pending input on the *parent's* shared
        // terminal fd and break the parent shell's prompt.
        if self.raw_mode_active || self.original_termios.is_some() {
            let mut cur: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut cur) } == 0 {
                cur.c_lflag |= libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN;
                cur.c_iflag |= libc::ICRNL;
                cur.c_oflag |= libc::OPOST;
                unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &cur); }
            }
        }
    }

    /// Decode a single VIO byte to a Unicode char via the active codepage.
    pub(crate) fn decode_vio_byte(b: u8, cp: u32) -> char {
        if b < 0x80 { return b as char; }
        let s = super::codepage::cp_decode(&[b], cp);
        s.chars().next().unwrap_or(b as char)
    }

    /// Write a Unicode char to stdout as UTF-8.
    fn write_char(stdout: &mut io::Stdout, ch: char) {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        let _ = stdout.write_all(s.as_bytes());
    }

    /// Switch ANSI support on or off (`VioSetAnsi`); an unfinished escape
    /// sequence is dropped.
    pub fn set_ansi(&mut self, on: bool) {
        self.ansi_mode = on;
        self.ansi.reset();
    }

    /// The attribute TTY output is written with.
    pub fn tty_attr(&self) -> u8 { self.rendition.attr() }

    /// Write text as `VioWrtTTY` does, at the cursor, advancing it.
    ///
    /// CR, LF, backspace (non-destructive), BEL and TAB (to the next multiple
    /// of 8) are commands; the rest is text in the current ANSI attribute.
    /// While ANSI support is on, escape sequences are carried out — also when
    /// split across calls. Updates the screen buffer; in terminal (non-SDL)
    /// mode also echoes the result to the host terminal.
    /// `cp` is the active OS/2 codepage used to decode non-ASCII bytes.
    pub fn write_tty(&mut self, text: &[u8], cp: u32) {
        self.write_tty_to(text, cp, !self.sdl_mode);
    }

    /// `write_tty` that only updates the screen buffer and cursor, for
    /// console output whose bytes go to a host pipe or file unchanged: the
    /// screen the program reads back (`VioReadCellStr`, `VioGetCurPos`)
    /// stays the one it wrote.
    pub fn track_tty(&mut self, text: &[u8], cp: u32) {
        self.write_tty_to(text, cp, false);
    }

    fn write_tty_to(&mut self, text: &[u8], cp: u32, echo: bool) {
        let mut out = TtyOut::new(!echo);
        for &b in text {
            if !self.ansi_mode {
                self.tty_byte(b, cp, &mut out);
                continue;
            }
            match self.ansi.feed(b) {
                Feed::Pending => {}
                Feed::Text(b) => self.tty_byte(b, cp, &mut out),
                Feed::EscThen(b) => {
                    self.tty_byte(ESC, cp, &mut out);
                    self.tty_byte(b, cp, &mut out);
                }
                Feed::Sequence(csi) => self.apply_csi(&csi, &mut out),
            }
        }
        out.finish();
    }

    /// `write_tty` in attribute `attr` instead of the ANSI one (the built-in
    /// CMD.EXE's coloured prompt and messages); the ANSI rendition is kept.
    pub fn write_tty_attr(&mut self, text: &[u8], attr: u8, cp: u32) {
        let saved = self.rendition;
        self.rendition = Rendition::from_attr(attr);
        self.write_tty(text, cp);
        self.rendition = saved;
    }

    /// One TTY byte that is not part of an escape sequence.
    fn tty_byte(&mut self, ch: u8, cp: u32, out: &mut TtyOut) {
        match ch {
            b'\n' => {
                self.cursor_col = 0;
                self.line_feed();
                out.newline();
            }
            b'\r' => {
                self.cursor_col = 0;
                out.raw(b"\r");
            }
            0x08 => {
                self.cursor_col = self.cursor_col.saturating_sub(1);
                out.raw(b"\x08");
            }
            0x07 => out.raw(b"\x07"),
            b'\t' => {
                let next = (self.cursor_col / TAB_WIDTH + 1) * TAB_WIDTH;
                let spaces = next.min(self.cols).saturating_sub(self.cursor_col).max(1);
                for _ in 0..spaces {
                    self.put_tty_char(b' ', cp, out);
                }
            }
            _ => self.put_tty_char(ch, cp, out),
        }
    }

    /// Put one character at the cursor and advance, wrapping (or, with
    /// wrapping off, staying in the last column) and scrolling.
    fn put_tty_char(&mut self, ch: u8, cp: u32, out: &mut TtyOut) {
        if self.rows == 0 || self.cols == 0 { return; }
        let attr = self.tty_attr();
        let decoded = Self::decode_vio_byte(ch, cp);
        self.cursor_row = self.cursor_row.min(self.rows - 1);
        self.cursor_col = self.cursor_col.min(self.cols - 1);
        let idx = self.cursor_row as usize * self.cols as usize + self.cursor_col as usize;
        if idx < self.buffer.len() {
            self.buffer[idx] = (decoded, attr);
            self.raw_bytes[idx] = ch;
        }
        let shown = if ch == ESC { ESC_GLYPH } else { decoded };
        out.text(shown, attr);
        if self.cursor_col + 1 < self.cols {
            self.cursor_col += 1;
        } else if self.wrap {
            self.cursor_col = 0;
            self.line_feed();
        } else {
            // Wrapping off: later characters overwrite the last column.
            out.cursor(self.cursor_row, self.cursor_col);
        }
    }

    /// Move the cursor down a row, scrolling the screen at the bottom. The
    /// terminal scrolls by itself when the newline or wrapped text reaches it.
    fn line_feed(&mut self) {
        if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        } else if self.rows > 0 {
            self.shift_up(0, self.rows - 1, 1, (' ', DEFAULT_ATTR), b' ');
            self.cursor_row = self.rows - 1;
        }
    }

    /// Carry out a complete ANSI control sequence.
    fn apply_csi(&mut self, csi: &Csi, out: &mut TtyOut) {
        if self.rows == 0 || self.cols == 0 { return; }
        let (last_row, last_col) = (self.rows - 1, self.cols - 1);
        match (csi.private, csi.final_byte) {
            (None, b'A') => self.cursor_row = self.cursor_row.saturating_sub(csi.count(0)),
            (None, b'B') => self.cursor_row = self.cursor_row.saturating_add(csi.count(0)).min(last_row),
            (None, b'C') => self.cursor_col = self.cursor_col.saturating_add(csi.count(0)).min(last_col),
            (None, b'D') => self.cursor_col = self.cursor_col.saturating_sub(csi.count(0)),
            (None, b'H' | b'f') => {
                self.cursor_row = (csi.count(0) - 1).min(last_row);
                self.cursor_col = (csi.count(1) - 1).min(last_col);
            }
            (None, b's') => self.saved_cursor = Some((self.cursor_row, self.cursor_col)),
            (None, b'u') => {
                if let Some((r, c)) = self.saved_cursor {
                    self.cursor_row = r.min(last_row);
                    self.cursor_col = c.min(last_col);
                }
            }
            (None, b'J') if csi.param(0, 0) == ANSI_ED_SCREEN => {
                let attr = self.tty_attr();
                self.buffer.fill((' ', attr));
                self.raw_bytes.fill(b' ');
                self.cursor_row = 0;
                self.cursor_col = 0;
                out.clear_screen(attr);
                return;
            }
            (None, b'K') if csi.param(0, 0) == 0 => {
                let attr = self.tty_attr();
                let start = self.cursor_row as usize * self.cols as usize + self.cursor_col as usize;
                let end = (self.cursor_row as usize + 1) * self.cols as usize;
                if end <= self.buffer.len() && start <= end {
                    self.buffer[start..end].fill((' ', attr));
                    self.raw_bytes[start..end].fill(b' ');
                }
                out.erase_line(attr);
                return;
            }
            (None, b'm') => {
                self.rendition.apply_sgr(&csi.params);
                return;
            }
            // Display modes: 0–3 are the 40 / 80 × 25 text modes (mono or
            // colour: the same here), set like VioSetMode and cleared; the
            // graphics modes 4–6 are not supported (a text-only console).
            (Some(_), b'h') if csi.param(0, 0) <= ANSI_MODE_LAST_TEXT => {
                let cols = if csi.param(0, 0) < ANSI_MODE_FIRST_80_COLS { 40 } else { 80 };
                self.resize(ANSI_MODE_ROWS, cols);
                let attr = self.tty_attr();
                self.buffer.fill((' ', attr));
                self.raw_bytes.fill(b' ');
                self.cursor_row = 0;
                self.cursor_col = 0;
                out.clear_screen(attr);
                return;
            }
            (Some(_), b'h') if csi.param(0, 0) == ANSI_MODE_WRAP => { self.wrap = true; return; }
            (Some(_), b'l') if csi.param(0, 0) == ANSI_MODE_WRAP => { self.wrap = false; return; }
            // Graphics display modes 4–6, the cursor position report (ESC[6n) and
            // keyboard reassignment (ESC[...p) are not supported; other
            // sequences are dropped.
            _ => {
                log::debug!("ANSI: ignored sequence {:?}", csi);
                return;
            }
        }
        out.cursor(self.cursor_row, self.cursor_col);
    }

    /// Scroll rows `top..=bottom` of the buffer up by `lines` (≥ 1), filling
    /// the vacated rows; no terminal output.
    fn shift_up(&mut self, top: u16, bottom: u16, lines: u16, fill_cell: (char, u8), fill_raw: u8) {
        if top > bottom || bottom >= self.rows || lines == 0 { return; }
        let cols = self.cols as usize;
        let lines = lines.min(bottom - top + 1);
        for row in top..(bottom - lines + 1) {
            let dst = row as usize * cols;
            let src = (row + lines) as usize * cols;
            for c in 0..cols {
                if src + c < self.buffer.len() && dst + c < self.buffer.len() {
                    self.buffer[dst + c] = self.buffer[src + c];
                    self.raw_bytes[dst + c] = self.raw_bytes[src + c];
                }
            }
        }
        for row in (bottom - lines + 1)..=bottom {
            let base = row as usize * cols;
            for c in 0..cols {
                if base + c < self.buffer.len() {
                    self.buffer[base + c] = fill_cell;
                    self.raw_bytes[base + c] = fill_raw;
                }
            }
        }
    }

    /// Clear the whole screen to blanks in `attr` and home the cursor (the
    /// built-in CMD.EXE's CLS).
    pub fn clear_screen(&mut self, attr: u8) {
        self.buffer.fill((' ', attr));
        self.raw_bytes.fill(b' ');
        self.cursor_row = 0;
        self.cursor_col = 0;
        let mut out = TtyOut::new(self.sdl_mode);
        out.clear_screen(attr);
        out.finish();
    }

    /// Write an attributed string at a specific position.
    /// `cp` is the active OS/2 codepage used to decode non-ASCII bytes.
    pub fn write_char_str_att(&mut self, row: u16, col: u16, text: &[u8], attr: u8, cp: u32) {
        let sdl = self.sdl_mode;
        let mut stdout = io::stdout();
        if !sdl {
            let _ = write!(stdout, "\x1b[{};{}H", row + 1, col + 1);
            self.write_ansi_attr(&mut stdout, attr);
        }
        for (c, &ch) in (col..self.cols).zip(text.iter()) {
            let decoded = Self::decode_vio_byte(ch, cp);
            let idx = row as usize * self.cols as usize + c as usize;
            if idx < self.buffer.len() {
                self.buffer[idx] = (decoded, attr);
                self.raw_bytes[idx] = ch;
            }
            if !sdl { Self::write_char(&mut stdout, decoded); }
        }
        if !sdl {
            let _ = stdout.write_all(b"\x1b[0m");
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Fill N cells starting at (row, col) with a character+attribute pair.
    /// `cell.0` is already a decoded Unicode char (caller decodes via active codepage).
    /// `raw_byte` is the original guest byte, stored for the DBCS annotation pass.
    pub fn write_n_cell(&mut self, row: u16, col: u16, raw_byte: u8, cell: (char, u8), count: u16) {
        let sdl = self.sdl_mode;
        let mut stdout = io::stdout();
        if !sdl {
            let _ = write!(stdout, "\x1b[{};{}H", row + 1, col + 1);
            self.write_ansi_attr(&mut stdout, cell.1);
        }
        for c in (col..self.cols).take(count as usize) {
            let idx = row as usize * self.cols as usize + c as usize;
            if idx < self.buffer.len() {
                self.buffer[idx] = cell;
                self.raw_bytes[idx] = raw_byte;
            }
            if !sdl { Self::write_char(&mut stdout, cell.0); }
        }
        if !sdl {
            let _ = stdout.write_all(b"\x1b[0m");
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Write (byte, attribute) cells from (row, col) onwards, continuing at the
    /// start of the next row and stopping at the end of the screen
    /// (`VioWrtCellStr`). Bytes are decoded in codepage `cp`; the raw bytes are
    /// kept for DBCS annotation.
    pub fn write_cell_str(&mut self, row: u16, col: u16, cells: &[(u8, u8)], cp: u32) {
        let start = row as usize * self.cols as usize + col as usize;
        let room = self.buffer.len().saturating_sub(start);
        let cols = self.cols.max(1) as usize;
        for (i, &(raw, attr)) in cells.iter().take(room).enumerate() {
            let idx = start + i;
            let cell = (Self::decode_vio_byte(raw, cp), attr);
            if self.sdl_mode {
                self.buffer[idx] = cell;
                self.raw_bytes[idx] = raw;
            } else {
                self.write_n_cell((idx / cols) as u16, (idx % cols) as u16, raw, cell, 1);
            }
        }
    }

    /// Fill N attribute bytes starting at (row, col), preserving characters.
    pub fn write_n_attr(&mut self, row: u16, col: u16, attr: u8, count: u16) {
        let sdl = self.sdl_mode;
        let mut stdout = io::stdout();
        if !sdl {
            let _ = write!(stdout, "\x1b[{};{}H", row + 1, col + 1);
            self.write_ansi_attr(&mut stdout, attr);
        }
        for c in (col..self.cols).take(count as usize) {
            let idx = row as usize * self.cols as usize + c as usize;
            if idx < self.buffer.len() {
                let ch = self.buffer[idx].0;
                self.buffer[idx].1 = attr;
                if !sdl { Self::write_char(&mut stdout, ch); }
            }
        }
        if !sdl {
            let _ = stdout.write_all(b"\x1b[0m");
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Read cell string from the screen buffer.
    /// Returns decoded Unicode chars; callers must re-encode to the active codepage for guest writes.
    pub fn read_cell_str(&self, row: u16, col: u16, max_len: u16) -> Vec<(char, u8)> {
        let mut result = Vec::new();
        for c in (col..self.cols).take(max_len as usize / 2) {
            let idx = row as usize * self.cols as usize + c as usize;
            if idx < self.buffer.len() {
                result.push(self.buffer[idx]);
            }
        }
        result
    }

    /// Scroll a region up by `lines` rows, filling the bottom with `fill_cell`.
    ///
    /// OS/2 special case: `lines == 0` means "clear the entire region" (fill all
    /// rows from `top` to `bottom` with `fill_cell` without scrolling).
    pub fn scroll_up(&mut self, top: u16, bottom: u16, lines: u16, fill_cell: (char, u8), fill_raw: u8) {
        if top > bottom || bottom >= self.rows { return; }
        if lines == 0 {
            // Clear entire region
            let cols = self.cols as usize;
            for row in top..=bottom {
                let base = row as usize * cols;
                for c in 0..cols {
                    if base + c < self.buffer.len() {
                        self.buffer[base + c] = fill_cell;
                        self.raw_bytes[base + c] = fill_raw;
                    }
                }
            }
            if !self.sdl_mode {
                let mut stdout = io::stdout();
                let _ = write!(stdout, "\x1b[{};{}r", top + 1, bottom + 1);
                for _ in 0..(bottom - top + 1) {
                    let _ = write!(stdout, "\x1b[{}S", 1);
                }
                let _ = write!(stdout, "\x1b[;r");
                let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
                let _ = stdout.flush();
            }
            return;
        }
        let lines = lines.min(bottom - top + 1);
        self.shift_up(top, bottom, lines, fill_cell, fill_raw);

        // Output ANSI scroll (terminal mode only)
        if !self.sdl_mode {
            let mut stdout = io::stdout();
            if top == 0 && bottom == self.rows - 1 {
                for _ in 0..lines {
                    let _ = writeln!(stdout, "\x1b[{};{}H", bottom + 1, 1);
                }
            } else {
                let _ = write!(stdout, "\x1b[{};{}r", top + 1, bottom + 1);
                let _ = write!(stdout, "\x1b[{}S", lines);
                let _ = write!(stdout, "\x1b[;r");
            }
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Scroll a region down by `lines` rows, filling the top with `fill_cell`.
    ///
    /// OS/2 special case: `lines == 0` means "clear the entire region".
    pub fn scroll_down(&mut self, top: u16, bottom: u16, lines: u16, fill_cell: (char, u8), fill_raw: u8) {
        if top > bottom || bottom >= self.rows { return; }
        if lines == 0 {
            let cols = self.cols as usize;
            for row in top..=bottom {
                let base = row as usize * cols;
                for c in 0..cols {
                    if base + c < self.buffer.len() {
                        self.buffer[base + c] = fill_cell;
                        self.raw_bytes[base + c] = fill_raw;
                    }
                }
            }
            if !self.sdl_mode {
                let mut stdout = io::stdout();
                let _ = write!(stdout, "\x1b[{};{}r", top + 1, bottom + 1);
                for _ in 0..(bottom - top + 1) {
                    let _ = write!(stdout, "\x1b[{}T", 1);
                }
                let _ = write!(stdout, "\x1b[;r");
                let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
                let _ = stdout.flush();
            }
            return;
        }
        let cols = self.cols as usize;
        let lines = lines.min(bottom - top + 1);

        // Shift buffer rows down
        for row in ((top + lines)..=bottom).rev() {
            let dst = row as usize * cols;
            let src = (row - lines) as usize * cols;
            for c in 0..cols {
                if src + c < self.buffer.len() && dst + c < self.buffer.len() {
                    self.buffer[dst + c] = self.buffer[src + c];
                    self.raw_bytes[dst + c] = self.raw_bytes[src + c];
                }
            }
        }
        // Fill vacated rows
        for row in top..(top + lines) {
            let base = row as usize * cols;
            for c in 0..cols {
                if base + c < self.buffer.len() {
                    self.buffer[base + c] = fill_cell;
                    self.raw_bytes[base + c] = fill_raw;
                }
            }
        }

        if !self.sdl_mode {
            let mut stdout = io::stdout();
            let _ = write!(stdout, "\x1b[{};{}r", top + 1, bottom + 1);
            let _ = write!(stdout, "\x1b[{}T", lines);
            let _ = write!(stdout, "\x1b[;r");
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Set cursor position and output ANSI escape (in terminal mode).
    pub fn set_cursor_pos(&mut self, row: u16, col: u16) {
        self.cursor_row = row.min(self.rows - 1);
        self.cursor_col = col.min(self.cols - 1);
        if !self.sdl_mode {
            let mut stdout = io::stdout();
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Set cursor visibility (in terminal mode, also emits ANSI escape).
    pub fn set_cursor_type(&mut self, visible: bool) {
        self.cursor_visible = visible;
        if !self.sdl_mode {
            let mut stdout = io::stdout();
            if visible {
                let _ = stdout.write_all(b"\x1b[?25h");
            } else {
                let _ = stdout.write_all(b"\x1b[?25l");
            }
            let _ = stdout.flush();
        }
    }

    /// Write ANSI color escape for an OS/2 attribute byte.
    fn write_ansi_attr(&self, stdout: &mut io::Stdout, attr: u8) {
        let _ = stdout.write_all(ansi_attr_escape(attr).as_bytes());
    }

    /// Read a single byte from stdin (non-blocking with short timeout).
    /// Returns None if no input available within timeout.
    pub fn read_byte(&self) -> Option<u8> {
        match self.read_input() {
            HostInput::Byte(b) => Some(b),
            HostInput::Empty | HostInput::EndOfInput => None,
        }
    }

    /// Read a byte from the host's standard input, telling "no key yet" from
    /// the end of input: a terminal in raw mode returns no bytes while no
    /// key is pressed, anything else (pipe, file, `/dev/null`) only at its end.
    pub fn read_input(&self) -> HostInput {
        let mut buf = [0u8; 1];
        // SAFETY: reads at most one byte into a one-byte local buffer.
        let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1) };
        classify_read(n, buf[0], std::io::IsTerminal::is_terminal(&std::io::stdin()))
    }
}

/// What a read of the host's standard input found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostInput {
    Byte(u8),
    /// Nothing yet (a terminal with no key pressed, or a read to retry).
    Empty,
    /// A pipe or file at its end: no key will ever come.
    EndOfInput,
}

/// Classify the result `n` of a one-byte `read(2)` of standard input.
fn classify_read(n: isize, byte: u8, stdin_is_terminal: bool) -> HostInput {
    match n {
        1 => HostInput::Byte(byte),
        0 if !stdin_is_terminal => HostInput::EndOfInput,
        _ => HostInput::Empty,
    }
}

/// Host-terminal echo of `write_tty` (terminal mode); does nothing in SDL
/// mode, where the renderer reads the buffer.
struct TtyOut {
    bytes: Option<Vec<u8>>,
    /// The attribute last set on the terminal, if any.
    attr: Option<u8>,
}

impl TtyOut {
    fn new(silent: bool) -> Self {
        TtyOut { bytes: if silent { None } else { Some(Vec::new()) }, attr: None }
    }

    fn raw(&mut self, b: &[u8]) {
        if let Some(v) = self.bytes.as_mut() { v.extend_from_slice(b); }
    }

    fn set_attr(&mut self, attr: u8) {
        // The default attribute is the terminal's own colours, so plain
        // output keeps the user's terminal theme.
        if attr == DEFAULT_ATTR {
            self.reset_attr();
        } else if self.bytes.is_some() && self.attr != Some(attr) {
            self.raw(ansi_attr_escape(attr).as_bytes());
            self.attr = Some(attr);
        }
    }

    fn reset_attr(&mut self) {
        if self.attr.take().is_some() { self.raw(b"\x1b[0m"); }
    }

    fn text(&mut self, ch: char, attr: u8) {
        if self.bytes.is_none() { return; }
        self.set_attr(attr);
        let mut buf = [0u8; 4];
        self.raw(ch.encode_utf8(&mut buf).as_bytes());
    }

    fn newline(&mut self) {
        // Reset first so the scrolled-in line is not painted in the colour.
        self.reset_attr();
        self.raw(b"\n");
    }

    fn cursor(&mut self, row: u16, col: u16) {
        if self.bytes.is_some() {
            let s = format!("\x1b[{};{}H", row as u32 + 1, col as u32 + 1);
            self.raw(s.as_bytes());
        }
    }

    fn clear_screen(&mut self, attr: u8) {
        if self.bytes.is_none() { return; }
        self.set_attr(attr);
        self.raw(b"\x1b[2J\x1b[1;1H");
    }

    fn erase_line(&mut self, attr: u8) {
        if self.bytes.is_none() { return; }
        self.set_attr(attr);
        self.raw(b"\x1b[K");
    }

    /// Bytes collected so far (tests).
    #[cfg(test)]
    fn collected(&self) -> &[u8] { self.bytes.as_deref().unwrap_or(&[]) }

    fn finish(mut self) {
        self.reset_attr();
        if let Some(v) = self.bytes.take()
            && !v.is_empty() {
            let mut stdout = io::stdout();
            let _ = stdout.write_all(&v);
            let _ = stdout.flush();
        }
    }
}

/// Terminal SGR escape for a VIO attribute byte.
fn ansi_attr_escape(attr: u8) -> String {
    let fg = CGA_TO_ANSI_FG[(attr & 0x07) as usize];
    let bg = CGA_TO_ANSI_BG[((attr >> 4) & 0x07) as usize];
    if attr & 0x08 != 0 {
        format!("\x1b[{};{};1m", fg, bg)
    } else {
        format!("\x1b[{};{}m", fg, bg)
    }
}

impl Drop for VioManager {
    fn drop(&mut self) {
        self.disable_raw_mode();
    }
}

/// Map a Linux terminal input byte/escape sequence to OS/2 (charcode, scancode).
/// Returns (ascii_char, scan_code) for the KBDKEYINFO struct.
pub fn map_key_to_os2(first: u8, vio: &VioManager) -> (u8, u8) {
    match first {
        // Regular ASCII characters
        0x01..=0x1A if first != 0x1B && first != 0x0D && first != 0x09 && first != 0x08 => {
            // Ctrl+A through Ctrl+Z (except ESC, CR, TAB, BS)
            (first, first + 0x1D) // approximate scan codes
        }
        0x0D => (0x0D, 0x1C), // Enter
        0x09 => (0x09, 0x0F), // Tab
        0x08 | 0x7F => (0x08, 0x0E), // Backspace
        0x1B => {
            // Escape sequence — try to read more
            if let Some(b'[') = vio.read_byte() {
                match vio.read_byte() {
                    Some(b'A') => (0x00, 0x48), // Up
                    Some(b'B') => (0x00, 0x50), // Down
                    Some(b'C') => (0x00, 0x4D), // Right
                    Some(b'D') => (0x00, 0x4B), // Left
                    Some(b'H') => (0x00, 0x47), // Home
                    Some(b'F') => (0x00, 0x4F), // End
                    Some(b'1') => {
                        let _ = vio.read_byte(); // consume '~'
                        (0x00, 0x47) // Home
                    }
                    Some(b'3') => {
                        let _ = vio.read_byte(); // consume '~'
                        (0xE0, 0x53) // Delete
                    }
                    Some(b'4') => {
                        let _ = vio.read_byte(); // consume '~'
                        (0x00, 0x4F) // End
                    }
                    Some(b'5') => {
                        let _ = vio.read_byte(); // consume '~'
                        (0x00, 0x49) // PgUp
                    }
                    Some(b'6') => {
                        let _ = vio.read_byte(); // consume '~'
                        (0x00, 0x51) // PgDn
                    }
                    _ => (0x1B, 0x01), // Unknown escape — return ESC
                }
            } else {
                (0x1B, 0x01) // Plain ESC
            }
        }
        0x20..=0x7E => {
            // Printable ASCII — map to approximate scan codes
            let scan = match first {
                b' ' => 0x39,
                b'0'..=b'9' => first - b'0' + 0x0B, // approximate
                b'a'..=b'z' => {
                    const MAP: [u8; 26] = [
                        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24,
                        0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14,
                        0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
                    ];
                    MAP[(first - b'a') as usize]
                }
                b'A'..=b'Z' => {
                    const MAP: [u8; 26] = [
                        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24,
                        0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14,
                        0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
                    ];
                    MAP[(first - b'A') as usize]
                }
                _ => 0,
            };
            (first, scan)
        }
        _ => (first, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_vio_byte_ascii() {
        assert_eq!(VioManager::decode_vio_byte(b'A', 437), 'A');
        assert_eq!(VioManager::decode_vio_byte(b' ', 850), ' ');
        assert_eq!(VioManager::decode_vio_byte(b'\n', 437), '\n');
    }

    #[test]
    fn test_decode_vio_byte_cp437_high() {
        // CP437 0x9C = '£' (pound sign)
        assert_eq!(VioManager::decode_vio_byte(0x9C, 437), '£');
    }

    #[test]
    fn test_decode_vio_byte_cp850_high() {
        // CP850 0xD0 = 'ð' (eth) — differs from CP437 0xD0 = '╨'
        let ch = VioManager::decode_vio_byte(0xD0, 850);
        assert_eq!(ch, 'ð', "CP850 0xD0 should decode to eth (ð), not CP437 box-drawing");
    }

    #[test]
    fn test_vio_manager_defaults() {
        let mgr = VioManager::new();
        assert!(mgr.rows >= 24);
        assert!(mgr.cols >= 80);
        assert_eq!(mgr.cursor_row, 0);
        assert_eq!(mgr.cursor_col, 0);
        assert!(mgr.cursor_visible);
        assert!(mgr.ansi_mode);
        assert_eq!(mgr.codepage, 437);
        assert_eq!(mgr.buffer.len(), mgr.rows as usize * mgr.cols as usize);
        assert_eq!(mgr.raw_bytes.len(), mgr.rows as usize * mgr.cols as usize);
    }

    #[test]
    fn test_screen_buffer_initial_content() {
        let mgr = VioManager::new();
        // All cells should be space with attribute 0x07
        for &(ch, attr) in &mgr.buffer {
            assert_eq!(ch, ' ');
            assert_eq!(attr, 0x07);
        }
    }

    #[test]
    fn test_scroll_up_buffer() {
        let mut mgr = VioManager::new();
        // Manually set row 1 content
        let cols = mgr.cols as usize;
        mgr.buffer[cols] = ('A', 0x07); // row 1, col 0

        mgr.scroll_up(0, mgr.rows - 1, 1, (' ', 0x07), b' ');

        // Row 0 should now have what was in row 1
        assert_eq!(mgr.buffer[0], ('A', 0x07));
        // Last row should be blank
        let last_row_start = (mgr.rows - 1) as usize * cols;
        assert_eq!(mgr.buffer[last_row_start], (' ', 0x07));
    }

    #[test]
    fn test_scroll_up_lines_zero_clears_region() {
        let mut mgr = VioManager::new();
        let cols = mgr.cols as usize;
        // Seed row 0 with non-blank content
        for cell in &mut mgr.buffer[..cols] { *cell = ('X', 0x07); }
        // VioScrollUp with lines=0 must clear the whole region
        mgr.scroll_up(0, 0, 0, (' ', 0x1F), b' ');
        // All cells in row 0 should be filled with the fill cell
        for (c, cell) in mgr.buffer[..cols].iter().enumerate() {
            assert_eq!(*cell, (' ', 0x1F), "cell {} not cleared", c);
        }
    }

    #[test]
    fn test_scroll_down_buffer() {
        let mut mgr = VioManager::new();
        let cols = mgr.cols as usize;
        mgr.buffer[0] = ('B', 0x07); // row 0, col 0

        mgr.scroll_down(0, mgr.rows - 1, 1, (' ', 0x07), b' ');

        // Row 1 should now have what was in row 0
        assert_eq!(mgr.buffer[cols], ('B', 0x07));
        // Row 0 should be blank
        assert_eq!(mgr.buffer[0], (' ', 0x07));
    }

    #[test]
    fn test_scroll_down_lines_zero_clears_region() {
        let mut mgr = VioManager::new();
        let cols = mgr.cols as usize;
        for cell in &mut mgr.buffer[..cols] { *cell = ('Y', 0x07); }
        mgr.scroll_down(0, 0, 0, ('-', 0x4E), b'-');
        for (c, cell) in mgr.buffer[..cols].iter().enumerate() {
            assert_eq!(*cell, ('-', 0x4E), "cell {} not cleared", c);
        }
    }

    #[test]
    fn test_read_cell_str() {
        let mut mgr = VioManager::new();
        let cols = mgr.cols as usize;
        mgr.buffer[cols + 5] = ('X', 0x1F); // row 1, col 5
        mgr.buffer[cols + 6] = ('Y', 0x2A); // row 1, col 6

        let cells = mgr.read_cell_str(1, 5, 4); // 4 bytes = 2 cells
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0], ('X', 0x1F));
        assert_eq!(cells[1], ('Y', 0x2A));
    }

    #[test]
    fn test_map_key_enter() {
        let mgr = VioManager::new();
        let (ch, scan) = map_key_to_os2(0x0D, &mgr);
        assert_eq!(ch, 0x0D);
        assert_eq!(scan, 0x1C);
    }

    #[test]
    fn test_map_key_printable() {
        let mgr = VioManager::new();
        let (ch, scan) = map_key_to_os2(b'a', &mgr);
        assert_eq!(ch, b'a');
        assert_eq!(scan, 0x1E); // 'a' scancode
    }

    #[test]
    fn test_map_key_backspace() {
        let mgr = VioManager::new();
        let (ch, scan) = map_key_to_os2(0x7F, &mgr);
        assert_eq!(ch, 0x08);
        assert_eq!(scan, 0x0E);
    }

    #[test]
    fn test_resize_expands_buffer() {
        let mut mgr = VioManager::new();
        mgr.resize(50, 132);
        assert_eq!(mgr.rows, 50);
        assert_eq!(mgr.cols, 132);
        assert_eq!(mgr.buffer.len(), 50 * 132);
        // New cells default to space with attribute 0x07
        assert_eq!(mgr.buffer[0], (' ', 0x07));
    }

    #[test]
    fn test_resize_shrinks_and_preserves_content() {
        let mut mgr = VioManager::new();
        // Seed row 0, col 0 with 'X', attr 0x4F
        mgr.buffer[0] = ('X', 0x4F);
        mgr.resize(10, 40);
        assert_eq!(mgr.rows, 10);
        assert_eq!(mgr.cols, 40);
        assert_eq!(mgr.buffer[0], ('X', 0x4F), "top-left cell preserved");
    }

    #[test]
    fn test_resize_clamps_cursor() {
        let mut mgr = VioManager::new();
        mgr.cursor_row = 24;
        mgr.cursor_col = 79;
        mgr.resize(5, 10);
        assert!(mgr.cursor_row < 5,  "cursor_row must be within new bounds");
        assert!(mgr.cursor_col < 10, "cursor_col must be within new bounds");
    }

    #[test]
    fn test_resize_noop_same_dimensions() {
        let mut mgr = VioManager::new();
        let orig_rows = mgr.rows;
        let orig_cols = mgr.cols;
        let orig_len  = mgr.buffer.len();
        mgr.resize(orig_rows, orig_cols);
        assert_eq!(mgr.buffer.len(), orig_len, "noop resize must not reallocate");
    }

    // ── ANSI escape sequences in TTY output ─────────────────────────────────

    /// An 80×25 screen whose TTY output does not echo to the test's stdout.
    fn screen() -> VioManager {
        let mut m = VioManager::new();
        m.enable_sdl_mode();
        m
    }

    fn row_text(m: &VioManager, row: usize, n: usize) -> String {
        let cols = m.cols as usize;
        m.buffer[row * cols..row * cols + n].iter().map(|c| c.0).collect()
    }

    #[test]
    fn test_tty_clear_screen_uses_the_ansi_attribute_and_homes() {
        let mut m = screen();
        m.write_tty(b"abc\r\ndef\x1b[44m\x1b[2J", 437);
        assert_eq!((m.cursor_row, m.cursor_col), (0, 0));
        assert!(m.buffer.iter().all(|&c| c == (' ', 0x17)), "blank in light grey on blue");
        // ESC[J and ESC[1J are not documented for OS/2: ignored.
        m.write_tty(b"x\x1b[J\x1b[1J", 437);
        assert_eq!(m.buffer[0], ('x', 0x17));
    }

    #[test]
    fn test_tty_cursor_moves_are_clamped() {
        let mut m = screen();
        m.write_tty(b"\x1b[30;100H", 437);
        assert_eq!((m.cursor_row, m.cursor_col), (24, 79));
        m.write_tty(b"\x1b[0;0f", 437);
        assert_eq!((m.cursor_row, m.cursor_col), (0, 0));
        m.write_tty(b"\x1b[5B\x1b[7C\x1b[2A\x1b[3D", 437);
        assert_eq!((m.cursor_row, m.cursor_col), (3, 4));
        m.write_tty(b"\x1b[9999A\x1b[9999D", 437);
        assert_eq!((m.cursor_row, m.cursor_col), (0, 0));
    }

    #[test]
    fn test_tty_save_and_restore_cursor() {
        let mut m = screen();
        m.write_tty(b"\x1b[u", 437); // nothing saved: stays
        assert_eq!((m.cursor_row, m.cursor_col), (0, 0));
        m.write_tty(b"\x1b[4;6H\x1b[s\x1b[H\x1b[u", 437);
        assert_eq!((m.cursor_row, m.cursor_col), (3, 5));
    }

    #[test]
    fn test_tty_erase_line_from_the_cursor() {
        let mut m = screen();
        m.write_tty(b"ABCDEF\r\nGHI\x1b[1;3H\x1b[K", 437);
        assert_eq!(row_text(&m, 0, 6), "AB    ");
        assert_eq!(row_text(&m, 1, 3), "GHI", "the next row is untouched");
        assert_eq!((m.cursor_row, m.cursor_col), (0, 2));
    }

    #[test]
    fn test_tty_sgr_colours_the_following_text() {
        let mut m = screen();
        m.write_tty(b"\x1b[1;33;41mY\x1b[0mn", 437);
        assert_eq!(m.buffer[0], ('Y', 0x4E));
        assert_eq!(m.buffer[1], ('n', 0x07));
    }

    #[test]
    fn test_tty_sequence_split_across_writes() {
        let mut m = screen();
        m.write_tty(b"ab\x1b", 437);
        m.write_tty(b"[3", 437);
        m.write_tty(b";4Hc", 437);
        assert_eq!(row_text(&m, 0, 3), "ab ", "no part of the sequence is shown");
        assert_eq!(m.buffer[2 * 80 + 3].0, 'c');
    }

    #[test]
    fn test_tty_ansi_off_shows_the_sequence() {
        let mut m = screen();
        m.write_tty(b"\x1b", 437); // an unfinished sequence…
        m.set_ansi(false);          // …is dropped
        m.write_tty(b"\x1b[2J", 437);
        assert_eq!(row_text(&m, 0, 4), "\u{1b}[2J");
        m.set_ansi(true);
        m.write_tty(b"\x1bQ", 437); // a lone ESC is a character
        assert_eq!(row_text(&m, 0, 6), "\u{1b}[2J\u{1b}Q");
    }

    #[test]
    fn test_tty_wrap_mode_and_scrolling() {
        let mut m = screen();
        m.write_tty(b"\x1b[=7l\x1b[1;79HABC", 437);
        assert_eq!(row_text(&m, 0, 80).trim_end(), format!("{}AC", " ".repeat(78)));
        assert_eq!((m.cursor_row, m.cursor_col), (0, 79));
        m.write_tty(b"\x1b[=7h\x1b[25;80HXY", 437);
        // Wrapping past the bottom row scrolls the screen up one row.
        assert_eq!(m.buffer[23 * 80 + 79].0, 'X');
        assert_eq!(m.buffer[24 * 80].0, 'Y');
        assert_eq!((m.cursor_row, m.cursor_col), (24, 1));
    }

    #[test]
    fn test_tty_display_modes_set_the_text_size() {
        let mut m = screen();
        m.write_tty(b"\x1b[5;5Hold\x1b[=1h", 437);
        assert_eq!((m.rows, m.cols), (25, 40), "mode 1: 40 x 25");
        assert_eq!((m.cursor_row, m.cursor_col), (0, 0), "cleared, home");
        assert!(m.buffer.iter().all(|c| c.0 == ' '));
        m.write_tty(b"\x1b[=3hX", 437);
        assert_eq!((m.rows, m.cols), (25, 80), "mode 3: 80 x 25");
        assert_eq!(m.buffer[0].0, 'X');
        m.write_tty(b"\x1b[=6h", 437);
        assert_eq!((m.rows, m.cols, m.buffer[0].0), (25, 80, 'X'), "a graphics mode changes nothing");
    }

    #[test]
    fn test_tty_tab_and_backspace() {
        let mut m = screen();
        m.write_tty(b"ab\tc\x08\x08d", 437);
        assert_eq!(row_text(&m, 0, 9), "ab     dc");
        m.write_tty(b"\x1b[1;80H\t", 437);
        assert_eq!((m.cursor_row, m.cursor_col), (1, 0), "a tab in the last column wraps");
    }

    #[test]
    fn test_tty_attr_override_keeps_the_ansi_rendition() {
        let mut m = screen();
        m.write_tty(b"\x1b[32m", 437);
        m.write_tty_attr(b"E", 0x0C, 437);
        m.write_tty(b"g", 437);
        assert_eq!(m.buffer[0], ('E', 0x0C));
        assert_eq!(m.buffer[1], ('g', 0x02));
    }

    #[test]
    fn test_clear_screen_homes_the_cursor() {
        let mut m = screen();
        m.write_tty(b"hello\r\nworld", 437);
        m.clear_screen(0x1F);
        assert_eq!((m.cursor_row, m.cursor_col), (0, 0));
        assert!(m.buffer.iter().all(|&c| c == (' ', 0x1F)));
    }

    #[test]
    fn test_track_tty_updates_the_buffer_silently() {
        let mut m = VioManager::new();
        m.resize(25, 80);
        // Not in SDL mode: write_tty would echo; track_tty must not (the
        // test would otherwise print to stdout) but still move the cursor.
        m.track_tty(b"\x1b[3;3Hok", 437);
        assert_eq!(m.buffer[2 * 80 + 2].0, 'o');
        assert_eq!((m.cursor_row, m.cursor_col), (2, 4));
    }

    #[test]
    fn test_terminal_echo_of_tty_output() {
        let mut out = TtyOut::new(false);
        out.text('a', 0x07);
        assert_eq!(out.collected(), b"a", "the default attribute uses the terminal's colours");
        out.text('b', 0x1F);
        out.text('c', 0x1F);
        out.newline();
        out.clear_screen(0x07);
        out.cursor(4, 9);
        assert_eq!(out.collected(), b"a\x1b[37;44;1mbc\x1b[0m\n\x1b[2J\x1b[1;1H\x1b[5;10H");
        let silent = TtyOut::new(true);
        assert!(silent.collected().is_empty());
    }

    #[test]
    fn test_classify_read_tells_end_of_input_from_no_key() {
        assert_eq!(classify_read(1, b'x', false), HostInput::Byte(b'x'));
        assert_eq!(classify_read(0, 0, false), HostInput::EndOfInput);
        assert_eq!(classify_read(0, 0, true), HostInput::Empty, "a raw terminal with no key pressed");
        assert_eq!(classify_read(-1, 0, false), HostInput::Empty, "an error such as EAGAIN is retried");
    }
}
