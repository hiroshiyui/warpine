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
//!   ones): Warpine takes 64, **unverified**. A value outside the range is
//!   clamped to it.
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

use std::ffi::OsStr;

use super::vfs::{DriveManager, ExistAction, FileAttribute, NewAction, OpenFlags, OpenMode, SharingMode};

/// The CONFIG.SYS Warpine reads.
pub const CONFIG_SYS_PATH: &str = "C:\\CONFIG.SYS";
/// Smallest `THREADS=` value (unverified: 32 in some sources).
pub const MIN_THREADS: u32 = 64;
/// Largest `THREADS=` value.
pub const MAX_THREADS: u32 = 4095;
/// The limit without a `THREADS=` statement.
pub const DEFAULT_THREADS: u32 = 64;
/// Environment override of the limit.
pub const THREADS_ENV: &str = "WARPINE_THREADS";
/// Most bytes of CONFIG.SYS read (the rest is ignored).
pub const MAX_CONFIG_SYS: usize = 256 * 1024;
/// Most digits of a value looked at (more saturate).
const MAX_DIGITS: usize = 12;
/// Most `LIBPATH` directories kept (unverified; the rest are ignored).
pub const MAX_LIBPATH_ENTRIES: usize = 64;
/// Longest `LIBPATH` directory kept (`CCHMAXPATH` less the file name).
pub const MAX_LIBPATH_DIR: usize = 260;
/// `LIBPATH` without a `LIBPATH=` statement: the system DLL directory.
pub const DEFAULT_LIBPATH: &[u8] = b"C:\\OS2\\DLL";
/// The environment before CONFIG.SYS's `SET` statements (Warpine's).
pub const BASE_ENVIRONMENT: &[&[u8]] = &[b"PATH=C:\\", b"COMSPEC=C:\\CMD.EXE", b"OS=OS2", b"TMP=C:\\TMP"];

/// What Warpine takes from CONFIG.SYS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSys {
    /// The last valid `THREADS=` value (not yet clamped).
    pub threads: Option<u64>,
    /// The last `LIBPATH=` statement's directories (codepage bytes).
    pub libpath: Option<Vec<Vec<u8>>>,
    /// The `SET` statements in order: upper-cased name, value (empty:
    /// remove the variable).
    pub set: Vec<(Vec<u8>, Vec<u8>)>,
}

impl ConfigSys {
    /// Parse CONFIG.SYS text (its first [`MAX_CONFIG_SYS`] bytes).
    pub fn parse(text: &[u8]) -> Self {
        let text = &text[..text.len().min(MAX_CONFIG_SYS)];
        let mut cfg = Self::default();
        for line in text.split(|&b| b == b'\n') {
            let line = line.trim_ascii();
            if line.is_empty() || is_rem(line) { continue; }
            if let Some(rest) = keyword_rest(line, b"SET") {
                let Some(eq) = rest.iter().position(|&b| b == b'=') else { continue };
                let name = rest[..eq].trim_ascii().to_ascii_uppercase();
                if name.is_empty() { continue; }
                cfg.set.push((name, rest[eq + 1..].trim_ascii().to_vec()));
                continue;
            }
            let Some(eq) = line.iter().position(|&b| b == b'=') else { continue };
            let (key, value) = (line[..eq].trim_ascii(), &line[eq + 1..]);
            if key.eq_ignore_ascii_case(b"THREADS") {
                if let Some(n) = parse_number(value) { cfg.threads = Some(n); }
            } else if key.eq_ignore_ascii_case(b"LIBPATH") {
                cfg.libpath = Some(split_path_list(value));
            }
        }
        cfg
    }

    /// The `LIBPATH` directories: the statement's, else [`DEFAULT_LIBPATH`].
    pub fn libpath_dirs(&self) -> Vec<Vec<u8>> {
        self.libpath.clone().unwrap_or_else(|| vec![DEFAULT_LIBPATH.to_vec()])
    }

    /// The environment a program starts with when no parent gives it one:
    /// [`BASE_ENVIRONMENT`] with the `SET` statements applied, within
    /// `inherit::MAX_ENV_BLOCK`.
    pub fn environment(&self) -> Vec<Vec<u8>> {
        let mut env: Vec<Vec<u8>> = BASE_ENVIRONMENT.iter().map(|s| s.to_vec()).collect();
        let name_of = |s: &[u8]| s.split(|&b| b == b'=').next().unwrap_or(&[]).to_ascii_uppercase();
        for (name, value) in &self.set {
            let pos = env.iter().position(|s| name_of(s) == *name);
            if value.is_empty() {
                if let Some(i) = pos { env.remove(i); }
                continue;
            }
            let entry = [name.as_slice(), b"=", value.as_slice()].concat();
            let size = env.iter().map(|s| s.len() + 1).sum::<usize>()
                - pos.map_or(0, |i| env[i].len() + 1) + entry.len() + 2;
            if size > super::inherit::MAX_ENV_BLOCK {
                log::warn!("CONFIG.SYS: SET {} makes the environment too large; ignored",
                           String::from_utf8_lossy(name));
                continue;
            }
            match pos {
                Some(i) => env[i] = entry,
                None => env.push(entry),
            }
        }
        env
    }
}

/// The rest of `line` after keyword `kw` and at least one blank; `None`
/// when `line` does not start with the keyword.
fn keyword_rest<'a>(line: &'a [u8], kw: &[u8]) -> Option<&'a [u8]> {
    let n = kw.len();
    (line.len() > n && line[..n].eq_ignore_ascii_case(kw) && line[n].is_ascii_whitespace())
        .then(|| line[n..].trim_ascii_start())
}

/// The directories of a `;`-separated list: trimmed, empty ones dropped,
/// over-long ones and those past [`MAX_LIBPATH_ENTRIES`] ignored.
pub fn split_path_list(value: &[u8]) -> Vec<Vec<u8>> {
    let dirs: Vec<Vec<u8>> = value.split(|&b| b == b';').map(<[u8]>::trim_ascii)
        .filter(|d| !d.is_empty() && d.len() <= MAX_LIBPATH_DIR)
        .map(<[u8]>::to_vec).collect();
    if dirs.len() > MAX_LIBPATH_ENTRIES {
        log::warn!("LIBPATH: {} directories; only the first {MAX_LIBPATH_ENTRIES} are searched", dirs.len());
    }
    dirs.into_iter().take(MAX_LIBPATH_ENTRIES).collect()
}

/// `n` clamped to the documented range.
pub fn clamp_threads(n: u64) -> u32 {
    n.clamp(u64::from(MIN_THREADS), u64::from(MAX_THREADS)) as u32
}

/// A decimal number (saturating), surrounding spaces allowed; `None` when
/// it is not one.
fn parse_number(s: &[u8]) -> Option<u64> {
    let s = s.trim_ascii();
    if s.is_empty() || s.len() > MAX_DIGITS || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(s.iter().fold(0u64, |n, &d| n.saturating_mul(10).saturating_add(u64::from(d - b'0'))))
}

/// Whether `line` (leading blanks removed) is a `REM` comment.
fn is_rem(line: &[u8]) -> bool {
    line.len() >= 3 && line[..3].eq_ignore_ascii_case(b"REM")
        && line.get(3).is_none_or(|c| c.is_ascii_whitespace())
}

/// The value of the last valid `THREADS=` statement of CONFIG.SYS text
/// `text` (not yet clamped).
pub fn parse_threads(text: &[u8]) -> Option<u64> {
    ConfigSys::parse(text).threads
}

/// The session's thread limit: `env` (`WARPINE_THREADS`) when it is a
/// number, else the CONFIG.SYS statement, else the default; clamped.
pub fn threads_limit(config_sys: Option<&[u8]>, env: Option<&OsStr>) -> u32 {
    limit_of(config_sys.and_then(parse_threads), env)
}

/// [`threads_limit`] with the CONFIG.SYS value already parsed.
fn limit_of(threads: Option<u64>, env: Option<&OsStr>) -> u32 {
    if let Some(n) = env.and_then(|v| parse_number(v.as_encoded_bytes())) {
        return clamp_threads(n);
    }
    if let Some(v) = env {
        log::warn!("{THREADS_ENV}={} is not a number; ignored", v.to_string_lossy());
    }
    threads.map_or(DEFAULT_THREADS, clamp_threads)
}

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
