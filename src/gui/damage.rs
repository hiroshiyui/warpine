// SPDX-License-Identifier: GPL-3.0-only

//! Damage tracking and clipped pixel copies for the desktop compositor.
//!
//! The compositor keeps the desktop image between frames and redraws only
//! the areas that changed (the *damage*). Everything here works in desktop
//! screen space (origin top-left, y down) on `0x00RRGGBB` pixel buffers and
//! needs no SDL, so it is unit-tested directly.

/// A half-open rectangle `[x0, x1) × [y0, y1)` in screen coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub const EMPTY: Rect = Rect { x0: 0, y0: 0, x1: 0, y1: 0 };

    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect {
        Rect { x0, y0, x1, y1 }
    }

    /// The rectangle at `(x, y)` with size `w` × `h`.
    pub fn at(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x0: x, y0: y, x1: x + w, y1: y + h }
    }

    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub fn width(&self) -> i32 { (self.x1 - self.x0).max(0) }
    pub fn height(&self) -> i32 { (self.y1 - self.y0).max(0) }

    /// The overlap of `self` and `other` (empty if they do not overlap).
    pub fn intersect(&self, other: &Rect) -> Rect {
        let r = Rect {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        if r.is_empty() { Rect::EMPTY } else { r }
    }

    /// The smallest rectangle containing both (an empty side is ignored).
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() { return *other; }
        if other.is_empty() { return *self; }
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    /// Whether `other` lies entirely inside `self`. An empty `other` is
    /// contained in anything.
    pub fn contains(&self, other: &Rect) -> bool {
        other.is_empty()
            || (other.x0 >= self.x0 && other.y0 >= self.y0
                && other.x1 <= self.x1 && other.y1 <= self.y1)
    }

    /// Whether the rectangles overlap or share an edge. Such rectangles are
    /// merged into their union, which costs little extra area.
    fn touches(&self, other: &Rect) -> bool {
        self.x0 <= other.x1 && other.x0 <= self.x1 && self.y0 <= other.y1 && other.y0 <= self.y1
    }
}

/// More separate damage rectangles than this are collapsed into their
/// bounding box: each rectangle costs a pass over the window stack.
pub const MAX_DAMAGE_RECTS: usize = 8;

/// The areas of the desktop to redraw in the next composite.
///
/// Rectangles are clipped to the desktop; overlapping or touching ones are
/// merged, and past [`MAX_DAMAGE_RECTS`] everything collapses into one
/// bounding box.
#[derive(Debug)]
pub struct Damage {
    bounds: Rect,
    rects: Vec<Rect>,
}

impl Damage {
    /// No damage on a desktop of `width` × `height`.
    pub fn new(width: i32, height: i32) -> Damage {
        Damage { bounds: Rect::at(0, 0, width, height), rects: Vec::new() }
    }

    pub fn is_empty(&self) -> bool { self.rects.is_empty() }

    pub fn rects(&self) -> &[Rect] { &self.rects }

    /// Damage the whole desktop.
    pub fn add_all(&mut self) {
        self.rects.clear();
        if !self.bounds.is_empty() {
            self.rects.push(self.bounds);
        }
    }

    pub fn add(&mut self, r: Rect) {
        let mut r = r.intersect(&self.bounds);
        if r.is_empty() { return; }
        // Absorb every rectangle the new one touches; repeat, since the
        // grown rectangle may now touch others.
        loop {
            let before = self.rects.len();
            self.rects.retain(|d| {
                if d.touches(&r) { r = r.union(d); false } else { true }
            });
            if self.rects.len() == before { break; }
        }
        self.rects.push(r);
        if self.rects.len() > MAX_DAMAGE_RECTS {
            let all = self.rects.iter().fold(Rect::EMPTY, |a, d| a.union(d));
            self.rects.clear();
            self.rects.push(all);
        }
    }
}

/// Fill `r ∩ clip` of a `dst_w`-wide buffer with `color`, one row slice at
/// a time. Parts outside the buffer are ignored.
pub fn fill_clipped(dst: &mut [u32], dst_w: usize, r: Rect, clip: Rect, color: u32) {
    let dst_h = (dst.len() / dst_w.max(1)) as i32;
    let r = r.intersect(&clip).intersect(&Rect::at(0, 0, dst_w as i32, dst_h));
    if r.is_empty() { return; }
    let (x0, x1) = (r.x0 as usize, r.x1 as usize);
    for y in r.y0 as usize..r.y1 as usize {
        dst[y * dst_w + x0..y * dst_w + x1].fill(color);
    }
}

/// Copy a `w` × `h` image from the top-left of `src` (rows `src_w` pixels
/// apart) so that its top-left lands at `(x, y)` in `dst`, keeping only the
/// part inside `clip` and inside `dst`. Copies one row slice at a time.
///
/// `w` and `h` must not exceed the source size; they are clamped to it.
#[allow(clippy::too_many_arguments)]
pub fn blit_clipped(dst: &mut [u32], dst_w: usize,
                    src: &[u32], src_w: usize,
                    x: i32, y: i32, w: i32, h: i32, clip: Rect) {
    let src_h = (src.len() / src_w.max(1)) as i32;
    let dst_h = (dst.len() / dst_w.max(1)) as i32;
    let (w, h) = (w.min(src_w as i32), h.min(src_h));
    let r = Rect::at(x, y, w, h)
        .intersect(&clip)
        .intersect(&Rect::at(0, 0, dst_w as i32, dst_h));
    if r.is_empty() { return; }
    let n = r.width() as usize;
    let sx = (r.x0 - x) as usize;
    for dy in r.y0..r.y1 {
        let s = (dy - y) as usize * src_w + sx;
        let d = dy as usize * dst_w + r.x0 as usize;
        dst[d..d + n].copy_from_slice(&src[s..s + n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_intersect_union_contains() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 20, 20);
        assert_eq!(a.intersect(&b), Rect::new(5, 5, 10, 10));
        assert_eq!(a.union(&b), Rect::new(0, 0, 20, 20));
        assert!(a.intersect(&Rect::new(10, 0, 20, 10)).is_empty(), "edges do not overlap");
        assert!(a.contains(&Rect::new(2, 2, 10, 10)));
        assert!(!a.contains(&b));
        assert!(a.contains(&Rect::EMPTY));
        assert_eq!(Rect::EMPTY.union(&b), b);
    }

    #[test]
    fn damage_clips_to_the_desktop_and_ignores_empty_rects() {
        let mut d = Damage::new(100, 50);
        d.add(Rect::new(-10, -10, 5, 5));
        d.add(Rect::new(200, 0, 300, 10));
        d.add(Rect::EMPTY);
        assert_eq!(d.rects(), &[Rect::new(0, 0, 5, 5)]);
    }

    #[test]
    fn damage_merges_overlapping_and_touching_rects_only() {
        let mut d = Damage::new(1000, 1000);
        d.add(Rect::new(0, 0, 10, 10));
        d.add(Rect::new(100, 100, 110, 110));
        assert_eq!(d.rects().len(), 2, "distant rects stay separate");
        d.add(Rect::new(10, 0, 20, 10)); // touches the first
        assert!(d.rects().contains(&Rect::new(0, 0, 20, 10)));
        // A rect bridging both merges everything into one.
        d.add(Rect::new(15, 5, 105, 105));
        assert_eq!(d.rects(), &[Rect::new(0, 0, 110, 110)]);
    }

    #[test]
    fn too_many_rects_collapse_into_their_bounding_box() {
        let mut d = Damage::new(1000, 1000);
        for i in 0..=MAX_DAMAGE_RECTS as i32 {
            d.add(Rect::at(i * 100, i * 50, 10, 10));
        }
        assert_eq!(d.rects(), &[Rect::new(0, 0, 810, 410)]);
    }

    #[test]
    fn add_all_replaces_everything_with_the_desktop() {
        let mut d = Damage::new(64, 32);
        d.add(Rect::new(1, 1, 2, 2));
        d.add_all();
        assert_eq!(d.rects(), &[Rect::new(0, 0, 64, 32)]);
    }

    /// 4×3 source with distinct pixels: value = 10*row + col + 1.
    fn src() -> Vec<u32> {
        (0..3).flat_map(|r| (0..4).map(move |c| 10 * r + c + 1)).collect()
    }

    #[test]
    fn blit_copies_inside_the_clip_only() {
        let mut dst = vec![0u32; 6 * 5];
        blit_clipped(&mut dst, 6, &src(), 4, 1, 1, 4, 3, Rect::new(2, 0, 6, 3));
        let row = |y: usize| dst[y * 6..y * 6 + 6].to_vec();
        assert_eq!(row(0), vec![0; 6]);
        assert_eq!(row(1), vec![0, 0, 2, 3, 4, 0], "column 1 is outside the clip");
        assert_eq!(row(2), vec![0, 0, 12, 13, 14, 0]);
        assert_eq!(row(3), vec![0; 6], "row 3 is outside the clip");
    }

    #[test]
    fn blit_clips_at_buffer_edges_and_negative_positions() {
        let mut dst = vec![0u32; 3 * 2];
        blit_clipped(&mut dst, 3, &src(), 4, -2, -1, 4, 3, Rect::new(-100, -100, 100, 100));
        assert_eq!(dst, vec![13, 14, 0, 23, 24, 0]);
    }

    #[test]
    fn blit_honours_a_size_smaller_than_the_source() {
        let mut dst = vec![0u32; 4 * 3];
        blit_clipped(&mut dst, 4, &src(), 4, 0, 0, 2, 2, Rect::new(0, 0, 4, 3));
        assert_eq!(dst, vec![1, 2, 0, 0, 11, 12, 0, 0, 0, 0, 0, 0]);
        // A size larger than the source is clamped to it.
        let mut dst = vec![0u32; 5 * 4];
        blit_clipped(&mut dst, 5, &src(), 4, 0, 0, 9, 9, Rect::new(0, 0, 5, 4));
        assert_eq!(dst[15..20], [0; 5]);
        assert_eq!(dst[10..15], [21, 22, 23, 24, 0]);
    }

    #[test]
    fn fill_clipped_fills_the_intersection() {
        let mut dst = vec![0u32; 4 * 3];
        fill_clipped(&mut dst, 4, Rect::new(-5, 1, 3, 9), Rect::new(1, 0, 4, 3), 7);
        assert_eq!(dst, vec![0, 0, 0, 0, 0, 7, 7, 0, 0, 7, 7, 0]);
    }
}
