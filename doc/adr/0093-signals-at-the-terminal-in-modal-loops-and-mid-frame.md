# ADR-0093: Ctrl+Break at a terminal, signals in modal loops, pipe writes cut mid-frame

- **Status:** Accepted; extends ADR-0035
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *Control Program Programming Reference* and *Programming Guide*: "Signal Exceptions", `XCPT_SIGNAL`, `DosKillThread`, `DosWrite` and `DosTransactNPipe` on named pipes.
  - *PM Programming Reference*: `WinDlgBox`, `WinProcessDlg`.
  - The Linux man pages `signal(7)` (`SIGQUIT`) and `termios(3)` (`VQUIT`).
  - The code: `host_signal.rs`, `signals.rs` (`keyboard_byte_signal`, `signal_waits_for`), `vcpu.rs` (`setup_dlg_dispatch`, `leave_dlg_loop_for_signal`), `npipe.rs` (`send_frame_or_interrupt`, `flush_tail_in_background`), `npipe_api.rs` (`np_transact`).
  - The samples: `samples/kill_test` (modes `kbdbrk`, `brk`), `samples/pm_dlg_signal_test`, `samples/npipe_cut_test`.

## Context

ADR-0035 made the common blocking calls interruptible. Three gaps were left:

1. **Ctrl+Break at a terminal.** A terminal has no Break key. `SIGQUIT`, what Ctrl+\ sends when the terminal is not in raw mode, was not handled, so it ended Warpine with a core dump.
2. **Modal dialog loops.** `WinDlgBox`, `WinProcessDlg` and `WinMessageBox` run their message loop in Warpine, on thread 1's behalf. A signal waited until the dialog ended, and a dialog nobody dismisses never ends.
3. **Pipe writes blocked mid-frame.** A named-pipe message travels as one frame. A write blocked partway through could not stop, because cutting the frame would break the stream. Neither a signal nor `DosKillThread` could end it before the reader took the data. The write half of `DosTransactNPipe` could stop, but not its wait for the reply.

## Decision

1. **Ctrl+\ is Ctrl+Break at a terminal.**
   - `SIGQUIT` arrives as `XCPT_SIGNAL_BREAK`, as `SIGINT` arrives as `XCPT_SIGNAL_INTR`. Warpine now catches it, so it no longer dumps core.
   - In raw mode, the byte Ctrl+\ (0x1C) read by `KbdCharIn` / `KbdStringIn` or a console `DosRead` is the signal, not a character, as Ctrl+C already is.
2. **Modal loops leave for the signal.** Between messages, the loop checks whether thread 1 has a deliverable signal or a request to end (`Loader::signal_waits_for`). If so, it puts the guest where a message's window procedure would be about to return: at `CALLBACK_RET_TRAP` on the message-call stack, under a `DlgRunLoop` frame. The vCPU loop then hands the signal to the handlers with that point as its context.
   - **A handler continues:** the trap pops the frame and the loop goes on.
   - **No handler takes the signal:** the process ends as anywhere else, with its exit list.
   - **A held signal** (not yet acknowledged) does not leave the loop.
3. **A pipe write cut mid-frame finishes in the background.**
   - **Interrupted mid-frame:** when a signal or `DosKillThread` interrupts a write after part of a frame went out, the frame counts as written and the call returns. The unsent rest is kept as the connection's tail, under the send lock, and a helper thread (one per connection at a time) sends it.
   - **Order:** every later write sends the tail before its own frame, so the reader always gets whole frames in order.
   - **Byte streams:** a write stops after the chunk that was cut, which counts as written.
   - **Unchanged:** a write interrupted with nothing written still fails and is made again (ADR-0035). A process that is ending still gives up at once, since its connections close with it.
4. **`DosTransactNPipe` / `DosCallNPipe`.** Once the message is out, a signal or kill ends the wait for the reply with `ERROR_INTERRUPT`; the call is not made again.

## Warpine's choices (unverified)

- Ctrl+\ stands for Ctrl+Break at a terminal; programs never receive the character 0x1C there.
- A signal reaches a modal loop between messages, never during a dialog procedure's own call.
- A message write cut mid-frame reports the whole message as written.
- An interrupted reply wait in `DosTransactNPipe` fails with `ERROR_INTERRUPT`.
- The helper thread keeps sending the tail as long as the connection stays open, even after the program closed its handle.

## Consequences

- **Terminal users** can raise Ctrl+Break, and a stray Ctrl+\ no longer kills Warpine.
- **PM programs** get signals while a dialog is up: Ctrl+C, `DosKillProcess` and the end of the session reach a program waiting in `WinDlgBox`.
- **`DosKillThread`** ends a thread blocked in any pipe write, and pipe streams stay intact.
- **Still not interruptible** (in `doc/TODOs.md`):
  - writes to anonymous pipes;
  - sockets;
  - host files on FIFOs and devices (regular files block only briefly, and Linux does not interrupt them either);
  - `WinSendMsg` to another thread;
  - critical-section waits;
  - CMD.EXE's child waits;
  - 16-bit programs' calls.
- **Tests:**
  - `host_signal.rs` and `signals.rs`: the `SIGQUIT` mapping and the Ctrl+\ byte;
  - `vcpu.rs` `a_signal_leaves_the_modal_loop_for_its_handlers`;
  - `npipe.rs`: a message cut mid-frame arrives whole and before later ones, and a byte stream cut mid-chunk;
  - `tests/integration.rs`:
    - `test_ctrl_backslash_from_the_keyboard_is_ctrl_break`;
    - `test_sigquit_is_ctrl_break`;
    - `gui_pm_dlg_signal_test_signals_reach_a_modal_dialog` (7 checks; it fails without the modal-loop change);
    - `test_pipe_writes_interrupted_mid_message` (15 checks).
