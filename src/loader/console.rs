// SPDX-License-Identifier: GPL-3.0-only
//
// Console manager for OS/2 VIO (Video I/O) subsystem.
// Maintains a screen buffer and cursor state, outputs via ANSI escape sequences.

use std::io::{self, Write};

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
    /// Screen buffer: (character, attribute) pairs, row-major.
    pub buffer: Vec<(u8, u8)>,
    pub rows: u16,
    pub cols: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    /// Cursor shape: first scan line (0 = top of cell).
    pub cursor_start: u8,
    /// Cursor shape: last scan line (15 = bottom of cell for 8×16 font).
    pub cursor_end: u8,
    pub ansi_mode: bool,
    pub codepage: u16,
    /// When `true`, all ANSI escape output is suppressed; the SDL2 text
    /// renderer reads the buffer directly.  Also forces rows/cols to 80×25
    /// and prevents `enable_raw_mode` from touching termios.
    pub sdl2_mode: bool,
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
            buffer: vec![(b' ', 0x07); size], // space with light gray on black
            rows,
            cols,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            cursor_start: 14,
            cursor_end: 15,
            ansi_mode: true,
            codepage: 437,
            sdl2_mode: false,
            raw_mode_active: false,
            original_termios: None,
            stdin_pending_lf: false,
            stdin_cooked_chars: 0,
        }
    }

    /// Switch to SDL2 text-mode: initialise to 80×25, suppress ANSI output.
    pub fn enable_sdl2_mode(&mut self) {
        self.sdl2_mode = true;
        // Start at the standard 80×25 text mode; VioSetMode may later change
        // the dimensions and the SDL2 window will resize to match.
        self.resize(25, 80);
    }

    /// Resize the virtual screen to `new_rows` × `new_cols`.
    ///
    /// Preserves as much of the existing buffer content as possible (top-left
    /// content is kept; new cells are initialised to space/attribute 0x07).
    /// Called by `VioSetMode` and `enable_sdl2_mode`.
    pub fn resize(&mut self, new_rows: u16, new_cols: u16) {
        if self.rows == new_rows && self.cols == new_cols { return; }
        let new_size = new_rows as usize * new_cols as usize;
        let mut new_buf: Vec<(u8, u8)> = vec![(b' ', 0x07); new_size];
        // Copy the top-left region that fits in both old and new dimensions.
        let copy_rows = self.rows.min(new_rows) as usize;
        let copy_cols = self.cols.min(new_cols) as usize;
        for r in 0..copy_rows {
            for c in 0..copy_cols {
                let src = r * self.cols as usize + c;
                let dst = r * new_cols as usize + c;
                new_buf[dst] = self.buffer[src];
            }
        }
        self.rows = new_rows;
        self.cols = new_cols;
        self.buffer = new_buf;
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
    /// No-op in SDL2 text mode (keyboard events come from SDL2 instead).
    pub fn enable_raw_mode(&mut self) {
        if self.raw_mode_active || self.sdl2_mode { return; }
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

    /// Write a CP437 byte to stdout as UTF-8.
    fn write_cp437_char(stdout: &mut io::Stdout, ch: u8) {
        let mut buf = [0u8; 4];
        let s = cp437_to_char(ch).encode_utf8(&mut buf);
        let _ = stdout.write_all(s.as_bytes());
    }

    /// Write a string at the current cursor position, advancing the cursor.
    ///
    /// Updates the screen buffer.  In terminal (non-SDL2) mode, also emits
    /// ANSI colour escapes so coloured text from `VioWrtTTY` is rendered with
    /// the correct CGA attribute.
    pub fn write_tty(&mut self, text: &[u8], attr: u8) {
        let sdl2 = self.sdl2_mode;
        let mut stdout = io::stdout();
        // Lazily emit the ANSI colour attribute on the first printable byte so
        // that pure-control strings (CR/LF/BS sequences) don't pollute the
        // terminal colour state unnecessarily.
        let mut attr_applied = false;
        for &ch in text {
            if ch == b'\n' {
                self.cursor_row += 1;
                self.cursor_col = 0;
                if self.cursor_row >= self.rows {
                    self.scroll_up(0, self.rows - 1, 1, (b' ', 0x07));
                    self.cursor_row = self.rows - 1;
                }
                if !sdl2 {
                    if attr_applied {
                        let _ = stdout.write_all(b"\x1b[0m");
                        attr_applied = false;
                    }
                    let _ = stdout.write_all(b"\n");
                }
            } else if ch == b'\r' {
                self.cursor_col = 0;
                if !sdl2 { let _ = stdout.write_all(b"\r"); }
            } else if ch == b'\x08' {
                // Backspace
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                }
                if !sdl2 { let _ = stdout.write_all(b"\x08"); }
            } else if ch == b'\x07' {
                // Bell
                if !sdl2 { let _ = stdout.write_all(b"\x07"); }
            } else {
                if self.cursor_row < self.rows && self.cursor_col < self.cols {
                    let idx = self.cursor_row as usize * self.cols as usize + self.cursor_col as usize;
                    if idx < self.buffer.len() {
                        self.buffer[idx] = (ch, attr);
                    }
                }
                self.cursor_col += 1;
                if self.cursor_col >= self.cols {
                    self.cursor_col = 0;
                    self.cursor_row += 1;
                    if self.cursor_row >= self.rows {
                        self.scroll_up(0, self.rows - 1, 1, (b' ', 0x07));
                        self.cursor_row = self.rows - 1;
                    }
                }
                if !sdl2 {
                    if !attr_applied {
                        self.write_ansi_attr(&mut stdout, attr);
                        attr_applied = true;
                    }
                    Self::write_cp437_char(&mut stdout, ch);
                }
            }
        }
        if !sdl2 {
            if attr_applied { let _ = stdout.write_all(b"\x1b[0m"); }
            let _ = stdout.flush();
        }
    }

    /// Write an attributed string at a specific position.
    pub fn write_char_str_att(&mut self, row: u16, col: u16, text: &[u8], attr: u8) {
        let sdl2 = self.sdl2_mode;
        let mut stdout = io::stdout();
        if !sdl2 {
            let _ = write!(stdout, "\x1b[{};{}H", row + 1, col + 1);
            self.write_ansi_attr(&mut stdout, attr);
        }
        let mut c = col;
        for &ch in text {
            if c >= self.cols { break; }
            let idx = row as usize * self.cols as usize + c as usize;
            if idx < self.buffer.len() {
                self.buffer[idx] = (ch, attr);
            }
            if !sdl2 { Self::write_cp437_char(&mut stdout, ch); }
            c += 1;
        }
        if !sdl2 {
            let _ = stdout.write_all(b"\x1b[0m");
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Fill N cells starting at (row, col) with a character+attribute pair.
    pub fn write_n_cell(&mut self, row: u16, col: u16, cell: (u8, u8), count: u16) {
        let sdl2 = self.sdl2_mode;
        let mut stdout = io::stdout();
        if !sdl2 {
            let _ = write!(stdout, "\x1b[{};{}H", row + 1, col + 1);
            self.write_ansi_attr(&mut stdout, cell.1);
        }
        let mut c = col;
        for _ in 0..count {
            if c >= self.cols { break; }
            let idx = row as usize * self.cols as usize + c as usize;
            if idx < self.buffer.len() {
                self.buffer[idx] = cell;
            }
            if !sdl2 { Self::write_cp437_char(&mut stdout, cell.0); }
            c += 1;
        }
        if !sdl2 {
            let _ = stdout.write_all(b"\x1b[0m");
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Fill N attribute bytes starting at (row, col), preserving characters.
    pub fn write_n_attr(&mut self, row: u16, col: u16, attr: u8, count: u16) {
        let sdl2 = self.sdl2_mode;
        let mut stdout = io::stdout();
        if !sdl2 {
            let _ = write!(stdout, "\x1b[{};{}H", row + 1, col + 1);
            self.write_ansi_attr(&mut stdout, attr);
        }
        let mut c = col;
        for _ in 0..count {
            if c >= self.cols { break; }
            let idx = row as usize * self.cols as usize + c as usize;
            if idx < self.buffer.len() {
                let ch = self.buffer[idx].0;
                self.buffer[idx].1 = attr;
                if !sdl2 { Self::write_cp437_char(&mut stdout, ch); }
            }
            c += 1;
        }
        if !sdl2 {
            let _ = stdout.write_all(b"\x1b[0m");
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Read cell string from the screen buffer.
    pub fn read_cell_str(&self, row: u16, col: u16, max_len: u16) -> Vec<(u8, u8)> {
        let mut result = Vec::new();
        let mut c = col;
        for _ in 0..max_len / 2 {
            if c >= self.cols { break; }
            let idx = row as usize * self.cols as usize + c as usize;
            if idx < self.buffer.len() {
                result.push(self.buffer[idx]);
            }
            c += 1;
        }
        result
    }

    /// Scroll a region up by `lines` rows, filling the bottom with `fill_cell`.
    ///
    /// OS/2 special case: `lines == 0` means "clear the entire region" (fill all
    /// rows from `top` to `bottom` with `fill_cell` without scrolling).
    pub fn scroll_up(&mut self, top: u16, bottom: u16, lines: u16, fill_cell: (u8, u8)) {
        if top > bottom || bottom >= self.rows { return; }
        if lines == 0 {
            // Clear entire region
            let cols = self.cols as usize;
            for row in top..=bottom {
                let base = row as usize * cols;
                for c in 0..cols {
                    if base + c < self.buffer.len() {
                        self.buffer[base + c] = fill_cell;
                    }
                }
            }
            if !self.sdl2_mode {
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
        let cols = self.cols as usize;
        let lines = lines.min(bottom - top + 1);

        // Shift buffer rows up
        for row in top..(bottom - lines + 1) {
            let dst = row as usize * cols;
            let src = (row + lines) as usize * cols;
            for c in 0..cols {
                if src + c < self.buffer.len() && dst + c < self.buffer.len() {
                    self.buffer[dst + c] = self.buffer[src + c];
                }
            }
        }
        // Fill vacated rows
        for row in (bottom - lines + 1)..=bottom {
            let base = row as usize * cols;
            for c in 0..cols {
                if base + c < self.buffer.len() {
                    self.buffer[base + c] = fill_cell;
                }
            }
        }

        // Output ANSI scroll (terminal mode only)
        if !self.sdl2_mode {
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
    pub fn scroll_down(&mut self, top: u16, bottom: u16, lines: u16, fill_cell: (u8, u8)) {
        if top > bottom || bottom >= self.rows { return; }
        if lines == 0 {
            let cols = self.cols as usize;
            for row in top..=bottom {
                let base = row as usize * cols;
                for c in 0..cols {
                    if base + c < self.buffer.len() {
                        self.buffer[base + c] = fill_cell;
                    }
                }
            }
            if !self.sdl2_mode {
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
                }
            }
        }
        // Fill vacated rows
        for row in top..(top + lines) {
            let base = row as usize * cols;
            for c in 0..cols {
                if base + c < self.buffer.len() {
                    self.buffer[base + c] = fill_cell;
                }
            }
        }

        if !self.sdl2_mode {
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
        if !self.sdl2_mode {
            let mut stdout = io::stdout();
            let _ = write!(stdout, "\x1b[{};{}H", self.cursor_row + 1, self.cursor_col + 1);
            let _ = stdout.flush();
        }
    }

    /// Set cursor visibility (in terminal mode, also emits ANSI escape).
    pub fn set_cursor_type(&mut self, visible: bool) {
        self.cursor_visible = visible;
        if !self.sdl2_mode {
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
        let fg_idx = (attr & 0x07) as usize;
        let fg_bright = (attr & 0x08) != 0;
        let bg_idx = ((attr >> 4) & 0x07) as usize;

        let fg = CGA_TO_ANSI_FG[fg_idx];
        let bg = CGA_TO_ANSI_BG[bg_idx];

        if fg_bright {
            let _ = write!(stdout, "\x1b[{};{};1m", fg, bg);
        } else {
            let _ = write!(stdout, "\x1b[{};{}m", fg, bg);
        }
    }

    /// Read a single byte from stdin (non-blocking with short timeout).
    /// Returns None if no input available within timeout.
    pub fn read_byte(&self) -> Option<u8> {
        let mut buf = [0u8; 1];
        let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1) };
        if n == 1 { Some(buf[0]) } else { None }
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
    }

    #[test]
    fn test_screen_buffer_initial_content() {
        let mgr = VioManager::new();
        // All cells should be space with attribute 0x07
        for &(ch, attr) in &mgr.buffer {
            assert_eq!(ch, b' ');
            assert_eq!(attr, 0x07);
        }
    }

    #[test]
    fn test_scroll_up_buffer() {
        let mut mgr = VioManager::new();
        // Manually set row 1 content
        let cols = mgr.cols as usize;
        mgr.buffer[cols] = (b'A', 0x07); // row 1, col 0

        mgr.scroll_up(0, mgr.rows - 1, 1, (b' ', 0x07));

        // Row 0 should now have what was in row 1
        assert_eq!(mgr.buffer[0], (b'A', 0x07));
        // Last row should be blank
        let last_row_start = (mgr.rows - 1) as usize * cols;
        assert_eq!(mgr.buffer[last_row_start], (b' ', 0x07));
    }

    #[test]
    fn test_scroll_up_lines_zero_clears_region() {
        let mut mgr = VioManager::new();
        let cols = mgr.cols as usize;
        // Seed row 0 with non-blank content
        for c in 0..cols { mgr.buffer[c] = (b'X', 0x07); }
        // VioScrollUp with lines=0 must clear the whole region
        mgr.scroll_up(0, 0, 0, (b' ', 0x1F));
        // All cells in row 0 should be filled with the fill cell
        for c in 0..cols {
            assert_eq!(mgr.buffer[c], (b' ', 0x1F), "cell {} not cleared", c);
        }
    }

    #[test]
    fn test_scroll_down_buffer() {
        let mut mgr = VioManager::new();
        let cols = mgr.cols as usize;
        mgr.buffer[0] = (b'B', 0x07); // row 0, col 0

        mgr.scroll_down(0, mgr.rows - 1, 1, (b' ', 0x07));

        // Row 1 should now have what was in row 0
        assert_eq!(mgr.buffer[cols], (b'B', 0x07));
        // Row 0 should be blank
        assert_eq!(mgr.buffer[0], (b' ', 0x07));
    }

    #[test]
    fn test_scroll_down_lines_zero_clears_region() {
        let mut mgr = VioManager::new();
        let cols = mgr.cols as usize;
        for c in 0..cols { mgr.buffer[c] = (b'Y', 0x07); }
        mgr.scroll_down(0, 0, 0, (b'-', 0x4E));
        for c in 0..cols {
            assert_eq!(mgr.buffer[c], (b'-', 0x4E), "cell {} not cleared", c);
        }
    }

    #[test]
    fn test_read_cell_str() {
        let mut mgr = VioManager::new();
        let cols = mgr.cols as usize;
        mgr.buffer[cols + 5] = (b'X', 0x1F); // row 1, col 5
        mgr.buffer[cols + 6] = (b'Y', 0x2A); // row 1, col 6

        let cells = mgr.read_cell_str(1, 5, 4); // 4 bytes = 2 cells
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0], (b'X', 0x1F));
        assert_eq!(cells[1], (b'Y', 0x2A));
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
        assert_eq!(mgr.buffer[0], (b' ', 0x07));
    }

    #[test]
    fn test_resize_shrinks_and_preserves_content() {
        let mut mgr = VioManager::new();
        // Seed row 0, col 0 with 'X', attr 0x4F
        mgr.buffer[0] = (b'X', 0x4F);
        mgr.resize(10, 40);
        assert_eq!(mgr.rows, 10);
        assert_eq!(mgr.cols, 40);
        assert_eq!(mgr.buffer[0], (b'X', 0x4F), "top-left cell preserved");
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
}
