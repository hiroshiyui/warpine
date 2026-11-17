# ADR-0018: Test against Open Watcom sample programs and golden screen signatures

- **Status:** Accepted
- **Date:** 2026-11-15 (scripted GUI sessions and goldens)
- **Sources:** `CLAUDE.md` (Engineering policy, Conventions); commit 926e6cb ("add scripted GUI sessions and golden regression tests"); `src/gui/gui_script.rs`; `tests/golden/`

## Context

Under the clean-room policy (ADR-0004) the reference for correct behaviour is how real OS/2 programs behave. PM rendering also needs regression tests that do not depend on a display.

## Decision

- Every change comes with tests and, where it touches the OS/2 API, a sample OS/2 program in `samples/` (usually Open Watcom C) that prints one `ok` / `FAIL` line per check; integration tests run the samples under KVM.
- GUI behaviour is tested with scripted sessions (`WARPINE_GUI_SCRIPT`: clicks, keys, text, dumps) on SDL's dummy video driver. The dumped desktop is compared with golden tile signatures in `tests/golden/` (average colour per 16×16 tile, tolerance 6), regenerated only for intended changes.
- Tests never open windows on the user's display and never use the user's drive C (ADR-0009).

## Consequences

- Behaviour is checked from inside OS/2 programs, the same way users' programs see it.
- Integration tests need `/dev/kvm`, the Open Watcom toolchain and built samples, and skip when they are missing.
- Golden signatures must be regenerated, and the dump looked at, after an intended rendering change.
