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
//
// A command line is expanded (`%NAME%`, `env.rs`), split into commands
// joined by `&`, `&&` and `||` (`parse.rs`), and each command into a
// pipeline with its redirections. Every command leaves a return code: a
// program's exit code, or 0 / 1 for a built-in that worked / failed
// (ADR-0077).

mod batch;
mod builtins;
mod env;
mod external;
mod input;
mod parse;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::Read;
use std::io::Write as IoWrite;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use log::debug;

/// How often the shell looks whether the program it waits for ended.
const CHILD_POLL: Duration = Duration::from_millis(10);

/// The most output of one command the shell holds for a pipe (the commands
/// of a pipeline run one after another); more is read and dropped, so a
/// guest cannot grow the host's memory without bound.
const MAX_CAPTURED_OUTPUT: usize = 16 * 1024 * 1024;

use super::{SharedState, KbdKeyInfo};
use super::mutex_ext::MutexExt;
use super::constants::*;
use env::{env_block, env_set, expand_vars, parse_env, EnvError, DEFAULT_SHELL_ENV};
use parse::first_word;
use parse::{parse_command, parse_shell_flags, split_chain, tokenize, Chain, ParsedCommand};

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
    /// The bytes of a pipe or of a `< file` redirection, unread.
    Data(VecDeque<u8>),
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
    /// The return code of the last command (`&&`, `||`, `/C`).
    last_rc: Cell<u32>,
    /// Whether the built-in command running now reported an error.
    failed: Cell<bool>,
    /// ECHO ON: the prompt shows, and a script's lines are echoed.
    echo_on: Cell<bool>,
    /// The script this shell runs, if any (`batch.rs`).
    batch: RefCell<Option<batch::Batch>>,
    /// How many scripts deep this shell is.
    depth: usize,
}

/// Whether `line` is a `REM` remark (`REM`, `REM text`).
fn is_remark(line: &str) -> bool {
    let word = line.split(|c: char| c.is_whitespace()).next().unwrap_or("");
    word.eq_ignore_ascii_case("REM")
}

/// The return code of a built-in command that reported an error.
const RC_BUILTIN_FAILED: u32 = 1;

impl CmdShell {
    fn new(shared: Arc<SharedState>, env: Vec<(String, String)>) -> Self {
        CmdShell {
            shared,
            history: Vec::with_capacity(HISTORY_SIZE),
            output_sink: RefCell::new(OutputSink::Vio),
            input_source: RefCell::new(InputSource::Kbd),
            env: RefCell::new(env),
            last_rc: Cell::new(0),
            failed: Cell::new(false),
            echo_on: Cell::new(true),
            batch: RefCell::new(None),
            depth: 0,
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
            // `/C`: the shell ends with the command's return code.
            if run_once {
                return result.unwrap_or(self.last_rc.get());
            }
            if let Some(code) = result {
                return code;
            }
        }

        self.interactive_loop()
    }

    fn interactive_loop(&mut self) -> u32 {
        loop {
            if self.shared.exit_requested.load(Ordering::Relaxed) {
                return 0;
            }
            if self.echo_on.get() {
                let prompt = self.build_prompt();
                self.write_attr(prompt.as_bytes(), ATTR_HEADER);
            }

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
            OutputSink::Buffer(v) => {
                let room = MAX_CAPTURED_OUTPUT.saturating_sub(v.len());
                v.extend_from_slice(&bytes[..bytes.len().min(room)]);
            }
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

    /// Report an error of the command running now: its message, and a
    /// return code of `RC_BUILTIN_FAILED` for a built-in.
    fn error(&self, msg: &str) {
        self.writeln_attr(msg, ATTR_ERROR);
        self.failed.set(true);
    }

    // ── Prompt ────────────────────────────────────────────────────────────────

    fn build_prompt(&self) -> String {
        format!("[{}] ", self.current_path())
    }

    /// The current drive and directory as OS/2 writes them: `C:\`,
    /// `C:\SUB\DIR`.
    fn current_path(&self) -> String {
        let dm = self.shared.drive_mgr.lock_or_recover();
        let drive = dm.current_disk();
        format!("{}:\\{}", (b'A' + drive) as char, dm.current_dir_os2(drive))
    }

    // ── Command dispatch ─────────────────────────────────────────────────────

    /// Execute one command line. Returns `Some(exit_code)` when the shell
    /// should terminate, or `None` to keep looping.
    fn execute_line(&mut self, line: &str) -> Option<u32> {
        // `@` only keeps a script's line from being echoed.
        let line = line.trim().trim_start_matches('@').trim_start();
        // A label (`:name`, which `::` comments are too) does nothing, and
        // `REM` takes the rest of the line, `&` and `|` included.
        if line.is_empty() || line.starts_with(':') || is_remark(line) { return None; }
        let line = self.expand_line(line);
        for (link, command) in split_chain(&line) {
            // GOTO leaves the rest of its line.
            if self.jumped() { break; }
            let run = match link {
                Chain::Then => true,
                Chain::And => self.last_rc.get() == 0,
                Chain::Or => self.last_rc.get() != 0,
            };
            if !run { continue; }
            if let Some(code) = self.execute_pipeline(parse_command(&command)) {
                return Some(code);
            }
        }
        None
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
                self.last_rc.set(RC_BUILTIN_FAILED);
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
                    self.last_rc.set(RC_BUILTIN_FAILED);
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
            let content = host_path.map(std::fs::read);
            match content {
                Some(Ok(bytes)) => Some(InputSource::Data(bytes.into())),
                Some(Err(e)) => {
                    self.writeln_attr(&format!("Cannot read '{}': {e}", path), ATTR_ERROR);
                    self.last_rc.set(RC_BUILTIN_FAILED);
                    return None;
                }
                None => {
                    self.writeln_attr(&format!("File not found: {}", path), ATTR_ERROR);
                    self.last_rc.set(RC_BUILTIN_FAILED);
                    return None;
                }
            }
        } else {
            None
        };

        // The redirections hold for this command only: what was there
        // before (the screen, the keyboard, or a script's caller's file or
        // pipe) comes back after it.
        let mut saved_input = initial_stdin.map(|src| self.input_source.replace(src));
        let saved_output = final_sink.map(|sink| self.output_sink.replace(sink));

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
                // The captured output is the next command's input.
                let previous = self.input_source.replace(InputSource::Data(captured.into()));
                saved_input.get_or_insert(previous);
            } else {
                result = self.execute_segment(segment);
            }

            if result.is_some() { break; }
        }

        if let Some(output) = saved_output { self.output_sink.replace(output); }
        if let Some(input) = saved_input { self.input_source.replace(input); }

        result
    }

    /// Execute a single (non-pipeline) command segment.
    fn execute_segment(&mut self, line: &str) -> Option<u32> {
        let tokens = tokenize(line);
        if tokens.is_empty() { return None; }

        let cmd = tokens[0].to_uppercase();
        let args: Vec<&str> = tokens[1..].to_vec();

        debug!("cmd: cmd='{}' args={:?}", cmd, args);

        // A built-in leaves 0, or `RC_BUILTIN_FAILED` after an error;
        // a program its exit code (`exec_external`).
        self.failed.set(false);
        let builtin_rc = |shell: &Self| shell.last_rc.set(if shell.failed.get() { RC_BUILTIN_FAILED } else { 0 });
        // What follows the command's name, as typed.
        let rest = first_word(line).1;
        let result = match cmd.as_str() {
            "REM" => None,
            // The text after one separator, blanks kept.
            "ECHO" => {
                let after = &line.trim_start()[tokens[0].len()..];
                self.cmd_echo(after.strip_prefix([' ', '\t']).unwrap_or(after));
                None
            }
            // `ECHO.` writes an empty line, `ECHO.text` the text.
            _ if cmd.starts_with("ECHO.") => { self.writeln(&line.trim_start()["ECHO.".len()..]); None }
            "GOTO" => { self.cmd_goto(rest); None }
            "SHIFT" => { self.cmd_shift(); None }
            "SETLOCAL" => { self.cmd_setlocal(); None }
            "ENDLOCAL" => { self.end_local(); None }
            "PAUSE" => { self.cmd_pause(); None }
            // These leave the return code of the command they run.
            "IF" => return self.cmd_if(rest),
            "FOR" => return self.cmd_for(rest),
            "CALL" => return self.cmd_call(rest),
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
                    self.error("Invalid drive.");
                }
                None
            }
            // External program or .cmd script: it sets the return code.
            _ => return self.exec_external(tokens[0], &args, false),
        };
        builtin_rc(self);
        result
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
        // OS/2 passes one argument string: the host's arguments (`/C`,
        // `DIR C:\`) joined by blanks, as a shell would have typed them.
        let mut args: Vec<String> = vec!["CMD.EXE".to_string()];
        if !host_args.is_empty() { args.push(host_args.join(" ")); }
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
    use super::builtins::ver_line;

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
        assert!(screen.contains(&ver_line()), "VER names the release version: {screen:?}");
    }

    #[test]
    fn test_ver_line_ends_with_the_release_version() {
        let line = ver_line();
        let version = line.rsplit(' ').next().unwrap();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        let parts: Vec<&str> = version.split('.').collect();
        assert_eq!(parts.len(), 3, "a release version is X.Y.Z: {version}");
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()), "numeric: {version}");
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


    /// The output a shell wrote into its buffer.
    fn buffered(shell: &CmdShell) -> String {
        match &*shell.output_sink.borrow() {
            OutputSink::Buffer(b) => String::from_utf8_lossy(b).into_owned(),
            _ => unreachable!("the test set a buffer"),
        }
    }

    /// A built-in leaves 0, or 1 after an error; `&&` and `||` follow it;
    /// a `REM` line is skipped whole.
    #[test]
    fn builtins_leave_return_codes_for_and_and_or() {
        let (loader, _drive) = loader_with_drive_c();
        let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
        *shell.output_sink.borrow_mut() = OutputSink::Buffer(Vec::new());
        assert_eq!(shell.execute_line("cd NOSUCH && echo wrong || echo right"), None);
        assert_eq!(shell.last_rc.get(), 0, "ECHO worked");
        shell.execute_line("cd NOSUCH");
        assert_eq!(shell.last_rc.get(), RC_BUILTIN_FAILED);
        shell.execute_line("md SUB & cd SUB && echo in-sub");
        assert_eq!(shell.last_rc.get(), 0);
        shell.execute_line("REM x & echo remark-ran");
        shell.execute_line(":label & echo label-ran");
        let text = buffered(&shell);
        assert!(text.contains("right") && text.contains("in-sub"), "{text}");
        for wrong in ["wrong", "remark-ran", "label-ran"] {
            assert!(!text.contains(wrong), "{wrong}: {text}");
        }
        assert_eq!(shell.current_path(), "C:\\SUB");
    }

    /// A script writes where its caller's output goes (`S.CMD > file`),
    /// and the shell's own output comes back to the screen afterwards.
    #[test]
    fn a_scripts_output_follows_its_redirection() {
        let (loader, drive) = loader_with_drive_c();
        std::fs::write(drive.path().join("S.CMD"), b"@echo off\r\necho from-script\r\necho second | echo third\r\n").unwrap();
        let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
        shell.execute_line("S.CMD > OUT.TXT");
        let file = std::fs::read_to_string(drive.path().join("OUT.TXT")).unwrap();
        assert_eq!(file, "from-script\r\nthird\r\n");
        assert!(matches!(&*shell.output_sink.borrow(), OutputSink::Vio), "the screen again");
        assert!(matches!(&*shell.input_source.borrow(), InputSource::Kbd), "the keyboard again");
    }

    /// `/C` ends the shell with the command's return code, or EXIT's.
    #[test]
    fn slash_c_returns_the_commands_return_code() {
        let (loader, _drive) = loader_with_drive_c();
        let run = |line: &str| {
            let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
            *shell.output_sink.borrow_mut() = OutputSink::Buffer(Vec::new());
            shell.run(&["CMD.EXE".into(), format!("/C {line}")])
        };
        assert_eq!(run("cd NOSUCH"), RC_BUILTIN_FAILED);
        assert_eq!(run("echo fine"), 0);
        assert_eq!(run("exit 9"), 9);
        assert_eq!(run("cd NOSUCH & exit 4"), 4);
    }

    /// A bare name is looked for in the current directory, then along
    /// PATH, `.EXE` before `.CMD`; a name with a directory only there.
    #[test]
    fn programs_are_found_in_the_current_directory_then_along_path() {
        let (loader, drive) = loader_with_drive_c();
        std::fs::create_dir_all(drive.path().join("BIN")).unwrap();
        std::fs::create_dir_all(drive.path().join("WORK")).unwrap();
        std::fs::write(drive.path().join("BIN/TOOL.EXE"), b"x").unwrap();
        std::fs::write(drive.path().join("BIN/TOOL.CMD"), b"x").unwrap();
        std::fs::write(drive.path().join("BIN/ONLY.CMD"), b"x").unwrap();
        std::fs::write(drive.path().join("WORK/TOOL.EXE"), b"x").unwrap();
        let shell = CmdShell::new(Arc::clone(&loader.shared), parse_env(&["PATH=C:\\NONE;C:\\BIN"]));
        let found = |name: &str| shell.find_program(name)
            .map(|(p, script)| (p.strip_prefix(drive.path()).unwrap().display().to_string(), script));
        assert_eq!(found("tool"), Some(("BIN/TOOL.EXE".into(), false)), "along PATH, .EXE first");
        assert_eq!(found("only"), Some(("BIN/ONLY.CMD".into(), true)));
        assert_eq!(found("tool.cmd"), Some(("BIN/TOOL.CMD".into(), true)));
        assert_eq!(found("WORK\\TOOL"), Some(("WORK/TOOL.EXE".into(), false)), "a directory given");
        assert_eq!(found("NONE\\TOOL"), None, "only there");
        loader.shared.drive_mgr.lock_or_recover().set_current_dir("C:\\WORK").unwrap();
        assert_eq!(found("tool"), Some(("WORK/TOOL.EXE".into(), false)), "the current directory first");
        assert_eq!(found("bin"), None, "a directory is no program");
    }

    /// A pipe's bytes are lines for the shell's own reading.
    #[test]
    fn pipe_data_reads_as_lines() {
        let loader = Loader::new_mock();
        let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
        *shell.input_source.borrow_mut() = InputSource::Data(b"one\r\ntwo\nlast".to_vec().into());
        for want in ["one", "two", "last"] {
            assert!(matches!(shell.read_line(), ReadResult::Line(l) if l == want), "{want}");
        }
        assert!(matches!(shell.read_line(), ReadResult::Eof));
    }

    /// The captured output of a command is bounded.
    #[test]
    fn captured_output_is_bounded() {
        let loader = Loader::new_mock();
        let shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
        *shell.output_sink.borrow_mut() = OutputSink::Buffer(Vec::new());
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..(MAX_CAPTURED_OUTPUT / chunk.len() + 2) { shell.write_out(&chunk); }
        assert_eq!(buffered(&shell).len(), MAX_CAPTURED_OUTPUT);
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
