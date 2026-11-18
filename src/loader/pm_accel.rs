// SPDX-License-Identifier: GPL-3.0-only

//! Accelerator tables made by the application: `WinCreateAccelTable`,
//! `WinDestroyAccelTable`, `WinCopyAccelTable` and `WinQueryAccelTable`.
//! Tables live in `WindowManager::accel_tables` beside the ones loaded
//! from `RT_ACCELTABLE` resources (`WinLoadAccelTable`, `FCF_ACCELTABLE`)
//! and are translated the same way (`pm_dialog::accel_matches`).
//!
//! Behaviour follows the PM Reference pages and `<pmwin.h>` (`ACCELTABLE`:
//! `cAccel`, `codepage`, then `ACCEL { fs, key, cmd }`, packed to 2):
//! - `WinCreateAccelTable` makes a table from an `ACCELTABLE` in memory.
//! - `WinCopyAccelTable` copies a table out as an `ACCELTABLE`, at most
//!   `cbCopyMax` bytes; with a NULL buffer it returns the size it needs.
//! - `WinQueryAccelTable` returns the table set for a window
//!   (`NULLHANDLE` = the queue's).
//!
//! Unverified (Warpine's own choices): an unreadable `ACCELTABLE` fails
//! with `PMERR_PARAMETER_OUT_OF_RANGE`; `WinCopyAccelTable` copies the
//! first `cbCopyMax` bytes of the table as it would be laid out (a partial
//! entry is cut, `cAccel` stays the full count); destroying a table that is
//! set for windows takes it from them.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_dialog::{parse_accel_table, ACCEL_SIZE, AT_HEADER_SIZE};
use super::pm_types::{AccelEntry, WindowManager};
use super::{ApiResult, Loader};

/// Lay out `entries` as an `ACCELTABLE` of `codepage`.
pub fn serialize_accel_table(entries: &[AccelEntry], codepage: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(AT_HEADER_SIZE + entries.len() * ACCEL_SIZE);
    out.extend_from_slice(&(entries.len().min(u16::MAX as usize) as u16).to_le_bytes());
    out.extend_from_slice(&codepage.to_le_bytes());
    for e in entries.iter().take(u16::MAX as usize) {
        for v in [e.flags, e.key, e.cmd] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

impl WindowManager {
    /// Register a table of `codepage` and return its handle.
    pub fn create_accel_table(&mut self, entries: Vec<AccelEntry>, codepage: u16) -> u32 {
        let h = self.add_accel_table(entries);
        self.accel_codepages.insert(h, codepage);
        h
    }

    /// Destroy table `haccel`, taking it from the windows it was set for.
    /// False when there is no such table.
    pub fn destroy_accel_table(&mut self, haccel: u32) -> bool {
        if self.accel_tables.remove(&haccel).is_none() { return false; }
        self.accel_codepages.remove(&haccel);
        self.window_accel.retain(|_, h| *h != haccel);
        true
    }

    /// Table `haccel` laid out as an `ACCELTABLE`.
    pub fn accel_table_bytes(&self, haccel: u32) -> Option<Vec<u8>> {
        let entries = self.accel_tables.get(&haccel)?;
        Some(serialize_accel_table(entries, self.accel_codepages.get(&haccel).copied().unwrap_or(0)))
    }

    /// The table set for `hwnd` by `WinSetAccelTable` (0 = the queue's), or 0.
    pub fn window_accel_table(&self, hwnd: u32) -> u32 {
        self.window_accel.get(&hwnd).copied().unwrap_or(0)
    }
}

impl Loader {
    /// Read the `ACCELTABLE` at `ptr`: its entries and codepage.
    fn read_accel_table(&self, ptr: u32) -> Option<(Vec<AccelEntry>, u16)> {
        if ptr == 0 { return None; }
        let count = self.guest_read::<u16>(ptr)? as usize;
        let codepage = self.guest_read::<u16>(ptr.checked_add(2)?)?;
        // cAccel is a USHORT: at most 64K entries of 6 bytes.
        let data = self.guest_read_bytes(ptr, AT_HEADER_SIZE + count * ACCEL_SIZE)?;
        Some((parse_accel_table(&data), codepage))
    }

    /// `WinCreateAccelTable(hab, pAccelTable)` → HACCEL.
    pub(super) fn win_create_accel_table(&self, tid: u32, pacct: u32) -> ApiResult {
        let Some((entries, codepage)) = self.read_accel_table(pacct) else {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        };
        ApiResult::Normal(self.shared.window_mgr.lock_or_recover().create_accel_table(entries, codepage))
    }

    /// `WinDestroyAccelTable(haccel)` → BOOL.
    pub(super) fn win_destroy_accel_table(&self, tid: u32, haccel: u32) -> ApiResult {
        if !self.shared.window_mgr.lock_or_recover().destroy_accel_table(haccel) {
            return self.pm_fail(tid, PMERR_INVALID_HACCEL);
        }
        ApiResult::Normal(1)
    }

    /// `WinCopyAccelTable(haccel, pAccelTable, cbCopyMax)` → bytes copied,
    /// or with a NULL `pAccelTable` the size of the table.
    pub(super) fn win_copy_accel_table(&self, tid: u32, haccel: u32, pacct: u32, cb_max: u32) -> ApiResult {
        let Some(bytes) = self.shared.window_mgr.lock_or_recover().accel_table_bytes(haccel) else {
            return self.pm_fail(tid, PMERR_INVALID_HACCEL);
        };
        if pacct == 0 { return ApiResult::Normal(bytes.len() as u32); }
        let n = bytes.len().min(cb_max as usize);
        if self.guest_write_bytes(pacct, &bytes[..n]).is_none() {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        ApiResult::Normal(n as u32)
    }

    /// `WinQueryAccelTable(hab, hwnd)` → HACCEL (0 when none is set).
    pub(super) fn win_query_accel_table(&self, tid: u32, hwnd: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, true) { return ApiResult::Normal(0); }
        ApiResult::Normal(self.shared.window_mgr.lock_or_recover().window_accel_table(hwnd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_error::error_id;
    use super::super::vm_backend::mock::MockVcpu;

    const ORD_WIN_COPY_ACCEL_TABLE: u32 = 709;
    const ORD_WIN_CREATE_ACCEL_TABLE: u32 = 713;
    const ORD_WIN_DESTROY_ACCEL_TABLE: u32 = 723;
    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_QUERY_ACCEL_TABLE: u32 = 798;
    const ORD_WIN_SET_ACCEL_TABLE: u32 = 850;
    const BUF: u32 = 0x2000;
    const OUT: u32 = 0x3000;

    fn call(loader: &Loader, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
        match loader.handle_pmwin_call(&mut vcpu, 0, ordinal) {
            ApiResult::Normal(v) => v,
            other => panic!("PMWIN ordinal {} returned {:?}", ordinal, other),
        }
    }

    fn table() -> Vec<AccelEntry> {
        vec![
            AccelEntry { flags: AF_VIRTUALKEY, key: VK_F3 as u16, cmd: 300 },
            AccelEntry { flags: AF_CHAR | AF_CONTROL, key: b'S' as u16, cmd: 301 },
        ]
    }

    #[test]
    fn serialized_tables_parse_back() {
        let bytes = serialize_accel_table(&table(), 850);
        assert_eq!(bytes.len(), AT_HEADER_SIZE + 2 * ACCEL_SIZE);
        assert_eq!(&bytes[..4], &[2, 0, 0x52, 0x03]);
        assert_eq!(parse_accel_table(&bytes), table());
    }

    #[test]
    fn create_copy_query_and_destroy() {
        let loader = Loader::new_mock();
        loader.guest_write_bytes(BUF, &serialize_accel_table(&table(), 437)).unwrap();
        let h = call(&loader, ORD_WIN_CREATE_ACCEL_TABLE, &[MOCK_HAB, BUF]);
        assert_ne!(h, 0);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().accel_table(h), Some(&table()));
        // The size, then a full and a cut copy.
        let size = (AT_HEADER_SIZE + 2 * ACCEL_SIZE) as u32;
        assert_eq!(call(&loader, ORD_WIN_COPY_ACCEL_TABLE, &[h, 0, 0]), size);
        assert_eq!(call(&loader, ORD_WIN_COPY_ACCEL_TABLE, &[h, OUT, 1000]), size);
        assert_eq!(loader.guest_read_bytes(OUT, size as usize).unwrap(), serialize_accel_table(&table(), 437));
        loader.guest_write_bytes(OUT, &[0xEE; 16]).unwrap();
        assert_eq!(call(&loader, ORD_WIN_COPY_ACCEL_TABLE, &[h, OUT, 6]), 6);
        assert_eq!(loader.guest_read::<u8>(OUT + 6), Some(0xEE), "nothing past cbCopyMax");
        // Set for the queue and queried back; destroying takes it away.
        assert_eq!(call(&loader, ORD_WIN_SET_ACCEL_TABLE, &[MOCK_HAB, h, 0]), 1);
        assert_eq!(call(&loader, ORD_WIN_QUERY_ACCEL_TABLE, &[MOCK_HAB, 0]), h);
        assert_eq!(call(&loader, ORD_WIN_DESTROY_ACCEL_TABLE, &[h]), 1);
        assert_eq!(call(&loader, ORD_WIN_QUERY_ACCEL_TABLE, &[MOCK_HAB, 0]), 0);
        assert_eq!(call(&loader, ORD_WIN_DESTROY_ACCEL_TABLE, &[h]), 0);
        assert_eq!(call(&loader, ORD_WIN_GET_LAST_ERROR, &[MOCK_HAB]), error_id(SEVERITY_ERROR, PMERR_INVALID_HACCEL));
        assert_eq!(call(&loader, ORD_WIN_COPY_ACCEL_TABLE, &[h, 0, 0]), 0);
    }

    #[test]
    fn hostile_tables_fail_safely() {
        let loader = Loader::new_mock();
        assert_eq!(call(&loader, ORD_WIN_CREATE_ACCEL_TABLE, &[MOCK_HAB, 0]), 0);
        assert_eq!(call(&loader, ORD_WIN_GET_LAST_ERROR, &[MOCK_HAB]),
                   error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
        // A count running past the end of guest memory.
        let near_end = loader.shared.guest_mem.size() as u32 - 8;
        loader.guest_write::<u16>(near_end, 0xFFFF).unwrap();
        assert_eq!(call(&loader, ORD_WIN_CREATE_ACCEL_TABLE, &[MOCK_HAB, near_end]), 0);
        // An empty table is a table.
        loader.guest_write_bytes(BUF, &[0, 0, 0, 0]).unwrap();
        let h = call(&loader, ORD_WIN_CREATE_ACCEL_TABLE, &[MOCK_HAB, BUF]);
        assert_ne!(h, 0);
        assert_eq!(call(&loader, ORD_WIN_COPY_ACCEL_TABLE, &[h, 0, 0]), AT_HEADER_SIZE as u32);
        // A copy to an unwritable buffer fails.
        assert_eq!(call(&loader, ORD_WIN_COPY_ACCEL_TABLE, &[h, 0xFFFF_FFFE, 100]), 0);
        assert_eq!(call(&loader, ORD_WIN_QUERY_ACCEL_TABLE, &[MOCK_HAB, 0xDEAD]), 0);
    }
}
