// SPDX-License-Identifier: GPL-3.0-only

use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::loader::SharedState;
use super::message::GUIMessage;

// ── PmRenderer trait ───────────────────────────────────────────────────────

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

    /// Poll the underlying event source (SDL2 events, synthetic events, etc.)
    /// and post OS/2 messages to `shared` message queues.
    ///
    /// Returns `false` to signal the loop should exit (e.g. window closed).
    fn poll_events(&mut self, shared: &Arc<SharedState>) -> bool;

    /// Yield the calling thread for approximately one frame period.
    ///
    /// Default: 8 ms.  `HeadlessRenderer` overrides with a no-op for speed.
    fn frame_sleep(&self) {
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

// ── Main event loop ────────────────────────────────────────────────────────

/// Run the PM GUI event loop using `renderer` as the backend.
///
/// Must be called from the **main thread** when using `Sdl2Renderer`
/// (SDL2 event pump requirement).  Returns when `shared.exit_requested`
/// is set or `renderer.poll_events` returns `false`.
pub fn run_pm_loop(
    renderer: &mut dyn PmRenderer,
    shared: Arc<SharedState>,
    rx: std::sync::mpsc::Receiver<GUIMessage>,
) {
    loop {
        // Drain all pending GUI messages from the VCPU thread.
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

        renderer.frame_sleep();
    }
}
