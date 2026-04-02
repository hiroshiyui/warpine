// SPDX-License-Identifier: GPL-3.0-only

use std::sync::Arc;
use crate::loader::SharedState;
use super::message::GUIMessage;
use super::renderer::PmRenderer;

// ── Headless backend ───────────────────────────────────────────────────────

/// No-op renderer for CI and headless automated testing.
///
/// All `handle_message` calls are silently discarded.
/// `poll_events` always returns `keep_running`.
/// `frame_sleep` is a no-op to keep tests fast.
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
    fn handle_message(&mut self, msg: GUIMessage, _shared: &Arc<SharedState>) {
        self.message_count += 1;
        // In headless mode modal dialogs are auto-dismissed with MBID_OK.
        if let GUIMessage::ShowMessageBox { reply_tx, .. } = msg {
            let _ = reply_tx.send(1); // MBID_OK
        }
    }

    fn poll_events(&mut self, _shared: &Arc<SharedState>) -> bool {
        self.keep_running
    }

    fn frame_sleep(&self) {
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

        tx.send(GUIMessage::ClearBuffer { handle: 1 }).unwrap();
        tx.send(GUIMessage::PresentBuffer { handle: 1 }).unwrap();
        // Signal exit so the loop terminates after draining.
        drop(tx);
        renderer.keep_running = false;

        run_pm_loop(&mut renderer, shared, rx);
        assert_eq!(renderer.message_count, 2);
    }

    #[test]
    fn headless_renderer_exits_on_keep_running_false() {
        let shared = make_shared();
        let (_tx, rx) = create_gui_channel();
        let mut renderer = HeadlessRenderer::new();
        renderer.keep_running = false;

        // Must return immediately without hanging.
        run_pm_loop(&mut renderer, shared, rx);
        // Reaching here means the loop exited correctly.
    }

    #[test]
    fn headless_renderer_exits_on_exit_requested() {
        use std::sync::atomic::Ordering;
        let shared = make_shared();
        shared.exit_requested.store(true, Ordering::Relaxed);
        let (_tx, rx) = create_gui_channel();
        let mut renderer = HeadlessRenderer::new();

        run_pm_loop(&mut renderer, shared, rx);
    }

    #[test]
    fn headless_frame_sleep_is_noop() {
        let renderer = HeadlessRenderer::new();
        let start = std::time::Instant::now();
        renderer.frame_sleep();
        // Should complete in well under 1 ms.
        assert!(start.elapsed().as_millis() < 5);
    }

    #[test]
    fn headless_renderer_discards_all_message_variants() {
        let shared = make_shared();
        let (tx, rx) = create_gui_channel();
        let mut renderer = HeadlessRenderer::new();

        tx.send(GUIMessage::CreateWindow {
            class: "WC_FRAME".into(), title: "Test".into(), handle: 42,
        }).unwrap();
        tx.send(GUIMessage::DrawBox {
            handle: 42, x1: 0, y1: 0, x2: 10, y2: 10, color: 0xFF0000, fill: true,
        }).unwrap();
        tx.send(GUIMessage::DrawLine {
            handle: 42, x1: 0, y1: 0, x2: 5, y2: 5, color: 0x00FF00,
        }).unwrap();
        tx.send(GUIMessage::DrawText {
            handle: 42, x: 0, y: 0, text: "hi".into(), color: 0x0000FF,
        }).unwrap();
        drop(tx);
        renderer.keep_running = false;

        run_pm_loop(&mut renderer, shared, rx);
        assert_eq!(renderer.message_count, 4);
    }

    #[test]
    fn headless_renderer_default_matches_new() {
        let r = HeadlessRenderer::default();
        assert_eq!(r.message_count, 0);
        assert!(r.keep_running);
    }

    #[test]
    fn headless_show_message_box_replies_mbid_ok() {
        // The headless renderer must immediately reply MBID_OK (=1) so that
        // WinMessageBox doesn't hang in tests.
        let mut renderer = HeadlessRenderer::new();
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel::<u32>(1);
        renderer.handle_message(GUIMessage::ShowMessageBox {
            caption: "Title".into(),
            text:    "Body".into(),
            style:   0, // MB_OK
            reply_tx,
        }, &make_shared());
        let result = reply_rx.try_recv().expect("reply_tx should have been sent");
        assert_eq!(result, 1); // MBID_OK
    }
}
