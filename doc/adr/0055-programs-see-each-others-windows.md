# ADR-0055: Window lists, broadcasts, state changes and send hooks reach other programs

- **Status:** Accepted; completes the follow-ups of ADR-0052's phase B; amends ADR-0053 (`WM_MINMAXFRAME`) and ADR-0054 (send hooks)
- **Date:** 2026-11-19
- **Sources:**
  - Calls the server answers itself: `src/desktop/calls.rs` (`server_answer`), `src/desktop/model.rs` (`stacking`), `src/desktop/proto.rs` (`SERVER_ADDRESS`, `slot_address`, `WinCall::{Clients, Stacking, Children, Broadcast}`, `WinReply::Handles`, `MAX_HANDLES`)
  - Both sides: `src/loader/pm_remote.rs` (`desktop_windows`, `merge_by_stacking`, `broadcast_to_others`, `broadcast_for_other`, `set_pos_for_other`, `SentKind::MinMax`, `remote_wait`, `run_sent`)
  - Window enumeration: `src/loader/pm_enum.rs`
  - Sample `samples/pm_xproc_test`
  - Earlier decisions: ADR-0026, ADR-0052, ADR-0053, ADR-0054
  - IBM *PM Reference*: `WinBeginEnumWindows`, `WinGetNextWindow`, `WinEndEnumWindows`, `WinBroadcastMsg`, `WM_MINMAXFRAME`, `SendMsgHook`

## Context

After ADR-0053 and ADR-0054 a program could call and send to another program's window, but only if it already had the handle.

- **No way to learn handles.** Nothing listed the windows on the desktop: `WinBeginEnumWindows` was not implemented at all. Yet a window list, a task manager or the Workplace Shell starts from that list.
- **Broadcasts stopped at the process.** `WinBroadcastMsg(HWND_DESKTOP, …)` reached only the caller's own windows.
- **The client was not asked.** Minimizing or maximizing another program's frame skipped `WM_MINMAXFRAME`.
- **No send hooks.** Sends between programs ran no send hooks.

## Decision

### The server answers for the session

- **Server calls.** A `Call` at `SERVER_ADDRESS` (0, never a window handle) is answered by the server itself:
  - `Clients`: the slots of the connected programs;
  - `Stacking`: the session's frames, top first, from the desktop model.
- **Program calls.** A call at a program's *slot address* (its handle prefix with no window part) is about the program as a whole. The server routes it like any call to that slot:
  - `Children`: its windows on the desktop, top first;
  - `Broadcast`: a broadcast to them.
- **Bounds.** Replies carrying handles hold at most `MAX_HANDLES` (4096) of them.

### Window lists

- **New calls.** `WinBeginEnumWindows` / `WinGetNextWindow` / `WinEndEnumWindows` are new (`pm_enum.rs`). Each enumeration is a snapshot, top first, read one window at a time.
- **The desktop's children.** These are the windows of every program on the session's desktop (`desktop_windows`):
  - frames in the session's stacking order;
  - then each program's other windows (hidden frames, other top-level windows) in that program's order.
- **Another program's window.** Its children come from a `Children` call.

### Broadcasts

- **Who is reached.** `WinBroadcastMsg` to the desktop, and with `BMSG_POSTQUEUE`, also reaches every other program through a `Broadcast` call to its slot address.
- **What the other program does.** It posts the message to its windows (or queues), or for `BMSG_SEND` queues it for each window's thread like a sent message, with no answer awaited.

### State changes and send hooks on the window's thread

- **State changes.** A `WinSetWindowPos` that minimizes, maximizes or restores another program's frame is made like a send:
  - The owner makes the rest of the call at once, then queues a `SentKind::MinMax` for the frame's thread.
  - That thread asks the client `WM_MINMAXFRAME`. The `SWP` is passed in the call chain's block, now `CHAIN_BLOCK_BYTES` 48. TRUE keeps the frame as it is.
  - The thread then answers.
  - The caller waits without a time limit, running messages sent to itself meanwhile (`remote_wait`, which serves `WinSendMsg` too). The server does not expire these calls.
- **Send hooks.** A message sent from another program runs the send hooks of the window's queue, and the program's own system send hooks, on the receiving thread before the window procedure, with `fInterTask` TRUE.

### Warpine's choices (unverified against OS/2)

- **Broadcast sends.** Sends a broadcast makes to other programs' windows are not waited for. Within the program they still are, one after the other.
- **Enumeration snapshots.** A window of the program destroyed after `WinBeginEnumWindows` is skipped, and another program's window is returned as it was. At most 64 enumerations may be open (`PMERR_HEAP_OUT_OF_MEMORY` beyond). The children of `HWND_OBJECT` are the program's object windows, newest first.
- **Where send hooks run.** Send hooks run in the receiving program, like Warpine's in-process sends, which run the hooks of the window's queue.

## Consequences

- **What works.** A program can list every window on the session's desktop and in other programs' windows, and broadcast to every program. Another program's minimize, maximize and restore now ask the frame's client, which can refuse. `pm_xproc_test` checks all of these.
- **Costs.**
  - A desktop enumeration makes one call to the server for the programs, one for the stacking, and one per other program.
  - A broadcast to the desktop makes one call per other program.
- **Phase B is complete.** Shared shell services (switch list, atom table, DDE between programs) remain phase C.
