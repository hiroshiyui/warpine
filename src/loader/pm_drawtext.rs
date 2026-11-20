// SPDX-License-Identifier: GPL-3.0-or-later
//
// WinDrawText: one line of text formatted into a rectangle.
//
// From the PM Programming Reference: a carriage return or line feed ends the
// line; `cchText` -1 means a NUL-terminated string; the rectangle's boundary
// points are inside it; DT_LEFT / DT_CENTER / DT_RIGHT and DT_TOP /
// DT_VCENTER / DT_BOTTOM place the text; DT_WORDBREAK draws only the words
// that fit ("any number of leading spaces followed by one or more visible
// characters", trailing blanks not counted) but always at least one;
// DT_QUERYEXTENT changes the rectangle to the one that bounds the text;
// DT_ERASERECT erases the rectangle first; DT_TEXTATTRS takes the
// presentation space's colours instead of `clrFore` / `clrBack`; the
// background, the box bounding the text, is drawn with the PS's background
// mix (left alone by default); DT_MNEMONIC draws the character after the
// mnemonic prefix with emphasis; the result is the count of characters
// drawn — without DT_WORDBREAK the whole string's length; an invalid flag
// fails with PMERR_INVALID_FLAG.
//
// Warpine's own choices where the reference is silent (unverified): the
// count with DT_WORDBREAK takes in trailing blanks and the line end (CR, LF
// or CR LF) when the whole line fit, so a caller that advances by it moves
// on to the next line; nothing is drawn with DT_QUERYEXTENT; the rectangle
// written by DT_QUERYEXTENT includes its boundary, like the input one;
// DT_ERASERECT erases with the background colour; DT_HALFTONE draws the
// text in the mean of the foreground and background colours; the mnemonic
// prefix is `~` as in control texts, and emphasis is an underline, as is
// DT_UNDERSCORE; DT_EXTERNALLEADING adds nothing (Warpine's fonts have no
// external leading); the text is not clipped to the rectangle; the current
// position does not move.

use super::ApiResult;
use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_rect::Rectl;
use crate::gui::font::{self, CjkRegion, FontSpec};
use crate::gui::surface::{DrawOp, Mix};

/// Every flag WinDrawText knows.
const DT_ALL_FLAGS: u32 = DT_LEFT | DT_QUERYEXTENT | DT_UNDERSCORE | DT_STRIKEOUT | DT_TEXTATTRS
    | DT_EXTERNALLEADING | DT_CENTER | DT_RIGHT | DT_TOP | DT_VCENTER | DT_BOTTOM | DT_HALFTONE
    | DT_MNEMONIC | DT_WORDBREAK | DT_ERASERECT;

/// `cchText` meaning "NUL-terminated".
const CCH_NUL_TERMINATED: i32 = -1;
/// Most characters one call reads from the guest.
const MAX_DRAW_TEXT_CHARS: usize = 4096;
/// Pels between the bottom of the line box and an underline.
const UNDERLINE_RISE: i32 = 2;

const CR: u8 = b'\r';
const LF: u8 = b'\n';
const SPACE: u8 = b' ';

/// What one WinDrawText call draws from `bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineLayout {
    /// Bytes `0..drawn` are drawn.
    pub drawn: usize,
    /// The count the call returns.
    pub count: usize,
}

/// Lay out the first line of `bytes`: everything up to the first CR or LF,
/// or with `word_break` only the words that fit in `max_width` pels
/// (`width` measures a prefix). Words are split at spaces only, which are
/// never DBCS trail bytes, so a DBCS character is never cut in two.
pub fn layout_line(bytes: &[u8], word_break: bool, max_width: i32, width: impl Fn(&[u8]) -> i32) -> LineLayout {
    let line_end = bytes.iter().position(|&b| b == CR || b == LF).unwrap_or(bytes.len());
    if !word_break {
        return LineLayout { drawn: line_end, count: bytes.len() };
    }
    let terminator = match &bytes[line_end..] {
        [CR, LF, ..] => 2,
        [CR | LF, ..] => 1,
        _ => 0,
    };
    let mut drawn = 0;
    let mut i = 0;
    while i < line_end {
        // A word: leading spaces, then visible characters up to a space.
        let mut end = i;
        while end < line_end && bytes[end] == SPACE { end += 1; }
        if end == line_end { break; }
        while end < line_end && bytes[end] != SPACE { end += 1; }
        if drawn > 0 && width(&bytes[..end]) > max_width { break; }
        drawn = end;
        i = end;
    }
    let rest_blank = bytes[drawn..line_end].iter().all(|&b| b == SPACE);
    let count = if rest_blank { line_end + terminator } else { drawn };
    LineLayout { drawn, count }
}

/// `text` without its mnemonic prefix, and the character index the prefix
/// marked.
fn strip_mnemonic(text: &str) -> (String, Option<usize>) {
    super::pm_dialog::mnemonic_label(text)
}

/// Per-channel mean of two 0x00RRGGBB colours (DT_HALFTONE).
fn halftone(a: u32, b: u32) -> u32 {
    let mean = |shift: u32| ((((a >> shift) & 0xFF) + ((b >> shift) & 0xFF)) / 2) << shift;
    mean(16) | mean(8) | mean(0)
}

/// Where the text goes in `rc` (boundary inside): the left end and baseline.
fn place(rc: &Rectl, flags: u32, text_w: i32, ascent: i32, descent: i32) -> (i32, i32) {
    let avail_w = rc.x_right - rc.x_left + 1;
    let avail_h = rc.y_top - rc.y_bottom + 1;
    let x = if flags & DT_RIGHT != 0 {
        rc.x_right + 1 - text_w
    } else if flags & DT_CENTER != 0 {
        rc.x_left + (avail_w - text_w) / 2
    } else {
        rc.x_left
    };
    let y = if flags & DT_BOTTOM != 0 {
        rc.y_bottom + descent
    } else if flags & DT_VCENTER != 0 {
        rc.y_bottom + (avail_h - (ascent + descent)) / 2 + descent
    } else {
        rc.y_top + 1 - ascent
    };
    (x, y)
}

impl super::Loader {
    /// WinDrawText(hps, cchText, lpchText, prcl, clrFore, clrBack, flCmd).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn win_draw_text(&self, tid: u32, hps: u32, cch: i32, pch: u32, prcl: u32,
                                fore: u32, back: u32, flags: u32) -> ApiResult {
        if flags & !DT_ALL_FLAGS != 0 {
            return self.pm_fail(tid, PMERR_INVALID_FLAG);
        }
        if !self.shared.window_mgr.lock_or_recover().ps_map.contains_key(&hps) {
            return self.pm_fail(tid, PMERR_INV_HPS);
        }
        let Some(bytes) = self.draw_text_bytes(cch, pch) else { return ApiResult::Normal(0) };
        let Some(rc) = self.read_rectl(prcl) else { return ApiResult::Normal(0) };
        let cp = self.ps_codepage(hps);
        let font = FontSpec { cjk: CjkRegion::from_codepage(cp), ..self.ps_font(hps) };
        let mnemonic = flags & DT_MNEMONIC != 0;
        let shown = |b: &[u8]| -> (String, Option<usize>) {
            let text = super::codepage::cp_decode(b, cp);
            if mnemonic { strip_mnemonic(&text) } else { (text, None) }
        };
        let measure = |b: &[u8]| font::text_width(&shown(b).0, font);

        let layout = layout_line(&bytes, flags & DT_WORDBREAK != 0, rc.x_right - rc.x_left + 1, measure);
        let (text, mark) = shown(&bytes[..layout.drawn]);
        let m = font::metrics(font);
        let text_w = font::text_width(&text, font);
        let (x, y) = place(&rc, flags, text_w, m.ascent, m.descent);

        if flags & DT_QUERYEXTENT != 0 {
            let extent = Rectl::new(x, y - m.descent, x + text_w - 1, y + m.ascent - 1);
            if !self.write_rectl(prcl, extent) { return ApiResult::Normal(0); }
            return ApiResult::Normal(layout.count as u32);
        }

        let Some(t) = self.ps_target(hps) else { return ApiResult::Normal(0) };
        let (fg, bg) = if flags & DT_TEXTATTRS != 0 {
            (t.color, t.back_color)
        } else {
            let wm = self.shared.window_mgr.lock_or_recover();
            let ps = &wm.ps_map[&hps];
            (ps.resolve_color(fore), ps.resolve_color(back))
        };
        let fg = if flags & DT_HALFTONE != 0 { halftone(fg, bg) } else { fg };
        let bottom = y - m.descent;
        let box_op = |x1: i32, y1: i32, x2: i32, y2: i32, color: u32, mix: Mix| DrawOp::Box {
            x1, y1, x2, y2, color, fill: true, mix,
        };
        if flags & DT_ERASERECT != 0 {
            self.draw_ps(&t, box_op(rc.x_left, rc.y_bottom, rc.x_right, rc.y_top, bg, Mix::Overpaint));
        }
        if text_w > 0 {
            if let Some(mix) = t.back_mix {
                self.draw_ps(&t, box_op(x, bottom, x + text_w - 1, y + m.ascent - 1, bg, mix));
            }
            self.draw_ps(&t, DrawOp::TextFont { x, y, text: text.clone(), color: fg, font, mix: t.mix });
            if flags & DT_UNDERSCORE != 0 {
                let uy = bottom + UNDERLINE_RISE;
                self.draw_ps(&t, box_op(x, uy, x + text_w - 1, uy, fg, t.mix));
            }
            if flags & DT_STRIKEOUT != 0 {
                let sy = y + m.x_height / 2;
                self.draw_ps(&t, box_op(x, sy, x + text_w - 1, sy, fg, t.mix));
            }
            if let Some(mark) = mark
                && let Some((start, ch)) = text.char_indices().nth(mark)
            {
                let x1 = x + font::text_width(&text[..start], font);
                let w = font::text_width(&text[start..start + ch.len_utf8()], font);
                let uy = bottom + UNDERLINE_RISE;
                self.draw_ps(&t, box_op(x1, uy, x1 + w - 1, uy, fg, t.mix));
            }
        }
        ApiResult::Normal(layout.count as u32)
    }

    /// The string of a WinDrawText call: `cch` bytes, or up to the NUL for
    /// -1, at most `MAX_DRAW_TEXT_CHARS`; `None` for a count below -1 or an
    /// unreadable string.
    fn draw_text_bytes(&self, cch: i32, pch: u32) -> Option<Vec<u8>> {
        let len = match cch {
            CCH_NUL_TERMINATED => MAX_DRAW_TEXT_CHARS,
            n if n < CCH_NUL_TERMINATED => return None,
            n => (n as usize).min(MAX_DRAW_TEXT_CHARS),
        };
        let mut bytes = Vec::with_capacity(len.min(256));
        for i in 0..len as u32 {
            let b = self.guest_read::<u8>(pch.wrapping_add(i))?;
            if cch == CCH_NUL_TERMINATED && b == 0 { break; }
            bytes.push(b);
        }
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 pels per byte.
    fn w(b: &[u8]) -> i32 { b.len() as i32 * 10 }

    #[test]
    fn a_line_ends_at_cr_or_lf_and_counts_the_whole_string() {
        assert_eq!(layout_line(b"Hello", false, 0, w), LineLayout { drawn: 5, count: 5 });
        assert_eq!(layout_line(b"one\r\ntwo", false, 999, w), LineLayout { drawn: 3, count: 8 });
        assert_eq!(layout_line(b"one\ntwo", false, 999, w).drawn, 3);
        assert_eq!(layout_line(b"", false, 999, w), LineLayout { drawn: 0, count: 0 });
    }

    #[test]
    fn word_break_draws_the_words_that_fit() {
        // "aa bb cc": "aa bb" is 50 pels.
        assert_eq!(layout_line(b"aa bb cc", true, 50, w), LineLayout { drawn: 5, count: 5 });
        assert_eq!(layout_line(b"aa bb cc", true, 49, w), LineLayout { drawn: 2, count: 2 });
        // Everything fits: the count takes the whole line.
        assert_eq!(layout_line(b"aa bb", true, 999, w), LineLayout { drawn: 5, count: 5 });
        // Trailing blanks do not count for the fit, and are taken in.
        assert_eq!(layout_line(b"aa   ", true, 20, w), LineLayout { drawn: 2, count: 5 });
    }

    #[test]
    fn word_break_always_draws_one_word() {
        assert_eq!(layout_line(b"toolongword next", true, 5, w), LineLayout { drawn: 11, count: 11 });
        assert_eq!(layout_line(b"  lead", true, 5, w), LineLayout { drawn: 6, count: 6 }, "leading spaces belong to the word");
    }

    #[test]
    fn word_break_moves_past_the_line_end_when_the_line_fit() {
        assert_eq!(layout_line(b"aa\r\nbb", true, 999, w), LineLayout { drawn: 2, count: 4 });
        assert_eq!(layout_line(b"aa \nbb", true, 999, w), LineLayout { drawn: 2, count: 4 });
        assert_eq!(layout_line(b"\nbb", true, 999, w), LineLayout { drawn: 0, count: 1 }, "an empty line");
        assert_eq!(layout_line(b"aa bb\ncc", true, 20, w), LineLayout { drawn: 2, count: 2 });
    }

    #[test]
    fn placement_in_an_inclusive_rectangle() {
        let rc = Rectl::new(10, 20, 109, 59); // 100 × 40 pels
        // Top-left: the line's top row is yTop.
        assert_eq!(place(&rc, DT_LEFT | DT_TOP, 30, 12, 4), (10, 48));
        assert_eq!(place(&rc, DT_RIGHT | DT_BOTTOM, 30, 12, 4), (80, 24));
        assert_eq!(place(&rc, DT_CENTER | DT_VCENTER, 30, 12, 4), (45, 36));
    }

    #[test]
    fn halftone_is_the_mean_colour() {
        assert_eq!(halftone(0x000000, 0xFFFFFF), 0x7F7F7F);
        assert_eq!(halftone(0xFF0000, 0x0000FF), 0x7F007F);
    }

    // ── Through the API ────────────────────────────────────────────────

    use crate::loader::Loader;
    use crate::loader::mutex_ext::MutexExt;
    use crate::loader::pm_error::error_id;

    // CLR_RED, CLR_WHITE, CLR_BLUE (<pmgpi.h>).
    const CLR_RED_VALUE: u32 = 2;
    const CLR_WHITE_VALUE: u32 = (-2i32) as u32;
    const CLR_BLUE_VALUE: u32 = 1;

    const TEXT: u32 = 0x5000;
    const RC: u32 = 0x5100;

    fn window_ps(loader: &Loader) -> (u32, crate::gui::SharedSurface) {
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hwnd = wm.create_window("Test".to_string(), 0, 0);
        let w = wm.get_window_mut(hwnd).unwrap();
        (w.cx, w.cy) = (200, 100);
        let hps = wm.create_ps(hwnd);
        let surface = wm.frame_surface(hwnd).unwrap();
        surface.lock().unwrap().record = Some(Vec::new());
        (hps, surface)
    }

    fn put_text(loader: &Loader, s: &[u8]) {
        for (i, &b) in s.iter().enumerate() { loader.guest_write::<u8>(TEXT + i as u32, b).unwrap(); }
        loader.guest_write::<u8>(TEXT + s.len() as u32, 0).unwrap();
    }

    fn draw(loader: &Loader, hps: u32, cch: i32, flags: u32) -> u32 {
        match loader.win_draw_text(0, hps, cch, TEXT, RC, CLR_RED_VALUE, CLR_WHITE_VALUE, flags) {
            ApiResult::Normal(v) => v,
            _ => panic!("unexpected ApiResult"),
        }
    }

    fn texts(surface: &crate::gui::SharedSurface) -> Vec<(i32, i32, String, u32)> {
        surface.lock().unwrap().record.as_ref().unwrap().iter().filter_map(|op| match op {
            DrawOp::TextFont { x, y, text, color, .. } => Some((*x, *y, text.clone(), *color)),
            _ => None,
        }).collect()
    }

    #[test]
    fn draws_the_first_line_and_returns_the_length() {
        let loader = Loader::new_mock();
        let (hps, surface) = window_ps(&loader);
        put_text(&loader, b"Hi\r\nthere");
        assert!(loader.write_rectl(RC, Rectl::new(0, 0, 199, 99)));
        assert_eq!(draw(&loader, hps, -1, DT_LEFT | DT_TOP), 9);
        let drawn = texts(&surface);
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].2, "Hi");
        assert_eq!(drawn[0].3, 0x00FF_0000, "clrFore");
        assert_eq!(drawn[0].0, 0, "left");
        let m = font::metrics(FontSpec::SYSTEM);
        assert_eq!(drawn[0].1, 100 - m.ascent, "top row at yTop");
        // An explicit count.
        assert_eq!(draw(&loader, hps, 2, DT_LEFT), 2);
    }

    #[test]
    fn query_extent_draws_nothing_and_bounds_the_text() {
        let loader = Loader::new_mock();
        let (hps, surface) = window_ps(&loader);
        put_text(&loader, b"Warp");
        assert!(loader.write_rectl(RC, Rectl::new(10, 10, 109, 59)));
        assert_eq!(draw(&loader, hps, -1, DT_QUERYEXTENT | DT_RIGHT | DT_BOTTOM), 4);
        assert!(texts(&surface).is_empty());
        let w = font::text_width("Warp", FontSpec::SYSTEM);
        let m = font::metrics(FontSpec::SYSTEM);
        assert_eq!(loader.read_rectl(RC), Some(Rectl::new(110 - w, 10, 109, 10 + m.line_height - 1)));
    }

    #[test]
    fn text_attrs_mnemonic_and_erase() {
        let loader = Loader::new_mock();
        let (hps, surface) = window_ps(&loader);
        loader.shared.window_mgr.lock_or_recover().ps_map.get_mut(&hps).unwrap().set_color(CLR_BLUE_VALUE);
        put_text(&loader, b"~File");
        assert!(loader.write_rectl(RC, Rectl::new(0, 0, 199, 99)));
        assert_eq!(draw(&loader, hps, -1, DT_TEXTATTRS | DT_MNEMONIC | DT_ERASERECT), 5);
        let rec = surface.lock().unwrap().record.clone().unwrap();
        assert!(matches!(rec[0], DrawOp::Box { x1: 0, y1: 0, x2: 199, y2: 99, fill: true, .. }), "erased first");
        let drawn = texts(&surface);
        assert_eq!(drawn[0].2, "File", "prefix not drawn");
        assert_eq!(drawn[0].3, 0x0000_00FF, "the PS colour");
        assert!(rec.iter().skip(2).any(|op| matches!(op, DrawOp::Box { .. })), "mnemonic underline");
    }

    #[test]
    fn bad_flags_counts_and_handles_fail() {
        let loader = Loader::new_mock();
        let (hps, _) = window_ps(&loader);
        put_text(&loader, b"x");
        assert!(loader.write_rectl(RC, Rectl::new(0, 0, 9, 9)));
        assert_eq!(draw(&loader, hps, -1, 0x0001), 0, "reserved bit");
        assert_eq!(loader.shared.pm_errors.take(0), error_id(SEVERITY_ERROR, PMERR_INVALID_FLAG));
        assert_eq!(draw(&loader, hps, -2, DT_LEFT), 0);
        assert_eq!(draw(&loader, 0xBAD, -1, DT_LEFT), 0);
        assert_eq!(loader.shared.pm_errors.take(0), error_id(SEVERITY_ERROR, PMERR_INV_HPS));
        assert!(matches!(loader.win_draw_text(0, hps, -1, TEXT, 0, 0, 0, DT_LEFT), ApiResult::Normal(0)), "NULL prcl");
    }
}
