// SPDX-License-Identifier: GPL-3.0-only

//! Phase 6: VGA Text-Mode Renderer
//!
//! Renders an 80×25 VGA text buffer into an SDL2 window using an 8×16 IBM CP437
//! pixel font.  Keyboard events from SDL2 are pushed into `SharedState::kbd_queue`
//! for consumption by `KbdCharIn`.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::loader::{SharedState, KbdKeyInfo, MutexExt};

// ── CGA 16-colour palette (ARGB8888 = 0xFF_RR_GG_BB) ───────────────────────

/// Standard CGA/EGA 16-colour palette indexed by a 4-bit colour nibble.
///
/// Index = attribute nibble (bits 3:0 for foreground, bits 7:4 for background).
pub const CGA_PALETTE: [u32; 16] = [
    0xFF_00_00_00, // 0  Black
    0xFF_00_00_AA, // 1  Blue
    0xFF_00_AA_00, // 2  Green
    0xFF_00_AA_AA, // 3  Cyan
    0xFF_AA_00_00, // 4  Red
    0xFF_AA_00_AA, // 5  Magenta
    0xFF_AA_55_00, // 6  Brown
    0xFF_AA_AA_AA, // 7  Light Gray
    0xFF_55_55_55, // 8  Dark Gray
    0xFF_55_55_FF, // 9  Bright Blue
    0xFF_55_FF_55, // 10 Bright Green
    0xFF_55_FF_FF, // 11 Bright Cyan
    0xFF_FF_55_55, // 12 Bright Red
    0xFF_FF_55_FF, // 13 Bright Magenta
    0xFF_FF_FF_55, // 14 Yellow
    0xFF_FF_FF_FF, // 15 White
];

// ── CP437 8×16 pixel font ────────────────────────────────────────────────────

/// Return the 8×16 glyph bitmap for CP437 byte `ch`.
///
/// Rows 0-15 from top to bottom; bit 7 of each byte is the leftmost pixel.
/// - Bytes 0x20-0x7E delegate to the existing ASCII font table.
/// - Box-drawing and block-element characters are hand-crafted.
/// - All other bytes return a blank glyph.
pub fn get_cp437_glyph(ch: u8) -> [u8; 16] {
    // ASCII printable range: delegate to the existing 8×16 font
    if ch >= 0x20 && ch <= 0x7E {
        let idx = (ch - 0x20) as usize;
        let base = idx * 16;
        let mut g = [0u8; 16];
        g.copy_from_slice(&crate::font8x16::FONT_8X16[base..base + 16]);
        return g;
    }

    // CP437 special glyphs
    match ch {
        // ── Shade blocks ────────────────────────────────────────────────────
        0xB0 => [0xAA,0x55,0xAA,0x55,0xAA,0x55,0xAA,0x55,
                 0xAA,0x55,0xAA,0x55,0xAA,0x55,0xAA,0x55], // ░ light shade
        0xB1 => [0xFF,0xAA,0xFF,0x55,0xFF,0xAA,0xFF,0x55,
                 0xFF,0xAA,0xFF,0x55,0xFF,0xAA,0xFF,0x55], // ▒ medium shade
        0xB2 => [0xFF,0xFF,0xFF,0xAA,0xFF,0xFF,0xFF,0x55,
                 0xFF,0xFF,0xFF,0xAA,0xFF,0xFF,0xFF,0x55], // ▓ dark shade

        // ── Single-line box drawing ──────────────────────────────────────────
        // Vertical line at col 4 (bit 3 from right)
        0xB3 => [0x08; 16], // │
        // Horizontal line at row 7 (mid-cell)
        0xC4 => { let mut g = [0u8; 16]; g[7] = 0xFF; g } // ─

        // Corners and T-junctions (single-line)
        0xDA => { // ┌ top-left
            let mut g = [0u8; 16];
            g[7] = 0x0F; // right half of horizontal bar
            for r in 8..16 { g[r] = 0x08; } // lower vertical
            g
        }
        0xBF => { // ┐ top-right
            let mut g = [0u8; 16];
            g[7] = 0xF8; // left half of horizontal bar
            for r in 8..16 { g[r] = 0x08; }
            g
        }
        0xC0 => { // └ bottom-left
            let mut g = [0u8; 16];
            for r in 0..7 { g[r] = 0x08; }
            g[7] = 0x0F;
            g
        }
        0xD9 => { // ┘ bottom-right
            let mut g = [0u8; 16];
            for r in 0..7 { g[r] = 0x08; }
            g[7] = 0xF8;
            g
        }
        0xC3 => { // ├ left T
            let mut g = [0x08u8; 16];
            g[7] = 0x0F;
            g
        }
        0xB4 => { // ┤ right T
            let mut g = [0x08u8; 16];
            g[7] = 0xF8;
            g
        }
        0xC2 => { // ┬ top T
            let mut g = [0u8; 16];
            g[7] = 0xFF;
            for r in 8..16 { g[r] = 0x08; }
            g
        }
        0xC1 => { // ┴ bottom T
            let mut g = [0u8; 16];
            for r in 0..7 { g[r] = 0x08; }
            g[7] = 0xFF;
            g
        }
        0xC5 => { // ┼ cross
            let mut g = [0x08u8; 16];
            g[7] = 0xFF;
            g
        }

        // ── Double-line box drawing ──────────────────────────────────────────
        // Double vertical at cols 3 and 5 (0x14 = 0b00010100)
        0xBA => [0x14; 16], // ║
        // Double horizontal at rows 5 and 9
        0xCD => { let mut g = [0u8; 16]; g[5] = 0xFF; g[9] = 0xFF; g } // ═

        0xC9 => { // ╔ double top-left
            let mut g = [0u8; 16];
            g[4] = 0x1F; g[5] = 0x10; g[6] = 0x10;
            g[8] = 0x17;
            for r in 9..16 { g[r] = 0x14; }
            g
        }
        0xBB => { // ╗ double top-right
            let mut g = [0u8; 16];
            g[4] = 0xF8; g[5] = 0x08; g[6] = 0x08;
            g[8] = 0xE8;
            for r in 9..16 { g[r] = 0x14; }
            g
        }
        0xC8 => { // ╚ double bottom-left
            let mut g = [0u8; 16];
            for r in 0..6 { g[r] = 0x14; }
            g[6] = 0x17;
            g[8] = 0x10; g[9] = 0x10; g[10] = 0x1F;
            g
        }
        0xBC => { // ╝ double bottom-right
            let mut g = [0u8; 16];
            for r in 0..6 { g[r] = 0x14; }
            g[6] = 0xE8;
            g[8] = 0x08; g[9] = 0x08; g[10] = 0xF8;
            g
        }
        0xCC => { // ╠ double left T
            let mut g = [0x14u8; 16];
            g[5] = 0x17; g[9] = 0x17;
            g
        }
        0xB9 => { // ╣ double right T
            let mut g = [0x14u8; 16];
            g[5] = 0xF4; g[9] = 0xF4;
            g
        }
        0xCA => { // ╩ double bottom T
            let mut g = [0u8; 16];
            for r in 0..5 { g[r] = 0x14; }
            g[5] = 0xFF; g[9] = 0xFF;
            g
        }
        0xCB => { // ╦ double top T
            let mut g = [0u8; 16];
            g[5] = 0xFF; g[9] = 0xFF;
            for r in 10..16 { g[r] = 0x14; }
            g
        }
        0xCE => { // ╬ double cross
            let mut g = [0x14u8; 16];
            g[5] = 0xFF; g[9] = 0xFF;
            g
        }

        // ── Block elements ───────────────────────────────────────────────────
        0xDB => [0xFF; 16],  // █ full block
        0xDC => { let mut g = [0u8; 16]; for r in 8..16 { g[r] = 0xFF; } g } // ▄ lower half
        0xDF => { let mut g = [0u8; 16]; for r in 0..8  { g[r] = 0xFF; } g } // ▀ upper half
        0xDD => [0xF0; 16],  // ▌ left half
        0xDE => [0x0F; 16],  // ▐ right half

        // ── Additional single-line variant glyphs ────────────────────────────
        0xC6 => { // ╞
            let mut g = [0x08u8; 16];
            g[7] = 0x0F;
            g
        }
        0xC7 => { // ╟
            let mut g = [0x14u8; 16];
            g[7] = 0x1F;
            g
        }
        0xCF => { // ╧
            let mut g = [0u8; 16];
            for r in 0..7 { g[r] = 0x08; }
            g[7] = 0xFF;
            g
        }
        0xD0 => { // ╨
            let mut g = [0u8; 16];
            for r in 0..5 { g[r] = 0x14; }
            g[5] = 0xFF;
            g
        }
        0xD1 => { // ╤
            let mut g = [0u8; 16];
            g[5] = 0xFF;
            for r in 9..16 { g[r] = 0x08; }
            g
        }
        0xD2 => { // ╥
            let mut g = [0u8; 16];
            g[7] = 0xFF;
            for r in 8..16 { g[r] = 0x14; }
            g
        }

        // Everything else: blank
        _ => [0u8; 16],
    }
}

// ── VgaTextBuffer snapshot ───────────────────────────────────────────────────

/// Snapshot of the VioManager state for a single rendered frame.
pub struct VgaTextBuffer {
    pub rows: u16,
    pub cols: u16,
    /// (char, attr) pairs, row-major.  attr: bits 3:0 = fg (with bright), 7:4 = bg.
    pub cells: Vec<(u8, u8)>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    pub cursor_start: u8,
    pub cursor_end: u8,
}

impl VgaTextBuffer {
    /// Snapshot the current VioManager state.
    pub fn snapshot(shared: &SharedState) -> Self {
        let vio = shared.console_mgr.lock_or_recover();
        VgaTextBuffer {
            rows: vio.rows,
            cols: vio.cols,
            cells: vio.buffer.clone(),
            cursor_row: vio.cursor_row,
            cursor_col: vio.cursor_col,
            cursor_visible: vio.cursor_visible,
            cursor_start: vio.cursor_start,
            cursor_end: vio.cursor_end,
        }
    }
}

// ── TextModeRenderer trait ────────────────────────────────────────────────────

pub trait TextModeRenderer {
    /// Render one frame.  `blink_on` toggles the cursor blink phase.
    fn render_frame(&mut self, buf: &VgaTextBuffer, blink_on: bool);
    /// Poll input events; push keyboard info to `shared.kbd_queue`.
    /// Returns `false` to stop the loop (window closed or Ctrl+Esc).
    fn poll_events(&mut self, shared: &Arc<SharedState>) -> bool;
    /// Frame-rate limiter; default ≈60 fps (16 ms).
    fn frame_sleep(&self) {
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

// ── Main text-mode event loop ─────────────────────────────────────────────────

/// Drive a text-mode application's event loop on the main thread.
pub fn run_text_loop<R: TextModeRenderer>(renderer: &mut R, shared: Arc<SharedState>) {
    // Use wall-clock time for cursor blink so it is frame-rate independent.
    // 500 ms on / 500 ms off → 1 Hz blink, matching real VGA behaviour.
    let blink_epoch = std::time::Instant::now();

    loop {
        if shared.exit_requested.load(Ordering::Relaxed) { break; }

        if !renderer.poll_events(&shared) {
            shared.exit_requested.store(true, Ordering::Relaxed);
            shared.kbd_cond.notify_all();
            break;
        }

        if shared.exit_requested.load(Ordering::Relaxed) { break; }

        let blink_on = (blink_epoch.elapsed().as_millis() / 500) % 2 == 0;
        let buf = VgaTextBuffer::snapshot(&shared);
        renderer.render_frame(&buf, blink_on);

        renderer.frame_sleep();
    }
}

// ── SDL2 text-mode renderer ───────────────────────────────────────────────────

/// SDL2-backed VGA text-mode renderer.
///
/// Creates a fixed 640×400 window (80 cols × 8 px, 25 rows × 16 px).
/// Must be created and driven on the main thread.
pub struct Sdl2TextRenderer {
    canvas: sdl2::render::Canvas<sdl2::video::Window>,
    /// Persistent streaming texture — uploaded once per frame.
    texture: sdl2::render::Texture,
    event_pump: sdl2::EventPump,
    /// Pixel framebuffer: WIN_W × WIN_H ARGB8888 pixels.
    pixels: Vec<u32>,
}

impl Sdl2TextRenderer {
    pub const WIN_W: u32 = 80 * 8;   // 640
    pub const WIN_H: u32 = 25 * 16;  // 400

    pub fn new(sdl: &sdl2::Sdl, title: &str) -> Self {
        use sdl2::pixels::PixelFormatEnum;
        use sdl2::render::BlendMode;

        let video = sdl.video().expect("SDL2 video subsystem");
        let window = video
            .window(title, Self::WIN_W, Self::WIN_H)
            .position_centered()
            .build()
            .expect("SDL2 window");
        let canvas = window
            .into_canvas()
            .accelerated()
            .build()
            .expect("SDL2 canvas");
        let tc = canvas.texture_creator();
        let mut texture = tc
            .create_texture_streaming(PixelFormatEnum::ARGB8888, Self::WIN_W, Self::WIN_H)
            .expect("SDL2 streaming texture");
        texture.set_blend_mode(BlendMode::None);
        let event_pump = sdl.event_pump().expect("SDL2 event pump");
        let pixels = vec![0xFF_00_00_00u32; (Self::WIN_W * Self::WIN_H) as usize];
        Sdl2TextRenderer { canvas, texture, event_pump, pixels }
    }

    /// Upload the pixel buffer to the texture and blit to the screen.
    fn present(&mut self) {
        let w = Self::WIN_W as usize;
        let pixels = &self.pixels;
        self.texture.with_lock(None, |data: &mut [u8], pitch: usize| {
            for (y, row) in pixels.chunks(w).enumerate() {
                let dst = &mut data[y * pitch..y * pitch + w * 4];
                // Safety: row is &[u32] aligned to 4; cast to &[u8] with same byte count.
                let src: &[u8] = unsafe {
                    std::slice::from_raw_parts(row.as_ptr() as *const u8, row.len() * 4)
                };
                dst.copy_from_slice(src);
            }
        }).expect("texture lock failed");
        self.canvas.copy(&self.texture, None, None).expect("canvas copy failed");
        self.canvas.present();
    }
}

impl TextModeRenderer for Sdl2TextRenderer {
    fn render_frame(&mut self, buf: &VgaTextBuffer, blink_on: bool) {
        let cols = buf.cols as usize;
        let rows = buf.rows as usize;
        let win_w = Self::WIN_W as usize;
        let win_h = Self::WIN_H as usize;

        for row in 0..rows {
            for col in 0..cols {
                let idx = row * cols + col;
                let (ch, attr) = if idx < buf.cells.len() { buf.cells[idx] } else { (b' ', 0x07) };

                let fg = CGA_PALETTE[(attr & 0x0F) as usize];
                let bg = CGA_PALETTE[((attr >> 4) & 0x0F) as usize];

                let glyph = get_cp437_glyph(ch);
                let base_x = col * 8;
                let base_y = row * 16;

                for gr in 0..16usize {
                    let bits = glyph[gr];
                    let py = base_y + gr;
                    if py >= win_h { break; }
                    for gc in 0..8usize {
                        let px = base_x + gc;
                        if px >= win_w { break; }
                        self.pixels[py * win_w + px] =
                            if bits & (0x80 >> gc) != 0 { fg } else { bg };
                    }
                }

                // Cursor overlay for the cursor cell
                if buf.cursor_visible && blink_on
                    && row as u16 == buf.cursor_row && col as u16 == buf.cursor_col
                {
                    let cstart = buf.cursor_start as usize;
                    let cend   = (buf.cursor_end as usize).min(15);
                    // Guard against inverted/degenerate shapes from guest.
                    let (cstart, cend) = if cstart <= cend { (cstart, cend) } else { (14, 15) };
                    for gr in cstart..=cend {
                        let py = base_y + gr;
                        if py >= win_h { break; }
                        for gc in 0..8usize {
                            let px = base_x + gc;
                            if px >= win_w { break; }
                            // XOR-invert RGB channels: always visible regardless of
                            // cell fg/bg colour (including black-on-black attr=0x00).
                            let old = self.pixels[py * win_w + px];
                            self.pixels[py * win_w + px] = (old ^ 0x00_FF_FF_FF) | 0xFF_00_00_00;
                        }
                    }
                }
            }
        }

        self.present();
    }

    fn poll_events(&mut self, shared: &Arc<SharedState>) -> bool {
        use sdl2::event::Event;
        use sdl2::keyboard::{Keycode, Mod as KeyMod};
        use crate::gui::sdl2_renderer::sdl_scancode_to_os2;

        // Use wait_event_timeout so the thread sleeps inside SDL2 when idle
        // instead of busy-polling with poll_iter().  The 8 ms timeout gives
        // ~125 Hz frame rate (matching the PM renderer) while the OS
        // scheduler can keep this core idle between events.
        let maybe_event = self.event_pump.wait_event_timeout(8);
        let events = maybe_event.into_iter().chain(self.event_pump.poll_iter());

        for event in events {
            match event {
                Event::Quit { .. } => return false,
                Event::KeyDown { keycode: Some(kc), scancode: Some(sc), keymod, repeat: false, .. } => {
                    // Ctrl+Esc → graceful exit
                    if kc == Keycode::Escape
                        && (keymod.contains(KeyMod::LCTRLMOD) || keymod.contains(KeyMod::RCTRLMOD))
                    {
                        shared.exit_requested.store(true, Ordering::Relaxed);
                        shared.kbd_cond.notify_all();
                        return false;
                    }

                    // OS/2 KbdCharIn does not deliver events for pure modifier
                    // keys (Shift, Ctrl, Alt, CapsLock, NumLock, ScrollLock).
                    // Their state is reflected in the `state` field of the next
                    // real keystroke.  Enqueuing them causes the scan code byte
                    // (0x2A / 0x36 / …) to appear as a spurious character.
                    if matches!(kc,
                        Keycode::LShift | Keycode::RShift |
                        Keycode::LCtrl  | Keycode::RCtrl  |
                        Keycode::LAlt   | Keycode::RAlt   |
                        Keycode::CapsLock | Keycode::NumLockClear | Keycode::ScrollLock |
                        Keycode::LGui   | Keycode::RGui
                    ) {
                        continue;
                    }

                    let scan = sdl_scancode_to_os2(sc);
                    let ch   = sdl_keycode_to_text_char(kc, keymod);
                    let mut state: u16 = 0;
                    if keymod.intersects(KeyMod::LSHIFTMOD | KeyMod::RSHIFTMOD) { state |= 0x0008; }
                    if keymod.intersects(KeyMod::LCTRLMOD  | KeyMod::RCTRLMOD)  { state |= 0x0004; }
                    if keymod.intersects(KeyMod::LALTMOD   | KeyMod::RALTMOD)   { state |= 0x0002; }

                    {
                        let mut q = shared.kbd_queue.lock().unwrap();
                        q.push_back(KbdKeyInfo { ch, scan, state });
                    }
                    shared.kbd_cond.notify_one();
                }
                _ => {}
            }
        }
        true
    }

    fn frame_sleep(&self) {
        // No additional sleep needed — wait_event_timeout(8) in poll_events
        // already provides the frame-rate throttle and CPU yield.
    }
}

/// Convert an SDL2 `Keycode` + modifier flags to an OS/2 ASCII character byte.
///
/// Returns 0x00 for pure extended/navigation keys (arrows, F-keys, etc.).
fn sdl_keycode_to_text_char(kc: sdl2::keyboard::Keycode, mods: sdl2::keyboard::Mod) -> u8 {
    use sdl2::keyboard::{Keycode, Mod as KeyMod};

    let shifted = mods.intersects(KeyMod::LSHIFTMOD | KeyMod::RSHIFTMOD);
    let ctrl    = mods.intersects(KeyMod::LCTRLMOD  | KeyMod::RCTRLMOD);

    if ctrl {
        if let Some(c) = keycode_to_alpha(kc) {
            return c as u8 - b'a' + 1; // Ctrl+A..Z → 0x01..0x1A
        }
        return 0;
    }

    match kc {
        Keycode::Return | Keycode::KpEnter => 0x0D,
        Keycode::Backspace => 0x08,
        Keycode::Tab       => 0x09,
        Keycode::Escape    => 0x1B,
        Keycode::Space     => b' ',
        _ => {
            if let Some(c) = keycode_to_alpha(kc) {
                if shifted { c.to_ascii_uppercase() as u8 } else { c as u8 }
            } else {
                keycode_to_digit_or_symbol(kc, shifted).unwrap_or(0x00)
            }
        }
    }
}

fn keycode_to_alpha(kc: sdl2::keyboard::Keycode) -> Option<char> {
    use sdl2::keyboard::Keycode;
    match kc {
        Keycode::A => Some('a'), Keycode::B => Some('b'), Keycode::C => Some('c'),
        Keycode::D => Some('d'), Keycode::E => Some('e'), Keycode::F => Some('f'),
        Keycode::G => Some('g'), Keycode::H => Some('h'), Keycode::I => Some('i'),
        Keycode::J => Some('j'), Keycode::K => Some('k'), Keycode::L => Some('l'),
        Keycode::M => Some('m'), Keycode::N => Some('n'), Keycode::O => Some('o'),
        Keycode::P => Some('p'), Keycode::Q => Some('q'), Keycode::R => Some('r'),
        Keycode::S => Some('s'), Keycode::T => Some('t'), Keycode::U => Some('u'),
        Keycode::V => Some('v'), Keycode::W => Some('w'), Keycode::X => Some('x'),
        Keycode::Y => Some('y'), Keycode::Z => Some('z'),
        _ => None,
    }
}

fn keycode_to_digit_or_symbol(kc: sdl2::keyboard::Keycode, shifted: bool) -> Option<u8> {
    use sdl2::keyboard::Keycode;
    Some(match kc {
        Keycode::Num0 => if shifted { b')' } else { b'0' },
        Keycode::Num1 => if shifted { b'!' } else { b'1' },
        Keycode::Num2 => if shifted { b'@' } else { b'2' },
        Keycode::Num3 => if shifted { b'#' } else { b'3' },
        Keycode::Num4 => if shifted { b'$' } else { b'4' },
        Keycode::Num5 => if shifted { b'%' } else { b'5' },
        Keycode::Num6 => if shifted { b'^' } else { b'6' },
        Keycode::Num7 => if shifted { b'&' } else { b'7' },
        Keycode::Num8 => if shifted { b'*' } else { b'8' },
        Keycode::Num9 => if shifted { b'(' } else { b'9' },
        Keycode::Kp0 => b'0', Keycode::Kp1 => b'1', Keycode::Kp2 => b'2',
        Keycode::Kp3 => b'3', Keycode::Kp4 => b'4', Keycode::Kp5 => b'5',
        Keycode::Kp6 => b'6', Keycode::Kp7 => b'7', Keycode::Kp8 => b'8',
        Keycode::Kp9 => b'9',
        Keycode::KpPeriod   => b'.', Keycode::KpPlus     => b'+',
        Keycode::KpMinus    => b'-', Keycode::KpMultiply => b'*',
        Keycode::KpDivide   => b'/',
        Keycode::Minus        => if shifted { b'_' } else { b'-' },
        Keycode::Equals       => if shifted { b'+' } else { b'=' },
        Keycode::LeftBracket  => if shifted { b'{' } else { b'[' },
        Keycode::RightBracket => if shifted { b'}' } else { b']' },
        Keycode::Backslash    => if shifted { b'|' } else { b'\\' },
        Keycode::Semicolon    => if shifted { b':' } else { b';' },
        Keycode::Quote        => if shifted { b'"' } else { b'\'' },
        Keycode::Comma        => if shifted { b'<' } else { b',' },
        Keycode::Period       => if shifted { b'>' } else { b'.' },
        Keycode::Slash        => if shifted { b'?' } else { b'/' },
        Keycode::Backquote    => if shifted { b'~' } else { b'`' },
        _ => return None,
    })
}

// ── Headless text-mode renderer ───────────────────────────────────────────────

/// No-op text renderer for CI and automated testing.
pub struct HeadlessTextRenderer {
    /// Number of frames rendered (for test assertions).
    pub frame_count: u32,
    /// Controls whether `poll_events` returns `true` (continue).
    pub keep_running: bool,
}

impl HeadlessTextRenderer {
    pub fn new() -> Self {
        HeadlessTextRenderer { frame_count: 0, keep_running: true }
    }
}

impl Default for HeadlessTextRenderer {
    fn default() -> Self { Self::new() }
}

impl TextModeRenderer for HeadlessTextRenderer {
    fn render_frame(&mut self, _buf: &VgaTextBuffer, _blink_on: bool) {
        self.frame_count += 1;
    }

    fn poll_events(&mut self, _shared: &Arc<SharedState>) -> bool {
        self.keep_running
    }

    fn frame_sleep(&self) {
        // No-op in headless mode.
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use crate::loader::Loader;

    fn make_shared() -> Arc<crate::loader::SharedState> {
        Loader::new_mock().shared
    }

    // ── CP437 font ────────────────────────────────────────────────────────────

    #[test]
    fn ascii_glyph_has_pixels() {
        let g = get_cp437_glyph(b'A');
        assert!(g.iter().any(|&b| b != 0));
    }

    #[test]
    fn full_block_is_all_ones() {
        assert_eq!(get_cp437_glyph(0xDB), [0xFF; 16]); // █
    }

    #[test]
    fn lower_half_block_correct() {
        let g = get_cp437_glyph(0xDC); // ▄
        for r in 0..8  { assert_eq!(g[r], 0x00, "upper half must be blank at row {r}"); }
        for r in 8..16 { assert_eq!(g[r], 0xFF, "lower half must be set at row {r}"); }
    }

    #[test]
    fn upper_half_block_correct() {
        let g = get_cp437_glyph(0xDF); // ▀
        for r in 0..8  { assert_eq!(g[r], 0xFF, "upper half must be set at row {r}"); }
        for r in 8..16 { assert_eq!(g[r], 0x00, "lower half must be blank at row {r}"); }
    }

    #[test]
    fn left_half_block_correct() {
        assert_eq!(get_cp437_glyph(0xDD), [0xF0; 16]); // ▌
    }

    #[test]
    fn right_half_block_correct() {
        assert_eq!(get_cp437_glyph(0xDE), [0x0F; 16]); // ▐
    }

    #[test]
    fn vertical_line_is_uniform() {
        assert_eq!(get_cp437_glyph(0xB3), [0x08; 16]); // │
    }

    #[test]
    fn horizontal_line_has_exactly_one_filled_row() {
        let g = get_cp437_glyph(0xC4); // ─
        assert_eq!(g.iter().filter(|&&b| b == 0xFF).count(), 1);
    }

    #[test]
    fn double_vertical_line_is_uniform() {
        assert_eq!(get_cp437_glyph(0xBA), [0x14; 16]); // ║
    }

    #[test]
    fn double_horizontal_has_two_filled_rows() {
        let g = get_cp437_glyph(0xCD); // ═
        assert_eq!(g.iter().filter(|&&b| b == 0xFF).count(), 2);
    }

    #[test]
    fn unknown_byte_returns_blank() {
        assert_eq!(get_cp437_glyph(0x01), [0u8; 16]);
    }

    // ── CGA palette ───────────────────────────────────────────────────────────

    #[test]
    fn cga_black_is_opaque_black() {
        assert_eq!(CGA_PALETTE[0], 0xFF_00_00_00);
    }

    #[test]
    fn cga_white_is_opaque_white() {
        assert_eq!(CGA_PALETTE[15], 0xFF_FF_FF_FF);
    }

    #[test]
    fn cga_palette_all_opaque() {
        for &c in &CGA_PALETTE {
            assert_eq!(c >> 24, 0xFF, "palette entry must be fully opaque");
        }
    }

    // ── HeadlessTextRenderer ──────────────────────────────────────────────────

    #[test]
    fn headless_renderer_counts_frames() {
        let shared = make_shared();
        let mut r = HeadlessTextRenderer::new();
        let buf = VgaTextBuffer::snapshot(&shared);
        r.render_frame(&buf, true);
        r.render_frame(&buf, false);
        assert_eq!(r.frame_count, 2);
    }

    #[test]
    fn headless_renderer_stops_on_keep_running_false() {
        let shared = make_shared();
        let mut r = HeadlessTextRenderer { frame_count: 0, keep_running: false };
        run_text_loop(&mut r, shared);
        // Returns without hanging.
    }

    #[test]
    fn headless_renderer_stops_on_exit_requested() {
        let shared = make_shared();
        shared.exit_requested.store(true, Ordering::Relaxed);
        let mut r = HeadlessTextRenderer::new();
        run_text_loop(&mut r, shared);
    }

    #[test]
    fn headless_frame_sleep_is_noop() {
        let r = HeadlessTextRenderer::new();
        let start = std::time::Instant::now();
        r.frame_sleep();
        assert!(start.elapsed().as_millis() < 5);
    }

    #[test]
    fn headless_default_matches_new() {
        let r = HeadlessTextRenderer::default();
        assert_eq!(r.frame_count, 0);
        assert!(r.keep_running);
    }

    // ── VgaTextBuffer ─────────────────────────────────────────────────────────

    #[test]
    fn snapshot_reflects_vio_defaults() {
        let shared = make_shared();
        let buf = VgaTextBuffer::snapshot(&shared);
        assert!(buf.rows >= 24);
        assert!(buf.cols >= 80);
        assert_eq!(buf.cursor_row, 0);
        assert_eq!(buf.cursor_col, 0);
        assert!(buf.cursor_visible);
    }

    // ── KbdKeyInfo queue ──────────────────────────────────────────────────────

    // Modifier-only keys must not appear as characters (would output raw scan
    // code bytes like 0x2A='*' for LShift or 0x36='6' for RShift).
    #[test]
    fn modifier_keys_yield_no_char() {
        use sdl2::keyboard::{Keycode, Mod as KeyMod};
        let shift_mod = KeyMod::LSHIFTMOD;
        for kc in [Keycode::LShift, Keycode::RShift, Keycode::LCtrl, Keycode::RCtrl,
                   Keycode::LAlt,   Keycode::RAlt,   Keycode::CapsLock] {
            let ch = sdl_keycode_to_text_char(kc, shift_mod);
            assert_eq!(ch, 0x00, "{:?} should produce ch=0x00, got 0x{:02X}", kc, ch);
        }
    }

    #[test]
    fn kbd_queue_push_pop() {
        let shared = make_shared();
        shared.kbd_queue.lock().unwrap()
            .push_back(KbdKeyInfo { ch: b'A', scan: 0x1E, state: 0 });
        shared.kbd_cond.notify_one();
        let ki = shared.kbd_queue.lock().unwrap().pop_front().unwrap();
        assert_eq!(ki.ch, b'A');
        assert_eq!(ki.scan, 0x1E);
    }

    #[test]
    fn kbd_queue_multiple_keys() {
        let shared = make_shared();
        {
            let mut q = shared.kbd_queue.lock().unwrap();
            q.push_back(KbdKeyInfo { ch: b'H', scan: 0x23, state: 0 });
            q.push_back(KbdKeyInfo { ch: b'i', scan: 0x17, state: 0 });
        }
        shared.kbd_cond.notify_all();
        let mut q = shared.kbd_queue.lock().unwrap();
        assert_eq!(q.pop_front().unwrap().ch, b'H');
        assert_eq!(q.pop_front().unwrap().ch, b'i');
        assert!(q.pop_front().is_none());
    }
}
