# ADR-0037: Post pipe semaphores from per-end watcher threads, and sweep stale session directories by liveness

- **Status:** Accepted; extends ADR-0022
- **Date:** 2026-11-18
- **Sources:** batch 6; `src/loader/npipe_sem.rs` (`SharedState::pipe_sems`), `src/loader/npipe_dir.rs`, `src/loader/npipe.rs`; `samples/semfollow_test`, `samples/kill_test`; IBM *Control Program Programming Reference* (`DosSetNPipeSem`, `DosQueryNPipeSemState`, `PIPESEMSTATE`); ADR-0022, ADR-0032

## Context

`DosSetNPipeSem` attaches an event semaphore to a named-pipe end, to be posted when data arrives, room appears or the other end closes. With ADR-0022's host socket pairs, the other end can be in another Warpine process, so no code in this process sees its writes.

Separately, a Warpine process killed without cleaning up left its named-pipe session directory behind in the user's runtime directory.

## Decision

- **Pipe semaphores are posted by a watcher thread per attached end:**
  - It polls the end's host socket (every 50 ms).
  - It posts on new unread data, on room after a write had found the pipe full, and on a close or disconnect.
  - It stops when the end is closed, the attachment is replaced or removed, the semaphore is closed, or the process ends.
  - `DosQueryNPipeSemState` reports `NPSS_RDATA` / `NPSS_WSPACE` / `NPSS_CLOSE` records, and writes nothing on overflow.
- **Session directories carry liveness records:** each process records itself (`p-<pid>` with its `/proc` start time) under the directory's `flock`, as ADR-0032's thread directory does. The first process of a new session sweeps sibling pipe directories and removes one only if:
  - it is private to this user;
  - its lock can be taken without waiting;
  - it holds only sockets and records;
  - no recorded process is alive (a reused pid does not count).

## Consequences

- Pipe semaphores work across processes, at the cost of one polling thread per attached end; posts can lag by up to 50 ms.
- Killed sessions no longer leave pipe directories behind for good. The `warpine-threads-*` directories of ADR-0032 are removed by their last user, but a killed session's thread directory is not swept yet (`doc/TODOs.md`).
- Muxwait semaphores as pipe semaphores and a real `NPSS_WAIT` flag are not supported; the choices made where the reference is silent are marked unverified.
