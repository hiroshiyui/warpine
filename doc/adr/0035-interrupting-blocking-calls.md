# ADR-0035: Interrupt blocking calls through a per-thread call marker and one semaphore wake-up path

- **Status:** Accepted; extends ADR-0023
- **Date:** 2026-11-18
- **Sources:** batch 6; `src/loader/interrupt.rs`, `src/loader/vcpu.rs` (`begin_call` / `end_call`), `src/loader/ipc.rs` (`SemSignal`, `SemaphoreManager::wake_all`), `src/loader/doscalls.rs` (`wait_sem`, `dos_wait_mux_wait_sem`), `src/loader/signals.rs` (`wake_thread_1`, `request_exit`); IBM *Control Program Programming Reference* (`ERROR_INTERRUPT`)

## Context

ADR-0023 stops a thread that runs guest code. A thread blocked inside an API call (`DosSleep`, a semaphore wait, a pipe or keyboard read, `WinGetMsg`) still held a signal or a kill until the call returned, possibly for ever.

`DosWaitMuxWaitSem` also polled every 10 ms. Different waits used different condition variables, so there was no single way to wake them.

## Decision

- **A per-thread call marker:** `vcpu_loop` marks each 32-bit API call (`interrupt::begin_call` / `end_call`).
  - `Loader::interrupted()` is true for a thread killed by `DosKillThread`.
  - For thread 1 it is also true when a termination or a deliverable signal waits.
  - Blocking calls check it wherever they already check for process end, at least every 100 ms.
- **Two outcomes, by call:**
  - Calls the reference lets fail return `ERROR_INTERRUPT`: `DosSleep`, the semaphore waits, `DosWaitThread`, `DosWaitChild`, `DosConnectNPipe`, `DosWaitNPipe`.
  - Reads are made again after the signal handler, as if they had not started (the result is dropped and the guest is left at the API thunk): keyboard, console and pipe `DosRead`, `DosReadQueue`, `MouReadEventQue`, `WinGetMsg` / `WinPeekMsg`.
- **One wake-up path for semaphores:** `SemaphoreManager::wake_all` wakes every event and mutex condition variable and bumps a generation counter (`SemSignal`, a leaf lock).
  - `DosWaitMuxWaitSem` waits on that counter instead of polling, so no change between its look and its wait is lost.
  - Signal delivery (`wake_thread_1`) and process end (`request_exit`) call it.
  - Lock order: `sem_mgr`, then a semaphore object, then `SemSignal`.

## Consequences

- Signals, kills and process end reach a thread blocked in the common calls at once, and muxwait waits no longer burn CPU.
- **Still not interruptible:**
  - pipe writes, and the write part of `DosTransactNPipe` / `DosCallNPipe`;
  - the modal dialog loop, and `WinSendMsg` to another thread;
  - critical-section waits;
  - host file I/O, sockets, and MMPM/2 waits;
  - CMD.EXE's child waits;
  - every call made by 16-bit programs.
  
  These are listed in `doc/TODOs.md`.
- A new blocking call must check `interrupted()` and pick one of the two outcomes; a new semaphore-like wait must be reachable from `wake_all`.
