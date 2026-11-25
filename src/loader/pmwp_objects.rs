// SPDX-License-Identifier: GPL-3.0-or-later
//
//! PMWP's Workplace object calls (ADR-0129, ADR-0130): `WinQueryObject`,
//! `WinCreateObject`, `WinDestroyObject` and `WinSetObjectData`, over the
//! user profile where the SDK's `WPAbstract` keeps its objects
//! (`wps_setup.rs` names the format).
//!
//! The Workplace classes are guest code (`libwps`), and there is no
//! Workplace Shell process yet to hold their objects, so the host makes no
//! object itself: `WinCreateObject` writes a record of the class without
//! saved values and queues its title and setup string, and
//! `WinSetObjectData` queues more; `libwps` makes the object
//! (`wpSetupOnce`) or applies what is queued (`wpSetup`) when a process
//! asks for it (`wpclsQueryObject`). The handle is valid at once, in every
//! process; the class is checked only then (it must be registered there
//! and descend from `WPAbstract`).

use super::constants::HINI_USERPROFILE;
use super::mutex_ext::MutexExt;
use super::pmshapi::ProfileTable;
use super::vfs::DriveManager;
use super::wps_setup::{self as setup, APP_HANDLES, APP_OBJECTS, APP_OBJECT_IDS, APP_PENDING, FIRST_HANDLE, KEY_NEXT};
use super::Loader;

/// `WinCreateObject`'s flags (Warpine's values: the Reference names them
/// without values).
pub const CO_FAILIFEXISTS: u32 = 0;
pub const CO_REPLACEIFEXISTS: u32 = 1;
pub const CO_UPDATEIFEXISTS: u32 = 2;

/// How many taken handles the allocator steps over before it gives up.
const MAX_HANDLE_PROBES: u32 = 4096;
/// The longest string these calls read.
const MAX_STRING: usize = 4096;
/// The longest setup string queued for one object.
const MAX_QUEUED: usize = 64 * 1024;

/// The user profile, locked, for one call.
struct Store<'a> {
    profiles: &'a ProfileTable,
    dm: &'a DriveManager,
}

impl Store<'_> {
    fn get(&self, app: &[u8], key: &[u8]) -> Option<Vec<u8>> {
        self.profiles.query_value(self.dm, HINI_USERPROFILE, app, key).ok()
    }

    fn put(&self, app: &[u8], key: &[u8], value: Option<&[u8]>) -> bool {
        self.profiles.write(self.dm, HINI_USERPROFILE, app, Some(key), value).is_ok()
    }

    fn stored(&self, h: u32) -> bool {
        self.get(APP_OBJECTS, setup::handle_key(h).as_bytes()).is_some_and(|r| !r.is_empty())
    }

    /// The stored object an object ID names.
    fn by_id(&self, id: &[u8]) -> Option<u32> {
        let h = u32::from_le_bytes(<[u8; 4]>::try_from(self.get(APP_OBJECT_IDS, id)?.as_slice()).ok()?);
        self.stored(h).then_some(h)
    }

    /// A handle no stored object has, the counter moved past it.
    fn new_handle(&self) -> Option<u32> {
        let next = self.get(APP_HANDLES, KEY_NEXT)
            .and_then(|v| <[u8; 4]>::try_from(v.as_slice()).ok())
            .map(u32::from_le_bytes)
            .filter(|&n| n >= FIRST_HANDLE)
            .unwrap_or(FIRST_HANDLE);
        let h = (0..MAX_HANDLE_PROBES).map(|i| next.wrapping_add(i)).find(|&h| h >= FIRST_HANDLE && !self.stored(h))?;
        self.put(APP_HANDLES, KEY_NEXT, Some(&h.wrapping_add(1).to_le_bytes())).then_some(h)
    }

    /// `more` queued for object `h` (nothing to queue writes nothing).
    fn queue(&self, h: u32, more: &[u8]) -> bool {
        if more.is_empty() { return true; }
        let key = setup::handle_key(h);
        let queued = setup::append(&self.get(APP_PENDING, key.as_bytes()).unwrap_or_default(), more);
        queued.len() <= MAX_QUEUED && self.put(APP_PENDING, key.as_bytes(), Some(&queued))
    }

    /// Object `h` gone: its record, its queue and every ID that names it.
    fn destroy(&self, h: u32) -> bool {
        if !self.stored(h) { return false; }
        let key = setup::handle_key(h);
        let ids = self.profiles.query_names(self.dm, HINI_USERPROFILE, Some(APP_OBJECT_IDS)).unwrap_or_default();
        for id in ids.split(|&b| b == 0).filter(|n| !n.is_empty()) {
            if self.get(APP_OBJECT_IDS, id).is_some_and(|v| v == h.to_le_bytes()) {
                self.put(APP_OBJECT_IDS, id, None);
            }
        }
        self.put(APP_PENDING, key.as_bytes(), None);
        self.put(APP_OBJECTS, key.as_bytes(), None)
    }
}

impl Loader {
    /// A guest string (at most `MAX_STRING` bytes), never from Warpine's
    /// protected memory; empty for NULL.
    fn wps_string(&self, p: u32) -> Option<Vec<u8>> {
        if p == 0 { return Some(Vec::new()); }
        let mut out = Vec::new();
        while out.len() < MAX_STRING {
            let at = p.checked_add(out.len() as u32)?;
            match self.guest_read_buffer(at, 1)?[0] {
                0 => return Some(out),
                b => out.push(b),
            }
        }
        None
    }

    /// Run `f` over the user profile, locked (profiles before drives).
    fn with_wps_store<T>(&self, f: impl FnOnce(&Store) -> T) -> T {
        let profiles = self.shared.profiles.lock_or_recover();
        let dm = self.shared.drive_mgr.lock_or_recover();
        f(&Store { profiles: &profiles, dm: &dm })
    }

    /// WinQueryObject(PCSZ pszObjectID) -> HOBJECT: the handle of the
    /// stored object an object ID names; NULLHANDLE when it names none, or
    /// its object is no longer stored. A file-system path names no object
    /// yet (there is no `WPFileSystem`).
    pub fn win_query_object(&self, psz_id: u32) -> u32 {
        let Some(id) = self.wps_string(psz_id).filter(|i| !i.is_empty()) else { return 0 };
        self.with_wps_store(|s| s.by_id(&id).unwrap_or(0))
    }

    /// WinCreateObject(pszClassName, pszTitle, pszSetupString, pszLocation,
    /// ulFlags) -> HOBJECT: a new object of the class, made when a process
    /// first asks for it (ADR-0130). An object ID in the setup string that
    /// a stored object has already: `CO_FAILIFEXISTS` (and an unknown
    /// flag) gives NULLHANDLE, `CO_UPDATEIFEXISTS` queues the title and
    /// setup for that object and gives its handle, `CO_REPLACEIFEXISTS`
    /// destroys it first. The location is accepted and not used yet (there
    /// are no folders). Carets at the end of the title are dropped (the
    /// setup-string format cannot follow them with another item).
    pub fn win_create_object(&self, p_class: u32, p_title: u32, p_setup: u32, _p_location: u32, flags: u32) -> u32 {
        let (Some(class), Some(title), Some(setup_string)) =
            (self.wps_string(p_class), self.wps_string(p_title), self.wps_string(p_setup)) else { return 0 };
        let Some(record) = setup::new_record(&class) else { return 0 };
        // The title first, so the setup string's own TITLE (later) wins.
        let mut queued = Vec::new();
        if !title.is_empty() {
            queued.extend_from_slice(b"TITLE=");
            queued.extend(setup::title_value(&title));
        }
        let queued = setup::append(&queued, &setup_string);
        let id = setup::scan(&setup_string, b"OBJECTID").filter(|i| !i.is_empty());
        self.with_wps_store(|s| {
            if let Some(old) = id.as_deref().and_then(|i| s.by_id(i)) {
                match flags {
                    CO_UPDATEIFEXISTS => return if s.queue(old, &queued) { old } else { 0 },
                    CO_REPLACEIFEXISTS => { s.destroy(old); }
                    _ => return 0,
                }
            }
            let Some(h) = s.new_handle() else { return 0 };
            let key = setup::handle_key(h);
            if !s.put(APP_OBJECTS, key.as_bytes(), Some(&record)) { return 0; }
            let mapped = id.as_deref().is_none_or(|i| s.put(APP_OBJECT_IDS, i, Some(&h.to_le_bytes())));
            if !mapped || !s.queue(h, &queued) {
                s.destroy(h);
                return 0;
            }
            h
        })
    }

    /// WinDestroyObject(HOBJECT) -> BOOL: the stored object removed, with
    /// what was queued for it and its IDs; FALSE when there is none. A
    /// process that has it awake keeps its copy until it frees it.
    pub fn win_destroy_object(&self, h: u32) -> u32 {
        self.with_wps_store(|s| s.destroy(h)) as u32
    }

    /// WinSetObjectData(HOBJECT, pszSetupString) -> BOOL: the setup string
    /// queued for the stored object; `libwps` applies it (`wpSetup`) when a
    /// process next asks for the object. FALSE when there is no object.
    pub fn win_set_object_data(&self, h: u32, p_setup: u32) -> u32 {
        let Some(setup_string) = self.wps_string(p_setup) else { return 0 };
        self.with_wps_store(|s| s.stored(h) && s.queue(h, &setup_string)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::vfs::DriveConfig;
    use crate::loader::vfs_hostdir::HostDirBackend;

    /// A mock loader whose C: (where the user profile lives) is a
    /// temporary directory.
    fn loader() -> (Loader, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let l = Loader::new_mock();
        {
            let mut dm = l.shared.drive_mgr.lock_or_recover();
            dm.set_drive_config(2, DriveConfig { host_path: tmp.path().to_path_buf(), label: "T".into(), read_only: false });
            dm.mount(2, Box::new(HostDirBackend::new(tmp.path().to_path_buf()).unwrap()));
        }
        (l, tmp)
    }

    fn put(l: &Loader, at: u32, s: &str) -> u32 {
        l.guest_write_bytes(at, format!("{s}\0").as_bytes()).unwrap();
        at
    }

    fn get(l: &Loader, app: &[u8], key: &[u8]) -> Option<Vec<u8>> {
        l.with_wps_store(|s| s.get(app, key))
    }

    /// WinQueryObject reads the object-ID map WPAbstract writes, and gives
    /// a handle only while its object is still stored.
    #[test]
    fn object_ids_name_stored_objects() {
        let (l, _tmp) = loader();
        let id = put(&l, 0x0030_0000, "<WP_CARD>");
        assert_eq!(l.win_query_object(id), 0, "nothing mapped");
        l.with_wps_store(|s| s.put(APP_OBJECT_IDS, b"<WP_CARD>", Some(&0x0002_0005u32.to_le_bytes())));
        assert_eq!(l.win_query_object(id), 0, "mapped, but no record");
        l.with_wps_store(|s| s.put(APP_OBJECTS, b"00020005", Some(b"record")));
        assert_eq!(l.win_query_object(id), 0x0002_0005);
        l.with_wps_store(|s| s.put(APP_OBJECT_IDS, b"<WP_CARD>", Some(b"bad")));
        assert_eq!(l.win_query_object(id), 0, "a mapping of the wrong size");
        assert_eq!(l.win_query_object(0), 0, "no ID");
    }

    /// WinCreateObject writes an empty record and queues the title and
    /// setup; its flags decide what an ID already taken does;
    /// WinSetObjectData queues more; WinDestroyObject removes everything.
    #[test]
    fn objects_are_created_queued_and_destroyed() {
        let (l, _tmp) = loader();
        let class = put(&l, 0x0030_0000, "Card");
        let title = put(&l, 0x0030_0100, "Lunch;today");
        let setup_string = put(&l, 0x0030_0200, "TEXT=soup;OBJECTID=<WP_C>");
        let id = put(&l, 0x0030_0300, "<WP_C>");
        let h = l.win_create_object(class, title, setup_string, 0, CO_FAILIFEXISTS);
        assert_eq!(h, FIRST_HANDLE);
        assert_eq!(l.win_query_object(id), h);
        let key = setup::handle_key(h);
        assert_eq!(get(&l, APP_OBJECTS, key.as_bytes()), setup::new_record(b"Card"));
        assert_eq!(get(&l, APP_PENDING, key.as_bytes()).as_deref(), Some(&b"TITLE=Lunch^;today;TEXT=soup;OBJECTID=<WP_C>"[..]));
        // The ID is taken: fail, update (queued for the same object),
        // replace (a new object, the old one gone).
        assert_eq!(l.win_create_object(class, 0, setup_string, 0, CO_FAILIFEXISTS), 0);
        assert_eq!(l.win_create_object(class, 0, setup_string, 0, 7), 0, "an unknown flag fails");
        let more = put(&l, 0x0030_0400, "TEXT=stew;OBJECTID=<WP_C>");
        assert_eq!(l.win_create_object(class, 0, more, 0, CO_UPDATEIFEXISTS), h);
        assert!(get(&l, APP_PENDING, key.as_bytes()).unwrap().ends_with(b";TEXT=stew;OBJECTID=<WP_C>"));
        assert_eq!(l.win_set_object_data(h, put(&l, 0x0030_0500, "TEXT=pie")), 1);
        assert!(get(&l, APP_PENDING, key.as_bytes()).unwrap().ends_with(b";TEXT=pie"));
        let h2 = l.win_create_object(class, 0, setup_string, 0, CO_REPLACEIFEXISTS);
        assert!(h2 != 0 && h2 != h);
        assert_eq!(l.win_query_object(id), h2);
        assert!(get(&l, APP_OBJECTS, key.as_bytes()).is_none() && get(&l, APP_PENDING, key.as_bytes()).is_none());
        assert_eq!(l.win_set_object_data(h, setup_string), 0, "no longer stored");
        // Destroy: the record, the queue, the ID.
        assert_eq!(l.win_destroy_object(h2), 1);
        assert_eq!(l.win_query_object(id), 0);
        assert_eq!(l.win_destroy_object(h2), 0, "already gone");
        // No class, a class name too long, an object without an ID.
        assert_eq!(l.win_create_object(0, 0, 0, 0, 0), 0);
        assert_eq!(l.win_create_object(put(&l, 0x0030_0600, &"x".repeat(256)), 0, 0, 0, 0), 0);
        let plain = l.win_create_object(class, 0, 0, 0, 0);
        assert!(plain > h2, "handles are not given twice");
        assert_eq!(get(&l, APP_PENDING, setup::handle_key(plain).as_bytes()), None, "nothing queued");
        // A string in Warpine's protected memory is not read.
        assert_eq!(l.win_create_object(super::super::constants::SYSTEM_AREA_BASE, 0, 0, 0, 0), 0);
    }
}
