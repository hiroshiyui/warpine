# ADR-0111: Every Warpine process confines itself with Landlock; crash dumps go to the state directory

- **Status:** Accepted; adds a second layer to ADR-0066 (guests never reach host paths outside their drives)
- **Date:** 2026-11-21
- **Sources:**
  - The Linux kernel's Landlock documentation (`Documentation/userspace-api/landlock.rst`): rulesets, `path_beneath` rules, the access rights per ABI version, inheritance across `fork` and `execve`, best-effort use on older kernels.
  - The `landlock` crate (0.4.7, MIT OR Apache-2.0), the kernel developers' Rust binding.
  - The XDG Base Directory Specification: `XDG_STATE_HOME` (`~/.local/state`) for state such as logs.

## Context

Warpine keeps guests inside their drives in its own code: the VFS resolves every OS/2 path below a drive's root and refuses the rest (ADR-0066). If a guest ever found a way to run code in Warpine's own process, through a bug in Warpine or in a library it uses, nothing else would stop it reading the user's files: Warpine runs with the user's full rights.

The "Security Facilities" TODOs proposed confining the process. The user chose Landlock, on by default with a variable to turn it off, and crash dumps in `$XDG_STATE_HOME/warpine`.

## Decision

1. **`src/sandbox.rs` builds a policy and applies it** (`landlock` crate, ABI V5, best effort). It runs before any guest code, in every role:
   - programs: LX after the loader joins the session, NE likewise;
   - CMD.EXE, headless and windowed;
   - the desktop server.

   Landlock is inherited, so the processes a confined process starts are confined too.
2. **The policy** (`sandbox::policy`):
   - **read and write** (every right of the ABI):
     - each drive's host directory, or read only for a read-only drive;
     - the runtime directory (`XDG_RUNTIME_DIR`, else the temporary directory: the session's sockets, lock files and pipe directories);
     - `/dev` (KVM, the GPU, sound; Unix permissions still apply);
     - the crash-dump directory;
     - the directory of `WARPINE_DESKTOP_LOG`, and those of a GUI script's `dump` files;
   - **read:**
     - the system's files: `/usr`, `/etc`, `/sys`, `/proc`, `/run`, `/var/lib/dbus`, `/bin`, `/sbin`;
     - the program's own directory and file (its DLLs are looked for there, ADR-0092), and the GUI script;
     - in the home directory only what the display and sound read: `.Xauthority` (or `XAUTHORITY`), `.icons`, `.local/share/icons`, `.config/pulse`;
   - **execute:** Warpine's own binary (it starts its children and the desktop server) and the library directories (the dynamic loader).
   - A rule for a single file takes only the rights that apply to files, and paths that do not exist are left out.
3. **`WARPINE_SANDBOX=off`** (also `0`, `no`) leaves a process unconfined. A kernel without Landlock, or a failure to set it up, leaves the process unconfined with a warning in the log.
4. **`warpine --sandbox-check <path>`** confines itself as a program with no drives would and says whether the path is still readable. It lets a user see that the sandbox works on their system, and lets the tests check it.
5. **Crash dumps** go to `$XDG_STATE_HOME/warpine` (else `~/.local/state/warpine`, else the current directory), not the current directory, which is often the home directory the sandbox must not write. The path is printed on stderr as before (`crash_dump::crash_dir`).

## Warpine's choices (unverified)

- **The display and sound paths** were checked only with SDL's dummy drivers (the test suite). Wayland, X11, PulseAudio, PipeWire and GPU drivers on real desktops may read further files. `WARPINE_SANDBOX=off` is the way out, and a missing path belongs in the policy.
- **The whole runtime directory** is writable, not only Warpine's own directories in it: the session creates several, at different times and in different processes.
- **`/dev` is writable.** Device files are guarded by their Unix permissions; Landlock's device-ioctl right is granted with it.
- **Executing the library directories** is allowed because the dynamic loader runs from them; programs under `/usr/bin` cannot be run.
- **Unix sockets:** Landlock's filesystem rights do not cover connecting to them by path, so display and sound sockets need no rule; abstract sockets and network access are not restricted.
- **The program's directory is readable,** as its DLLs live there. A program started from the home directory makes the home directory's top level readable, not what lies below it.

## Consequences

- If a guest got control of a Warpine process, the kernel would still keep it out of everything in the home directory but the drives and the few display files: `warpine --sandbox-check ~/.ssh` answers `denied`.
- **Crash dumps** are no longer in the current directory: the stderr line names the file, under `~/.local/state/warpine/`.
- **Tests** set `XDG_STATE_HOME` to their temporary directory, and every integration test runs confined.
- **New dependencies:** `landlock`, `enumflags2` and `thiserror`, all MIT or Apache-2.0, checked by `deny.toml`.
- **Tests:**
  - `sandbox.rs`: the policy's grants, read-only drives, the `off` spellings, a program without a directory;
  - `crash_dump.rs`: the crash directory;
  - `integration.rs`: `--sandbox-check` denying the home directory and allowing the system's files.
