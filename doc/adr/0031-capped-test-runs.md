# ADR-0031: Run the commit checks inside a capped systemd scope

- **Status:** Accepted; extends ADR-0019
- **Date:** 2026-11-18
- **Sources:** `githooks/pre-commit`; the out-of-memory incident of 2026-11-18 (ADR-0028); systemd.resource-control(5) (`MemoryMax`, `MemorySwapMax`, `TasksMax`), systemd-run(1) (`--user --scope`)

## Context

ADR-0019 gates every commit on `cargo test` and clippy through the pre-commit hook. The integration tests run real Warpine processes, each with a KVM virtual machine (about 290 MB of host memory), and some tests start further Warpine processes (`DosExecPgm`, `WinStartApp`, CMD.EXE pipelines).

On 2026-11-18 a test program started itself without end. Nothing on the host limited it: the user session had `TasksMax=infinity`. The kernel's out-of-memory killer finally ended processes outside the test run, including the developer's other work. ADR-0028 fixes the cause inside Warpine, but a test run should be contained whatever goes wrong in it.

## Decision

- **The pre-commit hook runs `cargo test` and `cargo clippy` inside a transient `systemd-run --user --scope`** with `MemoryMax` (default 24G), `MemorySwapMax=0` and `TasksMax` (default 4096). The limits are overridable through `WARPINE_TEST_MEMORY_MAX` / `WARPINE_TEST_TASKS_MAX`.
- **Without a user systemd instance** (containers, CI, other systems), the hook prints a warning and runs the checks without caps, as before.
- **Long or process-starting test runs outside the hook** use the same scope by hand (README, developer guide), and Warpine runs in tests are wrapped in `timeout` and followed by a check that no Warpine process is left.

## Consequences

- A runaway in a test run is ended by the kernel inside its own cgroup. Memory pressure stays inside the scope instead of reaching unrelated programs.
- A legitimate run that needs more than the caps fails and must raise them explicitly; the defaults leave room for parallel builds and test threads on a 64 GB machine.
- Systems without systemd keep the old, unprotected behaviour; that is a known gap rather than a hard requirement on contributors.
