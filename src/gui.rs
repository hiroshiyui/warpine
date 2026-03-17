// SPDX-License-Identifier: GPL-3.0-only
use std::sync::Arc;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{BlendMode, Canvas, Texture};
use sdl2::video::Window;
use log::debug;
use crate::loader::{SharedState, OS2Message, MutexExt,
    WM_CLOSE, WM_SIZE, WM_PAINT, WM_CHAR, WM_MOUSEMOVE, WM_BUTTON1DOWN, WM_BUTTON1UP};

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
}

/// Sender half of the GUI channel — cheaply cloneable and `Send`.
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

// ── PmRenderer trait ───────────────────────────────────────────────────────

/// Backend abstraction for the Presentation Manager GUI loop.
///
/// All methods run on the **main thread** only.
/// Implementors are not required to be `Send`.
pub trait PmRenderer {
    /// Dispatch a single queued `GUIMessage` to the backend.
    ///
    /// Called once per message drained from the channel.
    /// Does not receive `shared` — all draw/window commands operate on backend-local state.
    fn handle_message(&mut self, msg: GUIMessage);

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
            renderer.handle_message(msg);
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

// ── SDL2 backend ───────────────────────────────────────────────────────────

/// Per-window state: SDL2 canvas, a cached streaming texture, and the pixel buffer.
///
/// The texture is created with the `unsafe_textures` feature so it carries no
/// lifetime — both `canvas` and `texture` are dropped together when `WindowData`
/// is dropped.
struct WindowData {
    canvas: Canvas<Window>,
    /// Streaming texture that mirrors the pixel buffer on the GPU side.
    texture: Texture,
    buffer: Vec<u32>,
    width: u32,
    height: u32,
}

impl WindowData {
    /// Upload `buffer` to the texture and blit to the canvas.
    fn present(&mut self) {
        let w = self.width as usize;
        let buf = &self.buffer;
        self.texture.with_lock(None, |data: &mut [u8], pitch: usize| {
            for (y, row) in buf.chunks(w).enumerate() {
                let dst = &mut data[y * pitch..y * pitch + w * 4];
                // Safety: row is a &[u32] aligned to 4; dst is &mut [u8] with
                // exactly w*4 bytes — same byte count as row.len()*4.
                let src: &[u8] = unsafe {
                    std::slice::from_raw_parts(row.as_ptr() as *const u8, row.len() * 4)
                };
                dst.copy_from_slice(src);
            }
        }).expect("texture lock failed");
        self.canvas.copy(&self.texture, None, None).expect("canvas copy failed");
        self.canvas.present();
    }

    /// Recreate the streaming texture after a window resize.
    fn resize_texture(&mut self, w: u32, h: u32) {
        let tc = self.canvas.texture_creator();
        self.texture = tc
            .create_texture_streaming(PixelFormatEnum::ARGB8888, w, h)
            .expect("Failed to recreate texture");
        self.texture.set_blend_mode(BlendMode::None);
        self.width = w;
        self.height = h;
        let pixel_count = (w as usize).checked_mul(h as usize)
            .expect("Window dimensions overflow");
        self.buffer = vec![0xFFFFFFFF_u32; pixel_count];
    }
}

/// SDL2-backed Presentation Manager renderer.
///
/// Created on the main thread; must stay on the main thread for the duration
/// of `run_pm_loop`.
pub struct Sdl2Renderer {
    video: sdl2::VideoSubsystem,
    event_pump: sdl2::EventPump,
    /// SDL2 window ID → PM handle
    id_to_handle: HashMap<u32, u32>,
    /// PM handle → window state
    windows: HashMap<u32, WindowData>,
}

impl Sdl2Renderer {
    /// Create an `Sdl2Renderer` from an existing SDL2 context.
    ///
    /// `sdl` must outlive this renderer.  Typical usage:
    /// ```rust,ignore
    /// let sdl = sdl2::init().unwrap();
    /// let mut renderer = Sdl2Renderer::new(&sdl);
    /// run_pm_loop(&mut renderer, shared, rx);
    /// ```
    pub fn new(sdl: &sdl2::Sdl) -> Self {
        let video = sdl.video().expect("SDL2 video subsystem init failed");
        let event_pump = sdl.event_pump().expect("SDL2 event pump init failed");
        Sdl2Renderer {
            video,
            event_pump,
            id_to_handle: HashMap::new(),
            windows: HashMap::new(),
        }
    }

    /// Handle a single SDL2 event, posting OS/2 messages to `shared` as needed.
    /// Returns `false` when the application should exit.
    fn handle_sdl_event(&mut self, event: Event, shared: &Arc<SharedState>) -> bool {
        match event {
            Event::Quit { .. } => {
                shared.exit_requested.store(true, Ordering::Relaxed);
                // Collect handles first to avoid borrowing self.windows while posting
                let handles: Vec<u32> = self.windows.keys().copied().collect();
                for handle in handles {
                    push_msg(shared, handle, WM_CLOSE, 0, 0);
                }
                return false;
            }
            Event::Window { window_id, win_event, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    use sdl2::event::WindowEvent;
                    match win_event {
                        WindowEvent::Close => {
                            push_msg(shared, handle, WM_CLOSE, 0, 0);
                            shared.exit_requested.store(true, Ordering::Relaxed);
                            return false;
                        }
                        WindowEvent::Resized(w, h) => {
                            let (w, h) = (w as u32, h as u32);
                            if let Some(wd) = self.windows.get_mut(&handle) {
                                wd.resize_texture(w, h);
                            }
                            // Sync OS2Window dimensions so WinQueryWindowRect stays accurate.
                            {
                                let mut wm = shared.window_mgr.lock_or_recover();
                                let client = wm.frame_to_client.get(&handle).copied();
                                for hwnd in std::iter::once(handle).chain(client) {
                                    if let Some(win) = wm.get_window_mut(hwnd) {
                                        win.cx = w as i32;
                                        win.cy = h as i32;
                                    }
                                }
                            }
                            let mp2 = (h << 16) | w;
                            push_msg(shared, handle, WM_SIZE, 0, mp2);
                            push_msg(shared, handle, WM_PAINT, 0, 0);
                        }
                        WindowEvent::Exposed => {
                            if let Some(wd) = self.windows.get_mut(&handle) {
                                wd.present();
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::KeyDown { window_id, keycode, repeat: false, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    let ch = keycode.map(sdl_keycode_to_char).unwrap_or(0);
                    let flags: u32 = 0x0001; // KC_CHAR (key down)
                    let mp1 = (flags << 16) | 1;
                    push_msg(shared, handle, WM_CHAR, mp1, ch);
                }
            }
            Event::KeyUp { window_id, keycode, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    let ch = keycode.map(sdl_keycode_to_char).unwrap_or(0);
                    let flags: u32 = 0x0041; // KC_CHAR | KC_KEYUP
                    let mp1 = (flags << 16) | 1;
                    push_msg(shared, handle, WM_CHAR, mp1, ch);
                }
            }
            Event::MouseMotion { window_id, x, y, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    let height = self.windows.get(&handle).map(|w| w.height).unwrap_or(480);
                    let os2_y = (height as i32 - 1) - y;
                    let mp1 = ((x as u32) & 0xFFFF) | ((os2_y as u32 & 0xFFFF) << 16);
                    push_msg(shared, handle, WM_MOUSEMOVE, mp1, 0);
                }
            }
            Event::MouseButtonDown { window_id, mouse_btn, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    match mouse_btn {
                        sdl2::mouse::MouseButton::Left =>
                            push_msg(shared, handle, WM_BUTTON1DOWN, 0, 0),
                        _ => {}
                    }
                }
            }
            Event::MouseButtonUp { window_id, mouse_btn, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    match mouse_btn {
                        sdl2::mouse::MouseButton::Left =>
                            push_msg(shared, handle, WM_BUTTON1UP, 0, 0),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        true
    }
}

impl PmRenderer for Sdl2Renderer {
    fn handle_message(&mut self, msg: GUIMessage) {
        match msg {
            GUIMessage::CreateWindow { title, handle, .. } => {
                let window = self.video
                    .window(&title, 640, 480)
                    .position_centered()
                    .resizable()
                    .build()
                    .expect("Failed to create SDL2 window");
                let sdl_id = window.id();
                let canvas = window
                    .into_canvas()
                    .software()
                    .build()
                    .expect("Failed to create SDL2 canvas");
                let (w, h) = canvas.output_size().expect("output_size failed");
                // Create a streaming texture; with `unsafe_textures` there is no
                // lifetime tie to `tc` — dropping `tc` is safe.
                let mut texture = {
                    let tc = canvas.texture_creator();
                    tc.create_texture_streaming(PixelFormatEnum::ARGB8888, w, h)
                        .expect("Failed to create streaming texture")
                };
                // BlendMode::None makes SDL2 ignore the alpha channel and render
                // every pixel as fully opaque, so 0x00RRGGBB colours display correctly.
                texture.set_blend_mode(BlendMode::None);
                let buffer = vec![0xFFFFFFFF_u32; (w * h) as usize];
                self.id_to_handle.insert(sdl_id, handle);
                self.windows.insert(handle, WindowData { canvas, texture, buffer, width: w, height: h });
                debug!("[GUI] Created SDL2 window for PM handle {}", handle);
            }
            GUIMessage::ResizeWindow { handle, width, height } => {
                if let Some(wd) = self.windows.get_mut(&handle) {
                    let _ = wd.canvas.window_mut().set_size(width, height);
                    debug!("[GUI] Resized window {} to {}x{}", handle, width, height);
                }
            }
            GUIMessage::MoveWindow { handle, x, y } => {
                if let Some(wd) = self.windows.get_mut(&handle) {
                    wd.canvas.window_mut().set_position(
                        sdl2::video::WindowPos::Positioned(x),
                        sdl2::video::WindowPos::Positioned(y),
                    );
                    debug!("[GUI] Moved window {} to ({}, {})", handle, x, y);
                }
            }
            GUIMessage::ShowWindow { handle, show } => {
                if let Some(wd) = self.windows.get_mut(&handle) {
                    if show { wd.canvas.window_mut().show(); }
                    else    { wd.canvas.window_mut().hide(); }
                    debug!("[GUI] Window {} visible={}", handle, show);
                }
            }
            GUIMessage::DrawBox { handle, x1, y1, x2, y2, color, fill } => {
                if let Some(wd) = self.windows.get_mut(&handle) {
                    render_rect_to_buffer(&mut wd.buffer, wd.width, wd.height, x1, y1, x2, y2, color, fill);
                }
            }
            GUIMessage::DrawLine { handle, x1, y1, x2, y2, color } => {
                if let Some(wd) = self.windows.get_mut(&handle) {
                    render_line_to_buffer(&mut wd.buffer, wd.width, wd.height, x1, y1, x2, y2, color);
                }
            }
            GUIMessage::DrawText { handle, x, y, text, color } => {
                if let Some(wd) = self.windows.get_mut(&handle) {
                    render_text_to_buffer(&mut wd.buffer, wd.width, wd.height, x, y, &text, color);
                }
            }
            GUIMessage::ClearBuffer { handle } => {
                if let Some(wd) = self.windows.get_mut(&handle) {
                    wd.buffer.fill(0xFFFFFFFF);
                }
            }
            GUIMessage::PresentBuffer { handle } => {
                if let Some(wd) = self.windows.get_mut(&handle) {
                    wd.present();
                }
            }
        }
    }

    fn poll_events(&mut self, shared: &Arc<SharedState>) -> bool {
        let events: Vec<_> = self.event_pump.poll_iter().collect();
        for event in events {
            if !self.handle_sdl_event(event, shared) {
                return false;
            }
        }
        true
    }

    // frame_sleep: use default 8 ms implementation
}

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
    fn handle_message(&mut self, _msg: GUIMessage) {
        self.message_count += 1;
    }

    fn poll_events(&mut self, _shared: &Arc<SharedState>) -> bool {
        self.keep_running
    }

    fn frame_sleep(&self) {
        // No-op: no sleep needed in headless mode.
    }
}

// ── Internal helpers ───────────────────────────────────────────────────────

/// Post an OS/2 message to the queue associated with `hwnd`.
///
/// Looks up the client window via `frame_to_client` first; falls back to
/// `hwnd` itself.  Notifies the condvar so `WinGetMsg` wakes up.
fn push_msg(shared: &Arc<SharedState>, hwnd: u32, msg: u32, mp1: u32, mp2: u32) {
    let wm = shared.window_mgr.lock_or_recover();
    let target = wm.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
    let hmq = wm.find_hmq_for_hwnd(target).or_else(|| wm.find_hmq_for_hwnd(hwnd));
    if let Some(hmq) = hmq {
        if let Some(mq_arc) = wm.get_mq(hmq) {
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(OS2Message {
                hwnd: target, msg, mp1, mp2, time: 0, x: 0, y: 0,
            });
            mq.cond.notify_one();
        }
    }
}

/// Map an SDL2 Keycode to an OS/2 character code.
///
/// SDL2 printable keycodes equal their Unicode / ASCII code point; non-printable
/// keys (arrows, F-keys, etc.) have the SDLK_SCANCODE_MASK bit set (0x40000000).
fn sdl_keycode_to_char(kc: Keycode) -> u32 {
    // SDL2 gives single-character names to printable keys (letters, digits, symbols).
    // Non-printable keys (arrows, F-keys, etc.) have multi-character names.
    let name = kc.name();
    let b = name.as_bytes();
    if b.len() == 1 && b[0].is_ascii() {
        // SDL2 names letter keys with an uppercase letter (e.g. "A"), but the
        // corresponding keycode represents the unshifted (lowercase) character.
        // Shift state is tracked separately via KeyMod and should be applied by
        // the application, not here.
        let c = if b[0].is_ascii_uppercase() { b[0] + 32 } else { b[0] };
        return c as u32;
    }
    // Explicit mappings for common control / whitespace keys.
    match kc {
        Keycode::Space     => b' ' as u32,
        Keycode::Return    => b'\r' as u32,
        Keycode::Tab       => b'\t' as u32,
        Keycode::Backspace => 8,
        Keycode::Escape    => 27,
        Keycode::Delete    => 127,
        _ => 0,
    }
}

// ── Standalone geometry helpers (testable without SDL2) ───────────────────

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

/// Render text into a raw pixel buffer (no SDL2 dependency).
/// `width`/`height` are buffer dimensions; coordinates are OS/2 bottom-left origin.
pub fn render_text_to_buffer(buf: &mut [u32], width: u32, height: u32, x: i32, y: i32, text: &str, color: u32) {
    if width == 0 || height == 0 { return; }
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
    if width == 0 || height == 0 { return; }
    let left = x1.min(x2).max(0).min(width as i32 - 1) as u32;
    let right = x1.max(x2).max(0).min(width as i32 - 1) as u32;
    let bottom = y1.min(y2).max(0).min(height as i32 - 1) as u32;
    let top = y1.max(y2).max(0).min(height as i32 - 1) as u32;
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
    if width == 0 || height == 0 { return; }
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
    fn test_sdl_keycode_to_char_printable() {
        // SDL2 printable keycodes equal their ASCII value directly
        assert_eq!(sdl_keycode_to_char(Keycode::A), b'a' as u32);
        assert_eq!(sdl_keycode_to_char(Keycode::Space), b' ' as u32);
        assert_eq!(sdl_keycode_to_char(Keycode::Num1), b'1' as u32);
    }

    #[test]
    fn test_sdl_keycode_to_char_special_returns_zero() {
        // Non-printable keys (arrows, F-keys) have SDLK_SCANCODE_MASK set
        assert_eq!(sdl_keycode_to_char(Keycode::Up), 0);
        assert_eq!(sdl_keycode_to_char(Keycode::F1), 0);
        assert_eq!(sdl_keycode_to_char(Keycode::Return), b'\r' as u32);
    }

    #[test]
    fn test_flip_y_origin() {
        assert_eq!(flip_y(0, 480), 479);
        assert_eq!(flip_y(479, 480), 0);
        assert_eq!(flip_y(239, 480), 240);
    }

    #[test]
    fn test_text_screen_y_not_inverted() {
        let height = 480u32;
        let char_h = 16i32;
        let y = 100i32;
        let top_row_y = text_screen_y(y, 0, char_h, height);
        let bot_row_y = text_screen_y(y, 15, char_h, height);
        assert!(top_row_y < bot_row_y, "font row 0 must be above row 15 on screen");
        assert_eq!(bot_row_y - top_row_y, 15);
    }

    #[test]
    fn test_text_screen_y_values() {
        assert_eq!(text_screen_y(100, 0, 16, 480), 364);
        assert_eq!(text_screen_y(100, 15, 16, 480), 379);
    }

    #[test]
    fn test_glyph_index() {
        assert_eq!(glyph_index(' '), 0);
        assert_eq!(glyph_index('!'), 1);
        assert_eq!(glyph_index('A'), 33);
        assert_eq!(glyph_index('~'), 94);
        assert_eq!(glyph_index('\n'), 0);
        assert_eq!(glyph_index('\x01'), 0);
    }

    #[test]
    fn test_render_text_writes_pixels() {
        let (w, h) = (80u32, 32u32);
        let mut buf = vec![0u32; (w * h) as usize];
        render_text_to_buffer(&mut buf, w, h, 0, 0, "A", 0xFF0000);
        let colored = buf.iter().filter(|&&p| p == 0xFF0000).count();
        assert!(colored > 0, "rendering 'A' should produce colored pixels");
    }

    #[test]
    fn test_render_text_right_side_up() {
        let (w, h) = (16u32, 32u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0xFFFFFF;
        render_text_to_buffer(&mut buf, w, h, 0, 8, "T", color);

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
        render_rect_to_buffer(&mut buf, w, h, 5, 5, 14, 14, color, true);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 100);
    }

    #[test]
    fn test_outline_rect() {
        let (w, h) = (20u32, 20u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x00FF00;
        render_rect_to_buffer(&mut buf, w, h, 5, 5, 14, 14, color, false);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 36);
    }

    #[test]
    fn test_line_horizontal() {
        let (w, h) = (20u32, 10u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x0000FF;
        render_line_to_buffer(&mut buf, w, h, 0, 5, 19, 5, color);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 20);
    }

    #[test]
    fn test_line_vertical() {
        let (w, h) = (10u32, 20u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x0000FF;
        render_line_to_buffer(&mut buf, w, h, 5, 0, 5, 19, color);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 20);
    }

    #[test]
    fn test_line_diagonal() {
        let (w, h) = (10u32, 10u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x0000FF;
        render_line_to_buffer(&mut buf, w, h, 0, 0, 9, 9, color);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 10);
    }

    #[test]
    fn test_rect_y_flip() {
        let (w, h) = (10u32, 10u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0xFF0000;
        render_rect_to_buffer(&mut buf, w, h, 0, 0, 9, 0, color, true);
        for x in 0..10 {
            assert_eq!(buf[9 * 10 + x], color, "pixel at screen row 9 (OS/2 y=0) should be set");
        }
        for x in 0..10 {
            assert_eq!(buf[0 * 10 + x], 0, "pixel at screen row 0 should be empty");
        }
    }

    // ── HeadlessRenderer tests ────────────────────────────────────────────

    fn make_shared() -> Arc<crate::loader::SharedState> {
        crate::loader::Loader::new_mock().shared
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
}
