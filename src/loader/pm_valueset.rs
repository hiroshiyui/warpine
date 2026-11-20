// SPDX-License-Identifier: GPL-3.0-or-later

//! The value set control (`WC_VALUESET`, `<pmstddlg.h>`): a grid of
//! `VSCDATA.usRowCount` × `usColumnCount` items — bit maps, icons, texts,
//! RGB colours or colour indices — of which one is selected (PM Reference
//! "Value Set Control Window Processing", PM Guide "Value Set Controls").
//!
//! Items are addressed `MPFROM2SHORT(row, column)`, both from 1; rows run
//! from the top, columns from the left (from the right with
//! `VS_RIGHTTOLEFT`). An item's type is its `VIA_*` type attribute — one of
//! `VIA_BITMAP` (the default), `VIA_ICON`, `VIA_TEXT`, `VIA_RGB`,
//! `VIA_COLORINDEX` —, taken at creation from the `VS_*` type style, plus the
//! flags `VIA_OWNERDRAW`, `VIA_DISABLED` (unavailable-state emphasis, cannot
//! be selected though the cursor reaches it), `VIA_DRAGGABLE`,
//! `VIA_DROPONABLE` (stored only: Warpine has no direct manipulation).
//! Each item fills a cell of the window divided by rows and columns until
//! `VM_SETMETRICS(VMA_ITEMSIZE)` fixes its size; the spacing between items
//! is 0 until `VMA_ITEMSPACING` sets it. Around each item the control keeps
//! room for its emphasis: the selected item has a black box, the selection
//! cursor of a focused value set is a dotted outline.
//!
//! Messages: `VM_SETITEM` / `VM_QUERYITEM` (a text item takes a string and
//! answers through a `VSTEXT`), `VM_SETITEMATTR` / `VM_QUERYITEMATTR`,
//! `VM_SELECTITEM` / `VM_QUERYSELECTEDITEM`, `VM_SETMETRICS` /
//! `VM_QUERYMETRICS`. Notifications (`WM_CONTROL`): `VN_SELECT` and
//! `VN_ENTER` with `MPFROM2SHORT(row, column)` of the selection cursor,
//! `VN_SETFOCUS` / `VN_KILLFOCUS` with the value set (PM Reference,
//! "WM_CONTROL (in Value Set Controls)").
//!
//! Input: a click selects the item under the pointer, a double click (or
//! Enter) sends `VN_ENTER`; the arrow keys move the cursor and select the
//! item reached ("automatic selection"), Home / End go to the first / last
//! item of the row, Ctrl+Home / Ctrl+End to the top-left / bottom-right
//! item, PageUp / PageDown to the top / bottom row; left and right are
//! mirrored with `VS_RIGHTTOLEFT` (PM Guide).
//!
//! Owner drawing (`pm_owner.rs`): a `VIA_OWNERDRAW` item is drawn by the
//! owner (`WM_DRAWITEM`, `idItem` = `VDA_ITEM`, `hItem` =
//! `MPFROM2SHORT(row, column)`, `rclItem` = the item); a `VS_OWNERDRAW`
//! value set asks the owner for its background (`VDA_BACKGROUND`, the whole
//! window). TRUE keeps the owner's drawing; FALSE has the control draw it.
//!
//! Unverified (the documents are silent): the emphasis sizes and colours,
//! the border width, the 1 × 1 grid without `VSCDATA`, `VN_SELECT` also
//! for `VM_SELECTITEM` and for every click on an enabled item, Enter going
//! on to the owner after `VN_ENTER`, no wrapping of the arrow keys at the
//! edges, the cursor drawn only while the value set has the focus.

use super::constants::*;
use super::pm_types::{default_color, WindowManager};
use super::{ApiResult, Loader, MutexExt, Painter, CONTROL_BORDER_COLOR, CONTROL_LINE_H, CONTROL_TEXT_COLOR};
use crate::gui::font;
use crate::gui::pointer::PointerShape;
use crate::gui::surface::{DrawOp, Mix, Pattern};

pub(super) const CLASS_VALUESET: &str = "WC_VALUESET";

/// Most items a value set holds (Warpine's bound on hostile `VSCDATA`).
pub(super) const MAX_VALUESET_ITEMS: usize = 4096;
/// Largest item size or spacing `VM_SETMETRICS` takes (Warpine's bound).
const MAX_METRIC: u32 = 1024;
/// Smallest item size: "cannot be less than 2" (`VM_SETMETRICS`).
const MIN_ITEM_SIZE: u32 = 2;
/// Room around an item for the selection box and the cursor (unverified).
pub(super) const EMPHASIS: i32 = 4;
/// Width of the selection box, just outside the item (unverified).
const SELECT_BOX_W: i32 = 2;
/// Inset of the `VS_BORDER` border (unverified).
const BORDER_W: i32 = 2;
/// Longest item text kept, in bytes.
const MAX_ITEM_TEXT: usize = 256;
/// Line mask of the dotted cursor outline.
const DOTTED: u16 = 0xAAAA;
/// The type attributes; exactly one is set on an item.
const TYPE_ATTRS: u16 = VIA_BITMAP | VIA_ICON | VIA_TEXT | VIA_RGB | VIA_COLORINDEX;

/// One item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VsItem {
    /// `VIA_*` attributes.
    pub attr: u16,
    /// Bit map / pointer handle, RGB colour or colour index.
    pub value: u32,
    /// The text of a text item.
    pub text: String,
}

impl VsItem {
    pub fn kind(&self) -> u16 {
        self.attr & TYPE_ATTRS
    }
}

/// A value set's items, selection and metrics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValueSetState {
    pub rows: u16,
    pub cols: u16,
    /// Row by row from the top.
    pub items: Vec<VsItem>,
    /// Selected item and selection cursor, `(row, column)` from 1.
    pub selected: (u16, u16),
    pub cursor: (u16, u16),
    /// `VMA_ITEMSIZE`; `None` = the window divided by rows and columns.
    pub item_size: Option<(i32, i32)>,
    /// `VMA_ITEMSPACING` (horizontal, vertical).
    pub spacing: (i32, i32),
}

/// The type attribute of items in a value set of `style`.
pub fn default_kind(style: u32) -> u16 {
    if style & VS_TEXT != 0 {
        VIA_TEXT
    } else if style & VS_RGB != 0 {
        VIA_RGB
    } else if style & VS_COLORINDEX != 0 {
        VIA_COLORINDEX
    } else if style & VS_ICON != 0 {
        VIA_ICON
    } else {
        VIA_BITMAP
    }
}

/// `(rows, columns)` from `VSCDATA` bytes: 1 × 1 without (unverified), both
/// at least 1 and together at most [`MAX_VALUESET_ITEMS`] items.
pub fn parse_vscdata(ctl: &[u8]) -> (u16, u16) {
    let cb = ctl.get(..4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let ctl = &ctl[..cb.min(ctl.len())];
    let w = |off: usize| ctl.get(off..off + 2).map_or(1, |b| u16::from_le_bytes([b[0], b[1]]));
    let rows = w(VSCDATA_ROW_COUNT).clamp(1, MAX_VALUESET_ITEMS as u16);
    let cols = w(VSCDATA_COLUMN_COUNT).clamp(1, (MAX_VALUESET_ITEMS / rows as usize) as u16);
    (rows, cols)
}

impl ValueSetState {
    pub fn new((rows, cols): (u16, u16), style: u32) -> ValueSetState {
        ValueSetState {
            rows, cols,
            items: vec![VsItem { attr: default_kind(style), ..Default::default() }; rows as usize * cols as usize],
            selected: (1, 1),
            cursor: (1, 1),
            ..Default::default()
        }
    }

    /// The index of item `(row, col)`, if there is one.
    pub fn index(&self, (row, col): (u16, u16)) -> Option<usize> {
        ((1..=self.rows).contains(&row) && (1..=self.cols).contains(&col))
            .then(|| (row - 1) as usize * self.cols as usize + (col - 1) as usize)
    }

    pub fn item(&self, at: (u16, u16)) -> Option<&VsItem> {
        self.items.get(self.index(at)?)
    }

    pub fn item_mut(&mut self, at: (u16, u16)) -> Option<&mut VsItem> {
        let i = self.index(at)?;
        self.items.get_mut(i)
    }

    /// Whether item `at` exists and can be selected.
    pub fn selectable(&self, at: (u16, u16)) -> bool {
        self.item(at).is_some_and(|i| i.attr & VIA_DISABLED == 0)
    }

    /// `VM_SETITEMATTR`: set (`on`) or clear `attr` on item `at`. A type
    /// attribute replaces the item's type (only one may be given); clearing
    /// the type leaves the default, `VIA_BITMAP`. FALSE for a missing item
    /// or several types at once.
    pub fn set_attr(&mut self, at: (u16, u16), attr: u16, on: bool) -> bool {
        let kind = attr & TYPE_ATTRS;
        if kind.count_ones() > 1 {
            return false;
        }
        let Some(item) = self.item_mut(at) else { return false };
        if on {
            if kind != 0 {
                item.attr &= !TYPE_ATTRS;
            }
            item.attr |= attr;
        } else {
            item.attr &= !attr;
            if item.kind() == 0 {
                item.attr |= VIA_BITMAP;
            }
        }
        true
    }
}

/// A key's move of the selection cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    /// Towards the visual left / right.
    Left,
    Right,
    RowStart,
    RowEnd,
    First,
    Last,
    TopRow,
    BottomRow,
}

/// Where `m` takes the cursor at `(row, col)` in a `rows` × `cols` grid;
/// columns are numbered from the right when `rtl`. Moves stop at the edges.
pub fn moved((row, col): (u16, u16), rows: u16, cols: u16, rtl: bool, m: Move) -> (u16, u16) {
    // Column steps in numbering terms: the visual left is the next column
    // number right to left.
    let (left, right): (i32, i32) = if rtl { (1, -1) } else { (-1, 1) };
    let col_by = |d: i32| (col as i32 + d).clamp(1, cols as i32) as u16;
    let (first_col, last_col) = if rtl { (cols, 1) } else { (1, cols) };
    match m {
        Move::Up => (row.saturating_sub(1).max(1), col),
        Move::Down => ((row + 1).min(rows), col),
        Move::Left => (row, col_by(left)),
        Move::Right => (row, col_by(right)),
        Move::RowStart => (row, first_col),
        Move::RowEnd => (row, last_col),
        Move::First => (1, first_col),
        Move::Last => (rows, last_col),
        Move::TopRow => (1, col),
        Move::BottomRow => (rows, col),
    }
}

/// Where the items of a value set lie (slider coordinates of the control,
/// origin bottom-left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub cy: i32,
    pub border: i32,
    pub rows: u16,
    pub cols: u16,
    pub rtl: bool,
    pub item_w: i32,
    pub item_h: i32,
    pub sx: i32,
    pub sy: i32,
}

impl Layout {
    pub fn new(s: &ValueSetState, style: u32, cx: i32, cy: i32) -> Layout {
        let border = if style & VS_BORDER != 0 { BORDER_W } else { 0 };
        let (sx, sy) = s.spacing;
        let (cols, rows) = (s.cols.max(1) as i32, s.rows.max(1) as i32);
        let (item_w, item_h) = s.item_size.unwrap_or_else(|| {
            let cell = |len: i32, n: i32, sp: i32| (len - 2 * border - (n - 1) * sp) / n;
            ((cell(cx, cols, sx) - 2 * EMPHASIS).max(MIN_ITEM_SIZE as i32),
             (cell(cy, rows, sy) - 2 * EMPHASIS).max(MIN_ITEM_SIZE as i32))
        });
        Layout { cy, border, rows: s.rows, cols: s.cols, rtl: style & VS_RIGHTTOLEFT != 0,
                 item_w, item_h, sx, sy }
    }

    fn cell_w(&self) -> i32 { self.item_w + 2 * EMPHASIS }
    fn cell_h(&self) -> i32 { self.item_h + 2 * EMPHASIS }

    /// The item `(row, col)` as `(x, y, w, h)` in the control.
    pub fn item_rect(&self, (row, col): (u16, u16)) -> (i32, i32, i32, i32) {
        let vis = if self.rtl { self.cols as i32 - col as i32 } else { col as i32 - 1 };
        let x = self.border + vis * (self.cell_w() + self.sx) + EMPHASIS;
        let top = self.cy - self.border - (row as i32 - 1) * (self.cell_h() + self.sy) - EMPHASIS;
        (x, top - self.item_h, self.item_w, self.item_h)
    }

    /// The item whose cell (item and emphasis room) holds `(x, y)`.
    pub fn item_at(&self, (x, y): (i32, i32)) -> Option<(u16, u16)> {
        let (dx, dy) = (x - self.border, self.cy - self.border - 1 - y);
        if dx < 0 || dy < 0 {
            return None;
        }
        let (pw, ph) = (self.cell_w() + self.sx, self.cell_h() + self.sy);
        if dx % pw >= self.cell_w() || dy % ph >= self.cell_h() {
            return None;
        }
        let (vis, row) = (dx / pw, dy / ph + 1);
        if vis >= self.cols as i32 || row > self.rows as i32 {
            return None;
        }
        let col = if self.rtl { self.cols as i32 - vis } else { vis + 1 };
        Some((row as u16, col as u16))
    }
}

/// `MPFROM2SHORT(row, col)`.
fn rc(at: (u16, u16)) -> u32 {
    at.0 as u32 | ((at.1 as u32) << 16)
}

/// `(row, col)` of `MPFROM2SHORT(row, col)`.
fn at_of(mp: u32) -> (u16, u16) {
    (mp as u16, (mp >> 16) as u16)
}

/// Signed position of a mouse message's mp1.
fn mouse_pos(mp1: u32) -> (i32, i32) {
    ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32)
}

impl WindowManager {
    fn valueset_mut(&mut self, hwnd: u32) -> Option<&mut ValueSetState> {
        self.get_window_mut(hwnd)?.valueset.as_deref_mut()
    }

    /// Give a new value set `hwnd` its grid from `VSCDATA` bytes `ctl`.
    pub fn build_valueset(&mut self, hwnd: u32, ctl: &[u8]) {
        let Some(w) = self.get_window_mut(hwnd) else { return };
        if w.class_name != CLASS_VALUESET || w.valueset.is_some() {
            return;
        }
        w.valueset = Some(Box::new(ValueSetState::new(parse_vscdata(ctl), w.style)));
    }

    /// Give a new slider or value set its state from control data `ctl`.
    pub fn build_slider_or_valueset(&mut self, hwnd: u32, ctl: &[u8]) {
        self.build_slider(hwnd, ctl);
        self.build_valueset(hwnd, ctl);
    }
}

impl Loader {
    /// `WinCreateWindow` made `hwnd`: a slider or value set reads its
    /// control data (`SLDCDATA` / `VSCDATA`) at guest address `pctl`, as
    /// many bytes as its `cbSize` gives (at most `MAX_DLG_CTL_DATA`).
    pub(super) fn init_slider_or_valueset(&self, hwnd: u32, pctl: u32) {
        let is_ours = self.shared.window_mgr.lock_or_recover().get_window(hwnd)
            .is_some_and(|w| w.class_name == CLASS_VALUESET || w.class_name == super::pm_slider::CLASS_SLIDER);
        if !is_ours {
            return;
        }
        let ctl = if pctl == 0 {
            Vec::new()
        } else {
            let cb = self.guest_read::<u32>(pctl).unwrap_or(0) as usize;
            self.guest_read_bytes(pctl, cb.min(super::pm_dialog::MAX_DLG_CTL_DATA)).unwrap_or_default()
        };
        self.shared.window_mgr.lock_or_recover().build_slider_or_valueset(hwnd, &ctl);
    }

    /// Style, size and a copy of the state of value set `hwnd`.
    fn valueset_info(&self, hwnd: u32) -> Option<(u32, i32, i32, ValueSetState)> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let w = wm.get_window(hwnd)?;
        Some((w.style, w.cx, w.cy, w.valueset.as_deref()?.clone()))
    }

    fn update_valueset<R>(&self, hwnd: u32, f: impl FnOnce(&mut ValueSetState) -> R) -> Option<R> {
        self.shared.window_mgr.lock_or_recover().valueset_mut(hwnd).map(f)
    }

    /// Move the cursor of `hwnd` to `at` and select the item there if it can
    /// be; the owner is told `VN_SELECT` when it was selected.
    fn valueset_select(&self, hwnd: u32, at: (u16, u16)) -> bool {
        let selected = self.update_valueset(hwnd, |s| {
            s.index(at)?;
            s.cursor = at;
            let ok = s.selectable(at);
            if ok {
                s.selected = at;
            }
            Some(ok)
        }).flatten();
        let Some(selected) = selected else { return false };
        self.repaint_control(hwnd);
        if selected {
            self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, VN_SELECT, rc(at));
        }
        selected
    }

    /// The `VM_*` messages and the value set's input. `None` for messages
    /// left to the generic handling.
    pub(super) fn valueset_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let (style, cx, cy, s) = self.valueset_info(hwnd)?;
        let lay = Layout::new(&s, style, cx, cy);
        let ok = |b: bool| ApiResult::Normal(b as u32);
        let err = ApiResult::Normal(VSERR_INVALID_PARAMETERS as u32);
        let at = at_of(mp1);
        Some(match msg {
            WM_BUTTON1DOWN => {
                if style & WS_DISABLED != 0 {
                    return Some(ok(false));
                }
                self.set_focus_window(hwnd);
                match lay.item_at(mouse_pos(mp1)) {
                    Some(item) => ok(self.valueset_select(hwnd, item)),
                    None => ok(true),
                }
            }
            WM_BUTTON1DBLCLK => {
                if style & WS_DISABLED == 0
                    && let Some(item) = lay.item_at(mouse_pos(mp1))
                    && s.selectable(item)
                {
                    self.valueset_select(hwnd, item);
                    self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, VN_ENTER, rc(item));
                }
                ok(true)
            }
            WM_CHAR => ok(self.valueset_char(hwnd, style, &s, mp1, mp2)),
            // mp2 = the item: a handle, colour or index, or a text item's PSZ.
            VM_SETITEM => {
                let Some(item) = s.item(at) else { return Some(ok(false)) };
                let text = if item.kind() == VIA_TEXT && mp2 != 0 {
                    let mut t = self.read_guest_string_cp(mp2, self.window_codepage(hwnd));
                    super::pm_slider::truncate_utf8(&mut t, MAX_ITEM_TEXT);
                    t
                } else {
                    String::new()
                };
                self.update_valueset(hwnd, |s| if let Some(i) = s.item_mut(at) {
                    i.value = mp2;
                    i.text = text;
                });
                self.repaint_control(hwnd);
                ok(true)
            }
            // A text item answers through the VSTEXT at mp2.
            VM_QUERYITEM => {
                let Some(item) = s.item(at) else { return Some(err) };
                if item.kind() != VIA_TEXT {
                    return Some(ApiResult::Normal(item.value));
                }
                let bytes = super::codepage::cp_encode(&item.text, self.window_codepage(hwnd));
                if mp2 == 0 {
                    return Some(ApiResult::Normal(bytes.len() as u32));
                }
                let (Some(buf), Some(len)) = (self.guest_read::<u32>(mp2),
                                              self.guest_read::<u32>(mp2.wrapping_add(VSTEXT_BUF_LEN)))
                else { return Some(err) };
                ApiResult::Normal(self.copy_text_out(&bytes, buf, len as usize))
            }
            // mp2 = MPFROM2SHORT(attributes, fSet).
            VM_SETITEMATTR => {
                let set = self.update_valueset(hwnd, |s| s.set_attr(at, mp2 as u16, mp2 >> 16 != 0)).unwrap_or(false);
                if set {
                    self.repaint_control(hwnd);
                }
                ok(set)
            }
            VM_QUERYITEMATTR => s.item(at).map_or(err, |i| ApiResult::Normal(i.attr as u32)),
            VM_SELECTITEM => {
                if !s.selectable(at) {
                    return Some(ok(false));
                }
                ok(self.valueset_select(hwnd, at))
            }
            VM_QUERYSELECTEDITEM => ApiResult::Normal(rc(s.selected)),
            // mp1 = VMA_*, mp2 = MPFROM2SHORT(width / horizontal, height / vertical).
            VM_SETMETRICS => {
                let (a, b) = (mp2 & 0xFFFF, mp2 >> 16);
                let set = match mp1 & 0xFFFF {
                    VMA_ITEMSIZE if (MIN_ITEM_SIZE..=MAX_METRIC).contains(&a)
                        && (MIN_ITEM_SIZE..=MAX_METRIC).contains(&b) =>
                        self.update_valueset(hwnd, |s| s.item_size = Some((a as i32, b as i32))).is_some(),
                    VMA_ITEMSPACING if a <= MAX_METRIC && b <= MAX_METRIC =>
                        self.update_valueset(hwnd, |s| s.spacing = (a as i32, b as i32)).is_some(),
                    _ => false,
                };
                if set {
                    self.repaint_control(hwnd);
                }
                ok(set)
            }
            VM_QUERYMETRICS => match mp1 & 0xFFFF {
                VMA_ITEMSIZE => ApiResult::Normal(rc((lay.item_w as u16, lay.item_h as u16))),
                VMA_ITEMSPACING => ApiResult::Normal(rc((lay.sx as u16, lay.sy as u16))),
                _ => err,
            },
            _ => return None,
        })
    }

    /// A key for the value set: the cursor keys move the selection, Enter
    /// sends `VN_ENTER` (and goes on to the owner, as other keys do).
    fn valueset_char(&self, hwnd: u32, style: u32, s: &ValueSetState, mp1: u32, mp2: u32) -> bool {
        let fs = mp1 & 0xFFFF;
        let vk = if fs & KC_VIRTUALKEY != 0 { mp2 >> WM_CHAR_VKEY_SHIFT } else { 0 };
        let ctrl = fs & KC_CTRL != 0;
        let m = if fs & KC_ALT != 0 { None } else {
            match (vk, ctrl) {
                (VK_UP, false) => Some(Move::Up),
                (VK_DOWN, false) => Some(Move::Down),
                (VK_LEFT, false) => Some(Move::Left),
                (VK_RIGHT, false) => Some(Move::Right),
                (VK_HOME, false) => Some(Move::RowStart),
                (VK_END, false) => Some(Move::RowEnd),
                (VK_HOME, true) => Some(Move::First),
                (VK_END, true) => Some(Move::Last),
                (VK_PAGEUP, false) => Some(Move::TopRow),
                (VK_PAGEDOWN, false) => Some(Move::BottomRow),
                _ => None,
            }
        };
        let usable = style & WS_DISABLED == 0;
        if let Some(m) = m.filter(|_| usable) {
            if fs & KC_KEYUP == 0 {
                let to = moved(s.cursor, s.rows, s.cols, style & VS_RIGHTTOLEFT != 0, m);
                if to != s.cursor || s.selected != to {
                    self.valueset_select(hwnd, to);
                }
            }
            return true;
        }
        let enter = matches!(vk, VK_NEWLINE | VK_ENTER) && fs & (KC_CTRL | KC_ALT | KC_KEYUP) == 0;
        if enter && usable {
            self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, VN_ENTER, rc(s.cursor));
        }
        let owner = self.shared.window_mgr.lock_or_recover().notify_target(hwnd);
        if owner != 0 && owner != hwnd {
            self.post_control_message(owner, WM_CHAR, mp1, mp2);
        }
        false
    }

    /// The rectangle (control coordinates, right and top exclusive) of item
    /// `at` of value set `hwnd`.
    pub(super) fn valueset_item_rect(&self, hwnd: u32, at: (u16, u16)) -> Option<(i32, i32, i32, i32)> {
        let (style, cx, cy, s) = self.valueset_info(hwnd)?;
        s.index(at)?;
        let (x, y, w, h) = Layout::new(&s, style, cx, cy).item_rect(at);
        Some((x, y, x + w, y + h))
    }

    /// `(selected, attributes)` of item `at` of `hwnd`.
    pub(super) fn valueset_item_state(&self, hwnd: u32, at: (u16, u16)) -> Option<(bool, u16)> {
        let (_, _, _, s) = self.valueset_info(hwnd)?;
        Some((s.selected == at, s.item(at)?.attr))
    }

    /// WC_VALUESET: background, border and every item with its emphasis;
    /// owner-drawn parts are queued for the owner.
    pub(super) fn paint_valueset(&self, p: &Painter, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32)) {
        let Some((style, _, _, s)) = self.valueset_info(hwnd) else { return };
        p.fill((x, y, x + cx - 1, y + cy - 1), self.control_background(hwnd));
        if style & VS_BORDER != 0 {
            p.op(DrawOp::Box { x1: x, y1: y, x2: x + cx - 1, y2: y + cy - 1, color: CONTROL_BORDER_COLOR,
                               fill: false, mix: Mix::Overpaint });
        }
        if style & VS_OWNERDRAW != 0 {
            self.shared.window_mgr.lock_or_recover().queue_owner_call(super::pm_owner::OwnerCall {
                control: hwnd, kind: super::pm_owner::OwnerCallKind::ValueSetBackground,
            });
        }
        self.paint_valueset_items(p, hwnd, (x, y), &s, style);
    }

    /// Every item of `s` and its emphasis (owner-drawn items queued).
    fn paint_valueset_items(&self, p: &Painter, hwnd: u32, (x, y): (i32, i32), s: &ValueSetState, style: u32) {
        for row in 1..=s.rows {
            for col in 1..=s.cols {
                self.paint_valueset_item(p, hwnd, (x, y), s, style, (row, col), true);
            }
        }
    }

    /// Item `at` of `s` (at `(x, y)` in the surface): its cell cleared, its
    /// contents (or an owner call for them when `owner` allows), its border,
    /// disabled halftone, selection box and cursor.
    #[allow(clippy::too_many_arguments)]
    fn paint_valueset_item(&self, p: &Painter, hwnd: u32, (x, y): (i32, i32), s: &ValueSetState, style: u32,
                           at: (u16, u16), owner: bool) {
        let Some(item) = s.item(at) else { return };
        let (cx, cy) = self.shared.window_mgr.lock_or_recover().get_window(hwnd).map_or((0, 0), |w| (w.cx, w.cy));
        let lay = Layout::new(s, style, cx, cy);
        let (ix, iy, iw, ih) = lay.item_rect(at);
        // Items outside the window (it clips them) are not drawn at all: a
        // hostile grid of large items costs nothing beyond what shows.
        if ix + iw + EMPHASIS <= 0 || ix - EMPHASIS >= cx || iy + ih + EMPHASIS <= 0 || iy - EMPHASIS >= cy {
            return;
        }
        let (ix, iy) = (x + ix, y + iy);
        let bg = self.control_background(hwnd);
        p.fill((ix - EMPHASIS, iy - EMPHASIS, ix + iw + EMPHASIS - 1, iy + ih + EMPHASIS - 1), bg);
        let disabled = item.attr & VIA_DISABLED != 0;
        if item.attr & VIA_OWNERDRAW != 0 && owner {
            self.shared.window_mgr.lock_or_recover().queue_owner_call(super::pm_owner::OwnerCall {
                control: hwnd, kind: super::pm_owner::OwnerCallKind::ValueSetItem { row: at.0, col: at.1 },
            });
        }
        self.paint_valueset_contents(p, item, style, (ix, iy, iw, ih));
        let outline = |inset: i32, color: u32, style: u16| {
            let (x1, y1, x2, y2) = (ix - inset, iy - inset, ix + iw - 1 + inset, iy + ih - 1 + inset);
            for (a, b, c, d) in [(x1, y1, x2, y1), (x2, y1, x2, y2), (x2, y2, x1, y2), (x1, y2, x1, y1)] {
                p.op(DrawOp::Line { x1: a, y1: b, x2: c, y2: d, color, mix: Mix::Overpaint, style });
            }
        };
        if style & VS_ITEMBORDER != 0 {
            outline(1, CONTROL_BORDER_COLOR, crate::gui::surface::SOLID_LINE);
        }
        if disabled && item.kind() != VIA_TEXT {
            // Unavailable-state emphasis: every other pel in the background.
            let (w, h) = (iw.max(0) as u32, ih.max(0) as u32);
            let src = (0..h).flat_map(|r| (0..w).map(move |c| ((r + c) % 2 == 0).then_some(bg))).collect();
            p.op(DrawOp::Blit { x: ix, y: iy, w, h, src: Some(src), rop: ROP_SRCCOPY as u8, pattern: Pattern::solid(0) });
        }
        if s.selected == at {
            for i in 1..=SELECT_BOX_W {
                outline(i + 1, CONTROL_TEXT_COLOR, crate::gui::surface::SOLID_LINE);
            }
        }
        if s.cursor == at && self.shared.window_mgr.lock_or_recover().focus_hwnd == hwnd {
            outline(EMPHASIS, CONTROL_TEXT_COLOR, DOTTED);
        }
    }

    /// The contents of `item` in the rectangle `(x, y, w, h)` of the surface.
    fn paint_valueset_contents(&self, p: &Painter, item: &VsItem, style: u32, (x, y, w, h): (i32, i32, i32, i32)) {
        if w <= 0 || h <= 0 {
            return;
        }
        let rect = (x, y, x + w - 1, y + h - 1);
        match item.kind() {
            VIA_RGB => p.fill(rect, item.value & 0x00FF_FFFF),
            VIA_COLORINDEX => p.fill(rect, default_color(item.value)),
            VIA_TEXT => {
                let (shown, _) = super::pm_dialog::mnemonic_label(&item.text);
                let label = font::fit_text(&shown, w, p.font());
                let tw = font::text_width(label, p.font());
                let (tx, ty) = (x + (w - tw) / 2, y + (h - CONTROL_LINE_H) / 2);
                if item.attr & VIA_DISABLED != 0 {
                    p.op(DrawOp::Text { x: tx, y: ty, text: label.to_string(), color: CONTROL_BORDER_COLOR, cjk: p.cjk });
                } else {
                    super::control_label(p, tx, ty, &item.text, w);
                }
            }
            VIA_BITMAP => { self.paint_bitmap_handle(p, (x, y, w, h), item.value, style & VS_SCALEBITMAPS != 0); }
            VIA_ICON => { self.paint_icon_handle(p, (x, y, w, h), item.value, false); }
            _ => {}
        }
    }

    /// Draw bitmap `hbm` centred in the rectangle `(x, y, w, h)`, stretched
    /// to it with `scale`, else clipped. FALSE when it is no bitmap.
    pub(super) fn paint_bitmap_handle(&self, p: &Painter, rect: (i32, i32, i32, i32), hbm: u32, scale: bool) -> bool {
        let surface = self.shared.window_mgr.lock_or_recover().bitmaps.get(&hbm).map(|b| b.surface.clone());
        let Some(surface) = surface else { return false };
        let s = surface.lock_or_recover();
        let (bw, bh) = (s.width as i32, s.height as i32);
        blit_scaled(p, rect, (bw, bh), scale, |sx, sy| s.pel(sx, sy));
        true
    }

    /// Draw icon (pointer) `hptr` centred in the rectangle `(x, y, w, h)`:
    /// stretched to it with `fit`, else shrunk only when larger. System
    /// icons are Warpine's pictograms (`pm_msgbox::icon_pels`). FALSE when
    /// there is no picture.
    pub(super) fn paint_icon_handle(&self, p: &Painter, (x, y, w, h): (i32, i32, i32, i32), hptr: u32, fit: bool) -> bool {
        let shape = self.shared.pointers.lock_or_recover().shape(hptr);
        match shape {
            Some(PointerShape::Image(img)) => {
                let (iw, ih) = (img.width as i32, img.height as i32);
                // RGBA rows from the top; transparent pels are None.
                let pel = |sx: i32, sy: i32| {
                    let i = ((ih - 1 - sy) * iw + sx) as usize * 4;
                    img.rgba.get(i..i + 4).and_then(|c| (c[3] >= 0x80)
                        .then(|| (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32))
                };
                blit_scaled(p, (x, y, w, h), (iw, ih), fit || iw > w || ih > h, pel);
                true
            }
            Some(PointerShape::System(_)) => {
                // The message box pictograms stand for the system icons.
                let sptr = hptr.wrapping_sub(super::pm_pointer::PointerManager::system_handle(0));
                let Some(pels) = super::pm_msgbox::icon_pels(sptr) else { return false };
                let n = super::pm_msgbox::MSGBOX_ICON_SIZE;
                let pel = |sx: i32, sy: i32| pels.get(((n - 1 - sy) * n + sx) as usize).copied().flatten();
                blit_scaled(p, (x, y, w, h), (n, n), fit || n > w || n > h, pel);
                true
            }
            None => false,
        }
    }

    /// The owner drew (`drawn`) or left part `part` of value set `hwnd`:
    /// the item `at` (`VDA_ITEM`) or the background (`VDA_BACKGROUND`).
    /// A left item is drawn by the control; after a background the owner
    /// drew, the items are drawn over it again.
    pub(super) fn valueset_owner_done(&self, hwnd: u32, at: Option<(u16, u16)>, drawn: bool) {
        let Some((style, _, _, s)) = self.valueset_info(hwnd) else { return };
        let (frame, rect) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            (wm.find_frame_for_hwnd(hwnd), wm.get_abs_rect_in_frame(hwnd))
        };
        let (x, y, _, _) = rect;
        let p = Painter::new(self, frame, hwnd).clipped_to(rect);
        match at {
            Some(at) if !drawn => self.paint_valueset_item(&p, hwnd, (x, y), &s, style, at, false),
            None if drawn => {
                for row in 1..=s.rows {
                    for col in 1..=s.cols {
                        self.paint_valueset_item(&p, hwnd, (x, y), &s, style, (row, col), false);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Draw a `sw` × `sh` picture whose pel at `(sx, sy)` (bottom-left origin)
/// is `pel(sx, sy)` into `(x, y, w, h)`: stretched to it (nearest pel) when
/// `scale`, else centred and clipped. `None` pels leave the surface as is.
fn blit_scaled(p: &Painter, (x, y, w, h): (i32, i32, i32, i32), (sw, sh): (i32, i32), scale: bool,
               pel: impl Fn(i32, i32) -> Option<u32>) {
    if sw <= 0 || sh <= 0 || w <= 0 || h <= 0 {
        return;
    }
    let (dw, dh) = if scale { (w, h) } else { (sw.min(w), sh.min(h)) };
    let (ox, oy) = if scale { (0, 0) } else { ((sw - dw) / 2, (sh - dh) / 2) };
    let src: Vec<Option<u32>> = (0..dh).flat_map(|r| (0..dw).map(move |c| (r, c))).map(|(r, c)| {
        if scale {
            pel((c as i64 * sw as i64 / dw as i64) as i32, (r as i64 * sh as i64 / dh as i64) as i32)
        } else {
            pel(ox + c, oy + r)
        }
    }).collect();
    p.op(DrawOp::Blit {
        x: x + (w - dw) / 2, y: y + (h - dh) / 2, w: dw as u32, h: dh as u32, src: Some(src),
        rop: ROP_SRCCOPY as u8, pattern: Pattern::solid(0),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::pm_types::OS2Message;

    fn vscdata(rows: u16, cols: u16) -> Vec<u8> {
        let mut v = (VSCDATA_SIZE as u32).to_le_bytes().to_vec();
        v.extend_from_slice(&rows.to_le_bytes());
        v.extend_from_slice(&cols.to_le_bytes());
        v
    }

    #[test]
    fn vscdata_is_parsed_and_bounded() {
        assert_eq!(parse_vscdata(&vscdata(2, 5)), (2, 5));
        assert_eq!(parse_vscdata(&[]), (1, 1));
        assert_eq!(parse_vscdata(&vscdata(0, 0)), (1, 1));
        let (r, c) = parse_vscdata(&vscdata(0xFFFF, 0xFFFF));
        assert!(r as usize * c as usize <= MAX_VALUESET_ITEMS);
        let (r, c) = parse_vscdata(&vscdata(100, 100));
        assert_eq!((r, c), (100, 40));
    }

    #[test]
    fn items_take_the_style_type_and_attributes_replace_it() {
        let mut s = ValueSetState::new((2, 3), VS_RGB | VS_BORDER);
        assert_eq!(s.items.len(), 6);
        assert!(s.items.iter().all(|i| i.attr == VIA_RGB));
        assert_eq!(ValueSetState::new((1, 1), 0).items[0].attr, VIA_BITMAP);
        assert_eq!(s.index((2, 3)), Some(5));
        assert_eq!(s.index((3, 1)), None);
        assert_eq!(s.index((0, 1)), None);
        assert!(s.set_attr((1, 2), VIA_TEXT, true));
        assert_eq!(s.item((1, 2)).unwrap().attr, VIA_TEXT);
        assert!(s.set_attr((1, 2), VIA_DISABLED | VIA_OWNERDRAW, true));
        assert!(!s.selectable((1, 2)));
        assert!(s.set_attr((1, 2), VIA_TEXT, false));
        assert_eq!(s.item((1, 2)).unwrap().attr, VIA_BITMAP | VIA_DISABLED | VIA_OWNERDRAW,
                   "the default type comes back");
        assert!(!s.set_attr((1, 2), VIA_TEXT | VIA_RGB, true), "two types at once");
        assert!(!s.set_attr((9, 9), VIA_DISABLED, true));
    }

    #[test]
    fn cursor_moves_stop_at_the_edges_and_mirror_right_to_left() {
        assert_eq!(moved((1, 1), 2, 4, false, Move::Left), (1, 1));
        assert_eq!(moved((1, 1), 2, 4, false, Move::Right), (1, 2));
        assert_eq!(moved((1, 1), 2, 4, true, Move::Right), (1, 1), "column 1 is rightmost");
        assert_eq!(moved((1, 2), 2, 4, true, Move::Right), (1, 1));
        assert_eq!(moved((1, 1), 2, 4, false, Move::Up), (1, 1));
        assert_eq!(moved((1, 3), 2, 4, false, Move::Down), (2, 3));
        assert_eq!(moved((2, 3), 2, 4, false, Move::Down), (2, 3));
        assert_eq!(moved((2, 3), 2, 4, false, Move::RowStart), (2, 1));
        assert_eq!(moved((2, 3), 2, 4, true, Move::RowStart), (2, 4));
        assert_eq!(moved((1, 3), 2, 4, false, Move::Last), (2, 4));
        assert_eq!(moved((2, 3), 2, 4, false, Move::First), (1, 1));
        assert_eq!(moved((2, 3), 2, 4, false, Move::TopRow), (1, 3));
        assert_eq!(moved((1, 3), 2, 4, false, Move::BottomRow), (2, 3));
    }

    #[test]
    fn layout_divides_the_window_and_hits_items() {
        let s = ValueSetState::new((2, 4), VS_RGB);
        let lay = Layout::new(&s, 0, 200, 100);
        assert_eq!((lay.item_w, lay.item_h), (50 - 2 * EMPHASIS, 50 - 2 * EMPHASIS));
        assert_eq!(lay.item_rect((1, 1)), (EMPHASIS, 100 - 50 + EMPHASIS, 42, 42));
        assert_eq!(lay.item_rect((2, 4)), (150 + EMPHASIS, EMPHASIS, 42, 42));
        assert_eq!(lay.item_at((1, 99)), Some((1, 1)));
        assert_eq!(lay.item_at((199, 0)), Some((2, 4)));
        assert_eq!(lay.item_at((60, 70)), Some((1, 2)));
        assert_eq!(lay.item_at((200, 50)), None, "right of the grid");
        assert_eq!(lay.item_at((-1, 50)), None);
        let rtl = Layout::new(&s, VS_RIGHTTOLEFT, 200, 100);
        assert_eq!(rtl.item_rect((1, 1)).0, 150 + EMPHASIS, "column 1 on the right");
        assert_eq!(rtl.item_at((5, 99)), Some((1, 4)));
        // Fixed size and spacing; the gaps hit nothing.
        let mut fixed = s.clone();
        fixed.item_size = Some((20, 10));
        fixed.spacing = (6, 2);
        let lay = Layout::new(&fixed, VS_BORDER, 200, 100);
        assert_eq!(lay.item_rect((1, 2)), (BORDER_W + 28 + 6 + EMPHASIS, 100 - BORDER_W - EMPHASIS - 10, 20, 10));
        assert_eq!(lay.item_at((BORDER_W + 28 + 2, 90)), None, "in the spacing");
        assert_eq!(lay.item_at((BORDER_W + 28 + 6, 90)), Some((1, 2)));
        assert_eq!(lay.item_at((BORDER_W + 1000, 90)), None);
        // A tiny window keeps items of the smallest size.
        let tiny = Layout::new(&s, 0, 4, 4);
        assert_eq!((tiny.item_w, tiny.item_h), (MIN_ITEM_SIZE as i32, MIN_ITEM_SIZE as i32));
    }

    // ── Messages ────────────────────────────────────────────────────────────

    struct Fx {
        loader: Loader,
        hmq: u32,
        client: u32,
        vs: u32,
    }

    const ID: u32 = 60;
    const BUF: u32 = 0x3000;

    /// A 200 × 100 value set of 2 × 4 items of `style` at (10, 10).
    fn fixture(style: u32) -> Fx {
        let loader = Loader::new_mock();
        let (hmq, client, vs) = {
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
            let h = wm.create_window(CLASS_VALUESET.into(), client, hmq);
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy, w.style, w.id, w.owner, w.visible) =
                (10, 10, 200, 100, WS_VISIBLE | style, ID, client, true);
            wm.build_slider_or_valueset(h, &vscdata(2, 4));
            (hmq, client, h)
        };
        Fx { loader, hmq, client, vs }
    }

    impl Fx {
        fn send(&self, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(self.vs, msg, mp1, mp2) {
                ApiResult::Normal(v) => v,
                other => panic!("unexpected {other:?}"),
            }
        }
        fn notes(&self) -> Vec<(u32, u32)> {
            let wm = self.loader.shared.window_mgr.lock_or_recover();
            let mq = wm.get_mq(self.hmq).unwrap();
            let msgs: Vec<OS2Message> = mq.lock_or_recover().messages.drain(..).collect();
            msgs.into_iter().filter(|m| m.msg == WM_CONTROL)
                .inspect(|m| assert_eq!((m.hwnd, m.mp1 & 0xFFFF), (self.client, ID)))
                .map(|m| (m.mp1 >> 16, m.mp2)).collect()
        }
        fn key(&self, vk: u32, extra: u32) -> u32 {
            self.send(WM_CHAR, KC_VIRTUALKEY | extra, vk << WM_CHAR_VKEY_SHIFT)
        }
        fn at(x: i32, y: i32) -> u32 {
            (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16)
        }
    }

    #[test]
    fn items_are_set_and_queried() {
        let f = fixture(VS_RGB);
        assert_eq!(f.send(VM_SETITEM, rc((1, 2)), 0x00FF_0000), 1);
        assert_eq!(f.send(VM_QUERYITEM, rc((1, 2)), 0), 0x00FF_0000);
        assert_eq!(f.send(VM_SETITEM, rc((3, 1)), 1), 0, "no third row");
        assert_eq!(f.send(VM_QUERYITEM, rc((0, 1)), 0), VSERR_INVALID_PARAMETERS as u32);
        // A text item.
        assert_eq!(f.send(VM_SETITEMATTR, rc((2, 1)), (1 << 16) | VIA_TEXT as u32), 1);
        assert_eq!(f.send(VM_QUERYITEMATTR, rc((2, 1)), 0), VIA_TEXT as u32);
        f.loader.guest_write_bytes(BUF, b"~Hello\0").unwrap();
        assert_eq!(f.send(VM_SETITEM, rc((2, 1)), BUF), 1);
        // VSTEXT { pszItemText, ulBufLen }.
        f.loader.guest_write::<u32>(BUF + 0x40, BUF + 0x100).unwrap();
        f.loader.guest_write::<u32>(BUF + 0x44, 4).unwrap();
        assert_eq!(f.send(VM_QUERYITEM, rc((2, 1)), BUF + 0x40), 3);
        assert_eq!(f.loader.guest_read_bytes(BUF + 0x100, 4).unwrap(), b"~He\0");
        f.loader.guest_write::<u32>(BUF + 0x44, 0).unwrap();
        assert_eq!(f.send(VM_QUERYITEM, rc((2, 1)), BUF + 0x40), 6, "the length to allocate");
        // Attributes.
        assert_eq!(f.send(VM_SETITEMATTR, rc((1, 1)), (1 << 16) | VIA_DISABLED as u32), 1);
        assert_eq!(f.send(VM_QUERYITEMATTR, rc((1, 1)), 0), (VIA_RGB | VIA_DISABLED) as u32);
        assert_eq!(f.send(VM_QUERYITEMATTR, rc((5, 5)), 0), VSERR_INVALID_PARAMETERS as u32);
    }

    #[test]
    fn selection_and_metrics() {
        let f = fixture(VS_COLORINDEX);
        assert_eq!(f.send(VM_QUERYSELECTEDITEM, 0, 0), rc((1, 1)), "row 1, column 1 at first");
        assert_eq!(f.send(VM_SELECTITEM, rc((2, 3)), 0), 1);
        assert_eq!(f.notes(), vec![(VN_SELECT, rc((2, 3)))]);
        assert_eq!(f.send(VM_QUERYSELECTEDITEM, 0, 0), rc((2, 3)));
        f.send(VM_SETITEMATTR, rc((1, 4)), (1 << 16) | VIA_DISABLED as u32);
        assert_eq!(f.send(VM_SELECTITEM, rc((1, 4)), 0), 0, "a disabled item");
        assert_eq!(f.send(VM_SELECTITEM, rc((9, 1)), 0), 0);
        assert_eq!(f.send(VM_QUERYSELECTEDITEM, 0, 0), rc((2, 3)));
        assert_eq!(f.send(VM_QUERYMETRICS, VMA_ITEMSIZE, 0), rc((42, 42)));
        assert_eq!(f.send(VM_QUERYMETRICS, VMA_ITEMSPACING, 0), 0);
        assert_eq!(f.send(VM_SETMETRICS, VMA_ITEMSIZE, rc((20, 12))), 1);
        assert_eq!(f.send(VM_SETMETRICS, VMA_ITEMSIZE, rc((1, 12))), 0, "below 2");
        assert_eq!(f.send(VM_SETMETRICS, VMA_ITEMSPACING, rc((3, 4))), 1);
        assert_eq!(f.send(VM_SETMETRICS, VMA_ITEMSPACING, rc((3, 5000))), 0);
        assert_eq!(f.send(VM_QUERYMETRICS, VMA_ITEMSIZE, 0), rc((20, 12)));
        assert_eq!(f.send(VM_QUERYMETRICS, VMA_ITEMSPACING, 0), rc((3, 4)));
        assert_eq!(f.send(VM_QUERYMETRICS, 7, 0), VSERR_INVALID_PARAMETERS as u32);
    }

    #[test]
    fn clicks_select_and_double_clicks_enter() {
        let f = fixture(VS_RGB);
        // Item (1, 2) spans x 54..96, y 54..96 of the 200 × 100 control.
        f.send(WM_BUTTON1DOWN, Fx::at(70, 70), 0);
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().focus_hwnd, f.vs);
        assert_eq!(f.notes(), vec![(VN_SETFOCUS, f.vs), (VN_SELECT, rc((1, 2)))]);
        f.send(WM_BUTTON1DBLCLK, Fx::at(70, 70), 0);
        assert_eq!(f.notes(), vec![(VN_SELECT, rc((1, 2))), (VN_ENTER, rc((1, 2)))]);
        // A disabled item takes the cursor, not the selection.
        f.send(VM_SETITEMATTR, rc((2, 1)), (1 << 16) | VIA_DISABLED as u32);
        f.send(WM_BUTTON1DOWN, Fx::at(10, 10), 0);
        assert_eq!(f.notes(), vec![]);
        assert_eq!(f.send(VM_QUERYSELECTEDITEM, 0, 0), rc((1, 2)));
        f.send(WM_BUTTON1DBLCLK, Fx::at(10, 10), 0);
        assert_eq!(f.notes(), vec![], "no VN_ENTER on a disabled item");
    }

    #[test]
    fn keys_move_the_selection() {
        let f = fixture(VS_RGB | VS_RIGHTTOLEFT);
        assert_eq!(f.key(VK_LEFT, 0), 1);
        assert_eq!(f.notes(), vec![(VN_SELECT, rc((1, 2)))], "right to left: left is the next column");
        f.key(VK_DOWN, 0);
        f.key(VK_END, KC_CTRL);
        f.key(VK_PAGEUP, 0);
        f.key(VK_HOME, 0);
        assert_eq!(f.notes(), vec![(VN_SELECT, rc((2, 2))), (VN_SELECT, rc((2, 1))),
                                   (VN_SELECT, rc((1, 1))), (VN_SELECT, rc((1, 4)))]);
        f.key(VK_UP, 0);
        assert_eq!(f.notes(), vec![], "at the top row");
        // Enter: VN_ENTER, and the key goes on to the owner.
        assert_eq!(f.key(VK_NEWLINE, 0), 0);
        assert_eq!(f.notes(), vec![(VN_ENTER, rc((1, 4)))]);
        f.key(VK_NEWLINE, 0);
        let wm = f.loader.shared.window_mgr.lock_or_recover();
        let mq = wm.get_mq(f.hmq).unwrap();
        assert!(mq.lock_or_recover().messages.iter().any(|m| m.msg == WM_CHAR && m.hwnd == f.client));
    }

    #[test]
    fn painting_draws_items_and_the_selection() {
        let f = fixture(VS_RGB | VS_ITEMBORDER);
        f.send(VM_SETITEM, rc((1, 1)), 0x0012_3456);
        f.send(VM_SETITEM, rc((2, 4)), 0x00AB_CDEF);
        f.send(VM_SETITEMATTR, rc((2, 4)), (1 << 16) | VIA_DISABLED as u32);
        let frame = f.loader.shared.window_mgr.lock_or_recover().find_frame_for_hwnd(f.vs);
        f.loader.shared.window_mgr.lock_or_recover().resize_surface(frame, 400, 300);
        f.send(WM_PAINT, 0, 0);
        let pel = |x: i32, y: i32| {
            let s = f.loader.shared.window_mgr.lock_or_recover().surfaces.get(&frame).unwrap().clone();
            let s = s.lock_or_recover();
            s.pel(x, y).unwrap()
        };
        // Item (1, 1) at control (4, 54) 42 × 42; the control at (10, 10).
        assert_eq!(pel(10 + 20, 10 + 70), 0x0012_3456);
        assert_eq!(pel(10 + EMPHASIS - 2, 10 + 70), CONTROL_TEXT_COLOR, "the selection box");
        assert_eq!(pel(10 + EMPHASIS - 1, 10 + 70), CONTROL_BORDER_COLOR, "the item border");
        // The disabled item is half background.
        let (a, b) = (pel(10 + 170, 10 + 20), pel(10 + 171, 10 + 20));
        assert!(a != b && (a == 0x00AB_CDEF || b == 0x00AB_CDEF));
    }

    #[test]
    fn items_larger_than_the_window_are_clipped_to_it() {
        let f = fixture(VS_RGB);
        for row in 1..=2u16 {
            for col in 1..=4u16 {
                f.send(VM_SETITEM, rc((row, col)), 0x0011_2233);
            }
        }
        assert_eq!(f.send(VM_SETMETRICS, VMA_ITEMSIZE, rc((MAX_METRIC as u16, MAX_METRIC as u16))), 1);
        let frame = f.loader.shared.window_mgr.lock_or_recover().find_frame_for_hwnd(f.vs);
        f.loader.shared.window_mgr.lock_or_recover().resize_surface(frame, 400, 300);
        f.send(WM_PAINT, 0, 0);
        let pel = |x: i32, y: i32| {
            let s = f.loader.shared.window_mgr.lock_or_recover().surfaces.get(&frame).unwrap().clone();
            let s = s.lock_or_recover();
            s.pel(x, y).unwrap()
        };
        // The control is (10, 10) 200 × 100: inside it the colour, outside
        // nothing was drawn.
        assert_eq!(pel(10 + 100, 10 + 50), 0x0011_2233);
        assert_ne!(pel(10 + 250, 10 + 50), 0x0011_2233, "right of the control");
        assert_ne!(pel(10 + 50, 10 + 150), 0x0011_2233, "above the control");
    }

    #[test]
    fn owner_drawn_items_and_background_are_queued() {
        let f = fixture(VS_RGB | VS_OWNERDRAW);
        f.send(VM_SETITEMATTR, rc((1, 3)), (1 << 16) | VIA_OWNERDRAW as u32);
        f.loader.shared.window_mgr.lock_or_recover().owner_calls.clear();
        f.send(WM_PAINT, 0, 0);
        use crate::loader::pm_owner::OwnerCallKind;
        let kinds: Vec<OwnerCallKind> = f.loader.shared.window_mgr.lock_or_recover().owner_calls.iter()
            .filter(|c| c.control == f.vs).map(|c| c.kind).collect();
        assert_eq!(kinds, vec![OwnerCallKind::ValueSetBackground, OwnerCallKind::ValueSetItem { row: 1, col: 3 }]);
        assert_eq!(f.loader.valueset_item_rect(f.vs, (1, 3)), Some((104, 54, 146, 96)));
        assert_eq!(f.loader.valueset_item_state(f.vs, (1, 1)), Some((true, VIA_RGB)));
    }

    #[test]
    fn create_window_reads_vscdata_from_the_guest() {
        let f = fixture(0);
        let vs2 = {
            let mut wm = f.loader.shared.window_mgr.lock_or_recover();
            wm.create_window(CLASS_VALUESET.into(), f.client, f.hmq)
        };
        f.loader.guest_write_bytes(BUF, &vscdata(3, 7)).unwrap();
        f.loader.init_slider_or_valueset(vs2, BUF);
        let wm = f.loader.shared.window_mgr.lock_or_recover();
        let s = wm.get_window(vs2).unwrap().valueset.as_ref().unwrap();
        assert_eq!((s.rows, s.cols, s.items.len()), (3, 7, 21));
    }
}
