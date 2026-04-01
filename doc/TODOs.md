# Warpine TODO List

This document tracks **open work only**. Completed items are documented in the
[Developer Guide](developer_guide.md) and [Reference Manual](reference_manual.md).

## Engineering Policy

**Near-clean-room, blackbox implementation.** All API behaviour is derived from
public documentation only (IBM *Control Program Programming Reference*, OS/2 Warp 4
Toolkit headers, IBM Developer Connection, ReactOS/osFree reference). No IBM-proprietary
DLL binaries, no ROM dumps, and no disassembly of original OS/2 system libraries.

---

## Completed Phases (summary)

| Area | Status | Reference |
|------|--------|-----------|
| Phases 1–7 baseline | Complete | [Developer Guide §20](developer_guide.md#appendix-development-phases) |
| LX/NE loader, KVM vCPU, GDT/IDT | Complete | Developer Guide §3–6 |
| DOSCALLS core (file I/O, memory, threads, semaphores, IPC) | Complete | Reference Manual §9 |
| VIO/KBD console subsystem | Complete | Developer Guide §15 |
| SDL2 VGA text renderer + GNU Unifont (SBCS + DBCS 16×16) | Complete | Developer Guide §15 |
| DBCS full support (B1–B8: cell annotation, render, encode, keyboard) | Complete | Developer Guide §15 |
| PM window management + built-in controls | Complete | Developer Guide §11 |
| GPI drawing primitives (20+ ordinals) | Complete | Developer Guide §12 |
| MMPM/2 audio (DosBeep, waveaudio MCI) | Complete | Developer Guide §10 |
| NE (16-bit OS/2 1.x) execution | Complete | Developer Guide §7 |
| Unicode-internal architecture (codepage↔UTF-8 at all boundaries) | Complete | Developer Guide §16 |
| UCONV.DLL emulation | Complete | Reference Manual §9 |
| DLL loader (recursive, ref-counted, INITTERM, builtin modules) | Complete | Developer Guide §20 |
| Structured Exception Handling (SEH) — DosSetExceptionHandler, DosRaiseException, DosUnwindException | Complete | Developer Guide (SEH section) |
| DosMapCase / NlsMapCase — full SBCS + DBCS + CP866 | Complete | Developer Guide §16 |
| Developer tooling (crash dump, GDB stub, API ring buffer) | Complete | Developer Guide §19 |
| Builtin CMD.EXE host Rust shell (core built-ins + .CMD scripts) | Complete | `src/loader/cmd.rs` |
| CMD.EXE I/O redirection (`>`, `>>`, `<`) + pipe (`\|`) + sample script | Complete | `src/loader/cmd.rs`, `samples/cmd_test/test.cmd` |

---

### Rust Guest Toolchain (`i686-warpine-os2`)

A complete toolchain for writing Warpine guest programs in Rust, producing valid
LX binaries without Open Watcom. Three components with strict dependency order:

```
Phase 1  →  lx-link  (src/bin/lx_link.rs)
Phase 2  →  targets/i686-warpine-os2.json
Phase 3  →  crates/warpine-os2-sys
Phase 4  →  crates/warpine-os2-rt
Phase 5  →  crates/warpine-os2
Phase 6  →  samples/rust_hello/
Phase 7  →  tests/integration.rs  test_rust_hello
```

#### Repo layout (new files)

```
targets/
  i686-warpine-os2.json       ← custom Rust target spec
  os2api.def                  ← ordinal map: "DOSCALLS.282 DosWrite"
                                 generated from api_registry.rs REGISTRY table
src/bin/
  lx_link.rs                  ← host ELF→LX linker; shares src/lx/header.rs
crates/
  warpine-os2-sys/            ← raw extern "system" + _Optlink macro
  warpine-os2-rt/             ← _start, panic_handler, global_allocator
  warpine-os2/                ← safe ergonomic wrappers
samples/rust_hello/           ← #![no_std] smoke-test guest binary
rust-toolchain.toml           ← pins nightly + rust-src component
```

Root `Cargo.toml` gains `[workspace]` block listing the three new crates.
`lx-link` is a second `[[bin]]` in the warpine package (no separate crate needed).

---

#### A — Custom Rust Target Spec (`targets/i686-warpine-os2.json`) ✓ Complete

- [x] Create `targets/i686-warpine-os2.json`:
  - `"llvm-target": "i686-unknown-none"`, `"arch": "x86"`, `"os": "none"`
  - `"linker-flavor": "ld"`, `"linker": "lx-link"` — rustc passes raw `-o`/`.o` args, no `-Wl,` wrapping
  - `"relocation-model": "static"` — no PLT; only `R_386_32` relocations emitted (verified)
  - `"panic-strategy": "abort"`, `"no-default-libraries": true`, `"disable-redzone": true`
  - `"dynamic-linking": false`, `"plt-by-default": false`
  - `"exe-suffix": ".exe"`, `"features": "+x87,+mmx"`, `"cpu": "i686"`
  - Note: `target-pointer-width` must be integer 32, not string; `needs-plt` renamed to `plt-by-default`; `pre/post-link-args` must be `{}`
- [x] Add `rust-toolchain.toml` in guest crate dirs pinning nightly + `rust-src` component
  (do NOT add at workspace root — host warpine builds with stable)
  Build command: `cargo +nightly build -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target /path/to/targets/i686-warpine-os2.json`
- [x] Verified: `cargo +nightly build -Z build-std=core,alloc` emits **ELF 32-bit LSB relocatable, Intel i386** objects with only `R_386_32` relocations — no PLT stubs, no `R_386_PC32`

---

#### B — LX Linker (`src/bin/lx_link.rs`)

This is the core engineering effort (~8–10 days). Reads ELF relocatable objects
(via the `object` crate — add to `Cargo.toml`) and writes a valid LX binary.

**Internal module structure:**

| Module | Responsibility |
|--------|----------------|
| `args` | CLI: `.o` files, `-o out.exe`, `--def api.def`; silently ignore unknown flags |
| `elf_reader` | Sections / symbols / relocs via `object` crate |
| `def_parser` | `"DOSCALLS.282 DosWrite"` → `HashMap<name,(module,ordinal)>` |
| `linker_state` | Merge `.text`/`.data`, assign VAs, build global symbol table |
| `reloc_processor` | ELF reloc → `ResolvedReloc::Internal` or `::Import` |
| `lx_writer` | Serialize MZ stub + LX structs using types from `src/lx/header.rs` |

**Object layout:**
- Object 1 (code): base `0x00010000`, flags `READABLE|EXECUTABLE|BIG`
- Object 2 (data+bss): base `0x00020000`, flags `READABLE|WRITABLE|BIG`
- LX header field `page_offset_shift = 0` (direct byte offsets)
- `esp_object=2`, `esp = data_size + bss_size - 64` (provisional stack top)

**Relocation rules:**
- `R_386_PC32` within same merged section → patch in place, **no LX fixup needed**
- `R_386_32` to internal symbol → `source_type=0x07`, `LxFixupTarget::InternalOffset`
- `R_386_32`/`R_386_PC32` to import → `source_type=0x07`/`0x08`, `LxFixupTarget::ExternalOrdinal`
- Import resolution must mirror `resolve_import()` in `descriptors.rs` exactly;
  `os2api.def` is generated from the REGISTRY table in `api_registry.rs`

**LX file layout:**
```
[0x00] MZ stub (64 bytes, e_lfanew = 0x40)
[0x40] LX header
       object table (2 × 24 bytes)
       page map entries
       entry table (1 bundle → _start at object 1 offset 0)
       fixup page table + fixup record stream
       imported modules name table ("DOSCALLS", "VIOCALLS", …)
       raw page data (code pages then data pages)
```

**Install for use:**
```bash
cargo build --bin lx-link
# or: cargo install --path . --bin lx-link
export PATH="$PATH:$(pwd)/target/debug"
```

- [x] `args` module: parse CLI flags, collect `.o` paths
- [x] `elf_reader` module: extract sections / symbols / relocations via `object` crate
- [x] `def_parser` module + generate `targets/os2api.def` from `api_registry.rs`
- [x] `linker_state` module: merge sections, assign VAs, build symbol table
- [x] `reloc_processor` module: classify and resolve all relocations
- [x] `lx_writer` module: serialize complete LX binary
- [x] Unit test — DEF parser roundtrip
- [x] Unit test — ELF reader section/symbol extraction
- [x] Unit test — section merge contrib_map offsets
- [x] Unit test — LX roundtrip: `LxFile::open()` parses `lx-link` output correctly
- [x] Integration test: link minimal `DosWrite` + `DosExit` object → run on Warpine (`test_rust_hello`)

---

#### C — `warpine-os2` Crate Family

**`crates/warpine-os2-sys`** — raw FFI, `#![no_std]`, no `#[link]` needed:
- [x] `extern "C"` blocks for DOSCALLS ordinals (DosExit, DosWrite, DosRead, DosOpen, DosClose, DosAllocMem, DosFreeMem, DosSetMem, DosCreateThread, DosWaitThread, DosKillThread, DosGetInfoBlocks, DosSleep, DosGetDateTime, DosQuerySysInfo, event/mutex semaphore APIs)
- [x] `extern "stdcall"` blocks for VIOCALLS / KBDCALLS Pascal-convention APIs (VioWrtTTY, VioGetCurPos, VioSetCurPos, KbdCharIn, KbdGetStatus) with reversed arg order
- [x] OS/2 primitive types: `APIRET`, `HFILE`, `ULONG`, `USHORT`, `HVIO`, `HKBD`, `PVOID`, `PCSZ`, `PFNTHREAD`, `DATETIME`, `KBDKEYINFO`, `KBDINFO`

**`crates/warpine-os2-rt`** — runtime support:

Stack layout on entry to `_start` (from `vcpu.rs` `create_initial_vcpu()`):
```
[ESP+0]  EXIT_TRAP_ADDR  (return address — never used)
[ESP+4]  hmod = 0
[ESP+8]  reserved = 0
[ESP+12] env_ptr
[ESP+16] cmdline_ptr
```
`_start` ignores all stack args and calls `os2_main()` then `DosExit`.

- [x] `#[no_mangle] pub unsafe extern "C" fn _start() -> !` — calls `os2_main()` then `DosExit(1, code)`
- [x] `#[panic_handler]` — calls `DosExit(1, 1)` unconditionally
- [x] `#[global_allocator]` backed by `DosAllocMem(PAG_READ|PAG_WRITE|PAG_COMMIT=0x13)` / `DosFreeMem`

**`crates/warpine-os2`** — safe wrappers, `#![no_std]`:
- [x] `mod file` — `write_stdout()`, `write_stderr()` → `Result<usize, u32>`
- [x] `mod memory` — `alloc()`, `free()`, `set_mem()` wrapping `DosAllocMem`/`DosFreeMem`/`DosSetMem`
- [x] `mod process` — `exit()` wrapping `DosExit`
- [x] `mod thread` — `sleep()`, `create()`, `wait()`, `kill()` wrapping `DosSleep`/`DosCreateThread`/`DosWaitThread`/`DosKillThread`
- [x] `mod vio` — `write_tty()`, `get_cur_pos()`, `set_cur_pos()` wrapping VIO Pascal APIs

**`samples/rust_hello/`:**
- [x] `#![no_std] #![no_main]` guest binary: `os2_main()` writes "Hello from Rust on Warpine!\r\n" via `DosWrite` and returns 0
- [x] Integration test `test_rust_hello` in `tests/integration.rs` (graceful skip when lx-link or nightly unavailable)

---

#### Key Risks

| Risk | Mitigation |
|------|-----------|
| Unexpected ELF reloc types from LLVM | `readelf -r` on first `.o`; `lx-link` errors clearly on unknown types |
| `R_386_PC32` to imports | Use `source_type=0x08` (self-relative) LX fixup; `MAGIC_API_BASE` always in 32-bit range |
| `DosAllocMem` arg count mismatch | Read `doscalls.rs` `dos_alloc_mem` before writing sys crate |
| `lx-link` not on PATH at link time | CI step: `cargo build --bin lx-link && export PATH=…` before guest builds |

---

## Architecture Backlog

### Ordinal Table Canonical Build Tool

Build a tool to manage the authoritative ordinal→name table used by `api_registry.rs`,
sourced exclusively from public documentation. **No real OS/2 system DLLs as input**
(clean-room policy).

- [ ] Extend `LxFile` to parse entry table + resident/non-resident name tables
- [ ] `src/bin/ordinals.rs` — dump `ordinal → name` map from an Open Watcom-built LX binary; `--emit-rust` flag
- [ ] `--check` mode — cross-reference against `api_registry` to surface mismatches
- [ ] Maintain `doc/ordinals/` — one `.txt` per module (DOSCALLS, PMWIN, PMGPI, …) from IBM documentation

---

## Phase 5 — Multimedia (remaining)

- [ ] **MIDI playback** — device type `midi`; requires FluidSynth / SDL2_mixer or ALSA sequencer; deferred (external dependency cost)

---

## Phase 7 — Application Compatibility (remaining)

### 16-bit (NE) Compatibility

NE execution baseline complete (`ne_hello` runs end-to-end). Remaining:

- [ ] **Watcom CRT NE apps** — Watcom 16-bit CRT requires LDT-based selectors (TI=1); would need stub LDT or full LDT emulation
- [ ] **Mode switching** — 16-bit NE code calling a 32-bit flat DLL
- [ ] **Broader 16-bit API coverage** — more DOSCALLS / VIOCALLS / KBDCALLS ordinals beyond minimal hello-world

### PM Menu System

- [ ] **Menu template parsing** — load `MENUTEMPLATE` resource from LX binary; create `WC_MENU` window hierarchy
- [ ] **`WinLoadMenu` / `WinSetMenu`** — attach menu to frame; store `hmenu` in `OS2Window`
- [ ] **`WinSendMsg` → menu → `WM_COMMAND`** — route menu-item activations to the frame's client window procedure
- [ ] **`WM_INITMENU` / `WM_MENUSELECT`** — sent before menu is displayed / on item highlight

### Dialog System

- [ ] **Dialog template parsing** — load `DLGTEMPLATE` from LX resource; auto-create child windows; enables `WinDlgBox` / `WinLoadDlg`
- [ ] **`WinDlgBox` / `WinLoadDlg`** — modal and modeless dialog creation; own `WinGetMsg` pump
- [ ] **`WinDismissDlg`** — posts `WM_DISMISS`; unblocks `WinDlgBox`
- [ ] **`WinDefDlgProc`** — keyboard navigation, Enter/Escape, default button

### PM Advanced Controls

- [ ] **`WC_CONTAINER`** — Icon / Name / Text / Detail / Tree views; record management
- [ ] **`WC_NOTEBOOK`** — tabbed property sheet
- [ ] **Drag and drop** — `DrgDrag`, `DrgAccessDraginfo`, `DM_DRAGOVER` / `DM_DROP`
- [ ] **Custom cursors** — `WinSetPointer` via `SDL_CreateColorCursor`
- [ ] **Printing** — `DevOpenDC`, `DevCloseDC`, basic spool API stubs

### TCP/IP Socket API

- [ ] `SO32DLL.DLL` / `TCP32DLL.DLL` thunks: `socket`, `bind`, `connect`, `listen`, `accept`, `send`, `recv`, `select`, `gethostbyname`, `getservbyname`, `setsockopt`, `getsockopt`, `closesocket`
- [ ] Map to Linux BSD socket syscalls; OS/2 `SOCE*` → errno mapping
- [ ] Enables: WebExplorer, Netscape for OS/2, FTP/IRC clients

### REXX Interpreter Bridge

- [ ] Bridge `REXXAPI.DLL` exports (`RexxStart`, `RexxRegisterSubcomDll`, `RexxVariablePool`) to [Regina REXX](http://regina-rexx.sourceforge.net/)
- [ ] Unlocks: OS/2 install programs, system tools, 4OS2 `.cmd` scripts

### Year 2038

- [ ] Audit `time_t` usage in DOSCALLS and CRT shim functions
- [ ] `DosGetDateTime` / `DosSetDateTime` use `DATETIME` (`USHORT` year) — verify not affected
- [ ] `FILESTATUS3` timestamps use `FDATE`/`FTIME` (7-bit year from 1980, max 2107) — verify not affected
- [ ] Redirect CRT time imports (`CLIB.DLL` / `CRTL.DLL` / `EMX.DLL`) to 64-bit-clean host implementations
- [ ] Optional: `WARPINE.DLL` escape — `WrpGetDateTime64` / `WrpTime64` for recompilable apps

---

## Phase 8 — SOM / Workplace Shell (long-term)

The Workplace Shell (WPS) is built entirely on IBM's System Object Model (SOM).
Multi-year effort; depends on Phase 7 PM completion.

### SOM Runtime Core (prerequisite for WPS)

- [ ] Object / class model: SOM class objects, method table dispatch, offset-based and name-lookup dispatch
- [ ] `SOMClassMgrObject` — global class manager; `SOMClassMgr_somFindClass()`, class registration, DLL-based class loading
- [ ] IDL metadata: parse or reconstruct method offsets and class hierarchy at runtime
- [ ] Binary ABI compatibility with IBM SOM 2.1 (for XWorkplace, Object Desktop)

### WPS Object Hierarchy (requires SOM runtime)

- [ ] `WPObject` — root: `wpInitData`, `wpSaveState`, `wpRestoreState`, `wpQueryTitle`, `wpOpen`, `wpDragOver`, `wpDrop`
- [ ] `WPFileSystem` — `wpQueryFilename`, `wpQueryAttr`
- [ ] `WPFolder` — Icon / Detail / Tree via `WC_CONTAINER`; `wpPopulate`
- [ ] `WPDesktop` — singleton root desktop; persists object positions in OS2.INI
- [ ] `WPProgram` — launches via `DosExecPgm`; `WPDataFile` — `.TYPE` EA for app association
- [ ] Persistence via `PrfWriteProfileData` / `PrfQueryProfileData` (OS2.INI / OS2SYS.INI)
- [ ] Settings notebook: `WinLoadDlg` + `WC_NOTEBOOK` + per-class property pages
- [ ] Drag and drop: `wpDragOver` / `wpDrop` / `wpCopyObject` / `wpMoveObject`

---

## Phase 9 — XE: 64-bit OS/2-lineage Platform (far future / vision)

Define and implement a new 64-bit executable format and API set as a natural evolution
of the OS/2 lineage. XE apps run natively on Warpine alongside existing 32-bit LX apps.

### XE Executable Format

- [ ] Define spec: XE header (`"XE"` signature, `cpu_type`, `object_count`, `entry_rip: u64`), 64-bit object table, fixup records, import/export tables
- [ ] `src/xe/` parser module mirroring `src/lx/`
- [ ] `detect_format()` in `main.rs` recognises `"XE"` signature
- [ ] `Loader::load_xe()` / `run_xe()` path

### KVM Long Mode Execution

- [ ] vCPU initialisation in long mode (`EFER.LME`, 4-level paging, 64-bit GDT with FS/GS for TIB/PIB)
- [ ] 64-bit `SharedState` TIB/PIB layout
- [ ] INT 3 thunk mechanism in long mode; args from `rdi/rsi/rdx/rcx/r8/r9` (System V AMD64 ABI)

### 64-bit API Set (`DOSCALLS64`, `PMWIN64`, …)

- [ ] Core I/O: `DosWrite64`, `DosRead64`, `DosOpen64`, `DosClose64`, `DosExit64`
- [ ] Memory: `DosAllocMem64` (full 64-bit address space), `DosFreeMem64`
- [ ] Threads: `DosCreateThread64`, `DosWaitThread64`
- [ ] Synchronisation: `DosCreateEventSem64`, `DosCreateMutexSem64`
- [ ] PM: `WinInitialize64`, `WinCreateStdWindow64`, `WinGetMsg64`, `WinDispatchMsg64`
- [ ] `UCONV64.DLL` — Unicode conversion using UTF-8 natively

### Toolchain Support

- [ ] `warpine-xe` Rust crate: safe bindings to the 64-bit API; `#![no_std]` compatible
- [ ] Custom Rust target `x86_64-warpine-xe` (bare-metal, System V ABI, XE output via linker script)
- [ ] Sample XE app in Rust: `samples/xe_hello/`
- [ ] Sample XE app in C (Clang `x86_64-unknown-none`)

### Dual-ABI Coexistence

- [ ] 32-bit LX and 64-bit XE run side-by-side under the same Warpine instance
- [ ] `DosExecPgm` detects XE format and spawns a 64-bit vCPU thread
- [ ] Shared `SharedState` managers serve both 32-bit and 64-bit guests
