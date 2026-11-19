// SPDX-License-Identifier: GPL-3.0-only

//! A Warpine process's link to the session's desktop server (ADR-0052).
//!
//! [`DesktopLink::open`] connects to the server listening in the session
//! directory, or starts it first (`warpine --desktop-server <dir>`) under
//! the directory's desktop lock, so that two processes starting together
//! start one server. After the handshake (`Hello` / `Welcome`: the slot that
//! prefixes this process's window handles, the desktop size) a reader
//! thread decodes the server's messages into an inbox and wakes the GUI
//! loop; any malformed message or the end of the connection marks the link
//! lost, and the process then ends its PM program.
//!
//! Calls on other programs' windows (phase B) go through the link too: an
//! API thread's [`DesktopLink::call`] waits for its answer at most
//! `CALL_TIMEOUT`, and the reader thread answers the calls other programs
//! make on this program's windows through the [`Responder`] — without
//! waiting for anything but the window manager's lock, which no thread
//! holds while it waits for an answer. A sent message is the exception on
//! both sides: the sender starts it with [`DesktopLink::begin_call`] and
//! comes back for the answer between the messages sent to itself, and the
//! receiver answers it with [`DesktopLink::answer`] once the window's
//! thread has run the window procedure.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::os::fd::BorrowedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::proto::{ClientMsg, ServerMsg, WinCall, WinReply, PROTOCOL_VERSION};
use super::transport::Conn;
use crate::loader::MutexExt;

/// `WARPINE_DESKTOP=local` keeps the desktop in this process (no server).
pub const DESKTOP_ENV: &str = "WARPINE_DESKTOP";
/// Where the server writes its log (default: nowhere).
pub const DESKTOP_LOG_ENV: &str = "WARPINE_DESKTOP_LOG";
/// Command-line switch that runs the desktop server.
pub const SERVER_SWITCH: &str = "--desktop-server";
/// The lock taken while the server is started or stops.
pub const LOCK_NAME: &str = "desktop.lock";
/// How long a new server may take to listen.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// Pause between connection attempts while it starts.
const START_POLL: Duration = Duration::from_millis(10);
/// How long a call on another program's window waits for its answer
/// (Warpine's choice, as `INPUT_STALL`).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Answers the calls other programs make on this program's windows.
pub trait Responder: Send + Sync {
    /// Make `call` (the server's call `id`) on window `hwnd` of this
    /// program: the reply, or `None` when it is given later through
    /// [`DesktopLink::answer`] (a sent message, run by the window's thread).
    fn answer(&self, id: u64, hwnd: u32, call: &WinCall) -> Option<WinReply>;
}

/// Called when the answer to a call arrives (or the link is lost).
pub type CallWaker = Arc<dyn Fn() + Send + Sync>;

/// A call waiting for its answer.
struct Waiting {
    reply: Option<WinReply>,
    wake: Option<CallWaker>,
}

/// This program's calls waiting for their answers.
#[derive(Default)]
struct Calls {
    waiting: HashMap<u32, Waiting>,
    next: u32,
}

/// Where a call started with [`DesktopLink::begin_call`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallState {
    /// No answer yet.
    Waiting,
    Answered(WinReply),
    /// The link is lost, or the call is unknown: no answer will come.
    Gone,
}

/// The desktop lock: an exclusive `flock` on `desktop.lock` in the session
/// directory, held while the file at the path is the one locked (the
/// server removes it when it stops).
pub struct DesktopLock {
    _file: File,
}

impl DesktopLock {
    pub fn take(dir: &Path) -> std::io::Result<DesktopLock> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let path = dir.join(LOCK_NAME);
        for _ in 0..100 {
            let file = std::fs::OpenOptions::new().read(true).write(true).create(true)
                .mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&path)?;
            file.lock()?;
            let held = file.metadata()?;
            let same = std::fs::symlink_metadata(&path).is_ok_and(|p| p.ino() == held.ino() && p.dev() == held.dev());
            if same {
                return Ok(DesktopLock { _file: file });
            }
        }
        Err(std::io::Error::other("desktop lock keeps disappearing"))
    }
}

/// Whether this process should use a desktop server: not with
/// `WARPINE_DESKTOP=local`.
pub fn server_wanted() -> bool {
    std::env::var(DESKTOP_ENV).map_or(true, |v| v != "local")
}

type Waker = Box<dyn Fn() + Send + Sync>;

/// The connection to the desktop server.
pub struct DesktopLink {
    conn: Arc<Conn>,
    /// This process's slot: the prefix of its window handles.
    pub slot: u32,
    /// Desktop size announced in `Welcome`.
    pub size: (u32, u32),
    inbox: Arc<Mutex<VecDeque<ServerMsg>>>,
    lost: Arc<AtomicBool>,
    waker: Arc<Mutex<Option<Waker>>>,
    calls: Arc<(Mutex<Calls>, Condvar)>,
    responder: Arc<Mutex<Option<Arc<dyn Responder>>>>,
}

impl DesktopLink {
    /// Connect to the server of session directory `dir`, starting it if
    /// none listens.
    pub fn open(dir: &Path) -> std::io::Result<Arc<DesktopLink>> {
        // Held through the handshake: a server about to stop takes this
        // lock first, so it cannot leave this process connecting to it.
        let _lock = DesktopLock::take(dir)?;
        let conn = {
            match Conn::connect(dir) {
                Ok(c) => c,
                Err(_) => {
                    start_server(dir)?;
                    let deadline = Instant::now() + START_TIMEOUT;
                    loop {
                        match Conn::connect(dir) {
                            Ok(c) => break c,
                            Err(e) if Instant::now() >= deadline => return Err(e),
                            Err(_) => std::thread::sleep(START_POLL),
                        }
                    }
                }
            }
        };
        Self::handshake(conn)
    }

    /// Say hello on `conn` and start reading.
    pub fn handshake(conn: Conn) -> std::io::Result<Arc<DesktopLink>> {
        conn.send(&ClientMsg::Hello { version: PROTOCOL_VERSION }.encode(), None)?;
        let bad = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string());
        let (packet, _) = conn.recv()?.ok_or_else(|| bad("desktop server closed the connection"))?;
        let ServerMsg::Welcome { version, slot, width, height } = ServerMsg::decode(&packet).map_err(|e| bad(&e.to_string()))?
            else { return Err(bad("no Welcome from the desktop server")) };
        if version != PROTOCOL_VERSION {
            return Err(bad("desktop protocol version mismatch"));
        }
        let link = Arc::new(DesktopLink {
            conn: Arc::new(conn), slot, size: (width, height),
            inbox: Arc::default(), lost: Arc::default(), waker: Arc::default(),
            calls: Arc::default(), responder: Arc::default(),
        });
        let (conn, inbox, lost, waker) = (link.conn.clone(), link.inbox.clone(), link.lost.clone(), link.waker.clone());
        let (calls, responder) = (link.calls.clone(), link.responder.clone());
        std::thread::Builder::new().name("desktop-link".into()).spawn(move || {
            loop {
                let msg = match conn.recv() {
                    Ok(Some((packet, None))) => ServerMsg::decode(&packet).ok(),
                    _ => None,
                };
                match msg {
                    Some(ServerMsg::Answered { id, reply }) => {
                        let (lock, cond) = &*calls;
                        // An answer nobody waits for any more is dropped.
                        let wake = lock.lock_or_recover().waiting.get_mut(&id).and_then(|w| {
                            w.reply = Some(reply);
                            cond.notify_all();
                            w.wake.clone()
                        });
                        if let Some(w) = wake { w(); }
                        continue;
                    }
                    Some(ServerMsg::Called { id, hwnd, call }) => {
                        let r = responder.lock_or_recover().clone();
                        let reply = match r {
                            Some(r) => r.answer(id, hwnd, &call),
                            None => Some(WinReply::Failed),
                        };
                        if let Some(reply) = reply
                            && conn.send(&ClientMsg::Answer { id, reply }.encode(), None).is_err() {
                            conn.shutdown();
                        }
                        continue;
                    }
                    Some(m) => inbox.lock_or_recover().push_back(m),
                    None => {
                        lost.store(true, Ordering::Release);
                        // Callers waiting for answers give up.
                        let (lock, cond) = &*calls;
                        let wakes: Vec<CallWaker> = lock.lock_or_recover().waiting.values()
                            .filter_map(|w| w.wake.clone()).collect();
                        cond.notify_all();
                        for w in wakes { w(); }
                        if let Some(w) = waker.lock_or_recover().as_ref() { w(); }
                        return;
                    }
                }
                if let Some(w) = waker.lock_or_recover().as_ref() { w(); }
            }
        })?;
        Ok(link)
    }

    /// Call `wake` whenever a message arrives or the link is lost.
    pub fn set_waker(&self, wake: Waker) {
        *self.waker.lock_or_recover() = Some(wake);
    }

    /// Send `msg`, with descriptor `fd` if given. A failure marks the link
    /// lost.
    pub fn send(&self, msg: &ClientMsg, fd: Option<BorrowedFd<'_>>) -> bool {
        let ok = self.conn.send(&msg.encode(), fd).is_ok();
        if !ok {
            self.lost.store(true, Ordering::Release);
        }
        ok
    }

    /// The messages received since the last call.
    pub fn take(&self) -> Vec<ServerMsg> {
        self.inbox.lock_or_recover().drain(..).collect()
    }

    /// The server is gone or misbehaved.
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Acquire) && self.inbox.lock_or_recover().is_empty()
    }

    /// Answer other programs' calls on this program's windows with
    /// `responder` (until then they fail).
    pub fn set_responder(&self, responder: Arc<dyn Responder>) {
        *self.responder.lock_or_recover() = Some(responder);
    }

    /// Make `call` on window `hwnd` of another program and wait for the
    /// answer: `None` when none came within `CALL_TIMEOUT` or the link is
    /// lost. Never call it while holding the window manager's lock: the
    /// other program may be calling this one meanwhile.
    pub fn call(&self, hwnd: u32, call: WinCall) -> Option<WinReply> {
        let id = self.begin_call(hwnd, call, None)?;
        let deadline = Instant::now() + CALL_TIMEOUT;
        loop {
            let now = Instant::now();
            match self.wait_call(id, deadline.saturating_duration_since(now)) {
                CallState::Answered(r) => return Some(r),
                CallState::Gone => return None,
                CallState::Waiting if now >= deadline => {
                    self.abandon_call(id);
                    return None;
                }
                CallState::Waiting => {}
            }
        }
    }

    /// Send `call` on window `hwnd` of another program without waiting:
    /// its id for [`Self::poll_call`] / [`Self::wait_call`], `None` when it
    /// could not be sent. `wake` is called when the answer arrives or the
    /// link is lost.
    pub fn begin_call(&self, hwnd: u32, call: WinCall, wake: Option<CallWaker>) -> Option<u32> {
        let id = {
            let mut c = self.calls.0.lock_or_recover();
            c.next = c.next.wrapping_add(1);
            let id = c.next;
            c.waiting.insert(id, Waiting { reply: None, wake });
            id
        };
        if self.send(&ClientMsg::Call { id, hwnd, call }, None) {
            Some(id)
        } else {
            self.abandon_call(id);
            None
        }
    }

    /// Where call `id` is; an answered or gone call is forgotten.
    pub fn poll_call(&self, id: u32) -> CallState {
        let mut c = self.calls.0.lock_or_recover();
        self.call_state(&mut c, id)
    }

    /// [`Self::poll_call`] after waiting at most `timeout` for the answer.
    pub fn wait_call(&self, id: u32, timeout: Duration) -> CallState {
        let (lock, cond) = &*self.calls;
        let mut c = lock.lock_or_recover();
        let state = self.call_state(&mut c, id);
        if state != CallState::Waiting || timeout.is_zero() {
            return state;
        }
        c = cond.wait_timeout(c, timeout).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        self.call_state(&mut c, id)
    }

    fn call_state(&self, c: &mut Calls, id: u32) -> CallState {
        match c.waiting.get(&id) {
            Some(Waiting { reply: Some(_), .. }) => {
                c.waiting.remove(&id).and_then(|w| w.reply).map_or(CallState::Gone, CallState::Answered)
            }
            Some(_) if !self.lost.load(Ordering::Acquire) => CallState::Waiting,
            Some(_) => {
                c.waiting.remove(&id);
                CallState::Gone
            }
            None => CallState::Gone,
        }
    }

    /// Stop waiting for call `id`: a late answer is dropped.
    pub fn abandon_call(&self, id: u32) {
        self.calls.0.lock_or_recover().waiting.remove(&id);
    }

    /// Answer call `id` that another program made on this program's
    /// window (one the [`Responder`] left for later).
    pub fn answer(&self, id: u64, reply: WinReply) -> bool {
        self.send(&ClientMsg::Answer { id, reply }, None)
    }

    /// Whether frame handle `hwnd` is this process's.
    pub fn owns(&self, hwnd: u32) -> bool {
        crate::loader::pm_types::handle_in_slot(hwnd, self.slot)
    }
}

impl Drop for DesktopLink {
    fn drop(&mut self) {
        self.conn.shutdown();
    }
}

/// Start `warpine --desktop-server <dir>`: the same program, in the same
/// environment (display, scripts, desktop size), without this process's
/// standard handles; its log goes to `WARPINE_DESKTOP_LOG` if set.
fn start_server(dir: &Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe()?;
    let log = match std::env::var_os(DESKTOP_LOG_ENV) {
        Some(p) => Stdio::from(File::options().create(true).append(true).open(PathBuf::from(p))?),
        None => Stdio::null(),
    };
    let mut child = Command::new(exe)
        .arg(SERVER_SWITCH).arg(dir)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(log)
        .spawn()?;
    // Reap it whenever it ends, so it never lingers as a zombie of ours.
    std::thread::Builder::new().name("desktop-reaper".into()).spawn(move || { let _ = child.wait(); })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handshake_gives_the_slot_and_messages_arrive_in_the_inbox() {
        let (client, server) = Conn::pair().unwrap();
        let t = std::thread::spawn(move || {
            let (p, _) = server.recv().unwrap().unwrap();
            assert_eq!(ClientMsg::decode(&p), Ok(ClientMsg::Hello { version: PROTOCOL_VERSION }));
            server.send(&ServerMsg::Welcome { version: PROTOCOL_VERSION, slot: 4, width: 800, height: 600 }.encode(),
                        None).unwrap();
            server.send(&ServerMsg::Flush { token: 1 }.encode(), None).unwrap();
            let (p, _) = server.recv().unwrap().unwrap();
            assert_eq!(ClientMsg::decode(&p), Ok(ClientMsg::FlushDone { token: 1 }));
            server.send(&[0xFF, 0xFF], None).unwrap(); // garbage: the link is lost
            server
        });
        let link = DesktopLink::handshake(client).unwrap();
        assert_eq!((link.slot, link.size), (4, (800, 600)));
        assert!(link.owns(5 << 20 | 0x1000) && !link.owns(0x1000));
        let deadline = Instant::now() + Duration::from_secs(5);
        while link.inbox.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(link.take(), vec![ServerMsg::Flush { token: 1 }]);
        assert!(link.send(&ClientMsg::FlushDone { token: 1 }, None));
        let _server = t.join().unwrap();
        while !link.lost() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(link.lost());
    }

    struct Echo;

    impl Responder for Echo {
        fn answer(&self, _id: u64, hwnd: u32, call: &WinCall) -> Option<WinReply> {
            match call {
                WinCall::Post { msg, .. } => Some(WinReply::Value(hwnd ^ msg)),
                WinCall::Send { .. } => None, // answered later
                _ => Some(WinReply::Failed),
            }
        }
    }

    #[test]
    fn calls_wait_for_their_answers_and_calls_made_here_are_answered() {
        let (client, server) = Conn::pair().unwrap();
        let t = std::thread::spawn(move || {
            let _ = server.recv().unwrap(); // Hello
            server.send(&ServerMsg::Welcome { version: PROTOCOL_VERSION, slot: 0, width: 1, height: 1 }.encode(),
                        None).unwrap();
            // This one calls; meanwhile another program calls this one.
            let (p, _) = server.recv().unwrap().unwrap();
            let Ok(ClientMsg::Call { id, hwnd: 0x20_1000, call: WinCall::Describe }) = ClientMsg::decode(&p) else {
                panic!("{p:?}")
            };
            server.send(&ServerMsg::Called { id: 77, hwnd: 0x10_1000, call: WinCall::Post { msg: 1, mp1: 0, mp2: 0 } }
                        .encode(), None).unwrap();
            let (p, _) = server.recv().unwrap().unwrap();
            assert_eq!(ClientMsg::decode(&p), Ok(ClientMsg::Answer { id: 77, reply: WinReply::Value(0x10_1001) }));
            // A stray answer first, then the right one.
            server.send(&ServerMsg::Answered { id: id + 100, reply: WinReply::Value(1) }.encode(), None).unwrap();
            server.send(&ServerMsg::Answered { id, reply: WinReply::Value(2) }.encode(), None).unwrap();
            // A second call is not answered: the server goes away.
            let _ = server.recv().unwrap().unwrap();
            drop(server);
        });
        let link = DesktopLink::handshake(client).unwrap();
        link.set_responder(Arc::new(Echo));
        assert_eq!(link.call(0x20_1000, WinCall::Describe), Some(WinReply::Value(2)));
        let started = Instant::now();
        assert_eq!(link.call(0x20_1000, WinCall::Describe), None);
        assert!(started.elapsed() < CALL_TIMEOUT, "a lost link ends the wait at once");
        t.join().unwrap();
        assert!(link.calls.0.lock().unwrap().waiting.is_empty());
    }

    #[test]
    fn a_sent_message_is_answered_later_and_its_sender_woken() {
        let (client, server) = Conn::pair().unwrap();
        let t = std::thread::spawn(move || {
            let _ = server.recv().unwrap(); // Hello
            server.send(&ServerMsg::Welcome { version: PROTOCOL_VERSION, slot: 0, width: 1, height: 1 }.encode(),
                        None).unwrap();
            let (p, _) = server.recv().unwrap().unwrap();
            let Ok(ClientMsg::Call { id, call: WinCall::Send { msg: 5, .. }, .. }) = ClientMsg::decode(&p) else {
                panic!("{p:?}")
            };
            // Another program sends to this one: no answer from the reader.
            server.send(&ServerMsg::Called { id: 9, hwnd: 0x10_1000, call: WinCall::Send { msg: 1, mp1: 0, mp2: 0 } }
                        .encode(), None).unwrap();
            // The answer the program gives later.
            let (p, _) = server.recv().unwrap().unwrap();
            assert_eq!(ClientMsg::decode(&p), Ok(ClientMsg::Answer { id: 9, reply: WinReply::Value(42) }));
            server.send(&ServerMsg::Answered { id, reply: WinReply::Value(7) }.encode(), None).unwrap();
            server
        });
        let link = DesktopLink::handshake(client).unwrap();
        link.set_responder(Arc::new(Echo));
        let woken = Arc::new(AtomicBool::new(false));
        let w = woken.clone();
        let id = link.begin_call(0x20_1000, WinCall::Send { msg: 5, mp1: 0, mp2: 0 },
                                 Some(Arc::new(move || w.store(true, Ordering::SeqCst)))).unwrap();
        assert_eq!(link.poll_call(id), CallState::Waiting);
        assert!(link.answer(9, WinReply::Value(42)));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = CallState::Waiting;
        while state == CallState::Waiting && Instant::now() < deadline {
            state = link.wait_call(id, Duration::from_millis(100));
        }
        assert_eq!(state, CallState::Answered(WinReply::Value(7)));
        assert!(woken.load(Ordering::SeqCst), "the waker ran");
        assert_eq!(link.poll_call(id), CallState::Gone, "answered once");
        drop(t.join().unwrap());
    }

    #[test]
    fn a_wrong_version_is_refused() {
        let (client, server) = Conn::pair().unwrap();
        let t = std::thread::spawn(move || {
            let _ = server.recv();
            server.send(&ServerMsg::Welcome { version: 99, slot: 0, width: 1, height: 1 }.encode(), None).unwrap();
            server
        });
        assert!(DesktopLink::handshake(client).is_err());
        drop(t.join());
    }

    #[test]
    fn the_desktop_lock_is_exclusive_and_survives_removal_of_its_file() {
        let dir = std::env::temp_dir().join(format!("warpine-desk-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let held = DesktopLock::take(&dir).unwrap();
        let d2 = dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            let l = DesktopLock::take(&d2).unwrap();
            tx.send(()).unwrap();
            l
        });
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "waits while held");
        // The holder removes the file (as a stopping server does) and lets go.
        std::fs::remove_file(dir.join(LOCK_NAME)).unwrap();
        drop(held);
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        drop(t.join());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
