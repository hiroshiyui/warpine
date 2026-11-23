// SPDX-License-Identifier: GPL-3.0-or-later
//
// OS/2 MOUCALLS (mouse) API for VIO programs.
//
// Behaviour follows the IBM *Control Program Programming Reference*
// (16-bit, "Mouse Function Calls": MouOpen, MouReadEventQue, MouSetEventMask,
// MouGetDevStatus / MouSetDevStatus, MouRemovePtr / MouDrawPtr,
// MouGetPtrShape / MouSetPtrShape, MouSetScaleFact, …); structures, flags
// and error codes come from the Open Watcom `<bsesub.h>` / `<bseerr.h>`,
// ordinals from the Open Watcom `os2386.lib` import library.
//
// The mouse of the one VIO session is per process here. Its events come from
// the host: the VIO window on the PM desktop (`gui::vio_window`) and the
// full-screen text window (`gui::text_renderer`) report the pointer in text
// cells through [`mouse_moved`] / [`mouse_button`].
//
// Choices without a documented source (unverified):
// - one handle, `MOU_HANDLE`; a second `MouOpen` returns it again without
//   resetting the state; a call with another or a closed handle fails with
//   `ERROR_MOUSE_INV_HANDLE`; `MouOpen` fails with `ERROR_MOUSE_NO_DEVICE`
//   when the program has no display (no mouse source: a terminal session);
// - the queue holds `MOU_QUEUE_MAX` events and drops the oldest when full;
// - a pointer that moves and changes a button in one host event is queued as
//   a motion event followed by a button event without motion;
// - `MouReadEventQue` without wait on an empty queue returns `NO_ERROR` with
//   an all-zero record (the reference says "a NULL record");
// - mickey mode reports, for motion events, the motion in mickeys computed
//   from the cell motion (8 × 16 pels per cell) and the scale factors
//   (mickeys per 8 pels); button events report 0, 0;
// - `MouSetPtrPos` queues no event; positions outside the screen and
//   collision areas that are not inside it are `ERROR_MOUSE_INV_PARMS`;
//   `NOPTRRECT` is read as the upper-left and lower-right cells, inclusive;
// - `MouSetEventMask` with reserved bits set fails with
//   `ERROR_MOUSE_INV_MASK`; `MouGetPtrShape` with a buffer smaller than the
//   image stores the size in `cb` and fails with `ERROR_MOUSE_INV_PARMS`;
// - 3 buttons, `MOU_MICKEYS_PER_CM` mickeys per centimetre;
// - the default text pointer is AND 0xFFFF / XOR 0x7700 (attribute inverted);
//   only the attribute byte of the masks is applied when the pointer is
//   drawn in the full-screen text window; in a PM window the PM pointer is
//   shown and the text pointer is never drawn;
// - the hot key is a set of button bits (1, 2, 4 for buttons 1–3; none at
//   first); other bits are `ERROR_MOUSE_INV_PARMS`;
// - `MouGetThreshold` / `MouSetThreshold` take the device handle the
//   reference reserves as 0, or the open handle; a `THRESHOLD` whose
//   `usLength` is below its size is `ERROR_MOUSE_INV_PARMS`; the defaults
//   are Warpine's (`MOU_DEFAULT_THRESHOLD`), and the levels change nothing,
//   as the host does the pointer's acceleration;
// - `MouRegister` checks its names' lengths (9 and 33 bytes with the NUL,
//   `ERROR_MOUSE_INVALID_ASCIIZ`) and the mask (bits for function codes 0
//   to 14H, `ERROR_MOUSE_INVALID_MASK`), and refuses a second registration
//   (`ERROR_MOUSE_REGISTER`); the registration is per process, and calls
//   are not passed to the registered entry point (not implemented);
//   `MouDeRegister` without one is `ERROR_MOUSE_DEREGISTER`;
// - `MouInitReal`, for the Base Video Subsystem at system initialisation,
//   is `ERROR_MOUSE_SMG_ONLY` for programs.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::Duration;

use log::{debug, warn};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::vm_backend::VcpuBackend;
use super::SharedState;

/// The handle `MouOpen` returns.
pub const MOU_HANDLE: u16 = 1;
/// Events the queue holds at most.
pub const MOU_QUEUE_MAX: usize = 64;
/// Buttons reported by `MouGetNumButtons`.
pub const MOU_NUM_BUTTONS: u16 = 3;
/// Mickeys per centimetre reported by `MouGetNumMickeys` (≈ 200 per inch).
pub const MOU_MICKEYS_PER_CM: u16 = 79;
/// `THRESHOLD`: usLength, then level 1, its multiplier, level 2, its
/// multiplier (USHORTs).
pub const THRESHOLD_SIZE: u16 = 10;
/// The thresholds before `MouSetThreshold` (Warpine's own).
pub const MOU_DEFAULT_THRESHOLD: [u16; 4] = [4, 2, 12, 4];
/// The hot key's possible button bits.
const MOU_HOTKEY_BUTTONS: u16 = 0x0007;
/// `MouRegister`'s longest module and entry names, NUL included.
const MOU_MODULE_NAME_MAX: u32 = 9;
const MOU_ENTRY_NAME_MAX: u32 = 33;
/// `MouRegister`'s mask: one bit per function code, 00H to 14H.
const MOU_REGISTER_MASK: u32 = (1 << 0x15) - 1;
/// Default scale factors after `MouOpen`: row, column (mickeys per 8 pels).
pub const MOU_DEFAULT_SCALE: (u16, u16) = (16, 8);
/// Largest valid scale factor (32K − 1).
pub const MOU_MAX_SCALE: u16 = 0x7FFF;
/// All event-mask bits (bits 7–15 are reserved).
pub const MOU_EVENT_BITS: u16 = 0x007F;
/// The `MouSetDevStatus` bits an application may set.
pub const MOU_SETTABLE_STATUS: u16 = MOUSE_DISABLED | MOUSE_MICKEYS;
/// Size in bytes of a text-mode pointer image (AND cell + XOR cell).
pub const MOU_TEXT_SHAPE_LEN: u16 = 4;
/// Default text pointer: AND mask cell, XOR mask cell (char low, attr high).
pub const MOU_DEFAULT_AND: u16 = 0xFFFF;
pub const MOU_DEFAULT_XOR: u16 = 0x7700;
/// Size of a packed `MOUEVENTINFO` (fs, time, row, col).
pub const MOUEVENTINFO_SIZE: u32 = 10;
/// Pels of a text cell, for mickey mode.
const CELL_W_PELS: i32 = 8;
const CELL_H_PELS: i32 = 16;

/// `MouSetPtrShape` / `MouGetPtrShape` ordinal-independent offsets in `PTRSHAPE`.
const PTRSHAPE_CB: u32 = 0;
const PTRSHAPE_COL: u32 = 2;
const PTRSHAPE_ROW: u32 = 4;
const PTRSHAPE_COLHOT: u32 = 6;
const PTRSHAPE_ROWHOT: u32 = 8;

/// One `MOUEVENTINFO`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MouEvent {
    pub fs: u16,
    pub time: u32,
    pub row: i16,
    pub col: i16,
}

/// A mouse button, numbered as OS/2 does: 1 left, 2 right, 3 middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouButton { B1, B2, B3 }

impl MouButton {
    fn bit(self) -> u8 {
        match self { MouButton::B1 => 1, MouButton::B2 => 2, MouButton::B3 => 4 }
    }
}

/// `fs` bits of buttons `buttons` (bit 0 = button 1) held, without motion.
fn down_bits(buttons: u8) -> u16 {
    let mut fs = 0;
    if buttons & 1 != 0 { fs |= MOUSE_BN1_DOWN; }
    if buttons & 2 != 0 { fs |= MOUSE_BN2_DOWN; }
    if buttons & 4 != 0 { fs |= MOUSE_BN3_DOWN; }
    fs
}

/// `fs` bits of a motion with buttons `buttons` held.
fn motion_bits(buttons: u8) -> u16 {
    let mut fs = 0;
    if buttons & 1 != 0 { fs |= MOUSE_MOTION_WITH_BN1_DOWN; }
    if buttons & 2 != 0 { fs |= MOUSE_MOTION_WITH_BN2_DOWN; }
    if buttons & 4 != 0 { fs |= MOUSE_MOTION_WITH_BN3_DOWN; }
    if fs == 0 { MOUSE_MOTION } else { fs }
}

/// A rectangle of cells, inclusive (`NOPTRRECT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect { pub row: u16, pub col: u16, pub row2: u16, pub col2: u16 }

impl CellRect {
    fn contains(&self, row: u16, col: u16) -> bool {
        (self.row..=self.row2).contains(&row) && (self.col..=self.col2).contains(&col)
    }
}

/// Where and how to draw the text pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPointer { pub row: u16, pub col: u16, pub and_mask: u16, pub xor_mask: u16 }

impl TextPointer {
    /// Apply the masks' attribute bytes to the attribute `attr` of the cell.
    pub fn apply_attr(&self, attr: u8) -> u8 {
        (attr & (self.and_mask >> 8) as u8) ^ (self.xor_mask >> 8) as u8
    }
}

/// The mouse of the VIO session.
#[derive(Debug)]
pub struct MouManager {
    open: bool,
    queue: VecDeque<MouEvent>,
    event_mask: u16,
    dev_status: u16,
    scale: (u16, u16),
    row: u16,
    col: u16,
    buttons: u8,
    shape: (u16, u16),
    hot: (u16, u16),
    /// The collision area: the pointer is not drawn inside it.
    collision: Option<CellRect>,
    /// Threads blocked in `MouReadEventQue`.
    readers: u32,
    /// `MouSetHotKey`'s buttons; `MouSetThreshold`'s levels; whether a
    /// subsystem is registered (`MouRegister`). They outlast `MouOpen`.
    hot_key: u16,
    threshold: [u16; 4],
    registered: bool,
}

impl Default for MouManager {
    fn default() -> Self { Self::new() }
}

impl MouManager {
    pub fn new() -> Self {
        MouManager {
            open: false,
            queue: VecDeque::new(),
            event_mask: MOU_EVENT_BITS,
            dev_status: 0,
            scale: MOU_DEFAULT_SCALE,
            row: 0,
            col: 0,
            buttons: 0,
            shape: (MOU_DEFAULT_AND, MOU_DEFAULT_XOR),
            hot: (0, 0),
            collision: None,
            readers: 0,
            hot_key: 0,
            threshold: MOU_DEFAULT_THRESHOLD,
            registered: false,
        }
    }

    pub fn hot_key(&self) -> u16 { self.hot_key }

    /// `MouSetHotKey`: buttons 1–3 only.
    pub fn set_hot_key(&mut self, buttons: u16) -> Result<(), u32> {
        if buttons & !MOU_HOTKEY_BUTTONS != 0 { return Err(ERROR_MOUSE_INV_PARMS); }
        self.hot_key = buttons;
        Ok(())
    }

    pub fn threshold(&self) -> [u16; 4] { self.threshold }
    pub fn set_threshold(&mut self, t: [u16; 4]) { self.threshold = t; }

    /// `MouRegister` with mask `mask`: one at a time.
    pub fn register(&mut self, mask: u32) -> Result<(), u32> {
        if mask == 0 || mask & !MOU_REGISTER_MASK != 0 { return Err(ERROR_MOUSE_INVALID_MASK); }
        if self.registered { return Err(ERROR_MOUSE_REGISTER); }
        self.registered = true;
        Ok(())
    }

    /// `MouDeRegister`.
    pub fn deregister(&mut self) -> Result<(), u32> {
        if !std::mem::take(&mut self.registered) { return Err(ERROR_MOUSE_DEREGISTER); }
        Ok(())
    }

    /// `MouOpen` on a `cols × rows` screen: the first open sets the state
    /// the reference lists (pointer centred and not drawn — the collision
    /// area is the whole screen —, all events, empty queue, status bits
    /// clear, scale 16/8, default shape). Returns the handle.
    pub fn open(&mut self, rows: u16, cols: u16) -> u16 {
        if !self.open {
            let (buttons, hot_key, threshold, registered) = (self.buttons, self.hot_key, self.threshold, self.registered);
            *self = MouManager::new();
            (self.buttons, self.hot_key, self.threshold, self.registered) = (buttons, hot_key, threshold, registered);
            self.open = true;
            self.row = rows / 2;
            self.col = cols / 2;
            self.collision = Some(CellRect {
                row: 0, col: 0, row2: rows.saturating_sub(1), col2: cols.saturating_sub(1),
            });
        }
        MOU_HANDLE
    }

    /// Whether `hmou` is the open handle.
    pub fn check(&self, hmou: u16) -> Result<(), u32> {
        if self.open && hmou == MOU_HANDLE { Ok(()) } else { Err(ERROR_MOUSE_INV_HANDLE) }
    }

    pub fn close(&mut self, hmou: u16) -> Result<(), u32> {
        self.check(hmou)?;
        self.open = false;
        self.queue.clear();
        Ok(())
    }

    pub fn is_open(&self) -> bool { self.open }
    pub fn len(&self) -> usize { self.queue.len() }
    pub fn is_empty(&self) -> bool { self.queue.is_empty() }
    pub fn flush(&mut self) { self.queue.clear(); }
    pub fn read(&mut self) -> Option<MouEvent> { self.queue.pop_front() }
    pub fn position(&self) -> (u16, u16) { (self.row, self.col) }
    pub fn event_mask(&self) -> u16 { self.event_mask }

    pub fn set_event_mask(&mut self, mask: u16) -> Result<(), u32> {
        if mask & !MOU_EVENT_BITS != 0 { return Err(ERROR_MOUSE_INV_MASK); }
        self.event_mask = mask;
        Ok(())
    }

    /// The device status: the settable bits plus a block read in progress.
    pub fn dev_status(&self) -> u16 {
        self.dev_status | if self.readers > 0 { MOUSE_BLOCKREAD } else { 0 }
    }

    pub fn set_dev_status(&mut self, status: u16) -> Result<(), u32> {
        if status & !MOU_SETTABLE_STATUS != 0 { return Err(ERROR_MOUSE_INV_PARMS); }
        self.dev_status = status;
        Ok(())
    }

    pub fn scale(&self) -> (u16, u16) { self.scale }

    pub fn set_scale(&mut self, row: u16, col: u16) -> Result<(), u32> {
        let ok = |v: u16| (1..=MOU_MAX_SCALE).contains(&v);
        if !ok(row) || !ok(col) { return Err(ERROR_MOUSE_INV_PARMS); }
        self.scale = (row, col);
        Ok(())
    }

    /// `MouSetPtrPos` on a `cols × rows` screen.
    pub fn set_position(&mut self, row: u16, col: u16, rows: u16, cols: u16) -> Result<(), u32> {
        if row >= rows || col >= cols { return Err(ERROR_MOUSE_INV_PARMS); }
        (self.row, self.col) = (row, col);
        Ok(())
    }

    /// `MouRemovePtr` on a `cols × rows` screen.
    pub fn remove_pointer(&mut self, area: CellRect, rows: u16, cols: u16) -> Result<(), u32> {
        if area.row > area.row2 || area.col > area.col2 || area.row2 >= rows || area.col2 >= cols {
            return Err(ERROR_MOUSE_INV_PARMS);
        }
        self.collision = Some(area);
        Ok(())
    }

    /// `MouDrawPtr`: no collision area any more.
    pub fn draw_pointer(&mut self) { self.collision = None; }

    pub fn shape(&self) -> ((u16, u16), (u16, u16)) { (self.shape, self.hot) }

    pub fn set_shape(&mut self, and_mask: u16, xor_mask: u16, col_hot: u16, row_hot: u16) {
        self.shape = (and_mask, xor_mask);
        self.hot = (col_hot, row_hot);
    }

    /// The text pointer to draw, if any: the mouse is open, drawing is not
    /// disabled and the pointer is outside the collision area.
    pub fn text_pointer(&self) -> Option<TextPointer> {
        if !self.open || self.dev_status & MOUSE_DISABLED != 0 { return None; }
        if self.collision.is_some_and(|c| c.contains(self.row, self.col)) { return None; }
        Some(TextPointer { row: self.row, col: self.col, and_mask: self.shape.0, xor_mask: self.shape.1 })
    }

    /// Queue `ev` if the mouse is open and the mask reports one of `kind`.
    fn queue(&mut self, kind: u16, ev: MouEvent) {
        if !self.open || self.event_mask & kind == 0 { return; }
        if self.queue.len() >= MOU_QUEUE_MAX {
            self.queue.pop_front();
        }
        self.queue.push_back(ev);
    }

    /// Host input: the pointer is over cell (`row`, `col`) with `buttons`
    /// (bit 0 = button 1) held, at `time` ms. Returns whether an event was
    /// queued.
    pub fn update(&mut self, row: u16, col: u16, buttons: u8, time: u32) -> bool {
        let before = self.queue.len();
        let (row, col, buttons) = (row.min(i16::MAX as u16), col.min(i16::MAX as u16), buttons & 7);
        if (row, col) != (self.row, self.col) {
            let fs = motion_bits(self.buttons);
            let (er, ec) = if self.dev_status & MOUSE_MICKEYS != 0 {
                let mickeys = |cells: i32, pels: i32, scale: u16| {
                    (cells * pels * scale as i32 / 8).clamp(i16::MIN as i32, i16::MAX as i32) as i16
                };
                (mickeys(row as i32 - self.row as i32, CELL_H_PELS, self.scale.0),
                 mickeys(col as i32 - self.col as i32, CELL_W_PELS, self.scale.1))
            } else {
                (row as i16, col as i16)
            };
            (self.row, self.col) = (row, col);
            self.queue(fs, MouEvent { fs, time, row: er, col: ec });
        }
        if buttons != self.buttons {
            let changed = buttons ^ self.buttons;
            self.buttons = buttons;
            let (er, ec) = if self.dev_status & MOUSE_MICKEYS != 0 { (0, 0) } else { (row as i16, col as i16) };
            self.queue(down_bits(changed), MouEvent { fs: down_bits(buttons), time, row: er, col: ec });
        }
        self.queue.len() != before
    }

    /// The buttons held now (bit 0 = button 1).
    pub fn buttons(&self) -> u8 { self.buttons }
}

/// Milliseconds since the host booted (the `QSV_MS_COUNT` clock).
fn now_ms() -> u32 {
    super::stubs::ms_count()
}

/// Host input: the pointer moved to cell (`row`, `col`).
pub fn mouse_moved(shared: &SharedState, row: u16, col: u16) {
    let mut mou = shared.mou_mgr.lock_or_recover();
    let buttons = mou.buttons();
    if mou.update(row, col, buttons, now_ms()) {
        shared.mou_cond.notify_all();
    }
}

/// Host input: `button` went down (`down`) or up over cell (`row`, `col`).
pub fn mouse_button(shared: &SharedState, row: u16, col: u16, button: MouButton, down: bool) {
    let mut mou = shared.mou_mgr.lock_or_recover();
    let buttons = if down { mou.buttons() | button.bit() } else { mou.buttons() & !button.bit() };
    if mou.update(row, col, buttons, now_ms()) {
        shared.mou_cond.notify_all();
    }
}

/// Whether any mouse button is held (a drag goes on outside the cells).
pub fn any_button_down(shared: &SharedState) -> bool {
    shared.mou_mgr.lock_or_recover().buttons() != 0
}

/// The text pointer to draw in the full-screen text window, if any.
pub fn text_pointer(shared: &SharedState) -> Option<TextPointer> {
    shared.mou_mgr.lock_or_recover().text_pointer()
}

impl super::Loader {
    pub(crate) fn handle_moucalls(&self, vcpu: &mut dyn VcpuBackend, _vcpu_id: u32, ordinal: u32) -> super::ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp as u32;
        // MOUCALLS uses the Pascal convention: the last argument at ESP+4.
        let arg = |n: u32| -> u32 { self.guest_read::<u32>(esp.wrapping_add(4 * n)).unwrap_or(0) };
        let h = |n: u32| -> u16 { arg(n) as u16 };
        let res = match ordinal {
            // MouGetPtrShape(pBuf, pShape, hmou)
            1  => self.mou_get_ptr_shape(arg(3), arg(2), h(1)),
            // MouSetPtrShape(pBuf, pShape, hmou)
            2  => self.mou_set_ptr_shape(arg(3), arg(2), h(1)),
            // MouGetNumMickeys(pcMickeys, hmou)
            3  => self.mou_write_u16(arg(2), h(1), |_| MOU_MICKEYS_PER_CM),
            // MouGetScaleFact(pScale, hmou)
            6  => self.mou_get_pair(arg(2), h(1), |m| m.scale()),
            // MouFlushQue(hmou)
            7  => self.mou_with(h(1), |m| { m.flush(); Ok(()) }),
            // MouGetNumButtons(pcButtons, hmou)
            8  => self.mou_write_u16(arg(2), h(1), |_| MOU_NUM_BUTTONS),
            // MouClose(hmou)
            9  => self.mou_with(h(1), |m| m.close(MOU_HANDLE)),
            // MouSetScaleFact(pScale, hmou)
            11 => self.mou_set_pair(arg(2), h(1), |m, row, col| m.set_scale(row, col)),
            // MouGetNumQueEl(pQueInfo, hmou)
            13 => self.mou_get_pair(arg(2), h(1), |m| (m.len() as u16, MOU_QUEUE_MAX as u16)),
            // MouGetEventMask(pMask, hmou)
            15 => self.mou_write_u16(arg(2), h(1), MouManager::event_mask),
            // MouSetEventMask(pMask, hmou)
            16 => self.mou_set_u16(arg(2), h(1), MouManager::set_event_mask),
            // MouOpen(pszDriver, phmou)
            17 => self.mou_open(arg(2), arg(1)),
            // MouRemovePtr(pRect, hmou)
            18 => self.mou_remove_ptr(arg(2), h(1)),
            // MouGetPtrPos(pPos, hmou)
            19 => self.mou_get_pair(arg(2), h(1), |m| m.position()),
            // MouReadEventQue(pEvent, pReadType, hmou)
            20 => self.mou_read_event_que(arg(3), arg(2), h(1)),
            // MouSetPtrPos(pPos, hmou)
            21 => {
                let (rows, cols) = self.vio_screen_size();
                self.mou_set_pair(arg(2), h(1), |m, row, col| m.set_position(row, col, rows, cols))
            }
            // MouGetDevStatus(pStatus, hmou)
            22 => self.mou_write_u16(arg(2), h(1), MouManager::dev_status),
            // MouSynch(fWait): nothing to synchronise with.
            23 => NO_ERROR,
            // MouGetHotKey(pfsButtons, hmou) / MouSetHotKey(pfsButtons, hmou)
            4  => self.mou_write_u16(arg(2), h(1), MouManager::hot_key),
            10 => self.mou_set_u16(arg(2), h(1), MouManager::set_hot_key),
            // MouDeRegister()
            14 => self.shared.mou_mgr.lock_or_recover().deregister().err().unwrap_or(NO_ERROR),
            // MouRegister(pszModName, pszEntryName, flFuncs)
            24 => self.mou_register(arg(3), arg(2), arg(1)),
            // MouInitReal(pszDriver): the Base Video Subsystem's alone.
            27 => ERROR_MOUSE_SMG_ONLY,
            // MouGetThreshold(pThreshold, hmou) / MouSetThreshold(pThreshold, hmou)
            29 => self.mou_threshold(arg(2), h(1), false),
            30 => self.mou_threshold(arg(2), h(1), true),
            // MouSetDevStatus(pStatus, hmou)
            25 => self.mou_set_u16(arg(2), h(1), MouManager::set_dev_status),
            // MouDrawPtr(hmou)
            26 => self.mou_with(h(1), |m| { m.draw_pointer(); Ok(()) }),
            _ => {
                warn!("Unimplemented MOUCALLS ordinal {}", ordinal);
                ERROR_INVALID_FUNCTION
            }
        };
        super::ApiResult::Normal(res)
    }

    /// Pascal convention: bytes of arguments the callee pops.
    pub(crate) fn moucalls_arg_bytes(&self, ordinal: u32) -> u64 {
        match ordinal {
            1 | 2 | 20 | 24 => 12,
            3 | 4 | 6 | 8 | 10 | 11 | 13 | 15 | 16 | 17 | 18 | 19 | 21 | 22 | 25 | 29 | 30 => 8,
            7 | 9 | 23 | 26 | 27 => 4,
            _ => 0,
        }
    }

    /// `MouRegister`: the names must fit (NUL included), the mask name
    /// functions that exist.
    fn mou_register(&self, module: u32, entry: u32, mask: u32) -> u32 {
        let fits = |p: u32, max: u32| p != 0
            && (0..max).any(|i| self.guest_read::<u8>(p.wrapping_add(i)) == Some(0));
        if !fits(module, MOU_MODULE_NAME_MAX) || !fits(entry, MOU_ENTRY_NAME_MAX) {
            return ERROR_MOUSE_INVALID_ASCIIZ;
        }
        self.shared.mou_mgr.lock_or_recover().register(mask).err().unwrap_or(NO_ERROR)
    }

    /// `MouGetThreshold` (`set` false) / `MouSetThreshold` with the
    /// `THRESHOLD` at `ptr`; `hmou` 0 (reserved) or the open handle.
    fn mou_threshold(&self, ptr: u32, hmou: u16, set: bool) -> u32 {
        let mut mou = self.shared.mou_mgr.lock_or_recover();
        if hmou != 0 && let Err(e) = mou.check(hmou) { return e; }
        let len = (ptr != 0).then(|| self.guest_read::<u16>(ptr)).flatten();
        if len.is_none_or(|l| l < THRESHOLD_SIZE) { return ERROR_MOUSE_INV_PARMS; }
        let field = |i: u32| ptr.wrapping_add(2 + 2 * i);
        if set {
            let mut t = [0u16; 4];
            for (i, v) in t.iter_mut().enumerate() {
                let Some(x) = self.guest_read::<u16>(field(i as u32)) else { return ERROR_MOUSE_INV_PARMS };
                *v = x;
            }
            mou.set_threshold(t);
        } else {
            for (i, v) in mou.threshold().iter().enumerate() {
                if self.guest_write::<u16>(field(i as u32), *v).is_none() { return ERROR_MOUSE_INV_PARMS; }
            }
        }
        NO_ERROR
    }

    /// The VIO screen size: rows, columns.
    fn vio_screen_size(&self) -> (u16, u16) {
        let con = self.shared.console_mgr.lock_or_recover();
        (con.rows, con.cols)
    }

    /// Run `f` on the mouse if `hmou` is its handle.
    fn mou_with(&self, hmou: u16, f: impl FnOnce(&mut MouManager) -> Result<(), u32>) -> u32 {
        let mut mou = self.shared.mou_mgr.lock_or_recover();
        match mou.check(hmou).and_then(|()| f(&mut mou)) {
            Ok(()) => NO_ERROR,
            Err(e) => e,
        }
    }

    /// Write the USHORT `f` gives to `ptr`.
    fn mou_write_u16(&self, ptr: u32, hmou: u16, f: impl FnOnce(&MouManager) -> u16) -> u32 {
        let v = {
            let mou = self.shared.mou_mgr.lock_or_recover();
            if let Err(e) = mou.check(hmou) { return e; }
            f(&mou)
        };
        if ptr == 0 || self.guest_write::<u16>(ptr, v).is_none() { return ERROR_MOUSE_INV_PARMS; }
        NO_ERROR
    }

    /// Read a USHORT from `ptr` and pass it to `f`.
    fn mou_set_u16(&self, ptr: u32, hmou: u16, f: impl FnOnce(&mut MouManager, u16) -> Result<(), u32>) -> u32 {
        let Some(v) = (ptr != 0).then(|| self.guest_read::<u16>(ptr)).flatten() else {
            return self.mou_with(hmou, |_| Err(ERROR_MOUSE_INV_PARMS));
        };
        self.mou_with(hmou, |m| f(m, v))
    }

    /// Write the pair of USHORTs `f` gives to `ptr` (PTRLOC, SCALEFACT, MOUQUEINFO).
    fn mou_get_pair(&self, ptr: u32, hmou: u16, f: impl FnOnce(&MouManager) -> (u16, u16)) -> u32 {
        let (a, b) = {
            let mou = self.shared.mou_mgr.lock_or_recover();
            if let Err(e) = mou.check(hmou) { return e; }
            f(&mou)
        };
        if ptr == 0 || self.guest_write::<u16>(ptr, a).and(self.guest_write::<u16>(ptr.wrapping_add(2), b)).is_none() {
            return ERROR_MOUSE_INV_PARMS;
        }
        NO_ERROR
    }

    /// Read a pair of USHORTs from `ptr` and pass them to `f`.
    fn mou_set_pair(&self, ptr: u32, hmou: u16, f: impl FnOnce(&mut MouManager, u16, u16) -> Result<(), u32>) -> u32 {
        let pair = (ptr != 0).then(|| {
            Some((self.guest_read::<u16>(ptr)?, self.guest_read::<u16>(ptr.wrapping_add(2))?))
        }).flatten();
        let Some((a, b)) = pair else {
            return self.mou_with(hmou, |_| Err(ERROR_MOUSE_INV_PARMS));
        };
        self.mou_with(hmou, |m| f(m, a, b))
    }

    /// MouOpen (ordinal 17). The driver name is not used: Warpine has one
    /// pointer driver.
    fn mou_open(&self, psz_driver: u32, phmou: u32) -> u32 {
        debug!("  MouOpen(driver=0x{:08X})", psz_driver);
        if !self.shared.use_sdl_text.load(Ordering::Relaxed) {
            // No display, so no pointer: a session on the host terminal.
            return ERROR_MOUSE_NO_DEVICE;
        }
        if phmou == 0 { return ERROR_MOUSE_INV_PARMS; }
        let (rows, cols) = self.vio_screen_size();
        // Write the handle before the device is opened, so a bad pointer
        // leaves it closed.
        if self.guest_write::<u16>(phmou, MOU_HANDLE).is_none() { return ERROR_MOUSE_INV_PARMS; }
        self.shared.mou_mgr.lock_or_recover().open(rows, cols);
        NO_ERROR
    }

    /// MouReadEventQue (ordinal 20).
    fn mou_read_event_que(&self, p_event: u32, p_read_type: u32, hmou: u16) -> u32 {
        let read_type = if p_read_type == 0 { None } else { self.guest_read::<u16>(p_read_type) };
        let wait = match read_type {
            Some(MOU_NOWAIT) => false,
            Some(MOU_WAIT) => true,
            _ => return self.mou_with(hmou, |_| Err(ERROR_MOUSE_INV_PARMS)),
        };
        if p_event == 0 || self.guest_read_bytes(p_event, MOUEVENTINFO_SIZE as usize).is_none() {
            return self.mou_with(hmou, |_| Err(ERROR_MOUSE_INV_PARMS));
        }
        let mut mou = self.shared.mou_mgr.lock_or_recover();
        let ev = loop {
            if let Err(e) = mou.check(hmou) {
                return e;
            }
            if let Some(ev) = mou.read() { break ev; }
            if !wait { break MouEvent::default(); }
            if self.stop_or_restart() { return ERROR_MOUSE_NO_DEVICE; }
            // Wait for an event; look at the exit flag every 50 ms.
            mou.readers += 1;
            let (guard, _) = self.shared.mou_cond
                .wait_timeout(mou, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner());
            mou = guard;
            mou.readers -= 1;
        };
        drop(mou);
        self.write_mou_event(p_event, ev);
        NO_ERROR
    }

    /// Write a packed MOUEVENTINFO (checked by the caller to be in guest memory).
    fn write_mou_event(&self, p: u32, ev: MouEvent) {
        let mut rec = [0u8; MOUEVENTINFO_SIZE as usize];
        rec[0..2].copy_from_slice(&ev.fs.to_le_bytes());
        rec[2..6].copy_from_slice(&ev.time.to_le_bytes());
        rec[6..8].copy_from_slice(&ev.row.to_le_bytes());
        rec[8..10].copy_from_slice(&ev.col.to_le_bytes());
        let _ = self.guest_write_bytes(p, &rec);
    }

    /// MouRemovePtr (ordinal 18): read NOPTRRECT.
    fn mou_remove_ptr(&self, p_rect: u32, hmou: u16) -> u32 {
        let rd = |off: u32| self.guest_read::<u16>(p_rect.wrapping_add(off));
        let rect = (p_rect != 0).then(|| Some(CellRect { row: rd(0)?, col: rd(2)?, row2: rd(4)?, col2: rd(6)? })).flatten();
        let Some(rect) = rect else { return self.mou_with(hmou, |_| Err(ERROR_MOUSE_INV_PARMS)); };
        let (rows, cols) = self.vio_screen_size();
        self.mou_with(hmou, |m| m.remove_pointer(rect, rows, cols))
    }

    /// MouGetPtrShape (ordinal 1): the 4-byte text pointer image.
    fn mou_get_ptr_shape(&self, p_buf: u32, p_shape: u32, hmou: u16) -> u32 {
        let ((and_mask, xor_mask), (col_hot, row_hot)) = {
            let mou = self.shared.mou_mgr.lock_or_recover();
            if let Err(e) = mou.check(hmou) { return e; }
            mou.shape()
        };
        if p_shape == 0 { return ERROR_MOUSE_INV_PARMS; }
        let Some(cb) = self.guest_read::<u16>(p_shape.wrapping_add(PTRSHAPE_CB)) else { return ERROR_MOUSE_INV_PARMS };
        let fields = [(PTRSHAPE_CB, MOU_TEXT_SHAPE_LEN), (PTRSHAPE_COL, 1), (PTRSHAPE_ROW, 1),
                      (PTRSHAPE_COLHOT, col_hot), (PTRSHAPE_ROWHOT, row_hot)];
        if self.guest_read_bytes(p_shape, 10).is_none() { return ERROR_MOUSE_INV_PARMS; }
        if cb < MOU_TEXT_SHAPE_LEN {
            // Too small: report the size needed.
            let _ = self.guest_write::<u16>(p_shape.wrapping_add(PTRSHAPE_CB), MOU_TEXT_SHAPE_LEN);
            return ERROR_MOUSE_INV_PARMS;
        }
        let mut img = [0u8; MOU_TEXT_SHAPE_LEN as usize];
        img[0..2].copy_from_slice(&and_mask.to_le_bytes());
        img[2..4].copy_from_slice(&xor_mask.to_le_bytes());
        if p_buf == 0 || self.guest_write_bytes(p_buf, &img).is_none() { return ERROR_MOUSE_INV_PARMS; }
        for (off, v) in fields {
            let _ = self.guest_write::<u16>(p_shape.wrapping_add(off), v);
        }
        NO_ERROR
    }

    /// MouSetPtrShape (ordinal 2): text modes take a 1 × 1 image of 4 bytes.
    fn mou_set_ptr_shape(&self, p_buf: u32, p_shape: u32, hmou: u16) -> u32 {
        let rd = |off: u32| self.guest_read::<u16>(p_shape.wrapping_add(off));
        let def = (p_shape != 0).then(|| Some((rd(PTRSHAPE_CB)?, rd(PTRSHAPE_COL)?, rd(PTRSHAPE_ROW)?,
                                                rd(PTRSHAPE_COLHOT)?, rd(PTRSHAPE_ROWHOT)?))).flatten();
        let img = (p_buf != 0).then(|| self.guest_read_bytes(p_buf, MOU_TEXT_SHAPE_LEN as usize)).flatten();
        match (def, img) {
            (Some((MOU_TEXT_SHAPE_LEN, 1, 1, col_hot, row_hot)), Some(img)) => {
                let and_mask = u16::from_le_bytes([img[0], img[1]]);
                let xor_mask = u16::from_le_bytes([img[2], img[3]]);
                self.mou_with(hmou, |m| { m.set_shape(and_mask, xor_mask, col_hot, row_hot); Ok(()) })
            }
            _ => self.mou_with(hmou, |_| Err(ERROR_MOUSE_INV_PARMS)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{Loader, ApiResult};
    use super::super::vm_backend::mock::MockVcpu;

    const ESP: u32 = 0x1000;
    const BUF: u32 = 0x2000;

    /// Call MOUCALLS `ordinal` with Pascal arguments `args` (first to last).
    fn call(loader: &Loader, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        vcpu.regs.rsp = ESP as u64;
        for (i, a) in args.iter().rev().enumerate() {
            loader.guest_write::<u32>(ESP + 4 + 4 * i as u32, *a).unwrap();
        }
        match loader.handle_moucalls(&mut vcpu, 0, ordinal) {
            ApiResult::Normal(r) => r,
            _ => panic!("unexpected result"),
        }
    }

    fn opened() -> Loader {
        let loader = Loader::new_mock();
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        assert_eq!(call(&loader, 17, &[0, BUF]), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(MOU_HANDLE));
        loader
    }

    const H: u32 = MOU_HANDLE as u32;

    fn read_event(loader: &Loader, wait: u16) -> (u32, MouEvent) {
        loader.guest_write::<u16>(BUF + 0x100, wait).unwrap();
        let rc = call(loader, 20, &[BUF, BUF + 0x100, H]);
        let ev = MouEvent {
            fs: loader.guest_read(BUF).unwrap(),
            time: loader.guest_read(BUF + 2).unwrap(),
            row: loader.guest_read(BUF + 6).unwrap(),
            col: loader.guest_read(BUF + 8).unwrap(),
        };
        (rc, ev)
    }

    // ── The queue model ─────────────────────────────────────────────────────

    #[test]
    fn open_sets_the_documented_initial_state() {
        let mut m = MouManager::new();
        assert_eq!(m.check(MOU_HANDLE), Err(ERROR_MOUSE_INV_HANDLE));
        assert_eq!(m.open(25, 80), MOU_HANDLE);
        assert_eq!(m.position(), (12, 40), "centred");
        assert_eq!(m.event_mask(), MOU_EVENT_BITS, "all events");
        assert_eq!((m.dev_status(), m.scale(), m.len()), (0, (16, 8), 0));
        assert_eq!(m.text_pointer(), None, "the collision area is the whole screen");
        m.draw_pointer();
        assert_eq!(m.text_pointer(), Some(TextPointer { row: 12, col: 40, and_mask: 0xFFFF, xor_mask: 0x7700 }));
        // A second open keeps the state.
        m.set_event_mask(MOUSE_BN1_DOWN).unwrap();
        m.open(25, 80);
        assert_eq!(m.event_mask(), MOUSE_BN1_DOWN);
        assert_eq!(m.check(2), Err(ERROR_MOUSE_INV_HANDLE));
        m.close(MOU_HANDLE).unwrap();
        assert_eq!(m.close(MOU_HANDLE), Err(ERROR_MOUSE_INV_HANDLE));
    }

    #[test]
    fn motion_and_buttons_make_events_with_the_resulting_state() {
        let mut m = MouManager::new();
        m.open(25, 80);
        assert!(m.update(3, 4, 0, 10));
        assert_eq!(m.read(), Some(MouEvent { fs: MOUSE_MOTION, time: 10, row: 3, col: 4 }));
        assert!(!m.update(3, 4, 0, 11), "nothing changed");
        // A press at a new cell: motion first, then the press without motion.
        assert!(m.update(5, 6, 1, 12));
        assert_eq!(m.read(), Some(MouEvent { fs: MOUSE_MOTION, time: 12, row: 5, col: 6 }));
        assert_eq!(m.read(), Some(MouEvent { fs: MOUSE_BN1_DOWN, time: 12, row: 5, col: 6 }));
        // Dragging with button 1, then pressing button 2 too.
        m.update(5, 7, 1, 13);
        assert_eq!(m.read().unwrap().fs, MOUSE_MOTION_WITH_BN1_DOWN);
        m.update(5, 7, 3, 14);
        assert_eq!(m.read().unwrap().fs, MOUSE_BN1_DOWN | MOUSE_BN2_DOWN);
        m.update(6, 7, 3, 15);
        assert_eq!(m.read().unwrap().fs, MOUSE_MOTION_WITH_BN1_DOWN | MOUSE_MOTION_WITH_BN2_DOWN);
        // Releasing everything: an event with no bits set.
        m.update(6, 7, 0, 16);
        assert_eq!(m.read(), Some(MouEvent { fs: 0, time: 16, row: 6, col: 7 }));
        assert!(m.is_empty());
        // Button 3 maps to its own bits.
        m.update(6, 7, 4, 17);
        assert_eq!(m.read().unwrap().fs, MOUSE_BN3_DOWN);
        m.update(6, 8, 4, 18);
        assert_eq!(m.read().unwrap().fs, MOUSE_MOTION_WITH_BN3_DOWN);
    }

    #[test]
    fn the_event_mask_filters_events_but_the_position_follows() {
        let mut m = MouManager::new();
        m.open(25, 80);
        assert_eq!(m.set_event_mask(0x80), Err(ERROR_MOUSE_INV_MASK));
        // Button 1 without motion only.
        m.set_event_mask(MOUSE_BN1_DOWN).unwrap();
        assert!(!m.update(1, 1, 0, 1), "motion is not reported");
        assert_eq!(m.position(), (1, 1));
        assert!(!m.update(1, 1, 2, 2), "button 2 is not reported");
        // Button 1 goes down: reported, with button 2's state too.
        assert!(m.update(1, 1, 3, 3));
        assert_eq!(m.read().unwrap().fs, MOUSE_BN1_DOWN | MOUSE_BN2_DOWN);
        assert!(!m.update(1, 2, 3, 4), "a drag is motion");
        assert!(!m.update(1, 2, 1, 5), "button 2 released");
        assert!(m.update(1, 2, 0, 6), "button 1 released");
        assert_eq!(m.read().unwrap().fs, 0);
        // Motion with button 1 only.
        m.set_event_mask(MOUSE_MOTION_WITH_BN1_DOWN).unwrap();
        assert!(!m.update(2, 2, 0, 7));
        assert!(!m.update(2, 2, 1, 8));
        assert!(m.update(3, 2, 1, 9));
        assert_eq!(m.read().unwrap().fs, MOUSE_MOTION_WITH_BN1_DOWN);
    }

    #[test]
    fn closed_mouse_tracks_the_pointer_without_queueing() {
        let mut m = MouManager::new();
        assert!(!m.update(2, 3, 1, 1));
        assert_eq!(m.position(), (2, 3));
        assert_eq!(m.buttons(), 1);
        m.open(25, 80);
        assert_eq!(m.buttons(), 1, "the held button is remembered");
        assert!(m.update(12, 40, 0, 2));
        assert_eq!(m.read().unwrap().fs, 0);
    }

    #[test]
    fn a_full_queue_drops_the_oldest_event() {
        let mut m = MouManager::new();
        m.open(100, 200);
        for i in 0..MOU_QUEUE_MAX as u16 + 5 {
            m.update(0, i + 1, 0, i as u32);
        }
        assert_eq!(m.len(), MOU_QUEUE_MAX);
        assert_eq!(m.read().unwrap().col, 6);
        m.flush();
        assert!(m.is_empty());
    }

    #[test]
    fn mickey_mode_reports_relative_motion() {
        let mut m = MouManager::new();
        m.open(25, 80);
        m.set_dev_status(MOUSE_MICKEYS).unwrap();
        m.update(13, 38, 0, 1); // one row down, two columns left of (12, 40)
        // 16 pels × 16 mickeys / 8 pels; 2 × 8 pels × 8 mickeys / 8 pels.
        assert_eq!(m.read(), Some(MouEvent { fs: MOUSE_MOTION, time: 1, row: 32, col: -16 }));
        m.update(13, 38, 1, 2);
        assert_eq!(m.read(), Some(MouEvent { fs: MOUSE_BN1_DOWN, time: 2, row: 0, col: 0 }));
    }

    #[test]
    fn device_status_scale_position_and_collision_are_validated() {
        let mut m = MouManager::new();
        m.open(25, 80);
        assert_eq!(m.set_dev_status(MOUSE_QUEUEBUSY), Err(ERROR_MOUSE_INV_PARMS));
        assert_eq!(m.set_dev_status(MOUSE_DISABLED), Ok(()));
        assert_eq!(m.dev_status(), MOUSE_DISABLED);
        assert_eq!(m.set_scale(0, 8), Err(ERROR_MOUSE_INV_PARMS));
        assert_eq!(m.set_scale(8, 0x8000), Err(ERROR_MOUSE_INV_PARMS));
        assert_eq!(m.set_scale(1, MOU_MAX_SCALE), Ok(()));
        assert_eq!(m.set_position(25, 0, 25, 80), Err(ERROR_MOUSE_INV_PARMS));
        assert_eq!(m.set_position(0, 80, 25, 80), Err(ERROR_MOUSE_INV_PARMS));
        assert_eq!(m.set_position(24, 79, 25, 80), Ok(()));
        let r = |row, col, row2, col2| CellRect { row, col, row2, col2 };
        assert_eq!(m.remove_pointer(r(5, 5, 4, 5), 25, 80), Err(ERROR_MOUSE_INV_PARMS));
        assert_eq!(m.remove_pointer(r(0, 0, 25, 0), 25, 80), Err(ERROR_MOUSE_INV_PARMS));
        // Drawing disabled: no text pointer, whatever the collision area.
        m.draw_pointer();
        assert_eq!(m.text_pointer(), None);
        m.set_dev_status(0).unwrap();
        assert!(m.text_pointer().is_some());
        // The pointer inside the collision area is not drawn; outside it is.
        m.remove_pointer(r(20, 70, 24, 79), 25, 80).unwrap();
        assert_eq!(m.text_pointer(), None);
        m.set_position(0, 0, 25, 80).unwrap();
        assert!(m.text_pointer().is_some());
    }

    #[test]
    fn the_text_pointer_inverts_the_attribute_by_default() {
        let p = TextPointer { row: 0, col: 0, and_mask: MOU_DEFAULT_AND, xor_mask: MOU_DEFAULT_XOR };
        assert_eq!(p.apply_attr(0x07), 0x70);
        assert_eq!(p.apply_attr(0x1E), 0x69);
        let p = TextPointer { and_mask: 0x00FF, xor_mask: 0x4F00, ..p };
        assert_eq!(p.apply_attr(0x07), 0x4F);
    }

    // ── The API ─────────────────────────────────────────────────────────────

    #[test]
    fn mou_open_needs_a_display_and_an_output_pointer() {
        let loader = Loader::new_mock();
        assert_eq!(call(&loader, 17, &[0, BUF]), ERROR_MOUSE_NO_DEVICE);
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        assert_eq!(call(&loader, 17, &[0, 0]), ERROR_MOUSE_INV_PARMS);
        assert!(!loader.shared.mou_mgr.lock().unwrap().is_open());
        assert_eq!(call(&loader, 17, &[0, BUF]), NO_ERROR);
        assert!(loader.shared.mou_mgr.lock().unwrap().is_open());
    }

    #[test]
    fn calls_with_a_bad_handle_fail() {
        let loader = Loader::new_mock();
        assert_eq!(call(&loader, 7, &[H]), ERROR_MOUSE_INV_HANDLE, "not open");
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        call(&loader, 17, &[0, BUF]);
        for (ord, args) in [(7, vec![H + 1]), (26, vec![9]), (19, vec![BUF, 0]), (20, vec![BUF, BUF + 0x10, 0])] {
            loader.guest_write::<u16>(BUF + 0x10, MOU_NOWAIT).unwrap();
            assert_eq!(call(&loader, ord, &args), ERROR_MOUSE_INV_HANDLE, "ordinal {ord}");
        }
        assert_eq!(call(&loader, 9, &[H]), NO_ERROR);
        assert_eq!(call(&loader, 9, &[H]), ERROR_MOUSE_INV_HANDLE, "closed");
    }

    #[test]
    fn read_event_que_without_wait_and_with_an_event() {
        let loader = opened();
        let (rc, ev) = read_event(&loader, MOU_NOWAIT);
        assert_eq!((rc, ev), (NO_ERROR, MouEvent::default()), "empty: a null record");
        mouse_button(&loader.shared, 12, 40, MouButton::B1, true);
        let (rc, ev) = read_event(&loader, MOU_WAIT);
        assert_eq!(rc, NO_ERROR);
        assert_eq!((ev.fs, ev.row, ev.col), (MOUSE_BN1_DOWN, 12, 40));
        assert_ne!(ev.time, 0);
        // A bad read type.
        assert_eq!(read_event(&loader, 2).0, ERROR_MOUSE_INV_PARMS);
        // A null event pointer.
        assert_eq!(call(&loader, 20, &[0, BUF + 0x100, H]), ERROR_MOUSE_INV_PARMS);
    }

    #[test]
    fn read_event_que_with_wait_blocks_until_an_event_arrives() {
        let loader = opened();
        let shared = loader.shared.clone();
        let feeder = std::thread::spawn(move || {
            // Wait until the reader blocks, then move the pointer.
            while shared.mou_mgr.lock().unwrap().dev_status() & MOUSE_BLOCKREAD == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            mouse_moved(&shared, 3, 9);
        });
        let (rc, ev) = read_event(&loader, MOU_WAIT);
        feeder.join().unwrap();
        assert_eq!((rc, ev.fs, ev.row, ev.col), (NO_ERROR, MOUSE_MOTION, 3, 9));
    }

    #[test]
    fn read_event_que_with_wait_ends_on_exit() {
        let loader = opened();
        loader.shared.exit_requested.store(true, Ordering::Relaxed);
        assert_eq!(read_event(&loader, MOU_WAIT).0, ERROR_MOUSE_NO_DEVICE);
    }

    #[test]
    fn queue_count_flush_and_masks_through_the_api() {
        let loader = opened();
        mouse_moved(&loader.shared, 1, 1);
        mouse_moved(&loader.shared, 1, 2);
        assert_eq!(call(&loader, 13, &[BUF, H]), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(2));
        assert_eq!(loader.guest_read::<u16>(BUF + 2), Some(MOU_QUEUE_MAX as u16));
        assert_eq!(call(&loader, 7, &[H]), NO_ERROR);
        call(&loader, 13, &[BUF, H]);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(0));
        // Event mask.
        assert_eq!(call(&loader, 15, &[BUF, H]), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(MOU_EVENT_BITS));
        loader.guest_write::<u16>(BUF, 0x0100).unwrap();
        assert_eq!(call(&loader, 16, &[BUF, H]), ERROR_MOUSE_INV_MASK);
        loader.guest_write::<u16>(BUF, MOUSE_BN1_DOWN).unwrap();
        assert_eq!(call(&loader, 16, &[BUF, H]), NO_ERROR);
        call(&loader, 15, &[BUF, H]);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(MOUSE_BN1_DOWN));
        assert_eq!(call(&loader, 16, &[0, H]), ERROR_MOUSE_INV_PARMS);
    }

    #[test]
    fn device_queries_and_settings_through_the_api() {
        let loader = opened();
        assert_eq!(call(&loader, 8, &[BUF, H]), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(MOU_NUM_BUTTONS));
        assert_eq!(call(&loader, 3, &[BUF, H]), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(MOU_MICKEYS_PER_CM));
        assert_eq!(call(&loader, 22, &[BUF, H]), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(0));
        loader.guest_write::<u16>(BUF, MOUSE_MICKEYS).unwrap();
        assert_eq!(call(&loader, 25, &[BUF, H]), NO_ERROR);
        call(&loader, 22, &[BUF, H]);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(MOUSE_MICKEYS));
        loader.guest_write::<u16>(BUF, MOUSE_FLUSH).unwrap();
        assert_eq!(call(&loader, 25, &[BUF, H]), ERROR_MOUSE_INV_PARMS);
        // Scale factors.
        assert_eq!(call(&loader, 6, &[BUF, H]), NO_ERROR);
        assert_eq!((loader.guest_read::<u16>(BUF), loader.guest_read::<u16>(BUF + 2)), (Some(16), Some(8)));
        loader.guest_write::<u16>(BUF, 4).unwrap();
        loader.guest_write::<u16>(BUF + 2, 2).unwrap();
        assert_eq!(call(&loader, 11, &[BUF, H]), NO_ERROR);
        assert_eq!(loader.shared.mou_mgr.lock().unwrap().scale(), (4, 2));
        loader.guest_write::<u16>(BUF, 0).unwrap();
        assert_eq!(call(&loader, 11, &[BUF, H]), ERROR_MOUSE_INV_PARMS);
        // Null output pointers.
        assert_eq!(call(&loader, 8, &[0, H]), ERROR_MOUSE_INV_PARMS);
        assert_eq!(call(&loader, 6, &[0, H]), ERROR_MOUSE_INV_PARMS);
        // MouSynch.
        assert_eq!(call(&loader, 23, &[0]), NO_ERROR);
    }

    #[test]
    fn pointer_position_and_collision_area_through_the_api() {
        let loader = opened();
        assert_eq!(call(&loader, 19, &[BUF, H]), NO_ERROR);
        assert_eq!((loader.guest_read::<u16>(BUF), loader.guest_read::<u16>(BUF + 2)), (Some(12), Some(40)));
        loader.guest_write::<u16>(BUF, 2).unwrap();
        loader.guest_write::<u16>(BUF + 2, 79).unwrap();
        assert_eq!(call(&loader, 21, &[BUF, H]), NO_ERROR);
        assert_eq!(loader.shared.mou_mgr.lock().unwrap().position(), (2, 79));
        loader.guest_write::<u16>(BUF + 2, 80).unwrap();
        assert_eq!(call(&loader, 21, &[BUF, H]), ERROR_MOUSE_INV_PARMS);
        assert_eq!(text_pointer(&loader.shared), None, "not drawn after MouOpen");
        assert_eq!(call(&loader, 26, &[H]), NO_ERROR);
        assert_eq!(text_pointer(&loader.shared).map(|p| (p.row, p.col)), Some((2, 79)));
        for (i, v) in [0u16, 70, 5, 79].iter().enumerate() {
            loader.guest_write::<u16>(BUF + 2 * i as u32, *v).unwrap();
        }
        assert_eq!(call(&loader, 18, &[BUF, H]), NO_ERROR);
        assert_eq!(text_pointer(&loader.shared), None, "inside the collision area");
        loader.guest_write::<u16>(BUF + 4, 25).unwrap();
        assert_eq!(call(&loader, 18, &[BUF, H]), ERROR_MOUSE_INV_PARMS);
    }

    #[test]
    fn pointer_shape_through_the_api() {
        let loader = opened();
        let def = BUF + 0x40;
        // Too small a buffer: the size needed comes back.
        loader.guest_write::<u16>(def, 2).unwrap();
        assert_eq!(call(&loader, 1, &[BUF, def, H]), ERROR_MOUSE_INV_PARMS);
        assert_eq!(loader.guest_read::<u16>(def), Some(MOU_TEXT_SHAPE_LEN));
        assert_eq!(call(&loader, 1, &[BUF, def, H]), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(MOU_DEFAULT_AND));
        assert_eq!(loader.guest_read::<u16>(BUF + 2), Some(MOU_DEFAULT_XOR));
        assert_eq!((loader.guest_read::<u16>(def + 2), loader.guest_read::<u16>(def + 4)), (Some(1), Some(1)));
        // A new shape.
        loader.guest_write::<u16>(BUF, 0x00FF).unwrap();
        loader.guest_write::<u16>(BUF + 2, 0x4F00).unwrap();
        assert_eq!(call(&loader, 2, &[BUF, def, H]), NO_ERROR);
        assert_eq!(loader.shared.mou_mgr.lock().unwrap().shape().0, (0x00FF, 0x4F00));
        // Text modes need a 1 × 1 image of 4 bytes.
        loader.guest_write::<u16>(def + 2, 2).unwrap();
        assert_eq!(call(&loader, 2, &[BUF, def, H]), ERROR_MOUSE_INV_PARMS);
        loader.guest_write::<u16>(def + 2, 1).unwrap();
        loader.guest_write::<u16>(def, 8).unwrap();
        assert_eq!(call(&loader, 2, &[BUF, def, H]), ERROR_MOUSE_INV_PARMS);
        assert_eq!(call(&loader, 2, &[0, def, H]), ERROR_MOUSE_INV_PARMS);
    }

    #[test]
    fn pascal_argument_bytes_match_the_prototypes() {
        let loader = Loader::new_mock();
        // MouReadEventQue(PMOUEVENTINFO, PUSHORT, HMOU), MouOpen(PSZ, PHMOU),
        // MouClose(HMOU), an unknown ordinal.
        assert_eq!(loader.moucalls_arg_bytes(20), 12);
        assert_eq!(loader.moucalls_arg_bytes(17), 8);
        assert_eq!(loader.moucalls_arg_bytes(9), 4);
        assert_eq!(loader.moucalls_arg_bytes(14), 0, "MouDeRegister(VOID)");
        assert_eq!(loader.moucalls_arg_bytes(29), 8);
        assert_eq!(loader.moucalls_arg_bytes(5), 0);
        assert_eq!(call(&loader, 5, &[]), ERROR_INVALID_FUNCTION);
    }

    #[test]
    fn the_hot_key_takes_buttons_only() {
        let loader = opened();
        const P: u32 = 0x3000;
        loader.guest_write::<u16>(P, 0x0005).unwrap();
        assert_eq!(call(&loader, 10, &[P, MOU_HANDLE as u32]), NO_ERROR, "buttons 1 and 3");
        loader.guest_write::<u16>(P, 0).unwrap();
        assert_eq!(call(&loader, 4, &[P, MOU_HANDLE as u32]), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(P), Some(5));
        loader.guest_write::<u16>(P, 0x0008).unwrap();
        assert_eq!(call(&loader, 10, &[P, MOU_HANDLE as u32]), ERROR_MOUSE_INV_PARMS, "no fourth button");
        assert_eq!(call(&loader, 4, &[P, 9]), ERROR_MOUSE_INV_HANDLE);
    }

    #[test]
    fn thresholds_are_kept_and_checked_for_size() {
        let loader = opened();
        const T: u32 = 0x3000;
        loader.guest_write::<u16>(T, THRESHOLD_SIZE).unwrap();
        assert_eq!(call(&loader, 29, &[T, 0]), NO_ERROR, "handle 0, as reserved");
        let read = |l: &Loader| (0..4).map(|i| l.guest_read::<u16>(T + 2 + 2 * i).unwrap()).collect::<Vec<_>>();
        assert_eq!(read(&loader), MOU_DEFAULT_THRESHOLD.to_vec());
        for (i, v) in [3u16, 2, 9, 5].iter().enumerate() {
            loader.guest_write::<u16>(T + 2 + 2 * i as u32, *v).unwrap();
        }
        assert_eq!(call(&loader, 30, &[T, MOU_HANDLE as u32]), NO_ERROR, "or the open handle");
        assert_eq!(loader.shared.mou_mgr.lock_or_recover().threshold(), [3, 2, 9, 5]);
        loader.guest_write::<u16>(T, 4).unwrap();
        assert_eq!(call(&loader, 29, &[T, 0]), ERROR_MOUSE_INV_PARMS, "too short");
        assert_eq!(call(&loader, 29, &[0, 0]), ERROR_MOUSE_INV_PARMS);
    }

    #[test]
    fn one_subsystem_registers_at_a_time() {
        let loader = Loader::new_mock();
        const MODULE: u32 = 0x3000;
        const ENTRY: u32 = 0x3100;
        loader.guest_write_bytes(MODULE, b"MOUSUB\0");
        loader.guest_write_bytes(ENTRY, b"MouRouter\0");
        assert_eq!(call(&loader, 14, &[]), ERROR_MOUSE_DEREGISTER, "nothing registered");
        assert_eq!(call(&loader, 24, &[MODULE, ENTRY, 0x0000_0003]), NO_ERROR);
        assert_eq!(call(&loader, 24, &[MODULE, ENTRY, 0x0000_0003]), ERROR_MOUSE_REGISTER, "one at a time");
        assert_eq!(call(&loader, 14, &[]), NO_ERROR);
        assert_eq!(call(&loader, 24, &[MODULE, ENTRY, 1 << 0x15]), ERROR_MOUSE_INVALID_MASK, "no function 15H");
        assert_eq!(call(&loader, 24, &[MODULE, ENTRY, 0]), ERROR_MOUSE_INVALID_MASK);
        loader.guest_write_bytes(MODULE, b"TOOLONGNAME\0");
        assert_eq!(call(&loader, 24, &[MODULE, ENTRY, 1]), ERROR_MOUSE_INVALID_ASCIIZ, "9 bytes at most");
        assert_eq!(call(&loader, 27, &[MODULE]), ERROR_MOUSE_SMG_ONLY, "MouInitReal");
    }
}
