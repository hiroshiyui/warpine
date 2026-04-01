// SPDX-License-Identifier: GPL-3.0-only

//! Phase 6: VGA Text-Mode Renderer
//!
//! Renders an 80×25 VGA text buffer into an SDL2 window using an 8×16 GNU
//! Unifont pixel font.  Keyboard events from SDL2 are pushed into
//! `SharedState::kbd_queue` for consumption by `KbdCharIn`.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::loader::{SharedState, KbdKeyInfo, MutexExt};

// Generated at build time from vendor/unifont/unifont.hex.
// Sorted (Unicode codepoint, 8×16 glyph) pairs for all half-width entries.
include!(concat!(env!("OUT_DIR"), "/font_unifont_sbcs.rs"));

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

// ── GNU Unifont 8×16 glyph lookup ────────────────────────────────────────────

/// Return the 8×16 glyph bitmap for a Unicode character from GNU Unifont 17.
///
/// Rows 0-15 from top to bottom; bit 7 of each byte is the leftmost pixel.
/// Uses binary search on the pre-sorted `UNIFONT_SBCS` table generated at
/// build time from `vendor/unifont/unifont.hex`.  Characters not present in
/// Unifont (e.g. Private Use Area codepoints U+E000–U+F8FF) return a blank
/// glyph `[0u8; 16]`.
pub fn get_glyph_for_char(ch: char) -> [u8; 16] {
    let cp = ch as u32;
    match UNIFONT_SBCS.binary_search_by_key(&cp, |&(c, _)| c) {
        Ok(idx) => UNIFONT_SBCS[idx].1,
        Err(_) => [0u8; 16],
    }
}

/// Return the 16×16 glyph bitmap for a wide (DBCS) Unicode character.
///
/// Each of the 16 rows is stored as two consecutive bytes: the first byte
/// covers pixels 0–7 (left half) and the second covers pixels 8–15 (right
/// half), MSB first (`bit 7 = leftmost pixel`).
///
/// **Phase B5 placeholder** — returns a blank 32-byte array for all inputs
/// until `build.rs` extracts the 16×16 entries from `vendor/unifont/unifont.hex`
/// and emits the `UNIFONT_WIDE` lookup table.  Once B5 is in place this
/// function will binary-search that table identically to `get_glyph_for_char`.
pub fn get_glyph_dbcs(_ch: char) -> [u8; 32] {
    [0u8; 32]
}

// ── DBCS cell classification ─────────────────────────────────────────────────

/// Per-cell classification for the DBCS annotation pass.
///
/// In OS/2 VIO text mode a DBCS character occupies two consecutive cells:
/// the first cell is `DbcsLead` and the second is `DbcsTail`.  All cells in
/// SBCS codepages (and any cell that is not part of a DBCS pair) are `Sbcs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Sbcs,
    DbcsLead,
    DbcsTail,
}

/// Classify every cell in a VGA text buffer as SBCS, DBCS-lead, or DBCS-tail.
///
/// Performs a single left-to-right scan per row using the lead-byte tables
/// from [`crate::loader::locale::dbcs_lead_ranges`].  A lead byte followed
/// by any trail byte within the same row produces a `(DbcsLead, DbcsTail)`
/// pair; an unpaired lead byte at the end of a row is treated as `Sbcs`.
/// For SBCS codepages the function returns all-`Sbcs` in O(1).
///
/// Returns a `Vec<CellKind>` of the same length as `raw_bytes`.
pub fn annotate_dbcs(raw_bytes: &[u8], codepage: u32, cols: u16) -> Vec<CellKind> {
    use crate::loader::locale::is_dbcs_lead_byte;

    let mut kinds = vec![CellKind::Sbcs; raw_bytes.len()];

    // Fast path: SBCS codepages have no lead bytes.
    if crate::loader::locale::dbcs_lead_ranges(codepage).is_empty() {
        return kinds;
    }

    let cols = cols as usize;
    if cols == 0 { return kinds; }

    let mut i = 0;
    while i < raw_bytes.len() {
        let col = i % cols;
        if is_dbcs_lead_byte(raw_bytes[i], codepage) && col + 1 < cols && i + 1 < raw_bytes.len() {
            // Lead byte with a valid trail position in the same row.
            kinds[i]     = CellKind::DbcsLead;
            kinds[i + 1] = CellKind::DbcsTail;
            i += 2;
        } else {
            i += 1;
        }
    }

    kinds
}

// ── VgaTextBuffer snapshot ───────────────────────────────────────────────────

/// Snapshot of the VioManager state for a single rendered frame.
pub struct VgaTextBuffer {
    pub rows: u16,
    pub cols: u16,
    /// (char, attr) pairs, row-major.  attr: bits 3:0 = fg (with bright), 7:4 = bg.
    pub cells: Vec<(char, u8)>,
    /// Raw guest bytes parallel to `cells`, used by the DBCS annotation pass.
    pub raw_bytes: Vec<u8>,
    /// Per-cell kind produced by `annotate_dbcs` at snapshot time.
    pub cell_kind: Vec<CellKind>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    pub cursor_start: u8,
    pub cursor_end: u8,
}

impl VgaTextBuffer {
    /// Snapshot the current VioManager state.
    ///
    /// Runs the DBCS annotation pass once per frame so renderers can
    /// distinguish DBCS lead/tail cells from SBCS cells without re-scanning.
    pub fn snapshot(shared: &SharedState) -> Self {
        let vio = shared.console_mgr.lock_or_recover();
        let raw_bytes = vio.raw_bytes.clone();
        let codepage = vio.codepage as u32;
        let cols = vio.cols;
        let cell_kind = annotate_dbcs(&raw_bytes, codepage, cols);
        VgaTextBuffer {
            rows: vio.rows,
            cols: vio.cols,
            cells: vio.buffer.clone(),
            raw_bytes,
            cell_kind,
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

        let blink_on = (blink_epoch.elapsed().as_millis() / 500).is_multiple_of(2);
        let buf = VgaTextBuffer::snapshot(&shared);
        renderer.render_frame(&buf, blink_on);

        renderer.frame_sleep();
    }
}

// ── SDL2 text-mode renderer ───────────────────────────────────────────────────

/// SDL2-backed VGA text-mode renderer.
///
/// Opens an SDL2 window sized to `cols × 8` × `rows × 16` pixels and renders
/// one 8×16 CP437 cell per screen cell.  When the `VgaTextBuffer` dimensions
/// change (e.g. after a `VioSetMode` call) the window is automatically
/// resized to match.
///
/// Must be created and driven on the main thread.
pub struct Sdl2TextRenderer {
    canvas: sdl2::render::Canvas<sdl2::video::Window>,
    /// Texture creator stored alongside the canvas so we can recreate the
    /// streaming texture on window resize without rebuilding the whole renderer.
    texture_creator: sdl2::render::TextureCreator<sdl2::video::WindowContext>,
    /// Persistent streaming texture — uploaded once per frame.
    texture: sdl2::render::Texture,
    event_pump: sdl2::EventPump,
    /// Pixel framebuffer: `current_cols*8 × current_rows*16` ARGB8888 pixels.
    pixels: Vec<u32>,
    /// Width of the current window in text columns.
    current_cols: u16,
    /// Height of the current window in text rows.
    current_rows: u16,
}

impl Sdl2TextRenderer {
    /// Initial (default) window size — 80 columns × 25 rows.
    const INIT_COLS: u16 = 80;
    const INIT_ROWS: u16 = 25;

    pub fn new(sdl: &sdl2::Sdl, title: &str) -> Self {
        use sdl2::pixels::PixelFormatEnum;
        use sdl2::render::BlendMode;

        let video = sdl.video().expect("SDL2 video subsystem");
        let init_w = Self::INIT_COLS as u32 * 8;
        let init_h = Self::INIT_ROWS as u32 * 16;
        let window = video
            .window(title, init_w, init_h)
            .position_centered()
            .build()
            .expect("SDL2 window");
        let canvas = window
            .into_canvas()
            .accelerated()
            .build()
            .expect("SDL2 canvas");
        let texture_creator = canvas.texture_creator();
        let mut texture = texture_creator
            .create_texture_streaming(PixelFormatEnum::ARGB8888, init_w, init_h)
            .expect("SDL2 streaming texture");
        texture.set_blend_mode(BlendMode::None);
        let event_pump = sdl.event_pump().expect("SDL2 event pump");
        let pixels = vec![0xFF_00_00_00u32; (init_w * init_h) as usize];
        Sdl2TextRenderer {
            canvas, texture_creator, texture, event_pump, pixels,
            current_cols: Self::INIT_COLS,
            current_rows: Self::INIT_ROWS,
        }
    }

    /// Upload the pixel buffer to the texture and blit to the screen.
    fn present(&mut self) {
        let w = self.current_cols as usize * 8;
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

    /// Resize the SDL2 window + recreate the streaming texture when the
    /// VioManager dimensions change.
    fn handle_resize(&mut self, new_cols: u16, new_rows: u16) {
        use sdl2::pixels::PixelFormatEnum;
        use sdl2::render::BlendMode;

        let new_w = new_cols as u32 * 8;
        let new_h = new_rows as u32 * 16;

        // Resize the SDL2 window (best-effort; may fail on some platforms
        // if the window was created without the RESIZABLE flag, but SDL2 will
        // silently clamp rather than panic).
        let _ = self.canvas.window_mut().set_size(new_w, new_h);

        // Recreate the streaming texture for the new dimensions.
        let mut new_tex = self.texture_creator
            .create_texture_streaming(PixelFormatEnum::ARGB8888, new_w, new_h)
            .expect("SDL2 texture resize failed");
        new_tex.set_blend_mode(BlendMode::None);
        self.texture = new_tex;

        self.pixels = vec![0xFF_00_00_00u32; (new_w * new_h) as usize];
        self.current_cols = new_cols;
        self.current_rows = new_rows;
    }
}

impl TextModeRenderer for Sdl2TextRenderer {
    fn render_frame(&mut self, buf: &VgaTextBuffer, blink_on: bool) {
        // Resize window + texture when VioManager dimensions change.
        if buf.cols != self.current_cols || buf.rows != self.current_rows {
            self.handle_resize(buf.cols, buf.rows);
        }

        let cols = buf.cols as usize;
        let rows = buf.rows as usize;
        let win_w = self.current_cols as usize * 8;
        let win_h = self.current_rows as usize * 16;

        for row in 0..rows {
            let mut col = 0usize;
            while col < cols {
                let idx = row * cols + col;
                let (ch, attr) = if idx < buf.cells.len() { buf.cells[idx] } else { (' ', 0x07) };
                let kind = if idx < buf.cell_kind.len() { buf.cell_kind[idx] } else { CellKind::Sbcs };

                let fg = CGA_PALETTE[(attr & 0x0F) as usize];
                let bg = CGA_PALETTE[((attr >> 4) & 0x0F) as usize];
                let base_x = col * 8;
                let base_y = row * 16;
                let cursor_here = buf.cursor_visible && blink_on
                    && row as u16 == buf.cursor_row && col as u16 == buf.cursor_col;

                match kind {
                    CellKind::DbcsLead => {
                        // ch is currently U+FFFD (each byte decoded individually).
                        // Phase B4 will update the snapshot or write path so ch holds
                        // the correct Unicode codepoint decoded from the lead+trail pair.
                        // Phase B5 will implement get_glyph_dbcs() with a real lookup table.
                        let glyph = get_glyph_dbcs(ch);

                        for gr in 0..16usize {
                            let py = base_y + gr;
                            if py >= win_h { break; }
                            let hi = glyph[gr * 2];
                            let lo = glyph[gr * 2 + 1];
                            for gc in 0..8usize {
                                let px = base_x + gc;
                                if px < win_w {
                                    self.pixels[py * win_w + px] =
                                        if hi & (0x80 >> gc) != 0 { fg } else { bg };
                                }
                            }
                            for gc in 0..8usize {
                                let px = base_x + 8 + gc;
                                if px < win_w {
                                    self.pixels[py * win_w + px] =
                                        if lo & (0x80 >> gc) != 0 { fg } else { bg };
                                }
                            }
                        }

                        // Cursor overlay: XOR-invert both 8-pixel halves.
                        if cursor_here {
                            let cstart = buf.cursor_start as usize;
                            let cend   = (buf.cursor_end as usize).min(15);
                            let (cstart, cend) = if cstart <= cend { (cstart, cend) } else { (14, 15) };
                            for gr in cstart..=cend {
                                let py = base_y + gr;
                                if py >= win_h { break; }
                                for gc in 0..16usize {
                                    let px = base_x + gc;
                                    if px < win_w {
                                        let old = self.pixels[py * win_w + px];
                                        self.pixels[py * win_w + px] =
                                            (old ^ 0x00_FF_FF_FF) | 0xFF_00_00_00;
                                    }
                                }
                            }
                        }

                        col += 2;
                    }

                    CellKind::DbcsTail => {
                        // Already rendered by the preceding DbcsLead cell.
                        // If the cursor lands on the tail cell, invert only the
                        // right 8-pixel half (this cell's column).
                        if buf.cursor_visible && blink_on
                            && row as u16 == buf.cursor_row && col as u16 == buf.cursor_col
                        {
                            let cstart = buf.cursor_start as usize;
                            let cend   = (buf.cursor_end as usize).min(15);
                            let (cstart, cend) = if cstart <= cend { (cstart, cend) } else { (14, 15) };
                            for gr in cstart..=cend {
                                let py = base_y + gr;
                                if py >= win_h { break; }
                                for gc in 0..8usize {
                                    let px = base_x + gc;
                                    if px < win_w {
                                        let old = self.pixels[py * win_w + px];
                                        self.pixels[py * win_w + px] =
                                            (old ^ 0x00_FF_FF_FF) | 0xFF_00_00_00;
                                    }
                                }
                            }
                        }
                        col += 1;
                    }

                    CellKind::Sbcs => {
                        let glyph = get_glyph_for_char(ch);

                        for (gr, &bits) in glyph.iter().enumerate() {
                            let py = base_y + gr;
                            if py >= win_h { break; }
                            for gc in 0..8usize {
                                let px = base_x + gc;
                                if px >= win_w { break; }
                                self.pixels[py * win_w + px] =
                                    if bits & (0x80 >> gc) != 0 { fg } else { bg };
                            }
                        }

                        // Cursor overlay for the cursor cell.
                        if cursor_here {
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
                                    self.pixels[py * win_w + px] =
                                        (old ^ 0x00_FF_FF_FF) | 0xFF_00_00_00;
                                }
                            }
                        }

                        col += 1;
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

    // ── GNU Unifont glyph lookup ──────────────────────────────────────────────

    #[test]
    fn glyph_for_char_ascii_has_pixels() {
        // ASCII printable chars must have at least one set pixel.
        let g = get_glyph_for_char('A');
        assert!(g.iter().any(|&b| b != 0), "'A' glyph should have pixels");
        let g2 = get_glyph_for_char('T');
        assert!(g2.iter().any(|&b| b != 0), "'T' glyph should have pixels");
    }

    #[test]
    fn glyph_for_char_box_drawing_has_pixels() {
        // U+2502 (│ BOX DRAWINGS LIGHT VERTICAL) — must be non-blank
        assert_ne!(get_glyph_for_char('│'), [0u8; 16]);
    }

    #[test]
    fn glyph_for_char_block_element_has_pixels() {
        // U+2588 (█ FULL BLOCK) — must be all-ones in Unifont
        assert_eq!(get_glyph_for_char('█'), [0xFF; 16]);
    }

    #[test]
    fn glyph_for_char_private_use_is_blank() {
        // U+E000 is in the Private Use Area — not defined in Unifont
        assert_eq!(get_glyph_for_char('\u{E000}'), [0u8; 16]);
    }

    #[test]
    fn glyph_for_char_latin1_extended_has_pixels() {
        // U+00D0 (Ð LATIN CAPITAL LETTER ETH) — not in CP437 but present in Unifont
        assert_ne!(get_glyph_for_char('Ð'), [0u8; 16]);
    }

    #[test]
    fn full_block_is_all_ones() {
        assert_eq!(get_glyph_for_char('█'), [0xFF; 16]); // U+2588
    }

    #[test]
    fn lower_half_block_correct() {
        let g = get_glyph_for_char('▄'); // U+2584
        for r in 0..8  { assert_eq!(g[r], 0x00, "upper half must be blank at row {r}"); }
        for r in 8..16 { assert_eq!(g[r], 0xFF, "lower half must be set at row {r}"); }
    }

    #[test]
    fn upper_half_block_correct() {
        let g = get_glyph_for_char('▀'); // U+2580
        for r in 0..8  { assert_eq!(g[r], 0xFF, "upper half must be set at row {r}"); }
        for r in 8..16 { assert_eq!(g[r], 0x00, "lower half must be blank at row {r}"); }
    }

    #[test]
    fn left_half_block_correct() {
        assert_eq!(get_glyph_for_char('▌'), [0xF0; 16]); // U+258C
    }

    #[test]
    fn right_half_block_correct() {
        assert_eq!(get_glyph_for_char('▐'), [0x0F; 16]); // U+2590
    }

    #[test]
    fn vertical_line_is_uniform() {
        assert_eq!(get_glyph_for_char('│'), [0x08; 16]); // U+2502
    }

    #[test]
    fn horizontal_line_has_exactly_one_filled_row() {
        let g = get_glyph_for_char('─'); // U+2500
        assert_eq!(g.iter().filter(|&&b| b == 0xFF).count(), 1);
    }

    #[test]
    fn double_vertical_line_is_uniform() {
        assert_eq!(get_glyph_for_char('║'), [0x14; 16]); // U+2551
    }

    #[test]
    fn double_horizontal_has_two_filled_rows() {
        let g = get_glyph_for_char('═'); // U+2550
        assert_eq!(g.iter().filter(|&&b| b == 0xFF).count(), 2);
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

    // ── get_glyph_dbcs (Phase B5 placeholder) ────────────────────────────────

    #[test]
    fn glyph_dbcs_is_placeholder_blank() {
        // Until B5 is implemented, every wide character returns a blank glyph.
        assert_eq!(get_glyph_dbcs('\u{4E2D}'), [0u8; 32]); // CJK: 中
        assert_eq!(get_glyph_dbcs('\u{AC00}'), [0u8; 32]); // Hangul: 가
        assert_eq!(get_glyph_dbcs('\u{FFFD}'), [0u8; 32]); // Replacement char
    }

    #[test]
    fn glyph_dbcs_row_stride_is_two_bytes() {
        // Sanity check: glyph is exactly 32 bytes (16 rows × 2 bytes per row).
        let g = get_glyph_dbcs('X');
        assert_eq!(g.len(), 32);
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

    #[test]
    fn snapshot_cell_kind_and_raw_bytes_lengths_match() {
        let shared = make_shared();
        let buf = VgaTextBuffer::snapshot(&shared);
        assert_eq!(buf.raw_bytes.len(), buf.cells.len());
        assert_eq!(buf.cell_kind.len(), buf.cells.len());
    }

    // ── annotate_dbcs ─────────────────────────────────────────────────────────

    #[test]
    fn annotate_sbcs_codepage_all_sbcs() {
        // CP437 has no lead bytes — every cell must be Sbcs.
        let raw = vec![b'A', 0x9C, b' ', 0xFF];
        let kinds = annotate_dbcs(&raw, 437, 4);
        assert!(kinds.iter().all(|&k| k == CellKind::Sbcs));
    }

    #[test]
    fn annotate_dbcs_cp932_basic_pair() {
        // 0x82 is a CP932 (Shift-JIS) lead byte.
        // Row of 4 cols: [0x82, 0xA0, b'X', b'Y']
        let raw = vec![0x82_u8, 0xA0, b'X', b'Y'];
        let kinds = annotate_dbcs(&raw, 932, 4);
        assert_eq!(kinds[0], CellKind::DbcsLead);
        assert_eq!(kinds[1], CellKind::DbcsTail);
        assert_eq!(kinds[2], CellKind::Sbcs);
        assert_eq!(kinds[3], CellKind::Sbcs);
    }

    #[test]
    fn annotate_dbcs_cp932_two_pairs() {
        // Two adjacent DBCS pairs on the same row (4 cols = exactly two pairs).
        let raw = vec![0x82_u8, 0xA0, 0x82, 0xA1];
        let kinds = annotate_dbcs(&raw, 932, 4);
        assert_eq!(kinds, vec![CellKind::DbcsLead, CellKind::DbcsTail,
                               CellKind::DbcsLead, CellKind::DbcsTail]);
    }

    #[test]
    fn annotate_dbcs_lead_at_last_col_is_sbcs() {
        // A lead byte at the last column of a row cannot form a pair (no next
        // column in the same row) → must be classified as Sbcs.
        // Row of 2 cols: col 0 = 0x82 (lead), col 1 = next row starts here
        // represented as a flat 4-element slice with cols=2.
        // row 0: [0x82, b'A']  — 0x82 at col 0 → should pair with b'A' at col 1
        // row 1: [0x82, b'B']  — 0x82 at col 0 → should pair with b'B' at col 1
        let raw = vec![0x82_u8, b'A', 0x82, b'B'];
        let kinds = annotate_dbcs(&raw, 932, 2);
        // col 0 of each row: DbcsLead; col 1: DbcsTail
        assert_eq!(kinds[0], CellKind::DbcsLead);
        assert_eq!(kinds[1], CellKind::DbcsTail);
        assert_eq!(kinds[2], CellKind::DbcsLead);
        assert_eq!(kinds[3], CellKind::DbcsTail);
    }

    #[test]
    fn annotate_dbcs_lead_at_last_col_no_pair() {
        // Lead byte is the very last cell of a row (col == cols-1) — no pair.
        // 3-col row: [b'X', b'Y', 0x82]  → the lead at col 2 is unpaired.
        let raw = vec![b'X', b'Y', 0x82_u8];
        let kinds = annotate_dbcs(&raw, 932, 3);
        assert_eq!(kinds[0], CellKind::Sbcs);
        assert_eq!(kinds[1], CellKind::Sbcs);
        assert_eq!(kinds[2], CellKind::Sbcs); // unpaired lead → Sbcs
    }

    #[test]
    fn annotate_dbcs_cp936_pair() {
        // CP936 lead range is 0x81–0xFE.
        let raw = vec![0x81_u8, 0x40, b'Z'];
        let kinds = annotate_dbcs(&raw, 936, 3);
        assert_eq!(kinds[0], CellKind::DbcsLead);
        assert_eq!(kinds[1], CellKind::DbcsTail);
        assert_eq!(kinds[2], CellKind::Sbcs);
    }

    #[test]
    fn annotate_dbcs_empty_input() {
        let kinds = annotate_dbcs(&[], 932, 80);
        assert!(kinds.is_empty());
    }

    #[test]
    fn annotate_dbcs_zero_cols_no_pairs() {
        // cols=0 guard: should return all-Sbcs without panic.
        let raw = vec![0x82_u8, 0xA0];
        let kinds = annotate_dbcs(&raw, 932, 0);
        assert!(kinds.iter().all(|&k| k == CellKind::Sbcs));
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
