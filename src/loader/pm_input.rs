// SPDX-License-Identifier: GPL-3.0-or-later

//! The system input queue: keyboard and pointer input in arrival order.
//!
//! PM routes an input event only when a thread reads it, not when it
//! arrives, so a key typed right after a key that moves the focus goes to
//! the new focus window. The GUI thread therefore does not post keystrokes to
//! a window's queue: it appends them here, and `WinGetMsg` / `WinPeekMsg` /
//! the modal dialog loop take the oldest event when its target window
//! belongs to the reading thread's queue. As in PM:
//!
//! - a thread reads its posted messages, then input, then `WM_PAINT`, then
//!   `WM_TIMER` ([`WindowManager::next_queued`], [`Rank`]);
//! - input is taken in order: an event for another thread's window blocks
//!   the events behind it until that thread has read it;
//! - a key goes to the focus window at the time it is read (`key_target`),
//!   and a typed character is encoded in that window's queue codepage then.
//!
//! Pointer input is routed when it is read too (ADR-0051). The PM Guide
//! ("Capturing Mouse Input") posts a mouse message "to the window that is
//! under the mouse pointer at the time the system reads the mouse input
//! events from the system message queue", and its events are "strictly
//! ordered so that a new event cannot be processed until all previous events
//! are fully processed": a key typed after a click goes to another window
//! "only if that window became active as a result of the mouse click". So
//! the GUI thread queues a pointer event in desktop coordinates
//! ([`InputEvent::Pointer`]), and when the event reaches the head of the
//! queue and its thread looks for input ([`WindowManager::take_up_input`]):
//!
//! - it is hit-tested: the capture window, a built-in control held with
//!   button 1, else the window under the pointer (a disabled frame takes
//!   none; a button-1 message goes to the built-in control under it);
//! - a button press activates its frame (`WM_ACTIVATE`, posted, so read
//!   before the click) and a button-1 press beside the focused control takes
//!   the focus from it;
//! - it becomes an [`InputEvent::Window`] for that window.
//!
//! The window under the pointer is the deepest visible window there (any
//! class; static controls let the pointer through; a disabled window takes
//! nothing). A window whose class has `CS_HITTEST` is asked `WM_HITTEST`
//! first, when the event is read (the PM Reference: "when an application
//! requests a message through WinPeekMsg or WinGetMsg"): `HT_NORMAL` takes
//! the message, `HT_TRANSPARENT` passes it to the windows beneath,
//! `HT_DISCARD` and `HT_ERROR` drop it (`HT_ERROR` still activates the frame
//! of a press). The reading thread makes that call ([`HitAsk`]) and reads
//! again (ADR-0058).
//!
//! Mouse moves are coalesced: a move queued right behind another move that
//! is not yet routed replaces it.
//!
//! Clicks on frame chrome (title bar, buttons, sizing border) stay with the
//! GUI thread, which activates the frame at once; it first fixes the target
//! of the keys still waiting ([`WindowManager::pin_pending_input`]), since
//! they were typed for the old active window.

use std::collections::VecDeque;

use super::constants::{
    KC_CHAR, WS_DISABLED, WM_ACTIVATE, WM_BUTTON1DBLCLK, WM_BUTTON1DOWN, WM_BUTTON1UP, WM_BUTTON2DBLCLK,
    WM_BUTTON2DOWN, WM_BUTTON3DBLCLK, WM_BUTTON3DOWN, WM_CHAR, WM_CHAR_REPEAT_SHIFT, WM_MOUSEMOVE,
    WM_PAINT, WM_TIMER, KC_VIRTUALKEY, KC_KEYUP, KC_SHIFT, KC_CTRL, KC_ALT, WM_CHAR_VKEY_SHIFT, VK_F10,
    WM_VSCROLL,
};
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
    /// A message for a window already fixed (a pointer event taken up, a
    /// key whose target a click on frame chrome fixed, a drag message).
    Window { hwnd: u32, msg: u32, mp1: u32, mp2: u32 },
    /// A pointer message (`WM_MOUSEMOVE`, `WM_BUTTONn*`) at desktop point
    /// `(x, y)` (bottom-left origin), hit-tested when it is read; `mp2` is
    /// its second parameter (the `KC_*` flags in the high word).
    Pointer { msg: u32, x: i32, y: i32, mp2: u32 },
}

/// `WM_CONTEXTMENU` (Warp 4): mp1 = the pointer position in the window, mp2
/// = TRUE when the pointer asked for it (button 2 clicked), FALSE for the
/// keyboard (Shift+F10).
pub const WM_CONTEXTMENU: u32 = 0x0424;
/// Lines one wheel notch scrolls (`WM_VSCROLL` `SB_LINEUP` /
/// `SB_LINEDOWN` to the window under the pointer, as OS/2 wheel drivers
/// send; Warp 4 has no wheel message).
pub const WHEEL_LINES: i32 = 3;

/// Whether `WM_CHAR` parameters `(mp1, mp2)` are Shift+F10 pressed.
pub fn is_context_key(mp1: u32, mp2: u32) -> bool {
    let fs = mp1 & 0xFFFF;
    fs & KC_VIRTUALKEY != 0 && fs & KC_KEYUP == 0 && fs & KC_SHIFT != 0 && fs & (KC_CTRL | KC_ALT) == 0
        && mp2 >> WM_CHAR_VKEY_SHIFT == VK_F10
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
    /// A pointer event whose thread is that of queue `hmq`: it must be taken
    /// up there ([`WindowManager::take_up_input`]) before it is delivered.
    TakeUp { hmq: u32 },
    /// The event cannot be delivered (no window or queue, a character the
    /// codepage lacks): drop it.
    Drop,
}

/// Whether `msg` presses a mouse button (`WM_BUTTONnDOWN` / `WM_BUTTONnDBLCLK`).
fn is_press(msg: u32) -> bool {
    matches!(msg, WM_BUTTON1DOWN | WM_BUTTON1DBLCLK | WM_BUTTON2DOWN | WM_BUTTON2DBLCLK
        | WM_BUTTON3DOWN | WM_BUTTON3DBLCLK)
}

/// Pack a point as a pointer message's mp1, `MPFROM2SHORT(x, y)`.
fn pack_point(x: i32, y: i32) -> u32 {
    (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16)
}

/// `CS_HITTEST` (`<pmwin.h>`): windows of the class are asked `WM_HITTEST`.
pub const CS_HITTEST: u32 = 0x0000_0008;
pub const WM_HITTEST: u32 = 0x000C;
pub const HT_NORMAL: i32 = 0;
pub const HT_TRANSPARENT: i32 = -1;
pub const HT_DISCARD: i32 = -2;
pub const HT_ERROR: i32 = -3;
/// Most windows one pointer event may pass through (`HT_TRANSPARENT`).
const MAX_HIT_SKIPS: usize = 64;
/// How deep the search for the window under the pointer goes.
const MAX_HIT_DEPTH: usize = 64;

/// `QS_*` flags of `WinQueryQueueStatus` (`<pmwin.h>`).
pub const QS_KEY: u32 = 0x0001;
pub const QS_MOUSEBUTTON: u32 = 0x0002;
pub const QS_MOUSEMOVE: u32 = 0x0004;
pub const QS_TIMER: u32 = 0x0008;
pub const QS_PAINT: u32 = 0x0010;
pub const QS_POSTMSG: u32 = 0x0020;
pub const QS_SEM1: u32 = 0x0040;
pub const QS_SEM2: u32 = 0x0080;
pub const QS_SEM3: u32 = 0x0100;
pub const QS_SEM4: u32 = 0x0200;
pub const QS_SENDMSG: u32 = 0x0400;
const WM_SEM1: u32 = 0x0025;
const WM_SEM2: u32 = 0x0026;
const WM_SEM3: u32 = 0x0027;
const WM_SEM4: u32 = 0x0028;

/// One thing in a queue as `WinQueryQueueStatus` tells them apart: a
/// queued message, an input event (its serial number) or a message another
/// program sent, with its `QS_*` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusKey {
    Message { flag: u32, hwnd: u32, msg: u32, mp1: u32, mp2: u32 },
    Input { flag: u32, seq: u64 },
    Sent { hwnd: u32, id: Option<u64> },
}

impl StatusKey {
    fn flag(&self) -> u32 {
        match *self {
            StatusKey::Message { flag, .. } | StatusKey::Input { flag, .. } => flag,
            StatusKey::Sent { .. } => QS_SENDMSG,
        }
    }
}

/// The `QS_*` flag of message `msg` in a queue.
fn status_flag(msg: u32) -> u32 {
    match msg {
        WM_CHAR => QS_KEY,
        WM_MOUSEMOVE => QS_MOUSEMOVE,
        m if (WM_BUTTON1DOWN..=WM_BUTTON3DBLCLK).contains(&m) => QS_MOUSEBUTTON,
        WM_PAINT => QS_PAINT,
        WM_TIMER => QS_TIMER,
        WM_SEM1 => QS_SEM1,
        WM_SEM2 => QS_SEM2,
        WM_SEM3 => QS_SEM3,
        WM_SEM4 => QS_SEM4,
        _ => QS_POSTMSG,
    }
}

/// A pointer event at the head of the input queue whose window is to be
/// asked `WM_HITTEST` by the thread of queue `hmq` before it is read:
/// `WM_HITTEST(mp1 = the point in `hwnd`'s coordinates, mp2 = 0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitAsk {
    pub hmq: u32,
    pub hwnd: u32,
    pub mp1: u32,
}

/// Where a pointer event goes: its window and the point in that window's
/// coordinates, and the frame it lies in (activated by a press).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerTarget {
    pub hwnd: u32,
    pub x: i32,
    pub y: i32,
    pub frame: u32,
    /// The window was chosen by the capture or a held control, not by the
    /// pointer's position: a press does not activate or move the focus.
    pub held: bool,
}

/// Where a queued message ranks when a thread reads its queue. The PM
/// Reference ranks `WM_TIMER` "lower than all other messages except
/// WM_SEM4" and the `WM_PAINT` / `WM_TIMER` messages the system generates
/// below `WM_SEM2`, itself "lower than any other message": so posted
/// messages and input come first, then `WM_PAINT`, then `WM_TIMER`.
/// (Warpine has no `WM_SEMn` messages.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Posted,
    Paint,
    Timer,
}

impl Rank {
    pub fn of(msg: u32) -> Rank {
        match msg {
            WM_PAINT => Rank::Paint,
            WM_TIMER => Rank::Timer,
            _ => Rank::Posted,
        }
    }
}

impl WindowManager {
    /// `WinGetMsg` / `WinPeekMsg` / the modal loop: the next message for
    /// queue `hmq` that passes `pass`, in PM's order — its posted messages
    /// in the order they were posted, then input from the system input
    /// queue, then `WM_PAINT`, then `WM_TIMER` (see [`Rank`]). Removed from
    /// its queue when `remove` is set.
    pub fn next_queued(&mut self, hmq: u32, remove: bool, pass: impl Fn(&WindowManager, &OS2Message) -> bool)
        -> Option<OS2Message>
    {
        let mq_arc = self.get_mq(hmq)?;
        // A click at the head of the input queue takes effect first: the
        // activation messages it posts are read before it.
        self.take_up_input(hmq);
        let asking = self.hit_ask.is_some_and(|a| a.hmq == hmq);
        // The queue's lock is held only while it is searched: reading input
        // may lock other queues.
        let take = |wm: &WindowManager, rank: Rank| {
            let mut mq = mq_arc.lock_or_recover();
            let i = mq.messages.iter().position(|m| Rank::of(m.msg) == rank && pass(wm, m))?;
            if remove { mq.messages.remove(i) } else { mq.messages.get(i).cloned() }
        };
        let found = take(self, Rank::Posted).or_else(|| {
            // Input waits for its WM_HITTEST answer; so do paint and timers.
            if asking { return None; }
            match self.next_input(hmq, false, |_| true) {
                Some(probe) if pass(self, &probe) =>
                    if remove { self.next_input(hmq, true, |_| true) } else { Some(probe) },
                _ => take(self, Rank::Paint).or_else(|| take(self, Rank::Timer)),
            }
        });
        found.map(|m| self.note_read(&mq_arc, m))
    }

    /// A message the thread of queue `mq` reads: stamped with the time and
    /// pointer position now if it has none yet (a message that is not
    /// input), and remembered as the queue's last message
    /// (`WinQueryMsgTime` / `WinQueryMsgPos`).
    fn note_read(&self, mq: &std::sync::Arc<std::sync::Mutex<super::pm_types::PM_MsgQueue>>,
                 mut m: OS2Message) -> OS2Message {
        if m.time == 0 {
            m.time = super::stubs::ms_count();
            (m.x, m.y) = self.key_state.pointer;
        }
        let mut q = mq.lock_or_recover();
        q.last_msg_time = m.time;
        q.last_msg_pos = (m.x, m.y);
        m
    }

    /// Append `ev` to the system input queue and wake the thread that is to
    /// read the oldest event.
    pub fn post_input(&mut self, ev: InputEvent) {
        if self.input.len() >= MAX_INPUT_EVENTS {
            log::warn!("system input queue full: event dropped");
            return;
        }
        let pos = self.key_state.pointer;
        // Input from the desktop server (ADR-0052) carries its token: the
        // server waits for it to be read before routing the next event.
        let token = self.input_tag;
        if token.is_some() { self.input_tag_used = true; }
        let time = super::stubs::ms_count();
        self.input_seq += 1;
        let seq = self.input_seq;
        // A move right behind a move not yet routed replaces it (not the
        // head while its window is being asked WM_HITTEST).
        if let InputEvent::Pointer { msg: WM_MOUSEMOVE, mp2, .. } = ev
            && (self.input.len() > 1 || self.hit_ask.is_none())
            && let Some(last) = self.input.back_mut()
            && matches!(last.ev, InputEvent::Pointer { msg: WM_MOUSEMOVE, mp2: m, .. } if m == mp2)
        {
            let old = std::mem::replace(last, QueuedInput { ev, time, pos, token, seq, ..QueuedInput::default() });
            if let Some(t) = old.token && Some(t) != token && !self.input.iter().any(|q| q.token == Some(t)) {
                self.ack_input(t);
            }
            self.wake_input_reader();
            return;
        }
        self.input.push_back(QueuedInput { ev, time, pos, token, seq, ..QueuedInput::default() });
        self.wake_input_reader();
    }

    /// Remove the oldest input event. When it is the last one of a desktop
    /// server event still waiting, the server hears that it was taken, with
    /// this process's active frame now (ADR-0052).
    fn pop_input(&mut self) {
        let Some(gone) = self.input.pop_front() else { return };
        self.hit_ask = None;
        if let Some(token) = gone.token
            && !self.input.iter().any(|q| q.token == Some(token)) {
            self.ack_input(token);
        }
    }

    /// A drag starts (`DrgDrag`): its loop takes the pointer input from
    /// here on, so pointer messages still waiting are not delivered (the
    /// drag channel has them already). Keys stay. Desktop server events
    /// whose input all went are acknowledged.
    pub fn drop_pointer_input(&mut self) {
        let is_pointer = |q: &QueuedInput| match q.ev {
            InputEvent::Pointer { .. } => true,
            InputEvent::Window { msg, .. } => msg == WM_MOUSEMOVE || (WM_BUTTON1DOWN..=WM_BUTTON3DBLCLK).contains(&msg),
            _ => false,
        };
        let gone: Vec<u64> = self.input.iter().filter(|q| is_pointer(q)).filter_map(|q| q.token).collect();
        self.input.retain(|q| !is_pointer(q));
        let mut acked = Vec::new();
        for token in gone {
            if !acked.contains(&token) && !self.input.iter().any(|q| q.token == Some(token)) {
                acked.push(token);
                self.ack_input(token);
            }
        }
    }

    /// Tell the desktop server that its input event `token` was taken.
    pub fn ack_input(&mut self, token: u64) {
        if self.input_ack.is_none() { return; }
        let active = self.activation_report();
        if let Some(ack) = &self.input_ack {
            ack(token, active);
        }
    }

    /// Fix the target of every key still waiting to the window that would
    /// get it now: called before the GUI thread itself moves the focus (a
    /// click), since those keys were typed before that.
    pub fn pin_pending_input(&mut self) {
        let pending = std::mem::take(&mut self.input);
        for queued in pending {
            let ev = match queued.ev {
                // Pointer events are hit-tested when read anyway.
                InputEvent::Window { .. } | InputEvent::Pointer { .. } => queued.ev.clone(),
                _ => match self.resolve_input(&queued) {
                    Head::Deliver { msg, .. } =>
                        InputEvent::Window { hwnd: msg.hwnd, msg: msg.msg, mp1: msg.mp1, mp2: msg.mp2 },
                    Head::Drop => continue,
                    Head::TakeUp { .. } => unreachable!("only pointer events are taken up"),
                },
            };
            self.input.push_back(QueuedInput { ev, ..queued });
        }
    }

    /// The oldest input event as a message for queue `hmq`, if it is meant
    /// for that queue and passes `pass`; removed from the input queue when
    /// `remove` is set. Undeliverable events at the head are dropped on the
    /// way; an event for another queue stays and wakes that queue's thread.
    pub fn next_input(&mut self, hmq: u32, remove: bool, pass: impl Fn(&OS2Message) -> bool)
        -> Option<OS2Message>
    {
        self.take_up_input(hmq);
        loop {
            let head = self.resolve_input(self.input.front()?);
            match head {
                Head::Drop => self.pop_input(),
                Head::TakeUp { hmq: target } => {
                    if target != hmq {
                        self.wake_queue(target);
                        return None;
                    }
                    self.take_up_input(hmq);
                    // Still a pointer event: its window is to be asked
                    // WM_HITTEST first.
                    if matches!(self.input.front().map(|q| &q.ev), Some(InputEvent::Pointer { .. })) {
                        return None;
                    }
                }
                Head::Deliver { hmq: target, msg } => {
                    if target != hmq {
                        self.wake_queue(target);
                        return None;
                    }
                    if !pass(&msg) {
                        return None;
                    }
                    if remove {
                        self.pop_input();
                        // WinGetKeyState answers as of the input read last.
                        self.key_state.read_input(&msg);
                        self.wake_input_reader();
                    }
                    return Some(msg);
                }
            }
        }
    }

    /// `WinQueryQueueStatus`: the `QS_*` kinds of what waits for the thread
    /// of queue `hmq` — its queued messages, the input it is to read and
    /// messages other programs sent it — in the high word; in the low word
    /// those of them that came since its last call (the PM Reference: "the
    /// types of messages added since the last call"). 0 without the queue.
    pub fn queue_status(&mut self, hmq: u32) -> u32 {
        let Some(mq) = self.get_mq(hmq) else { return 0 };
        let mut now: Vec<StatusKey> = Vec::new();
        for q in &self.input {
            let flag = match &q.ev {
                InputEvent::Key { .. } | InputEvent::Char(_) => QS_KEY,
                InputEvent::Window { msg, .. } | InputEvent::Pointer { msg, .. } => status_flag(*msg),
            };
            let reader = match self.resolve_input(q) {
                Head::Deliver { hmq, .. } | Head::TakeUp { hmq } => Some(hmq),
                Head::Drop => None,
            };
            if reader == Some(hmq) { now.push(StatusKey::Input { flag, seq: q.seq }); }
        }
        let mut q = mq.lock_or_recover();
        now.extend(q.messages.iter().map(|m| StatusKey::Message {
            flag: status_flag(m.msg), hwnd: m.hwnd, msg: m.msg, mp1: m.mp1, mp2: m.mp2,
        }));
        now.extend(q.sent.iter().map(|s| StatusKey::Sent { hwnd: s.hwnd, id: s.id }));
        let current = now.iter().fold(0, |acc, k| acc | k.flag());
        let added = now.iter().filter(|k| !q.status_seen.contains(k)).fold(0, |acc, k| acc | k.flag());
        q.status_seen = now.into_iter().collect();
        (current << 16) | added
    }

    /// Number of events in the system input queue.
    pub fn pending_input(&self) -> usize {
        self.input.len()
    }

    /// Where event `ev` goes if it is read now. A frame stands for its
    /// client, as for every message the GUI thread sends.
    fn resolve_input(&self, queued: &QueuedInput) -> Head {
        let ev = &queued.ev;
        if let InputEvent::Pointer { msg, x, y, .. } = *ev {
            return match self.pointer_target_skipping(msg, x, y, &queued.skip).and_then(|t| self.input_queue_of(t.hwnd)) {
                Some(hmq) => Head::TakeUp { hmq },
                None => Head::Drop,
            };
        }
        let (hwnd, msg, mp1, mp2) = match *ev {
            // Shift+F10, the context-menu key, is WM_CONTEXTMENU (mp2 FALSE:
            // not from the pointer) for the focus window.
            InputEvent::Key { mp1, mp2 } if is_context_key(mp1, mp2) => (self.key_target(), WM_CONTEXTMENU, 0, 0),
            InputEvent::Key { mp1, mp2 } => (self.key_target(), WM_CHAR, mp1, mp2),
            InputEvent::Char(_) => (self.key_target(), WM_CHAR, 0, 0),
            InputEvent::Window { hwnd, msg, mp1, mp2 } => (hwnd, msg, mp1, mp2),
            InputEvent::Pointer { .. } => unreachable!("resolved above"),
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
        let (x, y) = queued.pos;
        Head::Deliver { hmq, msg: OS2Message { hwnd: target, msg, mp1, mp2, time: queued.time, x, y } }
    }

    /// Wake the thread that is to read the oldest input event.
    fn wake_input_reader(&self) {
        if let Some(Head::Deliver { hmq, .. } | Head::TakeUp { hmq }) =
            self.input.front().map(|ev| self.resolve_input(ev))
        {
            self.wake_queue(hmq);
        }
    }

    /// The queue that reads input for window `hwnd` (a frame stands for its
    /// client), if it has one.
    fn input_queue_of(&self, hwnd: u32) -> Option<u32> {
        let target = self.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
        self.get_window(target)?;
        self.find_hmq_for_hwnd(target).or_else(|| self.find_hmq_for_hwnd(hwnd))
            .filter(|h| self.msg_queues.contains_key(h))
    }

    /// Take up the pointer event at the head of the input queue for the
    /// thread of queue `hmq`: undeliverable events at the head are dropped;
    /// a pointer event for that queue is hit-tested now, has its effects (a
    /// press activates its frame, a button-1 press beside the focused
    /// control takes the focus away) and becomes a message for its window.
    /// An event for another queue is left for that queue's thread.
    pub fn take_up_input(&mut self, hmq: u32) {
        while let Some(front) = self.input.front() {
            let InputEvent::Pointer { msg, x, y, mp2 } = front.ev else { return };
            let target = self.pointer_target_skipping(msg, x, y, &front.skip);
            let asked = front.asked;
            let Some(t) = target.filter(|t| self.input_queue_of(t.hwnd).is_some()) else {
                self.pop_input();
                continue;
            };
            if self.input_queue_of(t.hwnd) != Some(hmq) {
                return;
            }
            // A CS_HITTEST window is asked first (by the reading thread).
            if !t.held && asked != t.hwnd && self.asks_hit_test(t.hwnd) {
                self.hit_ask = Some(HitAsk { hmq, hwnd: t.hwnd, mp1: pack_point(t.x, t.y) });
                return;
            }
            if is_press(msg) && !t.held {
                self.activate_by_click(t.frame);
                if msg == WM_BUTTON1DOWN || msg == WM_BUTTON1DBLCLK {
                    self.click_beside_focus(t.frame, t.hwnd);
                }
            }
            // The wheel (WM_VSCROLL): the window under the pointer, or for a
            // scroll bar its owner, with mp1 0 rather than a point.
            let (hwnd, mp1) = if msg == WM_VSCROLL {
                let over_bar = self.get_window(t.hwnd).is_some_and(|w| w.class_name == "WC_SCROLLBAR");
                (if over_bar { self.notify_target(t.hwnd) } else { t.hwnd }, 0)
            } else {
                (t.hwnd, pack_point(t.x, t.y))
            };
            if let Some(front) = self.input.front_mut() {
                front.ev = InputEvent::Window { hwnd, msg, mp1, mp2 };
            }
            return;
        }
    }

    /// Where pointer message `msg` at desktop point `(x, y)` goes if it is
    /// read now: the capture window; for a move or a button-1 release the
    /// built-in control held with button 1; else the frame under the pointer
    /// unless it is disabled — a button-1 message there goes to the built-in
    /// control under the pointer, if any. `None`: no window takes it.
    pub fn pointer_target(&self, msg: u32, x: i32, y: i32) -> Option<PointerTarget> {
        self.pointer_target_skipping(msg, x, y, &[])
    }

    /// [`Self::pointer_target`] passing over the windows in `skip` (they
    /// answered `HT_TRANSPARENT`).
    pub fn pointer_target_skipping(&self, msg: u32, x: i32, y: i32, skip: &[u32]) -> Option<PointerTarget> {
        if self.capture_hwnd != 0 {
            let hwnd = self.capture_hwnd;
            let (lx, ly) = self.pointer_local(hwnd, x, y);
            let frame = self.find_frame_for_hwnd(hwnd);
            return Some(PointerTarget { hwnd, x: lx, y: ly, frame, held: true });
        }
        if matches!(msg, WM_MOUSEMOVE | WM_BUTTON1UP)
            && let Some((hwnd, lx, ly)) = self.pressed_control_point(x, y)
        {
            let frame = self.find_frame_for_hwnd(hwnd);
            return Some(PointerTarget { hwnd, x: lx, y: ly, frame, held: true });
        }
        let frame = self.z_hit_test(x, y)?;
        // The owner of a modal dialog is disabled: it takes no clicks (the
        // PM Guide: "disabled windows do not receive mouse messages").
        if self.frame_disabled(self.find_frame_for_hwnd(frame)) {
            return None;
        }
        let (lx, ly) = self.pointer_local(frame, x, y);
        let (hwnd, lx, ly) = self.window_under_pointer(frame, lx, ly, skip).unwrap_or((frame, lx, ly));
        let _ = msg;
        Some(PointerTarget { hwnd, x: lx, y: ly, frame, held: false })
    }

    /// The deepest visible window of `frame` under point `(x, y)` in message
    /// coordinates (`pointer_local`), with the point relative to it: the
    /// topmost child there, then its topmost child there, and so on. Static
    /// controls let the pointer through, a disabled window takes nothing (a
    /// `CS_HITTEST` one is asked instead), windows in `skip` and the menu bar
    /// are passed over. `None`: the frame itself (its client).
    pub fn window_under_pointer(&self, frame: u32, x: i32, y: i32, skip: &[u32]) -> Option<(u32, i32, i32)> {
        let (sx, sy) = self.frame_surface_point(frame, x, y);
        let client = self.frame_to_client.get(&frame).copied();
        let mut at = frame;
        let mut found = None;
        'down: for _ in 0..MAX_HIT_DEPTH {
            let kids: Vec<u32> = self.get_window(at).map(|w| w.children.clone()).unwrap_or_default();
            for &h in kids.iter().rev() {
                let Some(w) = self.get_window(h) else { continue };
                // A standard frame's client counts as shown with its frame.
                if (!w.visible && Some(h) != client) || skip.contains(&h) || w.class_name == "#Menu"
                    || w.class_name == "WC_STATIC" {
                    continue;
                }
                let (ax, ay, cx, cy) = self.get_abs_rect_in_frame(h);
                // A combination box itself is only its entry row; below it
                // lies its list, or what is under the box.
                let bottom = if w.combo.is_some() { ay + cy - super::pm_combo::COMBO_ENTRY_H } else { ay };
                if !(sx >= ax && sx < ax + cx && sy >= bottom && sy < ay + cy) { continue; }
                if w.style & WS_DISABLED != 0 && !self.asks_hit_test(h) { break 'down; }
                at = h;
                if Some(h) != client { found = Some((h, sx - ax, sy - ay)); }
                continue 'down;
            }
            break;
        }
        // Dropped lists of combination boxes lie over their neighbours.
        if let Some(hit) = self.control_at(frame, sx, sy)
            && self.get_window(hit.0).is_some_and(|w| w.class_name == super::pm_controls::CLASS_LISTBOX)
            && self.get_window(hit.0).and_then(|w| self.get_window(w.parent)).is_some_and(|p| p.combo.is_some())
            && !skip.contains(&hit.0) {
            return Some(hit);
        }
        found
    }

    /// Message coordinates of `frame` (`pointer_local`) in its surface: a
    /// standard frame's client lies at its place there; a dialog's surface
    /// starts inside its left border.
    fn frame_surface_point(&self, frame: u32, x: i32, y: i32) -> (i32, i32) {
        match self.get_window(frame) {
            Some(w) if !self.frame_to_client.contains_key(&frame) =>
                (x - super::constants::frame_border_width(w.frame_flags), y),
            _ => {
                let (cx, cy, _, _) = self.client_view(frame).unwrap_or_default();
                (x.saturating_add(cx), y.saturating_add(cy))
            }
        }
    }

    /// Whether `hwnd`'s class has `CS_HITTEST` and a window procedure to
    /// answer `WM_HITTEST`.
    pub fn asks_hit_test(&self, hwnd: u32) -> bool {
        self.get_window(hwnd).is_some_and(|w| w.pfn_wp != 0
            && self.get_class(&w.class_name).is_some_and(|c| c.style & CS_HITTEST != 0))
    }

    /// The `WM_HITTEST` waiting to be asked by the thread of queue `hmq`.
    pub fn take_hit_ask(&mut self, hmq: u32) -> Option<HitAsk> {
        self.hit_ask.filter(|a| a.hmq == hmq)
    }

    /// Window `hwnd` answered `WM_HITTEST` with `answer` for the pointer
    /// event at the head of the input queue.
    pub fn hit_test_answered(&mut self, hwnd: u32, answer: i32) {
        if self.hit_ask.is_none_or(|a| a.hwnd != hwnd) { return; }
        self.hit_ask = None;
        let Some(front) = self.input.front_mut() else { return };
        let InputEvent::Pointer { msg, x, y, .. } = front.ev else { return };
        match answer {
            HT_TRANSPARENT if front.skip.len() < MAX_HIT_SKIPS => front.skip.push(hwnd),
            HT_DISCARD | HT_TRANSPARENT => self.pop_input(),
            HT_ERROR => {
                // A press still brings its frame forward (the alarm is not
                // sounded).
                if is_press(msg) && let Some(t) = self.pointer_target(msg, x, y) && !t.held {
                    self.activate_by_click(t.frame);
                }
                self.pop_input();
            }
            _ => front.asked = hwnd,
        }
        self.wake_input_reader();
    }

    /// Desktop point `(x, y)` (bottom-left origin) in the coordinates of the
    /// window that gets the pointer messages addressed to `hwnd`: a standard
    /// frame's client (inside the frame's left border), else `hwnd` itself
    /// (a dialog, a window holding the capture).
    pub fn pointer_local(&self, hwnd: u32, x: i32, y: i32) -> (i32, i32) {
        let target = self.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
        let (ox, oy) = self.window_origin(target).unwrap_or((0, 0));
        (x - ox, y - oy)
    }

    /// The built-in control held down with mouse button 1, if any, and the
    /// desktop point `(x, y)` (bottom-left origin) relative to that control.
    pub fn pressed_control_point(&self, x: i32, y: i32) -> Option<(u32, i32, i32)> {
        let control = self.pressed_control;
        if control == 0 { return None; }
        let frame = self.find_frame_for_hwnd(control);
        let f = self.get_window(frame)?;
        // Frame-relative, then relative to the frame's surface, then to the control.
        let (sx, sy) = (x - f.x - super::constants::frame_border_width(f.frame_flags), y - f.y);
        let (bx, by, _, _) = self.get_abs_rect_in_frame(control);
        Some((control, sx - bx, sy - by))
    }

    /// The built-in control (or embedded dialog, ADR-0042) of `frame` under
    /// point `(x, y)` in message coordinates (`pointer_local`), with the
    /// point relative to it.
    pub fn control_under(&self, frame: u32, x: i32, y: i32) -> Option<(u32, i32, i32)> {
        // A standard frame's client lies at its place in the frame's surface
        // (above a horizontal frame scroll bar); a dialog's surface starts
        // inside its left border.
        let (sx, sy) = match self.get_window(frame) {
            Some(w) if !self.frame_to_client.contains_key(&frame) =>
                (x - super::constants::frame_border_width(w.frame_flags), y),
            _ => {
                let (cx, cy, _, _) = self.client_view(frame).unwrap_or_default();
                (x.saturating_add(cx), y.saturating_add(cy))
            }
        };
        self.control_at(frame, sx, sy).or_else(|| {
            let root = self.control_root(frame)?;
            self.embedded_dialog_at(root, sx, sy)
        })
    }

    /// A press on `frame` makes it the active window if it is not: it goes
    /// to the top and `WM_ACTIVATE` is posted, FALSE to the frame that was
    /// active, then TRUE to this one (each to a standard frame's client).
    fn activate_by_click(&mut self, frame: u32) {
        let prev = self.focused_hwnd;
        if !self.is_frame(frame) {
            return;
        }
        // Activated again: the desktop server may have given activation to
        // another program meanwhile (ADR-0108).
        self.claim_activation(frame);
        if prev == frame {
            return;
        }
        self.z_push_top(frame);
        if prev != 0 && self.get_window(prev).is_some() {
            self.post_msg(self.frame_msg_target(prev), WM_ACTIVATE, 0, frame);
        }
        self.post_msg(self.frame_msg_target(frame), WM_ACTIVATE, 1, prev);
    }

    /// A button-1 press on `hwnd` in `frame` that is not on a built-in
    /// control takes the keyboard focus from the control of that frame that
    /// has it (repainted without its caret).
    fn click_beside_focus(&mut self, frame: u32, hwnd: u32) {
        let focus = self.focus_hwnd;
        if hwnd != frame || focus == 0 || !self.frame_to_client.contains_key(&frame)
            || self.find_frame_for_hwnd(focus) != frame
        {
            return;
        }
        self.set_focus(0);
        self.invalidate_window_only(focus);
    }

    fn wake_queue(&self, hmq: u32) {
        if let Some(mq) = self.get_mq(hmq) {
            mq.lock_or_recover().cond.notify_all();
        }
    }
}

/// An event of the system input queue with when it arrived and where the
/// pointer was then (`QMSG.time` / `QMSG.ptl` of its message).
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedInput {
    pub ev: InputEvent,
    pub time: u32,
    pub pos: (i32, i32),
    /// The desktop server's event this came from, acknowledged once read.
    pub token: Option<u64>,
    /// A pointer event: the windows that answered `HT_TRANSPARENT`, and the
    /// one that answered `HT_NORMAL` (0: none yet).
    pub skip: Vec<u32>,
    pub asked: u32,
    /// Its serial number (`WinQueryQueueStatus` tells events apart by it).
    pub seq: u64,
}

impl Default for QueuedInput {
    fn default() -> Self {
        QueuedInput { ev: InputEvent::Char('\0'), time: 0, pos: (0, 0), token: None, skip: Vec::new(), asked: 0, seq: 0 }
    }
}

/// How a process acknowledges the desktop server's input: `(token, active
/// frame)`. Called with the window manager locked.
pub type InputAck = std::sync::Arc<dyn Fn(u64, crate::desktop::proto::Activation) + Send + Sync>;

/// The system input queue itself (a field of `WindowManager`).
pub type InputQueue = VecDeque<QueuedInput>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::proto::Activation;
    use crate::loader::constants::WM_BUTTON2UP;

    fn act(frame: u32, seq: u32) -> Activation {
        Activation { frame, seq }
    }

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

    fn post(wm: &WindowManager, hmq: u32, hwnd: u32, msg: u32) {
        wm.get_mq(hmq).unwrap().lock_or_recover().messages.push_back(
            OS2Message { hwnd, msg, mp1: 0, mp2: 0, time: 0, x: 0, y: 0 });
    }

    fn next_msg(wm: &mut WindowManager, hmq: u32) -> Option<u32> {
        wm.next_queued(hmq, true, |_, _| true).map(|m| m.msg)
    }

    #[test]
    fn posted_messages_then_input_then_paint_then_timer() {
        use crate::loader::constants::{WM_COMMAND, WM_SIZE};
        let (mut wm, [q1, _], [_, c1, ..], _) = two_frames();
        post(&wm, q1, c1, WM_TIMER);
        post(&wm, q1, c1, WM_PAINT);
        post(&wm, q1, c1, WM_SIZE);
        wm.post_input(InputEvent::Key { mp1: 0, mp2: 0 });
        post(&wm, q1, c1, WM_COMMAND);
        assert_eq!(next_msg(&mut wm, q1), Some(WM_SIZE));
        assert_eq!(next_msg(&mut wm, q1), Some(WM_COMMAND), "posted messages in posting order");
        assert_eq!(next_msg(&mut wm, q1), Some(WM_CHAR), "input after posted messages");
        assert_eq!(next_msg(&mut wm, q1), Some(WM_PAINT));
        assert_eq!(next_msg(&mut wm, q1), Some(WM_TIMER), "WM_TIMER last");
        assert_eq!(next_msg(&mut wm, q1), None);
    }

    #[test]
    fn a_filter_reaches_past_messages_of_higher_rank() {
        let (mut wm, [q1, _], [_, c1, ..], entry) = two_frames();
        post(&wm, q1, c1, WM_PAINT);
        post(&wm, q1, entry, WM_TIMER);
        wm.post_input(InputEvent::Key { mp1: 0, mp2: 0 });
        // Only the entry field's messages: the key (for the client) and the
        // client's WM_PAINT stay.
        let m = wm.next_queued(q1, true, |wm, m| wm.is_same_or_descendant(m.hwnd, entry));
        assert_eq!(m.map(|m| (m.hwnd, m.msg)), Some((entry, WM_TIMER)));
        // Peeking leaves the message.
        assert_eq!(wm.next_queued(q1, false, |_, _| true).map(|m| m.msg), Some(WM_CHAR));
        assert_eq!(next_msg(&mut wm, q1), Some(WM_CHAR));
        assert_eq!(next_msg(&mut wm, q1), Some(WM_PAINT));
    }

    #[test]
    fn the_input_queue_is_bounded() {
        let (mut wm, ..) = two_frames();
        for _ in 0..MAX_INPUT_EVENTS + 10 {
            wm.post_input(InputEvent::Key { mp1: 0, mp2: 0 });
        }
        assert_eq!(wm.pending_input(), MAX_INPUT_EVENTS);
    }

    /// `two_frames` with the frames on the desktop side by side — f1 at
    /// x 0, f2 at x 400, both 300 × 300 and visible, f2 on top — and the
    /// entry field at (10, 200) in c1, 200 × 25.
    fn placed_frames() -> (WindowManager, [u32; 2], [u32; 4], u32) {
        let (mut wm, qs, [f1, c1, f2, c2], entry) = two_frames();
        for (f, c, x) in [(f1, c1, 0), (f2, c2, 400)] {
            let w = wm.get_window_mut(f).unwrap();
            (w.x, w.y, w.cx, w.cy, w.visible) = (x, 0, 300, 300, true);
            let w = wm.get_window_mut(c).unwrap();
            (w.x, w.y, w.cx, w.cy, w.visible) = (0, 0, 300, 280, true);
            wm.z_push_top(f);
        }
        let w = wm.get_window_mut(entry).unwrap();
        (w.x, w.y, w.cx, w.cy) = (10, 200, 200, 25);
        (wm, qs, [f1, c1, f2, c2], entry)
    }

    fn pointer(wm: &mut WindowManager, msg: u32, x: i32, y: i32) {
        wm.post_input(InputEvent::Pointer { msg, x, y, mp2: 0 });
    }

    #[test]
    fn a_pointer_message_goes_to_the_window_under_the_pointer_when_read() {
        let (mut wm, [q1, q2], [f1, c1, f2, c2], _) = placed_frames();
        pointer(&mut wm, WM_MOUSEMOVE, 450, 100);
        // The frames swap places before the move is read: it goes to f1.
        wm.get_window_mut(f2).unwrap().x = 0;
        wm.get_window_mut(f1).unwrap().x = 400;
        assert_eq!(take(&mut wm, q2), None);
        let (hwnd, msg, mp1, _) = take(&mut wm, q1).unwrap();
        assert_eq!((hwnd, msg), (c1, WM_MOUSEMOVE));
        assert_eq!(mp1, pack_point(wm.pointer_local(f1, 450, 100).0, wm.pointer_local(f1, 450, 100).1));
        // Over no window at all, the event is dropped.
        pointer(&mut wm, WM_MOUSEMOVE, 900, 900);
        assert_eq!(take(&mut wm, q1), None);
        assert_eq!(wm.pending_input(), 0);
        let _ = c2;
    }

    #[test]
    fn a_click_activates_its_frame_when_read_and_later_keys_follow_it() {
        let (mut wm, [q1, q2], [f1, c1, f2, c2], _) = placed_frames();
        wm.focused_hwnd = f2;
        // The PM Guide's example: a key, a click on another window, a key.
        wm.post_input(InputEvent::Key { mp1: 1, mp2: 0 });
        pointer(&mut wm, WM_BUTTON1DOWN, 100, 100);
        wm.post_input(InputEvent::Key { mp1: 2, mp2: 0 });
        // Nothing changed on arrival.
        assert_eq!(wm.focused_hwnd, f2);
        assert_eq!(next_msg(&mut wm, q1), None, "the first key is f2's");
        assert_eq!(next_msg(&mut wm, q2), Some(WM_CHAR));
        // Reading the click activates f1: WM_ACTIVATE FALSE to c2, TRUE to
        // c1 — read before the click — and the next key goes to c1.
        assert_eq!(next_msg(&mut wm, q1), Some(WM_ACTIVATE));
        assert_eq!(wm.focused_hwnd, f1);
        assert_eq!(wm.z_order.last(), Some(&f1));
        let m = wm.next_queued(q1, true, |_, _| true).unwrap();
        assert_eq!((m.hwnd, m.msg), (c1, WM_BUTTON1DOWN));
        assert_eq!(take(&mut wm, q1), Some((c1, WM_CHAR, 2, 0)));
        let deact = wm.next_queued(q2, true, |_, _| true).unwrap();
        assert_eq!((deact.hwnd, deact.msg, deact.mp1, deact.mp2), (c2, WM_ACTIVATE, 0, f1));
    }

    #[test]
    fn button1_is_routed_to_a_control_and_a_press_beside_it_takes_its_focus() {
        let (mut wm, [q1, _], [f1, c1, ..], entry) = placed_frames();
        wm.focused_hwnd = f1;
        let (ex, ey) = {
            let (ox, oy) = wm.window_origin(c1).unwrap();
            (ox + 15, oy + 210)
        };
        pointer(&mut wm, WM_BUTTON1DOWN, ex, ey);
        let (hwnd, msg, mp1, _) = take(&mut wm, q1).unwrap();
        assert_eq!((hwnd, msg, mp1), (entry, WM_BUTTON1DOWN, pack_point(5, 10)));
        // Every button and the moves go to the window under the pointer.
        pointer(&mut wm, WM_BUTTON2DOWN, ex, ey);
        assert_eq!(take(&mut wm, q1).map(|m| m.0), Some(entry));
        // A press beside the control takes the focus when it is read.
        wm.focus_hwnd = entry;
        pointer(&mut wm, WM_BUTTON1DOWN, 250, 50);
        assert_eq!(wm.focus_hwnd, entry, "not on arrival");
        assert_eq!(take(&mut wm, q1).map(|m| m.0), Some(c1));
        assert_eq!(wm.focus_hwnd, 0);
        // A release beside it does not.
        wm.focus_hwnd = entry;
        pointer(&mut wm, WM_BUTTON1UP, 250, 50);
        assert_eq!(take(&mut wm, q1).map(|m| m.0), Some(c1));
        assert_eq!(wm.focus_hwnd, entry);
    }

    /// A child window of the program's own class under the pointer: placed
    /// at (20, 20) in c1, 60×40, with a static over its left half and a
    /// grandchild at (40, 10), 10×10.
    fn with_child(wm: &mut WindowManager, c1: u32, q1: u32, class_style: u32) -> (u32, u32, u32) {
        wm.register_class("Kid".into(), 0x5000, class_style);
        let kid = wm.create_window("Kid".into(), c1, q1);
        let w = wm.get_window_mut(kid).unwrap();
        (w.x, w.y, w.cx, w.cy, w.visible, w.pfn_wp) = (20, 20, 60, 40, true, 0x5000);
        let grand = wm.create_window("Kid".into(), kid, q1);
        let w = wm.get_window_mut(grand).unwrap();
        (w.x, w.y, w.cx, w.cy, w.visible, w.pfn_wp) = (40, 10, 10, 10, true, 0x5000);
        let label = wm.create_window("WC_STATIC".into(), kid, q1);
        let w = wm.get_window_mut(label).unwrap();
        (w.x, w.y, w.cx, w.cy, w.visible) = (0, 0, 30, 40, true);
        (kid, grand, label)
    }

    #[test]
    fn pointer_input_goes_to_the_deepest_window_under_it() {
        let (mut wm, [q1, _], [_, c1, ..], _) = placed_frames();
        let (kid, grand, _) = with_child(&mut wm, c1, q1, 0);
        // A standard frame's client need not carry WS_VISIBLE itself.
        wm.get_window_mut(c1).unwrap().visible = false;
        let (ox, oy) = wm.window_origin(c1).unwrap();
        pointer(&mut wm, WM_MOUSEMOVE, ox + 25, oy + 25);
        assert_eq!(take(&mut wm, q1).map(|m| (m.0, m.2)), Some((kid, pack_point(5, 5))), "through the static");
        pointer(&mut wm, WM_BUTTON2DOWN, ox + 65, oy + 35);
        assert_eq!(take(&mut wm, q1).map(|m| (m.0, m.2)), Some((grand, pack_point(5, 5))));
        pointer(&mut wm, WM_BUTTON1DOWN, ox + 150, oy + 100);
        assert_eq!(take(&mut wm, q1).map(|m| m.0), Some(c1), "beside them: the client");
        // A disabled window takes nothing: its parent does.
        wm.get_window_mut(grand).unwrap().style |= WS_DISABLED;
        pointer(&mut wm, WM_BUTTON1DOWN, ox + 65, oy + 35);
        assert_eq!(take(&mut wm, q1).map(|m| m.0), Some(kid));
    }

    #[test]
    fn a_cs_hittest_window_is_asked_when_the_event_is_read() {
        let (mut wm, [q1, _], [_, c1, ..], _) = placed_frames();
        let (kid, grand, _) = with_child(&mut wm, c1, q1, CS_HITTEST);
        let (ox, oy) = wm.window_origin(c1).unwrap();
        // HT_TRANSPARENT: the window beneath (here its parent) is asked next.
        pointer(&mut wm, WM_BUTTON1DOWN, ox + 65, oy + 35);
        assert_eq!(take(&mut wm, q1), None, "waits for the answer");
        assert_eq!(wm.next_queued(q1, true, |_, _| true), None, "paint and timers wait too");
        assert_eq!(wm.take_hit_ask(q1), Some(HitAsk { hmq: q1, hwnd: grand, mp1: pack_point(5, 5) }));
        wm.hit_test_answered(grand, HT_TRANSPARENT);
        assert_eq!(take(&mut wm, q1), None);
        assert_eq!(wm.take_hit_ask(q1).map(|a| a.hwnd), Some(kid));
        wm.hit_test_answered(kid, HT_NORMAL);
        assert_eq!(take(&mut wm, q1).map(|m| (m.0, m.2)), Some((kid, pack_point(45, 15))));
        assert_eq!(wm.take_hit_ask(q1), None);
        // HT_DISCARD drops the event; the next one is asked again.
        pointer(&mut wm, WM_MOUSEMOVE, ox + 25, oy + 25);
        assert_eq!(take(&mut wm, q1), None);
        wm.hit_test_answered(kid, HT_DISCARD);
        assert_eq!(wm.pending_input(), 0);
        // HT_ERROR drops it too.
        pointer(&mut wm, WM_BUTTON1DOWN, ox + 25, oy + 25);
        assert_eq!(take(&mut wm, q1), None);
        wm.hit_test_answered(kid, HT_ERROR);
        assert_eq!(wm.pending_input(), 0);
        // An answer for a window not asked changes nothing.
        pointer(&mut wm, WM_MOUSEMOVE, ox + 25, oy + 25);
        assert_eq!(take(&mut wm, q1), None);
        wm.hit_test_answered(grand, HT_DISCARD);
        assert_eq!(wm.pending_input(), 1);
    }

    #[test]
    fn mouse_moves_are_coalesced() {
        let (mut wm, [q1, _], [_, c1, ..], _) = placed_frames();
        let (ox, oy) = wm.window_origin(c1).unwrap();
        for i in 0..5 { pointer(&mut wm, WM_MOUSEMOVE, ox + 100 + i, oy + 100); }
        assert_eq!(wm.pending_input(), 1, "one move");
        pointer(&mut wm, WM_BUTTON1DOWN, ox + 104, oy + 100);
        pointer(&mut wm, WM_MOUSEMOVE, ox + 110, oy + 100);
        assert_eq!(wm.pending_input(), 3, "not across a click");
        assert_eq!(take(&mut wm, q1).map(|m| (m.1, m.2)), Some((WM_MOUSEMOVE, pack_point(104, 100))), "the last position");
    }

    #[test]
    fn queue_status_reports_what_waits_and_what_came_since_the_last_call() {
        let (mut wm, [q1, q2], [_, c1, _, c2], _) = placed_frames();
        assert_eq!(wm.queue_status(q1), 0);
        wm.post_input(InputEvent::Key { mp1: 0, mp2: 0 });
        post(&wm, q1, c1, WM_TIMER);
        assert_eq!(wm.queue_status(q1), ((QS_KEY | QS_TIMER) << 16) | QS_KEY | QS_TIMER);
        assert_eq!(wm.queue_status(q1), (QS_KEY | QS_TIMER) << 16, "nothing new");
        let (ox, oy) = wm.window_origin(c1).unwrap();
        pointer(&mut wm, WM_BUTTON1DOWN, ox + 150, oy + 100);
        post(&wm, q1, c1, 0x0025);
        assert_eq!(wm.queue_status(q1), ((QS_KEY | QS_TIMER | QS_MOUSEBUTTON | QS_SEM1) << 16) | QS_MOUSEBUTTON | QS_SEM1);
        assert_eq!(wm.queue_status(q2), 0, "the other queue's input is not its own");
        post(&wm, q2, c2, WM_PAINT);
        assert_eq!(wm.queue_status(q2), (QS_PAINT << 16) | QS_PAINT);
        assert_eq!(wm.queue_status(999), 0);
    }

    #[test]
    fn a_disabled_frame_takes_no_pointer_input() {
        let (mut wm, [q1, _], [f1, _, f2, _], _) = placed_frames();
        wm.focused_hwnd = f2;
        wm.get_window_mut(f1).unwrap().style |= crate::loader::constants::WS_DISABLED;
        pointer(&mut wm, WM_BUTTON1DOWN, 100, 100);
        assert_eq!(take(&mut wm, q1), None);
        assert_eq!(wm.pending_input(), 0);
        assert_eq!(wm.focused_hwnd, f2);
    }

    #[test]
    fn the_capture_and_a_held_control_take_the_pointer_without_activation() {
        let (mut wm, [q1, q2], [f1, c1, f2, c2], entry) = placed_frames();
        wm.focused_hwnd = f2;
        // The capture window gets a press anywhere, in its own coordinates,
        // and nothing is activated.
        wm.capture_hwnd = c2;
        pointer(&mut wm, WM_BUTTON1DOWN, 100, 100);
        let (hwnd, msg, mp1, _) = take(&mut wm, q2).unwrap();
        let (lx, ly) = wm.pointer_local(c2, 100, 100);
        assert_eq!((hwnd, msg, mp1), (c2, WM_BUTTON1DOWN, pack_point(lx, ly)));
        assert_eq!(wm.focused_hwnd, f2);
        // A control held with button 1 gets moves and the release.
        wm.capture_hwnd = 0;
        wm.pressed_control = entry;
        pointer(&mut wm, WM_MOUSEMOVE, 600, 100);
        pointer(&mut wm, WM_BUTTON1UP, 600, 100);
        assert_eq!(take(&mut wm, q1).map(|m| (m.0, m.1)), Some((entry, WM_MOUSEMOVE)));
        assert_eq!(take(&mut wm, q1).map(|m| (m.0, m.1)), Some((entry, WM_BUTTON1UP)));
        // Button 2 is not the held button's: hit-tested as usual.
        pointer(&mut wm, WM_BUTTON2UP, 100, 100);
        assert_eq!(take(&mut wm, q1).map(|m| m.0), Some(c1));
        let _ = (f1, q2);
    }

    #[test]
    fn pinning_leaves_pointer_events_to_be_hit_tested_when_read() {
        let (mut wm, [q1, _], [_, c1, ..], _) = placed_frames();
        pointer(&mut wm, WM_MOUSEMOVE, 100, 100);
        wm.pin_pending_input();
        assert!(matches!(wm.input.front().map(|q| &q.ev), Some(InputEvent::Pointer { .. })));
        assert_eq!(take(&mut wm, q1).map(|m| m.0), Some(c1));
    }

    #[test]
    fn server_input_is_acknowledged_when_its_last_event_is_taken() {
        use std::sync::{Arc, Mutex};
        let (mut wm, [q1, _], [f1, c1, ..], _) = placed_frames();
        wm.focused_hwnd = f1;
        let acks = Arc::new(Mutex::new(Vec::new()));
        let sink = acks.clone();
        wm.input_ack = Some(Arc::new(move |t, active| sink.lock().unwrap().push((t, active))));
        // One server event giving two input events (as a key and its text).
        wm.input_tag = Some(7);
        wm.post_input(InputEvent::Key { mp1: 1, mp2: 0 });
        wm.post_input(InputEvent::Char('a'));
        wm.input_tag = None;
        assert!(wm.input_tag_used);
        assert_eq!(take(&mut wm, q1).map(|m| m.0), Some(c1));
        assert!(acks.lock().unwrap().is_empty(), "one of its events still waits");
        assert_eq!(take(&mut wm, q1).map(|m| m.0), Some(c1));
        assert_eq!(*acks.lock().unwrap(), vec![(7, act(f1, 1))], "its first report of the active frame");
        // Peeking does not acknowledge; a dropped event does.
        wm.input_tag = Some(8);
        wm.post_input(InputEvent::Key { mp1: 2, mp2: 0 });
        wm.input_tag = Some(9);
        wm.post_input(InputEvent::Pointer { msg: WM_MOUSEMOVE, x: 5000, y: 5000, mp2: 0 });
        wm.input_tag = None;
        assert!(wm.next_input(q1, false, |_| true).is_some());
        assert_eq!(acks.lock().unwrap().len(), 1);
        assert!(take(&mut wm, q1).is_some());
        assert_eq!(take(&mut wm, q1), None, "the move over no window is dropped");
        assert_eq!(acks.lock().unwrap()[1..], [(8, act(f1, 1)), (9, act(f1, 1))], "repeated");
    }

    #[test]
    fn a_drag_drops_waiting_pointer_input_but_keeps_keys() {
        use std::sync::{Arc, Mutex};
        let (mut wm, [q1, _], [_, c1, ..], _) = placed_frames();
        let acks = Arc::new(Mutex::new(Vec::new()));
        let sink = acks.clone();
        wm.input_ack = Some(Arc::new(move |t, _| sink.lock().unwrap().push(t)));
        wm.input_tag = Some(1);
        pointer(&mut wm, WM_MOUSEMOVE, 100, 100);
        wm.post_input(InputEvent::Window { hwnd: c1, msg: WM_BUTTON2DOWN, mp1: 0, mp2: 0 });
        wm.input_tag = Some(2);
        wm.post_input(InputEvent::Key { mp1: 5, mp2: 0 });
        wm.input_tag = None;
        wm.drop_pointer_input();
        assert_eq!(*acks.lock().unwrap(), vec![1]);
        assert_eq!(take(&mut wm, q1), Some((c1, WM_CHAR, 5, 0)));
        assert_eq!(*acks.lock().unwrap(), vec![1, 2]);
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

    #[test]
    fn shift_f10_is_the_context_menu_key() {
        let (mut wm, [q1, _], [_, c1, ..], _) = two_frames();
        let f10 = VK_F10 << WM_CHAR_VKEY_SHIFT;
        assert!(is_context_key(KC_VIRTUALKEY | KC_SHIFT, f10));
        assert!(!is_context_key(KC_VIRTUALKEY, f10), "F10 alone is the menu key");
        assert!(!is_context_key(KC_VIRTUALKEY | KC_SHIFT | KC_KEYUP, f10));
        assert!(!is_context_key(KC_VIRTUALKEY | KC_SHIFT | KC_CTRL, f10));
        wm.post_input(InputEvent::Key { mp1: KC_VIRTUALKEY | KC_SHIFT, mp2: f10 });
        assert_eq!(take(&mut wm, q1), Some((c1, WM_CONTEXTMENU, 0, 0)), "not from the pointer");
    }

    #[test]
    fn the_wheel_scrolls_the_window_under_the_pointer_or_a_scroll_bars_owner() {
        let (mut wm, [q1, _], [_, c1, ..], _) = placed_frames();
        let down = crate::loader::constants::SB_LINEDOWN << 16;
        wm.post_input(InputEvent::Pointer { msg: WM_VSCROLL, x: 100, y: 100, mp2: down });
        assert_eq!(take(&mut wm, q1), Some((c1, WM_VSCROLL, 0, down)), "mp1 0, not a point");
        // Over a scroll bar: its owner scrolls.
        let bar = wm.create_window("WC_SCROLLBAR".into(), c1, q1);
        let w = wm.get_window_mut(bar).unwrap();
        (w.x, w.y, w.cx, w.cy, w.visible, w.owner) = (0, 0, 300, 280, true, c1);
        wm.post_input(InputEvent::Pointer { msg: WM_VSCROLL, x: 100, y: 100, mp2: down });
        assert_eq!(take(&mut wm, q1), Some((c1, WM_VSCROLL, 0, down)));
        // The context-menu click: the point, mp2 TRUE.
        wm.post_input(InputEvent::Pointer { msg: WM_CONTEXTMENU, x: 100, y: 100, mp2: 1 });
        let (hwnd, msg, _, mp2) = take(&mut wm, q1).unwrap();
        assert_eq!((msg, mp2), (WM_CONTEXTMENU, 1));
        assert!(hwnd == bar || hwnd == c1);
    }
}
