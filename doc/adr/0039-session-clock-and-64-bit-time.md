# ADR-0039: Keep a per-process session clock for DosSetDateTime; add 64-bit time calls in WARPINE.DLL

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** `doc/TODOs.md` "Dates after 2038"; `src/loader/clock.rs` (`SessionClock`, `SharedState::clock`), `src/loader/warpine_api.rs`, `src/loader/stubs.rs` (`DosGetDateTime`, `DosSetDateTime`, `DosSetFileInfo`, `DosSetPathInfo`, `DosQuerySysInfo`), `src/loader/vfs_hostdir.rs`, `src/loader/inherit.rs`; `samples/datetime_test`; IBM *Control Program Programming Reference* (`DATETIME`, `FDATE` / `FTIME`, `DosSetFileInfo`, `QSV_TIME_LOW` / `QSV_TIME_HIGH`); Open Watcom `h/time.h` (`typedef unsigned long time_t`)

## Context

The TODO list asked whether Warpine breaks in 2038 and what to do about it. An audit found the following.

- **The host side is 64-bit.** Linux's `time_t` is 64-bit on x86-64, and the OS/2 interfaces hold dates of their own: `DATETIME` has a `USHORT` year, `FDATE` a 7-bit year from 1980 (up to 2107), and `QSV_TIME_LOW` / `QSV_TIME_HIGH` a 64-bit second count.
- **Open Watcom programs are not affected until 2106.** Open Watcom links its C runtime statically, and its `time_t` is `unsigned long`.
- **Real bugs:**
  - Host file times were converted to `FDATE` / `FTIME` in UTC instead of local time.
  - Years after 2107 wrapped into the 7-bit field, and years before 1980 kept their month and day.
  - `DATETIME.timezone` had the wrong sign: the reference defines it as minutes west of UTC.
  - `DosSetFileInfo` / `DosSetPathInfo` ignored the times they were given.
  - `DosSetDateTime` was a stub, so nothing could test a date after 2038 without changing the host clock.
- **Third-party C-runtime DLLs can't be fixed from outside.** The TODO also proposed redirecting their time imports (`CLIB.DLL`, `CRTL.DLL`, `EMX.DLL`) to host implementations. EMX's `time_t` is a signed 32-bit `long`, and the 32-bit return value of those functions is part of their interface. The most a host implementation could do is read the value as unsigned, which ends in 2106, and nothing in the repository could test it.

## Decision

- **A session clock instead of the host clock.** `SessionClock` (`clock.rs`) is the host's time plus an offset in milliseconds.
  - `DosGetDateTime`, `DosQuerySysInfo(QSV_TIME_LOW / QSV_TIME_HIGH)` and the `WARPINE.DLL` calls read it.
  - `DosSetDateTime` moves the offset after checking the fields: years 1980–9999 and a real calendar date, otherwise `ERROR_TS_DATETIME`. The weekday and time-zone fields are not used, and the host's time zone stays in force.
  - Every child gets its parent's offset in `WARPINE_CLOCK_OFFSET` (`inherit::admit_child`).
  - The first process of a session can start at any local date with `WARPINE_CLOCK=YYYY-MM-DD[THH:MM[:SS]]`. The tests use this.
- **The offset is per process, not system-wide.** On OS/2, `DosSetDateTime` changes the system clock for every process. In Warpine, processes that are already running keep their own offset. A shared clock would need a session-directory file read on every `DosGetDateTime`, and per process is enough for programs that set the date and start children.
- **File times are local time, clamped to the `FDATE` range.** A time before 1980 becomes 1980-01-01 00:00:00, and one after 2107 becomes 2107-12-31 23:59:58; neither wraps any more. The setters set access and write times through `File::set_times`, and a date and time of 0 / 0 leave that time unchanged. The rest of `FILESTATUS3` is not set:
  - **Creation time:** Linux cannot set one.
  - **Attributes:** not set, as with `DosSetFileMode`.

  Host file times are real times and are not shifted by the session clock.
- **`WARPINE.DLL`**, a built-in module at `WARPINE_BASE` (20480) in the stub area, offers 64-bit time to programs that can be recompiled:
  - `WrpTime64`;
  - `WrpGetDateTime64`;
  - `WrpLocalTime64`;
  - `WrpMakeTime64`.

  Each takes pointers to a signed 64-bit count of seconds since 1970 UTC and a `DATETIME`, for years 1–9999. A program finds the module with `DosLoadModule("WARPINE")`, which fails on OS/2. Because the module is not an OS/2 API, it is not counted in the compatibility chart. `DosQueryProcAddr` by name now works for every built-in module.
- **The C-runtime DLL redirect is not implemented,** for the reasons above. Programs that need dates after 2038 should use Open Watcom's runtime (good until 2106) or `WARPINE.DLL`.

## Consequences

- Dates after 2038 and after 2106 can be tested on any host; `datetime_test` runs at 2040 and sets 2110 without touching the host clock.
- A program that sets the date affects only itself and the children it starts afterwards.
- Unverified behaviour, chosen and documented in the code:
  - the years `DosSetDateTime` accepts beyond the calendar checks;
  - the error for an invalid file date (`ERROR_INVALID_PARAMETER`);
  - what OS/2 reports for host file times outside the `FDATE` range.
- Name-based imports from registry modules other than DOSCALLS no longer add the module base twice (`api_registry::find_by_module_and_name` returns the module's own ordinal).
