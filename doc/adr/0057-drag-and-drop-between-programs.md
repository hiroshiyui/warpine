# ADR-0057: Drag and drop crosses programs: targets found through the server, DM_* messages as waited calls, drag blocks in the shared arena

- **Status:** Accepted; completes ADR-0052's phase C; extends ADR-0045 to other programs' windows
- **Date:** 2026-11-19
- **Sources:**
  - The drag: `src/loader/pm_drag.rs` (`drag_target_at`, `drag_action`, `DragAction`, `DragRun`, `drag_advance`, `drag_after_sent`, `accept_draginfo`, `drag_block_alloc`, `give_drag_block`, `drg_transfer_to_other`, `drg_push_draginfo`, `hstr_*`)
  - The target's side: `src/loader/pm_remote.rs` (`SentKind::Drag`, `SentFinish::Drag`, `AfterSent::Drag`, `Responder::answer` for `Drag` and `DragTarget`)
  - Server and protocol: `src/desktop/proto.rs` (`WinCall::FrameAt`, `DragTarget`, `Drag`, `StrAdd`, `StrDelete`, `StrName`; `PROTOCOL_VERSION` 6), `src/desktop/model.rs` (`frame_at`), `src/desktop/services.rs` (the session's string handles), `src/gui/desktop_server.rs`
  - Sample `samples/pm_xdrag_test`
  - Earlier decisions: ADR-0026, ADR-0044, ADR-0045, ADR-0049, ADR-0052–0056
  - IBM *PM Programming Reference*: `DrgDrag`, `DrgAllocDraginfo`, `DrgAccessDraginfo`, `DrgFreeDraginfo`, `DrgPushDraginfo`, `DrgSendTransferMsg`, `DrgPostTransferMsg`, `DrgAddStrHandle`, `DM_DRAGOVER`, `DM_DRAGLEAVE`, `DM_DROP`, `DM_RENDER`; *PM Programming Guide*, "Direct Manipulation"

## Context

ADR-0045 made drag and drop within one program. On OS/2 a drag crosses programs: the Workplace Shell, editors and file managers drop on each other. Since ADR-0052 the programs of a session share one desktop, and ADR-0054 lets a thread of one program run a message another program sent.

Crossing programs adds three problems to ADR-0045's loop:

- **Finding the target.** The dragging program knows only its own windows. The topmost frame under the pointer may be another program's.
- **Running the target's procedure.** Only a thread of the target's program can run it, and the source must keep serving the messages sent to it meanwhile. The target typically sends `DM_RENDER` back to the source while handling `DM_DROP`.
- **Sharing the data.** The target reads the source's `DRAGINFO`, `DRAGITEM`s and string handles, and the source reads the target's `DRAGTRANSFER`. In the PM Reference the target gains access to the source's `DRAGINFO` with `DrgAccessDraginfo`, `DrgPushDraginfo` gives it to another window's process, and both sides query the same string handles.

## Decision

### Finding the target

- **The frame.** On each pointer move the dragging thread asks the server which frame is topmost at the point (`WinCall::FrameAt` at `SERVER_ADDRESS`, answered from the desktop model's pictures).
- **The window.** For this program's frame it looks as before (`WindowManager::window_under`). For another program's frame it asks that program (`DragTarget`, answered on its link thread from its window manager): the built-in control, embedded dialog or client under the point, with the program's process id. Frame chrome and the desktop are no target.

### The conversation

- **One step at a time.** Each pointer move, the release or Esc makes one step of `DM_DRAGLEAVE` / `DM_DRAGOVER` / `DM_DROP` messages (`DragRun`).
- **Local and remote messages.** A run of messages to this program's windows is a call chain, as in ADR-0045. A message to another program's window is a `Drag` call, which the server routes like a send and never expires.
- **The target's side.** The target program queues it for the window's thread like a sent message (`SentKind::Drag`), which calls the window procedure, or a container's owner with `CN_*`, with `WinInSendMsg` TRUE. It then answers with the reply. A window that takes no part answers `DOR_NEVERDROP`.
- **Waiting.** The dragging thread waits for the answer without a time limit and runs the messages other programs send it meanwhile (`AfterSent::Drag` goes back to the wait), so `DM_RENDER` from the target reaches the source during `DM_DROP`. While it waits for input, it also runs such messages. A target program that ends or fails counts as `DOR_NEVERDROP`.

### Shared data

- **Drag blocks.** On the session's desktop `DrgAllocDraginfo` and `DrgAllocDragtransfer` allocate giveable blocks in the session's shared arena (ADR-0049), at the same address in every process.
- **Giving the `DRAGINFO`.** The source gives it to the target's process before the first message that needs it. The target accepts it as one of its own: its item count is bounded by the block. `DrgFreeDraginfo` in the target lets its hold go, and the block is gone once the source frees it too.
- **Explicit giving.** `DrgPushDraginfo` gives the `DRAGINFO` to a window's process.
- **Transfers.** `DrgSendTransferMsg` / `DrgPostTransferMsg` to another program's window give its `DRAGTRANSFER` array to that process and send or post through the server. The receiver's `DrgFreeDragtransfer` lets its hold go.
- **String handles.** On the session's desktop the server keeps them as text, like the system atoms (`StrAdd`, `StrDelete`, `StrName`). They are reference counted, shared by equal strings and converted from and to each program's codepage. `DrgDeleteDraginfoStrHandles` in the target therefore deletes the source's handles.
- **Without the session's desktop** (`WARPINE_DESKTOP=local`), blocks and strings stay in the process, as in ADR-0045.

### Warpine's choices (unverified against OS/2)

- **String handles.** They outlive the program that added them until their count drops to zero. A string is at most 16 KB of text.
- **Container targets.** A container in another program asks its owner with `CN_*`, as within a program.
- **Drag pointer.** The source program shows the pointer while it drags, over every program's frames.
- **Private blocks.** A `DRAGINFO` the arena could not hold is never offered to another program: such a target counts as `DOR_NEVERDROP`.

## Consequences

- **What works.** A program can drag to any program's windows on the session's desktop, and the rendering conversation works across programs. `pm_xdrag_test` drags from a parent to a child and checks both sides.
- **Phase C is complete.** With ADR-0056 the session's PM programs share the window list, atoms, DDE and drag and drop.
- **Costs.**
  - Each pointer move over another program costs two short calls (the server, the program) plus a waited one for `DM_DRAGOVER`.
  - Every string-handle call is a round trip to the server.
- **Still missing** (ADR-0045's list): host files dropped on windows, lazy drag, the drag image, key-driven `DM_DRAGOVER`.
