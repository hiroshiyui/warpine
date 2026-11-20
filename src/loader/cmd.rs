// SPDX-License-Identifier: GPL-3.0-or-later
//
// Builtin CMD.EXE — host-Rust command shell for Warpine.
//
// Invoked by `Loader::run_builtin_cmd()` whenever `DosExecPgm` targets
// "CMD.EXE" or "OS2SHELL.EXE".  The shell runs entirely on the host; it uses
// the existing `VioManager` for output and `kbd_queue` / termios for input so
// it works identically in SDL text-mode and headless terminal sessions.
//
// Supported built-ins: DIR, CD, SET, ECHO, CLS, VER, TYPE, MD/MKDIR,
//   RD/RMDIR, DEL/ERASE, HELP, REM, EXIT.
// Anything else is resolved via DriveManager and executed as a child Warpine
// process (EXEC_SYNC), with stdout relayed through VioManager.
// .CMD scripts are executed line-by-line through the same dispatch path.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Read;
use std::io::Write as IoWrite;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use log::debug;

/// How often the shell looks whether the program it waits for ended.
const CHILD_POLL: Duration = Duration::from_millis(10);

use super::{SharedState, KbdKeyInfo};
use super::mutex_ext::MutexExt;
use super::constants::*;
use super::vfs::FileAttribute;

// ── Constants ────────────────────────────────────────────────────────────────

const HISTORY_SIZE: usize = 20;
/// Default VGA text attribute: light gray on black.
const ATTR_NORMAL: u8 = 0x07;
/// Header / directory attribute: bright cyan on black.
const ATTR_HEADER: u8 = 0x0B;
/// Error messages: bright red on black.
const ATTR_ERROR: u8 = 0x0C;

// Scancodes (set 1 make codes).
const SCAN_BACKSPACE: u8 = 0x0E;
const SCAN_ENTER: u8 = 0x1C;
const SCAN_ESCAPE: u8 = 0x01;
const SCAN_UP: u8 = 0x48;
const SCAN_DOWN: u8 = 0x50;

// ── ReadResult ────────────────────────────────────────────────────────────────

enum ReadResult {
    Line(String),
    Eof,
}

// ── I/O redirection types ─────────────────────────────────────────────────────

enum OutputSink {
    Vio,
    File(std::fs::File),
    Buffer(Vec<u8>),
}

enum InputSource {
    Kbd,
    Lines(VecDeque<String>),
}

/// Parsed command line with pipeline segments and I/O redirections.
pub struct ParsedCommand {
    /// Pipeline segments (commands split at unquoted `|`)
    pub segments: Vec<String>,
    /// `< file` stdin redirect
    pub stdin_file: Option<String>,
    /// `> file` or `>> file` stdout redirect: (path, append)
    pub stdout_redir: Option<(String, bool)>,
}

// ── CmdShell ─────────────────────────────────────────────────────────────────

struct CmdShell {
    shared: Arc<SharedState>,
    history: Vec<String>,
    output_sink: RefCell<OutputSink>,
    input_source: RefCell<InputSource>,
    /// The shell's OS/2 environment (`SET`), in order. It is the shell's
    /// own: the Warpine process's host environment is never read or
    /// changed by the shell (a guest could otherwise set `LD_PRELOAD` for
    /// every later child, or read the host's secrets). Programs the shell
    /// starts get it through `WARPINE_OS2ENV`.
    env: RefCell<Vec<(String, String)>>,
}

/// The most variables and bytes of an environment the shell keeps.
const MAX_ENV_VARS: usize = 1024;
const MAX_ENV_BYTES: usize = 64 * 1024;

/// The OS/2 environment of a shell no program started: Warpine's default.
const DEFAULT_SHELL_ENV: &[&str] = &["PATH=C:\\", "COMSPEC=C:\\CMD.EXE", "OS=OS2", "TMP=C:\\TMP"];

/// `NAME=value` strings as (name, value) pairs; strings without `=` are dropped.
fn parse_env<S: AsRef<str>>(strings: &[S]) -> Vec<(String, String)> {
    strings.iter().filter_map(|s| {
        let (k, v) = s.as_ref().split_once('=')?;
        (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
    }).take(MAX_ENV_VARS).collect()
}

/// Why `SET` refused a change.
#[derive(Debug, PartialEq, Eq)]
enum EnvError { NoName, Full }

/// `SET name=value` on `env`: names are matched without case and kept as
/// first written; an empty value removes the variable. The environment
/// stays within `MAX_ENV_VARS` variables and `MAX_ENV_BYTES` bytes.
fn env_set(env: &mut Vec<(String, String)>, name: &str, value: &str) -> Result<(), EnvError> {
    if name.is_empty() || name.contains('\0') { return Err(EnvError::NoName); }
    let at = env.iter().position(|(k, _)| k.eq_ignore_ascii_case(name));
    if value.is_empty() {
        if let Some(i) = at { env.remove(i); }
        return Ok(());
    }
    let size: usize = env.iter().enumerate().filter(|&(i, _)| Some(i) != at)
        .map(|(_, (k, v))| k.len() + v.len() + 2).sum();
    if size + name.len() + value.len() + 2 > MAX_ENV_BYTES || (at.is_none() && env.len() >= MAX_ENV_VARS) {
        return Err(EnvError::Full);
    }
    let value = value.replace('\0', "");
    match at {
        Some(i) => env[i].1 = value,
        None => env.push((name.to_string(), value)),
    }
    Ok(())
}

/// The environment block of `env` for `WARPINE_OS2ENV`: `NAME=value`
/// strings, each with its NUL (the reader adds the block's end), as
/// `DosExecPgm` passes it.
fn env_block(env: &[(String, String)], codepage: u32) -> Vec<u8> {
    let strings: Vec<Vec<u8>> = env.iter()
        .map(|(k, v)| super::codepage::cp_encode(&format!("{k}={v}"), codepage)).collect();
    let mut block = super::inherit::join_env(&strings);
    block.pop();
    block
}

impl CmdShell {
    fn new(shared: Arc<SharedState>, env: Vec<(String, String)>) -> Self {
        CmdShell {
            shared,
            history: Vec::with_capacity(HISTORY_SIZE),
            output_sink: RefCell::new(OutputSink::Vio),
            input_source: RefCell::new(InputSource::Kbd),
            env: RefCell::new(env),
        }
    }

    // ── Entry point ──────────────────────────────────────────────────────────

    /// Run the shell. `raw_args` is the parsed double-null argument vector
    /// (`args[0]` = program name, `args[1]` = optional flag string).
    /// Returns the shell exit code.
    fn run(&mut self, raw_args: &[String]) -> u32 {
        let (run_once, initial_cmd) = parse_shell_flags(raw_args);

        if let Some(cmd) = initial_cmd {
            let result = self.execute_line(cmd.trim());
            if run_once {
                return result.unwrap_or(0);
            }
            if result.is_some() {
                return result.unwrap_or(0);
            }
        }

        self.interactive_loop()
    }

    fn interactive_loop(&mut self) -> u32 {
        loop {
            if self.shared.exit_requested.load(Ordering::Relaxed) {
                return 0;
            }
            let prompt = self.build_prompt();
            self.write_attr(prompt.as_bytes(), ATTR_HEADER);

            match self.read_line() {
                ReadResult::Eof => return 0,
                ReadResult::Line(line) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() { continue; }
                    self.history_push(trimmed.clone());
                    if let Some(exit_code) = self.execute_line(&trimmed) {
                        return exit_code;
                    }
                }
            }
        }
    }

    // ── Keyboard input ───────────────────────────────────────────────────────

    /// Block until a key is available. Returns `None` only on shutdown.
    fn read_key(&self) -> Option<KbdKeyInfo> {
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
    fn read_line(&mut self) -> ReadResult {
        // Check non-kbd input source first.
        {
            let mut src = self.input_source.borrow_mut();
            if let InputSource::Lines(ref mut lines) = *src {
                return match lines.pop_front() {
                    Some(line) => ReadResult::Line(line),
                    None => ReadResult::Eof,
                };
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
    fn replace_input(&self, current: &mut String, new_text: &str) {
        for _ in 0..current.len() {
            self.write_out(b"\x08 \x08");
        }
        *current = new_text.to_string();
        self.write_out(current.as_bytes());
    }

    // ── History ───────────────────────────────────────────────────────────────

    fn history_push(&mut self, line: String) {
        // Don't add duplicates of the most recent entry.
        if self.history.last().map(|s| s.as_str()) == Some(line.as_str()) {
            return;
        }
        if self.history.len() >= HISTORY_SIZE {
            self.history.remove(0);
        }
        self.history.push(line);
    }

    // ── Output helpers ────────────────────────────────────────────────────────

    fn write_out(&self, bytes: &[u8]) {
        self.write_sink(bytes, ATTR_NORMAL);
    }

    fn write_attr(&self, bytes: &[u8], attr: u8) {
        self.write_sink(bytes, attr);
    }

    fn write_sink(&self, bytes: &[u8], attr: u8) {
        let mut sink = self.output_sink.borrow_mut();
        match &mut *sink {
            OutputSink::Vio => {
                let cp = self.shared.active_codepage.load(Ordering::Relaxed);
                let mut vio = self.shared.console_mgr.lock_or_recover();
                vio.write_tty_attr(bytes, attr, cp);
            }
            OutputSink::File(f) => { f.write_all(bytes).ok(); }
            OutputSink::Buffer(v) => { v.extend_from_slice(bytes); }
        }
    }

    fn writeln(&self, s: &str) {
        self.write_out(s.as_bytes());
        self.write_out(b"\r\n");
    }

    fn writeln_attr(&self, s: &str, attr: u8) {
        self.write_attr(s.as_bytes(), attr);
        self.write_attr(b"\r\n", attr);
    }

    // ── Prompt ────────────────────────────────────────────────────────────────

    fn build_prompt(&self) -> String {
        let dm = self.shared.drive_mgr.lock_or_recover();
        let drive_idx = dm.current_disk();                          // 0-based
        let drive_letter = (b'A' + drive_idx) as char;
        let dir = dm.current_dir(drive_idx);
        if dir.is_empty() {
            format!("[{drive_letter}:\\] ")
        } else {
            format!("[{drive_letter}:\\{dir}] ")
        }
    }

    // ── Command dispatch ─────────────────────────────────────────────────────

    /// Execute one command line. Returns `Some(exit_code)` when the shell
    /// should terminate, or `None` to keep looping.
    fn execute_line(&mut self, line: &str) -> Option<u32> {
        // Strip inline comments (text after unquoted `::`)
        let line = if let Some(pos) = line.find("::") { &line[..pos] } else { line };
        let line = line.trim();
        if line.is_empty() { return None; }
        let parsed = parse_command(line);
        self.execute_pipeline(parsed)
    }

    /// Execute a pipeline parsed from a command line.
    fn execute_pipeline(&mut self, parsed: ParsedCommand) -> Option<u32> {
        let segments = parsed.segments;
        if segments.iter().all(|s| s.is_empty()) { return None; }

        // Validate and prepare stdout sink (but keep errors going to Vio during setup).
        let final_sink = if let Some((ref path, append)) = parsed.stdout_redir {
            // Inside a drive only: a path that does not map is an error,
            // never the guest's string used as a host path (ADR-0066).
            let host_path = self.shared.drive_mgr.lock_or_recover().resolve_new_host_path(path);
            let Some(host_path) = host_path else {
                self.writeln_attr(&format!("Cannot open '{path}': not on a drive"), ATTR_ERROR);
                return None;
            };
            match std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .append(append)
                .truncate(!append)
                .open(&host_path)
            {
                Ok(f) => Some(OutputSink::File(f)),
                Err(e) => {
                    self.writeln_attr(&format!("Cannot open '{}': {e}", path), ATTR_ERROR);
                    return None;
                }
            }
        } else {
            None
        };

        // Prepare stdin source.
        let initial_stdin = if let Some(ref path) = parsed.stdin_file {
            let host_path = {
                let dm = self.shared.drive_mgr.lock_or_recover();
                dm.resolve_to_host_path(path)
            };
            if let Some(p) = host_path {
                match std::fs::read_to_string(&p) {
                    Ok(content) => {
                        let lines: VecDeque<String> = content.lines()
                            .map(|l| l.to_string())
                            .collect();
                        Some(InputSource::Lines(lines))
                    }
                    Err(e) => {
                        self.writeln_attr(&format!("Cannot read '{}': {e}", path), ATTR_ERROR);
                        return None;
                    }
                }
            } else {
                self.writeln_attr(&format!("File not found: {}", path), ATTR_ERROR);
                return None;
            }
        } else {
            None
        };

        // Apply the prepared stdin source.
        if let Some(src) = initial_stdin {
            *self.input_source.borrow_mut() = src;
        }

        // Apply final stdout sink if any.
        if let Some(sink) = final_sink {
            *self.output_sink.borrow_mut() = sink;
        }

        let n = segments.len();
        let mut result = None;

        for (i, segment) in segments.iter().enumerate() {
            let segment = segment.trim();
            if segment.is_empty() { continue; }

            if i < n - 1 {
                // Non-last segment: capture output to buffer.
                let prev_sink = self.output_sink.replace(OutputSink::Buffer(Vec::new()));
                result = self.execute_segment(segment);
                let captured = match self.output_sink.replace(prev_sink) {
                    OutputSink::Buffer(v) => v,
                    _ => Vec::new(),
                };
                // Feed captured output as the next segment's stdin.
                let lines: VecDeque<String> = captured
                    .split(|&b| b == b'\n')
                    .map(|l| String::from_utf8_lossy(l).trim_end_matches('\r').to_string())
                    .collect();
                *self.input_source.borrow_mut() = InputSource::Lines(lines);
            } else {
                result = self.execute_segment(segment);
            }

            if result.is_some() { break; }
        }

        // Restore defaults.
        *self.output_sink.borrow_mut() = OutputSink::Vio;
        *self.input_source.borrow_mut() = InputSource::Kbd;

        result
    }

    /// Execute a single (non-pipeline) command segment.
    fn execute_segment(&mut self, line: &str) -> Option<u32> {
        let tokens = tokenize(line);
        if tokens.is_empty() { return None; }

        let cmd = tokens[0].to_uppercase();
        let args: Vec<&str> = tokens[1..].to_vec();

        debug!("cmd: cmd='{}' args={:?}", cmd, args);

        match cmd.as_str() {
            "REM" | "::" => None,
            "ECHO" => { self.cmd_echo(&args); None }
            "CLS"  => { self.cmd_cls(); None }
            "VER"  => { self.cmd_ver(); None }
            "HELP" => { self.cmd_help(); None }
            "SET"  => { self.cmd_set(&args); None }
            "CD" | "CHDIR" => { self.cmd_cd(&args); None }
            "DIR"  => { self.cmd_dir(&args); None }
            "TYPE" => { self.cmd_type(&args); None }
            "MD" | "MKDIR" => { self.cmd_md(&args); None }
            "RD" | "RMDIR" => { self.cmd_rd(&args); None }
            "DEL" | "ERASE" => { self.cmd_del(&args); None }
            "EXIT" => {
                let code = args.first()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                Some(code)
            }
            // Drive-letter change: "C:", "D:", …
            _ if cmd.len() == 2 && cmd.ends_with(':') => {
                let c = cmd.chars().next().unwrap();
                if c.is_ascii_alphabetic() {
                    self.cmd_change_drive(c as u8 - b'A' + 1);
                } else {
                    self.writeln_attr("Invalid drive.", ATTR_ERROR);
                }
                None
            }
            // External program or .cmd script
            _ => {
                self.exec_external(&cmd, &args);
                None
            }
        }
    }

    // ── Built-in commands ─────────────────────────────────────────────────────

    fn cmd_echo(&self, args: &[&str]) {
        if args.is_empty() {
            self.writeln("ECHO is on.");
        } else {
            self.writeln(&args.join(" "));
        }
    }

    fn cmd_cls(&self) {
        // Clear the screen and home the cursor on the terminal as well (the
        // old row-by-row blanking left the terminal cursor where it was).
        self.shared.console_mgr.lock_or_recover().clear_screen(ATTR_NORMAL);
    }

    fn cmd_ver(&self) {
        self.writeln("Warpine OS/2 Compatibility Layer [Builtin CMD.EXE]");
    }

    fn cmd_help(&self) {
        self.writeln("Available commands:");
        self.writeln("  DIR [path]         List directory");
        self.writeln("  CD [path]          Change directory");
        self.writeln("  <X>:               Change drive");
        self.writeln("  SET [var[=value]]  Show/set environment variable");
        self.writeln("  ECHO [text]        Display text");
        self.writeln("  TYPE file          Display file contents");
        self.writeln("  MD / MKDIR dir     Create directory");
        self.writeln("  RD / RMDIR dir     Remove directory");
        self.writeln("  DEL / ERASE file   Delete file");
        self.writeln("  CLS                Clear screen");
        self.writeln("  VER                Show version");
        self.writeln("  REM / ::           Comment");
        self.writeln("  EXIT [code]        Exit shell");
        self.writeln("  <program>          Run OS/2 executable");
    }

    fn cmd_set(&self, args: &[&str]) {
        if args.is_empty() {
            // List the shell's OS/2 environment.
            for (k, v) in self.env.borrow().iter() {
                self.writeln(&format!("{k}={v}"));
            }
            return;
        }
        let joined = args.join(" ");
        if let Some(eq) = joined.find('=') {
            // SET VAR=value; SET VAR= removes it.
            let name = joined[..eq].trim();
            let value = &joined[eq + 1..];
            match env_set(&mut self.env.borrow_mut(), name, value) {
                Ok(()) => {}
                Err(EnvError::NoName) => self.writeln_attr("Syntax error: no variable name.", ATTR_ERROR),
                Err(EnvError::Full) => self.writeln_attr("Out of environment space.", ATTR_ERROR),
            }
            return;
        }
        // SET VAR — show the variables whose names start with VAR.
        let prefix = joined.to_uppercase();
        let found: Vec<String> = self.env.borrow().iter()
            .filter(|(k, _)| k.to_uppercase().starts_with(&prefix))
            .map(|(k, v)| format!("{k}={v}")).collect();
        if found.is_empty() {
            self.writeln_attr("Environment variable not found.", ATTR_ERROR);
        }
        for line in found {
            self.writeln(&line);
        }
    }

    fn cmd_cd(&self, args: &[&str]) {
        if args.is_empty() {
            // Show current directory
            let dm = self.shared.drive_mgr.lock_or_recover();
            let drive_idx = dm.current_disk();
            let drive_letter = (b'A' + drive_idx) as char;
            let dir = dm.current_dir(drive_idx);
            if dir.is_empty() {
                self.writeln(&format!("{drive_letter}:\\"));
            } else {
                self.writeln(&format!("{drive_letter}:\\{dir}"));
            }
            return;
        }

        let path = args[0];
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        match dm.set_current_dir(path) {
            Ok(()) => {
                // Sync ProcessManager.current_dir so child process CWD is correct.
                let drive_idx = dm.current_disk();
                let new_dir = dm.current_dir(drive_idx).to_string();
                drop(dm);
                let mut pm = self.shared.process_mgr.lock_or_recover();
                pm.current_dir = if new_dir.is_empty() {
                    "\\".to_string()
                } else if new_dir.starts_with('\\') {
                    new_dir
                } else {
                    format!("\\{new_dir}")
                };
            }
            Err(e) => {
                drop(dm);
                self.writeln_attr(
                    &format!("The system cannot find the path specified. ({:?})", e),
                    ATTR_ERROR,
                );
            }
        }
    }

    fn cmd_change_drive(&self, disk_os2: u8) {
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        if dm.set_current_disk(disk_os2).is_err() {
            let letter = (b'A' + disk_os2 - 1) as char;
            drop(dm);
            self.writeln_attr(&format!("Drive {letter}: is not available."), ATTR_ERROR);
        }
    }

    fn cmd_dir(&self, args: &[&str]) {
        // Build the search pattern from the argument (default: current dir \*)
        let pattern = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            let drive_idx = dm.current_disk();
            let drive_letter = (b'A' + drive_idx) as char;
            let cur = dm.current_dir(drive_idx);
            let base = if cur.is_empty() {
                format!("{drive_letter}:\\")
            } else {
                format!("{drive_letter}:\\{cur}")
            };
            drop(dm);
            if let Some(&path_arg) = args.first() {
                // If path_arg ends with \ it's a directory, append *
                let p = path_arg.trim_end_matches(['\\', '/']);
                if path_arg.ends_with('\\') || path_arg.ends_with('/') {
                    format!("{p}\\*")
                } else if path_arg.contains('*') || path_arg.contains('?') {
                    path_arg.to_string()
                } else {
                    // Might be a directory — try listing its contents
                    format!("{p}\\*")
                }
            } else {
                format!("{base}*")
            }
        };

        // Show header
        let header_path = pattern.trim_end_matches('*').trim_end_matches('\\');
        self.writeln_attr(&format!(" Directory of {header_path}"), ATTR_HEADER);
        self.writeln("");

        // Collect entries
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        let result = dm.find_first(&pattern, FileAttribute(0x37), 1);
        let (handle, first_entry) = match result {
            Ok(r) => r,
            Err(_) => {
                drop(dm);
                self.writeln_attr("File not found.", ATTR_ERROR);
                return;
            }
        };

        let mut entries: Vec<super::vfs::DirEntry> = vec![first_entry];
        while let Ok(e) = dm.find_next(handle) {
            entries.push(e);
        }
        let _ = dm.find_close(handle);
        drop(dm);

        // Sort: directories first, then by name
        entries.sort_by(|a, b| {
            let a_dir = a.status.attributes.contains(FileAttribute::DIRECTORY);
            let b_dir = b.status.attributes.contains(FileAttribute::DIRECTORY);
            match (a_dir, b_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_uppercase().cmp(&b.name.to_uppercase()),
            }
        });

        let mut file_count = 0u32;
        let mut dir_count = 0u32;
        let mut total_bytes = 0u64;

        for e in &entries {
            let is_dir = e.status.attributes.contains(FileAttribute::DIRECTORY);
            let date = format_dos_date(e.status.last_write_date);
            let time = format_dos_time(e.status.last_write_time);
            if is_dir {
                dir_count += 1;
                self.writeln(&format!(
                    "{date}  {time}    <DIR>          {}",
                    e.name
                ));
            } else {
                file_count += 1;
                total_bytes += e.status.file_size as u64;
                self.writeln(&format!(
                    "{date}  {time}   {:>12}  {}",
                    e.status.file_size, e.name
                ));
            }
        }

        self.writeln("");
        self.writeln(&format!(
            "       {file_count:>3} file(s)  {:>12} bytes",
            total_bytes
        ));
        self.writeln(&format!("       {dir_count:>3} dir(s)"));
    }

    fn cmd_type(&self, args: &[&str]) {
        let Some(&path_arg) = args.first() else {
            self.writeln_attr("Required parameter missing.", ATTR_ERROR);
            return;
        };

        let host_path = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            dm.resolve_to_host_path(path_arg)
        };

        match host_path {
            None => self.writeln_attr("The system cannot find the file specified.", ATTR_ERROR),
            Some(p) => match std::fs::read(&p) {
                Err(e) => self.writeln_attr(&format!("Error reading file: {e}"), ATTR_ERROR),
                Ok(bytes) => {
                    self.write_out(&bytes);
                    self.write_out(b"\r\n");
                }
            },
        }
    }

    fn cmd_md(&self, args: &[&str]) {
        let Some(&path_arg) = args.first() else {
            self.writeln_attr("Required parameter missing.", ATTR_ERROR);
            return;
        };
        let dm = self.shared.drive_mgr.lock_or_recover();
        if let Err(e) = dm.create_dir(path_arg) {
            drop(dm);
            self.writeln_attr(&format!("Cannot create directory: {e:?}"), ATTR_ERROR);
        }
    }

    fn cmd_rd(&self, args: &[&str]) {
        let Some(&path_arg) = args.first() else {
            self.writeln_attr("Required parameter missing.", ATTR_ERROR);
            return;
        };
        let dm = self.shared.drive_mgr.lock_or_recover();
        if let Err(e) = dm.delete_dir(path_arg) {
            drop(dm);
            self.writeln_attr(&format!("Cannot remove directory: {e:?}"), ATTR_ERROR);
        }
    }

    fn cmd_del(&self, args: &[&str]) {
        let Some(&path_arg) = args.first() else {
            self.writeln_attr("Required parameter missing.", ATTR_ERROR);
            return;
        };
        let dm = self.shared.drive_mgr.lock_or_recover();
        if let Err(e) = dm.delete_file(path_arg) {
            drop(dm);
            self.writeln_attr(&format!("Could not delete file: {e:?}"), ATTR_ERROR);
        }
    }

    // ── External execution ────────────────────────────────────────────────────

    fn exec_external(&self, cmd: &str, args: &[&str]) {
        // Resolve: try as-is first (might have .EXE already), then append .EXE
        let (host_path, is_script) = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            let upper = cmd.to_uppercase();
            if upper.ends_with(".CMD") {
                (dm.resolve_to_host_path(cmd), true)
            } else if upper.ends_with(".EXE") {
                (dm.resolve_to_host_path(cmd), false)
            } else {
                let with_exe = format!("{cmd}.EXE");
                let p = dm.resolve_to_host_path(&with_exe);
                if p.is_some() {
                    (p, false)
                } else {
                    let with_cmd = format!("{cmd}.CMD");
                    (dm.resolve_to_host_path(&with_cmd), true)
                }
            }
        };

        match host_path {
            None => {
                self.writeln_attr(
                    &format!("'{cmd}' is not recognized as an internal or external command."),
                    ATTR_ERROR,
                );
            }
            Some(p) if is_script => {
                // A script shares the shell's environment: its SETs stay.
                let mut shell = CmdShell::new(Arc::clone(&self.shared), self.env.borrow().clone());
                shell.run_script(&p);
                *self.env.borrow_mut() = shell.env.into_inner();
            }
            Some(p) => {
                self.spawn_external(&p, args);
            }
        }
    }

    /// Spawn `warpine <prog_path> [args...]` as a child process and relay its
    /// stdout through the current output sink.
    fn spawn_external(&self, prog_path: &std::path::Path, args: &[&str]) {
        let warpine_exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                self.writeln_attr(&format!("Cannot locate warpine executable: {e}"), ATTR_ERROR);
                return;
            }
        };

        let mut cmd = std::process::Command::new(&warpine_exe);
        cmd.arg(prog_path);
        for a in args { cmd.arg(a); }
        // What this process got from its own parent is not the child's;
        // the child's environment is the shell's.
        super::inherit::clear_parent_vars(&mut cmd);
        let codepage = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        cmd.env(super::inherit::ENV_ENV, super::inherit::escape(&env_block(&self.env.borrow(), codepage)));
        // Bounded nesting and number of children, and a slot of the
        // session's THREADS= limit (inherit.rs).
        let slot = match super::inherit::admit_child(&self.shared, &mut cmd) {
            Ok(slot) => slot,
            Err(super::inherit::ChildLimit::Threads) => {
                self.writeln_attr("Cannot start another process: the THREADS= limit is reached",
                                  ATTR_ERROR);
                return;
            }
            Err(_) => {
                self.writeln_attr("Cannot start another process: too many nested or running processes",
                                  ATTR_ERROR);
                return;
            }
        };

        // Set CWD from ProcessManager so the child inherits the shell's directory.
        let cwd = {
            let pm = self.shared.process_mgr.lock_or_recover();
            let dir = pm.current_dir.replace('\\', "/").trim_start_matches('/').to_string();
            if dir.is_empty() {
                std::env::current_dir().ok()
            } else {
                std::env::current_dir().ok().map(|base| base.join(&dir))
            }
        };
        if let Some(ref cwd) = cwd
            && cwd.is_dir() { cmd.current_dir(cwd); }

        cmd.stdout(std::process::Stdio::piped())
           .stderr(std::process::Stdio::inherit());

        let to_vio = matches!(&*self.output_sink.borrow(), OutputSink::Vio);

        let spawned = cmd.spawn();
        super::inherit::child_started(&self.shared, slot,
                                      spawned.as_ref().ok().map(std::process::Child::id));
        match spawned {
            Err(e) => {
                self.writeln_attr(&format!("Failed to spawn program: {e}"), ATTR_ERROR);
            }
            Ok(mut child) => {
                if to_vio {
                    // Real-time relay to VioManager.
                    let shared_relay = Arc::clone(&self.shared);
                    let relay = child.stdout.take().map(move |pipe| {
                        std::thread::spawn(move || {
                            let mut buf = [0u8; 512];
                            let mut r = pipe;
                            loop {
                                match r.read(&mut buf) {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => {
                                        let cp = shared_relay.active_codepage.load(Ordering::Relaxed);
                                        let mut vio = shared_relay.console_mgr.lock_or_recover();
                                        vio.write_tty(&buf[..n], cp);
                                    }
                                }
                            }
                        })
                    });
                    self.wait_child(&mut child);
                    if let Some(t) = relay { let _ = t.join(); }
                } else {
                    // Buffer all output then write to sink (read on a
                    // thread of its own, so that the wait stays
                    // interruptible).
                    let reader = child.stdout.take().map(|mut pipe| std::thread::spawn(move || {
                        let mut output = Vec::new();
                        pipe.read_to_end(&mut output).ok();
                        output
                    }));
                    self.wait_child(&mut child);
                    let output = reader.and_then(|t| t.join().ok()).unwrap_or_default();
                    if !output.is_empty() {
                        self.write_out(&output);
                    }
                }
            }
        }
    }

    /// Wait for `child` to end. When the shell's call is interrupted (a
    /// signal, a kill) or its process ends, the child is asked to end
    /// (`SIGTERM`: its exit list runs) and, after `KILL_GRACE`, killed.
    fn wait_child(&self, child: &mut std::process::Child) {
        let mut asked: Option<std::time::Instant> = None;
        loop {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => {}
            }
            let stop = super::interrupt::call_interrupted(&self.shared)
                || self.shared.exit_requested.load(Ordering::Relaxed);
            if stop && asked.is_none() {
                if let Ok(pid) = i32::try_from(child.id()) {
                    // SAFETY: kill(2) on the pid of a child this shell
                    // started and has not yet reaped; no memory involved.
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
                asked = Some(std::time::Instant::now());
            }
            if asked.is_some_and(|t| t.elapsed() >= super::signals::KILL_GRACE) {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            std::thread::sleep(CHILD_POLL);
        }
    }

    // ── Script execution ──────────────────────────────────────────────────────

    fn run_script(&mut self, path: &std::path::Path) -> u32 {
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                self.writeln_attr(&format!("Cannot open script: {e}"), ATTR_ERROR);
                return 1;
            }
        };

        for line in content.lines() {
            if self.shared.exit_requested.load(Ordering::Relaxed) {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() { continue; }
            if let Some(exit_code) = self.execute_line(trimmed) {
                return exit_code;
            }
        }
        0
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

/// Parse `/C` / `/K` flags and extract the optional initial command.
/// Returns `(run_once, initial_command)`.
///
/// `/C <cmd>` — run command then exit (`run_once = true`)
/// `/K <cmd>` — run command then stay interactive (`run_once = false`)
/// No flag     — start interactive shell immediately
pub fn parse_shell_flags(args: &[String]) -> (bool, Option<String>) {
    // args[0] is the program name; args[1] is the optional argument string.
    let arg_str = match args.get(1) {
        Some(s) if !s.is_empty() => s.trim(),
        _ => return (false, None),
    };

    let upper = arg_str.to_uppercase();
    if upper.starts_with("/C ") || upper == "/C" {
        let cmd = arg_str[2..].trim().to_string();
        (true, if cmd.is_empty() { None } else { Some(cmd) })
    } else if upper.starts_with("/K ") || upper == "/K" {
        let cmd = arg_str[2..].trim().to_string();
        (false, if cmd.is_empty() { None } else { Some(cmd) })
    } else {
        // Treat anything else as a command to run interactively (/K semantics)
        (false, Some(arg_str.to_string()))
    }
}

/// Tokenize a command line, respecting double-quoted strings.
pub fn tokenize(line: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut in_quote = false;
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            in_quote = !in_quote;
            if start.is_none() { start = Some(i); }
        } else if b == b' ' && !in_quote {
            if let Some(s) = start.take() {
                tokens.push(&line[s..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
        i += 1;
    }
    if let Some(s) = start {
        tokens.push(&line[s..]);
    }
    tokens
}

/// Format an OS/2 DOS date word to `MM-DD-YY` string.
pub fn format_dos_date(date: u16) -> String {
    let day   = (date & 0x1F) as u32;
    let month = ((date >> 5) & 0x0F) as u32;
    let year  = ((date >> 9) & 0x7F) as u32 + 1980;
    format!("{month:02}-{day:02}-{:02}", year % 100)
}

/// Format an OS/2 DOS time word to `HH:MMa/p` string.
pub fn format_dos_time(time: u16) -> String {
    let hour   = ((time >> 11) & 0x1F) as u32;
    let minute = ((time >> 5) & 0x3F) as u32;
    let (h12, ampm) = if hour == 0 {
        (12, 'a')
    } else if hour < 12 {
        (hour, 'a')
    } else if hour == 12 {
        (12, 'p')
    } else {
        (hour - 12, 'p')
    };
    format!("{h12:2}:{minute:02}{ampm}")
}

/// Parse a command line into segments (pipeline), stdout redirect, and stdin redirect.
pub fn parse_command(line: &str) -> ParsedCommand {
    let mut segments: Vec<String> = Vec::new();
    let mut stdin_file: Option<String> = None;
    let mut stdout_redir: Option<(String, bool)> = None;

    let mut current = String::new();
    let mut in_quote = false;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        if ch == '"' {
            in_quote = !in_quote;
            current.push(ch);
            i += 1;
            continue;
        }
        if in_quote {
            current.push(ch);
            i += 1;
            continue;
        }
        if ch == '|' {
            segments.push(current.trim().to_string());
            current = String::new();
            i += 1;
            continue;
        }
        if ch == '>' {
            let append = i + 1 < chars.len() && chars[i + 1] == '>';
            if append { i += 1; }
            i += 1;
            while i < chars.len() && chars[i] == ' ' { i += 1; }
            let mut fname = String::new();
            while i < chars.len() && !matches!(chars[i], ' ' | '<' | '>' | '|') {
                fname.push(chars[i]);
                i += 1;
            }
            if !fname.is_empty() { stdout_redir = Some((fname, append)); }
            continue;
        }
        if ch == '<' {
            i += 1;
            while i < chars.len() && chars[i] == ' ' { i += 1; }
            let mut fname = String::new();
            while i < chars.len() && !matches!(chars[i], ' ' | '<' | '>' | '|') {
                fname.push(chars[i]);
                i += 1;
            }
            if !fname.is_empty() { stdin_file = Some(fname); }
            continue;
        }
        current.push(ch);
        i += 1;
    }
    let last = current.trim().to_string();
    if !last.is_empty() || segments.is_empty() {
        segments.push(last);
    }

    ParsedCommand { segments, stdin_file, stdout_redir }
}

// ── Loader entry point ────────────────────────────────────────────────────────

impl super::Loader {
    /// Entry point called by `dos_exec_pgm` when the target is CMD.EXE / OS2SHELL.EXE.
    ///
    /// `p_arg` — double-null-terminated argument string (guest address, may be 0)
    /// `p_res` — RESULTCODES pointer (guest address, may be 0)
    pub fn run_builtin_cmd(&self, p_arg: u32, p_res: u32) -> u32 {
        let args = if p_arg != 0 {
            self.parse_double_null_string(p_arg)
        } else {
            Vec::new()
        };
        debug!("run_builtin_cmd: args={:?}", args);

        // The calling program's environment (DosExecPgm with a NULL pEnv
        // gives the parent's; Warpine does not yet pass pEnv to the shell).
        let env_ptr = self.guest_read::<u32>(PIB_BASE + 0x10).unwrap_or(0);
        let codepage = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let strings: Vec<String> = if env_ptr != 0 { self.read_env_block(env_ptr) } else { Vec::new() }
            .iter().map(|s| super::codepage::cp_decode(s, codepage)).collect();
        let mut shell = CmdShell::new(Arc::clone(&self.shared), parse_env(&strings));
        let exit_code = shell.run(&args);

        if p_res != 0 {
            self.guest_write::<u32>(p_res, 0);
            self.guest_write::<u32>(p_res + 4, exit_code);
        }
        NO_ERROR
    }

    /// Entry point for the **terminal path**: `warpine CMD.EXE [args]` with
    /// `WARPINE_HEADLESS=1` or when stdout is not a terminal. Runs the shell
    /// synchronously and calls `std::process::exit` when done.
    pub fn run_builtin_cmd_main(&mut self, host_args: &[&str]) {
        let exit_code = self.run_builtin_cmd_main_inner(host_args);
        self.shared.leave_thread_session();
        std::process::exit(exit_code as i32);
    }

    /// Entry point for the **SDL text-window path**: spawned on a worker thread
    /// while `run_text_loop` runs on the main thread. Signals completion via
    /// `SharedState::exit_code` + `exit_requested` so the main thread can clean up.
    pub fn run_builtin_cmd_sdl(self: Arc<Self>, host_args: Vec<String>) {
        let args: Vec<&str> = host_args.iter().map(|s| s.as_str()).collect();
        let exit_code = self.run_builtin_cmd_main_inner(&args);
        self.shared.exit_code.store(exit_code as i32, std::sync::atomic::Ordering::Relaxed);
        self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
        self.shared.kbd_cond.notify_all();
    }

    fn run_builtin_cmd_main_inner(&self, host_args: &[&str]) -> u32 {
        // Synthesise the args vector that CmdShell::run() expects:
        // args[0] = program name (ignored by run()), args[1..] = shell args/flags.
        let mut args: Vec<String> = vec!["CMD.EXE".to_string()];
        args.extend(host_args.iter().map(|s| s.to_string()));
        debug!("run_builtin_cmd: args={:?}", args);

        // The OS/2 environment a parent passed, else Warpine's default —
        // never the host's.
        let codepage = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let given = self.shared.startup.lock_or_recover().env.clone();
        let strings: Vec<String> = match given {
            Some(env) => env.iter().map(|s| super::codepage::cp_decode(s, codepage)).collect(),
            None => DEFAULT_SHELL_ENV.iter().map(|s| s.to_string()).collect(),
        };
        let mut shell = CmdShell::new(Arc::clone(&self.shared), parse_env(&strings));
        shell.run(&args)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_shell_flags ────────────────────────────────────────────────────

    #[test]
    fn test_parse_flags_no_args() {
        let args: Vec<String> = vec![];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(!once);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_parse_flags_only_program_name() {
        let args = vec!["CMD.EXE".into()];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(!once);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_parse_flags_slash_c_with_command() {
        let args = vec!["CMD.EXE".into(), "/C dir c:\\".into()];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(once);
        assert_eq!(cmd.as_deref(), Some("dir c:\\"));
    }

    #[test]
    fn test_parse_flags_slash_c_alone() {
        let args = vec!["CMD.EXE".into(), "/C".into()];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(once);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_parse_flags_slash_k_with_command() {
        let args = vec!["CMD.EXE".into(), "/K echo hello".into()];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(!once);
        assert_eq!(cmd.as_deref(), Some("echo hello"));
    }

    // ── tokenize ────────────────────────────────────────────────────────────

    #[test]
    fn test_tokenize_simple() {
        assert_eq!(tokenize("dir c:\\"), vec!["dir", "c:\\"]);
    }

    #[test]
    fn test_tokenize_empty() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn test_tokenize_quoted() {
        let t = tokenize(r#"echo "hello world""#);
        assert_eq!(t, vec!["echo", r#""hello world""#]);
    }

    #[test]
    fn test_tokenize_multiple_spaces() {
        assert_eq!(tokenize("cd  c:\\os2"), vec!["cd", "c:\\os2"]);
    }

    // ── format_dos_date / format_dos_time ────────────────────────────────────

    #[test]
    fn test_format_dos_date_epoch() {
        // year=0 (1980), month=1, day=1 → 01-01-80
        let date: u16 = (1 << 5) | 1; // year field (bits 9-15) is 0
        assert_eq!(format_dos_date(date), "01-01-80");
    }

    #[test]
    fn test_format_dos_date_2024_march_15() {
        // year=44 (1980+44=2024), month=3, day=15
        let date: u16 = (44 << 9) | (3 << 5) | 15;
        assert_eq!(format_dos_date(date), "03-15-24");
    }

    #[test]
    fn test_format_dos_date_zero() {
        // All zero: day=0, month=0, year=0 → 00-00-80
        assert_eq!(format_dos_date(0), "00-00-80");
    }

    #[test]
    fn test_format_dos_time_midnight() {
        // 0:00 AM → 12:00a
        assert_eq!(format_dos_time(0), "12:00a");
    }

    #[test]
    fn test_format_dos_time_noon() {
        // 12:00 → 12:00p
        let time: u16 = 12 << 11; // minutes (bits 5-10) are 0
        assert_eq!(format_dos_time(time), "12:00p");
    }

    #[test]
    fn test_format_dos_time_afternoon() {
        // 14:30 → 2:30p
        let time: u16 = (14 << 11) | (30 << 5);
        assert_eq!(format_dos_time(time), " 2:30p");
    }

    #[test]
    fn test_format_dos_time_morning() {
        // 9:05 → 9:05a
        let time: u16 = (9 << 11) | (5 << 5);
        assert_eq!(format_dos_time(time), " 9:05a");
    }

    // ── Integration: run shell, queue keystrokes, check output ───────────────

    use super::super::Loader;
    use super::super::mutex_ext::MutexExt;

    /// Push keystrokes into the SDL kbd_queue and return the VioManager
    /// contents after the shell processes them.
    fn push_keys(loader: &Loader, keys: &[KbdKeyInfo]) {
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        let mut q = loader.shared.kbd_queue.lock().unwrap();
        for &k in keys {
            q.push_back(k);
        }
        drop(q);
        loader.shared.kbd_cond.notify_all();
    }

    fn key_char(ch: u8) -> KbdKeyInfo { KbdKeyInfo { ch, scan: 0, state: 0 } }
    fn key_scan(scan: u8) -> KbdKeyInfo { KbdKeyInfo { ch: 0, scan, state: 0 } }
    fn key_enter() -> KbdKeyInfo { KbdKeyInfo { ch: 0x0D, scan: SCAN_ENTER, state: 0 } }

    fn screen_text(loader: &Loader) -> String {
        let vio = loader.shared.console_mgr.lock_or_recover();
        vio.buffer.iter().map(|(c, _)| *c).collect()
    }

    #[test]
    fn test_cmd_ver_output() {
        let loader = Loader::new_mock();
        push_keys(&loader, &[
            key_char(b'v'), key_char(b'e'), key_char(b'r'), key_enter(),
            // exit
            key_char(b'e'), key_char(b'x'), key_char(b'i'), key_char(b't'), key_enter(),
        ]);
        let rc = loader.run_builtin_cmd(0, 0);
        assert_eq!(rc, NO_ERROR);
        let screen = screen_text(&loader);
        assert!(screen.contains("Warpine"), "screen should contain 'Warpine': {:?}", &screen[..80]);
    }

    #[test]
    fn test_cmd_echo_output() {
        let loader = Loader::new_mock();
        push_keys(&loader, &[
            key_char(b'e'), key_char(b'c'), key_char(b'h'), key_char(b'o'),
            key_char(b' '), key_char(b'H'), key_char(b'i'), key_enter(),
            key_char(b'e'), key_char(b'x'), key_char(b'i'), key_char(b't'), key_enter(),
        ]);
        let rc = loader.run_builtin_cmd(0, 0);
        assert_eq!(rc, NO_ERROR);
        let screen = screen_text(&loader);
        assert!(screen.contains('H') && screen.contains('i'),
            "echo Hi should appear on screen");
    }

    #[test]
    fn test_cmd_exit_code() {
        let loader = Loader::new_mock();
        // exit 42
        push_keys(&loader, &[
            key_char(b'e'), key_char(b'x'), key_char(b'i'), key_char(b't'),
            key_char(b' '), key_char(b'4'), key_char(b'2'), key_enter(),
        ]);
        let p_res = 0x3000u32;
        loader.guest_write::<u32>(p_res, 0xDEAD).unwrap();
        loader.guest_write::<u32>(p_res + 4, 0xDEAD).unwrap();
        let rc = loader.run_builtin_cmd(0, p_res);
        assert_eq!(rc, NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(p_res), Some(0));
        assert_eq!(loader.guest_read::<u32>(p_res + 4), Some(42));
    }

    #[test]
    fn test_cmd_backspace_editing() {
        let loader = Loader::new_mock();
        // Type "exiz", backspace to correct to "exit", enter
        push_keys(&loader, &[
            key_char(b'e'), key_char(b'x'), key_char(b'i'), key_char(b'z'),
            KbdKeyInfo { ch: 0x08, scan: SCAN_BACKSPACE, state: 0 },
            key_char(b't'), key_enter(),
        ]);
        let rc = loader.run_builtin_cmd(0, 0);
        assert_eq!(rc, NO_ERROR, "corrected 'exit' should run and return NO_ERROR");
    }

    #[test]
    fn test_cmd_history_navigation() {
        let loader = Loader::new_mock();
        // Type "ver", enter, then up arrow → should reload "ver", then enter again, then exit
        push_keys(&loader, &[
            key_char(b'v'), key_char(b'e'), key_char(b'r'), key_enter(),
            key_scan(SCAN_UP),   // recall "ver"
            key_enter(),          // run it again
            key_char(b'e'), key_char(b'x'), key_char(b'i'), key_char(b't'), key_enter(),
        ]);
        let rc = loader.run_builtin_cmd(0, 0);
        assert_eq!(rc, NO_ERROR);
        let screen = screen_text(&loader);
        // "Warpine" should appear twice (two ver runs)
        let count = screen.match_indices("Warpine").count();
        assert!(count >= 2, "ver ran twice, 'Warpine' should appear ≥2 times");
    }

    #[test]
    fn test_cmd_rem_is_ignored() {
        let loader = Loader::new_mock();
        push_keys(&loader, &[
            // REM this is a comment
            key_char(b'r'), key_char(b'e'), key_char(b'm'),
            key_char(b' '), key_char(b't'), key_char(b'e'), key_char(b's'), key_char(b't'),
            key_enter(),
            key_char(b'e'), key_char(b'x'), key_char(b'i'), key_char(b't'), key_enter(),
        ]);
        let rc = loader.run_builtin_cmd(0, 0);
        assert_eq!(rc, NO_ERROR, "REM line should not cause an error");
    }

    #[test]
    fn test_cmd_slash_c_runs_once() {
        // /C echo hello → echo, then exit immediately
        let (once, cmd) = parse_shell_flags(&[
            "CMD.EXE".into(),
            "/C echo hello".into(),
        ]);
        assert!(once);
        assert_eq!(cmd.as_deref(), Some("echo hello"));
    }

    #[test]
    fn test_cmd_unknown_command_does_not_crash() {
        let loader = Loader::new_mock();
        push_keys(&loader, &[
            // Type an unknown command
            key_char(b'x'), key_char(b'y'), key_char(b'z'), key_char(b'z'), key_enter(),
            key_char(b'e'), key_char(b'x'), key_char(b'i'), key_char(b't'), key_enter(),
        ]);
        let rc = loader.run_builtin_cmd(0, 0);
        assert_eq!(rc, NO_ERROR, "unknown command should not crash the shell");
        let screen = screen_text(&loader);
        assert!(screen.contains("not recognized"), "should show error for unknown command");
    }

    // ── parse_command ─────────────────────────────────────────────────────────

    #[test]
    fn test_parse_command_no_redirections() {
        let p = parse_command("echo hello world");
        assert_eq!(p.segments, vec!["echo hello world"]);
        assert!(p.stdin_file.is_none());
        assert!(p.stdout_redir.is_none());
    }

    #[test]
    fn test_parse_command_stdout_redirect() {
        let p = parse_command("echo hello > out.txt");
        assert_eq!(p.segments, vec!["echo hello"]);
        assert_eq!(p.stdout_redir, Some(("out.txt".to_string(), false)));
    }

    #[test]
    fn test_parse_command_append_redirect() {
        let p = parse_command("echo hello >> log.txt");
        assert_eq!(p.segments, vec!["echo hello"]);
        assert_eq!(p.stdout_redir, Some(("log.txt".to_string(), true)));
    }

    #[test]
    fn test_parse_command_stdin_redirect() {
        let p = parse_command("type < input.txt");
        assert_eq!(p.segments, vec!["type"]);
        assert_eq!(p.stdin_file, Some("input.txt".to_string()));
    }

    #[test]
    fn test_parse_command_pipe() {
        let p = parse_command("echo hello | ver");
        assert_eq!(p.segments, vec!["echo hello", "ver"]);
        assert!(p.stdin_file.is_none());
        assert!(p.stdout_redir.is_none());
    }

    #[test]
    fn test_parse_command_pipe_three_stages() {
        let p = parse_command("echo a | echo b | echo c");
        assert_eq!(p.segments.len(), 3);
    }

    #[test]
    fn test_parse_command_quoted_pipe_not_split() {
        let p = parse_command(r#"echo "a|b""#);
        assert_eq!(p.segments.len(), 1, "pipe inside quotes should not split");
    }

    #[test]
    fn test_parse_command_empty() {
        let p = parse_command("   ");
        // Should produce one empty segment
        assert_eq!(p.segments, vec![""]);
        assert!(p.stdin_file.is_none());
        assert!(p.stdout_redir.is_none());
    }

    // ── Pipeline integration ──────────────────────────────────────────────────

    #[test]
    fn test_cmd_pipe_echo_to_ver() {
        // "echo hi | ver" — ver output should appear
        let loader = Loader::new_mock();
        push_keys(&loader, &[
            key_char(b'e'), key_char(b'c'), key_char(b'h'), key_char(b'o'),
            key_char(b' '), key_char(b'h'), key_char(b'i'),
            key_char(b' '), key_char(b'|'), key_char(b' '),
            key_char(b'v'), key_char(b'e'), key_char(b'r'),
            key_enter(),
            key_char(b'e'), key_char(b'x'), key_char(b'i'), key_char(b't'), key_enter(),
        ]);
        let rc = loader.run_builtin_cmd(0, 0);
        assert_eq!(rc, NO_ERROR);
        let screen = screen_text(&loader);
        assert!(
            screen.contains("Warpine"),
            "ver output should appear after pipe: {:?}",
            &screen[..200.min(screen.len())]
        );
    }

    /// A mock loader whose drive C is a fresh temporary directory.
    fn loader_with_drive_c() -> (Loader, tempfile::TempDir) {
        let loader = Loader::new_mock();
        let dir = tempfile::tempdir().expect("tempdir");
        loader.shared.drive_mgr.lock_or_recover().mount(2,
            Box::new(super::super::vfs_hostdir::HostDirBackend::new(dir.path().to_path_buf()).expect("backend")));
        (loader, dir)
    }

    #[test]
    fn test_cmd_echo_redirect_to_file() {
        let (loader, drive) = loader_with_drive_c();
        let host = drive.path().join("CMDREDIR.TXT");
        {
            let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
            shell.execute_line("echo hello > C:\\CMDREDIR.TXT");
        }
        let content = std::fs::read_to_string(&host).unwrap_or_default();
        assert!(content.contains("hello"), "redirected file should contain 'hello', got: {content:?}");
    }

    #[test]
    fn test_cmd_append_redirect() {
        let (loader, drive) = loader_with_drive_c();
        let host = drive.path().join("CMDAPPND.TXT");
        {
            let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
            shell.execute_line("echo line1 > C:\\CMDAPPND.TXT");
            shell.execute_line("echo line2 >> C:\\CMDAPPND.TXT");
        }
        let content = std::fs::read_to_string(&host).unwrap_or_default();
        assert!(content.contains("line1"), "file should contain 'line1'");
        assert!(content.contains("line2"), "file should contain 'line2' after append");
    }

    /// A redirection never names a host file outside the drives: a host
    /// path, `..` out of the drive or a relative name with no drive all
    /// stay inside drive C or fail.
    #[test]
    fn test_cmd_redirect_stays_inside_the_drives() {
        let (loader, drive) = loader_with_drive_c();
        let outside = tempfile::tempdir().expect("tempdir");
        let target = outside.path().join("escaped.txt");
        let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
        shell.execute_line(&format!("echo no > {}", target.display()));
        assert!(!target.exists(), "a host path is not a drive path");
        shell.execute_line("echo no > C:\\..\\..\\escaped.txt");
        assert!(!drive.path().parent().unwrap().join("escaped.txt").exists(), "no way out through ..");
        shell.execute_line("echo yes > inside.txt");
        assert!(drive.path().join("inside.txt").exists(), "a relative name lands on the current drive");
    }

    #[test]
    fn set_changes_only_the_shells_environment() {
        let mut env = parse_env(&["PATH=C:\\", "Os=OS2", "junk", "=x"]);
        assert_eq!(env, vec![("PATH".into(), "C:\\".into()), ("Os".into(), "OS2".into())], "no name, no variable");
        assert_eq!(env_set(&mut env, "os", "Warp"), Ok(()));
        assert_eq!(env[1], ("Os".into(), "Warp".into()), "names match without case, keep their spelling");
        assert_eq!(env_set(&mut env, "LD_PRELOAD", "./x.so"), Ok(()));
        assert_eq!(env_set(&mut env, "", "x"), Err(EnvError::NoName), "SET =x does not panic");
        assert_eq!(env_set(&mut env, "PATH", ""), Ok(()));
        assert!(!env.iter().any(|(k, _)| k == "PATH"), "an empty value removes it");
        // The host process's environment is never touched.
        assert!(std::env::var_os("LD_PRELOAD").is_none_or(|v| v != "./x.so"));
        assert_eq!(env_block(&env, 437), b"Os=Warp\0LD_PRELOAD=./x.so\0".to_vec());
    }

    #[test]
    fn the_shells_environment_is_bounded() {
        let mut env = Vec::new();
        let big = "x".repeat(MAX_ENV_BYTES);
        assert_eq!(env_set(&mut env, "A", &big), Err(EnvError::Full));
        for i in 0..MAX_ENV_VARS { env_set(&mut env, &format!("V{i}"), "1").unwrap(); }
        assert_eq!(env_set(&mut env, "ONE_MORE", "1"), Err(EnvError::Full));
        assert_eq!(env_set(&mut env, "V0", "2"), Ok(()), "changing one still works");
        assert_eq!(env_set(&mut env, "N\0UL", "1"), Err(EnvError::NoName));
    }

    #[test]
    fn set_lists_the_shells_environment_not_the_hosts() {
        let loader = crate::loader::Loader::new_mock();
        let shell = CmdShell::new(Arc::clone(&loader.shared), parse_env(&["OS=OS2"]));
        *shell.output_sink.borrow_mut() = OutputSink::Buffer(Vec::new());
        shell.cmd_set(&[]);
        shell.cmd_set(&["HOME=C:\\HOME"]);
        shell.cmd_set(&["HO"]);
        let text = match &*shell.output_sink.borrow() {
            OutputSink::Buffer(b) => String::from_utf8_lossy(b).to_string(),
            _ => unreachable!(),
        };
        assert!(text.contains("OS=OS2") && text.contains("HOME=C:\\HOME"), "{text}");
        let host_path = std::env::var("PATH").unwrap_or_default();
        assert!(host_path.is_empty() || !text.contains(&host_path), "the host's PATH is not shown");
    }
}
