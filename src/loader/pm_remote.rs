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
//!
//! Other calls on another program's window fail with `PMERR_INVALID_HWND`
//! as before; `WinSendMsg` across programs needs the window's thread to run
//! its window procedure and comes later.
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

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_error::window_word_in_range;
use super::pm_types::handle_in_slot;
use super::pm_win::bound_window_geometry;
use super::{ApiResult, Loader};
use crate::desktop::client::Responder;
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

/// The PMWIN calls made on another program's window, with the stack offset
/// of their window handle.
const REMOTE_CALLS: [(u32, u64); 16] = [
    (ORD_POST_MSG, 4), (ORD_IS_WINDOW, 8), (ORD_QUERY_WINDOW_PROCESS, 4), (ORD_QUERY_WINDOW_POS, 4),
    (ORD_QUERY_WINDOW_RECT, 4), (ORD_QUERY_WINDOW_TEXT, 4), (ORD_QUERY_WINDOW_TEXT_LENGTH, 4),
    (ORD_IS_WINDOW_VISIBLE, 4), (ORD_IS_WINDOW_ENABLED, 4), (ORD_QUERY_WINDOW_ULONG, 4),
    (ORD_QUERY_WINDOW_USHORT, 4), (ORD_QUERY_WINDOW, 4), (ORD_QUERY_CLASS_NAME, 4), (ORD_SET_WINDOW_POS, 4),
    (ORD_SHOW_WINDOW, 4), (ORD_SET_ACTIVE_WINDOW, 8),
];

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
    /// windows of this program's slot that exist.
    fn answer(&self, hwnd: u32, call: &WinCall) -> WinReply {
        let mine = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.handle_slot.is_some_and(|s| handle_in_slot(hwnd, s)) && wm.is_window(hwnd)
        };
        if !mine {
            return WinReply::Failed;
        }
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
        }
    }
}
