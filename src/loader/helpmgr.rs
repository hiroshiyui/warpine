// SPDX-License-Identifier: GPL-3.0-only
//
// HELPMGR: the Help Manager calls without a help viewer.
//
// Programs create a help instance at start-up, associate it with their
// main window and load a help table; many refuse to start when that
// fails. Warpine accepts all of it and keeps the records — the HELPINIT
// fields, help tables, associations — but shows no help: a help instance
// is a hidden object window whose procedure accepts every message (`HM_*`)
// and returns 0, and F1 opens nothing. That help is not available is
// logged once.
//
// Behaviour follows the IBM PM Reference: `WinCreateHelpInstance` returns
// the instance's window handle (NULLHANDLE on error), `WinAssociateHelpInstance`
// with NULLHANDLE removes the association, and `WinQueryHelpInstance`
// traces the parent chain, then the owner chain, until it finds an
// associated window.

use std::collections::{HashMap, HashSet};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::WindowManager;

/// Window class of help instances.
pub const HELP_INSTANCE_CLASS: &str = "#HelpInstance";

// HELPINIT field offsets (<pmhelp.h>, 2-byte packed; every field 4 bytes).
const HELPINIT_CB: u32 = 0;
const HELPINIT_RETURN_CODE: u32 = 4;
const HELPINIT_TUTORIAL_NAME: u32 = 8;
const HELPINIT_HELP_TABLE: u32 = 12;
const HELPINIT_HMOD_HELP_TABLE: u32 = 16;
const HELPINIT_HMOD_ACCEL_ACTION_BAR: u32 = 20;
const HELPINIT_ID_ACCEL_TABLE: u32 = 24;
const HELPINIT_ID_ACTION_BAR: u32 = 28;
const HELPINIT_WINDOW_TITLE: u32 = 32;
const HELPINIT_SHOW_PANEL_ID: u32 = 36;
const HELPINIT_LIBRARY_NAME: u32 = 40;
/// `sizeof(HELPINIT)`.
pub const HELPINIT_SIZE: u32 = 44;

/// High word of a `phtHelpTable` that names a help-table resource
/// (`MAKELONG(id, 0xFFFF)`) instead of pointing at a `HELPTABLE`.
const HELP_TABLE_RESOURCE_MARK: u32 = 0xFFFF;

/// A help table given to an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpTable {
    /// A `HELPTABLE` resource (`RT_HELPTABLE`) of module `hmod`.
    Resource { id: u32, hmod: u32 },
    /// A `HELPTABLE` in the program's memory.
    Memory(u32),
}

/// What `WinCreateHelpInstance` was given (the readable HELPINIT fields).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelpInstance {
    pub tutorial: String,
    pub window_title: String,
    pub library: String,
    pub hmod_accel_action_bar: u32,
    pub id_accel_table: u32,
    pub id_action_bar: u32,
    pub show_panel_id: u32,
    /// Help tables in the order given: HELPINIT's, then `WinLoadHelpTable`
    /// / `WinCreateHelpTable`.
    pub tables: Vec<HelpTable>,
}

/// Help instances (by window handle) and associations.
#[derive(Debug, Default)]
pub struct HelpRegistry {
    instances: HashMap<u32, HelpInstance>,
    /// Application window → help instance.
    associations: HashMap<u32, u32>,
    /// Whether "help is not available" has been logged.
    warned: bool,
}

impl HelpRegistry {
    pub fn instance(&self, hwnd_help: u32) -> Option<&HelpInstance> {
        self.instances.get(&hwnd_help)
    }

    pub fn is_instance(&self, hwnd_help: u32) -> bool {
        self.instances.contains_key(&hwnd_help)
    }

    /// The instance associated with `hwnd` itself.
    pub fn associated(&self, hwnd: u32) -> Option<u32> {
        self.associations.get(&hwnd).copied()
    }

    /// Drop instances and associations of destroyed windows.
    pub fn forget_windows(&mut self, doomed: &[u32]) {
        for h in doomed {
            self.instances.remove(h);
        }
        self.associations.retain(|app, help| !doomed.contains(app) && !doomed.contains(help));
    }

    /// Whether the "no help" message is still to be logged (true once).
    fn first_use(&mut self) -> bool {
        !std::mem::replace(&mut self.warned, true)
    }
}

/// `WinQueryHelpInstance`: the instance associated with `hwnd`, its
/// parent chain, then its owner chain (and the owners' parent chains).
pub fn find_help_instance(wm: &WindowManager, hwnd: u32) -> u32 {
    let mut seen = HashSet::new();
    let mut next = vec![hwnd];
    while let Some(start) = next.pop() {
        let mut h = start;
        while h != 0 && h != HWND_DESKTOP && seen.insert(h) {
            if let Some(help) = wm.help.associated(h) {
                return help;
            }
            let Some(w) = wm.get_window(h) else { break };
            if w.owner != 0 && w.owner != HWND_DESKTOP {
                next.insert(0, w.owner);
            }
            h = w.parent;
        }
    }
    0
}

impl super::Loader {
    /// HELPINIT field `offset`, or 0 when the structure is shorter.
    fn helpinit_field(&self, phinit: u32, cb: u32, offset: u32) -> u32 {
        if offset + 4 > cb { return 0; }
        self.guest_read::<u32>(phinit + offset).unwrap_or(0)
    }

    fn helpinit_string(&self, phinit: u32, cb: u32, offset: u32) -> String {
        match self.helpinit_field(phinit, cb, offset) {
            0 => String::new(),
            p => self.read_guest_string(p),
        }
    }

    /// WinCreateHelpInstance(hab, phinit) → HWND of the instance.
    pub fn win_create_help_instance(&self, tid: u32, _hab: u32, phinit: u32) -> u32 {
        let Some(cb) = self.guest_read::<u32>(phinit + HELPINIT_CB).filter(|_| phinit != 0) else {
            self.pm_error(tid, PMERR_INVALID_PARM);
            return 0;
        };
        let cb = cb.min(HELPINIT_SIZE);
        let mut inst = HelpInstance {
            tutorial: self.helpinit_string(phinit, cb, HELPINIT_TUTORIAL_NAME),
            window_title: self.helpinit_string(phinit, cb, HELPINIT_WINDOW_TITLE),
            library: self.helpinit_string(phinit, cb, HELPINIT_LIBRARY_NAME),
            hmod_accel_action_bar: self.helpinit_field(phinit, cb, HELPINIT_HMOD_ACCEL_ACTION_BAR),
            id_accel_table: self.helpinit_field(phinit, cb, HELPINIT_ID_ACCEL_TABLE),
            id_action_bar: self.helpinit_field(phinit, cb, HELPINIT_ID_ACTION_BAR),
            show_panel_id: self.helpinit_field(phinit, cb, HELPINIT_SHOW_PANEL_ID),
            tables: Vec::new(),
        };
        let table = self.helpinit_field(phinit, cb, HELPINIT_HELP_TABLE);
        if table >> 16 == HELP_TABLE_RESOURCE_MARK {
            let hmod = self.helpinit_field(phinit, cb, HELPINIT_HMOD_HELP_TABLE);
            inst.tables.push(HelpTable::Resource { id: table & 0xFFFF, hmod });
        } else if table != 0 {
            inst.tables.push(HelpTable::Memory(table));
        }
        // Success: no error in the structure.
        if cb >= HELPINIT_RETURN_CODE + 4 {
            let _ = self.guest_write::<u32>(phinit + HELPINIT_RETURN_CODE, 0);
        }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let hmq = wm.thread_queue(tid).unwrap_or(0);
        let hwnd = wm.create_window(HELP_INSTANCE_CLASS.to_string(), HWND_OBJECT, hmq);
        if wm.help.first_use() {
            log::warn!("Help Manager: help instance for '{}' ({}) created, but Warpine has no help \
                        viewer — help requests are ignored", inst.window_title, inst.library);
        }
        wm.help.instances.insert(hwnd, inst);
        hwnd
    }

    /// WinDestroyHelpInstance(hwndHelp) → BOOL.
    pub fn win_destroy_help_instance(&self, tid: u32, hwnd_help: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if !wm.help.is_instance(hwnd_help) {
            drop(wm);
            self.pm_error(tid, PMERR_INVALID_HWND);
            return 0;
        }
        wm.destroy_window(hwnd_help);
        wm.help.forget_windows(&[hwnd_help]);
        1
    }

    /// WinQueryHelpInstance(hwndApp) → HWND (NULLHANDLE when none).
    pub fn win_query_help_instance(&self, hwnd_app: u32) -> u32 {
        find_help_instance(&self.shared.window_mgr.lock_or_recover(), hwnd_app)
    }

    /// WinAssociateHelpInstance(hwndHelp, hwndApp) → BOOL; NULLHANDLE
    /// removes `hwndApp`'s association.
    pub fn win_associate_help_instance(&self, tid: u32, hwnd_help: u32, hwnd_app: u32) -> u32 {
        if hwnd_app == 0 || !self.pm_check_hwnd(tid, hwnd_app, false) {
            if hwnd_app == 0 { self.pm_error(tid, PMERR_INVALID_HWND); }
            return 0;
        }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if hwnd_help == 0 {
            wm.help.associations.remove(&hwnd_app);
            return 1;
        }
        if !wm.help.is_instance(hwnd_help) {
            drop(wm);
            self.pm_error(tid, PMERR_INVALID_HWND);
            return 0;
        }
        wm.help.associations.insert(hwnd_app, hwnd_help);
        1
    }

    /// Add a help table to instance `hwnd_help`.
    fn add_help_table(&self, tid: u32, hwnd_help: u32, table: HelpTable) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        match wm.help.instances.get_mut(&hwnd_help) {
            Some(inst) => { inst.tables.push(table); 1 }
            None => {
                drop(wm);
                self.pm_error(tid, PMERR_INVALID_HWND);
                0
            }
        }
    }

    /// WinLoadHelpTable(hwndHelp, idHelpTable, hmod) → BOOL.
    pub fn win_load_help_table(&self, tid: u32, hwnd_help: u32, id: u32, hmod: u32) -> u32 {
        self.add_help_table(tid, hwnd_help, HelpTable::Resource { id, hmod })
    }

    /// WinCreateHelpTable(hwndHelp, phtHelpTable) → BOOL.
    pub fn win_create_help_table(&self, tid: u32, hwnd_help: u32, pht: u32) -> u32 {
        if pht == 0 {
            self.pm_error(tid, PMERR_INVALID_PARM);
            return 0;
        }
        self.add_help_table(tid, hwnd_help, HelpTable::Memory(pht))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::vm_backend::mock::MockVcpu;
    use crate::loader::{ApiResult, Loader};

    const TID: u32 = 1;
    const HELPINIT_AT: u32 = 0x0010_0000;
    const TITLE_AT: u32 = 0x0010_1000;
    const LIBRARY_AT: u32 = 0x0010_1100;
    const RES_HELP_TABLE: u32 = 1000;
    const HMOD: u32 = 0x55;

    fn write_helpinit(l: &Loader, cb: u32) {
        l.guest_write_bytes(TITLE_AT, b"My Help\0").unwrap();
        l.guest_write_bytes(LIBRARY_AT, b"MYAPP.HLP\0").unwrap();
        let fields = [cb, 0xDEAD, 0, (HELP_TABLE_RESOURCE_MARK << 16) | RES_HELP_TABLE, HMOD,
                      0, 0, 0, TITLE_AT, 1, LIBRARY_AT];
        for (i, f) in fields.iter().enumerate() {
            l.guest_write::<u32>(HELPINIT_AT + 4 * i as u32, *f).unwrap();
        }
    }

    fn last_error(l: &Loader) -> u32 {
        l.shared.pm_errors.take(TID) & 0xFFFF
    }

    #[test]
    fn create_records_the_helpinit_fields() {
        let l = Loader::new_mock();
        write_helpinit(&l, HELPINIT_SIZE);
        let h = l.win_create_help_instance(TID, MOCK_HAB, HELPINIT_AT);
        assert_ne!(h, 0);
        assert_eq!(l.guest_read::<u32>(HELPINIT_AT + HELPINIT_RETURN_CODE), Some(0));
        let wm = l.shared.window_mgr.lock_or_recover();
        let inst = wm.help.instance(h).unwrap();
        assert_eq!(inst.window_title, "My Help");
        assert_eq!(inst.library, "MYAPP.HLP");
        assert_eq!(inst.show_panel_id, 1);
        assert_eq!(inst.tables, vec![HelpTable::Resource { id: RES_HELP_TABLE, hmod: HMOD }]);
        assert!(wm.is_window(h), "the instance is a window");
    }

    #[test]
    fn a_short_helpinit_reads_only_its_fields() {
        let l = Loader::new_mock();
        write_helpinit(&l, HELPINIT_HELP_TABLE);
        let h = l.win_create_help_instance(TID, MOCK_HAB, HELPINIT_AT);
        let wm = l.shared.window_mgr.lock_or_recover();
        let inst = wm.help.instance(h).unwrap();
        assert!(inst.window_title.is_empty() && inst.tables.is_empty());
        drop(wm);
        assert_eq!(l.win_create_help_instance(TID, MOCK_HAB, 0), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_PARM);
        assert_eq!(l.win_create_help_instance(TID, MOCK_HAB, 0xFFFF_FFFE), 0);
    }

    #[test]
    fn associations_follow_parents_and_owners() {
        let l = Loader::new_mock();
        write_helpinit(&l, HELPINIT_SIZE);
        let help = l.win_create_help_instance(TID, MOCK_HAB, HELPINIT_AT);
        let (frame, child, owned) = {
            let mut wm = l.shared.window_mgr.lock_or_recover();
            let frame = wm.create_window("F".into(), HWND_DESKTOP, 0);
            let child = wm.create_window("C".into(), frame, 0);
            let owned = wm.create_window("O".into(), HWND_DESKTOP, 0);
            wm.get_window_mut(owned).unwrap().owner = child;
            (frame, child, owned)
        };
        assert_eq!(l.win_query_help_instance(child), 0);
        assert_eq!(l.win_associate_help_instance(TID, help, frame), 1);
        assert_eq!(l.win_query_help_instance(frame), help);
        assert_eq!(l.win_query_help_instance(child), help, "parent chain");
        assert_eq!(l.win_query_help_instance(owned), help, "owner chain");
        // NULLHANDLE removes the association.
        assert_eq!(l.win_associate_help_instance(TID, 0, frame), 1);
        assert_eq!(l.win_query_help_instance(child), 0);
        // Bad handles.
        assert_eq!(l.win_associate_help_instance(TID, frame, frame), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_HWND);
        assert_eq!(l.win_associate_help_instance(TID, help, 0xBAD), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_HWND);
        // Destroying the window drops its association.
        l.win_associate_help_instance(TID, help, frame);
        l.shared.window_mgr.lock_or_recover().destroy_window(frame);
        assert_eq!(l.shared.window_mgr.lock_or_recover().help.associated(frame), None);
    }

    #[test]
    fn help_tables_and_destruction() {
        let l = Loader::new_mock();
        write_helpinit(&l, HELPINIT_SIZE);
        let help = l.win_create_help_instance(TID, MOCK_HAB, HELPINIT_AT);
        assert_eq!(l.win_load_help_table(TID, help, 7, 0), 1);
        assert_eq!(l.win_create_help_table(TID, help, 0x2000), 1);
        assert_eq!(l.win_create_help_table(TID, help, 0), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_PARM);
        assert_eq!(l.shared.window_mgr.lock_or_recover().help.instance(help).unwrap().tables.len(), 3);
        assert_eq!(l.win_load_help_table(TID, 0xBAD, 7, 0), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_HWND);
        assert_eq!(l.win_destroy_help_instance(TID, help), 1);
        assert!(!l.shared.window_mgr.lock_or_recover().is_window(help));
        assert_eq!(l.win_destroy_help_instance(TID, help), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_HWND);
        assert_eq!(l.win_load_help_table(TID, help, 7, 0), 0);
    }

    #[test]
    fn hm_messages_sent_to_an_instance_are_accepted() {
        let l = Loader::new_mock();
        write_helpinit(&l, HELPINIT_SIZE);
        let help = l.win_create_help_instance(TID, MOCK_HAB, HELPINIT_AT);
        for msg in [HM_DISPLAY_HELP, HM_SET_ACTIVE_WINDOW, HM_CONTROL] {
            match l.dispatch_builtin_control(help, msg, 1, 2) {
                ApiResult::Normal(0) => {}
                other => panic!("{msg:#x}: {other:?}"),
            }
        }
        assert_eq!(last_error(&l), 0);
    }

    #[test]
    fn calls_are_dispatched_through_the_registry() {
        let l = Loader::new_mock();
        write_helpinit(&l, HELPINIT_SIZE);
        let call = |ordinal: u32, args: &[u32]| {
            let mut a = [0u32; 10];
            a[..args.len()].copy_from_slice(args);
            let entry = crate::loader::api_registry::find(HELPMGR_BASE + ordinal).unwrap();
            match (entry.handler)(&l, &mut MockVcpu::new(), TID, a) {
                ApiResult::Normal(v) => v,
                other => panic!("{other:?}"),
            }
        };
        let frame = l.shared.window_mgr.lock_or_recover().create_window("F".into(), HWND_DESKTOP, 0);
        let help = call(51, &[MOCK_HAB, HELPINIT_AT]);
        assert_ne!(help, 0);
        assert_eq!(call(54, &[help, frame]), 1);
        assert_eq!(call(53, &[frame]), help);
        assert_eq!(call(55, &[help, 1, 0]), 1);
        assert_eq!(call(56, &[help, 0x2000]), 1);
        assert_eq!(call(52, &[help]), 1);
        assert_eq!(call(53, &[frame]), 0);
    }
}
