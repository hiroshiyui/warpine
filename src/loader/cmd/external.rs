// SPDX-License-Identifier: GPL-3.0-or-later
//
// The built-in CMD.EXE running programs and .CMD scripts: a program is a
// child Warpine process; a script runs line by line in a shell that shares
// this one's environment.

use super::*;

/// How often a detached program is checked for its end.
const DETACHED_POLL: Duration = Duration::from_millis(200);

/// The return code of a program that ended without an exit code (killed by
/// a signal, or after a trap): Warpine's choice, unverified (ADR-0077).
const RC_PROGRAM_KILLED: u32 = 255;

impl CmdShell {
    // ── External execution ────────────────────────────────────────────────────

    /// Run a program or a `.CMD` script (`cmd` as typed); its return code
    /// becomes the shell's (`RC_BUILTIN_FAILED` when it cannot be found or
    /// started). `call`: CALL ran it, so a script comes back to this one.
    /// Returns `Some(code)` when the shell must end (EXIT in a script).
    pub(super) fn exec_external(&self, cmd: &str, args: &[&str], call: bool) -> Option<u32> {
        let rc = match self.find_program(cmd) {
            None => {
                self.error(&format!("'{cmd}' is not recognized as an internal or external command."));
                RC_BUILTIN_FAILED
            }
            Some((p, true)) => {
                let end = self.run_nested_script(&p, cmd, args, call);
                if end.exit { return Some(end.rc); }
                end.rc
            }
            Some((p, false)) => self.spawn_external(&p, args),
        };
        self.last_rc.set(rc);
        None
    }

    /// Find the program or script `name` names: a name with a drive or a
    /// directory only there; a bare name in the current directory, then in
    /// each directory of `PATH`. Without an extension, `.EXE` is tried
    /// before `.CMD`. Returns its host path and whether it is a script.
    pub(super) fn find_program(&self, name: &str) -> Option<(std::path::PathBuf, bool)> {
        self.find_program_with_name(name).map(|(path, script, _)| (path, script))
    }

    /// `find_program`, with the OS/2 path it was found at (fully
    /// qualified: `C:\BIN\TOOL.CMD`).
    pub(super) fn find_program_with_name(&self, name: &str) -> Option<(std::path::PathBuf, bool, String)> {
        let has_dir = name.contains(['\\', '/', ':']);
        let file = name.rsplit(['\\', '/', ':']).next().unwrap_or(name);
        let candidates: Vec<String> = if file.contains('.') {
            vec![name.to_string()]
        } else {
            vec![format!("{name}.EXE"), format!("{name}.CMD")]
        };
        let mut dirs = vec![String::new()];
        if !has_dir {
            let path = self.env.borrow().iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("PATH")).map(|(_, v)| v.clone()).unwrap_or_default();
            dirs.extend(path.split(';').map(str::trim).filter(|d| !d.is_empty())
                .map(|d| if d.ends_with(['\\', '/']) { d.to_string() } else { format!("{d}\\") }));
        }
        let current = self.current_path();
        let dm = self.shared.drive_mgr.lock_or_recover();
        dirs.iter().flat_map(|dir| candidates.iter().map(move |c| format!("{dir}{c}")))
            .find_map(|os2_path| dm.resolve_to_host_path(&os2_path).filter(|p| p.is_file()).map(|p| (p, os2_path)))
            .map(|(p, os2_path)| {
                let is_script = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd"));
                // A relative name is in the current directory.
                let full = if os2_path.contains(':') { os2_path } else if os2_path.starts_with('\\') {
                    format!("{}{os2_path}", &current[..2])
                } else if current.ends_with('\\') { format!("{current}{os2_path}") } else { format!("{current}\\{os2_path}") };
                (p, is_script, full.to_ascii_uppercase())
            })
    }

    /// The host command that runs `prog_path` with `args` as a child
    /// Warpine process: the shell's environment and current directories,
    /// and an admission under the session's limits (nesting, children,
    /// THREADS=). `None`, with an error written, when it may not start.
    fn child_command(&self, prog_path: &std::path::Path, args: &[&str])
                     -> Option<(std::process::Command, crate::loader::inherit::Admission)> {
        let warpine_exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                self.error(&format!("Cannot locate warpine executable: {e}"));
                return None;
            }
        };

        let mut cmd = std::process::Command::new(&warpine_exe);
        cmd.arg(prog_path);
        for a in args { cmd.arg(a); }
        // What this process got from its own parent is not the child's;
        // the child's environment is the shell's.
        crate::loader::inherit::clear_parent_vars(&mut cmd);
        let codepage = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        cmd.env(crate::loader::inherit::ENV_ENV, crate::loader::inherit::escape(&env_block(&self.env.borrow(), codepage)));
        // Bounded nesting and number of children, and a slot of the
        // session's THREADS= limit (inherit.rs).
        let slot = match crate::loader::inherit::admit_child(&self.shared, &mut cmd) {
            Ok(slot) => slot,
            Err(crate::loader::inherit::ChildLimit::Threads) => {
                self.error("Cannot start another process: the THREADS= limit is reached");
                return None;
            }
            Err(_) => {
                self.error("Cannot start another process: too many nested or running processes");
                return None;
            }
        };

        // The shell's current drive and directories (OS/2 paths; the
        // host's working directory is not involved).
        let cwd = crate::loader::inherit::cwd_strings(&self.shared.drive_mgr.lock_or_recover(), None);
        cmd.env(crate::loader::inherit::CWD_ENV, crate::loader::inherit::escape(&cwd));
        Some((cmd, slot))
    }

    /// `DETACH program [args]`: run a program in the background, without
    /// a console (no standard input or output), and go on at once. It
    /// counts as a child of the shell until it ends, so the session's
    /// limits hold. Writes its process id.
    pub(super) fn cmd_detach(&self, args: &[&str]) {
        let Some((&name, args)) = args.split_first() else {
            self.error("The syntax of the command is incorrect.");
            return;
        };
        let path = match self.find_program(name) {
            None => return self.error(&format!("'{name}' is not recognized as a program.")),
            Some((_, true)) => return self.error("DETACH runs programs, not scripts."),
            Some((path, false)) => path,
        };
        let Some((mut cmd, slot)) = self.child_command(&path, args) else { return };
        cmd.stdin(std::process::Stdio::null())
           .stdout(std::process::Stdio::null())
           .stderr(std::process::Stdio::null());
        if let Some(pid) = self.start_in_background(cmd, slot, name) {
            self.writeln(&format!("The Process Identification Number is {pid}."));
        }
    }

    /// Start `cmd` and go on without waiting: it counts as one of the
    /// shell's children (the session's limits hold) until a reaper thread
    /// sees it end. Its process id, or `None` with an error written.
    fn start_in_background(&self, mut cmd: std::process::Command, slot: crate::loader::inherit::Admission,
                           name: &str) -> Option<u32> {
        let pid = slot.pid;
        let spawned = cmd.spawn();
        crate::loader::inherit::child_started(&self.shared, slot, spawned.as_ref().ok().map(std::process::Child::id));
        let child = match spawned {
            Ok(child) => child,
            Err(e) => {
                self.error(&format!("Failed to start {name}: {e}"));
                return None;
            }
        };
        self.shared.process_mgr.lock_or_recover().insert_child(pid, child);
        let shared = Arc::clone(&self.shared);
        std::thread::spawn(move || loop {
            std::thread::sleep(DETACHED_POLL);
            let mut procs = shared.process_mgr.lock_or_recover();
            let Some(child) = procs.children.get_mut(&pid) else { return };
            if !matches!(child.try_wait(), Ok(None)) {
                procs.take_child(pid);
                return;
            }
        });
        Some(pid)
    }

    /// `START ["title"] [/C | /K | /N] [/F | /B] [/FS | /WIN | /PM] [/MAX | /MIN] [/I] [/PGM]
    /// [command]`: start a session of its own and go on (ADR-0082).
    ///
    /// - No command, `/K command`: a new CMD.EXE session (that stays).
    /// - `/C command`, or a command that is not a program: CMD.EXE runs it
    ///   and the session ends.
    /// - A program (or `/N`, `/PGM`): the program itself.
    /// - `/FS` full screen, `/WIN` (default) in a window on the desktop;
    ///   the session keeps the console, so on a terminal it opens its own
    ///   window. `/I`: the shell's environment, else Warpine's default.
    /// - `/F`, `/B`, `/MAX`, `/MIN`, `/PM` and the title are accepted and
    ///   not acted on (unverified).
    pub(super) fn cmd_start(&self, rest: &str) {
        let mut rest = rest.trim_start();
        let mut keep = None; // Some(true) /K, Some(false) /C
        let (mut direct, mut inherit_env, mut fullscreen) = (false, false, false);
        loop {
            if rest.starts_with('"') {
                // The title.
                match rest[1..].find('"') {
                    Some(end) => rest = rest[end + 2..].trim_start(),
                    None => rest = "",
                }
                continue;
            }
            let (word, after) = first_word(rest);
            if !word.starts_with('/') { break; }
            match word.to_ascii_uppercase().as_str() {
                "/C" => keep = Some(false),
                "/K" => keep = Some(true),
                "/N" | "/PGM" => direct = true,
                "/I" => inherit_env = true,
                "/FS" => fullscreen = true,
                "/WIN" => fullscreen = false,
                "/F" | "/B" | "/MAX" | "/MIN" | "/PM" => {}
                "/DOS" => return self.error("DOS sessions are not supported."),
                _ => return self.error(&format!("Invalid switch: {word}")),
            }
            rest = after;
        }
        let command = rest.trim();
        let (word, _) = first_word(command);
        let program = if command.is_empty() || keep.is_some() { None } else {
            match self.find_program(word) {
                Some((path, false)) => Some(path),
                _ if direct => return self.error(&format!("'{word}' is not recognized as a program.")),
                _ => None,
            }
        };
        let tokens = tokenize(command);
        let (path, args): (std::path::PathBuf, Vec<String>) = match program {
            Some(path) => (path, tokens.iter().skip(1).map(|t| t.to_string()).collect()),
            None => {
                // CMD.EXE (the built-in, by name) with /C or /K.
                let mut args = Vec::new();
                if !command.is_empty() {
                    args.push(if keep == Some(true) { "/K".into() } else { "/C".into() });
                    args.push(command.to_string());
                }
                (std::path::PathBuf::from("CMD.EXE"), args)
            }
        };
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let Some((mut cmd, slot)) = self.child_command(&path, &arg_refs) else { return };
        if !inherit_env {
            let codepage = self.shared.active_codepage.load(Ordering::Relaxed);
            let default = parse_env(DEFAULT_SHELL_ENV);
            cmd.env(crate::loader::inherit::ENV_ENV, crate::loader::inherit::escape(&env_block(&default, codepage)));
        }
        // A session of its own: no GUI script of this one's (tests), the
        // console as it is, no input of this shell's.
        cmd.env_remove(crate::gui::gui_script::SCRIPT_ENV);
        cmd.env(crate::gui::vio_window::VIO_ENV, if fullscreen { "fullscreen" } else { "window" });
        cmd.stdin(std::process::Stdio::null());
        let _ = self.start_in_background(cmd, slot, word);
    }

    /// Spawn `warpine <prog_path> [args...]` as a child process and relay its
    /// stdout through the current output sink. A pipe or `< file` before it
    /// is its standard input; else it shares the shell's. Returns its
    /// return code.
    pub(super) fn spawn_external(&self, prog_path: &std::path::Path, args: &[&str]) -> u32 {
        let Some((mut cmd, slot)) = self.child_command(prog_path, args) else { return RC_BUILTIN_FAILED };

        cmd.stdout(std::process::Stdio::piped())
           .stderr(std::process::Stdio::inherit());

        // The bytes of a pipe or `< file` go to the program's standard
        // input, which then ends; they are used up either way. The output
        // of a program before it in the pipeline is its input directly.
        let input = match std::mem::replace(&mut *self.input_source.borrow_mut(), InputSource::Kbd) {
            InputSource::Data(data) => {
                cmd.stdin(std::process::Stdio::piped());
                Some(Vec::from(data))
            }
            InputSource::Pipe(pipe) => {
                cmd.stdin(std::process::Stdio::from(pipe));
                None
            }
            InputSource::Kbd => None,
        };
        // Feeding the next command: it reads this program's output while
        // this one runs (ADR-0082).
        let feeds_next = self.feeds_next.get();

        let to_vio = matches!(&*self.output_sink.borrow(), OutputSink::Vio);

        let spawned = cmd.spawn();
        crate::loader::inherit::child_started(&self.shared, slot,
                                      spawned.as_ref().ok().map(std::process::Child::id));
        match spawned {
            Err(e) => {
                self.error(&format!("Failed to spawn program: {e}"));
                RC_BUILTIN_FAILED
            }
            Ok(mut child) => {
                // Written on a thread of its own: the program may read
                // little of it, or write a lot before it reads.
                let feeder = child.stdin.take().zip(input).map(|(mut stdin, data)| std::thread::spawn(move || {
                    // A program that ends before it read everything closes
                    // the pipe; what is left is dropped.
                    let _ = stdin.write_all(&data);
                }));
                if feeds_next {
                    // The pipeline waits for it at its end; the feeder ends
                    // with the program's input.
                    *self.piped_out.borrow_mut() = child.stdout.take();
                    self.running.borrow_mut().push(child);
                    return 0;
                }
                let status = if to_vio {
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
                    let status = self.wait_child(&mut child);
                    if let Some(t) = relay { let _ = t.join(); }
                    status
                } else {
                    // Buffer all output then write to sink (read on a
                    // thread of its own, so that the wait stays
                    // interruptible).
                    let reader = child.stdout.take().map(|mut pipe| std::thread::spawn(move || {
                        let mut output = Vec::new();
                        let mut buf = [0u8; 4096];
                        // To its end, keeping at most MAX_CAPTURED_OUTPUT.
                        while let Ok(n) = pipe.read(&mut buf) && n > 0 {
                            let room = MAX_CAPTURED_OUTPUT.saturating_sub(output.len());
                            output.extend_from_slice(&buf[..n.min(room)]);
                        }
                        output
                    }));
                    let status = self.wait_child(&mut child);
                    let output = reader.and_then(|t| t.join().ok()).unwrap_or_default();
                    if !output.is_empty() {
                        self.write_out(&output);
                    }
                    status
                };
                if let Some(t) = feeder { let _ = t.join(); }
                status.and_then(|s| s.code()).map_or(RC_PROGRAM_KILLED, |c| c as u32)
            }
        }
    }

    /// Wait for `child` to end. When the shell's call is interrupted (a
    /// signal, a kill) or its process ends, the child is asked to end
    /// (`SIGTERM`: its exit list runs) and, after `KILL_GRACE`, killed.
    /// Returns how it ended (`None` if that cannot be known).
    pub(super) fn wait_child(&self, child: &mut std::process::Child) -> Option<std::process::ExitStatus> {
        let mut asked: Option<std::time::Instant> = None;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Err(_) => return None,
                Ok(None) => {}
            }
            let stop = crate::loader::interrupt::call_interrupted(&self.shared)
                || self.shared.exit_requested.load(Ordering::Relaxed);
            if stop && asked.is_none() {
                if let Ok(pid) = i32::try_from(child.id()) {
                    // SAFETY: kill(2) on the pid of a child this shell
                    // started and has not yet reaped; no memory involved.
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
                asked = Some(std::time::Instant::now());
            }
            if asked.is_some_and(|t| t.elapsed() >= crate::loader::signals::KILL_GRACE) {
                let _ = child.kill();
                return child.wait().ok();
            }
            std::thread::sleep(CHILD_POLL);
        }
    }
}
