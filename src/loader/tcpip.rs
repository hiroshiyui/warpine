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
// OS/2 uses the OS/2 socket *handle* as the bit index in `fd_set` (max 64
// sockets, 2 × u32).  We translate each set bit to its Linux fd, build Linux
// `fd_set`s, call `select(2)`, then translate results back.
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

// OS/2 `fd_set` layout: 2 × u32 = 64 bits (max 64 sockets per set).
const OS2_FD_SET_WORDS: usize = 2;

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
        // SAFETY: domain/type/protocol are valid i32 values passed from guest args.
        let fd = unsafe { libc::socket(domain as i32, sock_type as i32, protocol as i32) };
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
        let addr_bytes = self.read_guest_bytes(name_ptr, namelen as usize);
        if addr_bytes.len() < namelen as usize {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_FAULT);
            return SOCK_ERROR;
        }
        // SAFETY: addr_bytes is a valid slice of at least namelen bytes read from bounded guest memory.
        let rc = unsafe {
            libc::bind(fd,
                addr_bytes.as_ptr() as *const libc::sockaddr,
                namelen as libc::socklen_t)
        };
        self.translate_rc(rc, SOCK_SUCCESS)
    }

    // ── connect (ordinal 3) ───────────────────────────────────────────────────
    //
    //   int connect(int s, struct sockaddr *name, int namelen)

    pub fn so_connect(&self, s: u32, name_ptr: u32, namelen: u32) -> u32 {
        debug!("connect(s=0x{:X}, name=0x{:X}, namelen={})", s, name_ptr, namelen);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let addr_bytes = self.read_guest_bytes(name_ptr, namelen as usize);
        if addr_bytes.len() < namelen as usize {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_FAULT);
            return SOCK_ERROR;
        }
        let rc = unsafe {
            libc::connect(fd,
                addr_bytes.as_ptr() as *const libc::sockaddr,
                namelen as libc::socklen_t)
        };
        self.translate_rc(rc, SOCK_SUCCESS)
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

    pub fn so_accept(&self, s: u32, addr_ptr: u32, addrlen_ptr: u32) -> u32 {
        debug!("accept(s=0x{:X}, addr=0x{:X}, addrlen_ptr=0x{:X})", s, addr_ptr, addrlen_ptr);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };

        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

        // SAFETY: storage is stack-allocated; len is bounded by sockaddr_storage size.
        let new_fd = unsafe {
            libc::accept(fd, &mut storage as *mut _ as *mut libc::sockaddr, &mut len)
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

    // ── sendto (ordinal 15) ───────────────────────────────────────────────────
    //
    //   int sendto(int s, const void *msg, int len, int flags,
    //              const struct sockaddr *to, int tolen)

    pub fn so_sendto(&self, s: u32, buf_ptr: u32, len: u32, flags: u32,
                     to_ptr: u32, tolen: u32) -> u32 {
        debug!("sendto(s=0x{:X}, len={}, flags={}, tolen={})", s, len, flags, tolen);
        let Some(fd) = self.so_fd(s) else { return SOCK_ERROR; };
        let data = self.read_guest_bytes(buf_ptr, len as usize);
        let sent = if to_ptr != 0 && tolen > 0 {
            let addr = self.read_guest_bytes(to_ptr, tolen as usize);
            unsafe {
                libc::sendto(fd, data.as_ptr() as *const libc::c_void, data.len(),
                    flags as i32,
                    addr.as_ptr() as *const libc::sockaddr,
                    tolen as libc::socklen_t)
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
        let optval = self.read_guest_bytes(optval_ptr, optlen as usize);
        let rc = unsafe {
            libc::setsockopt(fd, level as i32, optname as i32,
                optval.as_ptr() as *const libc::c_void,
                optlen as libc::socklen_t)
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
        let buf_len = self.guest_read::<u32>(optlen_ptr).unwrap_or(0);
        let Some(buf) = self.guest_slice_mut(optval_ptr, buf_len as usize) else {
            self.shared.socket_mgr.lock_or_recover().set_errno(SOCE_FAULT);
            return SOCK_ERROR;
        };
        let mut actual_len = buf_len as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(fd, level as i32, optname as i32,
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
    //   int select(int nfds, struct fd_set *readfds, struct fd_set *writefds,
    //              struct fd_set *exceptfds, struct timeval *timeout)
    //
    //   OS/2 fd_set: 2 × u32 (64 bits), bit N = OS/2 socket handle N.
    //   We translate each set OS/2 handle to its Linux fd, call select(2),
    //   then translate results back.
    //
    //   args[0] = nfds       — highest OS/2 handle + 1 (used as hint)
    //   args[1] = readfds    — ptr to OS/2 fd_set or 0
    //   args[2] = writefds   — ptr to OS/2 fd_set or 0
    //   args[3] = exceptfds  — ptr to OS/2 fd_set or 0
    //   args[4] = timeout    — ptr to struct timeval { tv_sec u32, tv_usec u32 } or 0

    pub fn so_select(&self, nfds: u32, rfds_ptr: u32, wfds_ptr: u32,
                     efds_ptr: u32, timeout_ptr: u32) -> u32 {
        debug!("select(nfds={}, rfds=0x{:X}, wfds=0x{:X}, efds=0x{:X}, timeout=0x{:X})",
               nfds, rfds_ptr, wfds_ptr, efds_ptr, timeout_ptr);

        // Build a snapshot of handle→fd mappings for translation.
        let handle_map: Vec<(u32, i32)> = {
            let mgr = self.shared.socket_mgr.lock_or_recover();
            mgr.sockets.iter().map(|(&h, &fd)| (h, fd)).collect()
        };

        // Read OS/2 fd_set bits (2 × u32 = 64 bits, handles 0..63).
        let read_os2_fdset = |ptr: u32| -> [u32; OS2_FD_SET_WORDS] {
            if ptr == 0 { return [0; OS2_FD_SET_WORDS]; }
            [
                self.guest_read::<u32>(ptr).unwrap_or(0),
                self.guest_read::<u32>(ptr + 4).unwrap_or(0),
            ]
        };

        let os2_rfds = read_os2_fdset(rfds_ptr);
        let os2_wfds = read_os2_fdset(wfds_ptr);
        let os2_efds = read_os2_fdset(efds_ptr);

        // Translate OS/2 handle bits → Linux fd_set, tracking max fd.
        let mut linux_rfds: libc::fd_set = unsafe { std::mem::zeroed() };
        let mut linux_wfds: libc::fd_set = unsafe { std::mem::zeroed() };
        let mut linux_efds: libc::fd_set = unsafe { std::mem::zeroed() };
        let mut max_fd: i32 = -1;

        let translate = |os2_fds: &[u32; OS2_FD_SET_WORDS],
                          linux_fds: &mut libc::fd_set,
                          map: &[(u32, i32)],
                          max: &mut i32| {
            unsafe { libc::FD_ZERO(linux_fds); }
            for &(handle, fd) in map {
                let h = handle as usize;
                if h < 64 {
                    let word = os2_fds[h / 32];
                    if word & (1 << (h % 32)) != 0 {
                        unsafe { libc::FD_SET(fd, linux_fds); }
                        if fd > *max { *max = fd; }
                    }
                }
            }
        };

        if rfds_ptr != 0 { translate(&os2_rfds, &mut linux_rfds, &handle_map, &mut max_fd); }
        if wfds_ptr != 0 { translate(&os2_wfds, &mut linux_wfds, &handle_map, &mut max_fd); }
        if efds_ptr != 0 { translate(&os2_efds, &mut linux_efds, &handle_map, &mut max_fd); }

        // Build Linux timeval from guest.
        let mut tv = libc::timeval { tv_sec: 0, tv_usec: 0 };
        let tv_ptr = if timeout_ptr != 0 {
            tv.tv_sec  = self.guest_read::<u32>(timeout_ptr).unwrap_or(0) as libc::time_t;
            tv.tv_usec = self.guest_read::<u32>(timeout_ptr + 4).unwrap_or(0) as libc::suseconds_t;
            &mut tv as *mut _
        } else {
            std::ptr::null_mut()
        };

        let rc = unsafe {
            libc::select(max_fd + 1,
                if rfds_ptr != 0 { &mut linux_rfds } else { std::ptr::null_mut() },
                if wfds_ptr != 0 { &mut linux_wfds } else { std::ptr::null_mut() },
                if efds_ptr != 0 { &mut linux_efds } else { std::ptr::null_mut() },
                tv_ptr)
        };
        if rc < 0 {
            let e = errno_to_soce(last_errno());
            self.shared.socket_mgr.lock_or_recover().set_errno(e);
            return SOCK_ERROR;
        }

        // Translate Linux fd_set results back to OS/2 fd_set bits.
        let translate_back = |linux_fds: &libc::fd_set,
                               ptr: u32,
                               map: &[(u32, i32)]| {
            if ptr == 0 { return; }
            let mut bits = [0u32; OS2_FD_SET_WORDS];
            for &(handle, fd) in map {
                let h = handle as usize;
                if h < 64 && unsafe { libc::FD_ISSET(fd, linux_fds) } {
                    bits[h / 32] |= 1 << (h % 32);
                }
            }
            let _ = self.guest_write::<u32>(ptr,     bits[0]);
            let _ = self.guest_write::<u32>(ptr + 4, bits[1]);
        };

        if rfds_ptr != 0 { translate_back(&linux_rfds, rfds_ptr, &handle_map); }
        if wfds_ptr != 0 { translate_back(&linux_wfds, wfds_ptr, &handle_map); }
        if efds_ptr != 0 { translate_back(&linux_efds, efds_ptr, &handle_map); }

        self.shared.socket_mgr.lock_or_recover().clear_errno();
        rc as u32
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
                .filter_map(|i| self.guest_read::<u8>(addr + i as u32))
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
        use std::os::fd::IntoRawFd;
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
        use std::io::Write;
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
}
