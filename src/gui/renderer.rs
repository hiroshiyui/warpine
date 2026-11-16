// SPDX-License-Identifier: GPL-3.0-only

use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::loader::SharedState;
use super::message::{GUIMessage, GUIReceiver, GuiWake};

// ── PmRenderer trait ───────────────────────────────────────────────────────

/// Fallback wait for a backend that cannot be woken: the loop then polls on
/// this period.
pub const POLL_PERIOD: std::time::Duration = std::time::Duration::from_millis(8);

/// Backend abstraction for the Presentation Manager GUI loop.
///
/// All methods run on the **main thread** only.
/// Implementors are not required to be `Send`.
pub trait PmRenderer {
    /// Dispatch a single queued `GUIMessage` to the backend.
    ///
    /// Called once per message drained from the channel.
    /// `shared` is provided so compositor-style renderers can read window positions
    /// from `SharedState::window_mgr` during `PresentBuffer`.
    fn handle_message(&mut self, msg: GUIMessage, shared: &Arc<SharedState>);

    /// Poll the underlying event source (SDL events, synthetic events, etc.)
    /// and post OS/2 messages to `shared` message queues.
    ///
    /// Returns `false` to signal the loop should exit (e.g. window closed).
    fn poll_events(&mut self, shared: &Arc<SharedState>) -> bool;

    /// Called once before the loop starts with the channel's wake-up, so the
    /// backend can install a hook that interrupts [`wait_for_work`](Self::wait_for_work)
    /// when another thread sends a message or marks a window dirty.
    /// Default: no hook (the backend polls).
    fn install_waker(&mut self, _wake: Arc<GuiWake>) {}

    /// Block until there may be work: input, a wake-up, or a deadline the
    /// backend knows about (next frame, next scripted step).
    ///
    /// Default: sleep [`POLL_PERIOD`], for backends without a wake-up hook.
    fn wait_for_work(&mut self, _shared: &Arc<SharedState>) {
        std::thread::sleep(POLL_PERIOD);
    }
}

// ── Main event loop ────────────────────────────────────────────────────────

/// Run the PM GUI event loop using `renderer` as the backend.
///
/// Must be called from the **main thread** when using `SdlRenderer`
/// (SDL event pump requirement).  Returns when `shared.exit_requested`
/// is set or `renderer.poll_events` returns `false`.
pub fn run_pm_loop(
    renderer: &mut dyn PmRenderer,
    shared: Arc<SharedState>,
    rx: GUIReceiver,
) {
    let wake = rx.wake_handle();
    renderer.install_waker(wake.clone());
    loop {
        // Rearm before draining: a message sent from here on wakes the next
        // wait, one sent before is drained now.
        wake.rearm();
        while let Ok(msg) = rx.try_recv() {
            renderer.handle_message(msg, &shared);
        }

        // Poll backend events; false means exit.
        if !renderer.poll_events(&shared) {
            return;
        }

        if shared.exit_requested.load(Ordering::Relaxed) {
            return;
        }

        renderer.wait_for_work(&shared);
    }
}
