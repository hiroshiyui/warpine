// SPDX-License-Identifier: GPL-3.0-only
use std::sync::Arc;
use std::num::NonZeroU32;
use std::collections::HashMap;
use winit::application::ApplicationHandler;
use winit::event::{WindowEvent, ElementState, MouseButton};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};
use softbuffer::{Context, Surface};
use log::debug;
use crate::loader::{SharedState, OS2Message, MutexExt, WM_CLOSE, WM_SIZE, WM_PAINT, WM_CHAR, WM_MOUSEMOVE, WM_BUTTON1DOWN, WM_BUTTON1UP};

pub enum GUIMessage {
    CreateWindow { class: String, title: String, handle: u32 },
    DrawBox { handle: u32, x1: i32, y1: i32, x2: i32, y2: i32, color: u32, fill: bool },
    DrawLine { handle: u32, x1: i32, y1: i32, x2: i32, y2: i32, color: u32 },
    DrawText { handle: u32, x: i32, y: i32, text: String, color: u32 },
    ClearBuffer { handle: u32 },
    PresentBuffer { handle: u32 },
}

pub struct WindowState {
    pub window: Arc<Window>,
    pub surface: Surface<Arc<Window>, Arc<Window>>,
    pub buffer: Vec<u32>,
    pub width: u32,
    pub height: u32,
}

pub struct GUIApp {
    shared: Arc<SharedState>,
    rx: std::sync::mpsc::Receiver<GUIMessage>,
    windows: HashMap<WindowId, u32>, // winit ID -> PM handle
    states: HashMap<u32, WindowState>, // PM handle -> state
    context: Option<Context<Arc<Window>>>,
}

impl GUIApp {
    pub fn new(shared: Arc<SharedState>, rx: std::sync::mpsc::Receiver<GUIMessage>) -> Self {
        GUIApp {
            shared,
            rx,
            windows: HashMap::new(),
            states: HashMap::new(),
            context: None,
        }
    }

    fn draw_rect(&mut self, handle: u32, x1: i32, y1: i32, x2: i32, y2: i32, color: u32, fill: bool) {
        if let Some(state) = self.states.get_mut(&handle) {
            render_rect_to_buffer(&mut state.buffer, state.width, state.height, x1, y1, x2, y2, color, fill);
        }
    }

    fn draw_line(&mut self, handle: u32, x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
        if let Some(state) = self.states.get_mut(&handle) {
            render_line_to_buffer(&mut state.buffer, state.width, state.height, x1, y1, x2, y2, color);
        }
    }

    fn draw_text(&mut self, handle: u32, x: i32, y: i32, text: &str, color: u32) {
        if let Some(state) = self.states.get_mut(&handle) {
            render_text_to_buffer(&mut state.buffer, state.width, state.height, x, y, text, color);
        }
    }

    fn present_buffer(&mut self, handle: u32) {
        if let Some(state) = self.states.get_mut(&handle) {
            let mut buffer = state.surface.buffer_mut().unwrap();
            buffer.copy_from_slice(&state.buffer);
            buffer.present().unwrap();
        }
    }

    fn push_msg(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) {
        let wm = self.shared.window_mgr.lock_or_recover();
        // Redirect frame window messages to the client window
        let target_hwnd = wm.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
        let hmq = wm.find_hmq_for_hwnd(target_hwnd)
            .or_else(|| wm.find_hmq_for_hwnd(hwnd));
        if let Some(hmq) = hmq {
            if let Some(mq_arc) = wm.get_mq(hmq) {
                let mut mq = mq_arc.lock_or_recover();
                mq.messages.push_back(OS2Message {
                    hwnd: target_hwnd, msg, mp1, mp2,
                    time: 0, x: 0, y: 0
                });
                mq.cond.notify_one();
            }
        }
    }
}

impl ApplicationHandler<()> for GUIApp {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: ()) {
        self.process_gui_messages(event_loop);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let pm_handle = self.windows.get(&id).cloned();

        match event {
            WindowEvent::CloseRequested => {
                if let Some(handle) = pm_handle {
                    self.push_msg(handle, WM_CLOSE, 0, 0);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(handle) = pm_handle {
                    if let Some(state) = self.states.get_mut(&handle) {
                        if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                            state.surface.resize(w, h).unwrap();
                            state.width = size.width;
                            state.height = size.height;
                            let pixel_count = (size.width as usize).checked_mul(size.height as usize)
                                .expect("Window dimensions overflow");
                            state.buffer = vec![0xFFFFFFFF; pixel_count];
                        }
                    }
                    let mp2 = (size.height << 16) | size.width;
                    self.push_msg(handle, WM_SIZE, 0, mp2);
                    self.push_msg(handle, WM_PAINT, 0, 0);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(handle) = pm_handle {
                    if let Some(state) = self.states.get_mut(&handle) {
                        let mut buffer = state.surface.buffer_mut().unwrap();
                        buffer.copy_from_slice(&state.buffer);
                        buffer.present().unwrap();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(handle) = pm_handle {
                    let flags: u32 = match event.state {
                        ElementState::Pressed => 0x0001,
                        ElementState::Released => 0x0041,
                    };
                    let char_code = event.text.as_ref()
                        .and_then(|t| t.chars().next())
                        .map(|c| c as u32)
                        .unwrap_or(0);
                    let mp1 = (flags << 16) | 1;
                    self.push_msg(handle, WM_CHAR, mp1, char_code);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(handle) = pm_handle {
                    if let Some(state) = self.states.get(&handle) {
                        let x = position.x as i32;
                        let y = (state.height as i32 - 1) - position.y as i32;
                        let mp1 = ((x as u32) & 0xFFFF) | (((y as u32) & 0xFFFF) << 16);
                        self.push_msg(handle, WM_MOUSEMOVE, mp1, 0);
                    }
                }
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                if let Some(handle) = pm_handle {
                    let msg = match (button, btn_state) {
                        (MouseButton::Left, ElementState::Pressed) => WM_BUTTON1DOWN,
                        (MouseButton::Left, ElementState::Released) => WM_BUTTON1UP,
                        _ => return,
                    };
                    self.push_msg(handle, msg, 0, 0);
                }
            }
            _ => (),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.process_gui_messages(event_loop);
    }
}

impl GUIApp {
    fn process_gui_messages(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                GUIMessage::CreateWindow { class: _, title, handle } => {
                    let window_attrs = winit::window::WindowAttributes::default()
                        .with_title(title)
                        .with_inner_size(winit::dpi::LogicalSize::new(640.0, 480.0));

                    let window = Arc::new(event_loop.create_window(window_attrs).unwrap());
                    let id = window.id();

                    if self.context.is_none() {
                        self.context = Some(Context::new(window.clone()).unwrap());
                    }

                    let mut surface = Surface::new(self.context.as_ref().unwrap(), window.clone()).unwrap();
                    let size = window.inner_size();
                    surface.resize(NonZeroU32::new(size.width).unwrap(), NonZeroU32::new(size.height).unwrap()).unwrap();

                    let width = size.width;
                    let height = size.height;
                    let pixel_count = (width as usize).checked_mul(height as usize)
                        .expect("Window dimensions overflow");
                    let buffer = vec![0xFFFFFFFF; pixel_count];

                    self.windows.insert(id, handle);
                    self.states.insert(handle, WindowState {
                        window, surface, buffer, width, height,
                    });

                    debug!("  [GUI] Created window for PM handle {}", handle);
                }
                GUIMessage::DrawBox { handle, x1, y1, x2, y2, color, fill } => {
                    self.draw_rect(handle, x1, y1, x2, y2, color, fill);
                }
                GUIMessage::DrawLine { handle, x1, y1, x2, y2, color } => {
                    self.draw_line(handle, x1, y1, x2, y2, color);
                }
                GUIMessage::DrawText { handle, x, y, text, color } => {
                    self.draw_text(handle, x, y, &text, color);
                }
                GUIMessage::ClearBuffer { handle } => {
                    if let Some(state) = self.states.get_mut(&handle) {
                        state.buffer.fill(0xFFFFFFFF); // White background
                    }
                }
                GUIMessage::PresentBuffer { handle } => {
                    self.present_buffer(handle);
                    if let Some(state) = self.states.get(&handle) {
                        state.window.request_redraw();
                    }
                }
            }
        }
    }
}

/// Channel wrapper that wakes the event loop when a message is sent
pub struct GUISender {
    tx: std::sync::mpsc::Sender<GUIMessage>,
    proxy: EventLoopProxy<()>,
}

impl GUISender {
    pub fn send(&self, msg: GUIMessage) -> Result<(), std::sync::mpsc::SendError<GUIMessage>> {
        let result = self.tx.send(msg);
        let _ = self.proxy.send_event(());
        result
    }
}

/// Create a GUI sender/receiver pair with event loop waking
pub fn create_gui_channel(event_loop: &EventLoop<()>) -> (GUISender, std::sync::mpsc::Receiver<GUIMessage>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let proxy = event_loop.create_proxy();
    (GUISender { tx, proxy }, rx)
}

// ── Standalone geometry helpers (testable without winit) ──

/// Flip an OS/2 Y coordinate (bottom-left origin) to screen Y (top-left origin).
pub fn flip_y(y: i32, height: u32) -> i32 {
    (height as i32 - 1) - y
}

/// Compute the screen Y for a font glyph row, given OS/2 baseline Y.
/// Font row 0 is the top of the glyph; OS/2 Y is bottom-left origin.
pub fn text_screen_y(y: i32, row: i32, char_h: i32, height: u32) -> i32 {
    height as i32 - y - char_h + row
}

/// Map a font character to its glyph index (0 = space for unknown/control chars).
pub fn glyph_index(ch: char) -> usize {
    let c = ch as u32;
    if c >= 32 && c <= 126 { (c - 32) as usize } else { 0 }
}

/// Render text into a raw pixel buffer (no winit dependency).
/// `width`/`height` are buffer dimensions; coordinates are OS/2 bottom-left origin.
pub fn render_text_to_buffer(buf: &mut [u32], width: u32, height: u32, x: i32, y: i32, text: &str, color: u32) {
    let char_w: i32 = 8;
    let char_h: i32 = 16;
    for (i, ch) in text.chars().enumerate() {
        let cx = x + (i as i32 * char_w);
        let gi = glyph_index(ch);
        for row in 0..char_h {
            let bits = crate::font8x16::FONT_8X16[gi * 16 + row as usize];
            for col in 0..char_w {
                if bits & (0x80 >> col) != 0 {
                    let px = cx + col;
                    let py = text_screen_y(y, row, char_h, height);
                    if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                        buf[(py as u32 * width + px as u32) as usize] = color;
                    }
                }
            }
        }
    }
}

/// Draw a filled or outlined rectangle into a raw pixel buffer.
/// Coordinates are OS/2 bottom-left origin.
pub fn render_rect_to_buffer(buf: &mut [u32], width: u32, height: u32,
                              x1: i32, y1: i32, x2: i32, y2: i32, color: u32, fill: bool) {
    let left = x1.min(x2).max(0) as u32;
    let right = x1.max(x2).min(width as i32 - 1) as u32;
    let bottom = y1.min(y2).max(0) as u32;
    let top = y1.max(y2).min(height as i32 - 1) as u32;
    let top_y = (height - 1) - bottom;
    let bottom_y = (height - 1) - top;

    if fill {
        for y in bottom_y..=top_y {
            for x in left..=right {
                let idx = (y * width + x) as usize;
                if idx < buf.len() { buf[idx] = color; }
            }
        }
    } else {
        for x in left..=right {
            let idx_b = (bottom_y * width + x) as usize;
            let idx_t = (top_y * width + x) as usize;
            if idx_b < buf.len() { buf[idx_b] = color; }
            if idx_t < buf.len() { buf[idx_t] = color; }
        }
        for y in bottom_y..=top_y {
            let idx_l = (y * width + left) as usize;
            let idx_r = (y * width + right) as usize;
            if idx_l < buf.len() { buf[idx_l] = color; }
            if idx_r < buf.len() { buf[idx_r] = color; }
        }
    }
}

/// Draw a line into a raw pixel buffer using Bresenham's algorithm.
/// Coordinates are OS/2 bottom-left origin.
pub fn render_line_to_buffer(buf: &mut [u32], width: u32, height: u32,
                              x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
    let flip = |y: i32| -> i32 { (height as i32 - 1) - y };
    let mut x = x1;
    let mut y = y1;
    let dx = (x2 - x1).abs();
    let dy = (y2 - y1).abs();
    let sx = if x1 < x2 { 1 } else { -1 };
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut err = dx - dy;

    loop {
        let fy = flip(y);
        if x >= 0 && x < width as i32 && fy >= 0 && fy < height as i32 {
            buf[(fy as u32 * width + x as u32) as usize] = color;
        }
        if x == x2 && y == y2 { break; }
        let e2 = 2 * err;
        if e2 > -dy { err -= dy; x += sx; }
        if e2 < dx { err += dx; y += sy; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flip_y_origin() {
        // In a 480-pixel-tall buffer, OS/2 y=0 (bottom) → screen y=479
        assert_eq!(flip_y(0, 480), 479);
        // OS/2 y=479 (top) → screen y=0
        assert_eq!(flip_y(479, 480), 0);
        // Midpoint
        assert_eq!(flip_y(239, 480), 240);
    }

    #[test]
    fn test_text_screen_y_not_inverted() {
        let height = 480u32;
        let char_h = 16i32;
        let y = 100i32; // OS/2 text baseline
        // Row 0 (top of glyph) should have a SMALLER screen y than row 15 (bottom)
        let top_row_y = text_screen_y(y, 0, char_h, height);
        let bot_row_y = text_screen_y(y, 15, char_h, height);
        assert!(top_row_y < bot_row_y, "font row 0 must be above row 15 on screen");
        // Consecutive rows increase by exactly 1
        assert_eq!(bot_row_y - top_row_y, 15);
    }

    #[test]
    fn test_text_screen_y_values() {
        // height=480, y=100, char_h=16
        // row 0: 480 - 100 - 16 + 0 = 364
        // row 15: 480 - 100 - 16 + 15 = 379
        assert_eq!(text_screen_y(100, 0, 16, 480), 364);
        assert_eq!(text_screen_y(100, 15, 16, 480), 379);
    }

    #[test]
    fn test_glyph_index() {
        assert_eq!(glyph_index(' '), 0);
        assert_eq!(glyph_index('!'), 1);
        assert_eq!(glyph_index('A'), 33);
        assert_eq!(glyph_index('~'), 94);
        // Control chars and non-ASCII map to 0 (space)
        assert_eq!(glyph_index('\n'), 0);
        assert_eq!(glyph_index('\x01'), 0);
    }

    #[test]
    fn test_render_text_writes_pixels() {
        let (w, h) = (80u32, 32u32);
        let mut buf = vec![0u32; (w * h) as usize];
        render_text_to_buffer(&mut buf, w, h, 0, 0, "A", 0xFF0000);
        // The letter 'A' should have set some non-zero pixels
        let colored = buf.iter().filter(|&&p| p == 0xFF0000).count();
        assert!(colored > 0, "rendering 'A' should produce colored pixels");
    }

    #[test]
    fn test_render_text_right_side_up() {
        // Render 'T' and verify the top horizontal bar is above the vertical stem
        let (w, h) = (16u32, 32u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0xFFFFFF;
        render_text_to_buffer(&mut buf, w, h, 0, 8, "T", color);

        // Find the topmost and bottommost screen rows with colored pixels
        let mut top_row = h as i32;
        let mut bot_row = -1i32;
        for py in 0..h as i32 {
            for px in 0..8 {
                if buf[(py as u32 * w + px as u32) as usize] == color {
                    if py < top_row { top_row = py; }
                    if py > bot_row { bot_row = py; }
                }
            }
        }
        assert!(top_row < bot_row);

        // The top row of the glyph (the bar of 'T') should have more pixels than
        // a middle row (the stem), confirming it's not upside down
        let count_at = |row: i32| -> usize {
            (0..8).filter(|&px| buf[(row as u32 * w + px as u32) as usize] == color).count()
        };
        let top_count = count_at(top_row);
        let mid_count = count_at((top_row + bot_row) / 2);
        assert!(top_count > mid_count,
            "top row should have more pixels (bar of T) than middle (stem): top={}, mid={}", top_count, mid_count);
    }

    #[test]
    fn test_filled_rect() {
        let (w, h) = (20u32, 20u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0xFF0000;
        // OS/2 coords: fill from (5,5) to (14,14)
        render_rect_to_buffer(&mut buf, w, h, 5, 5, 14, 14, color, true);
        let colored = buf.iter().filter(|&&p| p == color).count();
        // 10x10 box = 100 pixels
        assert_eq!(colored, 100);
    }

    #[test]
    fn test_outline_rect() {
        let (w, h) = (20u32, 20u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x00FF00;
        render_rect_to_buffer(&mut buf, w, h, 5, 5, 14, 14, color, false);
        let colored = buf.iter().filter(|&&p| p == color).count();
        // Outline of 10x10: 4*10 - 4 corners = 36
        assert_eq!(colored, 36);
    }

    #[test]
    fn test_line_horizontal() {
        let (w, h) = (20u32, 10u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x0000FF;
        render_line_to_buffer(&mut buf, w, h, 0, 5, 19, 5, color);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 20); // horizontal line spanning full width
    }

    #[test]
    fn test_line_vertical() {
        let (w, h) = (10u32, 20u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x0000FF;
        render_line_to_buffer(&mut buf, w, h, 5, 0, 5, 19, color);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 20); // vertical line spanning full height
    }

    #[test]
    fn test_line_diagonal() {
        let (w, h) = (10u32, 10u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x0000FF;
        render_line_to_buffer(&mut buf, w, h, 0, 0, 9, 9, color);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 10); // perfect diagonal
    }

    #[test]
    fn test_rect_y_flip() {
        // A rect at OS/2 y=0 (bottom) should draw at the bottom of the screen buffer (high py)
        let (w, h) = (10u32, 10u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0xFF0000;
        // OS/2 rect from (0,0) to (9,0) — single row at the very bottom
        render_rect_to_buffer(&mut buf, w, h, 0, 0, 9, 0, color, true);
        // Should be on screen row 9 (the last row)
        for x in 0..10 {
            assert_eq!(buf[9 * 10 + x], color, "pixel at screen row 9 (OS/2 y=0) should be set");
        }
        // Screen row 0 should be empty
        for x in 0..10 {
            assert_eq!(buf[0 * 10 + x], 0, "pixel at screen row 0 should be empty");
        }
    }
}
