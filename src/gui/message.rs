// SPDX-License-Identifier: GPL-3.0-only

// ── GUI message channel ────────────────────────────────────────────────────

use std::sync::mpsc::{Receiver, Sender, SendError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Requests that only the GUI thread can carry out.
///
/// Drawing does not go through this channel: API threads draw straight into
/// the frame's shared surface (`gui::surface`) and mark the frame dirty.
pub enum GUIMessage {
    /// Copy `text` to the host system clipboard.
    SetClipboardText(String),
    /// Ask the SDL backend to capture the mouse (`MouseUtil::capture`).
    /// `hwnd` is the capturing window (0 = release capture).
    SetMouseCapture(u32),
    /// Show this pointer as the host mouse cursor (`None` hides it).
    SetPointer(Option<super::pointer::PointerShape>),
    /// Open the system menu of `frame` (`WM_SYSCOMMAND(SC_SYSMENU)`), with
    /// its first item highlighted when opened from the `keyboard`.
    SystemMenu { frame: u32, keyboard: bool },
    /// Move (`size` false) or size `frame` with the keyboard
    /// (`WM_SYSCOMMAND(SC_MOVE / SC_SIZE)`).
    TrackFrame { frame: u32, size: bool },
    /// Show a pop-up menu (`WinPopupMenu`).
    PopupMenu(PopupRequest),
}

/// A pop-up menu to show (`WinPopupMenu`).
#[derive(Clone, Debug, PartialEq)]
pub struct PopupRequest {
    /// The window its notifications (`WM_COMMAND` …) go to.
    pub owner: u32,
    /// The menu's top-level items.
    pub items: Vec<crate::loader::pm_types::MenuItem>,
    /// Where it opens: desktop point, bottom-left origin (PM coordinates).
    pub x: i32,
    pub y: i32,
    /// `PU_POSITIONONITEM`: the item `item` lies under `(x, y)`.
    pub position_on_item: bool,
    /// The item to highlight (`PU_POSITIONONITEM` / `PU_SELECTITEM`).
    pub item: Option<u32>,
}

/// Hook a renderer installs so other threads can interrupt its wait.
pub type WakeHook = Box<dyn Fn() + Send + Sync>;

/// Wakes the GUI thread when another thread has work for it.
///
/// The GUI loop blocks between frames (`PmRenderer::wait_for_work`) instead
/// of sleeping on a fixed period, so anything that needs the GUI thread — a
/// queued [`GUIMessage`], a window marked dirty — must wake it. A renderer
/// can block in two ways:
///
/// * in its own event source, with a hook that interrupts that wait (the
///   SDL backend pushes an SDL user event, so input and wake-ups end the
///   same wait);
/// * in [`wait`](Self::wait), when the event source cannot block (SDL's
///   `dummy` video driver has no input to wait for).
///
/// Wake-ups are coalesced: after the first [`wake`](Self::wake) the hook is
/// not called again until the GUI thread [`rearm`](Self::rearm)s, which it
/// does just before draining its work. A burst of drawing messages therefore
/// costs one hook call, not one per message.
#[derive(Default)]
pub struct GuiWake {
    /// A wake-up arrived since the last `rearm`.
    pending: Mutex<bool>,
    cond: Condvar,
    hook: OnceLock<WakeHook>,
}

impl GuiWake {
    fn pending(&self) -> std::sync::MutexGuard<'_, bool> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Install the hook. Only the first call has an effect; returns whether
    /// this call installed it.
    pub fn set_hook(&self, hook: WakeHook) -> bool {
        self.hook.set(hook).is_ok()
    }

    /// Ask the GUI thread to run. Cheap when a wake-up is already pending.
    pub fn wake(&self) {
        let first = {
            let mut pending = self.pending();
            !std::mem::replace(&mut *pending, true)
        };
        if first {
            self.cond.notify_all();
            if let Some(hook) = self.hook.get() {
                hook();
            }
        }
    }

    /// Called by the GUI thread before it drains its work: later
    /// [`wake`](Self::wake) calls reach the hook again.
    pub fn rearm(&self) {
        *self.pending() = false;
    }

    /// Block until a wake-up arrives (or has arrived since the last
    /// [`rearm`](Self::rearm)) or `timeout` passes.
    pub fn wait(&self, timeout: Duration) {
        let guard = self.pending();
        let _ = self.cond.wait_timeout_while(guard, timeout, |pending| !*pending);
    }
}

/// Sender half of the GUI channel — cheaply cloneable and `Send`.
#[derive(Clone)]
pub struct GUISender {
    tx: Sender<GUIMessage>,
    wake: Arc<GuiWake>,
}

impl GUISender {
    /// Queue `msg` for the GUI thread and wake it.
    pub fn send(&self, msg: GUIMessage) -> Result<(), SendError<GUIMessage>> {
        self.tx.send(msg)?;
        self.wake.wake();
        Ok(())
    }

    /// Wake the GUI thread without sending a message, e.g. after changing
    /// shared window state that the compositor reads.
    pub fn wake(&self) {
        self.wake.wake();
    }
}

/// Receiver half of the GUI channel, owned by the GUI loop.
pub struct GUIReceiver {
    rx: Receiver<GUIMessage>,
    wake: Arc<GuiWake>,
}

impl GUIReceiver {
    pub fn try_recv(&self) -> Result<GUIMessage, TryRecvError> {
        self.rx.try_recv()
    }

    /// Every message queued so far, without blocking.
    pub fn try_iter(&self) -> std::sync::mpsc::TryIter<'_, GUIMessage> {
        self.rx.try_iter()
    }

    /// The wake-up shared with every [`GUISender`] of this channel.
    pub fn wake_handle(&self) -> Arc<GuiWake> {
        self.wake.clone()
    }
}

/// Create a GUI message channel.  The receiver is handed to `run_pm_loop`.
pub fn create_gui_channel() -> (GUISender, GUIReceiver) {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = Arc::new(GuiWake::default());
    (GUISender { tx, wake: wake.clone() }, GUIReceiver { rx, wake })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    #[test]
    fn wait_times_out_without_a_wake_up() {
        let wake = GuiWake::default();
        let start = Instant::now();
        wake.wait(Duration::from_millis(30));
        assert!(start.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn wait_returns_at_once_after_an_earlier_wake_up() {
        let wake = GuiWake::default();
        wake.wake();
        let start = Instant::now();
        wake.wait(Duration::from_secs(10));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn wake_from_another_thread_ends_the_wait() {
        let wake = Arc::new(GuiWake::default());
        let other = wake.clone();
        let start = Instant::now();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            other.wake();
        });
        wake.wait(Duration::from_secs(10));
        assert!(start.elapsed() < Duration::from_secs(5), "woken, not timed out");
        t.join().unwrap();
    }

    fn counting_hook(wake: &GuiWake) -> Arc<AtomicU32> {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        assert!(wake.set_hook(Box::new(move || { c.fetch_add(1, Ordering::Relaxed); })));
        calls
    }

    #[test]
    fn wake_calls_the_hook_once_until_rearmed() {
        let wake = GuiWake::default();
        let calls = counting_hook(&wake);
        wake.wake();
        wake.wake();
        wake.wake();
        assert_eq!(calls.load(Ordering::Relaxed), 1, "a burst of wake-ups is coalesced");
        wake.rearm();
        wake.wake();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn hook_can_be_installed_only_once() {
        let wake = GuiWake::default();
        let _calls = counting_hook(&wake);
        assert!(!wake.set_hook(Box::new(|| {})));
    }

    #[test]
    fn wake_before_the_hook_is_installed_is_harmless() {
        let wake = GuiWake::default();
        wake.wake();
        let calls = counting_hook(&wake);
        // Still pending from the early wake-up until the GUI thread rearms.
        wake.wake();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        wake.rearm();
        wake.wake();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn sending_a_message_wakes_the_gui_thread() {
        let (tx, rx) = create_gui_channel();
        let calls = counting_hook(&rx.wake_handle());
        tx.send(GUIMessage::SetClipboardText("a".into())).unwrap();
        tx.send(GUIMessage::SetMouseCapture(1)).unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(matches!(rx.try_recv(), Ok(GUIMessage::SetClipboardText(t)) if t == "a"));
        assert!(matches!(rx.try_recv(), Ok(GUIMessage::SetMouseCapture(1))));
        assert!(rx.try_recv().is_err());
    }
}
