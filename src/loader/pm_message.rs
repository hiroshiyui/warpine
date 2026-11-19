// SPDX-License-Identifier: GPL-3.0-only

//! PM message calls beyond posting and sending to one window:
//! `WinSendMsg` (920) with its send hooks and inter-thread sends,
//! `WinPostQueueMsg` (902), `WinBroadcastMsg` (901), `WinInSendMsg` (761),
//! `WinWaitMsg` (897), `WinWaitEventSem` (978), `WinRequestMutexSem` (979),
//! and the help hook of `WM_HELP`.
//!
//! # Sent messages and threads
//!
//! Warpine runs a window procedure on the thread that sends the message,
//! whichever thread's queue the window belongs to; PM would switch to the
//! receiving thread and run the procedure there while the sender waits.
//! The receiving thread therefore never processes sent messages while it
//! waits, and `WinWaitEventSem` / `WinRequestMutexSem` — which differ from
//! their `Dos` counterparts only in doing so — wait like them.
//! `WinInSendMsg` answers TRUE while the calling thread runs a window
//! procedure for a message sent to a window of another thread's queue
//! (where PM would answer on the receiving thread).
//!
//! Between programs it is PM's way (`pm_remote.rs`, ADR-0054): a message
//! sent to another program's window runs on that window's thread when it
//! reads or waits for messages, and `WinInSendMsg` is TRUE there. Those
//! waits (`WinGetMsg`, `WinPeekMsg`, `WinWaitMsg`, modal loops, a send to
//! another program) run them; `WinWaitEventSem` / `WinRequestMutexSem` do
//! not.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_hook::{CallChain, CallStep, ChainThen, HK_HELP, HLPM_FRAME, HLPM_MENU, HLPM_WINDOW};
use super::pm_types::{OS2Message, WindowManager};
use super::{ApiResult, Loader};

/// `WinBroadcastMsg` `fl`: post the message (no `BMSG_SEND`).
pub const BMSG_POST: u32 = 0x0000;
/// `fl`: send the message.
pub const BMSG_SEND: u32 = 0x0001;
/// `fl`: post it to every message queue instead of to windows.
pub const BMSG_POSTQUEUE: u32 = 0x0002;
/// `fl`: to all descendants, not only the children.
pub const BMSG_DESCENDANTS: u32 = 0x0004;
/// `fl`: to frame windows only.
pub const BMSG_FRAMEONLY: u32 = 0x0008;
/// Every `BMSG_*` flag.
pub const BMSG_ALL: u32 = BMSG_SEND | BMSG_POSTQUEUE | BMSG_DESCENDANTS | BMSG_FRAMEONLY;

/// Words of a `RECTL`.
const RECTL_WORDS: usize = 4;

fn is_desktop(hwnd: u32) -> bool {
    hwnd == 0 || hwnd == HWND_DESKTOP
}

impl WindowManager {
    /// The windows `WinBroadcastMsg` reaches below `parent`: its children
    /// (the desktop's: the main windows; `HWND_OBJECT`'s: the object
    /// windows), with `descendants` all windows below it in pre-order, with
    /// `frame_only` only frame windows (standard frames and dialogs) among
    /// them — the others are still searched below.
    pub fn broadcast_targets(&self, parent: u32, descendants: bool, frame_only: bool) -> Vec<u32> {
        let children = |h: u32| -> Vec<u32> {
            if h == HWND_OBJECT {
                let mut v: Vec<u32> = self.windows().filter(|w| w.parent == HWND_OBJECT).map(|w| w.handle).collect();
                v.sort_unstable();
                v
            } else {
                self.child_windows(h)
            }
        };
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Depth-first, pre-order: a stack of children still to visit.
        let mut stack: Vec<u32> = children(parent).into_iter().rev().collect();
        while let Some(h) = stack.pop() {
            if !self.is_window(h) || !seen.insert(h) {
                continue;
            }
            if !frame_only || self.is_frame(h) {
                out.push(h);
            }
            if descendants {
                stack.extend(children(h).into_iter().rev());
            }
        }
        out
    }

    /// The queue a window belongs to (not a fallback: 0 for none).
    fn own_queue(&self, hwnd: u32) -> u32 {
        self.get_window(hwnd).map_or(0, |w| w.hmq)
    }
}

impl Loader {
    /// `WinSendMsg(hwnd, msg, mp1, mp2)` (920): the send hooks of the
    /// window's queue first (the call is then made again), then the
    /// window procedure — on this thread, under a chain that `WinInSendMsg`
    /// sees when the window belongs to another thread's queue — or the
    /// built-in handling.
    pub(crate) fn win_send_msg(&self, tid: u32, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> ApiResult {
        // Taken first: the note must not outlive this call.
        let hooks_done = self.shared.window_mgr.lock_or_recover().hooks.take_send_hooks_done(tid);
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let (pfn_wp, target, caller) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            (wm.get_window(hwnd).map_or(0, |w| w.pfn_wp), wm.own_queue(hwnd), wm.thread_queue(tid).unwrap_or(0))
        };
        let inter_thread = target != 0 && target != caller;
        if !hooks_done && target != 0 {
            let steps = self.send_hook_steps(tid, target, [hwnd, msg, mp1, mp2], inter_thread);
            if !steps.is_empty() {
                return self.start_chain(CallChain::new(tid, steps, true, ChainThen::SendAgain));
            }
        }
        if pfn_wp == 0 {
            // No guest window procedure: the built-in control handler
            // (LM_INSERTITEM, LM_QUERYITEMCOUNT, ...).
            return self.dispatch_builtin_control(hwnd, msg, mp1, mp2);
        }
        if inter_thread {
            let mut chain = CallChain::new(tid, vec![CallStep::send(pfn_wp, hwnd, msg, mp1, mp2)],
                                           false, ChainThen::ReturnLast);
            chain.inter_thread_send = true;
            return self.start_chain(chain);
        }
        ApiResult::Callback { wnd_proc: pfn_wp, hwnd, msg, mp1, mp2 }
    }

    /// `WinPostQueueMsg(hmq, msg, mp1, mp2)` (902): post a message for no
    /// window (`QMSG.hwnd` NULLHANDLE) to queue `hmq`. FALSE with
    /// `PMERR_INVALID_HMQ` for a handle that is no queue.
    pub(crate) fn win_post_queue_msg(&self, tid: u32, hmq: u32, msg: u32, mp1: u32, mp2: u32) -> ApiResult {
        let mq = self.shared.window_mgr.lock_or_recover().get_mq(hmq);
        let Some(mq) = mq else { return self.pm_fail(tid, PMERR_INVALID_HMQ) };
        let mut q = mq.lock_or_recover();
        q.messages.push_back(OS2Message { hwnd: 0, msg, mp1, mp2, time: 0, x: 0, y: 0 });
        q.cond.notify_one();
        ApiResult::Normal(1)
    }

    /// `WinBroadcastMsg(hwnd, msg, mp1, mp2, fl)` (901): post or send the
    /// message to the children (`BMSG_DESCENDANTS`: all descendants) of
    /// `hwnd` (`BMSG_FRAMEONLY`: frame windows only), or with
    /// `BMSG_POSTQUEUE` post it to every message queue with no window
    /// (unverified: that `hwnd` and the other flags are then ignored). For
    /// the desktop and `BMSG_POSTQUEUE` it reaches the other programs of
    /// the session too, which run sent ones without this call waiting
    /// (`pm_remote.rs`). Sent messages are delivered one after the other, each
    /// after the send hooks of its window's queue. TRUE; FALSE for an
    /// unknown flag (`PMERR_INVALID_FLAG`, unverified) or a handle that is
    /// no window.
    pub(crate) fn win_broadcast_msg(&self, tid: u32, hwnd: u32, msg: u32, mp1: u32, mp2: u32, fl: u32) -> ApiResult {
        if fl & !BMSG_ALL != 0 {
            return self.pm_fail(tid, PMERR_INVALID_FLAG);
        }
        // The desktop's children and the queues of the session include
        // other programs' (pm_remote.rs).
        if fl & BMSG_POSTQUEUE != 0 || is_desktop(hwnd) {
            self.broadcast_to_others(msg, mp1, mp2, fl);
        }
        if fl & BMSG_POSTQUEUE != 0 {
            let queues: Vec<_> = {
                let wm = self.shared.window_mgr.lock_or_recover();
                wm.msg_queues.values().cloned().collect()
            };
            for mq in queues {
                let mut q = mq.lock_or_recover();
                q.messages.push_back(OS2Message { hwnd: 0, msg, mp1, mp2, time: 0, x: 0, y: 0 });
                q.cond.notify_one();
            }
            return ApiResult::Normal(1);
        }
        let parent = if hwnd == 0 { HWND_DESKTOP } else { hwnd };
        if !self.pm_check_hwnd(tid, parent, false) { return ApiResult::Normal(0); }
        let targets = self.shared.window_mgr.lock_or_recover()
            .broadcast_targets(parent, fl & BMSG_DESCENDANTS != 0, fl & BMSG_FRAMEONLY != 0);
        if fl & BMSG_SEND == BMSG_POST {
            for h in targets {
                self.post_to_window(h, msg, mp1, mp2);
            }
            return ApiResult::Normal(1);
        }
        let caller = self.shared.window_mgr.lock_or_recover().thread_queue(tid).unwrap_or(0);
        let mut steps = Vec::new();
        for h in targets {
            let (pfn_wp, target) = {
                let wm = self.shared.window_mgr.lock_or_recover();
                (wm.get_window(h).map_or(0, |w| w.pfn_wp), wm.own_queue(h))
            };
            if target != 0 {
                let inter_thread = target != caller;
                steps.extend(self.send_hook_steps(tid, target, [h, msg, mp1, mp2], inter_thread));
            }
            steps.push(CallStep::send(pfn_wp, h, msg, mp1, mp2));
        }
        if steps.is_empty() {
            return ApiResult::Normal(1);
        }
        self.start_chain(CallChain::new(tid, steps, false, ChainThen::Return(1)))
    }

    /// `WinInSendMsg(hab)` (761): see the module comment.
    pub(crate) fn win_in_send_msg(&self, tid: u32) -> ApiResult {
        ApiResult::Normal(self.shared.window_mgr.lock_or_recover().hooks.in_send(tid) as u32)
    }

    /// `WinWaitMsg(hab, msgFirst, msgLast)` (897): wait until the caller's
    /// queue holds a message in the range (`msg_in_range`; WM_QUIT always
    /// counts), leaving it there. TRUE; FALSE without a queue
    /// (`PMERR_NO_MSG_QUEUE`) or when the process ends.
    pub(crate) fn win_wait_msg(&self, tid: u32, first: u32, last: u32) -> ApiResult {
        if !self.has_msg_queue(tid) { return self.pm_fail(tid, PMERR_NO_MSG_QUEUE); }
        match self.next_message(tid, 0, 0, first, last, 0, true, true) {
            // A message from another program runs, then the wait goes on.
            Some(super::pm_win::Got::Sent(s)) =>
                self.run_sent(tid, s, super::pm_remote::AfterSent::Rerun(super::pm_remote::ORD_WAIT_MSG)),
            found => ApiResult::Normal(found.is_some() as u32),
        }
    }

    /// `WinDefWindowProc(hwnd, WM_HELP, mp1, mp2)`: a frame or a frame's
    /// client (or another main window) calls the help hooks of its queue;
    /// another window passes the message on to its parent (posted;
    /// unverified: PM sends it). The hook's arguments (unverified, see
    /// `HelpHook` in the PM Reference):
    ///
    /// * from a menu (`CMDSRC_MENU`): `HLPM_MENU`, `FID_MENU`, the item id;
    /// * in a frame: `HLPM_FRAME`, the frame's id, the focus window's id;
    /// * in another main window: `HLPM_WINDOW`, its id, the focus window's id;
    ///
    /// and `prcPosition` = the frame's (window's) rectangle on the desktop.
    pub(crate) fn help_hook(&self, tid: u32, hwnd: u32, mp1: u32, mp2: u32) -> ApiResult {
        let (target, parent, hmq, frame) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window(hwnd) else { return ApiResult::Normal(0) };
            let frame = if wm.is_frame(hwnd) { hwnd } else { wm.client_to_frame(hwnd) };
            let is_frame = wm.is_frame(frame);
            let target = if is_frame { frame } else { hwnd };
            let hmq = if w.hmq != 0 { w.hmq } else { wm.thread_queue(tid).unwrap_or(0) };
            (target, w.parent, hmq, is_frame)
        };
        if !frame && !is_desktop(parent) {
            self.post_to_window(parent, WM_HELP, mp1, mp2);
            return ApiResult::Normal(0);
        }
        let procs = self.hooks_to_call(tid, hmq, HK_HELP);
        if procs.is_empty() {
            return ApiResult::Normal(0);
        }
        let (args, rect) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let id = |h: u32| wm.get_window(h).map_or(0, |w| w.id);
            let focus = id(wm.focus_hwnd);
            let (mode, topic, sub) = if mp2 & 0xFFFF == CMDSRC_MENU {
                (HLPM_MENU, FID_MENU, mp1 & 0xFFFF)
            } else if frame {
                (HLPM_FRAME, id(target), focus)
            } else {
                (HLPM_WINDOW, id(target), focus)
            };
            let (x, y) = wm.window_origin(target).unwrap_or((0, 0));
            let (cx, cy) = wm.get_window(target).map_or((0, 0), |w| (w.cx, w.cy));
            let rect: [u32; RECTL_WORDS] = [x as u32, y as u32, x.saturating_add(cx) as u32, y.saturating_add(cy) as u32];
            ([MOCK_HAB, mode as u32, topic, sub, 0], rect)
        };
        let steps = procs.into_iter()
            .map(|p| CallStep::hook_with_block(p, args.to_vec(), rect.to_vec(), args.len() - 1, true))
            .collect();
        self.start_chain(CallChain::new(tid, steps, true, ChainThen::Return(0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_hook::{ChainEnd, HK_INPUT, HK_MSGFILTER, HK_SENDMSG, HMQ_CURRENT, PM_MODEL_2X};
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::vm_backend::VcpuBackend;

    const TID: u32 = 0;
    const PROC: u32 = 0x5000;
    /// `FID_CLIENT` (<pmwin.h>).
    const FID_CLIENT: u32 = 0x8008;

    /// A loader with a queue for thread `TID`, a class with procedure
    /// `PROC` and a frame / client pair of it: `(hmq, frame, client)`.
    fn setup() -> (Loader, u32, u32, u32) {
        let loader = Loader::new_mock();
        let (hmq, frame, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(TID, hmq);
            wm.register_class("App".into(), PROC, 0);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            (hmq, frame, client)
        };
        (loader, hmq, frame, client)
    }

    fn queued(loader: &Loader, hmq: u32) -> Vec<(u32, u32, u32, u32)> {
        let mq = loader.shared.window_mgr.lock_or_recover().get_mq(hmq).unwrap();
        let q = mq.lock_or_recover();
        q.messages.iter().map(|m| (m.hwnd, m.msg, m.mp1, m.mp2)).collect()
    }

    fn chain(r: ApiResult) -> Box<CallChain> {
        match r {
            ApiResult::CallChain(c) => c,
            other => panic!("a call chain, got {other:?}"),
        }
    }

    #[test]
    fn post_queue_msg_posts_for_no_window() {
        let (loader, hmq, _, _) = setup();
        assert!(matches!(loader.win_post_queue_msg(TID, hmq, WM_USER + 1, 2, 3), ApiResult::Normal(1)));
        assert_eq!(queued(&loader, hmq), vec![(0, WM_USER + 1, 2, 3)]);
        assert!(matches!(loader.win_post_queue_msg(TID, 0xDEAD, WM_USER, 0, 0), ApiResult::Normal(0)));
        assert_eq!(loader.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_HMQ);
    }

    #[test]
    fn broadcast_targets_children_descendants_and_frames() {
        let (loader, _, frame, client) = setup();
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hmq = wm.thread_queue(TID).unwrap();
        let button = wm.create_window("WC_BUTTON".into(), client, hmq);
        let obj = wm.create_window("App".into(), HWND_OBJECT, hmq);
        assert_eq!(wm.broadcast_targets(frame, false, false), vec![client]);
        assert_eq!(wm.broadcast_targets(frame, true, false), vec![client, button]);
        assert_eq!(wm.broadcast_targets(HWND_DESKTOP, false, false), vec![frame]);
        assert_eq!(wm.broadcast_targets(HWND_DESKTOP, true, false), vec![frame, client, button]);
        assert_eq!(wm.broadcast_targets(HWND_DESKTOP, true, true), vec![frame], "frames only");
        assert_eq!(wm.broadcast_targets(HWND_OBJECT, false, false), vec![obj]);
    }

    #[test]
    fn broadcast_post_queues_for_each_target_and_postqueue_for_each_queue() {
        let (loader, hmq, frame, client) = setup();
        assert!(matches!(loader.win_broadcast_msg(TID, frame, WM_USER, 1, 2, BMSG_POST | BMSG_DESCENDANTS),
                         ApiResult::Normal(1)));
        assert_eq!(queued(&loader, hmq), vec![(client, WM_USER, 1, 2)]);
        let other = loader.shared.window_mgr.lock_or_recover().create_mq(437);
        assert!(matches!(loader.win_broadcast_msg(TID, 0, WM_USER + 5, 0, 0, BMSG_POSTQUEUE), ApiResult::Normal(1)));
        assert_eq!(queued(&loader, other), vec![(0, WM_USER + 5, 0, 0)]);
        assert_eq!(queued(&loader, hmq).last(), Some(&(0, WM_USER + 5, 0, 0)));
        assert!(matches!(loader.win_broadcast_msg(TID, frame, WM_USER, 0, 0, 0x40), ApiResult::Normal(0)));
        assert_eq!(loader.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_FLAG);
        assert!(matches!(loader.win_broadcast_msg(TID, 0xDEAD, WM_USER, 0, 0, BMSG_POST), ApiResult::Normal(0)));
    }

    #[test]
    fn broadcast_send_calls_each_window_procedure_in_turn() {
        let (loader, _, frame, client) = setup();
        let c = chain(loader.win_broadcast_msg(TID, HWND_DESKTOP, WM_USER, 7, 8, BMSG_SEND | BMSG_DESCENDANTS));
        let windows: Vec<u32> = c.steps.iter().map(|s| match s {
            CallStep::Guest { proc_addr, args, .. } => { assert_eq!(*proc_addr, PROC); args[0] }
            CallStep::Builtin { hwnd, .. } => *hwnd,
        }).collect();
        assert_eq!(windows, vec![frame, client]);
        assert_eq!(c.then, ChainThen::Return(1));
    }

    #[test]
    fn send_runs_the_send_hooks_then_the_call_again() {
        let (loader, _, _, client) = setup();
        assert!(matches!(loader.win_set_hook(TID, HMQ_CURRENT, HK_SENDMSG, 0x7000), ApiResult::Normal(1)));
        let c = chain(loader.win_send_msg(TID, client, WM_USER, 1, 2));
        assert_eq!(c.then, ChainThen::SendAgain);
        let CallStep::Guest { proc_addr, args, block, block_arg, .. } = &c.steps[0] else { panic!() };
        assert_eq!((*proc_addr, args.as_slice(), *block_arg), (0x7000, &[MOCK_HAB, 0, 0][..], 1));
        assert_eq!(block, &vec![2, 1, WM_USER, client, PM_MODEL_2X], "SMHSTRUCT");
        assert!(loader.shared.window_mgr.lock_or_recover().hooks.in_hook(TID));
        assert_eq!(loader.finish_chain(&c), ChainEnd::Rerun(super::super::pm_hook::ORD_WIN_SEND_MSG));
        // The call made again goes to the window procedure, without hooks.
        assert!(matches!(loader.win_send_msg(TID, client, WM_USER, 1, 2),
                         ApiResult::Callback { wnd_proc: PROC, msg: WM_USER, .. }));
        assert!(matches!(loader.win_send_msg(TID, client, WM_USER, 1, 2), ApiResult::CallChain(_)),
                "the next send calls the hooks again");
    }

    #[test]
    fn a_send_to_another_threads_window_is_an_inter_thread_send() {
        let (loader, _, _, client) = setup();
        const OTHER: u32 = 5;
        loader.shared.window_mgr.lock_or_recover().hooks.set(0, HK_SENDMSG, 0x7000);
        // The other thread has a queue of its own.
        let other_q = loader.shared.window_mgr.lock_or_recover().create_mq(437);
        loader.shared.window_mgr.lock_or_recover().tid_to_hmq.insert(OTHER, other_q);
        let c = chain(loader.win_send_msg(OTHER, client, WM_USER, 0, 0));
        let CallStep::Guest { args, .. } = &c.steps[0] else { panic!() };
        assert_eq!(args[2], 1, "fInterTask");
        loader.finish_chain(&c);
        let c = chain(loader.win_send_msg(OTHER, client, WM_USER, 0, 0));
        assert!(c.inter_thread_send && c.then == ChainThen::ReturnLast);
        assert!(matches!(loader.win_in_send_msg(OTHER), ApiResult::Normal(1)));
        assert!(matches!(loader.win_in_send_msg(TID), ApiResult::Normal(0)));
        assert_eq!(loader.finish_chain(&c), ChainEnd::Return(0));
        assert!(matches!(loader.win_in_send_msg(OTHER), ApiResult::Normal(0)));
    }

    #[test]
    fn no_hooks_run_while_a_thread_runs_hooks() {
        let (loader, _, _, client) = setup();
        loader.win_set_hook(TID, HMQ_CURRENT, HK_SENDMSG, 0x7000);
        let c = chain(loader.win_send_msg(TID, client, WM_USER, 0, 0));
        // A hook procedure sending a message: straight to the window.
        assert!(matches!(loader.win_send_msg(TID, client, WM_USER, 0, 0), ApiResult::Callback { .. }));
        loader.finish_chain(&c);
    }

    #[test]
    fn call_msg_filter_returns_whether_a_hook_handled_the_message() {
        let (loader, _, _, _) = setup();
        assert!(matches!(loader.win_call_msg_filter(TID, 0x9000, 42), ApiResult::Normal(0)), "no hooks");
        loader.win_set_hook(TID, HMQ_CURRENT, HK_MSGFILTER, 0x7000);
        loader.win_set_hook(TID, HMQ_CURRENT, HK_MSGFILTER, 0x7100);
        let mut c = chain(loader.win_call_msg_filter(TID, 0x9000, 42));
        let procs: Vec<u32> = c.steps.iter().map(|s| match s { CallStep::Guest { proc_addr, .. } => *proc_addr, _ => 0 }).collect();
        assert_eq!(procs, vec![0x7100, 0x7000]);
        let CallStep::Guest { args, .. } = c.steps.pop_front().unwrap() else { panic!() };
        assert_eq!(args, vec![MOCK_HAB, 0x9000, 42]);
        c.returned(1, true);
        assert_eq!(loader.finish_chain(&c), ChainEnd::Return(1));
        assert!(!loader.shared.window_mgr.lock_or_recover().hooks.in_hook(TID));
    }

    #[test]
    fn set_hook_checks_the_queue_type_and_procedure() {
        let (loader, hmq, _, _) = setup();
        let fails = |r: ApiResult, code: u32| {
            assert!(matches!(r, ApiResult::Normal(0)));
            assert_eq!(loader.shared.pm_errors.take(TID) & 0xFFFF, code);
        };
        fails(loader.win_set_hook(TID, 0xDEAD, HK_INPUT, 0x7000), PMERR_INVALID_HMQ);
        fails(loader.win_set_hook(9, HMQ_CURRENT, HK_INPUT, 0x7000), 0);
        fails(loader.win_set_hook(TID, HMQ_CURRENT, 14, 0x7000), PMERR_PARAMETER_OUT_OF_RANGE);
        fails(loader.win_set_hook(TID, HMQ_CURRENT, HK_INPUT, 0), PMERR_PARAMETER_OUT_OF_RANGE);
        assert!(matches!(loader.win_set_hook(TID, hmq, HK_INPUT, 0x7000), ApiResult::Normal(1)));
        assert!(matches!(loader.win_set_hook(TID, 0, HK_INPUT, 0x7100), ApiResult::Normal(1)), "process hook");
        assert_eq!(loader.hooks_to_call(TID, hmq, HK_INPUT), vec![0x7000, 0x7100]);
        assert!(matches!(loader.win_release_hook(TID, HMQ_CURRENT, HK_INPUT, 0x7000), ApiResult::Normal(1)));
        fails(loader.win_release_hook(TID, HMQ_CURRENT, HK_INPUT, 0x7000), PMERR_PARAMETER_OUT_OF_RANGE);
    }

    /// The input hooks see the untranslated message in the caller's QMSG;
    /// a hook returning TRUE makes WinGetMsg run again, while a peek that
    /// leaves the message returns FALSE.
    #[test]
    fn input_hooks_end_a_get_or_peek() {
        let (loader, hmq, _, client) = setup();
        loader.win_set_hook(TID, HMQ_CURRENT, HK_INPUT, 0x7000);
        let pqmsg = 0x9000;
        {
            let mq = loader.shared.window_mgr.lock_or_recover().get_mq(hmq).unwrap();
            mq.lock_or_recover().messages.push_back(OS2Message { hwnd: client, msg: WM_USER, mp1: 1, mp2: 2, time: 0, x: 0, y: 0 });
        }
        let mut vcpu = MockVcpu::new();
        vcpu.regs.rsp = 0x8_0000;
        for (i, v) in [0x4000u32, MOCK_HAB, pqmsg, 0, 0, 0].into_iter().enumerate() {
            loader.guest_write::<u32>(0x8_0000 + 4 * i as u32, v).unwrap();
        }
        const ORD_WIN_GET_MSG: u32 = 915;
        let mut c = chain(loader.handle_pmwin_call(&mut vcpu, TID, ORD_WIN_GET_MSG));
        assert_eq!(loader.guest_read::<u32>(pqmsg + 4).unwrap(), WM_USER, "the message is in the QMSG");
        let CallStep::Guest { args, .. } = c.steps.pop_front().unwrap() else { panic!() };
        assert_eq!(args, vec![MOCK_HAB, pqmsg, PM_REMOVE]);
        c.returned(0, true);
        assert_eq!(loader.finish_chain(&c), ChainEnd::Return(1), "passed on: TRUE");
        c.stopped = true;
        assert_eq!(loader.finish_chain(&c), ChainEnd::Rerun(ORD_WIN_GET_MSG), "swallowed: the next one");
        let peek = CallChain::new(TID, vec![], true,
            ChainThen::Message { ordinal: 918, pqmsg, remove: false, peek: true });
        let mut peek = peek;
        peek.stopped = true;
        assert_eq!(loader.finish_chain(&peek), ChainEnd::Return(0));
        // WM_QUIT let through ends WinGetMsg with FALSE.
        loader.guest_write::<u32>(pqmsg + 4, WM_QUIT).unwrap();
        c.stopped = false;
        assert_eq!(loader.finish_chain(&c), ChainEnd::Return(0));
        let _ = vcpu.get_regs();
    }

    #[test]
    fn help_hooks_get_the_frame_mode_ids_and_position() {
        let (loader, _, frame, client) = setup();
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let f = wm.get_window_mut(frame).unwrap();
            (f.id, f.x, f.y, f.cx, f.cy) = (77, 10, 20, 300, 200);
            wm.get_window_mut(client).unwrap().id = FID_CLIENT;
            wm.focus_hwnd = client;
        }
        assert!(matches!(loader.help_hook(TID, client, 5, 0), ApiResult::Normal(0)), "no hooks");
        loader.win_set_hook(TID, HMQ_CURRENT, HK_HELP, 0x7000);
        let c = chain(loader.help_hook(TID, client, 5, 0));
        let CallStep::Guest { args, block, block_arg, stop_on_true, .. } = &c.steps[0] else { panic!() };
        assert_eq!(args[..4], [MOCK_HAB, HLPM_FRAME as u32, 77, FID_CLIENT]);
        assert_eq!((*block_arg, *stop_on_true), (4, true));
        assert_eq!(block, &vec![10, 20, 310, 220]);
        loader.finish_chain(&c);
        let c = chain(loader.help_hook(TID, client, 0x123, CMDSRC_MENU));
        let CallStep::Guest { args, .. } = &c.steps[0] else { panic!() };
        assert_eq!(args[1..4], [HLPM_MENU as u32, FID_MENU, 0x123]);
        loader.finish_chain(&c);
    }

    #[test]
    fn help_in_a_child_window_goes_to_its_parent() {
        let (loader, hmq, _, client) = setup();
        loader.win_set_hook(TID, HMQ_CURRENT, HK_HELP, 0x7000);
        let button = loader.shared.window_mgr.lock_or_recover().create_window("WC_BUTTON".into(), client, hmq);
        assert!(matches!(loader.help_hook(TID, button, 3, 0), ApiResult::Normal(0)));
        assert_eq!(queued(&loader, hmq), vec![(client, WM_HELP, 3, 0)]);
    }

    #[test]
    fn wait_msg_returns_once_a_message_in_range_is_there() {
        let (loader, hmq, _, client) = setup();
        loader.win_post_queue_msg(TID, hmq, WM_USER + 3, 0, 0);
        assert!(matches!(loader.win_wait_msg(TID, WM_USER, WM_USER + 5), ApiResult::Normal(1)));
        assert_eq!(queued(&loader, hmq).len(), 1, "left in the queue");
        assert!(matches!(loader.win_wait_msg(7, 0, 0), ApiResult::Normal(0)), "no queue");
        let _ = client;
    }
}
