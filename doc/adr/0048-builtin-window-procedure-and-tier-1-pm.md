# ADR-0048: A guest-callable procedure for built-in windows; standard frames run the default procedure; child presentation spaces in the frame surface

- **Status:** Accepted; extends ADR-0026 and ADR-0035
- **Date:** 2026-11-18
- **Sources:** `src/loader/pm_window.rs` (`builtin_window_proc`, `set_window_proc`, `wrp_builtin_window_proc`), `src/loader/warpine_api.rs` (`WARPINE.5`), `src/loader/mod.rs` (`dispatch_builtin_control`), `src/loader/pm_win.rs` (`def_window_proc`, `WinUpdateWindow`, `WinShowWindow`, `WinBeginPaint` / `WinEndPaint`), `src/loader/pm_frame.rs` (`frame_format_setup`, `frame_format_finish`, `show_created_frame`), `src/loader/pm_types.rs` (`DrawTarget::Child`, `dialog_items`, `is_guest_control`, `destroy_queue_windows`, `set_focus`), `src/loader/pm_dialog.rs` (`dialog_code`, `dialog_char_result`), `src/loader/npipe.rs`, `src/loader/tcpip.rs` (`so_wait`), `src/loader/vcpu.rs`; `samples/pm_subclass_test`; IBM *PM Programming Reference* (`WinSubclassWindow`, `WinQueryClassInfo`, `WM_FORMATFRAME`, `WM_QUERYFRAMECTLCOUNT`, `WM_CALCFRAMERECT`, `WM_CHAR`, `WM_QUERYDLGCODE`, `WM_MATCHMNEMONIC`, `WM_SETFOCUS`, `WinBeginPaint`, `WinUpdateWindow`), as on EDM/2

## Context

The tier-1 gaps found on 2026-11-18 had one root: built-in windows are host code, with no guest address for their window procedure. As a result:
- `WinSubclassWindow` on a built-in control returned 0, so a subclass procedure that passed messages on called address 0.
- A standard frame borrowed its client's procedure, because `WinCreateStdWindow` made the frame with the application's class. Subclassing the frame returned the client's procedure, and messages for the frame ran the client's code.
- A subclassed frame was never asked `WM_QUERYFRAMECTLCOUNT` / `WM_FORMATFRAME`.

Next to that:
- A presentation space of a window inside the client (a child, a control) drew into a surface of its own, never shown.
- `WinBeginPaint` did not clip to the update area.
- `WinUpdateWindow` only recomposited.
- Controls of the application's own classes took no part in dialog Tab and mnemonics.
- A thread's windows outlived it.
- Some blocking calls ignored `DosKillThread` and signals (ADR-0035).

## Decision

- **`WrpBuiltinWindowProc` (`WARPINE.5`)** is the built-in classes' window procedure as the guest sees it. It runs the message as `WinDispatchMsg` does for an unsubclassed built-in window.
  - `WinSubclassWindow`, `WinSetWindowPtr(QWP_PFNWP)`, `WinQueryWindowPtr(QWP_PFNWP)` and `WinQueryClassInfo` hand it out for built-in classes, frames and dialogs.
  - Setting it back makes the window plain built-in again (`pfn_wp` 0).
  - Subclassed `WC_*` controls are still painted, hit-tested and tabbed to as built-in controls.
  - Messages Warpine gives a control directly do not pass through the subclass procedure: repainting after `WinEndPaint`, and `WinDefDlgProc` pressing a button.
- **A standard frame has no guest procedure**, and the system frame procedure is Warpine's `WinDefWindowProc` (`def_window_proc`), with the frame formatting messages.
  - When a frame with a subclass procedure is laid out, two owner calls (`OwnerCallKind::FrameFormat`) ask `WM_QUERYFRAMECTLCOUNT`, then `WM_FORMATFRAME`. The second passes the client `RECTL` and room for that many `SWP`s (at most `MAX_FRAME_SWPS`), on a guest-stack block of up to 1 KB with `mp1` pointing into it.
  - The `SWP`s returned are applied to the frame's children directly, without laying the frame out again.
  - `WinCalcFrameRect` sends `WM_CALCFRAMERECT` to a subclassed frame as a call chain.
  - A `WC_FRAME` made with `WinCreateWindow` on the desktop is composited from the start, and its children are placed relative to it even without a client.
- **`WinDefWindowProc` sends `WM_CHAR` to the window's owner** ("The default window procedure sends the message to the owner window if it exists"), so a key an application control does not use reaches its dialog.
- **Focus changes post `WM_SETFOCUS`** to windows with a guest procedure: FALSE to the window losing it, then TRUE to the one gaining it, mp1 = the other window. PM sends the message; Warpine posts it.
- **Dialog items of the application's own classes** (`is_guest_control`) take the focus and are in Tab, arrow-group and first-focus order (`dialog_items`) when visible, enabled and `WS_TABSTOP`. Warpine does not ask them `WM_QUERYDLGCODE` for that.
  - A mnemonic no built-in control has is asked of them with `WM_MATCHMNEMONIC`, a call chain stopping at the first TRUE. That control takes the focus.
  - Built-in controls answer `WM_QUERYDLGCODE` (`DLGC_*` by class and style) and `WM_MATCHMNEMONIC` (their `~` mnemonic, any case).
- **Painting.**
  - A presentation space of a window inside a frame other than the client draws into the frame's surface at the window's place, clipped to it (`DrawTarget::Child`).
  - `WinBeginPaint` clips its presentation space to the update rectangle until `WinEndPaint` (`paint_clip`). It leaves it unclipped when nothing waits to be painted, Warpine's choice.
  - `WinUpdateWindow` has the window and its descendants that need painting paint before it returns, as synchronous-paint owner calls.
  - `WinShowWindow` invalidates a window it shows.
- **A thread's windows go with it.** When a thread ends, the windows of its message queue are destroyed without `WM_DESTROY` (no thread is left to run their procedures), before the queue.
- **More interruptible waits** (ADR-0035):
  - A killed thread leaves the modal dialog loop.
  - A pipe write blocked with nothing written is made again after the signal. A byte stream that got some chunks out stops with their count. A frame is never abandoned in the middle.
  - A blocking `accept`, `recv`, `recvfrom`, `send` or `sendto` waits in `poll` slices and fails with `SOCEINTR`. This does not apply to non-blocking sockets, `MSG_DONTWAIT`, or sockets with their own timeout.
  - The critical-section wait already gave up for a killed thread.
  - `WinSendMsg` to another thread's window runs the procedure on the caller's own thread as guest code, which a kick interrupts (ADR-0023).

## Consequences

- The common PM idioms work: subclassing an entry field or a frame to pass messages on, a tool bar or status line placed by `WM_FORMATFRAME`, custom controls in dialogs, drawing into child windows with `WinGetPS`, and `WinUpdateWindow` before reading back.
- Messages meant for a standard frame no longer run the client's procedure.
- Remaining limits:
  - a `send` larger than the socket's free room can still block after the wait;
  - a pipe write blocked in the middle of a frame waits for the reader;
  - `connect`, host file I/O, MMPM/2 waits and CMD.EXE's child waits are still not interruptible;
  - a child presentation space is clipped to the child but not to its ancestors;
  - `WM_SETFOCUS` is posted, not sent.
- Unverified:
  - `WinBeginPaint` with nothing to update drawing unclipped;
  - the `DLGC_*` codes of the built-in classes beyond the reference's list;
  - Tab order of application controls without `WM_QUERYDLGCODE`;
  - `WM_FORMATFRAME`'s `SWP` room;
  - the order of `WM_SETFOCUS` (loser first).
