# ADR-0058: Pointer input goes to the deepest window and asks WM_HITTEST when read; update areas are regions; must-complete sections hold signals

- **Status:** Accepted; extends ADR-0051 (pointer routing), ADR-0048 (child presentation spaces, paint clip) and ADR-0035 (interruptible calls)
- **Date:** 2026-11-19
- **Sources:**
  - Input: `src/loader/pm_input.rs` (`window_under_pointer`, `HitAsk`, `hit_test_answered`, `queue_status`, move coalescing in `post_input`), `src/loader/pm_win.rs` (`Got::HitTest`, `WinQueryQueueStatus`, `WinDefWindowProc(WM_HITTEST)`), `src/loader/pm_remote.rs` (`hit_test_chain`), `src/loader/vcpu.rs` (modal loop), `src/loader/pm_drag.rs` (`window_under`)
  - Painting: `src/loader/pm_paint.rs` (`OS2Window::update` as a `Region`, `invalidate_area`, `validate_area`, `update_region`), `src/loader/pm_region.rs` (`WinInvalidateRegion`, `WinValidateRegion`, `WinQueryUpdateRegion`, `WinExcludeUpdateRegion`), `src/loader/pm_types.rs` (`target_visible`), `src/loader/mod.rs` (`draw_clipped`)
  - Signals: `src/loader/threads.rs` (`must_complete_step`), `src/loader/signals.rs`, `src/loader/interrupt.rs` (`call_interrupted`), `src/loader/tcpip.rs` (`connect`, `send`), `src/loader/mmpm.rs` (`wait_or_stop`), `src/loader/cmd.rs` (`wait_child`)
  - Sample `samples/pm_paint_input_test`
  - IBM *PM Programming Reference*: `WM_HITTEST`, `CS_HITTEST`, `WinDefWindowProc`, `WinQueryQueueStatus`, `WinBeginPaint`, `WinInvalidateRegion`, `WinQueryUpdateRegion`; *Control Program Programming Reference* and *Guide*: `DosEnterMustComplete`, `DosExitMustComplete`, "Must-Complete Sections" (as on EDM/2 and the OS/2 Toolkit mirror)

## Context

Three gaps of the PM and kernel basics affected most programs:

- **Pointer input stopped at the client.**
  - Mouse messages went to a frame's client, or for button 1 to a built-in control.
  - Child windows of an application's own class (custom controls, panes, tool bars) never got any, and button 2 and 3 never reached a built-in control.
  - `CS_HITTEST` and `WM_HITTEST`, by which a window lets clicks through or refuses them, did not exist.
- **The update area was a rectangle.**
  - Two small invalidated areas far apart repainted everything between them.
  - `WinQueryUpdateRegion` could not report what was really invalid.
  - A child window's presentation space drew outside its parent.
- **Signals had no must-complete sections**, and `connect`, a large `send`, MMPM/2 waits and CMD.EXE's waits for its programs could not be interrupted.

## Decision

### Pointer input

- **The deepest window.** A pointer event goes to the deepest visible window under the pointer: the frame's topmost child there, then that child's topmost child there, and so on (`window_under_pointer`). This applies to any class, every button and moves.
- **What is passed over.**
  - Static controls let the pointer through.
  - A disabled window takes nothing (its parent does).
  - The menu bar stays frame chrome.
- **Unchanged.** The capture window and a built-in control held with button 1 still take the pointer first (ADR-0051).
- **`WM_HITTEST` is asked when the event is read**, as the reference describes ("when an application requests a message through WinPeekMsg or WinGetMsg"):
  - A window whose class has `CS_HITTEST` is asked before the event becomes its message. `pm_input` records a `HitAsk`, and the reading thread's `WinGetMsg` / `WinPeekMsg` / `WinWaitMsg` / modal loop sends `WM_HITTEST(mp1 = the point in the window's coordinates, mp2 = 0)` as a call chain, records the answer and reads again.
  - The event waits at the head of the input queue meanwhile; paint and timer messages wait too.
- **The answers.**
  - `HT_TRANSPARENT` passes the event to the windows beneath: the window is skipped and the search repeated.
  - `HT_DISCARD` drops it.
  - `HT_ERROR` drops it but still activates the frame of a press (no alarm is sounded).
  - Anything else delivers it.
  - `WinDefWindowProc` answers `HT_ERROR` for a disabled window, `HT_NORMAL` otherwise.
- **Drags agree.** `window_under` (drag targets, `WM_BEGINDRAG`) uses the same search.
- **Coalescing.** A mouse move queued right behind a move not yet routed replaces it, so a busy program does not fill the input queue with moves.
- **`WinQueryQueueStatus`** (PMWIN 825) reports the `QS_*` kinds:
  - in the high word, what waits for the calling thread: its queued messages, the input it is to read, messages other programs sent it;
  - in the low word, those of them that came since its previous call. The queue remembers what the last call saw.

### Painting

- **Update regions.** A window's update area is a `Region`:
  - invalidation adds exactly, validation removes exactly (holes included);
  - `WinQueryUpdateRect` and `prclPaint` report its bounding rectangle;
  - `WinQueryUpdateRegion` and `WinExcludeUpdateRegion` the region itself;
  - the `WinBeginPaint` presentation space is clipped to it.
  - A region too complex to combine (beyond `MAX_REGION_RECTS`) falls back to its bounding rectangle when growing, and stays as it is when cut.
- **Child presentation spaces.** A child window's presentation space is clipped to its rectangle cut by each ancestor's within the frame (`target_visible`).
- **Stale TODO.** `CS_SYNCPAINT` was already handled (it maps to `WS_SYNCPAINT`).

### Signals and interruptible calls

- **Must-complete sections.** `DosEnterMustComplete` / `DosExitMustComplete` keep a nesting count per thread, returned in `*pulNesting`; leaving with none open returns `ERROR_ALREADY_RESET`.
  - While thread 1 is in a section, signal exceptions and termination requests from outside wait. They are delivered when the section ends.
  - A thread that ends leaves its sections.
- **Sockets.**
  - A blocking `connect` is made without blocking and waited for in slices. A signal stops it with `SOCEINTR`, as BSD's `EINTR`, and the connection goes on.
  - A blocking `send` larger than the free room sends in slices. A signal stops it with the count sent so far, or `SOCEINTR` when nothing was sent.
- **MMPM/2.** `DosBeep` and `MCI_WAIT` playback wait in slices: a signal or a kill ends the wait. The tone is cut; the MCI sound plays on, as without `MCI_WAIT`.
- **CMD.EXE.** The shell waits for the program it started in slices. When its call is interrupted, it asks the program to end (`SIGTERM`, so the exit list runs) and kills it after `KILL_GRACE`.
- **16-bit programs.** Their calls are bracketed like 32-bit ones, so a kill or the process end can stop a blocking one. Their API set has none yet.

### Warpine's choices (unverified against OS/2)

- **Hit testing.**
  - Static controls let pointer input through.
  - `HT_TRANSPARENT` makes the search go on without the window: its siblings beneath, else its parent.
  - At most 64 windows passed over per event.
  - `HT_ERROR` sounds no alarm.
- **Coalescing.** Only a move right behind a move not yet routed is merged.
- **`WinQueryQueueStatus`.**
  - The window handle is not checked.
  - Input counts for the queue that would read it now.
  - Only `WinQueryQueueStatus` itself resets the low word.
- **Must-complete.**
  - The deepest nesting is 65535 (`ERROR_NESTING_TOO_DEEP` beyond).
  - Only thread 1's sections hold signals, since signals go to thread 1.
- **Interrupted waits.** The MCI playback interrupted in `MCI_WAIT` plays on; CMD.EXE ends the waited-for program when interrupted.

## Consequences

- **What works.** Custom child windows, panes and controls get every mouse message, transparent and refusing windows work, partial invalidation repaints only what is invalid, and must-complete sections protect critical code from signals. `pm_paint_input_test` checks all of these.
- **Changed behaviour.**
  - Built-in controls now also get `WM_MOUSEMOVE` and buttons 2 and 3 over them. They ignore what they do not use, as before for messages sent to them.
  - Mouse moves are merged.
- **Still missing.**
  - Signals in the modal dialog loop.
  - Signal focus across the processes of a session.
  - Ctrl+Break at a terminal.
  - A pipe write blocked in the middle of a frame.
  - Host file I/O.
  - `ERROR_INVALID_PROCID` for PM programs.
  - A GDB stop request kicking the vCPUs.
  - `WM_HITTEST` for clicks on frame chrome, which the GUI thread handles at once.
