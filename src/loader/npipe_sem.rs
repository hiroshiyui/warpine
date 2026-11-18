// SPDX-License-Identifier: GPL-3.0-only
//
//! Pipe semaphores: `DosSetNPipeSem` (DOSCALLS 251) and
//! `DosQueryNPipeSemState` (DOSCALLS 249) on the named pipes of
//! `npipe.rs` / `npipe_api.rs`.
//!
//! From the Control Program Reference (Toolkit 4.5 pages and EDM2):
//! - `DosSetNPipeSem(hpipe, hsem, key)` attaches an event semaphore to one
//!   end of a local named pipe; the system posts it when that end has data
//!   to read or write space (the end "whichever is reading"); one semaphore
//!   per end, a new one replaces the old; `key` tells pipes sharing one
//!   semaphore apart;
//! - `DosQueryNPipeSemState(hsem, pnpss, cbBuf)` writes one `PIPESEMSTATE`
//!   (`fStatus`, `fFlag`, `usKey`, `usAvail`; 6 bytes) per state of every
//!   pipe end the semaphore is attached to — `NPSS_RDATA` with the bytes to
//!   read, `NPSS_WSPACE` with the room to write, `NPSS_CLOSE` — and an
//!   `NPSS_EOI` record after the last; `ERROR_BUFFER_OVERFLOW` when they do
//!   not fit.
//!
//! Warpine's choices where the documents say nothing (unverified):
//! - the semaphore is also posted when the other end closes (or the server
//!   disconnects), since `NPSS_CLOSE` reports it; and once on attaching
//!   when data is already waiting;
//! - only event semaphores are accepted (the reference also names muxwait
//!   semaphores, without saying what posting one means):
//!   `ERROR_SEM_NOT_FOUND` otherwise; a semaphore created without
//!   `DC_SEM_SHARED` is accepted too; `hsem` 0 detaches;
//! - a key above 0xFFFF (`usKey` is a USHORT) is `ERROR_INVALID_PARAMETER`;
//! - attaching to a server end that has no client yet works (it is posted
//!   once a client connects and writes), so `ERROR_PIPE_NOT_CONNECTED` is
//!   never returned;
//! - an end that is not connected reports nothing; a closed one only
//!   `NPSS_CLOSE` (after `NPSS_RDATA` while unread data is left); an open
//!   one `NPSS_RDATA` when data waits and `NPSS_WSPACE` when it may write,
//!   with the pipe's buffer size for that direction (at most 0xFFFF) as the
//!   room; `fFlag` is always 0 (whether a thread waits at the other end,
//!   which may be another process, is not known);
//! - on `ERROR_BUFFER_OVERFLOW` nothing is written; the query does not
//!   reset the semaphore.
//!
//! The other end of a pipe may be in another Warpine process, so the posts
//! cannot come from the writer: every attachment has a watcher thread that
//! polls its end's socket (`npipe::poll_revents`, `WATCH_MS`) and posts the
//! semaphore when unread data grows, when the end becomes closed, and when
//! the socket has room again after a write found it full
//! (`Conn::write_blocked`). The watcher holds the end weakly and stops when
//! the end is gone (its handles closed), the attachment is replaced or
//! removed, the semaphore is closed, or the process ends.
//!
//! Locks: `SharedState::pipe_sems` is taken alone — no other `SharedState`
//! lock is held while it is, and none is taken inside it (only a pipe end's
//! own state and receive locks).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use log::debug;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::npipe::{self, ClientEnd, Conn, ServerInstance};
use super::npipe_api::NpEnd;
use super::SharedState;

/// How long a watcher waits on its socket before it looks at its stop
/// conditions again.
const WATCH_MS: i32 = 50;

/// A pipe end, held weakly.
#[derive(Clone)]
enum WeakEnd {
    Server(Weak<ServerInstance>),
    Client(Weak<ClientEnd>),
}

impl WeakEnd {
    fn of(end: &NpEnd) -> Self {
        match end {
            NpEnd::Server(i) => WeakEnd::Server(Arc::downgrade(i)),
            NpEnd::Client(c) => WeakEnd::Client(Arc::downgrade(c)),
        }
    }

    fn upgrade(&self) -> Option<NpEnd> {
        match self {
            WeakEnd::Server(w) => w.upgrade().map(NpEnd::Server),
            WeakEnd::Client(w) => w.upgrade().map(NpEnd::Client),
        }
    }

    fn same(&self, other: &WeakEnd) -> bool {
        match (self, other) {
            (WeakEnd::Server(a), WeakEnd::Server(b)) => a.ptr_eq(b),
            (WeakEnd::Client(a), WeakEnd::Client(b)) => a.ptr_eq(b),
            _ => false,
        }
    }
}

struct Attachment {
    end: WeakEnd,
    hsem: u32,
    key: u16,
    /// Tells the watcher to end.
    stop: Arc<AtomicBool>,
}

/// The pipe ends that have a semaphore attached, in attaching order.
#[derive(Default)]
pub struct PipeSemTable {
    list: Vec<Attachment>,
}

impl PipeSemTable {
    /// Drop attachments whose end is gone, stopping their watchers.
    fn prune(&mut self) {
        self.list.retain(|a| {
            let live = a.end.upgrade().is_some();
            if !live { a.stop.store(true, Ordering::Relaxed); }
            live
        });
    }

    /// Remove the attachment of `end`, if any.
    fn detach(&mut self, end: &WeakEnd) {
        self.list.retain(|a| {
            let it = a.end.same(end);
            if it { a.stop.store(true, Ordering::Relaxed); }
            !it
        });
    }

    /// Remove the attachment whose watcher has `stop`.
    fn remove_watcher(&mut self, stop: &Arc<AtomicBool>) {
        self.list.retain(|a| !Arc::ptr_eq(&a.stop, stop));
    }

    /// Stop every watcher (the process ends).
    pub fn stop_all(&mut self) {
        for a in self.list.drain(..) {
            a.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Attachments in the table (tests and tracing).
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// One `PIPESEMSTATE` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipeSemState {
    pub status: u8,
    pub flag: u8,
    pub key: u16,
    pub avail: u16,
}

impl PipeSemState {
    fn bytes(&self) -> [u8; PIPESEMSTATE_SIZE as usize] {
        let k = self.key.to_le_bytes();
        let a = self.avail.to_le_bytes();
        [self.status, self.flag, k[0], k[1], a[0], a[1]]
    }
}

/// The records of one attached end (none when it is not connected).
fn end_records(end: &NpEnd, key: u16) -> Vec<PipeSemState> {
    let Ok(conn) = end.conn() else { return Vec::new() };
    let (unread, closed) = conn.watch_state();
    let record = |status, avail: usize| PipeSemState {
        status, flag: 0, key, avail: avail.min(0xFFFF) as u16,
    };
    let mut out = Vec::new();
    if unread > 0 {
        out.push(record(NPSS_RDATA, unread));
    }
    if closed {
        out.push(record(NPSS_CLOSE, 0));
    } else if end.may_write(OPEN_ACCESS_READWRITE) {
        let spec = end.spec();
        let room = match end {
            NpEnd::Server(_) => spec.cb_out,
            NpEnd::Client(_) => spec.cb_in,
        };
        out.push(record(NPSS_WSPACE, room as usize));
    }
    out
}

/// What a watcher last saw on its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Seen {
    unread: usize,
    closed: bool,
}

/// Whether the change from `old` to `new` posts the semaphore: more data
/// to read, or the end newly closed.
fn worth_a_post(old: Seen, new: Seen) -> bool {
    new.unread > old.unread || (new.closed && !old.closed)
}

/// The watcher of one attachment (see the module notes).
fn watch(shared: Weak<SharedState>, end: WeakEnd, hsem: u32, stop: Arc<AtomicBool>) {
    let mut seen = Seen::default();
    let mut current: Option<Arc<Conn>> = None;
    loop {
        if stop.load(Ordering::Relaxed) { return; }
        let Some(state) = shared.upgrade() else { return };
        if state.exit_requested.load(Ordering::Relaxed) { return; }
        drop(state);
        // The connection (a server end may get a new one after a
        // disconnect); the end itself is not held across the wait.
        let Some(conn) = end.upgrade().map(|e| e.conn()) else { break };
        let Ok(conn) = conn else {
            current = None;
            seen = Seen::default();
            std::thread::sleep(Duration::from_millis(WATCH_MS as u64));
            continue;
        };
        if !current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &conn)) {
            current = Some(conn.clone());
            seen = Seen::default();
        }
        let want_room = conn.write_blocked.load(Ordering::Relaxed);
        let events = libc::POLLIN | if want_room { libc::POLLOUT } else { 0 };
        let revents = npipe::poll_revents(conn.raw_fd(), events, WATCH_MS);
        let (unread, closed) = conn.watch_state();
        let now = Seen { unread, closed };
        let mut post = worth_a_post(seen, now);
        if want_room && revents & libc::POLLOUT != 0 {
            conn.write_blocked.store(false, Ordering::Relaxed);
            post = true;
        }
        if !post && revents != 0 && now == seen {
            // Readable but nothing new (a closed socket, or a full receive
            // queue): do not spin.
            std::thread::sleep(Duration::from_millis(WATCH_MS as u64));
        }
        seen = now;
        drop(conn);
        if post {
            let Some(state) = shared.upgrade() else { return };
            let rc = state.sem_mgr.lock_or_recover().post_event(hsem);
            debug!("  pipe semaphore {hsem} posted -> {rc}");
            if rc == ERROR_INVALID_HANDLE {
                // The semaphore was closed: the attachment ends.
                state.pipe_sems.lock_or_recover().remove_watcher(&stop);
                return;
            }
        }
    }
    // The end is gone.
    if let Some(state) = shared.upgrade() {
        state.pipe_sems.lock_or_recover().remove_watcher(&stop);
    }
}

impl super::Loader {
    /// DosSetNPipeSem (ordinal 251): attach event semaphore `hsem` to pipe
    /// end `hpipe` with `key` (0 detaches).
    pub fn dos_set_npipe_sem(&self, hpipe: u32, hsem: u32, key: u32) -> u32 {
        debug!("  DosSetNPipeSem(hpipe={hpipe}, hsem={hsem}, key={key})");
        let Some((end, _)) = self.np_end(hpipe) else {
            return if self.handle_target(hpipe).is_some() { ERROR_BAD_PIPE } else { ERROR_INVALID_HANDLE };
        };
        let Ok(key) = u16::try_from(key) else { return ERROR_INVALID_PARAMETER };
        let weak = WeakEnd::of(&end);
        if hsem == 0 {
            let mut table = self.shared.pipe_sems.lock_or_recover();
            table.prune();
            table.detach(&weak);
            return NO_ERROR;
        }
        if self.shared.sem_mgr.lock_or_recover().get_event(hsem).is_none() {
            return ERROR_SEM_NOT_FOUND;
        }
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut table = self.shared.pipe_sems.lock_or_recover();
            table.prune();
            table.detach(&weak);
            table.list.push(Attachment { end: weak.clone(), hsem, key, stop: stop.clone() });
        }
        let shared = Arc::downgrade(&self.shared);
        let s2 = stop.clone();
        let spawned = std::thread::Builder::new().name("npipe-sem".into())
            .spawn(move || watch(shared, weak, hsem, s2));
        if spawned.is_err() {
            self.shared.pipe_sems.lock_or_recover().remove_watcher(&stop);
            return ERROR_NOT_ENOUGH_MEMORY;
        }
        NO_ERROR
    }

    /// The `PIPESEMSTATE` records of semaphore `hsem`, `NPSS_EOI` last.
    pub(crate) fn npipe_sem_records(&self, hsem: u32) -> Vec<PipeSemState> {
        let ends: Vec<(NpEnd, u16)> = {
            let mut table = self.shared.pipe_sems.lock_or_recover();
            table.prune();
            table.list.iter().filter(|a| a.hsem == hsem)
                .filter_map(|a| a.end.upgrade().map(|e| (e, a.key)))
                .collect()
        };
        let mut out: Vec<PipeSemState> = ends.iter().flat_map(|(e, k)| end_records(e, *k)).collect();
        out.push(PipeSemState { status: NPSS_EOI, flag: 0, key: 0, avail: 0 });
        out
    }

    /// DosQueryNPipeSemState (ordinal 249): the state of every pipe end
    /// semaphore `hsem` is attached to.
    pub fn dos_query_npipe_sem_state(&self, hsem: u32, pnpss: u32, cb_buf: u32) -> u32 {
        if self.shared.sem_mgr.lock_or_recover().get_event(hsem).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        let records = self.npipe_sem_records(hsem);
        let bytes: Vec<u8> = records.iter().flat_map(|r| r.bytes()).collect();
        if (cb_buf as usize) < bytes.len() {
            return ERROR_BUFFER_OVERFLOW;
        }
        if pnpss == 0 || self.guest_write_bytes(pnpss, &bytes).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        NO_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Loader;
    use std::time::Instant;

    const PH: u32 = 0x3000;
    const PH2: u32 = 0x3004;
    const PACT: u32 = 0x3008;
    const PNAME: u32 = 0x3100;
    const PBUF: u32 = 0x3200;
    const PSTATE: u32 = 0x3400;

    fn put_str(l: &Loader, at: u32, s: &str) -> u32 {
        l.guest_write_bytes(at, s.as_bytes()).unwrap();
        l.guest_write::<u8>(at + s.len() as u32, 0).unwrap();
        at
    }

    /// A connected duplex byte pipe: (server, client) handles.
    fn pipe_pair(l: &Loader, name: &str) -> (u32, u32) {
        let pname = put_str(l, PNAME, name);
        assert_eq!(l.dos_create_npipe(pname, PH, NP_ACCESS_DUPLEX, NP_NOWAIT | 1, 4096, 2048, 0), NO_ERROR);
        let hs = l.guest_read::<u32>(PH).unwrap();
        // Non-blocking: listens and returns at once.
        assert_eq!(l.dos_connect_npipe(hs), ERROR_PIPE_NOT_CONNECTED);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match l.np_open_client(name, PH2, PACT, OPEN_ACCESS_READWRITE) {
                NO_ERROR => break,
                ERROR_PIPE_BUSY if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                e => panic!("open: {e}"),
            }
        }
        let hc = l.guest_read::<u32>(PH2).unwrap();
        while l.dos_connect_npipe(hs) != NO_ERROR {
            assert!(Instant::now() < deadline, "never connected");
            std::thread::sleep(Duration::from_millis(5));
        }
        (hs, hc)
    }

    fn event(l: &Loader) -> u32 {
        assert_eq!(l.dos_create_event_sem(0, PH, DC_SEM_SHARED, 0), NO_ERROR);
        l.guest_read::<u32>(PH).unwrap()
    }

    fn post_count(l: &Loader, h: u32) -> u32 {
        l.shared.sem_mgr.lock_or_recover().get_event(h).unwrap().0.lock_or_recover().post_count
    }

    fn write(l: &Loader, h: u32, data: &[u8]) {
        let (end, st) = l.np_end(h).unwrap();
        assert_eq!(l.np_write(&end, st, data), Ok(data.len() as u32));
    }

    fn wait_posted(l: &Loader, hev: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while post_count(l, hev) == 0 {
            assert!(Instant::now() < deadline, "never posted");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn query(l: &Loader, hev: u32) -> Vec<PipeSemState> {
        l.npipe_sem_records(hev)
    }

    fn mock() -> (Loader, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let l = Loader::new_mock();
        *l.shared.npipes.lock_or_recover() = npipe::NamedPipes::new(npipe::PipeDir::at(dir.path().join("p")));
        (l, dir)
    }

    #[test]
    fn records_encode_little_endian() {
        let r = PipeSemState { status: NPSS_RDATA, flag: NPSS_WAIT, key: 0x1234, avail: 0xABCD };
        assert_eq!(r.bytes(), [1, 1, 0x34, 0x12, 0xCD, 0xAB]);
    }

    #[test]
    fn posts_follow_data_and_close() {
        assert!(worth_a_post(Seen::default(), Seen { unread: 3, closed: false }));
        assert!(!worth_a_post(Seen { unread: 3, closed: false }, Seen { unread: 1, closed: false }));
        assert!(worth_a_post(Seen { unread: 0, closed: false }, Seen { unread: 0, closed: true }));
        assert!(!worth_a_post(Seen { unread: 0, closed: true }, Seen { unread: 0, closed: true }));
    }

    #[test]
    fn set_checks_its_arguments() {
        let (l, _dir) = mock();
        let (hs, _hc) = pipe_pair(&l, "\\PIPE\\SEM\\ARGS");
        let hev = event(&l);
        assert_eq!(l.dos_set_npipe_sem(0x7777, hev, 1), ERROR_INVALID_HANDLE);
        assert_eq!(l.dos_set_npipe_sem(1, hev, 1), ERROR_BAD_PIPE, "stdout is no pipe");
        assert_eq!(l.dos_set_npipe_sem(hs, 9999, 1), ERROR_SEM_NOT_FOUND);
        assert_eq!(l.dos_set_npipe_sem(hs, hev, 0x1_0000), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_set_npipe_sem(hs, hev, 1), NO_ERROR);
        assert_eq!(l.dos_set_npipe_sem(hs, hev, 2), NO_ERROR, "replaces");
        assert_eq!(l.shared.pipe_sems.lock_or_recover().len(), 1);
        assert_eq!(l.dos_set_npipe_sem(hs, 0, 0), NO_ERROR, "detaches");
        assert!(l.shared.pipe_sems.lock_or_recover().is_empty());
        assert_eq!(l.dos_query_npipe_sem_state(9999, PSTATE, 64), ERROR_INVALID_PARAMETER);
    }

    #[test]
    fn data_write_space_and_close_are_reported() {
        let (l, _dir) = mock();
        let (hs, hc) = pipe_pair(&l, "\\PIPE\\SEM\\STATE");
        let hev = event(&l);
        assert_eq!(l.dos_set_npipe_sem(hs, hev, 7), NO_ERROR);
        // An open end that may write: write space only.
        assert_eq!(query(&l, hev), vec![
            PipeSemState { status: NPSS_WSPACE, flag: 0, key: 7, avail: 4096 },
            PipeSemState { status: NPSS_EOI, flag: 0, key: 0, avail: 0 },
        ]);
        // The client writes: the server's semaphore is posted.
        write(&l, hc, b"hello");
        wait_posted(&l, hev);
        assert_eq!(query(&l, hev)[0], PipeSemState { status: NPSS_RDATA, flag: 0, key: 7, avail: 5 });
        // The guest's view: the records and the EOI, 6 bytes each.
        assert_eq!(l.dos_query_npipe_sem_state(hev, PSTATE, 17), ERROR_BUFFER_OVERFLOW);
        assert_eq!(l.dos_query_npipe_sem_state(hev, PSTATE, 18), NO_ERROR);
        assert_eq!(l.guest_read::<u8>(PSTATE), Some(NPSS_RDATA));
        assert_eq!(l.guest_read::<u16>(PSTATE + 2), Some(7));
        assert_eq!(l.guest_read::<u16>(PSTATE + 4), Some(5));
        assert_eq!(l.guest_read::<u8>(PSTATE + 6), Some(NPSS_WSPACE));
        assert_eq!(l.guest_read::<u8>(PSTATE + 12), Some(NPSS_EOI));
        // The client closes: posted again, closed reported.
        assert_eq!(l.dos_reset_event_sem(hev, PBUF), NO_ERROR);
        assert!(l.close_pipe_handle(hc));
        wait_posted(&l, hev);
        let recs = query(&l, hev);
        assert_eq!(recs.iter().map(|r| r.status).collect::<Vec<_>>(), [NPSS_RDATA, NPSS_CLOSE, NPSS_EOI]);
        // The server's handle closed: the attachment goes.
        assert!(l.close_pipe_handle(hs));
        assert_eq!(query(&l, hev), vec![PipeSemState { status: NPSS_EOI, flag: 0, key: 0, avail: 0 }]);
        assert!(l.shared.pipe_sems.lock_or_recover().is_empty());
    }

    #[test]
    fn closing_the_semaphore_ends_the_watcher() {
        let (l, _dir) = mock();
        let (hs, hc) = pipe_pair(&l, "\\PIPE\\SEM\\GONE");
        let hev = event(&l);
        assert_eq!(l.dos_set_npipe_sem(hs, hev, 1), NO_ERROR);
        assert_eq!(l.dos_close_event_sem(hev), NO_ERROR);
        write(&l, hc, b"x");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !l.shared.pipe_sems.lock_or_recover().is_empty() {
            assert!(Instant::now() < deadline, "the watcher stayed");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
