// SPDX-License-Identifier: GPL-3.0-or-later
//! The resolver's parsers (src/loader/resolver.rs, src/loader/inet_names.rs):
//! a DNS answer comes from the network, the host's `/etc/hosts`,
//! `/etc/networks` and `/etc/resolv.conf` are outside Warpine's control, and
//! the names and address strings come from the program. The first byte picks
//! the input's kind: 0 a DNS message (every name in it expanded and skipped,
//! the header checked), 1 the hosts file, 2 the networks file, 3
//! `resolv.conf`, 4 a name or address string (packed and expanded back,
//! parsed and printed back).
#![no_main]

use libfuzzer_sys::fuzz_target;

/// The tree resolver.rs expects: its `super::inet_names`, both read from
/// src/loader/.
#[path = "../../src/loader"]
mod loader {
    #[allow(dead_code)]
    pub mod inet_names;
    #[allow(dead_code)]
    pub mod resolver;
}

use loader::inet_names::*;
use loader::resolver::*;

fn message(msg: &[u8]) {
    if let Some((rcode, _count)) = answer_status(msg, u16::from_be_bytes([msg[0], msg[1]])) {
        assert!(rcode <= 0x0F);
    }
    for pos in 0..msg.len() {
        let skipped = skip_name(msg, pos);
        if let Some((name, used)) = expand_name(msg, pos) {
            assert!(name.len() <= MAX_DNAME, "expanded name too long");
            assert_eq!(skipped, Some(used), "dn_skipname agrees with dn_expand");
            assert!(pos + used <= msg.len());
        }
    }
}

fn name(text: &[u8]) {
    if let Some(packed) = pack_name(text) {
        assert!(packed.len() <= MAX_DNAME + 1);
        let base = text.strip_suffix(b".").unwrap_or(text);
        assert_eq!(expand_name(&packed, 0), Some((base.to_vec(), packed.len())), "round trip");
        if let Some(q) = make_query(0x1234, text, 1, 1) {
            assert!(q.len() <= MAX_UDP_MESSAGE);
            assert_eq!(answer_status(&q, 0x1234), None, "a query is no answer");
            assert_eq!(skip_name(&q, 12), Some(packed.len()));
        }
    }
    if let Some(addr) = parse_inet_addr(text) {
        assert_eq!(parse_inet_addr(dotted(addr).as_bytes()), Some(addr));
    }
    if let Ok(s) = std::str::from_utf8(text) {
        let search = ["a.example".to_string(), "b".to_string()];
        assert!(search_names(s, &search).len() <= search.len() + 1);
    }
    let _ = parse_inet_network(text);
}

fuzz_target!(|data: &[u8]| {
    let Some((&kind, rest)) = data.split_first() else { return };
    match kind {
        0 if rest.len() >= 2 => message(&rest[..rest.len().min(MAX_UDP_MESSAGE)]),
        1 => {
            let hosts = parse_hosts(rest);
            assert!(hosts.len() <= MAX_DB_ENTRIES);
            assert!(hosts.iter().all(|(n, _)| !n.is_empty() && n.len() <= MAX_DB_NAME));
        }
        2 => {
            let nets = parse_networks(rest);
            assert!(nets.len() <= MAX_DB_ENTRIES);
            assert!(nets.iter().all(|(n, _)| !n.is_empty() && n.len() <= MAX_DB_NAME));
        }
        3 => {
            let (servers, search) = parse_resolv_conf(rest);
            assert!(servers.len() <= 3 && search.len() <= 6);
        }
        4 => name(rest),
        _ => {}
    }
});
