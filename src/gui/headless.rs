// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;
use crate::loader::SharedState;
use super::message::GUIMessage;
use super::renderer::PmRenderer;

// ── Headless backend ───────────────────────────────────────────────────────

/// No-op renderer for CI and headless automated testing.
///
/// All `handle_message` calls are silently discarded.
/// `poll_events` always returns `keep_running`.
/// `wait_for_work` is a no-op to keep tests fast.
pub struct HeadlessRenderer {
    /// Total number of messages dispatched so far (for test assertions).
    pub message_count: u32,
    /// Controls whether `poll_events` returns `true` (continue) or `false` (stop).
    /// Tests set this to `false` after sending their desired messages.
    pub keep_running: bool,
}

impl HeadlessRenderer {
    pub fn new() -> Self {
        HeadlessRenderer { message_count: 0, keep_running: true }
    }
}

impl Default for HeadlessRenderer {
    fn default() -> Self { Self::new() }
}

impl PmRenderer for HeadlessRenderer {
    fn handle_message(&mut self, _msg: GUIMessage, _shared: &Arc<SharedState>) {
        self.message_count += 1;
    }

    fn poll_events(&mut self, _shared: &Arc<SharedState>) -> bool {
        self.keep_running
    }

    fn wait_for_work(&mut self, _shared: &Arc<SharedState>) {
        // No-op: no sleep needed in headless mode.
    }
}

#[cfg(test)]
mod tests {
    use super::HeadlessRenderer;
    use super::super::{GUIMessage, create_gui_channel, run_pm_loop, PmRenderer};
    use crate::loader::Loader;
    use std::sync::Arc;

    fn make_shared() -> Arc<crate::loader::SharedState> {
        Loader::new_mock().shared
    }

    #[test]
    fn headless_renderer_counts_messages() {
        let shared = make_shared();
        let (tx, rx) = create_gui_channel();
        let mut renderer = HeadlessRenderer::new();

        tx.send(GUIMessage::SetClipboardText("x".into())).unwrap();
        tx.send(GUIMessage::SetMouseCapture(0)).unwrap();
        // Signal exit so the loop terminates after draining.
        drop(tx);
        renderer.keep_running = false;

        run_pm_loop(&mut renderer, shared, &rx);
        assert_eq!(renderer.message_count, 2);
    }

    #[test]
    fn headless_renderer_exits_on_keep_running_false() {
        let shared = make_shared();
        let (_tx, rx) = create_gui_channel();
        let mut renderer = HeadlessRenderer::new();
        renderer.keep_running = false;

        // Must return immediately without hanging.
        run_pm_loop(&mut renderer, shared, &rx);
        // Reaching here means the loop exited correctly.
    }

    #[test]
    fn headless_renderer_exits_on_exit_requested() {
        use std::sync::atomic::Ordering;
        let shared = make_shared();
        shared.exit_requested.store(true, Ordering::Relaxed);
        let (_tx, rx) = create_gui_channel();
        let mut renderer = HeadlessRenderer::new();

        run_pm_loop(&mut renderer, shared, &rx);
    }

    /// A VIO session runs the loop again with the same channel after a
    /// stretch full screen: messages sent meanwhile are handled then.
    #[test]
    fn the_loop_can_run_again_on_the_same_channel() {
        let shared = make_shared();
        let (tx, rx) = create_gui_channel();
        let mut renderer = HeadlessRenderer::new();
        renderer.keep_running = false;
        tx.send(GUIMessage::SetMouseCapture(0)).unwrap();
        run_pm_loop(&mut renderer, shared.clone(), &rx);
        tx.send(GUIMessage::SetMouseCapture(1)).unwrap();
        let mut again = HeadlessRenderer::new();
        again.keep_running = false;
        run_pm_loop(&mut again, shared, &rx);
        assert_eq!((renderer.message_count, again.message_count), (1, 1));
    }

    #[test]
    fn headless_wait_for_work_is_noop() {
        let mut renderer = HeadlessRenderer::new();
        let shared = make_shared();
        let start = std::time::Instant::now();
        renderer.wait_for_work(&shared);
        // Should complete in well under 1 ms.
        assert!(start.elapsed().as_millis() < 5);
    }

    #[test]
    fn headless_renderer_discards_all_message_variants() {
        let shared = make_shared();
        let (tx, rx) = create_gui_channel();
        let mut renderer = HeadlessRenderer::new();

        tx.send(GUIMessage::SetClipboardText("hi".into())).unwrap();
        tx.send(GUIMessage::SetMouseCapture(42)).unwrap();
        tx.send(GUIMessage::SetMouseCapture(0)).unwrap();
        drop(tx);
        renderer.keep_running = false;

        run_pm_loop(&mut renderer, shared, &rx);
        assert_eq!(renderer.message_count, 3);
    }

    #[test]
    fn headless_renderer_default_matches_new() {
        let r = HeadlessRenderer::default();
        assert_eq!(r.message_count, 0);
        assert!(r.keep_running);
    }
}
