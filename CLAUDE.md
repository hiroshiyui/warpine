# CLAUDE.md

Guidance for AI agents (Claude Code and others) working in this repository.

## Project

Warpine is an OS/2 compatibility layer for Linux, like WINE for OS/2. It runs:
- 32-bit OS/2 programs (LX format);
- 16-bit OS/2 1.x programs (NE format).

They run natively under KVM.
- **License:** GPL-3.0-only.
- **Engineering policy: near-clean-room, blackbox implementation.**
  - API behaviour comes from public documentation only: IBM's *Control Program* and *PM Programming References*, the OS/2 Warp 4 Toolkit headers, IBM Developer Connection, and osFree / ReactOS references.
  - No IBM binaries, ROM dumps or disassembly of OS/2 system libraries.
  - Behaviour Warpine had to choose is marked unverified, in its ADR and in [`doc/unverified.md`](doc/unverified.md).
  - The test oracle is the observable behaviour of Open Watcom-built OS/2 programs.
- **Where to read more:**
  - [`doc/developer_guide.md`](doc/developer_guide.md): how it is built, module by module;
  - [`doc/reference_manual.md`](doc/reference_manual.md): what the APIs do; the architecture diagram is at its head;
  - [`doc/adr/`](doc/adr/README.md): why things are as they are;
  - [`doc/TODOs.md`](doc/TODOs.md): open work;
  - [`CHANGELOG.md`](CHANGELOG.md): history;
  - [`samples/README.md`](samples/README.md): the sample programs.

## Build & test

```bash
cargo build
cargo run -- samples/hello/hello.exe       # run an OS/2 program
cargo test                                 # 2026 unit tests (1983 warpine + 19 gen_api + 13 lx_link + 5 gen_arch + 6 gen_compat) + 167 integration tests
cargo clippy --all-targets -- -D warnings  # must be clean before every commit (the pre-commit hook runs it)
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p TasksMax=4096 -- cargo test   # as the hook runs it (ADR-0031); afterwards `pgrep -c -x warpine` must be 0
WARPINE_UPDATE_GOLDEN=1 cargo test --test integration gui_   # regenerate golden pictures after an intended rendering change
```

- **Prerequisites:**
  - Linux with `/dev/kvm` on x86_64 (VT-x / AMD-V);
  - Rust 1.100.0, pinned in `rust-toolchain.toml` with clippy and rustfmt;
  - `libsdl3-dev`, found through pkg-config.
- **Integration tests:**
  - They skip when KVM or a sample binary is missing.
  - GUI tests run on SDL's `dummy` driver, under scripts (`WARPINE_GUI_SCRIPT`, `src/gui/gui_script.rs`), and compare dumps with `tests/golden/*.sig`.
- **Open Watcom samples:**
  - Fetch the compiler with `./vendor/setup_watcom.sh`: a pinned, dated snapshot checked by SHA-256 (ADR-0066); `--refresh` replaces a snapshot older than 2026-08-11 (ADR-0036).
  - Then run `make -C samples/<name>`, or `make -C samples` for all.
  - 4OS2: `cd samples/4os2 && ./fetch_source.sh && make`.
- **Rust guests:**
  - The crates in `crates/` (`warpine-os2-sys` / `-rt` / `warpine-os2`) are outside the host workspace, and pin `nightly-2026-11-15`.
  - Install the linker with `cargo build --bin lx_link && cp target/debug/lx_link ~/.cargo/bin/lx-link`.
  - In `samples/rust_hello`, build with `cargo build -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../targets/i686-warpine-os2.json`.
- **Generators:** each has a test that fails when its output is stale.
  - `cargo run --bin gen_api -- check | validate-doc`: `targets/os2api.def` is the single source of the ordinals.
  - `cargo run --bin gen_arch -- check | render` (ADR-0021): a new, renamed or removed module needs `doc/architecture/layers.toml` updated and a render.
  - `cargo run --bin gen_compat -- render` (ADR-0033): needed after an API is added, removed or stubbed.

## Architecture in brief

1. **Execution.**
   - `main.rs` parses the executable (`src/lx/`, `src/ne/`).
   - `Loader::load` maps it into guest memory and applies its fixups.
   - `Loader::run` sets up the TIB / PIB and runs a vCPU (`vcpu.rs`).
   - API calls are INT 3 thunks at `MAGIC_API_BASE` + ordinal. The VMEXIT goes to `api_dispatch.rs`: the static registry (`api_registry.rs`, 380 entries) or a sub-dispatcher (VIOCALLS / KBDCALLS / MOUCALLS / PMWIN / PMGPI, 363 ordinals), 743 entry points in all (`--compat` lists them).
2. **Threads.** One OS/2 thread is one host thread with its own vCPU.
   - Managers live in `SharedState` behind mutexes; see the lock levels in `src/loader/mod.rs`.
   - Notable orders: pointers before `window_mgr`; profiles before `drive_mgr`; the window manager before a surface, never the reverse.
   - Never call the desktop link under the window manager's lock.
3. **Guest calls from the host** (window procedures, hooks, owner drawing, sends between programs) run as call chains on the guest's thread (`pm_hook.rs`: `ApiResult::CallChain`, `ChainThen`, ADR-0026).
4. **PM.**
   - API threads rasterise into shared per-frame surfaces (`src/gui/surface.rs`); the GUI thread composites them (`src/gui/sdl_renderer.rs`, software or layered compositor).
   - Input waits in one system input queue and is routed when read (`pm_input.rs`, ADR-0051 / 0058).
   - Each session has one desktop server process (`src/desktop/`, `src/gui/desktop_server.rs`; protocol version 8, ADR-0052–0057, 0067, 0068). `WARPINE_DESKTOP=local` keeps the desktop in-process.
5. **Processes.** Each OS/2 process is its own Warpine host process (ADR-0020).
   - Shared memory goes through a sealed memfd arena (ADR-0049); shared semaphores through a memfd of slots (ADR-0050).
   - Named pipes are `AF_UNIX` sockets in a private session directory (ADR-0022).
6. **Files.** OS/2 drives map to host directories through the VFS (`vfs.rs`, `vfs_hostdir.rs`); drive C is `$XDG_DATA_HOME/warpine/drive_c`. Guests never reach host paths outside their drives.
7. **Built-in modules.** DOSCALLS, PMWIN, PMGPI, VIOCALLS, KBDCALLS, MOUCALLS, NLS, MSG, MDM, UCONV, SO32DLL / TCP32DLL, PMSHAPI, HELPMGR, PMSPL, PMCTLS, PMDRAG, SOM and Warpine's own WARPINE.
   - Each has an ordinal base in `constants.rs` (`module_base`).
   - PM sub-dispatcher arms must match `doc/os2_ordinals.md`; the `test_pm*_dispatch_ordinals_match_ordinal_table` tests check it.

**Guest memory layout** (`constants.rs`):

| Constant | Value | What it is |
|---|---|---|
| `MAGIC_API_BASE` | `0x01000000` | API thunks |
| `EXIT_TRAP_ADDR` | `0x010003FF` | Exit trap |
| `DYNAMIC_ALLOC_BASE` | `0x02000000` | Allocations, up to `SHARED_ARENA_BASE` |
| `GUEST_MEM_SIZE` | 256 MB | Guest memory |
| `SHARED_ARENA_BASE` | `0x0E000000` | The session's shared arena (32 MB) |
| `TIB_BASE` / `PIB_BASE` | `0x90000` / `0x91000` | Thread and process blocks; must stay below 1 MB |

The PIB holds `pib_ulpid` at `+0x00`, `ulppid` at `+0x04`, `hmte` at `+0x08`, `pchcmd` at `+0x0C` and `pchenv` at `+0x10`.

## Conventions

- **Priorities:**
  1. Safety and security: hypervisor-escape prevention, memory safety and guest isolation come first.
  2. Long-term readability.
  3. Validation that checks behaviour, not just that tests run.
- **Code:**
  - Keep API emulation, loader logic and format parsing in their own modules.
  - Use named constants, never magic numbers.
  - Unimplemented APIs get clear stubs.
  - OS/2 error codes return in EAX.
- **Unsafe code:**
  - Only for KVM and guest memory, with a safety comment.
  - Guest-supplied addresses and lengths are bounded before use.
  - `src/gui` and `mmpm.rs` are `#![forbid(unsafe_code)]`.
  - `clippy.toml` forbids `std::env::set_var` / `remove_var`.
- **Tests and samples:**
  - Every feature or fix comes with tests.
  - Write an Open Watcom sample in `samples/` (ok/FAIL lines, scripted where it needs input) to check assumptions, with an integration test in `tests/integration.rs`.
- **When in doubt** about scope or meaning, ask rather than guess.

### After every change

1. Update the docs:
   - `README.md`, `CHANGELOG.md`, `doc/*.md`, `samples/README.md`, and this file when its facts change;
   - remove finished items from `doc/TODOs.md`;
   - record unverified choices.
2. An architectural decision (a module slot, a storage format, a threading or rendering model, a dependency, a policy) gets an ADR in `doc/adr/`: the next number, Nygard format, listed in its README. A changed decision supersedes the old record.
3. Add the missing tests, run clippy, and run the generator checks above.
4. After fixing a bug, look for the same bug elsewhere.

### Commits and releases

- **Commits:**
  - Commit by topic, as Conventional Commits.
  - Never skip the hook (`--no-verify`).
- **Releases:**
  1. Add a `CHANGELOG.md` entry (Keep a Changelog).
  2. Bump `version` in `Cargo.toml`.
  3. Commit.
  4. Tag `vX.Y.Z` and push the tag.
  5. Run `gh release create`.

### Security rules

- Always guard against hypervisor escape; memory safety is the top priority.
- Guests never reach the host environment, host Unix sockets or host paths outside their drives (ADR-0066).
- Third-party inputs are pinned and verified:
  - dated downloads with SHA-256;
  - full commit SHAs;
  - `deny.toml` for crates, checked by the hook with `cargo deny check sources bans licenses`.
