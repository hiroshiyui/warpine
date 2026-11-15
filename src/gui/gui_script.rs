// SPDX-License-Identifier: GPL-3.0-only

//! Scripted GUI sessions for automated testing.
//!
//! When `WARPINE_GUI_SCRIPT` names a script file, the PM renderer replays it
//! against the virtual desktop: it injects mouse clicks, writes the composited
//! desktop to image files, and can end the session. Combined with
//! `SDL_VIDEODRIVER=dummy` this drives a PM application with no display, which
//! is how the GUI regression tests in `tests/integration.rs` work.
//!
//! # Script format
//!
//! One step per line, `<milliseconds since start> <action> [arguments]`.
//! Blank lines and lines starting with `#` are ignored.
//!
//! ```text
//! # open the File menu, pick the first item, capture, exit
//! 800  click 52 62
//! 1600 click 62 80
//! 2600 dump  /tmp/about.ppm
//! 2700 quit
//! ```
//!
//! * `click X Y` — left button press and release at desktop pixel `(X, Y)`
//!   (origin top-left).
//! * `dump PATH` — write the desktop as a binary PPM (P6) image.
//! * `quit` — end the session as if the desktop window had been closed.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Environment variable naming the script file.
pub const SCRIPT_ENV: &str = "WARPINE_GUI_SCRIPT";

/// One scripted action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Click { x: i32, y: i32 },
    Dump(PathBuf),
    Quit,
}

/// An [`Action`] and the time, in milliseconds since the session started, at
/// which it becomes due.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub at_ms: u64,
    pub action: Action,
}

/// Parse a script. Steps are returned ordered by time; steps with equal times
/// keep their order in the file. Errors name the offending line.
pub fn parse(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let err = |what: &str| format!("line {}: {what}: `{line}`", idx + 1);
        let mut words = line.split_whitespace();
        let at_ms = words.next().and_then(|w| w.parse::<u64>().ok())
            .ok_or_else(|| err("expected a time in milliseconds"))?;
        let action = match words.next() {
            Some("click") => {
                let mut coord = || words.next().and_then(|w| w.parse::<i32>().ok());
                match (coord(), coord()) {
                    (Some(x), Some(y)) => Action::Click { x, y },
                    _ => return Err(err("click needs X and Y")),
                }
            }
            Some("dump") => {
                // The path is the rest of the line, so it may contain spaces.
                let path = line.splitn(3, char::is_whitespace).nth(2).map(str::trim).unwrap_or("");
                if path.is_empty() {
                    return Err(err("dump needs a file path"));
                }
                steps.push(Step { at_ms, action: Action::Dump(PathBuf::from(path)) });
                continue;
            }
            Some("quit") => Action::Quit,
            _ => return Err(err("unknown action (expected click, dump or quit)")),
        };
        if words.next().is_some() {
            return Err(err("unexpected extra arguments"));
        }
        steps.push(Step { at_ms, action });
    }
    steps.sort_by_key(|s| s.at_ms);
    Ok(steps)
}

/// A script being replayed.
pub struct GuiScript {
    pending: VecDeque<Step>,
    /// Set on the first poll, so start-up time before the event loop runs
    /// does not eat into the script's timeline.
    started: Option<Instant>,
}

impl GuiScript {
    pub fn new(steps: Vec<Step>) -> Self {
        GuiScript { pending: steps.into(), started: None }
    }

    /// Load the script named by [`SCRIPT_ENV`], if set. A script that cannot
    /// be read or parsed is a fatal configuration error for a test run, so it
    /// is reported and the process exits instead of silently running without it.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os(SCRIPT_ENV)?;
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| parse(&text));
        match loaded {
            Ok(steps) => Some(GuiScript::new(steps)),
            Err(e) => {
                eprintln!("warpine: {SCRIPT_ENV}={}: {e}", Path::new(&path).display());
                std::process::exit(2);
            }
        }
    }

    /// Actions that have become due, in order.
    pub fn due(&mut self) -> Vec<Action> {
        let elapsed = self.started.get_or_insert_with(Instant::now).elapsed().as_millis() as u64;
        self.due_at(elapsed)
    }

    fn due_at(&mut self, elapsed_ms: u64) -> Vec<Action> {
        let mut out = Vec::new();
        while self.pending.front().is_some_and(|s| s.at_ms <= elapsed_ms) {
            out.extend(self.pending.pop_front().map(|s| s.action));
        }
        out
    }
}

/// Encode `width` × `height` 0x00RRGGBB pixels as a binary PPM (P6) image.
pub fn encode_ppm(pixels: &[u32], width: u32, height: u32) -> Vec<u8> {
    let count = (width as usize * height as usize).min(pixels.len());
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    out.reserve(count * 3);
    for &p in &pixels[..count] {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    // Pad a short buffer so the file always matches its header.
    out.resize(out.len() + (width as usize * height as usize - count) * 3, 0);
    out
}

/// Write a desktop buffer to `path` as a PPM image.
pub fn write_ppm(path: &Path, pixels: &[u32], width: u32, height: u32) -> std::io::Result<()> {
    std::fs::File::create(path)?.write_all(&encode_ppm(pixels, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_steps_and_orders_them_by_time() {
        let steps = parse("\
            # comment\n\
            \n\
            2600 dump  /tmp/my shots/about.ppm \n\
            800  click 52 62\n\
            2700 quit\n\
            800  click 1 2\n").unwrap();
        assert_eq!(steps, vec![
            Step { at_ms: 800, action: Action::Click { x: 52, y: 62 } },
            Step { at_ms: 800, action: Action::Click { x: 1, y: 2 } },
            Step { at_ms: 2600, action: Action::Dump(PathBuf::from("/tmp/my shots/about.ppm")) },
            Step { at_ms: 2700, action: Action::Quit },
        ]);
        assert_eq!(parse("").unwrap(), vec![]);
    }

    #[test]
    fn parse_rejects_malformed_lines_and_names_the_line() {
        for bad in ["soon click 1 2", "10 click 1", "10 click a b", "10 click 1 2 3",
                    "10 dump", "10 quit now", "10 drag 1 2", "10"] {
            let err = parse(&format!("5 quit\n{bad}\n")).unwrap_err();
            assert!(err.starts_with("line 2:"), "{bad:?} → {err}");
        }
    }

    #[test]
    fn due_releases_each_step_once_when_its_time_has_come() {
        let mut script = GuiScript::new(parse("100 click 1 1\n200 quit\n200 click 2 2\n").unwrap());
        assert!(script.due_at(99).is_empty());
        assert_eq!(script.due_at(100), vec![Action::Click { x: 1, y: 1 }]);
        assert!(script.due_at(150).is_empty());
        assert_eq!(script.due_at(5000), vec![Action::Quit, Action::Click { x: 2, y: 2 }]);
        assert!(script.due_at(9000).is_empty());
    }

    #[test]
    fn encode_ppm_writes_header_and_rgb_triples() {
        let ppm = encode_ppm(&[0x00FF_8040, 0xFF00_0001], 2, 1);
        assert_eq!(ppm, b"P6\n2 1\n255\n\xFF\x80\x40\x00\x00\x01");
        // A buffer shorter than the header claims is padded, never over-read.
        assert_eq!(encode_ppm(&[0x0012_3456], 2, 1).len(), b"P6\n2 1\n255\n".len() + 6);
    }
}
