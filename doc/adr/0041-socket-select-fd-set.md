# ADR-0041: Read select()'s fd_set as OS/2 TCP/IP's count-and-array layout; wait with poll(2)

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** `src/loader/tcpip.rs` (`so_select`, `read_fd_list`, `write_fd_list`); `samples/socket_test`; Open Watcom `h/os2/types.h` (`fd_set`, `FD_SET`, `FD_SETSIZE` 64, `MAXSOCKETS` 32768) and `h/os2/tcpustd.h` (`select`, `os2_select`); bitwiseworks libc `doc/ReleaseNotes.os2` and `src/emx/include/sys/select.h` (the emx/BSD bit mask, `bsdselect`, and the statement that OS/2 TCP/IP v5.0 uses another layout)

## Context

`select` read the guest's `fd_set` as a 64-bit bit mask of socket numbers 0–63. Warpine numbers sockets from `0xA000`, so no socket could ever be in a set, and `socket_test` failed.

- **The header Open Watcom programs are compiled with** declares the OS/2 TCP/IP `fd_set` differently, as a count and an array of socket numbers: `u_short fd_count; int fd_array[64]`, packed to 4. libc's notes confirm that this "OS/2 tcpip v5.0" layout differs from the BSD bit mask that `bsdselect` takes.
- **No public source we found gives the SO32DLL / TCP32DLL ordinals** of `select`, `bsdselect` and `os2_select`.
- **The old code had two other faults.** It used the host's `FD_SET`, which writes past Linux's 1024-descriptor `fd_set` for a large host descriptor. And it blocked without checking for interruption.

## Decision

- **`select` takes the count-and-array layout.** Each listed number must be an open socket (else `SOCENOTSOCK`), a count above 64 is `SOCEINVAL`, and `nfds` is ignored. On return each set holds its ready sockets in their original order, and the result is their total.
- **The wait uses `poll(2)` in 100 ms slices.** There is no host `FD_SETSIZE` limit, and a signal interrupts the wait with `SOCEINTR` (ADR-0035). A socket counts as readable or writable also on an error or hang-up, as with select(2).
- **Socket numbers stay at `0xA000` and above.** The new layout holds any `int`.
- **Not done, and marked unverified:** ordinal 13 for `select`, whether the BSD 4.3 stack's `select` took a bit mask, and `bsdselect` / `os2_select`.

## Consequences

- Programs built with Open Watcom's headers can use `select` on sockets; `socket_test` passes completely.
- A program built with emx/libc headers that passes a BSD bit mask to this entry point would be misread. It would call `bsdselect` (not provided) in any case.

## Addendum (2026-11-24): own ordinals by policy

ADR-0123 makes Warpine's own SO32DLL / TCP32DLL ordinals the policy where no public source gives them: documented calls are implemented at numbers of Warpine's (SO32DLL and TCP32DLL from 50, marked in the def), and programs built with Warpine's tools link them by name. The ordinals above stay unverified.
