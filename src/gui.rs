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

/// Sender half of the GUI channel — cheaply cloneable and `Send`.
pub struct GUISender {
    tx: std::sync::mpsc::Sender<GUIMessage>,
}

impl GUISender {
    pub fn send(&self, msg: GUIMessage) -> Result<(), std::sync::mpsc::SendError<GUIMessage>> {
        self.tx.send(msg)
    }
}

/// Create a GUI message channel.  The receiver is handed to `run_gui_loop`.
pub fn create_gui_channel() -> (GUISender, std::sync::mpsc::Receiver<GUIMessage>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (GUISender { tx }, rx)
}

/// Run the SDL2 GUI event loop.  Must be called from the **main thread**.
///
/// Returns when `shared.exit_requested` is set or the window is closed.
pub fn run_gui_loop(
    sdl: sdl2::Sdl,
    shared: Arc<SharedState>,
    rx: std::sync::mpsc::Receiver<GUIMessage>,
) {
    let video = sdl.video().expect("SDL2 video subsystem init failed");
    let mut event_pump = sdl.event_pump().expect("SDL2 event pump init failed");
    let mut app = GUIApp::new(video, shared.clone(), rx);

    loop {
        // Drain GUI messages from the VCPU thread first.
        app.process_gui_messages();

        // Process all pending SDL events.
        for event in event_pump.poll_iter() {
            if !app.handle_event(event) {
                return;
            }
        }

        if shared.exit_requested.load(Ordering::Relaxed) {
            return;
        }

        // Yield ~8 ms so we don't busy-spin the main thread.
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

// ── Internal app struct ────────────────────────────────────────────────────

struct GUIApp {
    shared: Arc<SharedState>,
    rx: std::sync::mpsc::Receiver<GUIMessage>,
    video: sdl2::VideoSubsystem,
    /// SDL2 window ID → PM handle
    id_to_handle: HashMap<u32, u32>,
    /// PM handle → window state
    windows: HashMap<u32, WindowData>,
}

impl GUIApp {
    fn new(
        video: sdl2::VideoSubsystem,
        shared: Arc<SharedState>,
        rx: std::sync::mpsc::Receiver<GUIMessage>,
    ) -> Self {
        GUIApp { shared, rx, video, id_to_handle: HashMap::new(), windows: HashMap::new() }
    }

    fn push_msg(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) {
        let wm = self.shared.window_mgr.lock_or_recover();
        let target = wm.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
        let hmq = wm.find_hmq_for_hwnd(target).or_else(|| wm.find_hmq_for_hwnd(hwnd));
        if let Some(hmq) = hmq {
            if let Some(mq_arc) = wm.get_mq(hmq) {
                let mut mq = mq_arc.lock_or_recover();
                mq.messages.push_back(OS2Message { hwnd: target, msg, mp1, mp2, time: 0, x: 0, y: 0 });
                mq.cond.notify_one();
            }
        }
    }

    fn process_gui_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
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
    }

    /// Returns `false` to signal the event loop should exit.
    fn handle_event(&mut self, event: Event) -> bool {
        match event {
            Event::Quit { .. } => {
                self.shared.exit_requested.store(true, Ordering::Relaxed);
                for &handle in self.windows.keys() {
                    self.push_msg(handle, WM_CLOSE, 0, 0);
                }
                return false;
            }
            Event::Window { window_id, win_event, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    use sdl2::event::WindowEvent;
                    match win_event {
                        WindowEvent::Close => {
                            self.push_msg(handle, WM_CLOSE, 0, 0);
                            self.shared.exit_requested.store(true, Ordering::Relaxed);
                            return false;
                        }
                        WindowEvent::Resized(w, h) => {
                            let (w, h) = (w as u32, h as u32);
                            if let Some(wd) = self.windows.get_mut(&handle) {
                                wd.resize_texture(w, h);
                            }
                            // Keep OS2Window dimensions in sync so WinQueryWindowRect
                            // returns the correct size after a user resize.
                            {
                                let mut wm = self.shared.window_mgr.lock_or_recover();
                                let client = wm.frame_to_client.get(&handle).copied();
                                for hwnd in std::iter::once(handle).chain(client) {
                                    if let Some(win) = wm.get_window_mut(hwnd) {
                                        win.cx = w as i32;
                                        win.cy = h as i32;
                                    }
                                }
                            }
                            let mp2 = (h << 16) | w;
                            self.push_msg(handle, WM_SIZE, 0, mp2);
                            self.push_msg(handle, WM_PAINT, 0, 0);
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
                    self.push_msg(handle, WM_CHAR, mp1, ch);
                }
            }
            Event::KeyUp { window_id, keycode, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    let ch = keycode.map(sdl_keycode_to_char).unwrap_or(0);
                    let flags: u32 = 0x0041; // KC_CHAR | KC_KEYUP
                    let mp1 = (flags << 16) | 1;
                    self.push_msg(handle, WM_CHAR, mp1, ch);
                }
            }
            Event::MouseMotion { window_id, x, y, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    let height = self.windows.get(&handle).map(|w| w.height).unwrap_or(480);
                    let os2_y = (height as i32 - 1) - y;
                    let mp1 = ((x as u32) & 0xFFFF) | ((os2_y as u32 & 0xFFFF) << 16);
                    self.push_msg(handle, WM_MOUSEMOVE, mp1, 0);
                }
            }
            Event::MouseButtonDown { window_id, mouse_btn, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    match mouse_btn {
                        sdl2::mouse::MouseButton::Left => self.push_msg(handle, WM_BUTTON1DOWN, 0, 0),
                        _ => {}
                    }
                }
            }
            Event::MouseButtonUp { window_id, mouse_btn, .. } => {
                if let Some(&handle) = self.id_to_handle.get(&window_id) {
                    match mouse_btn {
                        sdl2::mouse::MouseButton::Left => self.push_msg(handle, WM_BUTTON1UP, 0, 0),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        true
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
}
