// SPDX-License-Identifier: GPL-3.0-or-later
//
//! The `THREADS=` statement of `C:\CONFIG.SYS`: the most threads the whole
//! system — here, one Warpine session — runs at once (every process needs at
//! least its thread 1). `session_threads.rs` enforces it.
//!
//! From public OS/2 documentation (CONFIG.SYS reference of the OS/2 Warp
//! Command Reference and the tuning notes derived from it): `THREADS=n` sets
//! "the maximum number of threads … that OS/2 can run at the same time";
//! the upper bound is 4095, and the kernel's default is 64. Sources differ on
//! the lower bound (32 in OS/2 2.x notes, 64 in later ones): Warpine takes
//! 64, **unverified**. A value outside the range is clamped to it.
//!
//! The file is only read (through the VFS, drive C), never written or
//! created. Parsing (unverified details of OS/2's own CONFIG.SYS reader,
//! chosen to match how the statement is commonly written):
//! - keywords are case-insensitive, spaces around `=` are allowed;
//! - lines starting with `REM` are comments;
//! - the last valid `THREADS=` statement wins; a statement whose value is
//!   not a number is ignored.
//!
//! `WARPINE_THREADS` (environment) overrides the file — for tests, and to
//! give a child process its session's limit.

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
    let text = &text[..text.len().min(MAX_CONFIG_SYS)];
    let mut found = None;
    for line in text.split(|&b| b == b'\n') {
        let line = line.trim_ascii();
        if line.is_empty() || is_rem(line) { continue; }
        let Some(eq) = line.iter().position(|&b| b == b'=') else { continue };
        if !line[..eq].trim_ascii().eq_ignore_ascii_case(b"THREADS") { continue; }
        if let Some(n) = parse_number(&line[eq + 1..]) {
            found = Some(n);
        }
    }
    found
}

/// The session's thread limit: `env` (`WARPINE_THREADS`) when it is a
/// number, else the CONFIG.SYS statement, else the default; clamped.
pub fn threads_limit(config_sys: Option<&[u8]>, env: Option<&OsStr>) -> u32 {
    if let Some(n) = env.and_then(|v| parse_number(v.as_encoded_bytes())) {
        return clamp_threads(n);
    }
    if let Some(v) = env {
        log::warn!("{THREADS_ENV}={} is not a number; ignored", v.to_string_lossy());
    }
    config_sys.and_then(parse_threads).map_or(DEFAULT_THREADS, clamp_threads)
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

/// The limit of a session started now: `WARPINE_THREADS`, else drive C's
/// CONFIG.SYS.
pub fn session_limit(dm: &DriveManager) -> u32 {
    let env = std::env::var_os(THREADS_ENV);
    let text = if env.is_some() { None } else { read_config_sys(dm) };
    threads_limit(text.as_deref(), env.as_deref())
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
