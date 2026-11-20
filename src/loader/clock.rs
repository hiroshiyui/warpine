// SPDX-License-Identifier: GPL-3.0-or-later
//
//! The session clock and date conversions (ADR-0039).
//!
//! Warpine cannot set the host's clock, so `DosSetDateTime` moves a
//! per-process **clock offset** instead: [`SessionClock`] is the host's time
//! plus that offset, and every call that reports the current date reads it —
//! `DosGetDateTime`, `DosQuerySysInfo(QSV_TIME_LOW / QSV_TIME_HIGH)` and the
//! `WARPINE.DLL` calls (`warpine_api.rs`). A child process starts with its
//! parent's offset (`WARPINE_CLOCK_OFFSET`, set by `inherit::admit_child`);
//! the first process of a session can start at any date with
//! `WARPINE_CLOCK=YYYY-MM-DD[THH:MM[:SS]]` (local time), which is how the
//! tests run in 2040 or 2110.
//!
//! Times are kept as signed 64-bit milliseconds / seconds since
//! 1970-01-01 00:00 UTC on the host, whose `time_t` is 64-bit (checked at
//! compile time), so nothing here ends in 2038. The 32-bit limits that
//! remain belong to the OS/2 interfaces themselves: `FDATE` holds years
//! 1980–2107 ([`pack_fdate_ftime`] clamps to them) and `QSV_TIME_LOW` wraps
//! in 2106 (`QSV_TIME_HIGH` holds the rest).
//!
//! Unverified (no source): what OS/2 reports for host file times outside
//! the `FDATE` range (Warpine clamps), and which years `DosSetDateTime`
//! accepts beyond the calendar checks (Warpine takes 1980–9999).

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use log::warn;

/// The start date of the first process of a session, in local time:
/// `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM` or `YYYY-MM-DDTHH:MM:SS` (a space may
/// stand for the `T`).
pub const CLOCK_ENV: &str = "WARPINE_CLOCK";
/// The clock offset a child process inherits, in milliseconds (signed
/// decimal). Set by the parent; it takes precedence over [`CLOCK_ENV`].
pub const CLOCK_OFFSET_ENV: &str = "WARPINE_CLOCK_OFFSET";

/// The first year `DosSetDateTime` and `WARPINE_CLOCK` accept.
pub const MIN_SET_YEAR: i32 = 1980;
/// The last year `DosSetDateTime` and `WARPINE_CLOCK` accept.
pub const MAX_SET_YEAR: i32 = 9999;
/// The first year an `FDATE` can hold (its 7-bit year counts from 1980).
pub const FDATE_MIN_YEAR: i32 = 1980;
/// The last year an `FDATE` can hold (1980 + 127).
pub const FDATE_MAX_YEAR: i32 = FDATE_MIN_YEAR + 127;

const MS_PER_SECOND: i64 = 1000;
const SECONDS_PER_MINUTE: i32 = 60;

// Every conversion here relies on a 64-bit host `time_t`.
const _: () = assert!(std::mem::size_of::<libc::time_t>() == 8);

/// The clock of this process: the host's time plus an offset.
#[derive(Debug, Default)]
pub struct SessionClock {
    offset_ms: AtomicI64,
}

impl SessionClock {
    /// A clock `offset_ms` milliseconds from the host's.
    pub fn with_offset(offset_ms: i64) -> Self {
        SessionClock { offset_ms: AtomicI64::new(offset_ms) }
    }

    /// The clock of a new process: its parent's offset
    /// ([`CLOCK_OFFSET_ENV`]), else the start date in [`CLOCK_ENV`], else
    /// the host's time. Malformed values are reported and ignored.
    pub fn from_env() -> Self {
        if let Some(v) = std::env::var_os(CLOCK_OFFSET_ENV) {
            match v.to_str().and_then(|s| s.parse::<i64>().ok()) {
                Some(ms) => return Self::with_offset(ms),
                None => warn!("{CLOCK_OFFSET_ENV}={v:?} is not a number of milliseconds; ignored"),
            }
        }
        if let Some(v) = std::env::var_os(CLOCK_ENV) {
            match v.to_str().and_then(parse_clock_setting).and_then(|f| local_to_unix(&f)) {
                Some(secs) => {
                    let clock = Self::default();
                    clock.set_now_ms(secs.saturating_mul(MS_PER_SECOND));
                    return clock;
                }
                None => warn!("{CLOCK_ENV}={v:?} is not a date YYYY-MM-DD[THH:MM[:SS]] \
                               from {MIN_SET_YEAR} to {MAX_SET_YEAR}; ignored"),
            }
        }
        Self::default()
    }

    /// The offset from the host's clock, in milliseconds.
    pub fn offset_ms(&self) -> i64 {
        self.offset_ms.load(Ordering::Relaxed)
    }

    /// Milliseconds since 1970-01-01 00:00 UTC on this clock.
    pub fn now_ms(&self) -> i64 {
        host_now_ms().saturating_add(self.offset_ms())
    }

    /// Seconds since 1970-01-01 00:00 UTC on this clock.
    pub fn now_secs(&self) -> i64 {
        self.now_ms().div_euclid(MS_PER_SECOND)
    }

    /// Make this clock read `target_ms` now (`DosSetDateTime`).
    pub fn set_now_ms(&self, target_ms: i64) {
        self.offset_ms.store(target_ms.saturating_sub(host_now_ms()), Ordering::Relaxed);
    }
}

/// The host's time in milliseconds since 1970-01-01 00:00 UTC (negative
/// before it).
fn host_now_ms() -> i64 {
    system_time_ms(SystemTime::now())
}

/// `t` in milliseconds since 1970-01-01 00:00 UTC, saturating.
pub fn system_time_ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    }
}

/// `t` in whole seconds since 1970-01-01 00:00 UTC (rounded down).
pub fn system_time_secs(t: SystemTime) -> i64 {
    system_time_ms(t).div_euclid(MS_PER_SECOND)
}

/// The `SystemTime` `secs` seconds from 1970-01-01 00:00 UTC.
pub fn unix_to_system_time(secs: i64) -> SystemTime {
    if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    }
}

/// A calendar date and time of day. `weekday` (0 = Sunday) and
/// `utc_offset_secs` (east of UTC positive) are outputs of [`unix_to_local`]
/// and ignored by [`local_to_unix`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub weekday: u8,
    pub utc_offset_secs: i32,
}

impl LocalTime {
    /// A date and time of day (weekday and offset 0).
    pub const fn new(year: i32, month: u8, day: u8, hours: u8, minutes: u8, seconds: u8) -> Self {
        LocalTime { year, month, day, hours, minutes, seconds, weekday: 0, utc_offset_secs: 0 }
    }

    /// Whether the fields name a real date and time of day
    /// (`seconds` up to 59; leap seconds are not representable).
    pub fn is_valid(&self) -> bool {
        (1..=12).contains(&self.month)
            && self.day >= 1
            && self.day <= days_in_month(self.year, self.month)
            && self.hours < 24
            && self.minutes < 60
            && self.seconds < 60
    }

    /// The `DATETIME` time-zone field: minutes **west** of UTC, as the
    /// Control Program Reference defines it ("positive if it is earlier
    /// than GMT").
    pub fn os2_timezone(&self) -> i16 {
        (-self.utc_offset_secs / SECONDS_PER_MINUTE) as i16
    }
}

/// Whether `year` is a Gregorian leap year.
pub const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// The number of days of `month` (1–12) in `year`; 0 for another month.
pub const fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// `secs` since 1970-01-01 00:00 UTC in the host's local time zone, or
/// `None` when the year does not fit.
pub fn unix_to_local(secs: i64) -> Option<LocalTime> {
    let t: libc::time_t = secs;
    // SAFETY: an all-zero `tm` is a valid value (integers and a null
    // `tm_zone` pointer), and localtime_r only writes it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers refer to live locals; localtime_r is the
    // reentrant variant and keeps no reference to them.
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return None;
    }
    let year = tm.tm_year.checked_add(1900)?;
    Some(LocalTime {
        year,
        month: (tm.tm_mon + 1) as u8,
        day: tm.tm_mday as u8,
        hours: tm.tm_hour as u8,
        minutes: tm.tm_min as u8,
        // A leap second (60) is reported as the last ordinary second.
        seconds: tm.tm_sec.min(59) as u8,
        weekday: tm.tm_wday as u8,
        utc_offset_secs: tm.tm_gmtoff as i32,
    })
}

/// The time `lt` names in the host's local time zone, in seconds since
/// 1970-01-01 00:00 UTC; `None` for an invalid date. A time that daylight
/// saving skips or repeats is resolved as `mktime` does.
pub fn local_to_unix(lt: &LocalTime) -> Option<i64> {
    if !lt.is_valid() {
        return None;
    }
    // SAFETY: as in `unix_to_local`, an all-zero `tm` is valid.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = lt.year.checked_sub(1900)?;
    tm.tm_mon = i32::from(lt.month) - 1;
    tm.tm_mday = i32::from(lt.day);
    tm.tm_hour = i32::from(lt.hours);
    tm.tm_min = i32::from(lt.minutes);
    tm.tm_sec = i32::from(lt.seconds);
    tm.tm_isdst = -1; // let the time zone rules decide
    tm.tm_wday = -1; // mktime sets it only on success
    // SAFETY: `tm` is a live, initialised local that mktime reads and
    // normalises in place; it keeps no reference to it.
    let t = unsafe { libc::mktime(&mut tm) };
    // -1 is also a valid time (1969-12-31 23:59:59 UTC); only an unset
    // weekday tells the error apart.
    if t == -1 && tm.tm_wday < 0 {
        return None;
    }
    Some(t)
}

/// Parse a [`CLOCK_ENV`] value: `YYYY-MM-DD`, optionally followed by `T`
/// or a space and `HH:MM` or `HH:MM:SS`; the year from [`MIN_SET_YEAR`] to
/// [`MAX_SET_YEAR`].
pub fn parse_clock_setting(s: &str) -> Option<LocalTime> {
    let s = s.trim();
    let (date, time) = match s.find(['T', ' ']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let mut d = date.split('-');
    let year: i32 = number(d.next()?, 4)?;
    let month: u8 = number(d.next()?, 2)?;
    let day: u8 = number(d.next()?, 2)?;
    if d.next().is_some() {
        return None;
    }
    let (mut hours, mut minutes, mut seconds) = (0, 0, 0);
    if let Some(time) = time {
        let mut t = time.split(':');
        hours = number(t.next()?, 2)?;
        minutes = number(t.next()?, 2)?;
        if let Some(sec) = t.next() {
            seconds = number(sec, 2)?;
        }
        if t.next().is_some() {
            return None;
        }
    }
    let lt = LocalTime::new(year, month, day, hours, minutes, seconds);
    ((MIN_SET_YEAR..=MAX_SET_YEAR).contains(&year) && lt.is_valid()).then_some(lt)
}

/// A field of exactly `digits` ASCII digits.
fn number<T: std::str::FromStr>(s: &str, digits: usize) -> Option<T> {
    (s.len() == digits && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// Pack `lt` into an OS/2 `FDATE` / `FTIME` pair: date bits 15–9 the year
/// from 1980, 8–5 the month, 4–0 the day; time bits 15–11 the hour, 10–5
/// the minute, 4–0 the seconds / 2. A time before 1980 becomes
/// 1980-01-01 00:00:00 and one after 2107 2107-12-31 23:59:58, the ends of
/// the range, instead of wrapping into another year.
pub fn pack_fdate_ftime(lt: &LocalTime) -> (u16, u16) {
    let lt = if lt.year < FDATE_MIN_YEAR {
        LocalTime::new(FDATE_MIN_YEAR, 1, 1, 0, 0, 0)
    } else if lt.year > FDATE_MAX_YEAR {
        LocalTime::new(FDATE_MAX_YEAR, 12, 31, 23, 59, 58)
    } else {
        *lt
    };
    let date = (((lt.year - FDATE_MIN_YEAR) as u16) << 9) | (u16::from(lt.month) << 5) | u16::from(lt.day);
    let time = (u16::from(lt.hours) << 11) | (u16::from(lt.minutes) << 5) | (u16::from(lt.seconds) / 2);
    (date, time)
}

/// Unpack an `FDATE` / `FTIME` pair; `None` when a field is out of range.
pub fn unpack_fdate_ftime(date: u16, time: u16) -> Option<LocalTime> {
    let lt = LocalTime::new(
        FDATE_MIN_YEAR + i32::from(date >> 9),
        ((date >> 5) & 0x0F) as u8,
        (date & 0x1F) as u8,
        (time >> 11) as u8,
        ((time >> 5) & 0x3F) as u8,
        ((time & 0x1F) * 2) as u8,
    );
    lt.is_valid().then_some(lt)
}

/// The `FDATE` / `FTIME` of the host time `t` in local time.
pub fn system_time_to_fdate(t: SystemTime) -> (u16, u16) {
    match unix_to_local(system_time_secs(t)) {
        Some(lt) => pack_fdate_ftime(&lt),
        // Beyond what the host can break down: the nearer end of the range.
        None if system_time_ms(t) < 0 => pack_fdate_ftime(&LocalTime::new(FDATE_MIN_YEAR - 1, 1, 1, 0, 0, 0)),
        None => pack_fdate_ftime(&LocalTime::new(FDATE_MAX_YEAR + 1, 1, 1, 0, 0, 0)),
    }
}

/// What an `FDATE` / `FTIME` pair of `DosSetFileInfo` / `DosSetPathInfo`
/// asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTimeRequest {
    /// Both are 0: keep the file's time ("not changed", Control Program
    /// Reference, `DosSetFileInfo`).
    Keep,
    /// Set the file's time to this.
    Set(SystemTime),
    /// A field is out of range.
    Invalid,
}

/// Interpret an `FDATE` / `FTIME` pair to set (local time).
pub fn fdate_request(date: u16, time: u16) -> FileTimeRequest {
    if date == 0 && time == 0 {
        return FileTimeRequest::Keep;
    }
    match unpack_fdate_ftime(date, time).and_then(|lt| local_to_unix(&lt)) {
        Some(secs) => FileTimeRequest::Set(unix_to_system_time(secs)),
        None => FileTimeRequest::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unix time of 2038-01-19 03:14:08 UTC, one second past `i32::MAX`.
    const Y2038: i64 = 1 << 31;
    /// Unix time of 2106-02-07 06:28:16 UTC, one second past `u32::MAX`.
    const Y2106: i64 = 1 << 32;

    #[test]
    fn calendar_helpers() {
        assert!(is_leap_year(2000) && is_leap_year(2040) && !is_leap_year(2100) && !is_leap_year(2039));
        assert_eq!(days_in_month(2040, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2039, 4), 30);
        assert_eq!(days_in_month(2039, 13), 0);
        assert!(LocalTime::new(2040, 2, 29, 23, 59, 59).is_valid());
        assert!(!LocalTime::new(2039, 2, 29, 0, 0, 0).is_valid());
        assert!(!LocalTime::new(2039, 1, 1, 24, 0, 0).is_valid());
        assert!(!LocalTime::new(2039, 0, 1, 0, 0, 0).is_valid());
    }

    #[test]
    fn local_round_trip_past_2038_and_2106() {
        for secs in [0, Y2038 - 1, Y2038, Y2038 + 86_400 * 365, Y2106 - 1, Y2106, 7_258_118_400] {
            let lt = unix_to_local(secs).expect("breaks down");
            assert_eq!(local_to_unix(&lt), Some(secs), "{lt:?}");
        }
        let lt = unix_to_local(Y2106).unwrap();
        assert!(lt.year == 2106, "{lt:?}");
    }

    #[test]
    fn invalid_dates_do_not_convert() {
        assert_eq!(local_to_unix(&LocalTime::new(2039, 2, 30, 0, 0, 0)), None);
        assert_eq!(local_to_unix(&LocalTime::new(2039, 1, 1, 0, 60, 0)), None);
    }

    #[test]
    fn os2_timezone_is_minutes_west() {
        let east = LocalTime { utc_offset_secs: 9 * 3600, ..Default::default() };
        assert_eq!(east.os2_timezone(), -540);
        let west = LocalTime { utc_offset_secs: -5 * 3600, ..Default::default() };
        assert_eq!(west.os2_timezone(), 300);
    }

    #[test]
    fn fdate_packs_and_clamps() {
        let (d, t) = pack_fdate_ftime(&LocalTime::new(2099, 12, 31, 23, 59, 58));
        assert_eq!(d, (119 << 9) | (12 << 5) | 31);
        assert_eq!(t, (23 << 11) | (59 << 5) | 29);
        assert_eq!(unpack_fdate_ftime(d, t), Some(LocalTime::new(2099, 12, 31, 23, 59, 58)));
        // 2108 used to wrap into 1980; it is now the last representable time.
        assert_eq!(pack_fdate_ftime(&LocalTime::new(2108, 6, 1, 0, 0, 0)),
                   pack_fdate_ftime(&LocalTime::new(2107, 12, 31, 23, 59, 58)));
        // Before 1980: the first representable time, not year 0 with the old month.
        assert_eq!(pack_fdate_ftime(&LocalTime::new(1975, 6, 15, 12, 0, 0)), ((1 << 5) | 1, 0));
        assert_eq!(unpack_fdate_ftime((1 << 5) | 31, 0).map(|l| l.day), Some(31));
        assert_eq!(unpack_fdate_ftime((13 << 5) | 1, 0), None, "month 13");
        assert_eq!(unpack_fdate_ftime((2 << 5) | 30, 0), None, "1980-02-30");
    }

    #[test]
    fn fdate_requests() {
        assert_eq!(fdate_request(0, 0), FileTimeRequest::Keep);
        assert_eq!(fdate_request((13 << 5) | 1, 0), FileTimeRequest::Invalid);
        let (d, t) = pack_fdate_ftime(&LocalTime::new(2050, 6, 15, 10, 20, 30));
        let FileTimeRequest::Set(st) = fdate_request(d, t) else { panic!("not a time") };
        assert_eq!(system_time_to_fdate(st), (d, t));
    }

    #[test]
    fn system_times_around_the_epoch() {
        assert_eq!(system_time_ms(UNIX_EPOCH - Duration::from_millis(1500)), -1500);
        assert_eq!(system_time_secs(UNIX_EPOCH - Duration::from_millis(1500)), -2);
        assert_eq!(system_time_secs(unix_to_system_time(-86_400)), -86_400);
        assert_eq!(system_time_secs(unix_to_system_time(Y2106)), Y2106);
        // A file from 1969 is shown as 1980-01-01.
        assert_eq!(system_time_to_fdate(unix_to_system_time(-86_400)), ((1 << 5) | 1, 0));
    }

    #[test]
    fn clock_settings_parse_strictly() {
        assert_eq!(parse_clock_setting("2040-03-01"), Some(LocalTime::new(2040, 3, 1, 0, 0, 0)));
        assert_eq!(parse_clock_setting("2110-01-02T03:04"), Some(LocalTime::new(2110, 1, 2, 3, 4, 0)));
        assert_eq!(parse_clock_setting(" 2110-01-02 03:04:05 "), Some(LocalTime::new(2110, 1, 2, 3, 4, 5)));
        for bad in ["", "2040", "2040-3-1", "2040-02-30", "1979-12-31", "10000-01-01",
                    "2040-01-01T24:00", "2040-01-01T10", "2040-01-01T10:00:00:00", "+2040-01-01",
                    "2040-01-01-01"] {
            assert_eq!(parse_clock_setting(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_clock_moves_by_its_offset() {
        let clock = SessionClock::default();
        let host = host_now_ms();
        assert!((clock.now_ms() - host).abs() < 5_000);
        let target = Y2106 * MS_PER_SECOND + 123;
        clock.set_now_ms(target);
        assert!((clock.now_ms() - target).abs() < 5_000);
        assert!(clock.now_secs() >= Y2106 - 5);
        let child = SessionClock::with_offset(clock.offset_ms());
        assert!((child.now_ms() - clock.now_ms()).abs() < 5_000);
    }
}
