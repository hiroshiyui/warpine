// SPDX-License-Identifier: GPL-3.0-or-later
//
// The built-in CMD.EXE's line editor: keys from the PM / text window or the
// terminal, backspace, Esc and the history.

use super::*;

impl CmdShell {
    // ── Keyboard input ───────────────────────────────────────────────────────

    /// Block until a key is available. Returns `None` only on shutdown.
    pub(super) fn read_key(&self) -> Option<KbdKeyInfo> {
        if self.shared.use_sdl_text.load(Ordering::Relaxed) {
            let mut queue = self.shared.kbd_queue.lock().unwrap();
            loop {
                if self.shared.exit_requested.load(Ordering::Relaxed) {
                    return None;
                }
                if let Some(ki) = queue.pop_front() {
                    return Some(ki);
                }
                let (q, _) = self.shared.kbd_cond
                    .wait_timeout(queue, Duration::from_millis(50))
                    .unwrap();
                queue = q;
            }
        } else {
            // Termios path: enable raw mode and read a byte.
            {
                let mut vio = self.shared.console_mgr.lock_or_recover();
                vio.enable_raw_mode();
            }
            loop {
                if self.shared.exit_requested.load(Ordering::Relaxed) {
                    return None;
                }
                let byte = {
                    let vio = self.shared.console_mgr.lock_or_recover();
                    vio.read_byte()
                };
                if let Some(b) = byte {
                    // Minimal byte → KbdKeyInfo mapping for line-editor needs.
                    let (ch, scan) = match b {
                        0x0D       => (0x0D, SCAN_ENTER),
                        0x08 | 127 => (0x08, SCAN_BACKSPACE),
                        // ESC or ESC sequences for arrow keys: \x1b[ A/B
                        0x1B => {
                            let b2 = { self.shared.console_mgr.lock_or_recover().read_byte() };
                            if b2 == Some(b'[') {
                                let b3 = { self.shared.console_mgr.lock_or_recover().read_byte() };
                                match b3 {
                                    Some(b'A') => (0, SCAN_UP),
                                    Some(b'B') => (0, SCAN_DOWN),
                                    _ => (0x1B, SCAN_ESCAPE),
                                }
                            } else {
                                (0x1B, SCAN_ESCAPE)
                            }
                        }
                        other => (other, 0),
                    };
                    return Some(KbdKeyInfo { ch, scan, state: 0 });
                }
                // Only a terminal returns "nothing yet"; a pipe or file that
                // returns nothing is at end of input (`echo dir | warpine
                // CMD.EXE`), which ends the shell.
                if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Read a full line with basic line-editing: backspace, history (↑/↓), Esc
    /// to clear, and Enter to submit.
    pub(super) fn read_line(&mut self) -> ReadResult {
        // Check non-kbd input source first.
        self.drain_pipe_input();
        {
            let mut src = self.input_source.borrow_mut();
            if let InputSource::Data(ref mut data) = *src {
                if data.is_empty() { return ReadResult::Eof; }
                let end = data.iter().position(|&b| b == b'\n').map_or(data.len(), |i| i + 1);
                let line: Vec<u8> = data.drain(..end).collect();
                let line = String::from_utf8_lossy(&line);
                return ReadResult::Line(line.trim_end_matches(['\r', '\n']).to_string());
            }
        }

        let mut line = String::new();
        let mut hist_idx = self.history.len(); // past-end = "no history entry"

        loop {
            let Some(ki) = self.read_key() else {
                return ReadResult::Eof;
            };

            // Enter
            if ki.ch == 0x0D || ki.scan == SCAN_ENTER {
                self.write_out(b"\r\n");
                return ReadResult::Line(line);
            }

            // Backspace
            if ki.ch == 0x08 || ki.scan == SCAN_BACKSPACE {
                if !line.is_empty() {
                    line.pop();
                    // Erase last visible char: BS, space, BS
                    self.write_out(b"\x08 \x08");
                }
                continue;
            }

            // Escape — clear current line
            if ki.ch == 0x1B || ki.scan == SCAN_ESCAPE {
                // Erase everything typed so far
                for _ in 0..line.len() {
                    self.write_out(b"\x08 \x08");
                }
                line.clear();
                hist_idx = self.history.len();
                continue;
            }

            // Up arrow — older history
            if ki.scan == SCAN_UP {
                if hist_idx > 0 {
                    hist_idx -= 1;
                    self.replace_input(&mut line, &self.history[hist_idx].clone());
                }
                continue;
            }

            // Down arrow — newer history
            if ki.scan == SCAN_DOWN {
                if hist_idx < self.history.len() {
                    hist_idx += 1;
                    let replacement = if hist_idx < self.history.len() {
                        self.history[hist_idx].clone()
                    } else {
                        String::new()
                    };
                    self.replace_input(&mut line, &replacement);
                }
                continue;
            }

            // Printable ASCII
            if ki.ch >= 0x20 {
                line.push(ki.ch as char);
                self.write_out(&[ki.ch]);
            }
        }
    }

    /// Erase `current` from the display and replace it with `new_text`.
    pub(super) fn replace_input(&self, current: &mut String, new_text: &str) {
        for _ in 0..current.len() {
            self.write_out(b"\x08 \x08");
        }
        *current = new_text.to_string();
        self.write_out(current.as_bytes());
    }

    // ── History ───────────────────────────────────────────────────────────────

    pub(super) fn history_push(&mut self, line: String) {
        // Don't add duplicates of the most recent entry.
        if self.history.last().map(|s| s.as_str()) == Some(line.as_str()) {
            return;
        }
        if self.history.len() >= HISTORY_SIZE {
            self.history.remove(0);
        }
        self.history.push(line);
    }
}
