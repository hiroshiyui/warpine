// SPDX-License-Identifier: GPL-3.0-only

//! The session's shell services, kept by the desktop server (ADR-0056):
//! the switch list (the window list's entries) and the system atom table.
//! Pure bookkeeping, without sockets, so the rules can be tested on their
//! own.
//!
//! - **Switch list.** Every program of the session adds, changes and
//!   removes its own entries and sees everyone's. An entry belongs to the
//!   program that added it: only that program may change or remove it, and
//!   its entries go when it leaves. A `FCF_TASKLIST` frame's entry (`auto`)
//!   also goes with the frame. At most [`MAX_SWITCHES`] entries in all and
//!   [`MAX_SWITCHES_PER_CLIENT`] per program (`PMERR_NO_SPACE`), so one
//!   program cannot fill the session's list.
//! - **System atom table.** One table for the session; names are text,
//!   compared without regard to ASCII case (`pm_atom::AtomTable`). Atoms
//!   stay when the program that added them leaves, as in PM, where the
//!   system atom table lives as long as the system.
//!
//! Warpine's choices (unverified against OS/2): only an entry's own program
//! may change or remove it (`PMERR_INVALID_SWITCH_HANDLE` for another);
//! the limits above.

use std::collections::BTreeMap;

use super::model::ClientId;
use super::proto::{SwitchEntry, WinCall, WinReply, MAX_SWITCHES};
use crate::loader::pm_atom::{AtomName, AtomTable};
use crate::loader::pm_switch::{PMERR_INVALID_SWITCH_HANDLE, PMERR_NO_HANDLE, PMERR_NO_SPACE};

/// Most switch-list entries one program may have.
pub const MAX_SWITCHES_PER_CLIENT: usize = 64;
/// The first switch handle handed out.
const FIRST_SWITCH_HANDLE: u32 = 0x0001_0001;

/// A switch-list entry and the program it belongs to.
#[derive(Debug, Clone)]
struct Owned {
    owner: ClientId,
    /// A `FCF_TASKLIST` frame's entry, removed with the frame.
    auto: bool,
    entry: SwitchEntry,
}

/// The session's switch list and system atom table.
#[derive(Debug)]
pub struct Services {
    switches: BTreeMap<u32, Owned>,
    next: u32,
    atoms: AtomTable,
}

impl Default for Services {
    fn default() -> Self {
        Services { switches: BTreeMap::new(), next: FIRST_SWITCH_HANDLE, atoms: AtomTable::default() }
    }
}

impl Services {
    pub fn new() -> Services {
        Services::default()
    }

    /// Answer client `caller`'s `call` on the session's services; `Failed`
    /// for a call that is not about them.
    pub fn answer(&mut self, caller: ClientId, call: &WinCall) -> WinReply {
        let value = |r: Result<u32, u32>| r.map_or_else(WinReply::Error, WinReply::Value);
        match call {
            WinCall::SwitchAdd { entry, auto } => value(self.add(caller, entry.clone(), *auto)),
            WinCall::SwitchChange { entry } => value(self.owned_mut(caller, entry.handle).map(|o| {
                o.entry = SwitchEntry { handle: entry.handle, ..entry.clone() };
                0
            })),
            WinCall::SwitchRemove { handle } => value(self.owned_mut(caller, *handle).map(|_| 0).inspect(|_| {
                self.switches.remove(handle);
            })),
            WinCall::SwitchQuery { handle } => match self.switches.get(handle) {
                Some(o) => WinReply::Switches(vec![o.entry.clone()]),
                None => WinReply::Error(PMERR_INVALID_SWITCH_HANDLE),
            },
            WinCall::SwitchList => WinReply::Switches(self.switches.values().map(|o| o.entry.clone()).collect()),
            WinCall::SwitchHandle { hwnds, pid } => value(self.handle_for(hwnds, *pid).ok_or(PMERR_NO_HANDLE)),
            WinCall::SwitchForget { hwnds } => {
                self.switches.retain(|_, o| !(o.owner == caller && o.auto && hwnds.contains(&o.entry.hwnd)));
                WinReply::Value(1)
            }
            WinCall::AtomAdd { name } => value(self.atoms.add(&AtomName::from_bytes(name.as_bytes()))),
            WinCall::AtomFind { name } => value(self.atoms.find(&AtomName::from_bytes(name.as_bytes()))),
            WinCall::AtomDelete { atom } => value(self.atoms.delete(*atom).map(|()| 0)),
            WinCall::AtomUsage { atom } => value(self.atoms.usage(*atom)),
            WinCall::AtomName { atom } => match self.atoms.name(*atom) {
                Ok(n) => WinReply::Text(String::from_utf8_lossy(&n).into_owned()),
                Err(code) => WinReply::Error(code),
            },
            _ => WinReply::Failed,
        }
    }

    /// Client `c` left: its switch entries go.
    pub fn client_gone(&mut self, c: ClientId) {
        self.switches.retain(|_, o| o.owner != c);
    }

    /// Entries in the list.
    pub fn switch_count(&self) -> usize {
        self.switches.len()
    }

    fn add(&mut self, caller: ClientId, mut entry: SwitchEntry, auto: bool) -> Result<u32, u32> {
        let own = self.switches.values().filter(|o| o.owner == caller).count();
        if self.switches.len() >= MAX_SWITCHES || own >= MAX_SWITCHES_PER_CLIENT {
            return Err(PMERR_NO_SPACE);
        }
        while self.next == 0 || self.switches.contains_key(&self.next) {
            self.next = self.next.wrapping_add(1);
        }
        let h = self.next;
        self.next = self.next.wrapping_add(1);
        entry.handle = h;
        self.switches.insert(h, Owned { owner: caller, auto, entry });
        Ok(h)
    }

    /// Entry `h`, when it belongs to `caller`.
    fn owned_mut(&mut self, caller: ClientId, h: u32) -> Result<&mut Owned, u32> {
        self.switches.get_mut(&h).filter(|o| o.owner == caller).ok_or(PMERR_INVALID_SWITCH_HANDLE)
    }

    /// The entry of the first of `hwnds` that has one, or with no windows
    /// the first entry of process `pid`.
    fn handle_for(&self, hwnds: &[u32], pid: u32) -> Option<u32> {
        if hwnds.is_empty() {
            return self.switches.values().find(|o| o.entry.process == pid).map(|o| o.entry.handle);
        }
        hwnds.iter().find_map(|&w| self.switches.values().find(|o| o.entry.hwnd == w).map(|o| o.entry.handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::pm_atom::PMERR_INVALID_ATOM;

    fn entry(hwnd: u32, process: u32, title: &str) -> SwitchEntry {
        SwitchEntry { hwnd, process, title: title.into(), ..Default::default() }
    }

    fn handle(r: WinReply) -> u32 {
        match r {
            WinReply::Value(h) => h,
            other => panic!("a value, got {other:?}"),
        }
    }

    #[test]
    fn every_program_sees_the_entries_but_changes_only_its_own() {
        let mut s = Services::new();
        let a = handle(s.answer(1, &WinCall::SwitchAdd { entry: entry(0x20_1000, 7, "Program A"), auto: false }));
        let b = handle(s.answer(2, &WinCall::SwitchAdd { entry: entry(0x30_1000, 8, "Program B"), auto: true }));
        let WinReply::Switches(all) = s.answer(3, &WinCall::SwitchList) else { panic!() };
        assert_eq!(all.iter().map(|e| (e.handle, e.title.as_str())).collect::<Vec<_>>(),
                   vec![(a, "Program A"), (b, "Program B")]);
        let change = WinCall::SwitchChange { entry: SwitchEntry { handle: b, ..entry(0x30_1000, 8, "B2") } };
        assert_eq!(s.answer(1, &change), WinReply::Error(PMERR_INVALID_SWITCH_HANDLE), "not its entry");
        assert_eq!(s.answer(2, &change), WinReply::Value(0));
        let WinReply::Switches(q) = s.answer(1, &WinCall::SwitchQuery { handle: b }) else { panic!() };
        assert_eq!((q[0].handle, q[0].title.as_str()), (b, "B2"));
        assert_eq!(s.answer(1, &WinCall::SwitchRemove { handle: b }), WinReply::Error(PMERR_INVALID_SWITCH_HANDLE));
        assert_eq!(s.answer(1, &WinCall::SwitchHandle { hwnds: vec![9, 0x30_1000], pid: 0 }), WinReply::Value(b));
        assert_eq!(s.answer(1, &WinCall::SwitchHandle { hwnds: vec![], pid: 7 }), WinReply::Value(a));
        assert_eq!(s.answer(1, &WinCall::SwitchHandle { hwnds: vec![], pid: 99 }), WinReply::Error(PMERR_NO_HANDLE));
        assert_eq!(s.answer(1, &WinCall::SwitchRemove { handle: a }), WinReply::Value(0));
        assert_eq!(s.switch_count(), 1);
    }

    #[test]
    fn a_frames_entry_goes_with_the_frame_and_a_programs_with_the_program() {
        let mut s = Services::new();
        s.answer(1, &WinCall::SwitchAdd { entry: entry(0x20_1000, 7, "auto"), auto: true });
        s.answer(1, &WinCall::SwitchAdd { entry: entry(0x20_1000, 7, "added"), auto: false });
        s.answer(2, &WinCall::SwitchAdd { entry: entry(0x20_1000, 8, "another's"), auto: true });
        s.answer(1, &WinCall::SwitchForget { hwnds: vec![0x20_1000] });
        let WinReply::Switches(left) = s.answer(1, &WinCall::SwitchList) else { panic!() };
        assert_eq!(left.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), vec!["added", "another's"]);
        s.client_gone(1);
        assert_eq!(s.switch_count(), 1);
    }

    #[test]
    fn the_list_is_bounded_per_program_and_in_all() {
        let mut s = Services::new();
        for _ in 0..MAX_SWITCHES_PER_CLIENT {
            handle(s.answer(1, &WinCall::SwitchAdd { entry: SwitchEntry::default(), auto: false }));
        }
        assert_eq!(s.answer(1, &WinCall::SwitchAdd { entry: SwitchEntry::default(), auto: false }),
                   WinReply::Error(PMERR_NO_SPACE));
        let mut c = 2;
        while s.switch_count() < MAX_SWITCHES {
            s.answer(c, &WinCall::SwitchAdd { entry: SwitchEntry::default(), auto: false });
            if s.switch_count().is_multiple_of(MAX_SWITCHES_PER_CLIENT) { c += 1; }
        }
        assert_eq!(s.answer(99, &WinCall::SwitchAdd { entry: SwitchEntry::default(), auto: false }),
                   WinReply::Error(PMERR_NO_SPACE));
    }

    #[test]
    fn the_system_atom_table_is_shared_and_outlives_programs() {
        let mut s = Services::new();
        let atom = handle(s.answer(1, &WinCall::AtomAdd { name: "Warpine Größe".into() }));
        s.client_gone(1);
        assert_eq!(s.answer(2, &WinCall::AtomFind { name: "WARPINE Größe".into() }), WinReply::Value(atom),
                   "ASCII case ignored");
        assert_eq!(s.answer(2, &WinCall::AtomAdd { name: "warpine Größe".into() }), WinReply::Value(atom));
        assert_eq!(s.answer(2, &WinCall::AtomUsage { atom }), WinReply::Value(2));
        assert_eq!(s.answer(3, &WinCall::AtomName { atom }), WinReply::Text("Warpine Größe".into()));
        assert_eq!(s.answer(3, &WinCall::AtomName { atom: 5 }), WinReply::Text("#5".into()), "an integer atom");
        s.answer(2, &WinCall::AtomDelete { atom });
        s.answer(2, &WinCall::AtomDelete { atom });
        assert_eq!(s.answer(2, &WinCall::AtomUsage { atom }), WinReply::Error(PMERR_INVALID_ATOM));
        assert_eq!(s.answer(2, &WinCall::Describe), WinReply::Failed);
    }
}
