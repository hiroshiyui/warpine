// SPDX-License-Identifier: GPL-3.0-only

//! Frame controls and frame formatting: `WinCreateFrameControls`,
//! `WinCalcFrameRect`, the default frame handling of `WM_FORMATFRAME` /
//! `WM_QUERYFRAMECTLCOUNT` / `WM_CALCFRAMERECT`, `WinFlashWindow`,
//! `WinSetWindowPos` and `WinSetMultWindowPos`.
//!
//! Behaviour follows the PM Reference pages of the calls and messages and
//! the `<pmwin.h>` structures (`FRAMECDATA`, `SWP`):
//! - `WinCreateFrameControls` adds the frame controls named by
//!   `FRAMECDATA.flCreateFlags` to a frame window; `hmodResources` /
//!   `idResources` name its menu (`FCF_MENU`) and accelerator table
//!   (`FCF_ACCELTABLE`), `pszTitle` its title.
//! - `WinCalcFrameRect` with `fClient` TRUE turns a frame rectangle into
//!   its client rectangle, with FALSE a client rectangle into the frame
//!   rectangle; it fails when the result is empty.
//! - `WM_FORMATFRAME` (mp1 = `SWP` array, mp2 = client `RECTL`) fills in
//!   the positions of the frame controls and the client and returns their
//!   count; `WM_QUERYFRAMECTLCOUNT` returns how many `SWP`s that needs.
//! - `WinFlashWindow` starts or stops flashing a frame's title bar.
//! - `WinSetMultWindowPos` applies an array of `SWP`s.
//!
//! Warpine's frame model: the title bar and the menu bar sit above the
//! client, which reaches the frame's left, right and bottom edges (the
//! border is drawn over the client's edge). Frame scroll bars
//! (`FID_VERTSCROLL` / `FID_HORZSCROLL`) lie inside the border, along the
//! right and the bottom, and the client is what is left. The title bar,
//! system menu and minimize / maximize buttons are chrome drawn by the
//! compositor from `frame_flags`, not windows.
//!
//! Unverified (the references are silent; Warpine's own choices):
//! - `WM_QUERYFRAMECTLCOUNT` counts the client with the frame-control
//!   windows (the `SWP` array of `WM_FORMATFRAME` holds the client too);
//!   `WM_FORMATFRAME` lists the menu, the vertical and the horizontal
//!   scroll bar, then the client, with `SWP_SIZE | SWP_MOVE`.
//! - Warpine's frame layout does not send `WM_QUERYFRAMECTLCOUNT` /
//!   `WM_FORMATFRAME` / `WM_CALCFRAMERECT` to a subclassed frame
//!   procedure; the messages are answered when an application sends them.
//! - Frame scroll bars are `FRAME_SCROLLBAR_W` pels thick (`SV_CXVSCROLL` /
//!   `SV_CYHSCROLL`). They take the pointer like any scroll bar
//!   (`WindowManager::control_at` hits the frame's built-in children), and
//!   their `WM_VSCROLL` / `WM_HSCROLL` go to the client, as the frame
//!   procedure passes them on; the client draws at its place above a
//!   horizontal bar (`DrawTarget::Client`).
//! - A `FRAMECDATA` whose `cb` is less than its size fails
//!   (`PMERR_PARAMETER_OUT_OF_RANGE`).
//! - `WinFlashWindow` flashes by drawing the title bar active and inactive
//!   in turn at the cursor rate (`SV_CURSORRATE`), without a beep.
//! - An `SWP` of `WinSetMultWindowPos` that minimizes, maximizes or
//!   restores a frame does so without sending `WM_MINMAXFRAME`; any
//!   invalid window handle fails the call before a window is changed.

use std::collections::HashSet;

use log::debug;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_minmax::WinRect;
use super::pm_rect::Rectl;
use super::pm_types::WindowManager;
use super::{ApiResult, Loader};

/// Size of `FRAMECDATA` (`<pmwin.h>`, packed to 2) and the offsets of its
/// fields.
pub const FRAMECDATA_SIZE: u16 = 10;
const FRAMECDATA_FL_CREATE_FLAGS: u32 = 2;
const FRAMECDATA_HMOD_RESOURCES: u32 = 6;
const FRAMECDATA_ID_RESOURCES: u32 = 8;

/// Thickness of frame scroll bars (Warpine's own choice, the width of the
/// VIO window's scroll bars): `SV_CXVSCROLL` / `SV_CYHSCROLL`.
pub const FRAME_SCROLLBAR_W: i32 = 16;

/// Size of an `SWP` in guest memory and the offsets of its fields.
pub const SWP_SIZE_BYTES: u32 = 36;
const SWP_FL: u32 = 0;
const SWP_CY: u32 = 4;
const SWP_CX: u32 = 8;
const SWP_Y: u32 = 12;
const SWP_X: u32 = 16;
const SWP_HWND_INSERT_BEHIND: u32 = 20;
const SWP_HWND: u32 = 24;

/// At most this many `SWP`s are taken from one `WinSetMultWindowPos` call
/// (a bound on a guest-supplied count; far more windows than a process
/// could position at once).
pub const MAX_MULT_SWP: u32 = 4096;

/// Class name of frame windows created with `WinCreateWindow(WC_FRAME)`.
pub const FRAME_CLASS: &str = "WC_FRAME";

/// What decides a frame's layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FrameShape {
    pub cx: i32,
    pub cy: i32,
    /// Height of the title bar (0 without `FCF_TITLEBAR`).
    pub title_h: i32,
    /// Height of the menu bar (0 without a menu).
    pub menu_h: i32,
    /// Width of the border (`frame_border_width`).
    pub border: i32,
    pub vscroll: bool,
    pub hscroll: bool,
}

/// Where a frame's controls and client go, in frame coordinates
/// (bottom-left origin).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameLayout {
    pub menu: Option<WinRect>,
    pub vscroll: Option<WinRect>,
    pub hscroll: Option<WinRect>,
    pub client: WinRect,
    /// Size of the frame's drawing surface: the frame below its title bar
    /// and menu bar.
    pub surface: (i32, i32),
}

/// Lay out a frame of `shape`: the menu bar below the title bar, the
/// scroll bars inside the border along the right and the bottom, the
/// client in what is left (see the module comment).
pub fn format_frame(s: &FrameShape) -> FrameLayout {
    let below = (s.cy - s.title_h - s.menu_h).max(0);
    let menu = (s.menu_h > 0).then_some(WinRect { x: 0, y: below, cx: s.cx, cy: s.menu_h });
    let bar = FRAME_SCROLLBAR_W;
    let hbar_top = if s.hscroll { s.border + bar } else { 0 };
    let vscroll = s.vscroll.then(|| WinRect {
        x: (s.cx - s.border - bar).max(0),
        y: hbar_top.min(below),
        cx: bar.min(s.cx),
        cy: (below - hbar_top).max(0),
    });
    let hscroll = s.hscroll.then(|| WinRect {
        x: s.border.min(s.cx),
        y: s.border.min(below),
        cx: (s.cx - 2 * s.border - if s.vscroll { bar } else { 0 }).max(0),
        cy: bar.min(below),
    });
    let client_y = hbar_top.min(below);
    let client = WinRect {
        x: 0,
        y: client_y,
        cx: (s.cx - if s.vscroll { s.border + bar } else { 0 }).max(0),
        cy: below - client_y,
    };
    FrameLayout { menu, vscroll, hscroll, client, surface: (s.cx.max(0), below) }
}

/// What a frame of `shape` adds around its client on each side:
/// `(left, bottom, right, top)`.
fn frame_insets(s: &FrameShape) -> (i32, i32, i32, i32) {
    let bar = FRAME_SCROLLBAR_W;
    let right = if s.vscroll { s.border + bar } else { 0 };
    let bottom = if s.hscroll { s.border + bar } else { 0 };
    (0, bottom, right, s.title_h + s.menu_h)
}

/// `WinCalcFrameRect` with `fClient` TRUE: the client rectangle of a
/// frame occupying `frame` (in any coordinate system).
pub fn client_rect_from_frame(frame: Rectl, s: &FrameShape) -> Rectl {
    let (l, b, r, t) = frame_insets(s);
    Rectl::new(frame.x_left.saturating_add(l), frame.y_bottom.saturating_add(b),
               frame.x_right.saturating_sub(r), frame.y_top.saturating_sub(t))
}

/// `WinCalcFrameRect` with `fClient` FALSE: the frame rectangle around a
/// client occupying `client`.
pub fn frame_rect_from_client(client: Rectl, s: &FrameShape) -> Rectl {
    let (l, b, r, t) = frame_insets(s);
    Rectl::new(client.x_left.saturating_sub(l), client.y_bottom.saturating_sub(b),
               client.x_right.saturating_add(r), client.y_top.saturating_add(t))
}

/// Frames whose title bars flash (`WinFlashWindow`).
#[derive(Debug, Default)]
pub struct FlashState {
    frames: HashSet<u32>,
    /// The flash phase: flashing title bars are drawn inverted while set.
    inverted: bool,
}

impl FlashState {
    /// Start (`on`) or stop flashing `frame`.
    pub fn set(&mut self, frame: u32, on: bool) {
        if on { self.frames.insert(frame); } else { self.frames.remove(&frame); }
    }

    pub fn is_flashing(&self, frame: u32) -> bool {
        self.frames.contains(&frame)
    }

    /// Whether `frame`'s title bar is drawn active, given whether the
    /// frame is the active one: a flashing frame swaps the two in one
    /// phase of the flash.
    pub fn title_active(&self, frame: u32, active: bool) -> bool {
        active ^ (self.inverted && self.frames.contains(&frame))
    }
}

impl WindowManager {
    /// Whether `hwnd` is a frame window: a standard frame, a dialog, or a
    /// window of class `WC_FRAME`.
    pub fn is_frame_window(&self, hwnd: u32) -> bool {
        self.is_frame(hwnd) || self.get_window(hwnd).is_some_and(|w| w.class_name == FRAME_CLASS)
    }

    /// The frame-control child of `frame` with identity `id` (`FID_*`).
    fn frame_control(&self, frame: u32, id: u32) -> Option<u32> {
        self.find_child_by_id(frame, id)
    }

    /// What `frame`'s layout depends on, `None` for no window.
    pub fn frame_shape(&self, frame: u32) -> Option<FrameShape> {
        let w = self.get_window(frame)?;
        Some(FrameShape {
            cx: w.cx,
            cy: w.cy,
            title_h: if w.frame_flags & FCF_TITLEBAR != 0 { CHROME_TITLE_H } else { 0 },
            menu_h: if w.menu_hwnd != 0 { MENU_BAR_HEIGHT as i32 } else { 0 },
            border: frame_border_width(w.frame_flags),
            vscroll: self.frame_control(frame, FID_VERTSCROLL).is_some(),
            hscroll: self.frame_control(frame, FID_HORZSCROLL).is_some(),
        })
    }

    /// The frame-control windows of `frame` and its client with where they
    /// go, in `WM_FORMATFRAME` order: menu, vertical scroll bar, horizontal
    /// scroll bar, client.
    pub fn frame_control_places(&self, frame: u32) -> Vec<(u32, WinRect)> {
        let Some(shape) = self.frame_shape(frame) else { return Vec::new() };
        let layout = format_frame(&shape);
        let menu = self.get_window(frame).map_or(0, |w| w.menu_hwnd);
        let client = self.frame_to_client.get(&frame).copied()
            .or_else(|| self.frame_control(frame, FID_CLIENT));
        [
            (Some(menu).filter(|&h| h != 0), layout.menu),
            (self.frame_control(frame, FID_VERTSCROLL), layout.vscroll),
            (self.frame_control(frame, FID_HORZSCROLL), layout.hscroll),
            (client, Some(layout.client)),
        ].into_iter().filter_map(|(h, r)| Some((h?, r?))).collect()
    }

    /// Place the frame scroll bars of `frame` (its menu and client are
    /// placed by `layout_frame`).
    pub(super) fn place_frame_scroll_bars(&mut self, frame: u32, layout: &FrameLayout) {
        for (id, place) in [(FID_VERTSCROLL, layout.vscroll), (FID_HORZSCROLL, layout.hscroll)] {
            let (Some(h), Some(r)) = (self.frame_control(frame, id), place) else { continue };
            if let Some(w) = self.get_window_mut(h) {
                (w.x, w.y, w.cx, w.cy) = (r.x, r.y, r.cx, r.cy);
            }
        }
    }

    /// One step of the title-bar flash (`WinFlashWindow`): switch the
    /// phase and have the flashing frames redrawn. Frames that no longer
    /// exist stop flashing. Returns whether any frame flashes.
    pub fn flash_frames(&mut self) -> bool {
        let gone: Vec<u32> = self.frame_flash.frames.iter().copied()
            .filter(|&h| self.get_window(h).is_none()).collect();
        for h in gone { self.frame_flash.frames.remove(&h); }
        self.frame_flash.inverted = !self.frame_flash.inverted;
        let frames: Vec<u32> = self.frame_flash.frames.iter().copied().collect();
        for &h in &frames { self.mark_dirty(h); }
        !frames.is_empty()
    }
}

/// An `SWP` read from guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Swp {
    pub fl: u32,
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub behind: u32,
    pub hwnd: u32,
}

impl Loader {
    /// Read `SWP` number `i` of the array at `pswp`.
    fn read_swp(&self, pswp: u32, i: u32) -> Option<Swp> {
        let base = pswp.checked_add(i.checked_mul(SWP_SIZE_BYTES)?)?;
        let at = |off: u32| self.guest_read::<u32>(base.checked_add(off)?);
        Some(Swp {
            fl: at(SWP_FL)?,
            cy: at(SWP_CY)? as i32,
            cx: at(SWP_CX)? as i32,
            y: at(SWP_Y)? as i32,
            x: at(SWP_X)? as i32,
            behind: at(SWP_HWND_INSERT_BEHIND)?,
            hwnd: at(SWP_HWND)?,
        })
    }

    /// Write `swp` as `SWP` number `i` of the array at `pswp` (the
    /// reserved fields zeroed); false when it does not fit guest memory.
    fn write_swp(&self, pswp: u32, i: u32, swp: &Swp) -> bool {
        let Some(base) = i.checked_mul(SWP_SIZE_BYTES).and_then(|o| pswp.checked_add(o)) else { return false };
        if self.guest_read_bytes(base, SWP_SIZE_BYTES as usize).is_none() { return false; }
        let mut bytes = [0u8; SWP_SIZE_BYTES as usize];
        for (off, v) in [(SWP_FL, swp.fl), (SWP_CY, swp.cy as u32), (SWP_CX, swp.cx as u32),
                         (SWP_Y, swp.y as u32), (SWP_X, swp.x as u32),
                         (SWP_HWND_INSERT_BEHIND, swp.behind), (SWP_HWND, swp.hwnd)] {
            bytes[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.guest_write_bytes(base, &bytes).is_some()
    }

    /// `WinCreateFrameControls(hwndFrame, pfcdata, pszTitle)`.
    pub(super) fn win_create_frame_controls(&self, tid: u32, frame: u32, pfcdata: u32, psz_title: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, frame, false) { return ApiResult::Normal(0); }
        let Some(cb) = self.guest_read::<u16>(pfcdata).filter(|_| pfcdata != 0) else {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        };
        let read16 = |off: u32| self.guest_read::<u16>(pfcdata.wrapping_add(off));
        let (Some(fl), Some(hmod), Some(id)) = (
            self.guest_read::<u32>(pfcdata.wrapping_add(FRAMECDATA_FL_CREATE_FLAGS)),
            read16(FRAMECDATA_HMOD_RESOURCES), read16(FRAMECDATA_ID_RESOURCES),
        ) else { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE) };
        if cb < FRAMECDATA_SIZE { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        let title = (psz_title != 0).then(|| self.read_guest_string_cp(psz_title, self.thread_codepage(tid)));
        debug!("  [VCPU {}] WinCreateFrameControls frame={} fcf=0x{:08X} hmod={} id={}", tid, frame, fl, hmod, id);
        let (hmod, id) = (hmod as u32, id as u32);
        let menu_items = if fl & FCF_MENU != 0 { self.load_menu_resource(hmod, id, tid) } else { None };
        let accel = if fl & FCF_ACCELTABLE != 0 { self.load_accel_table(hmod, id) } else { None };
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let hmq = wm.get_window(frame).map_or(0, |w| w.hmq);
            if let Some(w) = wm.get_window_mut(frame) {
                w.frame_flags |= fl;
                if let Some(t) = title { w.text = t; }
            }
            if let Some(items) = menu_items {
                let h_menu = wm.create_window("#Menu".to_string(), frame, 0);
                if let Some(m) = wm.get_window_mut(h_menu) {
                    m.menu_items = items;
                    m.id = FID_MENU;
                    m.owner = frame;
                }
                // The new menu bar replaces any earlier one.
                wm.attach_menu(frame, h_menu);
            }
            if let Some(haccel) = accel { wm.set_window_accel(frame, haccel); }
            Self::add_frame_scroll_bars(&mut wm, frame, fl, hmq);
            // A frame on the desktop is composited from now on.
            let top_level = wm.get_window(frame).is_some_and(|w| w.parent == 0 || w.parent == HWND_DESKTOP);
            if top_level && !wm.z_order.contains(&frame) { wm.z_push_top(frame); }
        }
        self.relayout_frame(frame);
        self.shared.window_mgr.lock_or_recover().mark_all_dirty();
        ApiResult::Normal(1)
    }

    /// Create the frame scroll bars named by `fl` (`FCF_VERTSCROLL`,
    /// `FCF_HORZSCROLL`) that `frame` does not have yet: built-in
    /// `WC_SCROLLBAR`s owned by the frame, which `WM_VSCROLL` /
    /// `WM_HSCROLL` go to. Their place comes from `layout_frame`.
    pub(super) fn add_frame_scroll_bars(wm: &mut WindowManager, frame: u32, fl: u32, hmq: u32) {
        for (flag, id, style) in [(FCF_VERTSCROLL, FID_VERTSCROLL, SBS_VERT), (FCF_HORZSCROLL, FID_HORZSCROLL, SBS_HORZ)] {
            if fl & flag == 0 || wm.find_child_by_id(frame, id).is_some() { continue; }
            let h = wm.create_window("WC_SCROLLBAR".to_string(), frame, hmq);
            if let Some(w) = wm.get_window_mut(h) {
                w.id = id;
                w.owner = frame;
                w.style = style | WS_VISIBLE;
                w.visible = true;
            }
            // It paints itself once it has its place.
            wm.invalidate_window_only(h);
        }
    }

    /// Re-lay out `frame` after its controls changed: the surface follows
    /// and the client is told its new size.
    pub(super) fn relayout_frame(&self, frame: u32) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let client = wm.frame_to_client.get(&frame).copied();
        let old = client.and_then(|c| wm.get_window(c)).map(|w| (w.cx, w.cy));
        let surface = wm.layout_frame(frame);
        if wm.surfaces.contains_key(&frame) {
            wm.resize_surface(frame, surface.0, surface.1);
        } else if client.is_some() || wm.z_order.contains(&frame) {
            // A frame is composited once it has a surface.
            wm.frame_surface(frame);
        }
        let new = client.and_then(|c| wm.get_window(c)).map(|w| (w.cx, w.cy));
        if let (Some(client), Some(old), Some(new)) = (client, old, new) && old != new {
            wm.post_size(client, old, new);
        }
    }

    /// `WinCreateWindow` hook: a window with identity `FID_CLIENT` created
    /// in a `WC_FRAME` window that has no client yet becomes its client,
    /// and the frame a frame with a drawing surface (on the desktop: in the
    /// Z-order).
    pub(super) fn adopt_frame_client(&self, hwnd: u32) {
        let frame = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some((parent, id)) = wm.get_window(hwnd).map(|w| (w.parent, w.id)) else { return };
            let frame_class = wm.get_window(parent).is_some_and(|w| w.class_name == FRAME_CLASS);
            if id != FID_CLIENT || !frame_class || wm.frame_to_client.contains_key(&parent) { return; }
            wm.frame_to_client.insert(parent, hwnd);
            let top_level = wm.get_window(parent).is_some_and(|w| w.parent == 0 || w.parent == HWND_DESKTOP);
            if top_level && !wm.z_order.contains(&parent) { wm.z_push_top(parent); }
            parent
        };
        self.relayout_frame(frame);
        self.shared.window_mgr.lock_or_recover().invalidate(hwnd, None, false);
    }

    /// `WinCalcFrameRect(hwndFrame, prcl, fClient)`.
    pub(super) fn win_calc_frame_rect(&self, tid: u32, frame: u32, prcl: u32, from_frame: bool) -> ApiResult {
        if !self.pm_check_hwnd(tid, frame, false) { return ApiResult::Normal(0); }
        ApiResult::Normal(self.calc_frame_rect(frame, prcl, from_frame) as u32)
    }

    /// The `RECTL` at `prcl` turned from a frame rectangle into the client
    /// rectangle (`from_frame`) or the other way; false for a window that
    /// is no frame, a bad pointer or an empty result.
    fn calc_frame_rect(&self, frame: u32, prcl: u32, from_frame: bool) -> bool {
        let shape = {
            let wm = self.shared.window_mgr.lock_or_recover();
            if !wm.is_frame_window(frame) { return false; }
            wm.frame_shape(frame)
        };
        let (Some(shape), Some(r)) = (shape, self.read_rectl(prcl)) else { return false };
        let out = if from_frame { client_rect_from_frame(r, &shape) } else { frame_rect_from_client(r, &shape) };
        self.write_rectl(prcl, out) && !out.is_empty()
    }

    /// `WinDefWindowProc` for the frame formatting messages of a frame
    /// window; `None` when `hwnd` is no frame or `msg` is another message.
    pub(super) fn frame_default_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<u32> {
        if !matches!(msg, WM_FORMATFRAME | WM_QUERYFRAMECTLCOUNT | WM_CALCFRAMERECT) { return None; }
        let places = {
            let wm = self.shared.window_mgr.lock_or_recover();
            if !wm.is_frame_window(hwnd) { return None; }
            wm.frame_control_places(hwnd)
        };
        Some(match msg {
            WM_QUERYFRAMECTLCOUNT => places.len() as u32,
            WM_CALCFRAMERECT => {
                // mp1 = PRECTL, mp2 = TRUE when it is the frame rectangle.
                self.calc_frame_rect(hwnd, mp1, mp2 != 0) as u32
            }
            _ => {
                // mp1 = SWP array, mp2 = PRECTL that receives the client
                // rectangle. Returns the count of SWPs filled in.
                let mut n = 0;
                for (hwnd, r) in &places {
                    let swp = Swp { fl: SWP_SIZE | SWP_MOVE, x: r.x, y: r.y, cx: r.cx, cy: r.cy, behind: HWND_TOP, hwnd: *hwnd };
                    if mp1 == 0 || !self.write_swp(mp1, n, &swp) { break; }
                    n += 1;
                }
                let client = self.shared.window_mgr.lock_or_recover().frame_shape(hwnd)
                    .map(|s| format_frame(&s).client);
                if let Some(c) = client && mp2 != 0 {
                    self.write_rectl(mp2, Rectl::new(c.x, c.y, c.x + c.cx, c.y + c.cy));
                }
                n
            }
        })
    }

    /// `WinFlashWindow(hwnd, fFlash)`: start or stop flashing the title bar
    /// of the frame `hwnd` (or of the frame whose client it is).
    pub(super) fn win_flash_window(&self, tid: u32, hwnd: u32, flash: bool) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let frame = wm.client_to_frame(hwnd);
        if !wm.is_frame_window(frame) { return ApiResult::Normal(0); }
        wm.frame_flash.set(frame, flash);
        wm.mark_dirty(frame);
        ApiResult::Normal(1)
    }

    /// `WinSetWindowPos` without the `WM_MINMAXFRAME` round trip: change
    /// `hwnd` as `fl` says (geometry already bounded). Returns the state
    /// change still to be made, with its target, for a frame that is
    /// minimized, maximized or restored.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn set_window_pos(&self, hwnd: u32, hwnd_behind: u32, x: i32, y: i32, cx: i32, cy: i32, fl: u32)
                                 -> Option<(u32, WinRect)> {
        let mut wm = self.shared.window_mgr.lock_or_recover();

        // SWP_MINIMIZE / SWP_MAXIMIZE / SWP_RESTORE of a standard
        // frame: its position comes from the state change, which is
        // made after the rest, once the client has been asked.
        let state_op = super::pm_minmax::state_change(fl);
        let minmax = state_op.and_then(|op| {
            let pos = (fl & SWP_MOVE != 0).then_some((x, y));
            let size = (fl & SWP_SIZE != 0).then_some((cx, cy));
            wm.begin_minmax(hwnd, op, pos, size).map(|t| (op, t))
        });
        let fl = if minmax.is_some() { fl & !(SWP_MOVE | SWP_SIZE) } else { fl };

        // WM_SIZE goes to the window, and for a standard frame to its
        // client, whose size follows the frame's (WM_FORMATFRAME).
        let is_std_frame = wm.frame_to_client.contains_key(&hwnd);
        let size_target = wm.frame_msg_target(hwnd);
        let old_size = wm.get_window(size_target).map(|w| (w.cx, w.cy));

        // Update the OS2Window position/size state.
        if let Some(win) = wm.get_window_mut(hwnd) {
            if fl & SWP_MOVE != 0 {
                win.x = x;
                win.y = y;
            }
            if fl & SWP_SIZE != 0 {
                win.cx = cx;
                win.cy = cy;
            }
            if fl & SWP_SHOW != 0 { win.visible = true; }
            if fl & SWP_HIDE != 0 { win.visible = false; }
        }
        // Combination boxes and spin buttons place their parts.
        if fl & SWP_SIZE != 0 {
            wm.layout_combo(hwnd);
            wm.layout_spin(hwnd);
        }

        // VDR-B2: SWP_ZORDER — update the Z-order stack.
        if fl & SWP_ZORDER != 0 && wm.frame_to_client.contains_key(&hwnd) {
            match hwnd_behind {
                HWND_TOP              => wm.z_push_top(hwnd),
                HWND_BOTTOM           => wm.z_push_bottom(hwnd),
                0                     => {} // no-op
                behind                => wm.z_insert_behind(hwnd, behind),
            }
        }

        // VDR-E1: SWP_ACTIVATE — update focused_hwnd.
        if fl & SWP_ACTIVATE != 0 && wm.frame_to_client.contains_key(&hwnd) {
            wm.focused_hwnd = hwnd;
            wm.z_push_top(hwnd);
        }

        // VDR-B4: any geometry/visibility/z-order/activation change → dirty.
        if fl & (SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_HIDE | SWP_ACTIVATE) != 0 {
            wm.mark_dirty(hwnd);
        }
        // (SWP_ZORDER dirtiness is handled transitively via z_push_top/bottom/insert_behind)

        // A resized frame re-lays out its menu bar, frame controls and
        // client; its surface covers the frame below the title and menu bars.
        let (fb_cx, fb_cy) = if fl & SWP_SIZE != 0 && wm.is_frame_window(hwnd) {
            wm.layout_frame(hwnd)
        } else { (cx, cy) };

        // Position and visibility are read by the compositor from
        // the window; only the surface has to follow a size change.
        if fl & SWP_SIZE != 0 {
            wm.resize_surface(hwnd, fb_cx, fb_cy);
            let new_size = if is_std_frame {
                wm.get_window(size_target).map_or((fb_cx, fb_cy), |w| (w.cx, w.cy))
            } else { (cx, cy) };
            if let Some(old) = old_size && old != new_size {
                wm.post_size(size_target, old, new_size);
            }
        }
        if fl & (SWP_SHOW | SWP_HIDE) != 0 {
            wm.frame_visibility_changed(hwnd);
        }
        minmax
    }

    /// `WinSetMultWindowPos(hab, pswp, cswp)`: apply each `SWP` in turn.
    pub(super) fn win_set_mult_window_pos(&self, tid: u32, pswp: u32, cswp: u32) -> ApiResult {
        if cswp > MAX_MULT_SWP { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        if cswp == 0 { return ApiResult::Normal(1); }
        let Some(swps) = (0..cswp).map(|i| self.read_swp(pswp, i)).collect::<Option<Vec<Swp>>>()
            .filter(|_| pswp != 0) else {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        };
        if !swps.iter().all(|s| self.pm_check_hwnd(tid, s.hwnd, false)) { return ApiResult::Normal(0); }
        for s in swps {
            let (x, y, cx, cy) = super::pm_win::bound_window_geometry(s.x, s.y, s.cx, s.cy);
            if let Some((op, target)) = self.set_window_pos(s.hwnd, s.behind, x, y, cx, cy, s.fl) {
                self.shared.window_mgr.lock_or_recover().finish_minmax(s.hwnd, op, target);
            }
        }
        ApiResult::Normal(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_error::error_id;
    use super::super::vm_backend::mock::MockVcpu;

    const ORD_WIN_CALC_FRAME_RECT: u32 = 704;
    const ORD_WIN_FLASH_WINDOW: u32 = 745;
    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_SET_MULT_WINDOW_POS: u32 = 863;
    const ORD_WIN_CREATE_FRAME_CONTROLS: u32 = 906;
    const ORD_WIN_CREATE_WINDOW: u32 = 909;
    const ORD_WIN_DEF_WINDOW_PROC: u32 = 911;
    const ORD_WIN_WINDOW_FROM_ID: u32 = 899;

    /// Scratch guest memory for structures.
    const BUF: u32 = 0x2000;
    const BUF2: u32 = 0x3000;

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

    fn shape(vscroll: bool, hscroll: bool) -> FrameShape {
        FrameShape { cx: 300, cy: 200, title_h: CHROME_TITLE_H, menu_h: 20, border: 2, vscroll, hscroll }
    }

    /// A standard frame (title bar, sizing border) with a client: (frame, client).
    fn std_frame(loader: &Loader, fcf: u32) -> (u32, u32) {
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hmq = wm.create_mq(850);
        wm.tid_to_hmq.insert(0, hmq);
        let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        wm.frame_to_client.insert(frame, client);
        let w = wm.get_window_mut(frame).unwrap();
        (w.cx, w.cy, w.frame_flags, w.visible) = (300, 200, fcf, true);
        wm.layout_frame(frame);
        (frame, client)
    }

    fn write_rect(loader: &Loader, at: u32, r: [i32; 4]) {
        for (i, v) in r.iter().enumerate() { loader.guest_write::<i32>(at + 4 * i as u32, *v).unwrap(); }
    }

    fn read_rect(loader: &Loader, at: u32) -> [i32; 4] {
        std::array::from_fn(|i| loader.guest_read::<i32>(at + 4 * i as u32).unwrap())
    }

    #[test]
    fn a_frame_without_scroll_bars_keeps_the_client_below_title_and_menu() {
        let l = format_frame(&shape(false, false));
        assert_eq!(l.client, WinRect { x: 0, y: 0, cx: 300, cy: 160 });
        assert_eq!(l.menu, Some(WinRect { x: 0, y: 160, cx: 300, cy: 20 }));
        assert_eq!((l.vscroll, l.hscroll), (None, None));
        assert_eq!(l.surface, (300, 160));
    }

    #[test]
    fn scroll_bars_lie_inside_the_border_and_shrink_the_client() {
        let l = format_frame(&shape(true, true));
        let bar = FRAME_SCROLLBAR_W;
        assert_eq!(l.vscroll, Some(WinRect { x: 300 - 2 - bar, y: 2 + bar, cx: bar, cy: 160 - 2 - bar }));
        assert_eq!(l.hscroll, Some(WinRect { x: 2, y: 2, cx: 300 - 4 - bar, cy: bar }));
        assert_eq!(l.client, WinRect { x: 0, y: 2 + bar, cx: 300 - 2 - bar, cy: 160 - 2 - bar });
        // A tiny frame does not produce negative sizes.
        let tiny = format_frame(&FrameShape { cx: 5, cy: 10, ..shape(true, true) });
        for r in [tiny.client, tiny.vscroll.unwrap(), tiny.hscroll.unwrap()] {
            assert!(r.cx >= 0 && r.cy >= 0, "{r:?}");
        }
    }

    #[test]
    fn frame_and_client_rectangles_convert_both_ways() {
        let s = shape(true, false);
        let frame = Rectl::new(10, 20, 310, 220);
        let client = client_rect_from_frame(frame, &s);
        assert_eq!(client, Rectl::new(10, 20, 310 - 2 - FRAME_SCROLLBAR_W, 220 - 40));
        assert_eq!(frame_rect_from_client(client, &s), frame);
        // The client rectangle matches the layout's client.
        let l = format_frame(&s);
        assert_eq!((client.x_right - client.x_left, client.y_top - client.y_bottom), (l.client.cx, l.client.cy));
    }

    #[test]
    fn win_calc_frame_rect_converts_and_fails_when_empty() {
        let loader = Loader::new_mock();
        let (frame, client) = std_frame(&loader, FCF_TITLEBAR | FCF_SIZEBORDER);
        write_rect(&loader, BUF, [0, 0, 300, 200]);
        assert_eq!(call(&loader, ORD_WIN_CALC_FRAME_RECT, &[frame, BUF, 1]), 1);
        assert_eq!(read_rect(&loader, BUF), [0, 0, 300, 200 - CHROME_TITLE_H]);
        assert_eq!(call(&loader, ORD_WIN_CALC_FRAME_RECT, &[frame, BUF, 0]), 1);
        assert_eq!(read_rect(&loader, BUF), [0, 0, 300, 200]);
        // A frame rectangle smaller than the title bar leaves no client.
        write_rect(&loader, BUF, [0, 0, 100, 10]);
        assert_eq!(call(&loader, ORD_WIN_CALC_FRAME_RECT, &[frame, BUF, 1]), 0);
        // The client is no frame.
        write_rect(&loader, BUF, [0, 0, 100, 100]);
        assert_eq!(call(&loader, ORD_WIN_CALC_FRAME_RECT, &[client, BUF, 1]), 0);
        assert_eq!(call(&loader, ORD_WIN_CALC_FRAME_RECT, &[0xDEAD, BUF, 1]), 0);
        assert_eq!(call(&loader, ORD_WIN_GET_LAST_ERROR, &[0]), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
    }

    /// Write a FRAMECDATA at `at`.
    fn write_fcdata(loader: &Loader, at: u32, cb: u16, fl: u32) {
        loader.guest_write::<u16>(at, cb).unwrap();
        loader.guest_write::<u32>(at + 2, fl).unwrap();
        loader.guest_write::<u16>(at + 6, 0).unwrap();
        loader.guest_write::<u16>(at + 8, 1).unwrap();
    }

    #[test]
    fn frame_controls_add_chrome_and_scroll_bars_and_format_the_frame() {
        let loader = Loader::new_mock();
        let (frame, client) = std_frame(&loader, FCF_TITLEBAR);
        write_fcdata(&loader, BUF, FRAMECDATA_SIZE, FCF_SYSMENU | FCF_SIZEBORDER | FCF_VERTSCROLL | FCF_HORZSCROLL);
        loader.guest_write_bytes(BUF2, b"Controls\0").unwrap();
        assert_eq!(call(&loader, ORD_WIN_CREATE_FRAME_CONTROLS, &[frame, BUF, BUF2]), 1);
        let vbar = call(&loader, ORD_WIN_WINDOW_FROM_ID, &[frame, FID_VERTSCROLL]);
        let hbar = call(&loader, ORD_WIN_WINDOW_FROM_ID, &[frame, FID_HORZSCROLL]);
        assert!(vbar != 0 && hbar != 0);
        {
            let wm = loader.shared.window_mgr.lock_or_recover();
            let f = wm.get_window(frame).unwrap();
            assert_eq!(f.text, "Controls");
            assert_eq!(f.frame_flags & (FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER),
                       FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER);
            let expect = format_frame(&wm.frame_shape(frame).unwrap());
            let c = wm.get_window(client).unwrap();
            assert_eq!(WinRect { x: c.x, y: c.y, cx: c.cx, cy: c.cy }, expect.client);
            let v = wm.get_window(vbar).unwrap();
            assert_eq!(Some(WinRect { x: v.x, y: v.y, cx: v.cx, cy: v.cy }), expect.vscroll);
            assert_eq!(v.style & SBS_VERT, SBS_VERT);
            assert_eq!(v.owner, frame);
        }
        // A second call does not add a second bar.
        assert_eq!(call(&loader, ORD_WIN_CREATE_FRAME_CONTROLS, &[frame, BUF, 0]), 1);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().get_window(frame).unwrap().children.len(), 3);

        // WM_QUERYFRAMECTLCOUNT / WM_FORMATFRAME through WinDefWindowProc.
        assert_eq!(call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[frame, WM_QUERYFRAMECTLCOUNT, 0, 0]), 3);
        assert_eq!(call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[frame, WM_FORMATFRAME, BUF, BUF2]), 3);
        let swp = |i: u32| loader.read_swp(BUF, i).unwrap();
        assert_eq!([swp(0).hwnd, swp(1).hwnd, swp(2).hwnd], [vbar, hbar, client]);
        assert_eq!(swp(2).fl, SWP_SIZE | SWP_MOVE);
        let c = loader.shared.window_mgr.lock_or_recover().get_window(client).map(|w| (w.x, w.y, w.cx, w.cy)).unwrap();
        assert_eq!((swp(2).x, swp(2).y, swp(2).cx, swp(2).cy), c);
        assert_eq!(read_rect(&loader, BUF2), [c.0, c.1, c.0 + c.2, c.1 + c.3]);
        // WM_CALCFRAMERECT as WinCalcFrameRect.
        write_rect(&loader, BUF, [0, 0, 300, 200]);
        assert_eq!(call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[frame, WM_CALCFRAMERECT, BUF, 1]), 1);
        assert_eq!(read_rect(&loader, BUF), [c.0, c.1, c.0 + c.2, c.1 + c.3]);
        // Not a frame: the messages are not frame messages there.
        assert_eq!(call(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_QUERYFRAMECTLCOUNT, 0, 0]), 0);
    }

    /// A frame with both frame scroll bars, the client's surface recording.
    fn scrolled_frame(loader: &Loader) -> (u32, u32, u32, u32) {
        let (frame, client) = std_frame(loader, FCF_TITLEBAR | FCF_SIZEBORDER);
        write_fcdata(loader, BUF, FRAMECDATA_SIZE, FCF_VERTSCROLL | FCF_HORZSCROLL);
        assert_eq!(call(loader, ORD_WIN_CREATE_FRAME_CONTROLS, &[frame, BUF, 0]), 1);
        let vbar = call(loader, ORD_WIN_WINDOW_FROM_ID, &[frame, FID_VERTSCROLL]);
        let hbar = call(loader, ORD_WIN_WINDOW_FROM_ID, &[frame, FID_HORZSCROLL]);
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let (w, h) = wm.layout_frame(frame);
        wm.resize_surface(frame, w, h);
        for b in [vbar, hbar] {
            wm.get_window_mut(b).unwrap().scrollbar.set_range(5, 0, 10);
        }
        (frame, client, vbar, hbar)
    }

    /// The pointer reaches the frame scroll bars (in the frame's surface
    /// coordinates); their notifications go to the client.
    #[test]
    fn frame_scroll_bars_take_the_pointer_and_tell_the_client() {
        let loader = Loader::new_mock();
        let (frame, client, vbar, hbar) = scrolled_frame(&loader);
        let (v, h, c) = {
            let wm = loader.shared.window_mgr.lock_or_recover();
            (wm.get_abs_rect_in_frame(vbar), wm.get_abs_rect_in_frame(hbar), wm.client_view(frame).unwrap())
        };
        assert_eq!(c.1, CHROME_BORDER_W + FRAME_SCROLLBAR_W, "the client sits above the horizontal bar");
        {
            let wm = loader.shared.window_mgr.lock_or_recover();
            assert_eq!(wm.control_at(frame, v.0 + 3, v.1 + 4), Some((vbar, 3, 4)));
            assert_eq!(wm.control_at(frame, h.0 + 30, h.1 + 2), Some((hbar, 30, 2)));
            assert_eq!(wm.control_at(frame, 10, c.1 + 10), None, "the client itself");
        }
        // A press on the down arrow of the vertical bar (its bottom end).
        loader.dispatch_builtin_control(vbar, WM_BUTTON1DOWN, 2 | (2 << 16), 0);
        loader.dispatch_builtin_control(vbar, WM_BUTTON1UP, 2 | (2 << 16), 0);
        let hmq = loader.shared.window_mgr.lock_or_recover().thread_queue(0).unwrap();
        let mq = loader.shared.window_mgr.lock_or_recover().get_mq(hmq).unwrap();
        let got: Vec<(u32, u32, u32, u32)> = mq.lock_or_recover().messages.drain(..)
            .filter(|m| m.msg == WM_VSCROLL).map(|m| (m.hwnd, m.msg, m.mp1, m.mp2 >> 16)).collect();
        assert_eq!(got, [(client, WM_VSCROLL, FID_VERTSCROLL, SB_LINEDOWN),
                         (client, WM_VSCROLL, FID_VERTSCROLL, SB_ENDSCROLL)]);
    }

    /// The client's presentation space draws at the client's place in the
    /// frame's surface (above the horizontal bar) and inside the client.
    #[test]
    fn client_drawing_honours_its_place_in_the_frame() {
        use crate::gui::surface::{DrawOp, Mix};
        let loader = Loader::new_mock();
        let (frame, client, _, _) = scrolled_frame(&loader);
        let (hps, c, surface) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hps = wm.create_ps(client);
            (hps, wm.client_view(frame).unwrap(), wm.frame_surface(frame).unwrap())
        };
        let t = loader.ps_target(hps).unwrap();
        assert_eq!(t.target, super::super::pm_types::DrawTarget::Client(frame));
        const RED: u32 = 0xFF0000;
        loader.draw_ps(&t, DrawOp::Box { x1: -50, y1: -50, x2: 4000, y2: 4000, color: RED, fill: true,
                                         mix: Mix::Overpaint });
        let s = surface.lock_or_recover();
        assert_eq!(s.pel(0, c.1), Some(RED), "client (0, 0)");
        assert_eq!(s.pel(c.2 - 1, c.1 + c.3 - 1), Some(RED), "client top right");
        assert_ne!(s.pel(5, c.1 - 1), Some(RED), "not on the horizontal bar");
        assert_ne!(s.pel(c.2, c.1 + 5), Some(RED), "not on the vertical bar");
    }

    #[test]
    fn a_short_framecdata_fails() {
        let loader = Loader::new_mock();
        let (frame, _) = std_frame(&loader, FCF_TITLEBAR);
        write_fcdata(&loader, BUF, FRAMECDATA_SIZE - 2, FCF_VERTSCROLL);
        assert_eq!(call(&loader, ORD_WIN_CREATE_FRAME_CONTROLS, &[frame, BUF, 0]), 0);
        assert_eq!(call(&loader, ORD_WIN_GET_LAST_ERROR, &[0]), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_CREATE_FRAME_CONTROLS, &[frame, 0, 0]), 0);
        assert_eq!(call(&loader, ORD_WIN_WINDOW_FROM_ID, &[frame, FID_VERTSCROLL]), 0);
    }

    #[test]
    fn a_wc_frame_window_takes_controls_and_a_client() {
        let loader = Loader::new_mock();
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(850);
            wm.tid_to_hmq.insert(0, hmq);
        }
        loader.guest_write_bytes(BUF2, b"App\0").unwrap();
        // WinCreateWindow(HWND_DESKTOP, WC_FRAME, ..., 10, 10, 200, 150, ...)
        let frame = call(&loader, ORD_WIN_CREATE_WINDOW,
                         &[HWND_DESKTOP, WC_FRAME_ATOM, 0, WS_VISIBLE, 10, 10, 200, 150, 0, HWND_TOP, 7, 0, 0]);
        assert_ne!(frame, 0);
        write_fcdata(&loader, BUF, FRAMECDATA_SIZE, FCF_TITLEBAR | FCF_BORDER);
        assert_eq!(call(&loader, ORD_WIN_CREATE_FRAME_CONTROLS, &[frame, BUF, 0]), 1);
        assert!(loader.shared.window_mgr.lock_or_recover().z_order.contains(&frame));
        let client = call(&loader, ORD_WIN_CREATE_WINDOW,
                          &[frame, BUF2, 0, WS_VISIBLE, 0, 0, 0, 0, frame, HWND_TOP, FID_CLIENT, 0, 0]);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.frame_to_client.get(&frame), Some(&client));
        let c = wm.get_window(client).unwrap();
        assert_eq!((c.cx, c.cy), (200, 150 - CHROME_TITLE_H));
        assert!(wm.surfaces.contains_key(&frame));
    }

    #[test]
    fn flashing_swaps_the_title_bar_state_each_step() {
        let loader = Loader::new_mock();
        let (frame, client) = std_frame(&loader, FCF_TITLEBAR);
        assert_eq!(call(&loader, ORD_WIN_FLASH_WINDOW, &[client, 1]), 1, "the client's frame flashes");
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        assert!(wm.frame_flash.is_flashing(frame));
        assert!(!wm.frame_flash.title_active(frame, false));
        assert!(wm.flash_frames());
        assert!(wm.frame_flash.title_active(frame, false), "inverted phase");
        assert!(!wm.frame_flash.title_active(frame, true));
        assert!(wm.dirty.contains(&frame));
        wm.flash_frames();
        assert!(!wm.frame_flash.title_active(frame, false));
        drop(wm);
        assert_eq!(call(&loader, ORD_WIN_FLASH_WINDOW, &[frame, 0]), 1);
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.flash_frames(), "nothing flashes");
        assert!(!wm.frame_flash.title_active(frame, false));
        drop(wm);
        assert_eq!(call(&loader, ORD_WIN_FLASH_WINDOW, &[0xDEAD, 1]), 0);
    }

    #[test]
    fn set_mult_window_pos_moves_every_window_or_none() {
        let loader = Loader::new_mock();
        let (frame, client) = std_frame(&loader, FCF_TITLEBAR);
        let child = loader.shared.window_mgr.lock_or_recover().create_window("WC_STATIC".into(), client, 0);
        let swps = [
            Swp { fl: SWP_MOVE | SWP_SIZE, x: 40, y: 50, cx: 400, cy: 300, behind: 0, hwnd: frame },
            Swp { fl: SWP_MOVE | SWP_SIZE | SWP_SHOW, x: 5, y: 6, cx: 70, cy: 20, behind: 0, hwnd: child },
        ];
        for (i, s) in swps.iter().enumerate() { assert!(loader.write_swp(BUF, i as u32, s)); }
        assert_eq!(call(&loader, ORD_WIN_SET_MULT_WINDOW_POS, &[0, BUF, 2]), 1);
        {
            let wm = loader.shared.window_mgr.lock_or_recover();
            let f = wm.get_window(frame).unwrap();
            assert_eq!((f.x, f.y, f.cx, f.cy), (40, 50, 400, 300));
            assert_eq!(wm.get_window(client).map(|c| (c.cx, c.cy)), Some((400, 300 - CHROME_TITLE_H)));
            let c = wm.get_window(child).unwrap();
            assert_eq!((c.x, c.y, c.cx, c.cy, c.visible), (5, 6, 70, 20, true));
        }
        // A bad handle in the array changes nothing.
        let bad = [Swp { fl: SWP_MOVE, x: 1, y: 1, cx: 0, cy: 0, behind: 0, hwnd: frame },
                   Swp { fl: SWP_MOVE, x: 1, y: 1, cx: 0, cy: 0, behind: 0, hwnd: 0xDEAD }];
        for (i, s) in bad.iter().enumerate() { loader.write_swp(BUF, i as u32, s); }
        assert_eq!(call(&loader, ORD_WIN_SET_MULT_WINDOW_POS, &[0, BUF, 2]), 0);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().get_window(frame).map(|f| f.x), Some(40));
        // A hostile count or pointer fails without reading past memory.
        assert_eq!(call(&loader, ORD_WIN_SET_MULT_WINDOW_POS, &[0, BUF, MAX_MULT_SWP + 1]), 0);
        assert_eq!(call(&loader, ORD_WIN_SET_MULT_WINDOW_POS, &[0, 0xFFFF_FF00, 2]), 0);
        assert_eq!(call(&loader, ORD_WIN_SET_MULT_WINDOW_POS, &[0, BUF, 0]), 1, "nothing to do");
    }

    #[test]
    fn swp_round_trips_through_guest_memory() {
        let loader = Loader::new_mock();
        let s = Swp { fl: 3, x: -5, y: 7, cx: 100, cy: 50, behind: HWND_TOP, hwnd: 0x1234 };
        assert!(loader.write_swp(BUF, 1, &s));
        assert_eq!(loader.read_swp(BUF, 1), Some(s));
        assert!(!loader.write_swp(0xFFFF_FFF0, 0, &s), "past the end of guest memory");
        assert_eq!(loader.read_swp(0xFFFF_FFF0, 0), None);
    }
}
