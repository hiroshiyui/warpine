// SPDX-License-Identifier: GPL-3.0-only

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
//!
//! Warpine's choices (no public source says how PM does these):
//! - A call that gets no answer — the window is gone, its program ended or
//!   does not answer in time — fails with `PMERR_INVALID_HWND`.
//! - Window text and class names come back in the caller's codepage (the
//!   calling thread's queue codepage).
//! - `WinSetWindowPos` that minimizes, maximizes or restores another
//!   program's frame does not ask its client (`WM_MINMAXFRAME`): that would
//!   need the window's thread, as `WinSendMsg` does.
//!
//! Posted message parameters are passed as they are: a pointer means
//! something in the other program only for shared memory, as in PM.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_hook::{CallChain, CallStep, ChainEnd, ChainThen};
use super::pm_error::window_word_in_range;
use super::pm_types::{handle_in_slot, WindowManager};
use super::pm_win::bound_window_geometry;
use super::{ApiResult, Loader};
use crate::desktop::client::{CallState, CallWaker, Responder};
use crate::desktop::proto::{WinCall, WinReply, WindowInfo, MAX_COORD};

/// `WinPostMsg`.
const ORD_POST_MSG: u32 = 919;
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
const ORD_SEND_MSG: u32 = super::pm_hook::ORD_WIN_SEND_MSG;
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

/// A message another program sent to one of this program's windows, waiting
/// for the window's thread (module comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentMessage {
    /// The server's id of the call, for the answer.
    pub id: u64,
    pub hwnd: u32,
    pub msg: u32,
    pub mp1: u32,
    pub mp2: u32,
}

/// What a thread does once it has run a [`SentMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterSent {
    /// Make PMWIN call `ordinal` again (`WinGetMsg`, `WinPeekMsg`,
    /// `WinWaitMsg`).
    Rerun(u32),
    /// Go on waiting in its own `WinSendMsg` to another program.
    ResumeSend,
    /// Go on with the modal loop of dialog `hwnd_dlg`.
    ModalLoop { dlg_proc: u32, hwnd_dlg: u32, hmq: u32 },
}

/// The messages this program's threads are sending to other programs'
/// windows: per thread, the link's call ids, innermost last.
#[derive(Debug, Default)]
pub struct RemoteSends {
    stacks: HashMap<u32, Vec<u32>>,
    /// Threads whose `WinSendMsg` is made again after running a message
    /// sent to them: it goes on waiting for its answer.
    resume: HashSet<u32>,
}

impl RemoteSends {
    fn begin(&mut self, tid: u32, id: u32) {
        self.stacks.entry(tid).or_default().push(id);
    }

    /// The send thread `tid` goes on waiting for, if its `WinSendMsg` is
    /// made again after running a sent message (else it is a new send).
    fn resumed(&mut self, tid: u32) -> Option<u32> {
        if !self.resume.remove(&tid) { return None; }
        self.stacks.get(&tid).and_then(|s| s.last().copied())
    }

    fn mark_resume(&mut self, tid: u32) {
        self.resume.insert(tid);
    }

    fn end(&mut self, tid: u32, id: u32) {
        if let Some(s) = self.stacks.get_mut(&tid) {
            s.retain(|&i| i != id);
            if s.is_empty() { self.stacks.remove(&tid); }
        }
    }

    /// Thread `tid` ended: the sends it was waiting for.
    pub fn forget_thread(&mut self, tid: u32) -> Vec<u32> {
        self.resume.remove(&tid);
        self.stacks.remove(&tid).unwrap_or_default()
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

    /// The first message from another program waiting in queue `hmq`.
    pub fn take_sent(&self, hmq: u32) -> Option<SentMessage> {
        self.get_mq(hmq)?.lock_or_recover().sent.pop_front()
    }

    /// The ids of all messages from other programs waiting in queue `hmq`,
    /// taken out (the queue goes away).
    pub fn drain_sent(&self, hmq: u32) -> Vec<u64> {
        self.get_mq(hmq).map_or_else(Vec::new, |mq| mq.lock_or_recover().sent.drain(..).map(|s| s.id).collect())
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
    fn describe_remote(&self, tid: u32, hwnd: u32) -> Option<WindowInfo> {
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
            ORD_SEND_MSG => self.remote_send(tid, hwnd, arg(8), arg(12), arg(16)),
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
                ApiResult::Normal(self.value_remote(tid, hwnd, call))
            }
            ORD_SHOW_WINDOW => ApiResult::Normal(self.value_remote(tid, hwnd, WinCall::Show { show: arg(8) != 0 })),
            ORD_SET_ACTIVE_WINDOW => ApiResult::Normal(self.value_remote(tid, hwnd, WinCall::Activate)),
            _ => unreachable!("ordinal {ordinal} is in REMOTE_CALLS"),
        }
    }

    /// `WinSendMsg(hwnd, msg, mp1, mp2)` to another program's window: wait
    /// for the window procedure's result, running the messages other
    /// programs send to this thread's queue meanwhile (module comment).
    fn remote_send(&self, tid: u32, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> ApiResult {
        let Some(link) = self.shared.desktop_link.get() else { return self.pm_fail(tid, PMERR_INVALID_HWND) };
        let (hmq, wait, resumed) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let hmq = wm.thread_queue(tid);
            let wait = hmq.and_then(|h| wm.get_mq(h)).map(|mq| {
                let q = mq.lock_or_recover();
                (Arc::clone(&q.cond), Arc::clone(&q.lock))
            });
            (hmq, wait, wm.remote_sends.resumed(tid))
        };
        let id = match resumed {
            Some(id) => id,
            None => {
                // The answer wakes the thread where it waits: its queue.
                let wake = wait.clone().map(|(cond, lock)| -> CallWaker {
                    Arc::new(move || {
                        let _g = lock.lock_or_recover();
                        cond.notify_all();
                    })
                });
                let Some(id) = link.begin_call(hwnd, WinCall::Send { msg, mp1, mp2 }, wake) else {
                    return self.pm_fail(tid, PMERR_INVALID_HWND);
                };
                self.shared.window_mgr.lock_or_recover().remote_sends.begin(tid, id);
                id
            }
        };
        let end = |r: ApiResult| {
            self.shared.window_mgr.lock_or_recover().remote_sends.end(tid, id);
            r
        };
        loop {
            let state = match wait {
                Some(_) => link.poll_call(id),
                None => link.wait_call(id, SEND_WAIT_SLICE),
            };
            match state {
                CallState::Answered(WinReply::Value(v)) => return end(ApiResult::Normal(v)),
                CallState::Answered(_) | CallState::Gone => return end(self.pm_fail(tid, PMERR_INVALID_HWND)),
                CallState::Waiting => {}
            }
            // Messages sent to this thread meanwhile run first.
            if let Some(s) = hmq.and_then(|h| self.shared.window_mgr.lock_or_recover().take_sent(h)) {
                return self.run_sent(tid, s, AfterSent::ResumeSend);
            }
            if self.stop_or_restart() {
                link.abandon_call(id);
                return end(ApiResult::Normal(0));
            }
            if let Some((cond, lock)) = &wait {
                let g = lock.lock_or_recover();
                let _ = cond.wait_timeout(g, SEND_WAIT_SLICE);
            }
        }
    }

    /// Run message `s` from another program on thread `tid`: a chain calling
    /// its window's procedure (or the built-in handling), then answering
    /// with the result and going on as `after` says.
    pub(super) fn run_sent(&self, tid: u32, s: SentMessage, after: AfterSent) -> ApiResult {
        let proc_addr = self.shared.window_mgr.lock_or_recover().get_window(s.hwnd).map_or(0, |w| w.pfn_wp);
        let mut chain = CallChain::new(tid, vec![CallStep::send(proc_addr, s.hwnd, s.msg, s.mp1, s.mp2)], false,
                                       ChainThen::AnswerSent { id: s.id, after });
        // WinInSendMsg is TRUE while the procedure runs.
        chain.inter_thread_send = true;
        self.start_chain(chain)
    }

    /// The chain of a [`SentMessage`] is done: answer with the procedure's
    /// result (`Failed` when it was not called: the window is gone) and say
    /// how the thread goes on.
    pub(super) fn sent_done(&self, id: u64, after: AfterSent, chain: &CallChain) -> ChainEnd {
        let reply = chain.results.first().map_or(WinReply::Failed, |&v| WinReply::Value(v));
        if let Some(link) = self.shared.desktop_link.get() {
            link.answer(id, reply);
        }
        match after {
            AfterSent::Rerun(ordinal) => ChainEnd::Rerun(ordinal),
            AfterSent::ResumeSend => {
                self.shared.window_mgr.lock_or_recover().remote_sends.mark_resume(chain.tid);
                ChainEnd::Rerun(ORD_SEND_MSG)
            }
            AfterSent::ModalLoop { dlg_proc, hwnd_dlg, hmq } => ChainEnd::ModalLoop { dlg_proc, hwnd_dlg, hmq, msg: None },
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

    /// `WinSetWindowPos` made by another program on this program's window.
    #[allow(clippy::too_many_arguments)]
    fn set_pos_for_other(&self, hwnd: u32, behind: u32, x: i32, y: i32, cx: i32, cy: i32, fl: u32) -> bool {
        let (x, y, cx, cy) = bound_window_geometry(x, y, cx, cy);
        // The insert-behind window must be one of ours (or HWND_TOP / BOTTOM).
        let behind = if behind == HWND_TOP || behind == HWND_BOTTOM
            || self.shared.window_mgr.lock_or_recover().is_window(behind) { behind } else { 0 };
        if let Some((op, target)) = self.set_window_pos(hwnd, behind, x, y, cx, cy, fl) {
            // A state change: made without asking the client (module comment).
            self.shared.window_mgr.lock_or_recover().finish_minmax(hwnd, op, target);
        }
        true
    }
}

impl Responder for Loader {
    /// A call another program makes on window `hwnd` of this one: only on
    /// windows of this program's slot that exist. A sent message is queued
    /// for the window's thread, which answers it later.
    fn answer(&self, id: u64, hwnd: u32, call: &WinCall) -> Option<WinReply> {
        let mine = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.handle_slot.is_some_and(|s| handle_in_slot(hwnd, s)) && wm.is_window(hwnd)
        };
        if !mine {
            return Some(WinReply::Failed);
        }
        if let WinCall::Send { msg, mp1, mp2 } = *call {
            let wm = self.shared.window_mgr.lock_or_recover();
            let queued = wm.find_hmq_for_hwnd(hwnd)
                .is_some_and(|hmq| wm.queue_sent(hmq, SentMessage { id, hwnd, msg, mp1, mp2 }));
            return (!queued).then_some(WinReply::Failed);
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
            WinCall::SetPos { behind, x, y, cx, cy, fl } => value(self.set_pos_for_other(hwnd, behind, x, y, cx, cy, fl)),
            WinCall::Show { show } => {
                self.shared.window_mgr.lock_or_recover().show_window(hwnd, show);
                WinReply::Value(1)
            }
            WinCall::Activate => value(self.shared.window_mgr.lock_or_recover().set_active_window(hwnd)),
            WinCall::SetText { ref text } => {
                self.set_window_text(hwnd, text.clone());
                WinReply::Value(1)
            }
            // Queued by `answer`.
            WinCall::Send { .. } => WinReply::Failed,
        }
    }
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
        assert_eq!(wm.take_sent(hmq), Some(SentMessage { id: 7, hwnd: frame, msg: 0x1001, mp1: 1, mp2: 2 }));
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
        assert_eq!(c.then, ChainThen::AnswerSent { id: 5, after: AfterSent::Rerun(ORD_WAIT_MSG) });
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
        let s = SentMessage { id: 3, hwnd: frame, msg: 1, mp1: 0, mp2: 0 };
        let modal = AfterSent::ModalLoop { dlg_proc: 1, hwnd_dlg: 2, hmq: 3 };
        let c = chain(loader.run_sent(TID, s, modal));
        assert_eq!(loader.finish_chain(&c), ChainEnd::ModalLoop { dlg_proc: 1, hwnd_dlg: 2, hmq: 3, msg: None });
        // A send waiting for its answer goes on waiting for the same one.
        loader.shared.window_mgr.lock_or_recover().remote_sends.begin(TID, 41);
        let c = chain(loader.run_sent(TID, s, AfterSent::ResumeSend));
        assert_eq!(loader.finish_chain(&c), ChainEnd::Rerun(ORD_SEND_MSG));
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.remote_sends.resumed(TID), Some(41));
        assert_eq!(wm.remote_sends.resumed(TID), None, "a later WinSendMsg is a new send");
    }

    #[test]
    fn remote_sends_nest_per_thread() {
        let mut r = RemoteSends::default();
        r.begin(1, 10);
        r.begin(1, 11);
        r.begin(2, 20);
        r.mark_resume(1);
        assert_eq!(r.resumed(1), Some(11), "the innermost");
        r.end(1, 11);
        r.mark_resume(1);
        assert_eq!(r.resumed(1), Some(10));
        assert_eq!(r.forget_thread(2), vec![20]);
        assert_eq!(r.forget_thread(2), Vec::<u32>::new());
        r.end(1, 10);
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
        assert_eq!(normal(loader.remote_send(TID, foreign, 1, 0, 0)), 0);
        assert_eq!(loader.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_HWND);
    }
}
