// SPDX-License-Identifier: GPL-3.0-only

//! Calls on other programs' windows in flight through the desktop server
//! (ADR-0052 phase B). Pure bookkeeping, without sockets, so the rules can
//! be tested on their own.
//!
//! A client's `Call` names a window handle; the handle's desktop slot names
//! the client it belongs to (the *callee*). The router forwards the call
//! under an id of its own and passes the answer back to the caller — only
//! an answer from that callee, only once. A call is refused at once when
//! the handle has no slot, names the caller itself or a client that is not
//! connected, or when the caller already has `MAX_PENDING_CALLS` calls in
//! flight. Calls to a client that leaves are refused; calls left
//! unanswered for `CALL_EXPIRY` are refused too, so a callee that never
//! answers cannot make the server hold them for ever — except sent
//! messages (`WinSendMsg`), which wait as long as the window procedure
//! runs, as in PM; `MAX_PENDING_CALLS` still bounds them.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::model::ClientId;
use super::proto::{WinCall, WinReply};
use crate::loader::pm_types::handle_slot;

/// Most calls one client may have in flight.
pub const MAX_PENDING_CALLS: usize = 32;
/// How long the server keeps a call that was not answered (the caller has
/// given up by then: `client::CALL_TIMEOUT`).
pub const CALL_EXPIRY: Duration = Duration::from_secs(10);

/// A call made at `SERVER_ADDRESS`, answered by the server itself:
/// `Clients` with the connected clients' slots (ascending), `Stacking`
/// with the session's frames top first; any other call fails.
pub fn server_answer(call: &WinCall, clients: impl Iterator<Item = ClientId>, stacking: Vec<u32>) -> WinReply {
    match call {
        WinCall::Clients => {
            let mut slots: Vec<u32> = clients.collect();
            slots.sort_unstable();
            WinReply::Handles(slots)
        }
        WinCall::Stacking => WinReply::Handles(stacking),
        _ => WinReply::Failed,
    }
}

/// A forwarded call waiting for its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pending {
    caller: ClientId,
    /// The caller's id for it.
    id: u32,
    callee: ClientId,
    since: Instant,
    /// Refused after `CALL_EXPIRY` (every call but a sent message).
    expires: bool,
}

/// What to do with a client's call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Send it to `callee` as `Called { id }`.
    Forward { callee: ClientId, id: u64 },
    /// Answer `Failed` at once.
    Refuse,
}

/// A call whose caller is to be told it failed: the caller and its id.
pub type Refusal = (ClientId, u32);

#[derive(Debug, Default)]
pub struct CallRouter {
    pending: HashMap<u64, Pending>,
    next: u64,
}

impl CallRouter {
    pub fn new() -> CallRouter {
        CallRouter::default()
    }

    /// Client `caller` makes call `id` on window `hwnd`; `connected` tells
    /// whether a client is connected in a slot, `expires` whether the call
    /// is given up after `CALL_EXPIRY`.
    pub fn call(&mut self, caller: ClientId, id: u32, hwnd: u32, now: Instant, expires: bool,
                connected: impl Fn(ClientId) -> bool) -> Route {
        let Some(callee) = handle_slot(hwnd) else { return Route::Refuse };
        let in_flight = self.pending.values().filter(|p| p.caller == caller).count();
        if callee == caller || !connected(callee) || in_flight >= MAX_PENDING_CALLS {
            return Route::Refuse;
        }
        self.next += 1;
        self.pending.insert(self.next, Pending { caller, id, callee, since: now, expires });
        Route::Forward { callee, id: self.next }
    }

    /// Client `callee` answers forwarded call `id`: the caller and its id,
    /// or `None` for an id that is not that client's to answer (unknown,
    /// answered already, refused meanwhile, or another client's).
    pub fn answer(&mut self, callee: ClientId, id: u64) -> Option<(ClientId, u32)> {
        let p = self.pending.get(&id).filter(|p| p.callee == callee)?;
        let back = (p.caller, p.id);
        self.pending.remove(&id);
        Some(back)
    }

    /// Client `slot` left: the calls made to it are refused, the calls it
    /// made are forgotten.
    pub fn client_gone(&mut self, slot: ClientId) -> Vec<Refusal> {
        let mut out = Vec::new();
        self.pending.retain(|_, p| {
            if p.caller == slot { return false; }
            if p.callee == slot {
                out.push((p.caller, p.id));
                return false;
            }
            true
        });
        out
    }

    /// Calls unanswered for `CALL_EXPIRY` by `now`, refused (sent messages
    /// wait on).
    pub fn expire(&mut self, now: Instant) -> Vec<Refusal> {
        let mut out = Vec::new();
        self.pending.retain(|_, p| {
            let late = p.expires && now.saturating_duration_since(p.since) >= CALL_EXPIRY;
            if late { out.push((p.caller, p.id)); }
            !late
        });
        out
    }

    /// Calls in flight.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::pm_types::{FIRST_LOCAL_HWND, HWND_SLOT_SHIFT};

    /// A window of the client in slot `slot`.
    fn win(slot: u32) -> u32 {
        (slot + 1) << HWND_SLOT_SHIFT | FIRST_LOCAL_HWND
    }

    fn all(_: ClientId) -> bool { true }

    #[test]
    fn a_call_goes_to_the_windows_client_and_the_answer_back_once() {
        let (mut r, now) = (CallRouter::new(), Instant::now());
        let Route::Forward { callee, id } = r.call(1, 7, win(2), now, true, all) else { panic!() };
        assert_eq!(callee, 2);
        assert_eq!(r.answer(3, id), None, "only the callee answers");
        assert_eq!(r.answer(2, id), Some((1, 7)));
        assert_eq!(r.answer(2, id), None, "once");
        assert!(r.is_empty());
    }

    #[test]
    fn calls_without_a_callee_are_refused() {
        let (mut r, now) = (CallRouter::new(), Instant::now());
        assert_eq!(r.call(1, 1, 0x1000, now, true, all), Route::Refuse, "no slot in the handle");
        assert_eq!(r.call(1, 1, win(1), now, true, all), Route::Refuse, "the caller's own window");
        assert_eq!(r.call(1, 1, win(2), now, true, |c| c != 2), Route::Refuse, "nobody in that slot");
        assert!(r.is_empty());
    }

    #[test]
    fn a_caller_has_a_bounded_number_of_calls_in_flight() {
        let (mut r, now) = (CallRouter::new(), Instant::now());
        for i in 0..MAX_PENDING_CALLS as u32 {
            assert!(matches!(r.call(1, i, win(2), now, true, all), Route::Forward { .. }));
        }
        assert_eq!(r.call(1, 99, win(2), now, true, all), Route::Refuse);
        assert!(matches!(r.call(3, 1, win(2), now, true, all), Route::Forward { .. }), "another caller is not held up");
    }

    #[test]
    fn calls_to_a_client_that_leaves_or_never_answers_are_refused() {
        let (mut r, now) = (CallRouter::new(), Instant::now());
        let Route::Forward { id: to_2, .. } = r.call(1, 1, win(2), now, true, all) else { panic!() };
        r.call(2, 5, win(1), now, true, all);
        r.call(3, 6, win(4), now, true, all);
        let mut gone = r.client_gone(2);
        gone.sort();
        assert_eq!(gone, vec![(1, 1)], "the call client 2 made itself is just forgotten");
        assert_eq!(r.answer(2, to_2), None);
        assert_eq!(r.len(), 1);
        assert!(r.expire(now + CALL_EXPIRY / 2).is_empty());
        assert_eq!(r.expire(now + CALL_EXPIRY), vec![(3, 6)]);
        assert!(r.is_empty());
    }

    #[test]
    fn the_server_answers_for_the_session() {
        assert_eq!(server_answer(&WinCall::Clients, [4, 1, 2].into_iter(), vec![]), WinReply::Handles(vec![1, 2, 4]));
        assert_eq!(server_answer(&WinCall::Stacking, [1].into_iter(), vec![9, 8]), WinReply::Handles(vec![9, 8]));
        assert_eq!(server_answer(&WinCall::Describe, [1].into_iter(), vec![]), WinReply::Failed);
    }

    #[test]
    fn sent_messages_never_expire_but_count_and_end_with_their_callee() {
        let (mut r, now) = (CallRouter::new(), Instant::now());
        let Route::Forward { id, .. } = r.call(1, 1, win(2), now, false, all) else { panic!() };
        assert!(r.expire(now + CALL_EXPIRY * 100).is_empty(), "a window procedure may run for long");
        for i in 1..MAX_PENDING_CALLS as u32 {
            assert!(matches!(r.call(1, 1 + i, win(2), now, false, all), Route::Forward { .. }));
        }
        assert_eq!(r.call(1, 99, win(2), now, false, all), Route::Refuse, "still bounded");
        assert_eq!(r.answer(2, id), Some((1, 1)));
        assert_eq!(r.client_gone(2).len(), MAX_PENDING_CALLS - 1);
        assert!(r.is_empty());
    }
}
