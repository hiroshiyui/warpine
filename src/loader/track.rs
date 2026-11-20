// SPDX-License-Identifier: GPL-3.0-or-later

//! Tracking a rectangle (pure; ADR-0089).
//!
//! - [`KeyTrack`]: moving and sizing a frame with the keyboard (the GUI
//!   thread's frame tracking, `gui/frame_track.rs`, whose documentation
//!   describes it).
//! - [`Tracker`]: `WinTrackRect`'s rectangle (`pm_track.rs`), from a
//!   `TRACKINFO`. Pointer moves and arrow keys both change one offset from
//!   where the tracking started; the rectangle is the starting one with
//!   that offset applied to its moving edges (`TF_LEFT` / `TF_TOP` /
//!   `TF_RIGHT` / `TF_BOTTOM`), snapped to the grid (`TF_GRID`), held to
//!   the minimum and maximum size, and kept in the boundary
//!   (`TF_ALLINBOUNDARY` / `TF_PARTINBOUNDARY`).

use super::constants::{TF_ALLINBOUNDARY, TF_BOTTOM, TF_GRID, TF_LEFT, TF_MOVE, TF_PARTINBOUNDARY, TF_RIGHT, TF_TOP};
use super::pm_minmax::WinRect;

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

/// A rectangle as a `RECTL`: left and bottom inclusive, right and top
/// exclusive, in the coordinates of the window tracked in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rectl {
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
    pub top: i32,
}

impl Rectl {
    pub fn width(&self) -> i32 { self.right.saturating_sub(self.left) }
    pub fn height(&self) -> i32 { self.top.saturating_sub(self.bottom) }
}

/// A `TRACKINFO` (`<pmwin.h>`), as read from the program.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackInfo {
    pub border: (i32, i32),
    pub grid: (i32, i32),
    pub keyboard: (i32, i32),
    pub track: Rectl,
    pub boundary: Rectl,
    pub min_size: (i32, i32),
    pub max_size: (i32, i32),
    pub fs: u32,
}

/// `TRACKINFO` field offsets and size.
pub const TI_RCL_TRACK: u32 = 24;
pub const TRACKINFO_SIZE: u32 = 76;

impl TrackInfo {
    /// Parse the 76 bytes of a `TRACKINFO`.
    pub fn parse(b: &[u8]) -> Option<TrackInfo> {
        let l = |i: usize| -> Option<i32> { Some(i32::from_le_bytes(b.get(4 * i..4 * i + 4)?.try_into().ok()?)) };
        let rect = |i: usize| -> Option<Rectl> { Some(Rectl { left: l(i)?, bottom: l(i + 1)?, right: l(i + 2)?, top: l(i + 3)? }) };
        Some(TrackInfo {
            border: (l(0)?, l(1)?), grid: (l(2)?, l(3)?), keyboard: (l(4)?, l(5)?),
            track: rect(6)?, boundary: rect(10)?,
            min_size: (l(14)?, l(15)?), max_size: (l(16)?, l(17)?),
            fs: l(18)? as u32,
        })
    }
}

/// A `WinTrackRect` in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tracker {
    info: TrackInfo,
    /// The pointer where the tracking started.
    start: (i32, i32),
    /// The offset from the start that pointer and keys made.
    offset: (i32, i32),
}

impl Tracker {
    pub fn new(info: TrackInfo, start: (i32, i32)) -> Tracker {
        Tracker { info, start, offset: (0, 0) }
    }

    /// The rectangle the tracking started with.
    pub fn original(&self) -> Rectl { self.info.track }

    /// Where the pointer is put with `TF_SETPOINTERPOS`: the middle of the
    /// rectangle for a move, else the middle of its moving edges.
    pub fn pointer_start(&self) -> (i32, i32) {
        let r = self.info.track;
        let fs = self.info.fs;
        let mid = |a: i32, b: i32| ((a as i64 + b as i64) / 2) as i32;
        let x = if fs & TF_MOVE == TF_MOVE || fs & (TF_LEFT | TF_RIGHT) == 0 { mid(r.left, r.right) }
                else if fs & TF_LEFT != 0 { r.left } else { r.right.saturating_sub(1) };
        let y = if fs & TF_MOVE == TF_MOVE || fs & (TF_TOP | TF_BOTTOM) == 0 { mid(r.bottom, r.top) }
                else if fs & TF_BOTTOM != 0 { r.bottom } else { r.top.saturating_sub(1) };
        (x, y)
    }

    /// The pointer moved to `pos`: the rectangle now.
    pub fn pointer(&mut self, pos: (i32, i32)) -> Rectl {
        self.offset = (pos.0.saturating_sub(self.start.0), pos.1.saturating_sub(self.start.1));
        self.rect()
    }

    /// An arrow key moved the edges one keyboard step (`dir`: -1, 0, 1
    /// on each axis): the rectangle now.
    pub fn step(&mut self, dir: (i32, i32)) -> Rectl {
        let (kx, ky) = (self.info.keyboard.0.max(1), self.info.keyboard.1.max(1));
        self.offset = (self.offset.0.saturating_add(dir.0.saturating_mul(kx)),
                       self.offset.1.saturating_add(dir.1.saturating_mul(ky)));
        self.rect()
    }

    /// The rectangle for the current offset.
    pub fn rect(&self) -> Rectl {
        let i = &self.info;
        let snap = |d: i32, g: i32| if i.fs & TF_GRID != 0 && g > 0 { d / g * g } else { d };
        let (dx, dy) = (snap(self.offset.0, i.grid.0), snap(self.offset.1, i.grid.1));
        let mut r = i.track;
        let fs = i.fs;
        if fs & TF_LEFT != 0 { r.left = r.left.saturating_add(dx); }
        if fs & TF_RIGHT != 0 { r.right = r.right.saturating_add(dx); }
        if fs & TF_BOTTOM != 0 { r.bottom = r.bottom.saturating_add(dy); }
        if fs & TF_TOP != 0 { r.top = r.top.saturating_add(dy); }
        // The size limits, when one edge of an axis moves (the other stays).
        let limit = |len: i32, min: i32, max: i32| {
            let len = if max > 0 { len.min(max) } else { len };
            len.max(min.max(0))
        };
        match (fs & TF_LEFT != 0, fs & TF_RIGHT != 0) {
            (true, false) => r.left = r.right.saturating_sub(limit(r.width(), i.min_size.0, i.max_size.0)),
            (false, true) => r.right = r.left.saturating_add(limit(r.width(), i.min_size.0, i.max_size.0)),
            _ => {}
        }
        match (fs & TF_BOTTOM != 0, fs & TF_TOP != 0) {
            (true, false) => r.bottom = r.top.saturating_sub(limit(r.height(), i.min_size.1, i.max_size.1)),
            (false, true) => r.top = r.bottom.saturating_add(limit(r.height(), i.min_size.1, i.max_size.1)),
            _ => {}
        }
        let b = i.boundary;
        if fs & TF_ALLINBOUNDARY != 0 {
            r = keep_in(r, b, fs);
        } else if fs & TF_PARTINBOUNDARY != 0 {
            r = keep_touching(r, b, fs);
        }
        r
    }
}

/// `TF_ALLINBOUNDARY`: a moving rectangle is shifted back inside `b`; a
/// moving edge stops at `b`'s edge.
fn keep_in(mut r: Rectl, b: Rectl, fs: u32) -> Rectl {
    let axis = |lo: &mut i32, hi: &mut i32, blo: i32, bhi: i32, mlo: bool, mhi: bool| {
        if mlo && mhi {
            if *lo < blo { *hi = hi.saturating_add(blo.saturating_sub(*lo)); *lo = blo; }
            if *hi > bhi { *lo = lo.saturating_sub(hi.saturating_sub(bhi)); *hi = bhi; }
        } else if mlo {
            *lo = (*lo).clamp(blo, (*hi).max(blo));
        } else if mhi {
            *hi = (*hi).clamp((*lo).min(bhi), bhi);
        }
    };
    axis(&mut r.left, &mut r.right, b.left, b.right, fs & TF_LEFT != 0, fs & TF_RIGHT != 0);
    axis(&mut r.bottom, &mut r.top, b.bottom, b.top, fs & TF_BOTTOM != 0, fs & TF_TOP != 0);
    r
}

/// `TF_PARTINBOUNDARY`: a moving rectangle keeps at least one pel inside
/// `b` (Warpine's reading; edges are not held).
fn keep_touching(mut r: Rectl, b: Rectl, fs: u32) -> Rectl {
    if fs & TF_MOVE != TF_MOVE { return r; }
    let (w, h) = (r.width(), r.height());
    if r.right <= b.left { r.right = b.left.saturating_add(1); r.left = r.right.saturating_sub(w); }
    if r.left >= b.right { r.left = b.right.saturating_sub(1); r.right = r.left.saturating_add(w); }
    if r.top <= b.bottom { r.top = b.bottom.saturating_add(1); r.bottom = r.top.saturating_sub(h); }
    if r.bottom >= b.top { r.bottom = b.top.saturating_sub(1); r.top = r.bottom.saturating_add(h); }
    r
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

    fn info(fs: u32) -> TrackInfo {
        TrackInfo { border: (1, 1), grid: (10, 10), keyboard: (4, 4),
                    track: Rectl { left: 100, bottom: 100, right: 200, top: 150 },
                    boundary: Rectl { left: 0, bottom: 0, right: 400, top: 300 },
                    min_size: (20, 20), max_size: (300, 200), fs }
    }

    #[test]
    fn a_trackinfo_is_read_field_by_field() {
        let words: Vec<i32> = (1..=19).collect();
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let i = TrackInfo::parse(&bytes).unwrap();
        assert_eq!((i.border, i.grid, i.keyboard), ((1, 2), (3, 4), (5, 6)));
        assert_eq!(i.track, Rectl { left: 7, bottom: 8, right: 9, top: 10 });
        assert_eq!(i.boundary, Rectl { left: 11, bottom: 12, right: 13, top: 14 });
        assert_eq!((i.min_size, i.max_size, i.fs), ((15, 16), (17, 18), 19));
        assert_eq!(bytes.len() as u32, TRACKINFO_SIZE);
        assert_eq!(TrackInfo::parse(&bytes[..75]), None);
    }

    #[test]
    fn a_move_shifts_every_edge_and_keys_step_it() {
        let mut t = Tracker::new(info(TF_MOVE), (150, 120));
        assert_eq!(t.pointer((170, 110)), Rectl { left: 120, bottom: 90, right: 220, top: 140 });
        assert_eq!(t.step((1, 0)), Rectl { left: 124, bottom: 90, right: 224, top: 140 }, "a key adds a step");
        assert_eq!(t.pointer((150, 120)), t.original(), "back where it began");
    }

    #[test]
    fn one_edge_moves_within_the_size_limits() {
        let mut t = Tracker::new(info(TF_RIGHT), (200, 120));
        assert_eq!(t.pointer((230, 0)).right, 230, "only the right edge, only along x");
        assert_eq!(t.pointer((230, 0)).bottom, 100);
        assert_eq!(t.pointer((50, 0)), Rectl { left: 100, bottom: 100, right: 120, top: 150 }, "minimum width 20");
        assert_eq!(t.pointer((500, 0)).right, 400, "maximum width 300");
        let mut t = Tracker::new(info(TF_LEFT | TF_TOP), (100, 150));
        assert_eq!(t.pointer((90, 160)), Rectl { left: 90, bottom: 100, right: 200, top: 160 });
        let mut z = info(TF_TOP);
        z.max_size = (0, 0);
        let mut t = Tracker::new(z, (0, 150));
        assert_eq!(t.pointer((0, 900)).top, 900, "a maximum of 0 sets no limit");
    }

    #[test]
    fn the_grid_snaps_the_offset() {
        let mut t = Tracker::new(info(TF_MOVE | TF_GRID), (0, 0));
        assert_eq!(t.pointer((17, -12)).left, 110);
        assert_eq!(t.pointer((17, -12)).bottom, 90);
        let mut t = Tracker::new(info(TF_MOVE), (0, 0));
        assert_eq!(t.pointer((17, 0)).left, 117, "no snapping without TF_GRID");
    }

    #[test]
    fn the_boundary_holds_the_rectangle() {
        let mut t = Tracker::new(info(TF_MOVE | TF_ALLINBOUNDARY), (0, 0));
        assert_eq!(t.pointer((-500, 900)), Rectl { left: 0, bottom: 250, right: 100, top: 300 }, "shifted back inside");
        let mut t = Tracker::new(info(TF_RIGHT | TF_ALLINBOUNDARY), (200, 0));
        assert_eq!(t.pointer((450, 0)).right, 400, "an edge stops at the boundary");
        let mut t = Tracker::new(info(TF_MOVE | TF_PARTINBOUNDARY), (0, 0));
        let r = t.pointer((1000, 1000));
        assert_eq!((r.left, r.bottom), (399, 299), "one pel stays inside");
        assert_eq!((r.width(), r.height()), (100, 50));
    }

    #[test]
    fn the_pointer_starts_on_what_moves() {
        assert_eq!(Tracker::new(info(TF_MOVE), (0, 0)).pointer_start(), (150, 125));
        assert_eq!(Tracker::new(info(TF_RIGHT | TF_TOP), (0, 0)).pointer_start(), (199, 149));
        assert_eq!(Tracker::new(info(TF_LEFT), (0, 0)).pointer_start(), (100, 125));
    }

    #[test]
    fn extreme_offsets_do_not_overflow() {
        let mut t = Tracker::new(info(TF_MOVE), (i32::MIN, i32::MIN));
        let _ = t.pointer((i32::MAX, i32::MAX));
        let _ = t.step((1, 1));
        let mut t = Tracker::new(info(TF_RIGHT | TF_ALLINBOUNDARY), (0, 0));
        let _ = t.pointer((i32::MAX, i32::MIN));
        let mut wild = info(TF_MOVE | TF_PARTINBOUNDARY);
        wild.track = Rectl { left: i32::MIN, bottom: i32::MIN, right: i32::MAX, top: i32::MAX };
        wild.boundary = Rectl { left: i32::MAX, bottom: i32::MAX, right: i32::MIN, top: i32::MIN };
        wild.keyboard = (i32::MAX, i32::MAX);
        for fs in [TF_MOVE | TF_PARTINBOUNDARY, TF_MOVE | TF_ALLINBOUNDARY, TF_LEFT | TF_BOTTOM | TF_ALLINBOUNDARY] {
            wild.fs = fs;
            let mut t = Tracker::new(wild, (0, 0));
            let _ = t.pointer((i32::MIN, i32::MAX));
            let _ = t.step((-1, 1));
            let _ = t.pointer_start();
        }
    }
}
