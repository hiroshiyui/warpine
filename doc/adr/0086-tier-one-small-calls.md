# ADR-0086: Tier 1: small, fundamental PMWIN, DOSCALLS and PMWP calls

- **Status:** Accepted
- **Date:** 2026-11-20
- **Sources:**
  - Open Watcom's `h/os2/pmwin.h`, `pmwp.h`, `bsedos.h`, `bseerr.h`, `pmsei.h`, `pmtypes.h`;
  - the OS/2 Warp 4.5 Toolkit's `pmwin.h` and `bsedos.h` at the commit ADR-0072 pins (prototypes Open Watcom lacks: `WinGetDlgMsg`, `WinEnumClipbrdFmts`, `DosVerifyPidTid`, `DosRawReadNPipe`, `DosRawWriteNPipe`; the `ERRINFO` and `POINTERINFO` layouts);
  - `doc/os2_ordinals.md` (all the ordinals, PMWP's included);
  - the code:
    - `src/loader/pm_winutil.rs`, `src/loader/dos_misc.rs`;
    - changes to `pm_win.rs`, `doscalls.rs`, `ipc.rs`, `stubs.rs`, `managers.rs`, `lx_loader.rs`, `pm_hook.rs`, `pm_pointer.rs`, `pm_error.rs`, `pm_types.rs`, `pm_window.rs`, `pm_minmax.rs`;
  - the samples: `samples/pm_winutil_test`, `samples/dos_misc_test`.

## Context

A survey of the OS/2 2.0 to 4.0 exports Warpine lacked (the Tier 1 list) found small calls that ordinary programs use: string helpers, dialog helpers, painting helpers, waits, pointers made from bitmaps, error information, libraries, the high-resolution timer, protected file handles, queue peeking, and saving a window's position. Each is small; together they close common gaps.

## Decision

1. **PMWIN** (`pm_winutil.rs`, reached from `handle_pmwin_call`'s default arm; 42 ordinals):
   - **Strings:**
     - `WinUpper` and `WinUpperChar` (the codepage's case mapping, DBCS pairs included);
     - `WinNextChar` / `WinPrevChar` (DBCS aware);
     - `WinCompareStrings` (DosQueryCollate's collating sequence);
     - `WinCpTranslateChar` / `WinCpTranslateString`;
     - `WinGetCurrentTime` (the clock of the messages' time stamps).
   - **Dialogs:**
     - `WinCreateDlg` (a template in memory);
     - `WinMapDlgPoints` (the system font's dialog units, both ways);
     - `WinQueryDlgItemTextLength`;
     - `WinGetDlgMsg` (a dialog's own loop; FALSE once it is dismissed).
   - **Painting and windows:**
     - `WinGetClipPS` (as `WinGetPS`; the `PSF_*` flags are checked, otherwise ignored);
     - `WinGetScreenPS`, `WinInvertRect`;
     - `WinMakeRect` / `WinMakePoints` (no-ops in 32-bit PM);
     - `WinIsWindowShowing` (visible, not minimized, with a part inside each parent).
   - **Clipboard:**
     - `WinSetClipbrdOwner` / `WinQueryClipbrdOwner`: emptying posts `WM_DESTROYCLIPBOARD` to the owner and ends the ownership;
     - `WinSetClipbrdViewer` / `WinQueryClipbrdViewer`: closing after a change posts `WM_DRAWCLIPBOARD` to the viewer;
     - `WinEnumClipbrdFmts` (ascending), and `WinQueryClipbrdFmtInfo` (the `CFI_*` flags given to `WinSetClipbrdData`).
   - **Waits:** `WinWaitMuxWaitSem` (as `DosWaitMuxWaitSem`, like `WinWaitEventSem`), `WinCheckInput`, `WinIsThreadActive`.
   - **Pointers:**
     - `WinCreatePointer` and `WinCreatePointerIndirect`: from a two-tone mask bitmap twice the pointer's height (AND above, XOR below, white pels as 1 bits) and an optional colour bitmap;
     - `WinQueryPointerInfo`: what a pointer was made from; for other pointers fPointer and the hot spot;
     - `WinDrawPointer`: the image at its lower-left corner; `DP_HALFTONED`, `DP_INVERTED`; a system pointer draws nothing.
   - **Errors:**
     - `WinGetErrorInfo` takes the last error, as `WinGetLastError` does, into an `ERRINFO` block in new memory with one message;
     - `WinFreeErrorInfo` frees only such blocks;
     - `WinSetErrorInfo` sets the last error.
   - **Libraries:**
     - `WinLoadLibrary`: `DosLoadModule`, its `_DLL_InitTerm` chain ending with the handle (`ChainThen::LoadLibrary`);
     - `WinLoadProcedure`;
     - `WinDeleteLibrary`: `DosFreeModule`; its termination call ends with TRUE (`ChainThen::FreeLibrary`);
     - `WinDeleteProcedure`.
   - **User types:** `WinRegisterUserMsg` and `WinRegisterUserDatatype` are recorded (bounded), not yet used to carry pointers between programs.
   - **Positions:** `WinGetMaxPosition`, and `WinGetMinPosition` (the given point, else the remembered or first free icon slot).
2. **DOSCALLS / QUECALLS** (`dos_misc.rs`):
   - **The timer:** `DosTmrQueryFreq` / `DosTmrQueryTime` (1,193,182 Hz).
   - **`DosQueryModFromEIP`:** the loader now records the program's objects and each DLL's object sizes.
   - **Classification:** `DosErrClass` (a table built from the `ERRCLASS_*` meanings), `DosQueryProcType` (32-bit), `DosVerifyPidTid` (this process's threads).
   - **Locks:** `DosCancelLockRequest` (no lock request ever waits in the background).
   - **The `DosProtect*` calls:**
     - Open, Close, Read, Write, SetFilePtr, SetFileSize, Query/SetFHState, Query/SetFileInfo, EnumAttribute;
     - each is the plain call, once the FHLOCK matches the handle's. An FHLOCK is derived from the handle, so no table is kept.
   - **`DosPeekQueue`:** queue elements now have element codes; `DosReadQueue` reads the element a code names.
3. **PMWP** becomes a built-in module (base 28672; `STUB_AREA_SIZE` 29696) with `WinStoreWindowPos` / `WinRestoreWindowPos`. These keep Warpine's own record (`WRPWPOS1`: position, size, state, normal position, icon position) in the user profile; OS/2's layout is not public. Restoring a minimized or maximized frame sets its normal position first, then the state.
4. **Fixes found on the way:**
   - `WinFillRect` resolves its colour against its presentation space (RGB after `GpiCreateLogColorTable(LCOLF_RGB)`), as `WinDrawText` does; it used the global colour mapping, so RGB 0 drew the background colour.
   - `DosReadQueue`'s wait flag was inverted: `DCWW_WAIT` (0) now waits, `DCWW_NOWAIT` (1) returns `ERROR_QUE_EMPTY`.
   - Every built-in module (`module_base`), REXX and REXXAPI included, counts as loaded: `DosQueryModuleHandle` gives it a handle on first use, and `DosLoadModule` no longer keeps its own list.
   - `DosQueryProcAddr` by ordinal on a registry-dispatched built-in module finds only ordinals Warpine implements; any ordinal was resolved before. Open Watcom's C runtime probes DOSCALLS for its large-file calls (`DosOpenL` …) once `DosQueryModuleHandle("DOSCALLS")` answers; it now falls back to `DosOpen`, as on a system without them. Sub-dispatched modules (PMWIN, PMGPI, VIO / KBD / MOU) resolve every ordinal, as before.
5. **Left out:**
   - **No public prototype:** `WinGetErasePS` / `WinReleaseErasePS`, `WinMsgSemWait` / `WinMsgMuxSemWait` appear in no public header; `DosSetCp` is the 16-bit-era call (32-bit programs use `DosSetProcessCp`).
   - **Architectural, for later:**
     - `DosAllocThreadLocalMemory` / `DosFreeThreadLocalMemory`: the same address must hold each thread's own words, which needs per-thread page tables; Warpine's guests run unpaged in one shared address space;
     - `WinTrackRect` / `WinShowTrackRect`: a modal tracking loop on the API thread; frames track on the GUI thread (`frame_track.rs`).
   - **With their subsystem's open work:** `DosRawReadNPipe` / `DosRawWriteNPipe` (message-pipe framing, in TODOs).

## Warpine's choices (unverified)

- The `ERRINFO` layout of the Warp 4.5 Toolkit (ULONG fields, 20 bytes) rather than Open Watcom's (USHORT fields); 16-bit message offsets; the message text (`PM error 0x…`); `WinGetErrorInfo` taking the error.
- White pels as the 1 bits of a pointer's masks; a DBCS character in `WinUpperChar` as the lead byte low, the trail byte high; `WinDrawPointer` at the image's lower-left corner, drawing nothing for system pointers.
- `WinIsWindowShowing` ignoring windows that cover it; `WinGetClipPS` ignoring its `PSF_*` flags; `WinGetScreenPS` drawing nowhere visible.
- `WM_DRAWCLIPBOARD` and `WM_DESTROYCLIPBOARD` posted, not sent.
- `WinWaitMuxWaitSem` not processing sent messages while it waits.
- `WinRegisterUserMsg` / `WinRegisterUserDatatype` recorded only.
- The SWP flags `WinGetMinPosition` / `WinGetMaxPosition` write.
- The timer's frequency; `DosQueryModFromEIP`'s object numbers from 0; `DosErrClass`'s table; `DosQueryProcType` always 32-bit; `DosVerifyPidTid` answering only for this process; `DosCancelLockRequest` succeeding for any file handle.
- FHLOCK values; the plain calls still working on protected handles.
- `DosPeekQueue` copying the element (as `DosReadQueue` does) and answering `ERROR_QUE_EMPTY` past the last element.
- `WinStoreWindowPos`'s own record (no presentation parameters); `WinRestoreWindowPos` making the state change without `WM_MINMAXFRAME` and leaving the visibility as it is.

## Consequences

- **Coverage.** Programs using these calls run on: 61 entry points more (42 PMWIN, 18 DOSCALLS, 1 QUECALLS) plus PMWP's two, 875 in all.
- **Still open:** thread-local memory and `WinTrackRect` wait for their own decisions; the raw pipe calls for the pipe work.
- **Tests:**
  - unit tests in `pm_winutil.rs` (dialog units, case mapping, the window-position record) and `dos_misc.rs` (error classes, FHLOCKs);
  - the PMWIN dispatch-ordinal and count tests now cover `pm_winutil.rs`;
  - `tests/integration.rs`:
    - `gui_pm_winutil_test_checks_the_tier_one_pm_calls`: 95 checks;
    - `test_dos_misc_calls`: 30 checks;
    - `test_queue`: still passes with the fixed wait flag.
