// SPDX-License-Identifier: GPL-3.0-or-later
//
//! The 16-bit information segments of `DosGetInfoSeg` (DOSCALLS 8).
//!
//! `DosGetInfoSeg(PSEL pselGlobal, PSEL pselLocal)` gives a program two
//! selectors; `selector:0` addresses a `GINFOSEG` (system-wide: the time and
//! date, version, scheduler values) and a `LINFOSEG` (the process: ids,
//! priority, environment). Layouts from the OS/2 1.x toolkit header
//! (Open Watcom `h/os21x/bsedos.h`).
//!
//! Each segment sits at the start of its own reserved 64 KB tile
//! ([`GINFOSEG_ADDR`], [`LINFOSEG_ADDR`]), so the tiled data selector of
//! that tile (as `DosFlatToSel` forms it) addresses it at offset 0 and
//! `DosSelToFlat` maps it back. Programs read the `GINFOSEG` time fields
//! directly instead of calling `DosGetDateTime`, so a host thread refreshes
//! them every clock tick ([`INFOSEG_REFRESH`]) from the session clock
//! (`clock.rs`) once the first call has made them.
//!
//! The `LINFOSEG` describes the thread that called `DosGetInfoSeg`
//! (`tidCurrent`), filled at that call. Unverified (no source): the sign of
//! `GINFOSEG.timezone` (taken as `DATETIME`'s, minutes west of UTC), the
//! session numbers (`sgCurrent` 1, `sgMax` / `csgWindowableVioMax` /
//! `csgPMMax` as `DosQuerySysInfo` reports them), `prtyCurrent` (regular
//! class, level 0), `cbDataSegment` / `cbStack` / `cbHeap` / `selDS` (0 for
//! a 32-bit program). A 32-bit program that calls `DosGetInfoSeg` through a
//! Far16 thunk does not get here: Warpine bypasses such thunks (vcpu.rs).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use log::debug;

use super::clock::{self, LocalTime};
use super::constants::*;
use super::SharedState;

/// Guest address of the `GINFOSEG`: a tile of its own in the system area
/// (ADR-0094), out of every program's way.
pub const GINFOSEG_ADDR: u32 = SYSTEM_AREA_BASE + 0x2_0000;
/// Guest address of the `LINFOSEG`: the next tile.
pub const LINFOSEG_ADDR: u32 = SYSTEM_AREA_BASE + 0x3_0000;
/// Size of a `GINFOSEG`.
pub const GINFOSEG_SIZE: usize = 72;
/// Size of a `LINFOSEG`.
pub const LINFOSEG_SIZE: usize = 30;
/// How often the `GINFOSEG` time fields are refreshed: the clock tick.
pub const INFOSEG_REFRESH: Duration = Duration::from_millis(DOS_TIMER_TICK_MS as u64);

// Both segments start a tile, so their selectors address them at offset 0.
const _: () = assert!(GINFOSEG_ADDR.is_multiple_of(TILE_SIZE) && LINFOSEG_ADDR.is_multiple_of(TILE_SIZE));
// In the system area, after the TIB / PIB / thread-local page.
const _: () = assert!(GINFOSEG_ADDR > TLM_PAGE && LINFOSEG_ADDR + TILE_SIZE <= SYSTEM_AREA_BASE + SYSTEM_AREA_SIZE);

/// `GINFOSEG` offsets around the fields the refresher rewrites.
const TIMER_INTERVAL_OFFSET: usize = 14;
const DAY_OFFSET: usize = 16;
/// End of the time and date fields (after `weekday`).
const TIME_FIELDS_END: usize = 21;

/// The values `DosQuerySysInfo` reports, repeated in the `GINFOSEG`.
const MAX_SESSIONS: u8 = 16;
const MAX_WAIT_SECONDS: u8 = 5;
const MIN_SLICE_MS: u16 = 32;
const MAX_SLICE_MS: u16 = 248;
const BOOT_DRIVE_C: u16 = 3;
/// `DosGetHugeShift`: huge segments are 1 << 3 selectors apart.
const HUGE_SHIFT: u8 = 3;
/// The foreground session.
const CURRENT_SESSION: u8 = 1;
/// `PRTYC_REGULAR` << 8 | level 0.
const PRIORITY_REGULAR: u16 = 2 << 8;
/// PIB byte offset of `pib_ultype` (the process type).
const PIB_ULTYPE_OFFSET: u32 = 0x18;
/// PIB byte offset of `pib_hmte` (the executable's module handle).
const PIB_HMTE_OFFSET: u32 = 0x08;

/// The tiled data selector whose tile starts at `addr`.
pub fn tile_selector(addr: u32) -> u16 {
    ((TILED_SEL_START_INDEX + addr / TILE_SIZE) * 8) as u16
}

/// The time fields of a `GINFOSEG`: seconds since 1970 (wrapping in 2106,
/// as the field is 32-bit), milliseconds since boot, and the local date and
/// time with its hundredths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InfoTime {
    pub unix_secs: u32,
    pub msecs: u32,
    pub local: LocalTime,
    pub hundredths: u8,
}

/// Write the time fields of `t` into a `GINFOSEG` image.
pub fn put_time(g: &mut [u8; GINFOSEG_SIZE], t: &InfoTime) {
    g[0..4].copy_from_slice(&t.unix_secs.to_le_bytes());
    g[4..8].copy_from_slice(&t.msecs.to_le_bytes());
    g[8] = t.local.hours;
    g[9] = t.local.minutes;
    g[10] = t.local.seconds;
    g[11] = t.hundredths;
    g[12..14].copy_from_slice(&t.local.os2_timezone().to_le_bytes());
    g[16] = t.local.day;
    g[17] = t.local.month;
    g[18..20].copy_from_slice(&(t.local.year.clamp(0, i32::from(u16::MAX)) as u16).to_le_bytes());
    g[20] = t.local.weekday;
}

/// A whole `GINFOSEG` for time `t` in the process `pid` (the foreground).
pub fn build_ginfoseg(t: &InfoTime, pid: u16) -> [u8; GINFOSEG_SIZE] {
    let mut g = [0u8; GINFOSEG_SIZE];
    put_time(&mut g, t);
    // cusecTimerInterval: tenths of a millisecond, as QSV_TIMER_INTERVAL.
    g[14..16].copy_from_slice(&((DOS_TIMER_TICK_MS * 10) as u16).to_le_bytes());
    g[21] = OS2_VERSION_MAJOR as u8;
    g[22] = OS2_VERSION_MINOR as u8;
    g[23] = 0; // chRevisionLetter
    g[24] = CURRENT_SESSION; // sgCurrent
    g[25] = MAX_SESSIONS; // sgMax
    g[26] = HUGE_SHIFT;
    g[27] = 1; // fProtectModeOnly
    g[28..30].copy_from_slice(&pid.to_le_bytes()); // pidForeground
    g[30] = 1; // fDynamicSched
    g[31] = MAX_WAIT_SECONDS;
    g[32..34].copy_from_slice(&MIN_SLICE_MS.to_le_bytes());
    g[34..36].copy_from_slice(&MAX_SLICE_MS.to_le_bytes());
    g[36..38].copy_from_slice(&BOOT_DRIVE_C.to_le_bytes());
    // amecRAS[32] at 38..70 stays 0.
    g[70] = MAX_SESSIONS; // csgWindowableVioMax
    g[71] = MAX_SESSIONS; // csgPMMax
    g
}

/// What a `LINFOSEG` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalInfo {
    pub pid: u16,
    pub ppid: u16,
    pub tid: u16,
    pub process_type: u8,
    pub env_selector: u16,
    pub cmdline_offset: u16,
    pub hmod: u16,
}

/// A `LINFOSEG` image.
pub fn build_linfoseg(l: &LocalInfo) -> [u8; LINFOSEG_SIZE] {
    let mut b = [0u8; LINFOSEG_SIZE];
    b[0..2].copy_from_slice(&l.pid.to_le_bytes());
    b[2..4].copy_from_slice(&l.ppid.to_le_bytes());
    b[4..6].copy_from_slice(&PRIORITY_REGULAR.to_le_bytes());
    b[6..8].copy_from_slice(&l.tid.to_le_bytes());
    b[8..10].copy_from_slice(&u16::from(CURRENT_SESSION).to_le_bytes()); // sgCurrent
    // rfProcStatus, dummy1 at 10, 11 stay 0.
    b[12..14].copy_from_slice(&1u16.to_le_bytes()); // fForeground
    b[14] = l.process_type;
    // dummy2 at 15.
    b[16..18].copy_from_slice(&l.env_selector.to_le_bytes());
    b[18..20].copy_from_slice(&l.cmdline_offset.to_le_bytes());
    // cbDataSegment, cbStack, cbHeap at 20..26 stay 0.
    b[26..28].copy_from_slice(&l.hmod.to_le_bytes());
    // selDS at 28 stays 0.
    b
}

/// The time fields now, on the session clock.
fn info_time(shared: &SharedState) -> Option<InfoTime> {
    let ms = shared.clock.now_ms();
    let local = clock::unix_to_local(ms.div_euclid(1000))?;
    Some(InfoTime {
        unix_secs: ms.div_euclid(1000).max(0) as u32,
        msecs: super::stubs::ms_count(),
        local,
        hundredths: (ms.rem_euclid(1000) / 10) as u8,
    })
}

/// Refresh the `GINFOSEG` time fields every tick until the process ends.
fn refresh_ginfoseg(shared: Arc<SharedState>) {
    while !shared.exit_requested.load(Ordering::Relaxed) {
        std::thread::sleep(INFOSEG_REFRESH);
        let Some(t) = info_time(&shared) else { continue };
        let mut g = [0u8; GINFOSEG_SIZE];
        put_time(&mut g, &t);
        // The time fields only: cusecTimerInterval (14..16) is left alone.
        let ok = shared.guest_mem.write_bytes(GINFOSEG_ADDR, &g[..TIMER_INTERVAL_OFFSET])
            .and_then(|_| shared.guest_mem.write_bytes(GINFOSEG_ADDR + DAY_OFFSET as u32, &g[DAY_OFFSET..TIME_FIELDS_END]));
        if ok.is_none() {
            return;
        }
    }
}

impl super::Loader {
    /// `DosGetInfoSeg` (DOSCALLS 8): build both segments, start the
    /// `GINFOSEG` refresher on the first call, and write the two selectors
    /// to `p_global_sel` / `p_local_sel` (flat addresses of `SEL`s).
    pub fn dos_get_info_seg(&self, vcpu_id: u32, p_global_sel: u32, p_local_sel: u32) -> u32 {
        debug!("DosGetInfoSeg(pGlobal=0x{:08X}, pLocal=0x{:08X})", p_global_sel, p_local_sel);
        if p_global_sel == 0 || p_local_sel == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let Some(t) = info_time(&self.shared) else { return ERROR_INVALID_PARAMETER };
        let pid = self.shared.process_id() as u16;
        let g = build_ginfoseg(&t, pid);
        let env = self.guest_read::<u32>(PIB_BASE + PIB_PCHENV_OFFSET).unwrap_or(0);
        let cmd = self.guest_read::<u32>(PIB_BASE + PIB_PCHCMD_OFFSET).unwrap_or(0);
        let local = LocalInfo {
            pid,
            ppid: self.shared.parent_process_id() as u16,
            tid: super::threads::tid_of_vcpu(vcpu_id) as u16,
            process_type: self.guest_read::<u32>(PIB_BASE + PIB_ULTYPE_OFFSET).unwrap_or(0) as u8,
            env_selector: if env != 0 { tile_selector(env & !(TILE_SIZE - 1)) } else { 0 },
            // The command line's offset in the environment's tile, when it is there.
            cmdline_offset: if cmd != 0 && cmd / TILE_SIZE == env / TILE_SIZE { (cmd % TILE_SIZE) as u16 } else { 0 },
            hmod: self.guest_read::<u32>(PIB_BASE + PIB_HMTE_OFFSET).unwrap_or(0) as u16,
        };
        let written = self.guest_write_bytes(GINFOSEG_ADDR, &g)
            .and_then(|_| self.guest_write_bytes(LINFOSEG_ADDR, &build_linfoseg(&local)))
            .and_then(|_| self.guest_write::<u16>(p_global_sel, tile_selector(GINFOSEG_ADDR)))
            .and_then(|_| self.guest_write::<u16>(p_local_sel, tile_selector(LINFOSEG_ADDR)));
        if written.is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        if !self.shared.infoseg_refresher.swap(true, Ordering::AcqRel) {
            let shared = Arc::clone(&self.shared);
            if std::thread::Builder::new().name("infoseg".into()).spawn(move || refresh_ginfoseg(shared)).is_err() {
                self.shared.infoseg_refresher.store(false, Ordering::Release);
            }
        }
        NO_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;
    use crate::loader::mutex_ext::MutexExt;

    fn t(year: i32) -> InfoTime {
        let local = LocalTime { weekday: 4, utc_offset_secs: 9 * 3600, ..LocalTime::new(year, 1, 2, 3, 4, 5) };
        InfoTime { unix_secs: 0x8000_0001, msecs: 1234, local, hundredths: 56 }
    }

    #[test]
    fn ginfoseg_follows_the_toolkit_layout() {
        let g = build_ginfoseg(&t(2110), 42);
        assert_eq!(u32::from_le_bytes(g[0..4].try_into().unwrap()), 0x8000_0001, "time past 2038");
        assert_eq!(u32::from_le_bytes(g[4..8].try_into().unwrap()), 1234);
        assert_eq!(&g[8..12], &[3, 4, 5, 56]);
        assert_eq!(i16::from_le_bytes([g[12], g[13]]), -540, "minutes west of UTC");
        assert_eq!(u16::from_le_bytes([g[14], g[15]]), 320, "timer interval in tenths of a ms");
        assert_eq!((g[16], g[17]), (2, 1));
        assert_eq!(u16::from_le_bytes([g[18], g[19]]), 2110);
        assert_eq!(g[20], 4);
        assert_eq!((g[21], g[22]), (20, 45));
        assert_eq!(u16::from_le_bytes([g[28], g[29]]), 42);
        assert_eq!(u16::from_le_bytes([g[36], g[37]]), 3, "boot drive C:");
        assert_eq!(g[71], 16);
    }

    #[test]
    fn linfoseg_follows_the_toolkit_layout() {
        let l = build_linfoseg(&LocalInfo { pid: 42, ppid: 7, tid: 3, process_type: 2,
                                             env_selector: 0x0130, cmdline_offset: 0x40, hmod: 1 });
        assert_eq!(&l[0..8], &[42, 0, 7, 0, 0, 2, 3, 0]);
        assert_eq!(l[14], 2);
        assert_eq!(&l[16..20], &[0x30, 0x01, 0x40, 0]);
        assert_eq!(&l[26..28], &[1, 0]);
    }

    #[test]
    fn selectors_address_the_segments_at_offset_zero() {
        let l = Loader::new_mock();
        let sel = tile_selector(GINFOSEG_ADDR);
        assert_eq!(l.dos_sel_to_flat(u32::from(sel) << 16), GINFOSEG_ADDR);
        assert_eq!(l.dos_flat_to_sel(LINFOSEG_ADDR), u32::from(tile_selector(LINFOSEG_ADDR)) << 16);
    }

    #[test]
    fn dos_get_info_seg_fills_both_segments_and_keeps_the_time_current() {
        let l = Loader::new_mock();
        l.shared.clock.set_now_ms(4_102_444_800_000); // 2100-01-01 UTC
        let p = l.shared.mem_mgr.lock_or_recover().alloc(8).unwrap();
        assert_eq!(l.dos_get_info_seg(0, 0, p), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_get_info_seg(0, p, p + 2), NO_ERROR);
        assert_eq!(l.guest_read::<u16>(p), Some(tile_selector(GINFOSEG_ADDR)));
        assert_eq!(l.guest_read::<u16>(p + 2), Some(tile_selector(LINFOSEG_ADDR)));
        let year = l.guest_read::<u16>(GINFOSEG_ADDR + 18).unwrap();
        assert!((2099..=2100).contains(&year), "year {year}");
        assert_eq!(l.guest_read::<u16>(LINFOSEG_ADDR), Some(l.shared.process_id() as u16));
        assert_eq!(l.guest_read::<u16>(LINFOSEG_ADDR + 6), Some(1), "thread 1");
        // The refresher follows the clock without another call.
        l.shared.clock.set_now_ms(4_102_444_800_000 + 3_600_000);
        std::thread::sleep(INFOSEG_REFRESH * 6);
        let secs = l.guest_read::<u32>(GINFOSEG_ADDR).unwrap();
        assert!(secs >= 4_102_444_800 + 3_600 - 5, "time refreshed: {secs}");
        l.shared.exit_requested.store(true, Ordering::Relaxed);
    }
}
