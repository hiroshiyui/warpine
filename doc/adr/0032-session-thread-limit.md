# ADR-0032: Enforce CONFIG.SYS THREADS= across a session with per-process records in a private directory

- **Status:** Accepted; complements ADR-0028
- **Date:** 2026-11-18
- **Sources:** `src/loader/config_sys.rs`, `src/loader/session_threads.rs`, `src/loader/inherit.rs` (`admit_child`, `child_started`), `src/loader/doscalls.rs` (`DosCreateThread`), `src/main.rs` (`join_thread_session`); `samples/threads_limit_test`; OS/2 CONFIG.SYS `THREADS=` (range and default from public tuning notes and the ArcaOS kernel errata: default 64 "inherited from Warp 4", maximum 4095; the minimum is unverified); ADR-0020, ADR-0022, ADR-0028

## Context

OS/2 bounds the threads of the whole system with the `THREADS=` statement of `CONFIG.SYS`. Every process needs at least one thread, so the statement also bounds processes. When the table is full, `DosCreateThread` fails with `ERROR_MAX_THRDS_REACHED` and `DosExecPgm` with `ERROR_NO_PROC_SLOTS`.

After a runaway chain of self-starting processes exhausted the host (ADR-0028), the project wanted this standard facility as well as Warpine's own nesting limit.

In Warpine an OS/2 "system" is a session: the first Warpine process and every process started from it, each a separate host process (ADR-0020). The count must therefore be shared between host processes, and it must survive processes that are killed without cleaning up.

## Decision

- **The limit:** `THREADS=` is read from `C:\CONFIG.SYS` on Warpine's drive C, through the VFS and read-only (the file is never created or written).
  - Values are clamped to 64–4095, and the default is 64.
  - `WARPINE_THREADS` overrides it. Every child gets the first process's value, so the whole session keeps one limit.
- **A private session directory** (`WARPINE_THREAD_DIR`, under `$XDG_RUNTIME_DIR`, mode 0700, ownership and mode checked on the opened descriptor) holds:
  - one record per live process, `p-<pid>`, giving its pid, its start time from `/proc/<pid>/stat` and its thread count;
  - one record per child being started, `r-<parentpid>-<seq>`.
- **Locking:** every scan and write happens under an exclusive `flock` on the directory. A child's slot is reserved before the spawn, so two racing spawns cannot both pass. After the spawn, `child_started` either turns the reservation into the child's record or, if the spawn failed, removes it.
- **Liveness instead of cleanup duty:** a record counts only while its pid exists, is not a zombie, and has the recorded start time, so a reused pid does not count. Stale and malformed records are deleted by whichever process next scans them. A killed process therefore leaks no slots.
- **Records are hostile input:**
  - names are parsed strictly;
  - files must be regular, are opened `O_NOFOLLOW`, and hold at most 64 bytes;
  - counts and pids are bounded, and a scan reads a bounded number of entries.
- **Enforcement:**
  - `DosCreateThread` returns `ERROR_MAX_THRDS_REACHED` (164).
  - `DosExecPgm` returns `ERROR_NO_PROC_SLOTS` (89).
  - `WinStartApp` fails with `PMERR_CANNOT_START`.
  - CMD.EXE refuses with a message.
  - A child that finds no slot exits with 89 before running guest code.
- **Leaving:** a process removes its record when it ends and, as the last one, the directory. If the directory cannot be used, a process falls back to counting only its own threads, with a warning.
- ADR-0028's depth and children limits stay as a second, independent net.

## Consequences

- A session cannot run more than `THREADS=` threads in total, whatever its processes do. A runaway program fails with OS/2's own errors, which OS/2 programs already handle.
- Every thread start and end, and every spawn, takes a directory lock and a short scan. That is cheap next to starting a vCPU or a host process.
- The accounting relies on Linux specifics (`/proc/<pid>/stat` start times, `flock`). Another host would need its own liveness check.
- Test runs must give Warpine a temporary `XDG_RUNTIME_DIR`, as the integration-test helpers do, so session directories never land in the user's runtime directory.
