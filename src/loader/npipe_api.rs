// SPDX-License-Identifier: GPL-3.0-or-later
//
// The named-pipe calls of DOSCALLS on top of `npipe.rs`: DosCreateNPipe,
// DosConnectNPipe, DosDisConnectNPipe, DosPeekNPipe, DosTransactNPipe,
// DosCallNPipe, DosWaitNPipe, DosQueryNPHState, DosSetNPHState,
// DosQueryNPipeInfo, and the client end opened by `DosOpen("\PIPE\…")`.
// `DosRead` / `DosWrite` / `DosClose` / `DosDupHandle` reach the ends through
// the pipe table (`HandleManager`, `PipeObject::Server` / `Client`).
//
// Behaviour from the IBM Control Program Reference (EDM2 pages of each
// call); where it is silent the choice is marked "unverified".

use std::sync::Arc;
use std::sync::atomic::Ordering;

use log::debug;

use super::constants::*;
use super::managers::{PipeHandle, PipeObject};
use super::mutex_ext::MutexExt;
use super::npipe::{self, BlockedGuard, ClientEnd, EndMode, PipeSpec, ReadOut, ServerInstance};

/// A named-pipe end behind a handle.
#[derive(Clone)]
pub(crate) enum NpEnd {
    Server(Arc<ServerInstance>),
    Client(Arc<ClientEnd>),
}

/// Bits of `DosCreateNPipe`'s open mode that may be set.
const NP_OPEN_MODE_BITS: u32 = NP_NOWRITEBEHIND | NP_NOINHERIT | 0x7;
/// Bits of `DosCreateNPipe`'s pipe mode that may be set.
const NP_PIPE_MODE_BITS: u32 = NP_NOWAIT | 0x0C00 | 0x0300 | NP_ICOUNT;
/// Bits `DosSetNPHState` may set.
const NP_SET_STATE_BITS: u32 = NP_NOWAIT | 0x0300;
/// Bytes `DosPeekNPipe` counts for each message's header (unverified:
/// "including message headers", the header size is not documented).
const NP_MESSAGE_HEADER: usize = 2;
/// Largest single read or peek from a named pipe.
const NP_MAX_READ: u32 = 64 * 1024 * 1024;

impl NpEnd {
    pub(crate) fn spec(&self) -> &PipeSpec {
        match self {
            NpEnd::Server(i) => &i.pipe.spec,
            NpEnd::Client(c) => &c.spec,
        }
    }

    fn mode(&self) -> EndMode {
        match self {
            NpEnd::Server(i) => i.mode(),
            NpEnd::Client(c) => *c.mode.lock_or_recover(),
        }
    }

    fn set_mode(&self, mode: EndMode) {
        match self {
            NpEnd::Server(i) => i.st.lock_or_recover().mode = mode,
            NpEnd::Client(c) => *c.mode.lock_or_recover() = mode,
        }
    }

    fn blocked(&self) -> &std::sync::atomic::AtomicU32 {
        match self {
            NpEnd::Server(i) => &i.blocked,
            NpEnd::Client(c) => &c.blocked,
        }
    }

    /// The connection to read and write, or the error of an end without
    /// one: a server not connected (`ERROR_PIPE_NOT_CONNECTED`, unverified
    /// for DosRead / DosWrite).
    pub(crate) fn conn(&self) -> Result<Arc<npipe::Conn>, u32> {
        match self {
            NpEnd::Server(i) => i.conn().ok_or(ERROR_PIPE_NOT_CONNECTED),
            NpEnd::Client(c) => Ok(c.conn.clone()),
        }
    }

    /// The state `DosPeekNPipe` reports.
    fn state(&self) -> u32 {
        match self {
            NpEnd::Server(i) => i.state(),
            NpEnd::Client(c) => {
                if c.conn.peer_closed() || c.conn.disconnected() { NP_STATE_CLOSING } else { NP_STATE_CONNECTED }
            }
        }
    }

    /// Whether this end may read (`handle_access`: the client's `DosOpen`
    /// access) — inbound pipes carry data from the client to the server.
    fn may_read(&self, handle_access: u32) -> bool {
        let a = self.spec().access();
        match self {
            NpEnd::Server(_) => a == NP_ACCESS_INBOUND || a == NP_ACCESS_DUPLEX,
            NpEnd::Client(_) => (a == NP_ACCESS_OUTBOUND || a == NP_ACCESS_DUPLEX)
                && handle_access != OPEN_ACCESS_WRITEONLY,
        }
    }

    pub(crate) fn may_write(&self, handle_access: u32) -> bool {
        let a = self.spec().access();
        match self {
            NpEnd::Server(_) => a == NP_ACCESS_OUTBOUND || a == NP_ACCESS_DUPLEX,
            NpEnd::Client(_) => (a == NP_ACCESS_INBOUND || a == NP_ACCESS_DUPLEX)
                && handle_access != OPEN_ACCESS_READONLY,
        }
    }

    /// `DosQueryNPHState`'s value.
    fn nph_state(&self) -> u32 {
        let mode = self.mode();
        let spec = self.spec();
        let mut s = spec.pipe_mode & (NP_TYPE_MESSAGE | NP_ICOUNT);
        if mode.nowait { s |= NP_NOWAIT; }
        if mode.read_message { s |= NP_READMODE_MESSAGE; }
        if matches!(self, NpEnd::Server(_)) { s |= NP_END_SERVER; }
        s
    }
}

/// The `DosQueryFHState` value of a server end created with `open_mode`.
fn server_fh_state(open_mode: u32) -> u32 {
    let access = match open_mode & 0x7 {
        NP_ACCESS_INBOUND => OPEN_ACCESS_READONLY,
        NP_ACCESS_OUTBOUND => OPEN_ACCESS_WRITEONLY,
        _ => OPEN_ACCESS_READWRITE,
    };
    access | (open_mode & (NP_NOINHERIT | NP_NOWRITEBEHIND))
}

/// Split raw message records (`[len u16 LE][bytes]`); None when one runs
/// past the end.
fn raw_records(mut data: &[u8]) -> Option<Vec<&[u8]>> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let header = data.get(..npipe::RAW_HEADER)?;
        let len = u16::from_le_bytes([header[0], header[1]]) as usize;
        out.push(data.get(npipe::RAW_HEADER..npipe::RAW_HEADER + len)?);
        data = &data[npipe::RAW_HEADER + len..];
    }
    Some(out)
}

impl super::Loader {
    /// The named-pipe end behind handle `h`, with its handle state.
    pub(crate) fn np_end(&self, h: u32) -> Option<(NpEnd, u32)> {
        let hm = self.shared.handle_mgr.lock_or_recover();
        let e = hm.entry(h)?;
        match &e.obj {
            PipeObject::Server(i) => Some((NpEnd::Server(i.clone()), e.fh_state)),
            PipeObject::Client(c) => Some((NpEnd::Client(c.clone()), e.fh_state)),
            PipeObject::Host(_) => None,
        }
    }

    /// Whether a blocked named-pipe call should give up.
    fn np_stop(&self) -> bool {
        self.shutting_down()
    }

    /// A blocked pipe write is interrupted (ADR-0035): with nothing written
    /// yet (`fresh`) the call is made again after the signal; later it
    /// stops with what it wrote.
    fn np_interrupt(&self, fresh: bool) -> bool {
        if fresh { self.interrupt_restart() } else { self.interrupted() }
    }

    /// `DosRead` on a named-pipe end: `ERROR_MORE_DATA` with the first part
    /// of a message that did not fit.
    pub(crate) fn np_read(&self, end: &NpEnd, fh_state: u32, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        if !end.may_read(fh_state & 0x7) {
            return ERROR_ACCESS_DENIED;
        }
        let conn = match end.conn() { Ok(c) => c, Err(e) => return e };
        if let NpEnd::Client(_) = end && conn.disconnected() {
            return ERROR_PIPE_NOT_CONNECTED;
        }
        let mode = end.mode();
        let message = mode.read_message && end.spec().message_type();
        let _blocked = BlockedGuard::new(end.blocked());
        let len = len.min(NP_MAX_READ) as usize;
        match conn.read(len, message, mode.nowait, &|| self.stop_or_restart()) {
            Ok(ReadOut { data, more }) => {
                if self.guest_write_bytes(buf_ptr, &data).is_none() {
                    return ERROR_INVALID_PARAMETER;
                }
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, data.len() as u32); }
                if more { ERROR_MORE_DATA } else { NO_ERROR }
            }
            Err(e) => {
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 0); }
                e
            }
        }
    }

    /// `DosWrite` on a named-pipe end: one message per call on a message
    /// pipe; a non-blocking end that is full writes nothing (unverified).
    pub(crate) fn np_write(&self, end: &NpEnd, fh_state: u32, data: &[u8]) -> Result<u32, u32> {
        if !end.may_write(fh_state & 0x7) {
            return Err(ERROR_ACCESS_DENIED);
        }
        let conn = end.conn()?;
        let mode = end.mode();
        let _blocked = BlockedGuard::new(end.blocked());
        conn.write(data, end.spec().message_type(), mode.nowait, &|| self.np_stop(), &|fresh| self.np_interrupt(fresh))
            .map(|n| n as u32)
    }

    /// DosCreateNPipe (ordinal 243): create an instance of a named pipe
    /// (state disconnected).
    #[allow(clippy::too_many_arguments)]
    pub fn dos_create_npipe(&self, psz_name: u32, ph: u32, open_mode: u32, pipe_mode: u32,
                            cb_out: u32, cb_in: u32, timeout: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosCreateNPipe('{}', open=0x{:X}, pipe=0x{:X}, out={}, in={}, timeout={})",
               name, open_mode, pipe_mode, cb_out, cb_in, timeout);
        if !npipe::is_pipe_name(&name) {
            return ERROR_PATH_NOT_FOUND;
        }
        let type_bits = (pipe_mode >> 10) & 0x3;
        let read_bits = (pipe_mode >> 8) & 0x3;
        if open_mode & !NP_OPEN_MODE_BITS != 0 || open_mode & 0x7 > NP_ACCESS_DUPLEX
            || pipe_mode & !NP_PIPE_MODE_BITS != 0 || type_bits > 1 || read_bits > 1
            || pipe_mode & NP_ICOUNT == 0
            || (type_bits == 0 && read_bits == 1) // a byte pipe is read as bytes
            || ph == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let spec = PipeSpec {
            name: name.replace('/', "\\"),
            open_mode,
            pipe_mode: pipe_mode & (NP_TYPE_MESSAGE | NP_ICOUNT),
            cb_out, cb_in,
            timeout: if timeout == 0 { npipe::DEFAULT_WAIT_MS } else { timeout },
        };
        let mode = EndMode { nowait: pipe_mode & NP_NOWAIT != 0, read_message: read_bits == 1 };
        let inst = match self.shared.npipes.lock_or_recover().create(spec, mode) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let h = self.add_pipe_handle(PipeHandle {
            obj: PipeObject::Server(inst),
            fh_state: server_fh_state(open_mode),
        });
        if self.guest_write::<u32>(ph, h).is_none() {
            self.close_pipe_handle(h);
            return ERROR_INVALID_PARAMETER;
        }
        NO_ERROR
    }

    /// `DosOpen` of `\PIPE\name`: the client end, in blocking byte-read
    /// mode (CP Reference, DosQueryNPHState). `ERROR_FILE_NOT_FOUND` when
    /// no process of the session serves the name (unverified),
    /// `ERROR_PIPE_BUSY` when no instance listens.
    pub(crate) fn np_open_client(&self, name: &str, phf: u32, pul_action: u32, open_mode: u32) -> u32 {
        let path = match self.shared.npipes.lock_or_recover().socket_path(name) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let end = match npipe::open_client(&path, &name.replace('/', "\\")) {
            Ok(c) => c,
            Err(e) => return e,
        };
        let fh_state = open_mode & 0x7FFF & !0x0F00;
        let h = self.add_pipe_handle(PipeHandle {
            obj: PipeObject::Client(Arc::new(end)),
            fh_state,
        });
        if self.guest_write::<u32>(phf, h).is_none() {
            self.close_pipe_handle(h);
            return ERROR_INVALID_PARAMETER;
        }
        if pul_action != 0 { self.guest_write::<u32>(pul_action, FILE_EXISTED); }
        NO_ERROR
    }

    /// DosConnectNPipe (ordinal 241): listen for a client. Blocking: wait
    /// until one opens the pipe; non-blocking: `ERROR_PIPE_NOT_CONNECTED`
    /// but listening. A client that came and went: `ERROR_BROKEN_PIPE`.
    pub fn dos_connect_npipe(&self, h: u32) -> u32 {
        let Some((NpEnd::Server(inst), _)) = self.np_end(h) else { return ERROR_BAD_PIPE };
        let mut st = inst.st.lock_or_recover();
        match st.state {
            NP_STATE_CONNECTED => {
                return match &st.conn {
                    Some(c) if c.peer_closed() => ERROR_BROKEN_PIPE,
                    _ => NO_ERROR,
                };
            }
            NP_STATE_DISCONNECTED => st.state = NP_STATE_LISTENING,
            _ => {}
        }
        if st.mode.nowait {
            return ERROR_PIPE_NOT_CONNECTED;
        }
        let _blocked = BlockedGuard::new(&inst.blocked);
        loop {
            match st.state {
                NP_STATE_CONNECTED => return NO_ERROR,
                // DosDisConnectNPipe from another thread.
                NP_STATE_DISCONNECTED => return ERROR_PIPE_NOT_CONNECTED,
                _ => {}
            }
            if self.np_stop() || self.interrupted() {
                return ERROR_INTERRUPT;
            }
            st = inst.cond.wait_timeout(st, std::time::Duration::from_millis(100))
                .map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// DosDisConnectNPipe (ordinal 242): end the client's connection (its
    /// unread data is lost, it sees errors from then on) and go back to the
    /// disconnected state.
    pub fn dos_disconnect_npipe(&self, h: u32) -> u32 {
        let Some((NpEnd::Server(inst), _)) = self.np_end(h) else { return ERROR_BAD_PIPE };
        let mut st = inst.st.lock_or_recover();
        if let Some(conn) = st.conn.take() {
            conn.disconnect();
        }
        st.state = NP_STATE_DISCONNECTED;
        inst.cond.notify_all();
        NO_ERROR
    }

    /// DosPeekNPipe (ordinal 244): look at the data and state without
    /// taking anything; never blocks.
    pub fn dos_peek_npipe(&self, h: u32, buf: u32, cb: u32, pcb_actual: u32, p_avail: u32, p_state: u32) -> u32 {
        let Some((end, _)) = self.np_end(h) else { return ERROR_BAD_PIPE };
        let state = end.state();
        let (data, bytes, current) = match end.conn() {
            Ok(conn) => {
                let mode = end.mode();
                let message = end.spec().message_type();
                let p = conn.peek(cb.min(NP_MAX_READ) as usize, message && mode.read_message);
                let headers = if message { p.messages * NP_MESSAGE_HEADER } else { 0 };
                (p.data, p.bytes + headers, if message { p.current } else { 0 })
            }
            // Not connected: no data, the state says why (unverified).
            Err(_) => (Vec::new(), 0, 0),
        };
        if !data.is_empty() && self.guest_write_bytes(buf, &data).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        if pcb_actual != 0 { self.guest_write::<u32>(pcb_actual, data.len() as u32); }
        if p_avail != 0 {
            // AVAILDATA: cbpipe, cbmessage (USHORTs).
            self.guest_write::<u16>(p_avail, bytes.min(0xFFFF) as u16);
            self.guest_write::<u16>(p_avail.wrapping_add(2), current.min(0xFFFF) as u16);
        }
        if p_state != 0 { self.guest_write::<u32>(p_state, state); }
        NO_ERROR
    }

    /// DosRawReadNPipe (ordinal 246): read a message pipe's messages as
    /// raw records, each a USHORT length and the bytes (`npipe::Conn::read_raw`);
    /// a byte pipe reads as `DosRead`. Undocumented beyond its prototype:
    /// the record format and `ERROR_MORE_DATA` for a cut record are
    /// Warpine's (unverified). Blocks as the end's mode says.
    pub fn dos_raw_read_npipe(&self, h: u32, buf: u32, cb: u32, pcb_actual: u32) -> u32 {
        let Some((end, fh_state)) = self.np_end(h) else { return ERROR_BAD_PIPE };
        if !end.spec().message_type() {
            return self.np_read(&end, fh_state, buf, cb, pcb_actual);
        }
        if !end.may_read(fh_state & 0x7) {
            return ERROR_ACCESS_DENIED;
        }
        let conn = match end.conn() { Ok(c) => c, Err(e) => return e };
        if let NpEnd::Client(_) = end && conn.disconnected() {
            return ERROR_PIPE_NOT_CONNECTED;
        }
        let _blocked = BlockedGuard::new(end.blocked());
        match conn.read_raw(cb.min(NP_MAX_READ) as usize, end.mode().nowait, &|| self.stop_or_restart()) {
            Ok(ReadOut { data, more }) => {
                if self.guest_write_bytes(buf, &data).is_none() {
                    return ERROR_INVALID_PARAMETER;
                }
                if pcb_actual != 0 { self.guest_write::<u32>(pcb_actual, data.len() as u32); }
                if more { ERROR_MORE_DATA } else { NO_ERROR }
            }
            Err(e) => {
                if pcb_actual != 0 { self.guest_write::<u32>(pcb_actual, 0); }
                e
            }
        }
    }

    /// DosRawWriteNPipe (ordinal 247): write raw records (a USHORT length
    /// and the bytes) to a message pipe, each as one message; a byte pipe
    /// writes as `DosWrite`. Records that run past `cb` are
    /// `ERROR_INVALID_PARAMETER` with nothing written; a full non-blocking
    /// pipe stops after the records written so far (unverified, as
    /// `DosRawReadNPipe`). `*pcbActual` counts the records' bytes, headers
    /// included.
    pub fn dos_raw_write_npipe(&self, h: u32, buf: u32, cb: u32, pcb_actual: u32) -> u32 {
        let Some((end, fh_state)) = self.np_end(h) else { return ERROR_BAD_PIPE };
        let Some(data) = self.guest_read_bytes(buf, cb as usize) else { return ERROR_INVALID_PARAMETER };
        let records = if end.spec().message_type() {
            match raw_records(&data) { Some(r) => r, None => return ERROR_INVALID_PARAMETER }
        } else {
            vec![&data[..]]
        };
        let header = if end.spec().message_type() { npipe::RAW_HEADER as u32 } else { 0 };
        let mut written = 0u32;
        let mut rc = NO_ERROR;
        for record in records {
            match self.np_write(&end, fh_state, record) {
                Ok(n) if n as usize == record.len() => written += header + n,
                // A full non-blocking pipe: a message pipe wrote nothing of
                // this record, a byte pipe part of it.
                Ok(n) => { written += n; break; }
                Err(e) => { rc = e; break; }
            }
        }
        if pcb_actual != 0 { self.guest_write::<u32>(pcb_actual, written); }
        rc
    }

    /// Write a message and read the reply on a duplex message pipe in
    /// message-read mode (DosTransactNPipe / DosCallNPipe).
    #[allow(clippy::too_many_arguments)]
    fn np_transact(&self, end: &NpEnd, fh_state: u32, out: u32, cb_out: u32,
                   inb: u32, cb_in: u32, pcb_read: u32) -> u32 {
        let spec = end.spec();
        if spec.access() != NP_ACCESS_DUPLEX || !spec.message_type() || !end.mode().read_message {
            return ERROR_BAD_FORMAT;
        }
        if !end.may_write(fh_state & 0x7) || !end.may_read(fh_state & 0x7) {
            return ERROR_ACCESS_DENIED;
        }
        let conn = match end.conn() { Ok(c) => c, Err(e) => return e };
        if conn.has_unread() {
            // "fails if the pipe still has unread data" (error unverified).
            return ERROR_PIPE_BUSY;
        }
        let Some(msg) = self.guest_read_bytes(out, cb_out.min(npipe::MAX_MESSAGE as u32 + 1) as usize) else {
            return ERROR_INVALID_PARAMETER;
        };
        let _blocked = BlockedGuard::new(end.blocked());
        // Non-blocking mode has no effect on a transaction.
        if let Err(e) = conn.write(&msg, true, false, &|| self.np_stop(), &|fresh| self.np_interrupt(fresh)) {
            return e;
        }
        match conn.read(cb_in.min(NP_MAX_READ) as usize, true, false, &|| self.np_stop()) {
            Ok(ReadOut { data, more }) => {
                if self.guest_write_bytes(inb, &data).is_none() {
                    return ERROR_INVALID_PARAMETER;
                }
                if pcb_read != 0 { self.guest_write::<u32>(pcb_read, data.len() as u32); }
                if more { ERROR_MORE_DATA } else { NO_ERROR }
            }
            Err(e) => e,
        }
    }

    /// DosTransactNPipe (ordinal 252).
    pub fn dos_transact_npipe(&self, h: u32, out: u32, cb_out: u32, inb: u32, cb_in: u32, pcb_read: u32) -> u32 {
        let Some((end, fh_state)) = self.np_end(h) else { return ERROR_BAD_PIPE };
        self.np_transact(&end, fh_state, out, cb_out, inb, cb_in, pcb_read)
    }

    /// DosCallNPipe (ordinal 240): open, transact and close in one call,
    /// waiting up to `msec` for a free instance; still none:
    /// `ERROR_PIPE_BUSY` (unverified — the reference also names
    /// ERROR_INTERRUPT for the timeout).
    #[allow(clippy::too_many_arguments)]
    pub fn dos_call_npipe(&self, psz_name: u32, inb: u32, cb_in: u32, out: u32, cb_out: u32,
                          pcb_actual: u32, msec: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosCallNPipe('{}', cbIn={}, cbOut={}, msec={})", name, cb_in, cb_out, msec);
        if !npipe::is_pipe_name(&name) {
            return ERROR_FILE_NOT_FOUND;
        }
        let name = name.replace('/', "\\");
        let path = match self.shared.npipes.lock_or_recover().socket_path(&name) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(msec as u64);
        let end = loop {
            match npipe::open_client(&path, &name) {
                Ok(c) => break c,
                Err(ERROR_PIPE_BUSY) if std::time::Instant::now() < deadline => {
                    if self.np_stop() { return ERROR_INTERRUPT; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => return e,
            }
        };
        *end.mode.lock_or_recover() = EndMode { nowait: false, read_message: true };
        let end = NpEnd::Client(Arc::new(end));
        // The pipe's input buffer is the caller's output (pInbuf is written).
        self.np_transact(&end, OPEN_ACCESS_READWRITE, inb, cb_in, out, cb_out, pcb_actual)
    }

    /// DosWaitNPipe (ordinal 253): wait until an instance of the pipe
    /// listens; `ERROR_SEM_TIMEOUT` when none did in time.
    pub fn dos_wait_npipe(&self, psz_name: u32, msec: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosWaitNPipe('{}', {})", name, msec);
        if !npipe::is_pipe_name(&name) {
            return ERROR_BAD_PIPE;
        }
        let name = name.replace('/', "\\");
        let path = match self.shared.npipes.lock_or_recover().socket_path(&name) {
            Ok(p) => p,
            Err(e) => return e,
        };
        match npipe::wait_for_instance(&path, &name, msec, &|| self.np_stop() || self.interrupted()) {
            Ok(()) => NO_ERROR,
            Err(e) => e,
        }
    }

    /// DosQueryNPHState (ordinal 245).
    pub fn dos_query_nph_state(&self, h: u32, p_state: u32) -> u32 {
        let Some((end, _)) = self.np_end(h) else { return ERROR_BAD_PIPE };
        if p_state == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        if let NpEnd::Client(c) = &end && c.conn.disconnected() {
            return ERROR_PIPE_NOT_CONNECTED;
        }
        if self.guest_write::<u32>(p_state, end.nph_state()).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        NO_ERROR
    }

    /// DosSetNPHState (ordinal 250): blocking and read mode, both given.
    /// A byte pipe cannot be read as messages; an end with a blocked
    /// thread cannot become non-blocking (`ERROR_PIPE_BUSY`).
    pub fn dos_set_nph_state(&self, h: u32, state: u32) -> u32 {
        let Some((end, _)) = self.np_end(h) else { return ERROR_BAD_PIPE };
        let read_bits = (state >> 8) & 0x3;
        if state & !NP_SET_STATE_BITS != 0 || read_bits > 1
            || (read_bits == 1 && !end.spec().message_type()) {
            return ERROR_INVALID_PARAMETER;
        }
        if let NpEnd::Client(c) = &end && c.conn.disconnected() {
            return ERROR_PIPE_NOT_CONNECTED;
        }
        let new = EndMode { nowait: state & NP_NOWAIT != 0, read_message: read_bits == 1 };
        if new.nowait && !end.mode().nowait && end.blocked().load(Ordering::SeqCst) > 0 {
            return ERROR_PIPE_BUSY;
        }
        end.set_mode(new);
        NO_ERROR
    }

    /// DosQueryNPipeInfo (ordinal 248): level 1 = PIPEINFO, level 2 = the
    /// 4-byte client id (0 for local pipes). What does not fit is cut
    /// off with `ERROR_BUFFER_OVERFLOW`.
    pub fn dos_query_npipe_info(&self, h: u32, level: u32, buf: u32, cb: u32) -> u32 {
        let Some((end, _)) = self.np_end(h) else { return ERROR_BAD_PIPE };
        let info = match level {
            1 => {
                let spec = end.spec();
                let cur = match &end {
                    NpEnd::Server(i) => i.pipe.instance_count() as u32,
                    // When the client connected (another process may serve
                    // the pipe).
                    NpEnd::Client(c) => c.cur_inst,
                };
                let name = super::codepage::cp_encode(&spec.name,
                    self.shared.active_codepage.load(Ordering::Relaxed));
                let mut v = Vec::with_capacity(8 + name.len());
                v.extend_from_slice(&(spec.cb_out.min(0xFFFF) as u16).to_le_bytes());
                v.extend_from_slice(&(spec.cb_in.min(0xFFFF) as u16).to_le_bytes());
                v.push(spec.max_instances() as u8);
                v.push(cur.min(0xFF) as u8);
                // A name over 255 bytes has length 0 (CP Reference).
                v.push(if name.len() > 255 { 0 } else { name.len() as u8 });
                v.extend_from_slice(&name);
                v.push(0);
                v
            }
            2 => vec![0; 4],
            _ => return ERROR_INVALID_LEVEL,
        };
        let n = info.len().min(cb as usize);
        if n > 0 && self.guest_write_bytes(buf, &info[..n]).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        if n < info.len() { ERROR_BUFFER_OVERFLOW } else { NO_ERROR }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Loader;
    use super::super::npipe::{NamedPipes, PipeDir};
    use std::time::{Duration, Instant};

    const PH: u32 = 0x3000;
    const PCB: u32 = 0x3004;
    const PAVAIL: u32 = 0x3008;
    const PSTATE: u32 = 0x300C;
    const PACTION: u32 = 0x3010;
    const NAME: u32 = 0x3100;
    const BUF: u32 = 0x4000;
    const BUF2: u32 = 0x5000;

    /// A mock loader whose pipes live in a temporary directory.
    fn loader() -> (Loader, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let l = Loader::new_mock();
        *l.shared.npipes.lock().unwrap() = NamedPipes::new(PipeDir::at(dir.path().join("p")));
        (l, dir)
    }

    fn put(l: &Loader, at: u32, s: &str) -> u32 {
        l.guest_write_bytes(at, s.as_bytes()).unwrap();
        l.guest_write::<u8>(at + s.len() as u32, 0).unwrap();
        at
    }

    fn create(l: &Loader, name: &str, open: u32, pipe: u32) -> Result<u32, u32> {
        put(l, NAME, name);
        match l.dos_create_npipe(NAME, PH, open, pipe, 4096, 4096, 0) {
            NO_ERROR => Ok(l.guest_read::<u32>(PH).unwrap()),
            e => Err(e),
        }
    }

    fn open(l: &Loader, name: &str, mode: u32) -> Result<u32, u32> {
        put(l, NAME, name);
        match l.dos_open(NAME, PH, PACTION, 0x01, mode) {
            NO_ERROR => Ok(l.guest_read::<u32>(PH).unwrap()),
            e => Err(e),
        }
    }

    fn write(l: &Loader, h: u32, data: &[u8]) -> u32 {
        l.guest_write_bytes(BUF, data).unwrap();
        l.dos_write(h, BUF, data.len() as u32, PCB)
    }

    fn read(l: &Loader, h: u32, len: u32) -> (u32, Vec<u8>) {
        let rc = l.dos_read(h, BUF2, len, PCB);
        let n = l.guest_read::<u32>(PCB).unwrap();
        (rc, l.guest_read_bytes(BUF2, n as usize).unwrap())
    }

    /// Listen in the background, open, and wait until connected.
    fn connect(l: &Arc<Loader>, hs: u32, name: &str, mode: u32) -> u32 {
        let l2 = l.clone();
        let t = std::thread::spawn(move || l2.dos_connect_npipe(hs));
        let deadline = Instant::now() + Duration::from_secs(5);
        let hc = loop {
            match open(l, name, mode) {
                Ok(h) => break h,
                Err(ERROR_PIPE_BUSY) => {
                    assert!(Instant::now() < deadline, "never listened");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("DosOpen: {e}"),
            }
        };
        assert_eq!(t.join().unwrap(), NO_ERROR);
        hc
    }

    #[test]
    fn create_checks_its_arguments() {
        let (l, _d) = loader();
        assert_eq!(create(&l, "\\SEM32\\X", NP_ACCESS_DUPLEX, 1), Err(ERROR_PATH_NOT_FOUND));
        assert_eq!(create(&l, "\\PIPE\\A", 3, 1), Err(ERROR_INVALID_PARAMETER), "access 3");
        assert_eq!(create(&l, "\\PIPE\\A", 0x0008, 1), Err(ERROR_INVALID_PARAMETER), "reserved bit");
        assert_eq!(create(&l, "\\PIPE\\A", 0, 0), Err(ERROR_INVALID_PARAMETER), "icount 0");
        assert_eq!(create(&l, "\\PIPE\\A", 0, 0x0800 | 1), Err(ERROR_INVALID_PARAMETER), "type 2");
        assert_eq!(create(&l, "\\PIPE\\A", 0, NP_READMODE_MESSAGE | 1), Err(ERROR_INVALID_PARAMETER),
                   "byte pipe read as messages");
        put(&l, NAME, "\\PIPE\\A");
        assert_eq!(l.dos_create_npipe(NAME, 0, 0, 1, 0, 0, 0), ERROR_INVALID_PARAMETER);
        let h = create(&l, "\\PIPE\\A", NP_ACCESS_DUPLEX, 2).unwrap();
        assert!(create(&l, "\\pipe\\a", NP_ACCESS_DUPLEX, 2).is_ok(), "second instance");
        assert_eq!(create(&l, "\\PIPE\\A", NP_ACCESS_DUPLEX, 2), Err(ERROR_PIPE_BUSY), "two at most");
        assert_eq!(l.dos_query_h_type(h, PCB, 0), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PCB), Some(HANDTYPE_PIPE));
        // Not a named pipe.
        assert_eq!(l.dos_connect_npipe(1), ERROR_BAD_PIPE);
        assert_eq!(l.dos_query_nph_state(0x7777, PSTATE), ERROR_BAD_PIPE);
    }

    /// Server and client in one process: states, messages, peek, modes,
    /// info, transaction, disconnect.
    #[test]
    fn message_pipe_round_trip() {
        let (l, _d) = loader();
        let l = Arc::new(l);
        let hs = create(&l, "\\PIPE\\RT", NP_ACCESS_DUPLEX, NP_TYPE_MESSAGE | NP_READMODE_MESSAGE | 1).unwrap();
        assert_eq!(l.dos_peek_npipe(hs, BUF2, 16, PCB, PAVAIL, PSTATE), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PSTATE), Some(NP_STATE_DISCONNECTED));
        assert_eq!(open(&l, "\\PIPE\\RT", OPEN_ACCESS_READWRITE), Err(ERROR_PIPE_BUSY), "not listening");
        assert_eq!(open(&l, "\\PIPE\\NOPE", OPEN_ACCESS_READWRITE), Err(ERROR_FILE_NOT_FOUND));
        let hc = connect(&l, hs, "\\pipe\\rt", OPEN_ACCESS_READWRITE);
        assert_eq!(l.guest_read::<u32>(PACTION), Some(FILE_EXISTED));
        assert_eq!(l.dos_connect_npipe(hs), NO_ERROR, "already connected");
        assert_eq!(l.dos_connect_npipe(hc), ERROR_BAD_PIPE, "a client cannot connect");

        // States.
        assert_eq!(l.dos_query_nph_state(hs, PSTATE), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PSTATE), Some(NP_END_SERVER | NP_TYPE_MESSAGE | NP_READMODE_MESSAGE | 1));
        assert_eq!(l.dos_query_nph_state(hc, PSTATE), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PSTATE), Some(NP_TYPE_MESSAGE | 1), "client: blocking, byte read");
        assert_eq!(l.dos_query_nph_state(hc, 0), ERROR_INVALID_PARAMETER);

        // Client → server messages; a short read leaves the rest.
        assert_eq!(write(&l, hc, b"first"), NO_ERROR);
        assert_eq!(write(&l, hc, b"second"), NO_ERROR);
        assert_eq!(l.dos_peek_npipe(hs, BUF2, 3, PCB, PAVAIL, PSTATE), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PCB), Some(3));
        assert_eq!(l.guest_read::<u16>(PAVAIL), Some(11 + 2 * 2), "bytes + message headers");
        assert_eq!(l.guest_read::<u16>(PAVAIL + 2), Some(5));
        assert_eq!(l.guest_read::<u32>(PSTATE), Some(NP_STATE_CONNECTED));
        assert_eq!(read(&l, hs, 3), (ERROR_MORE_DATA, b"fir".to_vec()));
        assert_eq!(read(&l, hs, 64), (NO_ERROR, b"st".to_vec()));
        assert_eq!(read(&l, hs, 64), (NO_ERROR, b"second".to_vec()));
        // Non-blocking and empty.
        assert_eq!(l.dos_set_nph_state(hs, NP_NOWAIT | NP_READMODE_MESSAGE), NO_ERROR);
        assert_eq!(read(&l, hs, 64).0, ERROR_NO_DATA);
        assert_eq!(l.dos_set_nph_state(hs, NP_READMODE_MESSAGE), NO_ERROR);
        assert_eq!(l.dos_set_nph_state(hs, 0x0001), ERROR_INVALID_PARAMETER);

        // Transaction: client asks, server answers.
        let srv = {
            let l = l.clone();
            std::thread::spawn(move || {
                let (rc, q) = read(&l, hs, 64);
                assert_eq!((rc, q.as_slice()), (NO_ERROR, &b"question"[..]));
                write(&l, hs, b"answer")
            })
        };
        assert_eq!(l.dos_transact_npipe(hc, BUF, 0, BUF2, 64, PCB), ERROR_BAD_FORMAT, "client in byte-read mode");
        assert_eq!(l.dos_set_nph_state(hc, NP_READMODE_MESSAGE), NO_ERROR);
        l.guest_write_bytes(BUF, b"question").unwrap();
        assert_eq!(l.dos_transact_npipe(hc, BUF, 8, BUF2, 64, PCB), NO_ERROR);
        assert_eq!(srv.join().unwrap(), NO_ERROR);
        assert_eq!(l.guest_read_bytes(BUF2, 6).unwrap(), b"answer");

        // Info, level 1.
        assert_eq!(l.dos_query_npipe_info(hs, 1, BUF2, 64), NO_ERROR);
        let info = l.guest_read_bytes(BUF2, 16).unwrap();
        assert_eq!(&info[..7], &[0x00, 0x10, 0x00, 0x10, 1, 1, 8]);
        assert_eq!(&info[7..16], b"\\PIPE\\RT\0");
        assert_eq!(l.dos_query_npipe_info(hs, 1, BUF2, 5), ERROR_BUFFER_OVERFLOW);
        assert_eq!(l.dos_query_npipe_info(hs, 2, BUF2, 4), NO_ERROR);
        assert_eq!(l.dos_query_npipe_info(hs, 3, BUF2, 64), ERROR_INVALID_LEVEL);

        // Server disconnects: the client's data is gone, it gets errors.
        assert_eq!(write(&l, hs, b"unread"), NO_ERROR);
        assert_eq!(l.dos_disconnect_npipe(hs), NO_ERROR);
        assert_eq!(l.dos_disconnect_npipe(hc), ERROR_BAD_PIPE);
        assert_eq!(read(&l, hc, 64).0, ERROR_PIPE_NOT_CONNECTED);
        assert_eq!(write(&l, hc, b"x"), ERROR_BROKEN_PIPE);
        assert_eq!(l.dos_peek_npipe(hs, BUF2, 16, PCB, PAVAIL, PSTATE), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PSTATE), Some(NP_STATE_DISCONNECTED));
        assert_eq!(read(&l, hs, 64).0, ERROR_PIPE_NOT_CONNECTED);
        assert_eq!(l.dos_close(hc), NO_ERROR);

        // Listen again for a new client, which then closes: closing state.
        let hc2 = connect(&l, hs, "\\PIPE\\RT", OPEN_ACCESS_READWRITE);
        assert_eq!(l.dos_close(hc2), NO_ERROR);
        assert_eq!(l.dos_peek_npipe(hs, BUF2, 16, PCB, PAVAIL, PSTATE), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PSTATE), Some(NP_STATE_CLOSING));
        assert_eq!(read(&l, hs, 64), (NO_ERROR, vec![]), "end of data");
        assert_eq!(write(&l, hs, b"x"), ERROR_BROKEN_PIPE);
        assert_eq!(l.dos_connect_npipe(hs), ERROR_BROKEN_PIPE);
        assert_eq!(l.dos_close(hs), NO_ERROR);
    }

    /// Byte pipes: data flows as a stream; access directions are enforced;
    /// non-blocking connect listens.
    /// Raw records: what DosRawWriteNPipe writes arrives as messages, and
    /// DosRawReadNPipe gives messages back as records (unverified format).
    #[test]
    fn raw_reads_and_writes() {
        let (l, _d) = loader();
        let l = Arc::new(l);
        let hs = create(&l, "\\PIPE\\RAW", NP_ACCESS_DUPLEX, NP_TYPE_MESSAGE | NP_READMODE_MESSAGE | 1).unwrap();
        let hc = connect(&l, hs, "\\PIPE\\RAW", OPEN_ACCESS_READWRITE);
        let records = b"\x02\x00hi\x00\x00\x03\x00abc";
        l.guest_write_bytes(BUF, records);
        assert_eq!(l.dos_raw_write_npipe(hc, BUF, records.len() as u32, PCB), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PCB), Some(records.len() as u32));
        assert_eq!(read(&l, hs, 64), (NO_ERROR, b"hi".to_vec()), "one message per record");
        assert_eq!(l.dos_raw_read_npipe(hs, BUF2, 64, PCB), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PCB), Some(7));
        assert_eq!(l.guest_read_bytes(BUF2, 7).unwrap(), b"\x00\x00\x03\x00abc");
        // A record running past the buffer writes nothing.
        l.guest_write_bytes(BUF, b"\x01\x00x\x09\x00ab");
        assert_eq!(l.dos_raw_write_npipe(hc, BUF, 7, PCB), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_peek_npipe(hs, BUF2, 16, PCB, PAVAIL, PSTATE), NO_ERROR);
        assert_eq!(l.guest_read::<u16>(PAVAIL), Some(0));
        // A cut record: ERROR_MORE_DATA, the rest as the next record.
        assert_eq!(write(&l, hc, b"abcdef"), NO_ERROR);
        assert_eq!(l.dos_raw_read_npipe(hs, BUF2, 4, PCB), ERROR_MORE_DATA);
        assert_eq!(l.guest_read_bytes(BUF2, 4).unwrap(), b"\x02\x00ab");
        assert_eq!(l.dos_raw_read_npipe(hs, BUF2, 64, PCB), NO_ERROR);
        assert_eq!(l.guest_read_bytes(BUF2, 6).unwrap(), b"\x04\x00cdef");
        assert_eq!(l.dos_raw_read_npipe(0x7777, BUF2, 64, PCB), ERROR_BAD_PIPE);
    }

    #[test]
    fn byte_pipe_and_access() {
        let (l, _d) = loader();
        let l = Arc::new(l);
        let hs = create(&l, "\\PIPE\\IN", NP_ACCESS_INBOUND, NP_NOWAIT | 1).unwrap();
        assert_eq!(l.dos_connect_npipe(hs), ERROR_PIPE_NOT_CONNECTED, "non-blocking");
        let hc = open(&l, "\\PIPE\\IN", OPEN_ACCESS_WRITEONLY).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while l.dos_connect_npipe(hs) != NO_ERROR {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(write(&l, hc, b"ab"), NO_ERROR);
        assert_eq!(write(&l, hc, b"cd"), NO_ERROR);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < 4 {
            assert!(Instant::now() < deadline);
            let (rc, d) = read(&l, hs, 64);
            assert!(rc == NO_ERROR || rc == ERROR_NO_DATA);
            got.extend(d);
        }
        assert_eq!(got, b"abcd");
        assert_eq!(write(&l, hs, b"x"), ERROR_ACCESS_DENIED, "inbound: server only reads");
        assert_eq!(read(&l, hc, 4).0, ERROR_ACCESS_DENIED, "inbound: client only writes");
        assert_eq!(l.dos_set_nph_state(hs, NP_READMODE_MESSAGE), ERROR_INVALID_PARAMETER, "byte pipe");
        assert_eq!(l.dos_transact_npipe(hc, BUF, 1, BUF2, 1, PCB), ERROR_BAD_FORMAT);
    }

    /// DosWaitNPipe times out until an instance listens; DosCallNPipe does
    /// the whole exchange.
    #[test]
    fn wait_and_call() {
        let (l, _d) = loader();
        let l = Arc::new(l);
        put(&l, NAME, "\\PIPE\\NONE");
        assert_eq!(l.dos_wait_npipe(NAME, 10), ERROR_FILE_NOT_FOUND);
        put(&l, NAME, "\\QUEUES\\X");
        assert_eq!(l.dos_wait_npipe(NAME, 10), ERROR_BAD_PIPE);
        let hs = create(&l, "\\PIPE\\CALL", NP_ACCESS_DUPLEX, NP_TYPE_MESSAGE | NP_READMODE_MESSAGE | 1).unwrap();
        put(&l, NAME, "\\PIPE\\CALL");
        assert_eq!(l.dos_wait_npipe(NAME, 20), ERROR_SEM_TIMEOUT);
        let server = {
            let l = l.clone();
            std::thread::spawn(move || {
                assert_eq!(l.dos_connect_npipe(hs), NO_ERROR);
                let (rc, q) = read(&l, hs, 64);
                assert_eq!(rc, NO_ERROR);
                let mut reply = b"re:".to_vec();
                reply.extend(q);
                assert_eq!(write(&l, hs, &reply), NO_ERROR);
            })
        };
        assert_eq!(l.dos_wait_npipe(NAME, NP_INDEFINITE_WAIT), NO_ERROR);
        let name = put(&l, 0x3200, "\\PIPE\\CALL");
        l.guest_write_bytes(BUF, b"hi").unwrap();
        assert_eq!(l.dos_call_npipe(name, BUF, 2, BUF2, 3, PCB, 1000), ERROR_MORE_DATA, "reply too long");
        assert_eq!(l.guest_read_bytes(BUF2, 3).unwrap(), b"re:");
        server.join().unwrap();
    }

    /// A duplicated handle shares the end; the end closes with its last
    /// handle.
    #[test]
    fn dup_and_close() {
        let (l, _d) = loader();
        let l = Arc::new(l);
        let hs = create(&l, "\\PIPE\\DUP", NP_ACCESS_DUPLEX, 1).unwrap();
        let hc = connect(&l, hs, "\\PIPE\\DUP", OPEN_ACCESS_READWRITE);
        l.guest_write::<u32>(PCB, NEW_HANDLE_REQUEST).unwrap();
        assert_eq!(l.dos_dup_handle(hc, PCB), NO_ERROR);
        let hd = l.guest_read::<u32>(PCB).unwrap();
        assert_eq!(l.dos_close(hc), NO_ERROR);
        assert_eq!(write(&l, hd, b"via dup"), NO_ERROR);
        assert_eq!(read(&l, hs, 64), (NO_ERROR, b"via dup".to_vec()));
        assert_eq!(l.dos_close(hd), NO_ERROR);
        assert_eq!(read(&l, hs, 64), (NO_ERROR, vec![]), "the client end is closed");
    }
}
