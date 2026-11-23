// SPDX-License-Identifier: GPL-3.0-or-later

//! The circular slider (`WC_CIRCULARSLIDER`, PMCTLS; ADR-0120): a dial
//! with a value in a range, turned with the mouse, the keyboard or its
//! − / + buttons.
//!
//! What the PM Reference documents (the `CSM_*` pages): `CSM_SETRANGE` /
//! `CSM_QUERYRANGE` (SHORT low and high), `CSM_SETVALUE` /
//! `CSM_QUERYVALUE` (SHORT), `CSM_QUERYRADIUS` (USHORT pels),
//! `CSM_SETINCREMENT` / `CSM_QUERYINCREMENT` (the step of the buttons, and
//! the tick marks to skip), `CSM_SETBITMAPDATA` (pictures for the
//! buttons, `CSBITMAPDATA`); each returns TRUE, FALSE on an error; the
//! range is what the control reports with `CSN_TRACKING` and `CSN_CHANGED`.
//!
//! TODO(unverified): everything else is Warpine's choice, as the reference
//! copies do not describe it: the look (the window text under the dial,
//! the value in its middle, a knob near its rim, tick marks around it, a −
//! and a + button at its lower corners); the defaults (range 0–100, value
//! 0, a step of 1, a tick every 10); the dial's sweep (270°, the low end at
//! lower left, turning clockwise; `CSS_360` a whole turn from the top);
//! `CSS_POINTSELECT` (a press moves the value to the pointer; without it
//! the dial follows a drag only), `CSS_MIDPOINT` (a longer tick at the
//! middle), `CSS_PROPORTIONALTICKS` (ticks as long as a sixth of the
//! radius), `CSS_CIRCULARVALUE` (with `CSS_360`, steps past an end wrap);
//! the keys (Left / Down and Right / Up step, Page Down / Page Up step by
//! the tick increment, Home / End go to the ends); the notifications'
//! mp2 (the new value as a SHORT; `CSN_SETFOCUS`: TRUE or FALSE), sent
//! after a change by the user or `CSM_SETVALUE`, `CSN_TRACKING` while the
//! dial is dragged; held − / + buttons repeat. `CSN_QUERYBACKGROUNDCOLOR`
//! is not sent. `CSM_SETBITMAPDATA`'s pictures replace the button glyphs,
//! stretched to the buttons.

use super::constants::*;
use super::pm_types::WindowManager;
use super::{control_box, control_text, ApiResult, Loader, MutexExt, Painter, CONTROL_BORDER_COLOR,
            CONTROL_EMPHASIS_COLOR, CONTROL_FACE_COLOR, CONTROL_LINE_H,
            CONTROL_PRESSED_COLOR, CONTROL_TEXT_COLOR};
use crate::gui::font;
use crate::gui::surface::{AreaFill, DrawOp, Mix, Pattern, Stroke, SOLID_LINE};

pub(super) const CLASS_CIRCSLIDER: &str = "WC_CIRCULARSLIDER";

/// Defaults of a new circular slider.
const DEFAULT_HIGH: i16 = 100;
const DEFAULT_SCROLL: u16 = 1;
const DEFAULT_TICK: u16 = 10;
/// Side of a − / + button (pels).
const BUTTON_SIZE: i32 = 12;
/// Room around the dial for the tick marks, and their usual length.
const TICK_ROOM: i32 = 6;
const TICK_LEN: i32 = 3;
/// Radius of the knob, and how far in from the rim its centre is.
const KNOB_R: i32 = 3;
const KNOB_INSET: i32 = 6;
/// The smallest dial.
const MIN_RADIUS: i32 = 8;
/// Corners of the polygons that draw circles.
const CIRCLE_POINTS: usize = 48;
/// At most this many tick marks are drawn.
const MAX_TICKS: i32 = 360;

/// A circular slider's state.
#[derive(Clone, Debug, PartialEq)]
pub struct CircState {
    pub low: i16,
    pub high: i16,
    pub value: i16,
    /// The buttons' and keys' step, and the tick marks' spacing.
    pub scroll: u16,
    pub tick: u16,
    pub held: Option<Held>,
    /// `CSBITMAPDATA` (left up, left down, right up, right down).
    pub bitmaps: [u32; 4],
}

impl Default for CircState {
    fn default() -> Self {
        CircState { low: 0, high: DEFAULT_HIGH, value: 0, scroll: DEFAULT_SCROLL, tick: DEFAULT_TICK,
                    held: None, bitmaps: [0; 4] }
    }
}

/// What button 1 holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// The dial, since the value was `start`.
    Dial { start: i16 },
    /// The − (`up` false) or + button, with the pointer still on it.
    Button { up: bool, over: bool },
}

impl CircState {
    /// `v` pulled into the range.
    fn clamp(&self, v: i32) -> i16 {
        v.clamp(self.low as i32, self.high as i32) as i16
    }

    /// The value `steps` steps of `size` from the current one: stopped at
    /// the ends, or wrapping round them for `wrap`.
    fn stepped(&self, steps: i32, size: u16, wrap: bool) -> i16 {
        let v = self.value as i32 + steps * size as i32;
        let (lo, hi) = (self.low as i32, self.high as i32);
        if wrap && (v < lo || v > hi) {
            (lo + (v - lo).rem_euclid(hi - lo + 1)) as i16
        } else {
            self.clamp(v)
        }
    }
}

/// Where the parts of a `cx` × `cy` circular slider are (PM coordinates,
/// its lower-left corner at 0, 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub center: (i32, i32),
    pub radius: i32,
    /// Height of the text line under the dial (0 with `CSS_NOTEXT`).
    pub text_h: i32,
    /// The − and + buttons (x, y, w, h); empty with `CSS_NOBUTTON`.
    pub minus: (i32, i32, i32, i32),
    pub plus: (i32, i32, i32, i32),
    /// The sweep (degrees) and the angle of the low end (degrees,
    /// counter-clockwise from the right); values go clockwise.
    pub sweep: f32,
    pub start: f32,
}

impl Geometry {
    pub fn new(style: u32, cx: i32, cy: i32) -> Geometry {
        let text_h = if style & CSS_NOTEXT == 0 { CONTROL_LINE_H } else { 0 };
        let dial_h = (cy - text_h).max(0);
        let radius = (cx.min(dial_h) / 2 - TICK_ROOM).max(MIN_RADIUS);
        let center = (cx / 2, text_h + dial_h / 2);
        let (minus, plus) = if style & CSS_NOBUTTON == 0 {
            ((0, text_h, BUTTON_SIZE, BUTTON_SIZE), (cx - BUTTON_SIZE, text_h, BUTTON_SIZE, BUTTON_SIZE))
        } else {
            ((0, 0, 0, 0), (0, 0, 0, 0))
        };
        let (sweep, start) = if style & CSS_360 != 0 { (360.0, 90.0) } else { (270.0, 225.0) };
        Geometry { center, radius, text_h, minus, plus, sweep, start }
    }

    /// The angle (degrees) of value `v` of `s`.
    pub fn angle(&self, s: &CircState, v: i16) -> f32 {
        let span = (s.high as f32 - s.low as f32).max(1.0);
        self.start - (v as f32 - s.low as f32) / span * self.sweep
    }

    /// A point at `dist` from the centre at `angle` degrees.
    pub fn at(&self, angle: f32, dist: f32) -> (i32, i32) {
        let a = angle.to_radians();
        ((self.center.0 as f32 + dist * a.cos()).round() as i32, (self.center.1 as f32 + dist * a.sin()).round() as i32)
    }

    /// The value of `s` the pointer at `(x, y)` points to. Outside a 270°
    /// sweep it is the nearer end.
    pub fn value_at(&self, s: &CircState, (x, y): (i32, i32)) -> i16 {
        let (dx, dy) = ((x - self.center.0) as f32, (y - self.center.1) as f32);
        let a = dy.atan2(dx).to_degrees();
        let mut d = (self.start - a).rem_euclid(360.0);
        if d > self.sweep {
            d = if d - self.sweep < 360.0 - d { self.sweep } else { 0.0 };
        }
        let span = s.high as f32 - s.low as f32;
        s.clamp((s.low as f32 + (d / self.sweep * span).round()) as i32)
    }

    /// What is at `(x, y)`.
    pub fn hit(&self, (x, y): (i32, i32)) -> Option<Hit> {
        let inside = |(bx, by, w, h): (i32, i32, i32, i32)| x >= bx && x < bx + w && y >= by && y < by + h;
        if inside(self.minus) {
            return Some(Hit::Button { up: false });
        }
        if inside(self.plus) {
            return Some(Hit::Button { up: true });
        }
        let (dx, dy) = (x - self.center.0, y - self.center.1);
        let r = self.radius + TICK_ROOM;
        (dx * dx + dy * dy <= r * r).then_some(Hit::Dial)
    }
}

/// A part of the control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Dial,
    Button { up: bool },
}

/// Signed position of a mouse message's mp1.
fn mouse_pos(mp1: u32) -> (i32, i32) {
    ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32)
}

impl WindowManager {
    /// The state of circular slider `hwnd`, made on first use.
    fn circ_mut(&mut self, hwnd: u32) -> Option<&mut CircState> {
        let w = self.get_window_mut(hwnd).filter(|w| w.class_name == CLASS_CIRCSLIDER)?;
        Some(w.circslider.get_or_insert_with(Box::default))
    }
}

impl Loader {
    /// The style, size and state of circular slider `hwnd`.
    fn circ_info(&self, hwnd: u32) -> Option<(u32, i32, i32, CircState)> {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let s = wm.circ_mut(hwnd)?.clone();
        let w = wm.get_window(hwnd)?;
        Some((w.style, w.cx, w.cy, s))
    }

    fn update_circ<R>(&self, hwnd: u32, f: impl FnOnce(&mut CircState) -> R) -> Option<R> {
        self.shared.window_mgr.lock_or_recover().circ_mut(hwnd).map(f)
    }

    /// Set the value of `hwnd` to `v` (pulled into the range); if it
    /// changed, redraw and tell the owner `code` with the new value.
    fn circ_set(&self, hwnd: u32, v: i32, code: u32) -> bool {
        let changed = self.update_circ(hwnd, |s| {
            let v = s.clamp(v);
            (v != s.value).then(|| { s.value = v; v })
        }).flatten();
        let Some(v) = changed else { return false };
        self.repaint_control(hwnd);
        self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, code, v as u16 as u32);
        true
    }

    /// Write `v` as a SHORT to guest address `addr` (none for 0).
    fn put_short(&self, addr: u32, v: u16) -> bool {
        addr == 0 || self.guest_write::<u16>(addr, v).is_some()
    }

    /// A message to circular slider `hwnd`; `None` for the default
    /// processing.
    pub(super) fn circslider_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let (style, cx, cy, s) = self.circ_info(hwnd)?;
        let g = Geometry::new(style, cx, cy);
        let ok = |b: bool| ApiResult::Normal(b as u32);
        Some(match msg {
            CSM_SETRANGE => {
                let (low, high) = (mp1 as u16 as i16, mp2 as u16 as i16);
                if low >= high {
                    return Some(ok(false));
                }
                self.update_circ(hwnd, |s| { s.low = low; s.high = high; s.value = s.clamp(s.value as i32); });
                self.repaint_control(hwnd);
                ok(true)
            }
            CSM_QUERYRANGE => ok(self.put_short(mp1, s.low as u16) && self.put_short(mp2, s.high as u16)),
            CSM_SETVALUE => {
                let v = mp1 as u16 as i16 as i32;
                if v < s.low as i32 || v > s.high as i32 {
                    return Some(ok(false));
                }
                self.circ_set(hwnd, v, CSN_CHANGED);
                ok(true)
            }
            CSM_QUERYVALUE => ok(self.put_short(mp1, s.value as u16)),
            CSM_QUERYRADIUS => ok(self.put_short(mp1, g.radius as u16)),
            CSM_SETINCREMENT => {
                let (scroll, tick) = (mp1 as u16, mp2 as u16);
                if scroll == 0 {
                    return Some(ok(false));
                }
                self.update_circ(hwnd, |s| { s.scroll = scroll; s.tick = tick; });
                self.repaint_control(hwnd);
                ok(true)
            }
            CSM_QUERYINCREMENT => ok(self.put_short(mp1, s.scroll) && self.put_short(mp2, s.tick)),
            CSM_SETBITMAPDATA => {
                if mp1 == 0 {
                    return Some(ok(false));
                }
                let mut b = [0u32; 4];
                for (i, slot) in b.iter_mut().enumerate() {
                    let Some(h) = self.guest_read::<u32>(mp1 + 4 * i as u32) else { return Some(ok(false)) };
                    *slot = h;
                }
                self.update_circ(hwnd, |s| s.bitmaps = b);
                ok(true)
            }
            WM_BUTTON1DOWN | WM_BUTTON1DBLCLK => ok(self.circ_press(hwnd, style, &s, &g, mouse_pos(mp1))),
            WM_MOUSEMOVE => {
                if !self.circ_drag(hwnd, &s, &g, mouse_pos(mp1)) { return None; }
                ok(true)
            }
            WM_BUTTON1UP => ok(self.circ_release(hwnd)),
            // A held button steps again (WindowManager::post_drag_scroll).
            WM_TIMER if mp1 == TID_SCROLL => ok(self.circ_repeat(hwnd, style, &s)),
            WM_CHAR => ok(self.circ_char(hwnd, style, &s, mp1, mp2)),
            _ => return None,
        })
    }

    /// Button 1 pressed on the control.
    fn circ_press(&self, hwnd: u32, style: u32, s: &CircState, g: &Geometry, at: (i32, i32)) -> bool {
        if style & WS_DISABLED != 0 {
            return false;
        }
        let held = match g.hit(at) {
            Some(Hit::Button { up }) => Held::Button { up, over: true },
            Some(Hit::Dial) => Held::Dial { start: s.value },
            None => return true,
        };
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            wm.pressed_control = hwnd;
            if let Some(c) = wm.circ_mut(hwnd) {
                c.held = Some(held);
            }
        }
        match held {
            Held::Button { up, .. } => {
                let wrap = style & (CSS_360 | CSS_CIRCULARVALUE) == CSS_360 | CSS_CIRCULARVALUE;
                let v = s.stepped(if up { 1 } else { -1 }, s.scroll, wrap);
                if !self.circ_set(hwnd, v as i32, CSN_CHANGED) {
                    self.repaint_control(hwnd);
                }
            }
            Held::Dial { .. } if style & CSS_POINTSELECT != 0 => {
                self.circ_set(hwnd, g.value_at(s, at) as i32, CSN_TRACKING);
            }
            Held::Dial { .. } => {}
        }
        true
    }

    /// The pointer moved; while the dial is held it follows the pointer.
    fn circ_drag(&self, hwnd: u32, s: &CircState, g: &Geometry, at: (i32, i32)) -> bool {
        if self.shared.window_mgr.lock_or_recover().pressed_control != hwnd {
            return false;
        }
        match s.held {
            Some(Held::Dial { .. }) => { self.circ_set(hwnd, g.value_at(s, at) as i32, CSN_TRACKING); }
            Some(Held::Button { up, over }) => {
                let now = g.hit(at) == Some(Hit::Button { up });
                if now != over {
                    self.update_circ(hwnd, |c| c.held = Some(Held::Button { up, over: now }));
                    self.repaint_control(hwnd);
                }
            }
            None => {}
        }
        true
    }

    /// `WM_TIMER(TID_SCROLL)` while a button is held with the pointer on
    /// it: one more step.
    fn circ_repeat(&self, hwnd: u32, style: u32, s: &CircState) -> bool {
        let Some(Held::Button { up, over: true }) = s.held else { return false };
        if self.shared.window_mgr.lock_or_recover().pressed_control != hwnd {
            return false;
        }
        let wrap = style & (CSS_360 | CSS_CIRCULARVALUE) == CSS_360 | CSS_CIRCULARVALUE;
        let v = s.stepped(if up { 1 } else { -1 }, s.scroll, wrap);
        self.circ_set(hwnd, v as i32, CSN_CHANGED)
    }

    /// Button 1 released: a turned dial tells its owner `CSN_CHANGED`.
    fn circ_release(&self, hwnd: u32) -> bool {
        let (held, value) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if wm.pressed_control == hwnd {
                wm.pressed_control = 0;
            }
            let Some(c) = wm.circ_mut(hwnd) else { return false };
            (c.held.take(), c.value)
        };
        match held {
            Some(Held::Dial { start }) => {
                if value != start {
                    self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, CSN_CHANGED, value as u16 as u32);
                }
                true
            }
            Some(Held::Button { .. }) => { self.repaint_control(hwnd); true }
            None => false,
        }
    }

    /// A key: Left / Down and Right / Up step, Page Down / Page Up step by
    /// the tick increment, Home / End go to the ends; other keys go on to
    /// the owner.
    fn circ_char(&self, hwnd: u32, style: u32, s: &CircState, mp1: u32, mp2: u32) -> bool {
        let fs = mp1 & 0xFFFF;
        let vk = if fs & KC_VIRTUALKEY != 0 { mp2 >> WM_CHAR_VKEY_SHIFT } else { 0 };
        let wrap = style & (CSS_360 | CSS_CIRCULARVALUE) == CSS_360 | CSS_CIRCULARVALUE;
        let target = match vk {
            VK_LEFT | VK_DOWN => Some(s.stepped(-1, s.scroll, wrap)),
            VK_RIGHT | VK_UP => Some(s.stepped(1, s.scroll, wrap)),
            VK_PAGEDOWN => Some(s.stepped(-1, s.tick.max(1), wrap)),
            VK_PAGEUP => Some(s.stepped(1, s.tick.max(1), wrap)),
            VK_HOME => Some(s.low),
            VK_END => Some(s.high),
            _ => None,
        };
        let usable = style & WS_DISABLED == 0 && fs & (KC_CTRL | KC_ALT) == 0;
        let Some(v) = target.filter(|_| usable) else {
            let owner = self.shared.window_mgr.lock_or_recover().notify_target(hwnd);
            if owner != 0 && owner != hwnd {
                self.post_control_message(owner, WM_CHAR, mp1, mp2);
            }
            return false;
        };
        if fs & KC_KEYUP == 0 {
            self.circ_set(hwnd, v as i32, CSN_CHANGED);
        }
        true
    }

    /// WC_CIRCULARSLIDER: the dial with its ticks, knob and value, the − / +
    /// buttons and the window text under it.
    pub(super) fn paint_circslider(&self, p: &Painter, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32), text: &str) {
        let Some((style, _, _, s)) = self.circ_info(hwnd) else { return };
        let g = Geometry::new(style, cx, cy);
        let focused = self.shared.window_mgr.lock_or_recover().focus_hwnd == hwnd;
        p.fill((x, y, x + cx - 1, y + cy - 1), self.control_background(hwnd));
        let at = |a: f32, d: f32| { let (px, py) = g.at(a, d); (x + px, y + py) };
        let circle = |r: f32| (0..CIRCLE_POINTS).map(|i| at(i as f32 * 360.0 / CIRCLE_POINTS as f32, r)).collect::<Vec<_>>();
        let shape = |points: Vec<(i32, i32)>, fill: Option<u32>, edge: Option<(u32, u32)>| DrawOp::Shape {
            points, closed: true,
            fill: fill.map(|c| AreaFill { pattern: Pattern::solid(c), mix: Mix::Overpaint, back_mix: None }),
            stroke: edge.map(|(color, width)| Stroke { color, mix: Mix::Overpaint, style: SOLID_LINE, width }),
        };
        // The dial.
        let edge = if focused { CONTROL_EMPHASIS_COLOR } else { CONTROL_BORDER_COLOR };
        p.op(shape(circle(g.radius as f32), Some(CONTROL_FACE_COLOR), Some((edge, if focused { 2 } else { 1 }))));
        // Tick marks every `tick` values (the first and last always).
        if style & CSS_NOTICKS == 0 && s.tick > 0 {
            let span = s.high as i32 - s.low as i32;
            let mid = s.low as i32 + span / 2;
            let len = if style & CSS_PROPORTIONALTICKS != 0 { (g.radius / 6).max(TICK_LEN) } else { TICK_LEN };
            let values = (s.low as i32..=s.high as i32).step_by(s.tick as usize).take(MAX_TICKS as usize)
                .chain(std::iter::once(s.high as i32));
            for v in values {
                let a = g.angle(&s, v as i16);
                let l = if style & CSS_MIDPOINT != 0 && v == mid { len * 2 } else { len };
                let (x1, y1) = at(a, g.radius as f32 + 2.0);
                let (x2, y2) = at(a, (g.radius + 1 + l) as f32);
                p.op(DrawOp::Line { x1, y1, x2, y2, color: CONTROL_TEXT_COLOR, mix: Mix::Overpaint, style: SOLID_LINE });
            }
        }
        // The knob, and the value in the middle.
        let knob = CONTROL_TEXT_COLOR;
        let (kx, ky) = g.at(g.angle(&s, s.value), (g.radius - KNOB_INSET) as f32);
        let knob_points = (0..CIRCLE_POINTS / 3).map(|i| {
            let a = (i as f32 * 360.0 / (CIRCLE_POINTS / 3) as f32).to_radians();
            (x + kx + (KNOB_R as f32 * a.cos()).round() as i32, y + ky + (KNOB_R as f32 * a.sin()).round() as i32)
        }).collect();
        p.op(shape(knob_points, Some(knob), None));
        if style & CSS_NONUMBER == 0 {
            let num = s.value.to_string();
            let w = font::text_width(&num, p.font());
            control_text(p, x + g.center.0 - w / 2, y + g.center.1 - CONTROL_LINE_H / 2, &num);
        }
        // The − and + buttons.
        for (rect, up) in [(g.minus, false), (g.plus, true)] {
            let (bx, by, bw, bh) = rect;
            if bw <= 0 { continue; }
            let pressed = s.held == Some(Held::Button { up, over: true });
            control_box(p, (x + bx, y + by, bw - 1, bh - 1), if pressed { CONTROL_PRESSED_COLOR } else { CONTROL_FACE_COLOR });
            // CSM_SETBITMAPDATA's picture for this button and state,
            // stretched to it (the reference: "other bit maps are
            // stretched"), else the − / + glyph.
            let hbm = s.bitmaps[usize::from(up) * 2 + usize::from(pressed)];
            if hbm != 0 && self.paint_bitmap_handle(p, (x + bx + 1, y + by + 1, bw - 2, bh - 2), hbm, true) {
                continue;
            }
            let (mx, my) = (x + bx + bw / 2, y + by + bh / 2);
            p.fill((mx - 3, my, mx + 3, my), CONTROL_TEXT_COLOR);
            if up {
                p.fill((mx, my - 3, mx, my + 3), CONTROL_TEXT_COLOR);
            }
        }
        // The window text, centred under the dial.
        if g.text_h > 0 && !text.is_empty() {
            let label = font::fit_text(text, cx, p.font());
            let w = font::text_width(label, p.font());
            control_text(p, x + (cx - w) / 2, y, label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(low: i16, high: i16, value: i16) -> CircState {
        CircState { low, high, value, ..CircState::default() }
    }

    #[test]
    fn a_held_button_repeats_while_the_pointer_is_on_it() {
        let l = Loader::new_mock();
        let h = {
            let mut wm = l.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            let h = wm.create_window(CLASS_CIRCSLIDER.into(), 0, hmq);
            let w = wm.get_window_mut(h).unwrap();
            (w.cx, w.cy) = (120, 100 + CONTROL_LINE_H);
            h
        };
        let g = Geometry::new(0, 120, 100 + CONTROL_LINE_H);
        let at = |(x, y): (i32, i32)| (x as u16 as u32) | ((y as u16 as u32) << 16);
        let plus = at((g.plus.0 + 2, g.plus.1 + 2));
        let value = |l: &Loader| l.circ_info(h).unwrap().3.value;
        l.circslider_message(h, WM_BUTTON1DOWN, plus, 0);
        assert_eq!(value(&l), 1, "the press steps once");
        assert_eq!(l.shared.window_mgr.lock_or_recover().post_drag_scroll(), Some(h), "held: it repeats");
        l.circslider_message(h, WM_TIMER, TID_SCROLL, 0);
        l.circslider_message(h, WM_TIMER, TID_SCROLL, 0);
        assert_eq!(value(&l), 3);
        l.circslider_message(h, WM_MOUSEMOVE, at(g.center), 0);
        assert_eq!(l.shared.window_mgr.lock_or_recover().post_drag_scroll(), None, "not while off the button");
        l.circslider_message(h, WM_BUTTON1UP, at(g.center), 0);
        l.circslider_message(h, WM_TIMER, TID_SCROLL, 0);
        assert_eq!(value(&l), 3, "released: no more steps");
    }

    #[test]
    fn a_new_slider_has_the_default_range_and_steps() {
        let s = CircState::default();
        assert_eq!((s.low, s.high, s.value, s.scroll, s.tick), (0, 100, 0, 1, 10));
    }

    #[test]
    fn steps_stop_at_the_ends_or_wrap() {
        let s = state(0, 10, 9);
        assert_eq!(s.stepped(1, 1, false), 10);
        assert_eq!(s.stepped(1, 5, false), 10, "stopped at the high end");
        assert_eq!(s.stepped(1, 5, true), 3, "wrapped round: 9 + 5 = 14, 11 values");
        assert_eq!(state(0, 10, 1).stepped(-1, 5, true), 7);
        assert_eq!(state(0, 10, 1).stepped(-1, 5, false), 0);
    }

    #[test]
    fn the_dial_sweeps_270_degrees_clockwise_from_lower_left() {
        let g = Geometry::new(0, 100, 100 + CONTROL_LINE_H);
        let s = state(0, 100, 0);
        assert_eq!(g.angle(&s, 0), 225.0, "low at lower left");
        assert_eq!(g.angle(&s, 50), 90.0, "the middle at the top");
        assert_eq!(g.angle(&s, 100), -45.0, "high at lower right");
        let (cx, cy) = g.center;
        assert_eq!(g.value_at(&s, (cx, cy + 20)), 50, "straight up");
        assert_eq!(g.value_at(&s, (cx + 20, cy)), 83, "right: 135 of 270 degrees");
        assert_eq!(g.value_at(&s, (cx - 1, cy - 20)), 0, "below, left of the gap: the low end");
        assert_eq!(g.value_at(&s, (cx + 1, cy - 20)), 100, "below, right of it: the high end");
    }

    #[test]
    fn a_360_dial_starts_at_the_top() {
        let g = Geometry::new(CSS_360, 100, 100);
        let s = state(0, 360, 0);
        assert_eq!(g.angle(&s, 0), 90.0);
        let (cx, cy) = g.center;
        assert_eq!(g.value_at(&s, (cx + 20, cy)), 90, "a quarter turn clockwise");
        assert_eq!(g.value_at(&s, (cx, cy - 20)), 180);
    }

    #[test]
    fn parts_are_found_by_position() {
        let g = Geometry::new(0, 120, 100 + CONTROL_LINE_H);
        assert_eq!(g.hit((2, CONTROL_LINE_H + 2)), Some(Hit::Button { up: false }));
        assert_eq!(g.hit((117, CONTROL_LINE_H + 2)), Some(Hit::Button { up: true }));
        assert_eq!(g.hit(g.center), Some(Hit::Dial));
        assert_eq!(g.hit((60, 1)), None, "the text line");
        let nb = Geometry::new(CSS_NOBUTTON | CSS_NOTEXT, 120, 100);
        assert_eq!(nb.text_h, 0);
        assert_eq!(nb.hit((2, 2)), None, "no buttons");
        assert_eq!(nb.radius, 50 - TICK_ROOM);
    }
}
