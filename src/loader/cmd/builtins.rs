// SPDX-License-Identifier: GPL-3.0-or-later
//
// The built-in CMD.EXE's own commands: DIR, CD, SET, ECHO, CLS, VER, TYPE,
// MD, RD, DEL and HELP.

use super::*;
use crate::loader::vfs::FileAttribute;

impl CmdShell {
    // ── Built-in commands ─────────────────────────────────────────────────────

    pub(super) fn cmd_cls(&self) {
        // Clear the screen and home the cursor on the terminal as well (the
        // old row-by-row blanking left the terminal cursor where it was).
        self.shared.console_mgr.lock_or_recover().clear_screen(ATTR_NORMAL);
    }

    /// The version is the latest release's: every release sets
    /// `Cargo.toml`'s version to its tag (`vX.Y.Z`).
    pub(super) fn cmd_ver(&self) {
        self.writeln(&ver_line());
    }

    pub(super) fn cmd_help(&self) {
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
        self.writeln("  COPY src[+src] [dst]  Copy (or join) files");
        self.writeln("  REN old new        Rename files in place");
        self.writeln("  MOVE src [dst]     Move files or a directory on one drive");
        self.writeln("  CLS                Clear screen");
        self.writeln("  VER                Show version");
        self.writeln("  REM / ::           Comment");
        self.writeln("  ECHO ON|OFF        Show or hide the prompt and a script's lines");
        self.writeln("  IF [NOT] ERRORLEVEL n | EXIST file | a==b  command");
        self.writeln("  FOR %v IN (set) DO command");
        self.writeln("  GOTO label         Go on after :label (in a script)");
        self.writeln("  CALL script [args] Run a script and come back");
        self.writeln("  SHIFT              Move %1-%9 down by one (in a script)");
        self.writeln("  SETLOCAL/ENDLOCAL  Save / restore variables and directory (in a script)");
        self.writeln("  PAUSE              Wait for a key");
        self.writeln("  PROMPT [text]      Set the prompt ($P path, $G >, $E ESC, $_ new line ...)");
        self.writeln("  PATH [dirs | ;]    Show, set or clear the program search path");
        self.writeln("  DETACH program     Run a program in the background, without a console");
        self.writeln("  START [/C|/K] [/FS|/WIN] [/I] [command]  Start a session of its own");
        self.writeln("  EXIT [code]        Exit shell");
        self.writeln("  <program>          Run OS/2 executable");
    }

    /// `PROMPT text` sets the prompt's template (`$` codes, ADR-0080);
    /// `PROMPT` alone goes back to the default.
    pub(super) fn cmd_prompt(&self, text: &str) {
        let result = env_set(&mut self.env.borrow_mut(), "PROMPT", text.trim_end());
        if result.is_err() { self.error("Out of environment space."); }
    }

    /// `PATH` shows the search path, `PATH ;` clears it, `PATH dirs` sets
    /// it (directories separated by `;`, searched for programs).
    pub(super) fn cmd_path(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            let path = self.env.borrow().iter().find(|(k, _)| k.eq_ignore_ascii_case("PATH")).map(|(_, v)| v.clone());
            match path {
                Some(p) => self.writeln(&format!("PATH={p}")),
                None => self.writeln("No Path."),
            }
            return;
        }
        let value = if text == ";" { "" } else { text };
        if env_set(&mut self.env.borrow_mut(), "PATH", value).is_err() { self.error("Out of environment space."); }
    }

    pub(super) fn cmd_set(&self, args: &[&str]) {
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
                Err(EnvError::NoName) => self.error("Syntax error: no variable name."),
                Err(EnvError::Full) => self.error("Out of environment space."),
            }
            return;
        }
        // SET VAR — show the variables whose names start with VAR.
        let prefix = joined.to_uppercase();
        let found: Vec<String> = self.env.borrow().iter()
            .filter(|(k, _)| k.to_uppercase().starts_with(&prefix))
            .map(|(k, v)| format!("{k}={v}")).collect();
        if found.is_empty() {
            self.error("Environment variable not found.");
        }
        for line in found {
            self.writeln(&line);
        }
    }

    /// `CD` shows the current drive's directory; `CD path` changes it,
    /// for an existing directory only (`DriveManager::set_current_dir`).
    pub(super) fn cmd_cd(&self, args: &[&str]) {
        let Some(&path) = args.first() else {
            self.writeln(&self.current_path());
            return;
        };
        let result = self.shared.drive_mgr.lock_or_recover().set_current_dir(path);
        if result.is_err() {
            self.error("The system cannot find the path specified.");
        }
    }

    pub(super) fn cmd_change_drive(&self, disk_os2: u8) {
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        if dm.set_current_disk(disk_os2).is_err() {
            let letter = (b'A' + disk_os2 - 1) as char;
            drop(dm);
            self.error(&format!("Drive {letter}: is not available."));
        }
    }

    pub(super) fn cmd_dir(&self, args: &[&str]) {
        // Build the search pattern from the argument (default: current dir \*)
        let pattern = {
            let current = self.current_path();
            let base = if current.ends_with('\\') { current } else { format!("{current}\\") };
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
                self.error("File not found.");
                return;
            }
        };

        let mut entries: Vec<crate::loader::vfs::DirEntry> = vec![first_entry];
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
                total_bytes += e.status.file_size;
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

    pub(super) fn cmd_type(&self, args: &[&str]) {
        let Some(&path_arg) = args.first() else {
            self.error("Required parameter missing.");
            return;
        };

        let host_path = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            dm.resolve_to_host_path(path_arg)
        };

        match host_path {
            None => self.error("The system cannot find the file specified."),
            Some(p) => match std::fs::read(&p) {
                Err(e) => self.error(&format!("Error reading file: {e}")),
                Ok(bytes) => {
                    self.write_out(&bytes);
                    self.write_out(b"\r\n");
                }
            },
        }
    }

    pub(super) fn cmd_md(&self, args: &[&str]) {
        let Some(&path_arg) = args.first() else {
            self.error("Required parameter missing.");
            return;
        };
        let dm = self.shared.drive_mgr.lock_or_recover();
        if let Err(e) = dm.create_dir(path_arg) {
            drop(dm);
            self.error(&format!("Cannot create directory: {e:?}"));
        }
    }

    pub(super) fn cmd_rd(&self, args: &[&str]) {
        let Some(&path_arg) = args.first() else {
            self.error("Required parameter missing.");
            return;
        };
        let dm = self.shared.drive_mgr.lock_or_recover();
        if let Err(e) = dm.delete_dir(path_arg) {
            drop(dm);
            self.error(&format!("Cannot remove directory: {e:?}"));
        }
    }

    pub(super) fn cmd_del(&self, args: &[&str]) {
        let Some(&path_arg) = args.first() else {
            self.error("Required parameter missing.");
            return;
        };
        let dm = self.shared.drive_mgr.lock_or_recover();
        if let Err(e) = dm.delete_file(path_arg) {
            drop(dm);
            self.error(&format!("Could not delete file: {e:?}"));
        }
    }
}

/// What `VER` prints: the shell's name and Warpine's release version.
pub(super) fn ver_line() -> String {
    format!("Warpine OS/2 Compatibility Layer [Builtin CMD.EXE] version {}", env!("CARGO_PKG_VERSION"))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
