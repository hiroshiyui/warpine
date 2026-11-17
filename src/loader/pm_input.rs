// SPDX-License-Identifier: GPL-3.0-only

//! The system input queue: keyboard and pointer input in arrival order.
//!
//! PM routes an input event only when a thread reads it, not when it
//! arrives, so a key typed right after a key that moves the focus goes to
//! the new focus window. The GUI thread therefore does not post keystrokes to
//! a window's queue: it appends them here, and `WinGetMsg` / `WinPeekMsg` /
//! the modal dialog loop take the oldest event when its target window
//! belongs to the reading thread's queue. As in PM:
//!
//! - posted messages come before input (`next_message` looks here only
//!   when the thread's own queue has nothing that passes its filters);
//! - input is taken in order: an event for another thread's window blocks
//!   the events behind it until that thread has read it;
//! - a key goes to the focus window at the time it is read (`key_target`),
//!   and a typed character is encoded in that window's queue codepage then.
//!
//! Pointer messages carry the window they were hit-tested to; they go
//! through this queue so that keys typed before a click are handled before
//! it. A click changes the focus on the GUI thread (activation, a click
//! outside the focused control), so it first fixes the target of the keys
//! still waiting ([`WindowManager::pin_pending_input`]): they were typed for
//! the old focus.

use std::collections::VecDeque;

use super::constants::{KC_CHAR, WM_CHAR, WM_CHAR_REPEAT_SHIFT};
use super::mutex_ext::MutexExt;
use super::pm_types::{OS2Message, WindowManager};

/// At most this many events wait in the queue; later ones are dropped (a
/// thread that stopped reading its queue cannot make it grow without bound).
pub const MAX_INPUT_EVENTS: usize = 1024;

/// One event of the system input queue.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// A key press or release: the `WM_CHAR` parameters, for the focus
    /// window at the time the event is read.
    Key { mp1: u32, mp2: u32 },
    /// A typed character (SDL text input), encoded when the event is read.
    Char(char),
    /// A message for a window fixed when the event arrived (pointer input,
    /// or a key whose target a click fixed).
    Window { hwnd: u32, msg: u32, mp1: u32, mp2: u32 },
}

/// The `WM_CHAR` parameters `(mp1, mp2)` of typed character `ch` in
/// codepage `cp`: `KC_CHAR` with a repeat count of one, and the character in
/// mp2 — a single byte, or for a double-byte character the lead byte in the
/// low byte and the trail byte in the high byte. `None` for a control
/// character or one the codepage cannot represent.
pub fn char_message(ch: char, cp: u32) -> Option<(u32, u32)> {
    if ch.is_control() {
        return None;
    }
    let mut utf8 = [0u8; 4];
    let s = ch.encode_utf8(&mut utf8);
    let chr = match super::codepage::cp_encode(s, cp)[..] {
        // `cp_encode` substitutes '?' for unrepresentable characters.
        [b] if b != b'?' || ch == '?' => b as u32,
        [lead, trail] => lead as u32 | ((trail as u32) << 8),
        _ => return None,
    };
    Some((KC_CHAR | (1 << WM_CHAR_REPEAT_SHIFT), chr))
}

/// What reading the head of the input queue gives.
enum Head {
    /// The event goes to `hmq` as this message.
    Deliver { hmq: u32, msg: OS2Message },
    /// The event cannot be delivered (no window or queue, a character the
    /// codepage lacks): drop it.
    Drop,
}

impl WindowManager {
    /// Append `ev` to the system input queue and wake the thread that is to
    /// read the oldest event.
    pub fn post_input(&mut self, ev: InputEvent) {
        if self.input.len() >= MAX_INPUT_EVENTS {
            log::warn!("system input queue full: event dropped");
            return;
        }
        self.input.push_back(ev);
        self.wake_input_reader();
    }

    /// Fix the target of every key still waiting to the window that would
    /// get it now: called before the GUI thread itself moves the focus (a
    /// click), since those keys were typed before that.
    pub fn pin_pending_input(&mut self) {
        let pending = std::mem::take(&mut self.input);
        for ev in pending {
            let ev = match ev {
                InputEvent::Window { .. } => ev,
                other => match self.resolve_input(&other) {
                    Head::Deliver { msg, .. } =>
                        InputEvent::Window { hwnd: msg.hwnd, msg: msg.msg, mp1: msg.mp1, mp2: msg.mp2 },
                    Head::Drop => continue,
                },
            };
            self.input.push_back(ev);
        }
    }

    /// The oldest input event as a message for queue `hmq`, if it is meant
    /// for that queue and passes `pass`; removed from the input queue when
    /// `remove` is set. Undeliverable events at the head are dropped on the
    /// way; an event for another queue stays and wakes that queue's thread.
    pub fn next_input(&mut self, hmq: u32, remove: bool, pass: impl Fn(&OS2Message) -> bool)
        -> Option<OS2Message>
    {
        loop {
            let head = self.resolve_input(self.input.front()?);
            match head {
                Head::Drop => { self.input.pop_front(); }
                Head::Deliver { hmq: target, msg } => {
                    if target != hmq {
                        self.wake_queue(target);
                        return None;
                    }
                    if !pass(&msg) {
                        return None;
                    }
                    if remove {
                        self.input.pop_front();
                        self.wake_input_reader();
                    }
                    return Some(msg);
                }
            }
        }
    }

    /// Number of events in the system input queue.
    pub fn pending_input(&self) -> usize {
        self.input.len()
    }

    /// Where event `ev` goes if it is read now. A frame stands for its
    /// client, as for every message the GUI thread sends.
    fn resolve_input(&self, ev: &InputEvent) -> Head {
        let (hwnd, msg, mp1, mp2) = match *ev {
            InputEvent::Key { mp1, mp2 } => (self.key_target(), WM_CHAR, mp1, mp2),
            InputEvent::Char(_) => (self.key_target(), WM_CHAR, 0, 0),
            InputEvent::Window { hwnd, msg, mp1, mp2 } => (hwnd, msg, mp1, mp2),
        };
        let target = self.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
        // The window may have been destroyed since the event arrived.
        if self.get_window(target).is_none() {
            return Head::Drop;
        }
        let Some(hmq) = self.find_hmq_for_hwnd(target).or_else(|| self.find_hmq_for_hwnd(hwnd))
            .filter(|h| self.msg_queues.contains_key(h))
        else {
            return Head::Drop;
        };
        let (mp1, mp2) = match *ev {
            InputEvent::Char(ch) => {
                let cp = self.get_mq(hmq).map_or(0, |mq| mq.lock_or_recover().codepage);
                match char_message(ch, cp) {
                    Some(m) => m,
                    None => return Head::Drop,
                }
            }
            _ => (mp1, mp2),
        };
        Head::Deliver { hmq, msg: OS2Message { hwnd: target, msg, mp1, mp2, time: 0, x: 0, y: 0 } }
    }

    /// Wake the thread that is to read the oldest input event.
    fn wake_input_reader(&self) {
        if let Some(Head::Deliver { hmq, .. }) = self.input.front().map(|ev| self.resolve_input(ev)) {
            self.wake_queue(hmq);
        }
    }

    fn wake_queue(&self, hmq: u32) {
        if let Some(mq) = self.get_mq(hmq) {
            mq.lock_or_recover().cond.notify_all();
        }
    }
}

/// The system input queue itself (a field of `WindowManager`).
pub type InputQueue = VecDeque<InputEvent>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::constants::WM_BUTTON1DOWN;

    /// Two frames with clients on two queues, and an entry field in the
    /// first client. Returns `(wm, [hmq1, hmq2], [f1, c1, f2, c2], entry)`.
    fn two_frames() -> (WindowManager, [u32; 2], [u32; 4], u32) {
        let mut wm = WindowManager::new();
        let q1 = wm.create_mq(437);
        let q2 = wm.create_mq(850);
        let f1 = wm.create_window("App".into(), 0, q1);
        let c1 = wm.create_window("App".into(), f1, q1);
        wm.frame_to_client.insert(f1, c1);
        let f2 = wm.create_window("App".into(), 0, q2);
        let c2 = wm.create_window("App".into(), f2, q2);
        wm.frame_to_client.insert(f2, c2);
        let entry = wm.create_window("WC_ENTRYFIELD".into(), c1, q1);
        wm.get_window_mut(entry).unwrap().visible = true;
        wm.focused_hwnd = f1;
        (wm, [q1, q2], [f1, c1, f2, c2], entry)
    }

    fn take(wm: &mut WindowManager, hmq: u32) -> Option<(u32, u32, u32, u32)> {
        wm.next_input(hmq, true, |_| true).map(|m| (m.hwnd, m.msg, m.mp1, m.mp2))
    }

    #[test]
    fn a_key_goes_to_the_focus_window_when_it_is_read() {
        let (mut wm, [q1, _], [_, c1, ..], entry) = two_frames();
        wm.post_input(InputEvent::Key { mp1: 1, mp2: 2 });
        wm.post_input(InputEvent::Key { mp1: 3, mp2: 4 });
        // The first key goes to the client (no focus window yet); handling
        // it gives the entry field the focus, so the second goes there.
        assert_eq!(take(&mut wm, q1), Some((c1, WM_CHAR, 1, 2)));
        wm.focus_hwnd = entry;
        assert_eq!(take(&mut wm, q1), Some((entry, WM_CHAR, 3, 4)));
        assert_eq!(take(&mut wm, q1), None);
    }

    #[test]
    fn input_for_another_queue_waits_for_that_queue_and_blocks_later_input() {
        let (mut wm, [q1, q2], [_, c1, f2, c2], _) = two_frames();
        wm.focused_hwnd = f2;
        wm.post_input(InputEvent::Key { mp1: 1, mp2: 0 });
        wm.post_input(InputEvent::Window { hwnd: c1, msg: WM_BUTTON1DOWN, mp1: 0, mp2: 0 });
        assert_eq!(take(&mut wm, q1), None, "the oldest event belongs to the other queue");
        assert_eq!(take(&mut wm, q2), Some((c2, WM_CHAR, 1, 0)));
        assert_eq!(take(&mut wm, q1), Some((c1, WM_BUTTON1DOWN, 0, 0)));
        assert_eq!(wm.pending_input(), 0);
    }

    #[test]
    fn peeking_leaves_the_event_and_a_failing_filter_takes_nothing() {
        let (mut wm, [q1, _], [_, c1, ..], _) = two_frames();
        wm.post_input(InputEvent::Key { mp1: 7, mp2: 0 });
        assert_eq!(wm.next_input(q1, false, |_| true).map(|m| m.hwnd), Some(c1));
        assert_eq!(wm.next_input(q1, true, |m| m.msg != WM_CHAR), None);
        assert_eq!(wm.pending_input(), 1);
        assert_eq!(take(&mut wm, q1), Some((c1, WM_CHAR, 7, 0)));
    }

    #[test]
    fn a_character_is_encoded_in_the_reading_queue_codepage() {
        let (mut wm, [q1, q2], [_, c1, f2, c2], _) = two_frames();
        // é: 0x82 in CP437 and in CP850.
        wm.post_input(InputEvent::Char('é'));
        assert_eq!(take(&mut wm, q1), Some((c1, WM_CHAR, KC_CHAR | (1 << WM_CHAR_REPEAT_SHIFT), 0x82)));
        // Ø exists in CP850 (0x9D) only: dropped for a CP437 queue.
        wm.post_input(InputEvent::Char('Ø'));
        assert_eq!(take(&mut wm, q1), None);
        assert_eq!(wm.pending_input(), 0);
        wm.focused_hwnd = f2;
        wm.post_input(InputEvent::Char('Ø'));
        assert_eq!(take(&mut wm, q2).map(|m| (m.0, m.3)), Some((c2, 0x9D)));
    }

    #[test]
    fn pinning_keeps_waiting_keys_for_the_window_they_were_typed_for() {
        let (mut wm, [q1, q2], [_, _, f2, c2], entry) = two_frames();
        wm.focus_hwnd = entry;
        wm.post_input(InputEvent::Char('a'));
        wm.post_input(InputEvent::Key { mp1: 5, mp2: 0 });
        // A click activates the other frame: the waiting keys were typed
        // for the entry field and still go there.
        wm.pin_pending_input();
        wm.focused_hwnd = f2;
        wm.post_input(InputEvent::Key { mp1: 6, mp2: 0 });
        assert_eq!(take(&mut wm, q1).map(|m| (m.0, m.3)), Some((entry, b'a' as u32)));
        assert_eq!(take(&mut wm, q1), Some((entry, WM_CHAR, 5, 0)));
        assert_eq!(take(&mut wm, q2), Some((c2, WM_CHAR, 6, 0)));
    }

    #[test]
    fn input_for_a_destroyed_window_or_without_a_target_is_dropped() {
        let (mut wm, [q1, _], [_, c1, ..], entry) = two_frames();
        wm.post_input(InputEvent::Window { hwnd: entry, msg: WM_BUTTON1DOWN, mp1: 0, mp2: 0 });
        wm.destroy_window(entry);
        wm.focused_hwnd = 0;
        wm.post_input(InputEvent::Key { mp1: 1, mp2: 0 });
        wm.post_input(InputEvent::Window { hwnd: c1, msg: WM_BUTTON1DOWN, mp1: 0, mp2: 0 });
        assert_eq!(take(&mut wm, q1), Some((c1, WM_BUTTON1DOWN, 0, 0)));
        assert_eq!(wm.pending_input(), 0);
    }

    #[test]
    fn the_input_queue_is_bounded() {
        let (mut wm, ..) = two_frames();
        for _ in 0..MAX_INPUT_EVENTS + 10 {
            wm.post_input(InputEvent::Key { mp1: 0, mp2: 0 });
        }
        assert_eq!(wm.pending_input(), MAX_INPUT_EVENTS);
    }

    #[test]
    fn char_message_encodes_one_character() {
        assert_eq!(char_message('é', 850), Some((KC_CHAR | (1 << WM_CHAR_REPEAT_SHIFT), 0x82)));
        // 日 in CP932: lead byte low, trail byte high.
        assert_eq!(char_message('日', 932).map(|m| m.1), Some(0xFA93));
        // Unrepresentable characters are dropped, not sent as '?'; a real
        // '?' is kept. Control characters never come through as text.
        assert_eq!(char_message('日', 437), None);
        assert_eq!(char_message('?', 437).map(|m| m.1), Some(b'?' as u32));
        assert_eq!(char_message('\r', 437), None);
    }
}
