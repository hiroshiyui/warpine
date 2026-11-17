// SPDX-License-Identifier: GPL-3.0-only

//! Moving and sizing a frame with the keyboard: system menu Move / Size,
//! Alt+F7 / Alt+F8 (`WM_SYSCOMMAND(SC_MOVE / SC_SIZE)`).
//!
//! PM's frame answers these with `WM_TRACKFRAME`, which tracks the frame
//! with `WinTrackRect`. Its keyboard interface (PM Reference, WinTrackRect):
//! Enter "accepts the new position or size", Esc "cancels the current
//! tracking operation", each arrow moves the rectangle a step
//! (`cxKeyboard` / `cyKeyboard`) in its direction, and when the pointer is
//! on a horizontal edge Left and Right move it to a corner of that edge
//! (Up and Down likewise on a vertical edge). So a keyboard size works on
//! edges: the first Left / Right picks the left / right edge, the first
//! Up / Down the top / bottom edge, the other direction then makes it a
//! corner, and further arrows move the chosen edges. That the first arrow
//! only picks the edge, without moving it, is Warpine's reading (the
//! pointer starts in the middle of the window).
//!
//! [`KeyTrack`] is the pure state machine; the renderer applies the
//! rectangles it returns (a PM frame, or the VIO window with its own size
//! limits) and Warpine moves the window itself rather than a tracking
//! outline (as its mouse drags do).

use crate::loader::constants::{TF_BOTTOM, TF_LEFT, TF_RIGHT, TF_TOP};
use crate::loader::pm_minmax::WinRect;

/// Step of an arrow key while tracking a PM frame, in pels (the
/// `cxKeyboard` / `cyKeyboard` of WinTrackRect's example; the frame's own
/// values are not documented).
pub const FRAME_TRACK_STEP: i32 = 8;

/// A key that works a keyboard move or size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKey { Left, Right, Up, Down, Enter, Esc }

/// What a key did to the tracking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackOutcome {
    /// Tracking goes on; the window is to take this rectangle (`None`: it
    /// stays as it is).
    Continue(Option<WinRect>),
    /// Enter: the window keeps where it is.
    Accept,
    /// Esc: the window goes back to this, its rectangle before tracking.
    Cancel(WinRect),
}

/// A keyboard move or size in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyTrack {
    /// Sizing (true) or moving.
    pub size: bool,
    /// The window's rectangle when tracking started.
    pub orig: WinRect,
    /// `TF_LEFT` / `TF_RIGHT` / `TF_TOP` / `TF_BOTTOM`: the edges a size
    /// moves (none until an arrow picks one).
    edges: u32,
    /// Pels per arrow key, horizontally and vertically.
    step: (i32, i32),
}

impl KeyTrack {
    /// Start moving (`size` false) or sizing a window at `orig`, `step`
    /// pels per arrow key.
    pub fn new(orig: WinRect, size: bool, step: (i32, i32)) -> KeyTrack {
        KeyTrack { size, orig, edges: 0, step }
    }

    /// The edges a size moves (`TF_*`).
    pub fn edges(&self) -> u32 {
        self.edges
    }

    /// Work the tracking with `key`, the window being at `current`;
    /// `clamp` limits a size `(cx, cy)` (minimum and maximum size).
    pub fn key(&mut self, current: WinRect, key: TrackKey, clamp: impl Fn(i32, i32) -> (i32, i32))
        -> TrackOutcome
    {
        let (dx, dy) = match key {
            TrackKey::Enter => return TrackOutcome::Accept,
            TrackKey::Esc => return TrackOutcome::Cancel(self.orig),
            TrackKey::Left => (-self.step.0, 0),
            TrackKey::Right => (self.step.0, 0),
            TrackKey::Up => (0, self.step.1),
            TrackKey::Down => (0, -self.step.1),
        };
        if !self.size {
            return TrackOutcome::Continue(Some(WinRect { x: current.x + dx, y: current.y + dy, ..current }));
        }
        // The first arrow along an axis picks that edge.
        if dx != 0 && self.edges & (TF_LEFT | TF_RIGHT) == 0 {
            self.edges |= if dx < 0 { TF_LEFT } else { TF_RIGHT };
            return TrackOutcome::Continue(None);
        }
        if dy != 0 && self.edges & (TF_TOP | TF_BOTTOM) == 0 {
            self.edges |= if dy > 0 { TF_TOP } else { TF_BOTTOM };
            return TrackOutcome::Continue(None);
        }
        let WinRect { x, y, cx, cy } = current;
        let left = self.edges & TF_LEFT != 0;
        let bottom = self.edges & TF_BOTTOM != 0;
        // Moving the right / top edge out grows the window, moving the
        // left / bottom edge out (left / down) too.
        let ncx = if left { cx - dx } else { cx + dx };
        let ncy = if bottom { cy - dy } else { cy + dy };
        let (ncx, ncy) = clamp(ncx, ncy);
        // The opposite edges stay where they are.
        let nx = if left { x + cx - ncx } else { x };
        let ny = if bottom { y + cy - ncy } else { y };
        let next = WinRect { x: nx, y: ny, cx: ncx, cy: ncy };
        TrackOutcome::Continue((next != current).then_some(next))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: WinRect = WinRect { x: 100, y: 200, cx: 300, cy: 150 };
    fn free(cx: i32, cy: i32) -> (i32, i32) { (cx.max(32), cy.max(32)) }

    fn applied(t: &mut KeyTrack, at: WinRect, key: TrackKey) -> WinRect {
        match t.key(at, key, free) {
            TrackOutcome::Continue(r) => r.unwrap_or(at),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_move_steps_the_window_and_esc_puts_it_back() {
        let mut t = KeyTrack::new(R, false, (8, 8));
        let r = applied(&mut t, R, TrackKey::Right);
        assert_eq!(r, WinRect { x: 108, ..R });
        let r = applied(&mut t, r, TrackKey::Down);
        assert_eq!(r, WinRect { x: 108, y: 192, ..R }, "down lowers the bottom-left origin");
        let r = applied(&mut t, r, TrackKey::Up);
        let r = applied(&mut t, r, TrackKey::Left);
        assert_eq!(r, R);
        assert_eq!(t.key(r, TrackKey::Esc, free), TrackOutcome::Cancel(R));
        assert_eq!(t.key(r, TrackKey::Enter, free), TrackOutcome::Accept);
    }

    #[test]
    fn the_first_arrow_of_a_size_picks_an_edge() {
        let mut t = KeyTrack::new(R, true, (8, 8));
        assert_eq!(t.key(R, TrackKey::Right, free), TrackOutcome::Continue(None));
        assert_eq!(t.edges(), TF_RIGHT);
        let r = applied(&mut t, R, TrackKey::Right);
        assert_eq!(r, WinRect { cx: 308, ..R }, "the right edge moves right");
        let r = applied(&mut t, r, TrackKey::Left);
        let r = applied(&mut t, r, TrackKey::Left);
        assert_eq!(r, WinRect { cx: 292, ..R }, "and back left, past its start");
        // Up / Down on a vertical edge make it a corner.
        assert_eq!(t.key(r, TrackKey::Down, free), TrackOutcome::Continue(None));
        assert_eq!(t.edges(), TF_RIGHT | TF_BOTTOM);
        let r2 = applied(&mut t, r, TrackKey::Down);
        assert_eq!(r2, WinRect { y: 192, cy: 158, ..r }, "the bottom edge moves down, the top stays");
        assert_eq!(r2.y + r2.cy, R.y + R.cy);
    }

    #[test]
    fn left_and_top_edges_keep_the_opposite_edges_in_place() {
        let mut t = KeyTrack::new(R, true, (8, 16));
        t.key(R, TrackKey::Left, free);
        t.key(R, TrackKey::Up, free);
        assert_eq!(t.edges(), TF_LEFT | TF_TOP);
        let r = applied(&mut t, R, TrackKey::Left);
        assert_eq!(r, WinRect { x: 92, cx: 308, ..R }, "the left edge moves out");
        let r = applied(&mut t, r, TrackKey::Up);
        assert_eq!(r, WinRect { x: 92, cx: 308, cy: 166, ..R }, "the top edge moves up");
    }

    #[test]
    fn sizes_are_clamped_and_an_unchanged_size_reports_nothing() {
        let small = WinRect { x: 0, y: 0, cx: 40, cy: 40 };
        let mut t = KeyTrack::new(small, true, (8, 8));
        t.key(small, TrackKey::Left, free);
        let r = applied(&mut t, small, TrackKey::Right);
        assert_eq!(r, WinRect { x: 8, cx: 32, ..small }, "the left edge moves in");
        assert_eq!(t.key(r, TrackKey::Right, free), TrackOutcome::Continue(None), "not below the minimum");
        // A maximum size, as the VIO window has.
        let max = |cx: i32, cy: i32| (cx.min(40), cy.min(40));
        let mut t = KeyTrack::new(small, true, (8, 8));
        t.key(small, TrackKey::Right, max);
        assert_eq!(t.key(small, TrackKey::Right, max), TrackOutcome::Continue(None));
    }
}
