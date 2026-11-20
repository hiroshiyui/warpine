# Architecture Decision Records

Why Warpine is built the way it is. Each record states one decision, its context and its consequences, in the format of Michael Nygard's "Documenting Architecture Decisions" (ADR-0001).

To add one: copy the layout of an existing record, take the next number, and add it to this list. A changed decision gets a new record that supersedes the old one; the old record is kept.

| ADR | Decision | Status | Date |
|---|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted | 2026-11-18 |
| [0002](0002-gpl-3-only.md) | License Warpine under GPL-3.0-only | Superseded by 0074 | 2026-03-12 |
| [0003](0003-kvm-native-execution.md) | Run OS/2 code natively on KVM | Accepted | 2026-03-12 |
| [0004](0004-near-clean-room-policy.md) | Implement OS/2 from public documentation only (near-clean-room) | Accepted | 2026-03-18 |
| [0005](0005-api-thunks-and-module-slots.md) | Dispatch API calls through INT 3 thunks with one ordinal slot per built-in module | Accepted | 2026-03-14 |
| [0006](0006-ordinal-sources-of-truth.md) | Use the Open Watcom import library and toolkit headers as the sources of ordinals and constants | Accepted | 2026-03-14 |
| [0007](0007-threads-on-vcpus.md) | Run each OS/2 thread on its own host thread and KVM vCPU | Accepted | 2026-03-12 |
| [0008](0008-unicode-inside.md) | Keep text as Unicode inside Warpine and convert at the codepage boundary | Accepted | 2026-03-17 |
| [0009](0009-drives-on-host-directories.md) | Map OS/2 drives to host directories, with C: in the user's data directory | Accepted | 2026-03-14 |
| [0010](0010-sdl3-safe-only.md) | Use SDL3 through the safe sdl3-rs API only | Accepted | 2026-11-16 |
| [0011](0011-own-desktop-compositor.md) | Draw the PM desktop in one host window with Warpine's own compositor | Accepted | 2026-04-02 |
| [0012](0012-fonts-plex-and-unifont.md) | Draw PM text with IBM Plex and text mode with GNU Unifont | Accepted | 2026-11-15 |
| [0013](0013-vio-in-pm-window.md) | Run text-mode (VIO) programs in a PM window by default | Accepted | 2026-11-17 |
| [0014](0014-builtin-cmd.md) | Provide CMD.EXE as a built-in Rust shell | Accepted | 2026-04-01 |
| [0015](0015-rust-guest-toolchain.md) | Build Rust guest programs with Warpine's own ELF-to-LX linker | Accepted | 2026-04-01 |
| [0016](0016-locale-from-host.md) | Take the country and codepage from the host locale | Accepted | 2026-11-17 |
| [0017](0017-profile-text-format.md) | Store OS/2 profiles (INI files) in Warpine's own text format | Accepted | 2026-11-18 |
| [0018](0018-testing-oracle-and-goldens.md) | Test against Open Watcom sample programs and golden screen signatures | Accepted | 2026-11-15 |
| [0019](0019-quality-gates.md) | Gate every commit on tests and lint-free code with a pinned toolchain | Accepted | 2026-03-31 |
| [0020](0020-one-process-per-host-process.md) | Run each OS/2 process as its own host process | Accepted | 2026-03-13 |
| [0021](0021-generated-architecture-diagram.md) | Generate the architecture diagram from a checked layer map | Accepted | 2026-11-18 |
| [0022](0022-named-pipes-and-inherited-handles.md) | Carry named pipes and inherited handles over host sockets and the host exec | Accepted | 2026-11-18 |
| [0023](0023-vcpu-kick.md) | Kick vCPUs out of guest code with `immediate_exit` and a real-time signal | Accepted | 2026-11-18 |
| [0024](0024-regions-and-clipping.md) | Represent regions as y-bands of x spans and clip by saving and restoring pels | Accepted | 2026-11-18 |
| [0025](0025-owner-draw-and-combo-lists.md) | Run owner-draw calls before the API returns, and drop combination-box lists inside the parent | Accepted | 2026-11-18 |
| [0026](0026-call-chains-and-process-hooks.md) | Run hooks and broadcast sends as call chains that may re-run the API, and scope system hooks to the process | Accepted | 2026-11-18 |
| [0027](0027-gpi-transforms-per-primitive.md) | Apply GPI transforms to each primitive when it is called, with an identity fast path | Accepted | 2026-11-18 |
| [0028](0028-process-nesting-limit.md) | Limit how deeply Warpine processes start one another and how many children each keeps | Accepted | 2026-11-18 |
| [0029](0029-standard-dialogs-and-print-modules.md) | Build the standard file and font dialogs in memory, and give PMCTLS and PMSPL module slots | Accepted | 2026-11-18 |
| [0030](0030-process-local-shell-services.md) | Keep the switch list, atom tables and DDE within one Warpine process | Superseded by ADR-0056 | 2026-11-18 |
| [0031](0031-capped-test-runs.md) | Run the commit checks inside a capped systemd scope | Accepted | 2026-11-18 |
| [0032](0032-session-thread-limit.md) | Enforce CONFIG.SYS THREADS= across a session with per-process records in a private directory | Accepted | 2026-11-18 |
| [0033](0033-generated-compatibility-chart.md) | Generate the API compatibility chart from Warpine's source and a dated, cited API table | Accepted | 2026-11-18 |
| [0034](0034-ending-a-process-from-outside.md) | End a process from outside with KILLPROC, its exit list and a grace period | Accepted | 2026-11-18 |
| [0035](0035-interrupting-blocking-calls.md) | Interrupt blocking calls through a per-thread call marker and one semaphore wake-up path | Accepted | 2026-11-18 |
| [0036](0036-session-unique-process-ids.md) | Give every process a session-unique id chosen by its parent; follow the documented entry ABI | Accepted | 2026-11-18 |
| [0037](0037-pipe-semaphores-and-session-directory-hygiene.md) | Post pipe semaphores from per-end watcher threads, and sweep stale session directories by liveness | Accepted | 2026-11-18 |
| [0038](0038-client-origin-and-dialog-notifications.md) | Draw client windows at their place in the frame surface; send dialog notifications through call chains | Accepted | 2026-11-18 |
| [0039](0039-session-clock-and-64-bit-time.md) | Keep a per-process session clock for DosSetDateTime; add 64-bit time calls in WARPINE.DLL | Accepted | 2026-11-18 |
| [0040](0040-reserved-tiles-for-info-segments.md) | Keep the 16-bit information segments in reserved tiles, refreshed by a host thread | Accepted | 2026-11-18 |
| [0041](0041-socket-select-fd-set.md) | Read select()'s fd_set as OS/2 TCP/IP's count-and-array layout; wait with poll(2) | Accepted | 2026-11-18 |
| [0042](0042-embedded-dialogs.md) | Embed a dialog whose parent is a window into that window | Accepted | 2026-11-18 |
| [0043](0043-container-record-memory.md) | Keep container records in guest memory, carved from per-container arenas | Accepted | 2026-11-18 |
| [0044](0044-container-sort-call-chains.md) | Run a container sort as a resumable merge sort, one comparison per call chain | Accepted | 2026-11-18 |
| [0045](0045-drag-and-drop-within-a-process.md) | Drag and drop within a process: a GUI-fed drag channel and DM_* messages as call chains | Accepted (extended to other programs by ADR-0057) | 2026-11-18 |
| [0046](0046-container-icon-view-filter-and-editing.md) | Container icon view, filtering and direct editing: one filter chain, edits written back through the owner's buffer | Accepted | 2026-11-18 |
| [0047](0047-light-hinting-with-skrifa.md) | Light hinting of Latin text with skrifa, and a contrast curve | Accepted | 2026-11-18 |
| [0048](0048-builtin-window-procedure-and-tier-1-pm.md) | A guest-callable procedure for built-in windows; standard frames run the default procedure; child presentation spaces in the frame surface | Accepted | 2026-11-18 |
| [0049](0049-shared-memory-across-processes.md) | Shared memory across processes: a sealed memfd arena at the top of guest memory, a table in the session directory | Accepted | 2026-11-19 |
| [0050](0050-shared-semaphores-across-processes.md) | Shared semaphores across processes: a host-only memfd of slots, an OFD lock and a futex | Accepted | 2026-11-19 |
| [0051](0051-pointer-input-routed-when-read.md) | Pointer input routed when it is read: clicks hit-tested and activating at the head of the input queue | Accepted (the window found extended by ADR-0058) | 2026-11-19 |
| [0052](0052-session-desktop-server.md) | One desktop per session: a desktop server process, with Warpine processes as its clients | Accepted (phases A and B: ADR-0053–0055; phase C: ADR-0056, ADR-0057) | 2026-11-19 |
| [0053](0053-calls-on-other-programs-windows.md) | Calls on other programs' windows go through the desktop server and are answered on the owner's link thread | Accepted (`WM_MINMAXFRAME` amended by ADR-0055) | 2026-11-19 |
| [0054](0054-messages-sent-to-other-programs-windows.md) | Messages sent to another program's window run on the window's thread when it reads messages | Accepted (send hooks amended by ADR-0055) | 2026-11-19 |
| [0055](0055-programs-see-each-others-windows.md) | Window lists, broadcasts, state changes and send hooks reach other programs | Accepted | 2026-11-19 |
| [0056](0056-session-shell-services.md) | The desktop server keeps the session's switch list and system atom table; DDE crosses programs | Accepted | 2026-11-19 |
| [0057](0057-drag-and-drop-between-programs.md) | Drag and drop crosses programs: targets found through the server, DM_* messages as waited calls, drag blocks in the shared arena | Accepted | 2026-11-19 |
| [0058](0058-deepest-window-hit-test-update-regions-must-complete.md) | Pointer input goes to the deepest window and asks WM_HITTEST when read; update areas are regions; must-complete sections hold signals | Accepted | 2026-11-19 |
| [0059](0059-som-runtime-kernel.md) | A built-in SOM.DLL keeps classes host-side and objects in guest memory; executables load and initialize their DLLs | Accepted (stage 1); partly superseded by 0060 and 0063 | 2026-11-19 |
| [0060](0060-som-metaclasses-multiple-inheritance.md) | SOM tokens name their introducing class; derived metaclasses, multiple inheritance and a runtime heap | Accepted | 2026-11-19 |
| [0061](0061-som-cpp-bindings.md) | SOM's C++ bindings create objects through the runtime; somDefaultInit and somDestruct | Accepted | 2026-11-19 |
| [0062](0062-som-class-loading.md) | SOM loads a class's DLL by name through replaceable runtime variables; DLLs are searched by plain module name only | Accepted | 2026-11-19 |
| [0063](0063-module-lifecycle.md) | Module lifecycle: failed startup routines stop the program, OS/2 paths for DosLoadModule, zeroed memory, SOM class unloading | Accepted | 2026-11-19 |
| [0064](0064-notebook-turns-page-dialogs-and-tab-across-pages.md) | Notebook turns ask the owner first; the notebook loads page dialogs and owns its buttons; Tab crosses into embedded dialogs | Accepted | 2026-11-19 |
| [0065](0065-container-details-panes-owner-drawing-title-edits.md) | Container details panes, horizontal scrolling, owner drawing through owner calls, title and heading edits; hidden built-in windows do not paint | Accepted | 2026-11-19 |
| [0066](0066-guest-isolation-and-supply-chain-hardening.md) | Guests cannot touch the host environment or host Unix sockets; downloads and toolchains are pinned and verified; a cargo-deny policy guards dependencies | Accepted | 2026-11-19 |
| [0067](0067-context-menus-wheel-and-container-trees-grids.md) | WM_CONTEXTMENU and the mouse wheel as routed pointer input; container tree lines, tree moves, grids, erasing and stacking | Accepted | 2026-11-19 |
| [0068](0068-file-drags-host-drops-and-drag-operations.md) | File drags, host files dropped on windows, the drag's operation keys and WM_ENDDRAG | Accepted | 2026-11-20 |
| [0069](0069-embedded-dialog-chrome-and-tab.md) | An embedded dialog draws its own chrome, and Tab crosses between it and the client's controls | Accepted | 2026-11-20 |
| [0070](0070-som-ordinals-from-osfree.md) | SOM.DLL's ordinals 1–47 from osFree's export list; Warpine's own exports at 512–1023 (moved by ADR-0071) | Accepted | 2026-11-20 |
| [0071](0071-a-wider-som-ordinal-range.md) | The SOM module is 4096 ordinals wide; Warpine's own SOM exports at 3072–4095 | Accepted | 2026-11-20 |
| [0072](0072-som-root-class-layouts-from-the-toolkit.md) | The SOM root classes follow IBM's layouts (ClassData release order, somMethodTab header, somParentMtabStruct), read from the OS/2 Warp 4 Toolkit | Accepted | 2026-11-20 |
| [0073](0073-som-build-class-from-static-class-info.md) | somBuildClass builds classes from IBM's static class information (somStaticClassInfo) | Accepted | 2026-11-20 |
| [0074](0074-gpl-3-or-later.md) | License Warpine under GPL-3.0-or-later | Accepted | 2026-11-20 |
| [0075](0075-remove-4os2.md) | Remove 4OS2; the built-in CMD.EXE is Warpine's shell; shell redirection stays inside the drives | Accepted | 2026-11-20 |
| [0076](0076-guest-crates-stay-gpl-and-the-binary-carries-its-font-notices.md) | The guest crates stay GPL-3.0-or-later; `warpine --licenses` carries the built-in fonts' notices | Accepted | 2026-11-20 |
| [0077](0077-cmd-exe-foundations.md) | The built-in CMD.EXE's foundations: OS/2 syntax, return codes, programs reading pipes, inherited current directories | Accepted | 2026-11-20 |
| [0078](0078-cmd-exe-batch-control.md) | The built-in CMD.EXE's batch control: ECHO, `%0`–`%9`, SHIFT, IF, FOR, GOTO, CALL, SETLOCAL, PAUSE | Accepted | 2026-11-20 |
| [0079](0079-cmd-exe-file-commands.md) | The built-in CMD.EXE's COPY, REN and MOVE; a safe `DosEditName`, `DosCopy` never onto itself, names unique without regard to case | Accepted | 2026-11-20 |
