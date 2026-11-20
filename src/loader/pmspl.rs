// SPDX-License-Identifier: GPL-3.0-or-later

//! Printing: the PMSPL spooler and the printer side of PMGPI's device
//! contexts — without any printer.
//!
//! Warpine has no printer drivers, print queues or ports, and printing is
//! not supported. Programs commonly ask the spooler what there is at
//! start-up or when the user opens a print dialog, and must be told
//! "nothing" in the documented ways rather than crash on a missing entry
//! point:
//!
//! * The enumerations (`SplEnumQueue`, `SplEnumPrinter`, `SplEnumDevice`,
//!   `SplEnumDriver`, `SplEnumPort`, `SplEnumQueueProcessor`) succeed with
//!   an empty list: `*pcReturned = *pcTotal = *pcbNeeded = 0` and
//!   `NO_ERROR`. An information level outside the documented ones is
//!   `ERROR_INVALID_LEVEL`; a computer name (a network spooler) is
//!   `NERR_InvalidComputer` — unverified: Warpine has no network.
//! * `SplQueryQueue` / `SplQueryDevice` name a queue / device that does not
//!   exist: `NERR_QNotFound` / `NERR_DestNotFound`.
//! * `SplQmOpen` fails (`SPL_ERROR`, a NULL handle) with
//!   `PMERR_SPL_QUEUE_NOT_FOUND` when the open data names a queue and
//!   `PMERR_SPL_NO_DEFAULT_QUEUE` when it does not (both unverified as the
//!   exact codes); every other `SplQm*` call is given a handle no
//!   `SplQmOpen` returned: `SPL_ERROR` with `PMERR_SPL_INV_HSPL`.
//! * `DevOpenDC(OD_QUEUED / OD_DIRECT)` fails with `DEV_ERROR` and
//!   `PMERR_DRIVER_NOT_FOUND` (other unsupported types: `PMERR_INV_DC_TYPE`)
//!   — see `pm_bitmap.rs`.
//! * `DevEscape` on a device context Warpine has (memory, information,
//!   window) implements no escape: `DEVESC_NOTIMPLEMENTED`; on an unknown
//!   one `DEVESC_ERROR` with `PMERR_INV_HDC`.
//! * `DevPostDeviceModes` (a printer driver's job-properties dialog) has no
//!   driver to ask: `DPDM_ERROR` with `PMERR_DRIVER_NOT_FOUND` (unverified).
//!
//! The error values come from `<pmerr.h>` / `<bseerr.h>`; which ones the
//! real spooler returns where the IBM references are silent is marked
//! unverified above.

use super::constants::*;
use super::Loader;

// ── Return values and errors (<pmspl.h>, <pmerr.h>, <pmdev.h>) ──────────────

/// `SPL_ERROR`: a failed `SplQm*` call (FALSE / NULL handle).
pub const SPL_ERROR: u32 = 0;
/// `NERR_BASE + 50`: no such queue.
#[allow(non_upper_case_globals)]
pub const NERR_QNotFound: u32 = 2150;
/// `NERR_BASE + 52`: no such print device.
#[allow(non_upper_case_globals)]
pub const NERR_DestNotFound: u32 = 2152;
/// `NERR_BASE + 251`: the computer name is not valid (no network spooler).
#[allow(non_upper_case_globals)]
pub const NERR_InvalidComputer: u32 = 2351;
/// `PMERR_SPL_INV_HSPL`: not a spooler handle.
pub const PMERR_SPL_INV_HSPL: u32 = 0x4005;
/// `PMERR_SPL_QUEUE_NOT_FOUND`: the queue named does not exist.
pub const PMERR_SPL_QUEUE_NOT_FOUND: u32 = 0x4024;
/// `PMERR_SPL_NO_DEFAULT_QUEUE`: no queue named and no default queue.
pub const PMERR_SPL_NO_DEFAULT_QUEUE: u32 = 0x4028;
/// `PMERR_DRIVER_NOT_FOUND`: no such presentation driver.
pub const PMERR_DRIVER_NOT_FOUND: u32 = 0x2026;
/// `PMERR_INV_DC_TYPE`: a `DevOpenDC` type Warpine does not open.
pub const PMERR_INV_DC_TYPE: u32 = 0x2060;
/// `DPDM_ERROR`: `DevPostDeviceModes` failed.
pub const DPDM_ERROR: i32 = -1;
/// `OD_QUEUED` / `OD_DIRECT`: printer device contexts.
pub const OD_QUEUED: u32 = 2;
pub const OD_DIRECT: u32 = 5;
/// DevEscape results (IBM *Presentation Manager Programming Reference*;
/// the Open Watcom headers do not define them): the escape failed, or the
/// device does not implement it.
pub const DEVESC_ERROR: i32 = -1;
pub const DEVESC_NOTIMPLEMENTED: i32 = 0;

/// The spooler enumerations and the information levels each documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplEnum {
    Queue,
    Printer,
    Device,
    /// Drivers, ports and queue processors: any level (the levels each
    /// accepts are not checked — unverified).
    Other,
}

impl SplEnum {
    /// Whether `level` is a documented information level of this
    /// enumeration: `PRQINFO0..6`, printers level 0 only, `PRDINFO0..3`.
    pub fn level_ok(self, level: u32) -> bool {
        match self {
            SplEnum::Queue => level <= 6,
            SplEnum::Printer => level == 0,
            SplEnum::Device => level <= 3,
            SplEnum::Other => true,
        }
    }
}

/// The result of an enumeration with no entries: the error code, before
/// any count is written. `computer` = a computer name was given.
pub fn enum_result(kind: SplEnum, level: u32, computer: bool) -> u32 {
    if computer {
        NERR_InvalidComputer
    } else if !kind.level_ok(level) {
        ERROR_INVALID_LEVEL
    } else {
        NO_ERROR
    }
}

/// The error of `SplQmOpen` with `named` = the open data names a queue.
pub fn qm_open_error(named: bool) -> u32 {
    if named { PMERR_SPL_QUEUE_NOT_FOUND } else { PMERR_SPL_NO_DEFAULT_QUEUE }
}

impl Loader {
    /// Whether the string at `psz` is non-empty (`psz` may be NULL).
    fn spl_named(&self, psz: u32) -> bool {
        psz != 0 && self.guest_read::<u8>(psz).is_some_and(|b| b != 0)
    }

    /// Write 0 to each of the guest ULONGs at `ptrs` that is not NULL.
    fn spl_zero(&self, ptrs: &[u32]) {
        for &p in ptrs {
            if p != 0 {
                let _ = self.guest_write::<u32>(p, 0);
            }
        }
    }

    /// `SplEnumQueue` / `SplEnumDevice` / `SplEnumDriver` / … : an empty list.
    #[allow(clippy::too_many_arguments)]
    pub fn spl_enum(&self, kind: SplEnum, psz_computer: u32, level: u32,
                    pc_returned: u32, pc_total: u32, pcb_needed: u32) -> u32 {
        let rc = enum_result(kind, level, self.spl_named(psz_computer));
        if rc == NO_ERROR {
            self.spl_zero(&[pc_returned, pc_total, pcb_needed]);
        }
        rc
    }

    /// `SplQueryQueue` / `SplQueryDevice`: the name given is not there.
    pub fn spl_query(&self, psz_computer: u32, pcb_needed: u32, not_found: u32) -> u32 {
        if self.spl_named(psz_computer) {
            return NERR_InvalidComputer;
        }
        self.spl_zero(&[pcb_needed]);
        not_found
    }

    /// `SplQmOpen(pszToken, lCount, pqmdopData)`: no queue to open. The
    /// first field of the open data (`QMOPENDATA`, as `DEVOPENSTRUC`) is the
    /// queue name.
    pub fn spl_qm_open(&self, tid: u32, l_count: u32, pqmdop: u32) -> u32 {
        let named = l_count as i32 >= 1 && pqmdop != 0
            && self.guest_read::<u32>(pqmdop).is_some_and(|p| self.spl_named(p));
        self.pm_error(tid, qm_open_error(named));
        SPL_ERROR
    }

    /// `SplQmStartDoc` / `SplQmWrite` / `SplQmEndDoc` / `SplQmClose` /
    /// `SplQmAbort` / `SplQmAbortDoc`: no handle is valid.
    pub fn spl_qm_invalid(&self, tid: u32) -> u32 {
        self.pm_error(tid, PMERR_SPL_INV_HSPL);
        SPL_ERROR
    }

    /// PMGPI's printer calls; `None` for other ordinals.
    pub(super) fn handle_print_call(&self, tid: u32, ordinal: u32, arg: &dyn Fn(u64) -> u32) -> Option<u32> {
        Some(match ordinal {
            605 => {
                // DevEscape(HDC hdc, LONG lCode, LONG lInCount, PBYTE pbInData,
                //           PLONG plOutCount, PBYTE pbOutData) → LONG
                self.dev_escape(tid, arg(4)) as u32
            }
            609 => {
                // DevPostDeviceModes(HAB hab, PDRIVDATA pdrivDriverData,
                //                    PSZ pszDriverName, PSZ pszDeviceName,
                //                    PSZ pszName, ULONG flOptions) → LONG
                self.pm_error(tid, PMERR_DRIVER_NOT_FOUND);
                DPDM_ERROR as u32
            }
            _ => return None,
        })
    }

    /// DevEscape on `hdc`: no escape is implemented by Warpine's device
    /// contexts (no printer, no `DEVESC_QUERYESCSUPPORT` answer either).
    fn dev_escape(&self, tid: u32, hdc: u32) -> i32 {
        use super::mutex_ext::MutexExt;
        if self.shared.window_mgr.lock_or_recover().dcs.contains_key(&hdc) {
            DEVESC_NOTIMPLEMENTED
        } else {
            self.pm_error(tid, PMERR_INV_HDC);
            DEVESC_ERROR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::api_registry;
    use crate::loader::mutex_ext::MutexExt;
    use crate::loader::vm_backend::mock::MockVcpu;
    use crate::loader::ApiResult;

    const BUF: u32 = 0x0008_0000;

    /// Call PMSPL ordinal `ord` with `args` through the registry.
    fn spl(l: &Loader, ord: u32, args: &[u32]) -> u32 {
        let e = api_registry::find(PMSPL_BASE + ord).expect("registered");
        let mut a = [0u32; 10];
        a[..args.len()].copy_from_slice(args);
        match (e.handler)(l, &mut MockVcpu::new(), 0, a) {
            ApiResult::Normal(v) => v,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn last_error(l: &Loader) -> u32 {
        l.shared.pm_errors.take(0) & 0xFFFF
    }

    /// The Dev* arms of `handle_print_call` sit at the ordinals of the
    /// import library snapshot (the Gpi* arms are checked in pm_gpi.rs).
    #[test]
    fn dev_arms_match_the_ordinal_table() {
        let table = include_str!("../../doc/os2_ordinals.md");
        let src = include_str!("pmspl.rs");
        for (ord, name) in [(605, "DevEscape"), (609, "DevPostDeviceModes")] {
            assert!(table.contains(&format!("| {ord} | {name} |")), "{name} is PMGPI.{ord}");
            assert!(src.contains(&format!("{ord} => {{\n                // {name}(")), "{name} arm");
        }
    }

    #[test]
    fn levels_and_computer_names() {
        assert_eq!(enum_result(SplEnum::Queue, 3, false), NO_ERROR);
        assert_eq!(enum_result(SplEnum::Queue, 6, false), NO_ERROR);
        assert_eq!(enum_result(SplEnum::Queue, 7, false), ERROR_INVALID_LEVEL);
        assert_eq!(enum_result(SplEnum::Printer, 0, false), NO_ERROR);
        assert_eq!(enum_result(SplEnum::Printer, 1, false), ERROR_INVALID_LEVEL);
        assert_eq!(enum_result(SplEnum::Device, 3, false), NO_ERROR);
        assert_eq!(enum_result(SplEnum::Device, 4, false), ERROR_INVALID_LEVEL);
        assert_eq!(enum_result(SplEnum::Other, 99, false), NO_ERROR);
        assert_eq!(enum_result(SplEnum::Queue, 3, true), NERR_InvalidComputer, "no network spooler");
        assert_eq!(qm_open_error(true), PMERR_SPL_QUEUE_NOT_FOUND);
        assert_eq!(qm_open_error(false), PMERR_SPL_NO_DEFAULT_QUEUE);
    }

    #[test]
    fn enumerations_return_empty_lists() {
        let l = Loader::new_mock();
        for p in 0..3 { l.guest_write::<u32>(BUF + 4 * p, 0xDEAD).unwrap(); }
        let (ret, tot, need) = (BUF, BUF + 4, BUF + 8);
        // SplEnumQueue(NULL, 3, pBuf, cbBuf, &ret, &tot, &need, NULL)
        assert_eq!(spl(&l, 399, &[0, 3, BUF + 64, 256, ret, tot, need, 0]), NO_ERROR);
        for p in [ret, tot, need] { assert_eq!(l.guest_read::<u32>(p), Some(0)); }
        // SplEnumPrinter(NULL, 0, SPL_PR_QUEUE, …)
        l.guest_write::<u32>(ret, 7).unwrap();
        assert_eq!(spl(&l, 441, &[0, 0, 1, BUF + 64, 256, ret, tot, need, 0]), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(ret), Some(0));
        assert_eq!(spl(&l, 441, &[0, 1, 1, BUF + 64, 256, ret, tot, need, 0]), ERROR_INVALID_LEVEL);
        for ord in [382, 406, 407, 408] {
            assert_eq!(spl(&l, ord, &[0, 0, 0, 0, ret, tot, need, 0]), NO_ERROR, "ordinal {ord}");
        }
        // NULL count pointers are not written.
        assert_eq!(spl(&l, 399, &[0, 0, 0, 0, 0, 0, 0, 0]), NO_ERROR);
        // A computer name.
        l.guest_write_bytes(BUF + 32, b"\\\\SERVER\0").unwrap();
        assert_eq!(spl(&l, 399, &[BUF + 32, 3, 0, 0, ret, tot, need, 0]), NERR_InvalidComputer);
    }

    #[test]
    fn queries_find_nothing() {
        let l = Loader::new_mock();
        l.guest_write_bytes(BUF + 32, b"LPT1Q\0").unwrap();
        l.guest_write::<u32>(BUF, 99).unwrap();
        assert_eq!(spl(&l, 397, &[0, BUF + 32, 3, BUF + 64, 256, BUF]), NERR_QNotFound);
        assert_eq!(l.guest_read::<u32>(BUF), Some(0));
        assert_eq!(spl(&l, 381, &[0, BUF + 32, 0, BUF + 64, 256, BUF]), NERR_DestNotFound);
    }

    #[test]
    fn queue_manager_calls_fail_with_the_documented_errors() {
        let l = Loader::new_mock();
        // DEVOPENSTRUC-like data: queue name first.
        l.guest_write_bytes(BUF + 32, b"PRINTQ\0").unwrap();
        l.guest_write::<u32>(BUF, BUF + 32).unwrap();
        assert_eq!(spl(&l, 301, &[BUF + 40, 4, BUF]), SPL_ERROR);
        assert_eq!(last_error(&l), PMERR_SPL_QUEUE_NOT_FOUND);
        assert_eq!(spl(&l, 301, &[0, 0, 0]), SPL_ERROR);
        assert_eq!(last_error(&l), PMERR_SPL_NO_DEFAULT_QUEUE);
        for (ord, args) in [(302, vec![1, 0]), (303, vec![1]), (304, vec![1, 4, BUF]),
                            (305, vec![1]), (306, vec![1]), (308, vec![1])] {
            assert_eq!(spl(&l, ord, &args), SPL_ERROR, "ordinal {ord}");
            assert_eq!(last_error(&l), PMERR_SPL_INV_HSPL, "ordinal {ord}");
        }
    }

    #[test]
    fn device_escapes_and_job_properties_are_not_available() {
        let l = Loader::new_mock();
        let hdc = l.shared.window_mgr.lock_or_recover().open_dc(crate::loader::pm_types::DeviceContext::Memory);
        let arg = |args: [u32; 6]| move |off: u64| args[(off / 4) as usize - 1];
        assert_eq!(l.handle_print_call(0, 605, &arg([hdc, 8150, 0, 0, 0, 0])), Some(DEVESC_NOTIMPLEMENTED as u32));
        assert_eq!(l.handle_print_call(0, 605, &arg([0xBAD, 8150, 0, 0, 0, 0])), Some(DEVESC_ERROR as u32));
        assert_eq!(last_error(&l), PMERR_INV_HDC);
        assert_eq!(l.handle_print_call(0, 609, &arg([0; 6])), Some(DPDM_ERROR as u32));
        assert_eq!(last_error(&l), PMERR_DRIVER_NOT_FOUND);
        assert_eq!(l.handle_print_call(0, 610, &arg([0; 6])), None, "DevOpenDC is pm_bitmap.rs's");
    }
}
