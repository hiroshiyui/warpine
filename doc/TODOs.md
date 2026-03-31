# Warpine TODO List

This document tracks the tasks required to reach a functional OS/2 compatibility layer.

## Engineering Policy

**Near-clean-room, blackbox implementation.** Warpine implements the OS/2 API surface from public documentation only — IBM's *Control Program Programming Reference*, the OS/2 Warp 4 Toolkit headers, published IBM Developer Connection materials, and open-source reference implementations (e.g., ReactOS, osFree, WINE analogues). No IBM-proprietary DLL binaries, no ROM dumps, and no disassembly of original OS/2 system libraries are used as implementation inputs. Behaviour is inferred solely from the observable behaviour of OS/2 applications compiled with the Open Watcom toolchain and from the public specifications listed above.

---

## Completed Work

Phases 1–7 baseline are complete. Detailed descriptions of each phase, the APIs implemented, architectural decisions, and verification evidence are documented in:

- **[Developer Guide](developer_guide.md)** — Sections 1–20 cover all subsystem architectures; Appendix (Section 20) has per-phase development narratives.
- **[Reference Manual](reference_manual.md)** — Section 9 lists all 222 implemented APIs with ordinals; Section 11 covers guest memory layout and GDT.

---

## Developer Tooling (complete)

**A — Enhanced Crash Dump** — `src/loader/crash_dump.rs`. On fatal VMEXITs: captures registers, segment descriptors, stack, code bytes at EIP, and API ring history. Writes to `warpine-crash-<pid>.txt` + stderr. 13 unit tests. See [Developer Guide §19](developer_guide.md#developer-tooling) and [Reference Manual §7](reference_manual.md#crash-dumps).

**B — GDB Remote Stub** — `src/loader/gdb_stub.rs`. GDB RSP over TCP via `gdbstub 0.7`; software breakpoints, single-step (`KVM_GUESTDBG_SINGLESTEP`), Ctrl-C interrupt, full register/memory access. `--gdb <port>` CLI flag. See [Developer Guide §19](developer_guide.md#developer-tooling) and [Reference Manual §6](reference_manual.md#gdb-debugging).

**C — API Call Ring Buffer** — `src/loader/api_ring.rs`. Last 256 API calls in a bounded `VecDeque`, populated unconditionally. Included in crash dumps. 9 unit tests. See [Developer Guide §19](developer_guide.md#developer-tooling) and [Reference Manual §7](reference_manual.md#crash-dumps).

---

## Architecture & Refactoring Backlog

### Ordinal Table Canonical Build Tool
Build a tool to manage the authoritative ordinal→name table used by `api_registry.rs`, sourced exclusively from public documentation (IBM CP Programming Reference, OS/2 Warp 4 Toolkit headers, osFree project). **No real OS/2 system DLLs are used as input** (clean-room policy).

Implementation plan:
1. Extend `LxFile` to parse entry table + resident/non-resident name tables (currently only import tables are parsed) — useful for `jpos2dll.dll` and other Open Watcom-built DLLs in `samples/`
2. `src/bin/ordinals.rs` — dump complete `ordinal → name` map from an LX binary built by us; output as text or `--emit-rust` for `const` definitions
3. `--check` mode — cross-reference against warpine's `api_registry` to surface mismatches between documented and implemented ordinals
4. Maintain a hand-curated `doc/ordinals/` directory with one `.txt` per module (DOSCALLS, PMWIN, PMGPI, …) derived from public IBM documentation

### Structured API Trace — Remaining
Per-argument typed names complete (see [Developer Guide §19](developer_guide.md#debugging)).
- [ ] TUI debug overlay showing live API call stream, memory map, window hierarchy, and PM message queue

---

## Phase 5 — Multimedia and 16-bit Support

### Audio/Video (MMPM/2) — Remaining
MCI_SEEK, MCI_SET volume, MCI_NOTIFY, and MCI_RECORD stub are all complete. Remaining:

- [ ] MIDI playback device type (currently only `waveaudio` supported) — requires FluidSynth/SDL2_mixer or ALSA sequencer; deferred due to external dependency cost

### 16-bit Compatibility (NE format)
**NE execution baseline complete.** NE format parser (`src/ne/`): NeHeader, segment/relocation/entry tables, name table, 16 unit tests. Full NE loader in `src/loader/ne_exec.rs`: `load_ne()`, `apply_ne_fixups()`, `setup_guest_ne()`, `setup_and_run_ne_cli()`, `handle_ne_api_call()`, `ne_api_arg_bytes()`. GDT tiling with data tiles (DPL=2) and code tiles for CALL FAR. `ne_hello` pure-assembly sample runs `DosWrite`+`DosExit` end-to-end; integration test `test_ne_hello` passes. See [Developer Guide §20](developer_guide.md#appendix-development-phases).

Remaining:
- [ ] **Watcom CRT NE apps** — the Watcom C runtime for 16-bit OS/2 requires LDT-based selectors (TI=1) that our GDT-tile model cannot provide; would need a stub LDT or full LDT emulation
- [ ] **Mode switching** — transitions between 16-bit NE code and 32-bit flat code (e.g., 16-bit app calling a 32-bit DLL)
- [ ] **Broader 16-bit API coverage** — more DOSCALLS, VIOCALLS, KBDCALLS ordinals needed for real NE applications beyond minimal hello-world

---

## Phase 7: Application Compatibility Expansion

Goal: raise the fraction of real OS/2 applications that run correctly.

### DLL Loader Chain
**Baseline complete** — `DosLoadModule`/`DosQueryProcAddr`/`DosQueryModuleHandle` implemented; `jpos2dll.dll` loads at runtime. See [Developer Guide §20](developer_guide.md#appendix-development-phases).

Remaining:
- [ ] Call DLL initialisation routines (`DLL_INITTERM` / `eip_object`) at load and unload time — entry point address is logged; actual call requires vCPU call-injection (not yet implemented)

### DOSCALLS Long Tail
- [ ] **Structured Exception Handling** — real per-thread handler chain; `DosRaiseException`; `DosUnwindException`
- [ ] **NLS / DBCS** — `DosQueryDBCSEnv` (DBCS lead-byte table), full `DosMapCase` for non-Latin codepages

### Unicode-Internal Architecture (long-term goal)
Convert Warpine's internal string representation to UTF-8, with codepage↔UTF-8 conversion at every guest/host API boundary. Modelled on Wine's ANSI→UTF-16 approach.

**Foundation complete:** `DosSetProcessCp` now stores the active process codepage in `SharedState::active_codepage` (`AtomicU32`); `DosQueryCp` reads it atomically. `codepage.rs` provides `cp_decode`/`cp_encode` with embedded CP437/850/852 upper-half tables and `encoding_rs` for Windows/DBCS codepages. `read_guest_string` decodes all 57 call sites through the active codepage automatically.

Remaining:
- [ ] **Path strings** — `DosOpen`, `DosFindFirst/Next`, `DosDelete`, `DosMove`, etc.: decode guest path bytes → UTF-8 before VFS lookup; encode result strings back to guest CP on return
- [ ] **VIO output** — `VioWrtTTY`, `VioWrtCharStrAtt`, etc.: decode CP bytes → Unicode codepoints at write time; `VioManager` screen buffer becomes `Vec<(char, u8)>` (codepoint + attribute)
- [ ] **SDL2 text renderer** — replace static CP437 8×16 bitmap glyph table with GNU Unifont (see *GNU Unifont Integration* sections above); Phase A covers SBCS, Phase B covers DBCS 16×16 glyphs
- [ ] **PM strings** — `WinSetWindowText`, window titles, menu items, clipboard text: decode at PM API entry
- [ ] **UCONV.DLL** — implement `UniCreateUconvObject`, `UniUconvToUcs`, `UniUconvFromUcs` etc. using `encoding_rs`; unlocks OS/2 apps that do their own Unicode conversion

Sequencing (remaining): path strings → VIO output → screen buffer/font → PM strings → UCONV.DLL.

### GNU Unifont Integration — SBCS (Phase A)

Replace the hand-crafted partial CP437 font with full 256-glyph tables generated at build time from GNU Unifont, then extend to additional SBCS code pages. Unifont is GPL-2+ with a font exception (compatible with GPL-3 Warpine for static embedding).

**Source files to vendor:**
- `vendor/unifont/unifont-<ver>.hex` — Unicode BMP (8×16 for SBCS, 16×16 for CJK)
- `vendor/codepage/CP437.TXT`, `CP850.TXT`, `CP852.TXT`, `CP866.TXT` — Unicode Consortium CP→Unicode mapping tables

**A1 — `build.rs` extractor**
- [ ] For each target codepage: parse `CP<n>.TXT` (u8 → char), look up each of the 256 codepoints in Unifont, emit `src/generated/font_cp<n>.rs` with `pub static GLYPHS: [[u8; 16]; 256]`
- [ ] Skip 16×16 Unifont entries (used only for DBCS — Phase B); undefined bytes → blank `[0u8; 16]`
- [ ] Generated files committed; `build.rs` only reruns if vendor sources change

**A2 — Codepage dispatcher in `text_renderer.rs`**
- [ ] `get_glyph_sbcs(ch: u8, cp: u32) -> [u8; 16]` dispatches to the correct generated table
- [ ] CP targets for initial delivery: 437 (drop-in), 850 (Western Europe), 852 (Central Europe), 866 (Cyrillic)

**A3 — Thread `active_codepage` through to renderer**
- [ ] Add `active_codepage: u32` to `VgaTextBuffer`, populated from `SharedState::active_codepage` at snapshot time
- [ ] Pass it into `render_frame()` and down to `get_glyph_sbcs()`

**A4 — Cleanup**
- [ ] Delete `src/font8x16.rs` and the hand-crafted `match` block in `get_cp437_glyph()`
- [ ] Update `src/gui/mod.rs` exports; remove `get_cp437_glyph` from public API
- [ ] Unlock `Os2Locale::codepage` for non-437 SBCS locales (850/852/866) once Watcom CRT path is confirmed safe

---

### GNU Unifont Integration — DBCS (Phase B)

DBCS (Double-Byte Character Set) support for CP932 (Shift-JIS / Japanese), CP936 (GBK / Simplified Chinese), CP949 (EUC-KR / Korean), CP950 (Big5 / Traditional Chinese). Depends on Phase A being complete.

**OS/2 DBCS cell model** (important context):
In OS/2 VIO text mode a DBCS character occupies two consecutive screen cells: cell N holds the lead byte + attribute, cell N+1 holds the trail byte + same attribute. `VioCheckCharType` distinguishes SBCS=0, DBCS-lead=2, DBCS-trail=3. `VioManager::buffer: Vec<(u8, u8)>` already stores raw lead/trail bytes naturally — no storage format change is needed.

**B1 — Lead-byte range tables**
- [ ] `dbcs_lead_ranges(cp: u32) -> &'static [(u8, u8)]` in `locale.rs`:
  - CP932: `(0x81, 0x9F), (0xE0, 0xFC)`
  - CP936 / 949 / 950: `(0x81, 0xFE)`
  - All others: `&[]` (SBCS)

**B2 — `CellKind` annotation in `VgaTextBuffer`**
- [ ] Add `pub enum CellKind { Sbcs, DbcsLead, DbcsTail }` and `pub cell_kind: Vec<CellKind>` (parallel to `cells[]`)
- [ ] `VgaTextBuffer::snapshot()` runs `annotate_dbcs(cells, codepage)` — a single left-to-right scan using `dbcs_lead_ranges()`; marks DBCS pairs, leaves everything else as `Sbcs`; O(cols×rows) per frame

**B3 — 16×16 DBCS render path in `Sdl2TextRenderer::render_frame()`**
- [ ] Replace column `for` loop with a `while col < cols` loop:
  - `DbcsLead`: decode lead+trail → Unicode codepoint, look up 16×16 Unifont glyph (`[u8; 32]`), render into pixels at `col*8 .. col*8+16` (two SBCS column widths), advance `col += 2`
  - `DbcsTail`: `col += 1` (already drawn by its lead cell)
  - `Sbcs`: existing 8×16 path, `col += 1`
- [ ] Window stays 640 px wide (80 cols × 8 px) — no resize needed

**B4 — DBCS Unicode mapping tables (build.rs extension)**
- [ ] Vendor `SHIFTJIS.TXT`, `GBK.TXT`, `KSX1001.TXT`, `BIG5.TXT` (Unicode Consortium)
- [ ] `build.rs` emits `src/generated/dbcs_cp<n>.rs`: sorted `&[(u16, u32)]` (DBCS codeword → Unicode codepoint); runtime lookup via `binary_search_by_key`
- [ ] `decode_dbcs(lead: u8, trail: u8, cp: u32) -> char` utility function

**B5 — 16×16 glyph extraction from Unifont**
- [ ] `build.rs` extracts Unifont `.hex` entries with 64 hex chars (16×16) as `[u8; 32]`
- [ ] Emit `src/generated/font_dbcs_wide.rs`: sorted `&[(u32, [u8; 32])]` keyed by Unicode codepoint
- [ ] `get_glyph_dbcs(cp: char) -> [u8; 32]` — `binary_search_by_key` lookup; falls back to two half-width glyphs if not found
- [ ] Scope: CJK Unified Ideographs (U+4E00–U+9FFF), Hangul Syllables (U+AC00–U+D7A3), Kana blocks (~20k–30k entries total, ~600 KB–1 MB per generated file)

**B6 — `NlsGetDBCSEv` — return real lead-byte table**
- [ ] Update the current empty-table stub to return the correct `(first, last)` pairs for the active DBCS codepage, terminated by `(0, 0)` per OS/2 spec

**B7 — `VioCheckCharType` (new VIO API)**
- [ ] `VioCheckCharType(pType *u16, row u16, col u16, hvio u16) → u32`
- [ ] Scans `VioManager::buffer` from column 0 of the given row to correctly classify mid-DBCS positions (must be left-to-right, stateful — cannot annotate a single cell in isolation)
- [ ] Returns 0 (SBCS), 2 (DBCS lead), 3 (DBCS trail)
- [ ] Register in `api_registry.rs` under `VIOCALLS_BASE`

**B8 — DBCS keyboard re-encoding**
- [ ] SDL2 `SDL_TEXTINPUT` events deliver UTF-8; re-encode to active DBCS codepage before pushing to `kbd_queue`
- [ ] Requires reverse mapping (Unicode → CP codeword) — derive from the same build.rs mapping tables

**Implementation order:** B1 → B2 → B3 → B4+B5 (parallel) → B6 → B7 → B8

**Key risks:**
| Risk | Mitigation |
|---|---|
| Watcom CRT crash on non-437 locale | Keep codepage=437 for 4OS2; unlock only per-app |
| DBCS trail byte collides with SBCS range | Annotation must always scan left-to-right from column 0 |
| Unifont missing glyphs for some codepoints | Fall back to two half-width 8×16 glyphs |
| Generated file size (CP932 ~1 MB) | Acceptable; or use `include_bytes!` + runtime decode |
| `VioCheckCharType` mid-row query | Scan full row from col 0, not just the queried position |

---

### Code Page and DBCS Support
- [ ] Full `DosMapCase` for non-Latin codepages (CP852, CP866, CP932, etc.)

### PM Menu System
- [ ] **Menu template parsing** — load `MENUTEMPLATE` resource from LX binary; create `WC_MENU` window hierarchy
- [ ] **`WinLoadMenu` / `WinSetMenu`** — attach menu to frame; store `hmenu` in `OS2Window`
- [ ] **`WinSendMsg` → menu → `WM_COMMAND`** — route menu-item activations to the frame's client window procedure
- [ ] **`WM_INITMENU` / `WM_MENUSELECT`** — sent before menu is displayed / on item highlight

### Dialog System
- [ ] **Dialog template parsing** — load `DLGTEMPLATE` from LX resource; auto-create child windows; enables real `WinDlgBox` / `WinLoadDlg`
- [ ] **`WinDlgBox` / `WinLoadDlg`** — modal and modeless dialog creation; runs its own `WinGetMsg` pump
- [ ] **`WinDismissDlg`** — posts `WM_DISMISS`; unblocks `WinDlgBox`
- [ ] **`WinDefDlgProc`** — default dialog procedure: keyboard navigation, Enter/Escape handling, default button

### GPI Drawing Primitives
GpiSetColor/BackColor, GpiQueryColor/BackColor, GpiSetMix/BackMix, GpiMove, GpiLine, GpiBox, GpiCharString/At, GpiErase, GpiFullArc, GpiCreatePS/DestroyPS, GpiCreateLogFont/DeleteSetId/SetCharSet/SetCharBox, GpiQueryFontMetrics (208-byte struct), GpiQueryFonts, GpiQueryTextBox (5-point box), GpiLoadFonts stubs, GpiSetLineType/Width stubs, and full `map_color` (CLR_* + palette + direct RGB) — all complete. See `src/loader/pm_gpi.rs`.

### PM Advanced Controls
- [ ] **`WC_CONTAINER`** — Icon / Name / Text / Detail / Tree view modes; record management; sorting and filtering
- [ ] **`WC_NOTEBOOK`** — tabbed property sheet
- [ ] **Drag and drop** — `DrgDrag`, `DrgAccessDraginfo`, `DM_DRAGOVER` / `DM_DROP`
- [ ] **Custom cursors** — `WinSetPointer` via `SDL_CreateColorCursor`
- [ ] **Printing** — `DevOpenDC`, `DevCloseDC`, basic spool API stubs

### TCP/IP Socket API
- [ ] `SO32DLL.DLL` / `TCP32DLL.DLL` thunks: `socket`, `bind`, `connect`, `listen`, `accept`, `send`, `recv`, `select`, `gethostbyname`, `getservbyname`, `setsockopt`, `getsockopt`, `closesocket`
- [ ] Map to Linux BSD socket syscalls; handle OS/2 `SOCE*` error codes → errno mapping
- [ ] Enables: WebExplorer, Netscape for OS/2, FTP/IRC clients, network-licensed software

### REXX Interpreter Bridge
- [ ] Bridge `REXXAPI.DLL` exports (`RexxStart`, `RexxRegisterSubcomDll`, `RexxVariablePool`) to [Regina REXX](http://regina-rexx.sourceforge.net/)
- [ ] Unlocks: OS/2 installation programs, system tools, 4OS2 `.cmd` scripts

### Year 2038 Problem
- [ ] Audit `time_t` usage in DOSCALLS and CRT shim functions
- [ ] `DosGetDateTime` / `DosSetDateTime` use `DATETIME` struct (`USHORT` year) — not affected; verify and document
- [ ] Intercept and redirect CRT time functions imported from CLIB.DLL / CRTL.DLL / EMX.DLL to 64-bit-clean host implementations
- [ ] `FILESTATUS3` timestamps use `FDATE`/`FTIME` (7-bit year from 1980, max 2107) — not affected; verify
- [ ] Optional: `WARPINE.DLL` escape hatch — `WrpGetDateTime64` / `WrpTime64` for programs that can be recompiled

---

## Phase 8: SOM / Workplace Shell (Long-term)

The Workplace Shell (WPS) is built entirely on IBM's System Object Model (SOM). This is a multi-year effort.

### SOM Runtime Core (prerequisite for WPS)
- [ ] Object / class model: SOM class objects, method table dispatch, offset-based and name-lookup dispatch
- [ ] `SOMClassMgrObject` — global class manager; `SOMClassMgr_somFindClass()`, class registration, DLL-based class loading
- [ ] IDL metadata: parse or reconstruct method offsets and class hierarchy at runtime
- [ ] Binary ABI compatibility with IBM SOM 2.1 so WPS extensions (XWorkplace, Object Desktop) load without recompilation

### WPS Object Hierarchy (requires SOM runtime)
- [ ] `WPObject` — root: `wpInitData`, `wpSaveState`, `wpRestoreState`, `wpQueryTitle`, `wpOpen`, `wpDragOver`, `wpDrop`
- [ ] `WPFileSystem` — `wpQueryFilename`, `wpQueryAttr`
- [ ] `WPFolder` — Icon / Detail / Tree via `WC_CONTAINER`; `wpPopulate`
- [ ] `WPDesktop` — singleton root desktop; persists object positions in OS2.INI
- [ ] `WPProgram` — launches via `DosExecPgm`; `WPDataFile` — `.TYPE` EA for app association
- [ ] Persistence via `PrfWriteProfileData` / `PrfQueryProfileData` (OS2.INI / OS2SYS.INI)
- [ ] Settings notebook: `WinLoadDlg` + `WC_NOTEBOOK` + per-class property pages
- [ ] Drag and drop protocol: `wpDragOver` / `wpDrop` / `wpCopyObject` / `wpMoveObject`

---

## Phase 9: XE — 64-bit OS/2-lineage Platform (far future / vision)

Goal: define and implement a new 64-bit executable format and API set as a natural evolution of the OS/2 lineage. XE apps run natively on Warpine alongside existing 32-bit LX apps. This transforms Warpine from a pure compatibility layer into a dual-ABI OS personality for x86-64 Linux.

### XE Executable Format

A new format following the MZ → LX precedent: MZ stub with `"XE"` signature at `e_lfanew`, fields widened to 64 bits where addresses appear.

- [ ] Define format spec: XE header (signature, cpu_type, object_count, entry_rip: u64, entry_rsp: u64), 64-bit object table (base_address: u64, size: u64, flags: u32), 64-bit fixup records, import/export tables (ordinal → u64 offset)
- [ ] `src/xe/` parser module mirroring `src/lx/` structure
- [ ] `detect_format()` in `main.rs` recognises `"XE"` signature
- [ ] `Loader::load_xe()` / `run_xe()` path in `lx_loader.rs`

### KVM Long Mode Execution

- [ ] vCPU initialisation in long mode (set `EFER.LME`, enable 4-level paging, 64-bit GDT — segments mostly flat, FS/GS for TIB/PIB)
- [ ] 64-bit `SharedState` TIB/PIB layout at well-known addresses
- [ ] INT 3 thunk mechanism unchanged — works identically in long mode; thunk handler reads args from `rdi/rsi/rdx/rcx/r8/r9` (System V AMD64 ABI) instead of the stack

### Calling Convention

**System V AMD64 ABI** (`rdi, rsi, rdx, rcx, r8, r9`, caller-saves `rax/rcx/rdx/rsi/rdi/r8–r11`, return in `rax`). Rationale: universal toolchain support (Rust, Clang, GCC) with no custom patches needed; Warpine's Rust host code already uses this ABI natively.

- [ ] Document `_XE64` calling convention in `doc/`
- [ ] Update `api_dispatch.rs` to extract arguments from 64-bit registers for XE calls

### 64-bit API Set (`DOSCALLS64`, `PMWIN64`, …)

Clean-break 64-bit API — pointer-sized arguments, 64-bit handles, `size_t` buffer lengths. New ordinal namespace separate from 32-bit DOSCALLS.

- [ ] Core I/O: `DosWrite64`, `DosRead64`, `DosOpen64`, `DosClose64`, `DosExit64`
- [ ] Memory: `DosAllocMem64` (full 64-bit address space), `DosFreeMem64`
- [ ] Threads: `DosCreateThread64`, `DosWaitThread64`
- [ ] Synchronisation: `DosCreateEventSem64`, `DosCreateMutexSem64`
- [ ] PM: `WinInitialize64`, `WinCreateStdWindow64`, `WinGetMsg64`, `WinDispatchMsg64` — same message model, 64-bit pointers
- [ ] `UCONV64.DLL` — Unicode conversion using UTF-8 natively (complements Unicode-internal architecture goal)

### Rust/Clang Toolchain Support

- [ ] `warpine-xe` Rust crate: safe bindings to the 64-bit API set; `#![no_std]` compatible
- [ ] Custom Rust target spec `x86_64-warpine-xe` (bare-metal, System V ABI, XE binary output via custom linker script)
- [ ] Sample XE app written in Rust: `samples/xe_hello/` — `DosWrite64` to stdout, `DosExit64`
- [ ] Sample XE app written in C (Clang `x86_64-unknown-none`): validates the ABI from C

### Dual-ABI Coexistence

- [ ] 32-bit LX apps and 64-bit XE apps run side-by-side under the same Warpine instance
- [ ] `DosExecPgm` detects XE format and spawns a 64-bit vCPU thread
- [ ] Shared `SharedState` managers (memory, handles, semaphores) serve both 32-bit and 64-bit guests
