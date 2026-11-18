# ADR-0030: Keep the switch list, atom tables and DDE within one Warpine process

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** batch 5; `src/loader/pm_switch.rs`, `src/loader/pm_atom.rs`, `src/loader/pm_dde.rs`, `src/loader/pm_startapp.rs`; IBM *Presentation Manager Programming Reference* (switch list, atom tables, DDE, `WinStartApp`); ADR-0020

## Context

On OS/2 the switch list (the window list), the system atom table and DDE conversations are system-wide: every PM program sees the other programs' switch entries, shares system atoms, and can start a DDE conversation with any window. ADR-0020 runs each OS/2 process as its own Warpine process with its own desktop and no shared PM state, and lists these services among the features that need a decision of their own.

## Decision

- **The switch list is per Warpine process.** A program sees and changes only its own entries; `FCF_TASKLIST` frames get an entry, removed with the frame.
- **The system atom table is per process,** next to the tables made with `WinCreateAtomTable`.
- **DDE works between the windows of one process.** `WinDdeInitiate` reaches the frames of this process; `DDEINIT` blocks are giveable shared memory that the receiver frees.
- **`WinStartApp` starts a separate Warpine process** (a session of its own, subject to ADR-0028's limits) and reports its end with `WM_APPTERMINATENOTIFY`, but shares none of these services with it.

## Consequences

- Single programs that use their own switch entries, atoms or internal DDE work; nothing crosses a process boundary, so no host channel needs securing.
- Cooperation between separate OS/2 programs does not work yet: one program cannot list or switch to another, find its atoms, or open a DDE conversation with it. That needs a session-wide host channel and probably a shared desktop, which would supersede this record.
