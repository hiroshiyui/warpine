// SPDX-License-Identifier: GPL-3.0-only
//
//! `WARPINE.DLL`: Warpine's own extensions for programs that can be
//! recompiled (ADR-0039). Not an OS/2 module — a program must check that
//! `DosLoadModule("WARPINE")` succeeds, so it still runs on OS/2.
//!
//! The 64-bit time calls give a signed 64-bit count of seconds since
//! 1970-01-01 00:00 UTC (`LONGLONG`, little-endian) on the session clock
//! (`clock.rs`), which neither ends in 2038 (a signed 32-bit `time_t`) nor
//! in 2106 (`QSV_TIME_LOW`, Open Watcom's unsigned `time_t`). All take
//! pointers, so the `_System` (cdecl) argument list stays 32-bit:
//!
//! | Ordinal | Call | |
//! |---|---|---|
//! | 1 | `WrpTime64(PLONGLONG ptime)` | the current time |
//! | 2 | `WrpGetDateTime64(PDATETIME pdt, PLONGLONG ptime)` | the current time as a local `DATETIME` and as seconds, read once |
//! | 3 | `WrpLocalTime64(const LONGLONG *ptime, PDATETIME pdt)` | seconds → local `DATETIME` |
//! | 4 | `WrpMakeTime64(const DATETIME *pdt, PLONGLONG ptime)` | local `DATETIME` → seconds |
//!
//! Errors: `ERROR_INVALID_PARAMETER` for a null or unreadable pointer, or a
//! time whose year is outside 1–9999; `ERROR_TS_DATETIME` for a `DATETIME`
//! field out of range. `pdt->hundredths` is 0 from `WrpLocalTime64` and
//! ignored by `WrpMakeTime64`, as are the weekday and time zone.

use log::debug;

use super::clock::{self, LocalTime};
use super::constants::*;

/// The years the conversions accept.
pub const WRP_MIN_YEAR: i32 = 1;
pub const WRP_MAX_YEAR: i32 = 9999;

const MS_PER_SECOND: i64 = 1000;

impl super::Loader {
    /// Write a `LONGLONG` at `p`.
    fn write_longlong(&self, p: u32, v: i64) -> u32 {
        match (p != 0).then(|| self.guest_write_bytes(p, &v.to_le_bytes())).flatten() {
            Some(()) => NO_ERROR,
            None => ERROR_INVALID_PARAMETER,
        }
    }

    /// `secs` as local time within the accepted years.
    fn wrp_local(secs: i64) -> Option<LocalTime> {
        clock::unix_to_local(secs).filter(|lt| (WRP_MIN_YEAR..=WRP_MAX_YEAR).contains(&lt.year))
    }

    /// `WrpTime64` (WARPINE.1).
    pub fn wrp_time64(&self, p_time: u32) -> u32 {
        debug!("  WrpTime64");
        self.write_longlong(p_time, self.shared.clock.now_secs())
    }

    /// `WrpGetDateTime64` (WARPINE.2): both forms of one reading.
    pub fn wrp_get_date_time64(&self, p_dt: u32, p_time: u32) -> u32 {
        debug!("  WrpGetDateTime64");
        if p_dt == 0 || p_time == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let ms = self.shared.clock.now_ms();
        let secs = ms.div_euclid(MS_PER_SECOND);
        let Some(lt) = Self::wrp_local(secs) else { return ERROR_INVALID_PARAMETER };
        let rc = self.write_datetime(p_dt, &lt, (ms.rem_euclid(MS_PER_SECOND) / 10) as u8);
        if rc != NO_ERROR {
            return rc;
        }
        self.write_longlong(p_time, secs)
    }

    /// `WrpLocalTime64` (WARPINE.3).
    pub fn wrp_local_time64(&self, p_time: u32, p_dt: u32) -> u32 {
        let Some(b) = (p_time != 0).then(|| self.guest_read_bytes(p_time, 8)).flatten() else {
            return ERROR_INVALID_PARAMETER;
        };
        let secs = i64::from_le_bytes(b.try_into().expect("8 bytes"));
        debug!("  WrpLocalTime64({secs})");
        if p_dt == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        match Self::wrp_local(secs) {
            Some(lt) => self.write_datetime(p_dt, &lt, 0),
            None => ERROR_INVALID_PARAMETER,
        }
    }

    /// `WrpMakeTime64` (WARPINE.4).
    pub fn wrp_make_time64(&self, p_dt: u32, p_time: u32) -> u32 {
        let Some((lt, _)) = (p_dt != 0).then(|| self.read_datetime(p_dt)).flatten() else {
            return ERROR_INVALID_PARAMETER;
        };
        debug!("  WrpMakeTime64({lt:?})");
        if !(WRP_MIN_YEAR..=WRP_MAX_YEAR).contains(&lt.year) {
            return ERROR_INVALID_PARAMETER;
        }
        match clock::local_to_unix(&lt) {
            Some(secs) => self.write_longlong(p_time, secs),
            None => ERROR_TS_DATETIME,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;

    use crate::loader::mutex_ext::MutexExt;

    /// 2200-01-01 00:00 UTC.
    const Y2200: i64 = 7_258_118_400;

    /// A mock loader and guest buffers for a `DATETIME` and a `LONGLONG`.
    fn setup() -> (Loader, u32, u32) {
        let l = Loader::new_mock();
        let buf = l.shared.mem_mgr.lock_or_recover().alloc(64).unwrap();
        (l, buf, buf + 32)
    }

    fn longlong(l: &Loader, p: u32) -> i64 {
        i64::from_le_bytes(l.guest_read_bytes(p, 8).unwrap().try_into().unwrap())
    }

    fn year(l: &Loader, p_dt: u32) -> u16 {
        l.guest_read::<u16>(p_dt + 6).unwrap()
    }

    #[test]
    fn time64_follows_the_session_clock_past_2106() {
        let (l, p_dt, p_time) = setup();
        l.shared.clock.set_now_ms(Y2200 * MS_PER_SECOND);
        assert_eq!(l.wrp_time64(p_time), NO_ERROR);
        assert!((longlong(&l, p_time) - Y2200).abs() < 5);
        assert_eq!(l.wrp_get_date_time64(p_dt, p_time), NO_ERROR);
        assert!((longlong(&l, p_time) - Y2200).abs() < 5);
        assert!((2199..=2200).contains(&year(&l, p_dt)), "year {}", year(&l, p_dt));
        assert_eq!(l.wrp_time64(0), ERROR_INVALID_PARAMETER);
        assert_eq!(l.wrp_get_date_time64(0, p_time), ERROR_INVALID_PARAMETER);
    }

    #[test]
    fn local_and_make_time64_round_trip() {
        let (l, p_dt, p_time) = setup();
        for secs in [-86_400, 0, (1 << 31), (1 << 32), Y2200] {
            l.guest_write_bytes(p_time, &secs.to_le_bytes()).unwrap();
            assert_eq!(l.wrp_local_time64(p_time, p_dt), NO_ERROR);
            l.guest_write_bytes(p_time, &[0; 8]).unwrap();
            assert_eq!(l.wrp_make_time64(p_dt, p_time), NO_ERROR);
            assert_eq!(longlong(&l, p_time), secs);
        }
    }

    #[test]
    fn conversions_reject_bad_input() {
        let (l, p_dt, p_time) = setup();
        // Far beyond year 9999.
        l.guest_write_bytes(p_time, &(Y2200 * 1000).to_le_bytes()).unwrap();
        assert_eq!(l.wrp_local_time64(p_time, p_dt), ERROR_INVALID_PARAMETER);
        assert_eq!(l.wrp_local_time64(0, p_dt), ERROR_INVALID_PARAMETER);
        // 2039-02-29 does not exist.
        let lt = LocalTime::new(2039, 2, 29, 0, 0, 0);
        assert_eq!(l.write_datetime(p_dt, &lt, 0), NO_ERROR);
        assert_eq!(l.wrp_make_time64(p_dt, p_time), ERROR_TS_DATETIME);
        assert_eq!(l.wrp_make_time64(0, p_time), ERROR_INVALID_PARAMETER);
        let lt = LocalTime::new(2039, 3, 1, 0, 0, 0);
        assert_eq!(l.write_datetime(p_dt, &lt, 0), NO_ERROR);
        assert_eq!(l.wrp_make_time64(p_dt, 0), ERROR_INVALID_PARAMETER);
    }
}
