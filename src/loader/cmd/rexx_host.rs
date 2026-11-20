// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX scripts in the built-in CMD.EXE (ADR-0083, ADR-0084): a `.CMD`
// whose first line starts with `/*` runs in the REXX interpreter
// (`loader::rexx`) with this shell as its host. Its commands (ADDRESS CMD)
// run here, echoed with the prompt while ECHO is on as OS/2's CMD.EXE does
// (hence the usual `'@ECHO OFF'`); SAY and TRACE write to the shell's
// output; PULL reads its input. Its streams and REXXUTIL's file functions
// reach files through the VFS, so they stay inside the drives.

use super::*;
use crate::loader::codepage::{cp_decode, cp_encode};
use crate::loader::rexx::interp::{error_message, run_program, CallType, FileInfo, Host, OpenFor};
use crate::loader::vfs::{ExistAction, FileAttribute, FileStatus, NewAction, OpenFlags, OpenMode, SeekMode, SharingMode};

/// The stack a REXX program runs on: its routines and expressions nest on
/// it, within the interpreter's own limits.
const REXX_STACK: usize = 64 * 1024 * 1024;

/// The largest REXX program read as an external routine.
const MAX_REXX_SOURCE: u64 = 4 * 1024 * 1024;

/// Whether a script is REXX: its first line starts with `/*`, blanks
/// before it allowed (unverified).
pub(super) fn is_rexx(bytes: &[u8]) -> bool {
    let first_line = bytes.split(|&b| b == b'\n').next().unwrap_or_default();
    let start = first_line.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(first_line.len());
    first_line[start..].starts_with(b"/*")
}

/// The shell as the REXX interpreter's host.
struct ShellHost<'a> {
    shell: &'a mut CmdShell,
    codepage: u32,
}

impl Host for ShellHost<'_> {
    fn say(&mut self, line: &[u8]) {
        self.shell.write_out(line);
        self.shell.write_out(b"\r\n");
    }

    fn command(&mut self, env: &[u8], command: &[u8]) -> Option<i64> {
        if env != crate::loader::rexx::interp::DEFAULT_ENV { return None; }
        let text = cp_decode(command, self.codepage);
        let line = text.trim();
        if self.shell.echo_on.get() && !line.starts_with('@') {
            self.shell.write_prompt();
            self.shell.writeln(line);
        }
        // EXIT run as a command ends nothing but the command here.
        let _ = self.shell.execute_line(line);
        Some(self.shell.last_rc.get() as i64)
    }

    fn read_line(&mut self) -> Option<Vec<u8>> {
        match self.shell.read_line() {
            ReadResult::Line(l) => Some(cp_encode(&l, self.codepage)),
            ReadResult::Eof => None,
        }
    }

    fn interrupted(&self) -> bool {
        self.shell.shared.exit_requested.load(Ordering::Relaxed)
            || crate::loader::interrupt::call_interrupted(&self.shell.shared)
    }

    fn now(&self) -> Option<(crate::loader::clock::LocalTime, u8)> {
        let ms = self.shell.shared.clock.now_ms();
        crate::loader::clock::unix_to_local(ms.div_euclid(1000)).map(|t| (t, (ms.rem_euclid(1000) / 10) as u8))
    }

    fn external(&mut self, name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let name = cp_decode(name, self.codepage);
        let (path, script, os2) = self.shell.find_program_with_name(&name)?;
        if !script || std::fs::metadata(&path).ok()?.len() > MAX_REXX_SOURCE { return None; }
        let source = std::fs::read(&path).ok()?;
        is_rexx(&source).then(|| (source, cp_encode(&os2, self.codepage)))
    }

    fn get_env(&self, name: &[u8]) -> Option<Vec<u8>> {
        let name = cp_decode(name, self.codepage);
        self.shell.env.borrow().iter().find(|(k, _)| k.eq_ignore_ascii_case(&name)).map(|(_, v)| cp_encode(v, self.codepage))
    }

    fn set_env(&mut self, name: &[u8], value: Option<&[u8]>) {
        let name = cp_decode(name, self.codepage);
        let value = value.map(|v| cp_decode(v, self.codepage)).unwrap_or_default();
        let _ = env_set(&mut self.shell.env.borrow_mut(), &name, &value);
    }

    fn write_out(&mut self, bytes: &[u8]) { self.shell.write_out(bytes); }

    fn write_err(&mut self, bytes: &[u8]) { self.shell.write_attr(bytes, ATTR_ERROR); }

    fn trace(&mut self, line: &[u8]) {
        self.shell.write_out(line);
        self.shell.write_out(b"\r\n");
    }

    fn clear_screen(&mut self) { self.shell.cmd_cls(); }

    fn full_name(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let (drive, rel) = self.shell.shared.drive_mgr.lock_or_recover().resolve_path(&self.os2(name)).ok()?;
        let rel = crate::loader::vfs::normalize_rel_path(&rel)?;
        Some(cp_encode(&format!("{}:\\{}", (b'A' + drive) as char, rel.replace('/', "\\")), self.codepage))
    }

    fn file_open(&mut self, name: &[u8], mode: OpenFor) -> Result<u32, u32> {
        let (access, flags) = match mode {
            OpenFor::Read => (OpenMode::ReadOnly, OpenFlags { exist_action: ExistAction::Open, new_action: NewAction::Fail }),
            OpenFor::Write => (OpenMode::ReadWrite, OpenFlags { exist_action: ExistAction::Open, new_action: NewAction::Create }),
            OpenFor::Replace => (OpenMode::ReadWrite, OpenFlags { exist_action: ExistAction::Replace, new_action: NewAction::Create }),
        };
        let path = self.os2(name);
        self.shell.shared.drive_mgr.lock_or_recover()
            .open_file(&path, access, SharingMode::DenyNone, flags, FileAttribute::NORMAL)
            .map(|(h, _)| h).map_err(|e| e.0)
    }

    fn file_read(&mut self, handle: u32, pos: u64, buf: &mut [u8]) -> Result<usize, u32> {
        let dm = self.shell.shared.drive_mgr.lock_or_recover();
        if !dm.is_file(handle) { return Err(crate::loader::rexx::interp::ERROR_INVALID_HANDLE); }
        dm.seek_file(handle, pos as i64, SeekMode::Begin).map_err(|e| e.0)?;
        dm.read_file(handle, buf).map_err(|e| e.0)
    }

    fn file_write(&mut self, handle: u32, pos: u64, data: &[u8]) -> Result<(), u32> {
        let dm = self.shell.shared.drive_mgr.lock_or_recover();
        if !dm.is_file(handle) { return Err(crate::loader::rexx::interp::ERROR_INVALID_HANDLE); }
        dm.seek_file(handle, pos as i64, SeekMode::Begin).map_err(|e| e.0)?;
        let mut done = 0;
        while done < data.len() {
            match dm.write_file(handle, &data[done..]) {
                Ok(0) => return Err(crate::loader::vfs::Os2Error::DISK_FULL.0),
                Ok(n) => done += n,
                Err(e) => return Err(e.0),
            }
        }
        Ok(())
    }

    fn file_size(&mut self, handle: u32) -> Result<u64, u32> {
        let dm = self.shell.shared.drive_mgr.lock_or_recover();
        if !dm.is_file(handle) { return Err(crate::loader::rexx::interp::ERROR_INVALID_HANDLE); }
        dm.query_file_info(handle, 1).map(|s| s.file_size as u64).map_err(|e| e.0)
    }

    fn file_close(&mut self, handle: u32) {
        let mut dm = self.shell.shared.drive_mgr.lock_or_recover();
        if dm.is_file(handle) { let _ = dm.close_file(handle); }
    }

    fn file_info(&mut self, name: &[u8]) -> Option<FileInfo> {
        let status = self.shell.shared.drive_mgr.lock_or_recover().query_path_info(&self.os2(name), 1).ok()?;
        Some(file_info(self.full_name(name)?, &status))
    }

    fn file_find(&mut self, pattern: &[u8], limit: usize) -> Result<Vec<FileInfo>, u32> {
        let pattern = self.os2(pattern);
        let mut dm = self.shell.shared.drive_mgr.lock_or_recover();
        let (handle, first) = dm.find_first(&pattern, FileAttribute(ATTR_ALL_ENTRIES), 1).map_err(|e| e.0)?;
        let mut found = Vec::new();
        let mut entry = Some(first);
        while let Some(e) = entry {
            if found.len() >= limit { break; }
            if e.name != "." && e.name != ".." { found.push(file_info(cp_encode(&e.name, self.codepage), &e.status)); }
            entry = dm.find_next(handle).ok();
        }
        let _ = dm.find_close(handle);
        Ok(found)
    }

    fn file_delete(&mut self, name: &[u8]) -> u32 {
        let path = self.os2(name);
        self.shell.shared.drive_mgr.lock_or_recover().delete_file(&path).err().map_or(0, |e| e.0)
    }

    fn dir_make(&mut self, name: &[u8]) -> u32 {
        let path = self.os2(name);
        self.shell.shared.drive_mgr.lock_or_recover().create_dir(&path).err().map_or(0, |e| e.0)
    }

    fn dir_remove(&mut self, name: &[u8]) -> u32 {
        let path = self.os2(name);
        self.shell.shared.drive_mgr.lock_or_recover().delete_dir(&path).err().map_or(0, |e| e.0)
    }

    fn drives(&mut self) -> Vec<u8> {
        let map = self.shell.shared.drive_mgr.lock_or_recover().logical_drive_map();
        (0..26u8).filter(|d| map & (1 << d) != 0).map(|d| b'A' + d).collect()
    }
}

/// The attributes a search matches: files and directories, hidden and
/// system ones too.
const ATTR_ALL_ENTRIES: u32 = 0x37;

/// A VFS file status as the interpreter's description of an entry.
fn file_info(name: Vec<u8>, status: &FileStatus) -> FileInfo {
    FileInfo {
        name, size: status.file_size as u64, date: status.last_write_date, time: status.last_write_time,
        attributes: status.attributes.0,
    }
}

impl ShellHost<'_> {
    /// A name from the script (process codepage) as an OS/2 path.
    fn os2(&self, name: &[u8]) -> String { cp_decode(name, self.codepage) }
}

impl CmdShell {
    /// Run the REXX program `source` (from `os2_name`) with the argument
    /// string `args`. Its EXIT or RETURN value, when a whole number, is
    /// the return code; an error is reported as OS/2 does, with the
    /// failing line, and its number is the return code (unverified).
    pub(super) fn run_rexx(&mut self, source: &[u8], os2_name: &str, args: &str) -> u32 {
        let codepage = self.shared.active_codepage.load(Ordering::Relaxed);
        let name = cp_encode(os2_name, codepage);
        let arg = cp_encode(args, codepage);
        let result = std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new().name("rexx".into()).stack_size(REXX_STACK).spawn_scoped(scope, || {
                let mut host = ShellHost { shell: self, codepage };
                run_program(&mut host, source, &name, vec![Some(arg)], CallType::Command)
            });
            spawned.map(|h| h.join())
        });
        match result {
            Ok(Ok(Ok(value))) => value.and_then(|v| crate::loader::rexx::number::Num::parse(&v)?.to_whole(9))
                .map_or(0, |n| n as u32),
            Ok(Ok(Err(e))) => {
                if e.line > 0 {
                    let text = source.split(|&b| b == b'\n').nth(e.line - 1).unwrap_or_default();
                    let text = cp_decode(text.strip_suffix(b"\r").unwrap_or(text), codepage);
                    self.writeln(&format!("{:6} +++ {}", e.line, text.trim()));
                }
                let mut msg = format!("REX{:04}: Error {} running {}", e.code, e.code, os2_name);
                if e.line > 0 { msg.push_str(&format!(", line {}", e.line)); }
                msg.push_str(&format!(": {}", error_message(e.code)));
                if let Some(d) = e.detail.as_deref().filter(|d| !d.is_empty()) { msg.push_str(&format!(" ({d})")); }
                self.error(&msg);
                e.code
            }
            _ => {
                self.error("The REXX interpreter could not run.");
                RC_BUILTIN_FAILED
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;
    use crate::loader::vfs_hostdir::HostDirBackend;

    /// Run REXX script `source` as `C:\\TEST.CMD` in a shell whose drive C
    /// is a new temporary directory; returns what it wrote and the
    /// directory.
    fn run_script(source: &str) -> (String, tempfile::TempDir) {
        let loader = Loader::new_mock();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("TEST.CMD"), source.replace('\n', "\r\n")).unwrap();
        {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            dm.mount(2, Box::new(HostDirBackend::new(dir.path().to_path_buf()).unwrap()));
            dm.set_current_disk(3).unwrap();
        }
        let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
        *shell.output_sink.borrow_mut() = OutputSink::Buffer(Vec::new());
        shell.execute_line("@ECHO OFF");
        shell.execute_line("TEST.CMD");
        let out = match &*shell.output_sink.borrow() {
            OutputSink::Buffer(b) => String::from_utf8_lossy(b).into_owned(),
            _ => unreachable!(),
        };
        (out, dir)
    }

    #[test]
    fn streams_read_and_write_files_on_the_drives() {
        let (out, dir) = run_script("/* */
call lineout 'OUT.TXT', 'first'
call lineout 'OUT.TXT', 'second'
call lineout 'OUT.TXT'
say lines('OUT.TXT', 'C') linein('OUT.TXT') linein('OUT.TXT') lines('OUT.TXT')
say stream('OUT.TXT', 'C', 'QUERY SIZE') stream('OUT.TXT', 'C', 'QUERY EXISTS') stream('NOPE.TXT', 'C', 'QUERY EXISTS')'|'
say charin('OUT.TXT', 1, 5) linein('OUT.TXT', 2) chars('OUT.TXT')
call charout 'OUT.TXT', 'XY', 1
say charin('OUT.TXT', 1, 5) stream('OUT.TXT', 'C', 'SEEK =1 READ') linein('OUT.TXT')
say stream('OUT.TXT', 'C', 'CLOSE') stream('OUT.TXT', 'C', 'OPEN WRITE REPLACE') lineout('OUT.TXT', 'only') stream('OUT.TXT')
call lineout 'OUT.TXT'
say lineout('..\\..\\ESCAPE.TXT', 'x') stream('Z:\\X.TXT', 'C', 'OPEN')
signal on notready
x = linein('MISSING.TXT')
exit
notready: say 'NR' condition('D') stream('MISSING.TXT', 'D')
");
        assert_eq!(out, "2 first second 0\r\n15 C:\\OUT.TXT |\r\nfirst second 0\r\nXYrst 1 XYrst\r\n\
                         READY: READY: 0 READY\r\n1 NOTREADY:3\r\nNR MISSING.TXT NOTREADY:2\r\n");
        assert_eq!(std::fs::read(dir.path().join("OUT.TXT")).unwrap(), b"only\r\n");
        assert!(!dir.path().parent().unwrap().join("ESCAPE.TXT").exists(), "a stream stays inside the drive");
    }

    #[test]
    fn rexxutil_works_with_files_on_the_drives() {
        let (out, dir) = run_script("/* */
call SysMkDir 'SUB'
call lineout 'SUB\\A.TXT', 'alpha beta'; call lineout 'SUB\\A.TXT'
call lineout 'SUB\\B.LOG', 'gamma'; call lineout 'SUB\\B.LOG'
rc = SysFileTree('SUB\\*.TXT', 'f', 'FO'); say rc f.0 f.1
rc = SysFileTree('*', 'd', 'DO'); say d.0 d.1
rc = SysFileTree('*.LOG', 'all', 'FSO'); say all.0 all.1
rc = SysFileTree('SUB\\A.TXT', 'line'); say words(line.1) word(line.1, 3) word(line.1, 4)
rc = SysFileSearch('BETA', 'SUB\\A.TXT', 'hit', 'N'); say rc hit.0 hit.1
say SysFileSearch('x', 'NONE.TXT', 'none') SysSearchPath('NOSUCH', 'SUB\\A.TXT')
t = SysTempFileName('SUB\\T???.TMP'); say length(t) left(t, 5) (stream(t, 'C', 'QUERY EXISTS') = '')
say SysFileDelete('SUB\\A.TXT') SysFileDelete('SUB\\A.TXT') SysFileDelete('SUB\\B.LOG') SysRmDir('SUB')
say word(SysDriveMap(), 1) SysBootDrive()
");
        assert_eq!(out, "0 1 C:\\SUB\\A.TXT\r\n1 C:\\SUB\r\n1 C:\\SUB\\B.LOG\r\n5 12 A----\r\n0 1 1 alpha beta\r\n\
                         3 C:\\SUB\\A.TXT\r\n12 SUB\\T 1\r\n0 2 0 0\r\nC: C:\r\n");
        assert!(!dir.path().join("SUB").exists());
    }

    #[test]
    fn rexx_scripts_start_with_a_comment() {
        assert!(is_rexx(b"/* REXX */\r\nsay 1"));
        assert!(is_rexx(b"  /**/"));
        assert!(!is_rexx(b"@echo off\r\n/* not */"));
        assert!(!is_rexx(b"\r\n/* a blank first line */"));
        assert!(!is_rexx(b""));
    }
}
