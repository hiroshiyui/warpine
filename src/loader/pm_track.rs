// SPDX-License-Identifier: GPL-3.0-or-later
//
//! `WinTrackRect` / `WinShowTrackRect` (PMWIN 890 / 882; ADR-0089):
//! tracking a rectangle with the pointer or the keyboard on the calling
//! thread (IBM *PM Programming Reference*).
//!
//! - **Input.** The call captures the pointer for the tracking window, so
//!   every move and button release reaches the thread's queue through the
//!   usual routing (`pm_input.rs`), and loops on that queue
//!   ([`super::Loader::take_message`]), taking only pointer and keyboard
//!   messages (`WM_MOUSEMOVE` .. `WM_CHAR`); other messages wait.
//!   - Pointer moves move the edges.
//!   - Releasing the button that was down when the call began accepts (any
//!     release when none was), and so does Enter.
//!   - Esc cancels.
//!   - The arrows step by `cxKeyboard` / `cyKeyboard`.
//!   - The rectangle arithmetic is [`super::track::Tracker`].
//! - **Messages other programs send** the thread, and `WM_HITTEST` asks,
//!   run as `WinGetMsg` runs them; the call is then made again
//!   (`AfterSent::Rerun`) and finds its session, so it goes on. A session
//!   knows its call by the return address and the `TRACKINFO` pointer; a
//!   second `WinTrackRect` on the thread while one runs (from a window
//!   procedure) fails.
//! - **The outline** is the rectangle's border (`cxBorder` / `cyBorder`
//!   wide) inverted on `hps`, or on a PS of `hwnd` when `hps` is
//!   NULLHANDLE. Drawing it again removes it. `WinShowTrackRect(FALSE)` /
//!   `(TRUE)` hide and show it, counted. With `hwnd` = `HWND_DESKTOP` and no
//!   `hps`, nothing is drawn (ADR-0089).
//! - **At the end** the outline goes, the capture goes back to the window
//!   that had it, and `rclTrack` holds the rectangle (the starting one
//!   after a cancel). Returns TRUE when accepted, FALSE when cancelled.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_win::Taken;
use super::track::{Rectl, TrackInfo, Tracker, TI_RCL_TRACK, TRACKINFO_SIZE};
use super::ApiResult;
use crate::gui::surface::{DrawOp, Mix};
use crate::gui::GUIMessage;
use log::debug;

/// A `WinTrackRect` in progress on one thread (a field of
/// `WindowManager`, by vCPU).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackSession {
    /// The window the rectangle is in.
    pub hwnd: u32,
    /// The call: its return address and `TRACKINFO` pointer.
    call: (u32, u32),
    tracker: Tracker,
    /// The rectangle now.
    current: Rectl,
    /// Where the outline is drawn (0: nowhere), and whether Warpine made
    /// that presentation space.
    hps: u32,
    own_ps: bool,
    border: (i32, i32),
    /// The outline drawn now.
    shown: Option<Rectl>,
    /// `WinShowTrackRect(FALSE)` calls not yet undone.
    hidden: u32,
    /// The window holding the capture for the tracking, and the one that
    /// held it before.
    capture: u32,
    prev_capture: u32,
    /// The release that ends a pointer tracking (`WM_BUTTONnUP`; 0: any).
    end_on: u32,
    /// The call is being made again after a message the thread ran.
    resuming: bool,
}

/// The pointer-and-keyboard message range the tracking takes.
const TRACK_FIRST: u32 = WM_MOUSEMOVE;
const TRACK_LAST: u32 = WM_CHAR;

/// The button-release message for a button-down message, if it is one.
fn release_of(down: u32) -> Option<u32> {
    match down {
        WM_BUTTON1DOWN => Some(WM_BUTTON1UP),
        WM_BUTTON2DOWN => Some(WM_BUTTON2UP),
        WM_BUTTON3DOWN => Some(WM_BUTTON3UP),
        _ => None,
    }
}

/// The border strips of `r`, `b` wide, that together invert the outline
/// once: top and bottom full width, left and right between them; the
/// whole rectangle when the strips would overlap. Inclusive corners.
fn outline_strips(r: Rectl, b: (i32, i32)) -> Vec<(i32, i32, i32, i32)> {
    let (bx, by) = (b.0.max(1), b.1.max(1));
    if r.width() <= 0 || r.height() <= 0 {
        return Vec::new();
    }
    let (l, bo, ri, t) = (r.left, r.bottom, r.right.saturating_sub(1), r.top.saturating_sub(1));
    if r.width() <= bx.saturating_mul(2) || r.height() <= by.saturating_mul(2) {
        return vec![(l, bo, ri, t)];
    }
    vec![
        (l, t - by + 1, ri, t),
        (l, bo, ri, bo + by - 1),
        (l, bo + by, l + bx - 1, t - by),
        (ri - bx + 1, bo + by, ri, t - by),
    ]
}

impl super::Loader {
    /// Invert the outline of `r` on `hps` (nothing for no PS).
    fn invert_outline(&self, hps: u32, r: Rectl, border: (i32, i32)) {
        if hps == 0 { return; }
        let Some(t) = self.ps_target(hps) else { return };
        for (x1, y1, x2, y2) in outline_strips(r, border) {
            self.draw_ps(&t, DrawOp::Box { x1, y1, x2, y2, color: 0, fill: true, mix: Mix::Invert });
        }
    }

    /// Show the session's outline at `r`, removing the old one (when it
    /// is not hidden).
    fn move_outline(&self, s: &mut TrackSession, r: Rectl) {
        if s.hidden == 0 {
            if let Some(old) = s.shown { self.invert_outline(s.hps, old, s.border); }
            self.invert_outline(s.hps, r, s.border);
            s.shown = Some(r);
        }
        s.current = r;
    }

    /// Set the capture to `hwnd`, as `WinSetCapture` does.
    fn set_track_capture(&self, hwnd: u32) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        wm.capture_hwnd = hwnd;
        if let Some(tx) = &wm.gui_tx {
            let _ = tx.send(GUIMessage::SetMouseCapture(hwnd));
        }
    }

    /// WinTrackRect(HWND hwnd, HPS hps, PTRACKINFO pti) -> BOOL. `ret` is
    /// the call's return address (see [`TrackSession::call`]).
    pub(super) fn win_track_rect(&self, vcpu: u32, ordinal: u32, ret: u32, hwnd: u32, hps: u32, pti: u32) -> ApiResult {
        let existing = self.shared.window_mgr.lock_or_recover().tracks.get(&vcpu).copied();
        if let Some(mut s) = existing {
            if s.resuming && s.call == (ret, pti) {
                s.resuming = false;
                self.shared.window_mgr.lock_or_recover().tracks.insert(vcpu, s);
                return self.track_loop(vcpu, ordinal);
            }
            // A second tracking on the thread (unverified error).
            return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS);
        }
        if !self.has_msg_queue(vcpu) { return self.pm_fail(vcpu, PMERR_NO_MSG_QUEUE); }
        let desktop = hwnd == HWND_DESKTOP;
        if !desktop && !self.pm_check_hwnd(vcpu, hwnd, false) { return ApiResult::Normal(0); }
        let Some(info) = self.guest_read_bytes(pti, TRACKINFO_SIZE as usize).and_then(|b| TrackInfo::parse(&b)) else {
            return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS);
        };
        // The pointer goes to the tracking window; for the desktop to the
        // thread's focus window, when it has it.
        let (capture, prev_capture, origin, pointer, end_on) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let own = |h: u32| wm.get_window(h).is_some_and(|w| wm.queue_thread(w.hmq) == Some(vcpu));
            let capture = if desktop { Some(wm.focus_hwnd).filter(|&f| own(f)) } else { Some(hwnd) };
            let origin = wm.window_origin(hwnd).unwrap_or((0, 0));
            let held = [(VK_BUTTON1, WM_BUTTON1UP), (VK_BUTTON2, WM_BUTTON2UP), (VK_BUTTON3, WM_BUTTON3UP)]
                .into_iter().find(|&(vk, _)| wm.key_state.key_state(vk) & KEY_STATE_DOWN != 0).map_or(0, |(_, up)| up);
            (capture, wm.capture_hwnd, origin, wm.key_state.pointer, held)
        };
        let Some(capture) = capture else { return self.pm_fail(vcpu, PMERR_INVALID_HWND) };
        let mut tracker = Tracker::new(info, (pointer.0 - origin.0, pointer.1 - origin.1));
        if info.fs & TF_SETPOINTERPOS != 0 {
            let at = tracker.pointer_start();
            let _ = self.win_set_pointer_pos(vcpu, HWND_DESKTOP, at.0 + origin.0, at.1 + origin.1);
            tracker = Tracker::new(info, at);
        }
        let (ps, own_ps) = match hps {
            0 if desktop => (0, false),
            0 => (self.shared.window_mgr.lock_or_recover().create_ps(hwnd), true),
            h => (h, false),
        };
        debug!("  [VCPU {}] WinTrackRect hwnd={} rect={:?} fs={:#x}", vcpu, hwnd, info.track, info.fs);
        let mut s = TrackSession {
            hwnd, call: (ret, pti), tracker, current: info.track, hps: ps, own_ps,
            border: info.border, shown: None, hidden: 0, capture, prev_capture, end_on, resuming: false,
        };
        self.move_outline(&mut s, info.track);
        self.set_track_capture(capture);
        self.shared.window_mgr.lock_or_recover().tracks.insert(vcpu, s);
        self.track_loop(vcpu, ordinal)
    }

    /// Take the tracking's input until it ends, or until the thread is
    /// to run a message first.
    fn track_loop(&self, vcpu: u32, ordinal: u32) -> ApiResult {
        use super::pm_remote::AfterSent;
        loop {
            let taken = self.take_message(vcpu, 0, TRACK_FIRST, TRACK_LAST, PM_REMOVE, true, false);
            let Some(mut s) = self.shared.window_mgr.lock_or_recover().tracks.get(&vcpu).copied() else {
                return ApiResult::Normal(0);
            };
            let resume = |s: &mut TrackSession| {
                s.resuming = true;
                self.shared.window_mgr.lock_or_recover().tracks.insert(vcpu, *s);
            };
            match taken {
                Some(Taken::Sent(m)) => { resume(&mut s); return self.run_sent(vcpu, m, AfterSent::Rerun(ordinal)); }
                Some(Taken::HitTest(a)) => { resume(&mut s); return self.hit_test_chain(vcpu, a, AfterSent::Rerun(ordinal)); }
                // Interrupted (the call is made again) or the process ends.
                None => { resume(&mut s); return ApiResult::Normal(0); }
                Some(Taken::Message(m)) => {
                    let end = match m.msg {
                        WM_QUIT => {
                            // WM_QUIT cancels; it stays for the program's own loop.
                            let wm = self.shared.window_mgr.lock_or_recover();
                            if let Some(&hmq) = wm.tid_to_hmq.get(&vcpu) { wm.post_queue_msg(hmq, WM_QUIT, m.mp1, m.mp2); }
                            Some(false)
                        }
                        WM_MOUSEMOVE => {
                            let origin = |h: u32| self.shared.window_mgr.lock_or_recover().window_origin(h).unwrap_or((0, 0));
                            let (o_msg, o_trk) = (origin(m.hwnd), origin(s.hwnd));
                            let x = (m.mp1 & 0xFFFF) as i16 as i32 + o_msg.0 - o_trk.0;
                            let y = (m.mp1 >> 16) as i16 as i32 + o_msg.1 - o_trk.1;
                            let r = s.tracker.pointer((x, y));
                            if r != s.current { self.move_outline(&mut s, r); }
                            None
                        }
                        WM_BUTTON1UP | WM_BUTTON2UP | WM_BUTTON3UP if s.end_on == 0 || s.end_on == m.msg => Some(true),
                        WM_CHAR => {
                            let flags = m.mp1 & 0xFFFF;
                            let vk = (m.mp2 >> WM_CHAR_VKEY_SHIFT) & 0xFF;
                            if flags & KC_KEYUP != 0 || flags & KC_VIRTUALKEY == 0 { None } else {
                                let dir = match vk {
                                    VK_LEFT => Some((-1, 0)), VK_RIGHT => Some((1, 0)),
                                    VK_UP => Some((0, 1)), VK_DOWN => Some((0, -1)),
                                    _ => None,
                                };
                                match (vk, dir) {
                                    (_, Some(d)) => {
                                        let r = s.tracker.step(d);
                                        if r != s.current { self.move_outline(&mut s, r); }
                                        None
                                    }
                                    (VK_NEWLINE | VK_ENTER, _) => Some(true),
                                    (VK_ESC, _) => Some(false),
                                    _ => None,
                                }
                            }
                        }
                        // A press with no button held makes the next release end it.
                        down => {
                            if s.end_on == 0 && let Some(up) = release_of(down) { s.end_on = up; }
                            None
                        }
                    };
                    if let Some(accept) = end {
                        return self.track_end(vcpu, s, accept);
                    }
                    self.shared.window_mgr.lock_or_recover().tracks.insert(vcpu, s);
                }
            }
        }
    }

    /// End the tracking: remove the outline, give the capture back, write
    /// `rclTrack` (the start after a cancel) and return whether it was
    /// accepted.
    fn track_end(&self, vcpu: u32, mut s: TrackSession, accept: bool) -> ApiResult {
        if s.hidden == 0 && let Some(old) = s.shown.take() {
            self.invert_outline(s.hps, old, s.border);
        }
        self.finish_track(vcpu, &s);
        let r = if accept { s.current } else { s.tracker.original() };
        let at = s.call.1.wrapping_add(TI_RCL_TRACK);
        for (i, v) in [r.left, r.bottom, r.right, r.top].into_iter().enumerate() {
            let _ = self.guest_write::<i32>(at.wrapping_add(4 * i as u32), v);
        }
        debug!("  [VCPU {}] WinTrackRect ends: {} {:?}", vcpu, if accept { "accepted" } else { "cancelled" }, r);
        ApiResult::Normal(accept as u32)
    }

    /// Forget thread `vcpu`'s tracking: the capture goes back and a PS
    /// Warpine made is freed (also when the thread ends while tracking).
    pub(crate) fn finish_track(&self, vcpu: u32, s: &TrackSession) {
        let restore = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            wm.tracks.remove(&vcpu);
            if s.own_ps { wm.remove_ps(s.hps); }
            wm.capture_hwnd == s.capture
        };
        if restore {
            let back = if self.shared.window_mgr.lock_or_recover().is_window(s.prev_capture) { s.prev_capture } else { 0 };
            self.set_track_capture(back);
        }
    }

    /// WinShowTrackRect(HWND hwnd, BOOL fShow) -> BOOL: hide (counted) or
    /// show again the outline of the tracking in `hwnd`; FALSE when none
    /// runs there.
    pub(super) fn win_show_track_rect(&self, vcpu: u32, hwnd: u32, show: bool) -> ApiResult {
        let found = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.tracks.iter().find(|(_, s)| s.hwnd == hwnd).map(|(&v, &s)| (v, s))
        };
        let Some((owner, mut s)) = found else { return self.pm_fail(vcpu, PMERR_INVALID_HWND) };
        if show {
            if s.hidden > 0 {
                s.hidden -= 1;
                if s.hidden == 0 {
                    self.invert_outline(s.hps, s.current, s.border);
                    s.shown = Some(s.current);
                }
            }
        } else {
            if s.hidden == 0 && let Some(old) = s.shown.take() {
                self.invert_outline(s.hps, old, s.border);
            }
            s.hidden = s.hidden.saturating_add(1);
        }
        self.shared.window_mgr.lock_or_recover().tracks.insert(owner, s);
        ApiResult::Normal(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_types::OS2Message;
    use super::super::Loader;

    const PTI: u32 = 0x3000;

    /// The value a call returned (it never chains in these tests).
    fn val(r: ApiResult) -> u32 {
        match r { ApiResult::Normal(v) => v, other => panic!("{other:?}") }
    }
    const RET: u32 = 0x1_2345;

    /// A loader with a window on thread 0's queue, `msgs` queued for it,
    /// and a TRACKINFO at `PTI` (rectangle 10,10 – 60,40, keyboard step 5).
    fn setup(fs: u32, msgs: &[(u32, u32, u32)]) -> (Loader, u32) {
        let loader = Loader::new_mock();
        let hwnd = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            wm.register_class("Trk".into(), 0x1234, 0);
            let hwnd = wm.create_window("Trk".into(), 0, hmq);
            let mq = wm.get_mq(hmq).unwrap();
            for &(msg, mp1, mp2) in msgs {
                mq.lock_or_recover().messages.push_back(OS2Message { hwnd, msg, mp1, mp2, time: 0, x: 0, y: 0 });
            }
            hwnd
        };
        let words: [i32; 19] = [1, 1, 0, 0, 5, 5, 10, 10, 60, 40, 0, 0, 0, 0, 0, 0, 0, 0, fs as i32];
        for (i, w) in words.iter().enumerate() { loader.guest_write::<i32>(PTI + 4 * i as u32, *w).unwrap(); }
        (loader, hwnd)
    }

    fn rect(l: &Loader) -> [i32; 4] {
        [0, 1, 2, 3].map(|i| l.guest_read::<i32>(PTI + TI_RCL_TRACK + 4 * i).unwrap())
    }

    fn mouse(x: i32, y: i32) -> (u32, u32, u32) { (WM_MOUSEMOVE, (x as u32 & 0xFFFF) | ((y as u32) << 16), 0) }
    fn key(vk: u32) -> (u32, u32, u32) { (WM_CHAR, KC_VIRTUALKEY, vk << WM_CHAR_VKEY_SHIFT) }

    /// The pointer moves the rectangle; the release accepts it; the
    /// capture goes back; the session is gone.
    #[test]
    fn the_pointer_moves_and_a_release_accepts() {
        let (l, hwnd) = setup(TF_MOVE, &[mouse(7, 3), mouse(20, 5), (WM_BUTTON1UP, 0, 0)]);
        l.shared.window_mgr.lock_or_recover().capture_hwnd = 0;
        assert_eq!(val(l.win_track_rect(0, 890, RET, hwnd, 0, PTI)), 1);
        assert_eq!(rect(&l), [30, 15, 80, 45], "moved by the last pointer offset");
        let wm = l.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.capture_hwnd, 0, "the capture went back");
        assert!(wm.tracks.is_empty());
    }

    /// Arrows step one edge; Enter accepts; other messages stay queued.
    #[test]
    fn keys_step_the_edges_and_enter_accepts() {
        let (l, hwnd) = setup(TF_RIGHT | TF_TOP, &[(WM_TIMER, 1, 0), key(VK_RIGHT), key(VK_RIGHT), key(VK_UP), key(VK_NEWLINE)]);
        assert_eq!(val(l.win_track_rect(0, 890, RET, hwnd, 0, PTI)), 1);
        assert_eq!(rect(&l), [10, 10, 70, 45]);
        let wm = l.shared.window_mgr.lock_or_recover();
        let hmq = wm.tid_to_hmq[&0];
        let left: Vec<u32> = wm.get_mq(hmq).unwrap().lock_or_recover().messages.iter().map(|m| m.msg).collect();
        assert_eq!(left, vec![WM_TIMER], "only the tracking's input was taken");
    }

    /// Esc cancels: FALSE and the starting rectangle; a key release and a
    /// non-virtual key do nothing.
    #[test]
    fn esc_cancels_and_keeps_the_start() {
        let up = (WM_CHAR, KC_VIRTUALKEY | KC_KEYUP, VK_RIGHT << WM_CHAR_VKEY_SHIFT);
        let (l, hwnd) = setup(TF_MOVE, &[key(VK_RIGHT), up, (WM_CHAR, 0, 0), key(VK_ESC)]);
        assert_eq!(val(l.win_track_rect(0, 890, RET, hwnd, 0, PTI)), 0);
        assert_eq!(rect(&l), [10, 10, 60, 40]);
    }

    /// WM_QUIT cancels and stays for the program's own loop.
    #[test]
    fn wm_quit_cancels_and_stays_queued() {
        let (l, hwnd) = setup(TF_MOVE, &[(WM_QUIT, 0, 0)]);
        assert_eq!(val(l.win_track_rect(0, 890, RET, hwnd, 0, PTI)), 0);
        let wm = l.shared.window_mgr.lock_or_recover();
        let hmq = wm.tid_to_hmq[&0];
        assert!(wm.get_mq(hmq).unwrap().lock_or_recover().messages.iter().any(|m| m.msg == WM_QUIT));
    }

    /// A second tracking on the thread fails while one runs; a call made
    /// again (the same return address and TRACKINFO) resumes it.
    #[test]
    fn a_second_tracking_fails_and_the_same_call_resumes() {
        let (l, hwnd) = setup(TF_MOVE, &[]);
        let mut s = TrackSession {
            hwnd, call: (RET, PTI), tracker: Tracker::new(TrackInfo::default(), (0, 0)), current: Rectl::default(),
            hps: 0, own_ps: false, border: (1, 1), shown: None, hidden: 0, capture: hwnd, prev_capture: 0,
            end_on: 0, resuming: false,
        };
        l.shared.window_mgr.lock_or_recover().tracks.insert(0, s);
        assert_eq!(val(l.win_track_rect(0, 890, RET + 4, hwnd, 0, PTI)), 0, "another call: fails");
        assert_eq!(val(l.win_track_rect(0, 890, RET, hwnd, 0, PTI)), 0, "not resuming: fails");
        s.resuming = true;
        l.shared.window_mgr.lock_or_recover().tracks.insert(0, s);
        // Resumed: the loop finds Esc and ends.
        {
            let wm = l.shared.window_mgr.lock_or_recover();
            let hmq = wm.tid_to_hmq[&0];
            let (m, mp1, mp2) = key(VK_ESC);
            wm.get_mq(hmq).unwrap().lock_or_recover().messages.push_back(OS2Message { hwnd, msg: m, mp1, mp2, time: 0, x: 0, y: 0 });
        }
        assert_eq!(val(l.win_track_rect(0, 890, RET, hwnd, 0, PTI)), 0);
        assert!(l.shared.window_mgr.lock_or_recover().tracks.is_empty(), "ended");
    }

    /// WinShowTrackRect counts hides; FALSE without a tracking there.
    #[test]
    fn show_track_rect_counts_hides() {
        let (l, hwnd) = setup(TF_MOVE, &[]);
        assert_eq!(val(l.win_show_track_rect(0, hwnd, false)), 0, "no tracking");
        let s = TrackSession {
            hwnd, call: (RET, PTI), tracker: Tracker::new(TrackInfo::default(), (0, 0)), current: Rectl::default(),
            hps: 0, own_ps: false, border: (1, 1), shown: Some(Rectl::default()), hidden: 0, capture: hwnd,
            prev_capture: 0, end_on: 0, resuming: false,
        };
        l.shared.window_mgr.lock_or_recover().tracks.insert(0, s);
        let hidden = |l: &Loader| { let wm = l.shared.window_mgr.lock_or_recover(); (wm.tracks[&0].hidden, wm.tracks[&0].shown.is_some()) };
        l.win_show_track_rect(0, hwnd, false);
        l.win_show_track_rect(0, hwnd, false);
        assert_eq!(hidden(&l), (2, false));
        l.win_show_track_rect(0, hwnd, true);
        assert_eq!(hidden(&l), (1, false), "still hidden once");
        l.win_show_track_rect(0, hwnd, true);
        assert_eq!(hidden(&l), (0, true), "shown again");
        assert_eq!(val(l.win_show_track_rect(0, hwnd, true)), 1, "an extra show does nothing");
        assert_eq!(hidden(&l), (0, true));
    }

    /// A bad TRACKINFO pointer fails; no session is left.
    #[test]
    fn an_unreadable_trackinfo_fails() {
        let (l, hwnd) = setup(TF_MOVE, &[]);
        assert_eq!(val(l.win_track_rect(0, 890, RET, hwnd, 0, 0xFFFF_FFF0)), 0);
        assert!(l.shared.window_mgr.lock_or_recover().tracks.is_empty());
    }

    #[test]
    fn the_outline_is_four_strips_or_the_whole_rectangle() {
        let r = Rectl { left: 10, bottom: 20, right: 110, top: 70 };
        let s = outline_strips(r, (2, 3));
        assert_eq!(s, vec![(10, 67, 109, 69), (10, 20, 109, 22), (10, 23, 11, 66), (108, 23, 109, 66)]);
        // Every pel of the border once: the strips do not overlap.
        let area: i32 = s.iter().map(|&(x1, y1, x2, y2)| (x2 - x1 + 1) * (y2 - y1 + 1)).sum();
        assert_eq!(area, 100 * 50 - 96 * 44);
        assert_eq!(outline_strips(Rectl { left: 0, bottom: 0, right: 4, top: 50 }, (2, 2)), vec![(0, 0, 3, 49)], "too narrow");
        assert_eq!(outline_strips(Rectl { left: 5, bottom: 5, right: 5, top: 9 }, (1, 1)), vec![], "empty");
        assert_eq!(outline_strips(r, (0, 0)).len(), 4, "a border of 0 is 1 pel");
    }
}
