// SPDX-License-Identifier: GPL-3.0-or-later
//
//! `C:\CONFIG.SYS`: the statements Warpine takes from it (ADR-0032,
//! ADR-0092).
//!
//! - `THREADS=n`: the most threads the whole system — here, one Warpine
//!   session — runs at once (every process needs at least its thread 1).
//!   `session_threads.rs` enforces it.
//! - `LIBPATH=dir;dir;…`: the directories searched for DLLs
//!   (`lx_loader.rs` `find_dll_path`), between BEGINLIBPATH and ENDLIBPATH.
//! - `SET name=value`: a variable of the environment every program starts
//!   with when no parent gives it one.
//!
//! From public OS/2 documentation (CONFIG.SYS reference of the OS/2 Warp
//! Command Reference and the tuning notes derived from it):
//! - `THREADS=n` sets "the maximum number of threads … that OS/2 can run at
//!   the same time"; the upper bound is 4095, and the kernel's default is 64.
//!   Sources differ on the lower bound (32 in OS/2 2.x notes, 64 in later
//!   ones) and on the upper one (4096 in a WPS settings help, 4095 on
//!   EDM2). A second search (2026-11-25) found the same secondary
//!   consensus for Warp 3, Warp 4 and ArcaOS — 64 to 4095, 64 without the
//!   statement (EDM/2's Config.sys Documentation Project, a CONFIG.SYS
//!   editor's data file, Arca Noae's kernel errata: "The default number of
//!   threads in the ArcaOS kernel is 64, inherited from Warp 4"), 4095 in
//!   IBM's Redbook GG24-3730 — but still no text of IBM's Command
//!   Reference (`CMDREF.INF` exists online only inside disk images of
//!   install media, which Warpine does not use). Warpine takes 64 to 4095,
//!   **unverified**. No source says whether OS/2 rejects or clamps a value
//!   outside the range; Warpine clamps it.
//! - `LIBPATH` lists directories separated by `;`, `.` being the current
//!   directory; it is not an environment variable.
//! - `SET` puts a variable in the environment programs start with.
//!
//! The file is only read (through the VFS, drive C), never written or
//! created. Parsing (unverified details of OS/2's own CONFIG.SYS reader,
//! chosen to match how the statements are commonly written):
//! - keywords are case-insensitive, spaces around `=` are allowed;
//! - lines starting with `REM` are comments;
//! - the last valid `THREADS=` and `LIBPATH=` statements win; a `THREADS=`
//!   whose value is not a number is ignored;
//! - a `SET` name is upper-cased and trimmed, its value trimmed; a later
//!   `SET` of the same name replaces an earlier one, and an empty value
//!   removes the variable; `%name%` is not expanded.
//!
//! Each Warpine process reads the file once, when it first needs it
//! ([`super::SharedState::config_sys`]) — so an edit applies to programs
//! started afterwards, where OS/2 reads it at boot only. The session's
//! thread limit is the exception: its first process passes it on.
//!
//! `WARPINE_THREADS` (environment) overrides the file's `THREADS=` — for
//! tests, and to give a child process its session's limit.

use super::vfs::{DriveManager, ExistAction, FileAttribute, NewAction, OpenFlags, OpenMode, SharingMode};

pub use super::config_sys_parse::*;

/// The text of `C:\CONFIG.SYS` (at most `MAX_CONFIG_SYS` bytes), read
/// through the VFS; `None` when drive C or the file is missing.
pub fn read_config_sys(dm: &DriveManager) -> Option<Vec<u8>> {
    let (drive, rel) = dm.resolve_path(CONFIG_SYS_PATH).ok()?;
    let backend = dm.backend(drive).ok()?;
    // Open only an existing file, read-only: nothing is ever created.
    let flags = OpenFlags { exist_action: ExistAction::Open, new_action: NewAction::Fail };
    let (handle, _) = backend.open(&rel, OpenMode::ReadOnly, SharingMode::DenyNone, flags,
                                   FileAttribute::NORMAL).ok()?;
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    while data.len() < MAX_CONFIG_SYS {
        match backend.read(handle, &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let take = n.min(MAX_CONFIG_SYS - data.len());
                data.extend_from_slice(&buf[..take]);
            }
        }
    }
    let _ = backend.close(handle);
    Some(data)
}

/// The limit of a session started now: `WARPINE_THREADS`, else `cfg`
/// (drive C's CONFIG.SYS).
pub fn session_limit(cfg: &ConfigSys) -> u32 {
    limit_of(cfg.threads, std::env::var_os(THREADS_ENV).as_deref())
}

/// CONFIG.SYS of drive C, parsed; empty when there is none.
pub fn load(dm: &DriveManager) -> ConfigSys {
    read_config_sys(dm).map(|t| ConfigSys::parse(&t)).unwrap_or_default()
}

impl super::SharedState {
    /// Drive C's CONFIG.SYS, read the first time it is needed and kept for
    /// the life of this process. Takes `drive_mgr` that once: never call it
    /// under a lock that comes after `drive_mgr`.
    pub fn config_sys(&self) -> std::sync::Arc<ConfigSys> {
        use super::mutex_ext::MutexExt;
        std::sync::Arc::clone(self.config_sys_cell.get_or_init(|| {
            std::sync::Arc::new(load(&self.drive_mgr.lock_or_recover()))
        }))
    }

    /// The environment a program starts with when no parent gives one
    /// ([`ConfigSys::environment`]).
    pub fn initial_environment(&self) -> Vec<Vec<u8>> {
        self.config_sys().environment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn statement_found_case_insensitively_with_spaces() {
        assert_eq!(parse_threads(b"threads=300\r\n"), Some(300));
        assert_eq!(parse_threads(b"  Threads = 128  \r\n"), Some(128));
        assert_eq!(parse_threads(b"\tTHREADS\t=\t70"), Some(70));
    }

    #[test]
    fn rem_lines_and_other_statements_are_skipped() {
        let text = b"REM THREADS=999\r\nrem\tTHREADS=998\r\nPROTECTONLY=YES\r\nTHREADSX=5\r\nREMTHREADS=1\r\n";
        assert_eq!(parse_threads(text), None);
        assert_eq!(parse_threads(b"REM a\nTHREADS=100\n"), Some(100));
    }

    #[test]
    fn last_valid_statement_wins() {
        assert_eq!(parse_threads(b"THREADS=100\nTHREADS=200\n"), Some(200));
        assert_eq!(parse_threads(b"THREADS=100\nTHREADS=abc\nTHREADS=\n"), Some(100));
        assert_eq!(parse_threads(b"THREADS=-5\n"), None);
    }

    #[test]
    fn huge_values_saturate_and_clamp() {
        assert_eq!(parse_threads(b"THREADS=99999999999\n"), Some(99_999_999_999));
        assert_eq!(parse_threads(b"THREADS=9999999999999999999999\n"), None);
        assert_eq!(clamp_threads(99_999_999_999), MAX_THREADS);
        assert_eq!(clamp_threads(0), MIN_THREADS);
        assert_eq!(clamp_threads(70), 70);
    }

    #[test]
    fn limit_from_env_file_or_default() {
        assert_eq!(threads_limit(None, None), DEFAULT_THREADS);
        assert_eq!(threads_limit(Some(b"THREADS=500"), None), 500);
        assert_eq!(threads_limit(Some(b"THREADS=1"), None), MIN_THREADS);
        assert_eq!(threads_limit(Some(b"THREADS=100000"), None), MAX_THREADS);
        assert_eq!(threads_limit(Some(b"THREADS=500"), Some(OsStr::new("70"))), 70);
        assert_eq!(threads_limit(Some(b"THREADS=500"), Some(OsStr::new("junk"))), 500);
        assert_eq!(threads_limit(None, Some(OsStr::new("10"))), MIN_THREADS);
    }

    #[test]
    fn libpath_last_statement_wins_trimmed_and_bounded() {
        let cfg = ConfigSys::parse(b"LIBPATH=C:\\A\r\nrem LIBPATH=C:\\X\r\n libpath = .; C:\\OS2\\DLL ;;C:\\B;\r\n");
        assert_eq!(cfg.libpath, Some(vec![b".".to_vec(), b"C:\\OS2\\DLL".to_vec(), b"C:\\B".to_vec()]));
        assert_eq!(ConfigSys::parse(b"LIBPATH=\n").libpath, Some(vec![]), "an empty LIBPATH searches nothing");
        assert_eq!(ConfigSys::default().libpath_dirs(), vec![DEFAULT_LIBPATH.to_vec()]);
        let many: Vec<u8> = (0..100).flat_map(|i| format!("C:\\D{i};").into_bytes()).collect();
        assert_eq!(split_path_list(&many).len(), MAX_LIBPATH_ENTRIES);
        let long = [b"C:\\".as_slice(), &[b'X'; MAX_LIBPATH_DIR]].concat();
        assert_eq!(split_path_list(&[long.as_slice(), b";C:\\OK"].concat()), vec![b"C:\\OK".to_vec()]);
    }

    #[test]
    fn set_statements_build_the_environment() {
        let cfg = ConfigSys::parse(concat!(
            "SET PATH=C:\\OS2;C:\\\r\n", "set  myVar = spaced value  \r\n", "SET TMP=\r\n",
            "REM SET HIDDEN=1\r\n", "SETX=1\r\n", "SET =nameless\r\n", "SET NOEQUALS\r\n",
            "SET A=1\r\n", "SET A=%PATH%;2\r\n", "SET EQ=a=b\r\n", "SET\tTAB=t\r\n").as_bytes());
        assert_eq!(cfg.environment(), vec![
            b"PATH=C:\\OS2;C:\\".to_vec(), b"COMSPEC=C:\\CMD.EXE".to_vec(), b"OS=OS2".to_vec(),
            b"MYVAR=spaced value".to_vec(), b"A=%PATH%;2".to_vec(), b"EQ=a=b".to_vec(), b"TAB=t".to_vec(),
        ]);
        assert_eq!(ConfigSys::default().environment(),
                   BASE_ENVIRONMENT.iter().map(|s| s.to_vec()).collect::<Vec<_>>());
    }

    #[test]
    fn set_statements_stay_within_an_environment_block() {
        let big = "X".repeat(40 * 1024);
        let cfg = ConfigSys::parse(format!("SET ONE={big}\nSET TWO={big}\nSET THREE=3\n").as_bytes());
        let env = cfg.environment();
        assert!(env.iter().any(|s| s.starts_with(b"ONE=")));
        assert!(!env.iter().any(|s| s.starts_with(b"TWO=")), "too large: ignored");
        assert!(env.iter().any(|s| s == b"THREE=3"));
        assert!(env.iter().map(|s| s.len() + 1).sum::<usize>() < super::super::inherit::MAX_ENV_BLOCK);
    }

    #[test]
    fn statements_are_read_once_per_process() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CONFIG.SYS"), b"THREADS=99\r\nSET K3=yes\r\n").unwrap();
        let l = super::super::Loader::new_mock();
        { use super::super::mutex_ext::MutexExt; l.shared.drive_mgr.lock_or_recover() }
            .mount(2, Box::new(super::super::vfs_hostdir::HostDirBackend::new(dir.path().to_path_buf()).unwrap()));
        assert_eq!(l.shared.config_sys().threads, Some(99));
        assert!(l.shared.initial_environment().contains(&b"K3=yes".to_vec()));
        std::fs::write(dir.path().join("CONFIG.SYS"), b"THREADS=200\r\n").unwrap();
        assert_eq!(l.shared.config_sys().threads, Some(99), "kept from the first read");
    }

    #[test]
    fn config_sys_read_through_drive_c_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut dm = DriveManager::with_drive_c(dir.path().to_path_buf());
        dm.mount(2, Box::new(super::super::vfs_hostdir::HostDirBackend::new(dir.path().to_path_buf()).unwrap()));
        assert_eq!(read_config_sys(&dm), None);
        // Reading a missing file creates nothing.
        assert!(!dir.path().join("CONFIG.SYS").exists());
        std::fs::write(dir.path().join("CONFIG.SYS"), b"THREADS=77\r\n").unwrap();
        assert_eq!(read_config_sys(&dm).as_deref(), Some(&b"THREADS=77\r\n"[..]));
        let text = read_config_sys(&dm);
        assert_eq!(threads_limit(text.as_deref(), None), 77);
    }
}
