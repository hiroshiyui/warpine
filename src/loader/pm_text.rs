// SPDX-License-Identifier: GPL-3.0-only

//! GPI text layout (PMGPI): character attributes and the text calls that
//! follow them.
//!
//! * Character attributes of a presentation space ([`TextAttrs`]):
//!   `GpiSetCharAngle` (the baseline direction, a `GRADIENTL`),
//!   `GpiSetCharShear` (the direction of the characters' vertical strokes),
//!   `GpiSetCharDirection` (`CHDIRN_*`), `GpiSetCharExtra` /
//!   `GpiSetCharBreakExtra` (FIXED spacing added to every character / to
//!   the break character), `GpiSetTextAlignment` (`TA_*`) and
//!   `GpiSetCharMode` (`CM_*`, stored only), with their queries.
//! * The layout of a string ([`layout`], pure): the origin of each character
//!   in the *text frame* — `u` along the baseline direction, `v`
//!   perpendicular to it (up) —, the concatenation point and the text box,
//!   for a direction, extra spacing or an increment vector, with the
//!   alignment applied. [`Frame`] maps the text frame to world coordinates
//!   (pels; the world → device transform is not applied here).
//! * `GpiCharString` / `GpiCharStringAt` (drawn from `pm_gpi.rs`),
//!   `GpiCharStringPos` / `GpiCharStringPosAt` (`CHS_*`),
//!   `GpiQueryCharStringPos` / `GpiQueryCharStringPosAt`, `GpiQueryTextBox`
//!   and `GpiQueryWidthTable` all use the same layout.
//!
//! Upright, left-to-right text without extra spacing or alignment is drawn
//! as one [`DrawOp::TextFont`], exactly as before these attributes existed;
//! everything else is a [`DrawOp::GlyphRun`] of placed glyphs, transformed
//! by the angle and shear (`gui/glyph_transform.rs`).
//!
//! The PM reference leaves several details open; Warpine's choices are
//! marked "unverified" where they are made.

use super::constants::*;
use super::locale::is_dbcs_lead_byte;
use super::mutex_ext::MutexExt;
use crate::gui::font::{self, CjkRegion, FontMetricsPx, FontSpec};
use crate::gui::glyph_transform::{Matrix, IDENTITY};
use crate::gui::region::{RRect, Region, SetOp};
use crate::gui::surface::{AreaFill, DrawOp, Pattern};

// ── pmgpi.h ──────────────────────────────────────────────────────────────────

/// `GpiCharStringPos` options.
pub const CHS_OPAQUE:     u32 = 0x0001;
pub const CHS_VECTOR:     u32 = 0x0002;
pub const CHS_LEAVEPOS:   u32 = 0x0008;
pub const CHS_CLIP:       u32 = 0x0010;
pub const CHS_UNDERSCORE: u32 = 0x0200;
pub const CHS_STRIKEOUT:  u32 = 0x0400;
const CHS_VALID: u32 = CHS_OPAQUE | CHS_VECTOR | CHS_LEAVEPOS | CHS_CLIP | CHS_UNDERSCORE | CHS_STRIKEOUT;

/// `GpiQueryTextBox` points.
pub const TXTBOX_TOPLEFT:     u32 = 0;
pub const TXTBOX_BOTTOMLEFT:  u32 = 1;
pub const TXTBOX_TOPRIGHT:    u32 = 2;
pub const TXTBOX_BOTTOMRIGHT: u32 = 3;
pub const TXTBOX_CONCAT:      u32 = 4;
pub const TXTBOX_COUNT:       u32 = 5;

/// Character modes (stored and reported; every Warpine font is an outline
/// font, so the mode changes nothing drawn).
pub const CM_ERROR:   i32 = -1;
pub const CM_DEFAULT: u32 = 0;
pub const CM_MODE1:   u32 = 1;
pub const CM_MODE3:   u32 = 3;

/// Character directions.
pub const CHDIRN_ERROR:     i32 = -1;
pub const CHDIRN_DEFAULT:   u32 = 0;
pub const CHDIRN_LEFTRIGHT: u32 = 1;
pub const CHDIRN_TOPBOTTOM: u32 = 2;
pub const CHDIRN_RIGHTLEFT: u32 = 3;
pub const CHDIRN_BOTTOMTOP: u32 = 4;

/// Text alignment (`GpiSetTextAlignment`).
pub const TA_NORMAL_HORIZ:   u32 = 0x0001;
pub const TA_LEFT:           u32 = 0x0002;
pub const TA_CENTER:         u32 = 0x0003;
pub const TA_RIGHT:          u32 = 0x0004;
pub const TA_STANDARD_HORIZ: u32 = 0x0005;
pub const TA_NORMAL_VERT:    u32 = 0x0100;
pub const TA_TOP:            u32 = 0x0200;
pub const TA_HALF:           u32 = 0x0300;
pub const TA_BASE:           u32 = 0x0400;
pub const TA_BOTTOM:         u32 = 0x0500;
pub const TA_STANDARD_VERT:  u32 = 0x0600;

/// `pmerr.h`.
pub const PMERR_INV_CHAR_ANGLE_ATTR:     u32 = 0x204B;
pub const PMERR_INV_CHAR_DIRECTION_ATTR: u32 = 0x204C;
pub const PMERR_INV_CHAR_MODE_ATTR:      u32 = 0x204D;
pub const PMERR_INV_CHAR_POS_OPTIONS:    u32 = 0x204E;
pub const PMERR_INV_CHAR_SHEAR_ATTR:     u32 = 0x2050;
pub const PMERR_INV_FIRST_CHAR:          u32 = 0x2071;
pub const PMERR_INV_RECT:                u32 = 0x20BD;
pub const PMERR_INV_CHAR_ALIGN_ATTR:     u32 = 0x2117;

/// 1.0 as a FIXED (16.16).
const FIXED_ONE: f32 = 65536.0;
/// The break character: the one `GpiSetCharBreakExtra` widens (the
/// `sBreakChar` Warpine reports in `FONTMETRICS`).
const BREAK_CHAR: char = ' ';
/// Code points a width table covers: single bytes and DBCS pairs.
const WIDTH_TABLE_LIMIT: i64 = 0x1_0000;
/// Size of a POINTL / LONG in guest memory.
const POINTL_SIZE: u32 = 8;
const LONG_SIZE: u32 = 4;

// ── Character attributes ─────────────────────────────────────────────────────

/// The character attributes of a presentation space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextAttrs {
    /// `GRADIENTL` of `GpiSetCharAngle`: the baseline runs towards it.
    pub angle: (i32, i32),
    /// `POINTL` of `GpiSetCharShear`: the vertical strokes run towards it
    /// (`y` is never 0).
    pub shear: (i32, i32),
    /// `CHDIRN_*` (never `CHDIRN_DEFAULT`).
    pub direction: u32,
    /// FIXED world units added to every character's advance.
    pub char_extra: i32,
    /// FIXED world units added to the break character's advance.
    pub break_extra: i32,
    /// `(TA_* horizontal, TA_* vertical)` as set.
    pub align: (u32, u32),
    /// `CM_*` (never `CM_DEFAULT`).
    pub mode: u32,
}

impl Default for TextAttrs {
    fn default() -> TextAttrs {
        TextAttrs {
            angle: (1, 0),
            shear: (0, 1),
            direction: CHDIRN_LEFTRIGHT,
            char_extra: 0,
            break_extra: 0,
            align: (TA_NORMAL_HORIZ, TA_NORMAL_VERT),
            // unverified: the reference gives mode 1 as the default.
            mode: CM_MODE1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction { LeftRight, RightLeft, TopBottom, BottomTop }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HAlign { Left, Center, Right }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VAlign { Top, Half, Base, Bottom }

impl TextAttrs {
    fn dir(&self) -> Direction {
        match self.direction {
            CHDIRN_RIGHTLEFT => Direction::RightLeft,
            CHDIRN_TOPBOTTOM => Direction::TopBottom,
            CHDIRN_BOTTOMTOP => Direction::BottomTop,
            _ => Direction::LeftRight,
        }
    }

    /// The alignment in effect. "Normal" puts the start point where the
    /// direction starts (left / right / centre; base / top / bottom), so
    /// it moves nothing. unverified: "standard" is taken as left (right
    /// for right-to-left) and bottom (top for top-to-bottom).
    fn alignment(&self) -> (HAlign, VAlign) {
        let dir = self.dir();
        let h = match self.align.0 {
            TA_LEFT => HAlign::Left,
            TA_CENTER => HAlign::Center,
            TA_RIGHT => HAlign::Right,
            TA_STANDARD_HORIZ if dir == Direction::RightLeft => HAlign::Right,
            TA_STANDARD_HORIZ => HAlign::Left,
            _ => match dir {
                Direction::LeftRight => HAlign::Left,
                Direction::RightLeft => HAlign::Right,
                Direction::TopBottom | Direction::BottomTop => HAlign::Center,
            },
        };
        let v = match self.align.1 {
            TA_TOP => VAlign::Top,
            TA_HALF => VAlign::Half,
            TA_BASE => VAlign::Base,
            TA_BOTTOM => VAlign::Bottom,
            TA_STANDARD_VERT if dir == Direction::TopBottom => VAlign::Top,
            TA_STANDARD_VERT => VAlign::Bottom,
            _ => match dir {
                Direction::LeftRight | Direction::RightLeft => VAlign::Base,
                Direction::TopBottom => VAlign::Top,
                Direction::BottomTop => VAlign::Bottom,
            },
        };
        (h, v)
    }

    /// Horizontal shear factor and vertical sign of the glyph shape:
    /// glyph-local `(u, v)` → `(u + s·v, sign·v)`. unverified: a negative
    /// shear `y` turns the characters upside down.
    fn shear_factor(&self) -> (f32, f32) {
        let (x, y) = self.shear;
        if y == 0 { return (0.0, 1.0); }
        (x as f32 / (y as f32).abs(), if y < 0 { -1.0 } else { 1.0 })
    }

    /// The transform of each glyph's shape: shear, then the baseline frame.
    pub fn glyph_matrix(&self) -> Matrix {
        let f = Frame::new(self.angle);
        let (s, sy) = self.shear_factor();
        let (bx, by, nx, ny) = (f.bx as f32, f.by as f32, f.nx as f32, f.ny as f32);
        [bx, bx * s + nx * sy, by, by * s + ny * sy]
    }
}

/// The baseline frame of a character angle: `b` the unit vector along the
/// baseline, `n` the unit vector perpendicular to it (b turned a quarter
/// counter-clockwise).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    bx: f64,
    by: f64,
    nx: f64,
    ny: f64,
}

impl Frame {
    /// The frame of `GRADIENTL` `(x, y)`; (0, 0) is the default angle.
    pub fn new((x, y): (i32, i32)) -> Frame {
        let (x, y) = (x as f64, y as f64);
        let len = x.hypot(y);
        if len == 0.0 { return Frame { bx: 1.0, by: 0.0, nx: 0.0, ny: 1.0 }; }
        let (bx, by) = (x / len, y / len);
        Frame { bx, by, nx: -by, ny: bx }
    }

    /// World point of text-frame `(u, v)` from `start`, rounded to pels.
    pub fn to_world(&self, start: (i32, i32), (u, v): (f32, f32)) -> (i32, i32) {
        let (u, v) = (u as f64, v as f64);
        ((start.0 as f64 + u * self.bx + v * self.nx).round() as i32,
         (start.1 as f64 + u * self.by + v * self.ny).round() as i32)
    }
}

// ── Layout ───────────────────────────────────────────────────────────────────

/// One character of a string: its glyph, how many bytes of the string it
/// takes (2 for a DBCS pair) and its advance in the current font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unit {
    pub ch: char,
    pub bytes: u8,
    pub advance: f32,
}

/// The vertical metrics a layout needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontBox {
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
}

impl From<FontMetricsPx> for FontBox {
    fn from(m: FontMetricsPx) -> FontBox {
        FontBox { ascent: m.ascent as f32, descent: m.descent as f32, line_height: m.line_height as f32 }
    }
}

/// A laid-out string in the text frame, alignment applied.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Origin (on its baseline, left edge) of each character.
    pub origins: Vec<(f32, f32)>,
    /// Where a following string would start.
    pub concat: (f32, f32),
    /// The text box in `TXTBOX_*` order: top left, bottom left, top right,
    /// bottom right.
    pub corners: [(f32, f32); 4],
    /// The box's extent along the baseline, `(u0, u1)`.
    pub span: (f32, f32),
}

/// Lay out `units` with `attrs`: each character advances by its width plus
/// the character extra (plus the break extra for the break character), or
/// by its entry of `increments` (then the extras do not apply — unverified:
/// the reference describes the increments as replacing the widths). Left to
/// right and right to left step along the baseline (right to left places
/// each character left of the pen); top to bottom and bottom to top step
/// by the line height, each character centred on the line through the
/// start (unverified: where the reference is silent on the vertical
/// directions, the start point is the top / bottom centre of the first
/// character cell).
pub fn layout(units: &[Unit], increments: Option<&[f32]>, attrs: &TextAttrs, fb: FontBox) -> Layout {
    let dir = attrs.dir();
    let vertical = matches!(dir, Direction::TopBottom | Direction::BottomTop);
    let char_extra = attrs.char_extra as f32 / FIXED_ONE;
    let break_extra = attrs.break_extra as f32 / FIXED_ONE;
    let step = |i: usize, u: &Unit| -> f32 {
        if let Some(inc) = increments { return inc.get(i).copied().unwrap_or(0.0); }
        let size = if vertical { fb.line_height } else { u.advance };
        size + char_extra + if u.ch == BREAK_CHAR { break_extra } else { 0.0 }
    };
    let mut origins = Vec::with_capacity(units.len());
    let mut pen = 0.0f32;
    for (i, u) in units.iter().enumerate() {
        match dir {
            Direction::LeftRight => { origins.push((pen, 0.0)); pen += step(i, u); }
            Direction::RightLeft => { pen -= step(i, u); origins.push((pen, 0.0)); }
            Direction::TopBottom => { origins.push((-u.advance / 2.0, pen - fb.ascent)); pen -= step(i, u); }
            Direction::BottomTop => { origins.push((-u.advance / 2.0, pen + fb.descent)); pen += step(i, u); }
        }
    }
    let (s, sy) = attrs.shear_factor();
    // Extent of the box (before shear) and the baseline of the first cell.
    let (u0, u1, v_bottom, v_top, base, concat) = if vertical {
        let half = units.iter().map(|u| u.advance / 2.0).fold(0.0f32, f32::max);
        let base = if dir == Direction::TopBottom { -fb.ascent } else { fb.descent };
        (-half, half, pen.min(0.0), pen.max(0.0), base, (0.0, pen))
    } else {
        let (lo, hi) = (sy * -fb.descent, sy * fb.ascent);
        (pen.min(0.0), pen.max(0.0), lo.min(hi), lo.max(hi), 0.0, (pen, 0.0))
    };
    // Corners: along the baseline the sheared cell's top and bottom edges
    // (glyph-local v = ascent and −descent); a vertical string is a plain
    // box of its cells.
    let corners = if vertical {
        [(u0, v_top), (u0, v_bottom), (u1, v_top), (u1, v_bottom)]
    } else {
        let (top, bottom) = (fb.ascent, -fb.descent);
        [(u0 + s * top, sy * top), (u0 + s * bottom, sy * bottom),
         (u1 + s * top, sy * top), (u1 + s * bottom, sy * bottom)]
    };
    let (h, v) = attrs.alignment();
    let ref_u = match h { HAlign::Left => u0, HAlign::Center => (u0 + u1) / 2.0, HAlign::Right => u1 };
    let ref_v = match v {
        VAlign::Top => v_top,
        VAlign::Half => (v_top + v_bottom) / 2.0,
        VAlign::Base => base,
        VAlign::Bottom => v_bottom,
    };
    let shift = |(a, b): (f32, f32)| (a - ref_u, b - ref_v);
    Layout {
        origins: origins.into_iter().map(shift).collect(),
        concat: shift(concat),
        corners: corners.map(shift),
        span: (u0 - ref_u, u1 - ref_u),
    }
}

/// Whether `l` places the string as plain upright text at the start point:
/// left to right, every character at its own width (no extras or
/// increments), no alignment offset.
fn plain(attrs: &TextAttrs, increments: bool) -> bool {
    attrs.dir() == Direction::LeftRight && !increments && attrs.char_extra == 0 && attrs.break_extra == 0
        && attrs.alignment() == (HAlign::Left, VAlign::Base) && attrs.glyph_matrix() == IDENTITY
}

/// Split `bytes` (codepage `cp`) into characters: DBCS lead bytes take the
/// next byte with them.
pub fn split_chars(bytes: &[u8], cp: u32) -> Vec<(char, u8)> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let n = if is_dbcs_lead_byte(bytes[i], cp) && i + 1 < bytes.len() { 2 } else { 1 };
        let ch = super::codepage::cp_decode(&bytes[i..i + n], cp).chars().next().unwrap_or('\u{FFFD}');
        out.push((ch, n as u8));
        i += n;
    }
    out
}

/// Per-character increments from per-byte ones: a DBCS character takes
/// the sum of its two bytes' increments (unverified).
fn unit_increments(units: &[Unit], per_byte: &[i32]) -> Vec<f32> {
    let mut at = 0usize;
    units.iter().map(|u| {
        let sum: i64 = per_byte.iter().skip(at).take(u.bytes as usize).map(|&v| v as i64).sum();
        at += u.bytes as usize;
        sum as f32
    }).collect()
}

/// How a string is drawn by `draw_string`.
struct StringCall {
    at: Option<(i32, i32)>,
    /// `RECTL` of `GpiCharStringPos` (`x_left, y_bottom, x_right, y_top`).
    rect: Option<(i32, i32, i32, i32)>,
    options: u32,
    increments: Option<Vec<i32>>,
}

impl super::Loader {
    /// The PMGPI character-attribute and text-layout calls by ordinal;
    /// `None` for other ordinals.
    pub(super) fn handle_text_call(&self, tid: u32, ordinal: u32, arg: &dyn Fn(u64) -> u32) -> Option<u32> {
        Some(match ordinal {
            580 => {
                // GpiCharStringPos(HPS hps, PRECTL prclRect, ULONG flOptions,
                //                  LONG lCount, PCH pchString, PLONG alAdx) → LONG
                self.char_string_pos(tid, arg(4), None, arg(8), arg(12), arg(16) as i32, arg(20), arg(24))
            }
            581 => {
                // GpiCharStringPosAt(HPS hps, PPOINTL pptlStart, PRECTL prclRect, ULONG flOptions,
                //                    LONG lCount, PCH pchString, PLONG alAdx) → LONG
                let Some(at) = self.get_pointl(arg(8)) else { return Some(self.text_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE)) };
                self.char_string_pos(tid, arg(4), Some(at), arg(12), arg(16), arg(20) as i32, arg(24), arg(28))
            }
            584 => {
                // GpiQueryCharStringPos(HPS hps, ULONG flOptions, LONG lCount, PCH pchString,
                //                       PLONG alXincrements, PPOINTL aptlPositions) → BOOL
                self.query_char_string_pos(tid, arg(4), None, arg(8), arg(12) as i32, arg(16), arg(20), arg(24))
            }
            585 => {
                // GpiQueryCharStringPosAt(HPS hps, PPOINTL pptlStart, ULONG flOptions, LONG lCount,
                //                         PCH pchString, PLONG alXincrements, PPOINTL aptlPositions) → BOOL
                let Some(at) = self.get_pointl(arg(8)) else { return Some(self.text_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE)) };
                self.query_char_string_pos(tid, arg(4), Some(at), arg(12), arg(16) as i32, arg(20), arg(24), arg(28))
            }
            492 => {
                // GpiQueryWidthTable(HPS hps, LONG lFirstChar, LONG lCount, PLONG alData) → BOOL
                self.query_width_table(tid, arg(4), arg(8) as i32, arg(12) as i32, arg(16))
            }
            509 => {
                // GpiSetCharAngle(HPS hps, PGRADIENTL pgradlAngle) → BOOL
                let Some(angle) = self.get_pointl(arg(8)) else { return Some(self.text_fail(tid, PMERR_INV_CHAR_ANGLE_ATTR)) };
                // (0, 0) selects the default angle (unverified).
                let angle = if angle == (0, 0) { TextAttrs::default().angle } else { angle };
                self.set_text_attr(tid, arg(4), |a| a.angle = angle)
            }
            429 => {
                // GpiQueryCharAngle(HPS hps, PGRADIENTL pgradlAngle) → BOOL
                self.query_text_point(tid, arg(4), arg(8), |a| a.angle)
            }
            514 => {
                // GpiSetCharShear(HPS hps, PPOINTL pptlShear) → BOOL
                match self.get_pointl(arg(8)) {
                    Some(shear) if shear.1 != 0 => self.set_text_attr(tid, arg(4), |a| a.shear = shear),
                    _ => self.text_fail(tid, PMERR_INV_CHAR_SHEAR_ATTR),
                }
            }
            434 => {
                // GpiQueryCharShear(HPS hps, PPOINTL pptlShear) → BOOL
                self.query_text_point(tid, arg(4), arg(8), |a| a.shear)
            }
            511 => {
                // GpiSetCharDirection(HPS hps, LONG lDirection) → BOOL
                let dir = match arg(8) {
                    CHDIRN_DEFAULT => CHDIRN_LEFTRIGHT,
                    d @ CHDIRN_LEFTRIGHT..=CHDIRN_BOTTOMTOP => d,
                    _ => return Some(self.text_fail(tid, PMERR_INV_CHAR_DIRECTION_ATTR)),
                };
                self.set_text_attr(tid, arg(4), |a| a.direction = dir)
            }
            431 => {
                // GpiQueryCharDirection(HPS hps) → LONG
                self.text_attrs(tid, arg(4)).map_or(CHDIRN_ERROR as u32, |a| a.direction)
            }
            512 => {
                // GpiSetCharMode(HPS hps, LONG lMode) → BOOL
                let mode = match arg(8) {
                    CM_DEFAULT => CM_MODE1,
                    m @ CM_MODE1..=CM_MODE3 => m,
                    _ => return Some(self.text_fail(tid, PMERR_INV_CHAR_MODE_ATTR)),
                };
                self.set_text_attr(tid, arg(4), |a| a.mode = mode)
            }
            432 => {
                // GpiQueryCharMode(HPS hps) → LONG
                self.text_attrs(tid, arg(4)).map_or(CM_ERROR as u32, |a| a.mode)
            }
            614 => {
                // GpiSetCharExtra(HPS hps, FIXED fxExtra) → BOOL
                let extra = arg(8) as i32;
                self.set_text_attr(tid, arg(4), |a| a.char_extra = extra)
            }
            613 => {
                // GpiQueryCharExtra(HPS hps, PFIXED pfxExtra) → BOOL
                self.query_text_long(tid, arg(4), arg(8), |a| a.char_extra)
            }
            616 => {
                // GpiSetCharBreakExtra(HPS hps, FIXED fxBreakExtra) → BOOL
                let extra = arg(8) as i32;
                self.set_text_attr(tid, arg(4), |a| a.break_extra = extra)
            }
            615 => {
                // GpiQueryCharBreakExtra(HPS hps, PFIXED pfxBreakExtra) → BOOL
                self.query_text_long(tid, arg(4), arg(8), |a| a.break_extra)
            }
            649 => {
                // GpiSetTextAlignment(HPS hps, LONG lHoriz, LONG lVert) → BOOL
                let (h, v) = (arg(8), arg(12));
                if !(TA_NORMAL_HORIZ..=TA_STANDARD_HORIZ).contains(&h)
                    || !(TA_NORMAL_VERT..=TA_STANDARD_VERT).contains(&v) || v & 0xFF != 0
                {
                    return Some(self.text_fail(tid, PMERR_INV_CHAR_ALIGN_ATTR));
                }
                self.set_text_attr(tid, arg(4), |a| a.align = (h, v))
            }
            648 => {
                // GpiQueryTextAlignment(HPS hps, PLONG plHoriz, PLONG plVert) → BOOL
                let Some(a) = self.text_attrs(tid, arg(4)) else { return Some(GPI_ERROR) };
                let ok = self.put_long(arg(8), a.align.0 as i32) && self.put_long(arg(12), a.align.1 as i32);
                if ok { GPI_OK } else { self.text_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE) }
            }
            _ => return None,
        })
    }

    /// Record PM error `code` for the thread and return `GPI_ERROR`.
    fn text_fail(&self, tid: u32, code: u32) -> u32 {
        self.pm_error(tid, code);
        GPI_ERROR
    }

    /// A POINTL from a guest pointer; `None` for NULL or outside guest memory.
    fn get_pointl(&self, ptr: u32) -> Option<(i32, i32)> {
        if ptr == 0 { None } else { self.read_pointl(ptr) }
    }

    /// Write a LONG; false for a NULL pointer or one outside guest memory.
    fn put_long(&self, ptr: u32, v: i32) -> bool {
        ptr != 0 && self.guest_write::<i32>(ptr, v).is_some()
    }

    /// Write `pts` as consecutive POINTLs at `ptr`, all or nothing: false
    /// (nothing written) for a NULL pointer or an array not wholly in guest
    /// memory. Nothing to write is always fine.
    fn put_points(&self, ptr: u32, pts: &[(i32, i32)]) -> bool {
        if pts.is_empty() { return true; }
        if ptr == 0 || self.guest_slice_mut(ptr, pts.len() * POINTL_SIZE as usize).is_none() { return false; }
        pts.iter().enumerate().all(|(i, &(x, y))| {
            let at = ptr + i as u32 * POINTL_SIZE;
            self.put_long(at, x) && self.put_long(at + LONG_SIZE, y)
        })
    }

    /// The character attributes of `hps`; `None` (with
    /// `PMERR_INV_HPS`) for an unknown presentation space.
    fn text_attrs(&self, tid: u32, hps: u32) -> Option<TextAttrs> {
        let a = self.shared.window_mgr.lock_or_recover().ps_map.get(&hps).map(|ps| ps.text);
        if a.is_none() { self.pm_error(tid, PMERR_INV_HPS); }
        a
    }

    fn set_text_attr(&self, tid: u32, hps: u32, f: impl FnOnce(&mut TextAttrs)) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(ps) = wm.ps_map.get_mut(&hps) else {
            drop(wm);
            return self.text_fail(tid, PMERR_INV_HPS);
        };
        f(&mut ps.text);
        GPI_OK
    }

    fn query_text_point(&self, tid: u32, hps: u32, ptr: u32, f: impl FnOnce(&TextAttrs) -> (i32, i32)) -> u32 {
        let Some(a) = self.text_attrs(tid, hps) else { return GPI_ERROR };
        let (x, y) = f(&a);
        if self.put_points(ptr, &[(x, y)]) { GPI_OK } else { self.text_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE) }
    }

    fn query_text_long(&self, tid: u32, hps: u32, ptr: u32, f: impl FnOnce(&TextAttrs) -> i32) -> u32 {
        let Some(a) = self.text_attrs(tid, hps) else { return GPI_ERROR };
        if self.put_long(ptr, f(&a)) { GPI_OK } else { self.text_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE) }
    }

    /// The characters of `bytes` in the PS's codepage with their advances,
    /// and the font they are drawn in (its CJK face pinned to the codepage).
    fn text_units(&self, hps: u32, bytes: &[u8]) -> (Vec<Unit>, FontSpec) {
        let cp = self.ps_codepage(hps);
        let font = FontSpec { cjk: CjkRegion::from_codepage(cp), ..self.ps_font(hps) };
        let units = split_chars(bytes, cp).into_iter()
            .map(|(ch, bytes)| Unit { ch, bytes, advance: font::char_advance(ch, font) })
            .collect();
        (units, font)
    }

    /// Read `count` increments (LONGs) at `ptr`; `None` outside guest memory.
    fn read_increments(&self, count: usize, ptr: u32) -> Option<Vec<i32>> {
        if ptr == 0 && count > 0 { return None; }
        (0..count as u32).map(|i| self.guest_read::<i32>(ptr.checked_add(i.checked_mul(LONG_SIZE)?)?)).collect()
    }

    /// GpiCharString / GpiCharStringAt: draw `bytes` at `at` (or the current
    /// position) and move the current position to the concatenation point.
    pub(super) fn char_string(&self, hps: u32, at: Option<(i32, i32)>, bytes: &[u8]) -> u32 {
        self.draw_string(hps, bytes, StringCall { at, rect: None, options: 0, increments: None })
    }

    /// GpiCharStringPos / GpiCharStringPosAt.
    #[allow(clippy::too_many_arguments)]
    fn char_string_pos(&self, tid: u32, hps: u32, at: Option<(i32, i32)>, prcl: u32, options: u32,
                       count: i32, pch: u32, adx: u32) -> u32 {
        if !self.shared.window_mgr.lock_or_recover().ps_map.contains_key(&hps) {
            return self.text_fail(tid, PMERR_INV_HPS);
        }
        if options & !CHS_VALID != 0 { return self.text_fail(tid, PMERR_INV_CHAR_POS_OPTIONS); }
        let Some(bytes) = self.gpi_string_bytes(count, pch) else {
            return self.text_fail(tid, PMERR_INV_LENGTH_OR_COUNT);
        };
        // unverified: a NULL rectangle with CHS_OPAQUE or CHS_CLIP is an error.
        let rect = if options & (CHS_OPAQUE | CHS_CLIP) != 0 {
            match self.read_rectl(prcl) {
                Some(r) => Some((r.x_left, r.y_bottom, r.x_right, r.y_top)),
                None => return self.text_fail(tid, PMERR_INV_RECT),
            }
        } else {
            None
        };
        let increments = if options & CHS_VECTOR != 0 {
            match self.read_increments(bytes.len(), adx) {
                Some(v) => Some(v),
                None => return self.text_fail(tid, PMERR_INV_LENGTH_OR_COUNT),
            }
        } else {
            None
        };
        self.draw_string(hps, &bytes, StringCall { at, rect, options, increments })
    }

    fn draw_string(&self, hps: u32, bytes: &[u8], call: StringCall) -> u32 {
        let Some(t) = self.ps_target(hps) else { return GPI_ERROR };
        let attrs = self.shared.window_mgr.lock_or_recover().ps_map.get(&hps).map(|ps| ps.text).unwrap_or_default();
        let start = call.at.unwrap_or(t.pos);
        let (units, font) = self.text_units(hps, bytes);
        let fb = FontBox::from(font::metrics(font));
        let incs = call.increments.as_ref().map(|v| unit_increments(&units, v));
        let lay = layout(&units, incs.as_deref(), &attrs, fb);
        let frame = Frame::new(attrs.angle);
        let end = if call.options & CHS_LEAVEPOS != 0 { t.pos } else { frame.to_world(start, lay.concat) };
        // In an area or path bracket text is not drawn (outline-font text
        // in paths is not supported); the position still moves.
        if self.in_bracket(hps) {
            self.set_current_pos(hps, end);
            return GPI_OK;
        }
        // The rectangle: exclusive at the right and top (unverified).
        let rect_region = call.rect.map(|(l, b, r, tp)| Region::rect(RRect::new(l.min(r), b.min(tp), l.max(r), b.max(tp))));
        if call.options & CHS_OPAQUE != 0 && let Some((l, b, r, tp)) = call.rect && l != r && b != tp {
            self.draw_ps(&t, DrawOp::Box {
                x1: l.min(r), y1: b.min(tp), x2: l.max(r) - 1, y2: b.max(tp) - 1,
                color: t.back_color, fill: true, mix: crate::gui::surface::Mix::Overpaint,
            });
        }
        let clip: Option<std::sync::Arc<Region>> = match (call.options & CHS_CLIP != 0, rect_region) {
            (true, Some(r)) => Some(std::sync::Arc::new(match t.clip.as_deref() {
                Some(c) => Region::combine(&r, c, SetOp::And).unwrap_or_else(Region::empty),
                None => r,
            })),
            _ => t.clip.clone(),
        };
        let draw = |op: DrawOp| self.draw_clipped(t.target, op, clip.as_deref());
        let plain = plain(&attrs, incs.is_some());
        let world = |p: (f32, f32)| frame.to_world(start, p);
        // The background mix applies to the string's text box.
        if let Some(mix) = t.back_mix && !units.is_empty() {
            if plain {
                let advance = lay.concat.0.round() as i32;
                let (x, y) = start;
                if advance > 0 {
                    draw(DrawOp::Box {
                        x1: x, y1: y - fb.descent as i32, x2: x + advance - 1, y2: y + fb.ascent as i32 - 1,
                        color: t.back_color, fill: true, mix,
                    });
                }
            } else {
                let [tl, bl, tr, br] = lay.corners;
                draw(DrawOp::Shape {
                    points: vec![world(bl), world(br), world(tr), world(tl)], closed: true,
                    fill: Some(AreaFill { pattern: Pattern::solid(t.back_color), mix, back_mix: None }),
                    stroke: None,
                });
            }
        }
        if plain {
            let text: String = units.iter().map(|u| u.ch).collect();
            draw(DrawOp::TextFont { x: start.0, y: start.1, text, color: t.color, font, mix: t.mix });
        } else if !units.is_empty() {
            let glyphs = units.iter().zip(&lay.origins).map(|(u, &o)| {
                let (x, y) = world(o);
                (u.ch, x, y)
            }).collect();
            draw(DrawOp::GlyphRun { glyphs, color: t.color, font, mix: t.mix, matrix: attrs.glyph_matrix() });
        }
        for (flag, which) in [(CHS_UNDERSCORE, Decoration::Underscore), (CHS_STRIKEOUT, Decoration::Strikeout)] {
            if call.options & flag != 0 {
                for band in decoration_bands(&units, &lay, &attrs, font, which) {
                    draw(DrawOp::Shape {
                        points: band.iter().map(|&p| world(p)).collect(), closed: true,
                        fill: Some(AreaFill { pattern: Pattern::solid(t.color), mix: t.mix, back_mix: None }),
                        stroke: None,
                    });
                }
            }
        }
        self.set_current_pos(hps, end);
        GPI_OK
    }

    /// GpiQueryCharStringPos / GpiQueryCharStringPosAt: the world position
    /// of each byte's character (both bytes of a DBCS character get its
    /// origin — unverified) and the concatenation point, `count + 1`
    /// POINTLs. Only `CHS_VECTOR` matters; other `CHS_*` bits are accepted
    /// and ignored (unverified). The current position does not move.
    #[allow(clippy::too_many_arguments)]
    fn query_char_string_pos(&self, tid: u32, hps: u32, at: Option<(i32, i32)>, options: u32, count: i32,
                             pch: u32, adx: u32, aptl: u32) -> u32 {
        let Some(attrs) = self.text_attrs(tid, hps) else { return GPI_ERROR };
        if options & !CHS_VALID != 0 { return self.text_fail(tid, PMERR_INV_CHAR_POS_OPTIONS); }
        let Some(bytes) = self.gpi_string_bytes(count, pch) else {
            return self.text_fail(tid, PMERR_INV_LENGTH_OR_COUNT);
        };
        let increments = if options & CHS_VECTOR != 0 {
            match self.read_increments(bytes.len(), adx) {
                Some(v) => Some(v),
                None => return self.text_fail(tid, PMERR_INV_LENGTH_OR_COUNT),
            }
        } else {
            None
        };
        let start = at.unwrap_or_else(|| {
            self.shared.window_mgr.lock_or_recover().ps_map.get(&hps).map_or((0, 0), |ps| ps.current_pos)
        });
        let (units, font) = self.text_units(hps, &bytes);
        let incs = increments.as_ref().map(|v| unit_increments(&units, v));
        let lay = layout(&units, incs.as_deref(), &attrs, FontBox::from(font::metrics(font)));
        let frame = Frame::new(attrs.angle);
        let points: Vec<(i32, i32)> = units.iter().zip(&lay.origins)
            .flat_map(|(u, &o)| std::iter::repeat_n(frame.to_world(start, o), u.bytes as usize))
            .chain(std::iter::once(frame.to_world(start, lay.concat)))
            .collect();
        if !self.put_points(aptl, &points) { return self.text_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        GPI_OK
    }

    /// GpiQueryTextBox: the text box of the string relative to its start
    /// point, in `TXTBOX_*` order (at most `TXTBOX_COUNT` points), with the
    /// angle, shear, direction, extra spacing and alignment applied
    /// (unverified: the reference does not say whether the alignment
    /// moves the box; Warpine reports the box where the string is drawn).
    pub(super) fn query_text_box(&self, tid: u32, hps: u32, count: i32, pch: u32, npoints: i32, aptl: u32) -> u32 {
        let Some(attrs) = self.text_attrs(tid, hps) else { return GPI_ERROR };
        let Some(bytes) = self.gpi_string_bytes(count, pch) else {
            return self.text_fail(tid, PMERR_INV_LENGTH_OR_COUNT);
        };
        let (units, font) = self.text_units(hps, &bytes);
        let lay = layout(&units, None, &attrs, FontBox::from(font::metrics(font)));
        let frame = Frame::new(attrs.angle);
        let [tl, bl, tr, br] = lay.corners;
        let mut pts = [tl, bl, tr, br, lay.concat].map(|p| frame.to_world((0, 0), p));
        if plain(&attrs, false) {
            // The width exactly as the string advances when drawn.
            let w = font::text_width(&units.iter().map(|u| u.ch).collect::<String>(), font);
            pts[TXTBOX_TOPRIGHT as usize].0 = w;
            pts[TXTBOX_BOTTOMRIGHT as usize].0 = w;
            pts[TXTBOX_CONCAT as usize].0 = w;
        }
        if !self.put_points(aptl, &pts[..npoints.clamp(0, TXTBOX_COUNT as i32) as usize]) {
            return self.text_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        GPI_OK
    }

    /// GpiQueryWidthTable: the advance of `count` code points from `first`
    /// in the PS's codepage and font, each with the character extra (and
    /// the break extra for the break character) — unverified: the
    /// reference does not say whether the extras are included; Warpine
    /// reports what `GpiCharString` advances by. A code point above 0xFF
    /// is a DBCS pair (lead byte high); one that is not a valid pair in the
    /// codepage has width 0 (unverified).
    fn query_width_table(&self, tid: u32, hps: u32, first: i32, count: i32, data: u32) -> u32 {
        let Some(attrs) = self.text_attrs(tid, hps) else { return GPI_ERROR };
        if first < 0 { return self.text_fail(tid, PMERR_INV_FIRST_CHAR); }
        if count < 0 || first as i64 + count as i64 > WIDTH_TABLE_LIMIT {
            return self.text_fail(tid, PMERR_INV_LENGTH_OR_COUNT);
        }
        if count > 0 && (data == 0 || self.guest_slice_mut(data, count as usize * LONG_SIZE as usize).is_none()) {
            return self.text_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        let cp = self.ps_codepage(hps);
        let font = FontSpec { cjk: CjkRegion::from_codepage(cp), ..self.ps_font(hps) };
        let (char_extra, break_extra) = (attrs.char_extra as f32 / FIXED_ONE, attrs.break_extra as f32 / FIXED_ONE);
        for i in 0..count as u32 {
            let code = first as u32 + i;
            let ch = if code <= u8::MAX as u32 {
                split_chars(&[code as u8], cp).first().map(|&(c, _)| c)
            } else {
                let pair = [(code >> 8) as u8, code as u8];
                match split_chars(&pair, cp).as_slice() {
                    [(c, 2)] => Some(*c),
                    _ => None,
                }
            };
            let width = ch.map_or(0, |c| {
                let extra = char_extra + if c == BREAK_CHAR { break_extra } else { 0.0 };
                (font::char_advance(c, font) + extra).round() as i32
            });
            // In guest memory: the whole table was checked above.
            if !self.put_long(data + i * LONG_SIZE, width) {
                return self.text_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
            }
        }
        GPI_OK
    }
}

/// `CHS_UNDERSCORE` or `CHS_STRIKEOUT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decoration { Underscore, Strikeout }

/// Bands (text-frame quadrilaterals) of an underscore or a strikeout: one
/// along a horizontal string, one per character of a vertical one.
/// Position and thickness are Warpine's (unverified: the reference names
/// the options only): an underscore half the descent below the baseline,
/// a strikeout at half the x-height, both 1/16 of the line height thick
/// (at least one pel).
fn decoration_bands(units: &[Unit], lay: &Layout, attrs: &TextAttrs, font: FontSpec, which: Decoration)
                    -> Vec<[(f32, f32); 4]> {
    if units.is_empty() { return Vec::new(); }
    let m = font::metrics(font);
    let thick = (m.line_height as f32 / 16.0).round().max(1.0);
    let bottom = match which {
        Decoration::Underscore => -(m.descent as f32 / 2.0).round().max(1.0),
        Decoration::Strikeout => (m.x_height as f32 / 2.0).round(),
    };
    // Inclusive corners, as the shape rasteriser fills them.
    let band = |u0: f32, u1: f32, base: f32| {
        let (v0, v1) = (base + bottom, base + bottom + thick - 1.0);
        let u1 = (u1 - 1.0).max(u0);
        [(u0, v0), (u1, v0), (u1, v1), (u0, v1)]
    };
    match attrs.dir() {
        Direction::LeftRight | Direction::RightLeft => vec![band(lay.span.0, lay.span.1, lay.origins[0].1)],
        Direction::TopBottom | Direction::BottomTop => units.iter().zip(&lay.origins)
            .map(|(u, &(x, y))| band(x, x + u.advance, y)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FB: FontBox = FontBox { ascent: 12.0, descent: 4.0, line_height: 16.0 };

    fn units(advances: &[f32]) -> Vec<Unit> {
        advances.iter().map(|&a| Unit { ch: 'x', bytes: 1, advance: a }).collect()
    }

    fn attrs(f: impl FnOnce(&mut TextAttrs)) -> TextAttrs {
        let mut a = TextAttrs::default();
        f(&mut a);
        a
    }

    #[test]
    fn left_to_right_steps_by_the_widths() {
        let l = layout(&units(&[5.0, 7.0, 3.0]), None, &TextAttrs::default(), FB);
        assert_eq!(l.origins, [(0.0, 0.0), (5.0, 0.0), (12.0, 0.0)]);
        assert_eq!(l.concat, (15.0, 0.0));
        assert_eq!(l.corners, [(0.0, 12.0), (0.0, -4.0), (15.0, 12.0), (15.0, -4.0)]);
        assert!(plain(&TextAttrs::default(), false));
        assert!(!plain(&TextAttrs::default(), true));
    }

    #[test]
    fn extras_and_increments() {
        let mut u = units(&[5.0, 5.0, 5.0]);
        u[1].ch = BREAK_CHAR;
        let a = attrs(|a| { a.char_extra = 2 << 16; a.break_extra = 3 << 16; });
        let l = layout(&u, None, &a, FB);
        assert_eq!(l.origins, [(0.0, 0.0), (7.0, 0.0), (17.0, 0.0)]);
        assert_eq!(l.concat, (24.0, 0.0));
        // Increments replace widths and extras.
        let l = layout(&u, Some(&[10.0, 20.0, 1.0]), &a, FB);
        assert_eq!(l.origins, [(0.0, 0.0), (10.0, 0.0), (30.0, 0.0)]);
        assert_eq!(l.concat, (31.0, 0.0));
        // A fraction of a pel (FIXED 0.5).
        let l = layout(&units(&[4.0, 4.0]), None, &attrs(|a| a.char_extra = 1 << 15), FB);
        assert_eq!(l.concat, (9.0, 0.0));
    }

    #[test]
    fn right_to_left_places_characters_left_of_the_pen() {
        let a = attrs(|a| a.direction = CHDIRN_RIGHTLEFT);
        let l = layout(&units(&[5.0, 7.0]), None, &a, FB);
        assert_eq!(l.origins, [(-5.0, 0.0), (-12.0, 0.0)]);
        assert_eq!(l.concat, (-12.0, 0.0));
        assert_eq!(l.span, (-12.0, 0.0));
    }

    #[test]
    fn vertical_directions_stack_cells() {
        let tb = layout(&units(&[6.0, 10.0]), None, &attrs(|a| a.direction = CHDIRN_TOPBOTTOM), FB);
        assert_eq!(tb.origins, [(-3.0, -12.0), (-5.0, -28.0)]);
        assert_eq!(tb.concat, (0.0, -32.0));
        assert_eq!(tb.corners, [(-5.0, 0.0), (-5.0, -32.0), (5.0, 0.0), (5.0, -32.0)]);
        let bt = layout(&units(&[6.0, 10.0]), None, &attrs(|a| a.direction = CHDIRN_BOTTOMTOP), FB);
        assert_eq!(bt.origins, [(-3.0, 4.0), (-5.0, 20.0)]);
        assert_eq!(bt.concat, (0.0, 32.0));
    }

    #[test]
    fn alignment_moves_the_reference_point() {
        let u = units(&[10.0, 10.0]);
        let at = |h, v| layout(&u, None, &attrs(|a| a.align = (h, v)), FB);
        assert_eq!(at(TA_CENTER, TA_BASE).origins[0], (-10.0, 0.0));
        assert_eq!(at(TA_RIGHT, TA_BASE).concat, (0.0, 0.0));
        assert_eq!(at(TA_LEFT, TA_TOP).origins[0], (0.0, -12.0));
        assert_eq!(at(TA_LEFT, TA_BOTTOM).origins[0], (0.0, 4.0));
        assert_eq!(at(TA_LEFT, TA_HALF).origins[0], (0.0, -4.0));
        assert_eq!(at(TA_STANDARD_HORIZ, TA_STANDARD_VERT).origins[0], (0.0, 4.0));
        // Normal alignment never moves the start, in any direction.
        for dir in CHDIRN_LEFTRIGHT..=CHDIRN_BOTTOMTOP {
            let a = attrs(|a| a.direction = dir);
            let l = layout(&u, None, &a, FB);
            let c = l.corners;
            let (h, v) = a.alignment();
            assert!(match h { HAlign::Left => c[0].0 == 0.0, HAlign::Right => c[2].0 == 0.0,
                              HAlign::Center => c[0].0 == -c[2].0 }, "dir {dir}");
            assert!(v == VAlign::Base || c[0].1 == 0.0 || c[1].1 == 0.0, "dir {dir}");
        }
    }

    #[test]
    fn angle_frames_and_glyph_matrices() {
        let f = Frame::new((0, 5));
        assert_eq!(f.to_world((10, 10), (4.0, 0.0)), (10, 14), "baseline points up");
        assert_eq!(f.to_world((10, 10), (0.0, 3.0)), (7, 10), "up points left");
        assert_eq!(Frame::new((0, 0)), Frame::new((1, 0)));
        assert_eq!(Frame::new((3, 4)).to_world((0, 0), (5.0, 0.0)), (3, 4));
        assert_eq!(TextAttrs::default().glyph_matrix(), IDENTITY);
        assert_eq!(attrs(|a| a.angle = (0, 1)).glyph_matrix(), [0.0, -1.0, 1.0, 0.0]);
        assert_eq!(attrs(|a| a.angle = (-1, 0)).glyph_matrix(), [-1.0, 0.0, 0.0, -1.0]);
        // Shear (1, 2): the top leans right by half its height.
        assert_eq!(attrs(|a| a.shear = (1, 2)).glyph_matrix(), [1.0, 0.5, 0.0, 1.0]);
        assert_eq!(attrs(|a| a.shear = (0, -1)).glyph_matrix(), [1.0, 0.0, 0.0, -1.0]);
        // A sheared box is a parallelogram.
        let l = layout(&units(&[10.0]), None, &attrs(|a| a.shear = (1, 2)), FB);
        assert_eq!(l.corners, [(6.0, 12.0), (-2.0, -4.0), (16.0, 12.0), (8.0, -4.0)]);
        assert!(!plain(&attrs(|a| a.shear = (1, 2)), false));
    }

    #[test]
    fn dbcs_strings_split_into_pairs() {
        // "日本" in CP932 and an ASCII letter.
        let bytes = [0x93, 0xFA, 0x96, 0x7B, b'A'];
        assert_eq!(split_chars(&bytes, 932), [('日', 2), ('本', 2), ('A', 1)]);
        assert_eq!(split_chars(&[0x93], 932).len(), 1, "a lone lead byte is one character");
        assert_eq!(split_chars(&[0x82, 0x41], 850), [('é', 1), ('A', 1)]);
        let u = [Unit { ch: '日', bytes: 2, advance: 0.0 }, Unit { ch: 'A', bytes: 1, advance: 0.0 }];
        assert_eq!(unit_increments(&u, &[10, 6, 9]), [16.0, 9.0]);
        assert_eq!(unit_increments(&u, &[10]), [10.0, 0.0], "a short array counts as 0");
    }

    #[test]
    fn decorations_follow_the_string() {
        let u = units(&[10.0, 10.0]);
        let a = TextAttrs::default();
        let l = layout(&u, None, &a, FB);
        let bands = decoration_bands(&u, &l, &a, FontSpec::SYSTEM, Decoration::Underscore);
        assert_eq!(bands.len(), 1);
        assert!(bands[0][0].1 < 0.0 && bands[0][0].0 == 0.0 && bands[0][1].0 == 19.0);
        let strike = decoration_bands(&u, &l, &a, FontSpec::SYSTEM, Decoration::Strikeout);
        assert!(strike[0][0].1 > 0.0);
        let tb = attrs(|a| a.direction = CHDIRN_TOPBOTTOM);
        let l = layout(&u, None, &tb, FB);
        assert_eq!(decoration_bands(&u, &l, &tb, FontSpec::SYSTEM, Decoration::Underscore).len(), 2);
        assert!(decoration_bands(&[], &l, &a, FontSpec::SYSTEM, Decoration::Underscore).is_empty());
    }
}

/// The calls through the PMGPI dispatcher, on a mock loader.
#[cfg(test)]
mod api_tests {
    use super::*;
    use super::super::Loader;
    use super::super::pm_region::testing::*;
    use crate::gui::SharedSurface;

    // PMGPI ordinals (doc/os2_ordinals.md).
    const CHAR_STRING_AT: u32 = 359;
    const CHAR_STRING_POS: u32 = 580;
    const CHAR_STRING_POS_AT: u32 = 581;
    const QUERY_POS: u32 = 584;
    const QUERY_POS_AT: u32 = 585;
    const QUERY_TEXT_BOX: u32 = 489;
    const QUERY_WIDTH_TABLE: u32 = 492;
    const SET_ANGLE: u32 = 509;
    const QUERY_ANGLE: u32 = 429;
    const SET_SHEAR: u32 = 514;
    const QUERY_SHEAR: u32 = 434;
    const SET_DIRECTION: u32 = 511;
    const QUERY_DIRECTION: u32 = 431;
    const SET_MODE: u32 = 512;
    const QUERY_MODE: u32 = 432;
    const SET_EXTRA: u32 = 614;
    const QUERY_EXTRA: u32 = 613;
    const SET_BREAK_EXTRA: u32 = 616;
    const QUERY_BREAK_EXTRA: u32 = 615;
    const SET_ALIGN: u32 = 649;
    const QUERY_ALIGN: u32 = 648;
    const SET_BACK_COLOR: u32 = 504;
    const SET_BACK_MIX: u32 = 505;
    const SET_CP: u32 = 518;

    const PT: u32 = 0x6000;
    const TEXT: u32 = 0x6100;
    const OUT: u32 = 0x6200;
    const ADX: u32 = 0x7000;
    const RECT: u32 = 0x7100;
    const BAD: u32 = 0xFFFF_FFF0;

    fn put_text(loader: &Loader, text: &[u8]) {
        for (i, &b) in text.iter().enumerate() {
            loader.guest_write::<u8>(TEXT + i as u32, b).unwrap();
        }
    }

    fn put_longs(loader: &Loader, at: u32, values: &[i32]) {
        for (i, &v) in values.iter().enumerate() {
            loader.guest_write::<i32>(at + 4 * i as u32, v).unwrap();
        }
    }

    fn points(loader: &Loader, at: u32, n: usize) -> Vec<(i32, i32)> {
        (0..n as u32).map(|i| (loader.guest_read::<i32>(at + 8 * i).unwrap(),
                               loader.guest_read::<i32>(at + 8 * i + 4).unwrap())).collect()
    }

    fn surface(loader: &Loader, hwnd: u32) -> SharedSurface {
        let s = loader.shared.window_mgr.lock_or_recover().frame_surface(hwnd).unwrap();
        s.lock().unwrap().record = Some(Vec::new());
        s
    }

    fn recorded(s: &SharedSurface) -> Vec<DrawOp> {
        s.lock().unwrap().record.replace(Vec::new()).unwrap()
    }

    fn current_pos(loader: &Loader, hps: u32) -> (i32, i32) {
        loader.shared.window_mgr.lock_or_recover().ps_map[&hps].current_pos
    }

    /// Pels of `s` that differ from the blank surface, as PM (x, y).
    fn painted(s: &SharedSurface) -> Vec<(i32, i32)> {
        let s = s.lock().unwrap();
        let blank = crate::gui::surface::SURFACE_BLANK & 0x00FF_FFFF;
        let mut out = Vec::new();
        for y in 0..s.height as i32 {
            for x in 0..s.width as i32 {
                if s.pel(x, y) != Some(blank) { out.push((x, y)); }
            }
        }
        out
    }

    fn advance(loader: &Loader, hps: u32, ch: char) -> f32 {
        let cjk = CjkRegion::from_codepage(loader.ps_codepage(hps));
        font::char_advance(ch, FontSpec { cjk, ..loader.ps_font(hps) })
    }

    #[test]
    fn attributes_set_query_and_reject() {
        let (loader, _, hps) = setup();
        // Defaults.
        assert_eq!(gpi(&loader, QUERY_ANGLE, &[hps, OUT]), GPI_OK);
        assert_eq!(points(&loader, OUT, 1), [(1, 0)]);
        assert_eq!(gpi(&loader, QUERY_SHEAR, &[hps, OUT]), GPI_OK);
        assert_eq!(points(&loader, OUT, 1), [(0, 1)]);
        assert_eq!(gpi(&loader, QUERY_DIRECTION, &[hps]), CHDIRN_LEFTRIGHT);
        assert_eq!(gpi(&loader, QUERY_MODE, &[hps]), CM_MODE1);
        assert_eq!(gpi(&loader, QUERY_ALIGN, &[hps, OUT, OUT + 4]), GPI_OK);
        assert_eq!(points(&loader, OUT, 1), [(TA_NORMAL_HORIZ as i32, TA_NORMAL_VERT as i32)]);
        // Round trips.
        put_points(&loader, PT, &[(-3, 7)]);
        assert_eq!(gpi(&loader, SET_ANGLE, &[hps, PT]), GPI_OK);
        assert_eq!(gpi(&loader, QUERY_ANGLE, &[hps, OUT]), GPI_OK);
        assert_eq!(points(&loader, OUT, 1), [(-3, 7)]);
        assert_eq!(gpi(&loader, SET_SHEAR, &[hps, PT]), GPI_OK);
        assert_eq!(gpi(&loader, QUERY_SHEAR, &[hps, OUT]), GPI_OK);
        assert_eq!(points(&loader, OUT, 1), [(-3, 7)]);
        assert_eq!(gpi(&loader, SET_DIRECTION, &[hps, CHDIRN_BOTTOMTOP]), GPI_OK);
        assert_eq!(gpi(&loader, QUERY_DIRECTION, &[hps]), CHDIRN_BOTTOMTOP);
        assert_eq!(gpi(&loader, SET_DIRECTION, &[hps, CHDIRN_DEFAULT]), GPI_OK);
        assert_eq!(gpi(&loader, QUERY_DIRECTION, &[hps]), CHDIRN_LEFTRIGHT);
        assert_eq!(gpi(&loader, SET_MODE, &[hps, CM_MODE3]), GPI_OK);
        assert_eq!(gpi(&loader, QUERY_MODE, &[hps]), CM_MODE3);
        assert_eq!(gpi(&loader, SET_EXTRA, &[hps, 0x0002_8000]), GPI_OK);
        assert_eq!(gpi(&loader, QUERY_EXTRA, &[hps, OUT]), GPI_OK);
        assert_eq!(loader.guest_read::<u32>(OUT).unwrap(), 0x0002_8000);
        assert_eq!(gpi(&loader, SET_BREAK_EXTRA, &[hps, (-(3 << 16)) as u32]), GPI_OK);
        assert_eq!(gpi(&loader, QUERY_BREAK_EXTRA, &[hps, OUT]), GPI_OK);
        assert_eq!(loader.guest_read::<i32>(OUT).unwrap(), -(3 << 16));
        assert_eq!(gpi(&loader, SET_ALIGN, &[hps, TA_CENTER, TA_HALF]), GPI_OK);
        assert_eq!(gpi(&loader, QUERY_ALIGN, &[hps, OUT, OUT + 4]), GPI_OK);
        assert_eq!(points(&loader, OUT, 1), [(TA_CENTER as i32, TA_HALF as i32)]);
        // Errors.
        put_points(&loader, PT, &[(1, 0)]);
        assert_eq!(gpi(&loader, SET_SHEAR, &[hps, PT]), GPI_ERROR, "a horizontal shear");
        assert_eq!(last_error(&loader), PMERR_INV_CHAR_SHEAR_ATTR);
        assert_eq!(gpi(&loader, SET_DIRECTION, &[hps, 5]), GPI_ERROR);
        assert_eq!(last_error(&loader), PMERR_INV_CHAR_DIRECTION_ATTR);
        assert_eq!(gpi(&loader, SET_MODE, &[hps, 4]), GPI_ERROR);
        assert_eq!(last_error(&loader), PMERR_INV_CHAR_MODE_ATTR);
        for (h, v) in [(0, TA_BASE), (6, TA_BASE), (TA_LEFT, 0x0700), (TA_LEFT, 0x0201)] {
            assert_eq!(gpi(&loader, SET_ALIGN, &[hps, h, v]), GPI_ERROR, "{h:#x} {v:#x}");
            assert_eq!(last_error(&loader), PMERR_INV_CHAR_ALIGN_ATTR);
        }
        assert_eq!(gpi(&loader, SET_ANGLE, &[hps, BAD]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_ANGLE, &[hps, BAD]), GPI_ERROR);
        // NULL pointers are refused, never read or written at address 0.
        loader.guest_write::<u32>(0, 0xA5A5_A5A5).unwrap();
        assert_eq!(gpi(&loader, SET_ANGLE, &[hps, 0]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_ANGLE, &[hps, 0]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_EXTRA, &[hps, 0]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_ALIGN, &[hps, 0, OUT]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_TEXT_BOX, &[hps, 0, TEXT, TXTBOX_COUNT, 0]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_POS, &[hps, 0, 0, TEXT, 0, 0]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_WIDTH_TABLE, &[hps, 0x41, 1, 0]), GPI_ERROR);
        assert_eq!(gpi(&loader, CHAR_STRING_POS, &[hps, 0, CHS_VECTOR, 1, TEXT, 0]), GPI_ERROR);
        assert_eq!(loader.guest_read::<u32>(0).unwrap(), 0xA5A5_A5A5);
        assert_eq!(gpi(&loader, QUERY_DIRECTION, &[0xDEAD]), CHDIRN_ERROR as u32);
        assert_eq!(last_error(&loader), PMERR_INV_HPS);
        assert_eq!(gpi(&loader, QUERY_MODE, &[0xDEAD]), CM_ERROR as u32);
        assert_eq!(gpi(&loader, SET_EXTRA, &[0xDEAD, 0]), GPI_ERROR);
        // (0, 0) restores the default angle.
        put_points(&loader, PT, &[(0, 0)]);
        assert_eq!(gpi(&loader, SET_ANGLE, &[hps, PT]), GPI_OK);
        gpi(&loader, QUERY_ANGLE, &[hps, OUT]);
        assert_eq!(points(&loader, OUT, 1), [(1, 0)]);
    }

    /// Default attributes keep the one-op upright path; anything else is a
    /// glyph run at the laid-out positions.
    #[test]
    fn char_string_paths_and_current_position() {
        let (loader, hwnd, hps) = setup();
        let s = surface(&loader, hwnd);
        put_text(&loader, b"AB");
        put_points(&loader, PT, &[(10, 20)]);
        assert_eq!(gpi(&loader, CHAR_STRING_AT, &[hps, PT, 2, TEXT]), GPI_OK);
        assert!(matches!(recorded(&s).as_slice(), [DrawOp::TextFont { x: 10, y: 20, .. }]));
        let w = font::text_width("AB", loader.ps_font(hps));
        assert_eq!(current_pos(&loader, hps), (10 + w, 20));

        // Character extra 3: a glyph run, B three pels further on.
        gpi(&loader, SET_EXTRA, &[hps, 3 << 16]);
        gpi(&loader, CHAR_STRING_AT, &[hps, PT, 2, TEXT]);
        let a = advance(&loader, hps, 'A');
        let ops = recorded(&s);
        let [DrawOp::GlyphRun { glyphs, matrix, .. }] = ops.as_slice() else { panic!("{ops:?}") };
        assert_eq!(glyphs, &[('A', 10, 20), ('B', 10 + (a + 3.0).round() as i32, 20)]);
        assert_eq!(*matrix, IDENTITY);
        let b = advance(&loader, hps, 'B');
        assert_eq!(current_pos(&loader, hps), (10 + (a + b + 6.0).round() as i32, 20));
    }

    #[test]
    fn char_string_pos_vector_leavepos_and_errors() {
        let (loader, hwnd, hps) = setup();
        let s = surface(&loader, hwnd);
        put_text(&loader, b"abc");
        put_longs(&loader, ADX, &[20, 30, 5]);
        put_points(&loader, PT, &[(5, 40)]);
        assert_eq!(gpi(&loader, CHAR_STRING_POS_AT, &[hps, PT, 0, CHS_VECTOR, 3, TEXT, ADX]), GPI_OK);
        let ops = recorded(&s);
        let [DrawOp::GlyphRun { glyphs, .. }] = ops.as_slice() else { panic!("{ops:?}") };
        assert_eq!(glyphs, &[('a', 5, 40), ('b', 25, 40), ('c', 55, 40)]);
        assert_eq!(current_pos(&loader, hps), (60, 40));
        // From the current position, leaving it where it was.
        assert_eq!(gpi(&loader, CHAR_STRING_POS, &[hps, 0, CHS_VECTOR | CHS_LEAVEPOS, 3, TEXT, ADX]), GPI_OK);
        assert_eq!(current_pos(&loader, hps), (60, 40));
        let ops = recorded(&s);
        assert!(matches!(&ops[..], [DrawOp::GlyphRun { glyphs, .. }] if glyphs[2] == ('c', 110, 40)));
        // Errors: unknown option bit, NULL rectangle with CHS_OPAQUE,
        // increments outside guest memory, bad count, bad PS.
        assert_eq!(gpi(&loader, CHAR_STRING_POS, &[hps, 0, 0x0004, 3, TEXT, 0]), GPI_ERROR);
        assert_eq!(last_error(&loader), PMERR_INV_CHAR_POS_OPTIONS);
        assert_eq!(gpi(&loader, CHAR_STRING_POS, &[hps, 0, CHS_OPAQUE, 3, TEXT, 0]), GPI_ERROR);
        assert_eq!(last_error(&loader), PMERR_INV_RECT);
        assert_eq!(gpi(&loader, CHAR_STRING_POS, &[hps, 0, CHS_VECTOR, 3, TEXT, BAD]), GPI_ERROR);
        assert_eq!(last_error(&loader), PMERR_INV_LENGTH_OR_COUNT);
        assert_eq!(gpi(&loader, CHAR_STRING_POS, &[hps, 0, 0, (-1i32) as u32, TEXT, 0]), GPI_ERROR);
        assert_eq!(gpi(&loader, CHAR_STRING_POS, &[0xDEAD, 0, 0, 3, TEXT, 0]), GPI_ERROR);
        assert_eq!(last_error(&loader), PMERR_INV_HPS);
        assert!(recorded(&s).is_empty(), "failed calls draw nothing");
        assert_eq!(current_pos(&loader, hps), (60, 40));
    }

    /// CHS_OPAQUE fills the rectangle with the background colour; CHS_CLIP
    /// keeps the text inside it.
    #[test]
    fn opaque_and_clip_rectangles() {
        let (loader, hwnd, hps) = setup();
        let s = surface(&loader, hwnd);
        gpi(&loader, SET_BACK_COLOR, &[hps, CLR_BLUE]);
        put_text(&loader, b"WWWW");
        put_rects(&loader, RECT, &[(10, 10, 30, 40)]);
        put_points(&loader, PT, &[(12, 20)]);
        assert_eq!(gpi(&loader, CHAR_STRING_POS_AT, &[hps, PT, RECT, CHS_OPAQUE | CHS_CLIP, 4, TEXT, 0]), GPI_OK);
        assert_eq!(pel(&loader, hps, 10, 10), CLR_BLUE);
        assert_eq!(pel(&loader, hps, 29, 39), CLR_BLUE);
        assert_ne!(pel(&loader, hps, 30, 39), CLR_BLUE, "exclusive at the right");
        let pels = painted(&s);
        assert!(pels.iter().all(|&(x, y)| (10..30).contains(&x) && (10..40).contains(&y)), "clipped to the rectangle");
        // Unclipped, the four W reach past x = 30.
        put_points(&loader, PT, &[(12, 60)]);
        gpi(&loader, CHAR_STRING_POS_AT, &[hps, PT, 0, 0, 4, TEXT, 0]);
        assert!(painted(&s).iter().any(|&(x, y)| x > 30 && y > 50));
    }

    #[test]
    fn query_char_string_pos_reports_each_byte() {
        let (loader, _, hps) = setup();
        put_text(&loader, b"ab");
        put_points(&loader, PT, &[(7, 9)]);
        assert_eq!(gpi(&loader, QUERY_POS_AT, &[hps, PT, 0, 2, TEXT, 0, OUT]), GPI_OK);
        let (a, b) = (advance(&loader, hps, 'a'), advance(&loader, hps, 'b'));
        assert_eq!(points(&loader, OUT, 3), [(7, 9), (7 + a.round() as i32, 9), (7 + (a + b).round() as i32, 9)]);
        assert_eq!(current_pos(&loader, hps), (0, 0), "the current position does not move");
        // From the current position, with increments.
        move_to(&loader, hps, (100, 50));
        put_longs(&loader, ADX, &[11, 4]);
        assert_eq!(gpi(&loader, QUERY_POS, &[hps, CHS_VECTOR, 2, TEXT, ADX, OUT]), GPI_OK);
        assert_eq!(points(&loader, OUT, 3), [(100, 50), (111, 50), (115, 50)]);
        // Right to left.
        gpi(&loader, SET_DIRECTION, &[hps, CHDIRN_RIGHTLEFT]);
        gpi(&loader, QUERY_POS, &[hps, CHS_VECTOR, 2, TEXT, ADX, OUT]);
        assert_eq!(points(&loader, OUT, 3), [(89, 50), (85, 50), (85, 50)]);
        // A DBCS character gives both of its bytes its origin.
        assert_eq!(gpi(&loader, SET_CP, &[hps, 932]), 1);
        gpi(&loader, SET_DIRECTION, &[hps, CHDIRN_LEFTRIGHT]);
        put_text(&loader, &[0x93, 0xFA, b'A']);
        put_longs(&loader, ADX, &[10, 6, 9]);
        gpi(&loader, QUERY_POS, &[hps, CHS_VECTOR, 3, TEXT, ADX, OUT]);
        assert_eq!(points(&loader, OUT, 4), [(100, 50), (100, 50), (116, 50), (125, 50)]);
        // Errors: unwritable output, bad PS.
        assert_eq!(gpi(&loader, QUERY_POS, &[hps, 0, 3, TEXT, 0, BAD]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_POS, &[0xDEAD, 0, 3, TEXT, 0, OUT]), GPI_ERROR);
    }

    /// GpiQueryTextBox follows the angle and alignment (the corners in
    /// TXTBOX_* order).
    #[test]
    fn text_box_follows_the_attributes() {
        let (loader, _, hps) = setup();
        let m = font::metrics(loader.ps_font(hps));
        put_text(&loader, b"Hi");
        let w = font::text_width("Hi", loader.ps_font(hps));
        assert_eq!(gpi(&loader, QUERY_TEXT_BOX, &[hps, 2, TEXT, TXTBOX_COUNT, OUT]), GPI_OK);
        assert_eq!(points(&loader, OUT, 5), [(0, m.ascent), (0, -m.descent), (w, m.ascent), (w, -m.descent), (w, 0)]);
        // A quarter turn: the baseline runs up, the top of the text faces left.
        put_points(&loader, PT, &[(0, 1)]);
        gpi(&loader, SET_ANGLE, &[hps, PT]);
        gpi(&loader, QUERY_TEXT_BOX, &[hps, 2, TEXT, TXTBOX_COUNT, OUT]);
        let pts = points(&loader, OUT, 5);
        assert_eq!(pts[TXTBOX_TOPLEFT as usize], (-m.ascent, 0));
        assert_eq!(pts[TXTBOX_BOTTOMRIGHT as usize], (m.descent, w));
        assert_eq!(pts[TXTBOX_CONCAT as usize], (0, w));
        put_points(&loader, PT, &[(1, 0)]);
        gpi(&loader, SET_ANGLE, &[hps, PT]);
        // Centred: the box straddles the start point.
        gpi(&loader, SET_ALIGN, &[hps, TA_CENTER, TA_BASE]);
        gpi(&loader, QUERY_TEXT_BOX, &[hps, 2, TEXT, TXTBOX_COUNT, OUT]);
        let pts = points(&loader, OUT, 5);
        assert!((pts[0].0 + pts[2].0).abs() <= 1, "{pts:?}");
        // Fewer points requested: only those are written.
        loader.guest_write::<i32>(OUT + 16, 12345).unwrap();
        gpi(&loader, QUERY_TEXT_BOX, &[hps, 2, TEXT, 2, OUT]);
        assert_eq!(loader.guest_read::<i32>(OUT + 16).unwrap(), 12345);
        assert_eq!(gpi(&loader, QUERY_TEXT_BOX, &[hps, 2, TEXT, TXTBOX_COUNT, BAD]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_TEXT_BOX, &[hps, (-1i32) as u32, TEXT, TXTBOX_COUNT, OUT]), GPI_ERROR);
    }

    #[test]
    fn width_table_in_the_ps_codepage() {
        let (loader, _, hps) = setup();
        assert_eq!(gpi(&loader, QUERY_WIDTH_TABLE, &[hps, 0x41, 3, OUT]), GPI_OK);
        let widths: Vec<i32> = (0..3).map(|i| loader.guest_read::<i32>(OUT + 4 * i).unwrap()).collect();
        let want: Vec<i32> = ['A', 'B', 'C'].iter().map(|&c| advance(&loader, hps, c).round() as i32).collect();
        assert_eq!(widths, want);
        assert!(widths.iter().all(|&w| w > 0));
        // Extras are included; the break extra only for the space.
        gpi(&loader, SET_EXTRA, &[hps, 2 << 16]);
        gpi(&loader, SET_BREAK_EXTRA, &[hps, 5 << 16]);
        gpi(&loader, QUERY_WIDTH_TABLE, &[hps, 0x20, 2, OUT]);
        assert_eq!(loader.guest_read::<i32>(OUT).unwrap(), (advance(&loader, hps, ' ') + 7.0).round() as i32);
        assert_eq!(loader.guest_read::<i32>(OUT + 4).unwrap(), (advance(&loader, hps, '!') + 2.0).round() as i32);
        // A DBCS pair in CP932: 0x93FA is 日; 0x0141 is no pair.
        gpi(&loader, SET_EXTRA, &[hps, 0]);
        gpi(&loader, SET_CP, &[hps, 932]);
        assert_eq!(gpi(&loader, QUERY_WIDTH_TABLE, &[hps, 0x93FA, 1, OUT]), GPI_OK);
        assert_eq!(loader.guest_read::<i32>(OUT).unwrap(), advance(&loader, hps, '日').round() as i32);
        gpi(&loader, QUERY_WIDTH_TABLE, &[hps, 0x141, 1, OUT]);
        assert_eq!(loader.guest_read::<i32>(OUT).unwrap(), 0);
        // Bounds.
        assert_eq!(gpi(&loader, QUERY_WIDTH_TABLE, &[hps, (-1i32) as u32, 1, OUT]), GPI_ERROR);
        assert_eq!(last_error(&loader), PMERR_INV_FIRST_CHAR);
        assert_eq!(gpi(&loader, QUERY_WIDTH_TABLE, &[hps, 0xFFFF, 2, OUT]), GPI_ERROR);
        assert_eq!(last_error(&loader), PMERR_INV_LENGTH_OR_COUNT);
        assert_eq!(gpi(&loader, QUERY_WIDTH_TABLE, &[hps, 0, (-1i32) as u32, OUT]), GPI_ERROR);
        assert_eq!(gpi(&loader, QUERY_WIDTH_TABLE, &[hps, 0, 256, BAD]), GPI_ERROR);
        assert_eq!(last_error(&loader), PMERR_PARAMETER_OUT_OF_RANGE);
        assert_eq!(gpi(&loader, QUERY_WIDTH_TABLE, &[hps, 0, 0, BAD]), GPI_OK, "nothing to write");
    }

    /// Text at a quarter turn is drawn rotated: a tall column above and to
    /// the left of the start point, not a row to its right.
    #[test]
    fn rotated_text_is_drawn_rotated() {
        let (loader, hwnd, hps) = setup();
        let s = surface(&loader, hwnd);
        put_points(&loader, PT, &[(0, 1)]);
        gpi(&loader, SET_ANGLE, &[hps, PT]);
        put_text(&loader, b"HHHH");
        put_points(&loader, PT, &[(100, 10)]);
        assert_eq!(gpi(&loader, CHAR_STRING_AT, &[hps, PT, 4, TEXT]), GPI_OK);
        let ops = recorded(&s);
        assert!(matches!(&ops[..], [DrawOp::GlyphRun { matrix, .. }] if *matrix == [0.0, -1.0, 1.0, 0.0]), "{ops:?}");
        let pels = painted(&s);
        let (x0, x1) = (pels.iter().map(|p| p.0).min().unwrap(), pels.iter().map(|p| p.0).max().unwrap());
        let (y0, y1) = (pels.iter().map(|p| p.1).min().unwrap(), pels.iter().map(|p| p.1).max().unwrap());
        assert!(y1 - y0 > 2 * (x1 - x0), "a column: x {x0}..{x1}, y {y0}..{y1}");
        assert!(x1 <= 100 && x0 >= 100 - 16, "left of the start: {x0}..{x1}");
        assert!(y0 >= 9, "above the start: {y0}");
        let w = font::text_width("HHHH", loader.ps_font(hps));
        assert_eq!(current_pos(&loader, hps), (100, 10 + w), "the position moves up");
    }

    /// A background mix under transformed text fills the text box as a
    /// shape; underscore and strikeout are bands in the text colour.
    #[test]
    fn background_box_and_decorations() {
        let (loader, hwnd, hps) = setup();
        let s = surface(&loader, hwnd);
        gpi(&loader, SET_BACK_MIX, &[hps, FM_OVERPAINT]);
        put_points(&loader, PT, &[(1, 2)]);
        gpi(&loader, SET_SHEAR, &[hps, PT]);
        put_text(&loader, b"ab");
        put_points(&loader, PT, &[(20, 40)]);
        gpi(&loader, CHAR_STRING_POS_AT, &[hps, PT, 0, CHS_UNDERSCORE | CHS_STRIKEOUT, 2, TEXT, 0]);
        let ops = recorded(&s);
        assert_eq!(ops.len(), 4, "{ops:?}");
        assert!(matches!(&ops[0], DrawOp::Shape { closed: true, fill: Some(_), .. }), "background box");
        assert!(matches!(&ops[1], DrawOp::GlyphRun { matrix, .. } if *matrix == [1.0, 0.5, 0.0, 1.0]));
        assert!(matches!(&ops[2], DrawOp::Shape { points, .. } if points.iter().all(|p| p.1 < 40)), "underscore");
        assert!(matches!(&ops[3], DrawOp::Shape { points, .. } if points.iter().all(|p| p.1 > 40)), "strikeout");
    }
}
