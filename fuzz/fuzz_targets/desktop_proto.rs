// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop protocol's decoders (ADR-0052): the server decodes every
//! client's packets and every client the server's, so a hostile program
//! that took over its Warpine process could send anything. A decoded
//! message must also encode and decode to itself.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/desktop/proto.rs"]
#[allow(dead_code)]
mod proto;

/// What proto.rs takes from the rest of Warpine (src/loader/pm_types.rs,
/// src/loader/constants.rs): the same items, with the same values.
mod loader {
    pub mod pm_types {
        pub const HWND_SLOT_SHIFT: u32 = 20;
        pub const MAX_HANDLE_SLOTS: u32 = 1023;
        pub fn handle_slot(hwnd: u32) -> Option<u32> {
            (hwnd >> HWND_SLOT_SHIFT).checked_sub(1).filter(|&s| s < MAX_HANDLE_SLOTS)
        }
    }
    pub mod constants {
        pub const SWP_MOVE: u32 = 0x0002;
        pub const SWP_MINIMIZE: u32 = 0x0400;
        pub const SWP_MAXIMIZE: u32 = 0x0800;
        pub const SWP_RESTORE: u32 = 0x1000;
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = proto::ClientMsg::decode(data) {
        assert_eq!(proto::ClientMsg::decode(&m.encode()).as_ref(), Ok(&m), "a client message round-trips");
    }
    if let Ok(m) = proto::ServerMsg::decode(data) {
        assert_eq!(proto::ServerMsg::decode(&m.encode()).as_ref(), Ok(&m), "a server message round-trips");
    }
});
