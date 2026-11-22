// SPDX-License-Identifier: GPL-3.0-or-later

//! Owner drawing: built-in controls that let their owner draw (list boxes
//! with `LS_OWNERDRAW`, `BS_USERBUTTON` buttons) send it `WM_MEASUREITEM`,
//! `WM_DRAWITEM` with an `OWNERITEM`, or `WM_CONTROL(BN_PAINT)` with a
//! `USERBUTTON`, synchronously, as `WinSendMsg` would.
//!
//! A built-in control paints on the host, often in the middle of another
//! API (a click handled by `WinDispatchMsg`, `WinEndPaint` repainting the
//! controls, `LM_INSERTITEM`), where no guest code can run. So painting
//! does what it can at once — the control's background and border — and
//! queues the owner's part as [`OwnerCall`]s in `WindowManager::owner_calls`.
//! When the API returns to a thread whose message queue the owner belongs to
//! (`vcpu.rs`, after `ApiResult::Normal`, and in the modal dialog loop after
//! a message handled on the host), the calls are made one after the other
//! (`FrameKind::OwnerDraw`), each with its own presentation space on the
//! control, before the API's result reaches the caller.
//!
//! * `WM_MEASUREITEM` (list box): mp1 = list box id, mp2 = item index; the
//!   low word of the result is the item height (0 keeps the system font's).
//!   Sent when an item is inserted (the reference: "when items are inserted
//!   or deleted"; Warpine sends it on insertion only, and once for a burst
//!   of insertions not yet drawn).
//! * `WM_DRAWITEM` (list box): mp1 = list box id, mp2 = `POWNERITEM` with
//!   the item's index (`idItem`), handle (`hItem`, `LM_SETITEMHANDLE`) and
//!   rectangle. Warpine paints whole items: `fsState` is TRUE for a selected
//!   item and `fsStateOld` FALSE. TRUE = the owner drew the item; if
//!   `fsState` and `fsStateOld` then differ the list box inverts the item to
//!   show the selection (owners that draw the selection themselves set both
//!   to FALSE). FALSE = the list box draws the item's text itself.
//! * `WM_DRAWITEM` (slider, value set): `idItem` = the part (`SDA_*`,
//!   `VDA_ITEM` / `VDA_BACKGROUND`), `hItem` = `MPFROM2SHORT(row, column)`
//!   of a value set item, `rclItem` = the part; TRUE keeps what the owner
//!   drew, FALSE has the control draw the part (`pm_slider.rs`,
//!   `pm_valueset.rs`).
//! * `WM_CONTROL(BN_PAINT)` (user button): mp2 = `PUSERBUTTON`, `fsState` =
//!   `BDS_HILITED` / `BDS_DISABLED` / `BDS_DEFAULT`, `fsStateOld` the state
//!   it was last painted in. The button erases itself to the button face
//!   first; the result is not used (owners return 0 from `WM_CONTROL`).
//!
//! The presentation space: the owner draws in the control's own
//! coordinates (`rclItem` is relative to the control, as in PM). Warpine's
//! GPI draws a window's presentation space in its frame's surface without
//! the window's origin, so an owner call gets a presentation space on a
//! *canvas* instead — a bitmap the size of the control holding a copy of
//! its pels — and what the owner drew there (the item, or the whole user
//! button) is copied back into the control afterwards. This also keeps the
//! owner's drawing inside the item.
//!
//! A thread runs one owner-draw chain at a time: calls queued while the
//! owner draws (its handler sending messages to the control) wait for the
//! running chain, which takes them after the current call.

use super::constants::*;
use super::pm_types::{owner_menu_item_w, MenuItem, PsOrigin, WindowManager};
use crate::gui::surface::{Surface, MAX_SURFACE_DIM};
use super::{Loader, MutexExt, Painter};
use crate::gui::surface::{DrawOp, Mix};

/// Most owner calls waiting at once (a guest that keeps invalidating
/// owner-drawn controls cannot grow the queue without bound).
pub(crate) const MAX_OWNER_CALLS: usize = 1024;
/// Bytes reserved on the guest stack for the structure a call passes.
pub(crate) const OWNER_BLOCK_BYTES: u32 = 64;
/// The largest structure an owner call passes (a `WM_FORMATFRAME`'s
/// `RECTL` and `SWP` array).
pub(crate) const MAX_OWNER_BLOCK_BYTES: u32 = 1024;
/// Tallest list box item an owner can ask for, in pels.
pub(crate) const MAX_ITEM_HEIGHT: i32 = 1024;

/// What an owner is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerCallKind {
    /// `WM_MEASUREITEM` for list box item `index`.
    MeasureList { index: u32 },
    /// `WM_DRAWITEM` for list box item `index`.
    DrawListItem { index: usize },
    /// `WM_CONTROL(BN_PAINT)` for a user button.
    UserButton,
    /// `WM_DRAWITEM` for the `MIS_OWNERDRAW` pull-down item `id` of a menu;
    /// `hilite` `Some(on)` when its highlight changes (`MIA_HILITED` in
    /// `fsAttribute` and the opposite in `fsAttributeOld`).
    DrawMenuItem { id: u16, hilite: Option<bool> },
    /// `WM_INITMENU(id, menu)` to the owner of a menu (`pm_menubar.rs`).
    MenuInit { id: u16 },
    /// `WM_MENUSELECT(MPFROM2SHORT(id, post), menu)`; with `post`, the
    /// command message posted when the owner returns TRUE.
    MenuSelect { id: u16, post: Option<(u32, u32, u32)> },
    /// `WM_MENUEND(id, menu)`.
    MenuEnd { id: u16 },
    /// `WM_ERASEBACKGROUND` from a frame to its client `control`
    /// (`pm_paint_style.rs`).
    EraseBackground,
    /// `WM_PAINT` sent at once to `WS_SYNCPAINT` window `control`
    /// (`pm_paint_style.rs`).
    SyncPaint,
    /// `WM_DRAWITEM` for part `part` (`SDA_*`) of an `SLS_OWNERDRAW` slider.
    SliderPart { part: u32 },
    /// `WM_DRAWITEM` (`VDA_ITEM`) for a `VIA_OWNERDRAW` value set item.
    ValueSetItem { row: u16, col: u16 },
    /// `WM_DRAWITEM` (`VDA_BACKGROUND`) for a `VS_OWNERDRAW` value set.
    ValueSetBackground,
    /// `WM_CONTROL(BKN_PAGESELECTEDPENDING)` with a `PAGESELECTNOTIFY`:
    /// the notebook is about to turn from page `cur` to page `new`, which
    /// it does when the owner leaves `ulPageIdNew` non-zero.
    NotebookPending { cur: u32, new: u32 },
    /// `WM_CONTROL(BKN_PAGESELECTED)` with a `PAGESELECTNOTIFY`: the
    /// notebook turned from page `cur` to page `new` (`pm_notebook.rs`).
    NotebookSelected { cur: u32, new: u32 },
    /// `WM_CONTROL(BKN_PAGEDELETED)` with a `DELETENOTIFY` for a deleted
    /// page (its window, data and tab bitmap).
    NotebookDeleted { page: u32, data: u32, bitmap: u32 },
    /// `WM_CONTROL(code)` from a container with a three-ULONG structure
    /// (`NOTIFYRECORDEMPHASIS`, `NOTIFYRECORDENTER`; `pm_container.rs`).
    ContainerNotify { code: u16, block: [u32; 3] },
    /// `WM_CONTROL(code)` from a container with a plain mp2 (a record:
    /// `CN_EXPANDTREE`, `CN_COLLAPSETREE`).
    ContainerValue { code: u16, mp2: u32 },
    /// `WM_CONTROL(CN_INITDRAG)` with a `CNRDRAGINIT` (container, record,
    /// x, y, cx, cy).
    ContainerInitDrag { block: [u32; 6] },
    /// `WM_CONTROL(code)` from a container with a `CNREDITDATA`
    /// (`CN_BEGINEDIT`, `CN_REALLOCPSZ`, `CN_ENDEDIT`).
    ContainerEdit { code: u16, block: [u32; 7] },
    /// `WM_DRAWITEM` for part `part` (`CMA_ICON` / `CMA_TEXT`) of container
    /// record `rec` (`fi`: its details-view column, else 0): an
    /// `OWNERITEM` whose `hItem` points to a `CNRDRAWITEMINFO` (`pRecord`,
    /// `pFieldInfo`) placed after it (`pm_container.rs`).
    ContainerDrawItem { rec: u32, fi: u32, part: u32 },
    /// A subclassed frame lays itself out (`pm_frame.rs`): `count` `None`
    /// asks `WM_QUERYFRAMECTLCOUNT`, `Some(n)` sends `WM_FORMATFRAME` with
    /// room for `n` `SWP`s, which are applied when it returns.
    FrameFormat { count: Option<u16> },
    /// `WM_CONTROLPOINTER` with the pointer `hptr` the control shows; the
    /// pointer the owner returns is shown instead (`pm_pointer.rs`).
    ControlPointer { hptr: u32 },
}

/// The menu item `id` in `items` (searched through the submenus).
fn find_menu_item(items: &mut [MenuItem], id: u16) -> Option<&mut MenuItem> {
    for item in items {
        if item.id == id {
            return Some(item);
        }
        if let Some(found) = find_menu_item(&mut item.children, id) {
            return Some(found);
        }
    }
    None
}

/// One owner call waiting to be made: `kind` for control `control`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerCall {
    pub control: u32,
    pub kind: OwnerCallKind,
}

/// How to make an owner call.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OwnerSetup {
    /// Call `wnd_proc(owner, msg, mp1, mp2)`. A non-empty `block` is copied
    /// to the guest stack and mp2 points to it; `hps` is the presentation
    /// space made for the call (0 = none).
    Call { wnd_proc: u32, owner: u32, msg: u32, mp1: u32, mp2: u32, block: Vec<u32>, hps: u32 },
    /// No guest procedure to call (a built-in owner) or nothing to do any
    /// more (the control or item is gone): finish as if it returned FALSE.
    Skip,
}

/// What follows an owner-draw chain on the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerThen {
    /// Return `result` to the API's caller.
    Return(u32),
    /// Go on with a modal dialog loop (`FrameKind::DlgRunLoop`).
    DlgLoop { dlg_proc: u32, hwnd_dlg: u32, hmq: u32 },
}

/// A running owner-draw chain of the thread of queue `hmq`.
#[derive(Debug)]
pub(crate) struct OwnerChain {
    pub hmq: u32,
    pub then: OwnerThen,
}

impl WindowManager {
    /// Queue `call` unless the same call is waiting already. Returns whether
    /// it was queued.
    pub(crate) fn queue_owner_call(&mut self, call: OwnerCall) -> bool {
        if self.owner_calls.contains(&call) || self.owner_calls.len() >= MAX_OWNER_CALLS {
            return false;
        }
        self.owner_calls.push(call);
        true
    }

    /// The queue of the window that owner calls of `control` go to.
    fn owner_queue(&self, control: u32) -> Option<u32> {
        let owner = self.notify_target(control);
        self.find_hmq_for_hwnd(if owner != 0 { owner } else { control })
    }

    /// Take the next owner call for the thread of queue `hmq`; calls of
    /// controls that are gone are dropped.
    pub(crate) fn next_owner_call(&mut self, hmq: u32) -> Option<OwnerCall> {
        let mut calls = std::mem::take(&mut self.owner_calls);
        calls.retain(|c| self.windows_contain(c.control));
        self.owner_calls = calls;
        let at = self.owner_calls.iter().position(|c| self.owner_queue(c.control) == Some(hmq))?;
        Some(self.owner_calls.remove(at))
    }

    fn windows_contain(&self, hwnd: u32) -> bool {
        self.is_window(hwnd)
    }

    /// Start an owner-draw chain on the thread of queue `hmq` if calls are
    /// waiting for it and none is running there. Returns whether it started.
    pub(crate) fn begin_owner_chain(&mut self, hmq: u32) -> bool {
        if hmq == 0 || self.owner_drawing.contains(&hmq) {
            return false;
        }
        let waiting = self.owner_calls.iter()
            .any(|c| self.is_window(c.control) && self.owner_queue(c.control) == Some(hmq));
        waiting && self.owner_drawing.insert(hmq)
    }

    /// The chain of queue `hmq` has ended.
    pub(crate) fn end_owner_chain(&mut self, hmq: u32) {
        self.owner_drawing.remove(&hmq);
    }
}

impl Loader {
    /// Start an owner-draw chain for the thread `vcpu_id` (its message
    /// queue). Returns the queue when one started.
    pub(crate) fn begin_owner_chain_for_thread(&self, vcpu_id: u32) -> Option<u32> {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let hmq = wm.thread_queue(vcpu_id)?;
        wm.begin_owner_chain(hmq).then_some(hmq)
    }

    /// Queue a `WM_DRAWITEM` for each item of list box `hwnd` in `items`,
    /// replacing those still waiting for it (a repaint draws them anew).
    pub(super) fn queue_list_item_draws(&self, hwnd: u32, items: std::ops::Range<usize>) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        wm.owner_calls.retain(|c| !(c.control == hwnd && matches!(c.kind, OwnerCallKind::DrawListItem { .. })));
        for index in items {
            wm.queue_owner_call(OwnerCall { control: hwnd, kind: OwnerCallKind::DrawListItem { index } });
        }
    }

    /// A presentation space for an owner call on control `hwnd`: on a
    /// canvas bitmap the size of the control holding a copy of its pels
    /// (see the module comment).
    fn owner_ps(&self, hwnd: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let frame = wm.find_frame_for_hwnd(hwnd);
        let (ax, ay, cx, cy) = wm.get_abs_rect_in_frame(hwnd);
        let (w, h) = (cx.clamp(1, MAX_SURFACE_DIM as i32), cy.clamp(1, MAX_SURFACE_DIM as i32));
        let mut canvas = Surface::new(w as u32, h as u32);
        if let Some(surface) = wm.frame_surface(frame) {
            let s = surface.lock_or_recover();
            for y in 0..h {
                let row: Vec<u32> = (0..w).map(|x| s.pel(ax + x, ay + y).unwrap_or(super::CONTROL_FIELD_COLOR)).collect();
                canvas.put_row(y, &row);
            }
        }
        let hbm = wm.create_bitmap(canvas, 24, 1);
        let hps = wm.create_ps(0);
        if let Some(ps) = wm.ps_map.get_mut(&hps) {
            ps.bitmap = hbm;
            ps.origin = PsOrigin::Cached;
        }
        hps
    }

    /// Free the canvas of owner-call presentation space `hps`, first copying
    /// `rect` of it (control coordinates, right and top exclusive; `None` =
    /// nothing) into control `hwnd`.
    fn release_owner_ps(&self, hwnd: u32, hps: u32, rect: Option<(i32, i32, i32, i32)>) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let hbm = wm.ps_map.remove(&hps).map_or(0, |ps| ps.bitmap);
        let Some(canvas) = wm.bitmaps.remove(&hbm) else { return };
        let Some((l, b, r, t)) = rect else { return };
        let frame = wm.find_frame_for_hwnd(hwnd);
        let (ax, ay, _, _) = wm.get_abs_rect_in_frame(hwnd);
        let rows: Vec<(i32, Vec<u32>)> = {
            let c = canvas.surface.lock_or_recover();
            (b.max(0)..t).filter_map(|y| {
                let row = c.row_pels(y)?;
                let (x0, x1) = (l.clamp(0, row.len() as i32) as usize, r.clamp(0, row.len() as i32) as usize);
                (x0 < x1).then(|| (y, row[x0..x1].to_vec()))
            }).collect()
        };
        let Some(surface) = wm.frame_surface(frame) else { return };
        wm.mark_dirty(frame);
        let mut s = surface.lock_or_recover();
        for (y, row) in rows {
            s.put_span(ax + l.max(0), ay + y, &row);
        }
    }

    /// How to make owner call `call` (see [`OwnerSetup`]).
    pub(crate) fn owner_call_setup(&self, call: &OwnerCall) -> OwnerSetup {
        match call.kind {
            OwnerCallKind::DrawMenuItem { id, hilite } => return self.menu_item_setup(call.control, id, hilite),
            OwnerCallKind::MenuInit { .. } | OwnerCallKind::MenuSelect { .. } | OwnerCallKind::MenuEnd { .. } => {
                return self.menu_notify_setup(call);
            }
            OwnerCallKind::EraseBackground => return self.erase_background_setup(call.control),
            OwnerCallKind::SyncPaint => return self.sync_paint_setup(call.control),
            OwnerCallKind::FrameFormat { count } => return self.frame_format_setup(call.control, count),
            _ => {}
        }
        let (owner, wnd_proc, id, style, item) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window(call.control) else { return OwnerSetup::Skip };
            let owner = wm.notify_target(call.control);
            let wnd_proc = wm.get_window(owner).map_or(0, |o| o.pfn_wp);
            let item = match call.kind {
                OwnerCallKind::DrawListItem { index } => w.listbox_items.get(index).map(|_| {
                    (w.listbox_sel.is_selected(index), w.item_handles.get(index).copied().unwrap_or(0))
                }),
                _ => None,
            };
            (owner, wnd_proc, w.id, w.style, item)
        };
        if wnd_proc == 0 || owner == call.control {
            return OwnerSetup::Skip;
        }
        match call.kind {
            OwnerCallKind::MeasureList { index } => OwnerSetup::Call {
                wnd_proc, owner, msg: WM_MEASUREITEM, mp1: id & 0xFFFF, mp2: index, block: Vec::new(), hps: 0,
            },
            OwnerCallKind::DrawListItem { index } => {
                let (Some((selected, handle)), Some((l, b, r, t))) = (item, self.list_item_rect(call.control, index))
                else { return OwnerSetup::Skip };
                let hps = self.owner_ps(call.control);
                let block = vec![
                    call.control, hps, selected as u32, 0, 0, 0,
                    l as u32, b as u32, r as u32, t as u32,
                    index as u32, handle,
                ];
                OwnerSetup::Call { wnd_proc, owner, msg: WM_DRAWITEM, mp1: id & 0xFFFF, mp2: 0, block, hps }
            }
            OwnerCallKind::NotebookPending { cur, new } => OwnerSetup::Call {
                wnd_proc, owner, msg: WM_CONTROL,
                mp1: (super::pm_notebook::BKN_PAGESELECTEDPENDING << 16) | (id & 0xFFFF), mp2: 0,
                block: vec![call.control, cur, new], hps: 0,
            },
            OwnerCallKind::NotebookSelected { cur, new } => OwnerSetup::Call {
                wnd_proc, owner, msg: WM_CONTROL,
                mp1: (super::pm_notebook::BKN_PAGESELECTED << 16) | (id & 0xFFFF), mp2: 0,
                block: vec![call.control, cur, new], hps: 0,
            },
            OwnerCallKind::NotebookDeleted { page, data, bitmap } => OwnerSetup::Call {
                wnd_proc, owner, msg: WM_CONTROL,
                mp1: (super::pm_notebook::BKN_PAGEDELETED << 16) | (id & 0xFFFF), mp2: 0,
                block: vec![call.control, page, data, bitmap], hps: 0,
            },
            OwnerCallKind::ContainerNotify { code, block } => OwnerSetup::Call {
                wnd_proc, owner, msg: WM_CONTROL,
                mp1: (u32::from(code) << 16) | (id & 0xFFFF), mp2: 0,
                block: block.to_vec(), hps: 0,
            },
            OwnerCallKind::ContainerInitDrag { block } => OwnerSetup::Call {
                wnd_proc, owner, msg: WM_CONTROL,
                mp1: (super::pm_container::CN_INITDRAG << 16) | (id & 0xFFFF), mp2: 0,
                block: block.to_vec(), hps: 0,
            },
            OwnerCallKind::ContainerEdit { code, block } => OwnerSetup::Call {
                wnd_proc, owner, msg: WM_CONTROL,
                mp1: (u32::from(code) << 16) | (id & 0xFFFF), mp2: 0,
                block: block.to_vec(), hps: 0,
            },
            OwnerCallKind::ControlPointer { hptr } => OwnerSetup::Call {
                wnd_proc, owner, msg: WM_CONTROLPOINTER, mp1: id & 0xFFFF, mp2: hptr, block: Vec::new(), hps: 0,
            },
            OwnerCallKind::ContainerValue { code, mp2 } => OwnerSetup::Call {
                wnd_proc, owner, msg: WM_CONTROL,
                mp1: (u32::from(code) << 16) | (id & 0xFFFF), mp2, block: Vec::new(), hps: 0,
            },
            OwnerCallKind::ContainerDrawItem { rec, fi, part } => {
                let Some((l, b, r, t)) = self.container_item_rect(call.control, rec, fi, part) else { return OwnerSetup::Skip };
                let attr = self.container_record_attr(rec);
                let selected = u32::from(attr & super::pm_container::CRA_SELECTED != 0);
                let hps = self.owner_ps(call.control);
                // hItem (word 11) is set to the CNRDRAWITEMINFO's address
                // when the block is on the stack (vcpu.rs).
                let block = vec![call.control, hps, selected, attr, selected, attr, l as u32, b as u32, r as u32, t as u32,
                                 part, 0, rec, fi];
                OwnerSetup::Call { wnd_proc, owner, msg: WM_DRAWITEM, mp1: id & 0xFFFF, mp2: 0, block, hps }
            }
            OwnerCallKind::UserButton => {
                let state = self.user_button_state(call.control, style);
                let old = self.shared.window_mgr.lock_or_recover().get_window_mut(call.control)
                    .map_or(0, |w| std::mem::replace(&mut w.drawn_state, state));
                let hps = self.owner_ps(call.control);
                OwnerSetup::Call {
                    wnd_proc, owner, msg: WM_CONTROL, mp1: (BN_PAINT << 16) | (id & 0xFFFF), mp2: 0,
                    block: vec![call.control, hps, state, old], hps,
                }
            }
            OwnerCallKind::SliderPart { part } => {
                let Some((l, b, r, t)) = self.slider_part_rect(call.control, part) else { return OwnerSetup::Skip };
                let hps = self.owner_ps(call.control);
                let block = vec![call.control, hps, 0, 0, 0, 0, l as u32, b as u32, r as u32, t as u32, part, 0];
                OwnerSetup::Call { wnd_proc, owner, msg: WM_DRAWITEM, mp1: id & 0xFFFF, mp2: 0, block, hps }
            }
            OwnerCallKind::ValueSetItem { row, col } => {
                let (Some((l, b, r, t)), Some((selected, attr))) = (self.valueset_item_rect(call.control, (row, col)),
                                                                   self.valueset_item_state(call.control, (row, col)))
                else { return OwnerSetup::Skip };
                let hps = self.owner_ps(call.control);
                let (state, attr) = (selected as u32, attr as u32);
                let block = vec![call.control, hps, state, attr, state, attr, l as u32, b as u32, r as u32, t as u32,
                                 VDA_ITEM, row as u32 | (col as u32) << 16];
                OwnerSetup::Call { wnd_proc, owner, msg: WM_DRAWITEM, mp1: id & 0xFFFF, mp2: 0, block, hps }
            }
            OwnerCallKind::ValueSetBackground => {
                let Some((cx, cy)) = self.shared.window_mgr.lock_or_recover().get_window(call.control)
                    .map(|w| (w.cx, w.cy)) else { return OwnerSetup::Skip };
                let hps = self.owner_ps(call.control);
                let block = vec![call.control, hps, 0, 0, 0, 0, 0, 0, cx as u32, cy as u32, VDA_BACKGROUND, 0];
                OwnerSetup::Call { wnd_proc, owner, msg: WM_DRAWITEM, mp1: id & 0xFFFF, mp2: 0, block, hps }
            }
            OwnerCallKind::DrawMenuItem { .. } | OwnerCallKind::EraseBackground
            | OwnerCallKind::MenuInit { .. } | OwnerCallKind::MenuSelect { .. } | OwnerCallKind::MenuEnd { .. }
            | OwnerCallKind::SyncPaint | OwnerCallKind::FrameFormat { .. } => OwnerSetup::Skip,
        }
    }

    /// `WM_DRAWITEM` for pull-down item `id` of menu `menu`: sent to the
    /// menu's owner — a frame's client for a frame's menu — with a
    /// presentation space on a bitmap the size of the item
    /// (`owner_menu_item_w()` × `MENU_DROPDOWN_ITEM_H`, the menu's face),
    /// `rclItem` = the bitmap, `fsAttribute` / `fsAttributeOld` = the item's
    /// `MIA_*` attributes, `idItem` = its id.
    fn menu_item_setup(&self, menu: u32, id: u16, hilite: Option<bool>) -> OwnerSetup {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(m) = wm.get_window_mut(menu) else { return OwnerSetup::Skip };
        let Some(attr) = find_menu_item(&mut m.menu_items, id).map(|i| i.attr as u32) else { return OwnerSetup::Skip };
        let owner = wm.notify_target(menu);
        let owner = wm.frame_to_client.get(&owner).copied().unwrap_or(owner);
        let wnd_proc = wm.get_window(owner).map_or(0, |o| o.pfn_wp);
        if wnd_proc == 0 {
            return OwnerSetup::Skip;
        }
        let (w, h) = (owner_menu_item_w(), MENU_DROPDOWN_ITEM_H);
        let mut surface = Surface::new(w as u32, h as u32);
        surface.draw(DrawOp::Box { x1: 0, y1: 0, x2: w - 1, y2: h - 1, color: super::CONTROL_FACE_COLOR,
                                   fill: true, mix: Mix::Overpaint });
        let hbm = wm.create_bitmap(surface, 24, 1);
        let hps = wm.create_ps(0);
        if let Some(ps) = wm.ps_map.get_mut(&hps) {
            ps.bitmap = hbm;
        }
        let hl = MIA_HILITED as u32;
        let (now, old) = match hilite {
            Some(on) => (if on { attr | hl } else { attr & !hl }, if on { attr & !hl } else { attr | hl }),
            None => (attr, attr),
        };
        let block = vec![menu, hps, now, now, old, old, 0, 0, w as u32, h as u32, id as u32, 0];
        let menu_id = wm.get_window(menu).map_or(0, |m| m.id);
        OwnerSetup::Call { wnd_proc, owner, msg: WM_DRAWITEM, mp1: menu_id & 0xFFFF, mp2: 0, block, hps }
    }

    /// A `WM_DRAWITEM` for menu item `id` of `menu` returned `result`: TRUE
    /// keeps what the owner drew in its bitmap as the item's picture,
    /// FALSE leaves the item to be drawn as text. The bitmap and its
    /// presentation space go.
    fn menu_item_finish(&self, menu: u32, id: u16, hps: u32, result: u32) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let hbm = wm.ps_map.remove(&hps).map_or(0, |ps| ps.bitmap);
        let image = wm.bitmaps.remove(&hbm).and_then(|b| {
            let s = b.surface.lock_or_recover();
            let h = MENU_DROPDOWN_ITEM_H;
            let rows: Option<Vec<Vec<u32>>> = (0..h).rev().map(|y| s.row_pels(y)).collect();
            rows.map(|r| r.concat())
        });
        let frame = wm.notify_target(menu);
        if let Some(m) = wm.get_window_mut(menu)
            && let Some(item) = find_menu_item(&mut m.menu_items, id) {
            item.image = if result != 0 { image.map(std::sync::Arc::new) } else { None };
        }
        wm.mark_dirty(frame);
    }

    /// The `BDS_*` state user button `hwnd` (of `style`) is in.
    fn user_button_state(&self, hwnd: u32, style: u32) -> u32 {
        let hilited = self.shared.window_mgr.lock_or_recover().button_highlighted(hwnd);
        let mut state = 0;
        if hilited { state |= BDS_HILITED; }
        if style & WS_DISABLED != 0 { state |= BDS_DISABLED; }
        if style & BS_DEFAULT != 0 { state |= BDS_DEFAULT; }
        state
    }

    /// Owner call `call` returned `result`, with `block` (the structure
    /// passed, as the owner left it); `hps` is freed.
    pub(crate) fn owner_call_finish(&self, call: &OwnerCall, hps: u32, result: u32, block: &[u32]) {
        match call.kind {
            OwnerCallKind::DrawMenuItem { id, .. } => return self.menu_item_finish(call.control, id, hps, result),
            OwnerCallKind::MenuSelect { post: Some((msg, mp1, mp2)), .. } => {
                if result != 0 { self.post_menu_command(call.control, msg, mp1, mp2); }
                return;
            }
            OwnerCallKind::MenuInit { .. } | OwnerCallKind::MenuSelect { .. } | OwnerCallKind::MenuEnd { .. } => return,
            OwnerCallKind::EraseBackground => {
                return self.erase_background_finish(call.control, hps, result, block);
            }
            OwnerCallKind::SyncPaint => return,
            OwnerCallKind::ControlPointer { hptr } => return self.control_pointer_answered(hptr, result),
            OwnerCallKind::FrameFormat { count } => return self.frame_format_finish(call.control, count, result, block),
            // The owner's answer decides whether the page turns.
            OwnerCallKind::NotebookPending { cur, new } => {
                return self.notebook_turn_answered(call.control, cur, new, block);
            }
            // The owner's answer decides whether the edited text is kept.
            OwnerCallKind::ContainerEdit { code, block } if u32::from(code) == super::pm_container::CN_REALLOCPSZ => {
                return self.container_realloc_done(call.control, result, block);
            }
            _ => {}
        }
        // What the owner drew goes into the control: the item it drew, the
        // whole user button.
        let drawn = match call.kind {
            OwnerCallKind::DrawListItem { index } if result != 0 => self.list_item_rect(call.control, index),
            OwnerCallKind::UserButton => self.shared.window_mgr.lock_or_recover()
                .get_window(call.control).map(|w| (0, 0, w.cx, w.cy)),
            OwnerCallKind::SliderPart { part } if result != 0 => self.slider_part_rect(call.control, part),
            OwnerCallKind::ValueSetItem { row, col } if result != 0 => {
                self.valueset_item_rect(call.control, (row, col))
            }
            OwnerCallKind::ValueSetBackground if result != 0 => self.shared.window_mgr.lock_or_recover()
                .get_window(call.control).map(|w| (0, 0, w.cx, w.cy)),
            OwnerCallKind::ContainerDrawItem { rec, fi, part } if result != 0 => {
                self.container_item_rect(call.control, rec, fi, part)
            }
            _ => None,
        };
        if hps != 0 {
            self.release_owner_ps(call.control, hps, drawn);
        }
        match call.kind {
            OwnerCallKind::MeasureList { .. } => {
                let h = result as u16 as i16 as i32;
                if h > 0 {
                    let changed = self.shared.window_mgr.lock_or_recover().get_window_mut(call.control)
                        .map(|w| std::mem::replace(&mut w.item_height, h.min(MAX_ITEM_HEIGHT)) != w.item_height);
                    if changed == Some(true) {
                        self.repaint_control(call.control);
                    }
                }
            }
            OwnerCallKind::DrawListItem { index } => {
                let (frame, rect) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    (wm.find_frame_for_hwnd(call.control), wm.get_abs_rect_in_frame(call.control))
                };
                let p = Painter::new(self, frame, call.control);
                if result == 0 {
                    self.paint_list_item(&p, call.control, index, rect);
                } else if block.len() >= 5 && block[2] != block[4]
                    && let Some((l, b, r, t)) = self.list_item_rect(call.control, index)
                {
                    // TRUE with fsState != fsStateOld: the list box shows
                    // the selection by inverting the item.
                    let (ox, oy) = (rect.0, rect.1);
                    p.op(DrawOp::Box { x1: ox + l, y1: oy + b, x2: ox + r - 1, y2: oy + t - 1,
                                       color: 0, fill: true, mix: Mix::Invert });
                }
            }
            OwnerCallKind::SliderPart { part } => self.slider_owner_done(call.control, part, result != 0),
            OwnerCallKind::ValueSetItem { row, col } => {
                self.valueset_owner_done(call.control, Some((row, col)), result != 0);
            }
            OwnerCallKind::ValueSetBackground => self.valueset_owner_done(call.control, None, result != 0),
            // FALSE: the container draws the item itself.
            OwnerCallKind::ContainerDrawItem { rec, fi, part } if result == 0 => {
                let (frame, rect) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    (wm.find_frame_for_hwnd(call.control), wm.get_abs_rect_in_frame(call.control))
                };
                let p = Painter::new(self, frame, call.control);
                self.paint_container_item(&p, call.control, rect, rec, fi, part);
            }
            OwnerCallKind::ContainerDrawItem { .. } => {}
            OwnerCallKind::UserButton | OwnerCallKind::DrawMenuItem { .. }
            | OwnerCallKind::MenuInit { .. } | OwnerCallKind::MenuSelect { .. } | OwnerCallKind::MenuEnd { .. }
            | OwnerCallKind::EraseBackground | OwnerCallKind::SyncPaint
            | OwnerCallKind::NotebookSelected { .. } | OwnerCallKind::NotebookDeleted { .. }
            | OwnerCallKind::NotebookPending { .. }
            | OwnerCallKind::ContainerNotify { .. } | OwnerCallKind::ContainerValue { .. }
            | OwnerCallKind::ContainerInitDrag { .. } | OwnerCallKind::ContainerEdit { .. }
            | OwnerCallKind::FrameFormat { .. } | OwnerCallKind::ControlPointer { .. } => {}
        }
    }
}

#[cfg(test)]
impl Loader {
    /// Make the queued owner calls as a guest owner answering `reply(msg)`
    /// would; returns each call made as (owner, msg, mp1, mp2).
    pub(crate) fn drain_owner_calls(&self, reply: impl Fn(u32) -> u32) -> Vec<(u32, u32, u32, u32)> {
        let mut made = Vec::new();
        loop {
            let call = {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if wm.owner_calls.is_empty() { break; }
                wm.owner_calls.remove(0)
            };
            match self.owner_call_setup(&call) {
                OwnerSetup::Call { owner, msg, mp1, mp2, block, hps, .. } => {
                    made.push((owner, msg, mp1, mp2));
                    self.owner_call_finish(&call, hps, reply(msg), &block);
                }
                OwnerSetup::Skip => self.owner_call_finish(&call, 0, 0, &[]),
            }
        }
        made
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        loader: Loader,
        hmq: u32,
        client: u32,
    }

    /// A frame with a client whose window procedure is at 0x5000.
    fn fixture() -> Fx {
        let loader = Loader::new_mock();
        let (hmq, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("WC_FRAME".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            for (h, cx, cy) in [(frame, 400, 300), (client, 400, 280)] {
                let w = wm.get_window_mut(h).unwrap();
                w.cx = cx;
                w.cy = cy;
                w.visible = true;
            }
            wm.get_window_mut(client).unwrap().pfn_wp = 0x5000;
            (hmq, client)
        };
        Fx { loader, hmq, client }
    }

    impl Fx {
        fn control(&self, class: &str, style: u32, (x, y, cx, cy): (i32, i32, i32, i32), id: u32) -> u32 {
            let mut wm = self.loader.shared.window_mgr.lock_or_recover();
            let h = wm.create_window(class.into(), self.client, self.hmq);
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy, w.style, w.id, w.owner, w.visible) = (x, y, cx, cy, style, id, self.client, true);
            h
        }
        fn send(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(hwnd, msg, mp1, mp2) {
                crate::loader::ApiResult::Normal(v) => v,
                other => panic!("unexpected {other:?}"),
            }
        }
        fn insert(&self, list: u32, text: &str) {
            let addr = 0x0008_0000;
            self.loader.guest_write_bytes(addr, format!("{text}\0").as_bytes());
            self.send(list, LM_INSERTITEM, LIT_END, addr);
        }
        /// Run the waiting owner calls of the client's thread with `reply`
        /// standing in for the window procedure; returns what was sent.
        fn run(&self, reply: impl Fn(&OwnerSetup) -> (u32, Vec<u32>)) -> Vec<(u32, u32, u32, Vec<u32>)> {
            let mut sent = Vec::new();
            assert!(self.loader.shared.window_mgr.lock_or_recover().begin_owner_chain(self.hmq));
            loop {
                let call = self.loader.shared.window_mgr.lock_or_recover().next_owner_call(self.hmq);
                let Some(call) = call else { break };
                let setup = self.loader.owner_call_setup(&call);
                let (result, block) = reply(&setup);
                match &setup {
                    OwnerSetup::Call { owner, msg, mp1, mp2, block: b, hps, .. } => {
                        sent.push((*owner, *msg, *mp1, if b.is_empty() { vec![*mp2] } else { b.clone() }));
                        self.loader.owner_call_finish(&call, *hps, result, &block);
                    }
                    OwnerSetup::Skip => self.loader.owner_call_finish(&call, 0, 0, &[]),
                }
            }
            self.loader.shared.window_mgr.lock_or_recover().end_owner_chain(self.hmq);
            sent
        }
    }

    fn block_of(s: &OwnerSetup) -> Vec<u32> {
        match s { OwnerSetup::Call { block, .. } => block.clone(), OwnerSetup::Skip => Vec::new() }
    }

    #[test]
    fn inserting_into_an_owner_drawn_list_measures_then_draws_the_visible_items() {
        let fx = fixture();
        let list = fx.control("WC_LISTBOX", LS_OWNERDRAW, (10, 10, 120, 100), 7);
        fx.insert(list, "one");
        fx.insert(list, "two");
        fx.send(list, LM_SETITEMHANDLE, 1, 0xBEEF);
        let sent = fx.run(|s| match s {
            OwnerSetup::Call { msg: WM_MEASUREITEM, .. } => (24, Vec::new()),
            s => (1, block_of(s)),
        });
        assert_eq!(sent[0].1, WM_MEASUREITEM, "measured first");
        assert_eq!((sent[0].0, sent[0].2, sent[0].3[0]), (fx.client, 7, 0));
        let draws: Vec<_> = sent.iter().filter(|s| s.1 == WM_DRAWITEM).collect();
        assert_eq!(draws.len(), 2, "each visible item once, after the measure: {sent:?}");
        let b = &draws[1].3;
        assert_eq!(b[0], list);
        assert_eq!((b[10], b[11]), (1, 0xBEEF), "idItem, hItem");
        assert_eq!(b[9] - b[7], 24, "rclItem is one measured row high");
        assert_eq!(b[6], 1, "rclItem relative to the list box");
        assert_eq!(fx.loader.shared.window_mgr.lock_or_recover().get_window(list).unwrap().item_height, 24);
        // The presentation spaces and their canvases were freed.
        assert!(fx.loader.shared.window_mgr.lock_or_recover().ps_map.is_empty());
        assert!(fx.loader.shared.window_mgr.lock_or_recover().bitmaps.is_empty());
    }

    #[test]
    fn draw_item_reports_the_selection_and_false_draws_the_text() {
        let fx = fixture();
        let list = fx.control("WC_LISTBOX", LS_OWNERDRAW, (10, 10, 120, 100), 7);
        fx.insert(list, "alpha");
        fx.insert(list, "beta");
        fx.run(|s| (0, block_of(s)));
        fx.send(list, LM_SELECTITEM, 1, 1);
        let frame = fx.loader.shared.window_mgr.lock_or_recover().find_frame_for_hwnd(list);
        let surface = fx.loader.shared.window_mgr.lock_or_recover().frame_surface(frame).unwrap();
        surface.lock_or_recover().record = Some(Vec::new());
        let sent = fx.run(|s| (0, block_of(s)));
        let states: Vec<_> = sent.iter().filter(|s| s.1 == WM_DRAWITEM).map(|s| (s.3[10], s.3[2], s.3[4])).collect();
        assert_eq!(states, vec![(0, 0, 0), (1, 1, 0)]);
        // FALSE: the list box drew the items' text.
        let texts: Vec<String> = surface.lock_or_recover().record.iter().flatten().filter_map(|op| match op {
            DrawOp::Text { text, .. } => Some(text.clone()), _ => None,
        }).collect();
        assert_eq!(texts, vec!["alpha", "beta"]);
    }

    #[test]
    fn true_with_a_changed_state_inverts_the_item_and_equal_states_leave_it() {
        let fx = fixture();
        let list = fx.control("WC_LISTBOX", LS_OWNERDRAW, (10, 10, 120, 100), 7);
        fx.insert(list, "alpha");
        fx.send(list, LM_SELECTITEM, 0, 1);
        let frame = fx.loader.shared.window_mgr.lock_or_recover().find_frame_for_hwnd(list);
        let surface = fx.loader.shared.window_mgr.lock_or_recover().frame_surface(frame).unwrap();
        let inverts = || surface.lock_or_recover().record.iter().flatten()
            .filter(|op| matches!(op, DrawOp::Box { mix: Mix::Invert, .. })).count();
        surface.lock_or_recover().record = Some(Vec::new());
        fx.run(|s| (1, block_of(s)));
        assert_eq!(inverts(), 1, "fsState TRUE, fsStateOld FALSE: inverted");
        fx.loader.repaint_control(list);
        surface.lock_or_recover().record = Some(Vec::new());
        fx.run(|s| {
            let mut b = block_of(s);
            if b.len() > 4 { b[2] = 0; b[4] = 0; }
            (1, b)
        });
        assert_eq!(inverts(), 0, "the owner drew the selection itself");
    }

    #[test]
    fn user_buttons_send_bn_paint_with_their_state_and_the_old_one() {
        let fx = fixture();
        let button = fx.control("WC_BUTTON", BS_USERBUTTON | BS_DEFAULT, (10, 10, 80, 30), 9);
        fx.loader.repaint_control(button);
        let sent = fx.run(|s| (0, block_of(s)));
        assert_eq!(sent.len(), 1);
        let (owner, msg, mp1, b) = &sent[0];
        assert_eq!((*owner, *msg, *mp1), (fx.client, WM_CONTROL, (BN_PAINT << 16) | 9));
        assert_eq!((b[0], b[2], b[3]), (button, BDS_DEFAULT, 0));
        fx.send(button, BM_SETHILITE, 1, 0);
        let sent = fx.run(|s| (0, block_of(s)));
        assert_eq!((sent[0].3[2], sent[0].3[3]), (BDS_DEFAULT | BDS_HILITED, BDS_DEFAULT));
    }

    #[test]
    fn calls_wait_for_the_owners_thread_and_one_chain_runs_at_a_time() {
        let fx = fixture();
        let button = fx.control("WC_BUTTON", BS_USERBUTTON, (10, 10, 80, 30), 9);
        fx.loader.repaint_control(button);
        let mut wm = fx.loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.begin_owner_chain(fx.hmq + 1), "nothing for another queue");
        assert_eq!(wm.next_owner_call(fx.hmq + 1), None);
        assert!(wm.begin_owner_chain(fx.hmq));
        assert!(!wm.begin_owner_chain(fx.hmq), "already running");
        wm.end_owner_chain(fx.hmq);
        // The same call is queued once; the queue is bounded.
        let call = OwnerCall { control: button, kind: OwnerCallKind::UserButton };
        assert!(!wm.queue_owner_call(call));
        for i in 0..MAX_OWNER_CALLS + 10 {
            wm.queue_owner_call(OwnerCall { control: button, kind: OwnerCallKind::DrawListItem { index: i } });
        }
        assert_eq!(wm.owner_calls.len(), MAX_OWNER_CALLS);
    }

    #[test]
    fn owner_drawn_menu_items_are_drawn_into_a_picture_of_the_item() {
        let fx = fixture();
        let (frame, menu) = {
            let mut wm = fx.loader.shared.window_mgr.lock_or_recover();
            let frame = wm.get_window(fx.client).unwrap().parent;
            let menu = wm.create_window("#Menu".into(), frame, 0);
            let item = |id, style| MenuItem { id, style, attr: 0, text: "x".into(), children: vec![], image: None };
            let mut file = item(10, MIS_SUBMENU);
            file.children = vec![item(11, MIS_OWNERDRAW), item(12, 0)];
            let m = wm.get_window_mut(menu).unwrap();
            (m.menu_items, m.owner, m.id) = (vec![file], frame, FID_MENU);
            (m.cx, m.cy) = (400, MENU_BAR_HEIGHT as i32);
            (frame, menu)
        };
        let _ = frame;
        // Open the pull-down: only the owner-drawn item is queued.
        fx.send(menu, WM_BUTTON1DOWN, (MENU_BAR_ITEM_INSET as u32 + 2) | (2 << 16), 0);
        let sent = fx.run(|s| match s {
            // WM_INITMENU before the pull-down drops: nothing to draw.
            OwnerSetup::Call { msg, .. } if *msg != WM_DRAWITEM => (0, Vec::new()),
            OwnerSetup::Call { hps, .. } => {
                // The owner fills the item black through its PS.
                let target = fx.loader.shared.window_mgr.lock_or_recover().ps_draw_target(*hps).unwrap();
                fx.loader.draw_to(target, DrawOp::Box { x1: 0, y1: 0, x2: 4000, y2: 4000, color: 0,
                                                        fill: true, mix: Mix::Overpaint });
                (1, block_of(s))
            }
            OwnerSetup::Skip => (0, Vec::new()),
        });
        // WM_INITMENU for the bar and for File come first (pm_menubar.rs).
        assert_eq!(sent.iter().map(|s| (s.1, s.2)).collect::<Vec<_>>()[..2], [(WM_INITMENU, FID_MENU), (WM_INITMENU, 10)]);
        let sent: Vec<_> = sent.into_iter().filter(|s| s.1 == WM_DRAWITEM).collect();
        assert_eq!(sent.len(), 1, "{sent:?}");
        let (owner, msg, mp1, b) = &sent[0];
        assert_eq!((*owner, *msg, *mp1), (fx.client, WM_DRAWITEM, FID_MENU));
        assert_eq!((b[0], b[8], b[9], b[10]), (menu, owner_menu_item_w() as u32, MENU_DROPDOWN_ITEM_H as u32, 11));
        let wm = fx.loader.shared.window_mgr.lock_or_recover();
        let items = &wm.get_window(menu).unwrap().menu_items[0].children;
        let image = items[0].image.as_ref().expect("the owner's picture");
        assert_eq!(image.len(), (owner_menu_item_w() * MENU_DROPDOWN_ITEM_H) as usize);
        assert!(image.iter().all(|&p| p == 0));
        assert!(items[1].image.is_none());
        assert!(wm.bitmaps.is_empty() && wm.ps_map.is_empty(), "bitmap and PS freed");
    }

    #[test]
    fn the_owner_draws_in_control_coordinates_and_only_its_item_reaches_the_control() {
        let fx = fixture();
        let list = fx.control("WC_LISTBOX", LS_OWNERDRAW, (10, 10, 120, 100), 7);
        fx.insert(list, "one");
        fx.insert(list, "two");
        const PAINT: u32 = 0x00_12_34_56;
        fx.run(|s| match s {
            OwnerSetup::Call { msg: WM_DRAWITEM, hps, .. } => {
                // The owner paints far beyond its item.
                let target = fx.loader.shared.window_mgr.lock_or_recover().ps_draw_target(*hps).unwrap();
                fx.loader.draw_to(target, DrawOp::Box { x1: 0, y1: 0, x2: 500, y2: 500, color: PAINT,
                                                        fill: true, mix: Mix::Overpaint });
                (1, block_of(s))
            }
            s => (0, block_of(s)),
        });
        let (frame, (ax, ay, _, _)) = {
            let wm = fx.loader.shared.window_mgr.lock_or_recover();
            (wm.find_frame_for_hwnd(list), wm.get_abs_rect_in_frame(list))
        };
        let surface = fx.loader.shared.window_mgr.lock_or_recover().frame_surface(frame).unwrap();
        let s = surface.lock_or_recover();
        let (l, b, r, t) = fx.loader.list_item_rect(list, 0).unwrap();
        assert_eq!(s.pel(ax + l, ay + b), Some(PAINT), "the item");
        assert_eq!(s.pel(ax + r - 1, ay + t - 1), Some(PAINT));
        assert_ne!(s.pel(ax, ay), Some(PAINT), "the list's border is not the item");
        assert_ne!(s.pel(ax - 1, ay + b), Some(PAINT), "nothing outside the list box");
    }

    #[test]
    fn a_built_in_owner_gets_no_call_and_the_default_drawing_happens() {
        let fx = fixture();
        let list = fx.control("WC_LISTBOX", LS_OWNERDRAW, (10, 10, 120, 100), 7);
        fx.loader.shared.window_mgr.lock_or_recover().get_window_mut(fx.client).unwrap().pfn_wp = 0;
        fx.insert(list, "alpha");
        let sent = fx.run(|s| (0, block_of(s)));
        assert!(sent.is_empty());
    }
}
