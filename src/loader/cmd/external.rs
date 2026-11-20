// SPDX-License-Identifier: GPL-3.0-or-later
//
// The built-in CMD.EXE running programs and .CMD scripts: a program is a
// child Warpine process; a script runs line by line in a shell that shares
// this one's environment.

use super::*;

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
        let dm = self.shared.drive_mgr.lock_or_recover();
        dirs.iter().flat_map(|dir| candidates.iter().map(move |c| format!("{dir}{c}")))
            .find_map(|os2_path| dm.resolve_to_host_path(&os2_path).filter(|p| p.is_file()))
            .map(|p| {
                let is_script = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd"));
                (p, is_script)
            })
    }

    /// Spawn `warpine <prog_path> [args...]` as a child process and relay its
    /// stdout through the current output sink. A pipe or `< file` before it
    /// is its standard input; else it shares the shell's. Returns its
    /// return code.
    pub(super) fn spawn_external(&self, prog_path: &std::path::Path, args: &[&str]) -> u32 {
        let warpine_exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                self.error(&format!("Cannot locate warpine executable: {e}"));
                return RC_BUILTIN_FAILED;
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
                return RC_BUILTIN_FAILED;
            }
            Err(_) => {
                self.error("Cannot start another process: too many nested or running processes");
                return RC_BUILTIN_FAILED;
            }
        };

        // The shell's current drive and directories (OS/2 paths; the
        // host's working directory is not involved).
        let cwd = crate::loader::inherit::cwd_strings(&self.shared.drive_mgr.lock_or_recover(), None);
        cmd.env(crate::loader::inherit::CWD_ENV, crate::loader::inherit::escape(&cwd));

        cmd.stdout(std::process::Stdio::piped())
           .stderr(std::process::Stdio::inherit());

        // The bytes of a pipe or `< file` go to the program's standard
        // input, which then ends; they are used up either way.
        let input = match &mut *self.input_source.borrow_mut() {
            InputSource::Data(data) => Some(Vec::from(std::mem::take(data))),
            InputSource::Kbd => None,
        };
        if input.is_some() { cmd.stdin(std::process::Stdio::piped()); }

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
