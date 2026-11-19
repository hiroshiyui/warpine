# ADR-0051: Pointer input routed when it is read: clicks hit-tested and activating at the head of the input queue

- **Status:** Accepted; extends the system input queue (`pm_input.rs`); the window found extended by ADR-0058 (the deepest window, `WM_HITTEST`)
- **Date:** 2026-11-19
- **Sources:** `src/loader/pm_input.rs` (`InputEvent::Pointer`, `take_up_input`, `pointer_target`), `src/gui/sdl_renderer.rs` (`push_pointer`, `activate_on_arrival`); `samples/pm_inputorder_test`; IBM *Presentation Manager Programming Guide*, "Mouse and Keyboard Input" ("System Message Queue", "Capturing Mouse Input", "Mouse Messages"), as on EDM/2

## Context

Keys already went into one system input queue and were routed to the focus window when a thread read them. Pointer input was not:

- the GUI thread hit-tested a click when it arrived;
- if the click activated another frame or landed beside the focused control, the GUI thread changed activation and focus there and then;
- before doing so, it fixed the target of the keys still waiting (`pin_pending_input`).

So a click took effect before the input typed ahead of it had been handled. A program that moved its windows while busy had clicks delivered to the window that used to be under the pointer.

The PM Programming Guide describes PM differently:

- **Input order.** Input events are "strictly ordered so that a new event cannot be processed until all previous events are fully processed". The system "cannot determine the destination window of an input event until then".
- **Keys after a click.** In the Guide's example, a key typed after a click on another window goes there "only if that window became active as a result of the mouse click".
- **Pointer target.** Mouse messages go to "the window that is under the mouse pointer at the time the system reads the mouse input events from the system message queue", unless a window has the capture.
- **Disabled windows.** By default "disabled windows do not receive mouse messages".

## Decision

- **Pointer events.** The GUI thread queues a pointer event as `InputEvent::Pointer`: the message, the desktop point and mp2 (the `KC_*` flags). This covers `WM_MOUSEMOVE`, presses, double clicks and releases on a client area or dialog.
- **Taking up the head of the queue.** When such an event reaches the head of the queue and its thread looks for messages, `take_up_input` runs in `next_queued` before the posted messages are searched:
  1. **Hit test** (`pointer_target`), in this order:
     - the capture window;
     - for a move or a button-1 release, the built-in control held with button 1;
     - otherwise the frame under the pointer, unless that frame is disabled (then the event is dropped);
     - a button-1 message on that frame goes to the built-in control or embedded dialog under the pointer (`control_under`).
  2. **Effects.** A press that was not taken by the capture or a held control activates its frame: it goes to the top of the Z-order, and `WM_ACTIVATE` is posted FALSE to the old frame, then TRUE to the new one. A button-1 press beside the focused control of that frame takes the focus from it.
  3. **Conversion.** The event becomes an `InputEvent::Window` for its window.
- **Order of what the program reads.** The activation messages are posted, and posted messages rank above input, so the program reads `WM_ACTIVATE` before the click. Keys queued after the click resolve against the new focus.
- **Another thread's window.** An event whose window belongs to another queue waits for that queue's thread, as keys already did. `wake_input_reader` wakes that thread.
- **What stays on the GUI thread.** Clicks on frame chrome (title bar, buttons, system-menu icon), the sizing border and the VIO window are handled at once. These are the system's own frame controls, and the VIO window has no message queue.
  - These clicks still activate the frame immediately (`activate_on_arrival`).
  - Before that, `pin_pending_input` fixes the target of the keys typed before them. Pointer events in the queue are left alone, since they are hit-tested when read anyway.
  - Drags (`WM_BEGINDRAG`, PMDRAG) keep their arrival-time target.

## Consequences

- **Order.** A click is handled strictly after the input before it. Clicks reach the window under the pointer when they are read, so they follow windows a busy program has moved. `samples/pm_inputorder_test` fails on the old routing and passes now.
- **Disabled frames.** These take no pointer input at all, including moves: their events are dropped, where before only clicks were refused.
- **Click activation.** The activating `WM_ACTIVATE` now carries TRUE (1), as from `WinSetActiveWindow`; it was 2. It is also posted when no frame was active before.
- **Under the capture.** A button-1 press is no longer rerouted to a built-in control under the pointer, and nothing is activated. The capture window gets every pointer message, as the Guide describes.
- **Busy programs.** A click on a busy program's client area activates the frame only when the program reads it. A click on its title bar still activates the frame at once.
- **Peeking.** `WinPeekMsg` with `PM_NOREMOVE` also takes up a click at the head of the queue, so the activation happens once even if the click is only peeked.

### Unverified

- Whether PM activates at the moment it reads the click or while it processes it (the Guide's example allows both; the effects come before the click either way).
- Whether a frame title-bar click waits for the queue in PM too.
- The `WM_ACTIVATE` mp1 value for activation by a click.
- Whether `WM_HITTEST` / `CS_HITTEST` takes part. Warpine does not send `WM_HITTEST`.
