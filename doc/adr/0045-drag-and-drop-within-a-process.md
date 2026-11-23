# ADR-0045: Drag and drop within a process: a GUI-fed drag channel and DM_* messages as call chains

- **Status:** Accepted; extended to other programs' windows by ADR-0057
- **Date:** 2026-11-18
- **Sources:** `src/loader/pm_drag.rs` (`DragTable`, `DragChannel`, `DragFeed`, `drg_drag`, `drag_continue`, `drag_chain_done`, `window_under`), `src/gui/sdl_renderer.rs` (`DragPress`, `WM_BEGINDRAG`, feeding the drag), `src/loader/pm_container.rs` (`CN_INITDRAG`, `CN_DRAGOVER` / `CN_DRAGLEAVE` / `CN_DROP`), `src/loader/pm_hook.rs` (`ChainThen::Drag`); `samples/pm_drag_test`; IBM *PM Programming Reference* (`DrgDrag`, `DM_DRAGOVER`, `DM_DRAGLEAVE`, `DM_DROP`, `WM_BEGINDRAG`, `DrgFreeDraginfo`, `CN_INITDRAG`, `CNRDRAGINIT`, `CNRDRAGINFO`) and *PM Programming Guide* ("Direct Manipulation"), as on EDM/2; ADR-0026, ADR-0044, ADR-0020

## Context

PM drag and drop is a conversation between windows that runs inside one call.
- Pressing the drag button (button 2) and moving the pointer posts `WM_BEGINDRAG` to the window under the press.
- The source builds a `DRAGINFO` and calls `DrgDrag`, which does not return until the drop. Meanwhile the system:
  - sends `DM_DRAGOVER` to the window under the pointer whenever the pointer moves. The reply (`DOR_DROP`, `DOR_NODROP`, `DOR_NEVERDROP` …) sets the pointer, and after `DOR_NEVERDROP` that window gets no more `DM_DRAGOVER` until the pointer re-enters it;
  - sends `DM_DRAGLEAVE` when the pointer leaves a window or the drag is abandoned;
  - on the release, sends `DM_DROP` if the last reply was `DOR_DROP`, and `DrgDrag` returns the target, else NULLHANDLE.
- Within the dragging process, `DrgFreeDraginfo` fails with `PMERR_SOURCE_SAME_AS_TARGET` until `DrgDrag` returns. The target frees nothing, and the source frees the block afterwards.

In Warpine, pointer input arrives on the GUI thread, while `DrgDrag` runs on the guest thread's vCPU, and only that thread can call the target's window procedure.

## Decision

- **A drag channel between the GUI thread and `DrgDrag`.**
  - `SharedState::drag_channel` holds a `DragFeed`: the pointer position in desktop coordinates, whether it moved, a released button, and Esc, under its own lock with a condition variable.
  - The renderer arms the feed when it posts `WM_BEGINDRAG`: button 2 held and moved by `DRAG_THRESHOLD` pels, sent to the window under the press. A release that comes before `DrgDrag` starts is still recorded.
  - While a drag runs (`active`), the renderer routes no pointer input and gives the moves, releases and Esc to the feed.
- **`DrgDrag` is a loop on the calling thread.**
  - It records a `DragSession` (source, `DRAGINFO`, terminating button, window under the pointer, last reply) and waits on the feed. The wait wakes every 100 ms to see whether the thread was killed or signalled.
  - Each move finds the window under the pointer (`WindowManager::window_under`: a built-in control, an embedded dialog or the client). It makes the `DM_DRAGLEAVE` / `DM_DRAGOVER` sends as a call chain (ADR-0026) with `ChainThen::Drag`.
  - When the chain returns, `drag_chain_done` records the reply, shows the pointer (the system's illegal sign unless `DOR_DROP`), waits again, and continues with the next chain through `ChainEnd::Next` (ADR-0044).
  - The release sends `DM_DROP` (after `DOR_DROP`) or `DM_DRAGLEAVE`, then ends the session and returns.
- **Who answers.**
  - A window with a guest procedure gets the messages itself.
  - A container asks its owner: `WM_CONTROL` `CN_DRAGOVER` / `CN_DRAGLEAVE` / `CN_DROP` with a `CNRDRAGINFO` (the `DRAGINFO` and the record under the pointer). The owner's reply is the container's reply; the reference is ambiguous on this point.
  - Other built-in controls and frame chrome answer `DOR_NEVERDROP` without a call, and `WinDefWindowProc` returns `DOR_NEVERDROP` for `DM_DRAGOVER`.
  - A container that gets `WM_BEGINDRAG` sends `CN_INITDRAG` with a `CNRDRAGINIT` (the record under the pointer, desktop position, offset to the record's origin).
- **Data in guest memory, by address.** `DrgAllocDraginfo` / `DrgAllocDragtransfer` blocks come from the memory manager and are registered in `DragTable`. Only registered blocks and items are accepted. String handles are host-side, reference counted and shared by equal strings (unverified).
- **One process.** Only this process's windows take part, and messages for another thread's window are made on the dragging thread. Warpine runs one OS/2 program per host process (ADR-0020), and a drag between programs needs the session desktop server. `DrgPushDraginfo`, `DrgSendTransferMsg` and `DrgPostTransferMsg` therefore have nothing to give away; the latter two reset `fsReply` and send or post.

## Consequences

- Programs can drag between their own windows and to and from containers; the rendering conversation (`DM_RENDER` …) is ordinary messages between their windows.
- Drags between OS/2 programs, and host files dropped on a window, are not supported yet. Neither are `DrgDragFiles`, `DrgAcceptDroppedFiles`, lazy drag, the drag image (`DrgSetDragImage` is accepted, not drawn), `DM_DRAGOVER` on key presses, target emphasis drawn by the container, or `CN_DRAGAFTER`.
- Unverified: the drag button and threshold (button 2, 4 pels; `SV_BEGINDRAG` is not documented); `xDrop` / `yDrop` updated at each `DM_DRAGOVER` and at the drop; a container passing its owner's reply on; `WM_BEGINDRAG` not passed to the owner by `WinDefWindowProc` (it returns FALSE here; PM sends it to the owner).

## Addendum (2026-11-23): drag images, container target emphasis, value sets

- **Drag images** (`pm_dragimage.rs`). `DrgDrag`'s `DRAGIMAGE`s are composed into one picture shown as the host pointer: icons (`DRG_ICON`) and bit maps (`DRG_BITMAP`), stretched with `DRG_STRETCH`, each with its lower-left corner at `cxOffset` / `cyOffset` from the hot spot, the first at the bottom. Over a window that refuses, a red "no" sign covers the hot spot; a `DrgSetDragPointer` pointer shows instead over one that accepts. `DrgSetDragImage` replaces the picture during the drag. Drawing it as the pointer keeps the compositors unchanged and follows the pointer at host speed.
- **Container target emphasis.** The container keeps the emphasis its owner's last `DOR_DROP` reply allows (`CnrState::target`): an outline round the record under the pointer or round the container, or an insertion line after a record. With `CA_ORDEREDTARGETEMPH` / `CA_MIXEDTARGETEMPH` in the name, text and details views it sends `CN_DRAGAFTER` (the record the drop goes after, `CMA_FIRST` before the first).
- **Value sets** start drags with `VN_INITDRAG` on `VIA_DRAGGABLE` items and ask their owner as targets with `VN_DRAGOVER` / `VN_DRAGLEAVE` / `VN_DROP`.
- Unverified (Warpine's choices): the picture's 256-pel bound, the "no" sign's look, polygons and `DRG_TRANSPARENT` not drawn, `DrgSetDragImage` outside a drag only checking the `DRAGINFO`; the outline and line as the emphasis; the bands of ordered drops (a row's upper half before it, lower half after; mixed: the middle half on the row); `CN_DROP` naming the record under the pointer also with ordered emphasis; `VSDRAGINIT`'s `cx` / `cy` as the offset to the item's origin; `VSDRAGINFO` naming row and column 0 off a `VIA_DROPONABLE` item.
