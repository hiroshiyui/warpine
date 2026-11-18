# ADR-0007: Run each OS/2 thread on its own host thread and KVM vCPU

- **Status:** Accepted; the consequence that a vCPU in guest code cannot be interrupted is superseded by ADR-0023
- **Date:** 2026-03-12 (vCPU per thread); 2026-11-18 (thread table)
- **Sources:** `CLAUDE.md` (Concurrency model); commits 9f80c47 (thread exit), 7e8a4de (per-thread exception chains); `src/loader/threads.rs`; developer guide (Threading Model)

## Context

OS/2 programs are multi-threaded. Warpine's API calls block on the host (semaphores, message queues, I/O), so a guest thread needs a host thread to block on.

## Decision

Each OS/2 thread is a native Rust thread with its own KVM vCPU. Managers live in `SharedState` behind mutexes with a documented lock order and poison-recovering locks.

- A thread table (`threads.rs`) records running and ended threads, each thread's stack and TIB, and kill flags. Ending a thread ends only that thread; the last thread ends the process.
- KVM vCPUs cannot be destroyed, so idle vCPUs of ended threads are reused.
- Per-thread state lives in the thread's own TIB, which its FS selector points at; the exception-handler chain is one example.

## Consequences

- Blocking calls are simple: the host thread waits.
- A thread that runs in guest code without calling Warpine cannot be stopped from outside: `DosKillThread`, critical sections and exit lists take effect at its next exit to Warpine (`doc/TODOs.md`).
- Lock-order discipline is required in every manager.
