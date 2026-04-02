// SPDX-License-Identifier: GPL-3.0-only

use std::sync::Arc;
use std::collections::HashMap;
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod as KeyMod, Scancode};
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{BlendMode, Canvas, Texture};
use sdl2::video::Window;
use log::debug;
use sdl2::messagebox::{show_message_box, ButtonData, MessageBoxButtonFlag, MessageBoxFlag};
use crate::loader::{SharedState, OS2Message, MutexExt,
    WM_CLOSE, WM_SIZE, WM_PAINT, WM_CHAR, WM_MOUSEMOVE,
    WM_BUTTON1DOWN, WM_BUTTON1UP, WM_BUTTON2DOWN, WM_BUTTON2UP, WM_BUTTON3DOWN, WM_BUTTON3UP,
    KC_CHAR, KC_VIRTUALKEY, KC_SCANCODE, KC_SHIFT, KC_CTRL, KC_ALT, KC_KEYUP,
    VK_BACKSPACE, VK_TAB, VK_NEWLINE, VK_ESC, VK_SPACE,
    VK_PAGEUP, VK_PAGEDOWN, VK_END, VK_HOME, VK_LEFT, VK_UP, VK_RIGHT, VK_DOWN,
    VK_INSERT, VK_DELETE, VK_SCRLLOCK, VK_NUMLOCK, VK_ENTER,
    VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6,
    VK_F7, VK_F8, VK_F9, VK_F10, VK_F11, VK_F12,
    CF_TEXT, KC_PREVDOWN,
    FCF_TITLEBAR, FCF_SYSMENU, FCF_MINBUTTON, FCF_MAXBUTTON,
    FCF_BORDER, FCF_DLGBORDER, FCF_SIZEBORDER,
    WM_ACTIVATE, WM_SYSCOMMAND, SC_MINIMIZE, SC_MAXIMIZE};
use super::message::GUIMessage;
use super::renderer::PmRenderer;
use super::render_utils::{render_text_to_buffer, render_rect_to_buffer, render_line_to_buffer};

// ── VDR-A: Single-surface desktop renderer ────────────────────────────────

/// Desktop background colour (OS/2-style teal).
const DESKTOP_BG: u32 = 0x00408040;

// ── VDR-C: Window chrome constants ────────────────────────────────────────

/// Height of the title bar chrome in desktop pixels.
const CHROME_TITLE_H: i32 = 20;
/// Width of the frame border in desktop pixels (each side).
const CHROME_BORDER_W: i32 = 2;

/// Width of each title-bar button (close/min/max/sysmenu), in desktop pixels.
const CHROME_BTN_W: i32   = 16;
/// Gap between adjacent title-bar buttons.
const CHROME_BTN_GAP: i32 = 2;
/// Right margin before the close button.
const CHROME_BTN_MARGIN: i32 = 2;
/// Resize-hit zone width at each window edge (VDR-C5).
const RESIZE_HIT_W: i32 = 6;

/// Active (focused) title bar colour — OS/2 navy blue.
const CHROME_TITLE_ACTIVE: u32   = 0x0000_0080;
/// Inactive title bar colour — dark gray.
const CHROME_TITLE_INACTIVE: u32 = 0x0040_4040;
/// Title bar text colour.
const CHROME_TEXT_COLOR: u32     = 0x00FF_FFFF;
/// Frame border colour — medium gray.
const CHROME_BORDER_COLOR: u32   = 0x0080_8080;

/// Draw ASCII/Unicode text into a desktop-space pixel buffer.
///
/// `sx`/`sy` are screen coordinates (y=0 at top, x=0 at left).
/// Uses 8×16 GNU Unifont half-width glyphs (one byte per row, MSB = leftmost).
fn draw_text_screen(buf: &mut [u32], buf_w: usize, buf_h: usize, sx: i32, sy: i32, text: &str, color: u32) {
    const CHAR_W: i32 = 8;
    for (i, ch) in text.chars().enumerate() {
        let cx = sx + i as i32 * CHAR_W;
        let glyph = crate::gui::text_renderer::get_glyph_for_char(ch);
        for (row, &bits) in glyph.iter().enumerate() {
            let py = sy + row as i32;
            if py < 0 || py >= buf_h as i32 { continue; }
            for col in 0..CHAR_W {
                if bits & (0x80 >> col) != 0 {
                    let px = cx + col;
                    if px >= 0 && px < buf_w as i32 {
                        let idx = py as usize * buf_w + px as usize;
                        if idx < buf.len() { buf[idx] = color; }
                    }
                }
            }
        }
    }
}

/// Draw a filled axis-aligned rectangle into a desktop-space pixel buffer.
///
/// Coordinates are in SDL2 screen space (y=0 at top). Clips to buffer bounds.
fn draw_filled_rect(buf: &mut [u32], buf_w: usize,
                    x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
    let buf_h = buf.len() / buf_w.max(1);
    let rx0 = x0.max(0) as usize;
    let ry0 = y0.max(0) as usize;
    let rx1 = (x1.min(buf_w as i32)).max(0) as usize;
    let ry1 = (y1.min(buf_h as i32)).max(0) as usize;
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            buf[y * buf_w + x] = color;
        }
    }
}

/// Off-screen pixel buffer for one PM frame/dialog window.
struct FrameBuffer {
    buffer: Vec<u32>,
    width:  u32,
    height: u32,
}

impl FrameBuffer {
    fn new(width: u32, height: u32) -> Self {
        let n = (width as usize).saturating_mul(height as usize);
        FrameBuffer { buffer: vec![0xFFFFFFFF_u32; n], width, height }
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width  = width;
        self.height = height;
        let n = (width as usize).saturating_mul(height as usize);
        self.buffer = vec![0xFFFFFFFF_u32; n];
    }
}

/// The single SDL2 window that acts as the OS/2 desktop.
struct DesktopCanvas {
    canvas:  Canvas<Window>,
    texture: Texture,
    buffer:  Vec<u32>,
    width:   u32,
    height:  u32,
}

impl DesktopCanvas {
    fn new(video: &sdl2::VideoSubsystem, w: u32, h: u32) -> Self {
        let window = video
            .window("Warpine \u{2014} OS/2 Compatibility Layer", w, h)
            .position_centered()
            .resizable()
            .build()
            .expect("Failed to create desktop SDL2 window");
        let canvas = window
            .into_canvas()
            .software()
            .build()
            .expect("Failed to create desktop canvas");
        let (cw, ch) = canvas.output_size().expect("output_size failed");
        let texture = {
            let tc = canvas.texture_creator();
            let mut t = tc
                .create_texture_streaming(PixelFormatEnum::ARGB8888, cw, ch)
                .expect("Failed to create desktop texture");
            t.set_blend_mode(BlendMode::None);
            t
        };
        let n = (cw as usize).saturating_mul(ch as usize);
        DesktopCanvas { canvas, texture, buffer: vec![DESKTOP_BG; n], width: cw, height: ch }
    }

    /// Recreate the streaming texture after a resize.
    fn resize_texture(&mut self, w: u32, h: u32) {
        self.width  = w;
        self.height = h;
        let tc = self.canvas.texture_creator();
        let mut t = tc
            .create_texture_streaming(PixelFormatEnum::ARGB8888, w, h)
            .expect("Failed to recreate desktop texture");
        t.set_blend_mode(BlendMode::None);
        self.texture = t;
        let n = (w as usize).saturating_mul(h as usize);
        self.buffer = vec![DESKTOP_BG; n];
    }

    /// Upload `buffer` to the texture and present.
    fn present(&mut self) {
        let w = self.width as usize;
        let buf = &self.buffer;
        self.texture.with_lock(None, |data: &mut [u8], pitch: usize| {
            for (y, row) in buf.chunks(w).enumerate() {
                let dst = &mut data[y * pitch..y * pitch + w * 4];
                // Safety: row is &[u32] aligned to 4; dst has exactly row.len()*4 bytes.
                let src: &[u8] = unsafe {
                    std::slice::from_raw_parts(row.as_ptr() as *const u8, row.len() * 4)
                };
                dst.copy_from_slice(src);
            }
        }).expect("desktop texture lock failed");
        self.canvas.copy(&self.texture, None, None).expect("canvas copy failed");
        self.canvas.present();
    }
}

/// Active window-drag state (VDR-D3).
struct DragState {
    /// HWND of the frame being dragged.
    hwnd: u32,
    /// Horizontal offset (SDL2 pixels) from the frame's left edge to the press point.
    anchor_x: i32,
    /// Vertical offset (SDL2 pixels) from the top of the frame in SDL2 coords to the press point.
    anchor_y_sdl: i32,
}

/// Which edge or corner is being resized (VDR-C5).
#[derive(Clone, Copy, PartialEq)]
enum ResizeEdge {
    N, S, E, W, NE, NW, SE, SW,
}

/// Active resize-drag state (VDR-C5).
struct ResizeState {
    hwnd:    u32,
    edge:    ResizeEdge,
    /// SDL2 cursor position at press.
    press_x: i32,
    press_y: i32,
    /// Window geometry at press time (OS/2 coords).
    orig_x: i32, orig_y: i32, orig_cx: i32, orig_cy: i32,
}

/// SDL2-backed Presentation Manager renderer.
///
/// VDR-A: one SDL2 window (the desktop), one `FrameBuffer` per PM frame/dialog.
/// `PresentBuffer` composites all visible frames onto the desktop surface using
/// window positions from `SharedState::window_mgr`.
///
/// Created on the main thread; must stay on the main thread for the duration
/// of `run_pm_loop`.
pub struct Sdl2Renderer {
    video:       sdl2::VideoSubsystem,
    event_pump:  sdl2::EventPump,
    desktop:     DesktopCanvas,
    /// PM handle → off-screen frame buffer.
    frame_buffers: HashMap<u32, FrameBuffer>,
    /// Last seen host clipboard text — used to detect changes between frames.
    cached_clipboard: String,
    /// Active drag operation (VDR-D3); `None` when no drag is in progress.
    drag_state: Option<DragState>,
    /// Active resize operation (VDR-C5); `None` when no resize is in progress.
    resize_state: Option<ResizeState>,
    /// Timestamp of the last full compositor repaint (VDR-B3).
    last_composite: std::time::Instant,
}

impl Sdl2Renderer {
    /// Create an `Sdl2Renderer` from an existing SDL2 context.
    ///
    /// Reads `WARPINE_DESKTOP_W` / `WARPINE_DESKTOP_H` env vars for the desktop
    /// size; defaults to 1024×768.
    ///
    /// `sdl` must outlive this renderer.  Typical usage:
    /// ```rust,ignore
    /// let sdl = sdl2::init().unwrap();
    /// let mut renderer = Sdl2Renderer::new(&sdl);
    /// run_pm_loop(&mut renderer, shared, rx);
    /// ```
    pub fn new(sdl: &sdl2::Sdl) -> Self {
        let dw: u32 = std::env::var("WARPINE_DESKTOP_W")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
        let dh: u32 = std::env::var("WARPINE_DESKTOP_H")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(768);
        let video      = sdl.video().expect("SDL2 video subsystem init failed");
        let event_pump = sdl.event_pump().expect("SDL2 event pump init failed");
        let desktop    = DesktopCanvas::new(&video, dw, dh);
        Sdl2Renderer {
            video,
            event_pump,
            desktop,
            frame_buffers: HashMap::new(),
            cached_clipboard: String::new(),
            drag_state: None,
            resize_state: None,
            last_composite: std::time::Instant::now(),
        }
    }

    /// Return a mutable reference to the `FrameBuffer` for `handle`, allocating
    /// one lazily if it doesn't exist yet (VDR-A5: dialogs skip `CreateWindow`).
    ///
    /// The initial size is read from `OS2Window::cx / cy` in `SharedState`; falls
    /// back to the desktop size if the window is unknown (e.g. a frame window that
    /// hasn't received a `WinSetWindowPos` yet).
    fn get_or_create_fb<'s>(
        frame_buffers: &'s mut HashMap<u32, FrameBuffer>,
        desktop: &DesktopCanvas,
        handle: u32,
        shared: &Arc<SharedState>,
    ) -> Option<&'s mut FrameBuffer> {
        frame_buffers.entry(handle).or_insert_with(|| {
            let (w, h) = {
                let wm = shared.window_mgr.lock_or_recover();
                wm.get_window(handle)
                    .filter(|win| win.cx > 0 && win.cy > 0)
                    .map(|win| {
                        let title_h = if win.frame_flags & FCF_TITLEBAR != 0 { CHROME_TITLE_H } else { 0 };
                        (win.cx as u32, (win.cy - title_h).max(1) as u32)
                    })
                    .unwrap_or((desktop.width, desktop.height))
            };
            FrameBuffer::new(w, h)
        });
        frame_buffers.get_mut(&handle)
    }

    /// Composite all visible PM windows onto `desktop.buffer` and present.
    ///
    /// Z-order (back-to-front) is read from `shared.window_mgr.z_order`.
    /// Each frame is blitted at its OS/2 screen position converted to SDL2 coords:
    ///   `dst_top_y = desktop_height - win.y - win.cy`
    /// After blitting the client content, window chrome (title bar, border) is drawn
    /// directly onto the desktop buffer (VDR-C1, VDR-C2, VDR-E3).
    fn composite_and_present(&mut self, shared: &Arc<SharedState>) {
        let dw = self.desktop.width as usize;
        let dh = self.desktop.height as i32;

        // Fill background.
        self.desktop.buffer.fill(DESKTOP_BG);

        // Snapshot z_order + window state without holding the lock during blit.
        struct FrameSnap {
            hwnd: u32,
            x: i32, y: i32, cx: i32, cy: i32,
            visible: bool,
            frame_flags: u32,
            title: String,
            is_focused: bool,
        }
        let frames: Vec<FrameSnap> = {
            let wm = shared.window_mgr.lock_or_recover();
            let focused = wm.focused_hwnd;
            wm.z_order.iter().filter_map(|&hwnd| {
                wm.get_window(hwnd).map(|w| FrameSnap {
                    hwnd,
                    x: w.x, y: w.y, cx: w.cx, cy: w.cy,
                    visible: w.visible,
                    frame_flags: w.frame_flags,
                    title: w.text.clone(),
                    is_focused: hwnd == focused,
                })
            }).collect()
        };

        for snap in &frames {
            if !snap.visible { continue; }
            let fb = match self.frame_buffers.get(&snap.hwnd) {
                Some(fb) => fb,
                None => continue,
            };

            // Compute chrome heights so client content is offset below the title bar.
            let title_h = if snap.frame_flags & FCF_TITLEBAR != 0 { CHROME_TITLE_H } else { 0 };
            let border_w = if snap.frame_flags & (FCF_BORDER | FCF_DLGBORDER | FCF_SIZEBORDER) != 0 {
                CHROME_BORDER_W
            } else { 0 };

            // OS/2 y=0 is bottom; SDL2 y=0 is top.
            let dst_top_y = dh - snap.y - snap.cy;

            // Blit frame buffer (client content), offset below the title bar.
            let content_top_y = dst_top_y + title_h;
            let blit_h = (fb.height as i32)
                .min(snap.cy - title_h)
                .min(dh - content_top_y.max(0));
            let blit_w = (fb.width as i32)
                .min(snap.cx - border_w * 2)
                .min(dw as i32 - (snap.x + border_w).max(0));
            if blit_h > 0 && blit_w > 0 {
                for py in 0..blit_h as usize {
                    let dy = content_top_y + py as i32;
                    if dy < 0 || dy >= dh { continue; }
                    let src_row_start = py * fb.width as usize;
                    let dst_row_start = dy as usize * dw;
                    for px in 0..blit_w as usize {
                        let dx = snap.x + border_w + px as i32;
                        if dx < 0 || dx >= dw as i32 { continue; }
                        self.desktop.buffer[dst_row_start + dx as usize] =
                            fb.buffer[src_row_start + px];
                    }
                }
            }

            // VDR-C1/C3/C4: draw title bar chrome on the desktop buffer.
            if title_h > 0 {
                let title_color = if snap.is_focused {
                    CHROME_TITLE_ACTIVE   // VDR-E3: navy for focused
                } else {
                    CHROME_TITLE_INACTIVE // VDR-E3: dark gray for others
                };
                let x0 = snap.x.max(0);
                let x1 = (snap.x + snap.cx).min(dw as i32);
                let y0 = dst_top_y.max(0);
                let y1 = (dst_top_y + title_h).min(dh);
                for ty in y0..y1 {
                    for tx in x0..x1 {
                        self.desktop.buffer[ty as usize * dw + tx as usize] = title_color;
                    }
                }
                // VDR-C3: system-menu icon — small gray square on the left.
                let (text_left, text_y) = {
                    let cy = (title_h - 8) / 2; // vertical centre offset for 8px glyphs
                    let mut left = snap.x + 4;
                    if snap.frame_flags & FCF_SYSMENU != 0 {
                        draw_filled_rect(
                            &mut self.desktop.buffer, dw,
                            snap.x + 3, dst_top_y + 3,
                            snap.x + 3 + CHROME_BTN_W, dst_top_y + title_h - 3,
                            0x00C0C0C0,
                        );
                        left = snap.x + 3 + CHROME_BTN_W + 4;
                    }
                    (left, dst_top_y + cy)
                };
                // Title text after the system-menu icon.
                draw_text_screen(
                    &mut self.desktop.buffer, dw, dh as usize,
                    text_left, text_y, &snap.title, CHROME_TEXT_COLOR);
                // VDR-C4: right-side buttons — close, then max (if any), then min (if any).
                let mut btn_x = snap.x + snap.cx - CHROME_BTN_MARGIN - CHROME_BTN_W;
                // Close button (always present when title bar is shown).
                draw_text_screen(
                    &mut self.desktop.buffer, dw, dh as usize,
                    btn_x, text_y, "\u{00D7}", CHROME_TEXT_COLOR);
                btn_x -= CHROME_BTN_W + CHROME_BTN_GAP;
                if snap.frame_flags & FCF_MAXBUTTON != 0 {
                    draw_text_screen(
                        &mut self.desktop.buffer, dw, dh as usize,
                        btn_x, text_y, "^", CHROME_TEXT_COLOR);
                    btn_x -= CHROME_BTN_W + CHROME_BTN_GAP;
                }
                if snap.frame_flags & FCF_MINBUTTON != 0 {
                    draw_text_screen(
                        &mut self.desktop.buffer, dw, dh as usize,
                        btn_x, text_y, "_", CHROME_TEXT_COLOR);
                }
            }

            // VDR-C2: draw frame border.
            if border_w > 0 {
                let bx0 = snap.x.max(0) as usize;
                let bx1 = (snap.x + snap.cx).min(dw as i32) as usize;
                let by0 = dst_top_y.max(0) as usize;
                let by1 = (dst_top_y + snap.cy).min(dh) as usize;
                let bc = CHROME_BORDER_COLOR;
                for bw_i in 0..border_w as usize {
                    // Top edge
                    let ty = by0 + bw_i;
                    if ty < by1 {
                        for tx in bx0..bx1 { self.desktop.buffer[ty * dw + tx] = bc; }
                    }
                    // Bottom edge
                    let by = by1.saturating_sub(1 + bw_i);
                    if by >= by0 {
                        for tx in bx0..bx1 { self.desktop.buffer[by * dw + tx] = bc; }
                    }
                    // Left edge
                    let lx = bx0 + bw_i;
                    if lx < bx1 {
                        for ty in by0..by1 { self.desktop.buffer[ty * dw + lx] = bc; }
                    }
                    // Right edge
                    let rx = bx1.saturating_sub(1 + bw_i);
                    if rx >= bx0 {
                        for ty in by0..by1 { self.desktop.buffer[ty * dw + rx] = bc; }
                    }
                }
            }
        }

        self.desktop.present();
    }

    /// Handle a single SDL2 event, posting OS/2 messages to `shared` as needed.
    /// Returns `false` when the application should exit.
    fn handle_sdl_event(&mut self, event: Event, shared: &Arc<SharedState>) -> bool {
        let dh = self.desktop.height;
        match event {
            Event::Quit { .. } => {
                shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                let focused = shared.window_mgr.lock_or_recover().focused_hwnd;
                if focused != 0 {
                    push_msg(shared, focused, WM_CLOSE, 0, 0);
                }
                return false;
            }
            Event::Window { win_event, .. } => {
                use sdl2::event::WindowEvent;
                match win_event {
                    WindowEvent::Close => {
                        shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                        let focused = shared.window_mgr.lock_or_recover().focused_hwnd;
                        if focused != 0 {
                            push_msg(shared, focused, WM_CLOSE, 0, 0);
                        }
                        return false;
                    }
                    WindowEvent::Resized(w, h) => {
                        let (w, h) = (w as u32, h as u32);
                        self.desktop.resize_texture(w, h);
                        // Sync the focused frame's dimensions.
                        let focused = {
                            let mut wm = shared.window_mgr.lock_or_recover();
                            let focused = wm.focused_hwnd;
                            if focused != 0 {
                                let client = wm.frame_to_client.get(&focused).copied();
                                for hwnd in std::iter::once(focused).chain(client) {
                                    if let Some(win) = wm.get_window_mut(hwnd) {
                                        win.cx = w as i32;
                                        win.cy = h as i32;
                                    }
                                }
                                if let Some(fb) = self.frame_buffers.get_mut(&focused) {
                                    fb.resize(w, h);
                                }
                            }
                            focused
                        };
                        if focused != 0 {
                            let mp2 = (h << 16) | w;
                            push_msg(shared, focused, WM_SIZE, 0, mp2);
                            push_msg(shared, focused, WM_PAINT, 0, 0);
                        }
                    }
                    WindowEvent::Exposed => {
                        self.composite_and_present(shared);
                    }
                    _ => {}
                }
            }
            Event::KeyDown { keycode, scancode, keymod, repeat, .. } => {
                let focused = shared.window_mgr.lock_or_recover().focused_hwnd;
                if focused != 0 {
                    let (mp1, mp2) = build_wm_char(keycode, scancode, keymod, repeat, false);
                    push_msg(shared, focused, WM_CHAR, mp1, mp2);
                }
            }
            Event::KeyUp { keycode, scancode, keymod, .. } => {
                let focused = shared.window_mgr.lock_or_recover().focused_hwnd;
                if focused != 0 {
                    let (mp1, mp2) = build_wm_char(keycode, scancode, keymod, false, true);
                    push_msg(shared, focused, WM_CHAR, mp1, mp2);
                }
            }
            Event::MouseMotion { x, y, .. } => {
                // VDR-C5: if a resize is active, update window geometry.
                if let Some(ref rs) = self.resize_state {
                    let dx = x - rs.press_x;
                    let dy = y - rs.press_y; // dy positive = downward in SDL2
                    let hwnd = rs.hwnd;
                    let (orig_x, orig_y, orig_cx, orig_cy) =
                        (rs.orig_x, rs.orig_y, rs.orig_cx, rs.orig_cy);
                    let edge = rs.edge;
                    const MIN_SZ: i32 = 32;
                    let mut wm = shared.window_mgr.lock_or_recover();
                    if let Some(win) = wm.get_window_mut(hwnd) {
                        match edge {
                            ResizeEdge::E  => { win.cx = (orig_cx + dx).max(MIN_SZ); }
                            ResizeEdge::W  => {
                                let new_cx = (orig_cx - dx).max(MIN_SZ);
                                win.x  = orig_x + (orig_cx - new_cx);
                                win.cx = new_cx;
                            }
                            ResizeEdge::S  => {
                                // SDL2 dy>0 = downward = decreasing OS/2 y
                                let new_cy = (orig_cy - dy).max(MIN_SZ);
                                win.y  = orig_y + (orig_cy - new_cy);
                                win.cy = new_cy;
                            }
                            ResizeEdge::N  => { win.cy = (orig_cy + dy).max(MIN_SZ); }
                            ResizeEdge::SE => {
                                win.cx = (orig_cx + dx).max(MIN_SZ);
                                let new_cy = (orig_cy - dy).max(MIN_SZ);
                                win.y  = orig_y + (orig_cy - new_cy);
                                win.cy = new_cy;
                            }
                            ResizeEdge::SW => {
                                let new_cx = (orig_cx - dx).max(MIN_SZ);
                                win.x  = orig_x + (orig_cx - new_cx);
                                win.cx = new_cx;
                                let new_cy = (orig_cy - dy).max(MIN_SZ);
                                win.y  = orig_y + (orig_cy - new_cy);
                                win.cy = new_cy;
                            }
                            ResizeEdge::NE => {
                                win.cx = (orig_cx + dx).max(MIN_SZ);
                                win.cy = (orig_cy + dy).max(MIN_SZ);
                            }
                            ResizeEdge::NW => {
                                let new_cx = (orig_cx - dx).max(MIN_SZ);
                                win.x  = orig_x + (orig_cx - new_cx);
                                win.cx = new_cx;
                                win.cy = (orig_cy + dy).max(MIN_SZ);
                            }
                        }
                    }
                // VDR-D3: if a drag is active, move the window.
                } else if let Some(ref ds) = self.drag_state {
                    let hwnd = ds.hwnd;
                    let anchor_x = ds.anchor_x;
                    let anchor_y_sdl = ds.anchor_y_sdl;
                    let mut wm = shared.window_mgr.lock_or_recover();
                    if let Some(win) = wm.get_window_mut(hwnd) {
                        let new_top_y_sdl = y - anchor_y_sdl;
                        win.x = x - anchor_x;
                        win.y = dh as i32 - new_top_y_sdl - win.cy;
                        // Clamp so window is never fully off-screen.
                        win.x = win.x.max(-(win.cx - 32));
                        win.y = win.y.max(0);
                    }
                    // Also sync client window position.
                    if let Some(&client) = wm.frame_to_client.get(&hwnd) {
                        let (fx, fy) = wm.get_window(hwnd).map(|w| (w.x, w.y)).unwrap_or((0, 0));
                        if let Some(cw) = wm.get_window_mut(client) {
                            cw.x = fx;
                            cw.y = fy;
                        }
                    }
                } else {
                    let os2_y = (dh as i32 - 1) - y;
                    // VDR-D6: captured window overrides hit-test.
                    let capture = shared.window_mgr.lock_or_recover().capture_hwnd;
                    let target = if capture != 0 {
                        Some(capture)
                    } else {
                        let wm = shared.window_mgr.lock_or_recover();
                        wm.z_hit_test(x, os2_y)
                    };
                    if let Some(hwnd) = target {
                        let (local_x, local_y) = {
                            let wm = shared.window_mgr.lock_or_recover();
                            wm.get_window(hwnd)
                                .map(|w| (x - w.x, os2_y - w.y))
                                .unwrap_or((x, os2_y))
                        };
                        let mp1 = ((local_x as u32) & 0xFFFF) | ((local_y as u32 & 0xFFFF) << 16);
                        push_msg(shared, hwnd, WM_MOUSEMOVE, mp1, 0);
                    }
                }
            }
            Event::MouseButtonDown { mouse_btn, x, y, .. } => {
                let os2_y = (dh as i32 - 1) - y;
                // VDR-D6: captured window overrides hit-test.
                let capture = shared.window_mgr.lock_or_recover().capture_hwnd;
                let target = if capture != 0 {
                    Some(capture)
                } else {
                    let wm = shared.window_mgr.lock_or_recover();
                    wm.z_hit_test(x, os2_y)
                };
                if let Some(hwnd) = target {
                    // VDR-D4: activate window on click; post WM_ACTIVATE to both windows.
                    let prev_focused = {
                        let mut wm = shared.window_mgr.lock_or_recover();
                        let frame = wm.client_to_frame(hwnd);
                        let frame = if wm.frame_to_client.contains_key(&frame) { frame } else { hwnd };
                        let prev = wm.focused_hwnd;
                        if wm.frame_to_client.contains_key(&frame) && prev != frame {
                            wm.focused_hwnd = frame;
                            wm.z_push_top(frame);
                            prev
                        } else { 0 }
                    };
                    if prev_focused != 0 {
                        // Notify old frame it lost focus.
                        push_msg(shared, prev_focused, WM_ACTIVATE, 0, hwnd);
                        // Notify new frame it gained focus (WA_CLICK = 2).
                        let new_focused = shared.window_mgr.lock_or_recover().focused_hwnd;
                        push_msg(shared, new_focused, WM_ACTIVATE, 2, prev_focused);
                    }
                    // VDR-D2/C3/C4: chrome hit-testing (title bar + buttons).
                    let chrome_hit = {
                        let wm = shared.window_mgr.lock_or_recover();
                        if let Some(win) = wm.get_window(hwnd) {
                            if win.frame_flags & FCF_TITLEBAR != 0 {
                                chrome_hit_test(win.x, win.cx, win.frame_flags,
                                                x, os2_y, win.y, win.cy)
                            } else {
                                ChromeHit::None
                            }
                        } else {
                            ChromeHit::None
                        }
                    };
                    match chrome_hit {
                        ChromeHit::Close => {
                            push_msg(shared, hwnd, WM_CLOSE, 0, 0);
                        }
                        ChromeHit::SysMenu => {
                            // VDR-C3: single-click on system menu — treat as title bar drag.
                            let (anchor_x, dst_top_y) = {
                                let wm = shared.window_mgr.lock_or_recover();
                                wm.get_window(hwnd)
                                    .map(|w| (x - w.x, dh as i32 - w.y - w.cy))
                                    .unwrap_or((0, 0))
                            };
                            self.drag_state = Some(DragState {
                                hwnd, anchor_x, anchor_y_sdl: y - dst_top_y,
                            });
                        }
                        ChromeHit::Minimize => {
                            // VDR-C4: post WM_SYSCOMMAND(SC_MINIMIZE) to the client.
                            push_msg(shared, hwnd, WM_SYSCOMMAND, SC_MINIMIZE, 0);
                        }
                        ChromeHit::Maximize => {
                            // VDR-C4: post WM_SYSCOMMAND(SC_MAXIMIZE) to the client.
                            push_msg(shared, hwnd, WM_SYSCOMMAND, SC_MAXIMIZE, 0);
                        }
                        ChromeHit::TitleBar => {
                            // VDR-D3: start drag — record how far into the title bar the user clicked.
                            let (anchor_x, dst_top_y) = {
                                let wm = shared.window_mgr.lock_or_recover();
                                wm.get_window(hwnd)
                                    .map(|w| (x - w.x, dh as i32 - w.y - w.cy))
                                    .unwrap_or((0, 0))
                            };
                            self.drag_state = Some(DragState {
                                hwnd, anchor_x, anchor_y_sdl: y - dst_top_y,
                            });
                        }
                        ChromeHit::None => {
                            // VDR-C5: check resize edge before forwarding to app.
                            let resize_edge = {
                                let wm = shared.window_mgr.lock_or_recover();
                                wm.get_window(hwnd).and_then(|win| {
                                    resize_hit_test(win.x, win.y, win.cx, win.cy,
                                                    win.frame_flags, x, os2_y)
                                })
                            };
                            if let Some(edge) = resize_edge {
                                let (orig_x, orig_y, orig_cx, orig_cy) = {
                                    let wm = shared.window_mgr.lock_or_recover();
                                    wm.get_window(hwnd)
                                        .map(|w| (w.x, w.y, w.cx, w.cy))
                                        .unwrap_or((0, 0, 0, 0))
                                };
                                self.resize_state = Some(ResizeState {
                                    hwnd, edge,
                                    press_x: x, press_y: y,
                                    orig_x, orig_y, orig_cx, orig_cy,
                                });
                            } else {
                            let (local_x, local_y) = {
                                let wm = shared.window_mgr.lock_or_recover();
                                wm.get_window(hwnd)
                                    .map(|w| (x - w.x, os2_y - w.y))
                                    .unwrap_or((x, os2_y))
                            };
                            let mp1 = ((local_x as u32) & 0xFFFF) | ((local_y as u32 & 0xFFFF) << 16);
                            use sdl2::mouse::MouseButton;
                            let msg = match mouse_btn {
                                MouseButton::Left   => Some(WM_BUTTON1DOWN),
                                MouseButton::Right  => Some(WM_BUTTON2DOWN),
                                MouseButton::Middle => Some(WM_BUTTON3DOWN),
                                _ => None,
                            };
                            if let Some(m) = msg { push_msg(shared, hwnd, m, mp1, 0); }
                            }
                        }
                    }
                }
            }
            Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                // VDR-C5: end resize on button release.
                if let Some(rs) = self.resize_state.take() {
                    let hwnd = rs.hwnd;
                    let (new_cx, new_cy) = {
                        let wm = shared.window_mgr.lock_or_recover();
                        wm.get_window(hwnd).map(|w| (w.cx, w.cy)).unwrap_or((rs.orig_cx, rs.orig_cy))
                    };
                    // Resize the FrameBuffer to match the new client-area dimensions.
                    let client_w = new_cx.max(1) as u32;
                    let client_h = new_cy.max(1) as u32;
                    if let Some(fb) = self.frame_buffers.get_mut(&hwnd)
                        && (fb.width != client_w || fb.height != client_h) {
                        fb.resize(client_w, client_h);
                    }
                    // Notify the app so it repaints.
                    let mp2 = ((new_cy as u32) << 16) | (new_cx as u32);
                    push_msg(shared, hwnd, WM_SIZE, 0, mp2);
                    push_msg(shared, hwnd, WM_PAINT, 0, 0);
                    self.composite_and_present(shared);
                    return true;
                }
                // VDR-D3: end drag on button release, regardless of which button.
                if self.drag_state.is_some() {
                    self.drag_state = None;
                    // Trigger one final composite to show the window at its resting position.
                    self.composite_and_present(shared);
                    return true;
                }
                let _ = (x, y); // suppress unused-variable warnings for x/y used above
                let os2_y = (dh as i32 - 1) - y;
                // VDR-D6: captured window overrides hit-test.
                let capture = shared.window_mgr.lock_or_recover().capture_hwnd;
                let target = if capture != 0 {
                    Some(capture)
                } else {
                    let wm = shared.window_mgr.lock_or_recover();
                    wm.z_hit_test(x, os2_y)
                };
                if let Some(hwnd) = target {
                    let (local_x, local_y) = {
                        let wm = shared.window_mgr.lock_or_recover();
                        wm.get_window(hwnd)
                            .map(|w| (x - w.x, os2_y - w.y))
                            .unwrap_or((x, os2_y))
                    };
                    let mp1 = ((local_x as u32) & 0xFFFF) | ((local_y as u32 & 0xFFFF) << 16);
                    use sdl2::mouse::MouseButton;
                    let msg = match mouse_btn {
                        MouseButton::Left   => Some(WM_BUTTON1UP),
                        MouseButton::Right  => Some(WM_BUTTON2UP),
                        MouseButton::Middle => Some(WM_BUTTON3UP),
                        _ => None,
                    };
                    if let Some(m) = msg { push_msg(shared, hwnd, m, mp1, 0); }
                }
            }
            _ => {}
        }
        true
    }
}

/// Map the low 4 bits of a WinMessageBox `flStyle` to an array of SDL2 button
/// descriptors: `(button_id, label, returnkey_default, escapekey_default)`.
fn mb_buttons(style: u32) -> &'static [(i32, &'static str, bool, bool)] {
    use crate::loader::{MBID_OK, MBID_CANCEL, MBID_ABORT, MBID_RETRY, MBID_IGNORE,
                        MBID_YES, MBID_NO, MBID_ENTER,
                        MB_OKCANCEL, MB_RETRYCANCEL, MB_ABORTRETRYIGNORE,
                        MB_YESNO, MB_YESNOCANCEL, MB_CANCEL, MB_ENTER, MB_ENTERCANCEL};
    // (button_id, label, returnkey_default, escapekey_default)
    match style & 0x0F {
        x if x == MB_OKCANCEL         => &[(MBID_OK as i32,     "OK",     true,  false),
                                           (MBID_CANCEL as i32, "Cancel", false, true)],
        x if x == MB_RETRYCANCEL      => &[(MBID_RETRY as i32,  "Retry",  true,  false),
                                           (MBID_CANCEL as i32, "Cancel", false, true)],
        x if x == MB_ABORTRETRYIGNORE => &[(MBID_ABORT as i32,  "Abort",  true,  false),
                                           (MBID_RETRY as i32,  "Retry",  false, false),
                                           (MBID_IGNORE as i32, "Ignore", false, true)],
        x if x == MB_YESNO            => &[(MBID_YES as i32,    "Yes",    true,  false),
                                           (MBID_NO as i32,     "No",     false, true)],
        x if x == MB_YESNOCANCEL      => &[(MBID_YES as i32,    "Yes",    true,  false),
                                           (MBID_NO as i32,     "No",     false, false),
                                           (MBID_CANCEL as i32, "Cancel", false, true)],
        x if x == MB_CANCEL           => &[(MBID_CANCEL as i32, "Cancel", true,  true)],
        x if x == MB_ENTER            => &[(MBID_ENTER as i32,  "Enter",  true,  false)],
        x if x == MB_ENTERCANCEL      => &[(MBID_ENTER as i32,  "Enter",  true,  false),
                                           (MBID_CANCEL as i32, "Cancel", false, true)],
        _                             => &[(MBID_OK as i32,     "OK",     true,  true)], // MB_OK
    }
}

impl PmRenderer for Sdl2Renderer {
    fn handle_message(&mut self, msg: GUIMessage, shared: &Arc<SharedState>) {
        match msg {
            GUIMessage::CreateWindow { handle, .. } => {
                // VDR-A2: allocate a FrameBuffer sized to the window's client area.
                // The compositor blits the FrameBuffer below the title bar chrome, so
                // use the actual window dimensions (not the desktop canvas size).
                let (w, h) = {
                    let wm = shared.window_mgr.lock_or_recover();
                    wm.get_window(handle)
                        .filter(|win| win.cx > 0 && win.cy > 0)
                        .map(|win| {
                            let title_h = if win.frame_flags & FCF_TITLEBAR != 0 { CHROME_TITLE_H } else { 0 };
                            (win.cx as u32, (win.cy - title_h).max(1) as u32)
                        })
                        .unwrap_or((self.desktop.width, self.desktop.height))
                };
                let fb = FrameBuffer::new(w, h);
                self.frame_buffers.insert(handle, fb);
                debug!("[GUI] Allocated FrameBuffer {}x{} for PM handle {}", w, h, handle);
            }
            GUIMessage::ResizeWindow { handle, width, height } => {
                if let Some(fb) = self.frame_buffers.get_mut(&handle) {
                    fb.resize(width, height);
                    debug!("[GUI] Resized FrameBuffer {} to {}x{}", handle, width, height);
                }
            }
            GUIMessage::MoveWindow { .. } | GUIMessage::ShowWindow { .. } => {
                // No-op: position/visibility live in OS2Window; compositor reads from there.
            }
            GUIMessage::DrawBox { handle, x1, y1, x2, y2, color, fill } => {
                if let Some(fb) = Self::get_or_create_fb(
                    &mut self.frame_buffers, &self.desktop, handle, shared) {
                    render_rect_to_buffer(&mut fb.buffer, fb.width, fb.height,
                                         x1, y1, x2, y2, color, fill);
                }
            }
            GUIMessage::DrawLine { handle, x1, y1, x2, y2, color } => {
                if let Some(fb) = Self::get_or_create_fb(
                    &mut self.frame_buffers, &self.desktop, handle, shared) {
                    render_line_to_buffer(&mut fb.buffer, fb.width, fb.height,
                                         x1, y1, x2, y2, color);
                }
            }
            GUIMessage::DrawText { handle, x, y, text, color } => {
                if let Some(fb) = Self::get_or_create_fb(
                    &mut self.frame_buffers, &self.desktop, handle, shared) {
                    render_text_to_buffer(&mut fb.buffer, fb.width, fb.height, x, y, &text, color);
                }
            }
            GUIMessage::ClearBuffer { handle } => {
                if let Some(fb) = Self::get_or_create_fb(
                    &mut self.frame_buffers, &self.desktop, handle, shared) {
                    fb.buffer.fill(0xFFFFFFFF);
                }
            }
            GUIMessage::PresentBuffer { .. } => {
                // VDR-A4: composite all visible frames onto the desktop and present.
                self.composite_and_present(shared);
            }
            GUIMessage::SetClipboardText(text) => {
                let _ = self.video.clipboard().set_clipboard_text(&text);
                self.cached_clipboard = text;
            }
            GUIMessage::SetMouseCapture(hwnd) => {
                // hwnd == 0 releases capture; any other value acquires it.
                // Safety: SDL2 is initialised; SDL_CaptureMouse is thread-safe.
                unsafe {
                    sdl2::sys::SDL_CaptureMouse(if hwnd != 0 {
                        sdl2::sys::SDL_bool::SDL_TRUE
                    } else {
                        sdl2::sys::SDL_bool::SDL_FALSE
                    });
                }
            }
            GUIMessage::ShowMessageBox { caption, text, style, reply_tx } => {
                // Map MB_ICON* bits to an SDL2 flag.
                let flag = match style & 0x00F0 {
                    x if x == crate::loader::MB_ICONHAND        => MessageBoxFlag::ERROR,
                    x if x == crate::loader::MB_ICONQUESTION    => MessageBoxFlag::WARNING,
                    x if x == crate::loader::MB_ICONEXCLAMATION => MessageBoxFlag::WARNING,
                    x if x == crate::loader::MB_ICONASTERISK    => MessageBoxFlag::INFORMATION,
                    _                                            => MessageBoxFlag::INFORMATION,
                };

                let button_defs = mb_buttons(style);
                let buttons: Vec<ButtonData> = button_defs.iter().map(|&(id, label, ret, esc)| {
                    let mut f = MessageBoxButtonFlag::NOTHING;
                    if ret { f |= MessageBoxButtonFlag::RETURNKEY_DEFAULT; }
                    if esc { f |= MessageBoxButtonFlag::ESCAPEKEY_DEFAULT; }
                    ButtonData { flags: f, button_id: id, text: label }
                }).collect();

                let mbid = match show_message_box(flag, &buttons, &caption, &text, None, None) {
                    Ok(sdl2::messagebox::ClickedButton::CustomButton(b)) => b.button_id as u32,
                    // Close button or error → default to first button's id
                    _ => button_defs.first().map(|b| b.0 as u32).unwrap_or(crate::loader::MBID_OK),
                };
                debug!("[GUI] ShowMessageBox '{}' → MBID={}", caption, mbid);
                let _ = reply_tx.send(mbid);
            }
        }
    }

    fn poll_events(&mut self, shared: &Arc<SharedState>) -> bool {
        // Sync the host clipboard into SharedState so WinQueryClipbrdData can read it.
        if let Ok(host_text) = self.video.clipboard().clipboard_text()
            && host_text != self.cached_clipboard {
                self.cached_clipboard = host_text.clone();
                let mut wm = shared.window_mgr.lock_or_recover();
                wm.clipboard_text = host_text;
                wm.clipboard.insert(CF_TEXT, 0); // invalidate stale guest pointer
        }

        let events: Vec<_> = self.event_pump.poll_iter().collect();
        for event in events {
            if !self.handle_sdl_event(event, shared) {
                return false;
            }
        }

        // VDR-B3: periodic 60Hz compositor tick.
        // Recomposite unconditionally every ~16ms so drag animations, focus changes,
        // and any other state updates are visible even without an app PresentBuffer.
        if self.last_composite.elapsed() >= std::time::Duration::from_millis(16) {
            self.composite_and_present(shared);
            self.last_composite = std::time::Instant::now();
        }

        true
    }

    // frame_sleep: use default 8 ms implementation
}

// ── Internal helpers ───────────────────────────────────────────────────────

/// Result of a chrome hit-test on a mouse click.
#[derive(PartialEq)]
enum ChromeHit {
    /// Click is on the close (×) button — send WM_CLOSE.
    Close,
    /// Click is on the system-menu icon (VDR-C3) — send WM_SYSCOMMAND(SC_CLOSE) on double-click;
    /// for single-click, treat as title bar (activate only).
    SysMenu,
    /// Click is on the minimize button (VDR-C4) — send WM_SYSCOMMAND(SC_MINIMIZE).
    Minimize,
    /// Click is on the maximize button (VDR-C4) — send WM_SYSCOMMAND(SC_MAXIMIZE).
    Maximize,
    /// Click is in the title bar but not on any button — activate / drag.
    TitleBar,
    /// Click is in the client area — forward to the app.
    None,
}

/// Perform a chrome hit-test for a click at SDL2 `(x, _)` / OS/2 `os2_y` on `win`.
///
/// Returns the hit target.  `dst_top_y` is the SDL2 y of the top of the frame's
/// title bar.  Call only when the title bar is enabled (`FCF_TITLEBAR`).
fn chrome_hit_test(win_x: i32, win_cx: i32, win_fcf: u32,
                   x: i32, os2_y: i32, win_y: i32, win_cy: i32) -> ChromeHit {
    // Title bar occupies OS/2 y in [win_y + win_cy - CHROME_TITLE_H, win_y + win_cy).
    let title_bot_os2 = win_y + win_cy - CHROME_TITLE_H;
    if os2_y < title_bot_os2 {
        return ChromeHit::None;
    }
    // Right-side buttons: close is rightmost, then max (if present), then min.
    let mut btn_right = win_x + win_cx - CHROME_BTN_MARGIN;
    let close_x0 = btn_right - CHROME_BTN_W;
    if x >= close_x0 && x < btn_right {
        return ChromeHit::Close;
    }
    btn_right = close_x0 - CHROME_BTN_GAP;
    if win_fcf & FCF_MAXBUTTON != 0 {
        let max_x0 = btn_right - CHROME_BTN_W;
        if x >= max_x0 && x < btn_right {
            return ChromeHit::Maximize;
        }
        btn_right = max_x0 - CHROME_BTN_GAP;
    }
    if win_fcf & FCF_MINBUTTON != 0 {
        let min_x0 = btn_right - CHROME_BTN_W;
        if x >= min_x0 && x < btn_right {
            return ChromeHit::Minimize;
        }
    }
    // Left-side: system menu icon.
    if win_fcf & FCF_SYSMENU != 0 {
        let sysmenu_x1 = win_x + 3 + CHROME_BTN_W;
        if x >= win_x + 3 && x < sysmenu_x1 {
            return ChromeHit::SysMenu;
        }
    }
    ChromeHit::TitleBar
}

/// Test if a click at `(x, os2_y)` is on a resize edge of the window (VDR-C5).
///
/// Returns `Some(edge)` when `FCF_SIZEBORDER` is set and the click is within
/// `RESIZE_HIT_W` pixels of an edge.  Does NOT include the title-bar area for
/// N/NE/NW edges — those are handled by the chrome hit-test first.
fn resize_hit_test(win_x: i32, win_y: i32, win_cx: i32, win_cy: i32,
                   win_fcf: u32, x: i32, os2_y: i32) -> Option<ResizeEdge> {
    if win_fcf & FCF_SIZEBORDER == 0 { return None; }
    let near_left   = x   >= win_x            && x   < win_x            + RESIZE_HIT_W;
    let near_right  = x   >= win_x + win_cx - RESIZE_HIT_W && x < win_x + win_cx;
    let near_bottom = os2_y >= win_y            && os2_y < win_y            + RESIZE_HIT_W;
    let near_top    = os2_y >= win_y + win_cy - RESIZE_HIT_W && os2_y < win_y + win_cy;
    match (near_left, near_right, near_bottom, near_top) {
        (true,  false, false, false) => Some(ResizeEdge::W),
        (false, true,  false, false) => Some(ResizeEdge::E),
        (false, false, true,  false) => Some(ResizeEdge::S),
        (false, false, false, true ) => Some(ResizeEdge::N),
        (true,  false, true,  false) => Some(ResizeEdge::SW),
        (false, true,  true,  false) => Some(ResizeEdge::SE),
        (true,  false, false, true ) => Some(ResizeEdge::NW),
        (false, true,  false, true ) => Some(ResizeEdge::NE),
        _ => None,
    }
}

/// Post an OS/2 message to the queue associated with `hwnd`.
///
/// Looks up the client window via `frame_to_client` first; falls back to
/// `hwnd` itself.  Notifies the condvar so `WinGetMsg` wakes up.
pub fn push_msg(shared: &Arc<SharedState>, hwnd: u32, msg: u32, mp1: u32, mp2: u32) {
    let wm = shared.window_mgr.lock_or_recover();
    let target = wm.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
    let hmq = wm.find_hmq_for_hwnd(target).or_else(|| wm.find_hmq_for_hwnd(hwnd));
    if let Some(hmq) = hmq
        && let Some(mq_arc) = wm.get_mq(hmq) {
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(OS2Message {
                hwnd: target, msg, mp1, mp2, time: 0, x: 0, y: 0,
            });
            mq.cond.notify_one();
    }
}

/// Map SDL2 hardware Scancode → IBM PC Set-1 make code (OS/2 scan code field).
///
/// Extended keys (with E0 prefix on real hardware) share the same Set-1 make
/// code in OS/2; the distinction is handled via VK_ virtual-key codes instead.
pub fn sdl_scancode_to_os2(sc: Scancode) -> u8 {
    match sc {
        Scancode::Escape       => 0x01,
        Scancode::Num1         => 0x02,
        Scancode::Num2         => 0x03,
        Scancode::Num3         => 0x04,
        Scancode::Num4         => 0x05,
        Scancode::Num5         => 0x06,
        Scancode::Num6         => 0x07,
        Scancode::Num7         => 0x08,
        Scancode::Num8         => 0x09,
        Scancode::Num9         => 0x0A,
        Scancode::Num0         => 0x0B,
        Scancode::Minus        => 0x0C,
        Scancode::Equals       => 0x0D,
        Scancode::Backspace    => 0x0E,
        Scancode::Tab          => 0x0F,
        Scancode::Q            => 0x10,
        Scancode::W            => 0x11,
        Scancode::E            => 0x12,
        Scancode::R            => 0x13,
        Scancode::T            => 0x14,
        Scancode::Y            => 0x15,
        Scancode::U            => 0x16,
        Scancode::I            => 0x17,
        Scancode::O            => 0x18,
        Scancode::P            => 0x19,
        Scancode::LeftBracket  => 0x1A,
        Scancode::RightBracket => 0x1B,
        Scancode::Return       => 0x1C,
        Scancode::LCtrl        => 0x1D,
        Scancode::RCtrl        => 0x1D, // E0-1D
        Scancode::A            => 0x1E,
        Scancode::S            => 0x1F,
        Scancode::D            => 0x20,
        Scancode::F            => 0x21,
        Scancode::G            => 0x22,
        Scancode::H            => 0x23,
        Scancode::J            => 0x24,
        Scancode::K            => 0x25,
        Scancode::L            => 0x26,
        Scancode::Semicolon    => 0x27,
        Scancode::Apostrophe   => 0x28,
        Scancode::Grave        => 0x29,
        Scancode::LShift       => 0x2A,
        Scancode::Backslash    => 0x2B,
        Scancode::Z            => 0x2C,
        Scancode::X            => 0x2D,
        Scancode::C            => 0x2E,
        Scancode::V            => 0x2F,
        Scancode::B            => 0x30,
        Scancode::N            => 0x31,
        Scancode::M            => 0x32,
        Scancode::Comma        => 0x33,
        Scancode::Period       => 0x34,
        Scancode::Slash        => 0x35,
        Scancode::KpDivide     => 0x35, // E0-35
        Scancode::RShift       => 0x36,
        Scancode::KpMultiply   => 0x37,
        Scancode::PrintScreen  => 0x37, // E0-37
        Scancode::LAlt         => 0x38,
        Scancode::RAlt         => 0x38, // E0-38
        Scancode::Space        => 0x39,
        Scancode::CapsLock     => 0x3A,
        Scancode::F1           => 0x3B,
        Scancode::F2           => 0x3C,
        Scancode::F3           => 0x3D,
        Scancode::F4           => 0x3E,
        Scancode::F5           => 0x3F,
        Scancode::F6           => 0x40,
        Scancode::F7           => 0x41,
        Scancode::F8           => 0x42,
        Scancode::F9           => 0x43,
        Scancode::F10          => 0x44,
        Scancode::NumLockClear => 0x45,
        Scancode::ScrollLock   => 0x46,
        Scancode::Kp7          => 0x47,
        Scancode::Home         => 0x47, // E0-47
        Scancode::Kp8          => 0x48,
        Scancode::Up           => 0x48, // E0-48
        Scancode::Kp9          => 0x49,
        Scancode::PageUp       => 0x49, // E0-49
        Scancode::KpMinus      => 0x4A,
        Scancode::Kp4          => 0x4B,
        Scancode::Left         => 0x4B, // E0-4B
        Scancode::Kp5          => 0x4C,
        Scancode::Kp6          => 0x4D,
        Scancode::Right        => 0x4D, // E0-4D
        Scancode::KpPlus       => 0x4E,
        Scancode::Kp1          => 0x4F,
        Scancode::End          => 0x4F, // E0-4F
        Scancode::Kp2          => 0x50,
        Scancode::Down         => 0x50, // E0-50
        Scancode::Kp3          => 0x51,
        Scancode::PageDown     => 0x51, // E0-51
        Scancode::Kp0          => 0x52,
        Scancode::Insert       => 0x52, // E0-52
        Scancode::KpPeriod     => 0x53,
        Scancode::Delete       => 0x53, // E0-53
        Scancode::KpEnter      => 0x1C, // E0-1C
        Scancode::F11          => 0x57,
        Scancode::F12          => 0x58,
        _                      => 0x00,
    }
}

/// Map an SDL2 Keycode to the OS/2 VK_* virtual key code.
/// Returns 0 for keys that have no VK_ mapping (regular printable characters).
pub fn sdl_keycode_to_vk(kc: Keycode) -> u32 {
    match kc {
        Keycode::Backspace => VK_BACKSPACE,
        Keycode::Tab       => VK_TAB,
        Keycode::Return    => VK_NEWLINE,
        Keycode::KpEnter   => VK_ENTER,
        Keycode::Escape    => VK_ESC,
        Keycode::Space     => VK_SPACE,
        Keycode::PageUp    => VK_PAGEUP,
        Keycode::PageDown  => VK_PAGEDOWN,
        Keycode::End       => VK_END,
        Keycode::Home      => VK_HOME,
        Keycode::Left      => VK_LEFT,
        Keycode::Up        => VK_UP,
        Keycode::Right     => VK_RIGHT,
        Keycode::Down      => VK_DOWN,
        Keycode::Insert    => VK_INSERT,
        Keycode::Delete    => VK_DELETE,
        Keycode::ScrollLock => VK_SCRLLOCK,
        Keycode::NumLockClear => VK_NUMLOCK,
        Keycode::F1        => VK_F1,
        Keycode::F2        => VK_F2,
        Keycode::F3        => VK_F3,
        Keycode::F4        => VK_F4,
        Keycode::F5        => VK_F5,
        Keycode::F6        => VK_F6,
        Keycode::F7        => VK_F7,
        Keycode::F8        => VK_F8,
        Keycode::F9        => VK_F9,
        Keycode::F10       => VK_F10,
        Keycode::F11       => VK_F11,
        Keycode::F12       => VK_F12,
        _                  => 0,
    }
}

/// Build WM_CHAR MP1 and MP2 from SDL2 key event fields.
///
/// MP1 encoding (per OS/2 PM Reference):
///   bits  7–0  : cRepeat (1 for normal press, 0 for auto-repeat we treat separately)
///   bits 15–8  : hardware scan code (IBM PC Set-1)
///   bits 31–16 : KC_* flags
///
/// MP2 encoding:
///   bits 15–0  : usCh (character code, 0 for pure virtual keys)
///   bits 31–16 : usVKey (VK_* code, 0 for pure character keys)
fn build_wm_char(
    keycode:  Option<Keycode>,
    scancode: Option<Scancode>,
    keymod:   KeyMod,
    repeat:   bool,
    key_up:   bool,
) -> (u32, u32) {
    let sc  = scancode.map(sdl_scancode_to_os2).unwrap_or(0);
    let vk  = keycode.map(sdl_keycode_to_vk).unwrap_or(0);
    let ch  = keycode.map(sdl_keycode_to_char).unwrap_or(0);

    // Modifier state → KC_* flags
    let mut flags = KC_SCANCODE;
    if keymod.intersects(KeyMod::LSHIFTMOD | KeyMod::RSHIFTMOD) { flags |= KC_SHIFT; }
    if keymod.intersects(KeyMod::LCTRLMOD  | KeyMod::RCTRLMOD)  { flags |= KC_CTRL;  }
    if keymod.intersects(KeyMod::LALTMOD   | KeyMod::RALTMOD)   { flags |= KC_ALT;   }
    if key_up  { flags |= KC_KEYUP;   }
    if repeat  { flags |= KC_PREVDOWN; }

    if vk != 0 { flags |= KC_VIRTUALKEY; }
    if ch != 0 { flags |= KC_CHAR;       }

    let mp1 = (flags << 16) | ((sc as u32) << 8) | 1;
    let mp2 = ch | (vk << 16);
    (mp1, mp2)
}

/// Map an SDL2 Keycode to the unshifted OS/2 character code.
fn sdl_keycode_to_char(kc: Keycode) -> u32 {
    let name = kc.name();
    let b = name.as_bytes();
    if b.len() == 1 && b[0].is_ascii() {
        // Letter keys arrive as uppercase names; return lowercase (unshifted).
        let c = if b[0].is_ascii_uppercase() { b[0] + 32 } else { b[0] };
        return c as u32;
    }
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

#[cfg(test)]
mod tests {
    use super::{sdl_keycode_to_char, sdl_keycode_to_vk, sdl_scancode_to_os2, build_wm_char, mb_buttons};
    use sdl2::keyboard::{Keycode, Mod as KeyMod, Scancode};
    use crate::loader::{MBID_OK, MBID_CANCEL, MBID_RETRY, MBID_YES, MBID_NO, MBID_ENTER,
                        MB_OK, MB_OKCANCEL, MB_RETRYCANCEL, MB_YESNO, MB_ENTERCANCEL};

    #[test]
    fn mb_buttons_ok_has_one_entry_with_mbid_ok() {
        let btns = mb_buttons(MB_OK);
        assert_eq!(btns.len(), 1);
        assert_eq!(btns[0].0, MBID_OK as i32);
        assert!(btns[0].2, "OK should be returnkey default");
    }

    #[test]
    fn mb_buttons_ok_cancel_has_two_entries() {
        let btns = mb_buttons(MB_OKCANCEL);
        assert_eq!(btns.len(), 2);
        assert_eq!(btns[0].0, MBID_OK as i32);
        assert_eq!(btns[1].0, MBID_CANCEL as i32);
        assert!(btns[1].3, "Cancel should be escapekey default");
    }

    #[test]
    fn mb_buttons_retry_cancel() {
        let btns = mb_buttons(MB_RETRYCANCEL);
        assert_eq!(btns[0].0, MBID_RETRY as i32);
        assert_eq!(btns[1].0, MBID_CANCEL as i32);
    }

    #[test]
    fn mb_buttons_yes_no() {
        let btns = mb_buttons(MB_YESNO);
        assert_eq!(btns.len(), 2);
        assert_eq!(btns[0].0, MBID_YES as i32);
        assert_eq!(btns[1].0, MBID_NO as i32);
    }

    #[test]
    fn mb_buttons_enter_cancel() {
        let btns = mb_buttons(MB_ENTERCANCEL);
        assert_eq!(btns[0].0, MBID_ENTER as i32);
        assert_eq!(btns[1].0, MBID_CANCEL as i32);
    }

    #[test]
    fn mb_buttons_icon_bits_ignored_in_button_selection() {
        // MB_OKCANCEL | MB_ICONEXCLAMATION
        let btns = mb_buttons(0x0031);
        assert_eq!(btns.len(), 2);
        assert_eq!(btns[0].0, MBID_OK as i32);
    }

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
    fn scancode_alpha_keys_are_set1() {
        assert_eq!(sdl_scancode_to_os2(Scancode::A), 0x1E);
        assert_eq!(sdl_scancode_to_os2(Scancode::Z), 0x2C);
        assert_eq!(sdl_scancode_to_os2(Scancode::Q), 0x10);
    }

    #[test]
    fn scancode_function_keys() {
        assert_eq!(sdl_scancode_to_os2(Scancode::F1),  0x3B);
        assert_eq!(sdl_scancode_to_os2(Scancode::F10), 0x44);
        assert_eq!(sdl_scancode_to_os2(Scancode::F11), 0x57);
        assert_eq!(sdl_scancode_to_os2(Scancode::F12), 0x58);
    }

    #[test]
    fn scancode_extended_nav_keys() {
        assert_eq!(sdl_scancode_to_os2(Scancode::Up),       0x48);
        assert_eq!(sdl_scancode_to_os2(Scancode::Down),     0x50);
        assert_eq!(sdl_scancode_to_os2(Scancode::Left),     0x4B);
        assert_eq!(sdl_scancode_to_os2(Scancode::Right),    0x4D);
        assert_eq!(sdl_scancode_to_os2(Scancode::Home),     0x47);
        assert_eq!(sdl_scancode_to_os2(Scancode::End),      0x4F);
        assert_eq!(sdl_scancode_to_os2(Scancode::PageUp),   0x49);
        assert_eq!(sdl_scancode_to_os2(Scancode::PageDown), 0x51);
        assert_eq!(sdl_scancode_to_os2(Scancode::Insert),   0x52);
        assert_eq!(sdl_scancode_to_os2(Scancode::Delete),   0x53);
    }

    #[test]
    fn vk_mapping_for_special_keys() {
        use crate::loader::*;
        assert_eq!(sdl_keycode_to_vk(Keycode::F1),        VK_F1);
        assert_eq!(sdl_keycode_to_vk(Keycode::F12),       VK_F12);
        assert_eq!(sdl_keycode_to_vk(Keycode::Up),        VK_UP);
        assert_eq!(sdl_keycode_to_vk(Keycode::Return),    VK_NEWLINE);
        assert_eq!(sdl_keycode_to_vk(Keycode::KpEnter),   VK_ENTER);
        assert_eq!(sdl_keycode_to_vk(Keycode::Escape),    VK_ESC);
        assert_eq!(sdl_keycode_to_vk(Keycode::Backspace), VK_BACKSPACE);
    }

    #[test]
    fn vk_mapping_returns_zero_for_printable() {
        assert_eq!(sdl_keycode_to_vk(Keycode::A), 0);
        assert_eq!(sdl_keycode_to_vk(Keycode::Num5), 0);
    }

    #[test]
    fn build_wm_char_regular_key() {
        use crate::loader::*;
        // 'A' key down, no modifiers
        let (mp1, mp2) = build_wm_char(
            Some(Keycode::A), Some(Scancode::A),
            KeyMod::NOMOD, false, false,
        );
        let flags = mp1 >> 16;
        let sc = (mp1 >> 8) & 0xFF;
        let ch = mp2 & 0xFFFF;
        let vk = mp2 >> 16;
        assert!(flags & KC_CHAR != 0,     "KC_CHAR should be set for 'a'");
        assert!(flags & KC_SCANCODE != 0, "KC_SCANCODE always set");
        assert!(flags & KC_VIRTUALKEY == 0, "KC_VIRTUALKEY not set for plain char");
        assert_eq!(sc, 0x1E, "scan code for A is 0x1E");
        assert_eq!(ch, b'a' as u32);
        assert_eq!(vk, 0);
    }

    #[test]
    fn build_wm_char_function_key() {
        use crate::loader::*;
        let (mp1, mp2) = build_wm_char(
            Some(Keycode::F1), Some(Scancode::F1),
            KeyMod::NOMOD, false, false,
        );
        let flags = mp1 >> 16;
        let sc = (mp1 >> 8) & 0xFF;
        let vk = mp2 >> 16;
        assert!(flags & KC_VIRTUALKEY != 0, "KC_VIRTUALKEY set for F1");
        assert!(flags & KC_SCANCODE != 0,   "KC_SCANCODE always set");
        assert!(flags & KC_CHAR == 0,       "KC_CHAR not set for F1");
        assert_eq!(sc, 0x3B, "scan code for F1");
        assert_eq!(vk, VK_F1);
    }

    #[test]
    fn build_wm_char_shift_modifier() {
        use crate::loader::*;
        let (mp1, _) = build_wm_char(
            Some(Keycode::A), Some(Scancode::A),
            KeyMod::LSHIFTMOD, false, false,
        );
        let flags = mp1 >> 16;
        assert!(flags & KC_SHIFT != 0, "KC_SHIFT set when shift held");
    }

    #[test]
    fn build_wm_char_keyup_flag() {
        use crate::loader::*;
        let (mp1, _) = build_wm_char(
            Some(Keycode::A), Some(Scancode::A),
            KeyMod::NOMOD, false, true,
        );
        let flags = mp1 >> 16;
        assert!(flags & KC_KEYUP != 0, "KC_KEYUP set on key release");
    }
}
