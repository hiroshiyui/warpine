// SPDX-License-Identifier: GPL-3.0-or-later

//! Phase 6: VGA Text-Mode Renderer
//!
//! Renders an 80×25 VGA text buffer into an SDL window using an 8×16 GNU
//! Unifont pixel font.  Keyboard events from SDL are pushed into
//! `SharedState::kbd_queue` for consumption by `KbdCharIn`.
//!
//! Like the PM desktop, the text window replays a GUI script
//! (`WARPINE_GUI_SCRIPT`, `gui_script.rs`): its keys and text are fed in as
//! SDL key events, `dump PATH` writes the current frame as a PPM, `quit`
//! ends the session, and host keyboard input is ignored meanwhile.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::loader::{SharedState, KbdKeyInfo, MutexExt};

// Generated at build time from vendor/unifont/unifont.hex.
// Sorted (Unicode codepoint, 8×16 glyph) pairs for all half-width entries.
include!(concat!(env!("OUT_DIR"), "/font_unifont_sbcs.rs"));

// Generated at build time from vendor/unifont/unifont.hex.
// Exposes `get_glyph_dbcs(ch: char) -> [u8; 32]` for all 16×16 wide entries.
include!(concat!(env!("OUT_DIR"), "/font_unifont_wide.rs"));

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
        use crate::loader::codepage::decode_dbcs;
        let vio = shared.console_mgr.lock_or_recover();
        let raw_bytes = vio.raw_bytes.clone();
        let codepage = vio.codepage as u32;
        let cols = vio.cols;
        let cell_kind = annotate_dbcs(&raw_bytes, codepage, cols);
        let mut cells = vio.buffer.clone();
        // Fix up DBCS lead cells: VioManager stored U+FFFD for each byte decoded
        // individually.  Now that we know which cells are DbcsLead, decode the
        // correct Unicode codepoint from the raw lead+trail byte pair.
        for (i, kind) in cell_kind.iter().enumerate() {
            if matches!(kind, CellKind::DbcsLead) && i + 1 < raw_bytes.len() {
                let ch = decode_dbcs(raw_bytes[i], raw_bytes[i + 1], codepage);
                cells[i].0 = ch;
            }
        }
        VgaTextBuffer {
            rows: vio.rows,
            cols: vio.cols,
            cells,
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

/// The text cell (row, column) nearest to point (`x`, `y`) of a window of
/// `cols × rows` 8 × 16 cells.
pub fn cell_in_window(x: f32, y: f32, cols: u16, rows: u16) -> (u16, u16) {
    let cell = |v: f32, size: f32, n: u16| ((v / size).max(0.0) as u16).min(n.saturating_sub(1));
    (cell(y, 16.0, rows), cell(x, 8.0, cols))
}

/// The screen of the full-screen text window: the VIO buffer with the
/// program's text-mode mouse pointer (MOUCALLS, drawn after `MouDrawPtr`)
/// applied to its cell's attribute.
pub fn full_screen_snapshot(shared: &SharedState) -> VgaTextBuffer {
    let mut buf = VgaTextBuffer::snapshot(shared);
    if let Some(p) = crate::loader::moucalls::text_pointer(shared)
        && p.row < buf.rows && p.col < buf.cols
        && let Some(cell) = buf.cells.get_mut(p.row as usize * buf.cols as usize + p.col as usize)
    {
        cell.1 = p.apply_attr(cell.1);
    }
    buf
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

/// Drive a text-mode application's event loop on the main thread. Returns
/// when the program ends, or with `SharedState::vio_switch_requested` set
/// when Alt+Home asks for the other display of a VIO session (`vio_switch`);
/// the program then keeps running.
pub fn run_text_loop<R: TextModeRenderer>(renderer: &mut R, shared: Arc<SharedState>) {
    // Use wall-clock time for cursor blink so it is frame-rate independent.
    // 500 ms on / 500 ms off → 1 Hz blink, matching real VGA behaviour.
    let blink_epoch = std::time::Instant::now();

    loop {
        if shared.exit_requested.load(Ordering::Relaxed) { break; }

        let keep_going = renderer.poll_events(&shared);
        // Alt+Home in a VIO session: the caller shows the other display.
        if super::vio_switch::switch_pending(&shared) { break; }
        if !keep_going {
            // The window was closed (or Ctrl+Esc): the program is asked to
            // end (XCPT_SIGNAL_KILLPROC, then its exit list); the window
            // stays until it has, at most `KILL_GRACE`. Closing it again
            // ends the program at once.
            crate::loader::signals::end_session(&shared);
        }

        if shared.exit_requested.load(Ordering::Relaxed) { break; }

        let blink_on = (blink_epoch.elapsed().as_millis() / 500).is_multiple_of(2);
        let buf = full_screen_snapshot(&shared);
        renderer.render_frame(&buf, blink_on);

        renderer.frame_sleep();
    }
}


/// Draw the cells of `buf` — 8 × 16 Unifont glyphs in the CGA colours, 16-pel
/// wide DBCS characters, the cursor XOR-inverted while `blink_on` — into
/// `pixels`, an ARGB buffer `win_w` × `win_h` pels with rows from the top.
/// Shared by the text window and the VIO window on the PM desktop.
pub fn render_cells(buf: &VgaTextBuffer, blink_on: bool, pixels: &mut [u32], win_w: usize, win_h: usize) {
    let cols = buf.cols as usize;
    let rows = buf.rows as usize;

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
                    // ch holds the correct Unicode codepoint decoded from the
                    // lead+trail raw byte pair by VgaTextBuffer::snapshot() (B4).
                    // Phase B5 will replace the stub get_glyph_dbcs() with a real
                    // lookup in the Unifont wide-glyph binary table.
                    let glyph = get_glyph_dbcs(ch);

                    for gr in 0..16usize {
                        let py = base_y + gr;
                        if py >= win_h { break; }
                        let hi = glyph[gr * 2];
                        let lo = glyph[gr * 2 + 1];
                        for gc in 0..8usize {
                            let px = base_x + gc;
                            if px < win_w {
                                pixels[py * win_w + px] =
                                    if hi & (0x80 >> gc) != 0 { fg } else { bg };
                            }
                        }
                        for gc in 0..8usize {
                            let px = base_x + 8 + gc;
                            if px < win_w {
                                pixels[py * win_w + px] =
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
                                    let old = pixels[py * win_w + px];
                                    pixels[py * win_w + px] =
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
                                    let old = pixels[py * win_w + px];
                                    pixels[py * win_w + px] =
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
                            pixels[py * win_w + px] =
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
                                let old = pixels[py * win_w + px];
                                pixels[py * win_w + px] =
                                    (old ^ 0x00_FF_FF_FF) | 0xFF_00_00_00;
                            }
                        }
                    }

                    col += 1;
                }
            }
        }
    }
}

// ── SDL text-mode renderer ───────────────────────────────────────────────────

/// SDL-backed VGA text-mode renderer: the full-screen display of a VIO
/// session.
///
/// Opens a desktop-full-screen SDL window (an ordinary window when full
/// screen is refused) showing the `cols × 8` × `rows × 16` text screen, one
/// 8×16 Unifont cell per screen cell, scaled up by whole multiples with its
/// aspect ratio kept and black bars around it (SDL's integer-scale logical
/// presentation). Host pointer positions are converted back to that logical
/// size, and a GUI-script `dump` writes the logical frame. When the
/// `VgaTextBuffer` dimensions change (e.g. after a `VioSetMode` call) the
/// texture and logical size follow.
///
/// Must be created and driven on the main thread.
pub struct SdlTextRenderer {
    canvas: sdl3::render::Canvas<sdl3::video::Window>,
    /// Texture creator kept for the life of the process, so we can recreate
    /// the streaming texture on window resize and the lifetime-checked
    /// texture can be `'static` (no `unsafe_textures`). It keeps the window
    /// alive, so a parked window is reused ([`TextWindow`]).
    texture_creator: &'static sdl3::render::TextureCreator<sdl3::video::WindowContext>,
    /// Persistent streaming texture — uploaded once per frame.
    texture: sdl3::render::Texture<'static>,
    /// MMPM/2 and DosBeep audio, played on this (main) thread.
    audio: super::audio::SdlAudio,
    event_pump: sdl3::EventPump,
    /// Pixel framebuffer: `current_cols*8 × current_rows*16` ARGB8888 pixels.
    pixels: Vec<u32>,
    /// Width of the current window in text columns.
    current_cols: u16,
    /// Height of the current window in text rows.
    current_rows: u16,
    /// The GUI script being replayed (`WARPINE_GUI_SCRIPT`), if any.
    script: Option<super::gui_script::GuiScript>,
}

/// The text display's SDL window while the other display of a VIO session
/// is shown (Alt+Home): hidden, to be shown again by the next
/// [`SdlTextRenderer::open`].
pub struct TextWindow {
    canvas: sdl3::render::Canvas<sdl3::video::Window>,
    texture_creator: &'static sdl3::render::TextureCreator<sdl3::video::WindowContext>,
    texture: sdl3::render::Texture<'static>,
    cols: u16,
    rows: u16,
}

/// How the text screen is scaled to the window: by whole multiples, aspect
/// ratio kept, black bars around it.
const TEXT_PRESENTATION: sdl3::sys::render::SDL_RendererLogicalPresentation =
    sdl3::sys::render::SDL_LOGICAL_PRESENTATION_INTEGER_SCALE;

/// Width and height in pels of one text cell.
const CELL_W: u32 = 8;
const CELL_H: u32 = 16;

/// Pixel size of a `cols × rows` text screen.
fn text_screen_px(cols: u16, rows: u16) -> (u32, u32) {
    (cols as u32 * CELL_W, rows as u32 * CELL_H)
}

/// A streaming texture for a `w × h` text screen, scaled with the nearest
/// pel so the glyphs stay sharp.
fn text_texture(creator: &'static sdl3::render::TextureCreator<sdl3::video::WindowContext>,
                w: u32, h: u32) -> sdl3::render::Texture<'static> {
    use sdl3::pixels::PixelFormat;
    use sdl3::render::{BlendMode, ScaleMode};
    let mut texture = creator
        .create_texture_streaming(PixelFormat::ARGB8888, w, h)
        .expect("SDL streaming texture");
    texture.set_blend_mode(BlendMode::None);
    texture.set_scale_mode(ScaleMode::Nearest);
    texture
}

impl SdlTextRenderer {
    /// Initial (default) window size — 80 columns × 25 rows.
    const INIT_COLS: u16 = 80;
    const INIT_ROWS: u16 = 25;

    /// Open the display, and audio output for `audio` (attached before the
    /// guest starts, so its first DosBeep is not lost).
    pub fn new(sdl: &sdl3::Sdl, title: &str, audio: &super::audio::AudioService) -> Self {
        Self::open(sdl, title, audio, &mut super::vio_switch::Parked::default())
    }

    /// Open the display full screen, reusing what `parked` holds: the
    /// window, audio output and GUI script of an earlier display of the
    /// session.
    pub fn open(sdl: &sdl3::Sdl, title: &str, audio: &super::audio::AudioService,
                parked: &mut super::vio_switch::Parked) -> Self {
        let video = sdl.video().expect("SDL video subsystem");
        let TextWindow { mut canvas, texture_creator, texture, cols, rows } = match parked.text.take() {
            Some(window) => window,
            None => {
                let (w, h) = text_screen_px(Self::INIT_COLS, Self::INIT_ROWS);
                let window = video
                    .window(title, w, h)
                    .position_centered()
                    .build()
                    .expect("SDL window");
                let canvas = window.into_canvas();
                let texture_creator: &'static _ = Box::leak(Box::new(canvas.texture_creator()));
                let texture = text_texture(texture_creator, w, h);
                TextWindow { canvas, texture_creator, texture, cols: Self::INIT_COLS, rows: Self::INIT_ROWS }
            }
        };
        let (w, h) = text_screen_px(cols, rows);
        if let Err(e) = canvas.set_logical_size(w, h, TEXT_PRESENTATION) {
            log::warn!("[TEXT] no scaled presentation ({e}); the screen is drawn 1:1");
        }
        {
            let window = canvas.window_mut();
            let _ = window.set_title(title);
            window.show();
            // Desktop full screen (no video mode change); the window stays
            // an ordinary one if that is refused.
            if let Err(e) = window.set_fullscreen(true) {
                log::warn!("[TEXT] full screen refused ({e}); using a window");
            }
            window.raise();
        }
        // TextInput (IME-composed text) is per window and off by default.
        video.text_input().start(canvas.window());
        let event_pump = sdl.event_pump().expect("SDL event pump");
        let pixels = vec![0xFF_00_00_00u32; (w * h) as usize];
        let mut audio_out = parked.take_audio(sdl);
        // This loop polls every 8 ms, so it needs no wake-up hook.
        audio_out.attach(audio, Box::new(|| {}));
        SdlTextRenderer {
            canvas, texture_creator, texture, event_pump, pixels,
            audio: audio_out,
            current_cols: cols,
            current_rows: rows,
            script: parked.take_script(),
        }
    }

    /// Hide the display and leave its window, audio output and GUI script
    /// in `parked` for the next display of the session.
    pub fn park(self, parked: &mut super::vio_switch::Parked) {
        let SdlTextRenderer { mut canvas, texture_creator, texture, audio, current_cols, current_rows,
                              script, .. } = self;
        {
            let window = canvas.window_mut();
            let _ = window.set_fullscreen(false);
            window.hide();
        }
        parked.text = Some(TextWindow { canvas, texture_creator, texture,
                                        cols: current_cols, rows: current_rows });
        parked.audio = Some(audio);
        parked.script = Some(script);
    }

    /// Write the frame now on screen — the text buffer rendered afresh,
    /// cursor shown — to `path` as a PPM (the `dump` step of a GUI script).
    fn dump_frame(&mut self, shared: &SharedState, path: &std::path::Path) {
        self.render_frame(&full_screen_snapshot(shared), true);
        // The logical text screen, whatever the window's size.
        let (w, h) = text_screen_px(self.current_cols, self.current_rows);
        if let Err(e) = super::gui_script::write_ppm(path, &self.pixels, w, h) {
            eprintln!("warpine: cannot write {}: {e}", path.display());
        }
    }

    /// The text cell (row, column) nearest to window point (`x`, `y`).
    fn cell_at(&self, x: f32, y: f32) -> (u16, u16) {
        cell_in_window(x, y, self.current_cols, self.current_rows)
    }

    /// Upload the pixel buffer to the texture and blit to the screen.
    fn present(&mut self) {
        let w = text_screen_px(self.current_cols, self.current_rows).0 as usize;
        let pixels = &self.pixels;
        self.texture.with_lock(None, |data: &mut [u8], pitch: usize| {
            for (y, row) in pixels.chunks(w).enumerate() {
                let dst = &mut data[y * pitch..y * pitch + w * 4];
                dst.copy_from_slice(bytemuck::cast_slice(row));
            }
        }).expect("texture lock failed");
        // The bars around the scaled screen are the clear colour.
        self.canvas.set_draw_color(sdl3::pixels::Color::RGB(0, 0, 0));
        self.canvas.clear();
        self.canvas.copy(&self.texture, None::<sdl3::render::FRect>, None::<sdl3::render::FRect>)
            .expect("canvas copy failed");
        self.canvas.present();
    }

    /// Follow a change of the VioManager dimensions: a new streaming texture
    /// and logical size. A full-screen window keeps its size; the windowed
    /// fallback is resized to the screen (best effort: SDL clamps rather
    /// than fails).
    fn handle_resize(&mut self, new_cols: u16, new_rows: u16) {
        let (new_w, new_h) = text_screen_px(new_cols, new_rows);
        if let Err(e) = self.canvas.set_logical_size(new_w, new_h, TEXT_PRESENTATION) {
            log::warn!("[TEXT] logical size {new_w}×{new_h} refused ({e})");
        }
        let window = self.canvas.window_mut();
        if window.fullscreen_state() == sdl3::video::FullscreenType::Off {
            let _ = window.set_size(new_w, new_h);
        }
        self.texture = text_texture(self.texture_creator, new_w, new_h);
        self.pixels = vec![0xFF_00_00_00u32; (new_w * new_h) as usize];
        self.current_cols = new_cols;
        self.current_rows = new_rows;
    }
}

impl TextModeRenderer for SdlTextRenderer {
    fn render_frame(&mut self, buf: &VgaTextBuffer, blink_on: bool) {
        // Resize window + texture when VioManager dimensions change.
        if buf.cols != self.current_cols || buf.rows != self.current_rows {
            self.handle_resize(buf.cols, buf.rows);
        }

        let (win_w, win_h) = (self.current_cols as usize * 8, self.current_rows as usize * 16);
        render_cells(buf, blink_on, &mut self.pixels, win_w, win_h);

        self.present();
    }

    fn poll_events(&mut self, shared: &Arc<SharedState>) -> bool {
        use sdl3::event::Event;
        use sdl3::keyboard::{Keycode, Mod as KeyMod};

        // Audio commands from the vCPU threads.
        self.audio.pump(&shared.audio);

        // Use wait_event_timeout so the thread sleeps inside SDL when idle
        // instead of busy-polling with poll_iter().  The 8 ms timeout gives
        // ~125 Hz frame rate (matching the PM renderer) while the OS
        // scheduler can keep this core idle between events.
        let maybe_event = self.event_pump.wait_event_timeout(std::time::Duration::from_millis(8));
        // A scripted session ignores the host keyboard, so it runs the same
        // on a real display as on the dummy driver.
        let scripted = self.script.is_some();
        // Host pointer positions are in window pels; the screen is scaled,
        // so they are converted to the logical text screen (`cols × 8` by
        // `rows × 16`). Scripted input is already in those coordinates.
        let canvas = &self.canvas;
        let mut events: Vec<Event> = maybe_event.into_iter().chain(self.event_pump.poll_iter())
            .filter(|e| !(scripted && crate::gui::sdl_renderer::is_host_input(e)))
            .map(|mut e| { e.convert_coords(canvas); e })
            .collect();
        // Dumps wait until the input scheduled before them has been queued.
        let mut dumps = Vec::new();
        let due = self.script.as_mut().map(super::gui_script::GuiScript::due).unwrap_or_default();
        for input in due {
            match input {
                super::gui_script::Input::Dump(path) => dumps.push(path),
                input => events.extend(crate::gui::sdl_renderer::script_input_to_event(&input)),
            }
        }

        for event in events {
            match event {
                Event::Quit { .. } => return false,
                // SDL sends Quit only when the last window closes; after
                // Alt+Home the hidden desktop window still exists.
                Event::Window { win_event: sdl3::event::WindowEvent::CloseRequested, window_id, .. }
                    if window_id == self.canvas.window().id() => return false,
                Event::KeyDown { keycode: Some(kc), scancode: Some(sc), keymod, repeat: false, .. } => {
                    // Ctrl+Esc ends the session, as closing the window does.
                    if kc == Keycode::Escape
                        && (keymod.contains(KeyMod::LCTRLMOD) || keymod.contains(KeyMod::RCTRLMOD))
                    {
                        return false;
                    }
                    // Alt+Home: back to the PM window. The loop returns;
                    // input after it in this pass is dropped.
                    if super::vio_switch::request_switch(shared, kc, keymod, false) {
                        return false;
                    }

                    kbd_key_down(shared, kc, sc, keymod);
                }

                // TextInput carries UTF-8 text composed by the IME (e.g. CJK characters).
                // For ASCII (text == single byte < 0x80) the KeyDown handler already
                // enqueued the character, so skip it here to avoid double-delivery.
                // For non-ASCII (DBCS or multi-byte sequences), re-encode to the active
                // codepage and push each resulting byte as a separate KbdKeyInfo.
                Event::TextInput { text, .. } => kbd_text_input(shared, &text),

                // The program's mouse (MOUCALLS): the cell under the pointer.
                Event::MouseMotion { x, y, .. } => {
                    let (row, col) = self.cell_at(x, y);
                    crate::loader::moucalls::mouse_moved(shared, row, col);
                }
                Event::MouseButtonDown { mouse_btn, x, y, .. } | Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                    let down = matches!(event, Event::MouseButtonDown { .. });
                    if let Some(b) = crate::gui::sdl_renderer::mou_button(mouse_btn) {
                        let (row, col) = self.cell_at(x, y);
                        crate::loader::moucalls::mouse_button(shared, row, col, b, down);
                    }
                }

                _ => {}
            }
        }
        for path in dumps {
            self.dump_frame(shared, &path);
        }
        true
    }

    fn frame_sleep(&self) {
        // No additional sleep needed — wait_event_timeout(8) in poll_events
        // already provides the frame-rate throttle and CPU yield.
    }
}

/// A key press for a VIO program (`KbdCharIn`): the character, scan code and
/// shift state go into `kbd_queue`. Pure modifier keys are not delivered —
/// OS/2 reports their state with the next real keystroke. Used by the text
/// window and the VIO window on the PM desktop.
pub fn kbd_key_down(shared: &SharedState, kc: sdl3::keyboard::Keycode, sc: sdl3::keyboard::Scancode,
                    keymod: sdl3::keyboard::Mod) {
    use sdl3::keyboard::{Keycode, Mod as KeyMod};
    use crate::gui::sdl_renderer::sdl_scancode_to_os2;
    if matches!(kc,
        Keycode::LShift | Keycode::RShift |
        Keycode::LCtrl  | Keycode::RCtrl  |
        Keycode::LAlt   | Keycode::RAlt   |
        Keycode::CapsLock | Keycode::NumLockClear | Keycode::ScrollLock |
        Keycode::LGui   | Keycode::RGui
    ) {
        return;
    }
    // Ctrl+C and Ctrl+Break are signal exceptions for the program, not
    // keystrokes (the KBD subsystem's ASCII mode).
    let ctrl = keymod.intersects(KeyMod::LCTRLMOD | KeyMod::RCTRLMOD);
    if let Some(signal) = crate::loader::signals::key_signal(
        ctrl, kc == Keycode::C, kc == Keycode::Pause, kc == Keycode::Cancel) {
        crate::loader::signals::post_signal(shared, signal);
        return;
    }
    let scan = sdl_scancode_to_os2(sc);
    let ch   = sdl_keycode_to_text_char(kc, keymod);
    let mut state: u16 = 0;
    if keymod.intersects(KeyMod::LSHIFTMOD | KeyMod::RSHIFTMOD) { state |= 0x0008; }
    if keymod.intersects(KeyMod::LCTRLMOD  | KeyMod::RCTRLMOD)  { state |= 0x0004; }
    if keymod.intersects(KeyMod::LALTMOD   | KeyMod::RALTMOD)   { state |= 0x0002; }
    shared.kbd_queue.lock_or_recover().push_back(KbdKeyInfo { ch, scan, state });
    shared.kbd_cond.notify_one();
}

/// Text from SDL `TextInput` for a VIO program. Single ASCII characters
/// already came with their key press ([`kbd_key_down`]); anything else
/// (IME-composed CJK, accented letters) is encoded in the active codepage and
/// queued byte by byte with scan code 0.
pub fn kbd_text_input(shared: &SharedState, text: &str) {
    use crate::loader::codepage::cp_encode;
    if text.is_empty() || (text.len() == 1 && text.as_bytes()[0] < 0x80) {
        return;
    }
    let cp = shared.active_codepage.load(Ordering::Relaxed);
    {
        let mut q = shared.kbd_queue.lock_or_recover();
        for byte in cp_encode(text, cp) {
            q.push_back(KbdKeyInfo { ch: byte, scan: 0, state: 0 });
        }
    }
    shared.kbd_cond.notify_one();
}

/// Convert an SDL `Keycode` + modifier flags to an OS/2 ASCII character byte.
///
/// Returns 0x00 for pure extended/navigation keys (arrows, F-keys, etc.).
fn sdl_keycode_to_text_char(kc: sdl3::keyboard::Keycode, mods: sdl3::keyboard::Mod) -> u8 {
    use sdl3::keyboard::{Keycode, Mod as KeyMod};

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

fn keycode_to_alpha(kc: sdl3::keyboard::Keycode) -> Option<char> {
    use sdl3::keyboard::Keycode;
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

fn keycode_to_digit_or_symbol(kc: sdl3::keyboard::Keycode, shifted: bool) -> Option<u8> {
    use sdl3::keyboard::Keycode;
    Some(match kc {
        Keycode::_0 => if shifted { b')' } else { b'0' },
        Keycode::_1 => if shifted { b'!' } else { b'1' },
        Keycode::_2 => if shifted { b'@' } else { b'2' },
        Keycode::_3 => if shifted { b'#' } else { b'3' },
        Keycode::_4 => if shifted { b'$' } else { b'4' },
        Keycode::_5 => if shifted { b'%' } else { b'5' },
        Keycode::_6 => if shifted { b'^' } else { b'6' },
        Keycode::_7 => if shifted { b'&' } else { b'7' },
        Keycode::_8 => if shifted { b'*' } else { b'8' },
        Keycode::_9 => if shifted { b'(' } else { b'9' },
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
        Keycode::Apostrophe        => if shifted { b'"' } else { b'\'' },
        Keycode::Comma        => if shifted { b'<' } else { b',' },
        Keycode::Period       => if shifted { b'>' } else { b'.' },
        Keycode::Slash        => if shifted { b'?' } else { b'/' },
        Keycode::Grave    => if shifted { b'~' } else { b'`' },
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
        for (r, &row) in g.iter().enumerate().take(16) {
            let want = if r < 8 { 0x00 } else { 0xFF };
            assert_eq!(row, want, "lower half block: row {r}");
        }
    }

    #[test]
    fn upper_half_block_correct() {
        let g = get_glyph_for_char('▀'); // U+2580
        for (r, &row) in g.iter().enumerate().take(16) {
            let want = if r < 8 { 0xFF } else { 0x00 };
            assert_eq!(row, want, "upper half block: row {r}");
        }
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
    fn a_display_switch_returns_without_ending_the_program() {
        let shared = make_shared();
        shared.vio_switch_requested.store(true, Ordering::Relaxed);
        // poll_events says stop, as the text window does on Alt+Home.
        let mut r = HeadlessTextRenderer { frame_count: 0, keep_running: false };
        run_text_loop(&mut r, shared.clone());
        assert!(!shared.exit_requested.load(Ordering::Relaxed));
        assert_eq!(r.frame_count, 0);
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

    // ── get_glyph_dbcs (Phase B5 — real Unifont wide-glyph lookup) ──────────

    #[test]
    fn glyph_dbcs_row_stride_is_two_bytes() {
        // Sanity check: glyph is exactly 32 bytes (16 rows × 2 bytes per row).
        let g = get_glyph_dbcs('中');
        assert_eq!(g.len(), 32);
    }

    #[test]
    fn glyph_dbcs_cjk_ideograph_is_non_blank() {
        // U+4E2D (中, CJK UNIFIED IDEOGRAPH) — must have real pixels in Unifont.
        let g = get_glyph_dbcs('\u{4E2D}');
        assert!(g.iter().any(|&b| b != 0), "中 (U+4E2D) should have a non-blank glyph");
    }

    #[test]
    fn glyph_dbcs_hangul_syllable_is_non_blank() {
        // U+AC00 (가, HANGUL SYLLABLE GA) — must have real pixels in Unifont.
        let g = get_glyph_dbcs('\u{AC00}');
        assert!(g.iter().any(|&b| b != 0), "가 (U+AC00) should have a non-blank glyph");
    }

    #[test]
    fn glyph_dbcs_hiragana_is_non_blank() {
        // U+3042 (あ, HIRAGANA LETTER A) — in the Unifont wide table.
        let g = get_glyph_dbcs('\u{3042}');
        assert!(g.iter().any(|&b| b != 0), "あ (U+3042) should have a non-blank glyph");
    }

    #[test]
    fn glyph_dbcs_private_use_area_is_blank() {
        // U+E000 is in the Private Use Area — not defined in Unifont wide table.
        assert_eq!(get_glyph_dbcs('\u{E000}'), [0u8; 32]);
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

    /// Snapshot must fix-up DbcsLead cells so cells[i].0 is the correct
    /// Unicode codepoint, not the U+FFFD that single-byte decode would produce.
    #[test]
    fn snapshot_dbcs_lead_cell_decoded_correctly() {
        use crate::loader::locale::is_dbcs_lead_byte;
        let shared = make_shared();
        // Write a CP936 DBCS pair (你 = 0xC4 0xE3) into the VIO buffer at (0,0).
        {
            let mut vio = shared.console_mgr.lock_or_recover();
            vio.codepage = 936;
            // Manually plant raw bytes for the DBCS pair.
            vio.raw_bytes[0] = 0xC4;
            vio.raw_bytes[1] = 0xE3;
            // cells[0].0 is set to U+FFFD by single-byte decode (normal write path).
            vio.buffer[0] = ('\u{FFFD}', 0x07);
            vio.buffer[1] = ('\u{FFFD}', 0x07);
        }
        assert!(is_dbcs_lead_byte(0xC4, 936)); // sanity
        let buf = VgaTextBuffer::snapshot(&shared);
        assert_eq!(buf.cell_kind[0], CellKind::DbcsLead);
        assert_eq!(buf.cell_kind[1], CellKind::DbcsTail);
        // After snapshot fix-up, cells[0].0 must be 你 (U+4F60).
        assert_eq!(buf.cells[0].0, '\u{4F60}',
            "DbcsLead cell char should be decoded to 你, got {:?}", buf.cells[0].0);
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
        use sdl3::keyboard::{Keycode, Mod as KeyMod};
        let shift_mod = KeyMod::LSHIFTMOD;
        for kc in [Keycode::LShift, Keycode::RShift, Keycode::LCtrl, Keycode::RCtrl,
                   Keycode::LAlt,   Keycode::RAlt,   Keycode::CapsLock] {
            let ch = sdl_keycode_to_text_char(kc, shift_mod);
            assert_eq!(ch, 0x00, "{:?} should produce ch=0x00, got 0x{:02X}", kc, ch);
        }
    }

    #[test]
    fn window_points_map_to_the_nearest_cell() {
        assert_eq!(cell_in_window(0.0, 0.0, 80, 25), (0, 0));
        assert_eq!(cell_in_window(15.9, 31.9, 80, 25), (1, 1));
        assert_eq!(cell_in_window(639.0, 399.0, 80, 25), (24, 79));
        assert_eq!(cell_in_window(-4.0, 900.0, 80, 25), (24, 0));
    }

    #[test]
    fn the_mouse_pointer_is_drawn_in_the_full_screen_snapshot() {
        let shared = crate::loader::Loader::new_mock().get_shared();
        shared.console_mgr.lock_or_recover().enable_sdl_mode();
        let attr_at = |row: usize, col: usize| full_screen_snapshot(&shared).cells[row * 80 + col].1;
        let plain = attr_at(12, 40);
        shared.mou_mgr.lock_or_recover().open(25, 80);
        assert_eq!(attr_at(12, 40), plain, "not drawn before MouDrawPtr");
        shared.mou_mgr.lock_or_recover().draw_pointer();
        let p = crate::loader::moucalls::text_pointer(&shared).unwrap();
        assert_eq!(attr_at(12, 40), p.apply_attr(plain));
        assert_ne!(attr_at(12, 40), plain);
        crate::loader::moucalls::mouse_moved(&shared, 0, 0);
        assert_eq!((attr_at(12, 40), attr_at(0, 0)), (plain, p.apply_attr(plain)));
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

    // ── B8 — DBCS TextInput re-encoding ──────────────────────────────────────

    /// Verify that the re-encoding logic used by the TextInput event handler
    /// produces the correct DBCS byte sequence.  (The handler itself calls
    /// `cp_encode(text, cp)` and pushes each byte — this test validates that.)
    #[test]
    fn dbcs_text_input_cp936_encodes_to_gbk() {
        use crate::loader::codepage::cp_encode;
        // 你 in GBK is 0xC4 0xE3
        let bytes = cp_encode("你", 936);
        assert_eq!(bytes, vec![0xC4, 0xE3],
            "cp_encode should encode 你 to GBK 0xC4 0xE3");
    }

    #[test]
    fn dbcs_text_input_cp932_encodes_to_sjis() {
        use crate::loader::codepage::cp_encode;
        // あ in Shift-JIS is 0x82 0xA0
        let bytes = cp_encode("あ", 932);
        assert_eq!(bytes, vec![0x82, 0xA0],
            "cp_encode should encode あ to SJIS 0x82 0xA0");
    }

    #[test]
    fn dbcs_text_input_ascii_passthrough_unchanged() {
        use crate::loader::codepage::cp_encode;
        // ASCII chars must encode to themselves in any SBCS codepage.
        let bytes = cp_encode("A", 437);
        assert_eq!(bytes, vec![b'A']);
    }
}
