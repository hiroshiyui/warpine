// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Address strings and domain names, the pure parsers behind TCP/IP's
//! `inet_addr`, `inet_network`, `inet_makeaddr`, `inet_lnaof` /
//! `inet_netof`, `inet_ntoa` and the resolver's name packing (`dn_comp`,
//! `dn_expand`, `dn_skipname`; RFC 1035 §4.1.4). Kept apart from
//! `tcpip_util.rs`'s guest-memory calls so the fuzz target
//! (`fuzz/fuzz_targets/resolver.rs`) can include them by `#[path]`: a DNS
//! answer comes from the network and a name from the program, so neither
//! is trusted.

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
                return Some((out, consumed.unwrap_or_else(|| at + 1 - pos)));
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
                consumed.get_or_insert_with(|| at + 2 - pos);
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
        // A pointer back to the root two bytes before it: the pointer's
        // two bytes are what the name takes (found by the resolver fuzz
        // target: the length was worked out from the root's offset too,
        // which underflowed).
        assert_eq!(expand_name(b"\x00\x00\xC0\x00", 2), Some((Vec::new(), 2)));
        // A pointer to a pointer further back: still the first pointer's two.
        assert_eq!(expand_name(b"\x00\xC0\x00\x00\xC0\x01", 4), Some((Vec::new(), 2)));
    }
}
