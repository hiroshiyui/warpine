// SPDX-License-Identifier: GPL-3.0-only

//! Window destruction with `WM_DESTROY` (`WinDestroyWindow`, the end of
//! `WinDlgBox`).
//!
//! Following the `WinDestroyWindow` reference, the window is hidden, then
//! `WM_DESTROY` is sent first to the window being destroyed and then to its
//! child windows — each window before its own children — while all of them
//! still exist. Only after the last `WM_DESTROY` has returned are the
//! windows removed ([`WindowManager::destroy_window`]).
//!
//! A frame's `WM_DESTROY` processing (Warpine's frame default) also destroys
//! the windows it owns and the windows owned by its descendants (a dialog
//! owned by the client, an owned frame): they get their `WM_DESTROY` right
//! after the frame's and before its children's (that order is Warpine's
//! choice). If the focus is in the frame, it moves to the frame's parent
//! (none for a frame on the desktop: the next active frame's client).
//!
//! The messages are sent synchronously, like `WinSendMsg`: a window with a
//! guest window procedure is called through a `FrameKind::DestroyWindows`
//! callback frame (`vcpu.rs`), whose return continues the walk
//! ([`Loader::advance_destroy`]); built-in controls and windows without a
//! procedure get Warpine's default handling, which does nothing more.
//!
//! The walk is lazy: a window's children and owned windows are collected
//! after its own `WM_DESTROY` has returned, so a child that the handler
//! destroys itself is simply gone, and every window is visited once per
//! destruction (`DestroyChain::visited`) — owner cycles cannot loop.
//! Re-entrancy: a `WM_DESTROY` handler may call `WinDestroyWindow` on any
//! window. For a window already sent `WM_DESTROY` (itself, its parent, the
//! window being destroyed) the call returns TRUE and does nothing: that
//! window goes when the destruction in progress ends. Destroying another
//! window starts a nested destruction, which passes over windows that had
//! `WM_DESTROY` already (walking on to their children), so no window gets
//! the message twice.

use std::collections::HashSet;

use super::pm_types::WindowManager;
use super::{ApiResult, Loader, MutexExt};

/// The state of one destruction: which windows still have to be sent
/// `WM_DESTROY`, and which windows are removed at the end.
#[derive(Debug)]
pub(crate) struct DestroyChain {
    /// What the destroying API returns when the chain ends (TRUE for
    /// `WinDestroyWindow`, the dialog result for `WinDlgBox`).
    pub result: u32,
    /// Windows waiting for `WM_DESTROY`; the last one is next.
    pending: Vec<u32>,
    /// The window whose `WM_DESTROY` is being processed: its children and
    /// owned windows are collected when it returns.
    current: u32,
    /// Windows this chain has reached.
    visited: HashSet<u32>,
    /// The roots of the subtrees removed at the end: the window itself and
    /// the windows owned by destroyed frames.
    removals: Vec<u32>,
}

impl DestroyChain {
    pub(crate) fn new(root: u32, result: u32) -> Self {
        DestroyChain {
            result,
            pending: vec![root],
            current: 0,
            visited: HashSet::new(),
            removals: vec![root],
        }
    }
}

/// How a request to destroy a window starts.
#[derive(Debug)]
pub(crate) enum DestroyStart {
    /// No such window (`PMERR_INVALID_HWND`): FALSE.
    Invalid,
    /// The window belongs to another thread's message queue: FALSE.
    WrongThread,
    /// The window has been sent `WM_DESTROY` already and goes when that
    /// destruction ends: TRUE, nothing more is done.
    InProgress,
    /// Run this chain.
    Start(Box<DestroyChain>),
}

impl WindowManager {
    /// Start destroying `hwnd` on behalf of thread `tid` (`None`: an
    /// implicit destruction, which any thread may do), with `result` as the
    /// value returned when it ends.
    pub(crate) fn begin_destroy(&self, hwnd: u32, tid: Option<u32>, result: u32) -> DestroyStart {
        let Some(w) = self.get_window(hwnd) else { return DestroyStart::Invalid };
        if let Some(tid) = tid && w.hmq != 0 && self.thread_queue(tid) != Some(w.hmq) {
            return DestroyStart::WrongThread;
        }
        if self.destroying.contains(&hwnd) {
            return DestroyStart::InProgress;
        }
        DestroyStart::Start(Box::new(DestroyChain::new(hwnd, result)))
    }

    /// The next window of `chain` to be sent `WM_DESTROY`, with the window
    /// procedure to send it to (0: Warpine's default handling), or `None`
    /// when every window has had it. The window is marked as destroying;
    /// the window being destroyed (and each owned window) is hidden first,
    /// and a frame moves the focus out of itself.
    pub(crate) fn destroy_step(&mut self, chain: &mut DestroyChain) -> Option<(u32, u32)> {
        loop {
            let done = std::mem::take(&mut chain.current);
            if done != 0 {
                self.collect_destroy_followers(chain, done);
            }
            let h = chain.pending.pop()?;
            if !chain.visited.insert(h) || !self.is_window(h) {
                continue;
            }
            if self.destroying.contains(&h) {
                // An outer destruction sent it WM_DESTROY already; its
                // children may still be waiting.
                chain.current = h;
                continue;
            }
            self.destroying.insert(h);
            if chain.removals.contains(&h) && let Some(w) = self.get_window_mut(h) {
                w.visible = false;
                self.mark_dirty(h);
            }
            if self.is_frame(h) && self.focus_hwnd != 0 && self.is_same_or_descendant(self.focus_hwnd, h) {
                let parent = self.get_window(h).map_or(0, |w| w.parent);
                self.set_focus(if self.is_window(parent) { parent } else { 0 });
            }
            chain.current = h;
            return Some((h, self.destroy_proc(h)));
        }
    }

    /// Queue what follows `hwnd` after its `WM_DESTROY`: the windows owned
    /// by a frame (and by its descendants), then its children, in that
    /// order.
    fn collect_destroy_followers(&self, chain: &mut DestroyChain, hwnd: u32) {
        let Some(w) = self.get_window(hwnd) else { return };
        let mut next: Vec<u32> = Vec::new();
        if self.is_frame(hwnd) {
            for owned in self.owned_outside(hwnd) {
                if !chain.removals.contains(&owned) {
                    chain.removals.push(owned);
                }
                next.push(owned);
            }
        }
        next.extend(w.children.iter().copied());
        chain.pending.extend(next.into_iter().rev());
    }

    /// The windows owned by `frame` or by one of its descendants that lie
    /// outside its subtree, each counted once (an owned window's own
    /// descendants go with it), in handle order.
    fn owned_outside(&self, frame: u32) -> Vec<u32> {
        let mut handles: Vec<u32> = self.window_handles();
        handles.sort_unstable();
        let candidates: Vec<u32> = handles.into_iter().filter(|&h| {
            let owner = self.get_window(h).map_or(0, |w| w.owner);
            owner != 0 && self.is_window(owner)
                && self.is_same_or_descendant(owner, frame)
                && !self.is_same_or_descendant(h, frame)
        }).collect();
        candidates.iter().copied().filter(|&h| {
            let parent = self.get_window(h).map_or(0, |w| w.parent);
            !candidates.iter().any(|&c| c != h && self.is_same_or_descendant(parent, c))
        }).collect()
    }

    /// The procedure `WM_DESTROY` goes to: the window's own, except for a
    /// standard frame that has not been subclassed — Warpine gives a frame
    /// its client's class procedure, but a frame's messages belong to the
    /// frame control, whose default is Warpine's.
    fn destroy_proc(&self, hwnd: u32) -> u32 {
        let Some(w) = self.get_window(hwnd) else { return 0 };
        if self.frame_to_client.contains_key(&hwnd)
            && self.get_class(&w.class_name).is_some_and(|c| c.pfn_wp == w.pfn_wp) {
            return 0;
        }
        w.pfn_wp
    }

    /// End `chain`: remove the destroyed windows. Returns the frames that
    /// survive but lost their menu bar with them.
    pub(crate) fn destroy_finish(&mut self, chain: &DestroyChain) -> Vec<u32> {
        let removed = |wm: &Self, h: u32| chain.removals.iter().any(|&r| wm.is_same_or_descendant(h, r));
        let bare: Vec<u32> = self.frame_to_client.keys().copied()
            .filter(|&f| !removed(self, f))
            .filter(|&f| self.get_window(f).is_some_and(|w| w.menu_hwnd != 0 && removed(self, w.menu_hwnd)))
            .collect();
        for &r in &chain.removals {
            self.destroy_window(r);
        }
        bare
    }
}

impl Loader {
    /// WinDestroyWindow (PMWIN 728) on thread `tid`.
    pub(crate) fn win_destroy_window(&self, tid: u32, hwnd: u32) -> ApiResult {
        let start = self.shared.window_mgr.lock_or_recover().begin_destroy(hwnd, Some(tid), 1);
        match start {
            DestroyStart::Invalid => self.pm_fail(tid, super::constants::PMERR_INVALID_HWND),
            DestroyStart::WrongThread => ApiResult::Normal(0),
            DestroyStart::InProgress => ApiResult::Normal(1),
            DestroyStart::Start(chain) => self.run_destroy(chain),
        }
    }

    /// Destroy dialog `dialog` at the end of `WinDlgBox`, returning
    /// `result` to its caller when done.
    pub(crate) fn destroy_dialog(&self, dialog: u32, result: u32) -> ApiResult {
        let start = self.shared.window_mgr.lock_or_recover().begin_destroy(dialog, None, result);
        match start {
            DestroyStart::Start(chain) => self.run_destroy(chain),
            _ => ApiResult::Normal(result),
        }
    }

    /// Run `chain` on the host until a guest window procedure has to be
    /// called (`ApiResult::DestroyWindows`) or it has ended
    /// (`ApiResult::Normal` with its result).
    pub(crate) fn run_destroy(&self, mut chain: Box<DestroyChain>) -> ApiResult {
        match self.advance_destroy(&mut chain) {
            Some((hwnd, wnd_proc)) => ApiResult::DestroyWindows { chain, hwnd, wnd_proc },
            None => ApiResult::Normal(chain.result),
        }
    }

    /// Continue `chain` after the last `WM_DESTROY` returned: windows with
    /// Warpine's default handling are passed on the spot; the next one with
    /// a guest procedure is returned. `None` = all were sent and the
    /// windows have been removed.
    pub(crate) fn advance_destroy(&self, chain: &mut DestroyChain) -> Option<(u32, u32)> {
        loop {
            let step = self.shared.window_mgr.lock_or_recover().destroy_step(chain);
            match step {
                Some((hwnd, 0)) => {
                    log::debug!("WM_DESTROY hwnd={hwnd:#x} (default handling)");
                }
                Some((hwnd, wnd_proc)) => {
                    log::debug!("WM_DESTROY hwnd={hwnd:#x} → 0x{wnd_proc:08X}");
                    return Some((hwnd, wnd_proc));
                }
                None => break,
            }
        }
        let bare = self.shared.window_mgr.lock_or_recover().destroy_finish(chain);
        for frame in bare {
            if self.shared.window_mgr.lock_or_recover().is_window(frame) {
                self.attach_menu_to_frame(frame, 0);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::constants::*;
    use crate::loader::pm_types::{DeviceContext, PsOrigin};

    const PROC: u32 = 0x1234;

    /// Run `chain` as the guest would, returning at once from every
    /// `WM_DESTROY`, and calling `during(hwnd)` inside each handler.
    fn drive(loader: &Loader, first: ApiResult, mut during: impl FnMut(u32)) -> (Vec<u32>, u32) {
        let mut sent = Vec::new();
        let mut r = first;
        loop {
            match r {
                ApiResult::Normal(v) => return (sent, v),
                ApiResult::DestroyWindows { mut chain, hwnd, wnd_proc } => {
                    assert_ne!(wnd_proc, 0);
                    sent.push(hwnd);
                    during(hwnd);
                    r = match loader.advance_destroy(&mut chain) {
                        Some((hwnd, wnd_proc)) => ApiResult::DestroyWindows { chain, hwnd, wnd_proc },
                        None => ApiResult::Normal(chain.result),
                    };
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    /// Frame + client (class "App" with a procedure) with two children of
    /// the class, a WC_BUTTON grandchild under the first, all on thread 0.
    fn tree(loader: &Loader) -> (u32, u32, u32, u32, u32) {
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hmq = wm.create_mq(437);
        wm.tid_to_hmq.insert(0, hmq);
        wm.register_class("App".into(), PROC, 0);
        let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        wm.frame_to_client.insert(frame, client);
        wm.get_window_mut(frame).unwrap().visible = true;
        wm.z_push_top(frame);
        wm.focused_hwnd = frame;
        let a = wm.create_window("App".into(), client, hmq);
        let b = wm.create_window("App".into(), client, hmq);
        let button = wm.create_window("WC_BUTTON".into(), a, hmq);
        (frame, client, a, b, button)
    }

    #[test]
    fn wm_destroy_goes_to_the_window_then_its_children_while_they_exist() {
        let loader = Loader::new_mock();
        let (frame, client, a, b, button) = tree(&loader);
        let start = loader.win_destroy_window(0, frame);
        let (sent, result) = drive(&loader, start, |h| {
            let wm = loader.shared.window_mgr.lock_or_recover();
            for w in [frame, client, a, b, button] {
                assert!(wm.is_window(w), "{w:#x} exists during WM_DESTROY of {h:#x}");
            }
            assert!(!wm.get_window(frame).unwrap().visible, "hidden first");
            assert_eq!(wm.query_window(client, QW_PARENT), frame);
        });
        // The frame itself has Warpine's default handling; the button too.
        assert_eq!(sent, vec![client, a, b]);
        assert_eq!(result, 1);
        let wm = loader.shared.window_mgr.lock_or_recover();
        for w in [frame, client, a, b, button] {
            assert!(!wm.is_window(w));
        }
        assert!(wm.destroying.is_empty());
        assert!(!wm.z_order.contains(&frame));
    }

    #[test]
    fn destroying_a_child_unlinks_it_and_unknown_handles_fail() {
        let loader = Loader::new_mock();
        let (_, client, a, b, button) = tree(&loader);
        let (sent, result) = drive(&loader, loader.win_destroy_window(0, a), |_| {});
        assert_eq!((sent, result), (vec![a], 1));
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.is_window(button) && wm.is_window(b));
        assert_eq!(wm.get_window(client).unwrap().children, vec![b]);
        drop(wm);
        assert!(matches!(loader.win_destroy_window(0, a), ApiResult::Normal(0)), "already destroyed");
        assert!(matches!(loader.win_destroy_window(0, HWND_DESKTOP), ApiResult::Normal(0)));
    }

    #[test]
    fn another_threads_window_cannot_be_destroyed() {
        let loader = Loader::new_mock();
        let (frame, ..) = tree(&loader);
        assert!(matches!(loader.win_destroy_window(5, frame), ApiResult::Normal(0)), "thread without a queue");
        let other = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let q = wm.create_mq(437);
            wm.tid_to_hmq.insert(5, q);
            q
        };
        assert!(matches!(loader.win_destroy_window(5, frame), ApiResult::Normal(0)), "queue {other:#x} is not the frame's");
        assert!(loader.shared.window_mgr.lock_or_recover().is_window(frame));
    }

    #[test]
    fn a_frame_destroys_the_windows_it_and_its_children_own() {
        let loader = Loader::new_mock();
        let (frame, client, a, b, _) = tree(&loader);
        let (owned_frame, owned_client, dialog, dlg_child) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.thread_queue(0).unwrap();
            let f = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let c = wm.create_window("App".into(), f, hmq);
            wm.frame_to_client.insert(f, c);
            wm.get_window_mut(f).unwrap().owner = frame;
            wm.z_push_top(f);
            // A dialog owned by the client, with a control it owns too.
            let d = wm.create_window(WindowManager::DIALOG_CLASS.into(), HWND_DESKTOP, hmq);
            wm.get_window_mut(d).unwrap().owner = client;
            wm.get_window_mut(d).unwrap().pfn_wp = PROC;
            let dc = wm.create_window("App".into(), d, hmq);
            wm.get_window_mut(dc).unwrap().owner = d;
            (f, c, d, dc)
        };
        let (sent, _) = drive(&loader, loader.win_destroy_window(0, frame), |h| {
            if h == dialog {
                assert!(!loader.shared.window_mgr.lock_or_recover().get_window(dialog).unwrap().visible);
            }
        });
        // Owned windows (handle order) right after the frame, then children.
        assert_eq!(sent, vec![owned_client, dialog, dlg_child, client, a, b]);
        let wm = loader.shared.window_mgr.lock_or_recover();
        for w in [owned_frame, owned_client, dialog, dlg_child] {
            assert!(!wm.is_window(w));
        }
        assert!(wm.z_order.is_empty());
    }

    #[test]
    fn owner_cycles_end() {
        let loader = Loader::new_mock();
        let (frame, ..) = tree(&loader);
        let other = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.thread_queue(0).unwrap();
            let f = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let c = wm.create_window("App".into(), f, hmq);
            wm.frame_to_client.insert(f, c);
            wm.get_window_mut(f).unwrap().owner = frame;
            wm.get_window_mut(frame).unwrap().owner = f;
            f
        };
        let (sent, result) = drive(&loader, loader.win_destroy_window(0, frame), |_| {});
        assert_eq!(result, 1);
        assert_eq!(sent.len(), sent.iter().collect::<HashSet<_>>().len(), "once each");
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.is_window(frame) && !wm.is_window(other));
    }

    #[test]
    fn destroying_again_from_wm_destroy_is_ignored_and_nested_ones_send_once() {
        let loader = Loader::new_mock();
        let (frame, client, a, b, button) = tree(&loader);
        // The client destroys itself, the frame, and child `b` at once.
        let (sent, result) = drive(&loader, loader.win_destroy_window(0, frame), |h| {
            if h != client { return; }
            for w in [client, frame] {
                assert!(matches!(loader.win_destroy_window(0, w), ApiResult::Normal(1)), "in progress: TRUE");
            }
            let (nested, r) = drive(&loader, loader.win_destroy_window(0, b), |_| {});
            assert_eq!((nested, r), (vec![b], 1));
            assert!(!loader.shared.window_mgr.lock_or_recover().is_window(b), "removed at once");
        });
        assert_eq!((sent, result), (vec![client, a], 1), "b had it from the nested call");
        assert!(!loader.shared.window_mgr.lock_or_recover().is_window(button));

        // A child's handler destroying an ancestor not yet being destroyed:
        // the nested destruction finishes the rest; each window once.
        let (frame, client, a, b, _) = tree(&loader);
        let (sent, result) = drive(&loader, loader.win_destroy_window(0, client), |h| {
            if h == a {
                let (nested, _) = drive(&loader, loader.win_destroy_window(0, frame), |_| {});
                assert_eq!(nested, vec![b], "client and a had it");
            }
        });
        assert_eq!((sent, result), (vec![client, a], 1));
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.is_window(frame) && wm.destroying.is_empty());
    }

    #[test]
    fn the_focus_leaves_a_destroyed_frame_and_its_presentation_spaces_go() {
        let loader = Loader::new_mock();
        let (frame, client, a, _, button) = tree(&loader);
        let (cached, created, hdc) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.set_focus(button);
            let cached = wm.create_ps(client);
            wm.ps_map.get_mut(&cached).unwrap().origin = PsOrigin::Cached;
            let hdc = wm.open_dc(DeviceContext::Window(a));
            let created = wm.create_ps(a);
            wm.ps_map.get_mut(&created).unwrap().hdc = hdc;
            (cached, created, hdc)
        };
        let (_, _) = drive(&loader, loader.win_destroy_window(0, frame), |_| {
            assert_eq!(loader.shared.window_mgr.lock_or_recover().focus_hwnd, 0,
                "the frame's parent is the desktop: no focus window");
        });
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.ps_map.contains_key(&cached), "WinGetPS space released");
        let ps = wm.ps_map.get(&created).expect("GpiCreatePS space stays");
        assert_eq!((ps.hwnd, ps.hdc), (0, 0), "disassociated");
        assert!(!wm.dcs.contains_key(&hdc));
    }

    #[test]
    fn a_dialog_ended_by_win_dlg_box_gets_wm_destroy() {
        let loader = Loader::new_mock();
        let (_, client, ..) = tree(&loader);
        let (dialog, ok) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.thread_queue(0).unwrap();
            let d = wm.create_window(WindowManager::DIALOG_CLASS.into(), HWND_DESKTOP, hmq);
            let w = wm.get_window_mut(d).unwrap();
            (w.owner, w.pfn_wp) = (client, PROC);
            let ok = wm.create_window("WC_BUTTON".into(), d, hmq);
            wm.begin_modal_dialog(d, true);
            let w = wm.get_window_mut(d).unwrap();
            (w.dialog_dismissed, w.dialog_result) = (true, 7);
            (d, ok)
        };
        let (result, destroy) = loader.shared.window_mgr.lock_or_recover().end_modal_dialog(dialog);
        assert_eq!((result, destroy), (7, true));
        let (sent, result) = drive(&loader, loader.destroy_dialog(dialog, result), |_| {});
        assert_eq!((sent, result), (vec![dialog], 7));
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.is_window(dialog) && !wm.is_window(ok));
        drop(wm);
        assert!(matches!(loader.destroy_dialog(dialog, 3), ApiResult::Normal(3)), "gone already");
    }

    #[test]
    fn a_subclassed_frame_gets_wm_destroy() {
        let loader = Loader::new_mock();
        let (frame, client, a, b, _) = tree(&loader);
        loader.shared.window_mgr.lock_or_recover().get_window_mut(frame).unwrap().pfn_wp = 0x5678;
        let (sent, _) = drive(&loader, loader.win_destroy_window(0, frame), |_| {});
        assert_eq!(sent, vec![frame, client, a, b]);
    }
}
