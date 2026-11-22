// SPDX-License-Identifier: GPL-3.0-or-later

//! The shape of a frame in the modern theme (ADR-0118): rounded outer
//! corners, antialiased, and a soft shadow below it.
//!
//! A frame's pels stay a rectangle everywhere Warpine keeps them (client
//! surfaces, chrome pixmaps, the pictures sent to the desktop server); the
//! shape is applied where frames are composited:
//!
//! * a desktop buffer (the software compositor, the desktop server):
//!   [`draw_shadow`] darkens what lies below before the frame is drawn,
//!   [`corners_under`] keeps the pels its corners will cover and
//!   [`blend_corners`] mixes them back by the corners' coverage after;
//! * textures (the layered compositor): [`shape_alpha`] gives a frame's
//!   pels their coverage as alpha, and [`shadow_slices`] is a small shadow
//!   picture drawn stretched in nine slices under each frame.
//!
//! Corners and shadow use the same distance to a rounded rectangle, so all
//! compositors draw the same shape. The pels a corner leaves bare still
//! belong to the frame for the pointer (hit testing does not change).

use super::damage::Rect;

/// Radius of a frame's rounded corners, in pels.
pub const CORNER_RADIUS: i32 = 8;
/// How far the shadow reaches beyond the frame, in pels.
pub const SHADOW_SIZE: i32 = 12;
/// How far the shadow is moved down (light from above).
pub const SHADOW_DY: i32 = 3;
/// Darkness of the shadow where it is strongest (of 255).
pub const SHADOW_ALPHA: u32 = 0x58;

/// The corner radius a `w` × `h` frame can take.
fn radius(w: i32, h: i32) -> i32 {
    CORNER_RADIUS.min(w / 2).min(h / 2).max(0)
}

/// Signed distance from point `(px, py)` to a `w` × `h` rectangle at the
/// origin with corners of radius `r` (negative inside).
fn rounded_rect_distance(px: f32, py: f32, w: f32, h: f32, r: f32) -> f32 {
    let (hx, hy) = (w / 2.0, h / 2.0);
    let qx = (px - hx).abs() - (hx - r);
    let qy = (py - hy).abs() - (hy - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// How much of pel `(x, y)` of a `w` × `h` frame its rounded outline
/// covers, 0 to 255 (255 everywhere but near the corners).
pub fn coverage(x: i32, y: i32, w: i32, h: i32) -> u8 {
    let r = radius(w, h);
    if r == 0 || (x >= r && x < w - r) || (y >= r && y < h - r) {
        return 255;
    }
    let d = rounded_rect_distance(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, r as f32);
    ((0.5 - d).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The four `r` × `r` corner squares of frame `f`, where [`coverage`] can be
/// below 255.
pub fn corner_squares(f: Rect) -> [Rect; 4] {
    let r = radius(f.width(), f.height());
    [Rect::at(f.x0, f.y0, r, r), Rect::at(f.x1 - r, f.y0, r, r),
     Rect::at(f.x0, f.y1 - r, r, r), Rect::at(f.x1 - r, f.y1 - r, r, r)]
}

/// Whether frame `f`, rounded, hides all of `area`: inside it and clear of
/// its corners.
pub fn covers(f: Rect, area: Rect) -> bool {
    f.contains(&area) && corner_squares(f).iter().all(|c| c.intersect(&area).is_empty())
}

/// Everything frame `f`'s shadow darkens.
pub fn shadow_extent(f: Rect) -> Rect {
    Rect::new(f.x0 - SHADOW_SIZE, f.y0 - SHADOW_SIZE + SHADOW_DY, f.x1 + SHADOW_SIZE, f.y1 + SHADOW_SIZE + SHADOW_DY)
}

/// The darkness (of 255) frame `f` casts on desktop pel `(x, y)`.
pub fn shadow_alpha(f: Rect, x: i32, y: i32) -> u32 {
    let (w, h) = (f.width(), f.height());
    let r = radius(w, h) as f32;
    let px = (x - f.x0) as f32 + 0.5;
    let py = (y - f.y0 - SHADOW_DY) as f32 + 0.5;
    let d = rounded_rect_distance(px, py, w as f32, h as f32, r);
    if d <= 0.0 { return SHADOW_ALPHA; }
    let t = 1.0 - d / SHADOW_SIZE as f32;
    if t <= 0.0 { 0 } else { (SHADOW_ALPHA as f32 * t * t).round() as u32 }
}

/// `pel` darkened by `alpha` (of 255).
fn darken(pel: u32, alpha: u32) -> u32 {
    let k = 255 - alpha.min(255);
    let ch = |s: u32| (((pel >> s) & 0xFF) * k / 255) << s;
    (pel & 0xFF00_0000) | ch(16) | ch(8) | ch(0)
}

/// `over` laid on `under` with coverage `cov` (of 255).
fn mix(under: u32, over: u32, cov: u8) -> u32 {
    let (a, b) = (cov as u32, 255 - cov as u32);
    let ch = |s: u32| ((((over >> s) & 0xFF) * a + ((under >> s) & 0xFF) * b) / 255) << s;
    ch(16) | ch(8) | ch(0)
}

/// Curve the chrome inside frame `f`'s two top corners, inside `clip`: the
/// edge line (`edge`, the outermost pel ring) and the next `rings` − 1 rings
/// (`face`) follow the rounded outline, so the title bar's corners are
/// rounded with it instead of standing square inside the curve. The
/// compositor's corner coverage then shapes the outside; the client, which
/// never reaches the top corners, keeps its pels.
pub fn round_top_chrome(buf: &mut [u32], buf_w: usize, f: Rect, rings: i32, edge: u32, face: u32, clip: Rect) {
    let (w, h) = (f.width(), f.height());
    let r = radius(w, h);
    let [tl, tr, ..] = corner_squares(f);
    for sq in [tl, tr] {
        let area = in_buffer(sq.intersect(&clip), buf_w, buf.len());
        for y in area.y0..area.y1 {
            for x in area.x0..area.x1 {
                let d = rounded_rect_distance((x - f.x0) as f32 + 0.5, (y - f.y0) as f32 + 0.5,
                                              w as f32, h as f32, r as f32);
                let cov = |t: f32| (t.clamp(0.0, 1.0) * 255.0).round() as u8;
                let i = y as usize * buf_w + x as usize;
                let pel = mix(buf[i], face, cov(d + rings as f32 + 0.5));
                buf[i] = mix(pel, edge, cov(d + 1.5));
            }
        }
    }
}

/// The part of `r` inside a `buf_w`-wide buffer of `len` pels.
fn in_buffer(r: Rect, buf_w: usize, len: usize) -> Rect {
    let h = len.checked_div(buf_w).unwrap_or(0);
    r.intersect(&Rect::at(0, 0, buf_w.min(i32::MAX as usize) as i32, h.min(i32::MAX as usize) as i32))
}

/// Darken the pels of `buf` frame `f`'s shadow falls on, inside `clip`,
/// leaving those the frame itself will cover.
pub fn draw_shadow(buf: &mut [u32], buf_w: usize, f: Rect, clip: Rect) {
    let area = in_buffer(shadow_extent(f).intersect(&clip), buf_w, buf.len());
    let (w, h) = (f.width(), f.height());
    for y in area.y0..area.y1 {
        for x in area.x0..area.x1 {
            if f.contains(&Rect::at(x, y, 1, 1)) && coverage(x - f.x0, y - f.y0, w, h) == 255 {
                continue;
            }
            let a = shadow_alpha(f, x, y);
            if a > 0 {
                let i = y as usize * buf_w + x as usize;
                buf[i] = darken(buf[i], a);
            }
        }
    }
}

/// A pel a corner will partly cover: where it is, what lies there before
/// the frame is drawn, and the corner's coverage.
pub struct UnderCorner {
    at: usize,
    pel: u32,
    cov: u8,
}

/// The pels of `buf` inside `clip` that frame `f`'s corners leave partly
/// or wholly bare, as they are before the frame is drawn; `keep` is a
/// rectangle drawn with the frame that is not shaped (an open pull-down).
pub fn corners_under(buf: &[u32], buf_w: usize, f: Rect, clip: Rect, keep: Rect) -> Vec<UnderCorner> {
    let (w, h) = (f.width(), f.height());
    let mut out = Vec::new();
    for c in corner_squares(f) {
        let area = in_buffer(c.intersect(&clip), buf_w, buf.len());
        for y in area.y0..area.y1 {
            for x in area.x0..area.x1 {
                if keep.contains(&Rect::at(x, y, 1, 1)) { continue; }
                let cov = coverage(x - f.x0, y - f.y0, w, h);
                if cov < 255 {
                    let at = y as usize * buf_w + x as usize;
                    out.push(UnderCorner { at, pel: buf[at], cov });
                }
            }
        }
    }
    out
}

/// Mix the frame just drawn back over what [`corners_under`] kept, by the
/// corners' coverage.
pub fn blend_corners(buf: &mut [u32], under: &[UnderCorner]) {
    for u in under {
        if let Some(p) = buf.get_mut(u.at) {
            *p = mix(u.pel, *p, u.cov);
        }
    }
}

/// Give the pels of `region` (frame-local, rows `px_w` apart starting at
/// the region's top-left) of a `w` × `h` frame their coverage as alpha
/// (ARGB), for a texture drawn with blending.
pub fn shape_alpha(px: &mut [u32], px_w: usize, region: Rect, (w, h): (i32, i32)) {
    for y in region.y0..region.y1 {
        for x in region.x0..region.x1 {
            let i = (y - region.y0) as usize * px_w + (x - region.x0) as usize;
            if let Some(p) = px.get_mut(i) {
                *p = (*p & 0x00FF_FFFF) | (coverage(x, y, w, h) as u32) << 24;
            }
        }
    }
}

/// The shadow of a small frame, as ARGB pels (black, alpha = darkness),
/// to be drawn in nine slices under any frame: returns the picture's side
/// and its pels. The corner slices are `SHADOW_SIZE + CORNER_RADIUS` wide;
/// the middle row and column are the straight edges, which stretch.
pub fn shadow_slices() -> (i32, Vec<u32>) {
    let inner = 2 * CORNER_RADIUS + 2;
    let side = inner + 2 * SHADOW_SIZE;
    let f = Rect::at(SHADOW_SIZE, SHADOW_SIZE - SHADOW_DY, inner, inner);
    let px = (0..side * side).map(|i| shadow_alpha(f, i % side, i / side) << 24).collect();
    (side, px)
}

/// Where the nine slices of [`shadow_slices`] go under frame `f`:
/// `(source, destination)` rectangles, the source in the picture.
pub fn slice_places(f: Rect) -> Vec<(Rect, Rect)> {
    let side = 2 * CORNER_RADIUS + 2 + 2 * SHADOW_SIZE;
    let m = SHADOW_SIZE + CORNER_RADIUS;
    let d = shadow_extent(f);
    if d.width() < 2 * m || d.height() < 2 * m {
        return vec![(Rect::at(0, 0, side, side), d)];
    }
    let sx = [0, m, side - m, side];
    let sy = sx;
    let dx = [d.x0, d.x0 + m, d.x1 - m, d.x1];
    let dy = [d.y0, d.y0 + m, d.y1 - m, d.y1];
    let mut out = Vec::with_capacity(9);
    for j in 0..3 {
        for i in 0..3 {
            out.push((Rect::new(sx[i], sy[j], sx[i + 1], sy[j + 1]), Rect::new(dx[i], dy[j], dx[i + 1], dy[j + 1])));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_top_chrome_follows_the_rounded_corner() {
        const TITLE: u32 = 0x0000_0080;
        let (w, h) = (40, 30);
        let mut buf = vec![TITLE; (w * h) as usize];
        let f = Rect::at(0, 0, w, h);
        round_top_chrome(&mut buf, w as usize, f, 2, 0x0080_8080, 0x00D4_D0C8, f);
        let at = |x: i32, y: i32| buf[(y * w + x) as usize];
        assert_eq!(at(0, 0), 0x0080_8080, "outside the curve: edge colour, shaped away by the compositor");
        assert_eq!(at(20, 0), TITLE, "past the corner squares nothing changes");
        assert_eq!(at(2, 2), 0x0080_8080, "the edge line follows the curve");
        assert_eq!(at(7, 7), TITLE, "well inside the curve the title bar stays");
        assert_eq!(at(0, 20), TITLE, "the bottom corners are left alone");
        assert_ne!(at(1, 6), TITLE, "the title's square corner is covered");
    }

    #[test]
    fn corners_are_rounded_and_the_rest_is_covered() {
        let (w, h) = (100, 60);
        assert_eq!(coverage(0, 0, w, h), 0, "the very corner is bare");
        assert_eq!(coverage(50, 0, w, h), 255, "the top edge between corners");
        assert_eq!(coverage(0, 30, w, h), 255, "the left edge between corners");
        assert_eq!(coverage(CORNER_RADIUS, CORNER_RADIUS, w, h), 255, "inside the arc");
        assert_eq!((coverage(w - 1, 0, w, h), coverage(0, h - 1, w, h), coverage(w - 1, h - 1, w, h)), (0, 0, 0));
        let edge = coverage(2, 2, w, h);
        assert!(edge > 0 && edge < 255, "antialiased on the arc: {edge}");
        // Symmetric.
        assert_eq!(coverage(2, 2, w, h), coverage(w - 3, h - 3, w, h));
        // A tiny frame takes a smaller radius and is still covered in its middle.
        assert_eq!(coverage(3, 3, 6, 6), 255);
    }

    #[test]
    fn the_shadow_fades_out_and_falls_below() {
        let f = Rect::at(100, 100, 200, 100);
        assert_eq!(shadow_alpha(f, 150, 150), SHADOW_ALPHA, "under the frame");
        let near = shadow_alpha(f, 150, 200 + SHADOW_DY + 1);
        let far = shadow_alpha(f, 150, 200 + SHADOW_DY + SHADOW_SIZE - 2);
        assert!(near > far && far > 0, "{near} {far}");
        assert_eq!(shadow_alpha(f, 150, 200 + SHADOW_DY + SHADOW_SIZE + 1), 0, "beyond its reach");
        assert!(shadow_alpha(f, 150, 100 - SHADOW_SIZE + SHADOW_DY + 1) < near, "lighter above (moved down)");
        assert!(shadow_extent(f).contains(&f));
    }

    #[test]
    fn corners_are_mixed_with_what_lies_below() {
        let f = Rect::at(2, 2, 40, 30);
        let mut buf = vec![0x0000_FF00u32; 50 * 40];
        let clip = Rect::at(0, 0, 50, 40);
        let under = corners_under(&buf, 50, f, clip, Rect::EMPTY);
        assert!(!under.is_empty());
        // The frame is drawn red over its whole rectangle...
        for y in f.y0..f.y1 { for x in f.x0..f.x1 { buf[y as usize * 50 + x as usize] = 0x00FF_0000; } }
        blend_corners(&mut buf, &under);
        // ...but its very corner shows the green below, its middle red.
        assert_eq!(buf[2 * 50 + 2], 0x0000_FF00);
        assert_eq!(buf[10 * 50 + 10], 0x00FF_0000);
        // A kept rectangle (a pull-down) is not shaped.
        let under = corners_under(&buf, 50, f, clip, Rect::at(0, 0, 10, 10));
        assert!(under.iter().all(|u| u.at >= 10 * 50 || u.at % 50 >= 10));
    }

    #[test]
    fn rounded_frames_cover_only_away_from_their_corners() {
        let f = Rect::at(0, 0, 100, 100);
        assert!(covers(f, Rect::at(20, 20, 10, 10)));
        assert!(!covers(f, Rect::at(0, 0, 10, 10)), "a corner lets below through");
        assert!(!covers(f, Rect::at(90, 20, 20, 10)), "outside");
    }

    #[test]
    fn shadows_darken_the_ring_not_the_frame() {
        let f = Rect::at(20, 20, 40, 30);
        let mut buf = vec![0x00FF_FFFFu32; 100 * 80];
        draw_shadow(&mut buf, 100, f, Rect::at(0, 0, 100, 80));
        assert_eq!(buf[30 * 100 + 30], 0x00FF_FFFF, "the frame's own pels are left for it");
        assert!(buf[(50 + SHADOW_DY + 2) as usize * 100 + 40] < 0x00FF_FFFF, "below the frame is darker");
        assert_eq!(buf[2 * 100 + 2], 0x00FF_FFFF, "far away is untouched");
    }

    #[test]
    fn texture_alpha_and_shadow_slices_match_the_buffer_shape() {
        let mut px = vec![0x0012_3456u32; 4 * 4];
        shape_alpha(&mut px, 4, Rect::at(0, 0, 4, 4), (100, 60));
        assert_eq!(px[0] >> 24, coverage(0, 0, 100, 60) as u32);
        assert_eq!(px[0] & 0x00FF_FFFF, 0x12_3456, "colour kept");
        assert_eq!(px[3 * 4 + 3] >> 24, coverage(3, 3, 100, 60) as u32);
        let (side, sh) = shadow_slices();
        assert_eq!(sh.len(), (side * side) as usize);
        let places = slice_places(Rect::at(100, 100, 200, 100));
        assert_eq!(places.len(), 9);
        let union = places.iter().fold(Rect::EMPTY, |u, (_, d)| u.union(d));
        assert_eq!(union, shadow_extent(Rect::at(100, 100, 200, 100)));
    }
}
