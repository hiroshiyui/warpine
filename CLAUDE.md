# CLAUDE.md

Guidance for AI agents (Claude Code and others) working in this repository.

## Project

Warpine is an OS/2 compatibility layer for Linux, like WINE for OS/2. It runs:
- 32-bit OS/2 programs (LX format);
- 16-bit OS/2 1.x programs (NE format).

They run natively under KVM.
- **License:** GPL-3.0-or-later (ADR-0074).
- **Goal: an OS/2-alike API and its runtime environment** — a familiar, robust API set as the foundation of a Free Software platform; an OS/2-API-compatible product, not a clone. Running every existing OS/2 application is not a goal; compatibility work for a legacy application that matters to people is done case by case. Anything that needs proprietary intellectual property is skipped, not worked around.
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
cargo test                                 # 2258 unit tests (2172 warpine + 23 gen_api + 37 lx_link + 8 os2rc + 6 os2msg + 5 gen_arch + 7 gen_compat) + 211 integration tests
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
  - The VIO / KBD / MOU samples generate a 32-bit copy of Open Watcom's `bsesub.h` at build time (`samples/bsesub32.mk`).
- **Rust guests:**
  - The crates in `crates/` (`warpine-os2-sys` / `-rt` / `warpine-os2`) are outside the host workspace, and pin `nightly-2026-11-15`.
  - Install the linker with `cargo build --bin lx_link && cp target/debug/lx_link ~/.cargo/bin/lx-link`.
  - In `samples/rust_hello`, build with `cargo build -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../targets/i686-warpine-os2.json`.
- **C guests from GCC or clang** (ADR-0095): unmodified compilers emit freestanding i386 ELF (`-m32 -ffreestanding -fno-pic`) and `lx-link` makes the LX program.
  - Build with `cargo build --bin lx_link`, then `make -C samples/c_hello` (or `CC=clang`).
  - `lx-link` refuses position-independent code and fails on undefined symbols.
- **The C SDK** (ADR-0096–0101): `sdk/include/os2.h` declares everything (Dos*, Que*, NLS, MSG, Vio*, Kbd*, Mou*, PM in `os2/pm.h` and the PM module headers it includes; `INCL_*` select nothing), `rexxsaa.h`, `os2me.h`, and a small C runtime (stdio, stdlib with `strtod`, string, math, time).
  - Build it with `make -C sdk` (or `CC=clang`); a program's Makefile includes `sdk/os2.mk` (see `samples/c_sdk_test`).
  - Resources (ADR-0099): `cargo build --bin os2rc`; `os2rc -I sdk/include x.rc -o x.res`, then pass `x.res` to `lx-link` (see `samples/c_pm_test`). Its output must stay byte-identical to `wrc -zm` (`os2rc_matches_open_watcom_wrc`).
  - Messages (ADR-0105): `cargo build --bin os2msg`; `os2msg x.txt -o x.msg` writes Warpine's own message format (`src/loader/msgfile.rs`, shared with `lx-link` through `#[path]`); a `.msg` given to `lx-link` is bound into the program (see `samples/c_msg_test`). IBM `MKMSGF` files are not read.
  - Every call a header declares must be in `targets/os2api.def`; VIO / KBD / MOU calls go through `OS2_PASCAL_API`; every integer `#define` must equal Warpine's constant of that name.
  - The def lists PMWIN / PMGPI calls the sub-dispatchers serve: a new PM call needs its def line too.
  - Structure layouts match Open Watcom's (`tests/sdk_layout`, ADR-0101): a new structure Open Watcom also has goes into `fields.inc`.
  - DLLs: `lx-link --dll --export A,B.2 [--initinstance --terminstance] $(OS2_DLL0) objs libos2c.a -o x.dll`; a `.dll` input links its exports as imports.
- **Every C sample from GCC or clang** (ADR-0101): `samples/build_sdk.sh gcc|clang [sample...]` builds into `samples/build/<cc>/`; `WARPINE_SAMPLES=gcc cargo test --test integration` runs the tests on those builds (Open Watcom's are the default). In tests, name sample paths through `sample_path` / `sample_exists`.
  - CI (ADR-0106): `ci/run.sh [watcom|gcc|clang]` runs the matrix entries of `.github/workflows/ci.yml` locally, in the pinned image of `vendor/setup_ci_image.sh` (cargo into `target/ci`, samples into `samples/build/ci-<cc>`).
  - Pinned compilers: `./vendor/setup_compilers.sh` (a Podman image from a Debian base pinned by digest, packages from a dated snapshot), then `CC_CMD=vendor/pinned/gcc OUT_TAG=gcc-pinned samples/build_sdk.sh gcc`. The container sees only the repository.
- **C++** (ADR-0103): `$(OS2_CXX) $(OS2_CXXFLAGS)` from `os2.mk` (g++ / clang++, the SDK's `include/c++`), linked with `$(OS2_LIBCXX)` before `$(OS2_LIBC)`; exceptions, RTTI, `new` / `delete`, `__cxa_atexit` (see `samples/cxx_test`). Exceptions pass between the program and its DLLs (each module's runtime registers its unwind tables, and all share a per-thread runtime block: `WARPINE.7`–`10`, `os2/warpine.h`, ADR-0107), but must not cross a call into Warpine. An OS/2 exception handler may `throw`: the unwinder steps from its return address `XCPT_RET_TRAP` into the trapping context, so the trapping frames' destructors run (ADR-0110; C++ and the C library are built with `-fasynchronous-unwind-tables`). `setjmp.h`: `longjmp` unwinds the OS/2 handlers registered since `setjmp`.
- **Source-level debugging** (ADR-0102): objects with DWARF (`-g`) make `lx-link` write `<output>.dbg`; `warpine --gdb 1234 prog.exe`, then `gdb -ex 'symbol-file prog.dbg' -ex 'target remote :1234'` (see `samples/c_gdb_test`). DLLs' companions come from the stub's library list, also after `DosLoadModule` (ADR-0104).
- **Generators:** each has a test that fails when its output is stale.
  - `cargo run --bin gen_api -- check | validate-doc | check-headers`: `targets/os2api.def` is the single source of the ordinals; `check-headers` holds the SDK's headers to it (ADR-0096).
  - `cargo run --bin gen_arch -- check | render` (ADR-0021): a new, renamed or removed module needs `doc/architecture/layers.toml` updated and a render.
  - `cargo run --bin gen_compat -- render` (ADR-0033): needed after an API is added, removed or stubbed.

## Architecture in brief

1. **Execution.**
   - `main.rs` parses the executable (`src/lx/`, `src/ne/`).
   - `Loader::load` maps it into guest memory and applies its fixups.
   - `Loader::run` sets up the TIB / PIB and runs a vCPU (`vcpu.rs`).
   - API calls are INT 3 thunks at `MAGIC_API_BASE` + ordinal. The VMEXIT goes to `api_dispatch.rs`: the static registry (`api_registry.rs`, 478 entries) or a sub-dispatcher (VIOCALLS / KBDCALLS / MOUCALLS / PMWIN / PMGPI, 407 ordinals), 885 entry points in all (`--compat` lists them).
2. **Threads.** One OS/2 thread is one host thread with its own vCPU.
   - 32-bit threads run paged with their own page tables: memory is mapped one-to-one except the thread-local page (`paging.rs`, ADR-0088); host accesses there go to the running thread's page (`guest_mem.rs`).
   - The guest runs at ring 0: the host never walks its page tables, and a guest that changes its paging (caught by the `os2-paging-watch` kicks) or spins on memory past its RAM ends as a trap. After an MMIO exit, `finish_mmio` completes the access before the registers change.
   - Managers live in `SharedState` behind mutexes; see the lock levels in `src/loader/mod.rs`.
   - Notable orders: pointers before `window_mgr`; profiles before `drive_mgr`; the window manager before a surface, never the reverse.
   - Never call the desktop link under the window manager's lock.
3. **Guest calls from the host** (window procedures, hooks, owner drawing, sends between programs) run as call chains on the guest's thread (`pm_hook.rs`: `ApiResult::CallChain`, `ChainThen`, ADR-0026).
4. **PM.**
   - API threads rasterise into shared per-frame surfaces (`src/gui/surface.rs`); the GUI thread composites them (`src/gui/sdl_renderer.rs`, software or layered compositor).
   - Input waits in one system input queue and is routed when read (`pm_input.rs`, ADR-0051 / 0058).
   - Each session has one desktop server process (`src/desktop/`, `src/gui/desktop_server.rs`; protocol version 10, ADR-0052–0057, 0067, 0068, 0108). `WARPINE_DESKTOP=local` keeps the desktop in-process.
5. **Processes.** Each OS/2 process is its own Warpine host process (ADR-0020).
   - Shared memory goes through a sealed memfd arena (ADR-0049); shared semaphores through a memfd of slots (ADR-0050).
   - Named pipes are `AF_UNIX` sockets in a private session directory (ADR-0022).
6. **Files.** OS/2 drives map to host directories through the VFS (`vfs.rs`, `vfs_hostdir.rs`); drive C is `$XDG_DATA_HOME/warpine/drive_c`. Guests never reach host paths outside their drives.
7. **Built-in modules.** DOSCALLS, PMWIN, PMGPI, VIOCALLS, KBDCALLS, MOUCALLS, NLS, MSG, MDM, UCONV, SO32DLL / TCP32DLL, PMSHAPI, HELPMGR, PMSPL, PMCTLS, PMDRAG, SOM, REXX / REXXAPI (on the built-in REXX interpreter), PMWP (window positions only) and Warpine's own WARPINE.
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
| `EXE_AREA_START` – `EXE_AREA_END` | `0x10000` – `0x01000000` | The program area: an EXE's objects must lie inside it, without overlapping, or the load is refused (ADR-0094) |
| `SYSTEM_AREA_BASE` | `0x01800000` (256 KB) | Warpine's own structures, outside every program (ADR-0094): GDT, IDT, TIB, PIB, environment page, `TLM_PAGE`, information segments |
| `TIB_BASE` / `PIB_BASE` | `0x01810000` / `0x01811000` | Thread 1's block and the process block, in the system area |
| `TLM_PAGE` | `0x0181F000` | The thread-local memory page: each 32-bit thread's own page tables (`paging.rs`, ADR-0088) map it to the thread's own page |
| `FS_WINDOW` | `0x01840000` | Mapped by each thread's tables to its own TIB; selector `0x18`'s base, so reloading FS keeps the TIB (ADR-0110). The low 64 KB are not present: NULL pointers trap |

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

- Every Warpine process confines itself with Landlock (`src/sandbox.rs`, ADR-0111): a new host path a process must reach goes into `sandbox::policy`. `WARPINE_SANDBOX=off` turns it off; crash dumps go to `$XDG_STATE_HOME/warpine`.
- The parsers of untrusted input are fuzzed (`fuzz/`, ADR-0112): `fuzz/seed.sh`, then `fuzz/run.sh <target> [seconds]`; a new parser of guest- or file-supplied data gets a target.

- Always guard against hypervisor escape; memory safety is the top priority.
- Guests never reach the host environment, host Unix sockets or host paths outside their drives (ADR-0066).
- Third-party inputs are pinned and verified:
  - dated downloads with SHA-256;
  - full commit SHAs;
  - `deny.toml` for crates, checked by the hook with `cargo deny check sources bans licenses`.
