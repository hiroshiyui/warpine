# ADR-0020: Run each OS/2 process as its own host process

- **Status:** Accepted
- **Date:** 2026-03-13
- **Sources:** commit 6e0ed6b ("implement process execution APIs (DosExecPgm, DosWaitChild)"); `src/loader/process.rs` (`dos_exec_pgm`), `src/loader/managers.rs` (`ProcessManager`); the maintainer's question on how the Workplace Shell starts programs (2026-11-18)

## Context

On OS/2 every program runs in a process of its own, with a private address space below a shared arena, which holds DLL code and shared memory at the same address in every process. Processes start others with `DosExecPgm` (child processes) or through the session manager (`DosStartSession`, `WinStartApp`), as related or independent sessions; programs started from the Workplace Shell run as separate processes. All PM programs share one session and one system input queue.

Warpine runs a guest in one KVM virtual machine per host process, with one vCPU per OS/2 thread (ADR-0003, ADR-0007). An OS/2 process could either be another VM in the same host process or a host process of its own.

## Decision

Each OS/2 process is a separate Warpine host process with its own KVM virtual machine and guest memory.

- `DosExecPgm` starts the child by running Warpine itself (`std::env::current_exe()`) on the child program, with the parent's current directory; redirected standard handles to files are handed to the child as host files (`clone_host_file`). In synchronous mode the parent relays the child's output to its own console, and the child runs headless.
- The parent keeps the host `Child` in its `ProcessManager` and numbers its children from 100; `DosWaitChild` and `DosKillProcess` act on that host process.
- Nothing is shared between Warpine processes unless a feature explicitly crosses the boundary through a host mechanism (host files today; pipes and shared memory as they are implemented).

## Consequences

- Processes are more isolated than on OS/2: there is no shared arena between them, so DLL data and shared memory are per process, and a PM program that stops reading input cannot freeze another process's windows.
- A crash of one OS/2 process (or of its Warpine) does not take others down; a child killed with `DosKillProcess` is a host process killed.
- OS/2 features that span processes need deliberate host-level work, each a decision of its own: `DosGiveSharedMem` and named shared memory across processes, named semaphores shared between processes, pipes inherited by child processes (the 4OS2 pipe hang, `doc/TODOs.md`), `DosSendSignalException` to another process, and later `WinStartApp` / `DosStartSession` (related or independent sessions, `WM_APPTERMINATENOTIFY`) and the switch list.
- Each Warpine process shows its own PM desktop window; several PM programs do not yet share one desktop.
- Process ids are not consistent across processes: every Warpine process reports the same constant id for itself (`PROCESS_ID`, 42, in its PIB and in `DosQueryMutexSem`), while a parent identifies its children by ids it assigns from 100. A child's own id therefore differs from the one its parent holds (`doc/TODOs.md`).
- Starting a process costs a host process and a VM, which is heavier than an OS/2 `DosExecPgm`.
