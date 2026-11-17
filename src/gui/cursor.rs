// SPDX-License-Identifier: GPL-3.0-only

//! The PM text cursor (`WinCreateCursor`) as the compositors draw it: an
//! inverted rectangle, halftone or frame laid over a frame's client
//! pixels when they are put on the screen. It is never drawn into the
//! frame's surface, so the application's own painting cannot leave stale
//! cursor pels behind and drawing never has to hide it first.

use super::damage::Rect;

/// Pels of the cursor are inverted with this mask (`0x00RRGGBB`).
const INVERT: u32 = 0x00FF_FFFF;

/// How the cursor looks (`CURSOR_SOLID` / `CURSOR_HALFTONE` / `CURSOR_FRAME`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Solid,
    /// Every other pel, in a checkerboard.
    Halftone,
    /// The outline of the rectangle, one pel wide.
    Frame,
}

/// A cursor to show on a frame, in the PM coordinates of the frame's
/// surface (origin bottom-left; `x0`/`y0` inclusive, `x1`/`y1` exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorMark {
    pub rect: (i32, i32, i32, i32),
    /// Where it may show: its window, cut to the clip rectangle.
    pub clip: (i32, i32, i32, i32),
    pub kind: CursorKind,
}

impl CursorMark {
    /// A PM rectangle as screen rows of a surface `height` pels tall.
    fn screen(r: (i32, i32, i32, i32), height: u32) -> Rect {
        let h = height.min(i32::MAX as u32) as i32;
        Rect::new(r.0, h - r.3, r.2, h - r.1)
    }

    /// The pels it covers on a surface `height` pels tall (top row first),
    /// clipped.
    pub fn surface_rect(&self, height: u32) -> Rect {
        Self::screen(self.rect, height).intersect(&Self::screen(self.clip, height))
    }

    /// Lay the cursor over `buf` (`buf_w` pels a row), where the surface's
    /// top-left pel is at `origin`, touching only pels inside `clip`
    /// (buffer coordinates). The surface is `height` pels tall.
    pub fn apply(&self, buf: &mut [u32], buf_w: usize, origin: (i32, i32), height: u32, clip: Rect) {
        let full = Self::screen(self.rect, height);
        let buf_h = (buf.len() / buf_w.max(1)) as i32;
        let r = self.surface_rect(height).offset(origin.0, origin.1)
            .intersect(&clip)
            .intersect(&Rect::at(0, 0, buf_w.min(i32::MAX as usize) as i32, buf_h));
        for y in r.y0..r.y1 {
            let (sy, row) = (y - origin.1, y as usize * buf_w);
            for x in r.x0..r.x1 {
                let sx = x - origin.0;
                let on = match self.kind {
                    CursorKind::Solid => true,
                    CursorKind::Halftone => (sx + sy).rem_euclid(2) == 0,
                    CursorKind::Frame => sx == full.x0 || sx == full.x1 - 1
                        || sy == full.y0 || sy == full.y1 - 1,
                };
                if on {
                    buf[row + x as usize] ^= INVERT;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 8;
    const H: u32 = 6;

    fn mark(kind: CursorKind) -> CursorMark {
        // PM rows 1..4 (screen rows 2..5), columns 2..6.
        CursorMark { rect: (2, 1, 6, 4), clip: (0, 0, 8, 6), kind }
    }

    fn inverted(buf: &[u32]) -> Vec<(usize, usize)> {
        buf.iter().enumerate().filter(|(_, p)| **p != 0).map(|(i, _)| (i % W, i / W)).collect()
    }

    #[test]
    fn a_solid_cursor_inverts_its_rectangle_in_screen_rows() {
        let mut buf = vec![0; W * H as usize];
        let m = mark(CursorKind::Solid);
        assert_eq!(m.surface_rect(H), Rect::new(2, 2, 6, 5));
        m.apply(&mut buf, W, (0, 0), H, Rect::at(0, 0, 8, 6));
        assert_eq!(inverted(&buf).len(), 12);
        assert_eq!(buf[2 * W + 2], INVERT);
        // Applying it again restores the pels.
        m.apply(&mut buf, W, (0, 0), H, Rect::at(0, 0, 8, 6));
        assert!(buf.iter().all(|&p| p == 0));
    }

    #[test]
    fn halftone_and_frame_cursors() {
        let mut buf = vec![0; W * H as usize];
        mark(CursorKind::Halftone).apply(&mut buf, W, (0, 0), H, Rect::at(0, 0, 8, 6));
        assert_eq!(inverted(&buf).len(), 6, "every other pel");
        assert!(inverted(&buf).iter().all(|(x, y)| (x + y) % 2 == 0));
        let mut buf = vec![0; W * H as usize];
        mark(CursorKind::Frame).apply(&mut buf, W, (0, 0), H, Rect::at(0, 0, 8, 6));
        assert_eq!(inverted(&buf).len(), 10, "the outline of 4 x 3");
        assert_eq!(buf[3 * W + 3], 0, "the inside is left alone");
    }

    #[test]
    fn clipping_by_the_window_the_clip_and_the_caller() {
        let mut m = mark(CursorKind::Solid);
        m.clip = (0, 0, 4, 6);
        let mut buf = vec![0; W * H as usize];
        m.apply(&mut buf, W, (0, 0), H, Rect::at(0, 0, 8, 6));
        assert_eq!(inverted(&buf).len(), 6, "two columns are left");
        // A frame cursor keeps its outline where it is clipped: no right edge.
        m.kind = CursorKind::Frame;
        let mut buf = vec![0; W * H as usize];
        m.apply(&mut buf, W, (0, 0), H, Rect::at(0, 0, 8, 6));
        assert!(inverted(&buf).iter().all(|&(x, _)| x < 4));
        assert_eq!(buf[3 * W + 3], 0, "column 3 is inside the outline");
        // The caller's clip and an origin.
        let m = mark(CursorKind::Solid);
        let mut buf = vec![0; W * H as usize];
        m.apply(&mut buf, W, (1, -2), H, Rect::new(0, 0, 8, 1));
        assert_eq!(inverted(&buf), vec![(3, 0), (4, 0), (5, 0), (6, 0)]);
        // Entirely off the buffer.
        let mut buf = vec![0; W * H as usize];
        m.apply(&mut buf, W, (100, 100), H, Rect::at(0, 0, 1000, 1000));
        assert!(buf.iter().all(|&p| p == 0));
    }
}
