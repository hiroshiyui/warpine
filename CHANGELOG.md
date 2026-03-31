# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **WinMessageBox** — real modal dialog via SDL2 `show_message_box`; vCPU thread blocks on a `SyncSender` until the user dismisses; all 9 MB_* button sets (OK/Cancel/Retry/Abort/Ignore/Yes/No/Enter) and MB_ICON* flags mapped; headless renderer auto-replies MBID_OK; 20 new MB_*/MBID_* constants; `GUIMessage::ShowMessageBox` channel variant; 7 new unit tests (`pm_win.rs`, `sdl2_renderer.rs`, `headless.rs`)
- **WinEnableWindow / WinIsWindowEnabled / WinQueryWindowPos** — `WinEnableWindow` (ord 735) toggles `WS_DISABLED` on `OS2Window::style` and posts `WM_ENABLE`; `WinIsWindowEnabled` (ord 773) reads the flag; `WinQueryWindowPos` (ord 837) fills a 28-byte SWP struct with x/y/cx/cy; `WinEnableWindowUpdate` (ord 736) stub; `WM_ENABLE = 0x0002` added to `constants.rs`
- **GPI drawing primitives** — `GpiSetBackColor` (518), `GpiQueryColor`/`GpiQueryBackColor` (520/521), `GpiSetMix`/`GpiSetBackMix` (509/503), `GpiQueryCurrentPosition` (416), `GpiSetCharSet` (481), `GpiSetCharBox` (482), `GpiCreateLogFont` (381), `GpiDeleteSetId` (385), `GpiCharString` (358), `GpiQueryFontMetrics` (464, writes full 208-byte FONTMETRICS struct), `GpiQueryFonts` (459), `GpiQueryTextBox` (476, 5-point bounding box), `GpiFullArc` (392), `GpiSetArcParams` (353), `GpiSetLineType`/`GpiSetLineWidth` (527/529/530), `GpiLoadFonts`/`GpiLoadPublicFonts` (399/400) stubs; `write_font_metrics()` helper; mock font constants (`MOCK_CHAR_W=8`, `MOCK_CHAR_H=16`, `MOCK_ASCENDER=12`, `MOCK_DESCENDER=4`); 5 new unit tests (`pm_gpi.rs`)
- **PM built-in controls** — `WinCreateWindow` (ord 709): creates any window/control from class atom or name string; applies `WS_VISIBLE`, id, style; posts initial `WM_PAINT`; `WinSubclassWindow` (ord 895): replaces window procedure and returns old one; `dispatch_builtin_control`: renders WC_BUTTON, WC_STATIC, WC_SCROLLBAR, WC_ENTRYFIELD, WC_MLE, WC_LISTBOX via `GUIMessage`
- **PM window management** — `WinSetWindowText` (877) updates `OS2Window::text`; `WinQueryWindowText` (841) copies text to guest buffer; `WinInvalidateRect` (765) posts `WM_PAINT`; `WinUpdateWindow` (892) sends `PresentBuffer`; `WinQueryWindowRect` (840) returns `(0,0,cx,cy)`; `WinSetWindowPos` (875) handles `SWP_MOVE`, `SWP_SIZE`, `SWP_SHOW`, `SWP_HIDE` for any hwnd; `WinQueryWindowPos` (837) fills SWP struct; `WinEnableWindow`/`WinIsWindowEnabled`/`WinEnableWindowUpdate` (735/773/736); `WinMessageBox` (789)
- **Recursive DLL import loading** — `load_dll_impl` iterates `lx_file.imported_modules`, skips the 10 built-in emulated modules (`BUILTIN_MODULES` constant), recursively loads any user-DLL dependency before applying fixups; cycle detection via `HashSet<String>` guard; 3 new unit tests (`lx_loader.rs`)
- **Reference-counted `DosFreeModule`** — `LoadedDll::ref_count` starts at 1; second `DosLoadModule` call increments it; `DosFreeModule` decrements and frees guest memory pages at zero; `DllManager::decrement_refcount` / `increment_refcount`; 4 new unit tests (`managers.rs`)
- **MMPM/2 audio expansion** — `MCI_SEEK`: seeks to millisecond position, updates `current_position` byte offset; `MCI_SET` with `MCI_SET_AUDIO | MCI_SET_VOLUME`: volume 0–100 applied via `SDL_MixAudioFormat` at play time; `mciSendString` "set … audio volume to N" also supported; `MCI_NOTIFY` flag: non-blocking play spawns watcher thread that posts `MM_MCINOTIFY` (0x0502) with `MCI_NOTIFY_SUCCESSFUL` to `hwndCallback` when queue drains; `mci_stop()` / device drop cancels with `MCI_NOTIFY_SUPERSEDED`; `MCI_RECORD` stub returns `MCIERR_UNSUPPORTED_FUNCTION`
- **DOS/VIO API expansion** — `DosKillThread` (111): removes JoinHandle from thread table; `DosScanEnv` (227): scans process environment block; `DosSetPriority` (236): no-op stub; `DosSetExtLIBPATH` (873) / `DosQueryExtLIBPATH` (874): BEGINLIBPATH/ENDLIBPATH stored in `SharedState`; `VioSetMode` (22): real row/col resize preserving buffer content and clamping cursor; `VioManager::resize()` helper; `VioManager` gains `Default` impl
- **Pre-commit quality gate** — `githooks/pre-commit` runs `cargo test` + `cargo clippy -- -D warnings`; blocks commit if any test fails or warning exists; `git config core.hooksPath githooks` activates it
- **map_color full CLR_*** — `Loader::map_color` now handles all OS/2 CLR_* constants: negative special values (CLR_BLACK=-1, CLR_WHITE=-2, CLR_DEFAULT=-3, CLR_BACKGROUND=0) + palette indices 1–15 + direct `0x00RRGGBB` pass-through for values ≥ 16; 3 new unit tests
- **PresentationSpace expansion** — `back_color`, `mix_mode`, `back_mix`, `char_set`, `char_box` fields added; `create_ps()` initialises defaults (white background, FM_OVERPAINT mix); `current_pos` tracking for GPI drawing state

### Fixed
- **Far16 LSS+JMP thunk bypass** — added variant handler for `LSS ESP, [EBP-n]` prefix before `JMP FAR` (66 EA); fixes Up/Down arrow crashes in 4OS2
- **DosWaitChild 5-arg signature** — stack read used wrong offset for 4th/5th args; caused 4OS2 prompt not returning after child process exit
- **4OS2 child-process prompt regression** — relay thread and DosWaitChild interaction corrected

### Added
- **NE (16-bit OS/2 1.x) Execution** — full NE loader and 16-bit execution path: NE segments mapped into GDT-tiled guest memory; CALL FAR import fixups patched to code tile `NE_THUNK_CODE_SELECTOR` (0x87B0); API calls dispatched via `handle_ne_api_call()` with Pascal calling convention; `ne_hello` (pure assembly, no Watcom CRT) runs `DosWrite`+`DosExit` correctly; integration test added (`ne_exec.rs`)
- **Modifier key suppression** — pure modifier keys (LShift, RShift, Ctrl, Alt, CapsLock) no longer enqueue `KbdKeyInfo` events; fixes 4OS2 printing `*`/`6` on Shift press (`text_renderer.rs`)
- **GDB Remote Stub** — `--gdb <port>` enables GDB RSP over TCP; software breakpoints, single-step, Ctrl-C interrupt, full register/memory access (`gdb_stub.rs`)
- **Crash Dump Facility** — structured crash reports on fatal VMEXITs: registers, segment descriptors, stack, code bytes at EIP, written to `warpine-crash-<pid>.txt` and stderr (`crash_dump.rs`)
- **API Call Ring Buffer** — last 256 API calls captured unconditionally for crash post-mortem (`api_ring.rs`)
- **GDT 16-bit code/data aliases** — GDT[4] (selector 0x20) 16-bit data alias, GDT[5] (selector 0x28) 16-bit code alias for Far16 JMP FAR thunks; tile base shifted to GDT[6]
- **Structured API trace** — per-argument typed names and strace-like formatter (`api_trace.rs`); `WARPINE_TRACE=strace|json` output modes
- **DLL loader chain** — `DosLoadModule`/`DosQueryProcAddr`/`DosQueryModuleHandle` implemented; `jpos2dll.dll` loads at runtime with all 7 exports resolved (`lx_loader.rs`, `managers.rs`)
- **GDT tiling** — 4096 tiled 16-bit data descriptors (GDT[6..4102]) enabling 16:16 addressing; `DosFlatToSel`/`DosSelToFlat` use proper tile arithmetic
- **SDL2 VGA Text Renderer** — 640×400 text-mode window with CP437 8×16 font, CGA 16-colour palette, blinking cursor; CLI apps default to SDL2 text window (`text_renderer.rs`)
- **PM Renderer Abstraction** — `PmRenderer` trait with `Sdl2Renderer` and `HeadlessRenderer` backends
- **MMPM/2 Audio** — `DosBeep` sine-wave tones; `mciSendCommand`/`mciSendString` for waveaudio via SDL2 audio queue (`mmpm.rs`)
- **HPFS-compatible VFS** — `VfsBackend` trait, `HostDirBackend` with case-insensitive lookup, extended attributes, file locking, sandbox isolation (`vfs.rs`, `vfs_hostdir.rs`)
- **NE Format Parser** — parser for OS/2 1.x 16-bit (NE) executables (`src/ne/`)
- **API thunk auto-registration** — `api_registry.rs` static sorted table (124 entries) with O(log n) lookup; `--compat` compatibility report
- **NLS** — `DosQueryCtryInfo`, `DosQueryCp`, `DosMapCase`, `DosGetDateTime`
- **Text-mode console** — `VioManager` with screen buffer, cursor, ANSI output; VIOCALLS and KBDCALLS subsystems
- **4OS2 compatibility** — 4OS2 command shell runs interactively; `dir`, `set`, `ver`, `md`, `rd`, `copy`, `move`, `del`, `attrib`, `tree` all work
- 22 sample OS/2 applications in `samples/`
- 281 unit tests, 9 integration tests

### Fixed
- **Far16 thunk bypass** — `#GP` on `66 EA` (Watcom `__Far16` JMP FAR) now fully bypassed in VMEXIT handler: scans stack for self-referential PUSH EBP frame, restores 32-bit caller state and returns EAX=0, avoiding all 16-bit execution (fixes 4OS2/JPOS2DLL crashes 606162, 623645, 630555)
- **SDL2 text-mode CPU idle** — replaced `poll_iter()` busy-loop with `wait_event_timeout(8)` so idle processes yield the CPU instead of pinning a full core
- Added 2048 code-tile GDT entries (GDT[4102..6149], type 0x9B) so `DosFlatToSel`/`DosSelToFlat` round-trip correctly for code-tile selectors
- Detach relay thread in `DosWaitChild` instead of joining (deadlock fix)
- Rate-limit MMIO exit handlers to prevent 100% CPU spin
- Read IDT exception frame from `SS.base+SP` when SS is 16-bit
- Relay child stdout to parent `VioManager` for `EXEC_ASYNC` `DosExecPgm`
- Detect headless mode via `is_terminal()` instead of env var
- `DosQueryAppType` uses `DriveManager` VFS for path resolution
