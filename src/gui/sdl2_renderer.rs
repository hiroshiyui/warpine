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
    CF_TEXT, KC_PREVDOWN, WM_CHAR_REPEAT_SHIFT, WM_CHAR_SCANCODE_SHIFT, WM_CHAR_VKEY_SHIFT,
    FCF_TITLEBAR, FCF_SYSMENU, FCF_MINBUTTON, FCF_MAXBUTTON,
    FCF_BORDER, FCF_DLGBORDER, FCF_SIZEBORDER,
    WM_ACTIVATE, WM_SYSCOMMAND, SC_MINIMIZE, SC_MAXIMIZE};
use crate::loader::constants::{
    MENU_BAR_HEIGHT, MENU_BAR_ITEM_INSET, MENU_BAR_ITEM_PAD, MENU_DROPDOWN_W, MENU_DROPDOWN_ITEM_H,
    MIS_SEPARATOR};
use crate::loader::pm_types::{MenuItem, menu_label, menu_bar_item_width};
use super::message::GUIMessage;
use super::renderer::PmRenderer;
use super::font::{self, FontSpec};
use super::render_utils::{render_text_to_buffer, render_text_font_to_buffer,
    render_rect_to_buffer, render_line_to_buffer};

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

/// Draw text into a desktop-space pixel buffer.
///
/// `sx`/`sy` are screen coordinates (y=0 at top, x=0 at left) of the top-left
/// corner of the text's line box, which is `spec.px` pixels tall.
#[allow(clippy::too_many_arguments)]
fn draw_text_screen(buf: &mut [u32], buf_w: usize, buf_h: usize,
                    sx: i32, sy: i32, text: &str, color: u32, spec: FontSpec) {
    let baseline = sy + font::metrics(spec).ascent;
    font::draw_text(buf, buf_w, buf_h, sx, baseline, text, color, spec);
}

// ── Frame chrome ─────────────────────────────────────────────────────────────

/// Face colour of raised chrome elements (buttons, sizing border).
const CHROME_FACE_COLOR: u32   = 0x00CC_CCCC;
/// Lit (top/left) edge of a raised bevel.
const CHROME_LIGHT_COLOR: u32  = 0x00FF_FFFF;
/// Shaded (bottom/right) edge of a raised bevel.
const CHROME_SHADOW_COLOR: u32 = 0x0080_8080;
/// Outermost dark edge and button glyphs.
const CHROME_DARK_COLOR: u32   = 0x0000_0000;
/// Vertical inset of the title-bar buttons inside the title bar.
const CHROME_BTN_INSET_Y: i32  = 2;

/// Geometry and state of one frame's chrome, in SDL2 screen space.
struct FrameChrome<'a> {
    x: i32,
    /// Screen y of the frame's top edge.
    top: i32,
    cx: i32,
    cy: i32,
    frame_flags: u32,
    title: &'a str,
    focused: bool,
}

/// Which pictogram a title-bar button carries.
#[derive(Clone, Copy)]
enum ButtonGlyph { Close, Maximize, Minimize, SysMenu }

/// Fill `[x0,x1) × [y0,y1)` with `face` and give it a 1-px raised bevel:
/// `light` along the top and left edges, `shadow` along the bottom and right.
#[allow(clippy::too_many_arguments)]
fn draw_raised_box(buf: &mut [u32], buf_w: usize, x0: i32, y0: i32, x1: i32, y1: i32,
                   face: u32, light: u32, shadow: u32) {
    draw_filled_rect(buf, buf_w, x0, y0, x1, y1, face);
    draw_filled_rect(buf, buf_w, x0, y0, x1, y0 + 1, light);
    draw_filled_rect(buf, buf_w, x0, y0, x0 + 1, y1, light);
    draw_filled_rect(buf, buf_w, x0, y1 - 1, x1, y1, shadow);
    draw_filled_rect(buf, buf_w, x1 - 1, y0, x1, y1, shadow);
}

/// Draw one raised title-bar button with its pictogram. `(x, y)` is the
/// button's top-left corner; it is `CHROME_BTN_W` wide and `h` tall.
fn draw_chrome_button(buf: &mut [u32], buf_w: usize, x: i32, y: i32, h: i32, glyph: ButtonGlyph) {
    let w = CHROME_BTN_W;
    draw_raised_box(buf, buf_w, x, y, x + w, y + h,
                    CHROME_FACE_COLOR, CHROME_LIGHT_COLOR, CHROME_SHADOW_COLOR);
    // Pictogram box: the button interior minus a 4-px margin.
    let (gx0, gy0, gx1, gy1) = (x + 4, y + 4, x + w - 4, y + h - 4);
    let ink = CHROME_DARK_COLOR;
    match glyph {
        ButtonGlyph::Close => {
            // Two 2-px-wide diagonals.
            let n = (gx1 - gx0).min(gy1 - gy0);
            for i in 0..n {
                draw_filled_rect(buf, buf_w, gx0 + i, gy0 + i, gx0 + i + 2, gy0 + i + 1, ink);
                draw_filled_rect(buf, buf_w, gx1 - i - 2, gy0 + i, gx1 - i, gy0 + i + 1, ink);
            }
        }
        ButtonGlyph::Maximize => {
            // Window outline with a thick top edge.
            draw_filled_rect(buf, buf_w, gx0, gy0, gx1, gy0 + 2, ink);
            draw_filled_rect(buf, buf_w, gx0, gy1 - 1, gx1, gy1, ink);
            draw_filled_rect(buf, buf_w, gx0, gy0, gx0 + 1, gy1, ink);
            draw_filled_rect(buf, buf_w, gx1 - 1, gy0, gx1, gy1, ink);
        }
        ButtonGlyph::Minimize => {
            // Bar along the bottom.
            draw_filled_rect(buf, buf_w, gx0, gy1 - 2, gx1, gy1, ink);
        }
        ButtonGlyph::SysMenu => {
            // Small raised plate with a horizontal slot.
            let my = (gy0 + gy1) / 2;
            draw_raised_box(buf, buf_w, gx0 - 1, my - 2, gx1 + 1, my + 2,
                            CHROME_LIGHT_COLOR, CHROME_DARK_COLOR, CHROME_DARK_COLOR);
        }
    }
}

/// Draw a frame's title bar (with system-menu, minimise, maximise and close
/// buttons) and border into the desktop buffer.
///
/// Button positions mirror `hit_test_chrome`: the close button sits
/// `CHROME_BTN_MARGIN` from the right edge, then maximise and minimise to its
/// left with `CHROME_BTN_GAP` between them; the system menu is 3 px from the
/// left edge.
fn draw_frame_chrome(buf: &mut [u32], buf_w: usize, c: &FrameChrome) {
    let buf_h = buf.len() / buf_w.max(1);
    let title_h = if c.frame_flags & FCF_TITLEBAR != 0 { CHROME_TITLE_H } else { 0 };

    if title_h > 0 {
        let title_color = if c.focused { CHROME_TITLE_ACTIVE } else { CHROME_TITLE_INACTIVE };
        draw_filled_rect(buf, buf_w, c.x, c.top, c.x + c.cx, c.top + title_h, title_color);
        // Hairline separating the title bar from the menu bar / client below.
        draw_filled_rect(buf, buf_w, c.x, c.top + title_h - 1, c.x + c.cx, c.top + title_h,
                         CHROME_SHADOW_COLOR);

        let btn_y = c.top + CHROME_BTN_INSET_Y;
        let btn_h = title_h - 2 * CHROME_BTN_INSET_Y - 1;
        let mut text_left = c.x + 4;
        if c.frame_flags & FCF_SYSMENU != 0 {
            draw_chrome_button(buf, buf_w, c.x + 3, btn_y, btn_h, ButtonGlyph::SysMenu);
            text_left = c.x + 3 + CHROME_BTN_W + 4;
        }

        // Right-side buttons: close, then maximise and minimise when present.
        let mut btn_x = c.x + c.cx - CHROME_BTN_MARGIN - CHROME_BTN_W;
        draw_chrome_button(buf, buf_w, btn_x, btn_y, btn_h, ButtonGlyph::Close);
        if c.frame_flags & FCF_MAXBUTTON != 0 {
            btn_x -= CHROME_BTN_W + CHROME_BTN_GAP;
            draw_chrome_button(buf, buf_w, btn_x, btn_y, btn_h, ButtonGlyph::Maximize);
        }
        if c.frame_flags & FCF_MINBUTTON != 0 {
            btn_x -= CHROME_BTN_W + CHROME_BTN_GAP;
            draw_chrome_button(buf, buf_w, btn_x, btn_y, btn_h, ButtonGlyph::Minimize);
        }

        // Title text, clipped so it never runs under the buttons.
        let text_y = c.top + (title_h - GLYPH_H) / 2;
        let avail = btn_x - 4 - text_left;
        let mut title: String = c.title.to_string();
        while !title.is_empty() && font::text_width(&title, FontSpec::SYSTEM_BOLD) > avail {
            title.pop();
        }
        draw_text_screen(buf, buf_w, buf_h, text_left, text_y, &title,
                         CHROME_TEXT_COLOR, FontSpec::SYSTEM_BOLD);
    }

    // Border: a sizing border is a raised bevel; a plain or dialog border is a
    // dark outline with a face-coloured inner ring.
    let has_border = c.frame_flags & (FCF_BORDER | FCF_DLGBORDER | FCF_SIZEBORDER) != 0;
    if has_border {
        let (outer_lt, outer_rb) = if c.frame_flags & FCF_SIZEBORDER != 0 {
            (CHROME_LIGHT_COLOR, CHROME_SHADOW_COLOR)
        } else {
            (CHROME_SHADOW_COLOR, CHROME_SHADOW_COLOR)
        };
        let rings = [(outer_lt, outer_rb), (CHROME_FACE_COLOR, CHROME_FACE_COLOR)];
        for (i, &(lt, rb)) in rings.iter().enumerate().take(CHROME_BORDER_W as usize) {
            let i = i as i32;
            let (x0, y0, x1, y1) = (c.x + i, c.top + i, c.x + c.cx - i, c.top + c.cy - i);
            draw_filled_rect(buf, buf_w, x0, y0, x1, y0 + 1, lt);
            draw_filled_rect(buf, buf_w, x0, y0, x0 + 1, y1, lt);
            draw_filled_rect(buf, buf_w, x0, y1 - 1, x1, y1, rb);
            draw_filled_rect(buf, buf_w, x1 - 1, y0, x1, y1, rb);
        }
    }
}

/// Line height of the PM system font (pixels).
const GLYPH_H: i32 = font::SYSTEM_FONT_PX as i32;
const MENU_BG_COLOR: u32        = 0x00D4_D0C8;
const MENU_EDGE_COLOR: u32      = 0x0080_8080;
const MENU_TEXT_COLOR: u32      = 0x0000_0000;
const MENU_HILITE_COLOR: u32    = 0x0000_0080;
const MENU_HILITE_TEXT: u32     = 0x00FF_FFFF;

/// Draw a frame's menu bar and, when `open_item` (1-based) is non-zero, the
/// pull-down of that top-level item, into a desktop-space pixel buffer.
///
/// `(x0, bar_top)` is the bar's top-left corner in SDL2 screen space and
/// `bar_w` its width. Item geometry mirrors the `#Menu` hit-testing in
/// `Loader::dispatch_builtin_control` (same insets, widths and row height).
fn draw_menu_chrome(buf: &mut [u32], buf_w: usize, x0: i32, bar_top: i32, bar_w: i32,
                    items: &[MenuItem], open_item: u32) {
    let buf_h = buf.len() / buf_w.max(1);
    let bar_h = MENU_BAR_HEIGHT as i32;
    let bar_bot = bar_top + bar_h;
    draw_filled_rect(buf, buf_w, x0, bar_top, x0 + bar_w, bar_bot, MENU_BG_COLOR);
    draw_filled_rect(buf, buf_w, x0, bar_bot - 1, x0 + bar_w, bar_bot, MENU_EDGE_COLOR);

    let text_y = bar_top + (bar_h - GLYPH_H) / 2;
    let mut item_x = x0 + MENU_BAR_ITEM_INSET;
    let mut open_x = None;
    for (i, item) in items.iter().enumerate() {
        if item.style & MIS_SEPARATOR != 0 { continue; }
        let label_w = menu_bar_item_width(&item.text);
        let is_open = open_item > 0 && i == (open_item - 1) as usize;
        if is_open {
            draw_filled_rect(buf, buf_w, item_x, bar_top, item_x + label_w, bar_bot, MENU_HILITE_COLOR);
            open_x = Some(item_x);
        }
        draw_text_screen(buf, buf_w, buf_h, item_x + MENU_BAR_ITEM_PAD / 2, text_y,
                         &menu_label(&item.text),
                         if is_open { MENU_HILITE_TEXT } else { MENU_TEXT_COLOR }, FontSpec::SYSTEM);
        item_x += label_w;
    }

    let (Some(drop_x), Some(parent)) =
        (open_x, items.get(open_item.wrapping_sub(1) as usize)) else { return };
    let drop_h = parent.children.len() as i32 * MENU_DROPDOWN_ITEM_H;
    let (dx1, dy1) = (drop_x + MENU_DROPDOWN_W, bar_bot + drop_h);
    draw_filled_rect(buf, buf_w, drop_x, bar_bot, dx1, dy1, MENU_EDGE_COLOR);
    draw_filled_rect(buf, buf_w, drop_x + 1, bar_bot, dx1 - 1, dy1 - 1, MENU_BG_COLOR);
    for (ci, child) in parent.children.iter().enumerate() {
        let row_top = bar_bot + ci as i32 * MENU_DROPDOWN_ITEM_H;
        if child.style & MIS_SEPARATOR != 0 {
            let sep_y = row_top + MENU_DROPDOWN_ITEM_H / 2;
            draw_filled_rect(buf, buf_w, drop_x + 2, sep_y, dx1 - 2, sep_y + 1, MENU_EDGE_COLOR);
        } else {
            draw_text_screen(buf, buf_w, buf_h, drop_x + 4, row_top,
                             &menu_label(&child.text), MENU_TEXT_COLOR, FontSpec::SYSTEM);
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
        let (dw, dh) = crate::loader::constants::desktop_size();
        let video      = sdl.video().expect("SDL2 video subsystem init failed");
        let event_pump = sdl.event_pump().expect("SDL2 event pump init failed");
        let desktop    = DesktopCanvas::new(&video, dw, dh);
        // Typed characters are taken from SDL TextInput events (see
        // `build_wm_char`); make sure they are being generated.
        video.text_input().start();
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
                    .map(|win| (win.cx as u32, (win.cy - wm.frame_chrome_top_h(handle)).max(1) as u32))
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
        // VDR-B4: skip the composite entirely if nothing has changed and no
        // interactive operation (drag/resize) is in progress.
        {
            let wm = shared.window_mgr.lock_or_recover();
            if wm.dirty.is_empty() && self.drag_state.is_none() && self.resize_state.is_none() {
                return;
            }
        }

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
            /// Attached menu bar: top-level items and the 1-based open item (0 = closed).
            menu: Option<(Vec<MenuItem>, u32)>,
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
                    menu: wm.get_window(w.menu_hwnd).filter(|_| w.menu_hwnd != 0).map(|m| {
                        (m.menu_items.clone(), m.window_ulong.get(&-1).copied().unwrap_or(0))
                    }),
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

            // Blit frame buffer (client content), offset below the title bar and menu bar.
            let menu_h = if snap.menu.is_some() { MENU_BAR_HEIGHT as i32 } else { 0 };
            let content_top_y = dst_top_y + title_h + menu_h;
            let blit_h = (fb.height as i32)
                .min(snap.cy - title_h - menu_h)
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

            // VDR-C1–C4: title bar, system menu, min/max/close buttons and border.
            draw_frame_chrome(&mut self.desktop.buffer, dw, &FrameChrome {
                x: snap.x, top: dst_top_y, cx: snap.cx, cy: snap.cy,
                frame_flags: snap.frame_flags, title: &snap.title, focused: snap.is_focused,
            });

            // Menu bar + open pull-down, drawn last so the pull-down overlays the
            // client content without ever being written into its FrameBuffer.
            if let Some((items, open_item)) = &snap.menu {
                draw_menu_chrome(
                    &mut self.desktop.buffer, dw,
                    snap.x + border_w, dst_top_y + title_h, snap.cx - border_w * 2,
                    items, *open_item);
            }
        }

        self.desktop.present();

        // VDR-B4: clear dirty set — everything has now been composited.
        shared.window_mgr.lock_or_recover().dirty.clear();
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
            // Keyboard input goes to the PM focus window (a control that was
            // clicked or given the focus with WinSetFocus), else to the
            // active frame's client.
            Event::KeyDown { keycode, scancode, keymod, repeat, .. } => {
                let target = shared.window_mgr.lock_or_recover().key_target();
                if target != 0 {
                    let (mp1, mp2) = build_wm_char(keycode, scancode, keymod, repeat, false);
                    push_msg(shared, target, WM_CHAR, mp1, mp2);
                }
            }
            Event::KeyUp { keycode, scancode, keymod, .. } => {
                let target = shared.window_mgr.lock_or_recover().key_target();
                if target != 0 {
                    let (mp1, mp2) = build_wm_char(keycode, scancode, keymod, false, true);
                    push_msg(shared, target, WM_CHAR, mp1, mp2);
                }
            }
            // Typed text: the final characters after keyboard layout, Shift,
            // dead keys and input-method composition. KeyDown above carries no
            // printable character, so nothing is delivered twice.
            Event::TextInput { text, .. } => {
                let target = shared.window_mgr.lock_or_recover().key_target();
                if target != 0 {
                    let cp = shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
                    for (mp1, mp2) in text_input_wm_chars(&text, cp) {
                        push_msg(shared, target, WM_CHAR, mp1, mp2);
                    }
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
                    wm.mark_dirty(hwnd); // VDR-B4
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
                    wm.mark_dirty(hwnd); // VDR-B4
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
                            use sdl2::mouse::MouseButton;
                            let msg = match mouse_btn {
                                MouseButton::Left   => Some(WM_BUTTON1DOWN),
                                MouseButton::Right  => Some(WM_BUTTON2DOWN),
                                MouseButton::Middle => Some(WM_BUTTON3DOWN),
                                _ => None,
                            };
                            // Button 1 goes to the built-in control under the
                            // pointer, if any; a click elsewhere takes the
                            // keyboard focus away from the focused control.
                            let (hwnd, local_x, local_y) = if mouse_btn == MouseButton::Left {
                                let (target, lost_focus) = route_button1(shared, hwnd, local_x, local_y, true);
                                if lost_focus != 0 {
                                    // Repaint the control without its caret.
                                    push_msg(shared, lost_focus, WM_PAINT, 0, 0);
                                }
                                target
                            } else {
                                (hwnd, local_x, local_y)
                            };
                            let mp1 = ((local_x as u32) & 0xFFFF) | ((local_y as u32 & 0xFFFF) << 16);
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
                    // Re-lay out the menu bar and client for the new frame size; the
                    // FrameBuffer and WM_SIZE both describe the client area.
                    let (new_cx, new_cy) = shared.window_mgr.lock_or_recover().layout_frame(hwnd);
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
                    use sdl2::mouse::MouseButton;
                    let (hwnd, local_x, local_y) = if mouse_btn == MouseButton::Left && capture == 0 {
                        route_button1(shared, hwnd, local_x, local_y, false).0
                    } else {
                        (hwnd, local_x, local_y)
                    };
                    let mp1 = ((local_x as u32) & 0xFFFF) | ((local_y as u32 & 0xFFFF) << 16);
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
                        .map(|win| (win.cx as u32, (win.cy - wm.frame_chrome_top_h(handle)).max(1) as u32))
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
            GUIMessage::DrawTextFont { handle, x, y, text, color, font } => {
                if let Some(fb) = Self::get_or_create_fb(
                    &mut self.frame_buffers, &self.desktop, handle, shared) {
                    render_text_font_to_buffer(&mut fb.buffer, fb.width, fb.height,
                                               x, y, &text, color, font);
                }
            }
            GUIMessage::ClearBuffer { handle } => {
                if let Some(fb) = Self::get_or_create_fb(
                    &mut self.frame_buffers, &self.desktop, handle, shared) {
                    fb.buffer.fill(0xFFFFFFFF);
                }
            }
            GUIMessage::PresentBuffer { handle } => {
                // VDR-A4: composite all visible frames onto the desktop and present.
                // VDR-B4: mark the frame dirty so composite_and_present doesn't skip.
                shared.window_mgr.lock_or_recover().mark_dirty(handle);
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

/// Width of the frame border that offsets the client area horizontally
/// inside a frame created with `frame_flags`.
fn frame_border_w(frame_flags: u32) -> i32 {
    if frame_flags & (FCF_BORDER | FCF_DLGBORDER | FCF_SIZEBORDER) != 0 { CHROME_BORDER_W } else { 0 }
}

/// Decide which window receives a mouse-button-1 message aimed at `frame`.
///
/// `(x, y)` is the pointer relative to the frame's bottom-left corner. If a
/// built-in control lies under the pointer the message goes to that control,
/// with the position relative to the control; otherwise it goes to the frame
/// (whose client `push_msg` resolves) unchanged.
///
/// With `press` set, a click that is not on a control ends that frame's
/// control focus: the second return value is the control that lost the focus
/// (0 if none) so the caller can have it repainted without its caret.
fn route_button1(shared: &Arc<SharedState>, frame: u32, x: i32, y: i32, press: bool)
    -> ((u32, i32, i32), u32)
{
    let mut wm = shared.window_mgr.lock_or_recover();
    // The client's buffer starts to the right of the frame's left border.
    let border = wm.get_window(frame).map_or(0, |w| frame_border_w(w.frame_flags));
    if let Some(hit) = wm.control_at(frame, x - border, y) {
        return (hit, 0);
    }
    let focus = wm.focus_hwnd;
    let lost = if press && focus != 0 && wm.frame_to_client.contains_key(&frame)
        && wm.find_frame_for_hwnd(focus) == frame
    {
        wm.focus_hwnd = 0;
        focus
    } else {
        0
    };
    ((frame, x, y), lost)
}

/// WM_CHAR `(mp1, mp2)` pairs for text typed or committed by the input
/// method (an SDL `TextInput` event).
///
/// Each character is encoded in the guest codepage `cp` and sent as one
/// character message: a single byte, or for a double-byte character the lead
/// byte in the low byte and the trail byte in the high byte of the character
/// word. Control characters and characters the codepage cannot represent are
/// dropped.
fn text_input_wm_chars(text: &str, cp: u32) -> Vec<(u32, u32)> {
    text.chars().filter(|ch| !ch.is_control()).filter_map(|ch| {
        let mut utf8 = [0u8; 4];
        let s = ch.encode_utf8(&mut utf8);
        let chr = match crate::loader::codepage::cp_encode(s, cp)[..] {
            // `cp_encode` substitutes '?' for unrepresentable characters.
            [b] if b != b'?' || ch == '?' => b as u32,
            [lead, trail] => lead as u32 | ((trail as u32) << 8),
            _ => return None,
        };
        Some((KC_CHAR | (1 << WM_CHAR_REPEAT_SHIFT), chr))
    }).collect()
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
/// Packing follows CHRMSG in `<pmwin.h>`:
///
/// MP1 layout:
///   bits 15–0  : KC_* flags (`fs`)
///   bits 23–16 : repeat count (`cRepeat`, always 1)
///   bits 31–24 : hardware scan code (`scancode`)
///
/// MP2 layout:
///   bits 15–0  : character code (`chr`, 0 if none)
///   bits 31–16 : virtual key code (`vkey`, 0 if none)
///
/// A key event only carries a character when no `TextInput` event will: for
/// control characters (Enter, Tab, Backspace, Esc, Delete) and for Ctrl/Alt
/// chords, which report the key's unshifted character for accelerators.
/// Printable characters are delivered by `text_input_wm_chars` instead, with
/// the keyboard layout and Shift state applied, so they never arrive twice.
fn build_wm_char(
    keycode:  Option<Keycode>,
    scancode: Option<Scancode>,
    keymod:   KeyMod,
    repeat:   bool,
    key_up:   bool,
) -> (u32, u32) {
    let sc  = scancode.map(sdl_scancode_to_os2).unwrap_or(0);
    let vk  = keycode.map(sdl_keycode_to_vk).unwrap_or(0);
    let chord = keymod.intersects(
        KeyMod::LCTRLMOD | KeyMod::RCTRLMOD | KeyMod::LALTMOD | KeyMod::RALTMOD);
    let ch  = keycode.map(sdl_keycode_to_char)
        .filter(|&c| chord || !is_text_input_char(c)).unwrap_or(0);

    // Modifier state → KC_* flags
    let mut flags = KC_SCANCODE;
    if keymod.intersects(KeyMod::LSHIFTMOD | KeyMod::RSHIFTMOD) { flags |= KC_SHIFT; }
    if keymod.intersects(KeyMod::LCTRLMOD  | KeyMod::RCTRLMOD)  { flags |= KC_CTRL;  }
    if keymod.intersects(KeyMod::LALTMOD   | KeyMod::RALTMOD)   { flags |= KC_ALT;   }
    if key_up  { flags |= KC_KEYUP;   }
    if repeat  { flags |= KC_PREVDOWN; }

    if vk != 0 { flags |= KC_VIRTUALKEY; }
    if ch != 0 { flags |= KC_CHAR;       }

    let mp1 = flags | (1 << WM_CHAR_REPEAT_SHIFT) | ((sc as u32) << WM_CHAR_SCANCODE_SHIFT);
    let mp2 = ch | (vk << WM_CHAR_VKEY_SHIFT);
    (mp1, mp2)
}

/// Whether a key's character is printable, i.e. SDL reports it through a
/// `TextInput` event rather than only as a key press.
fn is_text_input_char(ch: u32) -> bool {
    const SPACE: u32 = 0x20;
    const DEL: u32 = 0x7F;
    ch >= SPACE && ch != DEL
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
    use super::{text_input_wm_chars, route_button1, CHROME_BORDER_W, KC_CHAR, WM_CHAR_REPEAT_SHIFT,
                SharedState, MutexExt};
    use std::sync::Arc;
    use super::{draw_frame_chrome, FrameChrome, CHROME_TITLE_H, CHROME_BTN_W, CHROME_BTN_GAP,
                CHROME_BTN_MARGIN, CHROME_BTN_INSET_Y, CHROME_TITLE_ACTIVE, CHROME_TITLE_INACTIVE,
                CHROME_LIGHT_COLOR, CHROME_SHADOW_COLOR, CHROME_FACE_COLOR, CHROME_DARK_COLOR,
                FCF_TITLEBAR, FCF_SYSMENU, FCF_MINBUTTON, FCF_MAXBUTTON, FCF_BORDER, FCF_SIZEBORDER};
    use super::{draw_menu_chrome, menu_bar_item_width, MenuItem,
                MENU_BG_COLOR, MENU_EDGE_COLOR, MENU_HILITE_COLOR,
                MENU_BAR_HEIGHT, MENU_BAR_ITEM_INSET, MENU_DROPDOWN_W, MENU_DROPDOWN_ITEM_H, MIS_SEPARATOR};
    use crate::loader::constants::{MIS_TEXT, MIS_SUBMENU};
    use sdl2::keyboard::{Keycode, Mod as KeyMod, Scancode};
    use crate::loader::{MBID_OK, MBID_CANCEL, MBID_RETRY, MBID_YES, MBID_NO, MBID_ENTER,
                        MB_OK, MB_OKCANCEL, MB_RETRYCANCEL, MB_YESNO, MB_ENTERCANCEL};

    fn menu_item(text: &str, style: u16, children: Vec<MenuItem>) -> MenuItem {
        MenuItem { id: 0, style, attr: 0, text: text.to_string(), children }
    }

    /// The bar is drawn at the given screen position; with no open item nothing
    /// is painted below it.
    #[test]
    fn draw_menu_chrome_closed_paints_only_the_bar() {
        const W: usize = 200;
        let mut buf = vec![0xFFFF_FFFFu32; W * 100];
        let items = vec![menu_item("~File", MIS_TEXT | MIS_SUBMENU,
                                   vec![menu_item("~About", MIS_TEXT, vec![])])];
        draw_menu_chrome(&mut buf, W, 0, 10, W as i32, &items, 0);
        let px = |x: usize, y: usize| buf[y * W + x];
        assert_eq!(px(150, 9), 0xFFFF_FFFF, "row above the bar untouched");
        assert_eq!(px(150, 10), MENU_BG_COLOR);
        assert_eq!(px(150, 10 + MENU_BAR_HEIGHT as usize - 1), MENU_EDGE_COLOR, "bottom rule");
        assert_eq!(px(10, 10 + MENU_BAR_HEIGHT as usize), 0xFFFF_FFFF, "no pull-down when closed");
    }

    /// An open item is highlighted and its pull-down appears directly below the
    /// bar, one row per child, with a rule for separators.
    #[test]
    fn draw_menu_chrome_open_paints_highlight_and_pulldown() {
        const W: usize = 200;
        let mut buf = vec![0xFFFF_FFFFu32; W * 100];
        let items = vec![menu_item("~File", MIS_TEXT | MIS_SUBMENU, vec![
            menu_item("~About", MIS_TEXT, vec![]),
            menu_item("", MIS_SEPARATOR, vec![]),
            menu_item("E~xit", MIS_TEXT, vec![]),
        ])];
        draw_menu_chrome(&mut buf, W, 0, 10, W as i32, &items, 1);
        let px = |x: i32, y: i32| buf[y as usize * W + x as usize];
        let bar_bot = 10 + MENU_BAR_HEIGHT as i32;
        // Highlight spans the item's width at the bar's top row.
        assert_eq!(px(MENU_BAR_ITEM_INSET, 10), MENU_HILITE_COLOR);
        assert_eq!(px(MENU_BAR_ITEM_INSET + menu_bar_item_width("File"), 10), MENU_BG_COLOR);
        // Pull-down: 3 rows, left edge is the border, interior is menu background.
        let drop_bot = bar_bot + 3 * MENU_DROPDOWN_ITEM_H;
        assert_eq!(px(MENU_BAR_ITEM_INSET, bar_bot + 1), MENU_EDGE_COLOR);
        assert_eq!(px(MENU_BAR_ITEM_INSET + MENU_DROPDOWN_W - 2, bar_bot + 1), MENU_BG_COLOR);
        assert_eq!(px(MENU_BAR_ITEM_INSET + MENU_DROPDOWN_W, bar_bot + 1), 0xFFFF_FFFF, "right of pull-down");
        assert_eq!(px(MENU_BAR_ITEM_INSET + 10, drop_bot), 0xFFFF_FFFF, "below pull-down");
        // Separator rule in the middle of the second row.
        let sep_y = bar_bot + MENU_DROPDOWN_ITEM_H + MENU_DROPDOWN_ITEM_H / 2;
        assert_eq!(px(MENU_BAR_ITEM_INSET + 60, sep_y), MENU_EDGE_COLOR);
    }

    /// Chrome for a 200×100 frame at (10, 10) in a 240×140 buffer.
    fn chrome_buf(frame_flags: u32, focused: bool) -> Vec<u32> {
        let mut buf = vec![0x00FF_00FFu32; 240 * 140];
        draw_frame_chrome(&mut buf, 240, &FrameChrome {
            x: 10, top: 10, cx: 200, cy: 100, frame_flags, title: "Title", focused,
        });
        buf
    }

    /// The title bar is navy when focused and grey otherwise, and each button
    /// is a raised box (lit top-left corner, shaded bottom-right) exactly where
    /// the hit-test expects it.
    #[test]
    fn frame_chrome_draws_title_bar_and_raised_buttons() {
        let flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_MINBUTTON | FCF_MAXBUTTON;
        let buf = chrome_buf(flags, true);
        let px = |x: i32, y: i32| buf[y as usize * 240 + x as usize];
        // Title bar fill between the system menu and the title text start.
        assert_eq!(px(10 + 3 + CHROME_BTN_W + 1, 12), CHROME_TITLE_ACTIVE);
        assert_eq!(px(100, 10 + CHROME_TITLE_H), 0x00FF_00FF, "nothing drawn below the title bar");
        // Close, maximise, minimise from the right edge; system menu on the left.
        let close_x = 10 + 200 - CHROME_BTN_MARGIN - CHROME_BTN_W;
        let max_x = close_x - CHROME_BTN_W - CHROME_BTN_GAP;
        let min_x = max_x - CHROME_BTN_W - CHROME_BTN_GAP;
        let btn_y = 10 + CHROME_BTN_INSET_Y;
        let btn_bot = 10 + CHROME_TITLE_H - CHROME_BTN_INSET_Y - 1;
        for x in [close_x, max_x, min_x, 10 + 3] {
            assert_eq!(px(x, btn_y), CHROME_LIGHT_COLOR, "lit corner at x={x}");
            assert_eq!(px(x + CHROME_BTN_W - 1, btn_bot - 1), CHROME_SHADOW_COLOR, "shaded corner at x={x}");
            assert_eq!(px(x + 2, btn_y + 2), CHROME_FACE_COLOR, "face at x={x}");
        }
        // Each button carries a dark pictogram.
        for x in [close_x, max_x, min_x] {
            let dark = (0..CHROME_BTN_W).flat_map(|dx| (btn_y..btn_bot).map(move |y| (x + dx, y)))
                .filter(|&(x, y)| px(x, y) == CHROME_DARK_COLOR).count();
            assert!(dark >= 4, "button at x={x} has no pictogram");
        }
        // Inactive frames use the inactive title colour.
        let buf = chrome_buf(flags, false);
        assert_eq!(buf[12 * 240 + (10 + 3 + CHROME_BTN_W + 1) as usize], CHROME_TITLE_INACTIVE);
    }

    /// Without FCF_MINBUTTON / FCF_MAXBUTTON only the close button is drawn,
    /// and a long title is clipped before it.
    #[test]
    fn frame_chrome_omits_absent_buttons_and_clips_title() {
        let mut buf = vec![0u32; 240 * 140];
        draw_frame_chrome(&mut buf, 240, &FrameChrome {
            x: 10, top: 10, cx: 200, cy: 100, frame_flags: FCF_TITLEBAR,
            title: "A very long window title that cannot possibly fit in the bar", focused: true,
        });
        let px = |x: i32, y: i32| buf[y as usize * 240 + x as usize];
        let close_x = 10 + 200 - CHROME_BTN_MARGIN - CHROME_BTN_W;
        let max_x = close_x - CHROME_BTN_W - CHROME_BTN_GAP;
        assert_eq!(px(close_x, 10 + CHROME_BTN_INSET_Y), CHROME_LIGHT_COLOR);
        assert_eq!(px(max_x, 10 + CHROME_BTN_INSET_Y), CHROME_TITLE_ACTIVE, "no maximise button");
        // No title pixels in the 4-px gap before the close button or on it.
        for y in 10..10 + CHROME_TITLE_H - 1 {
            for x in close_x - 4..close_x {
                assert_eq!(px(x, y), CHROME_TITLE_ACTIVE, "title text ran into the buttons at ({x},{y})");
            }
        }
    }

    /// A sizing border is a raised bevel; a plain border is a uniform outline;
    /// no border flag draws nothing at the frame edge.
    #[test]
    fn frame_chrome_border_styles() {
        let edge = |flags: u32| {
            let buf = chrome_buf(flags, true);
            // top-left outer, bottom-right outer, inner ring (left side, mid-height)
            (buf[10 * 240 + 10], buf[109 * 240 + 209], buf[60 * 240 + 11])
        };
        assert_eq!(edge(FCF_SIZEBORDER), (CHROME_LIGHT_COLOR, CHROME_SHADOW_COLOR, CHROME_FACE_COLOR));
        assert_eq!(edge(FCF_BORDER), (CHROME_SHADOW_COLOR, CHROME_SHADOW_COLOR, CHROME_FACE_COLOR));
        assert_eq!(edge(0), (0x00FF_00FF, 0x00FF_00FF, 0x00FF_00FF));
    }

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
        let flags = mp1 & 0xFFFF;
        let sc = mp1 >> WM_CHAR_SCANCODE_SHIFT;
        assert_eq!((mp1 >> WM_CHAR_REPEAT_SHIFT) & 0xFF, 1, "repeat count is 1");
        let ch = mp2 & 0xFFFF;
        let vk = mp2 >> 16;
        assert!(flags & KC_CHAR == 0,     "printable characters come from TextInput");
        assert!(flags & KC_SCANCODE != 0, "KC_SCANCODE always set");
        assert!(flags & KC_VIRTUALKEY == 0, "KC_VIRTUALKEY not set for plain char");
        assert_eq!(sc, 0x1E, "scan code for A is 0x1E");
        assert_eq!(ch, 0);
        assert_eq!(vk, 0);
    }

    #[test]
    fn build_wm_char_function_key() {
        use crate::loader::*;
        let (mp1, mp2) = build_wm_char(
            Some(Keycode::F1), Some(Scancode::F1),
            KeyMod::NOMOD, false, false,
        );
        let flags = mp1 & 0xFFFF;
        let sc = mp1 >> WM_CHAR_SCANCODE_SHIFT;
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
        let (mp1, mp2) = build_wm_char(
            Some(Keycode::A), Some(Scancode::A),
            KeyMod::LSHIFTMOD, false, false,
        );
        let flags = mp1 & 0xFFFF;
        assert!(flags & KC_SHIFT != 0, "KC_SHIFT set when shift held");
        assert_eq!(mp2 & 0xFFFF, 0, "the shifted character comes from TextInput");
    }

    #[test]
    fn build_wm_char_keyup_flag() {
        use crate::loader::*;
        let (mp1, _) = build_wm_char(
            Some(Keycode::A), Some(Scancode::A),
            KeyMod::NOMOD, false, true,
        );
        let flags = mp1 & 0xFFFF;
        assert!(flags & KC_KEYUP != 0, "KC_KEYUP set on key release");
    }

    /// What `build_wm_char` produces is what the built-in edit controls and
    /// `WinTranslateAccel` decode (CHRMSG packing). Printable keys carry no
    /// character on key-down; editing keys and control characters do.
    #[test]
    fn build_wm_char_round_trips_through_the_edit_key_decoder() {
        use crate::loader::pm_edit::{key_from_wm_char, EditKey};
        let key = |kc, sc, km, up| {
            let (mp1, mp2) = build_wm_char(Some(kc), Some(sc), km, false, up);
            key_from_wm_char(mp1, mp2, 437)
        };
        for km in [KeyMod::NOMOD, KeyMod::RSHIFTMOD, KeyMod::CAPSMOD] {
            assert_eq!(key(Keycode::A, Scancode::A, km, false), None);
            assert_eq!(key(Keycode::Num1, Scancode::Num1, km, false), None);
            assert_eq!(key(Keycode::Space, Scancode::Space, km, false), None);
        }
        assert_eq!(key(Keycode::Backspace, Scancode::Backspace, KeyMod::NOMOD, false), Some((EditKey::Backspace, false)));
        assert_eq!(key(Keycode::Delete, Scancode::Delete, KeyMod::NOMOD, false), Some((EditKey::Delete, false)));
        assert_eq!(key(Keycode::Return, Scancode::Return, KeyMod::NOMOD, false), Some((EditKey::Enter, false)));
        assert_eq!(key(Keycode::Home, Scancode::Home, KeyMod::LSHIFTMOD, false), Some((EditKey::Home, false)));
        assert_eq!(key(Keycode::Left, Scancode::Left, KeyMod::NOMOD, true), Some((EditKey::Left, true)));
        assert_eq!(key(Keycode::Tab, Scancode::Tab, KeyMod::NOMOD, false), None);
        assert_eq!(key(Keycode::C, Scancode::C, KeyMod::LCTRLMOD, false), None);
    }

    #[test]
    fn build_wm_char_keeps_characters_that_text_input_will_not_send() {
        use crate::loader::*;
        let chr = |kc, sc, km| {
            let (mp1, mp2) = build_wm_char(Some(kc), Some(sc), km, false, false);
            ((mp1 & KC_CHAR != 0), mp2 & 0xFFFF)
        };
        // Control characters.
        assert_eq!(chr(Keycode::Return, Scancode::Return, KeyMod::NOMOD), (true, b'\r' as u32));
        assert_eq!(chr(Keycode::Tab, Scancode::Tab, KeyMod::NOMOD), (true, b'\t' as u32));
        assert_eq!(chr(Keycode::Backspace, Scancode::Backspace, KeyMod::NOMOD), (true, 8));
        assert_eq!(chr(Keycode::Escape, Scancode::Escape, KeyMod::NOMOD), (true, 27));
        // Ctrl / Alt chords report the key's character for accelerators.
        assert_eq!(chr(Keycode::C, Scancode::C, KeyMod::LCTRLMOD), (true, b'c' as u32));
        assert_eq!(chr(Keycode::F, Scancode::F, KeyMod::RALTMOD), (true, b'f' as u32));
        // Printable keys, shifted or not, do not.
        assert_eq!(chr(Keycode::C, Scancode::C, KeyMod::NOMOD), (false, 0));
        assert_eq!(chr(Keycode::Slash, Scancode::Slash, KeyMod::LSHIFTMOD), (false, 0));
        assert_eq!(chr(Keycode::Space, Scancode::Space, KeyMod::NOMOD), (false, 0));
    }

    #[test]
    fn text_input_becomes_codepage_wm_chars() {
        use crate::loader::pm_edit::{key_from_wm_char, EditKey};
        let decode = |text: &str, cp| text_input_wm_chars(text, cp).into_iter()
            .map(|(mp1, mp2)| match key_from_wm_char(mp1, mp2, cp) {
                Some((EditKey::Char(ch), false)) => ch,
                other => panic!("not a character: {other:?}"),
            }).collect::<String>();
        // Capitals and punctuation arrive exactly as SDL composed them.
        assert_eq!(decode("Hello, World? ~!@", 437), "Hello, World? ~!@");
        // é is 0x82 in CP850; KC_CHAR with a repeat count of one.
        assert_eq!(text_input_wm_chars("é", 850), vec![(KC_CHAR | (1 << WM_CHAR_REPEAT_SHIFT), 0x82)]);
        // 日本 in CP932: lead byte low, trail byte high.
        let msgs = text_input_wm_chars("a日本", 932);
        assert_eq!(msgs.iter().map(|m| m.1).collect::<Vec<_>>(), vec![b'a' as u32, 0xFA93, 0x7B96]);
        assert_eq!(decode("a日本", 932), "a日本");
        // Unrepresentable characters are dropped, not sent as '?'; a real '?'
        // is kept. Control characters never come through as text.
        assert_eq!(decode("日?x", 437), "?x");
        assert!(text_input_wm_chars("\r\n\t\u{8}", 437).is_empty());
        assert!(text_input_wm_chars("", 437).is_empty());
    }

    /// A std window with one entry field; returns `(shared, frame, client, entry)`.
    fn routed_window(frame_flags: u32) -> (Arc<SharedState>, u32, u32, u32) {
        let loader = crate::loader::Loader::new_mock();
        let shared = loader.shared.clone();
        let mut wm = shared.window_mgr.lock_or_recover();
        let frame = wm.create_window("App".into(), 0, 0);
        let client = wm.create_window("App".into(), frame, 0);
        wm.frame_to_client.insert(frame, client);
        wm.get_window_mut(frame).unwrap().frame_flags = frame_flags;
        let entry = wm.create_window("WC_ENTRYFIELD".into(), client, 0);
        let w = wm.get_window_mut(entry).unwrap();
        (w.x, w.y, w.cx, w.cy, w.visible) = (10, 200, 200, 25, true);
        drop(wm);
        (shared, frame, client, entry)
    }

    #[test]
    fn button1_is_routed_to_the_control_under_the_pointer() {
        let (shared, frame, _client, entry) = routed_window(0);
        // On the control: control-relative coordinates, focus untouched here
        // (the control takes the focus when it handles the message).
        assert_eq!(route_button1(&shared, frame, 15, 210, true), ((entry, 5, 10), 0));
        assert_eq!(route_button1(&shared, frame, 15, 210, false), ((entry, 5, 10), 0));
        // Off the control: the frame gets it unchanged.
        assert_eq!(route_button1(&shared, frame, 300, 100, true), ((frame, 300, 100), 0));
    }

    #[test]
    fn button1_accounts_for_the_frame_border() {
        let (shared, frame, _client, entry) = routed_window(FCF_SIZEBORDER);
        // Frame x = border + client x: the control's left edge is at 10 + border.
        let left = 10 + CHROME_BORDER_W;
        assert_eq!(route_button1(&shared, frame, left, 200, true), ((entry, 0, 0), 0));
        assert_eq!(route_button1(&shared, frame, left - 1, 200, true), ((frame, left - 1, 200), 0));
    }

    #[test]
    fn click_outside_controls_ends_control_focus() {
        let (shared, frame, _client, entry) = routed_window(0);
        shared.window_mgr.lock_or_recover().focus_hwnd = entry;
        // Releasing the button elsewhere does not change the focus.
        assert_eq!(route_button1(&shared, frame, 300, 100, false).1, 0);
        assert_eq!(shared.window_mgr.lock_or_recover().focus_hwnd, entry);
        // Pressing it does, and reports the control to repaint.
        assert_eq!(route_button1(&shared, frame, 300, 100, true), ((frame, 300, 100), entry));
        assert_eq!(shared.window_mgr.lock_or_recover().focus_hwnd, 0);
        assert_eq!(route_button1(&shared, frame, 300, 100, true).1, 0);
        // A click in another frame leaves this frame's focus control alone.
        shared.window_mgr.lock_or_recover().focus_hwnd = entry;
        let other = {
            let mut wm = shared.window_mgr.lock_or_recover();
            let f = wm.create_window("App".into(), 0, 0);
            let c = wm.create_window("App".into(), f, 0);
            wm.frame_to_client.insert(f, c);
            f
        };
        assert_eq!(route_button1(&shared, other, 300, 100, true), ((other, 300, 100), 0));
        assert_eq!(shared.window_mgr.lock_or_recover().focus_hwnd, entry);
    }
}
