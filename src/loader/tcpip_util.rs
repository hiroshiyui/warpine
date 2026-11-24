// SPDX-License-Identifier: GPL-3.0-or-later
//
// The TCP/IP network utility routines that need no network: address
// conversion (`inet_addr`, `inet_network`, `inet_ntoa`, `inet_makeaddr`,
// `inet_lnaof`, `inet_netof`), byte order (`bswap`, `lswap`, `htonl` …),
// the resolver's name packing (`dn_comp`, `dn_expand`, `dn_skipname`,
// `_getshort`, `_getlong`, `putshort`, `putlong`) and the protocol database
// (`getprotobyname`, `getprotobynumber`, `setprotoent` / `getprotoent` /
// `endprotoent`); and the SO32DLL calls `ioctl`, `readv` and `writev`.
//
// Semantics after IBM's TCP/IP Programming Reference and the BSD routines
// it documents. No public source gives SO32DLL's or TCP32DLL's ordinals:
// these calls sit at Warpine's own (targets/os2api.def), which programs
// built with Warpine's tools link by name (doc/unverified.md).

use log::debug;

use super::mutex_ext::MutexExt;

// ── Addresses ────────────────────────────────────────────────────────────────

/// `INADDR_NONE`: what `inet_addr` / `inet_network` return for a bad string.
pub const INADDR_NONE: u32 = 0xFFFF_FFFF;

/// The parts of a dotted address string: up to four numbers, each decimal,
/// octal (leading 0) or hexadecimal (0x), separated by dots.
fn address_parts(s: &[u8]) -> Option<Vec<u32>> {
    let mut parts = Vec::new();
    for field in s.split(|&b| b == b'.') {
        let (digits, radix) = match field {
            [b'0', b'x' | b'X', rest @ ..] => (rest, 16),
            [b'0', rest @ ..] if !rest.is_empty() => (rest, 8),
            _ => (field, 10),
        };
        if digits.is_empty() && radix != 16 { return None; }
        let mut v: u32 = 0;
        for &d in digits {
            let digit = (d as char).to_digit(radix)?;
            v = v.checked_mul(radix)?.checked_add(digit)?;
        }
        parts.push(v);
    }
    (1..=4).contains(&parts.len()).then_some(parts)
}

/// `inet_addr`: the address `a`, `a.b` (b 24 bits), `a.b.c` (c 16 bits)
/// or `a.b.c.d` in host order, or `None`.
pub fn parse_inet_addr(s: &[u8]) -> Option<u32> {
    let p = address_parts(s)?;
    let fits = |v: u32, bits: u32| bits >= 32 || v < (1 << bits);
    let byte = |v: u32| fits(v, 8);
    match p.as_slice() {
        [a] => Some(*a),
        [a, b] if byte(*a) && fits(*b, 24) => Some(a << 24 | b),
        [a, b, c] if byte(*a) && byte(*b) && fits(*c, 16) => Some(a << 24 | b << 16 | c),
        [a, b, c, d] if [a, b, c, d].iter().all(|v| byte(**v)) => Some(a << 24 | b << 16 | c << 8 | d),
        _ => None,
    }
}

/// `inet_network`: the parts packed from the right, each a byte
/// (`"10.1"` is 0x0A01), in host order, or `None`.
pub fn parse_inet_network(s: &[u8]) -> Option<u32> {
    let p = address_parts(s)?;
    if p.iter().any(|&v| v > 0xFF) { return None; }
    Some(p.iter().fold(0, |acc, &v| acc << 8 | v))
}

/// `inet_makeaddr`: a network number and a local address, in host order.
pub fn make_addr(net: u32, host: u32) -> u32 {
    if net < 128 {
        net << 24 | (host & 0x00FF_FFFF)
    } else if net < 65536 {
        net << 16 | (host & 0xFFFF)
    } else if net < 16_777_216 {
        net << 8 | (host & 0xFF)
    } else {
        net | host
    }
}

/// `inet_lnaof` / `inet_netof`: the local and network parts of a host-order
/// address by its class (A, B, or C and above).
pub fn split_addr(addr: u32) -> (u32, u32) {
    if addr & 0x8000_0000 == 0 {
        (addr & 0x00FF_FFFF, addr >> 24)
    } else if addr & 0xC000_0000 == 0x8000_0000 {
        (addr & 0xFFFF, addr >> 16)
    } else {
        (addr & 0xFF, addr >> 8)
    }
}

/// `inet_ntoa`'s dotted form of a host-order address.
pub fn dotted(addr: u32) -> String {
    let [a, b, c, d] = addr.to_be_bytes();
    format!("{a}.{b}.{c}.{d}")
}

// ── Domain names (RFC 1035 packing) ──────────────────────────────────────────

/// The longest domain name `dn_expand` / `dn_comp` handle (RFC 1035).
pub const MAX_DNAME: usize = 255;
/// A label's length byte has its two top bits clear; a pointer has both set.
const POINTER_BITS: u8 = 0xC0;
const MAX_LABEL: usize = 63;

/// `dn_expand`: the name packed at `pos` of `msg` as dotted text, and how
/// many bytes it takes at `pos` (a pointer ends it there).
pub fn expand_name(msg: &[u8], pos: usize) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut at = pos;
    let mut consumed = None;
    // Each pointer must go backwards, so a loop cannot run forever; the
    // hop count bounds it anyway.
    for _ in 0..msg.len().max(1) {
        let len = *msg.get(at)?;
        match len & POINTER_BITS {
            0 if len == 0 => {
                return Some((out, consumed.unwrap_or(at + 1 - pos)));
            }
            0 => {
                let label = msg.get(at + 1..at + 1 + len as usize)?;
                if !out.is_empty() { out.push(b'.'); }
                out.extend_from_slice(label);
                if out.len() > MAX_DNAME { return None; }
                at += 1 + len as usize;
            }
            POINTER_BITS => {
                let target = usize::from(u16::from_be_bytes([len & !POINTER_BITS, *msg.get(at + 1)?]));
                if target >= at { return None; }
                consumed.get_or_insert(at + 2 - pos);
                at = target;
            }
            _ => return None,
        }
    }
    None
}

/// `dn_skipname`: how many bytes the packed name at `pos` takes.
pub fn skip_name(msg: &[u8], pos: usize) -> Option<usize> {
    let mut at = pos;
    loop {
        let len = *msg.get(at)?;
        match len & POINTER_BITS {
            0 if len == 0 => return Some(at + 1 - pos),
            0 => at += 1 + len as usize,
            POINTER_BITS => { msg.get(at + 1)?; return Some(at + 2 - pos); }
            _ => return None,
        }
    }
}

/// `dn_comp` without compression: the dotted `name` as length-prefixed
/// labels and a zero byte (a trailing dot is allowed), or `None` for an
/// empty or oversized label.
pub fn pack_name(name: &[u8]) -> Option<Vec<u8>> {
    let name = name.strip_suffix(b".").unwrap_or(name);
    let mut out = Vec::new();
    if !name.is_empty() {
        for label in name.split(|&b| b == b'.') {
            if label.is_empty() || label.len() > MAX_LABEL { return None; }
            out.push(label.len() as u8);
            out.extend_from_slice(label);
        }
    }
    out.push(0);
    (out.len() <= MAX_DNAME + 1).then_some(out)
}

// ── Protocols ────────────────────────────────────────────────────────────────

/// Warpine's protocol database (OS/2 reads `%ETC%\PROTOCOL`; Warpine has
/// none, so these are the Internet protocols the stack offers):
/// (name, alias, number).
pub const PROTOCOLS: &[(&str, &str, u32)] = &[
    ("ip", "IP", 0),
    ("icmp", "ICMP", 1),
    ("tcp", "TCP", 6),
    ("udp", "UDP", 17),
];

/// The database index of a protocol by name or alias.
pub fn protocol_by_name(name: &[u8]) -> Option<usize> {
    PROTOCOLS.iter().position(|(n, a, _)| n.as_bytes() == name || a.as_bytes() == name)
}

/// The database index of a protocol by number.
pub fn protocol_by_number(number: u32) -> Option<usize> {
    PROTOCOLS.iter().position(|&(_, _, p)| p == number)
}

// ── Services and the resolver's error ────────────────────────────────────────

/// `h_errno` values (<netdb.h>).
pub const HOST_NOT_FOUND: u32 = 1;
/// `h_errno`: no answer, try again; an error it cannot recover from; the
/// name is valid but has no data of the type asked (`<netdb.h>`).
pub const TRY_AGAIN: u32 = 2;
pub const NO_RECOVERY: u32 = 3;
pub const NO_DATA: u32 = 4;
/// The most services `getservent` lists.
const MAX_SERVICES: usize = 4096;
/// libc's services cursor is process-wide state: one reader at a time.
static HOST_SERVICES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The host's services database (name, port in network order, protocol),
/// at most `MAX_SERVICES` entries.
fn host_services() -> Vec<(String, i32, String)> {
    let _one_reader = HOST_SERVICES.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    // SAFETY: setservent / getservent / endservent are called under the
    // lock above; each entry's strings are copied before the next call.
    unsafe {
        libc::setservent(0);
        while out.len() < MAX_SERVICES {
            let se = libc::getservent();
            if se.is_null() { break; }
            let text = |p: *const libc::c_char| std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
            out.push((text((*se).s_name), (*se).s_port, text((*se).s_proto)));
        }
        libc::endservent();
    }
    out
}

// ── ioctl ────────────────────────────────────────────────────────────────────

/// The socket `ioctl` requests Warpine answers, by their group and number
/// (the low 16 bits: the direction and size bits differ between the BSD
/// 4.3 and 4.4 headers, so they are not compared).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketIoctl {
    /// `FIONBIO` ('f', 126): non-blocking on or off.
    NonBlocking,
    /// `FIONREAD` ('f', 127): the bytes waiting to be read.
    BytesWaiting,
    /// `SIOCATMARK` ('s', 7): at the out-of-band mark.
    AtMark,
}

pub fn socket_ioctl(cmd: u32) -> Option<SocketIoctl> {
    match (((cmd >> 8) & 0xFF) as u8, cmd & 0xFF) {
        (b'f', 126) => Some(SocketIoctl::NonBlocking),
        (b'f', 127) => Some(SocketIoctl::BytesWaiting),
        (b's', 7) => Some(SocketIoctl::AtMark),
        _ => None,
    }
}

/// Linux's `SIOCATMARK` (<asm-generic/sockios.h>; not in the libc crate).
const LINUX_SIOCATMARK: libc::Ioctl = 0x8905;

/// The most `iovec`s `readv` / `writev` take (BSD's `UIO_MAXIOV`).
pub const MAX_IOVEC: u32 = 1024;
/// `struct iovec`: `iov_base`, `iov_len`.
const IOVEC_SIZE: u32 = 8;
/// The guest buffer for `inet_ntoa`'s text ("255.255.255.255" and NUL).
const NTOA_BUF: u32 = 16;
/// The guest buffer for a `protoent` (12 bytes), its alias list (8) and
/// its strings.
const PROTOENT_BUF: u32 = 64;

impl super::Loader {
    /// A NUL-terminated guest string's bytes (at most `MAX_DNAME * 4`).
    fn net_string(&self, p: u32) -> Option<Vec<u8>> {
        const LIMIT: u32 = (MAX_DNAME * 4) as u32;
        let mut out = Vec::new();
        for i in 0..LIMIT {
            match self.guest_read::<u8>(p.checked_add(i)?)? {
                0 => return Some(out),
                b => out.push(b),
            }
        }
        None
    }

    /// A per-process guest buffer of `size` bytes kept in `slot`.
    fn net_buffer(&self, slot: fn(&mut super::tcpip::SocketManager) -> &mut Option<u32>, size: u32) -> Option<u32> {
        let mut sockets = self.shared.socket_mgr.lock_or_recover();
        if let Some(addr) = *slot(&mut sockets) { return Some(addr); }
        let addr = self.shared.mem_mgr.lock_or_recover().alloc(size)?;
        *slot(&mut sockets) = Some(addr);
        Some(addr)
    }

    /// inet_addr: the network-order address of a dotted string, or
    /// `INADDR_NONE`.
    pub fn net_inet_addr(&self, p: u32) -> u32 {
        let addr = self.net_string(p).and_then(|s| parse_inet_addr(&s));
        debug!("inet_addr -> {addr:?}");
        addr.map_or(INADDR_NONE, u32::to_be)
    }

    /// inet_network: the host-order network number of a dotted string, or
    /// `INADDR_NONE`.
    pub fn net_inet_network(&self, p: u32) -> u32 {
        self.net_string(p).and_then(|s| parse_inet_network(&s)).unwrap_or(INADDR_NONE)
    }

    /// inet_ntoa: the dotted text of a network-order address, in a buffer
    /// of the process's that the next call overwrites.
    pub fn net_inet_ntoa(&self, addr: u32) -> u32 {
        let Some(buf) = self.net_buffer(|s| &mut s.ntoa_addr, NTOA_BUF) else { return 0 };
        let mut text = dotted(u32::from_be(addr)).into_bytes();
        text.push(0);
        if self.guest_write_bytes(buf, &text).is_none() { return 0; }
        buf
    }

    /// The `protoent` of database entry `i` in the process's buffer.
    fn net_protoent(&self, i: Option<usize>) -> u32 {
        let Some(&(name, alias, number)) = i.and_then(|i| PROTOCOLS.get(i)) else { return 0 };
        let Some(buf) = self.net_buffer(|s| &mut s.protoent_addr, PROTOENT_BUF) else { return 0 };
        let aliases = buf + 12;
        let name_at = aliases + 8;
        let alias_at = name_at + name.len() as u32 + 1;
        let ok = [(buf, name_at), (buf + 4, aliases), (buf + 8, number), (aliases, alias_at), (aliases + 4, 0)]
            .iter().all(|&(at, v)| self.guest_write::<u32>(at, v).is_some())
            && self.guest_write_bytes(name_at, &[name.as_bytes(), b"\0"].concat()).is_some()
            && self.guest_write_bytes(alias_at, &[alias.as_bytes(), b"\0"].concat()).is_some();
        if ok { buf } else { 0 }
    }

    /// getprotobyname: the protocol of a name or alias, or 0.
    pub fn net_getprotobyname(&self, p: u32) -> u32 {
        let i = self.net_string(p).and_then(|s| protocol_by_name(&s));
        self.net_protoent(i)
    }

    /// getprotobynumber: the protocol of a number, or 0.
    pub fn net_getprotobynumber(&self, number: u32) -> u32 {
        self.net_protoent(protocol_by_number(number))
    }

    /// setprotoent: start the database over (`stayopen` has nothing to
    /// keep open).
    pub fn net_setprotoent(&self) -> u32 {
        self.shared.socket_mgr.lock_or_recover().proto_cursor = 0;
        0
    }

    /// getprotoent: the next protocol of the database, or 0 at its end.
    pub fn net_getprotoent(&self) -> u32 {
        let i = {
            let mut sockets = self.shared.socket_mgr.lock_or_recover();
            let i = sockets.proto_cursor;
            sockets.proto_cursor = (i + 1).min(PROTOCOLS.len());
            i
        };
        self.net_protoent((i < PROTOCOLS.len()).then_some(i))
    }

    /// tcp_h_errno: the resolver's last error (`HOST_NOT_FOUND` after a
    /// lookup that found nothing), as the `h_errno` macro reads it.
    pub fn net_tcp_h_errno(&self) -> u32 {
        self.shared.socket_mgr.lock_or_recover().h_errno
    }

    /// setservent: start the services database over, reading it from the
    /// host the first time.
    pub fn net_setservent(&self) -> u32 {
        let mut sockets = self.shared.socket_mgr.lock_or_recover();
        if sockets.services.is_none() { sockets.services = Some(host_services()); }
        sockets.serv_cursor = 0;
        0
    }

    /// getservent: the next service, or 0 at the end of the database.
    pub fn net_getservent(&self) -> u32 {
        let next = {
            let mut sockets = self.shared.socket_mgr.lock_or_recover();
            if sockets.services.is_none() { sockets.services = Some(host_services()); }
            let i = sockets.serv_cursor;
            let entry = sockets.services.as_ref().and_then(|v| v.get(i).cloned());
            if entry.is_some() { sockets.serv_cursor += 1; }
            entry
        };
        next.map_or(0, |(name, port, proto)| self.write_servent_to_scratch(&name, port, &proto))
    }

    /// endservent: forget the database and its place.
    pub fn net_endservent(&self) -> u32 {
        let mut sockets = self.shared.socket_mgr.lock_or_recover();
        sockets.services = None;
        sockets.serv_cursor = 0;
        0
    }

    // ── The host and network databases, the resolver (resolver.rs) ─────

    /// sethostent(stayopen): read the host's `/etc/hosts` and start at its
    /// first entry.
    pub fn net_sethostent(&self) -> u32 {
        let hosts = super::resolver::parse_hosts(&super::resolver::read_bounded("/etc/hosts"));
        let mut sockets = self.shared.socket_mgr.lock_or_recover();
        sockets.hosts = Some(hosts);
        sockets.host_cursor = 0;
        0
    }

    /// gethostent: the next host, or 0 at the end.
    pub fn net_gethostent(&self) -> u32 {
        if self.shared.socket_mgr.lock_or_recover().hosts.is_none() { self.net_sethostent(); }
        let next = {
            let mut sockets = self.shared.socket_mgr.lock_or_recover();
            let i = sockets.host_cursor;
            let entry = sockets.hosts.as_ref().and_then(|v| v.get(i).cloned());
            if entry.is_some() { sockets.host_cursor += 1; }
            entry
        };
        next.map_or(0, |(name, ip)| self.write_hostent_to_scratch(&name, ip))
    }

    /// endhostent: forget the database and its place.
    pub fn net_endhostent(&self) -> u32 {
        let mut sockets = self.shared.socket_mgr.lock_or_recover();
        sockets.hosts = None;
        sockets.host_cursor = 0;
        0
    }

    fn host_networks() -> Vec<(String, u32)> {
        super::resolver::parse_networks(&super::resolver::read_bounded("/etc/networks"))
    }

    /// setnetent(stayopen): read the host's `/etc/networks`.
    pub fn net_setnetent(&self) -> u32 {
        let nets = Self::host_networks();
        let mut sockets = self.shared.socket_mgr.lock_or_recover();
        sockets.networks = Some(nets);
        sockets.net_cursor = 0;
        0
    }

    /// getnetent: the next network, or 0 at the end.
    pub fn net_getnetent(&self) -> u32 {
        if self.shared.socket_mgr.lock_or_recover().networks.is_none() { self.net_setnetent(); }
        let next = {
            let mut sockets = self.shared.socket_mgr.lock_or_recover();
            let i = sockets.net_cursor;
            let entry = sockets.networks.as_ref().and_then(|v| v.get(i).cloned());
            if entry.is_some() { sockets.net_cursor += 1; }
            entry
        };
        next.map_or(0, |(name, net)| self.write_netent_to_scratch(&name, net))
    }

    /// endnetent: forget the database and its place.
    pub fn net_endnetent(&self) -> u32 {
        let mut sockets = self.shared.socket_mgr.lock_or_recover();
        sockets.networks = None;
        sockets.net_cursor = 0;
        0
    }

    /// getnetbyname(name): the network of that name, or 0.
    pub fn net_getnetbyname(&self, p: u32) -> u32 {
        let name = self.read_guest_string(p);
        Self::host_networks().into_iter().find(|(n, _)| *n == name)
            .map_or(0, |(n, net)| self.write_netent_to_scratch(&n, net))
    }

    /// getnetbyaddr(net, type): the network of that number (AF_INET), or 0.
    pub fn net_getnetbyaddr(&self, net: u32, kind: u32) -> u32 {
        if kind != libc::AF_INET as u32 { return 0; }
        Self::host_networks().into_iter().find(|&(_, n)| n == net)
            .map_or(0, |(n, net)| self.write_netent_to_scratch(&n, net))
    }

    /// res_init: nothing to read ahead (every resolver call reads
    /// `/etc/resolv.conf`); 0.
    pub fn net_res_init(&self) -> u32 { 0 }

    /// res_mkquery(op, dname, class, type, data, datalen, newrr, buf,
    /// buflen): a standard query for `dname` in `buf`; its length, or -1
    /// (another op than QUERY, a bad name, a buffer too small).
    pub fn net_res_mkquery(&self, a: &[u32; 10]) -> u32 {
        const FAIL: u32 = u32::MAX;
        let (op, dname, class, qtype, buf, buflen) = (a[0], a[1], a[2], a[3], a[7], a[8]);
        if op != super::resolver::QUERY { return FAIL; }
        let name = self.read_guest_string(dname);
        let id = {
            let mut sockets = self.shared.socket_mgr.lock_or_recover();
            sockets.query_id = sockets.query_id.wrapping_add(1);
            sockets.query_id
        };
        let Some(q) = super::resolver::make_query(id, name.as_bytes(), class as u16, qtype as u16) else { return FAIL };
        if q.len() > buflen as usize || self.guest_write_bytes(buf, &q).is_none() { return FAIL; }
        q.len() as u32
    }

    /// The message at `msg` (`len` bytes) sent to the host's name servers;
    /// the answer, or None (with `SOCETIMEDOUT`).
    fn res_exchange(&self, msg: &[u8]) -> Option<Vec<u8>> {
        let (servers, _) = super::resolver::host_resolver();
        let answer = super::resolver::exchange(msg, &servers, super::resolver::server_wait());
        if answer.is_none() { self.shared.socket_mgr.lock_or_recover().set_errno(super::tcpip::SOCE_TIMEDOUT); }
        answer
    }

    /// res_send(msg, msglen, answer, anslen): the message sent, the answer
    /// (cut to `anslen`) at `answer`; its whole length, or -1.
    pub fn net_res_send(&self, msg: u32, msglen: u32, answer: u32, anslen: u32) -> u32 {
        if msglen as usize > super::resolver::MAX_UDP_MESSAGE { return u32::MAX; }
        let Some(m) = self.guest_read_buffer(msg, msglen as usize) else { return u32::MAX };
        let Some(a) = self.res_exchange(&m) else { return u32::MAX };
        let n = a.len().min(anslen as usize);
        if self.guest_write_bytes(answer, &a[..n]).is_none() { return u32::MAX; }
        a.len() as u32
    }

    /// One query for `name`: the answer's length at `answer`, or -1 with
    /// `h_errno` (`HOST_NOT_FOUND` for NXDOMAIN, `NO_DATA` for none of the
    /// type, `TRY_AGAIN` without an answer, `NO_RECOVERY` for other errors).
    fn res_query_name(&self, name: &str, class: u32, qtype: u32, answer: u32, anslen: u32) -> u32 {
        let id = {
            let mut sockets = self.shared.socket_mgr.lock_or_recover();
            sockets.query_id = sockets.query_id.wrapping_add(1);
            sockets.query_id
        };
        let set = |e: u32| { self.shared.socket_mgr.lock_or_recover().h_errno = e; u32::MAX };
        let Some(q) = super::resolver::make_query(id, name.as_bytes(), class as u16, qtype as u16) else { return set(NO_RECOVERY) };
        let Some(a) = self.res_exchange(&q) else { return set(TRY_AGAIN) };
        let n = a.len().min(anslen as usize);
        if self.guest_write_bytes(answer, &a[..n]).is_none() { return set(NO_RECOVERY); }
        match super::resolver::answer_status(&a, id) {
            None => set(TRY_AGAIN),
            Some((super::resolver::RCODE_NXDOMAIN, _)) => set(HOST_NOT_FOUND),
            Some((0, 0)) => set(NO_DATA),
            Some((0, _)) => a.len() as u32,
            Some(_) => set(TRY_AGAIN),
        }
    }

    /// res_query(dname, class, type, answer, anslen).
    pub fn net_res_query(&self, dname: u32, class: u32, qtype: u32, answer: u32, anslen: u32) -> u32 {
        let name = self.read_guest_string(dname);
        self.res_query_name(&name, class, qtype, answer, anslen)
    }

    /// res_search(dname, class, type, answer, anslen): res_query on each
    /// name of the search list (`resolver::search_names`); the first answer.
    pub fn net_res_search(&self, dname: u32, class: u32, qtype: u32, answer: u32, anslen: u32) -> u32 {
        let name = self.read_guest_string(dname);
        let (_, search) = super::resolver::host_resolver();
        for candidate in super::resolver::search_names(&name, &search) {
            let r = self.res_query_name(&candidate, class, qtype, answer, anslen);
            if r != u32::MAX { return r; }
        }
        u32::MAX
    }

    /// res_querydomain(name, domain, class, type, answer, anslen): res_query
    /// of `name.domain` (`name` when `domain` is NULL).
    pub fn net_res_querydomain(&self, a: &[u32; 10]) -> u32 {
        let name = self.read_guest_string(a[0]);
        let full = if a[1] == 0 { name } else { format!("{}.{}", name.trim_end_matches('.'), self.read_guest_string(a[1])) };
        self.res_query_name(&full, a[2], a[3], a[4], a[5])
    }

    /// dn_expand: unpack the name at `comp` of the message `[msg, eom)`
    /// into `exp` (at most `length` bytes with the NUL); the bytes the
    /// name takes at `comp`, or -1.
    pub fn net_dn_expand(&self, msg: u32, eom: u32, comp: u32, exp: u32, length: u32) -> u32 {
        const FAIL: u32 = u32::MAX;
        if comp < msg || comp >= eom || eom - msg > super::tcpip::MAX_MESSAGE { return FAIL; }
        let Some(bytes) = (0..eom - msg).map(|i| self.guest_read::<u8>(msg + i)).collect::<Option<Vec<u8>>>() else { return FAIL };
        let Some((mut name, used)) = expand_name(&bytes, (comp - msg) as usize) else { return FAIL };
        name.push(0);
        if name.len() > length as usize || self.guest_write_bytes(exp, &name).is_none() { return FAIL; }
        used as u32
    }

    /// dn_skipname: the bytes of the packed name at `comp` before `eom`, or -1.
    pub fn net_dn_skipname(&self, comp: u32, eom: u32) -> u32 {
        if comp >= eom || eom - comp > super::tcpip::MAX_MESSAGE { return u32::MAX; }
        let Some(bytes) = (0..eom - comp).map(|i| self.guest_read::<u8>(comp + i)).collect::<Option<Vec<u8>>>() else { return u32::MAX };
        skip_name(&bytes, 0).map_or(u32::MAX, |n| n as u32)
    }

    /// dn_comp: pack the dotted name at `exp` into `comp` (at most `length`
    /// bytes); the packed length, or -1. Warpine does not compress: the
    /// name is written in full (valid RFC 1035), and with a `dnptrs` list
    /// it is added there when it begins with a label and room is left.
    pub fn net_dn_comp(&self, exp: u32, comp: u32, length: u32, dnptrs: u32, lastdnptr: u32) -> u32 {
        let Some(packed) = self.net_string(exp).and_then(|s| pack_name(&s)) else { return u32::MAX };
        if packed.len() > length as usize || self.guest_write_bytes(comp, &packed).is_none() { return u32::MAX; }
        if dnptrs != 0 && packed.len() > 1 {
            // Append comp before the first NULL entry, if that is before lastdnptr.
            let mut slot = dnptrs + 4;
            while slot < lastdnptr {
                match self.guest_read::<u32>(slot) {
                    Some(0) => {
                        let _ = self.guest_write::<u32>(slot, comp);
                        if slot + 4 < lastdnptr { let _ = self.guest_write::<u32>(slot + 4, 0); }
                        break;
                    }
                    Some(_) => slot += 4,
                    None => break,
                }
            }
        }
        packed.len() as u32
    }

    /// _getshort / _getlong: a big-endian 16- / 32-bit value at `p`.
    pub fn net_get_be(&self, p: u32, size: u32) -> u32 {
        match size {
            2 => self.guest_read::<u16>(p).map_or(0, |v| u32::from(u16::from_be(v))),
            _ => self.guest_read::<u32>(p).map_or(0, u32::from_be),
        }
    }

    /// putshort / putlong: store `v` big-endian at `p`.
    pub fn net_put_be(&self, v: u32, p: u32, size: u32) -> u32 {
        match size {
            2 => { let _ = self.guest_write::<u16>(p, (v as u16).to_be()); }
            _ => { let _ = self.guest_write::<u32>(p, v.to_be()); }
        }
        0
    }

    /// ioctl (SO32DLL): `FIONBIO`, `FIONREAD` and `SIOCATMARK` on a socket;
    /// other requests fail with `SOCEINVAL`.
    pub fn net_ioctl(&self, s: u32, cmd: u32, data: u32) -> u32 {
        use super::tcpip::{SOCE_FAULT, SOCE_INVAL};
        debug!("ioctl(s=0x{s:X}, cmd=0x{cmd:08X})");
        let Some(fd) = self.socket_fd(s) else { return u32::MAX };
        let Some(req) = socket_ioctl(cmd) else { return self.socket_failure(SOCE_INVAL) };
        let result = match req {
            SocketIoctl::NonBlocking => {
                let Some(on) = self.guest_read::<u32>(data) else { return self.socket_failure(SOCE_FAULT) };
                let mut flag: libc::c_int = i32::from(on != 0);
                // SAFETY: FIONBIO reads one int from a live local.
                unsafe { libc::ioctl(fd, libc::FIONBIO, &mut flag) }
            }
            SocketIoctl::BytesWaiting | SocketIoctl::AtMark => {
                let mut value: libc::c_int = 0;
                let request = if req == SocketIoctl::BytesWaiting { libc::FIONREAD } else { LINUX_SIOCATMARK };
                // SAFETY: both requests write one int to a live local.
                let rc = unsafe { libc::ioctl(fd, request, &mut value) };
                if rc == 0 && self.guest_write::<i32>(data, value).is_none() { return self.socket_failure(SOCE_FAULT); }
                rc
            }
        };
        self.socket_result(result)
    }

    /// The guest `iovec`s at `iov`: (base, len) each, at most `MAX_IOVEC`
    /// and `MAX_SOCKET_BUF` bytes in all.
    fn net_iovecs(&self, iov: u32, count: u32) -> Option<Vec<(u32, u32)>> {
        if count == 0 || count > MAX_IOVEC { return None; }
        let mut out = Vec::new();
        let mut total = 0u64;
        for i in 0..count {
            let at = iov.checked_add(i * IOVEC_SIZE)?;
            let (base, len) = (self.guest_read::<u32>(at)?, self.guest_read::<u32>(at + 4)?);
            total += u64::from(len);
            if total > super::tcpip::MAX_SOCKET_BUF as u64 { return None; }
            out.push((base, len));
        }
        Some(out)
    }

    /// writev: send the `iovec`s' bytes as one `send`.
    pub fn net_writev(&self, s: u32, iov: u32, count: u32) -> u32 {
        let Some(vecs) = self.net_iovecs(iov, count) else { return self.socket_failure(super::tcpip::SOCE_INVAL) };
        let mut data = Vec::new();
        for (base, len) in vecs {
            let Some(part) = (0..len).map(|i| self.guest_read::<u8>(base.checked_add(i)?)).collect::<Option<Vec<u8>>>() else {
                return self.socket_failure(super::tcpip::SOCE_FAULT);
            };
            data.extend_from_slice(&part);
        }
        self.socket_send_bytes(s, &data)
    }

    /// readv: one `recv` of up to the `iovec`s' total, spread over them.
    pub fn net_readv(&self, s: u32, iov: u32, count: u32) -> u32 {
        let Some(vecs) = self.net_iovecs(iov, count) else { return self.socket_failure(super::tcpip::SOCE_INVAL) };
        let total: u32 = vecs.iter().map(|v| v.1).sum();
        let data = match self.socket_recv_bytes(s, total as usize) {
            Ok(d) => d,
            Err(rc) => return rc,
        };
        let mut rest = data.as_slice();
        for (base, len) in vecs {
            let (part, tail) = rest.split_at(rest.len().min(len as usize));
            if self.guest_write_bytes(base, part).is_none() { return self.socket_failure(super::tcpip::SOCE_FAULT); }
            rest = tail;
            if rest.is_empty() { break; }
        }
        data.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inet_addr_forms() {
        assert_eq!(parse_inet_addr(b"192.168.1.2"), Some(0xC0A8_0102));
        assert_eq!(parse_inet_addr(b"10.1"), Some(0x0A00_0001), "b fills 24 bits");
        assert_eq!(parse_inet_addr(b"10.1.2"), Some(0x0A01_0002), "c fills 16 bits");
        assert_eq!(parse_inet_addr(b"0x7f.0.0.01"), Some(0x7F00_0001), "hex and octal");
        assert_eq!(parse_inet_addr(b"3232235777"), Some(0xC0A8_0101), "one number");
        assert_eq!(parse_inet_addr(b"256.1.1.1"), None);
        assert_eq!(parse_inet_addr(b"1.2.3.4.5"), None);
        assert_eq!(parse_inet_addr(b"1..2"), None);
        assert_eq!(parse_inet_addr(b"08"), None, "8 is no octal digit");
        assert_eq!(parse_inet_addr(b""), None);
    }

    #[test]
    fn inet_network_and_classes() {
        assert_eq!(parse_inet_network(b"10.1"), Some(0x0A01));
        assert_eq!(parse_inet_network(b"128.2.0"), Some(0x0080_0200));
        assert_eq!(parse_inet_network(b"300"), None);
        assert_eq!(make_addr(10, 5), 0x0A00_0005);
        assert_eq!(make_addr(0x8002, 0x0304), 0x8002_0304);
        assert_eq!(make_addr(0xC0A801, 7), 0xC0A8_0107);
        assert_eq!(split_addr(0x0A00_0005), (5, 10));
        assert_eq!(split_addr(0x8002_0304), (0x0304, 0x8002));
        assert_eq!(split_addr(0xC0A8_0107), (7, 0xC0A801));
        assert_eq!(dotted(0x7F00_0001), "127.0.0.1");
    }

    #[test]
    fn names_pack_and_unpack() {
        let packed = pack_name(b"www.example.com.").unwrap();
        assert_eq!(packed, b"\x03www\x07example\x03com\x00");
        assert_eq!(pack_name(b"").unwrap(), b"\x00", "the root");
        assert_eq!(pack_name(b"a..b"), None);
        assert_eq!(pack_name(&[b'x'; 64]), None, "a label over 63 bytes");
        // A message with the name at 0 and a pointer to its "example.com" at 13.
        let mut msg = packed.clone();
        msg.extend_from_slice(b"\x04mail\xC0\x04");
        assert_eq!(expand_name(&msg, 0), Some((b"www.example.com".to_vec(), 17)));
        assert_eq!(expand_name(&msg, 17), Some((b"mail.example.com".to_vec(), 7)));
        assert_eq!(skip_name(&msg, 17), Some(7));
        assert_eq!(skip_name(&msg, 0), Some(17));
        // A pointer to itself or forwards is refused, as is a cut message.
        assert_eq!(expand_name(b"\xC0\x00", 0), None);
        assert_eq!(expand_name(b"\x03ab", 0), None);
        assert_eq!(skip_name(b"\x03abc", 0), None);
    }

    #[test]
    fn protocols_and_ioctl_requests() {
        assert_eq!(protocol_by_name(b"tcp").map(|i| PROTOCOLS[i].2), Some(6));
        assert_eq!(protocol_by_name(b"UDP").map(|i| PROTOCOLS[i].2), Some(17));
        assert_eq!(protocol_by_name(b"xns"), None);
        assert_eq!(protocol_by_number(1).map(|i| PROTOCOLS[i].0), Some("icmp"));
        // The BSD 4.4 header's values, and a BSD 4.3-style form.
        assert_eq!(socket_ioctl(0x8004_667E), Some(SocketIoctl::NonBlocking));
        assert_eq!(socket_ioctl(0x4004_667F), Some(SocketIoctl::BytesWaiting));
        assert_eq!(socket_ioctl(0x4004_7307), Some(SocketIoctl::AtMark));
        assert_eq!(socket_ioctl(0x667E), Some(SocketIoctl::NonBlocking));
        assert_eq!(socket_ioctl(0x4004_6601), None);
    }

    /// tcp_h_errno reports a failed lookup; setservent starts the services
    /// walk over (the host's database may be empty).
    #[test]
    fn resolver_error_and_service_walk() {
        let loader = super::super::Loader::new_mock();
        assert_eq!(loader.net_tcp_h_errno(), 0);
        assert_eq!(loader.so_gethostbyaddr(0, 4, 99), 0);
        assert_eq!(loader.net_tcp_h_errno(), HOST_NOT_FOUND);
        loader.net_setservent();
        let first = loader.net_getservent();
        let name = |se: u32| (se != 0).then(|| loader.guest_read::<u32>(se).unwrap()).map(|p| loader.net_string(p).unwrap());
        let first_name = name(first);
        loader.net_setservent();
        assert_eq!(name(loader.net_getservent()), first_name);
        loader.net_endservent();
        assert!(loader.shared.socket_mgr.lock_or_recover().services.is_none());
    }
}
