// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 process management API implementations.
// DosExecPgm, DosWaitChild, DosKillProcess, DosQueryAppType.

use std::process::Command;
use log::{debug, warn};

use super::constants::*;
use super::mutex_ext::MutexExt;

impl super::Loader {
    /// DosExecPgm (ordinal 283): execute a program.
    ///
    /// OS/2 signature: DosExecPgm(pObjname, cbObjname, execFlag, pArg, pEnv, pRes, pName)
    /// - execFlag: 0=EXEC_SYNC, 1=EXEC_ASYNC, 2=EXEC_ASYNCRESULT, 3=EXEC_TRACE, 4=EXEC_BACKGROUND
    /// - pArg: double-null-terminated string (program name \0 args \0\0)
    /// - pRes: pointer to RESULTCODES struct (codeTerminate, codeResult)
    /// - pName: program path
    pub fn dos_exec_pgm(&self, p_objname: u32, cb_objname: u32, exec_flag: u32,
                         p_arg: u32, _p_env: u32, p_res: u32, p_name: u32) -> u32 {
        let prog_name = self.read_guest_string(p_name);
        debug!("  DosExecPgm(prog='{}', execFlag={})", prog_name, exec_flag);

        // Parse the double-null-terminated argument string
        let args = if p_arg != 0 {
            self.parse_double_null_string(p_arg)
        } else {
            Vec::new()
        };
        debug!("    args: {:?}", args);

        // Resolve the program path through the sandbox
        let prog_path = match self.translate_path(&prog_name) {
            Ok(p) => p,
            Err(e) => {
                // Write failing module name to error buffer
                if p_objname != 0 && cb_objname > 0 {
                    let bytes = prog_name.as_bytes();
                    let n = bytes.len().min(cb_objname as usize - 1);
                    self.guest_write_bytes(p_objname, &bytes[..n]);
                    self.guest_write::<u8>(p_objname + n as u32, 0);
                }
                return e;
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
        if let Some(ref cwd) = cwd {
            if cwd.is_dir() {
                cmd.current_dir(cwd);
            }
        }

        match exec_flag {
            0 => {
                // EXEC_SYNC: run and wait
                match cmd.status() {
                    Ok(status) => {
                        let exit_code = status.code().unwrap_or(1) as u32;
                        if p_res != 0 {
                            self.guest_write::<u32>(p_res, 0);         // codeTerminate: 0=normal
                            self.guest_write::<u32>(p_res + 4, exit_code); // codeResult
                        }
                        NO_ERROR
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
                // EXEC_ASYNC / EXEC_ASYNCRESULT / EXEC_BACKGROUND: spawn and track
                match cmd.spawn() {
                    Ok(child) => {
                        let pid = self.shared.process_mgr.lock_or_recover().add_child(child);
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
    /// OS/2 signature: DosWaitChild(action, option, pRes, pPid)
    /// - action: 0=DCWA_PROCESS (specific pid), 1=DCWA_PROCESSTREE (any child)
    /// - option: 0=DCWW_WAIT, 1=DCWW_NOWAIT
    pub fn dos_wait_child(&self, action: u32, option: u32, p_res: u32, p_pid: u32) -> u32 {
        let target_pid = self.guest_read::<u32>(p_pid).unwrap_or(0);
        debug!("  DosWaitChild(action={}, option={}, pid={})", action, option, target_pid);

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
                if let Some((pid, code)) = self.shared.process_mgr.lock_or_recover().wait_any() {
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
                    if let Some((pid, code)) = self.shared.process_mgr.lock_or_recover().wait_any() {
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

    /// DosKillProcess (ordinal 237): kill a process.
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
    /// Returns application type flags. For simplicity, check if the file
    /// is an OS/2 LX executable and return appropriate flags.
    pub fn dos_query_app_type(&self, psz_name: u32, p_flags: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosQueryAppType('{}')", name);

        let path = match self.translate_path(&name) {
            Ok(p) => p,
            Err(e) => return e,
        };

        if !path.exists() {
            return ERROR_FILE_NOT_FOUND;
        }

        // Try to detect if it's an OS/2 executable by checking MZ+LX signature
        let app_type = if let Ok(mut f) = std::fs::File::open(&path) {
            use std::io::Read;
            let mut header = [0u8; 2];
            if f.read_exact(&mut header).is_ok() && header == [b'M', b'Z'] {
                // FAPPTYP_NOTWINDOWCOMPAT (1) = text-mode OS/2 app
                // This is a simplification — ideally parse the LX header flags
                1u32
            } else {
                0 // not an executable
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
    fn parse_double_null_string(&self, ptr: u32) -> Vec<String> {
        let mut strings = Vec::new();
        let mut current = String::new();
        let mut offset = 0u32;
        let max = 4096u32; // safety limit

        while offset < max {
            let byte = self.guest_read::<u8>(ptr + offset).unwrap_or(0);
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

#[cfg(test)]
mod tests {
    use super::super::managers::ProcessManager;

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
}
