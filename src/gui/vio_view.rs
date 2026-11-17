// SPDX-License-Identifier: GPL-3.0-only

//! Geometry and text of the VIO window's view on the logical video buffer
//! (LVB): which part of the text screen the window shows, its scroll bars,
//! marked rectangles and their text, and clipboard text turned into
//! keystrokes. Pure functions, no SDL; `vio_window.rs` uses them.
//!
//! IBM's `MouGetPtrPos` reference: "For a text window (VIO) application,
//! the text window is a view on the larger logical video buffer (LVB)". A
//! window smaller than the screen shows scroll bars (the GG24-3732
//! screenshot of an OS/2 window shows both). Their look and the details
//! below marked "unverified" are Warpine's own.

use crate::loader::vio_frame::{vio_scroll_bars, VIO_CELL_H, VIO_CELL_W, VIO_SCROLLBAR_W};
use crate::loader::KbdKeyInfo;
use super::text_renderer::CellKind;

/// Shortest scroll-bar thumb, in pels (Warpine's choice).
pub const MIN_THUMB: i32 = 8;

/// Most keystrokes one Paste types (Warpine's limit, so that a huge
/// clipboard cannot flood the keyboard queue).
pub const MAX_PASTE_KEYS: usize = 16 * 1024;

/// Character and scan code of Enter, which a line break is typed as.
pub const ENTER_KEY: (u8, u8) = (0x0D, 0x1C);
/// Character and scan code of Tab.
pub const TAB_KEY: (u8, u8) = (0x09, 0x0F);

/// How the client area of the VIO window is divided: the text view and, when
/// the window is smaller than the screen, a vertical scroll bar on its right
/// and a horizontal one below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VioLayout {
    pub cols: u16,
    pub rows: u16,
    /// Size of the text view in pels.
    pub view_w: i32,
    pub view_h: i32,
    /// Whether the vertical / horizontal scroll bar is shown.
    pub vbar: bool,
    pub hbar: bool,
}

impl VioLayout {
    /// The layout of a `client_w × client_h` client showing a `cols × rows`
    /// screen, with the scroll bars `vio_scroll_bars` asks for.
    pub fn new(client_w: i32, client_h: i32, cols: u16, rows: u16) -> VioLayout {
        let (cw, ch) = (client_w.max(0), client_h.max(0));
        let (vbar, hbar) = vio_scroll_bars(cw, ch, cols, rows);
        let view_w = (cw - if vbar { VIO_SCROLLBAR_W } else { 0 }).max(0);
        let view_h = (ch - if hbar { VIO_SCROLLBAR_W } else { 0 }).max(0);
        VioLayout { cols, rows, view_w, view_h, vbar, hbar }
    }

    /// Whole cells the view shows across and down.
    pub fn visible(&self) -> (i32, i32) {
        (self.view_w / VIO_CELL_W, self.view_h / VIO_CELL_H)
    }

    /// The largest view origin `(col, row)`: the last column and row then
    /// show whole at the right and bottom.
    pub fn max_origin(&self) -> (i32, i32) {
        let (vc, vr) = self.visible();
        ((self.cols as i32 - vc).max(0), (self.rows as i32 - vr).max(0))
    }

    /// `origin` moved into the range the view can scroll to.
    pub fn clamp_origin(&self, (col, row): (i32, i32)) -> (i32, i32) {
        let (mc, mr) = self.max_origin();
        (col.clamp(0, mc), row.clamp(0, mr))
    }

    /// The vertical scroll bar: along the view's right edge.
    pub fn vertical_bar(&self, origin: (i32, i32)) -> Option<ScrollBar> {
        self.vbar.then(|| ScrollBar {
            x: self.view_w, y: 0, w: VIO_SCROLLBAR_W, h: self.view_h, vertical: true,
            total: self.rows as i32, visible: self.visible().1, pos: origin.1,
        })
    }

    /// The horizontal scroll bar: along the view's bottom edge.
    pub fn horizontal_bar(&self, origin: (i32, i32)) -> Option<ScrollBar> {
        self.hbar.then(|| ScrollBar {
            x: 0, y: self.view_h, w: self.view_w, h: VIO_SCROLLBAR_W, vertical: false,
            total: self.cols as i32, visible: self.visible().0, pos: origin.0,
        })
    }

    /// The cell under client point `(x, y)` (from the client's top-left)
    /// with the view scrolled to `origin`, clamped into the view and the
    /// screen; `None` when the point is not over the text view.
    pub fn cell_at(&self, origin: (i32, i32), x: i32, y: i32) -> Option<(i32, i32)> {
        let inside = x >= 0 && y >= 0 && x < self.view_w && y < self.view_h;
        inside.then(|| self.cell_clamped(origin, x, y))
    }

    /// The cell under client point `(x, y)`, or the nearest one inside the
    /// view (a mark dragged past the view's edge).
    pub fn cell_clamped(&self, origin: (i32, i32), x: i32, y: i32) -> (i32, i32) {
        let x = x.clamp(0, (self.view_w - 1).max(0));
        let y = y.clamp(0, (self.view_h - 1).max(0));
        ((origin.0 + x / VIO_CELL_W).min(self.cols as i32 - 1).max(0),
         (origin.1 + y / VIO_CELL_H).min(self.rows as i32 - 1).max(0))
    }
}

/// The view origin after the cursor moved to `(col, row)`: scrolled as
/// little as possible to show it (keeping the cursor visible is Warpine's
/// choice; OS/2's behaviour is not documented).
pub fn follow_cursor(layout: &VioLayout, origin: (i32, i32), (col, row): (i32, i32)) -> (i32, i32) {
    let (vc, vr) = layout.visible();
    let follow = |o: i32, c: i32, v: i32| {
        if v <= 0 { o } else if c < o { c } else if c >= o + v { c - v + 1 } else { o }
    };
    layout.clamp_origin((follow(origin.0, col, vc), follow(origin.1, row, vr)))
}

/// Where a click on a scroll bar landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHit {
    /// The arrow at the top / left: one row or column back.
    LineBack,
    /// The arrow at the bottom / right: one row or column on.
    LineOn,
    /// The track before the thumb: a page back.
    PageBack,
    /// The track after the thumb: a page on.
    PageOn,
    /// The thumb, `offset` pels into it along the bar.
    Thumb { offset: i32 },
}

/// One scroll bar of the VIO window, in client coordinates (from the top-
/// left of the client): an arrow at each end, the track between them and a
/// thumb sized and placed by the visible part of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollBar {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub vertical: bool,
    /// Rows or columns of the screen, how many show, and the first shown.
    pub total: i32,
    pub visible: i32,
    pub pos: i32,
}

impl ScrollBar {
    /// Length of the bar and the size of each arrow button along it (the
    /// arrows shrink when the bar is too short for both).
    fn along(&self) -> (i32, i32) {
        let len = if self.vertical { self.h } else { self.w };
        (len, VIO_SCROLLBAR_W.min(len / 2))
    }

    /// Largest first row or column.
    pub fn max_pos(&self) -> i32 {
        (self.total - self.visible).max(0)
    }

    /// Start and length of the thumb along the bar (from the bar's start).
    pub fn thumb(&self) -> (i32, i32) {
        let (len, arrow) = self.along();
        let track = (len - 2 * arrow).max(0);
        let thumb = if self.total <= 0 { track } else {
            (track as i64 * self.visible as i64 / self.total as i64) as i32
        }.max(MIN_THUMB).min(track);
        let max = self.max_pos();
        let start = if max == 0 { 0 } else {
            ((track - thumb) as i64 * self.pos.clamp(0, max) as i64 / max as i64) as i32
        };
        (arrow + start, thumb)
    }

    /// What client point `(x, y)` hits, if it is on the bar.
    pub fn hit(&self, x: i32, y: i32) -> Option<BarHit> {
        if x < self.x || y < self.y || x >= self.x + self.w || y >= self.y + self.h { return None; }
        let a = if self.vertical { y - self.y } else { x - self.x };
        let (len, arrow) = self.along();
        let (t0, tl) = self.thumb();
        Some(if a < arrow {
            BarHit::LineBack
        } else if a >= len - arrow {
            BarHit::LineOn
        } else if a < t0 {
            BarHit::PageBack
        } else if a >= t0 + tl {
            BarHit::PageOn
        } else {
            BarHit::Thumb { offset: a - t0 }
        })
    }

    /// The first row or column when the thumb is dragged so that its start
    /// is `thumb_start` pels from the bar's start: the nearest position.
    pub fn pos_for_thumb(&self, thumb_start: i32) -> i32 {
        let (len, arrow) = self.along();
        let (_, tl) = self.thumb();
        let room = (len - 2 * arrow - tl).max(0);
        let max = self.max_pos();
        if room == 0 { return 0; }
        let t = (thumb_start - arrow).clamp(0, room) as i64;
        ((t * max as i64 + room as i64 / 2) / room as i64) as i32
    }

    /// The bar's first point along its axis, in client coordinates.
    pub fn start(&self) -> i32 {
        if self.vertical { self.y } else { self.x }
    }

    /// The new first row or column after a click on `hit` (not the thumb).
    pub fn scrolled(&self, hit: BarHit) -> i32 {
        let page = (self.visible - 1).max(1);
        let pos = match hit {
            BarHit::LineBack => self.pos - 1,
            BarHit::LineOn => self.pos + 1,
            BarHit::PageBack => self.pos - page,
            BarHit::PageOn => self.pos + page,
            BarHit::Thumb { .. } => self.pos,
        };
        pos.clamp(0, self.max_pos())
    }
}

/// Colours of the scroll bars.
const BAR_TRACK: u32 = 0x00A0_A0A0;
const BAR_FACE: u32 = 0x00CC_CCCC;
const BAR_LIGHT: u32 = 0x00FF_FFFF;
const BAR_SHADOW: u32 = 0x0080_8080;
const BAR_INK: u32 = 0x0000_0000;

/// Fill `[x0, x1) × [y0, y1)` of `buf` (`w` pels wide), clipped.
fn fill(buf: &mut [u32], w: usize, (x0, y0, x1, y1): (i32, i32, i32, i32), color: u32) {
    let r = super::damage::Rect::new(x0, y0, x1, y1);
    super::damage::fill_clipped(buf, w, r, r, color);
}

/// A raised box: face colour with a light top-left and dark bottom-right edge.
fn raised(buf: &mut [u32], w: usize, (x0, y0, x1, y1): (i32, i32, i32, i32)) {
    fill(buf, w, (x0, y0, x1, y1), BAR_FACE);
    fill(buf, w, (x0, y0, x1, y0 + 1), BAR_LIGHT);
    fill(buf, w, (x0, y0, x0 + 1, y1), BAR_LIGHT);
    fill(buf, w, (x0, y1 - 1, x1, y1), BAR_SHADOW);
    fill(buf, w, (x1 - 1, y0, x1, y1), BAR_SHADOW);
}

/// A small filled triangle in the box `(x0, y0, x1, y1)` pointing up, down,
/// left or right.
fn arrow(buf: &mut [u32], w: usize, (x0, y0, x1, y1): (i32, i32, i32, i32), vertical: bool, back: bool) {
    let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
    let n = ((x1 - x0).min(y1 - y0) / 4).max(1);
    for i in 0..n {
        // Row i of the triangle, from its tip: 2i + 1 pels wide.
        let d = if back { i - n / 2 } else { n / 2 - i };
        if vertical {
            fill(buf, w, (cx - i, cy + d, cx + i + 1, cy + d + 1), BAR_INK);
        } else {
            fill(buf, w, (cx + d, cy - i, cx + d + 1, cy + i + 1), BAR_INK);
        }
    }
}

impl ScrollBar {
    /// Draw the bar into `buf`, a client-sized buffer `w` pels wide.
    pub fn draw(&self, buf: &mut [u32], w: usize) {
        let (x0, y0, x1, y1) = (self.x, self.y, self.x + self.w, self.y + self.h);
        fill(buf, w, (x0, y0, x1, y1), BAR_TRACK);
        let (len, arrow_len) = self.along();
        let (t0, tl) = self.thumb();
        let part = |a0: i32, a1: i32| if self.vertical { (x0, y0 + a0, x1, y0 + a1) } else { (x0 + a0, y0, x0 + a1, y1) };
        for (a0, back) in [(0, true), (len - arrow_len, false)] {
            let r = part(a0, a0 + arrow_len);
            raised(buf, w, r);
            arrow(buf, w, r, self.vertical, back);
        }
        if tl > 0 {
            raised(buf, w, part(t0, t0 + tl));
        }
    }
}

/// A rectangle of cells, corners inclusive, normalised so that
/// `col0 <= col1` and `row0 <= row1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect {
    pub col0: i32,
    pub row0: i32,
    pub col1: i32,
    pub row1: i32,
}

impl CellRect {
    /// The rectangle spanned by two corner cells.
    pub fn spanning(a: (i32, i32), b: (i32, i32)) -> CellRect {
        CellRect { col0: a.0.min(b.0), row0: a.1.min(b.1), col1: a.0.max(b.0), row1: a.1.max(b.1) }
    }

    /// The whole `cols × rows` screen.
    pub fn screen(cols: u16, rows: u16) -> CellRect {
        CellRect { col0: 0, row0: 0, col1: cols as i32 - 1, row1: rows as i32 - 1 }
    }

    pub fn contains(&self, col: i32, row: i32) -> bool {
        col >= self.col0 && col <= self.col1 && row >= self.row0 && row <= self.row1
    }
}

/// The text of the cells in `rect` of a `cols`-wide screen: one line per
/// row with trailing blanks removed, lines separated by CR LF (no line break
/// after the last). The right half of a double-width character is skipped,
/// its left half carries the character; NULs read as blanks. That trailing
/// blanks go is from the GG24-3731 steps; the rest is Warpine's choice.
pub fn rect_text(cells: &[(char, u8)], kinds: &[CellKind], cols: u16, rect: CellRect) -> String {
    let cols = cols as i32;
    let mut lines = Vec::new();
    for row in rect.row0.max(0)..=rect.row1 {
        let mut line = String::new();
        for col in rect.col0.max(0)..=rect.col1.min(cols - 1) {
            let i = (row * cols + col) as usize;
            let Some(&(ch, _)) = cells.get(i) else { break };
            if matches!(kinds.get(i), Some(CellKind::DbcsTail)) { continue; }
            line.push(if ch == '\0' { ' ' } else { ch });
        }
        lines.push(line.trim_end_matches(' ').to_string());
    }
    lines.join("\r\n")
}

/// The keystrokes that type `text` into a VIO program (Paste: "The text is
/// copied from the clipboard character-by-character into the input area",
/// GG24-3731): each line break (CR LF, CR or LF) is Enter, a tab is Tab,
/// other control characters are dropped, and every other character is
/// encoded in codepage `cp` and typed byte by byte with scan code 0, as
/// `text_renderer::kbd_text_input` types composed text. At most
/// [`MAX_PASTE_KEYS`] keystrokes. Scan codes and the dropped characters
/// are Warpine's choice.
pub fn paste_keys(text: &str, cp: u32) -> Vec<KbdKeyInfo> {
    use crate::loader::codepage::cp_encode;
    let key = |(ch, scan): (u8, u8)| KbdKeyInfo { ch, scan, state: 0 };
    let mut keys = Vec::new();
    let mut chars = text.chars().peekable();
    let mut buf = [0u8; 4];
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                chars.next_if_eq(&'\n');
                keys.push(key(ENTER_KEY));
            }
            '\n' => keys.push(key(ENTER_KEY)),
            '\t' => keys.push(key(TAB_KEY)),
            c if c.is_control() => {}
            c if c.is_ascii() => keys.push(key((c as u8, 0))),
            c => keys.extend(cp_encode(c.encode_utf8(&mut buf), cp).into_iter().map(|b| key((b, 0)))),
        }
        if keys.len() >= MAX_PASTE_KEYS { break; }
    }
    keys.truncate(MAX_PASTE_KEYS);
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_screen_needs_no_scroll_bars() {
        let l = VioLayout::new(640, 400, 80, 25);
        assert_eq!(l, VioLayout { cols: 80, rows: 25, view_w: 640, view_h: 400, vbar: false, hbar: false });
        assert_eq!(l.max_origin(), (0, 0));
        assert!(l.vertical_bar((0, 0)).is_none() && l.horizontal_bar((0, 0)).is_none());
    }

    #[test]
    fn a_narrower_window_needs_both_bars() {
        // Narrower only: the horizontal bar takes height, so rows no longer fit.
        let l = VioLayout::new(600, 400, 80, 25);
        assert!(l.hbar && l.vbar);
        assert_eq!((l.view_w, l.view_h), (600 - VIO_SCROLLBAR_W, 400 - VIO_SCROLLBAR_W));
        // Shorter only, with room to spare across: just the vertical bar
        // would need 16 more pels, which the width does not have.
        let l = VioLayout::new(640, 300, 80, 25);
        assert!(l.vbar && l.hbar);
        // A client wider than the screen keeps room for the vertical bar.
        let l = VioLayout::new(640 + VIO_SCROLLBAR_W, 300, 80, 25);
        assert!(l.vbar && !l.hbar);
        assert_eq!(l.visible(), (80, 300 / 16));
        assert_eq!(l.max_origin(), (0, 25 - 18));
    }

    #[test]
    fn the_origin_stays_inside_the_screen() {
        let l = VioLayout::new(320 + 16, 160 + 16, 80, 25);
        assert_eq!(l.visible(), (40, 10));
        assert_eq!(l.max_origin(), (40, 15));
        assert_eq!(l.clamp_origin((-3, 99)), (0, 15));
        assert_eq!(l.clamp_origin((12, 7)), (12, 7));
    }

    #[test]
    fn the_view_follows_the_cursor() {
        let l = VioLayout::new(320 + 16, 160 + 16, 80, 25);
        assert_eq!(follow_cursor(&l, (0, 0), (5, 5)), (0, 0), "visible: no scrolling");
        assert_eq!(follow_cursor(&l, (0, 0), (50, 24)), (11, 15), "scrolled just far enough");
        assert_eq!(follow_cursor(&l, (30, 10), (2, 3)), (2, 3), "back up and left");
    }

    #[test]
    fn cells_under_the_pointer_include_the_scroll_position() {
        let l = VioLayout::new(320 + 16, 160 + 16, 80, 25);
        assert_eq!(l.cell_at((0, 0), 0, 0), Some((0, 0)));
        assert_eq!(l.cell_at((10, 3), 17, 33), Some((12, 5)));
        assert_eq!(l.cell_at((0, 0), 320, 0), None, "on the vertical bar");
        assert_eq!(l.cell_at((0, 0), -1, 0), None);
        assert_eq!(l.cell_clamped((0, 0), 5000, -20), (39, 0), "dragged out of the view");
    }

    fn bar() -> ScrollBar {
        // A 160-pel vertical bar for 25 rows, 10 visible, at row 0.
        ScrollBar { x: 320, y: 0, w: 16, h: 160, vertical: true, total: 25, visible: 10, pos: 0 }
    }

    #[test]
    fn the_thumb_shows_the_visible_part() {
        let b = bar();
        // Track 128 pels; 10 of 25 rows: 51 pels.
        assert_eq!(b.thumb(), (16, 51));
        let b = ScrollBar { pos: 15, ..bar() };
        assert_eq!(b.thumb(), (16 + 128 - 51, 51));
        // Many rows: the thumb keeps a minimum size.
        let b = ScrollBar { total: 10_000, ..bar() };
        assert_eq!(b.thumb().1, MIN_THUMB);
    }

    #[test]
    fn clicks_on_a_bar_scroll_a_row_or_a_page() {
        let b = ScrollBar { pos: 5, ..bar() };
        let (t0, tl) = b.thumb();
        assert_eq!(b.hit(325, 3), Some(BarHit::LineBack));
        assert_eq!(b.hit(325, 159), Some(BarHit::LineOn));
        assert_eq!(b.hit(325, t0 - 1), Some(BarHit::PageBack));
        assert_eq!(b.hit(325, t0 + tl), Some(BarHit::PageOn));
        assert_eq!(b.hit(325, t0 + 2), Some(BarHit::Thumb { offset: 2 }));
        assert_eq!(b.hit(319, 50), None);
        assert_eq!(b.scrolled(BarHit::LineBack), 4);
        assert_eq!(b.scrolled(BarHit::LineOn), 6);
        assert_eq!(b.scrolled(BarHit::PageOn), 14);
        assert_eq!(b.scrolled(BarHit::PageBack), 0);
        let end = ScrollBar { pos: 15, ..bar() };
        assert_eq!(end.scrolled(BarHit::LineOn), 15, "not past the end");
    }

    #[test]
    fn dragging_the_thumb_picks_the_nearest_position() {
        let b = bar();
        assert_eq!(b.pos_for_thumb(16), 0);
        assert_eq!(b.pos_for_thumb(16 + 77), 15, "the end of the track");
        assert_eq!(b.pos_for_thumb(500), 15);
        assert_eq!(b.pos_for_thumb(-500), 0);
        let mid = b.pos_for_thumb(16 + 77 / 2);
        assert!((7..=8).contains(&mid), "{mid}");
        // The thumb drawn at a position maps back to it.
        for pos in 0..=15 {
            let b = ScrollBar { pos, ..bar() };
            assert_eq!(b.pos_for_thumb(b.thumb().0), pos);
        }
    }

    #[test]
    fn a_bar_draws_its_track_arrows_and_thumb() {
        let (w, h) = (336usize, 160usize);
        let mut buf = vec![0u32; w * h];
        let b = ScrollBar { pos: 15, ..bar() };
        b.draw(&mut buf, w);
        let px = |x: usize, y: usize| buf[y * w + x];
        assert_eq!(px(10, 10), 0, "the view is untouched");
        assert_eq!(px(328, 20), BAR_TRACK, "the track above the thumb");
        assert_eq!(px(321, 2), BAR_FACE, "the arrow button");
        let (t0, _) = b.thumb();
        assert_eq!(px(328, t0 as usize + 3), BAR_FACE, "the thumb");
        assert!((0..16).any(|y| px(328, y) == BAR_INK), "the arrow's pictogram");
    }

    fn screen(lines: &[&str], cols: usize) -> Vec<(char, u8)> {
        lines.iter().flat_map(|l| {
            let mut v: Vec<(char, u8)> = l.chars().map(|c| (c, 7)).collect();
            v.resize(cols, (' ', 7));
            v
        }).collect()
    }

    #[test]
    fn a_marked_rectangle_becomes_lines_of_text() {
        let cells = screen(&["hello world", "  second   ", "third"], 12);
        let kinds = vec![CellKind::Sbcs; cells.len()];
        let r = CellRect::spanning((6, 2), (0, 0));
        assert_eq!(r, CellRect { col0: 0, row0: 0, col1: 6, row1: 2 });
        assert_eq!(rect_text(&cells, &kinds, 12, r), "hello w\r\n  secon\r\nthird");
        assert_eq!(rect_text(&cells, &kinds, 12, CellRect::spanning((2, 1), (2, 1))), "s");
        assert_eq!(rect_text(&cells, &kinds, 12, CellRect::screen(12, 3)),
                   "hello world\r\n  second\r\nthird", "Copy All: trailing blanks trimmed");
        assert_eq!(rect_text(&cells, &kinds, 12, CellRect::spanning((9, 1), (11, 1))), "", "blank");
    }

    #[test]
    fn double_width_characters_are_copied_once() {
        let cells = vec![('漢', 7), ('\u{fffd}', 7), ('a', 7), ('\0', 7)];
        let kinds = vec![CellKind::DbcsLead, CellKind::DbcsTail, CellKind::Sbcs, CellKind::Sbcs];
        assert_eq!(rect_text(&cells, &kinds, 4, CellRect::screen(4, 1)), "漢a");
        assert_eq!(rect_text(&cells, &kinds, 2, CellRect::screen(2, 2)), "漢\r\na");
    }

    #[test]
    fn pasted_text_is_typed_key_by_key() {
        let keys: Vec<(u8, u8)> = paste_keys("ab\r\nc\nd\re\tf\u{7}", 437)
            .iter().map(|k| (k.ch, k.scan)).collect();
        assert_eq!(keys, vec![(b'a', 0), (b'b', 0), ENTER_KEY, (b'c', 0), ENTER_KEY, (b'd', 0), ENTER_KEY,
                              (b'e', 0), TAB_KEY, (b'f', 0)]);
        // Non-ASCII characters are encoded in the active codepage.
        let keys: Vec<u8> = paste_keys("é", 437).iter().map(|k| k.ch).collect();
        assert_eq!(keys, vec![0x82], "é in CP437");
        let keys: Vec<u8> = paste_keys("漢", 932).iter().map(|k| k.ch).collect();
        assert_eq!(keys.len(), 2, "a DBCS character is two bytes");
        assert!(paste_keys(&"x".repeat(MAX_PASTE_KEYS + 10), 437).len() == MAX_PASTE_KEYS);
    }
}
