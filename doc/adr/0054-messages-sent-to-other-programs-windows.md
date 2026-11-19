# ADR-0054: Messages sent to another program's window run on the window's thread when it reads messages

- **Status:** Accepted; completes ADR-0052's phase B with ADR-0053; its send-hook choice amended by ADR-0055
- **Date:** 2026-11-19
- **Sources:**
  - Both sides of a sent message: `src/loader/pm_remote.rs` (`SentMessage`, `AfterSent`, `RemoteSends`, `remote_send`, `run_sent`, `sent_done`)
  - Where a thread runs them: `src/loader/pm_win.rs` (`next_message` → `Got::Sent`), `src/loader/pm_message.rs` (`WinWaitMsg`), `src/loader/vcpu.rs` (modal loop, thread end), `src/loader/pm_hook.rs` (`ChainThen::AnswerSent`)
  - Link and server: `src/desktop/client.rs` (`begin_call`, `poll_call`, `wait_call`, `answer`, `CallWaker`), `src/desktop/calls.rs` (calls that do not expire), `src/desktop/proto.rs` (`WinCall::Send`, `WinCall::SetText`)
  - Sample `samples/pm_xproc_test`
  - Earlier decisions: ADR-0026 (call chains), ADR-0052, ADR-0053
  - IBM *PM Reference*: `WinSendMsg`, `WinInSendMsg`, `WinGetMsg`, `WinPeekMsg`, `WinWaitMsg`

## Context

ADR-0053 made the calls on another program's windows that the window manager can answer. `WinSendMsg` is different: the window procedure is guest code in the other program's address space, so only a thread of that program can run it.

Within one program Warpine runs a window procedure on the sending thread (ADR-0026). Between programs that is impossible.

PM runs a message sent from another thread on the thread of the window's queue, when that thread reads or waits for messages. The sender waits, and meanwhile handles the messages sent to its own queue. Otherwise two threads sending to each other would deadlock.

## Decision

### The receiving program

- **Queued for the window's thread.** The link reader thread does not answer a `Send` call. It queues a `SentMessage` in the message queue of the window (`PM_MsgQueue::sent`, at most 64) and wakes the queue's thread. The answer comes later through `DesktopLink::answer`.
- **Where the thread runs it.** The thread runs a sent message wherever it reads or waits for messages, before any queued message:
  - `WinGetMsg`, `WinPeekMsg` and `WinWaitMsg`: `next_message` returns `Got::Sent`;
  - a modal loop;
  - the thread's own `WinSendMsg` to another program, while it waits.
- **How it runs.** A call chain (ADR-0026) calls the window procedure, or the built-in handling for a window without one. `WinInSendMsg` is TRUE meanwhile. `ChainThen::AnswerSent` then answers with the result and goes on as `AfterSent` says:
  - `Rerun`: the API is made again;
  - `ModalLoop`: the modal loop takes its next message;
  - `ResumeSend`: the thread goes back to waiting in its own send.
- **When the message cannot run.** A window gone before it runs gets `Failed`. A queue destroyed with messages waiting (`WinDestroyMsgQueue`, the thread ending) answers each of them `Failed`.

### The sending program

- **Starting the send.** `remote_send` starts the call with `DesktopLink::begin_call` and a waker that notifies the sender's queue.
- **Waiting.** It then waits on its queue's condition variable for either:
  - the answer, which it returns;
  - a message sent to its own queue, which it runs first (`AfterSent::ResumeSend`).
- **Resuming the same send.** Running a message ends by making `WinSendMsg` again. `RemoteSends` keeps each thread's sends in progress, innermost last. The `resume` mark makes the repeated call wait for the same answer. A send made by a procedure the thread runs is a new, nested one.
- **No time limit.** The sender waits as long as the window procedure runs, as in PM.
  - The server does not expire `Send` calls; `MAX_PENDING_CALLS` still bounds them.
  - The wait ends when the answer comes, when the link is lost or the receiving program ends (the server refuses its calls), or when the sending process ends.

### Other calls that send messages

- **`WinSetWindowText`** is a `SetText` call, answered from the window manager like the calls of ADR-0053. Within one program Warpine sets the text without sending `WM_SETWINDOWPARAMS` either.
- **`WinDestroyWindow`** on another program's window fails, as it does on another thread's window.

### Warpine's choices (unverified against OS/2)

- A failed send returns 0 with `PMERR_INVALID_HWND`.
- Send hooks (`HK_SENDMSG`) do not run for a send to another program. Amended by ADR-0055: the receiving thread runs its queue's send hooks.
- Message parameters pass as they are: a pointer means something to the receiver only in shared memory.
- `WinSetWindowText` text is read in the calling thread's codepage.

## Consequences

- **What works.** A program can send to another program's windows and get the window procedure's result. Two programs can send to each other while each waits: `pm_xproc_test` checks a send, a send answered while the receiver sends back to the waiting sender, `WinInSendMsg` on both sides, `WinSetWindowText`, and a send to an ended program.
- **Like PM, also in its faults.** A receiver that never reads its queue holds its senders until it does, or until it ends.
- **The same path for all waits.** Every place a PM thread waits for messages now also runs messages sent from other programs. A new kind of wait must do the same, or sends to its thread's windows wait until it reads messages again.
- **Two models.** Within a program a sent message still runs on the sending thread (ADR-0026). Only sends between programs use the window's thread.
- **Still missing then**, done since by ADR-0055:
  - `WinBroadcastMsg` to other programs' windows;
  - `WM_MINMAXFRAME` for a state change another program makes;
  - enumerating the desktop's windows across programs.
