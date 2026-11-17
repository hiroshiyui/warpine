// SPDX-License-Identifier: GPL-3.0-only

//! Check boxes and radio buttons: their check state (`BM_SETCHECK`,
//! `BM_QUERYCHECK`, `BM_QUERYCHECKINDEX`), what the auto styles do when the
//! user selects them, radio-button groups, and their painting.
//!
//! A group is a run of sibling windows from one `WS_GROUP` window up to the
//! next (the first sibling starts the first group). Radio buttons keep the
//! group's tab stop on the checked button: `BM_SETCHECK` sets `WS_TABSTOP`
//! on a radio button it checks and clears it on one it unchecks.

use super::constants::*;
use super::pm_types::WindowManager;
use super::{control_label, ApiResult, Loader, MutexExt, Painter, CONTROL_BORDER_COLOR,
            CONTROL_EMPHASIS_COLOR, CONTROL_FIELD_COLOR, CONTROL_LINE_H, CONTROL_PRESSED_COLOR,
            CONTROL_TEXT_COLOR};
use crate::gui::surface::{DrawOp, Mix, SOLID_LINE};

/// Check states (`BM_SETCHECK` mp1, `BM_QUERYCHECK` result).
pub(super) const CHECK_OFF: u32 = 0;
pub(super) const CHECK_ON: u32 = 1;
pub(super) const CHECK_INDETERMINATE: u32 = 2;

/// `BM_QUERYCHECKINDEX` when no radio button of the group is checked.
const NO_CHECK_INDEX: u32 = -1i32 as u32;

/// Side of the check box square and diameter of the radio button circle.
const INDICATOR_SIZE: i32 = 13;
/// Gap between the indicator and the label.
const INDICATOR_GAP: i32 = 5;
/// Radius of the dot in a checked radio button.
const RADIO_DOT_RADIUS: i32 = 3;
/// Inset of the indeterminate mark inside the check box square.
const INDETERMINATE_INSET: i32 = 3;
/// Colour of the indeterminate mark: half-way grey, unlike the black tick.
const INDETERMINATE_COLOR: u32 = 0x0060_6060;
/// Gap between the label and the focus mark around it.
const FOCUS_MARK_GAP: i32 = 2;
/// Dash pattern of the focus mark around a check box or radio button label.
const FOCUS_MARK_STYLE: u16 = 0xCCCC;

/// The kind of button (`BS_PUSHBUTTON`, `BS_CHECKBOX`, ...) in `style`.
fn kind(style: u32) -> u32 {
    style & BS_PRIMARYSTYLES
}

/// Whether a button of `style` has a check state: the check boxes, radio
/// buttons and three-state buttons.
pub(super) fn has_check_state(style: u32) -> bool {
    matches!(kind(style), BS_CHECKBOX | BS_AUTOCHECKBOX | BS_RADIOBUTTON
        | BS_AUTORADIOBUTTON | BS_3STATE | BS_AUTO3STATE)
}

/// Whether a button of `style` is a radio button (plain or auto).
pub(super) fn is_radio(style: u32) -> bool {
    matches!(kind(style), BS_RADIOBUTTON | BS_AUTORADIOBUTTON)
}

/// Whether the arrow keys check an auto-radio button of `style` when they
/// move the focus onto it (not with `BS_NOCURSORSELECT`).
pub(super) fn cursor_selects(style: u32) -> bool {
    kind(style) == BS_AUTORADIOBUTTON && style & BS_NOCURSORSELECT == 0
}

/// The state an auto check box or auto three-state button takes when the
/// user selects it in `state`: a check box toggles, a three-state button
/// goes unchecked → checked → indeterminate → unchecked. `None` for every
/// other kind.
pub(super) fn toggled(style: u32, state: u32) -> Option<u32> {
    match kind(style) {
        BS_AUTOCHECKBOX => Some(if state == CHECK_OFF { CHECK_ON } else { CHECK_OFF }),
        BS_AUTO3STATE => Some(match state {
            CHECK_OFF => CHECK_ON,
            CHECK_ON => CHECK_INDETERMINATE,
            _ => CHECK_OFF,
        }),
        _ => None,
    }
}

/// `BM_SETCHECK` mp1 as a state: 0 and 2 as given, anything else checked.
fn check_state_from(mp1: u32) -> u32 {
    match mp1 & 0xFFFF {
        0 => CHECK_OFF,
        2 => CHECK_INDETERMINATE,
        _ => CHECK_ON,
    }
}

/// Horizontal half-widths of a disc of `radius` pels, one per row from
/// `-radius` to `radius`: the pels within `radius + ½` of the centre.
pub(super) fn disc_rows(radius: i32) -> Vec<(i32, i32)> {
    (-radius..=radius).map(|dy| {
        // Largest dx with dx² + dy² ≤ (r + ½)², in integers: ≤ r² + r.
        let limit = radius * radius + radius - dy * dy;
        let mut dx = 0;
        while (dx + 1) * (dx + 1) <= limit { dx += 1; }
        (dy, dx)
    }).collect()
}

impl WindowManager {
    /// The style of `hwnd` if it is a `WC_BUTTON` with a check state.
    fn check_button_style(&self, hwnd: u32) -> Option<u32> {
        self.get_window(hwnd)
            .filter(|w| w.class_name == "WC_BUTTON" && has_check_state(w.style))
            .map(|w| w.style)
    }

    /// The siblings in the group of `hwnd` (itself included), in order.
    pub fn button_group(&self, hwnd: u32) -> Vec<u32> {
        let Some(parent) = self.get_window(hwnd).map(|w| w.parent) else { return Vec::new() };
        let siblings: Vec<u32> = match self.get_window(parent) {
            Some(p) => p.children.clone(),
            None => return vec![hwnd],
        };
        let Some(at) = siblings.iter().position(|&h| h == hwnd) else { return vec![hwnd] };
        let starts_group = |h: &u32| self.get_window(*h).is_some_and(|w| w.style & WS_GROUP != 0);
        let start = siblings[..=at].iter().rposition(starts_group).unwrap_or(0);
        let end = siblings[at + 1..].iter().position(starts_group).map_or(siblings.len(), |i| at + 1 + i);
        siblings[start..end].to_vec()
    }

    /// The radio buttons in the group of `hwnd`, in order.
    pub fn radio_group(&self, hwnd: u32) -> Vec<u32> {
        self.button_group(hwnd).into_iter()
            .filter(|&h| self.check_button_style(h).is_some_and(is_radio))
            .collect()
    }

    /// `BM_SETCHECK`: give check box / radio button `hwnd` check `state`;
    /// a radio button becomes its group's tab stop when checked and stops
    /// being one when not. Returns the previous state, `None` (nothing
    /// changed) when `hwnd` has no check state.
    pub fn set_button_check(&mut self, hwnd: u32, state: u32) -> Option<u32> {
        let style = self.check_button_style(hwnd)?;
        let w = self.get_window_mut(hwnd)?;
        let old = std::mem::replace(&mut w.check_state, state);
        if is_radio(style) {
            if state == CHECK_OFF { w.style &= !WS_TABSTOP } else { w.style |= WS_TABSTOP }
        }
        Some(old)
    }

    /// `BM_QUERYCHECK`: the check state of `hwnd`, 0 for a button without one.
    pub fn button_check(&self, hwnd: u32) -> u32 {
        if self.check_button_style(hwnd).is_none() { return CHECK_OFF; }
        self.get_window(hwnd).map_or(CHECK_OFF, |w| w.check_state)
    }

    /// `BM_QUERYCHECKINDEX`: the zero-based index of the checked radio
    /// button among the radio buttons of the group of `hwnd`; `None` when
    /// none is checked or `hwnd` is not a radio button.
    pub fn checked_radio_index(&self, hwnd: u32) -> Option<usize> {
        if !self.check_button_style(hwnd).is_some_and(is_radio) { return None; }
        self.radio_group(hwnd).iter().position(|&h| self.button_check(h) != CHECK_OFF)
    }

    /// The user selected button `hwnd` (a click, Space, a mnemonic, or an
    /// arrow key onto an auto-radio button): an auto check box toggles, an
    /// auto three-state button cycles, an auto-radio button is checked and
    /// the other auto-radio buttons of its group unchecked. Other buttons
    /// keep their state. Returns the buttons whose state changed.
    pub fn select_button(&mut self, hwnd: u32) -> Vec<u32> {
        let Some(style) = self.check_button_style(hwnd) else { return Vec::new() };
        let mut changed = Vec::new();
        let mut set = |wm: &mut Self, h: u32, state: u32| {
            if wm.set_button_check(h, state).is_some_and(|old| old != state) { changed.push(h); }
        };
        if let Some(state) = toggled(style, self.button_check(hwnd)) {
            set(self, hwnd, state);
        } else if kind(style) == BS_AUTORADIOBUTTON {
            for h in self.radio_group(hwnd) {
                if h == hwnd {
                    set(self, h, CHECK_ON);
                } else if self.check_button_style(h).is_some_and(|s| kind(s) == BS_AUTORADIOBUTTON) {
                    set(self, h, CHECK_OFF);
                }
            }
        }
        changed
    }
}

impl Loader {
    /// `BM_SETCHECK` / `BM_QUERYCHECK` / `BM_QUERYCHECKINDEX` sent to button
    /// `hwnd`.
    pub(super) fn button_message(&self, hwnd: u32, msg: u32, mp1: u32) -> ApiResult {
        match msg {
            BM_SETCHECK => {
                let old = self.shared.window_mgr.lock_or_recover()
                    .set_button_check(hwnd, check_state_from(mp1));
                if old.is_some() {
                    self.repaint_control(hwnd);
                }
                ApiResult::Normal(old.unwrap_or(CHECK_OFF))
            }
            BM_QUERYCHECK => ApiResult::Normal(self.shared.window_mgr.lock_or_recover().button_check(hwnd)),
            BM_QUERYCHECKINDEX => ApiResult::Normal(self.shared.window_mgr.lock_or_recover()
                .checked_radio_index(hwnd).map_or(NO_CHECK_INDEX, |i| i as u32)),
            _ => ApiResult::Normal(0),
        }
    }

    /// The user selected button `hwnd`: apply the auto styles' state change
    /// and repaint the buttons that changed.
    pub(super) fn select_button(&self, hwnd: u32) {
        let changed = self.shared.window_mgr.lock_or_recover().select_button(hwnd);
        for h in changed {
            self.repaint_control(h);
        }
    }

    /// A check box or radio button: its indicator — a square with a tick
    /// when checked and a grey square inside when indeterminate, or a
    /// circle with a dot when checked — left of its label, both centred
    /// vertically, on the background of its parent. The indicator is shaded
    /// while the button is held down; a dashed mark surrounds the label
    /// while the button has the focus.
    pub(super) fn paint_check_button(&self, p: &Painter, hwnd: u32,
                                     (x, y, cx, cy): (i32, i32, i32, i32), text: &str) {
        let (style, state, pressed, focused) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window(hwnd) else { return };
            (w.style, w.check_state, wm.pressed_control == hwnd, wm.focus_hwnd == hwnd)
        };
        p.fill((x, y, x + cx, y + cy), self.control_background(hwnd));
        let face = if pressed { CONTROL_PRESSED_COLOR } else { CONTROL_FIELD_COLOR };
        let (ix, iy) = (x, y + (cy - INDICATOR_SIZE) / 2);
        if is_radio(style) {
            let r = INDICATOR_SIZE / 2;
            let (ox, oy) = (ix + r, iy + r);
            disc(p, (ox, oy), r, CONTROL_BORDER_COLOR);
            disc(p, (ox, oy), r - 1, face);
            if state != CHECK_OFF {
                disc(p, (ox, oy), RADIO_DOT_RADIUS, CONTROL_TEXT_COLOR);
            }
        } else {
            let last = INDICATOR_SIZE - 1;
            p.fill((ix, iy, ix + last, iy + last), face);
            p.op(DrawOp::Box { x1: ix, y1: iy, x2: ix + last, y2: iy + last,
                               color: CONTROL_BORDER_COLOR, fill: false, mix: Mix::Overpaint });
            match state {
                CHECK_OFF => {}
                CHECK_ON => {
                    // A tick two pels thick: down from the left, up to the right.
                    for t in 0..2 {
                        line(p, (ix + 3, iy + 6 + t), (ix + 5, iy + 4 + t), CONTROL_TEXT_COLOR, SOLID_LINE);
                        line(p, (ix + 5, iy + 4 + t), (ix + 9, iy + 8 + t), CONTROL_TEXT_COLOR, SOLID_LINE);
                    }
                }
                _ => {
                    let i = INDETERMINATE_INSET;
                    p.fill((ix + i, iy + i, ix + last - i, iy + last - i), INDETERMINATE_COLOR);
                }
            }
        }
        let lx = ix + INDICATOR_SIZE + INDICATOR_GAP;
        let ly = y + (cy - CONTROL_LINE_H) / 2;
        let w = control_label(p, lx, ly, text, x + cx - lx);
        if focused {
            let g = FOCUS_MARK_GAP;
            let (x1, y1, x2, y2) = (lx - g, ly, lx + w + g - 1, ly + CONTROL_LINE_H - 1);
            for (a, b) in [((x1, y1), (x2, y1)), ((x2, y1), (x2, y2)), ((x2, y2), (x1, y2)), ((x1, y2), (x1, y1))] {
                line(p, a, b, CONTROL_EMPHASIS_COLOR, FOCUS_MARK_STYLE);
            }
        }
    }
}

/// A filled disc of `radius` pels around `(cx, cy)`.
fn disc(p: &Painter, (cx, cy): (i32, i32), radius: i32, color: u32) {
    for (dy, dx) in disc_rows(radius) {
        p.fill((cx - dx, cy + dy, cx + dx, cy + dy), color);
    }
}

fn line(p: &Painter, (x1, y1): (i32, i32), (x2, y2): (i32, i32), color: u32, style: u16) {
    p.op(DrawOp::Line { x1, y1, x2, y2, color, mix: Mix::Overpaint, style });
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_types::OS2Message;

    #[test]
    fn styles_with_a_check_state() {
        for k in [BS_CHECKBOX, BS_AUTOCHECKBOX, BS_RADIOBUTTON, BS_AUTORADIOBUTTON, BS_3STATE, BS_AUTO3STATE] {
            assert!(has_check_state(k | WS_TABSTOP), "{k}");
        }
        assert!(!has_check_state(BS_PUSHBUTTON | BS_DEFAULT));
        assert!(!has_check_state(7)); // BS_USERBUTTON
        assert!(is_radio(BS_RADIOBUTTON) && is_radio(BS_AUTORADIOBUTTON | BS_NOCURSORSELECT));
        assert!(!is_radio(BS_AUTOCHECKBOX));
        assert!(cursor_selects(BS_AUTORADIOBUTTON));
        assert!(!cursor_selects(BS_AUTORADIOBUTTON | BS_NOCURSORSELECT));
        assert!(!cursor_selects(BS_RADIOBUTTON), "only auto-radio buttons");
    }

    #[test]
    fn auto_check_boxes_toggle_and_three_state_buttons_cycle() {
        assert_eq!(toggled(BS_AUTOCHECKBOX, 0), Some(1));
        assert_eq!(toggled(BS_AUTOCHECKBOX, 1), Some(0));
        assert_eq!(toggled(BS_AUTOCHECKBOX, 2), Some(0));
        assert_eq!(toggled(BS_AUTO3STATE, 0), Some(1));
        assert_eq!(toggled(BS_AUTO3STATE, 1), Some(2));
        assert_eq!(toggled(BS_AUTO3STATE, 2), Some(0));
        for k in [BS_CHECKBOX, BS_3STATE, BS_RADIOBUTTON, BS_AUTORADIOBUTTON, BS_PUSHBUTTON] {
            assert_eq!(toggled(k, 0), None, "{k}");
        }
        assert_eq!(check_state_from(0), 0);
        assert_eq!(check_state_from(2), 2);
        assert_eq!(check_state_from(1), 1);
        assert_eq!(check_state_from(0x7), 1);
        assert_eq!(check_state_from(0x1_0000), 0, "only the low word counts");
    }

    #[test]
    fn disc_rows_are_symmetric_and_round() {
        let rows = disc_rows(6);
        assert_eq!(rows.len(), 13);
        assert_eq!(rows[6], (0, 6), "full width through the centre");
        assert_eq!(rows[0].1, rows[12].1);
        assert!(rows[0].1 < 6 && rows[0].1 > 0, "narrow at the top: {rows:?}");
        assert!(rows.windows(2).take(6).all(|w| w[0].1 <= w[1].1), "widening to the middle");
        assert_eq!(disc_rows(0), vec![(0, 0)]);
    }

    /// A client window with, in order: check box (WS_GROUP), auto check box,
    /// 3-state, auto 3-state, then a group of auto-radio, plain radio,
    /// auto-radio, and a push button starting the last group.
    struct Buttons {
        loader: Loader,
        hmq: u32,
        client: u32,
        check: u32,
        auto_check: u32,
        three: u32,
        auto_three: u32,
        radio_a: u32,
        radio_plain: u32,
        radio_b: u32,
        push: u32,
    }

    fn buttons() -> Buttons {
        let loader = Loader::new_mock();
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hmq = wm.create_mq(437);
        wm.tid_to_hmq.insert(0, hmq);
        let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        wm.frame_to_client.insert(frame, client);
        let mut id = 10;
        let mut button = |wm: &mut WindowManager, style: u32| {
            let h = wm.create_window("WC_BUTTON".into(), client, hmq);
            let w = wm.get_window_mut(h).unwrap();
            w.style = WS_VISIBLE | style;
            w.id = id;
            w.visible = true;
            id += 1;
            h
        };
        let check = button(&mut wm, BS_CHECKBOX | WS_GROUP);
        let auto_check = button(&mut wm, BS_AUTOCHECKBOX);
        let three = button(&mut wm, BS_3STATE);
        let auto_three = button(&mut wm, BS_AUTO3STATE);
        let radio_a = button(&mut wm, BS_AUTORADIOBUTTON | WS_GROUP);
        let radio_plain = button(&mut wm, BS_RADIOBUTTON);
        let radio_b = button(&mut wm, BS_AUTORADIOBUTTON);
        let push = button(&mut wm, BS_PUSHBUTTON | WS_GROUP | WS_TABSTOP);
        drop(wm);
        Buttons { loader, hmq, client, check, auto_check, three, auto_three, radio_a, radio_plain, radio_b, push }
    }

    impl Buttons {
        fn send(&self, hwnd: u32, msg: u32, mp1: u32) -> u32 {
            match self.loader.dispatch_builtin_control(hwnd, msg, mp1, 0) {
                ApiResult::Normal(v) => v,
                _ => panic!("built-in controls answer synchronously"),
            }
        }
        fn state(&self, hwnd: u32) -> u32 { self.send(hwnd, BM_QUERYCHECK, 0) }
        fn style(&self, hwnd: u32) -> u32 {
            self.loader.shared.window_mgr.lock_or_recover().get_window(hwnd).unwrap().style
        }
        fn take_messages(&self) -> Vec<OS2Message> {
            let wm = self.loader.shared.window_mgr.lock_or_recover();
            let mq = wm.get_mq(self.hmq).unwrap();
            let mut mq = mq.lock_or_recover();
            mq.messages.drain(..).collect()
        }
        fn clicked(&self) -> Vec<u32> {
            self.take_messages().into_iter()
                .filter(|m| m.msg == WM_CONTROL && m.mp1 >> 16 == BN_CLICKED)
                .map(|m| m.mp2).collect()
        }
    }

    #[test]
    fn bm_setcheck_returns_the_old_state_and_bm_querycheck_reads_it() {
        let b = buttons();
        assert_eq!(b.send(b.check, BM_SETCHECK, 1), 0);
        assert_eq!(b.state(b.check), 1);
        assert_eq!(b.send(b.check, BM_SETCHECK, 0), 1);
        assert_eq!(b.state(b.check), 0);
        assert_eq!(b.send(b.three, BM_SETCHECK, 2), 0);
        assert_eq!(b.state(b.three), 2, "indeterminate");
        assert_eq!(b.send(b.auto_three, BM_SETCHECK, 1), 0);
        assert_eq!(b.send(b.auto_three, BM_SETCHECK, 2), 1);
        // Push buttons have no check state.
        assert_eq!(b.send(b.push, BM_SETCHECK, 1), 0);
        assert_eq!(b.state(b.push), 0);
        // BM_SETCHECK does not notify the owner.
        assert!(b.clicked().is_empty());
    }

    #[test]
    fn bm_setcheck_keeps_the_tab_stop_on_the_checked_radio_button() {
        let b = buttons();
        assert_eq!(b.style(b.radio_b) & WS_TABSTOP, 0);
        b.send(b.radio_b, BM_SETCHECK, 1);
        assert_ne!(b.style(b.radio_b) & WS_TABSTOP, 0, "checked: tab stop");
        b.send(b.radio_b, BM_SETCHECK, 0);
        assert_eq!(b.style(b.radio_b) & WS_TABSTOP, 0, "unchecked: none");
        // It does not uncheck the others of the group.
        b.send(b.radio_a, BM_SETCHECK, 1);
        b.send(b.radio_b, BM_SETCHECK, 1);
        assert_eq!((b.state(b.radio_a), b.state(b.radio_b)), (1, 1));
        // Check boxes keep their WS_TABSTOP as it is.
        b.send(b.check, BM_SETCHECK, 1);
        assert_eq!(b.style(b.check) & WS_TABSTOP, 0);
        assert_ne!(b.style(b.check) & WS_GROUP, 0, "other style bits stay");
    }

    #[test]
    fn bm_querycheckindex_counts_the_radio_buttons_of_the_group() {
        let b = buttons();
        for h in [b.radio_a, b.radio_plain, b.radio_b] {
            assert_eq!(b.send(h, BM_QUERYCHECKINDEX, 0), NO_CHECK_INDEX, "none checked");
        }
        b.send(b.radio_b, BM_SETCHECK, 1);
        for h in [b.radio_a, b.radio_plain, b.radio_b] {
            assert_eq!(b.send(h, BM_QUERYCHECKINDEX, 0), 2, "any button of the group answers");
        }
        b.send(b.radio_plain, BM_SETCHECK, 1);
        assert_eq!(b.send(b.radio_a, BM_QUERYCHECKINDEX, 0), 1, "the first checked one");
        // Not a radio button: -1.
        b.send(b.check, BM_SETCHECK, 1);
        assert_eq!(b.send(b.check, BM_QUERYCHECKINDEX, 0), NO_CHECK_INDEX);
        assert_eq!(b.send(b.push, BM_QUERYCHECKINDEX, 0), NO_CHECK_INDEX);
    }

    #[test]
    fn groups_run_from_one_ws_group_sibling_to_the_next() {
        let b = buttons();
        let wm = b.loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.button_group(b.auto_three), vec![b.check, b.auto_check, b.three, b.auto_three]);
        assert_eq!(wm.button_group(b.radio_plain), vec![b.radio_a, b.radio_plain, b.radio_b]);
        assert_eq!(wm.radio_group(b.radio_a), vec![b.radio_a, b.radio_plain, b.radio_b]);
        assert_eq!(wm.button_group(b.push), vec![b.push]);
        assert!(wm.radio_group(b.check).is_empty());
        assert_eq!(wm.button_group(b.client).len(), 1, "the client is alone under its frame");
    }

    #[test]
    fn clicks_toggle_auto_check_boxes_and_notify_the_owner() {
        let b = buttons();
        b.loader.click_button(b.auto_check, true);
        assert_eq!(b.state(b.auto_check), 1);
        assert_eq!(b.clicked(), vec![b.auto_check]);
        b.loader.click_button(b.auto_check, false);
        assert_eq!(b.state(b.auto_check), 0);
        assert_eq!(b.clicked(), vec![b.auto_check], "Space or a mnemonic too");
        // A plain check box only notifies: the owner sets the state.
        b.loader.click_button(b.check, true);
        assert_eq!(b.state(b.check), 0);
        assert_eq!(b.clicked(), vec![b.check]);
        // An auto three-state button cycles; a plain one stays.
        let cycle: Vec<u32> = (0..3).map(|_| { b.loader.click_button(b.auto_three, true); b.state(b.auto_three) }).collect();
        assert_eq!(cycle, [1, 2, 0]);
        b.loader.click_button(b.three, true);
        assert_eq!(b.state(b.three), 0);
        // A disabled one does nothing.
        b.loader.shared.window_mgr.lock_or_recover().get_window_mut(b.auto_check).unwrap().style |= WS_DISABLED;
        b.take_messages();
        b.loader.click_button(b.auto_check, true);
        assert_eq!(b.state(b.auto_check), 0);
        assert!(b.clicked().is_empty());
    }

    #[test]
    fn a_click_on_an_auto_radio_button_unchecks_the_other_auto_radio_buttons() {
        let b = buttons();
        b.send(b.radio_plain, BM_SETCHECK, 1);
        b.loader.click_button(b.radio_a, true);
        assert_eq!(b.state(b.radio_a), 1);
        assert_eq!(b.state(b.radio_plain), 1, "a plain radio button is the owner's business");
        b.loader.click_button(b.radio_b, true);
        assert_eq!((b.state(b.radio_a), b.state(b.radio_b)), (0, 1));
        assert_eq!(b.style(b.radio_a) & WS_TABSTOP, 0, "the tab stop moved");
        assert_ne!(b.style(b.radio_b) & WS_TABSTOP, 0);
        assert_eq!(b.clicked(), vec![b.radio_a, b.radio_b]);
        // Clicking the checked one again keeps it checked and notifies.
        b.loader.click_button(b.radio_b, true);
        assert_eq!(b.state(b.radio_b), 1);
        assert_eq!(b.clicked(), vec![b.radio_b]);
        // A plain radio button only notifies.
        b.send(b.radio_plain, BM_SETCHECK, 0);
        b.loader.click_button(b.radio_plain, true);
        assert_eq!(b.state(b.radio_plain), 0);
        assert_eq!(b.clicked(), vec![b.radio_plain]);
    }

    #[test]
    fn space_selects_a_focused_check_box() {
        let b = buttons();
        let (m1, m2) = (KC_VIRTUALKEY | KC_SCANCODE | (0x39 << 24), VK_SPACE << WM_CHAR_VKEY_SHIFT);
        b.send_char(b.auto_check, m1, m2);
        assert_eq!(b.state(b.auto_check), 1);
        assert_eq!(b.clicked(), vec![b.auto_check]);
    }

    impl Buttons {
        fn send_char(&self, hwnd: u32, mp1: u32, mp2: u32) {
            self.loader.dispatch_builtin_control(hwnd, WM_CHAR, mp1, mp2);
        }
    }

    /// The ops painting `hwnd`.
    fn paint_ops(b: &Buttons, hwnd: u32) -> Vec<DrawOp> {
        let frame = b.loader.shared.window_mgr.lock_or_recover().find_frame_for_hwnd(hwnd);
        let surface = b.loader.shared.window_mgr.lock_or_recover().frame_surface(frame).unwrap();
        {
            let mut wm = b.loader.shared.window_mgr.lock_or_recover();
            let w = wm.get_window_mut(hwnd).unwrap();
            (w.x, w.y, w.cx, w.cy, w.text) = (10, 10, 120, 20, "~Option".into());
        }
        surface.lock_or_recover().record = Some(Vec::new());
        b.loader.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
        surface.lock_or_recover().record.take().unwrap()
    }

    fn text_of(ops: &[DrawOp]) -> Vec<String> {
        ops.iter().filter_map(|o| match o { DrawOp::Text { text, .. } => Some(text.clone()), _ => None }).collect()
    }

    fn has_color(ops: &[DrawOp], c: u32) -> bool {
        ops.iter().any(|o| matches!(o,
            DrawOp::Box { color, .. } | DrawOp::Line { color, .. } if *color == c))
    }

    #[test]
    fn check_boxes_paint_a_tick_or_an_indeterminate_mark() {
        let b = buttons();
        let off = paint_ops(&b, b.auto_three);
        assert_eq!(text_of(&off), ["Option"], "the label without its tilde");
        assert!(!off.iter().any(|o| matches!(o, DrawOp::Line { .. })), "no tick: {off:?}");
        assert!(!has_color(&off, INDETERMINATE_COLOR));
        b.send(b.auto_three, BM_SETCHECK, 1);
        let on = paint_ops(&b, b.auto_three);
        assert_eq!(on.iter().filter(|o| matches!(o, DrawOp::Line { color: CONTROL_TEXT_COLOR, .. })).count(), 4,
                   "a two-pel tick of two strokes");
        b.send(b.auto_three, BM_SETCHECK, 2);
        let mid = paint_ops(&b, b.auto_three);
        assert!(has_color(&mid, INDETERMINATE_COLOR), "a grey square: {mid:?}");
        assert!(!mid.iter().any(|o| matches!(o, DrawOp::Line { .. })));
    }

    #[test]
    fn radio_buttons_paint_a_dot_when_checked_and_a_focus_mark() {
        let b = buttons();
        let dot = |ops: &[DrawOp]| ops.iter().filter(|o| matches!(o,
            DrawOp::Box { color: CONTROL_TEXT_COLOR, fill: true, x1, x2, y1, y2, .. }
                if y1 == y2 && x2 - x1 <= 2 * RADIO_DOT_RADIUS)).count();
        let off = paint_ops(&b, b.radio_a);
        assert_eq!(dot(&off), 0);
        assert!(off.iter().filter(|o| matches!(o, DrawOp::Box { color: CONTROL_BORDER_COLOR, .. })).count()
                == 2 * (INDICATOR_SIZE / 2) as usize + 1, "the circle row by row");
        b.send(b.radio_a, BM_SETCHECK, 1);
        let on = paint_ops(&b, b.radio_a);
        assert_eq!(dot(&on), 2 * RADIO_DOT_RADIUS as usize + 1, "the dot row by row: {on:?}");
        assert!(!on.iter().any(|o| matches!(o, DrawOp::Line { style: FOCUS_MARK_STYLE, .. })));
        b.loader.shared.window_mgr.lock_or_recover().focus_hwnd = b.radio_a;
        let focused = paint_ops(&b, b.radio_a);
        assert_eq!(focused.iter().filter(|o| matches!(o, DrawOp::Line { style: FOCUS_MARK_STYLE, .. })).count(), 4);
    }

    #[test]
    fn push_buttons_still_paint_as_push_buttons() {
        let b = buttons();
        let ops = paint_ops(&b, b.push);
        assert!(!ops.iter().any(|o| matches!(o, DrawOp::Line { .. })));
        assert_eq!(text_of(&ops), ["Option"]);
    }
}
