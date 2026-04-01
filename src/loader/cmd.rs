// SPDX-License-Identifier: GPL-3.0-only
//
// Builtin CMD.EXE — host-Rust command shell for Warpine.
//
// Invoked by `Loader::run_builtin_cmd()` whenever `DosExecPgm` targets
// "CMD.EXE" or "OS2SHELL.EXE".  The shell runs entirely on the host; it uses
// the existing `VioManager` for output and `kbd_queue` / termios for input so
// it works identically in SDL2 text-mode and headless terminal sessions.
//
// Supported built-ins: DIR, CD, SET, ECHO, CLS, VER, TYPE, MD/MKDIR,
//   RD/RMDIR, DEL/ERASE, HELP, REM, EXIT.
// Anything else is resolved via DriveManager and executed as a child Warpine
// process (EXEC_SYNC), with stdout relayed through VioManager.
// .CMD scripts are executed line-by-line through the same dispatch path.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use log::debug;

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

// ── CmdShell ─────────────────────────────────────────────────────────────────

struct CmdShell {
    shared: Arc<SharedState>,
    history: Vec<String>,
}

impl CmdShell {
    fn new(shared: Arc<SharedState>) -> Self {
        CmdShell { shared, history: Vec::with_capacity(HISTORY_SIZE) }
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
        if self.shared.use_sdl2_text.load(Ordering::Relaxed) {
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
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Read a full line with basic line-editing: backspace, history (↑/↓), Esc
    /// to clear, and Enter to submit.
    fn read_line(&mut self) -> ReadResult {
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
        let cp = self.shared.active_codepage.load(Ordering::Relaxed);
        let mut vio = self.shared.console_mgr.lock_or_recover();
        vio.write_tty(bytes, ATTR_NORMAL, cp);
    }

    fn write_attr(&self, bytes: &[u8], attr: u8) {
        let cp = self.shared.active_codepage.load(Ordering::Relaxed);
        let mut vio = self.shared.console_mgr.lock_or_recover();
        vio.write_tty(bytes, attr, cp);
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
        let cp = self.shared.active_codepage.load(Ordering::Relaxed);
        let mut vio = self.shared.console_mgr.lock_or_recover();
        let rows = vio.rows;
        let cols = vio.cols;
        let blank = vec![b' '; cols as usize];
        for r in 0..rows {
            vio.write_char_str_att(r, 0, &blank, ATTR_NORMAL, cp);
        }
        vio.cursor_row = 0;
        vio.cursor_col = 0;
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
            // List all environment variables (host env, inherited by children)
            let mut vars: Vec<(String, String)> = std::env::vars().collect();
            vars.sort_by(|a, b| a.0.cmp(&b.0));
            for (k, v) in vars {
                self.writeln(&format!("{k}={v}"));
            }
        } else {
            let joined = args.join(" ");
            if let Some(eq) = joined.find('=') {
                let name = joined[..eq].trim();
                let value = &joined[eq + 1..];
                if value.is_empty() {
                    // SET VAR= → unset
                    // SAFETY: single-threaded shell context; no concurrent env access.
                    unsafe { std::env::remove_var(name) };
                } else {
                    // SAFETY: single-threaded shell context; no concurrent env access.
                    unsafe { std::env::set_var(name, value) };
                }
            } else {
                // SET VAR — show matching vars
                let prefix = joined.to_uppercase();
                let mut found = false;
                let mut vars: Vec<(String, String)> = std::env::vars().collect();
                vars.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in &vars {
                    if k.to_uppercase().starts_with(&prefix) {
                        self.writeln(&format!("{k}={v}"));
                        found = true;
                    }
                }
                if !found {
                    self.writeln_attr("Environment variable not found.", ATTR_ERROR);
                }
            }
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
                    // Write bytes through VIO (codepage-aware)
                    let cp = self.shared.active_codepage.load(Ordering::Relaxed);
                    let mut vio = self.shared.console_mgr.lock_or_recover();
                    vio.write_tty(&bytes, ATTR_NORMAL, cp);
                    drop(vio);
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
                let mut shell = CmdShell::new(Arc::clone(&self.shared));
                shell.run_script(&p);
            }
            Some(p) => {
                spawn_os2_program(&self.shared, &p, args);
            }
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

/// Spawn `warpine <prog_path> [args...]` as a child process and relay its
/// stdout to VioManager.  Mirrors the EXEC_SYNC path in `dos_exec_pgm`.
fn spawn_os2_program(shared: &Arc<SharedState>, prog_path: &std::path::Path, args: &[&str]) {
    let warpine_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            let cp = shared.active_codepage.load(Ordering::Relaxed);
            let mut vio = shared.console_mgr.lock_or_recover();
            let msg = format!("Cannot locate warpine executable: {e}\r\n");
            vio.write_tty(msg.as_bytes(), ATTR_ERROR, cp);
            return;
        }
    };

    let mut cmd = std::process::Command::new(&warpine_exe);
    cmd.arg(prog_path);
    for a in args { cmd.arg(a); }

    // Set CWD from ProcessManager so the child inherits the shell's directory.
    let cwd = {
        let pm = shared.process_mgr.lock_or_recover();
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

    match cmd.spawn() {
        Err(e) => {
            let cp = shared.active_codepage.load(Ordering::Relaxed);
            let mut vio = shared.console_mgr.lock_or_recover();
            let msg = format!("Failed to spawn program: {e}\r\n");
            vio.write_tty(msg.as_bytes(), ATTR_ERROR, cp);
        }
        Ok(mut child) => {
            let stdout_pipe = child.stdout.take();
            let shared_relay = Arc::clone(shared);
            let relay = stdout_pipe.map(move |pipe| {
                std::thread::spawn(move || {
                    let mut buf = [0u8; 512];
                    let mut r = pipe;
                    loop {
                        match r.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let cp = shared_relay.active_codepage.load(Ordering::Relaxed);
                                let mut vio = shared_relay.console_mgr.lock_or_recover();
                                vio.write_tty(&buf[..n], ATTR_NORMAL, cp);
                            }
                        }
                    }
                })
            });
            let _ = child.wait();
            if let Some(t) = relay { let _ = t.join(); }
        }
    }
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

        let mut shell = CmdShell::new(Arc::clone(&self.shared));
        let exit_code = shell.run(&args);

        if p_res != 0 {
            self.guest_write::<u32>(p_res, 0);
            self.guest_write::<u32>(p_res + 4, exit_code);
        }
        NO_ERROR
    }

    /// Entry point called from `main()` when the user runs `warpine CMD.EXE [args...]`
    /// directly from the host command line (no guest memory involved).
    pub fn run_builtin_cmd_main(&mut self, host_args: &[&str]) {
        // Synthesise the args vector that CmdShell::run() expects:
        // args[0] = program name (ignored by run()), args[1..] = shell args/flags.
        let mut args: Vec<String> = vec!["CMD.EXE".to_string()];
        args.extend(host_args.iter().map(|s| s.to_string()));
        debug!("run_builtin_cmd_main: args={:?}", args);

        let mut shell = CmdShell::new(Arc::clone(&self.shared));
        let exit_code = shell.run(&args);
        std::process::exit(exit_code as i32);
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
        let date: u16 = (0 << 9) | (1 << 5) | 1;
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
        let time: u16 = (12 << 11) | (0 << 5);
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

    /// Push keystrokes into the SDL2 kbd_queue and return the VioManager
    /// contents after the shell processes them.
    fn push_keys(loader: &Loader, keys: &[KbdKeyInfo]) {
        loader.shared.use_sdl2_text.store(true, Ordering::Relaxed);
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
}
