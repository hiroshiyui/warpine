// SPDX-License-Identifier: GPL-3.0-or-later

//! Calls on other programs' windows (ADR-0052 phase B).
//!
//! On the session's desktop every window handle names its program's desktop
//! slot (`(slot + 1) << HWND_SLOT_SHIFT | local`). A PMWIN call on a handle
//! of another slot is made through the desktop server: the caller's API
//! thread sends a [`WinCall`] and waits for the answer (at most
//! `CALL_TIMEOUT`), and the program the window belongs to answers on its
//! link thread ([`Responder`]), from its window manager.
//!
//! | Call | Made as |
//! |---|---|
//! | `WinPostMsg` | `Post`: queued for the window |
//! | `WinIsWindow`, `WinQueryWindowProcess`, `WinQueryWindowPos`, `WinQueryWindowRect`, `WinQueryWindowText`, `WinQueryWindowTextLength`, `WinIsWindowVisible`, `WinIsWindowEnabled`, `WinQueryClassName` | `Describe`: one description of the window |
//! | `WinQueryWindowULong`, `WinQueryWindowUShort` | `Word` |
//! | `WinQueryWindow` | `Query` |
//! | `WinSetWindowPos` | `SetPos` |
//! | `WinShowWindow` | `Show` |
//! | `WinSetActiveWindow` | `Activate` |
//! | `WinSetWindowText` | `SetText` |
//! | `WinSendMsg` | `Send`: run by the window's thread (below) |
//! | `WinBroadcastMsg(HWND_DESKTOP, …)` | `Broadcast` to every other program |
//! | `WinBeginEnumWindows` | `Children`, and for the desktop `Clients` / `Stacking` (`pm_enum.rs`) |
//!
//! Other calls on another program's window fail with `PMERR_INVALID_HWND`
//! as before. `WinDestroyWindow` fails as it does for another thread's
//! window.
//!
//! # Sent messages
//!
//! A message sent to another program's window is run by the thread of the
//! window's queue, as PM does between threads, and the sender waits for the
//! window procedure's result:
//!
//! - The receiving program's link thread queues the message in the
//!   window's message queue ([`SentMessage`], `PM_MsgQueue::sent`) and
//!   wakes the queue's thread.
//! - That thread runs it when it next reads or waits for messages —
//!   `WinGetMsg`, `WinPeekMsg`, `WinWaitMsg`, a modal loop, or its own
//!   `WinSendMsg` to another program — before any queued message: a call
//!   chain calls the window procedure (`WinInSendMsg` is TRUE meanwhile),
//!   answers with its result ([`ChainThen::AnswerSent`]) and goes on with
//!   what the thread was doing ([`AfterSent`]).
//! - The sender waits in its `WinSendMsg` without a time limit, as in PM,
//!   and runs the messages sent to its own queue meanwhile, so two programs
//!   sending to each other do not deadlock. Each run ends by making the
//!   `WinSendMsg` again, which goes on waiting for the same answer
//!   ([`RemoteSends`]); a send made by a window procedure it runs is a new
//!   one, nested inside.
//! - A send fails (0, `PMERR_INVALID_HWND`) when the window is gone, its
//!   program ends, or its queue is destroyed before the message is run.
//! - The send hooks of the window's queue (and the program's system send
//!   hooks) run on that thread first, with `fInterTask` TRUE.
//!
//! `WinSetWindowPos` that minimizes, maximizes or restores another
//! program's frame goes the same way: the frame's thread asks its client
//! with `WM_MINMAXFRAME` (TRUE keeps the frame as it is) before the change,
//! and the caller waits for it as for a send (ADR-0055).
//!
//! A broadcast to the desktop reaches the windows of every program: each
//! other program posts it to its windows, or for `BMSG_SEND` queues it for
//! their threads to run, without the caller waiting for them (Warpine's
//! choice).
//!
//! Warpine's choices (no public source says how PM does these):
//! - A call that gets no answer — the window is gone, its program ended or
//!   does not answer in time — fails with `PMERR_INVALID_HWND`.
//! - Window text and class names come back in the caller's codepage (the
//!   calling thread's queue codepage).
//!
//! Posted message parameters are passed as they are: a pointer means
//! something in the other program only for shared memory, as in PM.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_hook::{CallChain, CallStep, ChainEnd, ChainThen, CHAIN_BLOCK_BYTES};
use super::pm_minmax::{minmax_swp, WinRect, SWP_STRUCT_SIZE};
use super::pm_drag::{DM_DRAGLEAVE, DM_DRAGOVER, DOR_NEVERDROP};
use super::pm_error::window_word_in_range;
use super::pm_types::{handle_in_slot, WindowManager};
use super::pm_win::bound_window_geometry;
use super::{ApiResult, Loader};
use crate::desktop::client::{CallState, CallWaker, Responder};
use crate::desktop::proto::{slot_address, ServerMsg, WinCall, WinReply, WindowInfo, MAX_COORD, MAX_HANDLES, SERVER_ADDRESS};

// The SWP of WM_MINMAXFRAME is passed in a chained call's block.
const _: () = assert!(SWP_STRUCT_SIZE <= CHAIN_BLOCK_BYTES);

/// `WinPostMsg`.
pub(super) const ORD_POST_MSG: u32 = 919;
/// `WinIsWindow(hab, hwnd)`.
const ORD_IS_WINDOW: u32 = 772;
const ORD_QUERY_WINDOW_PROCESS: u32 = 838;
const ORD_QUERY_WINDOW_POS: u32 = 837;
const ORD_QUERY_WINDOW_RECT: u32 = 840;
const ORD_QUERY_WINDOW_TEXT: u32 = 841;
const ORD_QUERY_WINDOW_TEXT_LENGTH: u32 = 842;
const ORD_IS_WINDOW_VISIBLE: u32 = 775;
const ORD_IS_WINDOW_ENABLED: u32 = 773;
const ORD_QUERY_WINDOW_ULONG: u32 = 843;
const ORD_QUERY_WINDOW_USHORT: u32 = 844;
const ORD_QUERY_WINDOW: u32 = 834;
const ORD_QUERY_CLASS_NAME: u32 = 805;
const ORD_SET_WINDOW_POS: u32 = 875;
const ORD_SHOW_WINDOW: u32 = 883;
/// `WinSetActiveWindow(hwndDesktop, hwnd)`.
const ORD_SET_ACTIVE_WINDOW: u32 = 851;
const ORD_SET_WINDOW_TEXT: u32 = 877;
/// `WinSendMsg`.
pub(super) const ORD_SEND_MSG: u32 = super::pm_hook::ORD_WIN_SEND_MSG;
/// `WinWaitMsg`.
pub(super) const ORD_WAIT_MSG: u32 = 897;

/// Most messages from other programs waiting in one queue.
pub const MAX_SENT_MESSAGES: usize = 64;
/// How long a sender waits at a time before it looks again (its queue's
/// condition variable or the link wakes it sooner).
const SEND_WAIT_SLICE: Duration = Duration::from_millis(100);

/// The PMWIN calls made on another program's window, with the stack offset
/// of their window handle.
const REMOTE_CALLS: [(u32, u64); 18] = [
    (ORD_POST_MSG, 4), (ORD_IS_WINDOW, 8), (ORD_QUERY_WINDOW_PROCESS, 4), (ORD_QUERY_WINDOW_POS, 4),
    (ORD_QUERY_WINDOW_RECT, 4), (ORD_QUERY_WINDOW_TEXT, 4), (ORD_QUERY_WINDOW_TEXT_LENGTH, 4),
    (ORD_IS_WINDOW_VISIBLE, 4), (ORD_IS_WINDOW_ENABLED, 4), (ORD_QUERY_WINDOW_ULONG, 4),
    (ORD_QUERY_WINDOW_USHORT, 4), (ORD_QUERY_WINDOW, 4), (ORD_QUERY_CLASS_NAME, 4), (ORD_SET_WINDOW_POS, 4),
    (ORD_SHOW_WINDOW, 4), (ORD_SET_ACTIVE_WINDOW, 8), (ORD_SET_WINDOW_TEXT, 4), (ORD_SEND_MSG, 4),
];

/// What another program asks a window's thread to do (module comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentKind {
    /// Run the window procedure for a sent message.
    Message { msg: u32, mp1: u32, mp2: u32 },
    /// Ask the frame's client `WM_MINMAXFRAME`, then change the frame's
    /// state (`op`) to `target` unless it answers TRUE.
    MinMax { op: u32, target: WinRect },
    /// A `DM_*` message of another program's drag (ADR-0057), the pointer
    /// at desktop point `(x, y)`: to the window, or for a container to its
    /// owner as `CN_*`.
    Drag { msg: u32, pdinfo: u32, x: i32, y: i32 },
    /// Host files dropped at desktop point `(x, y)` (ADR-0068): conversation
    /// step `stage` (`HOST_DROP_*`); the first makes the `DRAGINFO` of drop
    /// `drop`, the later ones carry it (`pdinfo`).
    HostDrop { drop: u32, x: i32, y: i32, stage: u8, pdinfo: u32 },
}

/// A message another program sent to one of this program's windows, waiting
/// for the window's thread (module comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentMessage {
    /// The server's id of the call, for the answer; `None` for a message
    /// nobody waits for (a broadcast).
    pub id: Option<u64>,
    pub hwnd: u32,
    pub kind: SentKind,
}

impl SentMessage {
    /// Message `msg` sent to `hwnd` by call `id`.
    pub fn message(id: Option<u64>, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> SentMessage {
        SentMessage { id, hwnd, kind: SentKind::Message { msg, mp1, mp2 } }
    }
}

/// How the answer to a [`SentMessage`] is made from the calls run for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SentFinish {
    /// The window procedure's result.
    Reply,
    /// Change frame `hwnd`'s state unless the client said TRUE.
    MinMax { op: u32, target: WinRect },
    /// The reply to drag message `msg`; a window that took no part never
    /// accepts.
    Drag { msg: u32 },
    /// Step `stage` of a host drop with `DRAGINFO` `pdinfo` at `(x, y)`.
    HostDrop { stage: u8, pdinfo: u32, x: i32, y: i32 },
}

/// What a thread does once it has run a [`SentMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterSent {
    /// Make PMWIN call `ordinal` again (`WinGetMsg`, `WinPeekMsg`,
    /// `WinWaitMsg`).
    Rerun(u32),
    /// Go on waiting in its own call `ordinal` to another program
    /// (`WinSendMsg`, `WinSetWindowPos`).
    Resume(u32),
    /// Go on with the modal loop of dialog `hwnd_dlg`.
    ModalLoop { dlg_proc: u32, hwnd_dlg: u32, hmq: u32 },
    /// Go on with the thread's drag (`DrgDrag`).
    Drag,
}

/// Calls this program's threads make on other programs' windows and wait
/// for without a time limit (sends, state changes, DDE): per thread a
/// stack of batches, innermost last.
#[derive(Debug, Default)]
pub struct RemoteSends {
    stacks: HashMap<u32, Vec<Batch>>,
    /// Threads whose call is made again after running a message sent to
    /// them: it goes on with its batch.
    resume: HashSet<u32>,
}

/// The calls of one API call, made one after the other.
#[derive(Debug)]
struct Batch {
    /// The link's id of the call being waited for.
    current: Option<u32>,
    /// The calls still to make.
    rest: VecDeque<(u32, WinCall)>,
    /// The answers so far (`None`: none came).
    replies: Vec<Option<WinReply>>,
}

impl RemoteSends {
    fn begin(&mut self, tid: u32, calls: Vec<(u32, WinCall)>) {
        self.stacks.entry(tid).or_default().push(Batch { current: None, rest: calls.into(), replies: Vec::new() });
    }

    /// Whether thread `tid` goes on with its innermost batch (its call is
    /// made again after running a sent message) rather than starting one.
    fn resumed(&mut self, tid: u32) -> bool {
        self.resume.remove(&tid) && self.stacks.get(&tid).is_some_and(|s| !s.is_empty())
    }

    /// Whether thread `tid`'s next call goes on with a batch (without
    /// taking the mark).
    fn resuming(&self, tid: u32) -> bool {
        self.resume.contains(&tid)
    }

    fn mark_resume(&mut self, tid: u32) {
        self.resume.insert(tid);
    }

    fn top(&mut self, tid: u32) -> Option<&mut Batch> {
        self.stacks.get_mut(&tid).and_then(|s| s.last_mut())
    }

    /// The innermost batch is done: its answers.
    fn end(&mut self, tid: u32) -> Vec<Option<WinReply>> {
        let Some(s) = self.stacks.get_mut(&tid) else { return Vec::new() };
        let b = s.pop();
        if s.is_empty() { self.stacks.remove(&tid); }
        b.map(|b| b.replies).unwrap_or_default()
    }

    /// Thread `tid` ended: the calls it was waiting for.
    pub fn forget_thread(&mut self, tid: u32) -> Vec<u32> {
        self.resume.remove(&tid);
        self.stacks.remove(&tid).unwrap_or_default().into_iter().filter_map(|b| b.current).collect()
    }
}

impl WindowManager {
    /// Queue message `s` from another program for the thread of queue
    /// `hmq` and wake it: FALSE without the queue or when too many wait.
    pub fn queue_sent(&self, hmq: u32, s: SentMessage) -> bool {
        let Some(mq) = self.get_mq(hmq) else { return false };
        let mut q = mq.lock_or_recover();
        if q.sent.len() >= MAX_SENT_MESSAGES { return false; }
        q.sent.push_back(s);
        let (cond, lock) = (Arc::clone(&q.cond), Arc::clone(&q.lock));
        drop(q);
        let _g = lock.lock_or_recover();
        cond.notify_all();
        true
    }

    /// Post `msg` for no window to queue `hmq` (`WinPostQueueMsg`).
    pub fn post_queue_msg(&self, hmq: u32, msg: u32, mp1: u32, mp2: u32) {
        let Some(mq) = self.get_mq(hmq) else { return };
        let mut q = mq.lock_or_recover();
        q.messages.push_back(super::pm_types::OS2Message { hwnd: 0, msg, mp1, mp2, time: 0, x: 0, y: 0 });
        q.cond.notify_one();
    }

    /// The first message from another program waiting in queue `hmq`.
    pub fn take_sent(&self, hmq: u32) -> Option<SentMessage> {
        self.get_mq(hmq)?.lock_or_recover().sent.pop_front()
    }

    /// The ids of all messages from other programs waiting in queue `hmq`,
    /// taken out (the queue goes away).
    pub fn drain_sent(&self, hmq: u32) -> Vec<u64> {
        self.get_mq(hmq).map_or_else(Vec::new, |mq| mq.lock_or_recover().sent.drain(..).filter_map(|s| s.id).collect())
    }
}

impl Loader {
    /// PMWIN call `ordinal` (arguments at `arg(offset)`) when it is made on
    /// another program's window: make it through the desktop server.
    /// `None` for any other call, which runs here as usual.
    pub(super) fn remote_pmwin(&self, tid: u32, ordinal: u32, arg: &dyn Fn(u64) -> u32) -> Option<ApiResult> {
        let &(_, off) = REMOTE_CALLS.iter().find(|(o, _)| *o == ordinal)?;
        let hwnd = arg(off);
        if !self.shared.window_mgr.lock_or_recover().is_foreign(hwnd) {
            return None;
        }
        Some(self.remote_call(tid, ordinal, hwnd, arg))
    }

    /// Make `call` on another program's window `hwnd` (never under the
    /// window manager's lock): `None` when it failed or got no answer, and
    /// `PMERR_INVALID_HWND` is then the thread's last error.
    fn call_remote(&self, tid: u32, hwnd: u32, call: WinCall) -> Option<WinReply> {
        let reply = self.shared.desktop_link.get().and_then(|l| l.call(hwnd, call));
        match reply {
            None | Some(WinReply::Failed) => {
                self.pm_error(tid, PMERR_INVALID_HWND);
                None
            }
            r => r,
        }
    }

    /// `call_remote` for a description.
    pub(super) fn describe_remote(&self, tid: u32, hwnd: u32) -> Option<WindowInfo> {
        match self.call_remote(tid, hwnd, WinCall::Describe)? {
            WinReply::Info(i) => Some(*i),
            _ => {
                self.pm_error(tid, PMERR_INVALID_HWND);
                None
            }
        }
    }

    /// `call_remote` for a value; 0 for a failure.
    fn value_remote(&self, tid: u32, hwnd: u32, call: WinCall) -> u32 {
        match self.call_remote(tid, hwnd, call) {
            Some(WinReply::Value(v)) => v,
            Some(_) => {
                self.pm_error(tid, PMERR_INVALID_HWND);
                0
            }
            None => 0,
        }
    }

    /// Text in the calling thread's codepage, written as `WinQueryWindowText`
    /// does: at most `cch_max - 1` bytes and a null. Returns the bytes copied.
    fn write_text(&self, tid: u32, text: &str, cch_max: usize, buf: u32) -> u32 {
        let bytes = super::codepage::cp_encode(text, self.thread_codepage(tid));
        let n = bytes.len().min(cch_max.saturating_sub(1));
        if cch_max == 0 || buf == 0 { return 0; }
        self.guest_write_bytes(buf, &bytes[..n]);
        self.guest_write::<u8>(buf + n as u32, 0);
        n as u32
    }

    fn remote_call(&self, tid: u32, ordinal: u32, hwnd: u32, arg: &dyn Fn(u64) -> u32) -> ApiResult {
        let done = |ok: bool| ApiResult::Normal(ok as u32);
        match ordinal {
            ORD_SEND_MSG => match self.remote_wait(tid, ORD_SEND_MSG, hwnd, WinCall::Send { msg: arg(8), mp1: arg(12), mp2: arg(16) }) {
                Err(r) => r,
                Ok(Some(WinReply::Value(v))) => ApiResult::Normal(v),
                Ok(_) => self.pm_fail(tid, PMERR_INVALID_HWND),
            },
            ORD_SET_WINDOW_TEXT => {
                // In the calling thread's codepage, as text comes back.
                let text = self.read_guest_string_cp(arg(8), self.thread_codepage(tid));
                ApiResult::Normal(self.value_remote(tid, hwnd, WinCall::SetText { text }))
            }
            ORD_POST_MSG => {
                let call = WinCall::Post { msg: arg(8), mp1: arg(12), mp2: arg(16) };
                ApiResult::Normal(self.value_remote(tid, hwnd, call))
            }
            // WinIsWindow sets no error.
            ORD_IS_WINDOW => done(matches!(self.shared.desktop_link.get().and_then(|l| l.call(hwnd, WinCall::Describe)),
                                           Some(WinReply::Info(_)))),
            ORD_QUERY_WINDOW_PROCESS => {
                let Some(i) = self.describe_remote(tid, hwnd) else { return done(false) };
                for (ptr, value) in [(arg(8), i.pid), (arg(12), i.tid)] {
                    if ptr != 0 && self.guest_write::<u32>(ptr, value).is_none() {
                        return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
                    }
                }
                done(true)
            }
            ORD_QUERY_WINDOW_POS => {
                let pswp = arg(8);
                if pswp == 0 { return done(false); }
                let Some(i) = self.describe_remote(tid, hwnd) else { return done(false) };
                let words = [i.swp_fl, i.cy as u32, i.cx as u32, i.y as u32, i.x as u32, 0, hwnd, 0, 0];
                for (n, w) in words.iter().enumerate() {
                    if self.guest_write::<u32>(pswp + 4 * n as u32, *w).is_none() {
                        return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
                    }
                }
                done(true)
            }
            ORD_QUERY_WINDOW_RECT => {
                let prcl = arg(8);
                let Some(i) = self.describe_remote(tid, hwnd) else { return done(false) };
                if prcl != 0 {
                    for (n, w) in [0, 0, i.cx, i.cy].iter().enumerate() {
                        self.guest_write::<i32>(prcl + 4 * n as u32, *w);
                    }
                }
                done(true)
            }
            ORD_QUERY_WINDOW_TEXT => {
                let (cch_max, buf) = (arg(8) as usize, arg(12));
                let text = self.describe_remote(tid, hwnd).map(|i| i.text).unwrap_or_default();
                ApiResult::Normal(self.write_text(tid, &text, cch_max, buf))
            }
            ORD_QUERY_WINDOW_TEXT_LENGTH => {
                let Some(i) = self.describe_remote(tid, hwnd) else { return done(false) };
                ApiResult::Normal(super::codepage::cp_encode(&i.text, self.thread_codepage(tid)).len() as u32)
            }
            ORD_IS_WINDOW_VISIBLE => done(self.describe_remote(tid, hwnd).is_some_and(|i| i.showing)),
            ORD_IS_WINDOW_ENABLED => done(self.describe_remote(tid, hwnd).is_some_and(|i| i.style & WS_DISABLED == 0)),
            ORD_QUERY_WINDOW_ULONG => {
                ApiResult::Normal(self.value_remote(tid, hwnd, WinCall::Word { index: arg(8) as i32, size: 4 }))
            }
            ORD_QUERY_WINDOW_USHORT => {
                ApiResult::Normal(self.value_remote(tid, hwnd, WinCall::Word { index: arg(8) as i32, size: 2 }))
            }
            ORD_QUERY_WINDOW => {
                let code = arg(8) as i32;
                if !(QW_NEXT..=QW_FRAMEOWNER).contains(&code) {
                    return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
                }
                ApiResult::Normal(self.value_remote(tid, hwnd, WinCall::Query { code }))
            }
            ORD_QUERY_CLASS_NAME => {
                let (cch_max, pch) = (arg(8) as i32, arg(12));
                let Some(i) = self.describe_remote(tid, hwnd) else { return done(false) };
                if cch_max <= 0 || pch == 0 { return self.pm_fail(tid, PMERR_INVALID_STRING_PARM); }
                ApiResult::Normal(self.write_text(tid, &i.class, cch_max as usize, pch))
            }
            ORD_SET_WINDOW_POS => {
                let (x, y, cx, cy) = bound_window_geometry(arg(12) as i32, arg(16) as i32, arg(20) as i32,
                                                           arg(24) as i32);
                let call = WinCall::SetPos { behind: arg(8), x, y, cx, cy, fl: arg(28) };
                if call.expires() {
                    return ApiResult::Normal(self.value_remote(tid, hwnd, call));
                }
                // A state change: the frame's client is asked first, and
                // this thread waits for that as for a send.
                match self.remote_wait(tid, ORD_SET_WINDOW_POS, hwnd, call) {
                    Err(r) => r,
                    Ok(Some(WinReply::Value(v))) => ApiResult::Normal(v),
                    Ok(_) => self.pm_fail(tid, PMERR_INVALID_HWND),
                }
            }
            ORD_SHOW_WINDOW => ApiResult::Normal(self.value_remote(tid, hwnd, WinCall::Show { show: arg(8) != 0 })),
            ORD_SET_ACTIVE_WINDOW => ApiResult::Normal(self.value_remote(tid, hwnd, WinCall::Activate)),
            _ => unreachable!("ordinal {ordinal} is in REMOTE_CALLS"),
        }
    }

    /// Make `call` (of PMWIN API `ordinal`) on another program's window and
    /// wait for its answer without a time limit (see [`Self::remote_batch`]).
    fn remote_wait(&self, tid: u32, ordinal: u32, hwnd: u32, call: WinCall) -> Result<Option<WinReply>, ApiResult> {
        self.remote_batch(tid, ordinal, vec![(hwnd, call)]).map(|r| r.into_iter().next().flatten())
    }

    /// Whether thread `tid`'s API call goes on with the calls it was
    /// waiting for (made again after running a message sent to it), so
    /// that it must not prepare them again.
    pub(super) fn remote_resuming(&self, tid: u32) -> bool {
        self.shared.window_mgr.lock_or_recover().remote_sends.resuming(tid)
    }

    /// Make `calls` (window, call) of PMWIN API `ordinal` on other
    /// programs' windows one after the other, each waited for without a
    /// time limit, running the messages other programs send to this
    /// thread's queue meanwhile (module comment): `Ok` with an answer per
    /// call (`None`: none came), or `Err` with the result the API returns
    /// now — a chain running a message sent to this thread, after which
    /// the API is made again and goes on with the same calls (whatever
    /// `calls` it then passes).
    pub(super) fn remote_batch(&self, tid: u32, ordinal: u32, calls: Vec<(u32, WinCall)>)
        -> Result<Vec<Option<WinReply>>, ApiResult> {
        let Some(link) = self.shared.desktop_link.get() else { return Ok(vec![None; calls.len()]) };
        let (hmq, wait) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let hmq = wm.thread_queue(tid);
            let wait = hmq.and_then(|h| wm.get_mq(h)).map(|mq| {
                let q = mq.lock_or_recover();
                (Arc::clone(&q.cond), Arc::clone(&q.lock))
            });
            if !wm.remote_sends.resumed(tid) {
                wm.remote_sends.begin(tid, calls);
            }
            (hmq, wait)
        };
        // An answer wakes the thread where it waits: its queue.
        let wake = wait.clone().map(|(cond, lock)| -> CallWaker {
            Arc::new(move || {
                let _g = lock.lock_or_recover();
                cond.notify_all();
            })
        });
        loop {
            // The call waited for, or the next one to make.
            let (current, next) = {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                match wm.remote_sends.top(tid) {
                    Some(b) => (b.current, if b.current.is_none() { b.rest.pop_front() } else { None }),
                    None => return Ok(Vec::new()),
                }
            };
            let id = match (current, next) {
                (Some(id), _) => id,
                (None, None) => return Ok(self.shared.window_mgr.lock_or_recover().remote_sends.end(tid)),
                (None, Some((hwnd, call))) => {
                    let id = link.begin_call(hwnd, call, wake.clone());
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let Some(b) = wm.remote_sends.top(tid) else { return Ok(Vec::new()) };
                    match id {
                        Some(id) => { b.current = Some(id); id }
                        None => { b.replies.push(None); continue; }
                    }
                }
            };
            let state = match wait {
                Some(_) => link.poll_call(id),
                None => link.wait_call(id, SEND_WAIT_SLICE),
            };
            let reply = match state {
                CallState::Answered(r) => Some(Some(r)),
                CallState::Gone => Some(None),
                CallState::Waiting => None,
            };
            if let Some(reply) = reply {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(b) = wm.remote_sends.top(tid) {
                    b.current = None;
                    b.replies.push(reply);
                }
                continue;
            }
            // Messages sent to this thread meanwhile run first.
            if let Some(s) = hmq.and_then(|h| self.shared.window_mgr.lock_or_recover().take_sent(h)) {
                return Err(self.run_sent(tid, s, AfterSent::Resume(ordinal)));
            }
            if self.stop_or_restart() {
                link.abandon_call(id);
                return Ok(self.shared.window_mgr.lock_or_recover().remote_sends.end(tid));
            }
            if let Some((cond, lock)) = &wait {
                let g = lock.lock_or_recover();
                let _ = cond.wait_timeout(g, SEND_WAIT_SLICE);
            }
        }
    }

    /// Run message `s` from another program on thread `tid`: a chain
    /// calling the send hooks of the window's queue and its procedure (or
    /// the built-in handling) — or, for a state change, the client's
    /// `WM_MINMAXFRAME` —, then answering and going on as `after` says.
    pub(super) fn run_sent(&self, tid: u32, s: SentMessage, after: AfterSent) -> ApiResult {
        let (steps, finish) = match s.kind {
            SentKind::Message { msg, mp1, mp2 } => {
                let (proc_addr, hmq) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    wm.get_window(s.hwnd).map_or((0, 0), |w| (w.pfn_wp, w.hmq))
                };
                let mut steps = if hmq != 0 {
                    self.send_hook_steps(tid, hmq, [s.hwnd, msg, mp1, mp2], true)
                } else {
                    Vec::new()
                };
                steps.push(CallStep::send(proc_addr, s.hwnd, msg, mp1, mp2));
                (steps, SentFinish::Reply)
            }
            SentKind::MinMax { op, target } => {
                let (client, proc_addr) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let client = wm.frame_to_client.get(&s.hwnd).copied().unwrap_or(0);
                    (client, wm.get_window(client).map_or(0, |w| w.pfn_wp))
                };
                // WM_MINMAXFRAME(PSWP, 0): the SWP below the caller's stack.
                let steps = if proc_addr == 0 { Vec::new() } else {
                    vec![CallStep::Guest {
                        proc_addr, args: vec![client, WM_MINMAXFRAME, 0, 0],
                        block: minmax_swp(s.hwnd, op, target).to_vec(), block_arg: 2,
                        stop_on_true: false, window: client,
                    }]
                };
                (steps, SentFinish::MinMax { op, target })
            }
            SentKind::Drag { msg, pdinfo, x, y } => {
                let local = self.shared.window_mgr.lock_or_recover().window_under(x, y)
                    .filter(|w| w.0 == s.hwnd).map_or((0, 0), |w| (w.1, w.2));
                let steps = self.drag_step(s.hwnd, msg, pdinfo, (x, y), local).into_iter().collect();
                (steps, SentFinish::Drag { msg })
            }
            SentKind::HostDrop { drop, x, y, stage, pdinfo } => {
                use super::pm_drag::{HOST_DROP_DROP, HOST_DROP_OVER};
                let pdinfo = if stage == HOST_DROP_OVER { self.host_drop_draginfo(tid, drop, (x, y)) } else { pdinfo };
                let msg = match stage {
                    HOST_DROP_OVER => DM_DRAGOVER,
                    HOST_DROP_DROP => super::pm_drag::DM_DROP,
                    _ => super::pm_drag::DM_DRAGLEAVE,
                };
                let local = self.shared.window_mgr.lock_or_recover().window_under(x, y)
                    .filter(|w| w.0 == s.hwnd).map_or((0, 0), |w| (w.1, w.2));
                let steps = if pdinfo == 0 { Vec::new() } else {
                    self.drag_step(s.hwnd, msg, pdinfo, (x, y), local).into_iter().collect()
                };
                (steps, SentFinish::HostDrop { stage, pdinfo, x, y })
            }
        };
        let calls = steps.len() as u32;
        let mut chain = CallChain::new(tid, steps, false,
                                       ChainThen::AnswerSent { id: s.id, hwnd: s.hwnd, after, finish, calls });
        // WinInSendMsg is TRUE while the procedure runs.
        chain.inter_thread_send = true;
        self.start_chain(chain)
    }

    /// The chain of a [`SentMessage`] for window `hwnd` is done (`calls`:
    /// how many calls it was to make): answer — with the procedure's result,
    /// `Failed` when it was not called because the window is gone; after a
    /// state change, which is made unless the client said TRUE, TRUE — and
    /// say how the thread goes on.
    pub(super) fn sent_done(&self, id: Option<u64>, hwnd: u32, after: AfterSent, finish: SentFinish, calls: u32,
                            chain: &CallChain) -> ChainEnd {
        let complete = chain.results.len() == calls as usize;
        let reply = match finish {
            SentFinish::Reply if complete && calls > 0 => WinReply::Value(chain.last),
            SentFinish::Reply => WinReply::Failed,
            SentFinish::Drag { .. } if complete && calls > 0 => WinReply::Value(chain.last),
            SentFinish::Drag { msg } => WinReply::Value(if msg == DM_DRAGOVER { DOR_NEVERDROP } else { 0 }),
            SentFinish::HostDrop { stage, pdinfo, x, y } => {
                let took_part = complete && calls > 0;
                let reply = if took_part { chain.last } else { DOR_NEVERDROP };
                self.host_drop_step_done(hwnd, stage, pdinfo, (x, y), reply, took_part);
                WinReply::Value(reply)
            }
            SentFinish::MinMax { op, target } => {
                if calls == 0 || !complete || chain.last == 0 {
                    self.shared.window_mgr.lock_or_recover().finish_minmax(hwnd, op, target);
                }
                WinReply::Value(1)
            }
        };
        if let (Some(id), Some(link)) = (id, self.shared.desktop_link.get()) {
            link.answer(id, reply);
        }
        match after {
            AfterSent::Rerun(ordinal) => ChainEnd::Rerun(ordinal),
            AfterSent::Resume(ordinal) => {
                self.shared.window_mgr.lock_or_recover().remote_sends.mark_resume(chain.tid);
                ChainEnd::Rerun(ordinal)
            }
            AfterSent::ModalLoop { dlg_proc, hwnd_dlg, hmq } => ChainEnd::ModalLoop { dlg_proc, hwnd_dlg, hmq, msg: None },
            AfterSent::Drag => self.drag_after_sent(),
        }
    }

    /// Ask window `ask.hwnd` `WM_HITTEST` for the pointer event at the head
    /// of the input queue (`pm_input.rs`), then go on as `after` says.
    pub(super) fn hit_test_chain(&self, tid: u32, ask: super::pm_input::HitAsk, after: AfterSent) -> ApiResult {
        let pfn = self.shared.window_mgr.lock_or_recover().get_window(ask.hwnd).map_or(0, |w| w.pfn_wp);
        let steps = if pfn == 0 { Vec::new() } else {
            vec![CallStep::send(pfn, ask.hwnd, super::pm_input::WM_HITTEST, ask.mp1, 0)]
        };
        self.start_chain(CallChain::new(tid, steps, false, ChainThen::HitTest { hwnd: ask.hwnd, after }))
    }

    /// The `WM_HITTEST` of [`Self::hit_test_chain`] returned: its answer
    /// (`HT_NORMAL` when the window went first) steers the pointer event.
    pub(super) fn hit_test_done(&self, hwnd: u32, after: AfterSent, chain: &CallChain) -> ChainEnd {
        let answer = if chain.results.is_empty() { super::pm_input::HT_NORMAL } else { chain.last as i32 };
        self.shared.window_mgr.lock_or_recover().hit_test_answered(hwnd, answer);
        match after {
            AfterSent::Rerun(ordinal) | AfterSent::Resume(ordinal) => ChainEnd::Rerun(ordinal),
            AfterSent::ModalLoop { dlg_proc, hwnd_dlg, hmq } => ChainEnd::ModalLoop { dlg_proc, hwnd_dlg, hmq, msg: None },
            AfterSent::Drag => self.drag_after_sent(),
        }
    }

    /// The next message from another program waiting in queue `hmq`.
    pub(super) fn take_sent(&self, hmq: u32) -> Option<SentMessage> {
        self.shared.window_mgr.lock_or_recover().take_sent(hmq)
    }

    /// Answer `Failed` to messages from other programs that will not run
    /// (their queue went away).
    pub(super) fn refuse_sent(&self, ids: Vec<u64>) {
        if let Some(link) = self.shared.desktop_link.get() {
            for id in ids {
                link.answer(id, WinReply::Failed);
            }
        }
    }

    /// Thread `tid` ended: forget the sends it waited for.
    pub(super) fn forget_remote_sends(&self, tid: u32) {
        let ids = self.shared.window_mgr.lock_or_recover().remote_sends.forget_thread(tid);
        if let Some(link) = self.shared.desktop_link.get() {
            for id in ids {
                link.abandon_call(id);
            }
        }
    }

    /// The description of this program's window `hwnd` for another program.
    fn describe_window(&self, hwnd: u32) -> Option<WindowInfo> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let w = wm.get_window(hwnd)?;
        let coord = |v: i32| v.clamp(-MAX_COORD, MAX_COORD);
        Some(WindowInfo {
            pid: self.shared.process_id(),
            tid: wm.window_thread(hwnd).unwrap_or(super::threads::MAIN_TID),
            parent: w.parent,
            owner: w.owner,
            style: (w.style & !WS_VISIBLE) | if w.visible { WS_VISIBLE } else { 0 },
            id: w.id,
            x: coord(w.x),
            y: coord(w.y),
            cx: coord(w.cx),
            cy: coord(w.cy),
            swp_fl: wm.query_swp_flags(hwnd),
            showing: wm.is_window_visible(hwnd),
            class: wm.public_class_name(hwnd).unwrap_or_else(|| w.class_name.clone()),
            text: wm.get_window(wm.text_window(hwnd)).map(|t| t.text.clone()).unwrap_or_default(),
        })
    }

    /// `WinSetWindowPos` made by another program (call `id`) on this
    /// program's window: the answer, or `None` when a state change waits
    /// for the frame's thread to ask its client (module comment).
    #[allow(clippy::too_many_arguments)]
    fn set_pos_for_other(&self, id: u64, hwnd: u32, behind: u32, x: i32, y: i32, cx: i32, cy: i32, fl: u32)
        -> Option<WinReply> {
        let (x, y, cx, cy) = bound_window_geometry(x, y, cx, cy);
        // The insert-behind window must be one of ours (or HWND_TOP / BOTTOM).
        let behind = if behind == HWND_TOP || behind == HWND_BOTTOM
            || self.shared.window_mgr.lock_or_recover().is_window(behind) { behind } else { 0 };
        let Some((op, target)) = self.set_window_pos(hwnd, behind, x, y, cx, cy, fl) else {
            return Some(WinReply::Value(1));
        };
        let wm = self.shared.window_mgr.lock_or_recover();
        let s = SentMessage { id: Some(id), hwnd, kind: SentKind::MinMax { op, target } };
        let asked = wm.get_window(hwnd).map(|w| w.hmq).filter(|&q| q != 0).is_some_and(|q| wm.queue_sent(q, s));
        drop(wm);
        if asked {
            return None;
        }
        // No thread to ask: changed at once.
        self.shared.window_mgr.lock_or_recover().finish_minmax(hwnd, op, target);
        Some(WinReply::Value(1))
    }

    /// The children of this program's window `hwnd` (at its slot address:
    /// its windows on the desktop), top first.
    fn children_top_first(&self, hwnd: u32) -> Vec<u32> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let parent = if hwnd & !super::pm_types::HWND_SLOT_MASK == 0 { HWND_DESKTOP } else { hwnd };
        let mut v = wm.child_windows(parent);
        v.reverse();
        v.truncate(MAX_HANDLES);
        v
    }

    /// `WinBroadcastMsg(HWND_DESKTOP, …)` from another program on this
    /// program's windows (module comment): posted, or queued for their
    /// threads without an answer.
    fn broadcast_for_other(&self, msg: u32, mp1: u32, mp2: u32, fl: u32) -> WinReply {
        use super::pm_message::{BMSG_ALL, BMSG_DESCENDANTS, BMSG_FRAMEONLY, BMSG_POSTQUEUE, BMSG_SEND};
        if fl & !BMSG_ALL != 0 {
            return WinReply::Failed;
        }
        let wm = self.shared.window_mgr.lock_or_recover();
        if fl & BMSG_POSTQUEUE != 0 {
            let queues: Vec<u32> = wm.msg_queues.keys().copied().collect();
            for q in queues {
                wm.post_queue_msg(q, msg, mp1, mp2);
            }
            return WinReply::Value(1);
        }
        let targets = wm.broadcast_targets(HWND_DESKTOP, fl & BMSG_DESCENDANTS != 0, fl & BMSG_FRAMEONLY != 0);
        for h in targets {
            if fl & BMSG_SEND != 0 {
                let q = wm.get_window(h).map_or(0, |w| w.hmq);
                if q != 0 {
                    wm.queue_sent(q, SentMessage::message(None, h, msg, mp1, mp2));
                }
            } else {
                wm.post_msg(h, msg, mp1, mp2);
            }
        }
        WinReply::Value(1)
    }

    /// `WinSwitchToProgram` for another program's frame `hwnd`: shown and
    /// activated, as for a frame of this program.
    pub(super) fn switch_to_other_program(&self, hwnd: u32) -> bool {
        let Some(link) = self.shared.desktop_link.get() else { return false };
        matches!(link.call(hwnd, WinCall::Show { show: true }), Some(WinReply::Value(_)))
            && matches!(link.call(hwnd, WinCall::Activate), Some(WinReply::Value(_)))
    }

    /// Report a change of this program's active frame to the desktop
    /// server and wait until it took it (ADR-0108): after every API call,
    /// so that the server has it before the program does anything else (a
    /// program that activates a frame and then tells another program so
    /// through shared memory must not be overtaken by that program's
    /// activation). The server answers with the session's active frame.
    pub(crate) fn report_activation(&self) {
        let Some(link) = self.shared.desktop_link.get() else { return };
        let report = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if !wm.activation_changed() { return; }
            wm.activation_report()
        };
        if let Some(WinReply::Value(active)) = link.call(SERVER_ADDRESS, WinCall::Report(report)) {
            self.shared.window_mgr.lock_or_recover().session_active = active;
        }
    }

    /// The slots of the other programs on the session's desktop (none
    /// without a desktop server).
    pub(super) fn other_programs(&self) -> Vec<u32> {
        let Some(link) = self.shared.desktop_link.get() else { return Vec::new() };
        match link.call(SERVER_ADDRESS, WinCall::Clients) {
            Some(WinReply::Handles(slots)) => slots.into_iter().filter(|&s| s != link.slot).collect(),
            _ => Vec::new(),
        }
    }

    /// `WinBroadcastMsg(HWND_DESKTOP, …)` (or `BMSG_POSTQUEUE`) to the other
    /// programs' windows (module comment).
    pub(super) fn broadcast_to_others(&self, msg: u32, mp1: u32, mp2: u32, fl: u32) {
        let Some(link) = self.shared.desktop_link.get() else { return };
        for slot in self.other_programs() {
            link.call(slot_address(slot), WinCall::Broadcast { msg, mp1, mp2, fl });
        }
    }

    /// The children of another program's window `hwnd` (or, at a slot
    /// address, its windows on the desktop), top first.
    /// `None` when the window is gone or does not answer.
    pub(super) fn remote_children(&self, hwnd: u32) -> Option<Vec<u32>> {
        match self.shared.desktop_link.get().and_then(|l| l.call(hwnd, WinCall::Children)) {
            Some(WinReply::Handles(hs)) => Some(hs),
            _ => None,
        }
    }

    /// The windows on the session's desktop, top first: this program's and
    /// the other programs', frames in the session's stacking order, then
    /// each program's other windows in its own order.
    pub(super) fn desktop_windows(&self) -> Vec<u32> {
        let own = self.children_top_first(0);
        let Some(link) = self.shared.desktop_link.get() else { return own };
        let stacking = match link.call(SERVER_ADDRESS, WinCall::Stacking) {
            Some(WinReply::Handles(hs)) => hs,
            _ => Vec::new(),
        };
        let mut lists = vec![own];
        lists.extend(self.other_programs().into_iter().map(|s| self.remote_children(slot_address(s)).unwrap_or_default()));
        merge_by_stacking(&stacking, &lists)
    }
}

impl Responder for Loader {
    /// The server's word on activation, taken by the link's reader thread
    /// in order with the answers to this program's calls (ADR-0108).
    fn take_activation(&self, msg: &ServerMsg) -> bool {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        match *msg {
            ServerMsg::Deactivate { frame, new_active, seq } => wm.deactivate_frame(frame, new_active, seq),
            ServerMsg::Active { frame } => wm.session_active = frame,
            _ => return false,
        }
        true
    }

    /// A call another program makes on window `hwnd` of this one: only on
    /// windows of this program's slot that exist. A sent message is queued
    /// for the window's thread, which answers it later.
    fn answer(&self, id: u64, hwnd: u32, call: &WinCall) -> Option<WinReply> {
        let (in_slot, exists) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            (wm.handle_slot.is_some_and(|s| handle_in_slot(hwnd, s)), wm.is_window(hwnd))
        };
        // The program as a whole: its slot address.
        let whole = in_slot && wm_slot_address(hwnd);
        match *call {
            WinCall::Children if whole || (in_slot && exists) => {
                return Some(WinReply::Handles(self.children_top_first(hwnd)));
            }
            WinCall::Broadcast { msg, mp1, mp2, fl } if whole => {
                return Some(self.broadcast_for_other(msg, mp1, mp2, fl));
            }
            WinCall::DdeTargets if whole => {
                let hwnds = self.shared.window_mgr.lock_or_recover().dde_targets();
                return Some(WinReply::Windows { pid: self.shared.process_id(), hwnds });
            }
            _ if !(in_slot && exists) => return Some(WinReply::Failed),
            WinCall::Send { msg, mp1, mp2 } => {
                let wm = self.shared.window_mgr.lock_or_recover();
                let queued = wm.find_hmq_for_hwnd(hwnd)
                    .is_some_and(|hmq| wm.queue_sent(hmq, SentMessage::message(Some(id), hwnd, msg, mp1, mp2)));
                return (!queued).then_some(WinReply::Failed);
            }
            WinCall::SetPos { behind, x, y, cx, cy, fl } => {
                return self.set_pos_for_other(id, hwnd, behind, x, y, cx, cy, fl);
            }
            WinCall::Drag { msg, pdinfo, x, y } => {
                // The target reads the DRAGINFO the source gave it.
                if msg != DM_DRAGLEAVE && !self.accept_draginfo(pdinfo) {
                    return Some(WinReply::Value(if msg == DM_DRAGOVER { DOR_NEVERDROP } else { 0 }));
                }
                let wm = self.shared.window_mgr.lock_or_recover();
                let s = SentMessage { id: Some(id), hwnd, kind: SentKind::Drag { msg, pdinfo, x, y } };
                let queued = wm.find_hmq_for_hwnd(hwnd).is_some_and(|hmq| wm.queue_sent(hmq, s));
                return (!queued).then_some(WinReply::Failed);
            }
            _ => {}
        }
        Some(self.answer_now(hwnd, call))
    }
}

impl Loader {
    /// A call another program makes on window `hwnd` of this one, answered
    /// on the link thread from the window manager.
    fn answer_now(&self, hwnd: u32, call: &WinCall) -> WinReply {
        let value = |v: bool| if v { WinReply::Value(1) } else { WinReply::Failed };
        match *call {
            WinCall::Post { msg, mp1, mp2 } => {
                let wm = self.shared.window_mgr.lock_or_recover();
                let queued = wm.find_hmq_for_hwnd(hwnd).and_then(|q| wm.get_mq(q)).is_some();
                wm.post_msg(hwnd, msg, mp1, mp2);
                value(queued)
            }
            WinCall::Describe => self.describe_window(hwnd).map_or(WinReply::Failed, |i| WinReply::Info(Box::new(i))),
            WinCall::Word { index, size } => {
                let wm = self.shared.window_mgr.lock_or_recover();
                if !window_word_in_range(&wm, hwnd, index, size as u32) {
                    return WinReply::Failed;
                }
                WinReply::Value(if size == 2 {
                    wm.query_window_ushort(hwnd, index) as u32
                } else {
                    wm.query_window_ulong(hwnd, index)
                })
            }
            WinCall::Query { code } => {
                if !(QW_NEXT..=QW_FRAMEOWNER).contains(&code) {
                    return WinReply::Failed;
                }
                WinReply::Value(self.shared.window_mgr.lock_or_recover().query_window(hwnd, code))
            }
            WinCall::Show { show } => {
                self.shared.window_mgr.lock_or_recover().show_window(hwnd, show);
                WinReply::Value(1)
            }
            WinCall::Activate => {
                let (done, report) = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let done = wm.set_active_window(hwnd);
                    (done, wm.activation_changed().then(|| wm.activation_report()))
                };
                // Reported before the answer goes back, on the same link, so
                // the server takes it before the caller goes on (ADR-0108).
                if let (Some(report), Some(link)) = (report, self.shared.desktop_link.get()) {
                    link.notify(SERVER_ADDRESS, WinCall::Report(report));
                }
                value(done)
            }
            WinCall::SetText { ref text } => {
                self.set_window_text(hwnd, text.clone());
                WinReply::Value(1)
            }
            // The window in this frame that takes drag input at the point.
            WinCall::DragTarget { x, y } => {
                let wm = self.shared.window_mgr.lock_or_recover();
                match wm.window_under(x, y) {
                    Some((h, _, _)) if wm.find_frame_for_hwnd(h) == hwnd =>
                        WinReply::Windows { pid: self.shared.process_id(), hwnds: vec![h] },
                    _ => WinReply::Failed,
                }
            }
            // Made by `answer`, or the server's.
            _ => WinReply::Failed,
        }
    }
}

/// Whether `hwnd` is a slot address: a handle prefix without a window part.
fn wm_slot_address(hwnd: u32) -> bool {
    hwnd != 0 && hwnd & !super::pm_types::HWND_SLOT_MASK == 0
}

/// Windows of several programs (`lists`, each top first) in one order, top
/// first: those in `stacking` (the session's frames, top first) in its
/// order, then the rest of each list in its own order; each window once.
pub(super) fn merge_by_stacking(stacking: &[u32], lists: &[Vec<u32>]) -> Vec<u32> {
    let known: HashSet<u32> = lists.iter().flatten().copied().collect();
    let mut seen = HashSet::new();
    let mut out: Vec<u32> = stacking.iter().copied().filter(|h| known.contains(h) && seen.insert(*h)).collect();
    for l in lists {
        out.extend(l.iter().copied().filter(|h| seen.insert(*h)));
    }
    out.truncate(MAX_HANDLES);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_hook::CallChain;

    const TID: u32 = 0;
    const PROC: u32 = 0x5000;

    /// A loader in desktop slot 1 with a queue for thread `TID` and a frame
    /// of a class with procedure `PROC`: `(hmq, frame)`.
    fn setup() -> (Loader, u32, u32) {
        let loader = Loader::new_mock();
        let (hmq, frame) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.set_handle_slot(1);
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(TID, hmq);
            wm.register_class("App".into(), PROC, 0);
            (hmq, wm.create_window("App".into(), HWND_DESKTOP, hmq))
        };
        (loader, hmq, frame)
    }

    fn chain(r: ApiResult) -> Box<CallChain> {
        match r {
            ApiResult::CallChain(c) => c,
            other => panic!("a call chain, got {other:?}"),
        }
    }

    fn normal(r: ApiResult) -> u32 {
        match r {
            ApiResult::Normal(v) => v,
            other => panic!("a value, got {other:?}"),
        }
    }

    fn send(msg: u32) -> WinCall {
        WinCall::Send { msg, mp1: 1, mp2: 2 }
    }

    #[test]
    fn a_sent_message_waits_in_the_windows_queue_for_its_thread() {
        let (loader, hmq, frame) = setup();
        assert_eq!(loader.answer(7, frame, &send(0x1001)), None, "answered later");
        assert_eq!(loader.answer(8, frame + 99, &send(0x1001)), Some(WinReply::Failed), "no such window");
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.take_sent(hmq), Some(SentMessage::message(Some(7), frame, 0x1001, 1, 2)));
        assert_eq!(wm.take_sent(hmq), None);
    }

    #[test]
    fn a_full_queue_refuses_sent_messages_and_a_destroyed_one_gives_them_back() {
        let (loader, hmq, frame) = setup();
        for id in 0..MAX_SENT_MESSAGES as u64 {
            assert_eq!(loader.answer(id, frame, &send(1)), None);
        }
        assert_eq!(loader.answer(99, frame, &send(1)), Some(WinReply::Failed));
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.drain_sent(hmq), (0..MAX_SENT_MESSAGES as u64).collect::<Vec<_>>());
        assert_eq!(wm.take_sent(hmq), None);
    }

    #[test]
    fn reading_messages_runs_a_sent_message_first_and_answers_it() {
        let (loader, hmq, frame) = setup();
        loader.shared.window_mgr.lock_or_recover().post_msg(frame, 0x1002, 0, 0);
        assert_eq!(loader.answer(5, frame, &send(0x1001)), None);
        // WinWaitMsg: the sent message runs before the posted one is seen.
        let c = chain(loader.win_wait_msg(TID, 0, 0));
        assert_eq!(c.steps.iter().cloned().collect::<Vec<_>>(), vec![CallStep::send(PROC, frame, 0x1001, 1, 2)]);
        assert_eq!(c.then, ChainThen::AnswerSent { id: Some(5), hwnd: frame, after: AfterSent::Rerun(ORD_WAIT_MSG),
                                                   finish: SentFinish::Reply, calls: 1 });
        assert!(c.inter_thread_send);
        assert_eq!(normal(loader.win_in_send_msg(TID)), 1, "WinInSendMsg while it runs");
        let mut c = *c;
        c.steps.clear();
        c.returned(0x77, false);
        assert_eq!(loader.finish_chain(&c), ChainEnd::Rerun(ORD_WAIT_MSG));
        assert_eq!(normal(loader.win_in_send_msg(TID)), 0);
        // Then the wait sees the posted message.
        assert_eq!(normal(loader.win_wait_msg(TID, 0, 0)), 1);
        assert_eq!(loader.take_sent(hmq), None);
    }

    #[test]
    fn after_a_sent_message_a_modal_loop_or_a_send_goes_on() {
        let (loader, _, frame) = setup();
        let s = SentMessage::message(Some(3), frame, 1, 0, 0);
        let modal = AfterSent::ModalLoop { dlg_proc: 1, hwnd_dlg: 2, hmq: 3 };
        let c = chain(loader.run_sent(TID, s, modal));
        assert_eq!(loader.finish_chain(&c), ChainEnd::ModalLoop { dlg_proc: 1, hwnd_dlg: 2, hmq: 3, msg: None });
        // A send waiting for its answer goes on waiting for the same one.
        loader.shared.window_mgr.lock_or_recover().remote_sends.begin(TID, vec![(frame, WinCall::Activate)]);
        let c = chain(loader.run_sent(TID, s, AfterSent::Resume(ORD_SET_WINDOW_POS)));
        assert_eq!(loader.finish_chain(&c), ChainEnd::Rerun(ORD_SET_WINDOW_POS));
        assert!(loader.remote_resuming(TID));
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        assert!(wm.remote_sends.resumed(TID));
        assert!(!wm.remote_sends.resumed(TID), "a later call starts a batch of its own");
    }

    #[test]
    fn remote_batches_nest_per_thread() {
        let mut r = RemoteSends::default();
        r.begin(1, vec![(10, WinCall::Activate), (11, WinCall::Describe)]);
        r.begin(1, vec![(12, WinCall::Activate)]);
        r.begin(2, vec![]);
        r.top(1).unwrap().current = Some(7);
        r.mark_resume(1);
        assert!(r.resuming(1) && r.resumed(1) && !r.resuming(1));
        assert_eq!(r.top(1).unwrap().rest.len(), 1, "the innermost");
        r.top(1).unwrap().replies.push(Some(WinReply::Value(3)));
        assert_eq!(r.end(1), vec![Some(WinReply::Value(3))]);
        assert_eq!(r.top(1).unwrap().rest.len(), 2, "the outer one is next");
        r.top(1).unwrap().current = Some(5);
        assert_eq!(r.forget_thread(1), vec![5]);
        assert!(r.top(1).is_none());
        assert!(!r.resumed(2), "not marked");
        r.end(2);
        assert!(r.stacks.is_empty());
    }

    #[test]
    fn set_text_from_another_program_sets_the_windows_text() {
        let (loader, _, frame) = setup();
        let r = loader.answer(1, frame, &WinCall::SetText { text: "Grüße".into() });
        assert_eq!(r, Some(WinReply::Value(1)));
        assert_eq!(loader.describe_window(frame).unwrap().text, "Grüße");
    }

    #[test]
    fn without_a_desktop_link_a_send_to_a_foreign_window_fails() {
        let (loader, _, _) = setup();
        let foreign = (3 << super::super::pm_types::HWND_SLOT_SHIFT) | super::super::pm_types::FIRST_LOCAL_HWND;
        let arg = |off: u64| if off == 4 { foreign } else { 0 };
        assert_eq!(normal(loader.remote_call(TID, ORD_SEND_MSG, foreign, &arg)), 0);
        assert_eq!(loader.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_HWND);
    }

    /// A frame of class `App` with a client, and the client: `(frame, client)`.
    fn std_frame(loader: &Loader, hmq: u32) -> (u32, u32) {
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        wm.frame_to_client.insert(frame, client);
        if let Some(w) = wm.get_window_mut(frame) {
            (w.cx, w.cy) = (300, 200);
        }
        (frame, client)
    }

    #[test]
    fn another_programs_state_change_asks_the_client_on_its_thread() {
        let (loader, hmq, _) = setup();
        let (frame, client) = std_frame(&loader, hmq);
        let set = WinCall::SetPos { behind: 0, x: 0, y: 0, cx: 0, cy: 0, fl: SWP_MINIMIZE };
        assert_eq!(loader.answer(4, frame, &set), None, "answered once the client was asked");
        let s = loader.take_sent(hmq).unwrap();
        let SentKind::MinMax { op: SWP_MINIMIZE, target } = s.kind else { panic!("{s:?}") };
        assert_eq!((s.id, s.hwnd), (Some(4), frame));
        let c = chain(loader.run_sent(TID, s, AfterSent::Rerun(ORD_WAIT_MSG)));
        let Some(CallStep::Guest { proc_addr, args, block, block_arg, .. }) = c.steps.front().cloned() else { panic!() };
        assert_eq!((proc_addr, args, block_arg), (PROC, vec![client, WM_MINMAXFRAME, 0, 0], 2));
        assert_eq!(block, minmax_swp(frame, SWP_MINIMIZE, target).to_vec(), "the SWP as mp1");
        // TRUE: the frame stays as it is.
        let mut kept = (*c).clone();
        kept.steps.clear();
        kept.returned(1, false);
        loader.finish_chain(&kept);
        let minimized = |l: &Loader| l.shared.window_mgr.lock_or_recover().get_window(frame).unwrap().style & WS_MINIMIZED != 0;
        assert!(!minimized(&loader));
        // FALSE: it changes.
        let mut c = *c;
        c.steps.clear();
        c.returned(0, false);
        loader.finish_chain(&c);
        assert!(minimized(&loader));
    }

    #[test]
    fn the_send_hooks_of_the_windows_queue_run_before_its_procedure() {
        let (loader, hmq, frame) = setup();
        const HOOK: u32 = 0x6000;
        assert_eq!(normal(loader.win_set_hook(TID, hmq, super::super::pm_hook::HK_SENDMSG, HOOK)), 1);
        let c = chain(loader.run_sent(TID, SentMessage::message(Some(1), frame, 0x1001, 1, 2), AfterSent::Rerun(1)));
        assert_eq!(c.steps.len(), 2);
        let Some(CallStep::Guest { proc_addr: HOOK, args, block, .. }) = c.steps.front().cloned() else { panic!() };
        assert_eq!(args[2], 1, "fInterTask");
        assert_eq!(&block[..4], &[2, 1, 0x1001, frame], "SMHSTRUCT");
        assert_eq!(c.steps.back(), Some(&CallStep::send(PROC, frame, 0x1001, 1, 2)));
        let ChainThen::AnswerSent { calls: 2, .. } = c.then else { panic!() };
        // Only the hook ran (the window went meanwhile): no answer value.
        let mut c = *c;
        c.steps.clear();
        c.returned(0, false);
        assert_eq!(loader.finish_chain(&c), ChainEnd::Rerun(1));
    }

    #[test]
    fn another_programs_drag_finds_the_window_and_runs_on_its_thread() {
        use super::super::pm_drag::{DM_DROP, DOR_DROP};
        let (loader, hmq, _) = setup();
        let (frame, client) = std_frame(&loader, hmq);
        let (ox, oy) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            if let Some(w) = wm.get_window_mut(frame) {
                (w.x, w.y, w.visible, w.frame_flags) = (100, 50, true, FCF_TITLEBAR | FCF_SIZEBORDER);
            }
            wm.layout_frame(frame);
            if let Some(c) = wm.get_window_mut(client) { (c.visible, c.pfn_wp) = (true, PROC); }
            wm.z_push_top(frame);
            wm.window_origin(client).unwrap()
        };
        // The window under a point of the frame, with this process's id.
        let pid = loader.shared.process_id();
        assert_eq!(loader.answer(1, frame, &WinCall::DragTarget { x: ox + 10, y: oy + 10 }),
                   Some(WinReply::Windows { pid, hwnds: vec![client] }));
        assert_eq!(loader.answer(2, frame, &WinCall::DragTarget { x: 2, y: 2 }), Some(WinReply::Failed), "outside");
        // A DRAGINFO outside the session's arena cannot be read here.
        let over = WinCall::Drag { msg: DM_DRAGOVER, pdinfo: 0x3000, x: ox + 10, y: oy + 10 };
        assert_eq!(loader.answer(3, client, &over), Some(WinReply::Value(DOR_NEVERDROP)));
        // DM_DRAGLEAVE needs none: it waits for the window's thread, which
        // calls the procedure and answers with its reply.
        let leave = WinCall::Drag { msg: DM_DRAGLEAVE, pdinfo: 0x3000, x: 0, y: 0 };
        assert_eq!(loader.answer(4, client, &leave), None, "answered later");
        let s = loader.shared.window_mgr.lock_or_recover().take_sent(hmq).unwrap();
        assert_eq!(s.kind, SentKind::Drag { msg: DM_DRAGLEAVE, pdinfo: 0x3000, x: 0, y: 0 });
        let c = chain(loader.run_sent(TID, s, AfterSent::Rerun(1)));
        assert_eq!(c.steps.front(), Some(&CallStep::send(PROC, client, DM_DRAGLEAVE, 0x3000, 0)));
        assert!(c.inter_thread_send);
        let mut c = *c;
        c.steps.clear();
        c.returned(DOR_DROP, false);
        assert_eq!(loader.finish_chain(&c), ChainEnd::Rerun(1));
        // A window that takes no part (no procedure, not a container)
        // never accepts.
        loader.shared.window_mgr.lock_or_recover().get_window_mut(client).unwrap().pfn_wp = 0;
        let s = SentMessage { id: None, hwnd: client, kind: SentKind::Drag { msg: DM_DROP, pdinfo: 0x3000, x: 0, y: 0 } };
        let c = chain(loader.run_sent(TID, s, AfterSent::Rerun(1)));
        assert!(c.steps.is_empty());
        let ChainThen::AnswerSent { finish: SentFinish::Drag { msg: DM_DROP }, calls: 0, .. } = c.then else { panic!() };
    }

    #[test]
    fn a_message_sent_to_a_thread_that_is_not_dragging_returns() {
        let (loader, _, _) = setup();
        assert_eq!(loader.drag_after_sent(), ChainEnd::Return(0), "no drag: DrgDrag returns NULLHANDLE");
    }

    #[test]
    fn another_program_lists_and_broadcasts_to_this_programs_windows() {
        let (loader, hmq, a) = setup();
        let (frame, client) = std_frame(&loader, hmq);
        let me = slot_address(1);
        let Some(WinReply::Handles(top)) = loader.answer(1, me, &WinCall::Children) else { panic!() };
        assert!(top.contains(&a) && top.contains(&frame) && !top.contains(&client));
        assert_eq!(loader.answer(1, frame, &WinCall::Children), Some(WinReply::Handles(vec![client])));
        assert_eq!(loader.answer(1, slot_address(2), &WinCall::Children), Some(WinReply::Failed), "not this program");
        // Posted to the windows below the desktop.
        let post = WinCall::Broadcast { msg: 0x1005, mp1: 7, mp2: 0, fl: super::super::pm_message::BMSG_DESCENDANTS };
        assert_eq!(loader.answer(2, me, &post), Some(WinReply::Value(1)));
        let mq = loader.shared.window_mgr.lock_or_recover().get_mq(hmq).unwrap();
        let posted: Vec<u32> = mq.lock_or_recover().messages.iter().filter(|m| m.msg == 0x1005).map(|m| m.hwnd).collect();
        assert!(posted.contains(&client) && posted.contains(&frame));
        // Sent: queued for the windows' thread, no answer awaited.
        let send = WinCall::Broadcast { msg: 0x1006, mp1: 0, mp2: 0,
                                        fl: super::super::pm_message::BMSG_SEND | super::super::pm_message::BMSG_FRAMEONLY };
        assert_eq!(loader.answer(3, me, &send), Some(WinReply::Value(1)));
        let s = loader.take_sent(hmq).unwrap();
        assert_eq!((s.id, s.kind), (None, SentKind::Message { msg: 0x1006, mp1: 0, mp2: 0 }));
        assert_eq!(loader.answer(3, me, &WinCall::Broadcast { msg: 1, mp1: 0, mp2: 0, fl: 0x100 }),
                   Some(WinReply::Failed), "an unknown flag");
    }

    #[test]
    fn windows_of_several_programs_follow_the_sessions_stacking() {
        let a = vec![0x20_1001, 0x20_1002, 0x20_1003];
        let b = vec![0x30_1001, 0x30_1002];
        // The server knows the frames 0x30_1001 above 0x20_1002 above 0x20_1001.
        let order = merge_by_stacking(&[0x30_1001, 0x20_1002, 0x40_1000, 0x20_1001], &[a, b]);
        assert_eq!(order, vec![0x30_1001, 0x20_1002, 0x20_1001, 0x20_1003, 0x30_1002],
                   "stacked frames first, then the rest of each program; frames of programs not asked are left out");
    }

    #[test]
    fn enumerating_lists_children_top_first() {
        let (loader, hmq, a) = setup();
        let (frame, client) = std_frame(&loader, hmq);
        let h = normal(loader.win_begin_enum_windows(TID, frame));
        assert_eq!(normal(loader.win_get_next_window(TID, h)), client);
        assert_eq!(normal(loader.win_get_next_window(TID, h)), 0);
        assert_eq!(normal(loader.win_end_enum_windows(TID, h)), 1);
        assert_eq!(normal(loader.win_end_enum_windows(TID, h)), 0);
        assert_eq!(loader.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_HENUM);
        // The desktop: without a desktop server only this program's.
        loader.shared.window_mgr.lock_or_recover().z_push_top(frame);
        loader.shared.window_mgr.lock_or_recover().z_push_top(a);
        let h = normal(loader.win_begin_enum_windows(TID, HWND_DESKTOP));
        assert_eq!(normal(loader.win_get_next_window(TID, h)), a, "the top frame first");
        assert_eq!(normal(loader.win_get_next_window(TID, h)), frame);
        assert_eq!(normal(loader.win_begin_enum_windows(TID, frame + 99)), 0, "no window");
        assert_eq!(loader.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_HWND);
    }
}
