// SPDX-License-Identifier: GPL-3.0-only

//! Spin buttons (`WC_SPINBUTTON`, `<pmstddlg.h>`): a spin field — the
//! built-in entry field of `pm_controls.rs`, a child of the spin button —
//! and, for a master (`SPBS_MASTER`), up and down arrows right of it.
//!
//! The values are a numeric range (`SPBM_SETLIMITS`) or an array of strings
//! (`SPBM_SETARRAY`); the current value is a number or an array index
//! (`SPBM_SETCURRENTVALUE`). Spinning (`SPBM_SPINUP` / `SPBM_SPINDOWN`, the
//! arrows, Up / Down in the field) moves through them and wraps round at the
//! ends (unverified: the references do not say what happens at a limit).
//! `SPBM_QUERYVALUE` reads the field back: a number in range or an item of
//! the array (compared without case — unverified) is valid and, with
//! `SPBQ_UPDATEIFVALID` / `SPBQ_ALWAYSUPDATE`, becomes the current value;
//! `SPBQ_ALWAYSUPDATE` puts the last valid value back into a field holding
//! something invalid.
//!
//! A servant (`SPBS_SERVANT`, no arrows) joins a master with
//! `SPBM_SETMASTER`; the master's arrows spin the member of the group that
//! last had the focus (the master itself at first). `SPBM_SETMASTER(NULLHANDLE)`
//! stops a spin button spinning on its own: it still sends its
//! notifications, and the application sets its values.
//!
//! Styles: `SPBS_NUMERICONLY` (the field takes digits and, first, a minus
//! sign), `SPBS_READONLY`, `SPBS_JUSTLEFT` / `JUSTRIGHT` / `JUSTCENTER`
//! (the field's `ES_LEFT` / `ES_RIGHT` / `ES_CENTER`), `SPBS_PADWITHZEROS`
//! (numbers padded with zeros to the text limit, `SPBM_SETTEXTLIMIT`),
//! `SPBS_FASTSPIN` (a held arrow steps twice as far every
//! `FAST_SPIN_REPEATS` repeats, up to `FAST_SPIN_MAX_STEP` — Warpine's
//! own rates). `SPBS_NOBORDER` is accepted and not drawn differently.
//!
//! Notifications (`WM_CONTROL` to the owner, from the spin button):
//! `SPBN_UPARROW` / `SPBN_DOWNARROW` when an arrow is pressed or Up / Down
//! typed and `SPBN_ENDSPIN` when it is let go (from the master, mp2 = the
//! member spun), `SPBN_CHANGE` when the field's text changes (mp2 = the spin
//! button), `SPBN_SETFOCUS` (mp2 = the spin button) and `SPBN_KILLFOCUS`
//! (mp2 = NULLHANDLE).

use super::constants::*;
use super::pm_types::WindowManager;
use super::{control_box, ApiResult, Loader, MutexExt, Painter, CONTROL_FACE_COLOR,
            CONTROL_PRESSED_COLOR, CONTROL_TEXT_COLOR};

pub(super) const CLASS_SPINBUTTON: &str = "WC_SPINBUTTON";

/// Width of a master's arrows.
pub(super) const SPIN_ARROW_W: i32 = 16;
/// Most strings `SPBM_SETARRAY` takes.
pub(super) const MAX_SPIN_ITEMS: usize = 4096;
/// Longest string of a spin array, in bytes.
const MAX_SPIN_ITEM_LEN: usize = 256;
/// `SPBS_FASTSPIN`: repeats of a held arrow before its step doubles.
const FAST_SPIN_REPEATS: u32 = 10;
/// `SPBS_FASTSPIN`: the largest step of a held arrow.
const FAST_SPIN_MAX_STEP: i64 = 64;

/// The values of a spin button.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SpinValues {
    /// No values yet: nothing to spin.
    #[default]
    None,
    /// The numbers `low..=high`.
    Range { low: i32, high: i32 },
    /// An array of strings, spun by index.
    Array(Vec<String>),
}

/// An arrow of a master.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrow {
    Up,
    Down,
}

/// A spin button's values and state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpinState {
    /// The spin field (an entry field).
    pub entry: u32,
    pub values: SpinValues,
    /// The current number or array index.
    pub current: i32,
    /// The master of a servant (`SPBM_SETMASTER`); 0 for none.
    pub master: u32,
    /// Spins by itself (cleared by `SPBM_SETMASTER(NULLHANDLE)`).
    pub auto: bool,
    /// The member of a master's group that last had the focus.
    pub selected: u32,
    /// The arrow held down with the pointer.
    pub held: Option<Arrow>,
    /// How often the held arrow repeated.
    pub repeats: u32,
}

impl SpinState {
    /// The text of value `v` (a number or an index) with `pad` digits.
    fn text_of(&self, v: i32, pad: usize) -> Option<String> {
        match &self.values {
            SpinValues::None => None,
            SpinValues::Range { .. } if pad > 0 => Some(if v < 0 {
                format!("-{:0width$}", v.unsigned_abs(), width = pad.saturating_sub(1))
            } else {
                format!("{v:0pad$}")
            }),
            SpinValues::Range { .. } => Some(v.to_string()),
            SpinValues::Array(items) => usize::try_from(v).ok().and_then(|i| items.get(i).cloned()),
        }
    }

    /// The value `text` stands for, if it is valid.
    pub fn parse(&self, text: &str) -> Option<i32> {
        match &self.values {
            SpinValues::None => None,
            SpinValues::Range { low, high } => text.trim().parse::<i32>().ok().filter(|v| (*low..=*high).contains(v)),
            SpinValues::Array(items) => {
                let t = text.trim().to_lowercase();
                items.iter().position(|s| s.to_lowercase() == t).map(|i| i as i32)
            }
        }
    }

    /// `current` moved `steps` up (negative: down), wrapping round.
    pub fn spun(&self, steps: i64) -> Option<i32> {
        let (low, high) = match &self.values {
            SpinValues::None => return None,
            SpinValues::Range { low, high } => (*low as i64, *high as i64),
            SpinValues::Array(items) if items.is_empty() => return None,
            SpinValues::Array(items) => (0, items.len() as i64 - 1),
        };
        let span = high - low + 1;
        Some((low + (self.current as i64 - low + steps).rem_euclid(span)) as i32)
    }

    /// `v` pulled into the values' range.
    fn clamped(&self, v: i32) -> i32 {
        match &self.values {
            SpinValues::None => v,
            SpinValues::Range { low, high } => v.clamp(*low, *high),
            SpinValues::Array(items) => v.clamp(0, items.len().saturating_sub(1) as i32),
        }
    }

    /// Whether `v` is one of the values.
    fn holds(&self, v: i32) -> bool {
        match &self.values {
            SpinValues::None => false,
            SpinValues::Range { low, high } => (*low..=*high).contains(&v),
            SpinValues::Array(items) => usize::try_from(v).is_ok_and(|i| i < items.len()),
        }
    }
}

/// Where the parts of a spin button `cx` × `cy` of `style` go, relative to
/// it: `(field, up arrow, down arrow)` as `(x, y, cx, cy)`; the arrows are
/// empty for a servant.
pub(super) fn spin_layout(style: u32, cx: i32, cy: i32) -> [(i32, i32, i32, i32); 3] {
    let aw = if style & SPBS_MASTER != 0 { SPIN_ARROW_W.min(cx.max(0)) } else { 0 };
    let half = cy / 2;
    [(0, 0, cx - aw, cy), (cx - aw, half, aw, cy - half), (cx - aw, 0, aw, half)]
}

/// The entry field style for a spin button of `style`.
fn field_style(style: u32) -> u32 {
    let just = match style & SPBS_JUSTCENTER {
        SPBS_JUSTRIGHT => ES_RIGHT,
        SPBS_JUSTCENTER => ES_CENTER,
        _ => 0,
    };
    WS_VISIBLE | (style & WS_TABSTOP) | just | if style & SPBS_READONLY != 0 { ES_READONLY } else { 0 }
}

impl WindowManager {
    /// Whether `hwnd` is a spin button.
    pub fn is_spin(&self, hwnd: u32) -> bool {
        self.get_window(hwnd).is_some_and(|w| w.class_name == CLASS_SPINBUTTON)
    }

    /// The spin button whose field `hwnd` is.
    pub fn spin_of(&self, hwnd: u32) -> Option<u32> {
        let parent = self.get_window(hwnd)?.parent;
        let s = self.get_window(parent)?.spin.as_ref()?;
        (s.entry == hwnd).then_some(parent)
    }

    /// Create the spin field of new spin button `hwnd` and place it.
    pub fn build_spin(&mut self, hwnd: u32) {
        let Some((style, hmq, id, has)) = self.get_window(hwnd).map(|w| (w.style, w.hmq, w.id, w.spin.is_some()))
        else { return };
        if has { return; }
        let entry = self.create_window("WC_ENTRYFIELD".into(), hwnd, hmq);
        if let Some(w) = self.get_window_mut(entry) {
            (w.id, w.style, w.owner, w.visible) = (id, field_style(style), hwnd, true);
        }
        if let Some(w) = self.get_window_mut(hwnd) {
            w.spin = Some(Box::new(SpinState { entry, auto: true, selected: hwnd, ..Default::default() }));
        }
        self.layout_spin(hwnd);
    }

    /// Place the field of spin button `hwnd` for its size.
    pub fn layout_spin(&mut self, hwnd: u32) {
        let Some((style, cx, cy, entry)) = self.get_window(hwnd)
            .and_then(|w| Some((w.style, w.cx, w.cy, w.spin.as_ref()?.entry))) else { return };
        let [(x, y, fcx, fcy), ..] = spin_layout(style, cx, cy);
        if let Some(w) = self.get_window_mut(entry) {
            (w.x, w.y, w.cx, w.cy) = (x, y, fcx, fcy);
        }
    }

    fn spin_state(&self, hwnd: u32) -> Option<&SpinState> {
        self.get_window(hwnd)?.spin.as_deref()
    }

    fn spin_state_mut(&mut self, hwnd: u32) -> Option<&mut SpinState> {
        self.get_window_mut(hwnd)?.spin.as_deref_mut()
    }
}

/// Signed position of a mouse message's mp1.
fn mouse_pos(mp1: u32) -> (i32, i32) {
    ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32)
}

impl Loader {
    /// Messages to spin button `hwnd`; `None` for those it leaves to the
    /// generic handling.
    pub(super) fn spin_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let (style, cx, cy) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let w = wm.get_window(hwnd)?;
            w.spin.as_ref()?;
            (w.style, w.cx, w.cy)
        };
        let ok = |b: bool| ApiResult::Normal(b as u32);
        Some(match msg {
            WM_BUTTON1DOWN | WM_BUTTON1DBLCLK => ok(self.spin_press(hwnd, style, (cx, cy), mouse_pos(mp1))),
            WM_MOUSEMOVE => {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if wm.pressed_control == hwnd {
                    wm.drag_pointer = mouse_pos(mp1);
                }
                ok(true)
            }
            WM_BUTTON1UP => ok(self.spin_release(hwnd)),
            WM_TIMER if mp1 == TID_SCROLL => ok(self.spin_repeat(hwnd, style, (cx, cy))),
            WM_CONTROL => {
                self.spin_child_notify(hwnd, mp1 >> 16);
                ApiResult::Normal(0)
            }
            WM_CHAR => ok(self.spin_char(hwnd, mp1, mp2)),
            SPBM_SETLIMITS | SPBM_OVERRIDESETLIMITS => {
                let (high, low) = (mp1 as i32, mp2 as i32);
                if high < low {
                    return Some(ok(false));
                }
                let keep = msg == SPBM_OVERRIDESETLIMITS;
                self.spin_set_values(hwnd, SpinValues::Range { low, high }, keep);
                ok(true)
            }
            SPBM_QUERYLIMITS => {
                let range = match self.shared.window_mgr.lock_or_recover().spin_state(hwnd).map(|s| s.values.clone()) {
                    Some(SpinValues::Range { low, high }) => Some((high, low)),
                    _ => None,
                };
                let Some((high, low)) = range else { return Some(ok(false)) };
                let w1 = mp1 == 0 || self.guest_write::<i32>(mp1, high).is_some();
                let w2 = mp2 == 0 || self.guest_write::<i32>(mp2, low).is_some();
                ok(w1 && w2)
            }
            SPBM_SETARRAY => {
                let count = (mp2 & 0xFFFF) as usize;
                if mp1 == 0 || count == 0 || count > MAX_SPIN_ITEMS {
                    return Some(ok(false));
                }
                let cp = self.window_codepage(hwnd);
                let mut items = Vec::with_capacity(count);
                for i in 0..count as u32 {
                    let Some(p) = self.guest_read::<u32>(mp1.wrapping_add(4 * i)) else { return Some(ok(false)) };
                    let mut s = if p != 0 { self.read_guest_string_cp(p, cp) } else { String::new() };
                    if s.len() > MAX_SPIN_ITEM_LEN {
                        let mut end = MAX_SPIN_ITEM_LEN;
                        while !s.is_char_boundary(end) { end -= 1; }
                        s.truncate(end);
                    }
                    items.push(s);
                }
                self.spin_set_values(hwnd, SpinValues::Array(items), false);
                ok(true)
            }
            SPBM_SETCURRENTVALUE => {
                let v = mp1 as i32;
                let holds = self.shared.window_mgr.lock_or_recover().spin_state(hwnd).is_some_and(|s| s.holds(v));
                if holds {
                    self.spin_set_current(hwnd, v);
                }
                ok(holds)
            }
            SPBM_QUERYVALUE => ok(self.spin_query_value(hwnd, mp1, mp2)),
            SPBM_SPINUP | SPBM_SPINDOWN => {
                let n = mp1 as i64;
                ok(self.spin_by(hwnd, if msg == SPBM_SPINUP { n } else { -n }))
            }
            SPBM_SETTEXTLIMIT => {
                let entry = self.shared.window_mgr.lock_or_recover().spin_state(hwnd).map_or(0, |s| s.entry);
                if let Some(w) = self.shared.window_mgr.lock_or_recover().get_window_mut(entry) {
                    w.text_limit = (mp1 & 0xFFFF) as usize;
                }
                self.spin_show_current(hwnd);
                ok(true)
            }
            SPBM_SETMASTER => {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let valid = mp1 == 0 || wm.is_spin(mp1);
                if valid && let Some(s) = wm.spin_state_mut(hwnd) {
                    s.master = mp1;
                    s.auto = mp1 != 0;
                }
                ok(valid)
            }
            _ => return None,
        })
    }

    /// New values for spin button `hwnd`; the current value is the field's
    /// when it holds one, else pulled into range. With `keep` the current
    /// value and the field stay as they are.
    fn spin_set_values(&self, hwnd: u32, values: SpinValues, keep: bool) {
        let cur = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(entry) = wm.spin_state(hwnd).map(|s| s.entry) else { return };
            let text = wm.get_window(entry).map(|w| w.text.clone()).unwrap_or_default();
            let Some(s) = wm.spin_state_mut(hwnd) else { return };
            s.values = values;
            if keep {
                return;
            }
            s.parse(&text).unwrap_or_else(|| s.clamped(s.current))
        };
        self.spin_set_current(hwnd, cur);
    }

    /// Make `v` the current value of spin button `hwnd` and show it.
    fn spin_set_current(&self, hwnd: u32, v: i32) {
        if let Some(s) = self.shared.window_mgr.lock_or_recover().spin_state_mut(hwnd) {
            s.current = v;
        }
        self.spin_show_current(hwnd);
    }

    /// Put the current value of spin button `hwnd` into its field; the
    /// owner is told `SPBN_CHANGE` when the text changed.
    fn spin_show_current(&self, hwnd: u32) {
        let changed = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(style) = wm.get_window(hwnd).map(|w| w.style) else { return };
            let Some(s) = wm.spin_state(hwnd) else { return };
            let entry = s.entry;
            let pad = if style & SPBS_PADWITHZEROS != 0 {
                wm.get_window(entry).map_or(0, |w| w.text_limit)
            } else { 0 };
            let Some(text) = s.text_of(s.current, pad) else { return };
            let Some(e) = wm.get_window_mut(entry) else { return };
            if e.text == text {
                return;
            }
            let n = text.chars().count();
            e.text = text;
            (e.edit_anchor, e.edit_caret, e.edit_scroll) = (n, n, 0);
            entry
        };
        self.repaint_control(changed);
        self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, SPBN_CHANGE, hwnd);
    }

    /// Spin `hwnd` `steps` values on (negative: back). FALSE when there is
    /// nothing to spin.
    fn spin_by(&self, hwnd: u32, steps: i64) -> bool {
        let next = self.shared.window_mgr.lock_or_recover().spin_state(hwnd).and_then(|s| s.spun(steps));
        match next {
            Some(v) => { self.spin_set_current(hwnd, v); true }
            None => false,
        }
    }

    /// `SPBM_QUERYVALUE`: mp1 = storage (a LONG when the buffer size is 0,
    /// else a buffer), mp2 = MPFROM2SHORT(buffer size, `SPBQ_*`).
    fn spin_query_value(&self, hwnd: u32, storage: u32, mp2: u32) -> bool {
        let size = (mp2 & 0xFFFF) as usize;
        let mode = mp2 >> 16;
        let (valid, entry) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(entry) = wm.spin_state(hwnd).map(|s| s.entry) else { return false };
            let text = wm.get_window(entry).map(|w| w.text.clone()).unwrap_or_default();
            let Some(s) = wm.spin_state_mut(hwnd) else { return false };
            let valid = s.parse(&text);
            if let Some(v) = valid && mode != SPBQ_DONOTUPDATE {
                s.current = v;
            }
            (valid, entry)
        };
        if mode != SPBQ_DONOTUPDATE && (valid.is_some() || mode == SPBQ_ALWAYSUPDATE) {
            // The field shows the value it stands for (or the last valid one).
            self.spin_show_current(hwnd);
        }
        if storage != 0 {
            let (cur, text) = {
                let wm = self.shared.window_mgr.lock_or_recover();
                let cur = wm.spin_state(hwnd).map_or(0, |s| s.current);
                (cur, wm.get_window(entry).map(|w| w.text.clone()).unwrap_or_default())
            };
            if size == 0 {
                let _ = self.guest_write::<i32>(storage, cur);
            } else {
                let mut bytes = super::codepage::cp_encode(&text, self.window_codepage(hwnd));
                bytes.truncate(size - 1);
                bytes.push(0);
                let _ = self.guest_write_bytes(storage, &bytes);
            }
        }
        valid.is_some()
    }

    /// The member of `master`'s group its arrows spin.
    fn spin_target(&self, master: u32) -> u32 {
        let wm = self.shared.window_mgr.lock_or_recover();
        let sel = wm.spin_state(master).map_or(master, |s| s.selected);
        let member = sel == master || wm.spin_state(sel).is_some_and(|s| s.master == master);
        if member { sel } else { master }
    }

    /// An arrow of `master` (or Up / Down in a field) spins `target` one
    /// step (`step` values) and tells `master`'s owner.
    fn spin_arrow(&self, master: u32, target: u32, arrow: Arrow, step: i64) {
        let auto = self.shared.window_mgr.lock_or_recover().spin_state(target).is_some_and(|s| s.auto);
        if auto {
            self.spin_by(target, if arrow == Arrow::Up { step } else { -step });
        }
        let code = if arrow == Arrow::Up { SPBN_UPARROW } else { SPBN_DOWNARROW };
        self.shared.window_mgr.lock_or_recover().notify_owner(master, code, target);
    }

    /// The arrow of a master `size` big at `(x, y)`.
    fn arrow_at(style: u32, (cx, cy): (i32, i32), (x, y): (i32, i32)) -> Option<Arrow> {
        let [_, up, down] = spin_layout(style, cx, cy);
        let inside = |(ax, ay, aw, ah): (i32, i32, i32, i32)| x >= ax && x < ax + aw && y >= ay && y < ay + ah;
        if up.2 > 0 && inside(up) { Some(Arrow::Up) } else if down.2 > 0 && inside(down) { Some(Arrow::Down) } else { None }
    }

    /// Button 1 pressed on spin button `hwnd`: an arrow spins at once and
    /// repeats while held (`WM_TIMER(TID_SCROLL)`).
    fn spin_press(&self, hwnd: u32, style: u32, size: (i32, i32), at: (i32, i32)) -> bool {
        if style & WS_DISABLED != 0 {
            return false;
        }
        let Some(arrow) = Self::arrow_at(style, size, at) else { return false };
        let entry = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            wm.pressed_control = hwnd;
            wm.drag_pointer = at;
            let Some(s) = wm.spin_state_mut(hwnd) else { return false };
            s.held = Some(arrow);
            s.repeats = 0;
            s.entry
        };
        // The focus stays in (or goes to) a field of the group.
        let target = self.spin_target(hwnd);
        let focus = self.shared.window_mgr.lock_or_recover().focus_hwnd;
        if self.shared.window_mgr.lock_or_recover().spin_of(focus).is_none() {
            self.set_focus_window(entry);
        }
        self.repaint_control(hwnd);
        self.spin_arrow(hwnd, target, arrow, 1);
        true
    }

    /// Button 1 released: the held arrow comes up, `SPBN_ENDSPIN`.
    fn spin_release(&self, hwnd: u32) -> bool {
        let held = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if wm.pressed_control == hwnd {
                wm.pressed_control = 0;
            }
            wm.spin_state_mut(hwnd).and_then(|s| s.held.take())
        };
        if held.is_none() {
            return false;
        }
        self.repaint_control(hwnd);
        let target = self.spin_target(hwnd);
        self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, SPBN_ENDSPIN, target);
        true
    }

    /// `WM_TIMER(TID_SCROLL)`: a held arrow repeats while the pointer is on
    /// it, faster and faster with `SPBS_FASTSPIN`.
    fn spin_repeat(&self, hwnd: u32, style: u32, size: (i32, i32)) -> bool {
        let (held, at, repeats) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if wm.pressed_control != hwnd { return false; }
            let at = wm.drag_pointer;
            let Some(s) = wm.spin_state_mut(hwnd) else { return false };
            let Some(held) = s.held else { return false };
            s.repeats += 1;
            (held, at, s.repeats)
        };
        if Self::arrow_at(style, size, at) != Some(held) {
            return false;
        }
        let step = if style & SPBS_FASTSPIN != 0 {
            (1i64 << (repeats / FAST_SPIN_REPEATS).min(6)).min(FAST_SPIN_MAX_STEP)
        } else { 1 };
        let target = self.spin_target(hwnd);
        self.spin_arrow(hwnd, target, held, step);
        true
    }

    /// A key the spin field passed on: Up / Down spin (through the master
    /// of a servant); others go on to the owner.
    fn spin_char(&self, hwnd: u32, mp1: u32, mp2: u32) -> bool {
        let fs = mp1 & 0xFFFF;
        let vk = if fs & KC_VIRTUALKEY != 0 { mp2 >> WM_CHAR_VKEY_SHIFT } else { 0 };
        let arrow = match vk {
            VK_UP if fs & KC_ALT == 0 => Some(Arrow::Up),
            VK_DOWN if fs & KC_ALT == 0 => Some(Arrow::Down),
            _ => None,
        };
        let Some(arrow) = arrow else {
            let owner = self.shared.window_mgr.lock_or_recover().notify_target(hwnd);
            if owner != 0 && owner != hwnd {
                self.post_control_message(owner, WM_CHAR, mp1, mp2);
            }
            return false;
        };
        let master = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.spin_state(hwnd).map_or(hwnd, |s| if s.master != 0 { s.master } else { hwnd })
        };
        if fs & KC_KEYUP != 0 {
            self.shared.window_mgr.lock_or_recover().notify_owner(master, SPBN_ENDSPIN, hwnd);
        } else {
            self.spin_arrow(master, hwnd, arrow, 1);
        }
        true
    }

    /// `WM_CONTROL(code)` from the field of spin button `hwnd`.
    fn spin_child_notify(&self, hwnd: u32, code: u32) {
        match code {
            EN_CHANGE => self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, SPBN_CHANGE, hwnd),
            EN_SETFOCUS => {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let master = wm.spin_state(hwnd).map_or(0, |s| if s.master != 0 { s.master } else { hwnd });
                if let Some(m) = wm.spin_state_mut(master) {
                    m.selected = hwnd;
                }
                wm.notify_owner(hwnd, SPBN_SETFOCUS, hwnd);
            }
            EN_KILLFOCUS => self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, SPBN_KILLFOCUS, 0),
            _ => {}
        }
    }

    /// Whether the field of a `SPBS_NUMERICONLY` spin button refuses the
    /// typed character in `WM_CHAR` (mp1, mp2): only digits, and a minus
    /// sign at the start, are taken.
    pub(super) fn spin_refuses_char(&self, field: u32, mp1: u32, mp2: u32) -> bool {
        if mp1 & KC_CHAR == 0 || mp1 & (KC_CTRL | KC_ALT | KC_VIRTUALKEY) != 0 {
            return false;
        }
        let wm = self.shared.window_mgr.lock_or_recover();
        let Some(spin) = wm.spin_of(field) else { return false };
        if wm.get_window(spin).is_none_or(|w| w.style & SPBS_NUMERICONLY == 0) {
            return false;
        }
        let ch = (mp2 & 0xFFFF) as u8 as char;
        let caret_at_start = wm.get_window(field).is_some_and(|w| w.edit_caret.min(w.edit_anchor) == 0);
        !(ch.is_ascii_digit() || (ch == '-' && caret_at_start))
    }

    /// WC_SPINBUTTON: a master's up and down arrows (pressed while held).
    pub(super) fn paint_spin(&self, p: &Painter, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32)) {
        let Some((style, held)) = self.shared.window_mgr.lock_or_recover().get_window(hwnd)
            .and_then(|w| Some((w.style, w.spin.as_ref()?.held))) else { return };
        let [_, up, down] = spin_layout(style, cx, cy);
        let color = if style & WS_DISABLED != 0 { super::CONTROL_BORDER_COLOR } else { CONTROL_TEXT_COLOR };
        for (arrow, (ax, ay, aw, ah)) in [(Arrow::Up, up), (Arrow::Down, down)] {
            if aw <= 0 || ah <= 0 { continue; }
            let face = if held == Some(arrow) { CONTROL_PRESSED_COLOR } else { CONTROL_FACE_COLOR };
            control_box(p, (x + ax, y + ay, aw - 1, ah - 1), face);
            super::pm_scrollbar::draw_arrow(p, (x + ax, y + ay, aw, ah), true, arrow == Arrow::Up, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::pm_types::OS2Message;

    struct Fx {
        loader: Loader,
        hmq: u32,
        client: u32,
    }

    fn fixture() -> Fx {
        let loader = Loader::new_mock();
        let (hmq, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            wm.register_class("App".into(), 0x5000, 0);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            for h in [frame, client] {
                let w = wm.get_window_mut(h).unwrap();
                (w.cx, w.cy, w.visible) = (400, 300, true);
            }
            (hmq, client)
        };
        Fx { loader, hmq, client }
    }

    impl Fx {
        fn spin(&self, style: u32, id: u32) -> (u32, u32) {
            let mut wm = self.loader.shared.window_mgr.lock_or_recover();
            let h = wm.create_window(CLASS_SPINBUTTON.into(), self.client, self.hmq);
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy) = (10, 10 + 40 * id as i32, 100, 24);
            (w.style, w.id, w.owner, w.visible) = (style | WS_VISIBLE, id, self.client, true);
            wm.build_spin(h);
            let e = wm.get_window(h).unwrap().spin.as_ref().unwrap().entry;
            (h, e)
        }
        fn send(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(hwnd, msg, mp1, mp2) {
                ApiResult::Normal(v) => v,
                other => panic!("unexpected {other:?}"),
            }
        }
        fn text(&self, hwnd: u32) -> String {
            let wm = self.loader.shared.window_mgr.lock_or_recover();
            wm.get_window(wm.text_window(hwnd)).unwrap().text.clone()
        }
        fn set_text(&self, hwnd: u32, t: &str) {
            let mut wm = self.loader.shared.window_mgr.lock_or_recover();
            let e = wm.text_window(hwnd);
            wm.get_window_mut(e).unwrap().text = t.into();
        }
        fn query(&self, hwnd: u32, mode: u32) -> (u32, i32) {
            let at = 0x0008_0000;
            let ok = self.send(hwnd, SPBM_QUERYVALUE, at, mode << 16);
            (ok, self.loader.guest_read::<i32>(at).unwrap())
        }
        fn notes(&self) -> Vec<(u32, u32, u32)> {
            let mq = self.loader.shared.window_mgr.lock_or_recover().get_mq(self.hmq).unwrap();
            let msgs: Vec<OS2Message> = mq.lock_or_recover().messages.drain(..).collect();
            msgs.into_iter().filter(|m| m.msg == WM_CONTROL && m.hwnd == self.client)
                .map(|m| (m.mp1 & 0xFFFF, m.mp1 >> 16, m.mp2)).collect()
        }
    }

    fn pos(x: i32, y: i32) -> u32 { (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16) }

    #[test]
    fn values_spin_round_and_parse() {
        let s = SpinState { values: SpinValues::Range { low: -2, high: 3 }, current: 3, ..Default::default() };
        assert_eq!(s.spun(1), Some(-2), "wraps at the upper limit");
        assert_eq!(s.spun(-6), Some(3));
        assert_eq!(s.parse(" 2 "), Some(2));
        assert_eq!(s.parse("4"), None);
        assert_eq!(s.text_of(-2, 3), Some("-02".into()));
        assert_eq!(s.text_of(7, 3), Some("007".into()));
        let a = SpinState { values: SpinValues::Array(vec!["Mon".into(), "Tue".into()]), ..Default::default() };
        assert_eq!(a.spun(-1), Some(1));
        assert_eq!(a.parse("tue"), Some(1));
        assert_eq!(SpinState::default().spun(1), None, "nothing to spin");
    }

    #[test]
    fn the_layout_gives_a_master_arrows_and_a_servant_none() {
        let [field, up, down] = spin_layout(SPBS_MASTER, 100, 24);
        assert_eq!(field, (0, 0, 100 - SPIN_ARROW_W, 24));
        assert_eq!((up.1, up.3, down.1, down.3), (12, 12, 0, 12));
        let [field, up, _] = spin_layout(0, 100, 24);
        assert_eq!((field.2, up.2), (100, 0));
    }

    #[test]
    fn limits_current_value_and_spinning() {
        let fx = fixture();
        let (spin, _) = fx.spin(SPBS_MASTER | SPBS_NUMERICONLY, 1);
        assert_eq!(fx.send(spin, SPBM_SPINUP, 1, 0), 0, "no data yet");
        assert_eq!(fx.send(spin, SPBM_SETLIMITS, 1, 5), 0, "upper below lower");
        assert_eq!(fx.send(spin, SPBM_SETLIMITS, 10, 1), 1);
        assert_eq!(fx.text(spin), "1", "the current value moved to the nearest limit");
        assert_eq!(fx.send(spin, SPBM_SETCURRENTVALUE, 9, 0), 1);
        assert_eq!(fx.send(spin, SPBM_SETCURRENTVALUE, 11, 0), 0);
        assert_eq!(fx.send(spin, SPBM_SPINUP, 2, 0), 1);
        assert_eq!(fx.text(spin), "1", "9 + 2 wraps round to 1");
        assert_eq!(fx.send(spin, SPBM_SPINDOWN, 1, 0), 1);
        assert_eq!(fx.query(spin, SPBQ_UPDATEIFVALID), (1, 10));
        let lim = 0x0008_0100;
        assert_eq!(fx.send(spin, SPBM_QUERYLIMITS, lim, lim + 4), 1);
        assert_eq!((fx.loader.guest_read::<i32>(lim), fx.loader.guest_read::<i32>(lim + 4)), (Some(10), Some(1)));
        assert!(fx.notes().iter().any(|n| n.1 == SPBN_CHANGE), "SPBN_CHANGE on a new value");
    }

    #[test]
    fn query_value_validates_the_field_and_updates_as_asked() {
        let fx = fixture();
        let (spin, _) = fx.spin(SPBS_MASTER, 1);
        fx.send(spin, SPBM_SETLIMITS, 100, 0);
        fx.send(spin, SPBM_SETCURRENTVALUE, 5, 0);
        fx.set_text(spin, "42");
        assert_eq!(fx.query(spin, SPBQ_DONOTUPDATE), (1, 5), "valid, but not taken");
        assert_eq!(fx.query(spin, SPBQ_UPDATEIFVALID), (1, 42));
        fx.set_text(spin, "abc");
        assert_eq!(fx.query(spin, SPBQ_UPDATEIFVALID), (0, 42));
        assert_eq!(fx.text(spin), "abc", "left as typed");
        assert_eq!(fx.query(spin, SPBQ_ALWAYSUPDATE), (0, 42));
        assert_eq!(fx.text(spin), "42", "reset to the last valid value");
        // A string buffer.
        let at = 0x0008_0200;
        assert_eq!(fx.send(spin, SPBM_QUERYVALUE, at, 8 | (SPBQ_DONOTUPDATE << 16)), 1);
        assert_eq!(fx.loader.guest_read_bytes(at, 3).unwrap(), b"42\0");
    }

    #[test]
    fn an_array_spins_by_index() {
        let fx = fixture();
        let (spin, _) = fx.spin(SPBS_MASTER | SPBS_READONLY, 1);
        let (arr, strs) = (0x0008_0000u32, 0x0008_0100u32);
        for (i, t) in ["Mon", "Tue", "Wed"].iter().enumerate() {
            let p = strs + 16 * i as u32;
            fx.loader.guest_write_bytes(p, format!("{t}\0").as_bytes()).unwrap();
            fx.loader.guest_write::<u32>(arr + 4 * i as u32, p).unwrap();
        }
        assert_eq!(fx.send(spin, SPBM_SETARRAY, arr, 0), 0, "no items");
        assert_eq!(fx.send(spin, SPBM_SETARRAY, arr, 3), 1);
        assert_eq!(fx.text(spin), "Mon");
        fx.send(spin, SPBM_SPINDOWN, 1, 0);
        assert_eq!(fx.text(spin), "Wed", "wraps to the last item");
        assert_eq!(fx.query(spin, SPBQ_UPDATEIFVALID), (1, 2));
        let field = fx.loader.shared.window_mgr.lock_or_recover().text_window(spin);
        let style = fx.loader.shared.window_mgr.lock_or_recover().get_window(field).unwrap().style;
        assert!(style & ES_READONLY != 0, "SPBS_READONLY");
    }

    #[test]
    fn arrows_spin_repeat_while_held_and_notify() {
        let fx = fixture();
        let (spin, _) = fx.spin(SPBS_MASTER, 1);
        fx.send(spin, SPBM_SETLIMITS, 50, 0);
        fx.notes();
        // The up arrow: top half of the right edge.
        fx.send(spin, WM_BUTTON1DOWN, pos(95, 18), 0);
        assert_eq!(fx.text(spin), "1");
        assert_eq!(fx.loader.shared.window_mgr.lock_or_recover().post_drag_scroll(), Some(spin));
        fx.send(spin, WM_TIMER, TID_SCROLL, 0);
        fx.send(spin, WM_TIMER, TID_SCROLL, 0);
        assert_eq!(fx.text(spin), "3");
        // Off the arrow: no repeat.
        fx.send(spin, WM_MOUSEMOVE, pos(10, 10), 0);
        fx.send(spin, WM_TIMER, TID_SCROLL, 0);
        assert_eq!(fx.text(spin), "3");
        fx.send(spin, WM_BUTTON1UP, pos(10, 10), 0);
        let notes = fx.notes();
        assert!(notes.contains(&(1, SPBN_UPARROW, spin)));
        assert_eq!(notes.last().map(|n| n.1), Some(SPBN_ENDSPIN));
        fx.send(spin, WM_BUTTON1DOWN, pos(95, 3), 0);
        assert_eq!(fx.text(spin), "2", "the down arrow");
    }

    #[test]
    fn up_and_down_keys_spin_and_numeric_only_refuses_letters() {
        let fx = fixture();
        let (spin, field) = fx.spin(SPBS_MASTER | SPBS_NUMERICONLY, 1);
        fx.send(spin, SPBM_SETLIMITS, 9, 0);
        fx.send(field, WM_CHAR, KC_VIRTUALKEY, VK_UP << WM_CHAR_VKEY_SHIFT);
        // The field passes Up to the spin button.
        let mq = fx.loader.shared.window_mgr.lock_or_recover().get_mq(fx.hmq).unwrap();
        let passed: Vec<OS2Message> = mq.lock_or_recover().messages.drain(..).filter(|m| m.hwnd == spin).collect();
        for m in passed { fx.send(m.hwnd, m.msg, m.mp1, m.mp2); }
        assert_eq!(fx.text(spin), "1");
        assert!(fx.loader.spin_refuses_char(field, KC_CHAR, b'x' as u32));
        assert!(!fx.loader.spin_refuses_char(field, KC_CHAR, b'7' as u32));
        let (other, other_field) = fx.spin(SPBS_MASTER, 2);
        let _ = other;
        assert!(!fx.loader.spin_refuses_char(other_field, KC_CHAR, b'x' as u32));
    }

    #[test]
    fn a_master_spins_the_servant_that_had_the_focus() {
        let fx = fixture();
        let (master, mfield) = fx.spin(SPBS_MASTER, 1);
        let (servant, sfield) = fx.spin(0, 2);
        for s in [master, servant] { fx.send(s, SPBM_SETLIMITS, 59, 0); }
        assert_eq!(fx.send(servant, SPBM_SETMASTER, master, 0), 1);
        fx.loader.set_focus_window(sfield);
        // The focus notifications reach the spin buttons.
        let mq = fx.loader.shared.window_mgr.lock_or_recover().get_mq(fx.hmq).unwrap();
        let msgs: Vec<OS2Message> = mq.lock_or_recover().messages.drain(..).collect();
        for m in msgs.iter().filter(|m| m.hwnd == servant || m.hwnd == master) {
            fx.send(m.hwnd, m.msg, m.mp1, m.mp2);
        }
        fx.send(master, WM_BUTTON1DOWN, pos(95, 18), 0);
        assert_eq!((fx.text(master), fx.text(servant)), ("0".into(), "1".into()));
        let notes = fx.notes();
        assert!(notes.contains(&(1, SPBN_UPARROW, servant)), "{notes:?}");
        assert!(notes.contains(&(2, SPBN_SETFOCUS, servant)), "{notes:?}");
        // SPBM_SETMASTER(NULLHANDLE): notifications only.
        fx.send(servant, SPBM_SETMASTER, 0, 0);
        fx.send(servant, WM_CHAR, KC_VIRTUALKEY, VK_UP << WM_CHAR_VKEY_SHIFT);
        assert_eq!(fx.text(servant), "1");
        let _ = mfield;
    }
}
