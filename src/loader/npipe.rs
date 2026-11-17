// SPDX-License-Identifier: GPL-3.0-only
//
// Named pipes (`DosCreateNPipe`, `DosOpen("\PIPE\…")` …): the model and its
// host transport. The OS/2 calls on top of it are in `npipe_api.rs`.
//
// Every named-pipe connection is a host `AF_UNIX` stream socket pair
// carrying frames (`[kind u8][len u32 LE][payload]`): one frame per message
// of a message pipe, chunks of a byte pipe, or the server's "disconnect".
// Each end drains its socket into a local queue of messages (`Rx`), which
// gives message reads, `ERROR_MORE_DATA` remainders and `DosPeekNPipe`.
//
// A pipe name lives in a session directory (`PipeDir`, mode 0700, shared
// with the child processes through `WARPINE_PIPE_DIR`): the server process
// listens on a socket file named after a hash of the pipe name, and an
// accept thread hands each connecting client to a listening instance. So
// clients in the same Warpine process and in its child processes (separate
// Warpine processes) reach the pipe the same way; other host processes and
// other Warpine sessions do not see it.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use log::{debug, warn};

use super::constants::*;
use super::mutex_ext::MutexExt;

/// The environment variable that names the session's pipe directory.
pub const PIPE_DIR_ENV: &str = "WARPINE_PIPE_DIR";
/// Frame kinds.
const FRAME_DATA: u8 = 0;
const FRAME_DISCONNECT: u8 = 1;
const FRAME_HEADER: usize = 5;
/// The largest message (frame payload) Warpine sends or accepts.
pub const MAX_MESSAGE: usize = 1 << 20;
/// Byte pipes are sent in chunks of this size.
const BYTE_CHUNK: usize = 64 * 1024;
/// An end stops draining its socket once this much is queued.
const RX_CAP: usize = 4 << 20;
/// How long a blocked call sleeps before it looks at the process end.
const POLL_MS: i32 = 100;
/// Default `DosWaitNPipe` timeout of a pipe created with 0 (CP Reference).
pub const DEFAULT_WAIT_MS: u32 = 50;
/// Longest pipe name accepted (`\PIPE\` included).
pub const MAX_PIPE_NAME: usize = 260;
/// The handshake's request kinds.
const REQ_OPEN: u8 = b'O';
const REQ_WAIT: u8 = b'W';
/// Handshake statuses.
const REPLY_OK: u8 = 0;
const REPLY_BUSY: u8 = 1;
const REPLY_NOT_FOUND: u8 = 2;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// `poll(2)` one descriptor for `events` up to `ms`; true when ready.
fn poll_fd(fd: RawFd, events: i16, ms: i32) -> bool {
    let mut p = libc::pollfd { fd, events, revents: 0 };
    // SAFETY: `p` is one valid pollfd for the duration of the call.
    let n = unsafe { libc::poll(&mut p, 1, ms) };
    n > 0
}

/// Whether `name` is a local pipe name (`\PIPE\` followed by something).
pub fn is_pipe_name(name: &str) -> bool {
    let n = name.replace('/', "\\");
    n.len() > 6 && n[..6].eq_ignore_ascii_case("\\PIPE\\") && n.len() <= MAX_PIPE_NAME
}

/// The registry key of a pipe name: upper case, backslashes.
pub fn pipe_key(name: &str) -> String {
    name.replace('/', "\\").to_ascii_uppercase()
}

/// FNV-1a 64 of the key: the socket file's name.
fn name_hash(key: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

// ── Session directory ────────────────────────────────────────────────────────

/// The directory that holds the session's pipe sockets. Its path is chosen
/// by the first Warpine process of a session and passed to child processes;
/// it is created (mode 0700) when first needed. Sockets are reached through
/// `/proc/self/fd/<dir>/…`, so a long directory path never exceeds the
/// socket path limit.
#[derive(Default)]
pub struct PipeDir {
    path: Option<PathBuf>,
    dir: Option<File>,
    /// This process created the directory (and removes it when empty).
    created: bool,
}

impl PipeDir {
    /// A directory at `path` (created when needed).
    pub fn at(path: PathBuf) -> Self {
        PipeDir { path: Some(path), dir: None, created: false }
    }

    /// The session's directory: `WARPINE_PIPE_DIR`, else a new one for this
    /// process under `$XDG_RUNTIME_DIR` (or the temporary directory).
    pub fn from_env() -> Self {
        if let Some(p) = std::env::var_os(PIPE_DIR_ENV).filter(|p| !p.is_empty()) {
            return Self::at(PathBuf::from(p));
        }
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        Self::at(base.join(format!("warpine-pipes-{uid}-{}", std::process::id())))
    }

    /// The directory's path (what child processes get).
    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    /// Open (and create) the directory: it must be a real directory owned by
    /// this user and closed to everyone else.
    fn ensure(&mut self) -> std::io::Result<RawFd> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
        if let Some(d) = &self.dir {
            // Still there (another process of the session may have removed
            // it once empty; then it is made again).
            if d.metadata().is_ok_and(|m| m.nlink() > 0) {
                return Ok(d.as_raw_fd());
            }
            self.dir = None;
        }
        let path = self.path.clone().ok_or_else(|| std::io::Error::other("no pipe directory"))?;
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => self.created = true,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = std::fs::symlink_metadata(&path)?;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err(std::io::Error::other("pipe directory is not private"));
        }
        let dir = std::fs::OpenOptions::new().read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(&path)?;
        let fd = dir.as_raw_fd();
        self.dir = Some(dir);
        Ok(fd)
    }

    /// Remove the directory if this process made it and it is empty.
    fn remove_if_empty(&mut self) {
        if self.created && let Some(p) = &self.path {
            // Fails, harmlessly, while sockets of the session are in it.
            if std::fs::remove_dir(p).is_ok() {
                self.created = false;
                self.dir = None;
            }
        }
    }

    /// The socket path of pipe `key`.
    fn socket_path(&mut self, key: &str) -> std::io::Result<PathBuf> {
        let fd = self.ensure()?;
        Ok(PathBuf::from(format!("/proc/self/fd/{fd}/np-{:016x}", name_hash(key))))
    }
}

// ── A connection end ─────────────────────────────────────────────────────────

/// Data received on an end and not yet read.
#[derive(Default)]
struct Rx {
    /// Bytes of a frame not yet complete.
    raw: Vec<u8>,
    /// Complete messages (byte pipes: chunks).
    msgs: VecDeque<Vec<u8>>,
    /// Bytes of the front message already read.
    front_off: usize,
    /// Unread bytes in `msgs`.
    bytes: usize,
    /// The peer closed its end.
    eof: bool,
    /// The server disconnected the pipe (`DosDisConnectNPipe`).
    disconnected: bool,
}

impl Rx {
    /// Take complete frames out of `raw`.
    fn parse(&mut self) {
        let mut at = 0;
        while self.raw.len() - at >= FRAME_HEADER {
            let kind = self.raw[at];
            let len = u32::from_le_bytes(self.raw[at + 1..at + 5].try_into().unwrap()) as usize;
            if len > MAX_MESSAGE || kind > FRAME_DISCONNECT {
                // Not a Warpine peer: treat as the end of the connection.
                self.eof = true;
                self.raw.clear();
                return;
            }
            if self.raw.len() - at < FRAME_HEADER + len {
                break;
            }
            let body = self.raw[at + FRAME_HEADER..at + FRAME_HEADER + len].to_vec();
            at += FRAME_HEADER + len;
            if kind == FRAME_DATA {
                self.bytes += body.len();
                self.msgs.push_back(body);
            } else {
                // Unread data is lost (DosDisConnectNPipe).
                self.msgs.clear();
                self.front_off = 0;
                self.bytes = 0;
                self.disconnected = true;
            }
        }
        self.raw.drain(..at);
    }

    /// Unread bytes of the front message.
    fn front_left(&self) -> usize {
        self.msgs.front().map_or(0, |m| m.len() - self.front_off)
    }

    /// Read up to `max` bytes: one message (`message`), else across messages.
    /// Returns the bytes and whether the message has more.
    fn take(&mut self, max: usize, message: bool, consume: bool) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        if message {
            let Some(front) = self.msgs.front() else { return (out, false) };
            let left = &front[self.front_off..];
            let n = left.len().min(max);
            out.extend_from_slice(&left[..n]);
            let more = n < left.len();
            if consume {
                self.bytes -= n;
                if more { self.front_off += n } else { self.msgs.pop_front(); self.front_off = 0; }
            }
            return (out, more);
        }
        let mut off = self.front_off;
        let mut idx = 0;
        while out.len() < max && idx < self.msgs.len() {
            let m = &self.msgs[idx];
            let n = (m.len() - off).min(max - out.len());
            out.extend_from_slice(&m[off..off + n]);
            off += n;
            if off == m.len() { idx += 1; off = 0; }
        }
        if consume {
            self.bytes -= out.len();
            self.msgs.drain(..idx);
            self.front_off = off;
        }
        (out, false)
    }
}

/// What a read returned: the data and whether the message has more
/// (`ERROR_MORE_DATA`).
#[derive(Debug, PartialEq, Eq)]
pub struct ReadOut {
    pub data: Vec<u8>,
    pub more: bool,
}

/// What `DosPeekNPipe` sees on an end.
#[derive(Debug, PartialEq, Eq)]
pub struct PeekOut {
    pub data: Vec<u8>,
    /// Unread bytes in the pipe.
    pub bytes: usize,
    /// Messages in the pipe.
    pub messages: usize,
    /// Unread bytes of the current message.
    pub current: usize,
}

/// One end of a named-pipe connection.
pub struct Conn {
    sock: UnixStream,
    rx: Mutex<Rx>,
    tx: Mutex<()>,
}

impl Conn {
    pub fn new(sock: UnixStream) -> std::io::Result<Self> {
        sock.set_nonblocking(true)?;
        Ok(Conn { sock, rx: Mutex::new(Rx::default()), tx: Mutex::new(()) })
    }

    /// A connected pair (tests).
    #[cfg(test)]
    pub fn pair() -> (Conn, Conn) {
        let (a, b) = UnixStream::pair().unwrap();
        (Conn::new(a).unwrap(), Conn::new(b).unwrap())
    }

    pub fn raw_fd(&self) -> RawFd {
        self.sock.as_raw_fd()
    }

    /// A duplicate of the socket (for a child process; close-on-exec).
    pub fn try_clone_socket(&self) -> std::io::Result<UnixStream> {
        self.sock.try_clone()
    }

    /// Move what the socket holds into the queue (never blocks).
    fn drain_locked(&self, rx: &mut Rx) {
        let mut buf = [0u8; 16 * 1024];
        while !rx.eof && !(rx.bytes >= RX_CAP && !rx.msgs.is_empty()) {
            match (&self.sock).read(&mut buf) {
                Ok(0) => rx.eof = true,
                Ok(n) => { rx.raw.extend_from_slice(&buf[..n]); rx.parse(); }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => rx.eof = true,
            }
        }
    }

    pub fn drain(&self) {
        let mut rx = self.rx.lock_or_recover();
        self.drain_locked(&mut rx);
    }

    /// The peer closed its end.
    pub fn peer_closed(&self) -> bool {
        let mut rx = self.rx.lock_or_recover();
        self.drain_locked(&mut rx);
        rx.eof && !rx.disconnected
    }

    /// The server disconnected the pipe.
    pub fn disconnected(&self) -> bool {
        let mut rx = self.rx.lock_or_recover();
        self.drain_locked(&mut rx);
        rx.disconnected
    }

    /// Unread data waits on this end.
    pub fn has_unread(&self) -> bool {
        let mut rx = self.rx.lock_or_recover();
        self.drain_locked(&mut rx);
        rx.bytes > 0 || !rx.msgs.is_empty()
    }

    /// Read up to `max` bytes. A blocking read waits for a message (message
    /// read mode) or any data; at the peer's end it returns no data.
    /// `ERROR_NO_DATA` for a non-blocking read with nothing there;
    /// `ERROR_INTERRUPT` when `stop` says the process is ending.
    pub fn read(&self, max: usize, message: bool, nowait: bool, stop: &dyn Fn() -> bool) -> Result<ReadOut, u32> {
        loop {
            {
                let mut rx = self.rx.lock_or_recover();
                self.drain_locked(&mut rx);
                if !rx.msgs.is_empty() {
                    let (data, more) = rx.take(max, message, true);
                    return Ok(ReadOut { data, more });
                }
                if rx.eof || rx.disconnected {
                    return Ok(ReadOut { data: Vec::new(), more: false });
                }
            }
            if nowait {
                return Err(ERROR_NO_DATA);
            }
            if stop() {
                return Err(ERROR_INTERRUPT);
            }
            poll_fd(self.raw_fd(), libc::POLLIN, POLL_MS);
        }
    }

    /// Look at the data without taking it.
    pub fn peek(&self, max: usize, message: bool) -> PeekOut {
        let mut rx = self.rx.lock_or_recover();
        self.drain_locked(&mut rx);
        let (data, _) = rx.take(max, message, false);
        PeekOut { data, bytes: rx.bytes, messages: rx.msgs.len(), current: rx.front_left() }
    }

    /// Send one frame; `Ok(false)` when non-blocking and the socket is
    /// full. A frame once started is always finished.
    fn send_frame(&self, kind: u8, body: &[u8], nowait: bool, stop: &dyn Fn() -> bool) -> Result<bool, u32> {
        let _tx = self.tx.lock_or_recover();
        let mut frame = Vec::with_capacity(FRAME_HEADER + body.len());
        frame.push(kind);
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(body);
        let mut off = 0;
        while off < frame.len() {
            match (&self.sock).write(&frame[off..]) {
                Ok(0) => return Err(ERROR_BROKEN_PIPE),
                Ok(n) => off += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if off == 0 && nowait {
                        return Ok(false);
                    }
                    if stop() {
                        return Err(ERROR_INTERRUPT);
                    }
                    // Take in what the peer sends meanwhile, so two ends
                    // writing to each other do not both stay blocked.
                    self.drain();
                    poll_fd(self.raw_fd(), libc::POLLOUT, POLL_MS);
                }
                Err(_) => return Err(ERROR_BROKEN_PIPE),
            }
        }
        Ok(true)
    }

    /// Write `data` as one message (`message`) or as a byte stream; returns
    /// the bytes written (0 when non-blocking and full).
    pub fn write(&self, data: &[u8], message: bool, nowait: bool, stop: &dyn Fn() -> bool) -> Result<usize, u32> {
        if message && data.len() > MAX_MESSAGE {
            return Err(ERROR_INVALID_PARAMETER);
        }
        if self.peer_closed() || self.disconnected() {
            return Err(ERROR_BROKEN_PIPE);
        }
        if message {
            return Ok(if self.send_frame(FRAME_DATA, data, nowait, stop)? { data.len() } else { 0 });
        }
        let mut done = 0;
        for chunk in data.chunks(BYTE_CHUNK) {
            if !self.send_frame(FRAME_DATA, chunk, nowait, stop)? {
                break;
            }
            done += chunk.len();
        }
        Ok(done)
    }

    /// End the connection from the server's side: the client loses what it
    /// has not read.
    pub fn disconnect(&self) {
        let _ = self.send_frame(FRAME_DISCONNECT, &[], true, &|| true);
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
    }
}

// ── Pipes, instances and clients ─────────────────────────────────────────────

/// What `DosCreateNPipe` fixed for a pipe name (its first instance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeSpec {
    /// The name as created (`\PIPE\…`).
    pub name: String,
    /// `NP_ACCESS_*` (bits 0–2) and the other open-mode bits.
    pub open_mode: u32,
    /// Pipe type and instance count (`NP_TYPE_MESSAGE`, `NP_ICOUNT`).
    pub pipe_mode: u32,
    pub cb_out: u32,
    pub cb_in: u32,
    /// Default `DosWaitNPipe` timeout in ms.
    pub timeout: u32,
}

impl PipeSpec {
    pub fn access(&self) -> u32 { self.open_mode & 0x7 }
    pub fn message_type(&self) -> bool { self.pipe_mode & NP_TYPE_MESSAGE != 0 }
    pub fn max_instances(&self) -> u32 { self.pipe_mode & NP_ICOUNT }
}

/// A pipe name served by this process.
pub struct ServerPipe {
    pub spec: PipeSpec,
    path: PathBuf,
    insts: Mutex<PipeInsts>,
}

#[derive(Default)]
struct PipeInsts {
    list: Vec<Weak<ServerInstance>>,
    /// The accept thread ended: a new instance needs a new `ServerPipe`.
    closed: bool,
}

impl PipeInsts {
    fn live(&mut self) -> Vec<Arc<ServerInstance>> {
        self.list.retain(|w| w.strong_count() > 0);
        self.list.iter().filter_map(Weak::upgrade).collect()
    }
}

impl ServerPipe {
    /// Live instances of the pipe.
    pub fn instance_count(&self) -> usize {
        self.insts.lock_or_recover().live().len()
    }
}

/// The handle-specific mode of an end (`DosSetNPHState`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EndMode {
    pub nowait: bool,
    pub read_message: bool,
}

/// Server-side state of an instance.
pub struct InstState {
    /// `NP_STATE_DISCONNECTED`, `_LISTENING` or `_CONNECTED` (closing is
    /// a connected instance whose client has gone).
    pub state: u32,
    pub conn: Option<Arc<Conn>>,
    pub mode: EndMode,
}

/// One instance of a pipe: the server end (`DosCreateNPipe`).
pub struct ServerInstance {
    pub pipe: Arc<ServerPipe>,
    pub st: Mutex<InstState>,
    pub cond: Condvar,
    /// Threads blocked in a call on this end.
    pub blocked: AtomicU32,
}

impl ServerInstance {
    /// The state `DosPeekNPipe` reports.
    pub fn state(&self) -> u32 {
        let st = self.st.lock_or_recover();
        match (&st.conn, st.state) {
            (Some(c), NP_STATE_CONNECTED) if c.peer_closed() => NP_STATE_CLOSING,
            (_, s) => s,
        }
    }

    pub fn conn(&self) -> Option<Arc<Conn>> {
        self.st.lock_or_recover().conn.clone()
    }

    pub fn mode(&self) -> EndMode {
        self.st.lock_or_recover().mode
    }
}

/// The client end of a pipe (`DosOpen`).
pub struct ClientEnd {
    pub spec: PipeSpec,
    /// Instances when the client connected.
    pub cur_inst: u32,
    pub conn: Arc<Conn>,
    pub mode: Mutex<EndMode>,
    pub blocked: AtomicU32,
}

/// The named pipes this process serves, and its session directory.
#[derive(Default)]
pub struct NamedPipes {
    pub dir: PipeDir,
    pipes: HashMap<String, Weak<ServerPipe>>,
}

impl NamedPipes {
    pub fn new(dir: PipeDir) -> Self {
        NamedPipes { dir, pipes: HashMap::new() }
    }

    /// Create an instance of pipe `spec.name` (`DosCreateNPipe`): the first
    /// one fixes the pipe's type, instance count and default timeout.
    pub fn create(&mut self, spec: PipeSpec, mode: EndMode) -> Result<Arc<ServerInstance>, u32> {
        let key = pipe_key(&spec.name);
        if let Some(pipe) = self.pipes.get(&key).and_then(Weak::upgrade) {
            let mut insts = pipe.insts.lock_or_recover();
            if !insts.closed {
                let max = pipe.spec.max_instances();
                if max != NP_UNLIMITED_INSTANCES && insts.live().len() as u32 >= max {
                    return Err(ERROR_PIPE_BUSY);
                }
                let inst = new_instance(&pipe, mode);
                insts.list.push(Arc::downgrade(&inst));
                return Ok(inst);
            }
        }
        let path = self.dir.socket_path(&key).map_err(|e| {
            warn!("named pipe directory: {e}");
            ERROR_PATH_NOT_FOUND
        })?;
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) if e.kind() == ErrorKind::AddrInUse => {
                if UnixStream::connect(&path).is_ok() {
                    // Another process of the session serves this name.
                    return Err(ERROR_PIPE_BUSY);
                }
                // A socket left behind by a process that ended.
                let _ = std::fs::remove_file(&path);
                UnixListener::bind(&path).map_err(|_| ERROR_PATH_NOT_FOUND)?
            }
            Err(e) => {
                warn!("named pipe socket {}: {e}", path.display());
                return Err(ERROR_PATH_NOT_FOUND);
            }
        };
        listener.set_nonblocking(true).map_err(|_| ERROR_PATH_NOT_FOUND)?;
        let pipe = Arc::new(ServerPipe { spec, path, insts: Mutex::new(PipeInsts::default()) });
        let inst = new_instance(&pipe, mode);
        pipe.insts.lock_or_recover().list.push(Arc::downgrade(&inst));
        self.pipes.insert(key, Arc::downgrade(&pipe));
        let p = pipe.clone();
        std::thread::Builder::new().name("npipe-accept".into())
            .spawn(move || accept_loop(p, listener))
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        Ok(inst)
    }

    /// At the end of the process: remove the sockets of the pipes still
    /// served and the session directory if this process made it and
    /// nothing else is in it.
    pub fn close_all(&mut self) {
        for pipe in self.pipes.values().filter_map(Weak::upgrade) {
            let mut insts = pipe.insts.lock_or_recover();
            if !insts.closed {
                insts.closed = true;
                let _ = std::fs::remove_file(&pipe.path);
            }
        }
        self.pipes.clear();
        self.dir.remove_if_empty();
    }

    /// The socket path of pipe `name` (for a client).
    pub fn socket_path(&mut self, name: &str) -> Result<PathBuf, u32> {
        self.dir.socket_path(&pipe_key(name)).map_err(|_| ERROR_FILE_NOT_FOUND)
    }
}

fn new_instance(pipe: &Arc<ServerPipe>, mode: EndMode) -> Arc<ServerInstance> {
    Arc::new(ServerInstance {
        pipe: pipe.clone(),
        st: Mutex::new(InstState { state: NP_STATE_DISCONNECTED, conn: None, mode }),
        cond: Condvar::new(),
        blocked: AtomicU32::new(0),
    })
}

/// Accept clients for `pipe` until its last instance is gone.
fn accept_loop(pipe: Arc<ServerPipe>, listener: UnixListener) {
    loop {
        if poll_fd(listener.as_raw_fd(), libc::POLLIN, POLL_MS) {
            match listener.accept() {
                Ok((stream, _)) => serve_request(&pipe, stream),
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {}
                Err(e) => debug!("named pipe accept: {e}"),
            }
        }
        let mut insts = pipe.insts.lock_or_recover();
        if insts.live().is_empty() {
            insts.closed = true;
            let _ = std::fs::remove_file(&pipe.path);
            debug!("named pipe {} closed", pipe.spec.name);
            return;
        }
    }
}

/// Answer one connecting client: open (hand it to a listening instance) or
/// a `DosWaitNPipe` probe.
fn serve_request(pipe: &Arc<ServerPipe>, mut stream: UnixStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let mut head = [0u8; 3];
    if stream.read_exact(&mut head).is_err() { return; }
    let len = u16::from_le_bytes([head[1], head[2]]) as usize;
    if len > MAX_PIPE_NAME { return; }
    let mut name = vec![0u8; len];
    if stream.read_exact(&mut name).is_err() { return; }
    if pipe_key(&String::from_utf8_lossy(&name)) != pipe_key(&pipe.spec.name) {
        let _ = stream.write_all(&[REPLY_NOT_FOUND]);
        return;
    }
    let live = pipe.insts.lock_or_recover().live();
    let listening = live.iter().find(|i| i.st.lock_or_recover().state == NP_STATE_LISTENING).cloned();
    match head[0] {
        REQ_WAIT => {
            let mut reply = vec![if listening.is_some() { REPLY_OK } else { REPLY_BUSY }];
            reply.extend_from_slice(&pipe.spec.timeout.to_le_bytes());
            let _ = stream.write_all(&reply);
        }
        REQ_OPEN => {
            let Some(inst) = listening else {
                let _ = stream.write_all(&[REPLY_BUSY]);
                return;
            };
            let s = &pipe.spec;
            let mut reply = vec![REPLY_OK];
            for v in [s.open_mode, s.pipe_mode, s.cb_out, s.cb_in, s.timeout, live.len() as u32] {
                reply.extend_from_slice(&v.to_le_bytes());
            }
            if stream.write_all(&reply).is_err() { return; }
            let Ok(conn) = Conn::new(stream) else { return };
            let mut st = inst.st.lock_or_recover();
            if st.state == NP_STATE_LISTENING {
                st.conn = Some(Arc::new(conn));
                st.state = NP_STATE_CONNECTED;
                inst.cond.notify_all();
            }
        }
        _ => {}
    }
}

/// Send a request for `name` to the pipe's socket.
fn request(path: &PathBuf, op: u8, name: &str) -> Result<UnixStream, u32> {
    let mut stream = UnixStream::connect(path).map_err(|_| ERROR_FILE_NOT_FOUND)?;
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let bytes = name.as_bytes();
    let mut req = vec![op];
    req.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    req.extend_from_slice(bytes);
    stream.write_all(&req).map_err(|_| ERROR_FILE_NOT_FOUND)?;
    Ok(stream)
}

/// Connect to pipe `name` as a client (`DosOpen`): `ERROR_FILE_NOT_FOUND`
/// when nobody serves it, `ERROR_PIPE_BUSY` when no instance listens.
pub fn open_client(path: &PathBuf, name: &str) -> Result<ClientEnd, u32> {
    let mut stream = request(path, REQ_OPEN, name)?;
    let mut status = [0u8; 1];
    stream.read_exact(&mut status).map_err(|_| ERROR_PIPE_BUSY)?;
    match status[0] {
        REPLY_OK => {}
        REPLY_BUSY => return Err(ERROR_PIPE_BUSY),
        _ => return Err(ERROR_FILE_NOT_FOUND),
    }
    let mut info = [0u8; 24];
    stream.read_exact(&mut info).map_err(|_| ERROR_PIPE_BUSY)?;
    let v = |i: usize| u32::from_le_bytes(info[i * 4..i * 4 + 4].try_into().unwrap());
    let spec = PipeSpec { name: name.to_string(), open_mode: v(0), pipe_mode: v(1),
                          cb_out: v(2), cb_in: v(3), timeout: v(4) };
    let conn = Conn::new(stream).map_err(|_| ERROR_PIPE_BUSY)?;
    Ok(client_end(spec, v(5), conn))
}

/// A client end over `conn` (also for one inherited from a parent).
pub fn client_end(spec: PipeSpec, cur_inst: u32, conn: Conn) -> ClientEnd {
    ClientEnd { spec, cur_inst, conn: Arc::new(conn), mode: Mutex::new(EndMode::default()),
                blocked: AtomicU32::new(0) }
}

/// Ask pipe `name` whether an instance listens (`DosWaitNPipe`); returns
/// that and the pipe's default timeout.
pub fn probe(path: &PathBuf, name: &str) -> Result<(bool, u32), u32> {
    let mut stream = request(path, REQ_WAIT, name)?;
    let mut reply = [0u8; 5];
    match stream.read_exact(&mut reply[..1]) {
        Ok(()) if reply[0] == REPLY_NOT_FOUND => return Err(ERROR_FILE_NOT_FOUND),
        Ok(()) => {}
        Err(_) => return Err(ERROR_FILE_NOT_FOUND),
    }
    stream.read_exact(&mut reply[1..]).map_err(|_| ERROR_FILE_NOT_FOUND)?;
    Ok((reply[0] == REPLY_OK, u32::from_le_bytes(reply[1..5].try_into().unwrap())))
}

/// Wait until an instance of `name` listens, up to `msec` (0: the pipe's
/// default timeout, `NP_INDEFINITE_WAIT`: forever).
pub fn wait_for_instance(path: &PathBuf, name: &str, msec: u32, stop: &dyn Fn() -> bool) -> Result<(), u32> {
    let (ready, default) = probe(path, name)?;
    if ready {
        return Ok(());
    }
    let wait = match msec {
        NP_DEFAULT_WAIT => Some(Duration::from_millis(default as u64)),
        NP_INDEFINITE_WAIT => None,
        ms => Some(Duration::from_millis(ms as u64)),
    };
    let deadline = wait.map(|w| Instant::now() + w);
    loop {
        if stop() {
            return Err(ERROR_INTERRUPT);
        }
        let step = Duration::from_millis(10);
        if let Some(d) = deadline {
            let now = Instant::now();
            if now >= d {
                return Err(ERROR_SEM_TIMEOUT);
            }
            std::thread::sleep(step.min(d - now));
        } else {
            std::thread::sleep(step);
        }
        if probe(path, name)?.0 {
            return Ok(());
        }
    }
}

/// Counts a thread blocked on an end while it lives.
pub struct BlockedGuard<'a>(&'a AtomicU32);

impl<'a> BlockedGuard<'a> {
    pub fn new(counter: &'a AtomicU32) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        BlockedGuard(counter)
    }
}

impl Drop for BlockedGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_stop() -> bool { false }

    fn spec(name: &str, open_mode: u32, pipe_mode: u32) -> PipeSpec {
        PipeSpec { name: name.into(), open_mode, pipe_mode, cb_out: 4096, cb_in: 4096, timeout: 50 }
    }

    #[test]
    fn pipe_names() {
        assert!(is_pipe_name("\\PIPE\\X"));
        assert!(is_pipe_name("\\pipe\\a\\b"));
        assert!(is_pipe_name("/PIPE/X"));
        assert!(!is_pipe_name("\\PIPE\\"));
        assert!(!is_pipe_name("\\SEM32\\X"));
        assert!(!is_pipe_name(&format!("\\PIPE\\{}", "x".repeat(300))));
        assert_eq!(pipe_key("\\pipe\\Ab"), "\\PIPE\\AB");
        assert_ne!(name_hash("\\PIPE\\A"), name_hash("\\PIPE\\B"));
    }

    /// Message pipes keep message boundaries; a short buffer leaves the
    /// rest of the message for the next read.
    #[test]
    fn messages_and_remainders() {
        let (a, b) = Conn::pair();
        a.write(b"hello", true, false, &no_stop).unwrap();
        a.write(b"", true, false, &no_stop).unwrap();
        a.write(b"world!", true, false, &no_stop).unwrap();
        let p = b.peek(64, true);
        assert_eq!((p.data.as_slice(), p.bytes, p.messages, p.current), (&b"hello"[..], 11, 3, 5));
        assert_eq!(b.read(3, true, false, &no_stop).unwrap(), ReadOut { data: b"hel".to_vec(), more: true });
        assert_eq!(b.peek(64, true).current, 2);
        assert_eq!(b.read(64, true, false, &no_stop).unwrap(), ReadOut { data: b"lo".to_vec(), more: false });
        assert_eq!(b.read(64, true, false, &no_stop).unwrap(), ReadOut { data: vec![], more: false }, "empty message");
        // Byte read mode reads across what is left.
        a.write(b"tail", true, false, &no_stop).unwrap();
        assert_eq!(b.read(64, false, false, &no_stop).unwrap().data, b"world!tail");
        assert_eq!(b.read(64, false, true, &no_stop), Err(ERROR_NO_DATA), "non-blocking, empty");
        assert!(!b.has_unread());
    }

    /// The peer's close ends reads with no data and writes with
    /// ERROR_BROKEN_PIPE; a disconnect drops what was not read.
    #[test]
    fn close_and_disconnect() {
        let (a, b) = Conn::pair();
        a.write(b"left", false, false, &no_stop).unwrap();
        drop(a);
        assert!(b.peer_closed());
        assert_eq!(b.read(64, false, false, &no_stop).unwrap().data, b"left", "data before the close");
        assert_eq!(b.read(64, false, false, &no_stop).unwrap().data, b"");
        assert_eq!(b.write(b"x", false, false, &no_stop), Err(ERROR_BROKEN_PIPE));

        let (server, client) = Conn::pair();
        server.write(b"lost", false, false, &no_stop).unwrap();
        server.disconnect();
        assert!(client.disconnected());
        assert!(!client.has_unread(), "unread data is lost");
        assert_eq!(client.write(b"x", false, false, &no_stop), Err(ERROR_BROKEN_PIPE));
    }

    /// A blocked read wakes when data comes from another thread.
    #[test]
    fn blocking_read_waits() {
        let (a, b) = Conn::pair();
        let t = std::thread::spawn(move || b.read(16, false, false, &no_stop));
        std::thread::sleep(Duration::from_millis(50));
        a.write(b"late", false, false, &no_stop).unwrap();
        assert_eq!(t.join().unwrap().unwrap().data, b"late");
    }

    /// A blocked read gives up when the process ends.
    #[test]
    fn blocking_read_stops() {
        let (_a, b) = Conn::pair();
        assert_eq!(b.read(16, false, false, &|| true), Err(ERROR_INTERRUPT));
    }

    /// Big byte writes go in chunks and arrive whole, while the reader
    /// takes them on another thread.
    #[test]
    fn large_byte_stream() {
        let (a, b) = Conn::pair();
        let data: Vec<u8> = (0..300_000u32).map(|i| i as u8).collect();
        let expect = data.clone();
        let t = std::thread::spawn(move || {
            let mut got = Vec::new();
            while got.len() < expect.len() {
                got.extend(b.read(70_000, false, false, &no_stop).unwrap().data);
            }
            assert_eq!(got, expect);
        });
        assert_eq!(a.write(&data, false, false, &no_stop), Ok(300_000));
        t.join().unwrap();
        assert_eq!(a.write(&vec![0; MAX_MESSAGE + 1], true, false, &no_stop), Err(ERROR_INVALID_PARAMETER));
    }

    /// A foreign byte stream (no valid frames) ends the connection instead
    /// of allocating what its header claims.
    #[test]
    fn bad_frame_ends_the_connection() {
        let (mut raw, other) = UnixStream::pair().unwrap();
        let end = Conn::new(other).unwrap();
        raw.write_all(&[FRAME_DATA, 0xFF, 0xFF, 0xFF, 0x7F]).unwrap();
        assert!(end.peer_closed());
        assert_eq!(end.read(8, false, false, &no_stop).unwrap().data, b"");
    }

    /// Create, connect, open and the instance limit through the session
    /// directory.
    #[test]
    fn server_and_client() {
        let dir = tempfile::tempdir().unwrap();
        let mut np = NamedPipes::new(PipeDir::at(dir.path().join("pipes")));
        let s = spec("\\PIPE\\Unit\\One", NP_ACCESS_DUPLEX, NP_TYPE_MESSAGE | 1);
        let inst = np.create(s.clone(), EndMode::default()).unwrap();
        assert_eq!(np.create(s.clone(), EndMode::default()).err(), Some(ERROR_PIPE_BUSY), "one instance");
        let path = np.socket_path("\\pipe\\unit\\one").unwrap();
        assert_eq!(inst.state(), NP_STATE_DISCONNECTED);
        // Not listening: busy.
        assert_eq!(open_client(&path, "\\PIPE\\UNIT\\ONE").err(), Some(ERROR_PIPE_BUSY));
        assert_eq!(probe(&path, "\\PIPE\\UNIT\\ONE"), Ok((false, 50)));
        assert_eq!(wait_for_instance(&path, "\\PIPE\\UNIT\\ONE", 30, &no_stop), Err(ERROR_SEM_TIMEOUT));
        inst.st.lock().unwrap().state = NP_STATE_LISTENING;
        assert_eq!(wait_for_instance(&path, "\\PIPE\\UNIT\\ONE", 30, &no_stop), Ok(()));
        let client = open_client(&path, "\\pipe\\unit\\one").unwrap();
        assert_eq!(client.spec.pipe_mode, NP_TYPE_MESSAGE | 1);
        assert_eq!(client.cur_inst, 1);
        // The accept thread connects the instance.
        let deadline = Instant::now() + Duration::from_secs(5);
        while inst.state() != NP_STATE_CONNECTED {
            assert!(Instant::now() < deadline, "not connected");
            std::thread::sleep(Duration::from_millis(5));
        }
        client.conn.write(b"ping", true, false, &no_stop).unwrap();
        let sconn = inst.conn().unwrap();
        assert_eq!(sconn.read(16, true, false, &no_stop).unwrap().data, b"ping");
        drop(client);
        assert_eq!(inst.state(), NP_STATE_CLOSING);
        // Unknown names are not found.
        let other = np.socket_path("\\PIPE\\NONE").unwrap();
        assert_eq!(open_client(&other, "\\PIPE\\NONE").err(), Some(ERROR_FILE_NOT_FOUND));
        // The last instance gone, the name goes away.
        drop(sconn);
        drop(inst);
        let deadline = Instant::now() + Duration::from_secs(5);
        while open_client(&path, "\\PIPE\\UNIT\\ONE").err() != Some(ERROR_FILE_NOT_FOUND) {
            assert!(Instant::now() < deadline, "the pipe stayed");
            std::thread::sleep(Duration::from_millis(20));
        }
        // And can be created again.
        assert!(np.create(s, EndMode::default()).is_ok());
    }

    /// A directory that others can read is refused.
    #[test]
    fn pipe_dir_must_be_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let open = dir.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut d = PipeDir::at(open);
        assert!(d.socket_path("\\PIPE\\X").is_err());
        let mut none = PipeDir::default();
        assert!(none.socket_path("\\PIPE\\X").is_err());
    }
}
