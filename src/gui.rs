// SPDX-License-Identifier: GPL-3.0-only
use std::sync::Arc;
use std::num::NonZeroU32;
use std::collections::HashMap;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};
use softbuffer::{Context, Surface};
use crate::loader::SharedState;

pub enum GUIMessage {
    CreateWindow { class: String, title: String, handle: u32 },
    Invalidate { handle: u32 },
    DrawBox { handle: u32, x1: i32, y1: i32, x2: i32, y2: i32, color: u32, fill: bool },
    MoveTo { handle: u32, x: i32, y: i32 },
}

pub struct WindowState {
    pub window: Arc<Window>,
    pub surface: Surface<Arc<Window>, Arc<Window>>,
    pub buffer: Vec<u32>,
    pub width: u32,
    pub height: u32,
    pub current_x: i32,
    pub current_y: i32,
    pub current_color: u32,
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

            if fill {
                for y in bottom..=top {
                    for x in left..=right {
                        state.buffer[(y * state.width + x) as usize] = color;
                    }
                }
            } else {
                // Outline only
                for x in left..=right {
                    state.buffer[(bottom * state.width + x) as usize] = color;
                    state.buffer[(top * state.width + x) as usize] = color;
                }
                for y in bottom..=top {
                    state.buffer[(y * state.width + left) as usize] = color;
                    state.buffer[(y * state.width + right) as usize] = color;
                }
            }
        }
    }
}

impl ApplicationHandler for GUIApp {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(handle) = self.windows.remove(&id) {
                    self.states.remove(&handle);
                }
                if self.windows.is_empty() {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(handle) = self.windows.get(&id) {
                    if let Some(state) = self.states.get_mut(handle) {
                        let mut buffer = state.surface.buffer_mut().unwrap();
                        buffer.copy_from_slice(&state.buffer);
                        buffer.present().unwrap();
                    }
                }
            }
            _ => (),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                GUIMessage::CreateWindow { class: _, title, handle } => {
                    let window_attrs = Window::default_attributes()
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
                    let buffer = vec![0xFFFFFFFF; (width * height) as usize]; // Default white background
                    
                    self.windows.insert(id, handle);
                    self.states.insert(handle, WindowState {
                        window, surface, buffer, width, height,
                        current_x: 0, current_y: 0, current_color: 0
                    });
                }
                GUIMessage::MoveTo { handle, x, y } => {
                    if let Some(state) = self.states.get_mut(&handle) {
                        state.current_x = x;
                        state.current_y = y;
                    }
                }
                GUIMessage::DrawBox { handle, x1, y1, x2, y2, color, fill } => {
                    self.draw_rect(handle, x1, y1, x2, y2, color, fill);
                    if let Some(state) = self.states.get(&handle) {
                        state.window.request_redraw();
                    }
                }
                GUIMessage::Invalidate { handle } => {
                    if let Some(state) = self.states.get(&handle) {
                        state.window.request_redraw();
                    }
                }
            }
        }
    }
}

pub struct GUI {
    shared: Arc<SharedState>,
}

impl GUI {
    pub fn new(shared: Arc<SharedState>) -> Self {
        GUI { shared }
    }

    pub fn run(&self, rx: std::sync::mpsc::Receiver<GUIMessage>) {
        let event_loop = EventLoop::new().unwrap();
        let mut app = GUIApp::new(self.shared.clone(), rx);
        event_loop.run_app(&mut app).unwrap();
    }
}
