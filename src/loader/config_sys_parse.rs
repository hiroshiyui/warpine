// SPDX-License-Identifier: GPL-3.0-or-later
//
//! The text of `C:\CONFIG.SYS` parsed (`config_sys.rs` reads the file and
//! re-exports these): pure, with no I/O, so the fuzz target `config_sys`
//! includes it alone. The statements and the parsing rules are described
//! in `config_sys.rs`.

use std::ffi::OsStr;


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
pub(crate) fn keyword_rest<'a>(line: &'a [u8], kw: &[u8]) -> Option<&'a [u8]> {
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
pub(crate) fn parse_number(s: &[u8]) -> Option<u64> {
    let s = s.trim_ascii();
    if s.is_empty() || s.len() > MAX_DIGITS || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(s.iter().fold(0u64, |n, &d| n.saturating_mul(10).saturating_add(u64::from(d - b'0'))))
}

/// Whether `line` (leading blanks removed) is a `REM` comment.
pub(crate) fn is_rem(line: &[u8]) -> bool {
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
pub(crate) fn limit_of(threads: Option<u64>, env: Option<&OsStr>) -> u32 {
    if let Some(n) = env.and_then(|v| parse_number(v.as_encoded_bytes())) {
        return clamp_threads(n);
    }
    if let Some(v) = env {
        log::warn!("{THREADS_ENV}={} is not a number; ignored", v.to_string_lossy());
    }
    threads.map_or(DEFAULT_THREADS, clamp_threads)
}
