// SPDX-License-Identifier: GPL-3.0-or-later

//! The linear slider control (`WC_SLIDER`, `<pmstddlg.h>`): a shaft along
//! which an arm moves between a home position and the far end, two scales
//! of increments (`SLDCDATA`: increments and spacing of scale 1 and 2), tick
//! marks and texts on the primary scale, detents, optional buttons, a
//! ribbon strip — PM Reference "Slider Control Window Processing", PM Guide
//! "Slider Controls".
//!
//! Positions are pixels from the home position (`SLS_HOMELEFT` /
//! `SLS_HOMEBOTTOM`, or `SLS_HOMERIGHT` / `SLS_HOMETOP`); increment `i` of
//! the primary scale (`SLS_PRIMARYSCALE1` / `2`) lies `i × spacing` pixels
//! from home, the spacing of a scale given as 0 dividing the shaft evenly
//! ("the length of the shaft is always determined by the number of
//! increments and the spacing between increments": with a spacing the shaft
//! is as long as the increments need, without one it fills the window).
//! Scale 1 lies above a horizontal shaft and right of a vertical one, scale
//! 2 below / left (PM Guide); ticks (`SLM_SETTICKSIZE`, none until set) and
//! texts (`SLM_SETSCALETEXT`) are drawn on the primary scale.
//!
//! Messages: `SLM_SETSLIDERINFO` / `SLM_QUERYSLIDERINFO` (`SMA_*`: shaft
//! dimensions — only the breadth can be set — and position, arm dimensions,
//! arm position as pixels or as an increment), `SLM_SETTICKSIZE` /
//! `SLM_QUERYTICKSIZE` / `SLM_QUERYTICKPOS`, `SLM_SETSCALETEXT` /
//! `SLM_QUERYSCALETEXT`, `SLM_ADDDETENT` / `SLM_REMOVEDETENT` /
//! `SLM_QUERYDETENTPOS`. Notifications (`WM_CONTROL` to the owner):
//! `SLN_CHANGE` and `SLN_SLIDERTRACK` with the arm position in pixels from
//! home, `SLN_SETFOCUS` / `SLN_KILLFOCUS` with the slider (PM Reference,
//! "WM_CONTROL (in Slider Controls)").
//!
//! Input (PM Guide): dragging the arm sends `SLN_SLIDERTRACK` while it moves
//! and `SLN_CHANGE` when it is released (on the nearest increment with
//! `SLS_SNAPTOINCREMENT`, else where it was let go); a click on the shaft
//! moves the arm one increment toward the pointer, a click on a button one
//! increment in its direction, both repeating on `WM_TIMER(TID_SCROLL)`
//! while held; a click on a detent moves the arm to it. Left / Right
//! (horizontal) or Down / Up (vertical) move one increment, with Shift to
//! the next detent; Home and End go to the lowest and highest value. A
//! `SLS_READONLY` slider takes no input, has a narrow arm and no buttons.
//! The focus is a black square in the middle of the arm.
//!
//! Unverified (the documents give no figures): every size below (shaft
//! breadth, arm, buttons, gaps), the colours, detents drawn on the side
//! opposite the primary scale, `SLN_CHANGE` also for `SLM_SETSLIDERINFO`,
//! button 2 (drag to the pointer) not supported.
//!
//! `SLS_OWNERDRAW`: the slider paints itself, then asks its owner to draw
//! the background, shaft, ribbon strip and arm (`WM_DRAWITEM`, `idItem` =
//! `SDA_*`, `rclItem` = the part in slider coordinates) through
//! `pm_owner.rs`; TRUE keeps what the owner drew, FALSE has the slider draw
//! the part.

use super::constants::*;
use super::pm_types::WindowManager;
use super::{control_box, control_text, ApiResult, Loader, MutexExt, Painter, CONTROL_BORDER_COLOR,
            CONTROL_FACE_COLOR, CONTROL_FIELD_COLOR, CONTROL_LINE_H, CONTROL_PRESSED_COLOR,
            CONTROL_TEXT_COLOR};
use crate::gui::font;

pub(super) const CLASS_SLIDER: &str = "WC_SLIDER";

/// Breadth of the shaft until `SLM_SETSLIDERINFO` sets one (unverified).
const DEFAULT_SHAFT_BREADTH: i32 = 8;
/// Length and breadth of the arm until set (unverified).
const DEFAULT_ARM_LENGTH: i32 = 10;
const DEFAULT_ARM_BREADTH: i32 = 20;
/// The narrow arm of a read-only slider (unverified).
const READONLY_ARM_LENGTH: i32 = 4;
/// Side of each square slider button (unverified).
const BUTTON_SIZE: i32 = 16;
/// Gap between the buttons and the shaft, and the shaft's ends and the
/// window's edges (unverified).
const BUTTON_GAP: i32 = 2;
const EDGE_GAP: i32 = 2;
/// Gap between the arm's side and the tick marks / detent marks, and
/// between a tick and its text (unverified).
const TICK_GAP: i32 = 1;
const TEXT_GAP: i32 = 1;
/// Length of a detent mark across the shaft, and how far beside it a click
/// still hits it (unverified).
const DETENT_MARK_LEN: i32 = 6;
const DETENT_HIT: i32 = 3;
/// Side of the focus square on the arm.
const FOCUS_MARK: i32 = 4;
/// Ribbon strip colour (unverified).
const RIBBON_COLOR: u32 = 0x0000_0080;
/// Most increments a scale keeps (Warpine's bound on hostile control data).
pub(super) const MAX_INCREMENTS: u16 = 1024;
/// Longest range in pixels (positions are USHORTs, notifications SHORTs).
const MAX_RANGE: i32 = 32767;
/// Longest tick mark (Warpine's bound).
const MAX_TICK_LEN: u32 = 64;
/// Largest arm or shaft dimension `SLM_SETSLIDERINFO` takes (Warpine's bound).
const MAX_PART_DIM: u32 = 1024;
/// Most detents at once (Warpine's bound).
pub(super) const MAX_DETENTS: usize = 256;
/// Longest scale text kept, in bytes.
const MAX_SCALE_TEXT: usize = 256;

/// One scale of `SLDCDATA`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scale {
    pub increments: u16,
    /// Pixels between increments; 0 = the shaft is divided evenly.
    pub spacing: u16,
}

/// A direction along the shaft: towards its low end (left / bottom) or
/// its high end (right / top).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Low,
    High,
}

/// What is held with button 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// The arm, grabbed `grab` pels from its low end; it was at `start` (px).
    Arm { grab: i32, start: i32 },
    /// The shaft beside the arm: repeats towards `dir` while the pointer
    /// stays beyond the arm.
    Shaft(Dir),
    /// A button; `over` while the pointer is on it (drawn pressed).
    Button { dir: Dir, over: bool },
}

/// A slider's scales, arm and decorations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SliderState {
    pub scales: [Scale; 2],
    /// Arm position: pixels from home.
    pub arm: i32,
    pub shaft_breadth: i32,
    /// `SMA_SHAFTPOSITION`: the shaft's lower-left corner; `None` = laid out.
    pub shaft_pos: Option<(i32, i32)>,
    /// `SMA_SLIDERARMDIMENSIONS`: (length, breadth); `None` = the default.
    pub arm_size: Option<(i32, i32)>,
    /// Tick lengths of the primary scale's increments.
    pub ticks: Vec<u16>,
    /// Texts of the primary scale's increments.
    pub texts: Vec<String>,
    /// Detents: (id, pixels from home).
    pub detents: Vec<(u32, i32)>,
    pub next_detent: u32,
    pub held: Option<Held>,
}

/// `(scale1, scale2)` from `SLDCDATA` bytes; fields past the end of `ctl`
/// (or its `cbSize`) are 0. Increments are bounded by [`MAX_INCREMENTS`].
pub fn parse_sldcdata(ctl: &[u8]) -> [Scale; 2] {
    let cb = ctl.get(..4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let ctl = &ctl[..cb.min(ctl.len())];
    let w = |off: usize| ctl.get(off..off + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let scale = |inc, sp| Scale { increments: w(inc).min(MAX_INCREMENTS), spacing: w(sp) };
    [scale(SLDCDATA_SCALE1_INCREMENTS, SLDCDATA_SCALE1_SPACING),
     scale(SLDCDATA_SCALE2_INCREMENTS, SLDCDATA_SCALE2_SPACING)]
}

impl SliderState {
    /// A slider with `scales`, its arm at home.
    pub fn new(scales: [Scale; 2], style: u32) -> SliderState {
        let primary = scales[primary_index(style)].increments.max(1) as usize;
        SliderState {
            scales,
            shaft_breadth: DEFAULT_SHAFT_BREADTH,
            ticks: vec![0; primary],
            texts: vec![String::new(); primary],
            next_detent: 1,
            ..Default::default()
        }
    }
}

/// Index into `scales` of the primary scale of `style`.
fn primary_index(style: u32) -> usize {
    (style & SLS_PRIMARYSCALE2 != 0) as usize
}

/// Where the parts of a slider lie. `t` runs along the shaft from its low
/// end (x of a horizontal slider, y of a vertical one), `c` across it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub vertical: bool,
    pub home_high: bool,
    /// Shaft: start along, length, start across, breadth.
    pub shaft_t: i32,
    pub shaft_len: i32,
    pub shaft_c: i32,
    pub shaft_b: i32,
    /// Middle of the slider across the shaft.
    pub mid_c: i32,
    pub arm_len: i32,
    pub arm_b: i32,
    /// Pixels the arm can travel: positions run 0..=range.
    pub range: i32,
    /// Increments of the primary scale (at least 1) and their spacing (0 =
    /// even division).
    pub increments: i32,
    pub spacing: i32,
    /// The primary scale lies on the high side (above / right).
    pub scale_high: bool,
    /// Start along of the low button (the high one follows it).
    pub buttons_t: Option<i32>,
}

impl Geometry {
    /// The layout of slider `s` of `style`, `cx` × `cy`.
    pub fn new(s: &SliderState, style: u32, cx: i32, cy: i32) -> Geometry {
        let vertical = style & SLS_VERTICAL != 0;
        let read_only = style & SLS_READONLY != 0;
        let (len, breadth) = if vertical { (cy.max(0), cx.max(0)) } else { (cx.max(0), cy.max(0)) };
        let (arm_len, arm_b) = s.arm_size.unwrap_or((
            if read_only { READONLY_ARM_LENGTH } else { DEFAULT_ARM_LENGTH },
            DEFAULT_ARM_BREADTH.max(s.shaft_breadth),
        ));
        let buttons = if read_only { 0 } else { style & (SLS_BUTTONSLEFT | SLS_BUTTONSRIGHT) };
        let (low_buttons, high_buttons) = (buttons == SLS_BUTTONSLEFT, buttons == SLS_BUTTONSRIGHT);
        let button_room = 2 * BUTTON_SIZE + BUTTON_GAP;
        let start = EDGE_GAP + if low_buttons { button_room } else { 0 };
        let end = len - EDGE_GAP - if high_buttons { button_room } else { 0 };
        let scale = s.scales[primary_index(style)];
        let increments = scale.increments.max(1) as i32;
        let spacing = scale.spacing as i32;
        let range = if spacing > 0 && increments > 1 {
            ((increments - 1) as i64 * spacing as i64).min(MAX_RANGE as i64) as i32
        } else {
            (end - start - arm_len).clamp(0, MAX_RANGE)
        };
        let shaft_len = range + arm_len;
        let shaft_b = s.shaft_breadth;
        let (shaft_t, mid_c) = match s.shaft_pos {
            Some((x, y)) => {
                let (t, c) = if vertical { (y, x) } else { (x, y) };
                (t, c + shaft_b / 2)
            }
            None => (start, match style & (SLS_TOP | SLS_BOTTOM) {
                // SLS_TOP = SLS_RIGHT: the high side; SLS_BOTTOM = SLS_LEFT.
                SLS_TOP => breadth - (arm_b + 1) / 2,
                SLS_BOTTOM => arm_b / 2,
                _ => breadth / 2,
            }),
        };
        let buttons_t = if low_buttons {
            Some(shaft_t - BUTTON_GAP - 2 * BUTTON_SIZE)
        } else if high_buttons {
            Some(shaft_t + shaft_len + BUTTON_GAP)
        } else {
            None
        };
        Geometry {
            vertical, home_high: style & SLS_HOMERIGHT != 0,
            shaft_t, shaft_len, shaft_c: mid_c - shaft_b / 2, shaft_b, mid_c, arm_len, arm_b,
            range, increments, spacing, scale_high: primary_index(style) == 0, buttons_t,
        }
    }

    /// `(x, y, w, h)` in slider coordinates of the span `t0..t0+tl` along,
    /// `c0..c0+cl` across.
    pub fn rect(&self, t0: i32, tl: i32, c0: i32, cl: i32) -> (i32, i32, i32, i32) {
        if self.vertical { (c0, t0, cl, tl) } else { (t0, c0, tl, cl) }
    }

    /// The point `(t, c)` in slider coordinates.
    pub fn point(&self, t: i32, c: i32) -> (i32, i32) {
        if self.vertical { (c, t) } else { (t, c) }
    }

    /// `(t, c)` of the point `(x, y)` in slider coordinates.
    pub fn along(&self, (x, y): (i32, i32)) -> (i32, i32) {
        if self.vertical { (y, x) } else { (x, y) }
    }

    /// Where along the shaft position `px` (pixels from home) puts the
    /// arm's low end.
    pub fn arm_t(&self, px: i32) -> i32 {
        let px = px.clamp(0, self.range);
        self.shaft_t + if self.home_high { self.range - px } else { px }
    }

    /// The position (pixels from home) of an arm whose low end is at `t`.
    pub fn px_at(&self, t: i32) -> i32 {
        let off = (t - self.shaft_t).clamp(0, self.range);
        if self.home_high { self.range - off } else { off }
    }

    /// Pixels from home of increment `i` (moved into the scale).
    pub fn increment_px(&self, i: i32) -> i32 {
        let i = i.clamp(0, self.increments - 1) as i64;
        if self.increments <= 1 {
            0
        } else if self.spacing > 0 {
            (i * self.spacing as i64).min(self.range as i64) as i32
        } else {
            ((i * self.range as i64 + (self.increments as i64 - 1) / 2) / (self.increments as i64 - 1)) as i32
        }
    }

    /// The increment nearest position `px`.
    pub fn nearest_increment(&self, px: i32) -> i32 {
        (0..self.increments).min_by_key(|&i| (self.increment_px(i) - px).abs()).unwrap_or(0)
    }

    /// The nearest increment beyond `px` away from home (`away`) or towards
    /// it; `None` when there is none.
    pub fn next_increment(&self, px: i32, away: bool) -> Option<i32> {
        if away {
            (0..self.increments).map(|i| self.increment_px(i)).find(|&p| p > px)
        } else {
            (0..self.increments).rev().map(|i| self.increment_px(i)).find(|&p| p < px)
        }
    }

    /// Whether moving towards `dir` along the shaft moves away from home.
    pub fn away_from_home(&self, dir: Dir) -> bool {
        (dir == Dir::High) != self.home_high
    }

    /// Middle along of the arm at position `px` (where a tick points).
    pub fn mark_t(&self, px: i32) -> i32 {
        self.arm_t(px) + self.arm_len / 2
    }

    /// The arm at `px`: `(t, c)` of its low corner.
    pub fn arm_at(&self, px: i32) -> (i32, i32) {
        (self.arm_t(px), self.mid_c - self.arm_b / 2)
    }

    /// Across, the base of the primary scale's ticks (next to the arm),
    /// and the direction ticks grow in (+1 high, -1 low).
    pub fn tick_base(&self) -> (i32, i32) {
        if self.scale_high {
            (self.mid_c - self.arm_b / 2 + self.arm_b + TICK_GAP, 1)
        } else {
            (self.mid_c - self.arm_b / 2 - 1 - TICK_GAP, -1)
        }
    }

    /// Across, the band of the detent marks (opposite the primary scale):
    /// its low edge.
    pub fn detent_c(&self) -> i32 {
        let low = self.mid_c - self.arm_b / 2;
        if self.scale_high { low - TICK_GAP - DETENT_MARK_LEN } else { low + self.arm_b + TICK_GAP }
    }

    /// The low (`Dir::Low`) or high button: `(t, c)` of its low corner.
    pub fn button(&self, dir: Dir) -> Option<(i32, i32)> {
        let t = self.buttons_t?;
        let t = if dir == Dir::Low { t } else { t + BUTTON_SIZE };
        Some((t, self.mid_c - BUTTON_SIZE / 2))
    }
}

/// What a press at `(t, c)` hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Arm,
    Shaft(Dir),
    Button(Dir),
    /// The detent at this position (pixels from home).
    Detent(i32),
}

/// The part of slider `s` (geometry `g`) at `(t, c)`.
pub fn hit_test(s: &SliderState, g: &Geometry, (t, c): (i32, i32)) -> Option<Hit> {
    let inside = |(t0, c0): (i32, i32), tl: i32, cl: i32| t >= t0 && t < t0 + tl && c >= c0 && c < c0 + cl;
    for dir in [Dir::Low, Dir::High] {
        if g.button(dir).is_some_and(|b| inside(b, BUTTON_SIZE, BUTTON_SIZE)) {
            return Some(Hit::Button(dir));
        }
    }
    if inside(g.arm_at(s.arm), g.arm_len, g.arm_b) {
        return Some(Hit::Arm);
    }
    let dc = g.detent_c();
    if c >= dc && c < dc + DETENT_MARK_LEN
        && let Some(&(_, px)) = s.detents.iter().find(|(_, px)| (g.mark_t(*px) - t).abs() <= DETENT_HIT)
    {
        return Some(Hit::Detent(px));
    }
    let body_c = g.mid_c - g.arm_b / 2;
    if t >= g.shaft_t && t < g.shaft_t + g.shaft_len && c >= body_c && c < body_c + g.arm_b {
        return Some(Hit::Shaft(if t < g.arm_t(s.arm) { Dir::Low } else { Dir::High }));
    }
    None
}

/// The arm position one increment from `px` towards `dir`, if there is one.
pub fn step(g: &Geometry, px: i32, dir: Dir) -> Option<i32> {
    g.next_increment(px, g.away_from_home(dir))
}

/// The detent nearest `px` towards `dir`, if there is one.
pub fn next_detent(s: &SliderState, g: &Geometry, px: i32, dir: Dir) -> Option<i32> {
    let away = g.away_from_home(dir);
    s.detents.iter().map(|&(_, d)| d.clamp(0, g.range))
        .filter(|&d| if away { d > px } else { d < px })
        .min_by_key(|&d| (d - px).abs())
}

/// `MRFROM2SHORT(lo, hi)`.
fn two_shorts(lo: i32, hi: i32) -> u32 {
    (lo as u16 as u32) | ((hi as u16 as u32) << 16)
}

/// Signed position of a mouse message's mp1.
fn mouse_pos(mp1: u32) -> (i32, i32) {
    ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32)
}

impl WindowManager {
    fn slider_mut(&mut self, hwnd: u32) -> Option<&mut SliderState> {
        self.get_window_mut(hwnd)?.slider.as_deref_mut()
    }

    /// Give a new slider `hwnd` its state from `SLDCDATA` bytes `ctl`.
    pub fn build_slider(&mut self, hwnd: u32, ctl: &[u8]) {
        let Some(w) = self.get_window_mut(hwnd) else { return };
        if w.class_name != CLASS_SLIDER || w.slider.is_some() {
            return;
        }
        w.slider = Some(Box::new(SliderState::new(parse_sldcdata(ctl), w.style)));
    }
}

impl Loader {
    /// Style, size and a copy of the state of slider `hwnd`.
    fn slider_info(&self, hwnd: u32) -> Option<(u32, i32, i32, SliderState)> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let w = wm.get_window(hwnd)?;
        Some((w.style, w.cx, w.cy, w.slider.as_deref()?.clone()))
    }

    fn update_slider<R>(&self, hwnd: u32, f: impl FnOnce(&mut SliderState) -> R) -> Option<R> {
        self.shared.window_mgr.lock_or_recover().slider_mut(hwnd).map(f)
    }

    /// Move the arm of `hwnd` to `px` (pulled into the range); the owner is
    /// told `code` (`SLN_CHANGE` / `SLN_SLIDERTRACK`) when it moved.
    fn slider_move_to(&self, hwnd: u32, px: i32, code: u32) -> bool {
        let Some((style, cx, cy, s)) = self.slider_info(hwnd) else { return false };
        let px = px.clamp(0, Geometry::new(&s, style, cx, cy).range);
        if px == s.arm {
            return false;
        }
        self.update_slider(hwnd, |s| s.arm = px);
        self.repaint_control(hwnd);
        self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, code, px as u32);
        true
    }

    /// The `SLM_*` messages and the slider's input. `None` for messages
    /// left to the generic handling.
    pub(super) fn slider_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let (style, cx, cy, s) = self.slider_info(hwnd)?;
        let g = Geometry::new(&s, style, cx, cy);
        let ok = |b: bool| ApiResult::Normal(b as u32);
        let err = ApiResult::Normal(SLDERR_INVALID_PARAMETERS as u32);
        let lo = mp1 & 0xFFFF;
        let hi = mp1 >> 16;
        Some(match msg {
            WM_BUTTON1DOWN | WM_BUTTON1DBLCLK => ok(self.slider_press(hwnd, style, &s, &g, mouse_pos(mp1))),
            WM_MOUSEMOVE => ok(self.slider_drag(hwnd, &s, &g, mouse_pos(mp1))),
            WM_BUTTON1UP => ok(self.slider_release(hwnd, style, &g)),
            WM_TIMER if mp1 == TID_SCROLL => ok(self.slider_repeat(hwnd, &s, &g)),
            WM_CHAR => ok(self.slider_char(hwnd, style, &s, &g, mp1, mp2)),
            SLM_QUERYSLIDERINFO => ApiResult::Normal(match lo {
                SMA_SHAFTDIMENSIONS => two_shorts(g.shaft_len, g.shaft_b),
                SMA_SHAFTPOSITION => {
                    let (x, y, _, _) = g.rect(g.shaft_t, g.shaft_len, g.shaft_c, g.shaft_b);
                    two_shorts(x, y)
                }
                SMA_SLIDERARMDIMENSIONS => two_shorts(g.arm_len, g.arm_b),
                SMA_SLIDERARMPOSITION if hi == SMA_INCREMENTVALUE => g.nearest_increment(s.arm) as u32,
                SMA_SLIDERARMPOSITION => two_shorts(s.arm, g.range),
                _ => return Some(err),
            }),
            SLM_SETSLIDERINFO => {
                let set = match lo {
                    SMA_SHAFTDIMENSIONS => {
                        let b = mp2 & 0xFFFF;
                        (1..=MAX_PART_DIM).contains(&b)
                            && self.update_slider(hwnd, |s| s.shaft_breadth = b as i32).is_some()
                    }
                    SMA_SHAFTPOSITION => self.update_slider(hwnd, |s| {
                        s.shaft_pos = Some(((mp2 & 0xFFFF) as i16 as i32, (mp2 >> 16) as i16 as i32));
                    }).is_some(),
                    SMA_SLIDERARMDIMENSIONS => {
                        let (l, b) = (mp2 & 0xFFFF, mp2 >> 16);
                        (1..=MAX_PART_DIM).contains(&l) && (1..=MAX_PART_DIM).contains(&b)
                            && self.update_slider(hwnd, |s| s.arm_size = Some((l as i32, b as i32))).is_some()
                    }
                    SMA_SLIDERARMPOSITION => {
                        let v = (mp2 & 0xFFFF) as i32;
                        let px = if hi == SMA_INCREMENTVALUE {
                            (v < g.increments).then(|| g.increment_px(v))
                        } else {
                            (v <= g.range).then_some(v)
                        };
                        let Some(px) = px else { return Some(ok(false)) };
                        self.slider_move_to(hwnd, px, SLN_CHANGE);
                        return Some(ok(true));
                    }
                    _ => false,
                };
                if set {
                    self.repaint_control(hwnd);
                }
                ok(set)
            }
            // mp1 = MPFROM2SHORT(tick or SMA_SETALLTICKS, length).
            SLM_SETTICKSIZE => {
                let len = hi.min(MAX_TICK_LEN) as u16;
                let set = self.update_slider(hwnd, |s| {
                    if lo == SMA_SETALLTICKS {
                        s.ticks.iter_mut().for_each(|t| *t = len);
                        true
                    } else if let Some(t) = s.ticks.get_mut(lo as usize) {
                        *t = len;
                        true
                    } else {
                        false
                    }
                }).unwrap_or(false);
                if set {
                    self.repaint_control(hwnd);
                }
                ok(set)
            }
            SLM_QUERYTICKSIZE => s.ticks.get(lo as usize).map_or(err, |&t| ApiResult::Normal(t as u32)),
            SLM_QUERYTICKPOS => {
                if lo as i32 >= g.increments {
                    return Some(err);
                }
                let (base, _) = g.tick_base();
                let (x, y) = g.point(g.mark_t(g.increment_px(lo as i32)), base);
                ApiResult::Normal(two_shorts(x, y))
            }
            // mp1 = tick, mp2 = PSZ (NULL clears the text).
            SLM_SETSCALETEXT => {
                if lo as i32 >= g.increments {
                    return Some(ok(false));
                }
                let mut text = if mp2 != 0 {
                    self.read_guest_string_cp(mp2, self.window_codepage(hwnd))
                } else {
                    String::new()
                };
                truncate_utf8(&mut text, MAX_SCALE_TEXT);
                self.update_slider(hwnd, |s| if let Some(t) = s.texts.get_mut(lo as usize) { *t = text });
                self.repaint_control(hwnd);
                ok(true)
            }
            // mp1 = MPFROM2SHORT(tick, buffer length), mp2 = buffer.
            SLM_QUERYSCALETEXT => {
                let Some(text) = s.texts.get(lo as usize) else { return Some(err) };
                let bytes = super::codepage::cp_encode(text, self.window_codepage(hwnd));
                ApiResult::Normal(self.copy_text_out(&bytes, mp2, hi as usize))
            }
            // mp1 = pixels from home; returns the new detent's id (0: none).
            SLM_ADDDETENT => {
                let px = lo as i32;
                if px > g.range || s.detents.len() >= MAX_DETENTS {
                    return Some(ApiResult::Normal(0));
                }
                let id = self.update_slider(hwnd, |s| {
                    let id = s.next_detent;
                    s.next_detent = s.next_detent.wrapping_add(1).max(1);
                    s.detents.push((id, px));
                    id
                }).unwrap_or(0);
                self.repaint_control(hwnd);
                ApiResult::Normal(id)
            }
            SLM_REMOVEDETENT => {
                let removed = self.update_slider(hwnd, |s| {
                    let n = s.detents.len();
                    s.detents.retain(|&(id, _)| id != mp1);
                    s.detents.len() != n
                }).unwrap_or(false);
                if removed {
                    self.repaint_control(hwnd);
                }
                ok(removed)
            }
            // MRFROM2SHORT(pixels from home, scale it is on).
            SLM_QUERYDETENTPOS => match s.detents.iter().find(|&&(id, _)| id == mp1) {
                Some(&(_, px)) => {
                    let scale = if primary_index(style) == 0 { SMA_SCALE1 } else { SMA_SCALE2 };
                    ApiResult::Normal(two_shorts(px, scale as i32))
                }
                None => err,
            },
            _ => return None,
        })
    }

    /// Copy `bytes` and a NUL into the guest buffer `buf` of `len` bytes
    /// (cut to fit); returns the bytes copied, or the whole length when
    /// `len` is 0 (the size to allocate, NUL left out).
    pub(super) fn copy_text_out(&self, bytes: &[u8], buf: u32, len: usize) -> u32 {
        if len == 0 || buf == 0 {
            return bytes.len() as u32;
        }
        let n = bytes.len().min(len - 1);
        let mut out = bytes[..n].to_vec();
        out.push(0);
        if self.guest_write_bytes(buf, &out).is_none() {
            return 0;
        }
        n as u32
    }

    /// Button 1 pressed at `at` (slider coordinates).
    fn slider_press(&self, hwnd: u32, style: u32, s: &SliderState, g: &Geometry, at: (i32, i32)) -> bool {
        if style & (WS_DISABLED | SLS_READONLY) != 0 {
            return false;
        }
        self.set_focus_window(hwnd);
        let (t, c) = g.along(at);
        let Some(hit) = hit_test(s, g, (t, c)) else { return true };
        let held = match hit {
            Hit::Arm => Held::Arm { grab: t - g.arm_t(s.arm), start: s.arm },
            Hit::Shaft(dir) => Held::Shaft(dir),
            Hit::Button(dir) => Held::Button { dir, over: true },
            Hit::Detent(px) => {
                self.slider_move_to(hwnd, px, SLN_CHANGE);
                return true;
            }
        };
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            wm.pressed_control = hwnd;
            wm.drag_pointer = at;
            if let Some(s) = wm.slider_mut(hwnd) {
                s.held = Some(held);
            }
        }
        match held {
            Held::Shaft(dir) | Held::Button { dir, .. } => {
                if let Some(px) = step(g, s.arm, dir) {
                    self.slider_move_to(hwnd, px, SLN_CHANGE);
                }
                self.repaint_control(hwnd);
            }
            Held::Arm { .. } => {}
        }
        true
    }

    /// The pointer moved while button 1 holds a part of the slider.
    fn slider_drag(&self, hwnd: u32, s: &SliderState, g: &Geometry, at: (i32, i32)) -> bool {
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if wm.pressed_control != hwnd {
                return false;
            }
            wm.drag_pointer = at;
        }
        let (t, c) = g.along(at);
        match s.held {
            Some(Held::Arm { grab, .. }) => {
                self.slider_move_to(hwnd, g.px_at(t - grab), SLN_SLIDERTRACK);
            }
            Some(Held::Button { dir, over }) => {
                let now = hit_test(s, g, (t, c)) == Some(Hit::Button(dir));
                if now != over {
                    self.update_slider(hwnd, |s| s.held = Some(Held::Button { dir, over: now }));
                    self.repaint_control(hwnd);
                }
            }
            _ => {}
        }
        true
    }

    /// Button 1 released: a dragged arm snaps (`SLS_SNAPTOINCREMENT`) and
    /// the owner is told `SLN_CHANGE` if it moved.
    fn slider_release(&self, hwnd: u32, style: u32, g: &Geometry) -> bool {
        let (held, arm) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if wm.pressed_control == hwnd {
                wm.pressed_control = 0;
            }
            let Some(s) = wm.slider_mut(hwnd) else { return false };
            (s.held.take(), s.arm)
        };
        match held {
            Some(Held::Arm { start, .. }) => {
                let end = if style & SLS_SNAPTOINCREMENT != 0 {
                    g.increment_px(g.nearest_increment(arm))
                } else {
                    arm
                };
                if end != arm {
                    self.update_slider(hwnd, |s| s.arm = end);
                }
                self.repaint_control(hwnd);
                if end != start {
                    self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, SLN_CHANGE, end as u32);
                }
                true
            }
            Some(_) => {
                self.repaint_control(hwnd);
                true
            }
            None => false,
        }
    }

    /// `WM_TIMER(TID_SCROLL)` while a button or the shaft is held: one more
    /// increment while the pointer stays on the button / beyond the arm.
    fn slider_repeat(&self, hwnd: u32, s: &SliderState, g: &Geometry) -> bool {
        let at = {
            let wm = self.shared.window_mgr.lock_or_recover();
            if wm.pressed_control != hwnd {
                return false;
            }
            wm.drag_pointer
        };
        let (t, c) = g.along(at);
        let dir = match s.held {
            Some(Held::Button { dir, .. }) if hit_test(s, g, (t, c)) == Some(Hit::Button(dir)) => dir,
            Some(Held::Shaft(dir)) if hit_test(s, g, (t, c)) == Some(Hit::Shaft(dir)) => dir,
            _ => return false,
        };
        match step(g, s.arm, dir) {
            Some(px) => self.slider_move_to(hwnd, px, SLN_CHANGE),
            None => false,
        }
    }

    /// A key for the slider: arrows along its axis step (Shift: to the next
    /// detent), Home / End go to the ends; other keys go to the owner.
    fn slider_char(&self, hwnd: u32, style: u32, s: &SliderState, g: &Geometry, mp1: u32, mp2: u32) -> bool {
        let fs = mp1 & 0xFFFF;
        let vk = if fs & KC_VIRTUALKEY != 0 { mp2 >> WM_CHAR_VKEY_SHIFT } else { 0 };
        let plain = fs & (KC_CTRL | KC_ALT) == 0;
        let dir = match (vk, g.vertical) {
            (VK_LEFT, false) | (VK_DOWN, true) => Some(Dir::Low),
            (VK_RIGHT, false) | (VK_UP, true) => Some(Dir::High),
            _ => None,
        };
        let usable = style & (WS_DISABLED | SLS_READONLY) == 0;
        let handled = plain && usable && (dir.is_some() || vk == VK_HOME || vk == VK_END);
        if !handled {
            let owner = self.shared.window_mgr.lock_or_recover().notify_target(hwnd);
            if owner != 0 && owner != hwnd {
                self.post_control_message(owner, WM_CHAR, mp1, mp2);
            }
            return false;
        }
        if fs & KC_KEYUP != 0 {
            return true;
        }
        let target = match dir {
            Some(dir) if fs & KC_SHIFT != 0 => next_detent(s, g, s.arm, dir),
            Some(dir) => step(g, s.arm, dir),
            None if vk == VK_HOME => Some(0),
            None => Some(g.range),
        };
        if let Some(px) = target {
            self.slider_move_to(hwnd, px, SLN_CHANGE);
        }
        true
    }

    /// The rectangle of owner-drawn part `part` (`SDA_*`) of slider `hwnd`
    /// in its own coordinates, right and top exclusive.
    pub(super) fn slider_part_rect(&self, hwnd: u32, part: u32) -> Option<(i32, i32, i32, i32)> {
        let (style, cx, cy, s) = self.slider_info(hwnd)?;
        let g = Geometry::new(&s, style, cx, cy);
        let (x, y, w, h) = match part {
            SDA_BACKGROUND => (0, 0, cx, cy),
            SDA_SLIDERSHAFT => g.rect(g.shaft_t, g.shaft_len, g.shaft_c, g.shaft_b),
            SDA_RIBBONSTRIP => {
                let (t0, t1) = ribbon_span(&g, s.arm);
                g.rect(t0, t1 - t0, g.shaft_c, g.shaft_b)
            }
            SDA_SLIDERARM => {
                let (t, c) = g.arm_at(s.arm);
                g.rect(t, g.arm_len, c, g.arm_b)
            }
            _ => return None,
        };
        Some((x, y, x + w, y + h))
    }

    /// WC_SLIDER: background, scale, buttons, shaft, ribbon strip and arm;
    /// an owner-drawn slider then asks its owner for each part.
    pub(super) fn paint_slider(&self, p: &Painter, hwnd: u32, rect: (i32, i32, i32, i32)) {
        let Some(style) = self.shared.window_mgr.lock_or_recover().get_window(hwnd).map(|w| w.style) else { return };
        for part in [SDA_BACKGROUND, SDA_SLIDERSHAFT, SDA_RIBBONSTRIP, SDA_SLIDERARM] {
            self.paint_slider_part(p, hwnd, rect, part);
        }
        if style & SLS_OWNERDRAW != 0 {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let parts: &[u32] = if style & SLS_RIBBONSTRIP != 0 {
                &[SDA_BACKGROUND, SDA_SLIDERSHAFT, SDA_RIBBONSTRIP, SDA_SLIDERARM]
            } else {
                &[SDA_BACKGROUND, SDA_SLIDERSHAFT, SDA_SLIDERARM]
            };
            for &part in parts {
                wm.queue_owner_call(super::pm_owner::OwnerCall {
                    control: hwnd, kind: super::pm_owner::OwnerCallKind::SliderPart { part },
                });
            }
        }
    }

    /// Paint part `part` (`SDA_*`) of slider `hwnd` at `(x, y, cx, cy)`;
    /// the background includes the scale, detents and buttons.
    pub(super) fn paint_slider_part(&self, p: &Painter, hwnd: u32, (x, y, _, _): (i32, i32, i32, i32), part: u32) {
        let Some((style, cx, cy, s)) = self.slider_info(hwnd) else { return };
        let focused = self.shared.window_mgr.lock_or_recover().focus_hwnd == hwnd;
        let g = Geometry::new(&s, style, cx, cy);
        // A rectangle of the slider, as inclusive corners in the surface.
        let fill = |(rx, ry, w, h): (i32, i32, i32, i32), color: u32| if w > 0 && h > 0 {
            p.fill((x + rx, y + ry, x + rx + w - 1, y + ry + h - 1), color);
        };
        let boxed = |(rx, ry, w, h): (i32, i32, i32, i32), color: u32| if w > 0 && h > 0 {
            control_box(p, (x + rx, y + ry, w - 1, h - 1), color);
        };
        match part {
            SDA_BACKGROUND => {
                fill((0, 0, cx, cy), self.control_background(hwnd));
                self.paint_slider_decorations(p, (x, y), &s, &g, style);
            }
            SDA_SLIDERSHAFT => {
                let r = g.rect(g.shaft_t, g.shaft_len, g.shaft_c, g.shaft_b);
                boxed(r, CONTROL_FIELD_COLOR);
            }
            SDA_RIBBONSTRIP if style & SLS_RIBBONSTRIP != 0 => {
                let (t0, t1) = ribbon_span(&g, s.arm);
                // Inside the shaft's outline.
                fill(g.rect(t0 + 1, t1 - t0 - 2, g.shaft_c + 1, g.shaft_b - 2), RIBBON_COLOR);
            }
            SDA_SLIDERARM => {
                let (t, c) = g.arm_at(s.arm);
                let face = if matches!(s.held, Some(Held::Arm { .. })) { CONTROL_PRESSED_COLOR } else { CONTROL_FACE_COLOR };
                boxed(g.rect(t, g.arm_len, c, g.arm_b), face);
                if focused {
                    let (mt, mc) = (t + (g.arm_len - FOCUS_MARK) / 2, g.mid_c - FOCUS_MARK / 2);
                    fill(g.rect(mt, FOCUS_MARK, mc, FOCUS_MARK), CONTROL_TEXT_COLOR);
                }
            }
            _ => {}
        }
    }

    /// The parts drawn with the background: ticks and texts of the primary
    /// scale, detent marks and the buttons.
    fn paint_slider_decorations(&self, p: &Painter, (x, y): (i32, i32), s: &SliderState,
                                g: &Geometry, style: u32) {
        let fill = |(rx, ry, w, h): (i32, i32, i32, i32), color: u32| if w > 0 && h > 0 {
            p.fill((x + rx, y + ry, x + rx + w - 1, y + ry + h - 1), color);
        };
        let (base, sign) = g.tick_base();
        for i in 0..g.increments {
            let mt = g.mark_t(g.increment_px(i));
            let len = s.ticks.get(i as usize).copied().unwrap_or(0) as i32;
            if len > 0 {
                let c0 = if sign > 0 { base } else { base - len + 1 };
                fill(g.rect(mt, 1, c0, len), CONTROL_TEXT_COLOR);
            }
            let Some(text) = s.texts.get(i as usize).filter(|t| !t.is_empty()) else { continue };
            let w = font::text_width(text, p.font());
            let out = base + sign * (len + TEXT_GAP);
            let (tx, ty) = match (g.vertical, sign > 0) {
                (false, true) => (mt - w / 2, out),
                (false, false) => (mt - w / 2, out - CONTROL_LINE_H + 1),
                (true, true) => (out, mt - CONTROL_LINE_H / 2),
                (true, false) => (out - w + 1, mt - CONTROL_LINE_H / 2),
            };
            control_text(p, x + tx, y + ty, text);
        }
        let dc = g.detent_c();
        for &(_, px) in &s.detents {
            fill(g.rect(g.mark_t(px) - 1, 3, dc, DETENT_MARK_LEN), CONTROL_BORDER_COLOR);
        }
        let held = s.held;
        let glyph = if style & WS_DISABLED != 0 { CONTROL_BORDER_COLOR } else { CONTROL_TEXT_COLOR };
        for dir in [Dir::Low, Dir::High] {
            let Some((t, c)) = g.button(dir) else { continue };
            let pressed = held == Some(Held::Button { dir, over: true });
            let (bx, by, bw, bh) = g.rect(t, BUTTON_SIZE, c, BUTTON_SIZE);
            control_box(p, (x + bx, y + by, bw - 1, bh - 1),
                        if pressed { CONTROL_PRESSED_COLOR } else { CONTROL_FACE_COLOR });
            super::pm_scrollbar::draw_arrow(p, (x + bx, y + by, bw, bh), g.vertical,
                                            // The low button points to the low end: left / down.
                                            (dir == Dir::Low) != g.vertical, glyph);
        }
    }

    /// The owner drew (`drawn`) or left (`!drawn`) part `part` of slider
    /// `hwnd`: the slider draws a part left to it, and after a background
    /// the owner drew, the scale, detents and buttons over it again.
    pub(super) fn slider_owner_done(&self, hwnd: u32, part: u32, drawn: bool) {
        let (frame, rect) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            (wm.find_frame_for_hwnd(hwnd), wm.get_abs_rect_in_frame(hwnd))
        };
        let p = Painter::new(self, frame, hwnd).clipped_to(rect);
        if !drawn {
            self.paint_slider_part(&p, hwnd, rect, part);
        } else if part == SDA_BACKGROUND
            && let Some((style, cx, cy, s)) = self.slider_info(hwnd)
        {
            let g = Geometry::new(&s, style, cx, cy);
            self.paint_slider_decorations(&p, (rect.0, rect.1), &s, &g, style);
        }
    }
}

/// Along, the ribbon strip from the home end of the shaft to the middle of
/// the arm at `px`: `(start, end)`.
fn ribbon_span(g: &Geometry, px: i32) -> (i32, i32) {
    let mid = g.mark_t(px);
    if g.home_high { (mid, g.shaft_t + g.shaft_len) } else { (g.shaft_t, mid) }
}

/// Cut `s` to at most `max` bytes at a character boundary.
pub(super) fn truncate_utf8(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::pm_types::OS2Message;

    fn sldcdata(inc1: u16, sp1: u16, inc2: u16, sp2: u16) -> Vec<u8> {
        let mut v = (SLDCDATA_SIZE as u32).to_le_bytes().to_vec();
        for w in [inc1, sp1, inc2, sp2] {
            v.extend_from_slice(&w.to_le_bytes());
        }
        v
    }

    fn state(inc: u16, spacing: u16, style: u32) -> SliderState {
        SliderState::new(parse_sldcdata(&sldcdata(inc, spacing, 3, 0)), style)
    }

    #[test]
    fn sldcdata_is_parsed_and_bounded() {
        assert_eq!(parse_sldcdata(&sldcdata(6, 10, 3, 0)),
                   [Scale { increments: 6, spacing: 10 }, Scale { increments: 3, spacing: 0 }]);
        assert_eq!(parse_sldcdata(&sldcdata(60000, 1, 0, 0))[0].increments, MAX_INCREMENTS);
        // A short cbSize leaves out the fields past it; no data at all is zeroes.
        let mut short = sldcdata(6, 10, 3, 4);
        short[0] = 8;
        assert_eq!(parse_sldcdata(&short), [Scale { increments: 6, spacing: 10 }, Scale::default()]);
        assert_eq!(parse_sldcdata(&[]), [Scale::default(); 2]);
        assert_eq!(parse_sldcdata(&[0xFF, 0xFF, 0xFF, 0xFF, 5]), [Scale::default(); 2], "cbSize past the data");
        // The primary scale gives the ticks.
        assert_eq!(state(6, 10, 0).ticks.len(), 6);
        assert_eq!(state(6, 10, SLS_PRIMARYSCALE2).ticks.len(), 3);
    }

    #[test]
    fn fixed_spacing_sets_the_shaft_length() {
        let s = state(6, 20, 0);
        let g = Geometry::new(&s, 0, 300, 60);
        assert_eq!(g.range, 100);
        assert_eq!(g.shaft_len, 100 + DEFAULT_ARM_LENGTH);
        assert_eq!(g.shaft_t, EDGE_GAP);
        assert_eq!(g.mid_c, 30, "centred");
        assert_eq!((0..6).map(|i| g.increment_px(i)).collect::<Vec<_>>(), vec![0, 20, 40, 60, 80, 100]);
        assert_eq!(g.nearest_increment(29), 1);
        assert_eq!(g.nearest_increment(31), 2);
        assert_eq!(g.next_increment(20, true), Some(40));
        assert_eq!(g.next_increment(25, false), Some(20));
        assert_eq!(g.next_increment(100, true), None);
        assert_eq!(g.next_increment(0, false), None);
    }

    #[test]
    fn spacing_zero_fills_the_window() {
        let s = state(5, 0, 0);
        let g = Geometry::new(&s, 0, 204, 40);
        assert_eq!(g.range, 204 - 2 * EDGE_GAP - DEFAULT_ARM_LENGTH);
        assert_eq!(g.increment_px(4), g.range);
        assert_eq!(g.increment_px(2), (g.range + 1) / 2);
        // Buttons take room at their end.
        let gb = Geometry::new(&s, SLS_BUTTONSLEFT, 204, 40);
        assert_eq!(gb.range, g.range - 2 * BUTTON_SIZE - BUTTON_GAP);
        assert_eq!(gb.button(Dir::Low), Some((EDGE_GAP, 20 - BUTTON_SIZE / 2)));
        assert_eq!(gb.shaft_t, EDGE_GAP + 2 * BUTTON_SIZE + BUTTON_GAP);
        let both = Geometry::new(&s, SLS_BUTTONSLEFT | SLS_BUTTONSRIGHT, 204, 40);
        assert_eq!(both.buttons_t, None, "conflicting button styles: none");
        assert_eq!(Geometry::new(&s, SLS_BUTTONSRIGHT | SLS_READONLY, 204, 40).buttons_t, None);
        // A window too small leaves no range.
        assert_eq!(Geometry::new(&s, 0, 5, 40).range, 0);
    }

    #[test]
    fn hostile_spacing_is_bounded() {
        let s = state(MAX_INCREMENTS, 0xFFFF, 0);
        let g = Geometry::new(&s, 0, 100, 40);
        assert_eq!(g.range, MAX_RANGE);
        assert!(g.increment_px(MAX_INCREMENTS as i32 - 1) <= MAX_RANGE);
        assert_eq!(g.increment_px(100_000), g.increment_px(MAX_INCREMENTS as i32 - 1), "clamped");
    }

    #[test]
    fn home_and_orientation_map_positions() {
        let s = state(6, 20, 0);
        let g = Geometry::new(&s, SLS_HOMERIGHT, 300, 60);
        assert_eq!(g.arm_t(0), g.shaft_t + 100, "home at the right");
        assert_eq!(g.px_at(g.shaft_t), 100);
        assert!(g.away_from_home(Dir::Low));
        assert_eq!(step(&g, 40, Dir::Low), Some(60));
        let v = Geometry::new(&s, SLS_VERTICAL, 60, 300);
        assert_eq!(v.rect(5, 10, 20, 8), (20, 5, 8, 10));
        assert_eq!(v.along((7, 9)), (9, 7));
        assert_eq!(step(&v, 40, Dir::High), Some(60), "up moves away from a bottom home");
        // Placement across.
        assert_eq!(Geometry::new(&s, SLS_TOP, 300, 60).mid_c, 60 - (DEFAULT_ARM_BREADTH + 1) / 2);
        assert_eq!(Geometry::new(&s, SLS_BOTTOM, 300, 60).mid_c, DEFAULT_ARM_BREADTH / 2);
        // Scale 1 above, scale 2 below.
        assert_eq!(g.tick_base().1, 1);
        assert_eq!(Geometry::new(&s, SLS_PRIMARYSCALE2, 300, 60).tick_base().1, -1);
    }

    #[test]
    fn hits_arm_shaft_buttons_and_detents() {
        let mut s = state(6, 20, 0);
        s.arm = 40;
        s.detents.push((1, 80));
        let g = Geometry::new(&s, SLS_BUTTONSRIGHT, 300, 60);
        let arm_t = g.arm_t(40);
        assert_eq!(hit_test(&s, &g, (arm_t + 1, 30)), Some(Hit::Arm));
        assert_eq!(hit_test(&s, &g, (arm_t - 5, 30)), Some(Hit::Shaft(Dir::Low)));
        assert_eq!(hit_test(&s, &g, (arm_t + g.arm_len + 5, 30)), Some(Hit::Shaft(Dir::High)));
        let (bt, bc) = g.button(Dir::High).unwrap();
        assert_eq!(hit_test(&s, &g, (bt + 1, bc + 1)), Some(Hit::Button(Dir::High)));
        assert_eq!(hit_test(&s, &g, (bt - BUTTON_SIZE + 1, bc + 1)), Some(Hit::Button(Dir::Low)));
        assert_eq!(hit_test(&s, &g, (g.mark_t(80) + 2, g.detent_c() + 1)), Some(Hit::Detent(80)));
        assert_eq!(hit_test(&s, &g, (arm_t, 1)), None, "beside the slider");
        assert_eq!(next_detent(&s, &g, 40, Dir::High), Some(80));
        assert_eq!(next_detent(&s, &g, 40, Dir::Low), None);
    }

    // ── Messages ────────────────────────────────────────────────────────────

    struct Fx {
        loader: Loader,
        hmq: u32,
        client: u32,
        slider: u32,
    }

    const ID: u32 = 50;
    const BUF: u32 = 0x3000;

    /// A 300 × 60 slider (scale 1: 6 increments 20 pels apart) at (20, 20)
    /// in a client.
    fn fixture(style: u32) -> Fx {
        let loader = Loader::new_mock();
        let (hmq, client, slider) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            for h in [frame, client] {
                let w = wm.get_window_mut(h).unwrap();
                (w.cx, w.cy, w.visible) = (400, 300, true);
            }
            let h = wm.create_window(CLASS_SLIDER.into(), client, hmq);
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy, w.style, w.id, w.owner, w.visible) =
                (20, 20, 300, 60, WS_VISIBLE | style, ID, client, true);
            wm.build_slider(h, &sldcdata(6, 20, 3, 0));
            (hmq, client, h)
        };
        Fx { loader, hmq, client, slider }
    }

    impl Fx {
        fn send(&self, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(self.slider, msg, mp1, mp2) {
                ApiResult::Normal(v) => v,
                other => panic!("unexpected {other:?}"),
            }
        }
        /// The WM_CONTROL notifications queued since the last call: (code, mp2).
        fn notes(&self) -> Vec<(u32, u32)> {
            let wm = self.loader.shared.window_mgr.lock_or_recover();
            let mq = wm.get_mq(self.hmq).unwrap();
            let msgs: Vec<OS2Message> = mq.lock_or_recover().messages.drain(..).collect();
            msgs.into_iter().filter(|m| m.msg == WM_CONTROL)
                .inspect(|m| assert_eq!((m.hwnd, m.mp1 & 0xFFFF), (self.client, ID)))
                .map(|m| (m.mp1 >> 16, m.mp2)).collect()
        }
        fn geometry(&self) -> Geometry {
            let (style, cx, cy, s) = self.loader.slider_info(self.slider).unwrap();
            Geometry::new(&s, style, cx, cy)
        }
        fn at(&self, t: i32, c: i32) -> u32 {
            let (x, y) = self.geometry().point(t, c);
            (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16)
        }
        fn key(&self, vk: u32, extra: u32) -> u32 {
            self.send(WM_CHAR, KC_VIRTUALKEY | extra, vk << WM_CHAR_VKEY_SHIFT)
        }
    }

    fn info(kind: u32, attr: u32) -> u32 {
        kind | (attr << 16)
    }

    #[test]
    fn slider_info_is_set_and_queried() {
        let f = fixture(0);
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(SMA_SHAFTDIMENSIONS, 0), 0),
                   two_shorts(100 + DEFAULT_ARM_LENGTH, DEFAULT_SHAFT_BREADTH));
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_RANGEVALUE), 0), two_shorts(0, 100));
        assert_eq!(f.send(SLM_SETSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE), 3), 1);
        assert_eq!(f.notes(), vec![(SLN_CHANGE, 60)]);
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE), 0), 3);
        assert_eq!(f.send(SLM_SETSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE), 6), 0, "no 7th increment");
        assert_eq!(f.send(SLM_SETSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_RANGEVALUE), 33), 1);
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_RANGEVALUE), 0), two_shorts(33, 100));
        assert_eq!(f.send(SLM_SETSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_RANGEVALUE), 101), 0);
        assert_eq!(f.send(SLM_SETSLIDERINFO, info(SMA_SHAFTDIMENSIONS, 0), 12), 1);
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(SMA_SHAFTDIMENSIONS, 0), 0) >> 16, 12);
        assert_eq!(f.send(SLM_SETSLIDERINFO, info(SMA_SHAFTDIMENSIONS, 0), 0), 0, "a zero breadth");
        assert_eq!(f.send(SLM_SETSLIDERINFO, info(SMA_SLIDERARMDIMENSIONS, 0), two_shorts(14, 30)), 1);
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(SMA_SLIDERARMDIMENSIONS, 0), 0), two_shorts(14, 30));
        assert_eq!(f.send(SLM_SETSLIDERINFO, info(SMA_SLIDERARMDIMENSIONS, 0), two_shorts(14, 60000)), 0);
        assert_eq!(f.send(SLM_SETSLIDERINFO, info(SMA_SHAFTPOSITION, 0), two_shorts(10, 5)), 1);
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(SMA_SHAFTPOSITION, 0), 0), two_shorts(10, 5));
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(9, 0), 0), SLDERR_INVALID_PARAMETERS as u32);
    }

    #[test]
    fn ticks_texts_and_detents() {
        let f = fixture(0);
        assert_eq!(f.send(SLM_QUERYTICKSIZE, 2, 0), 0, "no ticks until set");
        assert_eq!(f.send(SLM_SETTICKSIZE, two_shorts(SMA_SETALLTICKS as i32, 5), 0), 1);
        assert_eq!(f.send(SLM_SETTICKSIZE, two_shorts(2, 9), 0), 1);
        assert_eq!(f.send(SLM_SETTICKSIZE, two_shorts(6, 9), 0), 0, "past the last tick");
        assert_eq!(f.send(SLM_QUERYTICKSIZE, 2, 0), 9);
        assert_eq!(f.send(SLM_QUERYTICKSIZE, 0, 0), 5);
        assert_eq!(f.send(SLM_QUERYTICKSIZE, 7, 0), SLDERR_INVALID_PARAMETERS as u32);
        assert_eq!(f.send(SLM_SETTICKSIZE, two_shorts(1, 5000), 0), 1);
        assert_eq!(f.send(SLM_QUERYTICKSIZE, 1, 0), MAX_TICK_LEN, "bounded");
        let g = f.geometry();
        let (base, _) = g.tick_base();
        assert_eq!(f.send(SLM_QUERYTICKPOS, 1, 0), two_shorts(g.mark_t(20), base));
        // Scale texts, encoded in the queue's codepage.
        f.loader.guest_write_bytes(BUF, b"Mid\0").unwrap();
        assert_eq!(f.send(SLM_SETSCALETEXT, 3, BUF), 1);
        assert_eq!(f.send(SLM_SETSCALETEXT, 9, BUF), 0);
        assert_eq!(f.send(SLM_QUERYSCALETEXT, two_shorts(3, 0), 0), 3, "length to allocate");
        assert_eq!(f.send(SLM_QUERYSCALETEXT, two_shorts(3, 3), BUF + 0x100), 2, "cut to the buffer");
        assert_eq!(f.loader.guest_read_bytes(BUF + 0x100, 3).unwrap(), b"Mi\0");
        assert_eq!(f.send(SLM_QUERYSCALETEXT, two_shorts(9, 8), BUF), SLDERR_INVALID_PARAMETERS as u32);
        // Detents.
        let d1 = f.send(SLM_ADDDETENT, 50, 0);
        let d2 = f.send(SLM_ADDDETENT, 90, 0);
        assert!(d1 != 0 && d2 != 0 && d1 != d2);
        assert_eq!(f.send(SLM_ADDDETENT, 101, 0), 0, "past the range");
        assert_eq!(f.send(SLM_QUERYDETENTPOS, d2, 0), two_shorts(90, SMA_SCALE1 as i32));
        assert_eq!(f.send(SLM_REMOVEDETENT, d2, 0), 1);
        assert_eq!(f.send(SLM_REMOVEDETENT, d2, 0), 0);
        assert_eq!(f.send(SLM_QUERYDETENTPOS, d2, 0), SLDERR_INVALID_PARAMETERS as u32);
    }

    #[test]
    fn detents_are_bounded() {
        let f = fixture(0);
        for _ in 0..MAX_DETENTS {
            assert_ne!(f.send(SLM_ADDDETENT, 10, 0), 0);
        }
        assert_eq!(f.send(SLM_ADDDETENT, 10, 0), 0);
    }

    #[test]
    fn dragging_the_arm_tracks_and_snaps() {
        let f = fixture(SLS_SNAPTOINCREMENT);
        let g = f.geometry();
        let c = g.mid_c;
        f.send(WM_BUTTON1DOWN, f.at(g.arm_t(0) + 2, c), 0);
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().pressed_control, f.slider);
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().focus_hwnd, f.slider, "a click focuses");
        f.notes();
        f.send(WM_MOUSEMOVE, f.at(g.arm_t(0) + 2 + 47, c), 0);
        assert_eq!(f.notes(), vec![(SLN_SLIDERTRACK, 47)]);
        f.send(WM_MOUSEMOVE, f.at(g.arm_t(0) + 2 + 47, c + 30), 0);
        assert_eq!(f.notes(), vec![], "no move along, no notification");
        f.send(WM_BUTTON1UP, f.at(g.arm_t(0) + 2 + 47, c), 0);
        assert_eq!(f.notes(), vec![(SLN_CHANGE, 40)], "snapped to the nearest increment");
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_RANGEVALUE), 0), two_shorts(40, 100));
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().pressed_control, 0);
    }

    #[test]
    fn without_snapping_the_arm_stays_where_released() {
        let f = fixture(0);
        let g = f.geometry();
        f.send(WM_BUTTON1DOWN, f.at(g.arm_t(0) + 2, g.mid_c), 0);
        f.send(WM_MOUSEMOVE, f.at(g.arm_t(0) + 2 + 500, g.mid_c), 0);
        f.send(WM_BUTTON1UP, 0, 0);
        assert_eq!(f.notes(), vec![(SLN_SETFOCUS, f.slider), (SLN_SLIDERTRACK, 100), (SLN_CHANGE, 100)],
                   "dragged past the end: the range's end");
    }

    #[test]
    fn shaft_and_buttons_step_and_repeat() {
        let f = fixture(SLS_BUTTONSLEFT);
        let g = f.geometry();
        // A shaft click right of the arm: one increment towards it.
        f.send(WM_BUTTON1DOWN, f.at(g.shaft_t + 90, g.mid_c), 0);
        f.notes();
        assert_eq!(f.send(SLM_QUERYSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_RANGEVALUE), 0) & 0xFFFF,
                   g.increment_px(1) as u32);
        {
            let wm = f.loader.shared.window_mgr.lock_or_recover();
            assert_eq!(wm.post_drag_scroll(), Some(f.slider), "held: the timer repeats");
        }
        f.notes();
        f.send(WM_TIMER, TID_SCROLL, 0);
        assert_eq!(f.notes(), vec![(SLN_CHANGE, g.increment_px(2) as u32)]);
        f.send(WM_BUTTON1UP, 0, 0);
        f.send(WM_TIMER, TID_SCROLL, 0);
        assert_eq!(f.notes(), vec![], "released: no repeat");
        // The low button steps back.
        let (bt, bc) = g.button(Dir::Low).unwrap();
        f.send(WM_BUTTON1DOWN, f.at(bt + 3, bc + 3), 0);
        f.send(WM_BUTTON1UP, 0, 0);
        assert_eq!(f.notes(), vec![(SLN_CHANGE, g.increment_px(1) as u32)]);
    }

    #[test]
    fn keys_step_jump_to_detents_and_ends() {
        let f = fixture(0);
        f.send(SLM_ADDDETENT, 70, 0);
        f.notes();
        assert_eq!(f.key(VK_RIGHT, 0), 1);
        assert_eq!(f.key(VK_RIGHT, KC_KEYUP), 1, "the release is the slider's too");
        assert_eq!(f.notes(), vec![(SLN_CHANGE, 20)]);
        f.key(VK_RIGHT, KC_SHIFT);
        assert_eq!(f.notes(), vec![(SLN_CHANGE, 70)], "Shift: the next detent");
        f.key(VK_END, 0);
        f.key(VK_LEFT, 0);
        assert_eq!(f.notes(), vec![(SLN_CHANGE, 100), (SLN_CHANGE, 80)]);
        f.key(VK_HOME, 0);
        f.key(VK_LEFT, 0);
        assert_eq!(f.notes(), vec![(SLN_CHANGE, 0)], "nothing below home");
        // Up / Down are not a horizontal slider's: they go to the owner.
        assert_eq!(f.key(VK_UP, 0), 0);
        let wm = f.loader.shared.window_mgr.lock_or_recover();
        let mq = wm.get_mq(f.hmq).unwrap();
        assert!(mq.lock_or_recover().messages.iter().any(|m| m.msg == WM_CHAR && m.hwnd == f.client));
    }

    #[test]
    fn a_read_only_slider_takes_no_input() {
        let f = fixture(SLS_READONLY);
        let g = f.geometry();
        assert_eq!(g.arm_len, READONLY_ARM_LENGTH);
        f.send(WM_BUTTON1DOWN, f.at(g.shaft_t + 90, g.mid_c), 0);
        f.key(VK_RIGHT, 0);
        assert_eq!(f.notes(), vec![]);
        assert!(!f.loader.shared.window_mgr.lock_or_recover().can_take_focus(f.slider));
    }

    #[test]
    fn focus_notifications() {
        let f = fixture(0);
        f.loader.set_focus_window(f.slider);
        f.loader.set_focus_window(0);
        assert_eq!(f.notes(), vec![(SLN_SETFOCUS, f.slider), (SLN_KILLFOCUS, f.slider)]);
    }

    #[test]
    fn painting_draws_the_shaft_ribbon_and_arm() {
        let f = fixture(SLS_RIBBONSTRIP);
        f.send(SLM_SETSLIDERINFO, info(SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE), 3);
        let frame = f.loader.shared.window_mgr.lock_or_recover().find_frame_for_hwnd(f.slider);
        f.loader.shared.window_mgr.lock_or_recover().resize_surface(frame, 400, 300);
        f.send(WM_PAINT, 0, 0);
        let pel = |x: i32, y: i32| {
            let s = f.loader.shared.window_mgr.lock_or_recover().surfaces.get(&frame).unwrap().clone();
            let s = s.lock_or_recover();
            s.pel(x, y).unwrap()
        };
        let g = f.geometry();
        let (sx, sy) = (20, 20);
        assert_eq!(pel(sx + g.shaft_t + 10, sy + g.mid_c), RIBBON_COLOR, "ribbon from home");
        assert_eq!(pel(sx + g.arm_t(60) + g.arm_len + 10, sy + g.mid_c), CONTROL_FIELD_COLOR, "shaft past the arm");
        assert_eq!(pel(sx + g.arm_t(60) + 2, sy + g.mid_c + 8), CONTROL_FACE_COLOR, "the arm");
    }

    #[test]
    fn an_owner_drawn_slider_queues_its_parts() {
        let f = fixture(SLS_OWNERDRAW);
        f.send(WM_PAINT, 0, 0);
        let wm = f.loader.shared.window_mgr.lock_or_recover();
        let parts: Vec<u32> = wm.owner_calls.iter().filter_map(|c| match c.kind {
            crate::loader::pm_owner::OwnerCallKind::SliderPart { part } if c.control == f.slider => Some(part),
            _ => None,
        }).collect();
        assert_eq!(parts, vec![SDA_BACKGROUND, SDA_SLIDERSHAFT, SDA_SLIDERARM]);
        drop(wm);
        let g = f.geometry();
        let (x, y, w, h) = g.rect(g.shaft_t, g.shaft_len, g.shaft_c, g.shaft_b);
        assert_eq!(f.loader.slider_part_rect(f.slider, SDA_SLIDERSHAFT), Some((x, y, x + w, y + h)));
        assert_eq!(f.loader.slider_part_rect(f.slider, SDA_BACKGROUND), Some((0, 0, 300, 60)));
        assert_eq!(f.loader.slider_part_rect(f.slider, 99), None);
    }
}
