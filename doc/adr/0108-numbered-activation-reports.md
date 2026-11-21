# ADR-0108: Numbered activation reports, taken before the program goes on

- **Status:** Accepted; amends ADR-0052 (the desktop protocol, version 9, then 10 with item 7)
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *PM Programming Reference*: `WinSetActiveWindow`, `WinQueryActiveWindow`, `WM_ACTIVATE`.
  - The failure itself: `pm_xproc_test` failed its activation checks now and then under load (2 of 15 runs with every CPU busy).

## Context

Under ADR-0052 each program reported its active frame to the desktop server with every update and input acknowledgement. When a report *changed* to a frame, the server made that frame the session's active one and sent the program that held activation before a `Deactivate`. Reports that did not change were ignored, so that a program which had not yet read its `Deactivate` could not take activation back by repeating itself.

Logging the failing runs of `pm_xproc_test` showed the race:
1. Program B showed its frame and reported it. The server made it active and sent A a `Deactivate`.
2. Before A's GUI thread read that message, A called `WinSetActiveWindow` on its own frame, which A still took to be active. Locally the call changed nothing, and A's next report repeated the old one, which the server ignored.
3. A then read the `Deactivate` and gave activation up. B kept it, and every later activation check failed.

Two more orderings had the same effect:
- **A late first report:** B activates its new frame and tells A through shared memory; A's `WinSetActiveWindow` reaches the server first; then B's report, sent by B's GUI thread at its next composite, takes activation from A.
- **Server messages behind the answers:** `Deactivate` and `Active` waited in the inbox for the GUI thread, while the answers to calls were taken at once by the link's reader thread. So after A's `WinSetActiveWindow` on B's frame returned, A could still think itself active, and `WinQueryActiveWindow` could name the old frame.

## Decision

1. **Reports are numbered** (`proto::Activation { frame, seq }`, in `Update`, `Ack` and the new `WinCall::Report`):
   - A program numbers its reports in order (`WindowManager::activation_report`), and reports anew whenever its active frame changes.
   - Activating a frame on purpose is a *claim* (`claim_activation`): a new report even when the frame was already active locally. The claims are `WinSetActiveWindow`, a switch to a frame, a click on a frame, `SWP_ACTIVATE`, and the VIO window coming back.
   - The server takes only a report newer than the program's last (numbers wrap around). Repeated and overtaken reports change nothing.
2. **`Deactivate` names the report it answers** (`seq`): the number of the report that made the frame active. A program whose newest report has another number, or that has an unreported claim, ignores it as stale (`deactivate_frame`).
3. **A change made by an API call is reported before the call returns.**
   - After every API call, a program with a desktop link reports a changed active frame in a `Report` call to the server and waits for the answer (`Loader::report_activation`, from `handle_api_call_ex`).
   - The answer is the session's active frame, which the program keeps as `session_active`.
   - A change made in answer to another program's `WinSetActiveWindow` is reported with `notify`, ahead of the answer on the same link.
4. **The server raises a frame that becomes active,** over every other program's frames, keeping its program's own order. A report can arrive before the update that raises the frame.
5. **The link's reader thread takes `Deactivate` and `Active` at once** (`Responder::take_activation`), in order with the answers to the program's calls. Without a responder they wait in the inbox as before.
6. **`PROTOCOL_VERSION` is 9.**
7. **Stacking too (amended the same day; protocol version 10).** The frames an API call raises or lowers travel in the same `Report` call, not in the renderer's next update.
   - On a thread that makes the guest's API calls (`pm_types::set_api_thread`: vCPU threads, and the link's thread answering other programs), `z_push_top` / `z_push_bottom` record them in `reported_raised` / `reported_lowered`. `WindowManager::take_report` sends them with the activation report.
   - The GUI thread's own raises, such as a click on a frame's title, still go in its updates.
   - The server places a frame announced in a `Report` before any update has drawn it (`DesktopModel::reported`, `announced`). The frame's first update then leaves it where it is instead of putting it on top.
   - Found when `pm_xproc_test`'s "the active one first" check failed under load (4 of 20 runs): B's raise of its new frame, sent with B's first update, arrived after A had activated and raised its own frame.

## Warpine's choices (unverified)

- **Which activations are claims:** the five listed above. A new frame or dialog becoming active is a changed report anyway.
- **One round trip** to the server after an API call that changed the active frame, on the calling thread. Programs that activate frames in a tight loop pay for it.
- **A click on the frame that is already active is also a claim:** its acknowledgement carries a new report. That changes nothing unless the server had given activation to another program meanwhile.

## Consequences

- `pm_xproc_test` passed 30 of 30 runs under the load that failed 2 of 15 before; with item 7, 30 of 30 under the load where its stacking check had failed 4 of 20. `pm_xshell_test`, `pm_xdrag_test` and `pm_switch_dde_test` passed 10 of 10 runs each under the same load.
- **Tests:**
  - `model.rs`: newer reports only, a claim taking activation back, raising in the client's order, wrap-around;
  - `pm_minmax.rs`: report numbering and stale `Deactivate`s;
  - `client.rs`: activation taken before the answer that follows it;
  - `proto.rs`: the new fields round-trip.
- **Found, not fixed:** the clipboard between programs has the same kind of race. A program's text reaches the others through its GUI thread and the server's broadcast, so another program told by other means that the clipboard changed can still read the old text. It is in `doc/TODOs.md`.
