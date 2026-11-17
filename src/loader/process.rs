// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 process management API implementations.
// DosExecPgm, DosWaitChild, DosKillProcess, DosQueryAppType.

use std::process::Command;
use std::io::Read;
use std::sync::Arc;
use log::{debug, warn};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::doscalls::HandleTarget;

/// What a child process gets for one of its standard handles.
enum ChildStdio {
    /// The parent's console: the child's output is relayed into it.
    Console,
    /// A host file or pipe the parent's handle was redirected to.
    Host(std::process::Stdio),
    /// The parent closed the handle.
    Closed,
}

impl super::Loader {
    /// What the child of `DosExecPgm` inherits for standard handle `hf`: a
    /// handle the parent redirected (`DosDupHandle(hFile, &1)`, as shells do
    /// for `prog > file`) is passed on as the host file or pipe.
    fn child_stdio(&self, hf: u32) -> ChildStdio {
        match self.handle_target(hf) {
            Some(HandleTarget::Console(_)) => ChildStdio::Console,
            Some(HandleTarget::File) => match self.shared.drive_mgr.lock_or_recover().clone_host_file(hf) {
                Some(f) => ChildStdio::Host(f.into()),
                None => ChildStdio::Console,
            },
            Some(HandleTarget::Pipe) => match self.shared.handle_mgr.lock_or_recover().get(hf).and_then(|f| f.try_clone().ok()) {
                Some(f) => ChildStdio::Host(f.into()),
                None => ChildStdio::Console,
            },
            None => ChildStdio::Closed,
        }
    }

    /// Set up the child's standard handles: input and error output are
    /// inherited from the host unless redirected; output is piped back for
    /// relaying into the parent's console unless redirected.
    fn set_child_stdio(&self, cmd: &mut Command) {
        use std::process::Stdio;
        match self.child_stdio(0) {
            ChildStdio::Host(s) => { cmd.stdin(s); }
            ChildStdio::Closed => { cmd.stdin(Stdio::null()); }
            ChildStdio::Console => {}
        }
        match self.child_stdio(STDOUT_HANDLE) {
            ChildStdio::Host(s) => { cmd.stdout(s); }
            ChildStdio::Closed => { cmd.stdout(Stdio::null()); }
            ChildStdio::Console => { cmd.stdout(Stdio::piped()); }
        }
        match self.child_stdio(2) {
            ChildStdio::Host(s) => { cmd.stderr(s); }
            ChildStdio::Closed => { cmd.stderr(Stdio::null()); }
            ChildStdio::Console => { cmd.stderr(Stdio::inherit()); }
        }
    }

    /// DosExecPgm (ordinal 283): execute a program.
    ///
    /// OS/2 signature: DosExecPgm(pObjname, cbObjname, execFlag, pArg, pEnv, pRes, pName)
    /// - execFlag: 0=EXEC_SYNC, 1=EXEC_ASYNC, 2=EXEC_ASYNCRESULT, 3=EXEC_TRACE, 4=EXEC_BACKGROUND
    /// - pArg: double-null-terminated string (program name \0 args \0\0)
    /// - pRes: pointer to RESULTCODES struct (codeTerminate, codeResult)
    /// - pName: program path
    #[allow(clippy::too_many_arguments)]
    pub fn dos_exec_pgm(&self, p_objname: u32, cb_objname: u32, exec_flag: u32,
                         p_arg: u32, _p_env: u32, p_res: u32, p_name: u32) -> u32 {
        let prog_name = self.read_guest_string(p_name);
        debug!("  DosExecPgm(prog='{}', execFlag={})", prog_name, exec_flag);

        // Intercept CMD.EXE / OS2SHELL.EXE → run builtin host-Rust shell
        {
            let base = prog_name.split(['\\', '/']).next_back()
                .unwrap_or(&prog_name).to_ascii_uppercase();
            if base == "CMD.EXE" || base == "OS2SHELL.EXE" {
                debug!("  DosExecPgm: intercepted CMD.EXE → builtin shell");
                return self.run_builtin_cmd(p_arg, p_res);
            }
        }

        // Parse the double-null-terminated argument string
        let args = if p_arg != 0 {
            self.parse_double_null_string(p_arg)
        } else {
            Vec::new()
        };
        debug!("    args: {:?}", args);

        // Resolve the program path via the DriveManager VFS (handles C:\ → host path,
        // case-insensitive lookup, per-drive current directory, etc.).
        let prog_path = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            match dm.resolve_to_host_path(&prog_name) {
                Some(p) => p,
                None => {
                    warn!("  DosExecPgm: cannot resolve '{}' via VFS", prog_name);
                    if p_objname != 0 && cb_objname > 0 {
                        let bytes = prog_name.as_bytes();
                        let n = bytes.len().min(cb_objname as usize - 1);
                        self.guest_write_bytes(p_objname, &bytes[..n]);
                        self.guest_write::<u8>(p_objname + n as u32, 0);
                    }
                    return ERROR_FILE_NOT_FOUND;
                }
            }
        };

        // Find the warpine executable path (ourselves)
        let warpine_exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => {
                warn!("  DosExecPgm: cannot find warpine executable");
                return ERROR_FILE_NOT_FOUND;
            }
        };

        // Build command: warpine <child.exe> [args...]
        let mut cmd = Command::new(&warpine_exe);
        cmd.arg(&prog_path);

        // Add arguments (skip the first element which is the program name)
        if args.len() > 1 {
            // The second element contains the command line arguments
            let arg_str = &args[1];
            if !arg_str.is_empty() {
                // Split by spaces for individual args
                for a in arg_str.split_whitespace() {
                    cmd.arg(a);
                }
            }
        }

        // Inherit the current working directory
        let cwd = {
            let proc_mgr = self.shared.process_mgr.lock_or_recover();
            let dir = proc_mgr.current_dir.replace('\\', "/").trim_start_matches('/').to_string();
            if dir.is_empty() {
                std::env::current_dir().ok()
            } else {
                std::env::current_dir().ok().map(|base| base.join(&dir))
            }
        };
        if let Some(ref cwd) = cwd
            && cwd.is_dir() {
                cmd.current_dir(cwd);
        }

        match exec_flag {
            0 => {
                // EXEC_SYNC: pipe only stdout so the child's program output
                // appears in the same SDL text window (or terminal) as the
                // parent (e.g. 4OS2).  stderr is left inherited so warpine's
                // own diagnostic log messages continue going to the terminal
                // and do NOT clutter the SDL window.
                // The child detects the piped stdout via isatty() and runs in
                // headless mode automatically — no env var required.
                // Standard handles the parent redirected go to the same file
                // or pipe instead.
                self.set_child_stdio(&mut cmd);

                match cmd.spawn() {
                    Ok(mut child) => {
                        let stdout_pipe = child.stdout.take();

                        let shared_out = self.shared.clone();
                        let stdout_thread = stdout_pipe.map(move |pipe| {
                            std::thread::spawn(move || relay_pipe_to_vio(pipe, shared_out))
                        });

                        let status = child.wait();

                        if let Some(t) = stdout_thread { let _ = t.join(); }

                        match status {
                            Ok(status) => {
                                let exit_code = status.code().unwrap_or(1) as u32;
                                if p_res != 0 {
                                    self.guest_write::<u32>(p_res, 0);
                                    self.guest_write::<u32>(p_res + 4, exit_code);
                                }
                                NO_ERROR
                            }
                            Err(e) => {
                                warn!("  DosExecPgm: wait failed: {}", e);
                                ERROR_FILE_NOT_FOUND
                            }
                        }
                    }
                    Err(e) => {
                        warn!("  DosExecPgm: spawn failed: {}", e);
                        if p_objname != 0 && cb_objname > 0 {
                            let bytes = prog_name.as_bytes();
                            let n = bytes.len().min(cb_objname as usize - 1);
                            self.guest_write_bytes(p_objname, &bytes[..n]);
                            self.guest_write::<u8>(p_objname + n as u32, 0);
                        }
                        ERROR_FILE_NOT_FOUND
                    }
                }
            }
            1 | 2 | 4 => {
                // EXEC_ASYNC / EXEC_ASYNCRESULT / EXEC_BACKGROUND: pipe stdout
                // so the child detects it's not a terminal and skips SDL.
                // stderr inherits so warpine diagnostics reach the terminal.
                // A background relay thread forwards the child's stdout to the
                // parent's VioManager so the output appears in the same SDL
                // text window as the parent (e.g. 4OS2). Redirected standard
                // handles go to their file or pipe instead.
                self.set_child_stdio(&mut cmd);
                match cmd.spawn() {
                    Ok(mut child) => {
                        let stdout_pipe = child.stdout.take();
                        let shared_out = self.shared.clone();
                        let mut proc_mgr = self.shared.process_mgr.lock_or_recover();
                        let pid = proc_mgr.add_child(child);
                        if let Some(pipe) = stdout_pipe {
                            let relay = std::thread::spawn(move || relay_pipe_to_vio(pipe, shared_out));
                            proc_mgr.add_relay_thread(pid, relay);
                        }
                        drop(proc_mgr);
                        if p_res != 0 {
                            self.guest_write::<u32>(p_res, 0);       // codeTerminate (unused for async)
                            self.guest_write::<u32>(p_res + 4, pid); // PID
                        }
                        NO_ERROR
                    }
                    Err(e) => {
                        warn!("  DosExecPgm: spawn failed: {}", e);
                        ERROR_FILE_NOT_FOUND
                    }
                }
            }
            _ => {
                warn!("  DosExecPgm: unsupported execFlag={}", exec_flag);
                ERROR_INVALID_FUNCTION
            }
        }
    }

    /// DosWaitChild (ordinal 280): wait for a child process.
    ///
    /// OS/2 signature: DosWaitChild(action, option, pRes, ppid, pid)
    /// - action: 0=DCWA_PROCESS (specific pid), 1=DCWA_PROCESSTREE (any child)
    /// - option: 0=DCWW_WAIT, 1=DCWW_NOWAIT
    /// - pRes: PRESULTCODES — output: exit info
    /// - ppid: PPID * — output: PID of the child that terminated
    /// - pid:  PID — input: PID to wait for (DCWA_PROCESS only)
    pub fn dos_wait_child(&self, action: u32, option: u32, p_res: u32, p_pid: u32, pid: u32) -> u32 {
        let target_pid = pid;   // 5th arg is the input PID (p_pid is an OUTPUT pointer)
        debug!("  DosWaitChild(action={}, option={}, target_pid={})", action, option, target_pid);

        if action == 0 && target_pid != 0 {
            // DCWA_PROCESS: wait for specific child
            let child = self.shared.process_mgr.lock_or_recover().take_child(target_pid);
            if let Some(mut child) = child {
                if option == 1 {
                    // DCWW_NOWAIT
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let code = status.code().unwrap_or(1) as u32;
                            if p_res != 0 {
                                self.guest_write::<u32>(p_res, 0);
                                self.guest_write::<u32>(p_res + 4, code);
                            }
                            self.guest_write::<u32>(p_pid, target_pid);
                            NO_ERROR
                        }
                        Ok(None) => {
                            // Not done yet — put it back
                            self.shared.process_mgr.lock_or_recover().children.insert(target_pid, child);
                            128 // ERROR_CHILD_NOT_COMPLETE
                        }
                        Err(_) => ERROR_INVALID_HANDLE,
                    }
                } else {
                    // DCWW_WAIT: blocking wait
                    match child.wait() {
                        Ok(status) => {
                            let code = status.code().unwrap_or(1) as u32;
                            // Drop the relay thread handle (detaches it).  The relay
                            // thread will exit on its own within microseconds: the
                            // child process has exited so the pipe write-end is closed,
                            // and the next pipe.read() will return EOF.  We must NOT
                            // call t.join() here — if the relay thread is currently
                            // inside write_tty (holding console_mgr) while the SDL
                            // renderer is also trying to acquire it, the join would
                            // deadlock until the next render frame (≥16 ms), which
                            // would make DosWaitChild appear to hang.
                            drop(self.shared.process_mgr.lock_or_recover().take_relay_thread(target_pid));
                            if p_res != 0 {
                                self.guest_write::<u32>(p_res, 0);
                                self.guest_write::<u32>(p_res + 4, code);
                            }
                            self.guest_write::<u32>(p_pid, target_pid);
                            NO_ERROR
                        }
                        Err(_) => ERROR_INVALID_HANDLE,
                    }
                }
            } else {
                ERROR_INVALID_HANDLE
            }
        } else {
            // DCWA_PROCESSTREE: wait for any child
            if option == 1 {
                // NOWAIT: try all children
                // Bound first: the guard of an `if let` scrutinee lives through the
                // block, which locks process_mgr again (a deadlock before).
                let finished = self.shared.process_mgr.lock_or_recover().wait_any();
                if let Some((pid, code)) = finished {
                    drop(self.shared.process_mgr.lock_or_recover().take_relay_thread(pid));
                    if p_res != 0 {
                        self.guest_write::<u32>(p_res, 0);
                        self.guest_write::<u32>(p_res + 4, code as u32);
                    }
                    self.guest_write::<u32>(p_pid, pid);
                    NO_ERROR
                } else {
                    128 // ERROR_CHILD_NOT_COMPLETE
                }
            } else {
                // WAIT: poll until a child finishes
                loop {
                    if self.shutting_down() { return ERROR_INVALID_FUNCTION; }
                    // Bound first: the guard of an `if let` scrutinee lives through the
                    // block, which locks process_mgr again (a deadlock before).
                    let finished = self.shared.process_mgr.lock_or_recover().wait_any();
                    if let Some((pid, code)) = finished {
                        drop(self.shared.process_mgr.lock_or_recover().take_relay_thread(pid));
                        if p_res != 0 {
                            self.guest_write::<u32>(p_res, 0);
                            self.guest_write::<u32>(p_res + 4, code as u32);
                        }
                        self.guest_write::<u32>(p_pid, pid);
                        return NO_ERROR;
                    }
                    // Check if there are any children at all
                    if self.shared.process_mgr.lock_or_recover().children.is_empty() {
                        return ERROR_INVALID_HANDLE; // no children to wait for
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }

    /// DosKillProcess (ordinal 235): kill a process.
    ///
    /// OS/2 signature: DosKillProcess(action, pid)
    /// - action: 0=DKP_PROCESSTREE, 1=DKP_PROCESS
    pub fn dos_kill_process(&self, _action: u32, pid: u32) -> u32 {
        debug!("  DosKillProcess(pid={})", pid);
        let child = self.shared.process_mgr.lock_or_recover().take_child(pid);
        if let Some(mut child) = child {
            match child.kill() {
                Ok(_) => {
                    let _ = child.wait(); // reap zombie
                    NO_ERROR
                }
                Err(_) => ERROR_INVALID_HANDLE,
            }
        } else {
            ERROR_INVALID_HANDLE
        }
    }

    /// DosQueryAppType (ordinal 323): query application type.
    ///
    /// Resolves the path through the DriveManager VFS (same as DosExecPgm /
    /// DosFindFirst), then inspects the MZ/LX/NE signature to determine the
    /// OS/2 application-type flags.
    pub fn dos_query_app_type(&self, psz_name: u32, p_flags: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosQueryAppType('{}')", name);

        let path = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            match dm.resolve_to_host_path(&name) {
                Some(p) => p,
                None => return ERROR_FILE_NOT_FOUND,
            }
        };

        if !path.exists() {
            return ERROR_FILE_NOT_FOUND;
        }

        // Detect executable type from MZ stub + secondary signature.
        // FAPPTYP_* flags (OS/2 Toolkit header values):
        //   0x0001 NOTWINDOWCOMPAT  — text-mode OS/2 32-bit app
        //   0x0002 WINDOWCOMPAT     — PM-capable OS/2 app
        //   0x0003 WINDOWAPI        — PM app
        //   0x0010 DLL              — dynamic link library
        //   0x0020 DOS              — real-mode DOS app
        let app_type = if let Ok(mut f) = std::fs::File::open(&path) {
            use std::io::{Read, Seek, SeekFrom};
            let mut mz = [0u8; 64];
            if f.read_exact(&mut mz).is_ok() && mz[0] == b'M' && mz[1] == b'Z' {
                // Read e_lfanew (offset 0x3C) to find secondary header
                let e_lfanew = u32::from_le_bytes([mz[60], mz[61], mz[62], mz[63]]);
                let mut sig = [0u8; 2];
                if f.seek(SeekFrom::Start(e_lfanew as u64)).is_ok()
                    && f.read_exact(&mut sig).is_ok()
                {
                    match &sig {
                        b"LX" => 1, // FAPPTYP_NOTWINDOWCOMPAT — OS/2 32-bit LX
                        b"NE" => 1, // OS/2 16-bit NE
                        b"PE" => 0, // PE — not an OS/2 app
                        _    => 1, // Unknown secondary — assume OS/2
                    }
                } else {
                    1 // MZ only, no secondary header — treat as OS/2
                }
            } else {
                0 // not an MZ executable
            }
        } else {
            0
        };

        if p_flags != 0 {
            self.guest_write::<u32>(p_flags, app_type);
        }
        NO_ERROR
    }

    /// Parse a double-null-terminated string from guest memory.
    /// Returns a vector of strings (typically: program name, then arguments).
    pub(crate) fn parse_double_null_string(&self, ptr: u32) -> Vec<String> {
        let mut strings = Vec::new();
        let mut current = String::new();
        let mut offset = 0u32;
        let max = 4096u32; // safety limit

        while offset < max {
            let byte = ptr.checked_add(offset).and_then(|a| self.guest_read::<u8>(a)).unwrap_or(0);
            if byte == 0 {
                if current.is_empty() {
                    break; // double null — done
                }
                strings.push(current.clone());
                current.clear();
            } else {
                current.push(byte as char);
            }
            offset += 1;
        }
        strings
    }
}

/// Read from `pipe` in a loop and write every chunk to the parent's
/// VioManager so the child's text output appears in the same SDL text
/// window (or terminal) as the parent process.
fn relay_pipe_to_vio<R: Read>(mut pipe: R, shared: Arc<super::SharedState>) {
    use super::mutex_ext::MutexExt;
    let mut buf = [0u8; 512];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let cp = shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
                let mut console = shared.console_mgr.lock_or_recover();
                console.write_tty(&buf[..n], cp);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChildStdio;
    use super::super::managers::ProcessManager;
    use super::super::Loader;
    use super::super::mutex_ext::MutexExt;

    /// An argument string at the top of the address space stops there
    /// instead of overflowing the address.
    #[test]
    fn test_double_null_string_at_the_top_of_memory() {
        let loader = Loader::new_mock();
        assert!(loader.parse_double_null_string(u32::MAX - 1).is_empty());
    }

    /// DosQueryAppType resolves paths through the DriveManager VFS and
    /// classifies MZ+LX executables as FAPPTYP_NOTWINDOWCOMPAT (1).
    #[test]
    fn test_dos_query_app_type_lx_exe() {
        let path = "samples/hello/hello.exe";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        // Mount C: → samples/hello/ so "C:\hello.exe" resolves correctly
        {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            dm.mount(2, Box::new(super::super::vfs_hostdir::HostDirBackend::new("samples/hello".into()).unwrap()));
        }
        // Write "C:\hello.exe" into guest memory at address 0x3000
        let name = b"C:\\hello.exe\0";
        loader.guest_write_bytes(0x3000, name).unwrap();
        let p_flags = 0x3100u32;
        loader.guest_write::<u32>(p_flags, 0xDEAD).unwrap();

        let rc = loader.dos_query_app_type(0x3000, p_flags);
        assert_eq!(rc, 0, "DosQueryAppType should return NO_ERROR");
        let flags = loader.guest_read::<u32>(p_flags).unwrap();
        assert_eq!(flags, 1, "LX exe should be FAPPTYP_NOTWINDOWCOMPAT (1)");
    }

    #[test]
    fn test_dos_query_app_type_missing_file() {
        let loader = Loader::new_mock();
        {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            dm.mount(2, Box::new(super::super::vfs_hostdir::HostDirBackend::new("samples/hello".into()).unwrap()));
        }
        let name = b"C:\\nonexistent_xyz.exe\0";
        loader.guest_write_bytes(0x3000, name).unwrap();
        let rc = loader.dos_query_app_type(0x3000, 0);
        assert_eq!(rc, 2, "missing file should return ERROR_FILE_NOT_FOUND (2)");
    }

    #[test]
    fn test_process_manager_add_child() {
        let mut mgr = ProcessManager::new();
        // Spawn a trivial child process
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = mgr.add_child(child);
        assert!(pid >= 100);
        assert!(mgr.children.contains_key(&pid));
    }

    #[test]
    fn test_process_manager_take_child() {
        let mut mgr = ProcessManager::new();
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = mgr.add_child(child);
        let taken = mgr.take_child(pid);
        assert!(taken.is_some());
        assert!(!mgr.children.contains_key(&pid));
        // Wait to reap
        taken.unwrap().wait().unwrap();
    }

    #[test]
    fn test_process_manager_wait_any() {
        let mut mgr = ProcessManager::new();
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = mgr.add_child(child);
        // Give it a moment to finish
        std::thread::sleep(std::time::Duration::from_millis(100));
        let result = mgr.wait_any();
        assert!(result.is_some());
        let (rpid, code) = result.unwrap();
        assert_eq!(rpid, pid);
        assert_eq!(code, 0);
    }

    /// DosWaitChild(DCWA_PROCESSTREE) returns once a child ends. It used to
    /// lock the process manager again while the `if let` scrutinee's guard
    /// was still held, a deadlock that hung 4OS2 after every external
    /// program.
    #[test]
    fn test_dos_wait_child_any_child_returns() {
        let loader = std::sync::Arc::new(Loader::new_mock());
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = loader.shared.process_mgr.lock_or_recover().add_child(child);
        let (p_res, p_pid) = (0x3000u32, 0x3010u32);
        let l = loader.clone();
        let waiter = std::thread::spawn(move || l.dos_wait_child(1, 0, p_res, p_pid, 0));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !waiter.is_finished() {
            assert!(std::time::Instant::now() < deadline, "DosWaitChild did not return");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(waiter.join().unwrap(), 0);
        assert_eq!(loader.guest_read::<u32>(p_pid), Some(pid));
        assert_eq!(loader.guest_read::<u32>(p_res + 4), Some(0), "exit code of `true`");
    }

    /// A standard handle the parent redirected to a file is passed to the
    /// child as that host file; the console is relayed (piped).
    #[test]
    fn test_child_stdio_follows_redirection() {
        let dir = tempfile::tempdir().unwrap();
        let loader = Loader::new_mock();
        loader.shared.drive_mgr.lock_or_recover().mount(2,
            Box::new(super::super::vfs_hostdir::HostDirBackend::new(dir.path().to_path_buf()).unwrap()));
        assert!(matches!(loader.child_stdio(1), ChildStdio::Console));
        let hf = {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            let (hf, _) = dm.open_file("C:\\child.txt", super::super::vfs::OpenMode::WriteOnly,
                super::super::vfs::SharingMode::DenyNone, super::super::vfs::OpenFlags::from_raw(0x12),
                super::super::vfs::FileAttribute::NORMAL).unwrap();
            dm.dup_file(hf, Some(1)).unwrap();
            hf
        };
        assert!(matches!(loader.child_stdio(1), ChildStdio::Host(_)));
        // The child writes into the file through the inherited descriptor.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "printf child"]);
        loader.set_child_stdio(&mut cmd);
        assert!(cmd.status().unwrap().success());
        loader.shared.drive_mgr.lock_or_recover().close_file(hf).unwrap();
        assert_eq!(std::fs::read(dir.path().join("child.txt")).unwrap(), b"child");
        // A closed standard handle gives the child nothing.
        loader.shared.drive_mgr.lock_or_recover().mark_std_closed(0);
        assert!(matches!(loader.child_stdio(0), ChildStdio::Closed));
    }
}
