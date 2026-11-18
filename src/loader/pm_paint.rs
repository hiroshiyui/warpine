// SPDX-License-Identifier: GPL-3.0-only

//! Window painting: update rectangles, `WinScrollWindow`, `WinDrawBorder`.
//!
//! Every window has an update rectangle ([`OS2Window::update`](super::pm_types::OS2Window),
//! window coordinates, exclusive at the right and top). PM keeps an update
//! *region*; Warpine keeps its bounding rectangle:
//!
//! - `WinInvalidateRect` adds a rectangle (NULL = the whole window) and,
//!   when there is something to paint, makes sure one `WM_PAINT` for the
//!   window waits in its queue (it ranks below posted messages and input,
//!   `pm_input::Rank`). Descendants are invalidated with it when
//!   `fIncludeChildren` is set or the window is not `WS_CLIPCHILDREN` (the
//!   PM Reference, WinInvalidateRect).
//! - `WinValidateRect` removes a rectangle: exactly when what is left is a
//!   rectangle, otherwise the update rectangle stays (an over-approximation:
//!   at worst a window paints more than it must).
//! - `WinQueryUpdateRect` reports it; `WinBeginPaint` reports it and empties
//!   it. When the update rectangle becomes empty, the waiting `WM_PAINT` is
//!   withdrawn: `WM_PAINT` comes only while there is something to paint.
//!
//! The presentation space of `WinBeginPaint` is not clipped to the update
//! rectangle (drawing outside it shows).
//!
//! `WinScrollWindow` moves the pels of the window's surface; the area the
//! scroll uncovers can be added to the update rectangle (`SW_INVALIDATERGN`)
//! and child windows can move with the pels (`SW_SCROLLCHILDREN`).
//! `WinDrawBorder` draws a rectangle's border (and interior) with a pattern
//! raster operation.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_rect::{Rectl, EMPTY_RECTL};
use super::pm_types::{OS2Message, WindowManager};
use super::{ApiResult, Loader};
use crate::gui::surface::{AreaFill, Pattern};
use crate::gui::{DrawOp, Mix};

/// The `flCmd` bits `WinDrawBorder` knows (`<pmwin.h>`).
const DB_ALL_FLAGS: u32 = DB_ROP | DB_INTERIOR | DB_AREAATTRS | DB_STANDARD | DB_DLGBORDER;

/// `WinDrawBorder(DB_DLGBORDER)` border colours: those of Warpine's active
/// and inactive title bars (the compositor's chrome).
pub const DLG_BORDER_ACTIVE_COLOR: u32 = 0x0000_0080;
pub const DLG_BORDER_INACTIVE_COLOR: u32 = 0x0040_4040;

/// Scroll distances beyond this are clamped: no surface is that large, so
/// the result is the same and the arithmetic cannot overflow.
const MAX_SCROLL: i32 = 1 << 20;
/// The most windows one `WinUpdateWindow` repaints.
const MAX_UPDATE_WINDOWS: usize = 4096;

/// `a` less `b` as disjoint rectangles (at most four; none when `b` covers
/// `a`).
pub fn rect_difference(a: &Rectl, b: &Rectl) -> Vec<Rectl> {
    if a.is_empty() { return Vec::new(); }
    let Some(i) = a.intersect(b) else { return vec![*a] };
    [
        Rectl::new(a.x_left, a.y_bottom, a.x_right, i.y_bottom),   // below
        Rectl::new(a.x_left, i.y_top, a.x_right, a.y_top),         // above
        Rectl::new(a.x_left, i.y_bottom, i.x_left, i.y_top),       // left
        Rectl::new(i.x_right, i.y_bottom, a.x_right, i.y_top),     // right
    ].into_iter().filter(|r| !r.is_empty()).collect()
}

fn area(r: &Rectl) -> i64 {
    if r.is_empty() { 0 } else { (r.x_right - r.x_left) as i64 * (r.y_top - r.y_bottom) as i64 }
}

/// The bounding rectangle of disjoint `rects` and the complexity of their
/// union (`RGN_NULL`, `RGN_RECT`, `RGN_COMPLEX`).
pub fn region_of(rects: &[Rectl]) -> (Rectl, u32) {
    let bound = rects.iter().fold(EMPTY_RECTL, |acc, r| acc.union(r));
    let complexity = if bound.is_empty() {
        RGN_NULL
    } else if rects.iter().map(area).sum::<i64>() == area(&bound) {
        RGN_RECT
    } else {
        RGN_COMPLEX
    };
    (bound, complexity)
}

/// Inclusive corners `(x1, y1, x2, y2)` of a rectangle to fill.
pub type Corners = (i32, i32, i32, i32);

/// The parts of a `WinDrawBorder` rectangle `rc` (exclusive at the right and
/// top) as inclusive corner pairs: the border pieces for side widths `cx`
/// and `cy`, and the interior. When the borders meet or overlap, the whole
/// rectangle is one border piece without interior; with no border, the
/// whole rectangle is the interior (the PM Reference, WinDrawBorder).
pub fn border_parts(rc: &Rectl, cx: i32, cy: i32) -> (Vec<Corners>, Option<Corners>) {
    if rc.is_empty() { return (Vec::new(), None); }
    let (l, b, r, t) = (rc.x_left, rc.y_bottom, rc.x_right - 1, rc.y_top - 1);
    let (w, h) = (r - l + 1, t - b + 1);
    let (cx, cy) = (cx.max(0), cy.max(0));
    if cx == 0 && cy == 0 {
        return (Vec::new(), Some((l, b, r, t)));
    }
    if 2 * cx as i64 >= w as i64 || 2 * cy as i64 >= h as i64 {
        return (vec![(l, b, r, t)], None);
    }
    let mut parts = Vec::new();
    if cy > 0 {
        parts.push((l, b, r, b + cy - 1));
        parts.push((l, t - cy + 1, r, t));
    }
    if cx > 0 {
        parts.push((l, b + cy, l + cx - 1, t - cy));
        parts.push((r - cx + 1, b + cy, r, t - cy));
    }
    (parts, Some((l + cx, b + cy, r - cx, t - cy)))
}

/// The area mix a `DB_*` raster operation stands for, `ps_mix` being the
/// presentation space's foreground mix (`DB_AREAMIXMODE`).
fn border_mix(rop: u32, ps_mix: Mix) -> Option<Mix> {
    match rop {
        DB_PATCOPY => Some(Mix::Overpaint),
        DB_PATINVERT => Some(Mix::Xor),
        DB_DESTINVERT => Some(Mix::Invert),
        DB_AREAMIXMODE => Some(ps_mix),
        _ => None,
    }
}

impl WindowManager {
    /// The window that gets `hwnd`'s paint messages and the offset from
    /// `hwnd`'s coordinates to its own: a standard frame paints through its
    /// client (the frame itself is Warpine's chrome).
    fn paint_target(&self, hwnd: u32) -> (u32, i32, i32) {
        match self.frame_to_client.get(&hwnd) {
            Some(&client) => {
                let (x, y) = self.get_window(client).map_or((0, 0), |w| (w.x, w.y));
                (client, -x, -y)
            }
            None => (hwnd, 0, 0),
        }
    }

    /// `(0, 0, cx, cy)` of `hwnd`.
    fn window_rectl(&self, hwnd: u32) -> Rectl {
        self.get_window(hwnd).map_or(EMPTY_RECTL, |w| Rectl::new(0, 0, w.cx, w.cy))
    }

    /// The update rectangle of `hwnd` (empty when it is valid).
    pub fn update_rect(&self, hwnd: u32) -> Rectl {
        let (target, ox, oy) = self.paint_target(hwnd);
        let r = self.get_window(target).map_or(EMPTY_RECTL, |w| w.update);
        if r.is_empty() { EMPTY_RECTL } else { r.offset(-ox, -oy) }
    }

    /// The child windows of `hwnd` that an invalidation or validation with
    /// `include_children` reaches, each with its offset into its own
    /// coordinates: none when `hwnd` clips its children and they are not
    /// asked for; only visible ones.
    fn paint_children(&self, hwnd: u32, include_children: bool) -> Vec<(u32, Rectl)> {
        let Some(w) = self.get_window(hwnd) else { return Vec::new() };
        if !include_children && w.style & WS_CLIPCHILDREN != 0 {
            return Vec::new();
        }
        w.children.iter().filter_map(|&c| {
            let cw = self.get_window(c)?;
            cw.visible.then(|| (c, Rectl::new(cw.x, cw.y, cw.x + cw.cx, cw.y + cw.cy)))
        }).collect()
    }

    /// WinInvalidateRect: add `rect` (window coordinates; `None` = the whole
    /// window) to the update rectangle of `hwnd` and of the descendants it
    /// reaches, and have each window with something to paint get a
    /// `WM_PAINT`. Marks the frame dirty.
    pub fn invalidate(&mut self, hwnd: u32, rect: Option<Rectl>, include_children: bool) {
        let (target, ox, oy) = self.paint_target(hwnd);
        let frame = self.client_to_frame(target);
        self.mark_dirty(frame);
        self.invalidate_window(target, rect.map(|r| r.offset(ox, oy)), include_children);
        // The frame itself repaints: its client is asked to erase first
        // (WM_ERASEBACKGROUND, pm_paint_style.rs).
        if target != hwnd {
            self.queue_erase_background(target);
        }
    }

    /// Invalidate the whole of `hwnd` but none of its children: a window
    /// created visible or resized repaints itself (its built-in controls are
    /// repainted with it by `WinEndPaint`).
    pub fn invalidate_window_only(&mut self, hwnd: u32) {
        let (target, ..) = self.paint_target(hwnd);
        let whole = self.window_rectl(target);
        if let Some(w) = self.get_window_mut(target) {
            w.update = w.update.union(&whole);
        }
        // A window of no size yet still gets its first WM_PAINT.
        self.post_paint(target);
        self.queue_sync_paint(target);
    }

    fn invalidate_window(&mut self, hwnd: u32, rect: Option<Rectl>, include_children: bool) {
        let whole = self.window_rectl(hwnd);
        let Some(r) = (match rect { None => Some(whole), Some(r) => r.intersect(&whole) }) else { return };
        if r.is_empty() { return; }
        if let Some(w) = self.get_window_mut(hwnd) {
            w.update = w.update.union(&r);
        }
        self.post_paint(hwnd);
        self.queue_sync_paint(hwnd);
        for (child, at) in self.paint_children(hwnd, include_children) {
            if let Some(part) = r.intersect(&at) {
                self.invalidate_window(child, Some(part.offset(-at.x_left, -at.y_bottom)), include_children);
            }
        }
    }

    /// Queue one `WM_PAINT` for `hwnd` unless one is waiting already.
    fn post_paint(&self, hwnd: u32) {
        let Some(mq_arc) = self.find_hmq_for_hwnd(hwnd).and_then(|h| self.get_mq(h)) else { return };
        let mut mq = mq_arc.lock_or_recover();
        if !mq.messages.iter().any(|m| m.hwnd == hwnd && m.msg == WM_PAINT) {
            mq.messages.push_back(OS2Message { hwnd, msg: WM_PAINT, mp1: 0, mp2: 0, time: 0, x: 0, y: 0 });
            mq.cond.notify_one();
        }
    }

    /// Withdraw the `WM_PAINT` waiting for `hwnd`, if any.
    fn withdraw_paint(&self, hwnd: u32) {
        if let Some(mq_arc) = self.find_hmq_for_hwnd(hwnd).and_then(|h| self.get_mq(h)) {
            mq_arc.lock_or_recover().messages.retain(|m| !(m.hwnd == hwnd && m.msg == WM_PAINT));
        }
    }

    /// WinValidateRect: remove `rect` (`None` = everything) from the update
    /// rectangle of `hwnd` and of the descendants it reaches. What is left
    /// must be a rectangle; when it would not be, the update rectangle stays.
    pub fn validate(&mut self, hwnd: u32, rect: Option<Rectl>, include_children: bool) {
        let (target, ox, oy) = self.paint_target(hwnd);
        self.validate_window(target, rect.map(|r| r.offset(ox, oy)), include_children);
    }

    fn validate_window(&mut self, hwnd: u32, rect: Option<Rectl>, include_children: bool) {
        let Some(w) = self.get_window_mut(hwnd) else { return };
        w.update = match rect {
            None => EMPTY_RECTL,
            Some(r) => match w.update.subtract(&r) {
                left if left.is_empty() => EMPTY_RECTL,
                left => left,
            },
        };
        if w.update.is_empty() {
            self.withdraw_paint(hwnd);
        }
        let whole = self.window_rectl(hwnd);
        for (child, at) in self.paint_children(hwnd, include_children) {
            let part = match rect {
                None => None,
                Some(r) => match r.intersect(&whole).and_then(|r| r.intersect(&at)) {
                    Some(p) => Some(p.offset(-at.x_left, -at.y_bottom)),
                    None => continue,
                },
            };
            self.validate_window(child, part, include_children);
        }
    }

    /// WinBeginPaint: the update rectangle of `hwnd`, which becomes empty.
    pub fn begin_paint(&mut self, hwnd: u32) -> Rectl {
        let (target, ox, oy) = self.paint_target(hwnd);
        let Some(w) = self.get_window_mut(target) else { return EMPTY_RECTL };
        let r = std::mem::replace(&mut w.update, EMPTY_RECTL);
        self.withdraw_paint(target);
        if r.is_empty() { r } else { r.offset(-ox, -oy) }
    }

    /// Clip `hps` to update rectangle `r` (window coordinates) for a
    /// `WinBeginPaint`; an empty `r` removes the paint clip. (A
    /// `WinBeginPaint` with nothing to update draws unclipped: Warpine's
    /// choice, so painting outside `WM_PAINT` still shows.)
    pub fn set_paint_clip(&mut self, hps: u32, r: Rectl) {
        let Some(ps) = self.ps_map.get_mut(&hps) else { return };
        ps.paint_clip = (!r.is_empty())
            .then(|| crate::gui::region::RRect::new(r.x_left, r.y_bottom, r.x_right, r.y_top));
        self.refresh_clip(hps);
    }

    /// WinUpdateWindow: `hwnd` and its descendants with a window procedure
    /// and something to paint get `WM_PAINT` before the call returns,
    /// parents first (the owner-call queue, as `WS_SYNCPAINT`).
    pub fn queue_update_window(&mut self, hwnd: u32) {
        let mut stack = vec![hwnd];
        let mut left = MAX_UPDATE_WINDOWS;
        while let Some(h) = stack.pop() {
            if left == 0 { break; }
            left -= 1;
            let Some(w) = self.get_window(h) else { continue };
            let (paint, kids) = (w.pfn_wp != 0 && w.visible && !w.update.is_empty(), w.children.clone());
            if paint {
                self.queue_owner_call(super::pm_owner::OwnerCall {
                    control: h, kind: super::pm_owner::OwnerCallKind::SyncPaint,
                });
            }
            stack.extend(kids.into_iter().rev());
        }
    }

    /// The whole of `hwnd` was painted without `WinBeginPaint` (a built-in
    /// control, `WinDefWindowProc`): it is valid now.
    pub fn painted(&mut self, hwnd: u32) {
        if let Some(w) = self.get_window_mut(hwnd) {
            w.update = EMPTY_RECTL;
        }
    }
}

impl Loader {
    /// WinInvalidateRect(hwnd, prcl, fIncludeChildren) → BOOL.
    /// `HWND_DESKTOP` invalidates every frame with its children.
    pub(super) fn win_invalidate_rect(&self, tid: u32, hwnd: u32, prcl: u32, include: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let rect = match prcl {
            0 => None,
            p => match self.read_rectl(p) { Some(r) => Some(r), None => return ApiResult::Normal(0) },
        };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if hwnd == HWND_DESKTOP || hwnd == HWND_OBJECT {
            if hwnd == HWND_DESKTOP {
                for frame in wm.z_order.clone() {
                    wm.invalidate(frame, None, true);
                }
            }
            return ApiResult::Normal(1);
        }
        wm.invalidate(hwnd, rect, include != 0);
        ApiResult::Normal(1)
    }

    /// WinValidateRect(hwnd, prcl, fIncludeClippedChildren) → BOOL.
    pub(super) fn win_validate_rect(&self, tid: u32, hwnd: u32, prcl: u32, include: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let rect = match prcl {
            0 => None,
            p => match self.read_rectl(p) { Some(r) => Some(r), None => return ApiResult::Normal(0) },
        };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if hwnd == HWND_DESKTOP {
            for frame in wm.z_order.clone() {
                wm.validate(frame, None, true);
            }
        } else if hwnd != HWND_OBJECT {
            wm.validate(hwnd, rect, include != 0);
        }
        ApiResult::Normal(1)
    }

    /// WinQueryUpdateRect(hwnd, prcl) → BOOL: TRUE when `hwnd` has
    /// something to paint; `prcl` (may be NULL) receives the update
    /// rectangle, (0,0,0,0) when there is none.
    pub(super) fn win_query_update_rect(&self, tid: u32, hwnd: u32, prcl: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let r = self.shared.window_mgr.lock_or_recover().update_rect(hwnd);
        let r = if r.is_empty() { EMPTY_RECTL } else { r };
        if prcl != 0 && !self.write_rectl(prcl, r) { return ApiResult::Normal(0); }
        ApiResult::Normal(!r.is_empty() as u32)
    }

    /// WinBeginPaint's update rectangle, written to `prcl` (if not NULL)
    /// and returned.
    pub(super) fn begin_paint_rect(&self, hwnd: u32, prcl: u32) -> Rectl {
        let r = self.shared.window_mgr.lock_or_recover().begin_paint(hwnd);
        if prcl != 0 {
            self.write_rectl(prcl, r);
        }
        r
    }

    /// WinScrollWindow(hwnd, dx, dy, prclScroll, prclClip, hrgnUpdate,
    /// prclUpdate, flOptions) → LONG complexity of the uncovered area.
    ///
    /// The pels of the scroll rectangle (NULL = the window) move by
    /// `(dx, dy)`; what lands outside the clip rectangle (NULL = the scroll
    /// rectangle) is dropped. The part of the scroll rectangle no moved pel
    /// covers is "uncovered": its bounding rectangle goes to `prclUpdate`,
    /// and with `SW_INVALIDATERGN` it joins the update rectangle (with the
    /// part of the old update rectangle in the scroll rectangle moved too).
    /// `SW_SCROLLCHILDREN` moves the child windows (with a scroll
    /// rectangle, those that intersect it). `hrgnUpdate`, when given, is set
    /// to the uncovered area (window coordinates).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn win_scroll_window(&self, tid: u32, hwnd: u32, dx: i32, dy: i32, prcl_scroll: u32,
                                    prcl_clip: u32, hrgn: u32, prcl_update: u32, fl: u32) -> ApiResult {
        if fl & !(SW_SCROLLCHILDREN | SW_INVALIDATERGN) != 0 {
            self.pm_error(tid, PMERR_INVALID_FLAG);
            return ApiResult::Normal(RGN_ERROR);
        }
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(RGN_ERROR); }
        if hwnd == HWND_DESKTOP || hwnd == HWND_OBJECT { return ApiResult::Normal(RGN_ERROR); }
        let read = |p: u32| -> Result<Option<Rectl>, ()> {
            if p == 0 { Ok(None) } else { self.read_rectl(p).map(Some).ok_or(()) }
        };
        let (Ok(scroll), Ok(clip)) = (read(prcl_scroll), read(prcl_clip)) else {
            return ApiResult::Normal(RGN_ERROR);
        };
        if self.scroll_update_region(tid, hrgn).is_err() { return ApiResult::Normal(RGN_ERROR); }
        let (dx, dy) = (dx.clamp(-MAX_SCROLL, MAX_SCROLL), dy.clamp(-MAX_SCROLL, MAX_SCROLL));

        let mut wm = self.shared.window_mgr.lock_or_recover();
        let (target, ox, oy) = wm.paint_target(hwnd);
        let whole = wm.window_rectl(target);
        let s = match scroll {
            None => Some(whole),
            Some(r) => r.offset(ox, oy).intersect(&whole),
        };
        let c = match clip {
            None => s,
            Some(r) => r.offset(ox, oy).intersect(&whole),
        };
        let area = s.zip(c).and_then(|(s, c)| s.intersect(&c));
        let dest = s.zip(c).and_then(|(s, c)| s.offset(dx, dy).intersect(&c));
        let uncovered = match (area, dest) {
            (Some(a), Some(d)) => rect_difference(&a, &d),
            (Some(a), None) => vec![a],
            (None, _) => Vec::new(),
        };
        let (bound, complexity) = region_of(&uncovered);

        // Move the pels on the frame's surface.
        let frame = wm.find_frame_for_hwnd(target);
        if let Some(d) = dest {
            let (ax, ay, _, _) = wm.get_abs_rect_in_frame(target);
            if let Some(surface) = wm.frame_surface(frame) {
                let d = d.offset(ax, ay);
                surface.lock_or_recover().scroll(d.x_left, d.y_bottom, d.x_right, d.y_top, dx, dy);
            }
        }
        wm.mark_dirty(frame);

        if fl & SW_SCROLLCHILDREN != 0 {
            let children = wm.get_window(target).map(|w| w.children.clone()).unwrap_or_default();
            for child in children {
                let Some(w) = wm.get_window_mut(child) else { continue };
                let at = Rectl::new(w.x, w.y, w.x + w.cx, w.y + w.cy);
                if scroll.is_none() || s.is_some_and(|s| s.intersect(&at).is_some()) {
                    w.x = (w.x + dx).clamp(-MAX_WINDOW_COORD, MAX_WINDOW_COORD);
                    w.y = (w.y + dy).clamp(-MAX_WINDOW_COORD, MAX_WINDOW_COORD);
                }
            }
        }

        if fl & SW_INVALIDATERGN != 0 {
            // The part of the old update rectangle in the scroll area moves.
            let old = wm.get_window(target).map_or(EMPTY_RECTL, |w| w.update);
            if let Some(moved) = area.and_then(|a| old.intersect(&a))
                .and_then(|m| c.and_then(|c| m.offset(dx, dy).intersect(&c)))
            {
                wm.invalidate(target, Some(moved), false);
            }
            for r in &uncovered {
                wm.invalidate(target, Some(*r), false);
            }
        }
        drop(wm);

        let in_window: Vec<Rectl> = uncovered.iter().map(|r| r.offset(-ox, -oy)).collect();
        self.set_region_rects(hrgn, &in_window);
        if prcl_update != 0 {
            let out = if bound.is_empty() { EMPTY_RECTL } else { bound.offset(-ox, -oy) };
            self.write_rectl(prcl_update, out);
        }
        ApiResult::Normal(complexity)
    }

    /// WinDrawBorder(hps, prcl, cx, cy, clrFore, clrBack, flCmd) → BOOL.
    ///
    /// The border runs inside `prcl` (exclusive at the right and top),
    /// `cx` pels wide at the left and right and `cy` at the bottom and top
    /// (times `SV_CXBORDER` / `SV_CYBORDER` with `DB_STANDARD`); with
    /// `DB_INTERIOR` the rest is filled too. The border is `clrFore` on
    /// `clrBack`, the interior the reverse, or with `DB_AREAATTRS` the
    /// presentation space's area pattern and colours (reversed inside),
    /// combined with the destination by the `DB_ROP` operation.
    /// `DB_DLGBORDER` draws in the active (with `DB_PATCOPY`) or inactive
    /// (with `DB_PATINVERT`) title-bar colour, the interior in `clrBack`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn win_draw_border(&self, tid: u32, hps: u32, prcl: u32, cx: i32, cy: i32,
                                  fore: u32, back: u32, fl: u32) -> ApiResult {
        if fl & !DB_ALL_FLAGS != 0 {
            return self.pm_fail(tid, PMERR_INVALID_FLAG);
        }
        let rop = fl & DB_ROP;
        if rop > DB_AREAMIXMODE {
            return self.pm_fail(tid, PMERR_INV_DRAW_BORDER_OPTION);
        }
        let Some(t) = self.ps_target(hps) else { return self.pm_fail(tid, PMERR_INV_HPS) };
        let Some(rc) = self.read_rectl(prcl) else { return ApiResult::Normal(0) };
        let (cx, cy) = if fl & DB_STANDARD != 0 {
            (cx.saturating_mul(super::pm_win::sys_value(SV_CXBORDER)),
             cy.saturating_mul(super::pm_win::sys_value(SV_CYBORDER)))
        } else {
            (cx, cy)
        };
        let (fore, back) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let ps = &wm.ps_map[&hps];
            (ps.resolve_color(fore), ps.resolve_color(back))
        };
        let solid = |fg: u32, bg: u32| Pattern { fg, bg, ..Pattern::solid(fg) };
        let (border, interior, mix) = if fl & DB_DLGBORDER != 0 {
            let color = if rop == DB_PATINVERT { DLG_BORDER_INACTIVE_COLOR } else { DLG_BORDER_ACTIVE_COLOR };
            (Pattern::solid(color), Pattern::solid(back), Mix::Overpaint)
        } else {
            let Some(mix) = border_mix(rop, t.mix) else { return self.pm_fail(tid, PMERR_INV_DRAW_BORDER_OPTION) };
            if fl & DB_AREAATTRS != 0 {
                let p = t.pattern;
                (p, Pattern { fg: p.bg, bg: p.fg, ..p }, mix)
            } else {
                (solid(fore, back), solid(back, fore), mix)
            }
        };
        let fill = |pattern: Pattern| Some(AreaFill { pattern, mix, back_mix: Some(mix) });
        let shape = |(x1, y1, x2, y2): Corners, pattern: Pattern| DrawOp::Shape {
            points: vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)], closed: true, fill: fill(pattern), stroke: None,
        };
        let (parts, inside) = border_parts(&rc, cx, cy);
        for part in parts {
            self.draw_ps(&t, shape(part, border));
        }
        if fl & DB_INTERIOR != 0
            && let Some(inside) = inside
        {
            self.draw_ps(&t, shape(inside, interior));
        }
        ApiResult::Normal(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::vm_backend::mock::MockVcpu;
    use crate::loader::pm_error::error_id;

    fn call(loader: &Loader, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
        match loader.handle_pmwin_call(&mut vcpu, 0, ordinal) {
            ApiResult::Normal(v) => v,
            other => panic!("PMWIN ordinal {} returned {:?}", ordinal, other),
        }
    }

    // PMWIN ordinals (doc/os2_ordinals.md).
    const WIN_DRAW_BORDER: u32 = 731;
    const WIN_QUERY_UPDATE_RECT: u32 = 831;
    const WIN_SCROLL_WINDOW: u32 = 849;
    const WIN_VALIDATE_RECT: u32 = 895;
    const ORD_WIN_BEGIN_PAINT: u32 = 703;
    const ORD_WIN_INVALIDATE_RECT: u32 = 765;
    const ORD_WIN_CREATE_MSG_QUEUE: u32 = 716;
    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const RC: u32 = 0x3000;
    const OUT: u32 = 0x3100;

    fn put_rect(loader: &Loader, at: u32, r: Rectl) {
        assert!(loader.write_rectl(at, r));
    }

    fn get_rect(loader: &Loader, at: u32) -> Rectl {
        loader.read_rectl(at).unwrap()
    }

    fn last_error(loader: &Loader) -> u32 {
        call(loader, ORD_WIN_GET_LAST_ERROR, &[0])
    }

    /// A queue, and a 200×100 visible window of it with a 40×20 child at (10, 10).
    fn setup() -> (Loader, u32, u32, u32) {
        let loader = Loader::new_mock();
        let hmq = call(&loader, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        let (win, child) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let win = wm.create_window("App".into(), 0, hmq);
            let w = wm.get_window_mut(win).unwrap();
            (w.cx, w.cy, w.visible) = (200, 100, true);
            let child = wm.create_window("Kid".into(), win, hmq);
            let c = wm.get_window_mut(child).unwrap();
            (c.x, c.y, c.cx, c.cy, c.visible) = (10, 10, 40, 20, true);
            (win, child)
        };
        (loader, hmq, win, child)
    }

    fn paints(loader: &Loader, hmq: u32) -> Vec<u32> {
        let wm = loader.shared.window_mgr.lock_or_recover();
        let mq = wm.get_mq(hmq).unwrap();
        let mq = mq.lock_or_recover();
        mq.messages.iter().filter(|m| m.msg == WM_PAINT).map(|m| m.hwnd).collect()
    }

    fn update(loader: &Loader, hwnd: u32) -> Rectl {
        loader.shared.window_mgr.lock_or_recover().update_rect(hwnd)
    }

    const ORD_WIN_FILL_RECT: u32 = 743;
    const ORD_WIN_GET_PS: u32 = 757;
    const ORD_WIN_END_PAINT: u32 = 738;
    const ORD_WIN_SHOW_WINDOW: u32 = 883;
    const ORD_WIN_UPDATE_WINDOW: u32 = 892;
    const ORD_GPI_QUERY_PEL: u32 = 476;
    const RED: u32 = 0x00FF_0000;

    /// A 300×200 frame whose client sits at (5, 10) in the frame's surface,
    /// 280×170, with a 40×30 child of the client at (20, 30).
    fn framed() -> (Loader, u32, u32, u32, u32) {
        let loader = Loader::new_mock();
        let hmq = call(&loader, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        let (frame, client, child) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let frame = wm.create_window("Frame".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            let child = wm.create_window("Kid".into(), client, hmq);
            for (h, geo) in [(frame, (0, 0, 300, 200)), (client, (5, 10, 280, 170)), (child, (20, 30, 40, 30))] {
                let w = wm.get_window_mut(h).unwrap();
                (w.x, w.y, w.cx, w.cy) = geo;
                w.visible = true;
            }
            (frame, client, child)
        };
        (loader, hmq, frame, client, child)
    }

    fn pel(loader: &Loader, frame: u32, x: i32, y: i32) -> Option<u32> {
        let s = loader.shared.window_mgr.lock_or_recover().frame_surface(frame).unwrap();
        let s = s.lock_or_recover();
        s.pel(x, y)
    }

    fn fill(loader: &Loader, hps: u32, r: Rectl, color: u32) {
        put_rect(loader, RC, r);
        assert_eq!(call(loader, ORD_WIN_FILL_RECT, &[hps, RC, color]), 1);
    }

    fn query_pel(loader: &Loader, hps: u32, x: i32, y: i32) -> u32 {
        let mut vcpu = MockVcpu::new();
        vcpu.regs.rsp = 0x1000;
        for (i, v) in [hps, OUT + 0x40].iter().enumerate() { loader.guest_write::<u32>(0x1004 + 4 * i as u32, *v).unwrap(); }
        loader.guest_write::<i32>(OUT + 0x40, x).unwrap();
        loader.guest_write::<i32>(OUT + 0x44, y).unwrap();
        match loader.handle_pmgpi_call(&mut vcpu, 0, ORD_GPI_QUERY_PEL) {
            ApiResult::Normal(v) => v,
            other => panic!("{other:?}"),
        }
    }

    /// A child window's PS draws at the child's place in its frame's
    /// surface, clipped to the child, and reads back from there.
    #[test]
    fn a_child_windows_ps_draws_at_its_place() {
        let (loader, _, frame, _, child) = framed();
        let hps = call(&loader, ORD_WIN_GET_PS, &[child]);
        assert_ne!(hps, 0);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().ps_draw_target(hps),
                   Some(super::super::pm_types::DrawTarget::Child { frame, hwnd: child }));
        let before = pel(&loader, frame, 5, 5);
        fill(&loader, hps, Rectl::new(-10, -10, 100, 100), RED);
        // The child spans the frame surface's (25, 40) .. (65, 70).
        assert_eq!(pel(&loader, frame, 25, 40), Some(RED), "its bottom-left corner");
        assert_eq!(pel(&loader, frame, 64, 69), Some(RED), "its top-right pel");
        assert_eq!(pel(&loader, frame, 65, 50), before, "clipped at its right edge");
        assert_eq!(pel(&loader, frame, 24, 50), before, "and its left");
        assert_eq!(pel(&loader, frame, 5, 5), before, "the rest is left alone");
        // GpiQueryPel reads the child's own coordinates.
        assert_eq!(query_pel(&loader, hps, 0, 0), 2, "red, as its colour-table index (CLR_RED)");
        assert_eq!(query_pel(&loader, hps, 40, 0), CLR_NOINDEX as u32, "outside the child");
    }

    /// WinBeginPaint clips its PS to the update rectangle until WinEndPaint.
    #[test]
    fn a_begin_paint_ps_is_clipped_to_the_update_rectangle() {
        let (loader, _, frame, client, _) = framed();
        // Paint the client white, validate it, then invalidate (10,10)-(30,30).
        let hps = call(&loader, ORD_WIN_GET_PS, &[client]);
        fill(&loader, hps, Rectl::new(0, 0, 280, 170), 0x00FF_FFFF);
        loader.shared.window_mgr.lock_or_recover().validate(client, None, true);
        put_rect(&loader, RC, Rectl::new(10, 10, 30, 30));
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[client, RC, 0]);
        let ps = call(&loader, ORD_WIN_BEGIN_PAINT, &[client, 0, OUT]);
        assert_eq!(get_rect(&loader, OUT), Rectl::new(10, 10, 30, 30));
        fill(&loader, ps, Rectl::new(0, 0, 280, 170), RED);
        // The client is at (5, 10) in the frame's surface.
        assert_eq!(pel(&loader, frame, 5 + 15, 10 + 15), Some(RED), "inside the update rectangle");
        assert_eq!(pel(&loader, frame, 5 + 50, 10 + 50), Some(0x00FF_FFFF), "outside: clipped");
        assert_eq!(call(&loader, ORD_WIN_END_PAINT, &[ps]), 1);
        // A PS passed in loses the clip at WinEndPaint; nothing to update
        // means no clip at all.
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[client, RC, 0]);
        assert_eq!(call(&loader, ORD_WIN_BEGIN_PAINT, &[client, hps, OUT]), hps);
        assert!(loader.shared.window_mgr.lock_or_recover().ps_map[&hps].paint_clip.is_some());
        call(&loader, ORD_WIN_END_PAINT, &[hps]);
        assert!(loader.shared.window_mgr.lock_or_recover().ps_map[&hps].paint_clip.is_none());
        assert_eq!(call(&loader, ORD_WIN_BEGIN_PAINT, &[client, hps, OUT]), hps);
        assert!(loader.shared.window_mgr.lock_or_recover().ps_map[&hps].paint_clip.is_none());
    }

    /// WinShowWindow invalidates a window it shows; WinUpdateWindow has the
    /// window and its descendants that need painting paint before it returns.
    #[test]
    fn show_invalidates_and_update_window_paints_at_once() {
        let (loader, hmq, _, client, child) = framed();
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            for h in [client, child] {
                let w = wm.get_window_mut(h).unwrap();
                w.pfn_wp = 0x5000;
            }
            wm.validate(client, None, true);
            wm.get_window_mut(child).unwrap().visible = false;
        }
        call(&loader, ORD_WIN_SHOW_WINDOW, &[child, 1]);
        assert!(!update(&loader, child).is_empty(), "shown: to be painted");
        assert!(paints(&loader, hmq).contains(&child));
        call(&loader, ORD_WIN_SHOW_WINDOW, &[child, 1]);
        // WinUpdateWindow on the client: only the child needs painting.
        loader.shared.window_mgr.lock_or_recover().owner_calls.clear();
        assert_eq!(call(&loader, ORD_WIN_UPDATE_WINDOW, &[client]), 1);
        let calls: Vec<u32> = loader.shared.window_mgr.lock_or_recover().owner_calls.iter()
            .filter(|c| c.kind == super::super::pm_owner::OwnerCallKind::SyncPaint).map(|c| c.control).collect();
        assert_eq!(calls, vec![child]);
    }

    #[test]
    fn difference_and_complexity() {
        let a = Rectl::new(0, 0, 10, 10);
        assert!(rect_difference(&a, &a).is_empty());
        assert_eq!(rect_difference(&a, &Rectl::new(20, 20, 30, 30)), vec![a]);
        // Scrolled up by 3: the bottom band is uncovered.
        let d = rect_difference(&a, &Rectl::new(0, 3, 10, 13));
        assert_eq!(d, vec![Rectl::new(0, 0, 10, 3)]);
        assert_eq!(region_of(&d), (Rectl::new(0, 0, 10, 3), RGN_RECT));
        // Diagonal: an L shape.
        let d = rect_difference(&a, &Rectl::new(2, 3, 12, 13));
        assert_eq!(region_of(&d), (a, RGN_COMPLEX));
        assert_eq!(d.iter().map(area).sum::<i64>(), 100 - 8 * 7);
        assert_eq!(region_of(&[]), (EMPTY_RECTL, RGN_NULL));
        // A hole: four pieces.
        assert_eq!(rect_difference(&a, &Rectl::new(2, 2, 8, 8)).len(), 4);
    }

    #[test]
    fn border_parts_follow_the_reference() {
        let rc = Rectl::new(10, 20, 30, 40); // 20 × 20
        let (parts, inside) = border_parts(&rc, 2, 3);
        assert_eq!(parts, vec![(10, 20, 29, 22), (10, 37, 29, 39), (10, 23, 11, 36), (28, 23, 29, 36)]);
        assert_eq!(inside, Some((12, 23, 27, 36)));
        // No border: everything is interior.
        assert_eq!(border_parts(&rc, 0, 0), (vec![], Some((10, 20, 29, 39))));
        // Borders that meet: one rectangle, no interior.
        assert_eq!(border_parts(&rc, 10, 1), (vec![(10, 20, 29, 39)], None));
        // Only horizontal sides.
        assert_eq!(border_parts(&rc, 0, 1).0.len(), 2);
        assert_eq!(border_parts(&Rectl::new(5, 5, 5, 9), 1, 1), (vec![], None));
    }

    #[test]
    fn invalidate_accumulates_and_posts_one_paint() {
        let (loader, hmq, win, child) = setup();
        put_rect(&loader, RC, Rectl::new(100, 50, 150, 90));
        // fIncludeChildren FALSE, no WS_CLIPCHILDREN: the child is reached
        // only where the rectangle meets it (here not).
        assert_eq!(call(&loader, ORD_WIN_INVALIDATE_RECT, &[win, RC, 0]), 1);
        assert_eq!(update(&loader, win), Rectl::new(100, 50, 150, 90));
        assert_eq!(update(&loader, child), EMPTY_RECTL);
        put_rect(&loader, RC, Rectl::new(0, 0, 20, 15));
        assert_eq!(call(&loader, ORD_WIN_INVALIDATE_RECT, &[win, RC, 0]), 1);
        assert_eq!(update(&loader, win), Rectl::new(0, 0, 150, 90), "union");
        assert_eq!(update(&loader, child), Rectl::new(0, 0, 10, 5), "child coordinates");
        assert_eq!(paints(&loader, hmq), vec![win, child], "one WM_PAINT per window");
        // Outside the window: clipped, nothing new.
        put_rect(&loader, RC, Rectl::new(500, 500, 600, 600));
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[win, RC, 0]);
        assert_eq!(update(&loader, win), Rectl::new(0, 0, 150, 90));
        // NULL: the whole window.
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[win, 0, 0]);
        assert_eq!(update(&loader, win), Rectl::new(0, 0, 200, 100));
        assert_eq!(update(&loader, child), Rectl::new(0, 0, 40, 20));
    }

    #[test]
    fn clip_children_keeps_children_out_unless_included() {
        let (loader, hmq, win, child) = setup();
        loader.shared.window_mgr.lock_or_recover().get_window_mut(win).unwrap().style |= WS_CLIPCHILDREN;
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[win, 0, 0]);
        assert_eq!(update(&loader, child), EMPTY_RECTL);
        assert_eq!(paints(&loader, hmq), vec![win]);
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[win, 0, 1]);
        assert_eq!(update(&loader, child), Rectl::new(0, 0, 40, 20));
        // An invisible child is not invalidated.
        let (loader, _, win, child) = setup();
        loader.shared.window_mgr.lock_or_recover().get_window_mut(child).unwrap().visible = false;
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[win, 0, 1]);
        assert_eq!(update(&loader, child), EMPTY_RECTL);
    }

    #[test]
    fn validate_query_and_begin_paint() {
        let (loader, hmq, win, child) = setup();
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[win, 0, 0]);
        // A band off the bottom leaves a rectangle.
        put_rect(&loader, RC, Rectl::new(0, 0, 200, 30));
        assert_eq!(call(&loader, WIN_VALIDATE_RECT, &[win, RC, 0]), 1);
        assert_eq!(update(&loader, win), Rectl::new(0, 30, 200, 100));
        assert_eq!(update(&loader, child), EMPTY_RECTL, "the child lies inside the band");
        assert!(!paints(&loader, hmq).contains(&child), "its WM_PAINT is withdrawn");
        // A hole would not leave a rectangle: unchanged.
        put_rect(&loader, RC, Rectl::new(50, 50, 60, 60));
        call(&loader, WIN_VALIDATE_RECT, &[win, RC, 0]);
        assert_eq!(update(&loader, win), Rectl::new(0, 30, 200, 100));
        assert_eq!(call(&loader, WIN_QUERY_UPDATE_RECT, &[win, OUT]), 1);
        assert_eq!(get_rect(&loader, OUT), Rectl::new(0, 30, 200, 100));
        assert_eq!(call(&loader, WIN_QUERY_UPDATE_RECT, &[win, 0]), 1, "NULL rectangle");
        // WinBeginPaint reports it and empties it.
        let hps = call(&loader, ORD_WIN_BEGIN_PAINT, &[win, 0, OUT]);
        assert_ne!(hps, 0);
        assert_eq!(get_rect(&loader, OUT), Rectl::new(0, 30, 200, 100));
        assert_eq!(update(&loader, win), EMPTY_RECTL);
        assert!(paints(&loader, hmq).is_empty());
        assert_eq!(call(&loader, WIN_QUERY_UPDATE_RECT, &[win, OUT]), 0);
        assert_eq!(get_rect(&loader, OUT), EMPTY_RECTL);
        // NULL validates everything.
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[win, 0, 0]);
        call(&loader, WIN_VALIDATE_RECT, &[win, 0, 0]);
        assert_eq!(update(&loader, win), EMPTY_RECTL);
        assert!(paints(&loader, hmq).is_empty(), "no WM_PAINT without an update rectangle");
        // A bad window.
        assert_eq!(call(&loader, WIN_QUERY_UPDATE_RECT, &[0xDEAD, OUT]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
        assert_eq!(call(&loader, WIN_VALIDATE_RECT, &[0xDEAD, 0, 0]), 0);
    }

    #[test]
    fn begin_paint_on_a_frame_reports_the_clients_update() {
        let loader = Loader::new_mock();
        let hmq = call(&loader, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        let (frame, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let frame = wm.create_window("Frame".into(), 0, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            for (h, cx, cy) in [(frame, 300, 200), (client, 300, 180)] {
                let w = wm.get_window_mut(h).unwrap();
                (w.cx, w.cy, w.visible) = (cx, cy, true);
            }
            (frame, client)
        };
        put_rect(&loader, RC, Rectl::new(10, 10, 20, 20));
        call(&loader, ORD_WIN_INVALIDATE_RECT, &[frame, RC, 0]);
        assert_eq!(update(&loader, client), Rectl::new(10, 10, 20, 20));
        assert_eq!(paints(&loader, hmq), vec![client], "the frame paints through its client");
        assert_eq!(update(&loader, frame), Rectl::new(10, 10, 20, 20));
    }

    fn surface_of(loader: &Loader, win: u32) -> crate::gui::SharedSurface {
        loader.shared.window_mgr.lock_or_recover().frame_surface(win).unwrap()
    }

    #[test]
    fn scroll_window_moves_pels_and_reports_the_uncovered_area() {
        let (loader, hmq, win, child) = setup();
        let surface = surface_of(&loader, win);
        {
            let mut s = surface.lock_or_recover();
            s.draw(DrawOp::Box { x1: 0, y1: 0, x2: 9, y2: 9, color: 0xFF0000, fill: true, mix: Mix::Overpaint });
            s.take_changes();
        }
        // Scroll up 20 and right 5: the red square moves to (5..14, 20..29).
        let r = call(&loader, WIN_SCROLL_WINDOW, &[win, 5, 20, 0, 0, 0, OUT, SW_INVALIDATERGN | SW_SCROLLCHILDREN]);
        assert_eq!(r, RGN_COMPLEX, "an L-shaped uncovered area");
        assert_eq!(get_rect(&loader, OUT), Rectl::new(0, 0, 200, 100));
        {
            let mut s = surface.lock_or_recover();
            assert_eq!(s.pel(5, 20), Some(0xFF0000));
            assert_eq!(s.pel(14, 29), Some(0xFF0000));
            assert_eq!(s.pel(15, 29), Some(crate::gui::surface::SURFACE_BLANK & 0xFF_FFFF));
            let changed = s.take_changes();
            assert!(!changed.is_empty(), "the move is a change for the layered compositor");
        }
        assert_eq!(update(&loader, win), Rectl::new(0, 0, 200, 100));
        assert!(paints(&loader, hmq).contains(&win));
        let wm = loader.shared.window_mgr.lock_or_recover();
        let c = wm.get_window(child).unwrap();
        assert_eq!((c.x, c.y), (15, 30), "the child moved");
        assert!(wm.dirty.contains(&win));
    }

    #[test]
    fn scroll_window_with_a_scroll_rectangle_and_no_invalidation() {
        let (loader, hmq, win, child) = setup();
        let surface = surface_of(&loader, win);
        surface.lock_or_recover().draw(DrawOp::Box {
            x1: 100, y1: 40, x2: 109, y2: 49, color: 0x00FF00, fill: true, mix: Mix::Overpaint });
        // Scroll the rectangle (100,0)-(200,100) down by 10.
        put_rect(&loader, RC, Rectl::new(100, 0, 200, 100));
        let r = call(&loader, WIN_SCROLL_WINDOW, &[win, 0, (-10i32) as u32, RC, 0, 0, OUT, SW_SCROLLCHILDREN]);
        assert_eq!(r, RGN_RECT);
        assert_eq!(get_rect(&loader, OUT), Rectl::new(100, 90, 200, 100), "the top band");
        let s = surface.lock_or_recover();
        assert_eq!(s.pel(100, 30), Some(0x00FF00));
        assert_eq!(s.pel(100, 39), Some(0x00FF00));
        assert_ne!(s.pel(100, 45), Some(0x00FF00), "blank pels moved over the old square");
        assert_ne!(s.pel(99, 30), Some(0x00FF00), "left of the scroll rectangle: unchanged");
        drop(s);
        assert_eq!(update(&loader, win), EMPTY_RECTL, "no SW_INVALIDATERGN");
        assert!(paints(&loader, hmq).is_empty());
        let wm = loader.shared.window_mgr.lock_or_recover();
        let c = wm.get_window(child).unwrap();
        assert_eq!((c.x, c.y), (10, 10), "the child is outside the scroll rectangle");
    }

    #[test]
    fn scroll_window_null_and_error_cases() {
        let (loader, _, win, _) = setup();
        assert_eq!(call(&loader, WIN_SCROLL_WINDOW, &[win, 0, 0, 0, 0, 0, OUT, 0]), RGN_NULL);
        assert_eq!(get_rect(&loader, OUT), EMPTY_RECTL);
        // Scrolled entirely away: the whole window is uncovered.
        assert_eq!(call(&loader, WIN_SCROLL_WINDOW, &[win, 0, 1_000_000, 0, 0, 0, OUT, 0]), RGN_RECT);
        assert_eq!(get_rect(&loader, OUT), Rectl::new(0, 0, 200, 100));
        assert_eq!(call(&loader, WIN_SCROLL_WINDOW, &[win, 0, i32::MIN as u32, 0, 0, 0, 0, 0]), RGN_RECT);
        assert_eq!(call(&loader, WIN_SCROLL_WINDOW, &[win, 0, 1, 0, 0, 0, 0, 4]), RGN_ERROR);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_FLAG));
        assert_eq!(call(&loader, WIN_SCROLL_WINDOW, &[0xDEAD, 0, 1, 0, 0, 0, 0, 0]), RGN_ERROR);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
        // A clip rectangle limits where pels land and what is uncovered.
        put_rect(&loader, RC, Rectl::new(0, 0, 50, 50));
        assert_eq!(call(&loader, WIN_SCROLL_WINDOW, &[win, 0, 10, 0, RC, 0, OUT, 0]), RGN_RECT);
        assert_eq!(get_rect(&loader, OUT), Rectl::new(0, 0, 50, 10));
    }

    fn border_ps(loader: &Loader) -> (u32, crate::gui::SharedSurface) {
        let (hps, win) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let win = wm.create_window("App".into(), 0, 0);
            let w = wm.get_window_mut(win).unwrap();
            (w.cx, w.cy) = (100, 100);
            (wm.create_ps(win), win)
        };
        (hps, surface_of(loader, win))
    }

    const CLR_RED: u32 = 2;
    const CLR_BLUE: u32 = 1;

    #[test]
    fn draw_border_paints_border_and_interior() {
        let loader = Loader::new_mock();
        let (hps, surface) = border_ps(&loader);
        put_rect(&loader, RC, Rectl::new(10, 10, 30, 30));
        assert_eq!(call(&loader, WIN_DRAW_BORDER, &[hps, RC, 2, 3, CLR_RED, CLR_BLUE, DB_PATCOPY | DB_INTERIOR]), 1);
        let s = surface.lock_or_recover();
        assert_eq!(s.pel(10, 10), Some(0xFF0000), "bottom-left corner");
        assert_eq!(s.pel(29, 29), Some(0xFF0000), "the top and right edges are inside the rectangle");
        assert_ne!(s.pel(30, 30), Some(0xFF0000));
        assert_eq!(s.pel(11, 20), Some(0xFF0000), "left side, 2 wide");
        assert_eq!(s.pel(12, 20), Some(0x0000FF), "interior in clrBack");
        assert_eq!(s.pel(20, 12), Some(0xFF0000), "bottom side, 3 high");
        assert_eq!(s.pel(20, 13), Some(0x0000FF));
    }

    #[test]
    fn draw_border_raster_operations() {
        let loader = Loader::new_mock();
        let (hps, surface) = border_ps(&loader);
        let blank = surface.lock_or_recover().pel(0, 0).unwrap();
        put_rect(&loader, RC, Rectl::new(0, 0, 10, 10));
        // Without DB_INTERIOR the inside stays.
        call(&loader, WIN_DRAW_BORDER, &[hps, RC, 1, 1, CLR_RED, CLR_BLUE, DB_PATCOPY]);
        assert_eq!(surface.lock_or_recover().pel(5, 5), Some(blank));
        call(&loader, WIN_DRAW_BORDER, &[hps, RC, 1, 1, CLR_RED, CLR_BLUE, DB_PATINVERT]);
        assert_eq!(surface.lock_or_recover().pel(0, 0), Some(0), "red XOR red");
        call(&loader, WIN_DRAW_BORDER, &[hps, RC, 1, 1, CLR_RED, CLR_BLUE, DB_DESTINVERT | DB_INTERIOR]);
        let s = surface.lock_or_recover();
        assert_eq!(s.pel(0, 0), Some(0xFFFFFF));
        assert_eq!(s.pel(5, 5), Some(!blank & 0xFFFFFF));
        drop(s);
        // DB_STANDARD: one system border wide.
        call(&loader, WIN_DRAW_BORDER, &[hps, RC, 1, 1, CLR_BLUE, CLR_RED, DB_STANDARD | DB_INTERIOR]);
        let s = surface.lock_or_recover();
        assert_eq!((s.pel(0, 5), s.pel(1, 5)), (Some(0x0000FF), Some(0xFF0000)));
        drop(s);
        // DB_DLGBORDER: title-bar colours.
        call(&loader, WIN_DRAW_BORDER, &[hps, RC, 2, 2, CLR_RED, CLR_BLUE, DB_DLGBORDER | DB_INTERIOR]);
        assert_eq!(surface.lock_or_recover().pel(0, 0), Some(DLG_BORDER_ACTIVE_COLOR));
        assert_eq!(surface.lock_or_recover().pel(5, 5), Some(0x0000FF));
        call(&loader, WIN_DRAW_BORDER, &[hps, RC, 2, 2, CLR_RED, CLR_BLUE, DB_DLGBORDER | DB_PATINVERT]);
        assert_eq!(surface.lock_or_recover().pel(0, 0), Some(DLG_BORDER_INACTIVE_COLOR));
    }

    #[test]
    fn draw_border_area_attributes_and_mix() {
        let loader = Loader::new_mock();
        let (hps, surface) = border_ps(&loader);
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let ps = wm.ps_map.get_mut(&hps).unwrap();
            ps.color = 0x00FF00;
            ps.back_color = 0x000000;
            ps.mix_mode = FM_XOR;
        }
        put_rect(&loader, RC, Rectl::new(0, 0, 10, 10));
        call(&loader, WIN_DRAW_BORDER, &[hps, RC, 1, 1, CLR_RED, CLR_BLUE, DB_AREAATTRS | DB_INTERIOR]);
        let s = surface.lock_or_recover();
        assert_eq!(s.pel(0, 0), Some(0x00FF00), "PS colour, not clrFore");
        assert_eq!(s.pel(5, 5), Some(0x000000), "colours swapped inside");
        drop(s);
        // DB_AREAMIXMODE: the PS's XOR mix.
        let before = surface.lock_or_recover().pel(0, 0).unwrap();
        call(&loader, WIN_DRAW_BORDER, &[hps, RC, 1, 1, CLR_RED, CLR_BLUE, DB_AREAMIXMODE]);
        assert_eq!(surface.lock_or_recover().pel(0, 0), Some(before ^ 0xFF0000));
    }

    #[test]
    fn draw_border_errors() {
        let loader = Loader::new_mock();
        let (hps, _) = border_ps(&loader);
        put_rect(&loader, RC, Rectl::new(0, 0, 10, 10));
        assert_eq!(call(&loader, WIN_DRAW_BORDER, &[hps, RC, 1, 1, 0, 0, 0x0400]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_FLAG));
        assert_eq!(call(&loader, WIN_DRAW_BORDER, &[hps, RC, 1, 1, 0, 0, 0x0005]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INV_DRAW_BORDER_OPTION));
        assert_eq!(call(&loader, WIN_DRAW_BORDER, &[0xBAD, RC, 1, 1, 0, 0, 0]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INV_HPS));
        assert_eq!(call(&loader, WIN_DRAW_BORDER, &[hps, 0, 1, 1, 0, 0, 0]), 0, "NULL rectangle");
        put_rect(&loader, RC, Rectl::new(10, 10, 0, 0));
        assert_eq!(call(&loader, WIN_DRAW_BORDER, &[hps, RC, 1, 1, 0, 0, 0]), 1, "empty: nothing to draw");
    }
}
