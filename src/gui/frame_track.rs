// SPDX-License-Identifier: GPL-3.0-or-later

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

//! The state machine lives in `loader/track.rs` (shared with
//! `WinTrackRect`, ADR-0089).

pub use crate::loader::track::{KeyTrack, TrackKey, TrackOutcome, FRAME_TRACK_STEP};
