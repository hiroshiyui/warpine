# ADR-0026: Run hooks and broadcast sends as call chains that may re-run the API, and scope system hooks to the process

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** batch 4 (PM messaging and hooks, painting styles); `src/loader/pm_hook.rs` (`CallChain`, `CallStep`, `ChainThen`), `src/loader/pm_message.rs`, `src/loader/vcpu.rs` (`continue_call_chain`, `FrameKind::CallChain`), `src/loader/pm_paint_style.rs`, `src/loader/pm_owner.rs`; IBM *Presentation Manager Programming Reference* (`WinSetHook`, `WinBroadcastMsg`, `WM_ERASEBACKGROUND`, `WS_SYNCPAINT`); ADR-0020, ADR-0025

## Context

Several PM calls must run guest code before they return to their caller:

- **Hooks** (`WinSetHook`): an input hook sees each message `WinGetMsg` / `WinPeekMsg` is about to return and may change or swallow it. A send hook sees each `WinSendMsg` before the window procedure does. `WinCallMsgFilter` returns what the message-filter hooks answered.
- **`WinBroadcastMsg(BMSG_SEND)`** calls the window procedure of every matching window in turn.
- **`WM_ERASEBACKGROUND`** and **`WS_SYNCPAINT`** windows get their message during the painting or invalidating call.

Warpine's APIs are host code running on the calling thread's vCPU loop. Guest code can run only when the vCPU re-enters the guest. ADR-0025 solved one case, owner drawing: host painting queues `OwnerCall`s, which run when the API returns. Hooks are harder in two ways. Their result decides what the API itself does: a swallowed message means fetching the next one, and a send must run its hooks before it reaches the procedure. And there may be any number of them, in a defined order.

OS/2 also has system-wide hooks (`hmq` = NULLHANDLE), which see the messages of every PM program.

## Decision

- **Call chains.** An API that has to run guest code before returning gives back `ApiResult::CallChain`: an ordered list of guest calls (`CallStep`s) and what to do afterwards (`ChainThen`). The vCPU (`continue_call_chain`) makes the calls one at a time, under `FrameKind::CallChain` frames just below the API caller's stack. It then either:
  - returns a value to the caller (`WinCallMsgFilter`, broadcasts); or
  - **re-runs the API** from its entry point, with its arguments still on the guest stack (`WinSendMsg` after its send hooks; `WinGetMsg` / `WinPeekMsg(PM_REMOVE)` after a hook swallowed the message, which fetches the next one).
  
  Each API keeps its logic in one place and stays restartable, instead of being split into continuation states.
- **No hooks inside hooks.** While a thread runs hook procedures, nothing they do calls hooks again. This bounds recursion, for example a send hook that sends.
- **Erase and sync-paint calls reuse the owner-call queue** of ADR-0025 (`OwnerCallKind::EraseBackground`, `SyncPaint`). They run when the API returns to the window's thread, before its result reaches the caller.
- **A system hook is a process hook.** One OS/2 program runs per Warpine host process (ADR-0020), so `hmq` = NULLHANDLE hooks every queue of this process and nothing of other OS/2 programs. Queue hooks are called before process hooks; within each set, the hook set last is called first.

## Consequences

- Hooks, broadcasts, `WM_ERASEBACKGROUND` and `WS_SYNCPAINT` run on the right thread before the API returns, as PM requires. The guest cannot tell host code from PM code.
- A re-run API must be idempotent up to the point where it hands back the chain. A re-run `WinGetMsg` must not lose or duplicate a message; tests check this.
- A program cannot hook another OS/2 program's messages, and journal hooks (`HK_JOURNALRECORD` / `PLAYBACK`) are stored but never called. Cross-process hooks would need a host IPC mechanism of their own.
- The modal loops (dialogs, message boxes, tracking) do not call `HK_MSGFILTER` yet: that needs a call chain inside `setup_dlg_dispatch` (`doc/TODOs.md`).
- There are now two ways to call guest code from inside an API: the owner-call queue (calls whose results only finish host drawing) and call chains (calls whose results steer the API). A new case should use whichever of the two fits, not a third mechanism.
