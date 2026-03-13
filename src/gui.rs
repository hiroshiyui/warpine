// SPDX-License-Identifier: GPL-3.0-only
use std::sync::Arc;
use std::num::NonZeroU32;
use std::collections::HashMap;
use winit::application::ApplicationHandler;
use winit::event::{WindowEvent, ElementState, MouseButton};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};
use softbuffer::{Context, Surface};
use crate::loader::{SharedState, OS2Message};

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
            let left = x1.min(x2).max(0) as u32;
            let right = x1.max(x2).min(state.width as i32 - 1) as u32;
            let bottom = y1.min(y2).max(0) as u32;
            let top = y1.max(y2).min(state.height as i32 - 1) as u32;

            // Flip Y-axis (OS/2 uses bottom-left origin)
            let top_y = (state.height - 1) - bottom;
            let bottom_y = (state.height - 1) - top;

            if fill {
                for y in bottom_y..=top_y {
                    for x in left..=right {
                        if (y * state.width + x) < (state.buffer.len() as u32) {
                            state.buffer[(y * state.width + x) as usize] = color;
                        }
                    }
                }
            } else {
                for x in left..=right {
                    if (bottom_y * state.width + x) < (state.buffer.len() as u32) {
                        state.buffer[(bottom_y * state.width + x) as usize] = color;
                    }
                    if (top_y * state.width + x) < (state.buffer.len() as u32) {
                        state.buffer[(top_y * state.width + x) as usize] = color;
                    }
                }
                for y in bottom_y..=top_y {
                    if (y * state.width + left) < (state.buffer.len() as u32) {
                        state.buffer[(y * state.width + left) as usize] = color;
                    }
                    if (y * state.width + right) < (state.buffer.len() as u32) {
                        state.buffer[(y * state.width + right) as usize] = color;
                    }
                }
            }
        }
    }

    fn draw_line(&mut self, handle: u32, x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
        if let Some(state) = self.states.get_mut(&handle) {
            let flip_y = |y: i32| -> i32 { (state.height as i32 - 1) - y };
            let mut x = x1;
            let mut y = y1;
            let dx = (x2 - x1).abs();
            let dy = (y2 - y1).abs();
            let sx = if x1 < x2 { 1 } else { -1 };
            let sy = if y1 < y2 { 1 } else { -1 };
            let mut err = dx - dy;

            loop {
                let fy = flip_y(y);
                if x >= 0 && x < state.width as i32 && fy >= 0 && fy < state.height as i32 {
                    state.buffer[(fy as u32 * state.width + x as u32) as usize] = color;
                }
                if x == x2 && y == y2 { break; }
                let e2 = 2 * err;
                if e2 > -dy { err -= dy; x += sx; }
                if e2 < dx { err += dx; y += sy; }
            }
        }
    }

    fn draw_text(&mut self, handle: u32, x: i32, y: i32, text: &str, color: u32) {
        if let Some(state) = self.states.get_mut(&handle) {
            let char_w: i32 = 8;
            let char_h: i32 = 16;
            for (i, ch) in text.chars().enumerate() {
                let cx = x + (i as i32 * char_w);
                let glyph_idx = if (ch as u32) >= 32 && (ch as u32) <= 126 {
                    (ch as u32 - 32) as usize
                } else {
                    0 // space for unknown chars
                };
                for row in 0..char_h {
                    let bits = crate::font8x16::FONT_8X16[glyph_idx * 16 + row as usize];
                    for col in 0..char_w {
                        if bits & (0x80 >> col) != 0 {
                            let px = cx + col;
                            // Flip Y: font row 0 is glyph top (OS/2 y + char_h - 1)
                            let py = state.height as i32 - y - char_h + row;
                            if px >= 0 && px < state.width as i32 && py >= 0 && py < state.height as i32 {
                                state.buffer[(py as u32 * state.width + px as u32) as usize] = color;
                            }
                        }
                    }
                }
            }
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
        let wm = self.shared.window_mgr.lock().unwrap();
        // Redirect frame window messages to the client window
        let target_hwnd = wm.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
        let hmq = wm.find_hmq_for_hwnd(target_hwnd)
            .or_else(|| wm.find_hmq_for_hwnd(hwnd));
        if let Some(hmq) = hmq {
            if let Some(mq_arc) = wm.get_mq(hmq) {
                let mut mq = mq_arc.lock().unwrap();
                mq.messages.push_back(OS2Message {
                    hwnd: target_hwnd, msg, mp1, mp2,
                    time: 0, x: 0, y: 0
                });
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
                    self.push_msg(handle, 0x0029, 0, 0); // WM_CLOSE
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(handle) = pm_handle {
                    if let Some(state) = self.states.get_mut(&handle) {
                        if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                            state.surface.resize(w, h).unwrap();
                            state.width = size.width;
                            state.height = size.height;
                            state.buffer = vec![0xFFFFFFFF; (size.width * size.height) as usize];
                        }
                    }
                    let mp2 = (size.height << 16) | size.width;
                    self.push_msg(handle, 0x0007, 0, mp2); // WM_SIZE
                    self.push_msg(handle, 0x0023, 0, 0); // WM_PAINT
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
                    self.push_msg(handle, 0x007A, mp1, char_code); // WM_CHAR
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(handle) = pm_handle {
                    if let Some(state) = self.states.get(&handle) {
                        let x = position.x as i32;
                        let y = (state.height as i32 - 1) - position.y as i32;
                        let mp1 = ((x as u32) & 0xFFFF) | (((y as u32) & 0xFFFF) << 16);
                        self.push_msg(handle, 0x0070, mp1, 0); // WM_MOUSEMOVE
                    }
                }
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                if let Some(handle) = pm_handle {
                    let msg = match (button, btn_state) {
                        (MouseButton::Left, ElementState::Pressed) => 0x0071,
                        (MouseButton::Left, ElementState::Released) => 0x0072,
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
                    let buffer = vec![0xFFFFFFFF; (width * height) as usize];

                    self.windows.insert(id, handle);
                    self.states.insert(handle, WindowState {
                        window, surface, buffer, width, height,
                    });

                    println!("  [GUI] Created window for PM handle {}", handle);
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
