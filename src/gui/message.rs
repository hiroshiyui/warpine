// SPDX-License-Identifier: GPL-3.0-only

// ── GUI message channel ────────────────────────────────────────────────────

pub enum GUIMessage {
    CreateWindow { class: String, title: String, handle: u32 },
    ResizeWindow { handle: u32, width: u32, height: u32 },
    MoveWindow { handle: u32, x: i32, y: i32 },
    ShowWindow { handle: u32, show: bool },
    DrawBox { handle: u32, x1: i32, y1: i32, x2: i32, y2: i32, color: u32, fill: bool },
    DrawLine { handle: u32, x1: i32, y1: i32, x2: i32, y2: i32, color: u32 },
    DrawText { handle: u32, x: i32, y: i32, text: String, color: u32 },
    ClearBuffer { handle: u32 },
    PresentBuffer { handle: u32 },
    /// Copy `text` to the host system clipboard.
    SetClipboardText(String),
    /// Instruct the SDL2 backend to call `SDL_CaptureMouse`.
    /// `hwnd` is the capturing window (0 = release capture).
    SetMouseCapture(u32),
}

/// Sender half of the GUI channel — cheaply cloneable and `Send`.
#[derive(Clone)]
pub struct GUISender {
    tx: std::sync::mpsc::Sender<GUIMessage>,
}

impl GUISender {
    pub fn send(&self, msg: GUIMessage) -> Result<(), std::sync::mpsc::SendError<GUIMessage>> {
        self.tx.send(msg)
    }
}

/// Create a GUI message channel.  The receiver is handed to `run_pm_loop`.
pub fn create_gui_channel() -> (GUISender, std::sync::mpsc::Receiver<GUIMessage>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (GUISender { tx }, rx)
}
