# CLAUDE.md

Guidance for AI agents working in this repository.

## Project

Warpine is an OS/2 compatibility layer for Linux, like WINE for OS/2: 32-bit (LX) and 16-bit OS/2 1.x (NE) programs run natively under KVM. GPL-3.0-or-later (ADR-0074).
- **Goal:** an OS/2-alike API and runtime as the foundation of a Free Software platform — API-compatible, not a clone. Running every OS/2 application is not a goal; legacy applications that matter are handled case by case. Anything needing proprietary IP is skipped, not worked around.
- **Near-clean-room, blackbox policy:**
  - behaviour from public documentation only (IBM's *Control Program* and *PM* references, the Warp 4 Toolkit headers, IBM Developer Connection, osFree / ReactOS);
  - no IBM binaries, ROM dumps or disassembly of OS/2 system libraries;
  - Warpine's own choices are marked unverified, in their ADR and [`doc/unverified.md`](doc/unverified.md);
  - the test oracle is the observable behaviour of Open Watcom-built programs.
- **Docs:** [`doc/developer_guide.md`](doc/developer_guide.md) (how it is built), [`doc/reference_manual.md`](doc/reference_manual.md) (what the APIs do; architecture diagram at its head), [`doc/adr/`](doc/adr/README.md) (why), [`doc/TODOs.md`](doc/TODOs.md) (open work), [`CHANGELOG.md`](CHANGELOG.md), [`samples/README.md`](samples/README.md).

## Build & test

```bash
cargo build
cargo run -- samples/hello/hello.exe       # run an OS/2 program
cargo test                                 # 2383 unit tests (2282 warpine + 23 gen_api + 39 lx_link + 12 os2rc + 9 somidl + 6 os2msg + 5 gen_arch + 7 gen_compat) + 227 integration tests
cargo clippy --all-targets -- -D warnings  # must be clean before every commit (the pre-commit hook runs it)
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p TasksMax=4096 -- cargo test   # as the hook runs it (ADR-0031); then `pgrep -c -x warpine` must be 0
WARPINE_UPDATE_GOLDEN=1 cargo test --test integration gui_   # regenerate golden pictures after an intended rendering change
```

- **Prerequisites:** Linux with `/dev/kvm` on x86_64; Rust 1.100.0 (`rust-toolchain.toml`, with clippy and rustfmt); `libsdl3-dev` via pkg-config.
- **Integration tests** skip when KVM or a sample is missing: guard with `can_run(sample)` (`run_sample` fails on a missing sample). GUI tests run on SDL's `dummy` driver under scripts (`WARPINE_GUI_SCRIPT`, `src/gui/gui_script.rs`) and compare dumps with `tests/golden/*.sig`.
- **Open Watcom samples:** `./vendor/setup_watcom.sh` fetches a pinned, dated snapshot checked by SHA-256 (ADR-0066; `--refresh` replaces one older than 2026-08-11, ADR-0036); then `make -C samples/<name>` or `make -C samples`. VIO / KBD / MOU samples generate a 32-bit `bsesub.h` (`samples/bsesub32.mk`).
- **Rust guests:** `crates/` (`warpine-os2-sys` / `-rt` / `warpine-os2`) are outside the workspace and pin `nightly-2026-11-15`. Linker: `cargo build --bin lx_link && cp target/debug/lx_link ~/.cargo/bin/lx-link`. In `samples/rust_hello`: `cargo build -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../targets/i686-warpine-os2.json`.
- **C from GCC / clang** (ADR-0095): freestanding i386 ELF (`-m32 -ffreestanding -fno-pic`) linked by `lx-link` (`cargo build --bin lx_link`, then `make -C samples/c_hello`, or `CC=clang`); it refuses PIC and undefined symbols.
- **The C SDK** (ADR-0096–0101): `sdk/include/os2.h` declares everything (Dos*, Que*, NLS, MSG, Vio*, Kbd*, Mou*; PM in `os2/pm.h` and its module headers; `INCL_*` select nothing), plus `rexxsaa.h`, `os2me.h` and a small C runtime. `make -C sdk`; a Makefile includes `sdk/os2.mk` (`samples/c_sdk_test`).
  - Resources (ADR-0099): `os2rc -I sdk/include x.rc -o x.res`, `.res` to `lx-link` (`samples/c_pm_test`); output byte-identical to `wrc -zm` (`os2rc_matches_open_watcom_wrc`).
  - SOM IDL (ADR-0122, in progress): `somidl -I dir --dump x.idl` parses SOM IDL (front end only so far; emitters to come), through the C preprocessor `src/cpp.rs` it shares with `os2rc`.
  - Messages (ADR-0105): `os2msg x.txt -o x.msg`, Warpine's own format (`src/loader/msgfile.rs`, shared with `lx-link` via `#[path]`), bound by `lx-link` (`samples/c_msg_test`). IBM `MKMSGF` files are not read.
  - Every declared call is in `targets/os2api.def` (PMWIN / PMGPI sub-dispatcher calls too); VIO / KBD / MOU use `OS2_PASCAL_API`; every integer `#define` equals Warpine's constant of that name. Structure layouts match Open Watcom's (`tests/sdk_layout`, ADR-0101): a new shared structure goes into `fields.inc`.
  - DLLs: `lx-link --dll --export A,B.2 [--initinstance --terminstance] $(OS2_DLL0) objs libos2c.a -o x.dll`; a `.dll` input links its exports as imports.
- **Every C sample from GCC / clang** (ADR-0101): `samples/build_sdk.sh gcc|clang [sample...]` → `samples/build/<cc>/`; `WARPINE_SAMPLES=gcc cargo test --test integration` tests them (Open Watcom's are the default); tests name samples via `sample_path` / `sample_exists`.
  - CI (ADR-0106): `ci/run.sh [watcom|gcc|clang]` runs `.github/workflows/ci.yml`'s matrix locally in the pinned image of `vendor/setup_ci_image.sh` (cargo into `target/ci`, samples into `samples/build/ci-<cc>`; a package-list change bumps `IMAGE_REVISION`; older images are removed). After pushing, check the Actions run; never release on a red CI.
  - Pinned compilers: `./vendor/setup_compilers.sh` (Podman, Debian base by digest, dated snapshot), then `CC_CMD=vendor/pinned/gcc OUT_TAG=gcc-pinned samples/build_sdk.sh gcc`; the container sees only the repository.
- **C++** (ADR-0103): `$(OS2_CXX) $(OS2_CXXFLAGS)`, linked with `$(OS2_LIBCXX)` before `$(OS2_LIBC)`: exceptions, RTTI, `new` / `delete`, `__cxa_atexit` (`samples/cxx_test`). Exceptions pass between a program and its DLLs (shared per-thread runtime block `WARPINE.7`–`10`, `os2/warpine.h`, ADR-0107) but must not cross a call into Warpine. An OS/2 exception handler may `throw` (unwinding from `XCPT_RET_TRAP` into the trapping frames, ADR-0110; all SDK code has `-fasynchronous-unwind-tables`). `longjmp` unwinds the OS/2 handlers registered since `setjmp`.
- **Debugging** (ADR-0102, 0104): `-g` objects make `lx-link` write `<output>.dbg`; `warpine --gdb 1234 prog.exe`, then `gdb -ex 'symbol-file prog.dbg' -ex 'target remote :1234'` (`samples/c_gdb_test`); DLL companions follow, also after `DosLoadModule`.
- **Generators** (each has a test that fails when stale):
  - `cargo run --bin gen_api -- check | validate-doc | check-headers`: `targets/os2api.def` is the single source of ordinals; `check-headers` holds the SDK to it (ADR-0096);
  - `cargo run --bin gen_arch -- check | render` (ADR-0021): a new, renamed or removed module needs `doc/architecture/layers.toml` and a render;
  - `cargo run --bin gen_compat -- render` (ADR-0033): after an API is added, removed or stubbed.

## Architecture in brief

1. **Execution:** `main.rs` parses the executable (`src/lx/`, `src/ne/`); `Loader::load` maps it and applies fixups; `Loader::run` sets up TIB / PIB and runs a vCPU (`vcpu.rs`). API calls are INT 3 thunks at `MAGIC_API_BASE` + ordinal; the VMEXIT goes to `api_dispatch.rs`: the registry (`api_registry.rs`, 480 entries) or a sub-dispatcher (VIOCALLS / KBDCALLS / MOUCALLS / PMWIN / PMGPI, 415 ordinals) — 895 entry points (`--compat`).
2. **Threads:** one OS/2 thread = one host thread with its own vCPU.
   - All threads run paged at ring 3 (`ring.rs`, ADR-0113: DPL-3 descriptors, a TSS and ring-0 stack per thread). 32-bit threads have their own page tables, identity-mapped except the thread-local page (`paging.rs`, ADR-0088; host accesses there go to the running thread's page, `guest_mem.rs`). GDT, IDT, ring-0 window and the paging arena (≤ 738 threads, ADR-0115) are supervisor-only, thunks read-only, CR0.WP set. No guest ring 0, no paging watch.
   - Warpine never writes its protected memory (GDT, IDT, stubs, paging arena) through a guest-given address: `guest_write` / `guest_write_bytes` / `guest_slice_mut` refuse it (`guest_mem::protected`); only Warpine's setup uses `system_write` / `system_write_bytes`, as must any new structure there.
   - The host never walks guest page tables; spinning past RAM ends as a trap. After an MMIO exit `finish_mmio` completes the access before registers change.
   - OS/2 page states (`vmem.rs`, ADR-0114): `DosAllocMem` / `DosSetMem` reserve, commit, decommit, read-only and guard pages (`XCPT_GUARD_PAGE_VIOLATION`) in shared process tables. Thread stacks (main too: `STACKSIZE` below the entry ESP, `MainStack`) are committed as touched, `XCPT_UNABLE_TO_GROW_STACK` near the bottom (`allocate_stack`, `grow_stack`). `DosQueryMem` reports `PAG_EXECUTE` for executable objects. Lost access flushes every vCPU's TLB (`flush_tlbs`). Objects without the LX write flag are read-only (`protect_objects`). Free memory is not present (`sync_allocations` before each vCPU runs guest code).
   - Managers live in `SharedState` behind mutexes (lock levels in `src/loader/mod.rs`). Orders: pointers before `window_mgr`; profiles before `drive_mgr`; window manager before a surface, never the reverse. Never call the desktop link under the window manager's lock.
3. **Guest calls from the host** (window procedures, hooks, owner drawing, cross-program sends) run as call chains on the guest's thread (`pm_hook.rs`: `ApiResult::CallChain`, `ChainThen`, ADR-0026).
4. **PM:** API threads rasterise into per-frame surfaces (`src/gui/surface.rs`); the GUI thread composites them (`src/gui/sdl_renderer.rs`). Input waits in one system queue, routed when read (`pm_input.rs`, ADR-0051 / 0058). One desktop server process per session (`src/desktop/`, `src/gui/desktop_server.rs`; protocol 12; ADR-0052–0057, 0067, 0068, 0108); `WARPINE_DESKTOP=local` keeps it in-process.
5. **Processes:** each OS/2 process is a host process (ADR-0020). Shared memory: a sealed memfd arena (ADR-0049); shared semaphores: a memfd of slots (ADR-0050); named pipes: `AF_UNIX` sockets in a private session directory (ADR-0022).
6. **Files:** drives map to host directories via the VFS (`vfs.rs`, `vfs_hostdir.rs`); C: is `$XDG_DATA_HOME/warpine/drive_c`. Guests never reach host paths outside their drives.
7. **Built-in modules:** DOSCALLS, PMWIN, PMGPI, VIOCALLS, KBDCALLS, MOUCALLS, NLS, MSG, MDM, UCONV, SO32DLL / TCP32DLL, PMSHAPI, HELPMGR, PMSPL, PMCTLS, PMDRAG, SOM, REXX / REXXAPI, PMWP (window positions only), WARPINE. Each has an ordinal base (`constants.rs`, `module_base`); PM sub-dispatcher arms must match `doc/os2_ordinals.md` (`test_pm*_dispatch_ordinals_match_ordinal_table`).

**Guest memory layout** (`constants.rs`):

| Constant | Value | What it is |
|---|---|---|
| `EXE_AREA_START` – `EXE_AREA_END` | `0x10000` – `0x01000000` | Program area; an EXE's objects must fit without overlap (ADR-0094) |
| `MAGIC_API_BASE` / `EXIT_TRAP_ADDR` | `0x01000000` / `0x010003FF` | API thunks / exit trap |
| `PAGING_EXTRA_BASE` – `_END` | `0x01400000` – `0x01800000` | Supervisor-only large page: 256 more paging blocks (738 threads, ADR-0115) |
| `SYSTEM_AREA_BASE` | `0x01800000` (256 KB) | GDT, IDT, TIB, PIB, environment, `TLM_PAGE`, info segments (ADR-0094) |
| `TIB_BASE` / `PIB_BASE` | `0x01810000` / `0x01811000` | Thread 1's TIB, the PIB |
| `TLM_PAGE` | `0x0181F000` | Thread-local page, mapped per 32-bit thread (ADR-0088) |
| `FS_WINDOW` | `0x01840000` | Each thread's TIB; selector `0x18`'s base (ADR-0110); low 64 KB not present (NULL traps) |
| `RING0_WINDOW` | `0x01841000` | Each thread's TSS (`TSS_SEL`) and ring-0 stack (ADR-0113) |
| `PAGING_ARENA_BASE` – `_END` | `0x01842000` – `0x02000000` | Supervisor-only: `PROCESS_TABLES` (4 + 48 pages, ADR-0114), then 482 threads' paging blocks (4 pages each); last 4 MB one large page |
| `DYNAMIC_ALLOC_BASE` | `0x02000000` | Allocations, up to `SHARED_ARENA_BASE` |
| `SHARED_ARENA_BASE` | `0x0E000000` | The session's shared arena (32 MB) |
| `GUEST_MEM_SIZE` | 256 MB | Guest memory |

PIB: `pib_ulpid` `+0x00`, `ulppid` `+0x04`, `hmte` `+0x08`, `pchcmd` `+0x0C`, `pchenv` `+0x10`.

## Conventions

- **Priorities:** (1) safety and security — hypervisor-escape prevention, memory safety, guest isolation; (2) long-term readability; (3) validation that checks behaviour, not just that tests run.
- **Code:** API emulation, loader logic and format parsing in their own modules; named constants, no magic numbers; clear stubs for unimplemented APIs; OS/2 error codes return in EAX.
- **Unsafe:** only for KVM and guest memory, with a safety comment; guest addresses and lengths bounded before use; `src/gui` and `mmpm.rs` are `#![forbid(unsafe_code)]`; `clippy.toml` forbids `std::env::set_var` / `remove_var`.
- **Tests and samples:** every feature or fix comes with tests, and an Open Watcom sample in `samples/` (ok/FAIL lines, scripted where it needs input) with an integration test in `tests/integration.rs`.
- **When in doubt** about scope or meaning, ask.

### After every change

1. Update `README.md`, `CHANGELOG.md`, `doc/*.md`, `samples/README.md` and this file when its facts change; remove finished items from `doc/TODOs.md`; record unverified choices.
2. An architectural decision (module slot, storage format, threading or rendering model, dependency, policy) gets an ADR in `doc/adr/` (next number, Nygard format, listed in its README); a changed decision supersedes the old record.
3. Add the missing tests, run clippy and the generator checks.
4. After fixing a bug, look for the same bug elsewhere.

### Commits and releases

- Commit by topic, as Conventional Commits; never skip the hook (`--no-verify`).
- Release: `CHANGELOG.md` entry (Keep a Changelog) → bump `Cargo.toml` `version` → commit → tag `vX.Y.Z` and push it → `gh release create`.

### Security rules

- A session's first process puts it in a systemd scope (8 GB, 8192 tasks; `src/cgroup.rs`, ADR-0116), never leaving a tighter cgroup, so tests stay in the hook's scope; `WARPINE_CGROUP=off` disables it.
- Every process confines itself with Landlock (`src/sandbox.rs`, ADR-0111): a new host path goes into `sandbox::policy`; `WARPINE_SANDBOX=off` disables it; crash dumps go to `$XDG_STATE_HOME/warpine`.
- Parsers of untrusted input are fuzzed (`fuzz/`, ADR-0112: `fuzz/seed.sh`, `fuzz/run.sh <target> [seconds]`); a new parser of guest- or file-supplied data gets a target.
- Guests never reach the host environment, host Unix sockets or host paths outside their drives (ADR-0066).
- Third-party inputs are pinned and verified: dated downloads with SHA-256, full commit SHAs, `deny.toml` for crates (`cargo deny check sources bans licenses` in the hook).
