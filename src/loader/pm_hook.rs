// SPDX-License-Identifier: GPL-3.0-only

//! PM hooks: `WinSetHook` (928), `WinReleaseHook` (927) and
//! `WinCallMsgFilter` (905), and the chains of guest calls that run hooks
//! (and the window procedures of `WinBroadcastMsg(BMSG_SEND)`) before an
//! API returns.
//!
//! # Scope
//!
//! A hook is set for a message queue (`hmq` = `HMQ_CURRENT` or a queue
//! handle) or, with `hmq` = NULLHANDLE, for the "system". Warpine runs one
//! OS/2 process per host process (ADR-0020), so a system hook is a
//! *process* hook here: it sees the messages of every queue of this process
//! and nothing of other OS/2 programs. Queue hooks are called before
//! process hooks; within each, the hook set last is called first. Setting
//! the same procedure for the same hook again moves it to the front
//! (unverified: whether PM chains it twice).
//!
//! # Hooks that are called
//!
//! * `HK_INPUT` — `BOOL InputHook(HAB, PQMSG, ULONG fs)`: by `WinGetMsg` /
//!   `WinPeekMsg` once the message is in the caller's `QMSG`, with `fs` =
//!   `PM_REMOVE` / `PM_NOREMOVE`. The hook may change the `QMSG`. TRUE
//!   swallows the message: `WinGetMsg` (and `WinPeekMsg(PM_REMOVE)`) then
//!   take the next one — the call is run again —, while
//!   `WinPeekMsg(PM_NOREMOVE)` returns FALSE and leaves it in the queue
//!   (unverified). Accelerators are translated after the hooks, so they see
//!   the keystroke (unverified: PM's order).
//! * `HK_SENDMSG` — `VOID SendMsgHook(HAB, PSMHSTRUCT, BOOL fInterTask)`:
//!   by `WinSendMsg` (and `WinBroadcastMsg(BMSG_SEND)`) before the message
//!   reaches the window procedure; the hooks of the *receiving* window's
//!   queue (unverified: which queue's). `fInterTask` is TRUE when the
//!   window belongs to another thread's queue; `SMHSTRUCT.model` is
//!   `PM_MODEL_2X`.
//! * `HK_MSGFILTER` — `BOOL MsgFilterHook(HAB, PQMSG, ULONG msgf)`: by
//!   `WinCallMsgFilter`, which returns TRUE when a hook did, and by the
//!   modal loops for every message they take, after accelerators are
//!   translated and before it is dispatched — `MSGF_DIALOGBOX` in
//!   `WinDlgBox` / `WinProcessDlg` (and the standard file and font
//!   dialogs), `MSGF_MESSAGEBOX` in `WinMessageBox`. The `QMSG` lies on the
//!   stack below the loop's caller; the hooks may change it, and the loop
//!   dispatches what they leave there. A hook returning TRUE swallows the
//!   message: the loop takes the next one. Warpine has no guest-side
//!   tracking loop (frames are moved and sized by the GUI thread), so
//!   `MSGF_TRACK` is never passed.
//! * `HK_HELP` — `BOOL HelpHook(HAB, ULONG usMode, ULONG idTopic,
//!   ULONG idSubTopic, PRECTL prcPosition)`: by `WinDefWindowProc` for a
//!   `WM_HELP` that reaches a frame or a frame's client (`pm_message.rs`).
//!
//! Other hook types (`HK_JOURNALRECORD`, `HK_JOURNALPLAYBACK`, ...) are
//! accepted and kept, but Warpine never calls them.
//!
//! While a thread runs hook procedures, no hook is called for what they do
//! (a send hook that sends a message would otherwise call itself without
//! end; unverified what PM does).
//!
//! # Call chains
//!
//! Hooks are guest code, and the API that calls them must not return to its
//! caller before they ran. Such an API returns `ApiResult::CallChain` with
//! the [`CallStep`]s to make; `vcpu.rs` (`continue_call_chain`) makes them
//! one after the other under `FrameKind::CallChain` frames, each just below
//! the API caller's stack, and finishes with the chain's [`ChainThen`]:
//! return a value, or run the API again (`WinSendMsg` after its send hooks,
//! `WinGetMsg` after a hook swallowed the message).

use std::collections::{HashMap, HashSet, VecDeque};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::OS2Message;
use super::{ApiResult, Loader};

// ── Hook constants (<pmwin.h>, INCL_WINHOOKS) ────────────────────────────

pub const HK_SENDMSG: i32 = 0;
pub const HK_INPUT: i32 = 1;
pub const HK_MSGFILTER: i32 = 2;
pub const HK_JOURNALRECORD: i32 = 3;
pub const HK_JOURNALPLAYBACK: i32 = 4;
pub const HK_HELP: i32 = 5;
pub const HK_LOADER: i32 = 6;
pub const HK_REGISTERUSERMSG: i32 = 7;
pub const HK_MSGCONTROL: i32 = 8;
pub const HK_PLIST_ENTRY: i32 = 9;
pub const HK_PLIST_EXIT: i32 = 10;
pub const HK_FINDWORD: i32 = 11;
pub const HK_CODEPAGECHANGED: i32 = 12;
pub const HK_CALLHOOK: i32 = 13;
pub const HK_WINDOWDC: i32 = 15;
pub const HK_DESTROYWINDOW: i32 = 16;
pub const HK_CHECKMSGFILTER: i32 = 20;
pub const HK_MSGINPUT: i32 = 21;
pub const HK_LOCKUP: i32 = 23;
pub const HK_FLUSHBUF: i32 = 24;

/// Every hook type `<pmwin.h>` defines.
pub const KNOWN_HOOKS: [i32; 20] = [
    HK_SENDMSG, HK_INPUT, HK_MSGFILTER, HK_JOURNALRECORD, HK_JOURNALPLAYBACK, HK_HELP,
    HK_LOADER, HK_REGISTERUSERMSG, HK_MSGCONTROL, HK_PLIST_ENTRY, HK_PLIST_EXIT, HK_FINDWORD,
    HK_CODEPAGECHANGED, HK_CALLHOOK, HK_WINDOWDC, HK_DESTROYWINDOW, HK_CHECKMSGFILTER,
    HK_MSGINPUT, HK_LOCKUP, HK_FLUSHBUF,
];

/// `msgf` of a message-filter hook (`<pmwin.h>`): the modal loop of a
/// dialog (`WinDlgBox`, `WinProcessDlg`).
pub const MSGF_DIALOGBOX: u32 = 1;
/// `msgf`: the modal loop of `WinMessageBox`.
pub const MSGF_MESSAGEBOX: u32 = 2;

/// `hmq` of `WinSetHook` / `WinReleaseHook`: the calling thread's queue.
pub const HMQ_CURRENT: u32 = 1;
/// `SMHSTRUCT.model`: a 32-bit (OS/2 2.x) message.
pub const PM_MODEL_2X: u32 = 1;

/// `usMode` of a help hook: help asked for in a frame window.
pub const HLPM_FRAME: i32 = -1;
/// `usMode`: help asked for in a window (a message box).
pub const HLPM_WINDOW: i32 = -2;
/// `usMode`: help asked for on a menu item.
pub const HLPM_MENU: i32 = -3;

/// Most hooks one queue (or the process) can have at once.
pub const MAX_HOOKS_PER_SCOPE: usize = 64;
/// Bytes reserved below the API caller's stack for the structure a chained
/// call passes (`SMHSTRUCT`, `RECTL`).
pub const CHAIN_BLOCK_BYTES: u32 = 32;
/// Most arguments of a chained call.
pub const MAX_CHAIN_ARGS: usize = 6;

/// Scope key of the process ("system") hooks in [`HookTable`].
const PROCESS_SCOPE: u32 = 0;

/// The hooks of every queue and of the process, and per-thread state of
/// the calls that run them.
#[derive(Debug, Default)]
pub struct HookTable {
    /// Hooks by scope (a queue handle, or [`PROCESS_SCOPE`]): `(type,
    /// procedure)`, the one set last first.
    hooks: HashMap<u32, Vec<(i32, u32)>>,
    /// Threads running hook procedures now, with their nesting depth.
    hook_depth: HashMap<u32, u32>,
    /// Threads whose next `WinSendMsg` has had its send hooks called.
    send_hooks_done: HashSet<u32>,
    /// Threads running window procedures for messages that other threads'
    /// windows were sent (`WinInSendMsg`), with their nesting depth.
    in_send: HashMap<u32, u32>,
}

/// Whether `kind` is a hook type `<pmwin.h>` defines.
pub fn is_known_hook(kind: i32) -> bool {
    KNOWN_HOOKS.contains(&kind)
}

impl HookTable {
    /// `WinSetHook`: add hook `pfn` of type `kind` to `scope` (a queue, or
    /// 0 for the process). FALSE when the scope already has
    /// [`MAX_HOOKS_PER_SCOPE`] hooks.
    pub fn set(&mut self, scope: u32, kind: i32, pfn: u32) -> bool {
        let list = self.hooks.entry(scope).or_default();
        list.retain(|&e| e != (kind, pfn));
        if list.len() >= MAX_HOOKS_PER_SCOPE {
            return false;
        }
        list.insert(0, (kind, pfn));
        true
    }

    /// `WinReleaseHook`: remove hook `pfn` of type `kind` from `scope`.
    /// FALSE when it was not set.
    pub fn release(&mut self, scope: u32, kind: i32, pfn: u32) -> bool {
        let Some(list) = self.hooks.get_mut(&scope) else { return false };
        let before = list.len();
        list.retain(|&e| e != (kind, pfn));
        let removed = list.len() != before;
        if list.is_empty() {
            self.hooks.remove(&scope);
        }
        removed
    }

    /// The procedures of the `kind` hooks for queue `hmq` in calling order:
    /// the queue's (set last first), then the process'.
    pub fn procs(&self, hmq: u32, kind: i32) -> Vec<u32> {
        let of = |scope: u32| self.hooks.get(&scope).into_iter().flatten()
            .filter(move |e| e.0 == kind).map(|e| e.1);
        let queue = (hmq != PROCESS_SCOPE).then(|| of(hmq)).into_iter().flatten();
        queue.chain(of(PROCESS_SCOPE)).collect()
    }

    /// Forget the hooks of queue `hmq` (destroyed).
    pub fn drop_queue(&mut self, hmq: u32) {
        if hmq != PROCESS_SCOPE {
            self.hooks.remove(&hmq);
        }
    }

    /// Whether thread `tid` is running hook procedures.
    pub fn in_hook(&self, tid: u32) -> bool {
        self.hook_depth.contains_key(&tid)
    }

    fn enter(map: &mut HashMap<u32, u32>, tid: u32) {
        *map.entry(tid).or_default() += 1;
    }

    fn leave(map: &mut HashMap<u32, u32>, tid: u32) {
        if let Some(n) = map.get_mut(&tid) {
            *n -= 1;
            if *n == 0 {
                map.remove(&tid);
            }
        }
    }

    pub fn enter_hook(&mut self, tid: u32) { Self::enter(&mut self.hook_depth, tid) }
    pub fn leave_hook(&mut self, tid: u32) { Self::leave(&mut self.hook_depth, tid) }

    /// `WinInSendMsg`: whether thread `tid` is processing a message sent
    /// to a window of another thread.
    pub fn in_send(&self, tid: u32) -> bool {
        self.in_send.contains_key(&tid)
    }
    pub fn enter_send(&mut self, tid: u32) { Self::enter(&mut self.in_send, tid) }
    pub fn leave_send(&mut self, tid: u32) { Self::leave(&mut self.in_send, tid) }

    /// Note that the send hooks of thread `tid`'s `WinSendMsg` ran.
    pub fn mark_send_hooks_done(&mut self, tid: u32) {
        self.send_hooks_done.insert(tid);
    }

    /// Whether the send hooks of thread `tid`'s `WinSendMsg` ran (the call
    /// is being made again); clears the note.
    pub fn take_send_hooks_done(&mut self, tid: u32) -> bool {
        self.send_hooks_done.remove(&tid)
    }

    /// Forget the per-thread state of thread `tid` (ended).
    pub fn forget_thread(&mut self, tid: u32) {
        self.hook_depth.remove(&tid);
        self.send_hooks_done.remove(&tid);
        self.in_send.remove(&tid);
    }
}

// ── Call chains ──────────────────────────────────────────────────────────

/// One call of a [`CallChain`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallStep {
    /// Call guest procedure `proc_addr` (`_System`) with `args`. A
    /// non-empty `block` is copied below the caller's stack and argument
    /// `block_arg` replaced by its address. With `stop_on_true`, a TRUE
    /// result ends the chain's calls ([`CallChain::stopped`]). `window`,
    /// when not 0, must still exist when the call is made (else it is
    /// skipped).
    Guest {
        proc_addr: u32,
        args: Vec<u32>,
        block: Vec<u32>,
        block_arg: usize,
        stop_on_true: bool,
        window: u32,
    },
    /// A message for a window without a guest procedure: handled on the
    /// host (`dispatch_builtin_control`).
    Builtin { hwnd: u32, msg: u32, mp1: u32, mp2: u32 },
}

impl CallStep {
    /// A hook call of `proc_addr` with `args` (which stop the chain on TRUE
    /// when `stop_on_true`).
    pub fn hook(proc_addr: u32, args: Vec<u32>, stop_on_true: bool) -> Self {
        CallStep::Guest { proc_addr, args, block: Vec::new(), block_arg: 0, stop_on_true, window: 0 }
    }

    /// A hook call passing `block` as argument `block_arg`.
    pub fn hook_with_block(proc_addr: u32, args: Vec<u32>, block: Vec<u32>, block_arg: usize,
                           stop_on_true: bool) -> Self {
        CallStep::Guest { proc_addr, args, block, block_arg, stop_on_true, window: 0 }
    }

    /// Message `msg` sent to `hwnd`: its procedure `wnd_proc`, or the
    /// built-in handling when that is 0.
    pub fn send(wnd_proc: u32, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Self {
        if wnd_proc == 0 {
            CallStep::Builtin { hwnd, msg, mp1, mp2 }
        } else {
            CallStep::Guest {
                proc_addr: wnd_proc, args: vec![hwnd, msg, mp1, mp2], block: Vec::new(),
                block_arg: 0, stop_on_true: false, window: hwnd,
            }
        }
    }
}

/// What a [`CallChain`] does once its calls are made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainThen {
    /// Return this value to the API's caller.
    Return(u32),
    /// Return the result of the last call made.
    ReturnLast,
    /// Return TRUE when a `stop_on_true` call returned TRUE.
    ReturnStopped,
    /// `WinSendMsg` after its send hooks: make the call again, without the
    /// hooks this time.
    SendAgain,
    /// `WinGetMsg` / `WinPeekMsg` (`ordinal`) after the input hooks, the
    /// message in the `QMSG` at `pqmsg`.
    Message { ordinal: u32, pqmsg: u32, remove: bool, peek: bool },
    /// The message-filter hooks of a modal loop saw the message in the
    /// `QMSG` at `pqmsg`: dispatch it (unless swallowed) and go on with
    /// the loop.
    ModalLoop { dlg_proc: u32, hwnd_dlg: u32, hmq: u32, pqmsg: u32 },
    /// Messages a standard dialog `hwnd` sent to its dialog procedure
    /// (`FDM_*`, `FNTM_*`; `pm_stddlg.rs`): act on the answers, free the
    /// strings at `buf`, and return `ret` (the default procedure's result).
    StdDialog { hwnd: u32, buf: u32, ret: u32 },
    /// A comparison of container `hwnd`'s sort `job` (`pm_container.rs`):
    /// feed the answer to the sort and ask the next one, or return `ret`
    /// (`keep`: also when the sort could not be applied).
    ContainerSort { hwnd: u32, job: u32, ret: u32, keep: bool },
    /// Container `hwnd`'s filter `job` asked its function about every
    /// record: mark the ones it rejected and return TRUE.
    ContainerFilter { hwnd: u32, job: u32 },
    /// `WinDefDlgProc` asked the application's controls of dialog `dlg`
    /// `WM_MATCHMNEMONIC` (`pm_dialog.rs`): the one that said TRUE takes
    /// the focus; returns whether one did.
    DialogMnemonic { dlg: u32 },
    /// A step of `DrgDrag`'s conversation (`pm_drag.rs`, `DRAG_STAGE_*`):
    /// record the `DM_DRAGOVER` reply and wait for the next input, or end.
    Drag { stage: u8 },
    /// A message another program sent (`pm_remote.rs`): answer call `id`
    /// with the window procedure's result, then go on as `after` says.
    AnswerSent { id: u64, after: super::pm_remote::AfterSent },
}

/// Guest calls an API makes before it returns (see the module comment).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CallChain {
    /// The thread making the calls.
    pub tid: u32,
    /// The calls still to make.
    pub steps: VecDeque<CallStep>,
    /// Result of the last call made.
    pub last: u32,
    /// Results of the calls made, in order.
    pub results: Vec<u32>,
    /// Whether a `stop_on_true` call returned TRUE.
    pub stopped: bool,
    /// The calls are hook calls: no hooks run for what they do.
    pub hooks: bool,
    /// The calls deliver a message sent to another thread's window
    /// (`WinInSendMsg`).
    pub inter_thread_send: bool,
    pub then: ChainThen,
}

/// How a finished [`CallChain`] goes on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ChainEnd {
    /// Return this value to the caller.
    Return(u32),
    /// Run PMWIN API `ordinal` again (its arguments are still on the stack).
    Rerun(u32),
    /// Back to the modal loop of dialog `hwnd_dlg`: dispatch `msg` (None
    /// when a filter hook swallowed it), then take the next message.
    ModalLoop { dlg_proc: u32, hwnd_dlg: u32, hmq: u32, msg: Option<OS2Message> },
    /// Go on with another chain (a container sort's next comparison).
    Next(Box<CallChain>),
}

/// PMWIN ordinal of `WinSendMsg`.
pub(crate) const ORD_WIN_SEND_MSG: u32 = 920;

impl CallChain {
    pub fn new(tid: u32, steps: Vec<CallStep>, hooks: bool, then: ChainThen) -> Self {
        CallChain { tid, steps: steps.into(), last: 0, results: Vec::new(), stopped: false, hooks, inter_thread_send: false, then }
    }

    /// A call returned `result`; `stops` = it was a `stop_on_true` call.
    pub fn returned(&mut self, result: u32, stops: bool) {
        self.last = result;
        self.results.push(result);
        if stops && result != 0 {
            self.stopped = true;
            self.steps.clear();
        }
    }
}

impl Loader {
    /// Start `chain`: note what the thread is doing (hooks, an
    /// inter-thread send) and return it as the API's result.
    pub(crate) fn start_chain(&self, chain: CallChain) -> ApiResult {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if chain.hooks { wm.hooks.enter_hook(chain.tid); }
        if chain.inter_thread_send { wm.hooks.enter_send(chain.tid); }
        drop(wm);
        ApiResult::CallChain(Box::new(chain))
    }

    /// The calls of `chain` are made: undo what [`Self::start_chain`] noted
    /// and say how the API ends.
    pub(crate) fn finish_chain(&self, chain: &CallChain) -> ChainEnd {
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if chain.hooks { wm.hooks.leave_hook(chain.tid); }
            if chain.inter_thread_send { wm.hooks.leave_send(chain.tid); }
            if chain.then == ChainThen::SendAgain {
                wm.hooks.mark_send_hooks_done(chain.tid);
            }
        }
        match chain.then {
            ChainThen::Return(v) => ChainEnd::Return(v),
            ChainThen::ReturnLast => ChainEnd::Return(chain.last),
            ChainThen::ReturnStopped => ChainEnd::Return(chain.stopped as u32),
            ChainThen::SendAgain => ChainEnd::Rerun(ORD_WIN_SEND_MSG),
            ChainThen::Message { ordinal, pqmsg, remove, peek } => {
                if chain.stopped {
                    // Swallowed: take the next message — except a peek
                    // that leaves messages in the queue, which would find
                    // the same one again.
                    return if peek && !remove { ChainEnd::Return(0) } else { ChainEnd::Rerun(ordinal) };
                }
                let msg = self.finish_hooked_message(pqmsg);
                ChainEnd::Return(if peek { 1 } else { (msg != WM_QUIT) as u32 })
            }
            ChainThen::ContainerSort { hwnd, job, ret, keep } => self.container_sort_step(hwnd, job, ret, keep, chain.last),
            ChainThen::ContainerFilter { hwnd, job } => self.container_filter_done(hwnd, job, &chain.results),
            ChainThen::DialogMnemonic { dlg } => self.dialog_mnemonic_done(dlg, chain),
            ChainThen::Drag { stage } => self.drag_chain_done(stage, chain.last),
            ChainThen::AnswerSent { id, after } => self.sent_done(id, after, chain),
            ChainThen::StdDialog { hwnd, buf, ret } => {
                self.std_chain_done(hwnd, buf, &chain.results);
                ChainEnd::Return(ret)
            }
            ChainThen::ModalLoop { dlg_proc, hwnd_dlg, hmq, pqmsg } => {
                let msg = (!chain.stopped).then(|| self.read_qmsg(pqmsg));
                ChainEnd::ModalLoop { dlg_proc, hwnd_dlg, hmq, msg }
            }
        }
    }

    /// The `QMSG` at `pqmsg` (fields that cannot be read are 0).
    fn read_qmsg(&self, pqmsg: u32) -> OS2Message {
        let rd = |o: u32| self.guest_read::<u32>(pqmsg.wrapping_add(o)).unwrap_or(0);
        OS2Message {
            hwnd: rd(QMSG_HWND), msg: rd(QMSG_MSG), mp1: rd(QMSG_MP1), mp2: rd(QMSG_MP2),
            time: rd(QMSG_TIME), x: rd(QMSG_PTL_X) as i32, y: rd(QMSG_PTL_Y) as i32,
        }
    }

    /// The message-filter hooks a modal loop of dialog `hwnd_dlg` (queue
    /// `hmq`) calls for `msg`: a chain passing the message in a `QMSG`
    /// written to `pqmsg` (free guest stack below the loop's caller), with
    /// `MSGF_MESSAGEBOX` for a message box and `MSGF_DIALOGBOX` otherwise.
    /// None when no hook is set (or the queue's thread runs hooks now).
    pub(crate) fn modal_filter_chain(&self, dlg_proc: u32, hwnd_dlg: u32, hmq: u32, msg: &OS2Message,
                                     pqmsg: u32) -> Option<Box<CallChain>> {
        let (tid, msgf) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let tid = wm.queue_thread(hmq)?;
            let message_box = wm.get_window(hwnd_dlg).is_some_and(|w| w.message_box);
            (tid, if message_box { MSGF_MESSAGEBOX } else { MSGF_DIALOGBOX })
        };
        let procs = self.hooks_to_call(tid, hmq, HK_MSGFILTER);
        if procs.is_empty() {
            return None;
        }
        let fields = [(QMSG_HWND, msg.hwnd), (QMSG_MSG, msg.msg), (QMSG_MP1, msg.mp1), (QMSG_MP2, msg.mp2),
                      (QMSG_TIME, msg.time), (QMSG_PTL_X, msg.x as u32), (QMSG_PTL_Y, msg.y as u32)];
        for (o, v) in fields {
            self.guest_write::<u32>(pqmsg.wrapping_add(o), v)?;
        }
        let steps = procs.into_iter().map(|p| CallStep::hook(p, vec![MOCK_HAB, pqmsg, msgf], true)).collect();
        let then = ChainThen::ModalLoop { dlg_proc, hwnd_dlg, hmq, pqmsg };
        match self.start_chain(CallChain::new(tid, steps, true, then)) {
            ApiResult::CallChain(chain) => Some(chain),
            _ => None,
        }
    }

    /// The input hooks let the message in the `QMSG` at `pqmsg` through:
    /// translate an accelerator keystroke in it, as `WinGetMsg` does, and
    /// return its message number.
    fn finish_hooked_message(&self, pqmsg: u32) -> u32 {
        let mut msg = self.read_qmsg(pqmsg);
        let before = (msg.hwnd, msg.msg, msg.mp1, msg.mp2);
        self.translate_queued_accelerator(&mut msg);
        if (msg.hwnd, msg.msg, msg.mp1, msg.mp2) != before {
            for (o, v) in [(QMSG_HWND, msg.hwnd), (QMSG_MSG, msg.msg), (QMSG_MP1, msg.mp1), (QMSG_MP2, msg.mp2)] {
                self.guest_write::<u32>(pqmsg.wrapping_add(o), v);
            }
        }
        msg.msg
    }

    /// The `kind` hooks to call for queue `hmq` on thread `tid`: none while
    /// the thread runs hooks already.
    pub(crate) fn hooks_to_call(&self, tid: u32, hmq: u32, kind: i32) -> Vec<u32> {
        let wm = self.shared.window_mgr.lock_or_recover();
        if wm.hooks.in_hook(tid) { Vec::new() } else { wm.hooks.procs(hmq, kind) }
    }

    /// The input hooks `WinGetMsg` / `WinPeekMsg` of thread `tid` call for
    /// a message written to `pqmsg`: none without a `QMSG` to pass them.
    pub(crate) fn input_hooks(&self, tid: u32, pqmsg: u32) -> Vec<u32> {
        let hmq = self.shared.window_mgr.lock_or_recover().thread_queue(tid);
        match hmq {
            Some(hmq) if pqmsg != 0 => self.hooks_to_call(tid, hmq, HK_INPUT),
            _ => Vec::new(),
        }
    }

    /// The queue scope `hmq` of `WinSetHook` / `WinReleaseHook` stands for
    /// on thread `tid`: `HMQ_CURRENT` = its queue, NULLHANDLE = the
    /// process, else a live queue. `Err(PMERR_*)` otherwise.
    fn hook_scope(&self, tid: u32, hmq: u32) -> Result<u32, u32> {
        let wm = self.shared.window_mgr.lock_or_recover();
        match hmq {
            HMQ_CURRENT => wm.thread_queue(tid).ok_or(PMERR_NO_MSG_QUEUE),
            PROCESS_SCOPE => Ok(PROCESS_SCOPE),
            h if wm.get_mq(h).is_some() => Ok(h),
            _ => Err(PMERR_INVALID_HMQ),
        }
    }

    /// `WinSetHook(hab, hmq, iHook, pfnHook, hmod)` (928). `hmod` (the DLL
    /// holding a system hook) is not needed: `pfnHook` is an address in
    /// this process either way.
    pub(crate) fn win_set_hook(&self, tid: u32, hmq: u32, kind: i32, pfn: u32) -> ApiResult {
        let scope = match self.hook_scope(tid, hmq) {
            Ok(s) => s,
            Err(code) => return self.pm_fail(tid, code),
        };
        // unverified: the error codes for an unknown hook type or a null
        // procedure.
        if !is_known_hook(kind) || pfn == 0 {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        if !self.shared.window_mgr.lock_or_recover().hooks.set(scope, kind, pfn) {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        ApiResult::Normal(1)
    }

    /// `WinReleaseHook(hab, hmq, iHook, pfnHook, hmod)` (927): FALSE when
    /// the hook is not set (unverified: the error code).
    pub(crate) fn win_release_hook(&self, tid: u32, hmq: u32, kind: i32, pfn: u32) -> ApiResult {
        let scope = match self.hook_scope(tid, hmq) {
            Ok(s) => s,
            Err(code) => return self.pm_fail(tid, code),
        };
        if !self.shared.window_mgr.lock_or_recover().hooks.release(scope, kind, pfn) {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        ApiResult::Normal(1)
    }

    /// `WinCallMsgFilter(hab, pqmsg, msgf)` (905): call the message-filter
    /// hooks of the caller's queue with `pqmsg` and `msgf`; TRUE when one
    /// returned TRUE (the others after it are not called), else FALSE.
    pub(crate) fn win_call_msg_filter(&self, tid: u32, pqmsg: u32, msgf: u32) -> ApiResult {
        let Some(hmq) = self.shared.window_mgr.lock_or_recover().thread_queue(tid) else {
            return self.pm_fail(tid, PMERR_NO_MSG_QUEUE);
        };
        let procs = self.hooks_to_call(tid, hmq, HK_MSGFILTER);
        if procs.is_empty() {
            return ApiResult::Normal(0);
        }
        let steps = procs.into_iter().map(|p| CallStep::hook(p, vec![MOCK_HAB, pqmsg, msgf], true)).collect();
        self.start_chain(CallChain::new(tid, steps, true, ChainThen::ReturnStopped))
    }

    /// `WinGetMsg` (`peek` false) / `WinPeekMsg` (`ordinal`) found a
    /// message and wrote it, untranslated, to `pqmsg`: call the input
    /// hooks `procs` of the caller's queue with it (see the module
    /// comment).
    pub(crate) fn input_hook_chain(&self, tid: u32, procs: Vec<u32>, ordinal: u32, pqmsg: u32,
                                   fl: u32, peek: bool) -> ApiResult {
        let steps = procs.into_iter().map(|p| CallStep::hook(p, vec![MOCK_HAB, pqmsg, fl], true)).collect();
        let then = ChainThen::Message { ordinal, pqmsg, remove: fl & PM_REMOVE != 0, peek };
        self.start_chain(CallChain::new(tid, steps, true, then))
    }

    /// The send-hook calls for message `[hwnd, msg, mp1, mp2]` sent to a
    /// window of queue `hmq` by thread `tid`: an `SMHSTRUCT` for each hook
    /// procedure.
    pub(crate) fn send_hook_steps(&self, tid: u32, hmq: u32, sent: [u32; 4], inter_thread: bool)
        -> Vec<CallStep>
    {
        let [hwnd, msg, mp1, mp2] = sent;
        self.hooks_to_call(tid, hmq, HK_SENDMSG).into_iter().map(|p| {
            // SMHSTRUCT: mp2, mp1, msg, hwnd, model.
            let smh = vec![mp2, mp1, msg, hwnd, PM_MODEL_2X];
            CallStep::hook_with_block(p, vec![MOCK_HAB, 0, inter_thread as u32], smh, 1, false)
        }).collect()
    }
}

/// Byte offsets of the `QMSG` fields.
pub const QMSG_HWND: u32 = 0;
pub const QMSG_MSG: u32 = 4;
pub const QMSG_MP1: u32 = 8;
pub const QMSG_MP2: u32 = 12;
pub const QMSG_TIME: u32 = 16;
pub const QMSG_PTL_X: u32 = 20;
pub const QMSG_PTL_Y: u32 = 24;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_are_called_queue_first_and_last_set_first() {
        let mut t = HookTable::default();
        assert!(t.set(0x3000, HK_INPUT, 0x100));
        assert!(t.set(0x3000, HK_INPUT, 0x200));
        assert!(t.set(PROCESS_SCOPE, HK_INPUT, 0x300));
        assert!(t.set(0x3000, HK_SENDMSG, 0x400));
        assert!(t.set(0x3001, HK_INPUT, 0x500));
        assert_eq!(t.procs(0x3000, HK_INPUT), vec![0x200, 0x100, 0x300]);
        assert_eq!(t.procs(0x3001, HK_INPUT), vec![0x500, 0x300]);
        assert_eq!(t.procs(0x3000, HK_SENDMSG), vec![0x400]);
        assert_eq!(t.procs(0x3000, HK_HELP), Vec::<u32>::new());
    }

    #[test]
    fn setting_a_hook_again_moves_it_to_the_front() {
        let mut t = HookTable::default();
        t.set(0x3000, HK_INPUT, 0x100);
        t.set(0x3000, HK_INPUT, 0x200);
        t.set(0x3000, HK_INPUT, 0x100);
        assert_eq!(t.procs(0x3000, HK_INPUT), vec![0x100, 0x200]);
    }

    #[test]
    fn release_removes_only_the_matching_hook() {
        let mut t = HookTable::default();
        t.set(0x3000, HK_INPUT, 0x100);
        t.set(0x3000, HK_MSGFILTER, 0x100);
        assert!(!t.release(0x3000, HK_INPUT, 0x999), "not set");
        assert!(!t.release(0x3001, HK_INPUT, 0x100), "other queue");
        assert!(t.release(0x3000, HK_INPUT, 0x100));
        assert!(!t.release(0x3000, HK_INPUT, 0x100), "released once");
        assert_eq!(t.procs(0x3000, HK_MSGFILTER), vec![0x100]);
    }

    #[test]
    fn a_scope_holds_a_bounded_number_of_hooks() {
        let mut t = HookTable::default();
        for p in 0..MAX_HOOKS_PER_SCOPE as u32 {
            assert!(t.set(0x3000, HK_INPUT, 0x1000 + p));
        }
        assert!(!t.set(0x3000, HK_INPUT, 0x9999));
        assert!(t.set(0x3000, HK_INPUT, 0x1000), "an existing one can move to the front");
    }

    #[test]
    fn a_destroyed_queue_loses_its_hooks_but_not_the_process_hooks() {
        let mut t = HookTable::default();
        t.set(0x3000, HK_INPUT, 0x100);
        t.set(PROCESS_SCOPE, HK_INPUT, 0x300);
        t.drop_queue(0x3000);
        t.drop_queue(PROCESS_SCOPE);
        assert_eq!(t.procs(0x3000, HK_INPUT), vec![0x300]);
    }

    #[test]
    fn hook_and_send_depths_nest() {
        let mut t = HookTable::default();
        t.enter_hook(7);
        t.enter_hook(7);
        t.leave_hook(7);
        assert!(t.in_hook(7));
        t.leave_hook(7);
        assert!(!t.in_hook(7));
        t.leave_hook(7); // unbalanced: no underflow
        assert!(!t.in_hook(7));
        t.enter_send(3);
        assert!(t.in_send(3) && !t.in_send(4));
        t.leave_send(3);
        assert!(!t.in_send(3));
        t.mark_send_hooks_done(5);
        assert!(t.take_send_hooks_done(5));
        assert!(!t.take_send_hooks_done(5), "taken once");
        t.enter_send(9);
        t.forget_thread(9);
        assert!(!t.in_send(9));
    }

    #[test]
    fn only_the_documented_hook_types_are_known() {
        assert!(is_known_hook(HK_INPUT) && is_known_hook(HK_FLUSHBUF));
        assert!(!is_known_hook(14) && !is_known_hook(-1) && !is_known_hook(25));
    }

    #[test]
    fn a_true_result_of_a_stopping_call_ends_the_chain() {
        let steps = vec![CallStep::hook(0x100, vec![1], true), CallStep::hook(0x200, vec![1], true)];
        let mut c = CallChain::new(1, steps, true, ChainThen::ReturnStopped);
        c.steps.pop_front();
        c.returned(0, true);
        assert!(!c.stopped && c.steps.len() == 1);
        c.returned(1, false);
        assert!(!c.stopped, "a call that does not stop the chain");
        c.returned(5, true);
        assert!(c.stopped && c.steps.is_empty() && c.last == 5);
    }

    #[test]
    fn a_send_to_a_window_without_procedure_is_built_in() {
        assert_eq!(CallStep::send(0, 9, WM_USER, 1, 2), CallStep::Builtin { hwnd: 9, msg: WM_USER, mp1: 1, mp2: 2 });
        let CallStep::Guest { args, window, .. } = CallStep::send(0x5000, 9, WM_USER, 1, 2) else { panic!() };
        assert_eq!((args, window), (vec![9, WM_USER, 1, 2], 9));
    }
}
