// SPDX-License-Identifier: GPL-3.0-only
//
// SO32DLL / TCP32DLL — OS/2 TCP/IP Socket API emulation.
//
// Implements the core BSD socket APIs exported by OS/2 Warp's SO32DLL.DLL and
// TCP32DLL.DLL (a re-export layer with identical ordinals).  All calls are
// forwarded to the Linux host via libc syscalls; OS/2 socket handles are mapped
// to Linux file descriptors in `SocketManager`.
//
// # Handle semantics
// `socket()` allocates a new OS/2 handle (starting at 0xA000) and stores the
// mapping OS/2-handle → Linux-fd in `SocketManager`.  `soclose()` removes the
// mapping and calls `close(2)`.  Handles are private to this manager and do not
// collide with `HandleManager` (which starts at PIPE_HANDLE_BASE = 0x1000).
//
// # sockaddr layout
// OS/2's `sockaddr_in` is binary-identical to Linux's (both follow POSIX):
//   `sa_family(u16) + sin_port(u16, network order) + sin_addr(u32) + sin_zero(8)`
// We read the raw bytes from guest memory and pass them directly to the host.
//
// # select() fd_set translation
// The OS/2 TCP/IP `fd_set` is a count and an array of socket numbers (Open
// Watcom `h/os2/types.h`), not a bit mask, so any socket number fits. Each
// listed socket is looked up, watched with poll(2) (no host FD_SETSIZE
// limit) and written back if ready; see `so_select`.
//
// # gethostbyname / getservbyname
// The resolved result is written into a scratch buffer owned by `SocketManager`
// and registered in `SharedState::mem_mgr` so it remains accessible after the
// call returns.
//
// # sock_errno()
// OS/2's `sock_errno()` returns the error from the last failed socket call.
// We store it in `SocketManager::last_sock_errno` and reset it to 0 on success.
//
// # SOCE* error codes (from OS/2 Warp TCP/IP toolkit <nerrno.h>)
//   These mirror BSD errno values but diverge at EWOULDBLOCK and above.

use std::collections::HashMap;
use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicI32, Ordering};

use tracing::{debug, warn};

use super::MutexExt;

// ── SOCE* error code constants ────────────────────────────────────────────────

pub const SOCE_INTR:           i32 = 4;
pub const SOCE_BADF:           i32 = 9;
pub const SOCE_ACCES:          i32 = 13;
pub const SOCE_FAULT:          i32 = 14;
pub const SOCE_INVAL:          i32 = 22;
pub const SOCE_MFILE:          i32 = 24;
pub const SOCE_WOULDBLOCK:     i32 = 35;
pub const SOCE_INPROGRESS:     i32 = 36;
pub const SOCE_ALREADY:        i32 = 37;
pub const SOCE_NOTSOCK:        i32 = 38;
pub const SOCE_DESTADDRREQ:    i32 = 39;
pub const SOCE_MSGSIZE:        i32 = 40;
pub const SOCE_PROTOTYPE:      i32 = 41;
pub const SOCE_NOPROTOOPT:     i32 = 42;
pub const SOCE_PROTONOSUPPORT: i32 = 43;
pub const SOCE_SOCKTNOSUPPORT: i32 = 44;
pub const SOCE_OPNOTSUPP:      i32 = 45;
pub const SOCE_AFNOSUPPORT:    i32 = 47;
pub const SOCE_ADDRINUSE:      i32 = 48;
pub const SOCE_ADDRNOTAVAIL:   i32 = 49;
pub const SOCE_NETDOWN:        i32 = 50;
pub const SOCE_NETUNREACH:     i32 = 51;
pub const SOCE_CONNABORTED:    i32 = 53;
pub const SOCE_CONNRESET:      i32 = 54;
pub const SOCE_NOBUFS:         i32 = 55;
pub const SOCE_ISCONN:         i32 = 56;
pub const SOCE_NOTCONN:        i32 = 57;
pub const SOCE_TIMEDOUT:       i32 = 60;
pub const SOCE_CONNREFUSED:    i32 = 61;
pub const SOCE_HOSTDOWN:       i32 = 64;
pub const SOCE_HOSTUNREACH:    i32 = 65;

/// Maximum byte count accepted from guest for any single socket buffer operation.
const MAX_SOCKET_BUF: usize = 64 * 1024;

// OS/2 socket success value
const SOCK_SUCCESS: u32 = 0;
// OS/2 socket error sentinel (returned from socket API on error)
const SOCK_ERROR: u32 = u32::MAX; // -1 as u32

// ── Socket option numbers (OS/2 toolkit <sys/socket.h>, <netinet/in.h>,
//    <netinet/tcp.h>; BSD numbering, unlike Linux's) ──────────────────────────
pub const OS2_SOL_SOCKET:     u32 = 0xFFFF;
pub const OS2_IPPROTO_TCP:    u32 = 6;
pub const OS2_TCP_NODELAY:    u32 = 0x0001;
pub const OS2_SO_DEBUG:       u32 = 0x0001;
pub const OS2_SO_ACCEPTCONN:  u32 = 0x0002;
pub const OS2_SO_REUSEADDR:   u32 = 0x0004;
pub const OS2_SO_KEEPALIVE:   u32 = 0x0008;
pub const OS2_SO_DONTROUTE:   u32 = 0x0010;
pub const OS2_SO_BROADCAST:   u32 = 0x0020;
pub const OS2_SO_LINGER:      u32 = 0x0080;
pub const OS2_SO_OOBINLINE:   u32 = 0x0100;
pub const OS2_SO_REUSEPORT:   u32 = 0x1000;
pub const OS2_SO_SNDBUF:      u32 = 0x1001;
pub const OS2_SO_RCVBUF:      u32 = 0x1002;
pub const OS2_SO_SNDLOWAT:    u32 = 0x1003;
pub const OS2_SO_RCVLOWAT:    u32 = 0x1004;
pub const OS2_SO_ERROR:       u32 = 0x1007;
pub const OS2_SO_TYPE:        u32 = 0x1008;
/// Longest option value passed through `setsockopt` (the largest supported
/// option, `struct linger`, is 8 bytes).
const MAX_SOCKOPT_LEN: u32 = 64;

/// The Linux `(level, optname)` of an OS/2 socket option, or `None` when
/// Warpine does not support it. Only options whose value has the same
/// layout on both sides (an `int`, or `struct linger` = two `int`s) are
/// mapped; `SO_SNDTIMEO` / `SO_RCVTIMEO` (a 32-bit `struct timeval` on
/// OS/2, 64-bit on Linux) and IP-level options are not.
pub fn linux_sockopt(level: u32, optname: u32) -> Option<(i32, i32)> {
    match level {
        OS2_SOL_SOCKET => {
            let name = match optname {
                OS2_SO_DEBUG => libc::SO_DEBUG,
                OS2_SO_ACCEPTCONN => libc::SO_ACCEPTCONN,
                OS2_SO_REUSEADDR => libc::SO_REUSEADDR,
                OS2_SO_KEEPALIVE => libc::SO_KEEPALIVE,
                OS2_SO_DONTROUTE => libc::SO_DONTROUTE,
                OS2_SO_BROADCAST => libc::SO_BROADCAST,
                OS2_SO_LINGER => libc::SO_LINGER,
                OS2_SO_OOBINLINE => libc::SO_OOBINLINE,
                OS2_SO_REUSEPORT => libc::SO_REUSEPORT,
                OS2_SO_SNDBUF => libc::SO_SNDBUF,
                OS2_SO_RCVBUF => libc::SO_RCVBUF,
                OS2_SO_SNDLOWAT => libc::SO_SNDLOWAT,
                OS2_SO_RCVLOWAT => libc::SO_RCVLOWAT,
                OS2_SO_ERROR => libc::SO_ERROR,
                OS2_SO_TYPE => libc::SO_TYPE,
                _ => return None,
            };
            Some((libc::SOL_SOCKET, name))
        }
        OS2_IPPROTO_TCP if optname == OS2_TCP_NODELAY => Some((libc::IPPROTO_TCP, libc::TCP_NODELAY)),
        _ => None,
    }
}

// ── select() fd_set ───────────────────────────────────────────────────────────
//
// The OS/2 TCP/IP `fd_set` of Open Watcom's `h/os2/types.h` (the TCP/IP 5.0
// layout): `u_short fd_count` (2 bytes of padding follow, `#pragma pack(4)`)
// and `int fd_array[FD_SETSIZE]` with FD_SETSIZE 64 — a count and the socket
// numbers themselves, so any socket number fits (Warpine's start at 0xA000).

/// `FD_SETSIZE`: socket numbers one `fd_set` holds.
pub const OS2_FD_SETSIZE: usize = 64;
/// Offset of `fd_array` (after `fd_count` and its padding).
const OS2_FD_ARRAY_OFFSET: usize = 4;
/// Size of an OS/2 `fd_set`.
pub const OS2_FD_SET_BYTES: usize = OS2_FD_ARRAY_OFFSET + 4 * OS2_FD_SETSIZE;
/// How long one poll(2) waits before `select` checks for an interrupt.
const SELECT_POLL_SLICE: std::time::Duration = std::time::Duration::from_millis(100);

/// The socket numbers of an `fd_set` (`OS2_FD_SET_BYTES` bytes); `None`
/// when `fd_count` is above `FD_SETSIZE`.
pub fn read_fd_list(bytes: &[u8]) -> Option<Vec<u32>> {
    let count = usize::from(u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]));
    if count > OS2_FD_SETSIZE {
        return None;
    }
    (0..count).map(|i| {
        let at = OS2_FD_ARRAY_OFFSET + 4 * i;
        bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }).collect()
}

/// An `fd_set` holding `sockets` (at most `FD_SETSIZE`; more are dropped).
pub fn write_fd_list(sockets: &[u32]) -> [u8; OS2_FD_SET_BYTES] {
    let mut out = [0u8; OS2_FD_SET_BYTES];
    let n = sockets.len().min(OS2_FD_SETSIZE);
    out[..2].copy_from_slice(&(n as u16).to_le_bytes());
    for (i, s) in sockets[..n].iter().enumerate() {
        let at = OS2_FD_ARRAY_OFFSET + 4 * i;
        out[at..at + 4].copy_from_slice(&s.to_le_bytes());
    }
    out
}

/// Whether a polled socket counts as ready for its set, as select(2) does:
/// readable or writable also on an error or hang-up.
fn select_ready(p: &libc::pollfd) -> bool {
    let hit = match p.events {
        libc::POLLIN => libc::POLLIN | libc::POLLHUP | libc::POLLERR,
        libc::POLLOUT => libc::POLLOUT | libc::POLLHUP | libc::POLLERR,
        _ => libc::POLLPRI,
    };
    p.revents & hit != 0
}

// ── SocketManager ─────────────────────────────────────────────────────────────

/// Tracks live OS/2 socket handles → Linux fd mappings and per-process state.
pub struct SocketManager {
    /// OS/2 socket handle → Linux file descriptor.
    sockets: HashMap<u32, i32>,
    next_handle: u32,
    /// Error code from the last failed socket call (returned by sock_errno).
    pub last_sock_errno: AtomicI32,
    /// Scratch guest memory address for gethostbyname/getservbyname results.
    /// Allocated once from mem_mgr on first use; 256 bytes.
    pub scratch_addr: Option<u32>,
}

impl Default for SocketManager {
    fn default() -> Self { Self::new() }
}

impl SocketManager {
    pub fn new() -> Self {
        Self {
            sockets: HashMap::new(),
            next_handle: 0xA000,
            last_sock_errno: AtomicI32::new(0),
            scratch_addr: None,
        }
    }

    /// Allocate a new OS/2 socket handle wrapping `fd`. Returns the handle.
    pub fn alloc(&mut self, fd: i32) -> u32 {
        let h = self.next_handle;
        self.next_handle += 1;
        self.sockets.insert(h, fd);
        h
    }

    /// Look up the Linux fd for an OS/2 socket handle.
    pub fn lookup(&self, h: u32) -> Option<i32> {
        self.sockets.get(&h).copied()
    }

    /// Remove the handle mapping (does NOT close the fd; caller must do that).
    pub fn remove(&mut self, h: u32) -> Option<i32> {
        self.sockets.remove(&h)
    }

    /// Set the last socket error code.
    pub fn set_errno(&self, code: i32) {
        self.last_sock_errno.store(code, Ordering::Relaxed);
    }

    /// Clear the last socket error code (call on success).
    pub fn clear_errno(&self) {
        self.last_sock_errno.store(0, Ordering::Relaxed);
    }
}

// ── Free-standing helpers ─────────────────────────────────────────────────────

/// Convert a Linux `errno` value to the corresponding OS/2 SOCE* code.
///
/// Many low values (≤ 34) are identical between Linux and OS/2.  The BSD
/// `AF_INET`, the only address family guests get, and the size of its
/// `sockaddr_in` (OS/2's is laid out as Linux's: family, port, address,
/// eight bytes of zero).
pub const AF_INET: u32 = 2;
pub const SOCKADDR_IN_LEN: usize = 16;

/// Whether a guest may open a socket of `domain` and `sock_type`: only
/// Internet sockets (`AF_INET`) of type stream, datagram or raw, without
/// Linux's flag bits. Unix-domain sockets would reach the host's own
/// (D-Bus, X11, Warpine's desktop and pipe sockets) and create files
/// outside the drives; netlink and the rest are the host's business.
pub fn check_socket_kind(domain: u32, sock_type: u32) -> Result<(), i32> {
    if domain != AF_INET { return Err(SOCE_AFNOSUPPORT); }
    match sock_type {
        1..=3 => Ok(()), // SOCK_STREAM, SOCK_DGRAM, SOCK_RAW
        _ => Err(SOCE_SOCKTNOSUPPORT),
    }
}

/// Whether `bytes` (a `sockaddr_in`) names an `AF_INET` address.
pub fn check_sockaddr_in(bytes: &[u8]) -> Result<(), i32> {
    let family = bytes.get(..2).map_or(0, |f| u16::from_le_bytes([f[0], f[1]]));
    if bytes.len() != SOCKADDR_IN_LEN || u32::from(family) != AF_INET { return Err(SOCE_AFNOSUPPORT); }
    Ok(())
}

/// extension codes (EWOULDBLOCK and above) diverge and must be remapped.
pub fn errno_to_soce(errno: i32) -> i32 {
    match errno {
        libc::EINTR          => SOCE_INTR,
        libc::EBADF          => SOCE_BADF,
        libc::EACCES         => SOCE_ACCES,
        libc::EFAULT         => SOCE_FAULT,
        libc::EINVAL         => SOCE_INVAL,
        libc::EMFILE         => SOCE_MFILE,
        libc::EAGAIN         => SOCE_WOULDBLOCK, // EWOULDBLOCK == EAGAIN on Linux
        libc::EINPROGRESS    => SOCE_INPROGRESS,
        libc::EALREADY       => SOCE_ALREADY,
        libc::ENOTSOCK       => SOCE_NOTSOCK,
        libc::EDESTADDRREQ   => SOCE_DESTADDRREQ,
        libc::EMSGSIZE       => SOCE_MSGSIZE,
        libc::EPROTOTYPE     => SOCE_PROTOTYPE,
        libc::ENOPROTOOPT    => SOCE_NOPROTOOPT,
        libc::EPROTONOSUPPORT => SOCE_PROTONOSUPPORT,
        libc::ESOCKTNOSUPPORT => SOCE_SOCKTNOSUPPORT,
        libc::EOPNOTSUPP     => SOCE_OPNOTSUPP,
        libc::EAFNOSUPPORT   => SOCE_AFNOSUPPORT,
        libc::EADDRINUSE     => SOCE_ADDRINUSE,
        libc::EADDRNOTAVAIL  => SOCE_ADDRNOTAVAIL,
        libc::ENETDOWN       => SOCE_NETDOWN,
        libc::ENETUNREACH    => SOCE_NETUNREACH,
        libc::ECONNABORTED   => SOCE_CONNABORTED,
        libc::ECONNRESET     => SOCE_CONNRESET,
        libc::ENOBUFS        => SOCE_NOBUFS,
        libc::EISCONN        => SOCE_ISCONN,
        libc::ENOTCONN       => SOCE_NOTCONN,
        libc::ETIMEDOUT      => SOCE_TIMEDOUT,
        libc::ECONNREFUSED   => SOCE_CONNREFUSED,
        libc::EHOSTDOWN      => SOCE_HOSTDOWN,
        libc::EHOSTUNREACH   => SOCE_HOSTUNREACH,
        // Low values that are identical between Linux and OS/2.
        n if n > 0 && n < 35 => n,
        _ => SOCE_INVAL,
    }
}

/// Read the last Linux errno (after a failed syscall).
fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(SOCE_INVAL)
}

// ── API implementations ───────────────────────────────────────────────────────

impl super::Loader {
    // ── Internal socket helpers ───────────────────────────────────────────────

    /// Look up the Linux fd for OS/2 handle `s`.
    ///
    /// Returns `Some(fd)` on success. On failure, stores `SOCE_BADF` in
    /// `socket_mgr.last_sock_errno` and returns `None`.
    ///
    /// # Deadlock avoidance
    /// The MutexGuard from `lock_or_recover()` used for the lookup is **dropped
    /// before this function returns**, so callers are free to call
    /// `lock_or_recover()` again with no risk of self-deadlock.
    fn so_fd(&self, s: u32) -> Option<i32> {
        let fd_opt = self.shared.socket_mgr.lock_or_recover().lookup(s);
        // ^^^ MutexGuard dropped at the semicolon (Option<i32> is an owned value).
        if fd_opt.is_none() {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_BADF);
        }
        fd_opt
    }


    // ── sock_init (ordinal 22) ────────────────────────────────────────────────
    //
    //   int sock_init(void)
    //
    //   No-op initialisation — Warpine does not need special socket setup.
    //   Returns 0 on success.

    pub fn so_sock_init(&self) -> u32 {
        debug!("sock_init()");
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        SOCK_SUCCESS
    }

    // ── sock_errno (ordinal 18) ───────────────────────────────────────────────
    //
    //   int sock_errno(void)
    //
    //   Return the error code from the last failed socket call.

    pub fn so_sock_errno(&self) -> u32 {
        let code = self.shared.socket_mgr.lock_or_recover().last_sock_errno.load(Ordering::Relaxed);
        debug!("sock_errno() → {}", code);
        code as u32
    }

    // ── psock_errno (ordinal 21) ──────────────────────────────────────────────
    //
    //   void psock_errno(const char *msg)
    //
    //   Print a socket error message to VIO (stub: log only).

    pub fn so_psock_errno(&self, msg_ptr: u32) -> u32 {
        let msg = self.read_guest_string(msg_ptr);
        let code = self.shared.socket_mgr.lock_or_recover().last_sock_errno.load(Ordering::Relaxed);
        debug!("psock_errno(\"{}\") errno={}", msg, code);
        SOCK_SUCCESS
    }

    // ── socket (ordinal 19) ───────────────────────────────────────────────────
    //
    //   int socket(int domain, int type, int protocol)
    //
    //   args[0] = domain    (AF_INET=2, AF_UNIX=1, …)
    //   args[1] = type      (SOCK_STREAM=1, SOCK_DGRAM=2, SOCK_RAW=3)
    //   args[2] = protocol  (0 = auto, IPPROTO_TCP=6, IPPROTO_UDP=17, …)
    //
    //   Returns the OS/2 socket handle (≥ 0xA000) on success, SOCK_ERROR on failure.

    pub fn so_socket(&self, domain: u32, sock_type: u32, protocol: u32) -> u32 {
        debug!("socket(domain={}, type={}, protocol={})", domain, sock_type, protocol);
        // Internet sockets only: a guest must not reach the host's Unix
        // sockets (D-Bus, X11, Warpine's own desktop and pipe sockets) or
        // netlink, nor pass Linux socket flags.
        if let Err(e) = check_socket_kind(domain, sock_type) {
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }
        // SAFETY: domain/type/protocol are valid i32 values passed from guest args.
        // Close-on-exec: a child process (DosExecPgm) must not keep the
        // socket open.
        let fd = unsafe { libc::socket(domain as i32, sock_type as i32 | libc::SOCK_CLOEXEC, protocol as i32) };
        if fd < 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            debug!("  → error {}", e);
            return SOCK_ERROR;
        }
        let h = {
            let mut mgr = self.shared.socket_mgr.lock_or_recover();
            mgr.clear_errno();
            mgr.alloc(fd)
        };
        debug!("  → handle 0x{:X} (fd {})", h, fd);
        h
    }

    // ── soclose / closesocket (ordinal 20) ────────────────────────────────────
    //
    //   int soclose(int s)
    //
    //   args[0] = s — OS/2 socket handle

    pub fn so_close(&self, s: u32) -> u32 {
        debug!("soclose(s=0x{:X})", s);
        let fd_opt = self.shared.socket_mgr.lock_or_recover().remove(s);
        let fd = match fd_opt {
            Some(fd) => fd,
            None => {
                self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_BADF);
                return SOCK_ERROR;
            }
        };
        let rc = unsafe { libc::close(fd) };
        if rc != 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        SOCK_SUCCESS
    }

    /// Read a `sockaddr_in` of `len` bytes at guest `ptr` for `bind`,
    /// `connect` or `sendto`: exactly `SOCKADDR_IN_LEN` bytes of an
    /// `AF_INET` address (see [`check_sockaddr_in`]).
    fn read_sockaddr_in(&self, ptr: u32, len: u32) -> Result<Vec<u8>, i32> {
        if (len as usize) < SOCKADDR_IN_LEN { return Err(SOCE_INVAL); }
        let bytes = self.read_guest_bytes(ptr, SOCKADDR_IN_LEN);
        if bytes.len() < SOCKADDR_IN_LEN { return Err(SOCE_FAULT); }
        check_sockaddr_in(&bytes)?;
        Ok(bytes)
    }

    // ── bind (ordinal 2) ─────────────────────────────────────────────────────
    //
    //   int bind(int s, struct sockaddr *name, int namelen)
    //
    //   args[0] = s        — OS/2 socket handle
    //   args[1] = name     — ptr to sockaddr in guest memory
    //   args[2] = namelen  — sizeof(*name)

    pub fn so_bind(&self, s: u32, name_ptr: u32, namelen: u32) -> u32 {
        debug!("bind(s=0x{:X}, name=0x{:X}, namelen={})", s, name_ptr, namelen);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let addr_bytes = match self.read_sockaddr_in(name_ptr, namelen) {
            Ok(a) => a,
            Err(e) => {
                self.shared.socket_mgr.lock_or_recover().set_errno(e);
                return SOCK_ERROR;
            }
        };
        // SAFETY: addr_bytes is a host buffer of SOCKADDR_IN_LEN bytes, its
        // own length passed.
        let rc = unsafe {
            libc::bind(fd,
                addr_bytes.as_ptr() as *const libc::sockaddr,
                addr_bytes.len() as libc::socklen_t)
        };
        self.translate_rc(rc, SOCK_SUCCESS)
    }

    // ── connect (ordinal 3) ───────────────────────────────────────────────────
    //
    //   int connect(int s, struct sockaddr *name, int namelen)

    pub fn so_connect(&self, s: u32, name_ptr: u32, namelen: u32) -> u32 {
        debug!("connect(s=0x{:X}, name=0x{:X}, namelen={})", s, name_ptr, namelen);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let addr_bytes = match self.read_sockaddr_in(name_ptr, namelen) {
            Ok(a) => a,
            Err(e) => {
                self.shared.socket_mgr.lock_or_recover().set_errno(e);
                return SOCK_ERROR;
            }
        };
        let addr_len = addr_bytes.len() as libc::socklen_t;
        if !self.so_waits_here(fd, libc::POLLOUT, 0) {
            // SAFETY: `addr_bytes` is a host buffer of `addr_len` bytes.
            let rc = unsafe {
                libc::connect(fd, addr_bytes.as_ptr() as *const libc::sockaddr, addr_len)
            };
            return self.translate_rc(rc, SOCK_SUCCESS);
        }
        // A blocking connect is made without blocking and waited for in
        // slices, so a signal stops it (SOCEINTR, as BSD's EINTR: the
        // connection goes on in the background).
        // SAFETY: F_GETFL / F_SETFL on a descriptor the socket table owns.
        let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        unsafe { libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK) };
        // SAFETY: `addr_bytes` is a host buffer of `addr_len` bytes.
        let rc = unsafe {
            libc::connect(fd, addr_bytes.as_ptr() as *const libc::sockaddr, addr_len)
        };
        let err = if rc == 0 { 0 } else { last_errno() };
        let result = match err {
            0 => Ok(()),
            libc::EINPROGRESS => self.so_poll(fd, libc::POLLOUT).and_then(|()| {
                let mut e: libc::c_int = 0;
                let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
                // SAFETY: `e` and `len` are live locals of the sizes passed.
                unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR, &mut e as *mut _ as *mut libc::c_void, &mut len) };
                if e == 0 { Ok(()) } else { Err(errno_to_soce(e)) }
            }),
            e => Err(errno_to_soce(e)),
        };
        // SAFETY: restores the flags read above.
        unsafe { libc::fcntl(fd, libc::F_SETFL, fl) };
        match result {
            Ok(()) => { self.shared.socket_mgr.lock_or_recover().clear_errno(); SOCK_SUCCESS }
            Err(e) => self.so_fail(e),
        }
    }

    // ── listen (ordinal 10) ───────────────────────────────────────────────────
    //
    //   int listen(int s, int backlog)

    pub fn so_listen(&self, s: u32, backlog: u32) -> u32 {
        debug!("listen(s=0x{:X}, backlog={})", s, backlog);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let rc = unsafe { libc::listen(fd, backlog as i32) };
        self.translate_rc(rc, SOCK_SUCCESS)
    }

    // ── accept (ordinal 1) ────────────────────────────────────────────────────
    //
    //   int accept(int s, struct sockaddr *addr, int *addrlen)
    //
    //   args[0] = s        — listening socket handle
    //   args[1] = addr     — ptr to sockaddr buffer in guest, or 0
    //   args[2] = addrlen  — ptr to u32 length in guest, or 0
    //
    //   Returns new OS/2 socket handle on success, SOCK_ERROR on failure.

    /// Wait until blocking socket `fd` is ready for `events` (`POLLIN` /
    /// `POLLOUT`), in slices, so a signal, a kill or the process end stops
    /// the call (`SOCEINTR`, ADR-0035). A socket that does not block
    /// (`O_NONBLOCK`, `MSG_DONTWAIT` in `flags`) or has its own timeout
    /// (`SO_RCVTIMEO` / `SO_SNDTIMEO`) is left to the call itself.
    fn so_wait(&self, fd: i32, events: i16, flags: u32) -> Result<(), i32> {
        if !self.so_waits_here(fd, events, flags) { return Ok(()); }
        self.so_poll(fd, events)
    }

    /// Whether a call on socket `fd` waiting for `events` would block
    /// without a time limit: a blocking socket without its own timeout,
    /// and no `MSG_DONTWAIT` in `flags`. Such waits are made here, in slices.
    fn so_waits_here(&self, fd: i32, events: i16, flags: u32) -> bool {
        if flags & libc::MSG_DONTWAIT as u32 != 0 { return false; }
        // SAFETY: F_GETFL on a descriptor the socket table owns; no memory.
        let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if fl < 0 || fl & libc::O_NONBLOCK != 0 { return false; }
        let opt = if events == libc::POLLOUT { libc::SO_SNDTIMEO } else { libc::SO_RCVTIMEO };
        let mut tv: libc::timeval = libc::timeval { tv_sec: 0, tv_usec: 0 };
        let mut len = std::mem::size_of::<libc::timeval>() as libc::socklen_t;
        // SAFETY: `tv` and `len` are live locals of the sizes passed.
        let got = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, opt, &mut tv as *mut _ as *mut libc::c_void, &mut len) };
        !(got == 0 && (tv.tv_sec != 0 || tv.tv_usec != 0))
    }

    /// Wait in slices until `fd` is ready for `events`; `SOCEINTR` when a
    /// signal, a kill or the process end stops the call.
    fn so_poll(&self, fd: i32, events: i16) -> Result<(), i32> {
        loop {
            let mut p = libc::pollfd { fd, events, revents: 0 };
            // SAFETY: one live pollfd for the duration of the call.
            let n = unsafe { libc::poll(&mut p, 1, SELECT_POLL_SLICE.as_millis() as i32) };
            if n > 0 || (n < 0 && last_errno() != libc::EINTR) {
                // Ready, or an error the call itself will report.
                return Ok(());
            }
            if self.interrupted() || self.shutting_down() {
                return Err(SOCE_INTR);
            }
        }
    }

    /// Record socket error `e` and fail.
    fn so_fail(&self, e: i32) -> u32 {
        self.shared.socket_mgr.lock_or_recover().set_errno(e);
        SOCK_ERROR
    }

    pub fn so_accept(&self, s: u32, addr_ptr: u32, addrlen_ptr: u32) -> u32 {
        debug!("accept(s=0x{:X}, addr=0x{:X}, addrlen_ptr=0x{:X})", s, addr_ptr, addrlen_ptr);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        if let Err(e) = self.so_wait(fd, libc::POLLIN, 0) { return self.so_fail(e); }

        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

        // SAFETY: storage is stack-allocated; len is bounded by sockaddr_storage size.
        let new_fd = unsafe {
            libc::accept4(fd, &mut storage as *mut _ as *mut libc::sockaddr, &mut len, libc::SOCK_CLOEXEC)
        };
        if new_fd < 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }

        // Write back address if caller provided a buffer.
        if addr_ptr != 0 && addrlen_ptr != 0 {
            let guest_len = self.guest_read::<u32>(addrlen_ptr).unwrap_or(0) as usize;
            let copy_len = (len as usize).min(guest_len);
            // SAFETY: storage is valid stack memory; copy_len is bounded by min(len, guest_len).
            let addr_bytes = unsafe {
                std::slice::from_raw_parts(&storage as *const _ as *const u8, copy_len)
            };
            self.guest_write_bytes(addr_ptr, addr_bytes);
            let _ = self.guest_write::<u32>(addrlen_ptr, len as u32);
        }

        let new_h = {
            let mut mgr = self.shared.socket_mgr.lock_or_recover();
            mgr.clear_errno();
            mgr.alloc(new_fd)
        };
        debug!("  → new handle 0x{:X} (fd {})", new_h, new_fd);
        new_h
    }

    // ── send (ordinal 14) ─────────────────────────────────────────────────────
    //
    //   int send(int s, const void *msg, int len, int flags)

    pub fn so_send(&self, s: u32, buf_ptr: u32, len: u32, flags: u32) -> u32 {
        debug!("send(s=0x{:X}, len={}, flags={})", s, len, flags);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let data = self.read_guest_bytes(buf_ptr, len as usize);
        if self.so_waits_here(fd, libc::POLLOUT, flags) {
            return self.so_send_in_slices(fd, &data, flags);
        }
        // SAFETY: `data` is a host buffer whose own length is passed.
        let sent = unsafe {
            libc::send(fd, data.as_ptr() as *const libc::c_void, data.len(), flags as i32)
        };
        if sent < 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        sent as u32
    }

    /// A blocking `send` of `data`: the part that fits is sent without
    /// blocking, then the rest after waiting for room, in slices. A signal
    /// stops it: the bytes sent so far, or `SOCEINTR` when none were.
    fn so_send_in_slices(&self, fd: i32, data: &[u8], flags: u32) -> u32 {
        let mut done = 0usize;
        loop {
            if let Err(e) = self.so_poll(fd, libc::POLLOUT) {
                if done > 0 { break; }
                return self.so_fail(e);
            }
            let rest = &data[done..];
            // SAFETY: `rest` is a host buffer whose own length is passed.
            let n = unsafe {
                libc::send(fd, rest.as_ptr() as *const libc::c_void, rest.len(), flags as i32 | libc::MSG_DONTWAIT)
            };
            if n < 0 {
                let e = last_errno();
                if e == libc::EAGAIN || e == libc::EWOULDBLOCK || e == libc::EINTR { continue; }
                if done > 0 { break; }
                return self.so_fail(errno_to_soce(e));
            }
            done += n as usize;
            if done >= data.len() { break; }
        }
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        done as u32
    }

    // ── sendto (ordinal 15) ───────────────────────────────────────────────────
    //
    //   int sendto(int s, const void *msg, int len, int flags,
    //              const struct sockaddr *to, int tolen)

    pub fn so_sendto(&self, s: u32, buf_ptr: u32, len: u32, flags: u32,
                     to_ptr: u32, tolen: u32) -> u32 {
        debug!("sendto(s=0x{:X}, len={}, flags={}, tolen={})", s, len, flags, tolen);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        if let Err(e) = self.so_wait(fd, libc::POLLOUT, flags) { return self.so_fail(e); }
        let data = self.read_guest_bytes(buf_ptr, len as usize);
        let sent = if to_ptr != 0 && tolen > 0 {
            let addr = match self.read_sockaddr_in(to_ptr, tolen) {
                Ok(a) => a,
                Err(e) => {
                    self.shared.socket_mgr.lock_or_recover().set_errno(e);
                    return SOCK_ERROR;
                }
            };
            // SAFETY: `data` and `addr` are host buffers whose own lengths
            // are passed, so the host never reads past them.
            unsafe {
                libc::sendto(fd, data.as_ptr() as *const libc::c_void, data.len(),
                    flags as i32,
                    addr.as_ptr() as *const libc::sockaddr,
                    addr.len() as libc::socklen_t)
            }
        } else {
            unsafe {
                libc::send(fd, data.as_ptr() as *const libc::c_void, data.len(), flags as i32)
            }
        };
        if sent < 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        sent as u32
    }

    // ── recv (ordinal 11) ───────────────────────────────────────────────────── ─────────────────────────────────────────────────────
    //
    //   int recv(int s, void *buf, int len, int flags)

    pub fn so_recv(&self, s: u32, buf_ptr: u32, len: u32, flags: u32) -> u32 {
        debug!("recv(s=0x{:X}, len={}, flags={})", s, len, flags);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let Some(buf) = self.guest_slice_mut(buf_ptr, len as usize) else {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_FAULT);
            return SOCK_ERROR;
        };
        if let Err(e) = self.so_wait(fd, libc::POLLIN, flags) { return self.so_fail(e); }
        let received = unsafe {
            libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, len as usize, flags as i32)
        };
        if received < 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        received as u32
    }

    // ── recvfrom (ordinal 12) ─────────────────────────────────────────────────
    //
    //   int recvfrom(int s, void *buf, int len, int flags,
    //                struct sockaddr *from, int *fromlen)

    pub fn so_recvfrom(&self, s: u32, buf_ptr: u32, len: u32, flags: u32,
                       from_ptr: u32, fromlen_ptr: u32) -> u32 {
        debug!("recvfrom(s=0x{:X}, len={}, flags={})", s, len, flags);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let Some(buf) = self.guest_slice_mut(buf_ptr, len as usize) else {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_FAULT);
            return SOCK_ERROR;
        };

        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut from_len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

        if let Err(e) = self.so_wait(fd, libc::POLLIN, flags) { return self.so_fail(e); }
        let received = unsafe {
            libc::recvfrom(fd, buf.as_mut_ptr() as *mut libc::c_void, len as usize,
                flags as i32,
                &mut storage as *mut _ as *mut libc::sockaddr,
                &mut from_len)
        };
        if received < 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }
        if from_ptr != 0 && fromlen_ptr != 0 {
            let guest_len = self.guest_read::<u32>(fromlen_ptr).unwrap_or(0) as usize;
            let copy_len = (from_len as usize).min(guest_len);
            // SAFETY: storage is valid stack memory; copy_len is bounded by min(len, guest_len).
            let addr_bytes = unsafe {
                std::slice::from_raw_parts(&storage as *const _ as *const u8, copy_len)
            };
            self.guest_write_bytes(from_ptr, addr_bytes);
            let _ = self.guest_write::<u32>(fromlen_ptr, from_len as u32);
        }
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        received as u32
    }

    // ── shutdown (ordinal 17) ─────────────────────────────────────────────────
    //
    //   int shutdown(int s, int how)
    //
    //   how: 0=recv, 1=send, 2=both (same values on OS/2 and Linux).

    pub fn so_shutdown(&self, s: u32, how: u32) -> u32 {
        debug!("shutdown(s=0x{:X}, how={})", s, how);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let rc = unsafe { libc::shutdown(fd, how as i32) };
        self.translate_rc(rc, SOCK_SUCCESS)
    }

    // ── getsockname (ordinal 7) ───────────────────────────────────────────────
    //
    //   int getsockname(int s, struct sockaddr *name, int *namelen)

    pub fn so_getsockname(&self, s: u32, name_ptr: u32, namelen_ptr: u32) -> u32 {
        debug!("getsockname(s=0x{:X})", s);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockname(fd, &mut storage as *mut _ as *mut libc::sockaddr, &mut len)
        };
        if rc != 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }
        let guest_len = self.guest_read::<u32>(namelen_ptr).unwrap_or(0) as usize;
        let copy_len = (len as usize).min(guest_len);
        // SAFETY: storage is valid stack memory; copy_len is bounded by min(len, guest_len).
        let addr_bytes = unsafe {
            std::slice::from_raw_parts(&storage as *const _ as *const u8, copy_len)
        };
        self.guest_write_bytes(name_ptr, addr_bytes);
        let _ = self.guest_write::<u32>(namelen_ptr, len as u32);
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        SOCK_SUCCESS
    }

    // ── getpeername (ordinal 6) ───────────────────────────────────────────────
    //
    //   int getpeername(int s, struct sockaddr *name, int *namelen)

    pub fn so_getpeername(&self, s: u32, name_ptr: u32, namelen_ptr: u32) -> u32 {
        debug!("getpeername(s=0x{:X})", s);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let rc = unsafe {
            libc::getpeername(fd, &mut storage as *mut _ as *mut libc::sockaddr, &mut len)
        };
        if rc != 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }
        let guest_len = self.guest_read::<u32>(namelen_ptr).unwrap_or(0) as usize;
        let copy_len = (len as usize).min(guest_len);
        // SAFETY: storage is valid stack memory; copy_len is bounded by min(len, guest_len).
        let addr_bytes = unsafe {
            std::slice::from_raw_parts(&storage as *const _ as *const u8, copy_len)
        };
        self.guest_write_bytes(name_ptr, addr_bytes);
        let _ = self.guest_write::<u32>(namelen_ptr, len as u32);
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        SOCK_SUCCESS
    }

    // ── setsockopt (ordinal 16) ───────────────────────────────────────────────
    //
    //   int setsockopt(int s, int level, int optname, const void *optval, int optlen)

    pub fn so_setsockopt(&self, s: u32, level: u32, optname: u32,
                         optval_ptr: u32, optlen: u32) -> u32 {
        debug!("setsockopt(s=0x{:X}, level={}, optname={}, optlen={})", s, level, optname, optlen);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let Some((level, optname)) = linux_sockopt(level, optname) else {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_NOPROTOOPT);
            return SOCK_ERROR;
        };
        if optlen > MAX_SOCKOPT_LEN {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_INVAL);
            return SOCK_ERROR;
        }
        let optval = self.read_guest_bytes(optval_ptr, optlen as usize);
        // SAFETY: `optval` is a host buffer of exactly `optval.len()` bytes,
        // and that length (≤ MAX_SOCKOPT_LEN) is what is passed.
        let rc = unsafe {
            libc::setsockopt(fd, level, optname,
                optval.as_ptr() as *const libc::c_void,
                optval.len() as libc::socklen_t)
        };
        self.translate_rc(rc, SOCK_SUCCESS)
    }

    // ── getsockopt (ordinal 8) ────────────────────────────────────────────────
    //
    //   int getsockopt(int s, int level, int optname, void *optval, int *optlen)

    pub fn so_getsockopt(&self, s: u32, level: u32, optname: u32,
                         optval_ptr: u32, optlen_ptr: u32) -> u32 {
        debug!("getsockopt(s=0x{:X}, level={}, optname={})", s, level, optname);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let Some((level, optname)) = linux_sockopt(level, optname) else {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_NOPROTOOPT);
            return SOCK_ERROR;
        };
        let buf_len = self.guest_read::<u32>(optlen_ptr).unwrap_or(0);
        let Some(buf) = self.guest_slice_mut(optval_ptr, buf_len as usize) else {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_FAULT);
            return SOCK_ERROR;
        };
        let mut actual_len = buf_len as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(fd, level, optname,
                buf.as_mut_ptr() as *mut libc::c_void,
                &mut actual_len)
        };
        if rc != 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }
        let _ = self.guest_write::<u32>(optlen_ptr, actual_len as u32);
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        SOCK_SUCCESS
    }

    // ── select (ordinal 13) ───────────────────────────────────────────────────
    //
    // select(int nfds, struct fd_set *readfds, struct fd_set *writefds,
    //        struct fd_set *exceptfds, struct timeval *timeout)
    //
    //   The OS/2 TCP/IP `fd_set` is a count and an array of socket numbers
    //   (Open Watcom `h/os2/types.h`: `u_short fd_count; int
    //   fd_array[FD_SETSIZE]` packed to 4, FD_SETSIZE 64), not a BSD bit
    //   mask — see `read_fd_list`. `nfds` is not needed and ignored. The
    //   sockets are watched with poll(2), in slices so that a signal can
    //   interrupt the wait (`SOCEINTR`, ADR-0035); on return each given set
    //   holds the ready sockets in their original order, and the result is
    //   their total count (0 = timed out).
    //
    //   Unverified: the SO32DLL / TCP32DLL ordinal of `select` (13 here) and
    //   whether the BSD 4.3 stack's `select` takes a bit mask instead
    //   (`bsdselect` / `os2_select` are not provided).
    pub fn so_select(&self, nfds: u32, rfds_ptr: u32, wfds_ptr: u32,
                     efds_ptr: u32, timeout_ptr: u32) -> u32 {
        debug!("select(nfds={}, rfds=0x{:X}, wfds=0x{:X}, efds=0x{:X}, timeout=0x{:X})",
               nfds, rfds_ptr, wfds_ptr, efds_ptr, timeout_ptr);
        match self.select_sockets([rfds_ptr, wfds_ptr, efds_ptr], timeout_ptr) {
            Ok(n) => {
                self.shared.socket_mgr.lock_or_recover().clear_errno();
                n
            }
            Err(e) => {
                self.shared.socket_mgr.lock_or_recover().set_errno(e);
                SOCK_ERROR
            }
        }
    }

    /// The body of `select`: the ready count, or a `SOCE*` error.
    fn select_sockets(&self, set_ptrs: [u32; 3], timeout_ptr: u32) -> Result<u32, i32> {
        // The three lists (read, write, exception); `None` for a null set.
        let mut lists: [Option<Vec<u32>>; 3] = [None, None, None];
        for (list, &ptr) in lists.iter_mut().zip(&set_ptrs) {
            if ptr != 0 {
                let bytes = self.guest_read_bytes(ptr, OS2_FD_SET_BYTES).ok_or(SOCE_FAULT)?;
                *list = Some(read_fd_list(&bytes).ok_or(SOCE_INVAL)?);
            }
        }
        let timeout = if timeout_ptr == 0 {
            None
        } else {
            let sec = self.guest_read::<i32>(timeout_ptr).ok_or(SOCE_FAULT)?;
            let usec = self.guest_read::<i32>(timeout_ptr + 4).ok_or(SOCE_FAULT)?;
            if sec < 0 || !(0..1_000_000).contains(&usec) {
                return Err(SOCE_INVAL);
            }
            Some(std::time::Duration::from_secs(sec as u64) + std::time::Duration::from_micros(usec as u64))
        };

        // One pollfd per socket and kind; every number must be an open socket.
        const EVENTS: [libc::c_short; 3] = [libc::POLLIN, libc::POLLOUT, libc::POLLPRI];
        let mut polls: Vec<libc::pollfd> = Vec::new();
        {
            let mgr = self.shared.socket_mgr.lock_or_recover();
            for (list, events) in lists.iter().zip(EVENTS) {
                for &s in list.iter().flatten() {
                    let fd = *mgr.sockets.get(&s).ok_or(SOCE_NOTSOCK)?;
                    polls.push(libc::pollfd { fd, events, revents: 0 });
                }
            }
        }

        let deadline = timeout.map(|t| std::time::Instant::now() + t);
        loop {
            let slice = match deadline {
                Some(d) => d.saturating_duration_since(std::time::Instant::now()).min(SELECT_POLL_SLICE),
                None => SELECT_POLL_SLICE,
            };
            // SAFETY: `polls` is a live, initialised array of `polls.len()`
            // pollfds for the duration of the call.
            let n = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, slice.as_millis() as i32) };
            if n < 0 {
                let e = last_errno();
                if e != libc::EINTR {
                    return Err(errno_to_soce(e));
                }
            } else if n > 0 {
                break;
            }
            if self.interrupted() {
                return Err(SOCE_INTR);
            }
            if self.shutting_down() || deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                // Timed out (or the process ends): nothing is ready.
                for p in &mut polls { p.revents = 0; }
                break;
            }
        }

        // Write back each set with only its ready sockets.
        let mut ready_total = 0u32;
        let mut polled = polls.iter();
        for (list, &ptr) in lists.iter().zip(&set_ptrs) {
            let Some(list) = list else { continue };
            let ready: Vec<u32> = list.iter().copied()
                .filter(|_| polled.next().is_some_and(select_ready))
                .collect();
            ready_total += ready.len() as u32;
            self.guest_write_bytes(ptr, &write_fd_list(&ready)).ok_or(SOCE_FAULT)?;
        }
        Ok(ready_total)
    }

    // ── gethostid (ordinal 4) ─────────────────────────────────────────────────
    //
    //   u_long gethostid(void)
    //
    //   Returns the 32-bit host identifier (loopback 127.0.0.1 = 0x0100007F as fallback).

    pub fn so_gethostid(&self) -> u32 {
        debug!("gethostid()");
        let id = unsafe { libc::gethostid() };
        self.shared.socket_mgr.lock_or_recover().clear_errno();
        id as u32
    }

    // ── gethostname (ordinal 5) ───────────────────────────────────────────────
    //
    //   int gethostname(char *name, int namelen)

    pub fn so_gethostname(&self, name_ptr: u32, namelen: u32) -> u32 {
        debug!("gethostname(namelen={})", namelen);
        let Some(buf) = self.guest_slice_mut(name_ptr, namelen as usize) else {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_FAULT);
            return SOCK_ERROR;
        };
        let rc = unsafe {
            libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, namelen as libc::size_t)
        };
        self.translate_rc(rc, SOCK_SUCCESS)
    }

    // ── gethostbyname (ordinal 40) ────────────────────────────────────────────
    //
    //   struct hostent *gethostbyname(const char *name)
    //
    //   args[0] = name — ptr to null-terminated hostname in guest memory
    //
    //   Returns a guest pointer to a `hostent`-like structure, or 0 on failure.
    //
    //   Guest layout (simplified, addresses are 32-bit flat):
    //     +0x00  h_name      u32  ptr to host name string
    //     +0x04  h_aliases   u32  ptr to null-terminated alias list (just null ptr)
    //     +0x08  h_addrtype  i32  AF_INET = 2
    //     +0x0C  h_length    i32  4 (IPv4)
    //     +0x10  h_addr_list u32  ptr to null-terminated addr list
    //     -- followed by:
    //     alias_list[0..4]    (null ptr)
    //     addr_list[0..4]     (ptr to ipv4 addr)
    //     addr_list[4..8]     (null ptr)
    //     ipv4 addr [4 bytes]
    //     name string (null-terminated)

    pub fn so_gethostbyname(&self, name_ptr: u32) -> u32 {
        let hostname = self.read_guest_string(name_ptr);
        debug!("gethostbyname(\"{}\")", hostname);

        // Resolve via Rust's std::net (delegates to getaddrinfo on Linux).
        let addr = match format!("{}:0", hostname).to_socket_addrs() {
            Ok(mut addrs) => {
                addrs.find_map(|a| match a { std::net::SocketAddr::V4(v4) => Some(*v4.ip()), _ => None })
            }
            Err(e) => {
                warn!("gethostbyname(\"{}\") failed: {}", hostname, e);
                None
            }
        };

        let ip = match addr {
            Some(ip) => ip.octets(),
            None => {
                self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_HOSTUNREACH);
                return 0;
            }
        };

        self.write_hostent_to_scratch(&hostname, ip)
    }

    // ── gethostbyaddr (ordinal 41) ────────────────────────────────────────────
    //
    //   struct hostent *gethostbyaddr(const char *addr, int len, int type)

    pub fn so_gethostbyaddr(&self, addr_ptr: u32, len: u32, addr_type: u32) -> u32 {
        debug!("gethostbyaddr(addr=0x{:X}, len={}, type={})", addr_ptr, len, addr_type);
        // Only support AF_INET (type=2), 4-byte addresses.
        if addr_type != libc::AF_INET as u32 || len < 4 {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_AFNOSUPPORT);
            return 0;
        }
        let ip_bytes = self.read_guest_bytes(addr_ptr, 4);
        if ip_bytes.len() < 4 {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_FAULT);
            return 0;
        }
        let ip: [u8; 4] = [ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]];
        let hostname = format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        // Write the hostent directly using the resolved dotted-quad as the name.
        self.write_hostent_to_scratch(&hostname, ip)
    }

    // ── getservbyname (ordinal 42) ────────────────────────────────────────────
    //
    //   struct servent *getservbyname(const char *name, const char *proto)
    //
    //   args[0] = name  — service name (e.g. "http")
    //   args[1] = proto — protocol name (e.g. "tcp") or 0
    //
    //   Returns guest ptr to servent, or 0 on failure.
    //
    //   Guest servent layout (at scratch + 0x40):
    //     +0x00  s_name    u32  ptr to service name
    //     +0x04  s_aliases u32  ptr to null alias list
    //     +0x08  s_port    i32  port in network byte order
    //     +0x0C  s_proto   u32  ptr to protocol name
    //     -- followed by:
    //     alias_list [null u32]
    //     name  string (null-terminated)
    //     proto string (null-terminated)

    pub fn so_getservbyname(&self, name_ptr: u32, proto_ptr: u32) -> u32 {
        let svc_name  = self.read_guest_string(name_ptr);
        let proto_str = if proto_ptr != 0 { self.read_guest_string(proto_ptr) } else { String::new() };
        debug!("getservbyname(\"{}\", \"{}\")", svc_name, proto_str);

        // Use libc's getservbyname (thread-unsafe but acceptable for single-threaded guest callers).
        let c_name  = std::ffi::CString::new(svc_name.as_str()).unwrap_or_default();
        let c_proto_owned = if proto_str.is_empty() {
            None
        } else {
            Some(std::ffi::CString::new(proto_str.as_str()).unwrap_or_default())
        };
        let c_proto = c_proto_owned.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        let se = unsafe { libc::getservbyname(c_name.as_ptr(), c_proto) };
        if se.is_null() {
            warn!("getservbyname(\"{}\") not found", svc_name);
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_INVAL);
            return 0;
        }

        // SAFETY: se is a non-null pointer returned by libc::getservbyname.
        let port       = unsafe { (*se).s_port } as i32;
        let host_name  = unsafe { std::ffi::CStr::from_ptr((*se).s_name).to_string_lossy().to_string() };
        let host_proto = unsafe { std::ffi::CStr::from_ptr((*se).s_proto).to_string_lossy().to_string() };

        self.write_servent_to_scratch(&host_name, port, &host_proto)
    }

    // ── getservbyport (ordinal 43) ────────────────────────────────────────────
    //
    //   struct servent *getservbyport(int port, const char *proto)

    pub fn so_getservbyport(&self, port: u32, proto_ptr: u32) -> u32 {
        let proto_str = if proto_ptr != 0 { self.read_guest_string(proto_ptr) } else { String::new() };
        debug!("getservbyport(port={}, proto=\"{}\")", port, proto_str);

        let c_proto_owned = if proto_str.is_empty() {
            None
        } else {
            Some(std::ffi::CString::new(proto_str.as_str()).unwrap_or_default())
        };
        let c_proto = c_proto_owned.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        let se = unsafe { libc::getservbyport(port as i32, c_proto) };
        if se.is_null() {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_INVAL);
            return 0;
        }
        // SAFETY: se is a non-null pointer returned by libc::getservbyport.
        let host_name  = unsafe { std::ffi::CStr::from_ptr((*se).s_name).to_string_lossy().to_string() };
        let host_proto = unsafe { std::ffi::CStr::from_ptr((*se).s_proto).to_string_lossy().to_string() };
        let resolved_port = unsafe { (*se).s_port } as i32;
        self.write_servent_to_scratch(&host_name, resolved_port, &host_proto)
    }

    // ── getprotobyname (ordinal 44) ───────────────────────────────────────────
    //
    //   struct protoent *getprotobyname(const char *name)
    //
    //   Stub: returns 0 (not found). Used only by rare apps that need raw protocol info.

    pub fn so_getprotobyname(&self, _name_ptr: u32) -> u32 {
        debug!("[STUB] getprotobyname() → 0 (not implemented)");
        0
    }

    // ── getprotobynumber (ordinal 45) ─────────────────────────────────────────
    //
    //   struct protoent *getprotobynumber(int proto)
    //
    //   Stub: returns 0.

    pub fn so_getprotobynumber(&self, _proto: u32) -> u32 {
        debug!("[STUB] getprotobynumber() → 0 (not implemented)");
        0
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Write a resolved hostent into the scratch buffer and return its guest address.
    ///
    /// Layout within 256-byte scratch area:
    ///   +0x00  hostent struct (24 bytes)
    ///   +0x18  alias_list: [null u32]           (4 bytes)
    ///   +0x1C  addr_list:  [ptr u32, null u32]  (8 bytes)
    ///   +0x24  ipv4 addr                         (4 bytes)
    ///   +0x28  name string (null-terminated)
    fn write_hostent_to_scratch(&self, hostname: &str, ip: [u8; 4]) -> u32 {
        let scratch = self.ensure_hostent_scratch();
        if scratch == 0 {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_NOBUFS);
            return 0;
        }

        let hostent_base  = scratch;
        let alias_base    = scratch + 0x18;
        let addrlist_base = scratch + 0x1C;
        let ip_base       = scratch + 0x24;
        let name_base     = scratch + 0x28;

        // Write name string.
        let name_bytes: Vec<u8> = hostname.bytes().chain(std::iter::once(0)).collect();
        self.guest_write_bytes(name_base, &name_bytes);

        // Write IPv4 address bytes.
        self.guest_write_bytes(ip_base, &ip);

        // Write alias list: [null].
        let _ = self.guest_write::<u32>(alias_base, 0);

        // Write addr list: [&ip, null].
        let _ = self.guest_write::<u32>(addrlist_base,     ip_base);
        let _ = self.guest_write::<u32>(addrlist_base + 4, 0);

        // Write hostent struct.
        let _ = self.guest_write::<u32>(hostent_base,        name_base);    // h_name
        let _ = self.guest_write::<u32>(hostent_base + 0x04, alias_base);   // h_aliases
        let _ = self.guest_write::<i32>(hostent_base + 0x08, libc::AF_INET);// h_addrtype
        let _ = self.guest_write::<i32>(hostent_base + 0x0C, 4);            // h_length
        let _ = self.guest_write::<u32>(hostent_base + 0x10, addrlist_base);// h_addr_list

        self.shared.socket_mgr.lock_or_recover().clear_errno();
        debug!("  → hostent at 0x{:X}, ip={}.{}.{}.{}", hostent_base, ip[0], ip[1], ip[2], ip[3]);
        hostent_base
    }

    /// Write a resolved servent into the scratch buffer and return its guest address.
    ///
    /// Layout at scratch + 0x40 (avoids collision with hostent at scratch + 0x00):
    ///   +0x00  s_name    u32  ptr to service name
    ///   +0x04  s_aliases u32  ptr to null alias list
    ///   +0x08  s_port    i32  port in network byte order
    ///   +0x0C  s_proto   u32  ptr to protocol name
    fn write_servent_to_scratch(&self, svc_name: &str, port: i32, proto_name: &str) -> u32 {
        let scratch = self.ensure_hostent_scratch();
        if scratch == 0 {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_NOBUFS);
            return 0;
        }

        // Cap name length to fit within the 256-byte scratch layout (name at +0x58, proto after).
        let max_name_len = 256usize.saturating_sub(0x58 + 2); // leave 2 bytes for proto + NUL
        let host_name = if svc_name.len() > max_name_len {
            warn!("getservbyname: service name too long for scratch buffer; truncating");
            &svc_name[..max_name_len]
        } else {
            svc_name
        };

        let servent_base  = scratch + 0x40;
        let alias_base    = scratch + 0x54;
        let sname_base    = scratch + 0x58;
        let sproto_base   = sname_base + host_name.len() as u32 + 1;

        // Check that proto fits within the 256-byte scratch area.
        let scratch_end = scratch + 256;
        let max_proto_len = scratch_end.saturating_sub(sproto_base + 1) as usize;
        let host_proto = if proto_name.len() > max_proto_len {
            warn!("getservbyname: protocol name too long for scratch buffer; truncating");
            &proto_name[..max_proto_len]
        } else {
            proto_name
        };

        // Write strings.
        let name_bytes: Vec<u8> = host_name.bytes().chain(std::iter::once(0)).collect();
        self.guest_write_bytes(sname_base, &name_bytes);
        let proto_bytes: Vec<u8> = host_proto.bytes().chain(std::iter::once(0)).collect();
        self.guest_write_bytes(sproto_base, &proto_bytes);

        // Write alias list: [null].
        let _ = self.guest_write::<u32>(alias_base, 0);

        // Write servent struct.
        let _ = self.guest_write::<u32>(servent_base,        sname_base);  // s_name
        let _ = self.guest_write::<u32>(servent_base + 0x04, alias_base);  // s_aliases
        let _ = self.guest_write::<i32>(servent_base + 0x08, port);         // s_port
        let _ = self.guest_write::<u32>(servent_base + 0x0C, sproto_base); // s_proto

        self.shared.socket_mgr.lock_or_recover().clear_errno();
        debug!("  → servent at 0x{:X}, port={}", servent_base, i16::from_be(port as i16));
        servent_base
    }

    /// Read `len` bytes from guest memory at `addr`. Returns fewer bytes if OOB.
    ///
    /// The read is capped at `MAX_SOCKET_BUF` to prevent runaway allocations from
    /// untrusted guest length fields.
    fn read_guest_bytes(&self, addr: u32, len: usize) -> Vec<u8> {
        let capped = len.min(MAX_SOCKET_BUF);
        if let Some(slice) = self.guest_slice_mut(addr, capped) {
            slice.to_vec()
        } else {
            // guest_slice_mut failed (OOB or zero len); fall back to safe byte-by-byte.
            (0..capped)
                .map_while(|i| self.guest_read::<u8>(addr.checked_add(i as u32)?))
                .collect()
        }
    }

    /// Translate a Linux syscall return value (0 = ok, -1 = error) into an OS/2 result.
    fn translate_rc(&self, rc: i32, ok_val: u32) -> u32 {
        if rc != 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            SOCK_ERROR
        } else {
            self.shared.socket_mgr.lock_or_recover().clear_errno();
            ok_val
        }
    }

    /// Ensure the hostent/servent scratch buffer is allocated in guest memory.
    /// Returns the guest address of the 256-byte scratch area, or 0 on OOM.
    fn ensure_hostent_scratch(&self) -> u32 {
        let existing = self.shared.socket_mgr.lock_or_recover().scratch_addr;
        if let Some(addr) = existing { return addr; }
        match self.shared.mem_mgr.lock_or_recover().alloc(256) {
            Some(addr) => {
                self.shared.socket_mgr.lock_or_recover().scratch_addr = Some(addr);
                addr
            }
            None => {
                warn!("ensure_hostent_scratch: out of guest memory");
                0
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Loader;

    // ── errno_to_soce ─────────────────────────────────────────────────────────

    #[test]
    fn test_errno_to_soce_identical_low_values() {
        // Low errno values (≤ 34) that are the same on Linux and OS/2.
        assert_eq!(errno_to_soce(libc::EINTR),  SOCE_INTR);
        assert_eq!(errno_to_soce(libc::EBADF),  SOCE_BADF);
        assert_eq!(errno_to_soce(libc::EACCES), SOCE_ACCES);
        assert_eq!(errno_to_soce(libc::EFAULT), SOCE_FAULT);
        assert_eq!(errno_to_soce(libc::EINVAL), SOCE_INVAL);
        assert_eq!(errno_to_soce(libc::EMFILE), SOCE_MFILE);
    }

    #[test]
    fn test_errno_to_soce_bsd_extensions() {
        // BSD extension codes that diverge from Linux.
        assert_eq!(errno_to_soce(libc::EAGAIN),          SOCE_WOULDBLOCK);
        assert_eq!(errno_to_soce(libc::EINPROGRESS),     SOCE_INPROGRESS);
        assert_eq!(errno_to_soce(libc::EALREADY),        SOCE_ALREADY);
        assert_eq!(errno_to_soce(libc::ENOTSOCK),        SOCE_NOTSOCK);
        assert_eq!(errno_to_soce(libc::EDESTADDRREQ),    SOCE_DESTADDRREQ);
        assert_eq!(errno_to_soce(libc::EMSGSIZE),        SOCE_MSGSIZE);
        assert_eq!(errno_to_soce(libc::EPROTOTYPE),      SOCE_PROTOTYPE);
        assert_eq!(errno_to_soce(libc::ENOPROTOOPT),     SOCE_NOPROTOOPT);
        assert_eq!(errno_to_soce(libc::EPROTONOSUPPORT), SOCE_PROTONOSUPPORT);
        assert_eq!(errno_to_soce(libc::ESOCKTNOSUPPORT), SOCE_SOCKTNOSUPPORT);
        assert_eq!(errno_to_soce(libc::EOPNOTSUPP),      SOCE_OPNOTSUPP);
        assert_eq!(errno_to_soce(libc::EAFNOSUPPORT),    SOCE_AFNOSUPPORT);
        assert_eq!(errno_to_soce(libc::EADDRINUSE),      SOCE_ADDRINUSE);
        assert_eq!(errno_to_soce(libc::EADDRNOTAVAIL),   SOCE_ADDRNOTAVAIL);
        assert_eq!(errno_to_soce(libc::ENETDOWN),        SOCE_NETDOWN);
        assert_eq!(errno_to_soce(libc::ENETUNREACH),     SOCE_NETUNREACH);
        assert_eq!(errno_to_soce(libc::ECONNABORTED),    SOCE_CONNABORTED);
        assert_eq!(errno_to_soce(libc::ECONNRESET),      SOCE_CONNRESET);
        assert_eq!(errno_to_soce(libc::ENOBUFS),         SOCE_NOBUFS);
        assert_eq!(errno_to_soce(libc::EISCONN),         SOCE_ISCONN);
        assert_eq!(errno_to_soce(libc::ENOTCONN),        SOCE_NOTCONN);
        assert_eq!(errno_to_soce(libc::ETIMEDOUT),       SOCE_TIMEDOUT);
        assert_eq!(errno_to_soce(libc::ECONNREFUSED),    SOCE_CONNREFUSED);
        assert_eq!(errno_to_soce(libc::EHOSTDOWN),       SOCE_HOSTDOWN);
        assert_eq!(errno_to_soce(libc::EHOSTUNREACH),    SOCE_HOSTUNREACH);
    }

    // ── SocketManager ─────────────────────────────────────────────────────────

    #[test]
    fn test_socket_manager_alloc_lookup_remove() {
        let mut mgr = SocketManager::new();
        let h1 = mgr.alloc(5);
        let h2 = mgr.alloc(7);
        assert_eq!(mgr.lookup(h1), Some(5));
        assert_eq!(mgr.lookup(h2), Some(7));
        assert_eq!(mgr.remove(h1), Some(5));
        assert_eq!(mgr.lookup(h1), None);
        assert_eq!(mgr.lookup(h2), Some(7));
    }

    #[test]
    fn test_socket_manager_handle_starts_at_0xa000() {
        let mut mgr = SocketManager::new();
        let h = mgr.alloc(3);
        assert_eq!(h, 0xA000);
        let h2 = mgr.alloc(4);
        assert_eq!(h2, 0xA001);
    }

    #[test]
    fn test_socket_manager_errno() {
        let mgr = SocketManager::new();
        assert_eq!(mgr.last_sock_errno.load(Ordering::Relaxed), 0);
        mgr.set_errno(SOCE_CONNREFUSED);
        assert_eq!(mgr.last_sock_errno.load(Ordering::Relaxed), SOCE_CONNREFUSED);
        mgr.clear_errno();
        assert_eq!(mgr.last_sock_errno.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_socket_manager_remove_unknown_returns_none() {
        let mut mgr = SocketManager::new();
        assert_eq!(mgr.remove(0xDEAD), None);
    }

    // ── sock_init / sock_errno via Loader::new_mock ───────────────────────────

    #[test]
    fn test_so_sock_init_returns_0() {
        let loader = Loader::new_mock();
        assert_eq!(loader.so_sock_init(), 0);
    }

    #[test]
    fn test_so_sock_errno_initially_0() {
        let loader = Loader::new_mock();
        assert_eq!(loader.so_sock_errno(), 0);
    }

    #[test]
    fn test_so_sock_errno_after_bad_close() {
        let loader = Loader::new_mock();
        // soclose on an invalid handle → sets sock_errno to SOCE_BADF.
        loader.so_close(0xDEAD);
        assert_eq!(loader.so_sock_errno(), SOCE_BADF as u32);
    }

    /// Sockets are close-on-exec: a child process started by DosExecPgm
    /// does not inherit them.
    #[test]
    fn test_so_socket_is_close_on_exec() {
        let loader = Loader::new_mock();
        let h = loader.so_socket(libc::AF_INET as u32, libc::SOCK_STREAM as u32, 0);
        assert_ne!(h, SOCK_ERROR);
        let fd = loader.shared.socket_mgr.lock_or_recover().lookup(h).unwrap();
        // SAFETY: fcntl F_GETFD on an open descriptor.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
        loader.so_close(h);
    }

    #[test]
    fn test_so_socket_invalid_domain_sets_errno() {
        let loader = Loader::new_mock();
        // AF_UNSPEC with SOCK_STREAM should fail with EINVAL on Linux.
        let result = loader.so_socket(0, 1, 0);
        // If it returns SOCK_ERROR the errno is set; if it somehow succeeds, close it.
        if result == SOCK_ERROR {
            let e = loader.so_sock_errno();
            assert!(e != 0, "sock_errno must be set on failure");
        } else {
            loader.so_close(result);
        }
    }

    #[test]
    fn test_select_empty_fdsets_with_timeout() {
        let loader = Loader::new_mock();
        // Allocate a timeval with 0,0 (immediate timeout).
        let tv_addr = loader.shared.mem_mgr.lock_or_recover().alloc(8).unwrap();
        let _ = loader.guest_write::<u32>(tv_addr,     0); // tv_sec
        let _ = loader.guest_write::<u32>(tv_addr + 4, 0); // tv_usec

        // select with no fd_sets and zero timeout must return 0 (no events, timed out).
        let rc = loader.so_select(0, 0, 0, 0, tv_addr);
        assert_eq!(rc, 0, "select with empty sets + zero timeout must return 0");
    }

    #[test]
    fn test_gethostbyname_localhost() {
        let loader = Loader::new_mock();
        let name_addr = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        loader.guest_write_bytes(name_addr, b"localhost\0");

        let ptr = loader.so_gethostbyname(name_addr);
        assert_ne!(ptr, 0, "gethostbyname(localhost) must return non-null");

        // Read h_addrtype (offset 0x08) — must be AF_INET = 2.
        let addr_type = loader.guest_read::<i32>(ptr + 0x08).unwrap();
        assert_eq!(addr_type, libc::AF_INET, "h_addrtype must be AF_INET");

        // Read h_length (offset 0x0C) — must be 4 for IPv4.
        let h_len = loader.guest_read::<i32>(ptr + 0x0C).unwrap();
        assert_eq!(h_len, 4, "h_length must be 4 for IPv4");

        // Read h_addr_list pointer (offset 0x10) and then the IP address.
        let addrlist_ptr = loader.guest_read::<u32>(ptr + 0x10).unwrap();
        let ip_ptr = loader.guest_read::<u32>(addrlist_ptr).unwrap();
        assert_ne!(ip_ptr, 0, "h_addr_list[0] must not be null");

        // The IP is stored as raw bytes in network byte order.
        // Read as individual bytes to avoid host-endian confusion.
        let b0 = loader.guest_read::<u8>(ip_ptr).unwrap();
        let b1 = loader.guest_read::<u8>(ip_ptr + 1).unwrap();
        let b2 = loader.guest_read::<u8>(ip_ptr + 2).unwrap();
        let b3 = loader.guest_read::<u8>(ip_ptr + 3).unwrap();
        assert_eq!([b0, b1, b2, b3], [127, 0, 0, 1], "localhost must resolve to 127.0.0.1");
    }

    #[test]
    fn test_gethostbyname_unknown_host_returns_null() {
        let loader = Loader::new_mock();
        let name_addr = loader.shared.mem_mgr.lock_or_recover().alloc(32).unwrap();
        loader.guest_write_bytes(name_addr, b"this.host.does.not.exist.invalid\0");

        let ptr = loader.so_gethostbyname(name_addr);
        assert_eq!(ptr, 0, "unknown host must return null");
    }

    #[test]
    fn test_getservbyname_http() {
        let loader = Loader::new_mock();
        let name_addr = loader.shared.mem_mgr.lock_or_recover().alloc(8).unwrap();
        loader.guest_write_bytes(name_addr, b"http\0");

        let ptr = loader.so_getservbyname(name_addr, 0);
        assert_ne!(ptr, 0, "getservbyname(http) must return non-null");

        // s_port is at offset 0x08, in network byte order.
        let port_net = loader.guest_read::<i32>(ptr + 0x08).unwrap();
        let port = i16::from_be(port_net as i16);
        assert_eq!(port, 80, "http service must have port 80");
    }

    #[test]
    fn test_getservbyname_unknown_returns_null() {
        let loader = Loader::new_mock();
        let name_addr = loader.shared.mem_mgr.lock_or_recover().alloc(32).unwrap();
        loader.guest_write_bytes(name_addr, b"definitely_not_a_real_service_xyz\0");

        let ptr = loader.so_getservbyname(name_addr, 0);
        assert_eq!(ptr, 0, "unknown service must return null");
    }

    #[test]
    fn test_so_bind_and_getsockname() {
        let loader = Loader::new_mock();
        // Create a real TCP socket via libc and insert into SocketManager.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(fd >= 0, "libc::socket failed");
        let h = loader.shared.socket_mgr.lock_or_recover().alloc(fd);

        // Prepare a sockaddr_in for 127.0.0.1:0.
        let sa_addr = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        loader.guest_write_bytes(sa_addr, &[2, 0, 0, 0, 127, 0, 0, 1, 0,0,0,0,0,0,0,0]);
        loader.guest_write::<u16>(sa_addr, 2u16); // AF_INET
        loader.guest_write::<u16>(sa_addr + 2, 0u16); // port 0

        let rc = loader.so_bind(h, sa_addr, 16);
        assert_eq!(rc, 0, "bind to 127.0.0.1:0 should succeed");

        // getsockname should return the assigned port.
        let name_addr = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        let len_addr  = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();
        loader.guest_write::<u32>(len_addr, 16);
        let rc2 = loader.so_getsockname(h, name_addr, len_addr);
        assert_eq!(rc2, 0, "getsockname should succeed");

        // Clean up.
        loader.so_close(h);
    }

    #[test]
    fn test_so_send_recv_loopback() {
        use std::net::{TcpListener, TcpStream};
        use std::os::unix::io::IntoRawFd;
        // Use Rust std to set up a loopback connection, then exercise send/recv via the handlers.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind failed");
        let port = listener.local_addr().unwrap().port();

        // Connect a client stream.
        let client = TcpStream::connect(format!("127.0.0.1:{}", port)).expect("connect failed");
        let (server, _) = listener.accept().expect("accept failed");

        let client_fd = client.into_raw_fd();
        let server_fd = server.into_raw_fd();

        let loader = Loader::new_mock();
        let cli_h  = loader.shared.socket_mgr.lock_or_recover().alloc(client_fd);
        let srv_h  = loader.shared.socket_mgr.lock_or_recover().alloc(server_fd);

        // Write "hello" via so_send.
        let send_addr = loader.shared.mem_mgr.lock_or_recover().alloc(8).unwrap();
        loader.guest_write_bytes(send_addr, b"hello");
        let sent = loader.so_send(cli_h, send_addr, 5, 0);
        assert_eq!(sent, 5, "so_send should return 5");

        // Read via so_recv.
        let recv_addr = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        // Give data time to arrive (loopback is instant but flush just in case).
        std::thread::sleep(std::time::Duration::from_millis(10));
        let got = loader.so_recv(srv_h, recv_addr, 16, 0);
        assert_eq!(got, 5, "so_recv should return 5");
        let b0 = loader.guest_read::<u8>(recv_addr).unwrap();
        assert_eq!(b0, b'h', "first byte should be 'h'");

        loader.so_close(cli_h);
        loader.so_close(srv_h);
    }

    #[test]
    fn fd_lists_follow_the_os2_tcpip_layout() {
        use super::{read_fd_list, write_fd_list, OS2_FD_SET_BYTES, OS2_FD_SETSIZE};
        let set = write_fd_list(&[0xA000, 0xA003, 7]);
        assert_eq!(set.len(), OS2_FD_SET_BYTES);
        assert_eq!(&set[..8], &[3, 0, 0, 0, 0x00, 0xA0, 0, 0], "count, padding, first socket");
        assert_eq!(read_fd_list(&set), Some(vec![0xA000, 0xA003, 7]));
        assert_eq!(read_fd_list(&write_fd_list(&[])), Some(vec![]));
        let mut too_many = write_fd_list(&[]);
        too_many[0] = OS2_FD_SETSIZE as u8 + 1;
        assert_eq!(read_fd_list(&too_many), None);
        let full: Vec<u32> = (0..80).collect();
        assert_eq!(read_fd_list(&write_fd_list(&full)).map(|v| v.len()), Some(OS2_FD_SETSIZE));
        assert_eq!(read_fd_list(&[1]), None, "short buffer");
    }

    /// select() reports a listening socket with a waiting connection (it used
    /// to read a bit mask, so no socket numbered from 0xA000 could be in it),
    /// keeps only ready sockets, and rejects a number that is no socket.
    #[test]
    fn select_reports_ready_sockets_by_number() {
        use super::{read_fd_list, write_fd_list, OS2_FD_SET_BYTES};
        use std::net::{TcpListener, TcpStream};
        use std::os::unix::io::IntoRawFd;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let idle = TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let loader = Loader::new_mock();
        let srv = loader.shared.socket_mgr.lock_or_recover().alloc(listener.into_raw_fd());
        let quiet = loader.shared.socket_mgr.lock_or_recover().alloc(idle.into_raw_fd());
        assert!(srv >= 0xA000);
        let buf = loader.shared.mem_mgr.lock_or_recover().alloc(2 * OS2_FD_SET_BYTES as u32 + 8).unwrap();
        let (rfds, tv) = (buf, buf + 2 * OS2_FD_SET_BYTES as u32);
        loader.guest_write::<i32>(tv, 2).unwrap();
        loader.guest_write::<i32>(tv + 4, 0).unwrap();
        loader.guest_write_bytes(rfds, &write_fd_list(&[quiet, srv])).unwrap();
        assert_eq!(loader.so_select(0, rfds, 0, 0, tv), 1);
        let ready = read_fd_list(&loader.guest_read_bytes(rfds, OS2_FD_SET_BYTES).unwrap());
        assert_eq!(ready, Some(vec![srv]), "only the socket with a connection");

        // Timeout: nothing ready, the set comes back empty.
        loader.guest_write::<i32>(tv, 0).unwrap();
        loader.guest_write_bytes(rfds, &write_fd_list(&[quiet])).unwrap();
        assert_eq!(loader.so_select(0, rfds, 0, 0, tv), 0);
        assert_eq!(read_fd_list(&loader.guest_read_bytes(rfds, OS2_FD_SET_BYTES).unwrap()), Some(vec![]));

        // A number that is no socket, a bad count, a bad timeout.
        loader.guest_write_bytes(rfds, &write_fd_list(&[0x1234])).unwrap();
        assert_eq!(loader.so_select(0, rfds, 0, 0, tv), SOCK_ERROR);
        assert_eq!(loader.so_sock_errno(), super::SOCE_NOTSOCK as u32);
        loader.guest_write::<u16>(rfds, 65).unwrap();
        assert_eq!(loader.so_select(0, rfds, 0, 0, tv), SOCK_ERROR);
        assert_eq!(loader.so_sock_errno(), super::SOCE_INVAL as u32);
        loader.guest_write_bytes(rfds, &write_fd_list(&[srv])).unwrap();
        loader.guest_write::<i32>(tv + 4, 1_000_000).unwrap();
        assert_eq!(loader.so_select(0, rfds, 0, 0, tv), SOCK_ERROR);
        assert_eq!(loader.so_sock_errno(), super::SOCE_INVAL as u32);
        loader.so_close(srv);
        loader.so_close(quiet);
    }

    #[test]
    fn os2_socket_options_map_to_linux() {
        use super::{linux_sockopt, OS2_SOL_SOCKET, OS2_SO_REUSEADDR, OS2_SO_TYPE,
                    OS2_IPPROTO_TCP, OS2_TCP_NODELAY};
        assert_eq!(linux_sockopt(OS2_SOL_SOCKET, OS2_SO_REUSEADDR),
                   Some((libc::SOL_SOCKET, libc::SO_REUSEADDR)));
        assert_eq!(linux_sockopt(OS2_SOL_SOCKET, OS2_SO_TYPE),
                   Some((libc::SOL_SOCKET, libc::SO_TYPE)));
        assert_eq!(linux_sockopt(OS2_IPPROTO_TCP, OS2_TCP_NODELAY),
                   Some((libc::IPPROTO_TCP, libc::TCP_NODELAY)));
        // Linux's own numbers are not OS/2's: SOL_SOCKET 1 is no OS/2 level.
        assert_eq!(linux_sockopt(libc::SOL_SOCKET as u32, libc::SO_REUSEADDR as u32), None);
        // SO_SNDTIMEO (0x1005) takes a timeval of a different size: refused.
        assert_eq!(linux_sockopt(OS2_SOL_SOCKET, 0x1005), None);
    }

    #[test]
    fn setsockopt_and_getsockopt_use_os2_numbers() {
        let loader = Loader::new_mock();
        let s = loader.so_socket(libc::AF_INET as u32, libc::SOCK_STREAM as u32, 0);
        assert_ne!(s, SOCK_ERROR);
        let (val, len) = (0x3000u32, 0x3100u32);
        loader.guest_write::<u32>(val, 1).unwrap();
        assert_eq!(loader.so_setsockopt(s, super::OS2_SOL_SOCKET, super::OS2_SO_REUSEADDR, val, 4), SOCK_SUCCESS);
        loader.guest_write::<u32>(len, 4).unwrap();
        assert_eq!(loader.so_getsockopt(s, super::OS2_SOL_SOCKET, super::OS2_SO_TYPE, val, len), SOCK_SUCCESS);
        assert_eq!(loader.guest_read::<u32>(val), Some(libc::SOCK_STREAM as u32));
        // An unsupported option and an oversized value fail without reaching the host.
        assert_eq!(loader.so_setsockopt(s, super::OS2_SOL_SOCKET, 0x1005, val, 8), SOCK_ERROR);
        assert_eq!(loader.so_setsockopt(s, super::OS2_SOL_SOCKET, super::OS2_SO_REUSEADDR, val, 4096), SOCK_ERROR);
        loader.so_close(s);
    }


    /// A blocking accept, recv and send give up with SOCEINTR when the
    /// thread is interrupted (a signal for thread 1, ADR-0035), instead of
    /// waiting in the host call; a non-blocking socket is left to the call.
    #[test]
    fn blocking_socket_calls_are_interrupted() {
        use crate::loader::interrupt::{begin_call, end_call};
        use crate::loader::threads::MAIN_TID;
        use crate::loader::constants::{SIG_SETFOCUS, XCPT_SIGNAL_INTR};
        let loader = Loader::new_mock();
        // A listening TCP socket on the loopback, nobody connecting.
        // SAFETY: plain socket calls on a descriptor created here.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        assert!(fd >= 0);
        let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
        let len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        assert_eq!(unsafe { libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) }, 0);
        assert_eq!(unsafe { libc::listen(fd, 1) }, 0);
        let h = loader.shared.socket_mgr.lock_or_recover().alloc(fd);
        // A signal waits for thread 1, which is in a call.
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        loader.post_signal(XCPT_SIGNAL_INTR);
        begin_call(MAIN_TID, true);
        let start = std::time::Instant::now();
        assert_eq!(loader.so_accept(h, 0, 0), SOCK_ERROR);
        assert_eq!(loader.shared.socket_mgr.lock_or_recover().last_sock_errno.load(Ordering::Relaxed), SOCE_INTR);
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "it did not hang in accept");
        end_call();
        // Non-blocking: accept fails at once with the call's own error.
        // SAFETY: fcntl on our descriptor.
        unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) };
        begin_call(MAIN_TID, true);
        assert_eq!(loader.so_accept(h, 0, 0), SOCK_ERROR);
        assert_ne!(loader.shared.socket_mgr.lock_or_recover().last_sock_errno.load(Ordering::Relaxed), SOCE_INTR, "not waited for");
        end_call();
    }

    /// A blocking send larger than the socket's free room sends what fits
    /// and returns that count when a signal stops it; with no room at all
    /// it fails with SOCEINTR.
    #[test]
    fn a_blocking_send_stops_at_a_signal() {
        use crate::loader::interrupt::{begin_call, end_call};
        use crate::loader::threads::MAIN_TID;
        use crate::loader::constants::{SIG_SETFOCUS, XCPT_SIGNAL_INTR};
        let loader = Loader::new_mock();
        let mut fds = [0i32; 2];
        // SAFETY: socketpair fills the two-element array.
        assert_eq!(unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) }, 0);
        let h = loader.shared.socket_mgr.lock_or_recover().alloc(fds[0]);
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        loader.post_signal(XCPT_SIGNAL_INTR);
        begin_call(MAIN_TID, true);
        const BIG: u32 = 8 << 20;
        let start = std::time::Instant::now();
        let sent = loader.so_send(h, 0x0200_0000, BIG, 0);
        assert!(sent > 0 && sent < BIG, "what fitted: {sent}");
        assert_eq!(loader.so_send(h, 0x0200_0000, BIG, 0), SOCK_ERROR, "no room left");
        assert_eq!(loader.shared.socket_mgr.lock_or_recover().last_sock_errno.load(Ordering::Relaxed), SOCE_INTR);
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        end_call();
        // SAFETY: closing the peer created above.
        unsafe { libc::close(fds[1]) };
    }

    // ── Only Internet sockets reach the host ─────────────────────────────

    #[test]
    fn only_internet_sockets_and_addresses_are_allowed() {
        assert_eq!(check_socket_kind(2, 1), Ok(()));
        assert_eq!(check_socket_kind(2, 2), Ok(()));
        assert_eq!(check_socket_kind(1, 1), Err(SOCE_AFNOSUPPORT), "AF_UNIX");
        assert_eq!(check_socket_kind(16, 3), Err(SOCE_AFNOSUPPORT), "AF_NETLINK");
        assert_eq!(check_socket_kind(10, 1), Err(SOCE_AFNOSUPPORT), "AF_INET6 is not offered");
        assert_eq!(check_socket_kind(2, 1 | 0x800), Err(SOCE_SOCKTNOSUPPORT), "Linux flag bits");
        let mut sin = [0u8; 16];
        sin[0] = 2;
        assert_eq!(check_sockaddr_in(&sin), Ok(()));
        sin[0] = 1;
        assert_eq!(check_sockaddr_in(&sin), Err(SOCE_AFNOSUPPORT), "a sockaddr_un");
        assert_eq!(check_sockaddr_in(&[2, 0]), Err(SOCE_AFNOSUPPORT), "too short");
    }

    #[test]
    fn a_guest_cannot_open_or_reach_a_unix_socket() {
        let loader = Loader::new_mock();
        assert_eq!(loader.so_socket(1, 1, 0), SOCK_ERROR);
        assert_eq!(loader.so_sock_errno(), SOCE_AFNOSUPPORT as u32);
        // An Internet socket connecting to a sockaddr_un is refused before
        // the host sees it.
        let s = loader.so_socket(2, 1, 0);
        assert_ne!(s, SOCK_ERROR);
        let mut sun = vec![1u8, 0];
        sun.extend_from_slice(b"/run/user/1000/bus\0");
        sun.resize(110, 0);
        loader.guest_write_bytes(0x9000, &sun).unwrap();
        assert_eq!(loader.so_connect(s, 0x9000, sun.len() as u32), SOCK_ERROR);
        assert_eq!(loader.so_sock_errno(), SOCE_AFNOSUPPORT as u32);
        assert_eq!(loader.so_bind(s, 0x9000, sun.len() as u32), SOCK_ERROR);
        assert_eq!(loader.so_connect(s, 0x9000, 8), SOCK_ERROR, "shorter than a sockaddr_in");
        assert_eq!(loader.so_sock_errno(), SOCE_INVAL as u32);
        loader.so_close(s);
    }
}
