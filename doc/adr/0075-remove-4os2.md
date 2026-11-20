# ADR-0075: Remove 4OS2; the built-in CMD.EXE is Warpine's shell

- **Status:** Accepted; makes ADR-0066's 4OS2 pin moot
- **Date:** 2026-11-20
- **Sources:**
  - 4OS2's `license.txt` (J.P. Software), at the commit `fetch_source.sh` pinned (`7e56d1ca63d4fb0df8d55a748656f0eb5c1d3494`);
  - the code: `src/loader/cmd.rs` (the built-in shell, ADR-0014), `src/loader/vfs.rs` (`resolve_new_host_path`), `src/loader/vfs_hostdir.rs` (`to_host_path_for_create`, `enforce_sandbox`), `src/loader/vcpu.rs` (`COMSPEC`);
  - the samples: `samples/bsesub32.mk`, `samples/dll_noinit`;
  - the tests: `tests/integration.rs` (the `run_cmd_*` helpers).

## Context

4OS2 was Warpine's main text-mode test program: a full OS/2 command shell exercising most of DOSCALLS, VIO and KBD. `samples/4os2` fetched its source and patched it. Several of the fetched and patched files had also been committed:
- `c/os2calls.c`, modified;
- `h/bsesub.h`, a modified Open Watcom header;
- the patches themselves, which contain 4OS2 and Open Watcom text.

An intellectual-property review found that these copies did not meet their licences' conditions:
- **4OS2.** Its licence permits redistribution only with its full notice, a J.P. Software notice in the code and a dated notice of changes, and none were there.
- **Open Watcom.** The modified header carries the Open Watcom licence's obligations.

The project's goal is an OS/2-alike API and its runtime environment, not a clone. Running existing applications is not required, and anything that needs proprietary or encumbered material is skipped. The built-in `CMD.EXE` (ADR-0014), Warpine's own code, already covers a shell's everyday use.

## Decision

- **4OS2 is removed:** `samples/4os2` and everything it fetched or patched. Git history is kept as it is: the old commits still contain the files, and the project chose not to rewrite them.
- **The built-in `CMD.EXE` is Warpine's shell.**
  - The default `COMSPEC` is `C:\CMD.EXE`.
  - The text-mode integration tests run it instead of 4OS2:
    - the VIO window: resize, maximize, minimize, mark, Copy All, paste;
    - full screen and Alt+Home;
    - `cls` and redirection;
    - the end of piped input.
  - The pipe into an external program is ignored until the shell supports it.
- **The VIO / KBD / MOU samples** that compiled against 4OS2's patched `bsesub.h` now generate the same header at build time from the local Open Watcom install, replacing `APIENTRY16` with `_Pascal` (`samples/bsesub32.mk`). No Open Watcom text is kept in the tree.
- **The DLL-loader unit tests** used 4OS2's `jpos2dll.dll`. They now use `samples/dll_noinit/noinit.dll`, Warpine's own DLL built without the C runtime, with three exports in both names tables and no initialization routine.
- **Redirection stays inside the drives.** Moving the tests uncovered a security bug in the built-in shell. When a redirection target (`> file`) did not exist yet, the shell opened the guest's path string as a host path, so a guest could start `CMD.EXE /C … > path` and write host files outside its drives (ADR-0066).
  - Redirection now goes through the VFS's create path (`DriveManager::resolve_new_host_path`, the backend's `resolve_for_create`). A path outside the drives is an error.
  - Creating a file through a dangling symbolic link, which would place it at the link's target, is now refused for every new file, `DosOpen` included.

## Consequences

- **Licences.** The tree no longer distributes 4OS2 or modified Open Watcom files.
- **Test coverage.** Tests that exercised the guest VIO path through a real OS/2 shell now run Warpine's host-side shell. The VIO, KBD and MOU guest paths remain covered by the VIO samples (`screen_test`, `ansi_test`, `dbcs_test`, `mou_test`, `kill_test` …).
- **What the built-in shell still lacks** (`doc/TODOs.md`): pipes into external programs, `PROMPT`, the batch-control commands and others. They are added as needed.
- **Screenshots.** The README's text-mode pictures show the built-in shell.
- **Tests:**
  - `vfs.rs`: `new_host_paths_stay_inside_the_drive`;
  - `cmd.rs`: `test_cmd_redirect_stays_inside_the_drives` and the redirect tests on a real drive;
  - the moved integration tests and the `noinit.dll` loader tests.
