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
}

pub struct GUIApp {
    shared: Arc<SharedState>,
    rx: std::sync::mpsc::Receiver<GUIMessage>,
    windows: HashMap<WindowId, Arc<Window>>,
    surfaces: HashMap<WindowId, Surface<Arc<Window>, Arc<Window>>>,
    context: Option<Context<Arc<Window>>>,
}

impl GUIApp {
    pub fn new(shared: Arc<SharedState>, rx: std::sync::mpsc::Receiver<GUIMessage>) -> Self {
        GUIApp {
            shared,
            rx,
            windows: HashMap::new(),
            surfaces: HashMap::new(),
            context: None,
        }
    }
}

impl ApplicationHandler for GUIApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.context.is_none() {
            // Context needs a handle to something that implements HasDisplayHandle.
            // In winit 0.30, we can get it from the event loop or a window.
            // softbuffer's Context::new expects a display handle.
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.windows.remove(&id);
                self.surfaces.remove(&id);
                if self.windows.is_empty() {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(surface) = self.surfaces.get_mut(&id) {
                    let window = self.windows.get(&id).unwrap();
                    let size = window.inner_size();
                    let mut buffer = surface.buffer_mut().unwrap();
                    for index in 0..(size.width * size.height) as usize {
                        buffer[index] = 0x00FFFFFF; // White
                    }
                    buffer.present().unwrap();
                }
            }
            _ => (),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                GUIMessage::CreateWindow { class: _, title, handle: _ } => {
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
                    
                    self.windows.insert(id, window);
                    self.surfaces.insert(id, surface);
                }
                GUIMessage::Invalidate { handle: _ } => {
                    for window in self.windows.values() {
                        window.request_redraw();
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
