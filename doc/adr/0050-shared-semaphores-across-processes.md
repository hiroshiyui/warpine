# ADR-0050: Shared semaphores across processes: a host-only memfd of slots, an OFD lock and a futex

- **Status:** Accepted; extends ADR-0035 and ADR-0049
- **Date:** 2026-11-19
- **Sources:** `src/loader/shared_sem.rs`, `src/loader/ipc.rs` (`SemaphoreManager::shared`, `try_mixed_mux`, `check_mux_records`), `src/loader/doscalls.rs` (the `Dos*Sem` calls), `src/loader/session_threads.rs`, `src/loader/inherit.rs`; `samples/shsem_test`; IBM *Control Program Programming Reference* (`DosCreateEventSem`, `DosOpenEventSem`, `DosCreateMutexSem`, `DosRequestMutexSem`, `DosQueryMutexSem`, `DosCreateMuxWaitSem`, `DosOpenMuxWaitSem`, `DosAddMuxWaitSem`), as on EDM/2

## Context

Named (`\SEM32\…`) and `DC_SEM_SHARED` semaphores are how OS/2 processes synchronise. Every OS/2 process is a Warpine host process of its own (ADR-0020), and until now semaphores lived in the process's `SemaphoreManager` only. A child could not open its parent's semaphores by name or by handle, nor wait for a post from another process.

The shared-memory table of ADR-0049 is a file read under a directory lock, which is too slow for semaphores: a post or a wait can happen thousands of times a second.

## Decision

- **A semaphore segment.** The first process of a session creates a second `memfd`, sealed against resizing (`SEGMENT_BYTES`: a 4 KB header and `MAX_SHARED_SEMS` = 512 slots of 1 KB). Children inherit it through `WARPINE_SEM_FD`, the same way as the arena.
  - Unlike the arena, it is mapped only into the host process, never into guest memory, so guests cannot reach semaphore state.
  - A child uses it only if it is a sealed memfd of the right size carrying the header's magic and version.
- **Slots.** A slot is one event or mutex, encoded field by field:
  - kind, generation and flags;
  - the name;
  - the post count, or the owner (OS/2 process id, thread id, host pid and start time), request count and owner-died record;
  - up to `MAX_OPENERS` (28) processes that have it open, with their counts.

  Decoding checks every field; a slot that does not decode is free.
- **Handles.** A shared semaphore's handle is `SHARED_SEM_BIT | generation << 16 | slot`. It has the same value in every process, so a handle passed to another process opens the semaphore there, as in OS/2. Freeing a slot advances its generation, so stale handles fail.
  - Private semaphores keep their small per-process handles.
  - Every `Dos*Sem` call routes on the handle bit; a create routes on `DC_SEM_SHARED` or a name.
- **One lock.** Every operation runs under an in-process mutex plus an open-file-description lock (`F_OFD_SETLKW`) on the process's own description of the memfd, opened through `/proc/self/fd`.
  - The kernel releases the lock of a dead process, so a process killed mid-operation cannot block the session.
  - Lock order: `sem_mgr`, then the segment, then local semaphore objects.
- **Waiting.** Waiters sleep with a non-private futex on the header's generation word.
  - Every change advances the word and wakes all waiters in every process, which then look again. Interruptions and stop conditions (ADR-0035) are checked at least every `SEM_POLL`, as for private semaphores.
  - Changes to private semaphores also wake the futex, so a muxwait list that mixes private and shared members sleeps on it alone.
- **Process identity and death.** Processes are recorded by OS/2 process id plus host identity (pid and `/proc` start time). An operation on a semaphore first sweeps it:
  - openers that are gone are dropped;
  - a mutex whose owning process is gone enters the owner-died state;
  - a semaphore without openers is deleted.

  A thread ending (`owner_ended`) and the process ending (`leave`, from `leave_thread_session`) do the same at once for themselves.
- **Semantics** follow the private implementation (`ipc.rs`):
  - post counts and `DCE_POSTONE` / `DCE_AUTORESET` reuse `EventSemaphore`;
  - mutexes are recursive, `ERROR_NOT_OWNER` on a release by a non-owner, and `ERROR_SEM_OWNER_DIED` for the next requester;
  - names are unique across kinds and across private muxwait names (`ERROR_DUPLICATE_NAME`);
  - `ERROR_SEM_BUSY` applies while a thread of the closing process waits;
  - the last close in the session deletes the semaphore.
- **Muxwait semaphores stay within their process**, including named ones, but their lists may hold shared events or mutexes. `try_mixed_mux` looks at all members, and takes them, under the segment's lock.

## Consequences

- Programs that synchronise across processes with named or shared semaphores work across Warpine processes. Pipe semaphores and DOS timers accept shared events.
- The segment costs 516 KB of shared, sparse memory per session. Each shared-semaphore call costs two `fcntl` calls, a 1 KB slot copy and a futex wake on change.
- Every waiter in the session wakes on every change, so a busy session wakes more often than it strictly needs to.
- Security: the segment is host-only, inside the user's own processes. A broken or hostile process of the same user can corrupt semaphore state, but decoding is bounded and checked, so Warpine never reads or writes outside the mapping.
- Limits:
  - at most 512 shared semaphores per session, and 28 processes holding one semaphore open;
  - named and shared muxwait semaphores cannot be opened from another process;
  - a semaphore that only a killed process had open is deleted only when another process next creates or opens one;
  - without a session, shared semaphores stay within the process.
- Unverified: the limits OS/2 itself places on shared semaphores (Warpine reports `ERROR_TOO_MANY_SEMAPHORES` and `ERROR_TOO_MANY_OPENS` at its own limits).

## Addendum (2026-11-24): a reaped child's semaphores swept at once

Every path that reaps a child (`SharedState::child_exit_code`) sweeps the segment (`Locked::sweep`) as well as the shared-memory table (ADR-0049 addendum): a semaphore only a killed child had open is deleted, and a mutex it owned is in the owner-died state, as soon as its parent has seen it end, not at the next look by any process.

## Addendum (2026-11-26): shared muxwait semaphores

Named and `DC_SEM_SHARED` muxwait semaphores now live in the segment too, lifting the limit above.

- **Slots.** A third slot kind, `KIND_MUXWAIT`: the flags (`DCMW_WAIT_ANY` / `DCMW_WAIT_ALL`, checked on decode), the list's kind (event, mutex or none while empty) and up to 64 `SEMRECORD`s (handle, user value) at offset 1024. Slots grow from 1 KB to 2 KB, so the segment is 1 MB + 4 KB, still sparse; the header's version goes to 2, and a child refuses a segment of another version. Decoding checks the count, that the kind matches it, that every handle is a shared one and that none repeats.
- **Members.** A shared list holds shared events or mutexes only; a handle of a private semaphore is `ERROR_WRONG_TYPE` (unverified: which error a private member gets). Creating a list or adding to it checks the members against the semaphores the calling process has open, with the same rules as private lists (`ipc::check_mux_records`).
- **Waiting.** `DosWaitMuxWaitSem` on a shared list goes through `wait_shared_sem`. Each look (`SharedSems::try_mux`) finds the members by handle whoever has them open, sweeps them for dead owners, and takes them, all under the segment's lock. A process that opened only the list can therefore wait on members it never opened itself (Warpine's reading, unverified). A member that no process has open any more ends the wait with `ERROR_INVALID_HANDLE`, as for private lists.
- **Changes.** `DosAddMuxWaitSem` / `DosDeleteMuxWaitSem` by any process that has the list open change it for all of them; every change wakes the session's waiters. `DosQueryMuxWaitSem` reports `DC_SEM_SHARED` for every shared list (unverified).
- **Without a session** named muxwait lists stay within their process, as before.

Tests: `shared_sem.rs` (muxwait slots and junk, lists of events and of mutexes across two simulated processes, owner death through a list, the API routing); `samples/shsem_test` (the child opens, reads and adds to the parent's named list, and the parent's wait ends with the record the child added).
