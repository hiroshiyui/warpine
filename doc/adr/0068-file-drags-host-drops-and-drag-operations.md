# ADR-0068: File drags, host files dropped on windows, the drag's operation keys and WM_ENDDRAG

- **Status:** Accepted; extends ADR-0045 (drag and drop within a process), ADR-0057 (between programs), ADR-0052 (desktop protocol, now version 8) and ADR-0066 (guest isolation)
- **Date:** 2026-11-20
- **Sources:**
  - The code:
    - `src/loader/pm_drag.rs` (`FileItem`, `file_draginfo`, `discard_draginfo`, `drg_drag_files`, `drg_query_draginfo_ptr`, `drg_query_draginfo_from_hwnd`, `drg_realloc_draginfo`, `operation_for`, `DragChannel::modifiers`, `queue_host_drop`, `host_drop_draginfo`, `host_drop_step_done`);
    - `src/loader/pm_remote.rs` (`SentKind::HostDrop`, `SentFinish::HostDrop`);
    - `src/loader/pm_win.rs` (`def_window_proc`);
    - `src/gui/sdl_renderer.rs` (`host_drop`, the drop events, `WM_ENDDRAG`, the modifier keys);
    - `src/gui/desktop_server.rs`, `src/gui/desktop_events.rs` (`drop_input`), `src/desktop/proto.rs` (`Input::Drop`, `MAX_DROP_FILES`, `MAX_DROP_PATH`);
    - `src/gui/gui_script.rs` (`drop X Y PATH`, `MOD+rdrag`).
  - Sample: `samples/pm_dropfiles_test`.
  - IBM *PM Programming Reference* (Warp 4): "Direct Manipulation Functions" (`DrgDragFiles`, `DrgQueryDraginfoPtr`, `DrgQueryDraginfoPtrFromHwnd`, `DrgReallocDraginfo`), `DM_DRAGOVER`, `DM_DROP`, `DM_DRAGLEAVE`, `WM_BEGINDRAG`, `WM_ENDDRAG`, `DRAGINFO`, `DRAGITEM`; the Warp 4 Toolkit's `<pmstddlg.h>` (`DRT_*`, `DO_*`, `DC_*`) and `<pmwin.h>` (`WM_ENDDRAG` 0x0421); CUA's Ctrl / Shift drag keys.

## Context

Drags worked within a program and between programs (ADR-0045, ADR-0057), but only for `DRAGINFO`s a program built itself:
- files dragged in from the host desktop (a file manager) did not reach a window;
- `DrgDragFiles`, the call that drags files without building the structure, was missing, as were the `DRAGINFO` enumeration and reallocation calls;
- `usOperation` was always `DO_DEFAULT`: Ctrl and Shift did nothing;
- a `WM_BEGINDRAG` that started no drag never ended with `WM_ENDDRAG`, and a child window that left the gestures to `WinDefWindowProc` lost them.

A host drop has no OS/2 source: no program's thread runs `DrgDrag` to make the `DM_*` calls. And a guest must never learn host paths outside its drives (ADR-0066).

## Decision

### File items

A file is a `DRAGITEM` with the RMF `<DRM_OS2FILE,DRF_UNKNOWN>`:
- the container name is the file's directory, with its trailing backslash, and the source and target names are its name;
- the type is `DRT_UNKNOWN` unless one is given;
- `fsSupportedOps` is `DO_COPYABLE | DO_MOVEABLE`, and `fsControl` is `DC_CONTAINER` for a directory.

`file_draginfo` builds such a `DRAGINFO`, with one string handle per distinct text.

`DrgDragFiles` builds one from its file names (types and target names optional) and drags it as `DrgDrag` does. The session is marked `files`: when it ends, the `DRAGINFO` and its handles go, and the call returns whether the files were dropped.

### Host drops

- **On the window's thread.** Host files dropped on the desktop become a `SentKind::HostDrop` for the window under the pointer, run by that window's thread wherever it reads messages, as messages sent from another program are (ADR-0054).
  - Its first step builds the `DRAGINFO` there, with no source window, and sends `DM_DRAGOVER`.
  - The answer queues the next step for the same thread: `DM_DROP` after `DOR_DROP`, else `DM_DRAGLEAVE`.
  - After the drop the `DRAGINFO` is the target's to free. After a leave, or when no window took part, Warpine frees it.
- **Only the program's own files.** Host paths are canonicalized and turned into OS/2 paths through the program's drives (`DriveManager::host_to_os2_path`). Paths outside every drive, and drive roots, are left out, so a guest never learns a host path outside its drives.
- **On the session's desktop.** The server collects SDL's `DropFile`s between `DropBegin` and `DropComplete`. It sends them, as one `Input::Drop { x, y, paths }` (protocol version 8; at most 16 paths of at most 4096 bytes, longer paths dropped rather than cut), to the program under the pointer, routed like motion. That program's renderer hands the drop to the window.
- **Scripts.** `drop X Y PATH` drops a host file in a GUI script.

### The operation

- **Keys.** Ctrl, Shift and Ctrl+Shift held mean `DO_COPY`, `DO_MOVE` and `DO_LINK`; none means `DO_DEFAULT`.
- **Recorded with the input.** The renderer reports the modifiers with every key event. The drag channel stamps the operation on each move and release it feeds, so input that arrived before `DrgDrag` started keeps the keys it came with.
- **Written before each message.** The operation is written into `usOperation` before each `DM_DRAGOVER` and before the `DM_DROP`. A change of the keys during a drag asks the window again.
- **Scripts.** `MOD+rdrag` holds modifiers until the release.

### Gestures and the default procedure

- Button 2 coming up after `WM_BEGINDRAG`, with no drag running, sends `WM_ENDDRAG` to the window under the pointer.
- `WinDefWindowProc` sends `WM_BEGINDRAG`, `WM_ENDDRAG` and `WM_CONTEXTMENU` on to the window's owner, as it does `WM_CHAR`.

### Enumeration and reallocation

- `DrgQueryDraginfoPtr` walks the program's `DRAGINFO`s by address (NULL: the first).
- `DrgQueryDraginfoPtrFromHwnd` finds the one whose `hwndSource` is the window.
- `DrgReallocDraginfo` makes a new block with the old header and the items that fit, and frees the old one.

### Warpine's choices (unverified)

- **File items.**
  - `fsSupportedOps` of file items, and type `Unknown` for host files.
  - `fSourceRender` of `DrgDragFiles` not being used.
  - `DrgDragFiles` returning TRUE only for a drop.
- **Host drops.**
  - No source window, and the target freeing the `DRAGINFO` after `DM_DROP`.
  - The drop happening where the pointer last was: SDL's drop events reach Warpine without a position.
- **The operation.**
  - Ctrl+Shift as `DO_LINK`.
  - A key change sending `DM_DRAGOVER` again.
- **Gestures.**
  - `WM_ENDDRAG`'s mp1 / mp2.
  - `WM_ENDDRAG` also coming when `DrgDrag` starts after the release.
  - The owner getting the three gestures from `WinDefWindowProc`: their reference pages' default processing is read as `WM_CHAR`'s.
- **Enumeration and reallocation.**
  - The enumeration order and NULL after the last.
  - `DrgQueryDraginfoPtrFromHwnd` taking the lowest address.
  - `DrgReallocDraginfo` refusing a `DRAGINFO` that is being dragged (`PMERR_ALREADY_DRAGGING`).

## Consequences

- **Files from the host.** Programs that take files from the WPS take them from the host's file manager too, as long as the files lie inside a drive. The drag goes through the program's own thread, so its replies are honoured as for any drag.
- **The protocol.** Its version is 8, so clients and servers must be from the same build.
- **Still open:** `DrgAcceptDroppedFiles`, `DrgQueryFormat`, lazy drag, drawing the drag image, container target emphasis and `CN_DRAGAFTER`, a host drop's position on real displays.
- **Tests:**
  - `pm_drag.rs`: operation keys, file items, `DrgDragFiles`, enumeration and reallocation, host drops through their sent steps;
  - `pm_window.rs`: the owner gets the gestures;
  - `proto.rs`, `desktop_events.rs`, `gui_script.rs`: the drop input and its limits;
  - `samples/pm_dropfiles_test`: 42 checks on both desktops, with a golden picture.
