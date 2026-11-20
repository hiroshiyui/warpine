// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Who uses a named-pipe session directory, and removing the directories
//! of sessions that are gone.
//!
//! A session directory (`npipe.rs`, `WARPINE_PIPE_DIR`,
//! `$XDG_RUNTIME_DIR/warpine-pipes-<uid>-<pid>`) holds the sockets of the
//! pipes its processes serve. A Warpine process ended by `SIGKILL` (or a
//! crash) cannot remove its sockets or the directory, so they stayed for
//! ever. Now:
//!
//! - every process that uses the directory records itself in it as
//!   `p-<pid>` (its pid and start time — field 22 of `/proc/<pid>/stat`, so
//!   a reused pid is not mistaken for it, as `session_threads.rs` does)
//!   under an exclusive `flock` on the directory, and removes the record
//!   when it ends normally;
//! - the first process of a new session looks at the other
//!   `warpine-pipes-<uid>-<n>` directories beside its own and removes one
//!   only when, under a non-blocking exclusive lock, none of its recorded
//!   processes lives (a directory without records — an older Warpine's —
//!   only when process `<n>` is gone) and it holds nothing but sockets
//!   (`np-<16 hex digits>`) and records. A directory locked by another
//!   process, not owned by this user, open to others, or holding anything
//!   else is left alone;
//! - a process that waited for the lock while its directory was removed
//!   sees the directory gone (link count 0) and makes it again.
//!
//! Everything read from the directories is treated as hostile: names are
//! parsed strictly, records are at most `MAX_RECORD` bytes, files are
//! opened without following links, and at most `MAX_ENTRIES` entries are
//! looked at per directory (`MAX_SESSIONS` directories per sweep).

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use super::session_threads::{Identity, StartTimeFn};

/// Name prefix of a member record.
const MEMBER_PREFIX: &str = "p-";
/// Name prefix of a pipe socket (`npipe.rs`), followed by 16 hex digits.
const SOCKET_PREFIX: &str = "np-";
const SOCKET_HEX_DIGITS: usize = 16;
/// Suffix of a record being written (`.p-<pid>.tmp`).
const PARTIAL_SUFFIX: &str = ".tmp";
/// Name prefix of the session directories (`warpine-pipes-<uid>-<pid>`).
pub const SESSION_PREFIX: &str = "warpine-pipes-";
/// Largest record file.
const MAX_RECORD: usize = 64;
/// Most entries looked at in one session directory.
const MAX_ENTRIES: usize = 4096;
/// Most directories looked at in one sweep.
const MAX_SESSIONS: usize = 1024;
/// Largest pid Linux hands out (`PID_MAX_LIMIT`).
const PID_MAX: u32 = 4_194_304;

/// A number made of 1 … 10 decimal digits, nothing else.
fn parse_pid(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 || !s.bytes().all(|b| b.is_ascii_digit()) { return None; }
    s.parse().ok().filter(|&p| p > 0 && p <= PID_MAX)
}

/// What an entry of a session directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Member(u32),
    Socket,
    /// A record being written (`join`) by a process that may have died.
    Partial,
}

fn parse_entry(name: &OsStr) -> Option<Entry> {
    let name = name.to_str()?;
    if let Some(pid) = name.strip_prefix(MEMBER_PREFIX) {
        return parse_pid(pid).map(Entry::Member);
    }
    if let Some(pid) = name.strip_prefix('.').and_then(|n| n.strip_prefix(MEMBER_PREFIX))
        .and_then(|n| n.strip_suffix(PARTIAL_SUFFIX)) {
        return parse_pid(pid).map(|_| Entry::Partial);
    }
    let hex = name.strip_prefix(SOCKET_PREFIX)?;
    (hex.len() == SOCKET_HEX_DIGITS && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(Entry::Socket)
}

fn member_name(pid: u32) -> String { format!("{MEMBER_PREFIX}{pid}") }

fn encode(who: Identity) -> String { format!("{} {}\n", who.pid, who.start) }

fn decode(bytes: &[u8]) -> Option<Identity> {
    if bytes.len() > MAX_RECORD { return None; }
    let text = std::str::from_utf8(bytes).ok()?.strip_suffix('\n')?;
    let (pid, start) = text.split_once(' ')?;
    let start_ok = !start.is_empty() && start.len() <= 20 && start.bytes().all(|b| b.is_ascii_digit());
    Some(Identity { pid: parse_pid(pid)?, start: start_ok.then(|| start.parse().ok()).flatten()? })
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Whether `dir` (open) is a directory of this user closed to others.
fn private(dir: &File) -> bool {
    dir.metadata().is_ok_and(|m| m.is_dir() && m.uid() == euid() && m.mode() & 0o077 == 0)
}

fn open_dir(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(path)
}

/// Record process `me` as a user of session directory `dir` (open, at
/// `path`). False when the directory was removed meanwhile (the caller
/// makes it again).
pub fn join(dir: &File, path: &Path, me: Identity) -> std::io::Result<bool> {
    dir.lock()?;
    let alive = dir.metadata().is_ok_and(|m| m.nlink() > 0);
    let written = if alive {
        // Written whole then renamed, so a reader never sees half a record.
        let tmp = path.join(format!(".{}{PARTIAL_SUFFIX}", member_name(me.pid)));
        std::fs::write(&tmp, encode(me))
            .and_then(|()| std::fs::rename(&tmp, path.join(member_name(me.pid))))
    } else {
        Ok(())
    };
    let _ = dir.unlock();
    written.map(|()| alive)
}

/// Remove process `pid`'s record from the session directory at `path`.
pub fn leave(path: &Path, pid: u32) {
    let _ = std::fs::remove_file(path.join(member_name(pid)));
}

/// The record in regular file `name` of `dir` (`None`: malformed, or not a
/// regular file).
fn read_record(dir: &Path, name: &OsStr) -> Option<Identity> {
    let f = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join(name)).ok()?;
    if !f.metadata().ok()?.is_file() { return None; }
    let mut buf = Vec::with_capacity(MAX_RECORD);
    f.take(MAX_RECORD as u64 + 1).read_to_end(&mut buf).ok()?;
    decode(&buf)
}

/// Whether process `who` still runs.
fn lives(who: Identity, start_time: &StartTimeFn) -> bool {
    start_time(who.pid) == Some(who.start)
}

/// Remove session directory `path` if it is stale (see the module
/// documentation); `name_pid` is the pid in its name. True when removed.
fn remove_if_stale(path: &Path, name_pid: u32, start_time: &StartTimeFn) -> bool {
    let Ok(dir) = open_dir(path) else { return false };
    if !private(&dir) { return false; }
    // Another process is joining it or looking at it: leave it.
    if dir.try_lock().is_err() { return false; }
    let Ok(entries) = std::fs::read_dir(path) else { return false };
    let mut names = Vec::new();
    let mut members = 0usize;
    for entry in entries.take(MAX_ENTRIES + 1) {
        let Ok(entry) = entry else { return false };
        let name = entry.file_name();
        match parse_entry(&name) {
            Some(Entry::Member(pid)) => {
                members += 1;
                match read_record(path, &name) {
                    Some(who) if who.pid == pid && lives(who, start_time) => return false,
                    // Its process is gone, or the record is junk.
                    _ => {}
                }
            }
            // Written under the lock this sweep holds: left by a process
            // that died while it wrote.
            Some(Entry::Socket | Entry::Partial) => {}
            None => return false,
        }
        names.push(name);
    }
    if names.len() > MAX_ENTRIES { return false; }
    // A directory of an older Warpine has no records: the process that
    // made it must be gone (its start time is not known; a reused pid
    // keeps the directory, which is safe).
    if members == 0 && start_time(name_pid).is_some() { return false; }
    for name in &names {
        if let Err(e) = std::fs::remove_file(path.join(name))
            && e.kind() != ErrorKind::NotFound {
            return false;
        }
    }
    std::fs::remove_dir(path).is_ok()
}

/// Remove the stale session directories of this user in `base`, except
/// `own` (the caller's). Returns how many were removed.
pub fn sweep(base: &Path, own: &Path, start_time: &StartTimeFn) -> usize {
    let prefix = format!("{SESSION_PREFIX}{}-", euid());
    let Ok(entries) = std::fs::read_dir(base) else { return 0 };
    let mut removed = 0;
    for entry in entries.take(MAX_SESSIONS).flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix(&prefix)).and_then(parse_pid) else { continue };
        let path = entry.path();
        if path == own { continue; }
        if remove_if_stale(&path, pid, start_time) {
            log::debug!("removed the stale pipe directory {}", path.display());
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::sync::{Arc, Mutex};

    /// Processes that live: pid → start time.
    fn processes(live: &[(u32, u64)]) -> (Arc<Mutex<HashMap<u32, u64>>>, StartTimeFn) {
        let table = Arc::new(Mutex::new(live.iter().copied().collect::<HashMap<_, _>>()));
        let t = Arc::clone(&table);
        (table, Arc::new(move |pid| t.lock().unwrap().get(&pid).copied()))
    }

    fn session(base: &Path, pid: u32) -> std::path::PathBuf {
        let p = base.join(format!("{SESSION_PREFIX}{}-{pid}", euid()));
        std::fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        p
    }

    fn join_as(path: &Path, pid: u32, start: u64) {
        let dir = open_dir(path).unwrap();
        assert!(join(&dir, path, Identity { pid, start }).unwrap());
    }

    #[test]
    fn names_and_records_are_parsed_strictly() {
        assert_eq!(parse_entry(OsStr::new("p-12")), Some(Entry::Member(12)));
        assert_eq!(parse_entry(OsStr::new("np-0123456789abcdef")), Some(Entry::Socket));
        assert_eq!(parse_entry(OsStr::new(".p-1.tmp")), Some(Entry::Partial));
        for bad in ["p-", "p-0", "p-x", "p-99999999999", "np-123", "np-0123456789abcdeg", "x", ".p-x.tmp", ".p-1"] {
            assert_eq!(parse_entry(OsStr::new(bad)), None, "{bad}");
        }
        let who = Identity { pid: 7, start: 99 };
        assert_eq!(decode(encode(who).as_bytes()), Some(who));
        for bad in ["7 99", "7  99\n", "x 99\n", "7 \n", "0 1\n", "7 99 1\n", ""] {
            assert_eq!(decode(bad.as_bytes()), None, "{bad:?}");
        }
        assert_eq!(decode(&[b'1'; MAX_RECORD + 1]), None);
    }

    #[test]
    fn a_session_whose_processes_are_gone_is_removed() {
        let base = tempfile::tempdir().unwrap();
        let own = session(base.path(), 1);
        let (table, f) = processes(&[(1, 10), (20, 5), (30, 6)]);
        let dead = session(base.path(), 20);
        join_as(&dead, 20, 5);
        std::fs::write(dead.join("np-0123456789abcdef"), b"").unwrap();
        let live = session(base.path(), 30);
        join_as(&live, 30, 6);
        assert_eq!(sweep(base.path(), &own, &f), 0, "all alive");
        table.lock().unwrap().remove(&20);
        assert_eq!(sweep(base.path(), &own, &f), 1);
        assert!(!dead.exists());
        assert!(live.exists() && own.exists());
    }

    #[test]
    fn a_reused_pid_does_not_keep_a_session_but_a_live_member_does() {
        let base = tempfile::tempdir().unwrap();
        let own = base.path().join("own");
        let s = session(base.path(), 40);
        join_as(&s, 40, 1);
        // The creator is gone, its pid reused by another process (other
        // start time); a child of the session still lives.
        join_as(&s, 41, 2);
        let (table, f) = processes(&[(40, 999), (41, 2)]);
        assert_eq!(sweep(base.path(), &own, &f), 0, "the child lives");
        table.lock().unwrap().remove(&41);
        assert_eq!(sweep(base.path(), &own, &f), 1);
    }

    #[test]
    fn foreign_or_busy_directories_are_left_alone() {
        let base = tempfile::tempdir().unwrap();
        let own = base.path().join("own");
        let (_t, f) = processes(&[]);
        // Something that is not Warpine's in it.
        let odd = session(base.path(), 50);
        std::fs::write(odd.join("notes.txt"), b"x").unwrap();
        // Open to others.
        let open = session(base.path(), 51);
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Locked by a process joining it.
        let busy = session(base.path(), 52);
        let lock = open_dir(&busy).unwrap();
        lock.lock().unwrap();
        // Not a session directory name.
        let other = base.path().join(format!("{SESSION_PREFIX}{}-x", euid()));
        std::fs::create_dir(&other).unwrap();
        // A symbolic link to a directory.
        let target = tempfile::tempdir().unwrap();
        let link = base.path().join(format!("{SESSION_PREFIX}{}-53", euid()));
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        assert_eq!(sweep(base.path(), &own, &f), 0);
        for p in [&odd, &open, &busy, &other] { assert!(p.exists(), "{}", p.display()); }
        assert!(target.path().exists());
        lock.unlock().unwrap();
        assert_eq!(sweep(base.path(), &own, &f), 1, "unlocked, empty, creator gone");
    }

    #[test]
    fn an_old_session_without_records_goes_with_its_creator() {
        let base = tempfile::tempdir().unwrap();
        let own = base.path().join("own");
        let s = session(base.path(), 60);
        std::fs::write(s.join("np-00000000000000ff"), b"").unwrap();
        let (table, f) = processes(&[(60, 1)]);
        assert_eq!(sweep(base.path(), &own, &f), 0);
        table.lock().unwrap().clear();
        assert_eq!(sweep(base.path(), &own, &f), 1);
    }

    #[test]
    fn joining_a_removed_directory_says_so_and_leaving_removes_the_record() {
        let base = tempfile::tempdir().unwrap();
        let s = session(base.path(), 70);
        let dir = open_dir(&s).unwrap();
        assert!(join(&dir, &s, Identity { pid: 70, start: 3 }).unwrap());
        assert!(s.join("p-70").exists());
        leave(&s, 70);
        assert!(!s.join("p-70").exists());
        std::fs::remove_dir(&s).unwrap();
        assert!(!join(&dir, &s, Identity { pid: 70, start: 3 }).unwrap(), "gone");
    }
}
