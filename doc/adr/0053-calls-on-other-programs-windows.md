# ADR-0053: Calls on other programs' windows go through the desktop server and are answered on the owner's link thread

- **Status:** Accepted; implements the first part of ADR-0052's phase B (`WinSendMsg` across programs still open)
- **Date:** 2026-11-19
- **Sources:**
  - Call routing: `src/desktop/calls.rs`, `src/gui/desktop_server.rs`
  - Protocol: `src/desktop/proto.rs` (`WinCall`, `WinReply`, `WindowInfo`, `Call` / `Answer`, `Called` / `Answered`, `Active`, `Update::lowered`)
  - Link: `src/desktop/client.rs` (`DesktopLink::call`, `Responder`, `CALL_TIMEOUT`)
  - Both sides of the calls: `src/loader/pm_remote.rs`
  - Sample `samples/pm_xproc_test`
  - Earlier decisions: ADR-0020, ADR-0052
  - IBM *PM Reference*: `WinPostMsg`, `WinIsWindow`, `WinQueryWindowProcess`, `WinQueryWindowPos`, `WinSetWindowPos`, `WinSetActiveWindow`, `WinQueryActiveWindow`, `WinQueryWindowUShort`

## Context

On the session's desktop (ADR-0052) every window handle names its program's desktop slot, so it is unique in the session. The calls still stopped at the process: another program's handle was no window. A window list, a launcher or the Workplace Shell needs to query, post to, move, activate and minimize the windows of other programs.

Each program is its own host process with its own window manager. The window a call names lives only in the program that owns it.

## Decision

### Routing

- **Which calls go out.** A PMWIN call on a handle of another slot (`WindowManager::is_foreign`) is made through the desktop server. The check sits at the head of the PMWIN dispatcher, `Loader::remote_pmwin`, for a fixed list of calls and the stack offset of their handle.
- **The message.** The caller sends `Call { id, hwnd, call }`. A `WinCall` is one of:
  - `Post`;
  - `Describe`: one `WindowInfo` serves the query calls;
  - `Word`, `Query`, `SetPos`, `Show`, `Activate`.
- **The server** (`desktop::calls::CallRouter`, pure and unit-tested):
  - forwards the call to the client of the handle's slot as `Called` under an id of its own;
  - passes back only that client's answer, and only once;
  - refuses a call at once (`Answered … Failed`) when the handle has no slot, names the caller, names a slot nobody holds, or the caller already has 32 calls in flight;
  - refuses calls to a client that leaves;
  - forgets calls unanswered for 10 s.
- **The caller** waits for the answer at most `CALL_TIMEOUT` (5 s). The link going away ends the wait at once.

### Answering

- **Who answers.** The owner's link reader thread answers through `Responder`, implemented by `Loader`, straight from the window manager. The window's own thread does not answer.
  - Every call in this list is served by the window manager.
  - None needs guest code.
  - So the answer does not depend on whether the owner's threads read their queues.
- **What it accepts.** It answers only for windows of its own slot that exist.
- **Deadlock rule.** No thread waits for an answer while holding the window manager's lock. So two programs calling each other cannot deadlock: each one's reader thread is free to answer.

### Activation and stacking

- **Active frame.** The server tells every client the session's active frame (`Active`). `WinQueryActiveWindow` reports another program's frame while none of the caller's own is active (`WindowManager::active_window`).
- **Lowered frames.** A frame sent to the bottom (`z_push_bottom`) is reported as `lowered`, like raised frames. The server puts it below every other program's frames.
  - Raised and lowered frames move before a client's own order fills the slots its frames hold.

### Warpine's choices (unverified against OS/2)

- **No answer.** A call that gets none fails with `PMERR_INVALID_HWND`.
- **Codepage.** Window text and class names come back in the calling thread's codepage.
- **Minimize and maximize.** `WinSetWindowPos` that minimizes, maximizes or restores another program's frame does not ask its client with `WM_MINMAXFRAME`: that needs the window's thread.
- **Message parameters** are passed as they are: a pointer means something to the receiver only in shared memory, as in PM.

## Consequences

- **What works.** A program can, on another program's windows:
  - query them: `WinIsWindow`, `WinQueryWindowProcess`, `WinQueryWindowPos`, `WinQueryWindowRect`, `WinQueryWindowText`, `WinQueryWindowTextLength`, `WinIsWindowVisible`, `WinIsWindowEnabled`, `WinQueryClassName`, `WinQueryWindowULong`, `WinQueryWindowUShort`, `WinQueryWindow`;
  - post to them: `WinPostMsg`;
  - move, size, stack, show, hide, minimize and activate them: `WinSetWindowPos`, `WinShowWindow`, `WinSetActiveWindow`.
- **Proof.** `pm_xproc_test` checks all of these from a parent on its child's frame.
- **Costs.** Each call costs a round trip through the server. A program whose link thread is slow delays only calls made on its own windows.
- **Found on the way.** These were wrong even within one program, and are fixed:
  - the client of `WinCreateStdWindow` now has the identity `FID_CLIENT`;
  - `QWS_ID` (`WinQueryWindowUShort` / `WinSetWindowUShort`) is the window's identity;
  - `QWL_HMQ` is its message queue.
- **Still missing.** `WinSendMsg` across programs, and calls that send messages (`WinSetWindowText`, `WinDestroyWindow` of another program's window). These need the window's thread to run its procedure while the sender waits and handles messages sent to it meanwhile. They keep failing with `PMERR_INVALID_HWND`.
