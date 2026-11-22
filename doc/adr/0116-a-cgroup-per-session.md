# ADR-0116: A cgroup per session

- **Status:** Accepted
- **Date:** 2026-11-22
- **Sources:**
  - systemd's D-Bus interface, `org.freedesktop.systemd1(5)`: `StartTransientUnit`, scope units and their `PIDs`, `MemoryMax`, `TasksMax` and `CollectMode` properties.
  - The Linux kernel's cgroup v2 documentation: `memory.max`, `pids.max`, and that a forked process starts in its parent's cgroup.

## Context

Each OS/2 process is a Warpine host process (ADR-0020) with 256 MB of guest memory and one host thread per OS/2 thread, up to 738 (ADR-0115). A session is a tree of them: its first process, the children it starts, and the desktop server. Landlock (ADR-0111) confines what each may reach, but nothing bounds how much memory or how many tasks a session takes: a runaway or hostile program could start processes and threads until the machine is out of either. The tests and the pre-commit hook already run under `systemd-run --scope` limits (ADR-0031); a user's session had none.

An unprivileged process cannot create a cgroup itself unless its own is delegated to it. On a systemd system, the user manager creates them on request, over D-Bus.

## Decision

1. **The session's first process asks the systemd user manager for a transient scope holding itself** (`src/cgroup.rs`, `enter_session_scope`), before it starts anything: the request is `StartTransientUnit` (`warpine-session-<pid>.scope`, mode `fail`, the process's PID, `MemoryMax`, `TasksMax`, `CollectMode=inactive-or-failed`), sent with `busctl`, as `systemd-run --scope` sends it. Every process the session starts inherits the cgroup.
   - The first process is the one without `WARPINE_PPID` (a Warpine parent sets it, ADR-0090); the desktop server is started by a session process.
   - It moves itself instead of re-running under `systemd-run`, so that a failure to make the scope is a clear answer, not an exit code mixed with the program's.
2. **Limits:** 8 GB of memory and 8192 tasks by default (`WARPINE_SESSION_MEMORY_MAX`, with a K / M / G / T suffix, and `WARPINE_SESSION_TASKS_MAX`). The kernel stops the session at them: an allocation beyond the memory ends in the OOM killer within the scope, a thread or process beyond the tasks fails to start.
3. **Without a user manager** (no systemd, a container, CI) the session runs without the limits, with a warning; `WARPINE_CGROUP=off` turns the scope off.
4. **A session never moves to a looser cgroup.** If a cgroup on the process's path that the new scope would not be under (below the user manager's `app.slice`; any, with no user manager on the path) has a `memory.max` or `pids.max` below the new scope's, the process stays where it is (`would_escape_limits`). The tests' and the hook's `systemd-run --scope` (4096 tasks) keep their processes so; systemd's default `TasksMax` on ordinary units is far above 8192 and does not.

## Alternatives considered

- **Re-running under `systemd-run --user --scope`.** The same scope, but a failure to make it could not be told from the program's own exit code, and every session would start twice.
- **Refusing to start without systemd.** Stricter, but CI, containers and systems without systemd would all need `WARPINE_CGROUP=off`.
- **Creating a child cgroup directly in `/sys/fs/cgroup`.** No D-Bus, but only where the process's cgroup is delegated to it, and cgroup v2's rule that a cgroup with controllers has no processes of its own would need the process moved into a leaf first.
- **A D-Bus library.** One call does not justify a dependency; `busctl` comes with systemd, which the scope needs anyway.

## Warpine's choices (unverified)

- **The default limits** (8 GB, 8192 tasks): room for a few dozen processes and a process at its thread limit, not measured against real workloads.
- **Waiting up to 500 ms** for the user manager to move the process, after which the session goes on and says so.

## Consequences

- On a systemd desktop, one session cannot take more than its limits; `systemctl --user status warpine-session-<pid>.scope` shows it, and the scope is gone when the session ends.
- The first process starts `busctl` once, before Landlock confines it.
- **Tests:** unit tests in `cgroup.rs` (sizes, limits, the first-process rule, the request's arguments, `would_escape_limits` on a fake hierarchy). By hand, on the development machine: a session in a terminal got its scope with 8 GB and 8192 tasks, and systemd removed it when the session ended; a session under the hook's `systemd-run --scope -p TasksMax=4096` stayed in it. The integration tests run under such a scope, so they never make one.
- **Found on the way:** the first version moved any session out of a cgroup with a limit into its own scope, sideways, which would have taken the tests out of the hook's limits; and a rule against any limit at all kept every desktop session out, since systemd gives nearly every unit a default `TasksMax`. Comparing values settled both.
