// SPDX-License-Identifier: GPL-3.0-only

//! Desktop compositors: how the PM frames get onto the host window.
//!
//! [`SdlRenderer`](super::SdlRenderer) works out *what* to show — frame
//! placements, chrome pixmaps, damage — and hands it to a [`Compositor`] as
//! a [`Scene`]. Two backends exist:
//!
//! * [`SoftwareCompositor`] keeps a desktop-sized pixel buffer, redraws only
//!   the damaged rectangles into it and uploads those to one streaming
//!   texture, on SDL's `software` renderer. Its output is the reference for
//!   the golden-image tests and it is the default on the headless `dummy` /
//!   `offscreen` video drivers.
//! * [`LayeredCompositor`] keeps one texture per frame (client content plus
//!   chrome) and one per open pull-down, re-renders a frame's texture only
//!   when its content or chrome changes, and draws the desktop as textured
//!   quads on whichever SDL renderer the platform offers (OpenGL, Vulkan,
//!   Metal, Direct3D, or SDL_GPU via `SDL_RENDER_DRIVER=gpu`) with vsync.
//!   Moving a window costs no pixel work, and nothing is copied
//!   desktop-wide on the CPU. It is the default on real displays.
//!
//! `WARPINE_COMPOSITOR=software|layered` overrides the choice. GPI drawing
//! stays on the CPU either way (the surfaces in `gui/surface.rs`).

use std::collections::HashSet;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use log::{info, warn};
use sdl3::pixels::{Color, PixelFormat};
use sdl3::render::{BlendMode, Canvas, FRect, Texture, TextureCreator};
use sdl3::video::{Window, WindowContext};
use crate::gui::damage::{Damage, Rect, blit_clipped, fill_clipped};
use crate::gui::surface::{MAX_SURFACE_DIM, SharedSurface};
use crate::loader::MutexExt;
use super::{ChromeCache, FramePlacement, DESKTOP_BG, MENU_DROPDOWN_W, draw_placed_frame};

/// Environment variable that picks the compositor (`software` or `layered`).
pub(super) const COMPOSITOR_ENV: &str = "WARPINE_COMPOSITOR";

/// SDL's software renderer, by name.
const SOFTWARE_RENDERER: &std::ffi::CStr = c"software";

/// SDL hint asking the renderer to synchronise presents with the display.
const RENDER_VSYNC_HINT: &str = "SDL_RENDER_VSYNC";

/// Largest frame texture, per side. Client surfaces are capped at
/// `MAX_SURFACE_DIM`, so a larger frame would add only chrome; GPUs that
/// cannot hold this much fail texture creation and the frame is skipped.
const MAX_LAYER_DIM: i32 = MAX_SURFACE_DIM as i32;

/// Window title of the desktop.
const DESKTOP_TITLE: &str = "Warpine \u{2014} OS/2 Compatibility Layer";

/// One frame of a [`Scene`].
pub(super) struct SceneFrame<'a> {
    pub placement: &'a FramePlacement,
    pub surface: &'a SharedSurface,
    pub chrome: &'a ChromeCache,
}

/// Something drawn over all frames: an open pop-up menu.
pub(super) struct Overlay<'a> {
    /// Where it goes on the desktop.
    pub rect: Rect,
    /// Its pixels, `rect`-sized, top row first.
    pub pixels: &'a [u32],
    /// Changes whenever `pixels` or `rect` do.
    pub generation: u64,
}

/// What to show: the visible frames back to front, and what changed.
pub(super) struct Scene<'a> {
    pub frames: Vec<SceneFrame<'a>>,
    /// Desktop areas whose pixels may differ from the last composite.
    pub damage: &'a Damage,
    /// Frames whose chrome (title, border, menu, pull-down) changed.
    pub chrome_changed: &'a HashSet<u32>,
    /// Everything must be redrawn (start-up, host window resized or
    /// exposed, render device reset).
    pub full: bool,
    /// Drawn over the frames.
    pub overlay: Option<Overlay<'a>>,
}

/// Puts a [`Scene`] on the desktop window.
pub(super) trait Compositor {
    /// Backend and SDL renderer, for logs.
    fn describe(&self) -> String;
    /// The desktop window.
    fn window(&self) -> &Window;
    /// Desktop size in pixels.
    fn size(&self) -> (u32, u32);
    /// The host window is now `w` × `h`.
    fn resize(&mut self, w: u32, h: u32);
    /// Draw `scene` and present it; returns the time spent uploading and
    /// presenting.
    fn compose(&mut self, scene: &Scene) -> Duration;
    /// The desktop as last composed: `(pixels, width, height)`, pixels
    /// `0x00RRGGBB`, top row first.
    fn snapshot(&mut self) -> (Vec<u32>, u32, u32);
}

/// Which [`Compositor`] to create.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompositorKind {
    Software,
    Layered,
}

impl CompositorKind {
    /// The backend named by `requested` (the [`COMPOSITOR_ENV`] value), else
    /// `Layered` on a real display and `Software` on the headless drivers,
    /// whose output the golden-image tests compare.
    pub fn choose(requested: Option<&str>, real_display: bool) -> CompositorKind {
        match requested.map(str::trim) {
            Some(s) if s.eq_ignore_ascii_case("software") => CompositorKind::Software,
            Some(s) if s.eq_ignore_ascii_case("layered") => CompositorKind::Layered,
            other => {
                if let Some(s) = other.filter(|s| !s.is_empty()) {
                    warn!("[GUI] {COMPOSITOR_ENV}={s} is not 'software' or 'layered'; using the default");
                }
                if real_display { CompositorKind::Layered } else { CompositorKind::Software }
            }
        }
    }
}

fn desktop_window(video: &sdl3::VideoSubsystem, w: u32, h: u32) -> Window {
    video.window(DESKTOP_TITLE, w, h)
        .position_centered()
        .resizable()
        .build()
        .expect("Failed to create desktop SDL window")
}

/// Create the desktop window and a `kind` compositor for it. A layered
/// compositor falls back to software if no renderer can be created.
pub(super) fn create(video: &sdl3::VideoSubsystem, w: u32, h: u32, kind: CompositorKind)
                     -> Box<dyn Compositor> {
    let compositor: Box<dyn Compositor> = match kind {
        CompositorKind::Software => Box::new(SoftwareCompositor::new(desktop_window(video, w, h))),
        CompositorKind::Layered => {
            sdl3::hint::set(RENDER_VSYNC_HINT, "1");
            // Let SDL pick the best renderer (`SDL_RENDER_DRIVER` overrides).
            match sdl3::render::create_renderer(desktop_window(video, w, h), None) {
                Ok(canvas) => Box::new(LayeredCompositor::new(canvas)),
                Err(e) => {
                    warn!("[GUI] no renderer for the layered compositor ({e}); using software");
                    Box::new(SoftwareCompositor::new(desktop_window(video, w, h)))
                }
            }
        }
    };
    info!("[GUI] compositor: {}", compositor.describe());
    compositor
}

/// A streaming texture of `w` × `h` pixels (XRGB, no blending).
fn xrgb_texture(creator: &'static TextureCreator<WindowContext>, w: u32, h: u32)
                -> Result<Texture<'static>, String> {
    let mut t = creator
        .create_texture_streaming(PixelFormat::ARGB8888, w, h)
        .map_err(|e| e.to_string())?;
    t.set_blend_mode(BlendMode::None);
    Ok(t)
}

/// Upload the `src` rectangle of `buf` (rows `buf_w` pixels apart) to the
/// same-sized rectangle at `dst` of `texture`.
///
/// `Texture::update` (`SDL_UpdateTexture`) reads `height` rows of `width`
/// pixels, `buf_w * 4` bytes apart, from the start of the slice it gets, and
/// sdl3-rs does not check that the slice holds them; this function does, and
/// refuses rectangles that do not lie inside `buf`.
fn upload(texture: &mut Texture, dst: (i32, i32), buf: &[u32], buf_w: usize, src: Rect)
          -> Result<(), String> {
    if src.is_empty() { return Ok(()); }
    if dst.0 < 0 || dst.1 < 0 {
        return Err(format!("upload to {dst:?} outside the texture"));
    }
    let Some(span) = upload_span(src, buf.len(), buf_w) else {
        return Err(format!("upload of {src:?} outside the {buf_w}-wide buffer of {} pixels", buf.len()));
    };
    let pixels = &buf[span];
    let area = sdl3::rect::Rect::new(dst.0, dst.1, src.width() as u32, src.height() as u32);
    texture.update(area, bytemuck::cast_slice(pixels), buf_w * 4).map_err(|e| e.to_string())
}

/// The pixels SDL reads to upload the non-empty rectangle `src` of a
/// `buf_w`-wide buffer of `len` pixels: from its first pixel to its last;
/// `None` unless `src` lies inside the buffer.
fn upload_span(src: Rect, len: usize, buf_w: usize) -> Option<std::ops::Range<usize>> {
    let buf_h = len / buf_w.max(1);
    let inside = src.x0 >= 0 && src.y0 >= 0 && !src.is_empty()
        && src.x1 as usize <= buf_w && src.y1 as usize <= buf_h;
    inside.then(|| src.y0 as usize * buf_w + src.x0 as usize
                   ..(src.y1 as usize - 1) * buf_w + src.x1 as usize)
}

fn frect(r: Rect) -> FRect {
    FRect::new(r.x0 as f32, r.y0 as f32, r.width() as f32, r.height() as f32)
}

fn bg_color() -> Color {
    Color::RGB((DESKTOP_BG >> 16) as u8, (DESKTOP_BG >> 8) as u8, DESKTOP_BG as u8)
}

// ── Software ──────────────────────────────────────────────────────────────

/// CPU compositing into one desktop buffer, uploaded to one texture.
pub(super) struct SoftwareCompositor {
    canvas:  Canvas<Window>,
    /// The canvas's texture factory, kept for the life of the process so the
    /// lifetime-checked texture can be `'static` (no `unsafe_textures`).
    creator: &'static TextureCreator<WindowContext>,
    texture: Texture<'static>,
    buffer:  Vec<u32>,
    width:   u32,
    height:  u32,
}

impl SoftwareCompositor {
    fn new(window: Window) -> Self {
        // All drawing is done on the CPU; SDL's software renderer avoids a
        // GPU round trip for one full-window texture.
        let canvas = sdl3::render::create_renderer(window, Some(SOFTWARE_RENDERER))
            .expect("Failed to create desktop canvas");
        let (w, h) = canvas.output_size().expect("output_size failed");
        let creator: &'static TextureCreator<WindowContext> =
            Box::leak(Box::new(canvas.texture_creator()));
        let texture = xrgb_texture(creator, w, h).expect("Failed to create desktop texture");
        let n = (w as usize).saturating_mul(h as usize);
        SoftwareCompositor { canvas, creator, texture, buffer: vec![DESKTOP_BG; n], width: w, height: h }
    }

    /// Upload the `damaged` parts of `buffer` to the texture and present.
    /// The texture keeps the rest from earlier frames.
    ///
    /// Uses `Texture::update` (`SDL_UpdateTexture`), which reads each
    /// rectangle straight out of `buffer` with the buffer's row pitch.
    fn present(&mut self, damaged: &[Rect]) {
        let w = self.width as usize;
        for r in damaged {
            let r = r.intersect(&Rect::at(0, 0, self.width as i32, self.height as i32));
            upload(&mut self.texture, (r.x0, r.y0), &self.buffer, w, r)
                .expect("desktop texture update failed");
        }
        self.canvas.copy(&self.texture, None::<FRect>, None::<FRect>).expect("canvas copy failed");
        self.canvas.present();
    }
}

impl Compositor for SoftwareCompositor {
    fn describe(&self) -> String {
        format!("software (SDL renderer '{}')", self.canvas.renderer_name)
    }

    fn window(&self) -> &Window { self.canvas.window() }

    fn size(&self) -> (u32, u32) { (self.width, self.height) }

    fn resize(&mut self, w: u32, h: u32) {
        self.width  = w;
        self.height = h;
        self.texture = xrgb_texture(self.creator, w, h).expect("Failed to create desktop texture");
        self.buffer = vec![DESKTOP_BG; (w as usize).saturating_mul(h as usize)];
    }

    /// Each damaged rectangle is filled with the desktop background and the
    /// frames that reach into it are drawn back to front, skipping frames
    /// an opaque frame above hides there.
    fn compose(&mut self, scene: &Scene) -> Duration {
        let dw = self.width as usize;
        for &r in scene.damage.rects() {
            fill_clipped(&mut self.buffer, dw, r, r, DESKTOP_BG);
            for (i, f) in scene.frames.iter().enumerate() {
                let p = f.placement;
                let visible = p.extent().intersect(&r);
                if visible.is_empty()
                    || scene.frames[i + 1..].iter()
                        .any(|q| q.placement.opaque && q.placement.frame.contains(&visible)) {
                    continue;
                }
                // The surface may have been resized since `p` was computed;
                // the blit clips to its current size.
                let fb = f.surface.lock_or_recover();
                draw_placed_frame(&mut self.buffer, dw, p, &fb, f.chrome, r);
            }
            if let Some(o) = &scene.overlay {
                draw_overlay(&mut self.buffer, dw, o, r);
            }
        }
        let started = Instant::now();
        self.present(scene.damage.rects());
        started.elapsed()
    }

    fn snapshot(&mut self) -> (Vec<u32>, u32, u32) {
        (self.buffer.clone(), self.width, self.height)
    }
}

/// Draw `o` into a desktop buffer `buf_w` pels wide, only inside `clip`.
fn draw_overlay(buf: &mut [u32], buf_w: usize, o: &Overlay, clip: Rect) {
    let (w, h) = (o.rect.width(), o.rect.height());
    if w == 0 || o.pixels.len() < w as usize * h as usize { return; }
    blit_clipped(buf, buf_w, o.pixels, w as usize, o.rect.x0, o.rect.y0, w, h, clip);
}

// ── Layered ───────────────────────────────────────────────────────────────

/// A frame's texture and what it was rendered from.
struct Layer {
    texture: Texture<'static>,
    /// Frame size the texture holds (clamped to [`MAX_LAYER_DIM`]).
    size: (i32, i32),
    /// Client surface generation and size it holds.
    generation: u64,
    fb_size: (u32, u32),
    /// The parts of the frame it paints, frame-local (see [`painted_parts`]).
    parts: Vec<Rect>,
    /// The open pull-down's texture and height.
    pulldown: Option<(Texture<'static>, i32)>,
}

/// One frame to draw: its texture's top-left on the desktop and where its
/// pull-down goes.
struct DrawItem {
    hwnd: u32,
    origin: (i32, i32),
    pulldown: Rect,
}

/// One texture per frame, drawn as quads by the SDL renderer.
pub(super) struct LayeredCompositor {
    canvas:  Canvas<Window>,
    creator: &'static TextureCreator<WindowContext>,
    layers:  HashMap<u32, Layer>,
    /// Back to front, from the last `compose`.
    draw_list: Vec<DrawItem>,
    /// Frame pixels being rendered, reused between frames.
    scratch: Vec<u32>,
    /// The overlay's texture, where it goes and the generation it holds.
    overlay: Option<(Texture<'static>, Rect, u64)>,
    width:   u32,
    height:  u32,
}

/// The parts of a frame that its pixels cover, as rectangles relative to
/// the desktop like `p`: the whole frame except any part of the client area
/// its surface (`fb_w` × `fb_h`, drawn from the client's top-left) does not
/// reach. Frames below show through that part, as with the software
/// compositor; it exists only while a surface lags a window resize.
fn painted_parts(p: &FramePlacement, fb_w: u32, fb_h: u32) -> Vec<Rect> {
    let c = p.client;
    let covered = Rect::at(c.x0, c.y0, fb_w.min(i32::MAX as u32) as i32,
                           fb_h.min(i32::MAX as u32) as i32).intersect(&c);
    let mut parts = vec![p.frame];
    for hole in c.subtract(&covered) {
        parts = parts.iter().flat_map(|r| r.subtract(&hole)).collect();
    }
    parts
}

/// Where changed client pels go in a frame's texture: `(dst, src)`, the
/// texture rectangle and the same-sized rectangle of the client surface.
/// `changed` is in surface coordinates; the texture is `size` (frame-local).
///
/// The client is drawn from its top-left corner at `p.client`, but the
/// frame border is drawn on top of it (the client reaches the frame's
/// bottom edge), so only the part inside the border ring may be replaced.
fn partial_upload(p: &FramePlacement, changed: Rect, size: (i32, i32)) -> (Rect, Rect) {
    let client = p.client.offset(-p.frame.x0, -p.frame.y0);
    let bw = super::frame_border_px(p.frame_flags);
    let inside_border = Rect::new(bw, bw, size.0 - bw, size.1 - bw);
    let dst = changed.offset(client.x0, client.y0)
        .intersect(&client).intersect(&inside_border).intersect(&Rect::at(0, 0, size.0, size.1));
    (dst, dst.offset(-client.x0, -client.y0))
}

impl FramePlacement {
    /// The same placement moved by `(dx, dy)`, without its pull-down.
    fn moved_without_pulldown(&self, dx: i32, dy: i32) -> FramePlacement {
        FramePlacement {
            hwnd: self.hwnd,
            frame_flags: self.frame_flags,
            frame: self.frame.offset(dx, dy),
            title: self.title.offset(dx, dy),
            menu_bar: self.menu_bar.offset(dx, dy),
            pulldown: Rect::EMPTY,
            client: self.client.offset(dx, dy),
            opaque: self.opaque,
        }
    }
}

impl LayeredCompositor {
    fn new(canvas: Canvas<Window>) -> Self {
        let (w, h) = canvas.output_size().expect("output_size failed");
        let creator: &'static TextureCreator<WindowContext> =
            Box::leak(Box::new(canvas.texture_creator()));
        LayeredCompositor { canvas, creator, layers: HashMap::new(), draw_list: Vec::new(),
                            scratch: Vec::new(), overlay: None, width: w, height: h }
    }

    /// Bring `f`'s textures up to date; returns whether the frame can be
    /// drawn.
    fn update_layer(&mut self, f: &SceneFrame, full: bool, chrome_changed: bool) -> bool {
        let p = f.placement;
        let (w, h) = (p.frame.width().min(MAX_LAYER_DIM), p.frame.height().min(MAX_LAYER_DIM));
        if w == 0 || h == 0 {
            self.layers.remove(&p.hwnd);
            return false;
        }
        let mut fb = f.surface.lock_or_recover();
        // Same size and chrome: only the client's changed pels need copying.
        let reusable = !full && !chrome_changed && self.layers.get(&p.hwnd)
            .is_some_and(|l| l.size == (w, h) && l.fb_size == (fb.width, fb.height));
        if reusable {
            let layer = self.layers.get_mut(&p.hwnd).expect("checked above");
            if layer.generation != fb.generation() {
                let (dst, src) = partial_upload(p, fb.take_changes(), (w, h));
                log::trace!("[GUI] frame {:#x}: partial upload {src:?}", p.hwnd);
                if let Err(e) = upload(&mut layer.texture, (dst.x0, dst.y0), &fb.buffer, fb.width as usize, src) {
                    warn!("[GUI] frame {:#x} texture upload failed: {e}", p.hwnd);
                }
                layer.generation = fb.generation();
            }
            drop(fb);
        } else {
            // Render the frame at the origin, without its pull-down (which
            // has its own texture), and upload all of it.
            let local = p.moved_without_pulldown(-p.frame.x0, -p.frame.y0);
            self.scratch.clear();
            self.scratch.resize(w as usize * h as usize, DESKTOP_BG);
            draw_placed_frame(&mut self.scratch, w as usize, &local, &fb, f.chrome, Rect::at(0, 0, w, h));
            let parts: Vec<Rect> = painted_parts(&local, fb.width, fb.height).iter()
                .map(|r| r.intersect(&Rect::at(0, 0, w, h)))
                .filter(|r| !r.is_empty())
                .collect();
            fb.take_changes();
            let (generation, fb_size) = (fb.generation(), (fb.width, fb.height));
            drop(fb);
            let reuse = self.layers.remove(&p.hwnd).filter(|l| l.size == (w, h));
            let mut texture = match reuse.map(|l| Ok(l.texture))
                .unwrap_or_else(|| xrgb_texture(self.creator, w as u32, h as u32)) {
                Ok(t) => t,
                Err(e) => {
                    warn!("[GUI] no {w}x{h} texture for frame {:#x}: {e}", p.hwnd);
                    return false;
                }
            };
            if let Err(e) = upload(&mut texture, (0, 0), &self.scratch, w as usize, Rect::at(0, 0, w, h)) {
                warn!("[GUI] frame {:#x} texture upload failed: {e}", p.hwnd);
            }
            self.layers.insert(p.hwnd, Layer { texture, size: (w, h), generation, fb_size,
                                               parts, pulldown: None });
        }
        let stale = !reusable;

        // The pull-down's texture follows the chrome.
        let pd = f.chrome.menu.as_ref().and_then(|m| m.pulldown.as_ref());
        let Some(layer) = self.layers.get_mut(&p.hwnd) else { return false };
        match pd {
            None => layer.pulldown = None,
            Some(pd) if stale || layer.pulldown.as_ref().is_none_or(|(_, h)| *h != pd.h) => {
                layer.pulldown = xrgb_texture(self.creator, MENU_DROPDOWN_W as u32, pd.h as u32)
                    .map_err(|e| warn!("[GUI] no pull-down texture: {e}"))
                    .ok()
                    .and_then(|mut t| {
                        upload(&mut t, (0, 0), &pd.pixels, MENU_DROPDOWN_W as usize,
                               Rect::at(0, 0, MENU_DROPDOWN_W, pd.h))
                            .map_err(|e| warn!("[GUI] pull-down upload failed: {e}"))
                            .ok()?;
                        Some((t, pd.h))
                    });
            }
            Some(_) => {}
        }
        true
    }

    /// Draw the desktop from the layers into the back buffer.
    fn draw(&mut self) {
        self.canvas.set_draw_color(bg_color());
        self.canvas.clear();
        for item in &self.draw_list {
            let Some(layer) = self.layers.get(&item.hwnd) else { continue };
            for &part in &layer.parts {
                let dst = part.offset(item.origin.0, item.origin.1);
                if let Err(e) = self.canvas.copy(&layer.texture, frect(part), frect(dst)) {
                    warn!("[GUI] drawing frame {:#x} failed: {e}", item.hwnd);
                }
            }
            if let Some((pd, _)) = &layer.pulldown && !item.pulldown.is_empty()
                && let Err(e) = self.canvas.copy(pd, None::<FRect>, frect(item.pulldown)) {
                warn!("[GUI] drawing a pull-down failed: {e}");
            }
        }
        if let Some((t, rect, _)) = &self.overlay
            && let Err(e) = self.canvas.copy(t, None::<FRect>, frect(*rect)) {
            warn!("[GUI] drawing a pop-up menu failed: {e}");
        }
    }

    /// Bring the overlay's texture up to date with `o`.
    fn update_overlay(&mut self, o: Option<&Overlay>, full: bool) {
        let Some(o) = o else {
            self.overlay = None;
            return;
        };
        if !full && self.overlay.as_ref().is_some_and(|(_, _, g)| *g == o.generation) {
            return;
        }
        let (w, h) = (o.rect.width(), o.rect.height());
        self.overlay = (w > 0 && h > 0).then(|| xrgb_texture(self.creator, w as u32, h as u32))
            .and_then(|t| t.map_err(|e| warn!("[GUI] no pop-up menu texture: {e}")).ok())
            .and_then(|mut t| {
                upload(&mut t, (0, 0), o.pixels, w as usize, Rect::at(0, 0, w, h))
                    .map_err(|e| warn!("[GUI] pop-up menu upload failed: {e}"))
                    .ok()?;
                Some((t, o.rect, o.generation))
            });
    }
}

impl Compositor for LayeredCompositor {
    fn describe(&self) -> String {
        format!("layered (SDL renderer '{}')", self.canvas.renderer_name)
    }

    fn window(&self) -> &Window { self.canvas.window() }

    fn size(&self) -> (u32, u32) { (self.width, self.height) }

    fn resize(&mut self, w: u32, h: u32) {
        self.width = w;
        self.height = h;
    }

    /// Re-renders the textures of frames whose content or chrome changed
    /// (all of them when `scene.full`), then draws every frame. Damage is
    /// not needed beyond that: the renderer redraws the whole back buffer.
    fn compose(&mut self, scene: &Scene) -> Duration {
        if scene.full {
            // Textures may have been lost with the render device.
            self.layers.clear();
        }
        let mut draw_list = Vec::with_capacity(scene.frames.len());
        for f in &scene.frames {
            let p = f.placement;
            if self.update_layer(f, scene.full, scene.chrome_changed.contains(&p.hwnd)) {
                draw_list.push(DrawItem { hwnd: p.hwnd, origin: (p.frame.x0, p.frame.y0),
                                          pulldown: p.pulldown });
            }
        }
        self.layers.retain(|h, _| scene.frames.iter().any(|f| f.placement.hwnd == *h));
        self.draw_list = draw_list;
        self.update_overlay(scene.overlay.as_ref(), scene.full);
        let started = Instant::now();
        self.draw();
        self.canvas.present();
        started.elapsed()
    }

    fn snapshot(&mut self) -> (Vec<u32>, u32, u32) {
        // The back buffer is undefined after a present: draw it again and
        // read it before the next present.
        self.draw();
        let read = self.canvas.read_pixels(None)
            .and_then(|s| s.convert_format(PixelFormat::ARGB8888));
        let s = match read {
            Ok(s) => s,
            Err(e) => {
                warn!("[GUI] cannot read the desktop back: {e}");
                return (vec![DESKTOP_BG; self.width as usize * self.height as usize],
                        self.width, self.height);
            }
        };
        let (w, h, pitch) = (s.width(), s.height(), s.pitch() as usize);
        let pixels = s.with_lock(|bytes| {
            let mut out = Vec::with_capacity(w as usize * h as usize);
            for row in bytes.chunks(pitch).take(h as usize) {
                out.extend(row[..w as usize * 4].as_chunks::<4>().0.iter()
                    .map(|&p| u32::from_ne_bytes(p) & 0x00FF_FFFF));
            }
            out
        });
        (pixels, w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::place_frame;
    use crate::loader::{FCF_BORDER, FCF_TITLEBAR};

    #[test]
    fn compositor_kind_from_the_environment_or_the_display() {
        use CompositorKind::*;
        assert_eq!(CompositorKind::choose(None, true), Layered, "real display");
        assert_eq!(CompositorKind::choose(None, false), Software, "headless driver");
        assert_eq!(CompositorKind::choose(Some("software"), true), Software);
        assert_eq!(CompositorKind::choose(Some(" Layered "), false), Layered);
        assert_eq!(CompositorKind::choose(Some("bogus"), false), Software, "unknown: default");
        assert_eq!(CompositorKind::choose(Some(""), true), Layered, "empty: default");
    }

    /// Partial uploads replace client pels only: never the border drawn
    /// over the client's bottom rows, never the title bar.
    #[test]
    fn partial_uploads_stay_inside_the_border() {
        use crate::loader::FCF_SIZEBORDER;
        let p = place_frame(1, 10, 10, 100, 80, FCF_TITLEBAR | FCF_SIZEBORDER, false, None, (96, 60), 600);
        let size = (p.frame.width(), p.frame.height());
        let bw = super::super::frame_border_px(FCF_SIZEBORDER);
        let client = p.client.offset(-p.frame.x0, -p.frame.y0);
        assert_eq!(client.y1, size.1, "the client reaches the frame's bottom edge");
        // The whole surface changed: everything but the border ring.
        let (dst, src) = partial_upload(&p, Rect::at(0, 0, 96, 60), size);
        assert_eq!(dst, Rect::new(client.x0, client.y0, client.x1, size.1 - bw));
        assert_eq!(src, dst.offset(-client.x0, -client.y0));
        assert_eq!((dst.width(), dst.height()), (src.width(), src.height()));
        // A change in the surface's bottom rows only: clipped above the border.
        let (dst, _) = partial_upload(&p, Rect::new(0, 58, 96, 60), size);
        assert!(dst.is_empty() || dst.y1 <= size.1 - bw, "{dst:?}");
        // A change in the middle is passed through unchanged.
        let (dst, src) = partial_upload(&p, Rect::new(5, 5, 9, 7), size);
        assert_eq!(src, Rect::new(5, 5, 9, 7));
        assert_eq!(dst, Rect::new(5, 5, 9, 7).offset(client.x0, client.y0));
        assert!(dst.y0 >= p.title.height(), "below the title bar");
    }

    #[test]
    fn uploads_read_only_inside_the_buffer() {
        // A 10 x 4 buffer.
        assert_eq!(upload_span(Rect::at(0, 0, 10, 4), 40, 10), Some(0..40), "everything");
        assert_eq!(upload_span(Rect::new(2, 1, 5, 3), 40, 10), Some(12..25), "rows 1-2, columns 2-4");
        assert_eq!(upload_span(Rect::new(9, 3, 10, 4), 40, 10), Some(39..40), "last pixel");
        for bad in [Rect::new(-1, 0, 3, 3), Rect::new(0, -1, 3, 3), Rect::new(0, 0, 11, 1),
                    Rect::new(0, 3, 10, 5), Rect::EMPTY] {
            assert_eq!(upload_span(bad, 40, 10), None, "{bad:?}");
        }
        assert_eq!(upload_span(Rect::at(0, 0, 10, 4), 39, 10), None, "a short buffer has 3 full rows");
        assert_eq!(upload_span(Rect::at(0, 0, 1, 1), 0, 0), None, "zero width");
    }

    #[test]
    fn a_covering_surface_paints_the_whole_frame() {
        let p = place_frame(1, 10, 10, 100, 80, FCF_TITLEBAR | FCF_BORDER, false, None, (96, 60), 600);
        let (cw, ch) = (p.client.width() as u32, p.client.height() as u32);
        assert_eq!(painted_parts(&p, cw, ch), vec![p.frame]);
        assert_eq!(painted_parts(&p, 500, 500), vec![p.frame], "a larger surface is clipped");
    }

    #[test]
    fn a_small_surface_leaves_the_rest_of_the_client_unpainted() {
        let p = place_frame(1, 10, 10, 100, 80, FCF_TITLEBAR | FCF_BORDER, false, None, (40, 20), 600);
        let parts = painted_parts(&p, 40, 20);
        let area = |rs: &[Rect]| rs.iter().map(|r| r.width() as i64 * r.height() as i64).sum::<i64>();
        let client = p.client.width() as i64 * p.client.height() as i64;
        assert_eq!(area(&parts), area(&[p.frame]) - (client - 40 * 20));
        let c = p.client;
        let inside = |x: i32, y: i32| parts.iter().any(|r| r.contains(&Rect::at(x, y, 1, 1)));
        assert!(inside(c.x0, c.y0), "the surface's top-left");
        assert!(inside(c.x0 + 39, c.y0 + 19), "the surface's bottom-right pel");
        assert!(!inside(c.x0 + 40, c.y0), "right of the surface");
        assert!(!inside(c.x0, c.y0 + 20), "below the surface");
        assert!(inside(p.title.x0, p.title.y0), "chrome is always painted");
        for (i, a) in parts.iter().enumerate() {
            for b in &parts[i + 1..] {
                assert!(a.intersect(b).is_empty(), "parts do not overlap");
            }
        }
    }

    #[test]
    fn moving_a_placement_keeps_its_layout_and_drops_the_pulldown() {
        let p = place_frame(1, 10, 10, 100, 80, FCF_TITLEBAR | FCF_BORDER, true, Some((5, 30)), (96, 40), 600);
        assert!(!p.pulldown.is_empty());
        let m = p.moved_without_pulldown(-p.frame.x0, -p.frame.y0);
        assert_eq!((m.frame.x0, m.frame.y0), (0, 0));
        assert_eq!(m.client, p.client.offset(-p.frame.x0, -p.frame.y0));
        assert_eq!(m.menu_bar, p.menu_bar.offset(-p.frame.x0, -p.frame.y0));
        assert!(m.pulldown.is_empty());
    }
}
