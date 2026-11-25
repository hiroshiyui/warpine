// SPDX-License-Identifier: GPL-3.0-or-later
//
//! TCP/IP's host and network databases and the resolver (TCP32DLL;
//! Warpine's own ordinals, ADR-0123), after OS/2 TCP/IP 4.0's `netdb.h` /
//! `resolv.h` and BIND 4's resolver they follow:
//!
//! - `sethostent` / `gethostent` / `endhostent` walk the host's
//!   `/etc/hosts`, `setnetent` / `getnetent` / `endnetent`,
//!   `getnetbyname` / `getnetbyaddr` its `/etc/networks` — the host's
//!   databases, as the services and `gethostbyname` already are (Warpine's
//!   choice: OS/2 reads `%ETC%\HOSTS` and `%ETC%\NETWORKS`). IPv4 only.
//! - `res_mkquery` builds a standard query (RFC 1035 §4.1); `res_send`
//!   sends a message over UDP to the host's name servers (`/etc/resolv.conf`,
//!   port 53, two rounds, five seconds each) and returns the answer;
//!   `res_query`, `res_search` (the search list for a name without a dot)
//!   and `res_querydomain` build, send and check the answer, setting
//!   `h_errno`. `res_init` reads nothing ahead of time: every call reads
//!   `/etc/resolv.conf` again.
//!
//! The parsers here are pure and bounded; everything read is treated as
//! untrusted.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use super::inet_names::{pack_name, parse_inet_network};

/// Most bytes of a host database file read.
pub const MAX_DB_FILE: usize = 256 * 1024;
/// Most entries kept from one file.
pub const MAX_DB_ENTRIES: usize = 4096;
/// Longest name kept.
pub const MAX_DB_NAME: usize = 255;
/// Largest DNS message over UDP (RFC 1035 §2.3.4).
pub const MAX_UDP_MESSAGE: usize = 512;
/// The most name servers and search domains used.
const MAX_SERVERS: usize = 3;
const MAX_SEARCH: usize = 6;
/// The DNS port, the wait per server and the rounds over the servers.
const DNS_PORT: u16 = 53;
const SERVER_WAIT: Duration = Duration::from_secs(5);
const ROUNDS: usize = 2;
/// Header fields (RFC 1035 §4.1.1).
const HEADER_LEN: usize = 12;
const FLAG_RD: u16 = 0x0100;
const RCODE_MASK: u8 = 0x0F;
pub const RCODE_NXDOMAIN: u8 = 3;
/// `res_mkquery`'s only op Warpine builds.
pub const QUERY: u32 = 0;

/// The lines of `text` without comments, split into fields.
fn fields(text: &[u8]) -> impl Iterator<Item = Vec<&[u8]>> {
    let text = &text[..text.len().min(MAX_DB_FILE)];
    text.split(|&b| b == b'\n').filter_map(|line| {
        let line = line.split(|&b| b == b'#').next().unwrap_or(&[]);
        let f: Vec<&[u8]> = line.split(|b| b.is_ascii_whitespace()).filter(|f| !f.is_empty()).collect();
        (!f.is_empty()).then_some(f)
    })
}

fn name_of(field: &[u8]) -> Option<String> {
    (field.len() <= MAX_DB_NAME && field.iter().all(|b| b.is_ascii_graphic()))
        .then(|| String::from_utf8_lossy(field).into_owned())
}

/// `/etc/hosts`: the IPv4 entries, each its address and first name.
pub fn parse_hosts(text: &[u8]) -> Vec<(String, [u8; 4])> {
    fields(text).filter_map(|f| {
        let ip: Ipv4Addr = std::str::from_utf8(f[0]).ok()?.parse().ok()?;
        Some((name_of(f.get(1)?)?, ip.octets()))
    }).take(MAX_DB_ENTRIES).collect()
}

/// `/etc/networks`: each network's name and number (host order, as
/// `inet_network` gives it).
pub fn parse_networks(text: &[u8]) -> Vec<(String, u32)> {
    fields(text).filter_map(|f| {
        Some((name_of(f[0])?, parse_inet_network(f.get(1)?)?))
    }).take(MAX_DB_ENTRIES).collect()
}

/// `/etc/resolv.conf`: the IPv4 name servers and the search list
/// (`search`, else `domain`).
pub fn parse_resolv_conf(text: &[u8]) -> (Vec<Ipv4Addr>, Vec<String>) {
    let (mut servers, mut search, mut domain) = (Vec::new(), None, None);
    for f in fields(text) {
        match f[0] {
            b"nameserver" => if let Some(ip) = f.get(1).and_then(|a| std::str::from_utf8(a).ok()?.parse().ok())
                && servers.len() < MAX_SERVERS {
                servers.push(ip);
            },
            b"search" => search = Some(f[1..].iter().filter_map(|d| name_of(d)).take(MAX_SEARCH).collect::<Vec<_>>()),
            b"domain" => domain = f.get(1).and_then(|d| name_of(d)),
            _ => {}
        }
    }
    (servers, search.unwrap_or_else(|| domain.into_iter().collect()))
}

/// A standard query for `name` (RFC 1035 §4.1): header `id`, recursion
/// desired, one question of `qtype` / `qclass`. None for a name that
/// cannot be packed.
pub fn make_query(id: u16, name: &[u8], qclass: u16, qtype: u16) -> Option<Vec<u8>> {
    let mut m = Vec::with_capacity(HEADER_LEN + name.len() + 6);
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&FLAG_RD.to_be_bytes());
    m.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    m.extend_from_slice(&[0; 6]); // ANCOUNT, NSCOUNT, ARCOUNT
    m.extend_from_slice(&pack_name(name)?);
    m.extend_from_slice(&qtype.to_be_bytes());
    m.extend_from_slice(&qclass.to_be_bytes());
    (m.len() <= MAX_UDP_MESSAGE).then_some(m)
}

/// An answer's response code and answer count; None when it is shorter
/// than a header or not a response to message `id`.
pub fn answer_status(answer: &[u8], id: u16) -> Option<(u8, u16)> {
    if answer.len() < HEADER_LEN || answer[0..2] != id.to_be_bytes() || answer[2] & 0x80 == 0 { return None; }
    Some((answer[3] & RCODE_MASK, u16::from_be_bytes([answer[6], answer[7]])))
}

/// The names `res_search` tries for `name`: as it is when it has a dot,
/// then with each search domain (also as it is last, when it has none).
pub fn search_names(name: &str, search: &[String]) -> Vec<String> {
    let base = name.trim_end_matches('.');
    if name.ends_with('.') { return vec![base.to_string()]; }
    let mut out = Vec::new();
    if name.contains('.') { out.push(base.to_string()); }
    out.extend(search.iter().map(|d| format!("{base}.{d}")));
    if !name.contains('.') { out.push(base.to_string()); }
    out
}

/// Send `msg` to each of `servers` in turn, `ROUNDS` times over, and
/// return the first reply that answers it (same id). Each server gets
/// `wait` in all: stray packets do not extend it (security audit
/// 2026-11-25: a timeout per receive let a flood keep a call waiting).
pub fn exchange(msg: &[u8], servers: &[SocketAddr], wait: Duration) -> Option<Vec<u8>> {
    if msg.len() < HEADER_LEN || msg.len() > MAX_UDP_MESSAGE { return None; }
    let id = u16::from_be_bytes([msg[0], msg[1]]);
    for _ in 0..ROUNDS {
        for server in servers {
            let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { continue };
            if sock.send_to(msg, server).is_err() { continue; }
            let deadline = std::time::Instant::now() + wait;
            let mut buf = [0u8; MAX_UDP_MESSAGE];
            loop {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() || sock.set_read_timeout(Some(left)).is_err() { break; }
                let Ok((n, from)) = sock.recv_from(&mut buf) else { break };
                if from == *server && n >= HEADER_LEN && buf[0..2] == id.to_be_bytes() {
                    return Some(buf[..n].to_vec());
                }
            }
        }
    }
    None
}

/// A query id nobody off the path can guess (std's randomly keyed hasher):
/// with the source port, what keeps a forged answer from being taken.
pub fn random_query_id() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(0);
    h.finish() as u16
}

/// The host's name servers (port 53) and search list.
pub fn host_resolver() -> (Vec<SocketAddr>, Vec<String>) {
    let text = read_bounded("/etc/resolv.conf");
    let (servers, search) = parse_resolv_conf(&text);
    (servers.into_iter().map(|ip| SocketAddr::from((ip, DNS_PORT))).collect(), search)
}

/// The first `MAX_DB_FILE` bytes of host file `path` (empty when unreadable).
pub fn read_bounded(path: &str) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(MAX_DB_FILE as u64).read_to_end(&mut out);
    }
    out
}

/// How long one server is waited for.
pub fn server_wait() -> Duration { SERVER_WAIT }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_networks_are_parsed_and_bounded() {
        let hosts = parse_hosts(b"# c\n127.0.0.1 localhost lh\n::1 ip6-localhost\n10.0.0.2\tbox # x\nbad line\n");
        assert_eq!(hosts, [("localhost".to_string(), [127, 0, 0, 1]), ("box".to_string(), [10, 0, 0, 2])]);
        let nets = parse_networks(b"loopback 127\nlink-local 169.254.0.0\nbad\n");
        assert_eq!(nets, [("loopback".to_string(), 127), ("link-local".to_string(), 0xA9FE_0000)]);
        let many = "1.2.3.4 h\n".repeat(MAX_DB_ENTRIES + 10);
        assert_eq!(parse_hosts(many.as_bytes()).len(), MAX_DB_ENTRIES);
        assert!(parse_hosts(format!("1.2.3.4 {}\n", "x".repeat(MAX_DB_NAME + 1)).as_bytes()).is_empty());
    }

    #[test]
    fn resolv_conf_gives_servers_and_the_search_list() {
        let (s, d) = parse_resolv_conf(b"nameserver 10.0.0.1\nnameserver ::1\nnameserver 10.0.0.2\nsearch a.example b.example\n");
        assert_eq!(s, [Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 0, 0, 2)]);
        assert_eq!(d, ["a.example", "b.example"]);
        let (_, d) = parse_resolv_conf(b"domain c.example\n");
        assert_eq!(d, ["c.example"]);
    }

    #[test]
    fn queries_follow_rfc_1035() {
        let q = make_query(0x1234, b"a.bc", 1, 1).unwrap();
        assert_eq!(q, [0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0, 1, b'a', 2, b'b', b'c', 0, 0, 1, 0, 1]);
        assert_eq!(make_query(1, &[b'x'; 64], 1, 1), None, "a label over 63 bytes");
        let mut a = q.clone();
        a[2] |= 0x80;
        a[3] = 0x83;
        a[7] = 2;
        assert_eq!(answer_status(&a, 0x1234), Some((RCODE_NXDOMAIN, 2)));
        assert_eq!(answer_status(&q, 0x1234), None, "not a response");
        assert_eq!(answer_status(&a, 0x9999), None, "another id");
    }

    #[test]
    fn search_tries_the_domains() {
        let s = ["a.example".to_string(), "b.example".to_string()];
        assert_eq!(search_names("host", &s), ["host.a.example", "host.b.example", "host"]);
        assert_eq!(search_names("w.org", &s), ["w.org", "w.org.a.example", "w.org.b.example"]);
        assert_eq!(search_names("abs.", &s), ["abs"]);
    }

    /// A reply from a local server answers; one with another id does not.
    #[test]
    fn exchange_takes_the_matching_reply() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = server.recv_from(&mut buf).unwrap();
            let mut wrong = buf[..n].to_vec();
            wrong[0] ^= 0xFF;
            server.send_to(&wrong, from).unwrap();
            buf[2] |= 0x80;
            server.send_to(&buf[..n], from).unwrap();
        });
        let q = make_query(0x4242, b"x.test", 1, 1).unwrap();
        let a = exchange(&q, &[addr], Duration::from_secs(2)).unwrap();
        assert_eq!(answer_status(&a, 0x4242), Some((0, 0)));
        t.join().unwrap();
        assert_eq!(exchange(&q[..4], &[addr], Duration::from_millis(10)), None, "not a message");
    }

    /// Stray replies do not extend the wait (security audit 2026-11-25):
    /// a server sending wrong ids without end gives up after the wait.
    #[test]
    fn stray_replies_do_not_extend_the_wait() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = stop.clone();
        let t = std::thread::spawn(move || {
            server.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
            let mut buf = [0u8; 512];
            let mut client = None;
            while !s.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok((n, from)) = server.recv_from(&mut buf) { client = Some((from, n)); }
                if let Some((from, n)) = client {
                    let mut wrong = buf[..n].to_vec();
                    wrong[0] ^= 0xFF;
                    let _ = server.send_to(&wrong, from);
                }
            }
        });
        let q = make_query(random_query_id(), b"x.test", 1, 1).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(exchange(&q, &[addr], Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(3), "gave up after the wait ({:?})", started.elapsed());
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        t.join().unwrap();
        let ids: std::collections::HashSet<u16> = (0..16).map(|_| random_query_id()).collect();
        assert!(ids.len() > 1, "query ids vary");
    }
}
