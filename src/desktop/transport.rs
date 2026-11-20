// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop socket (ADR-0052): a Unix `SOCK_SEQPACKET` socket in the
//! session directory, carrying one protocol message per packet and, for the
//! messages that need one, one file descriptor (`SCM_RIGHTS`).
//!
//! The socket file is reached as `/proc/self/fd/<dir>/desktop.sock` through
//! a descriptor of the session directory, so a deep directory does not run
//! into the 108-byte limit of a socket path. The directory is private (mode
//! 0700, ADR-0032) and the socket file is made mode 0600; the server also
//! accepts only peers of its own user ([`Conn::peer_uid`]).

use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;

use super::proto::MAX_MESSAGE;

/// Name of the socket in the session directory.
pub const SOCKET_NAME: &str = "desktop.sock";

fn last_err() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// A short path for `name` inside directory `dir`, through `/proc/self/fd`,
/// with the directory's descriptor that must stay open while it is used.
fn short_path(dir: &Path, name: &str) -> std::io::Result<(OwnedFd, String)> {
    use std::os::unix::fs::OpenOptionsExt;
    let d = std::fs::OpenOptions::new().read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC).open(dir)?;
    let fd = OwnedFd::from(d);
    let path = format!("/proc/self/fd/{}/{name}", fd.as_raw_fd());
    Ok((fd, path))
}

/// `sockaddr_un` for `path` and its length.
fn sockaddr(path: &str) -> std::io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    // SAFETY: sockaddr_un is plain old data; all zeroes is a valid value.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_bytes();
    if bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    for (d, s) in addr.sun_path.iter_mut().zip(bytes) {
        *d = *s as libc::c_char;
    }
    let len = std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1;
    Ok((addr, len as libc::socklen_t))
}

fn seqpacket_socket() -> std::io::Result<OwnedFd> {
    // SAFETY: plain socket(2) call; the result is checked before use.
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if raw < 0 {
        return Err(last_err());
    }
    // SAFETY: socket returned a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// The server's listening socket.
pub struct Listener {
    fd: OwnedFd,
}

impl Listener {
    /// Listen on `SOCKET_NAME` in `dir`, replacing a stale socket file.
    pub fn bind(dir: &Path) -> std::io::Result<Listener> {
        let (_dir_fd, path) = short_path(dir, SOCKET_NAME)?;
        let _ = std::fs::remove_file(&path);
        let fd = seqpacket_socket()?;
        let (addr, len) = sockaddr(&path)?;
        // SAFETY: `addr` is a valid sockaddr_un of `len` bytes.
        if unsafe { libc::bind(fd.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len) } != 0 {
            return Err(last_err());
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        // SAFETY: plain listen(2) on the socket owned above.
        if unsafe { libc::listen(fd.as_raw_fd(), 16) } != 0 {
            return Err(last_err());
        }
        Ok(Listener { fd })
    }

    /// Wait for the next connection.
    pub fn accept(&self) -> std::io::Result<Conn> {
        loop {
            // SAFETY: accept4 with null address pointers is allowed.
            let raw = unsafe {
                libc::accept4(self.fd.as_raw_fd(), std::ptr::null_mut(), std::ptr::null_mut(), libc::SOCK_CLOEXEC)
            };
            if raw >= 0 {
                // SAFETY: accept4 returned a new descriptor that nothing else owns.
                return Ok(Conn { fd: unsafe { OwnedFd::from_raw_fd(raw) } });
            }
            let e = last_err();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    /// Wake a thread blocked in `accept` (it then fails).
    pub fn shutdown(&self) {
        // SAFETY: plain shutdown(2) on an owned socket.
        unsafe { libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_RDWR); }
    }
}

/// One end of a desktop connection.
pub struct Conn {
    fd: OwnedFd,
}

impl Conn {
    /// Connect to the server listening in `dir`.
    pub fn connect(dir: &Path) -> std::io::Result<Conn> {
        let (_dir_fd, path) = short_path(dir, SOCKET_NAME)?;
        let fd = seqpacket_socket()?;
        let (addr, len) = sockaddr(&path)?;
        // SAFETY: `addr` is a valid sockaddr_un of `len` bytes.
        if unsafe { libc::connect(fd.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len) } != 0 {
            return Err(last_err());
        }
        Ok(Conn { fd })
    }

    /// A connected pair, for tests.
    pub fn pair() -> std::io::Result<(Conn, Conn)> {
        let mut fds = [0 as RawFd; 2];
        // SAFETY: `fds` has room for the two descriptors socketpair writes.
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) } != 0 {
            return Err(last_err());
        }
        // SAFETY: socketpair returned two new descriptors that nothing else owns.
        Ok(unsafe { (Conn { fd: OwnedFd::from_raw_fd(fds[0]) }, Conn { fd: OwnedFd::from_raw_fd(fds[1]) }) })
    }

    /// The user id of the process at the other end.
    pub fn peer_uid(&self) -> std::io::Result<u32> {
        // SAFETY: ucred is plain old data; getsockopt fills at most `len` bytes.
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let r = unsafe {
            libc::getsockopt(self.fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED,
                             &mut cred as *mut _ as *mut libc::c_void, &mut len)
        };
        if r != 0 {
            return Err(last_err());
        }
        Ok(cred.uid)
    }

    /// Send one packet, with descriptor `fd` if given.
    pub fn send(&self, packet: &[u8], fd: Option<BorrowedFd<'_>>) -> std::io::Result<()> {
        if packet.len() > MAX_MESSAGE {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        }
        let mut iov = libc::iovec { iov_base: packet.as_ptr() as *mut libc::c_void, iov_len: packet.len() };
        // Room for one descriptor's control message, aligned for cmsghdr.
        let mut cbuf = [0u64; 4];
        // SAFETY: msghdr is plain old data; all zeroes is a valid value.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if let Some(fd) = fd {
            // SAFETY: CMSG_SPACE / CMSG_LEN are pure size computations.
            let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
            debug_assert!(space <= std::mem::size_of_val(&cbuf));
            msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = space as _;
            // SAFETY: the control buffer is `space` bytes, enough for one
            // header and one descriptor; CMSG_FIRSTHDR points into it.
            unsafe {
                let c = libc::CMSG_FIRSTHDR(&msg);
                (*c).cmsg_level = libc::SOL_SOCKET;
                (*c).cmsg_type = libc::SCM_RIGHTS;
                (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
                std::ptr::write_unaligned(libc::CMSG_DATA(c) as *mut RawFd, fd.as_raw_fd());
            }
        }
        loop {
            // SAFETY: `msg` points at `iov` and `cbuf`, both alive here.
            let n = unsafe { libc::sendmsg(self.fd.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) };
            if n >= 0 {
                return Ok(());
            }
            let e = last_err();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    /// Wait for the next packet and the descriptor it carries; `None` at
    /// the end of the connection. A packet larger than `MAX_MESSAGE`, cut
    /// control data or more than one descriptor is an error (any
    /// descriptors received are closed).
    pub fn recv(&self) -> std::io::Result<Option<(Vec<u8>, Option<OwnedFd>)>> {
        let mut buf = vec![0u8; MAX_MESSAGE + 1];
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut libc::c_void, iov_len: buf.len() };
        let mut cbuf = [0u64; 8];
        // SAFETY: msghdr is plain old data; all zeroes is a valid value.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = std::mem::size_of_val(&cbuf) as _;
        let n = loop {
            // SAFETY: `msg` points at `iov` (into `buf`) and `cbuf`, both alive.
            let n = unsafe { libc::recvmsg(self.fd.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
            if n >= 0 { break n as usize; }
            let e = last_err();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e);
            }
        };
        // Take ownership of every descriptor received first, so none leaks.
        let mut fds = Vec::new();
        // SAFETY: walking the control messages recvmsg filled in, within
        // `msg_controllen`; CMSG_* stay inside the buffer.
        unsafe {
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            while !c.is_null() {
                if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                    let data = libc::CMSG_DATA(c) as *const RawFd;
                    let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                    for i in 0..count {
                        fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                    }
                }
                c = libc::CMSG_NXTHDR(&msg, c);
            }
        }
        if msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 || n > MAX_MESSAGE || fds.len() > 1 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "oversized desktop packet"));
        }
        if n == 0 && fds.is_empty() {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some((buf, fds.pop())))
    }

    /// End the connection both ways (wakes a thread blocked in `recv`).
    pub fn shutdown(&self) {
        // SAFETY: plain shutdown(2) on an owned socket.
        unsafe { libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_RDWR); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn packets_keep_their_boundaries_and_carry_a_descriptor() {
        let (a, b) = Conn::pair().unwrap();
        a.send(b"one", None).unwrap();
        a.send(b"two!", None).unwrap();
        let pix = crate::desktop::pixmap::SharedPixmap::create(2, 2).unwrap();
        pix.write_row(0, 1, &[42, 43]);
        a.send(b"fd", Some(pix.fd())).unwrap();
        assert_eq!(b.recv().unwrap().map(|(p, f)| (p, f.is_some())), Some((b"one".to_vec(), false)));
        assert_eq!(b.recv().unwrap().map(|(p, _)| p), Some(b"two!".to_vec()));
        let (p, fd) = b.recv().unwrap().unwrap();
        assert_eq!(p, b"fd");
        let other = crate::desktop::pixmap::SharedPixmap::open(fd.unwrap(), 2, 2).unwrap();
        let mut row = [0; 2];
        assert!(other.read_row(0, 1, &mut row));
        assert_eq!(row, [42, 43]);
        drop(a);
        assert!(b.recv().unwrap().is_none(), "end of the connection");
    }

    #[test]
    fn oversized_packets_are_not_sent() {
        let (a, _b) = Conn::pair().unwrap();
        assert!(a.send(&vec![0; MAX_MESSAGE + 1], None).is_err());
    }

    #[test]
    fn the_peer_is_this_user() {
        let (a, _b) = Conn::pair().unwrap();
        assert_eq!(a.peer_uid().unwrap(), unsafe { libc::getuid() });
    }

    #[test]
    fn a_listener_in_a_deep_directory_accepts_connections() {
        let base = std::env::temp_dir().join(format!("warpine-desk-test-{}", std::process::id()));
        let deep = base.join("a".repeat(60)).join("b".repeat(60));
        std::fs::create_dir_all(&deep).unwrap();
        let l = Listener::bind(&deep).unwrap();
        let c = Conn::connect(&deep).unwrap();
        let s = l.accept().unwrap();
        c.send(b"hi", None).unwrap();
        assert_eq!(s.recv().unwrap().map(|(p, _)| p), Some(b"hi".to_vec()));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(deep.join(SOCKET_NAME)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = s.fd.as_fd();
        drop((l, c, s));
        let _ = std::fs::remove_dir_all(&base);
    }
}
