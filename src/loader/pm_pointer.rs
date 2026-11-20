// SPDX-License-Identifier: GPL-3.0-or-later
//
// PM pointers: WinQuerySysPointer, WinSetPointer / WinQueryPointer,
// WinShowPointer, WinLoadPointer / WinDestroyPointer, and the RT_POINTER
// resource format.
//
// From IBM's PM Reference (EDM2 pages of the same names):
//
// * WinQuerySysPointer(hwndDesktop, iptr, fCopy): "TRUE: Create a copy of
//   the default system pointer and return its handle" (to be destroyed with
//   WinDestroyPointer); "FALSE: Return the handle of the current system
//   pointer". Errors PMERR_INVALID_HWND, PMERR_PARAMETER_OUT_OF_RANGE.
// * WinSetPointer(hwndDesktop, hptrNew): NULLHANDLE = "Remove pointer from
//   the screen". Errors PMERR_INVALID_HWND, PMERR_INVALID_HPTR.
// * WinQueryPointer(hwndDesktop): the pointer handle; NULLHANDLE on error.
// * WinShowPointer(hwndDesktop, fShow): TRUE decrements the pointer display
//   level ("not decremented to a negative value"), FALSE increments it; the
//   pointer is visible at level zero. "If a pointing device exists, the
//   initial setting of the pointer display level is zero". SV_POINTERLEVEL
//   reports the level.
// * WinLoadPointer(hwndDesktop, hmod, idPointer): "A new copy of the
//   pointer is created each time this function is called"; NULLHANDLE and
//   PMERR_RESOURCE_NOT_FOUND when the resource is missing.
// * WinDestroyPointer(hptr): "A pointer can only be destroyed by the thread
//   that created it"; "The system pointers and icons must not be
//   destroyed"; PMERR_INVALID_HPTR.
// * WM_MOUSEMOVE: "The default window procedure sets the pointer shape
//   using the WinSetPointer function" — the arrow, per the PM Guide ("If
//   you want the standard arrow pointer, pass this message on to
//   WinDefWindowProc").
//
// The pointer / icon format (OS/2 Warp 4 Toolkit <pmgpi.h>; Toolkit
// "Bitmap File Format"; PM Guide "Mouse Pointers and Icons"):
//
// * BITMAPFILEHEADER(2): USHORT usType ("PT" pointer, "IC" icon, "CP" colour
//   pointer, "CI" colour icon), ULONG cbSize, SHORT xHotspot, yHotspot,
//   ULONG offBits (from the start of the file), then a BITMAPINFOHEADER(2)
//   and its colour table.
// * The 1-bit bitmap is twice as high as the image: "The first bitmap
//   definition is the XOR mask … The second is the AND mask" — rows are
//   bottom-up, so the lower half is the XOR mask and the upper half the AND
//   mask. AND 0 / XOR 0 black, 0 / 1 white, 1 / 0 transparent (the
//   screen), 1 / 1 inverted screen.
// * Colour pointers / icons: a second BITMAPFILEHEADER(2) right after the
//   first's colour table describes the colour bitmap (same cx, real cy);
//   where AND is 0 the colour pel is shown.
// * A bitmap array ("BA"): BITMAPARRAYFILEHEADER(2) entries (usType, cbSize,
//   offNext — 0 for the last —, cxDisplay, cyDisplay) each followed by a
//   pointer's file header(s); "the preferred version is one that matches
//   the device size of the icon or pointer", else the first,
//   device-independent one.
//
// Host side: the effective pointer is sent to the GUI thread
// (`GUIMessage::SetPointer`), which shows it as the SDL cursor
// (`gui::pointer`). Inverted-screen pels cannot be shown by a colour
// cursor and are drawn black (what SDL shows for them where inversion is
// unsupported).

use std::collections::HashMap;
use std::sync::Arc;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_bitmap::{decode_rows, info_len, parse_header, BitmapFormat};
use super::ApiResult;
use crate::gui::pointer::{PointerImage, PointerShape, SystemPointer};
use crate::gui::GUIMessage;

/// File-header types (`BFT_*`, <pmgpi.h>).
const BFT_ICON: u16 = 0x4349;
const BFT_POINTER: u16 = 0x5450;
const BFT_COLORICON: u16 = 0x4943;
const BFT_COLORPOINTER: u16 = 0x5043;
const BFT_BITMAPARRAY: u16 = 0x4142;

/// Offsets in BITMAPFILEHEADER(2): hot spot, offBits, and the info header.
const FH_HOTSPOT_X: usize = 6;
const FH_HOTSPOT_Y: usize = 8;
const FH_OFF_BITS: usize = 10;
const FH_INFO: usize = 14;
/// BITMAPARRAYFILEHEADER(2): offNext and the embedded file header (the
/// display size between them is not used: entries are chosen by size).
const AH_OFF_NEXT: usize = 6;
const AH_FILE_HEADER: usize = 14;
/// At most this many bitmap-array entries are looked at.
const MAX_ARRAY_ENTRIES: usize = 64;

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off.checked_add(4)?)?.try_into().ok()?))
}

/// One decoded pointer of a resource.
#[derive(Debug)]
struct Candidate {
    image: PointerImage,
    color: bool,
}

/// The pel rows (bottom-up) of the bitmap whose file header is at `fh`,
/// with format `fmt`, `rows` rows; `None` if the data is not all there.
fn bitmap_rows(res: &[u8], fh: usize, fmt: &BitmapFormat, rows: u32) -> Option<Vec<Vec<Option<u32>>>> {
    let off = u32_at(res, fh + FH_OFF_BITS)? as usize;
    let len = fmt.data_len(rows)?;
    let data = res.get(off..off.checked_add(len)?)?;
    Some(decode_rows(fmt, data, rows))
}

/// Decode the pointer or icon whose (first) file header is at `fh`.
fn decode_entry(res: &[u8], fh: usize) -> Option<(PointerImage, bool)> {
    let color = match u16_at(res, fh)? {
        BFT_POINTER | BFT_ICON => false,
        BFT_COLORPOINTER | BFT_COLORICON => true,
        _ => return None,
    };
    let mut mask = parse_header(res.get(fh + FH_INFO..)?, false)?;
    let (w, h2) = (mask.cx, mask.cy);
    let h = h2 / 2;
    let valid = mask.bit_count == 1 && w > 0 && h > 0 && h2 % 2 == 0
        && w <= MAX_POINTER_DIM && h <= MAX_POINTER_DIM;
    if !valid { return None; }
    // Decode the masks to their bits, not to colours.
    mask.palette = vec![0, 1];
    let mask_rows = bitmap_rows(res, fh, &mask, h2)?;
    let colors = if color {
        let fh2 = fh + FH_INFO + info_len(mask.header_len, mask.bit_count);
        let cfmt = parse_header(res.get(fh2 + FH_INFO..)?, true)?;
        if (cfmt.cx, cfmt.cy) != (w, h) { return None; }
        Some(bitmap_rows(res, fh2, &cfmt, h)?)
    } else {
        None
    };
    let bit = |row: &Vec<Option<u32>>, x: usize| row.get(x).copied().flatten().unwrap_or(0) & 1;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    // Output rows top first: PM row h-1 down to 0.
    for y in (0..h as usize).rev() {
        let (xor_row, and_row) = (&mask_rows[y], &mask_rows[y + h as usize]);
        for x in 0..w as usize {
            let (and, xor) = (bit(and_row, x), bit(xor_row, x));
            let px: [u8; 4] = match (and, xor) {
                (1, 0) => [0, 0, 0, 0],
                // Inverted screen: black (a colour cursor cannot invert).
                (1, _) => [0, 0, 0, 0xFF],
                _ => {
                    let c = match &colors {
                        Some(rows) => rows[y].get(x).copied().flatten().unwrap_or(0),
                        None if xor == 1 => 0x00FF_FFFF,
                        None => 0,
                    };
                    [(c >> 16) as u8, (c >> 8) as u8, c as u8, 0xFF]
                }
            };
            rgba.extend(px);
        }
    }
    // The hot spot counts from the bottom-left pel; the image from the top.
    let hx = u16_at(res, fh + FH_HOTSPOT_X)? as i16 as i32;
    let hy = u16_at(res, fh + FH_HOTSPOT_Y)? as i16 as i32;
    let hot_x = hx.clamp(0, w as i32 - 1);
    let hot_y = (h as i32 - 1 - hy).clamp(0, h as i32 - 1);
    Some((PointerImage { width: w, height: h, hot_x, hot_y, rgba }, color))
}

/// Decode an RT_POINTER resource (a pointer or icon file, or a bitmap
/// array of them) into the image to show: from an array, the first entry
/// of the display's pointer size (`POINTER_SIZE`), a colour one before a
/// black-and-white one, else the first entry.
pub fn decode_pointer_resource(res: &[u8]) -> Option<PointerImage> {
    let mut candidates = Vec::new();
    if u16_at(res, 0)? == BFT_BITMAPARRAY {
        let mut at = 0usize;
        for _ in 0..MAX_ARRAY_ENTRIES {
            if u16_at(res, at) != Some(BFT_BITMAPARRAY) { break; }
            if let Some((image, color)) = decode_entry(res, at + AH_FILE_HEADER) {
                candidates.push(Candidate { image, color });
            }
            // Entries only go forward, so a loop cannot be followed; a
            // missing next entry ends the array.
            let Some(next) = u32_at(res, at + AH_OFF_NEXT) else { break };
            let next = next as usize;
            if next <= at { break; }
            at = next;
        }
    } else {
        let (image, color) = decode_entry(res, 0)?;
        candidates.push(Candidate { image, color });
    }
    let score = |c: &Candidate| {
        let device_size = c.image.width == POINTER_SIZE && c.image.height == POINTER_SIZE;
        (device_size, c.color)
    };
    let mut best: Option<Candidate> = None;
    for c in candidates {
        if best.as_ref().is_none_or(|b| score(&c) > score(b)) {
            best = Some(c);
        }
    }
    best.map(|b| b.image)
}

/// The host shape of system pointer `sptr`.
pub fn system_shape(sptr: u32) -> PointerShape {
    let sys = match sptr {
        SPTR_TEXT => SystemPointer::Text,
        SPTR_WAIT => SystemPointer::Wait,
        SPTR_SIZE | SPTR_MOVE => SystemPointer::Move,
        SPTR_SIZENWSE => SystemPointer::SizeNwse,
        SPTR_SIZENESW => SystemPointer::SizeNesw,
        SPTR_SIZEWE => SystemPointer::SizeWe,
        SPTR_SIZENS => SystemPointer::SizeNs,
        SPTR_ILLEGAL => SystemPointer::Illegal,
        SPTR_ICONINFORMATION | SPTR_ICONQUESTION | SPTR_ICONERROR | SPTR_ICONWARNING => {
            if let Some(img) = icon_image(sptr) {
                return PointerShape::Image(Arc::new(img));
            }
            SystemPointer::Arrow
        }
        // The arrow, and the icons and pen pointers Warpine has no
        // picture of.
        _ => SystemPointer::Arrow,
    };
    PointerShape::System(sys)
}

/// A message-box icon (`SPTR_ICON*`) as a pointer image, hot spot in the
/// middle.
fn icon_image(sptr: u32) -> Option<PointerImage> {
    let n = super::pm_msgbox::MSGBOX_ICON_SIZE as u32;
    let pels = super::pm_msgbox::icon_pels(sptr)?;
    let rgba = pels.iter().flat_map(|p| match p {
        Some(c) => [(c >> 16) as u8, (c >> 8) as u8, *c as u8, 0xFF],
        None => [0; 4],
    }).collect();
    Some(PointerImage { width: n, height: n, hot_x: n as i32 / 2, hot_y: n as i32 / 2, rgba })
}

/// Whether `iptr` is a system pointer index of `<pmwin.h>`.
pub fn valid_sptr(iptr: u32) -> bool {
    (SPTR_ARROW..=SPTR_ICONWARNING).contains(&iptr) || (SPTR_ILLEGAL..=SPTR_CPTR).contains(&iptr)
}

/// A pointer made by WinLoadPointer or copied by WinQuerySysPointer.
#[derive(Debug)]
struct Created {
    shape: PointerShape,
    /// Thread that created it: only it may destroy it.
    owner: u32,
}

/// The process's pointers and the one PM shows.
///
/// Lock order: taken before the window manager (whose GUI sender passes the
/// shape on), never while holding it.
#[derive(Debug)]
pub struct PointerManager {
    created: HashMap<u32, Created>,
    next: u32,
    /// The pointer set by WinSetPointer (0 = none).
    current: u32,
    /// The pointer display level (WinShowPointer); visible at 0.
    level: u32,
    /// The handle the host cursor shows (`None` = hidden).
    shown: Option<u32>,
}

impl Default for PointerManager {
    fn default() -> Self {
        let arrow = SYS_HPOINTER_BASE + SPTR_ARROW;
        PointerManager { created: HashMap::new(), next: FIRST_CREATED_HPOINTER,
                         current: arrow, level: 0, shown: Some(arrow) }
    }
}

impl PointerManager {
    /// The handle of system pointer `sptr` (shared, never destroyed).
    pub fn system_handle(sptr: u32) -> u32 { SYS_HPOINTER_BASE + sptr }

    /// The shape of `hptr`, if it is a pointer.
    pub fn shape(&self, hptr: u32) -> Option<PointerShape> {
        if let Some(c) = self.created.get(&hptr) {
            return Some(c.shape.clone());
        }
        let sptr = hptr.checked_sub(SYS_HPOINTER_BASE)?;
        valid_sptr(sptr).then(|| system_shape(sptr))
    }

    /// Register a new pointer of `shape` made by thread `owner`.
    pub fn create(&mut self, shape: PointerShape, owner: u32) -> u32 {
        let h = self.next;
        self.next += 1;
        self.created.insert(h, Created { shape, owner });
        h
    }

    /// Destroy `hptr` for thread `tid`: only a created pointer, by its own
    /// thread. The current pointer becomes the arrow.
    pub fn destroy(&mut self, hptr: u32, tid: u32) -> bool {
        if self.created.get(&hptr).is_none_or(|c| c.owner != tid) { return false; }
        self.created.remove(&hptr);
        if self.current == hptr {
            self.current = Self::system_handle(SPTR_ARROW);
        }
        true
    }

    /// WinSetPointer: `hptr` (0 hides the pointer); false for no pointer.
    pub fn set(&mut self, hptr: u32) -> bool {
        if hptr != 0 && self.shape(hptr).is_none() { return false; }
        self.current = hptr;
        true
    }

    pub fn current(&self) -> u32 { self.current }

    pub fn level(&self) -> u32 { self.level }

    /// WinShowPointer: TRUE lowers the display level (not below 0), FALSE
    /// raises it.
    pub fn show(&mut self, show: bool) {
        self.level = if show { self.level.saturating_sub(1) } else { self.level.saturating_add(1) };
    }

    /// The handle the host cursor should show (`None` = hidden).
    fn visible(&self) -> Option<u32> {
        (self.level == 0 && self.current != 0).then_some(self.current)
    }

    /// The host cursor's new shape, if it changed since the last call
    /// (`Some(None)` = hide it).
    pub fn take_update(&mut self) -> Option<Option<PointerShape>> {
        let visible = self.visible();
        if visible == self.shown { return None; }
        self.shown = visible;
        Some(visible.and_then(|h| self.shape(h)))
    }
}

/// The system values about the pointer: its size, display level, and
/// that a mouse is present.
pub fn pointer_sys_value(mgr: &PointerManager, index: i32) -> Option<i32> {
    match index {
        SV_CXPOINTER | SV_CYPOINTER => Some(POINTER_SIZE as i32),
        SV_POINTERLEVEL => Some(mgr.level() as i32),
        SV_MOUSEPRESENT => Some(1),
        _ => None,
    }
}

impl super::Loader {
    /// Pass a change of the shown pointer on to the GUI thread. Sent while
    /// the pointer lock is held, so updates arrive in order.
    fn update_host_pointer(&self, mgr: &mut PointerManager) {
        let Some(shape) = mgr.take_update() else { return };
        let wm = self.shared.window_mgr.lock_or_recover();
        if let Some(tx) = &wm.gui_tx {
            let _ = tx.send(GUIMessage::SetPointer(shape));
        }
    }

    /// Whether `hwnd` may name the desktop in a pointer call. NULLHANDLE is
    /// accepted too (unverified: programs pass it).
    fn pointer_desktop_ok(&self, tid: u32, hwnd: u32) -> bool {
        self.pm_check_hwnd(tid, hwnd, true)
    }

    /// WinQuerySysPointer(hwndDesktop, iptr, fCopy).
    pub(super) fn win_query_sys_pointer(&self, tid: u32, hwnd: u32, iptr: u32, copy: bool) -> ApiResult {
        if !self.pointer_desktop_ok(tid, hwnd) { return ApiResult::Normal(0); }
        if !valid_sptr(iptr) { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        let h = PointerManager::system_handle(iptr);
        if !copy { return ApiResult::Normal(h); }
        ApiResult::Normal(self.shared.pointers.lock_or_recover().create(system_shape(iptr), tid))
    }

    /// WinSetPointer(hwndDesktop, hptrNew).
    pub(super) fn win_set_pointer(&self, tid: u32, hwnd: u32, hptr: u32) -> ApiResult {
        if !self.pointer_desktop_ok(tid, hwnd) { return ApiResult::Normal(0); }
        let mut mgr = self.shared.pointers.lock_or_recover();
        if !mgr.set(hptr) {
            drop(mgr);
            return self.pm_fail(tid, PMERR_INVALID_HPTR);
        }
        self.update_host_pointer(&mut mgr);
        ApiResult::Normal(1)
    }

    /// WinQueryPointer(hwndDesktop).
    pub(super) fn win_query_pointer(&self, tid: u32, hwnd: u32) -> ApiResult {
        if !self.pointer_desktop_ok(tid, hwnd) { return ApiResult::Normal(0); }
        ApiResult::Normal(self.shared.pointers.lock_or_recover().current())
    }

    /// WinShowPointer(hwndDesktop, fShow).
    pub(super) fn win_show_pointer(&self, tid: u32, hwnd: u32, show: bool) -> ApiResult {
        if !self.pointer_desktop_ok(tid, hwnd) { return ApiResult::Normal(0); }
        let mut mgr = self.shared.pointers.lock_or_recover();
        mgr.show(show);
        self.update_host_pointer(&mut mgr);
        ApiResult::Normal(1)
    }

    /// WinLoadPointer(hwndDesktop, hmod, idPointer): a new pointer from the
    /// RT_POINTER resource of module `hmod` (0 = the executable).
    pub(super) fn win_load_pointer(&self, tid: u32, hwnd: u32, hmod: u32, id: u32) -> ApiResult {
        if !self.pointer_desktop_ok(tid, hwnd) { return ApiResult::Normal(0); }
        let Ok(id) = u16::try_from(id) else { return self.pm_fail(tid, PMERR_RESOURCE_NOT_FOUND) };
        let res = self.shared.resource_mgr.lock_or_recover().find(hmod, crate::lx::header::RT_POINTER, id);
        let image = res
            .and_then(|(addr, size)| self.guest_read_bytes(addr, size as usize))
            .and_then(|bytes| decode_pointer_resource(&bytes));
        // A resource that is not a pointer counts as missing (unverified).
        let Some(image) = image else { return self.pm_fail(tid, PMERR_RESOURCE_NOT_FOUND) };
        let h = self.shared.pointers.lock_or_recover().create(PointerShape::Image(Arc::new(image)), tid);
        ApiResult::Normal(h)
    }

    /// WinDestroyPointer(hptr).
    pub(super) fn win_destroy_pointer(&self, tid: u32, hptr: u32) -> ApiResult {
        let mut mgr = self.shared.pointers.lock_or_recover();
        if !mgr.destroy(hptr, tid) {
            drop(mgr);
            return self.pm_fail(tid, PMERR_INVALID_HPTR);
        }
        self.update_host_pointer(&mut mgr);
        ApiResult::Normal(1)
    }

    /// WinDefWindowProc's WM_MOUSEMOVE: show the arrow.
    pub(super) fn default_mouse_pointer(&self) {
        let mut mgr = self.shared.pointers.lock_or_recover();
        if mgr.set(PointerManager::system_handle(SPTR_ARROW)) {
            self.update_host_pointer(&mut mgr);
        }
    }

    /// WinQuerySysValue for the pointer values ([`pointer_sys_value`]).
    pub(super) fn pointer_sys_value(&self, index: i32) -> Option<i32> {
        pointer_sys_value(&self.shared.pointers.lock_or_recover(), index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::managers::EXE_RESOURCE_MODULE;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::Loader;
    use super::super::pm_msgbox::MSGBOX_ICON_SIZE;
    use crate::gui::{create_gui_channel, GUIReceiver};

    const ORD_WIN_DESTROY_POINTER: u32 = 727;
    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_LOAD_POINTER: u32 = 780;
    const ORD_WIN_QUERY_POINTER: u32 = 821;
    const ORD_WIN_QUERY_SYS_POINTER: u32 = 828;
    const ORD_WIN_QUERY_SYS_VALUE: u32 = 829;
    const ORD_WIN_SET_POINTER: u32 = 866;
    const ORD_WIN_SHOW_POINTER: u32 = 881;
    const ORD_WIN_DEF_WINDOW_PROC: u32 = 911;
    /// Guest address of resources written by the tests.
    const RES: u32 = 0x0030_0000;

    fn call(l: &Loader, tid: u32, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        for (i, &a) in args.iter().enumerate() {
            l.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
        match l.handle_pmwin_call(&mut vcpu, tid, ordinal) {
            ApiResult::Normal(v) => v,
            other => panic!("PMWIN ordinal {ordinal} returned {other:?}"),
        }
    }

    fn last_error(l: &Loader, tid: u32) -> u32 {
        call(l, tid, ORD_WIN_GET_LAST_ERROR, &[0]) & 0xFFFF
    }

    fn with_gui(l: &Loader) -> GUIReceiver {
        let (tx, rx) = create_gui_channel();
        l.shared.window_mgr.lock_or_recover().gui_tx = Some(tx);
        rx
    }

    fn pointer_messages(rx: &GUIReceiver) -> Vec<Option<PointerShape>> {
        rx.try_iter().filter_map(|m| match m {
            GUIMessage::SetPointer(s) => Some(s),
            _ => None,
        }).collect()
    }

    // ── A writer of the documented format ───────────────────────────────

    /// A BITMAPFILEHEADER of `ty` with a 12-byte BITMAPINFOHEADER.
    fn file_header(ty: u16, hot: (i16, i16), off_bits: u32, cx: u16, cy: u16, bits: u16) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend(ty.to_le_bytes());
        f.extend(26u32.to_le_bytes());
        f.extend(hot.0.to_le_bytes());
        f.extend(hot.1.to_le_bytes());
        f.extend(off_bits.to_le_bytes());
        f.extend(12u32.to_le_bytes());
        f.extend(cx.to_le_bytes());
        f.extend(cy.to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(bits.to_le_bytes());
        f
    }

    /// 1-bit rows (bottom-up) of width `w` from strings of '0' / '1'.
    fn mask_bits(rows: &[&str], w: usize) -> Vec<u8> {
        let rb = w.div_ceil(32) * 4;
        let mut out = Vec::new();
        for r in rows {
            let mut row = vec![0u8; rb];
            for (x, c) in r.bytes().enumerate() {
                if c == b'1' { row[x / 8] |= 0x80 >> (x % 8); }
            }
            out.extend(row);
        }
        out
    }

    /// A black-and-white 4×2 pointer (`ty` PT or IC) with hot spot `hot`:
    /// bottom row (XOR 1100, AND 0011): white, white, transparent,
    /// transparent; top row (XOR 0101, AND 0001): black, white, black,
    /// inverted.
    fn mono_pointer(ty: u16, hot: (i16, i16)) -> Vec<u8> {
        let mut f = file_header(ty, hot, 0, 4, 4, 1);
        f.extend([0, 0, 0, 0xFF, 0xFF, 0xFF]); // colour table: black, white
        let off = f.len() as u32;
        f[FH_OFF_BITS..FH_OFF_BITS + 4].copy_from_slice(&off.to_le_bytes());
        f.extend(mask_bits(&["1100", "0101", "0011", "0001"], 4));
        f
    }

    /// A colour 2×1 pointer: AND 01, XOR 00, colours red, green (24 bits).
    fn color_pointer() -> Vec<u8> {
        let mut f = file_header(BFT_COLORPOINTER, (1, 0), 0, 2, 2, 1);
        f.extend([0, 0, 0, 0xFF, 0xFF, 0xFF]);
        let second = f.len();
        f.extend(file_header(BFT_COLORPOINTER, (1, 0), 0, 2, 1, 24));
        let mask_off = f.len() as u32;
        f.extend(mask_bits(&["00", "01"], 2));
        let color_off = f.len() as u32;
        f.extend([0, 0, 0xFF, 0, 0xFF, 0, 0, 0]); // red, green (BGR), padding
        f[FH_OFF_BITS..FH_OFF_BITS + 4].copy_from_slice(&mask_off.to_le_bytes());
        f[second + FH_OFF_BITS..second + FH_OFF_BITS + 4].copy_from_slice(&color_off.to_le_bytes());
        f
    }

    /// `pointers` as a bitmap array; offBits inside each are relative to
    /// the pointer and are rebased here.
    fn array(pointers: &[Vec<u8>]) -> Vec<u8> {
        let mut f = Vec::new();
        for (i, p) in pointers.iter().enumerate() {
            let at = f.len();
            f.extend(BFT_BITMAPARRAY.to_le_bytes());
            f.extend(40u32.to_le_bytes());
            f.extend(0u32.to_le_bytes()); // offNext, patched below
            f.extend(0u16.to_le_bytes());
            f.extend(0u16.to_le_bytes());
            let base = f.len();
            let mut p = p.clone();
            // Rebase every offBits (one or two file headers).
            let mut fh = 0;
            loop {
                let off = u32_at(&p, fh + FH_OFF_BITS).unwrap() + base as u32;
                p[fh + FH_OFF_BITS..fh + FH_OFF_BITS + 4].copy_from_slice(&off.to_le_bytes());
                let color = u16_at(&p, fh) == Some(BFT_COLORPOINTER) && fh == 0;
                if !color { break; }
                fh = FH_INFO + 12 + 6;
            }
            f.extend(p);
            if i + 1 < pointers.len() {
                let next = f.len() as u32;
                f[at + AH_OFF_NEXT..at + AH_OFF_NEXT + 4].copy_from_slice(&next.to_le_bytes());
            }
        }
        f
    }

    fn px(img: &PointerImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.rgba[i..i + 4].try_into().unwrap()
    }

    const BLACK: [u8; 4] = [0, 0, 0, 0xFF];
    const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    #[test]
    fn black_and_white_pointer_combines_and_and_xor_masks() {
        let img = decode_pointer_resource(&mono_pointer(BFT_POINTER, (1, 0))).unwrap();
        assert_eq!((img.width, img.height), (4, 2));
        // Image rows are top first: the PM top row (y = 1) is row 0.
        assert_eq!([px(&img, 0, 0), px(&img, 1, 0), px(&img, 2, 0), px(&img, 3, 0)],
                   [BLACK, WHITE, BLACK, BLACK], "AND 1 / XOR 1 (inverted) is drawn black");
        assert_eq!([px(&img, 0, 1), px(&img, 1, 1), px(&img, 2, 1), px(&img, 3, 1)],
                   [WHITE, WHITE, CLEAR, CLEAR]);
        // Hot spot (1, 0) from the bottom-left is (1, 1) from the top-left.
        assert_eq!((img.hot_x, img.hot_y), (1, 1));
        assert!(decode_pointer_resource(&mono_pointer(BFT_ICON, (0, 0))).is_some(), "icons too");
    }

    #[test]
    fn color_pointer_shows_the_color_bitmap_where_and_is_zero() {
        let img = decode_pointer_resource(&color_pointer()).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(px(&img, 0, 0), [0xFF, 0, 0, 0xFF], "red");
        assert_eq!(px(&img, 1, 0), CLEAR, "AND 1 / XOR 0");
        assert_eq!((img.hot_x, img.hot_y), (1, 0));
    }

    #[test]
    fn bitmap_array_prefers_the_device_size_and_colour() {
        // Neither is 32×32: the colour one wins over the black-and-white.
        let img = decode_pointer_resource(&array(&[mono_pointer(BFT_POINTER, (0, 0)), color_pointer()])).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        // A single entry is used whatever it is.
        let img = decode_pointer_resource(&array(&[mono_pointer(BFT_POINTER, (0, 0))])).unwrap();
        assert_eq!((img.width, img.height), (4, 2));
    }

    #[test]
    fn bitmap_array_takes_the_32_pel_pointer() {
        let big = {
            let mut f = file_header(BFT_POINTER, (0, 0), 0, 32, 64, 1);
            f.extend([0, 0, 0, 0xFF, 0xFF, 0xFF]);
            let off = f.len() as u32;
            f[FH_OFF_BITS..FH_OFF_BITS + 4].copy_from_slice(&off.to_le_bytes());
            f.extend(vec![0u8; 4 * 64]);
            f
        };
        let img = decode_pointer_resource(&array(&[color_pointer(), big])).unwrap();
        assert_eq!((img.width, img.height), (32, 32));
    }

    #[test]
    fn malformed_pointers_are_rejected_without_panicking() {
        let good = mono_pointer(BFT_POINTER, (0, 0));
        // Every truncation fails cleanly.
        for n in 0..good.len() {
            assert!(decode_pointer_resource(&good[..n]).is_none(), "truncated to {n}");
        }
        // Not a pointer type.
        let mut bm = good.clone();
        bm[0..2].copy_from_slice(&0x4D42u16.to_le_bytes());
        assert!(decode_pointer_resource(&bm).is_none());
        // Odd mask height, 4-bit masks, too large.
        let odd = file_header(BFT_POINTER, (0, 0), 32, 4, 3, 1);
        assert!(decode_pointer_resource(&odd).is_none());
        let four = file_header(BFT_POINTER, (0, 0), 32, 4, 4, 4);
        assert!(decode_pointer_resource(&four).is_none());
        let huge = file_header(BFT_POINTER, (0, 0), 32, 4096, 8192, 1);
        assert!(decode_pointer_resource(&huge).is_none());
        // offBits past the end.
        let mut far = good.clone();
        far[FH_OFF_BITS..FH_OFF_BITS + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_pointer_resource(&far).is_none());
        // A colour pointer whose colour bitmap has another size.
        let mut c = color_pointer();
        let second = FH_INFO + 12 + 6;
        c[second + FH_INFO + 4..second + FH_INFO + 6].copy_from_slice(&3u16.to_le_bytes());
        assert!(decode_pointer_resource(&c).is_none());
        // A bitmap array whose last entry points back to itself or to the
        // first entry ends there.
        let mut looped = array(&[good.clone(), good.clone()]);
        let second_at = u32_at(&looped, AH_OFF_NEXT).unwrap() as usize;
        for back in [second_at as u32, 0] {
            looped[second_at + AH_OFF_NEXT..second_at + AH_OFF_NEXT + 4].copy_from_slice(&back.to_le_bytes());
            assert!(decode_pointer_resource(&looped).is_some());
        }
        // offNext past the end, or an entry that is not a pointer.
        let mut dangling = array(std::slice::from_ref(&good));
        dangling[AH_OFF_NEXT..AH_OFF_NEXT + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_pointer_resource(&dangling).is_some(), "the entries read so far");
        let mut junk = array(&[good]);
        junk[AH_FILE_HEADER] = b'X';
        assert!(decode_pointer_resource(&junk).is_none());
    }

    /// The files of samples/pm_pointer_test (make_ptr.py): the bitmap
    /// array yields its 32×32 colour pointer, red inside a green border,
    /// hot spot at the bottom-left; the cross is black and white.
    #[test]
    fn sample_pointer_files_decode() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/pm_pointer_test");
        let (Ok(array), Ok(cross)) = (std::fs::read(dir.join("array.ptr")), std::fs::read(dir.join("cross.ptr"))) else {
            eprintln!("skipped: run make_ptr.py in {}", dir.display());
            return;
        };
        let img = decode_pointer_resource(&array).unwrap();
        assert_eq!((img.width, img.height, img.hot_x, img.hot_y), (32, 32, 0, 31));
        assert_eq!(px(&img, 0, 0), [0, 0xFF, 0, 0xFF], "green border");
        assert_eq!(px(&img, 5, 5), [0xFF, 0, 0, 0xFF], "red inside");
        let img = decode_pointer_resource(&cross).unwrap();
        assert_eq!((img.width, img.height, img.hot_x, img.hot_y), (32, 32, 16, 15));
        assert_eq!(px(&img, 16, 15), WHITE, "the middle line");
        assert_eq!(px(&img, 15, 5), BLACK, "its outline");
        assert_eq!(px(&img, 0, 0), CLEAR);
    }

    #[test]
    fn hot_spot_is_clamped_into_the_image() {
        let img = decode_pointer_resource(&mono_pointer(BFT_POINTER, (-5, 100))).unwrap();
        assert_eq!((img.hot_x, img.hot_y), (0, 0));
    }

    #[test]
    fn system_pointers_have_stable_handles() {
        let l = Loader::new_mock();
        let arrow = call(&l, 1, ORD_WIN_QUERY_SYS_POINTER, &[HWND_DESKTOP, SPTR_ARROW, 0]);
        assert_ne!(arrow, 0);
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_SYS_POINTER, &[HWND_DESKTOP, SPTR_ARROW, 0]), arrow);
        let text = call(&l, 1, ORD_WIN_QUERY_SYS_POINTER, &[HWND_DESKTOP, SPTR_TEXT, 0]);
        assert!(text != 0 && text != arrow);
        for bad in [0, 15, 16, 17, SPTR_CPTR + 1] {
            assert_eq!(call(&l, 1, ORD_WIN_QUERY_SYS_POINTER, &[HWND_DESKTOP, bad, 0]), 0, "iptr {bad}");
            assert_eq!(last_error(&l, 1), PMERR_PARAMETER_OUT_OF_RANGE);
        }
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_SYS_POINTER, &[0xDEAD, SPTR_ARROW, 0]), 0);
        assert_eq!(last_error(&l, 1), PMERR_INVALID_HWND);
        // System pointers cannot be destroyed.
        assert_eq!(call(&l, 1, ORD_WIN_DESTROY_POINTER, &[arrow]), 0);
        assert_eq!(last_error(&l, 1), PMERR_INVALID_HPTR);
        // A copy is a new pointer of the same shape, destroyed once.
        let copy = call(&l, 1, ORD_WIN_QUERY_SYS_POINTER, &[HWND_DESKTOP, SPTR_WAIT, 1]);
        assert!(copy != 0 && copy != PointerManager::system_handle(SPTR_WAIT));
        assert_eq!(l.shared.pointers.lock_or_recover().shape(copy),
                   Some(PointerShape::System(SystemPointer::Wait)));
        assert_eq!(call(&l, 2, ORD_WIN_DESTROY_POINTER, &[copy]), 0, "another thread's pointer");
        assert_eq!(call(&l, 1, ORD_WIN_DESTROY_POINTER, &[copy]), 1);
        assert_eq!(call(&l, 1, ORD_WIN_DESTROY_POINTER, &[copy]), 0);
    }

    #[test]
    fn set_pointer_is_queried_back_and_reaches_the_host_cursor() {
        let l = Loader::new_mock();
        let rx = with_gui(&l);
        let arrow = PointerManager::system_handle(SPTR_ARROW);
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_POINTER, &[HWND_DESKTOP]), arrow, "the arrow at first");
        let wait = call(&l, 1, ORD_WIN_QUERY_SYS_POINTER, &[HWND_DESKTOP, SPTR_WAIT, 0]);
        assert_eq!(call(&l, 1, ORD_WIN_SET_POINTER, &[HWND_DESKTOP, wait]), 1);
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_POINTER, &[HWND_DESKTOP]), wait);
        // Setting it again changes nothing on the host.
        assert_eq!(call(&l, 1, ORD_WIN_SET_POINTER, &[HWND_DESKTOP, wait]), 1);
        assert_eq!(pointer_messages(&rx), vec![Some(PointerShape::System(SystemPointer::Wait))]);
        // Bad handles fail and keep the pointer.
        assert_eq!(call(&l, 1, ORD_WIN_SET_POINTER, &[HWND_DESKTOP, 0x1234]), 0);
        assert_eq!(last_error(&l, 1), PMERR_INVALID_HPTR);
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_POINTER, &[HWND_DESKTOP]), wait);
        // NULLHANDLE removes the pointer from the screen.
        assert_eq!(call(&l, 1, ORD_WIN_SET_POINTER, &[HWND_DESKTOP, 0]), 1);
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_POINTER, &[HWND_DESKTOP]), 0);
        assert_eq!(pointer_messages(&rx), vec![None]);
        // An icon pointer shows its picture.
        let info = call(&l, 1, ORD_WIN_QUERY_SYS_POINTER, &[HWND_DESKTOP, SPTR_ICONINFORMATION, 0]);
        call(&l, 1, ORD_WIN_SET_POINTER, &[HWND_DESKTOP, info]);
        let msgs = pointer_messages(&rx);
        assert!(matches!(msgs.as_slice(), [Some(PointerShape::Image(img))]
                         if img.width == MSGBOX_ICON_SIZE as u32), "{msgs:?}");
    }

    #[test]
    fn show_pointer_counts_the_display_level() {
        let l = Loader::new_mock();
        let rx = with_gui(&l);
        let level = || call(&l, 1, ORD_WIN_QUERY_SYS_VALUE, &[HWND_DESKTOP, SV_POINTERLEVEL as u32]);
        assert_eq!(level(), 0);
        assert_eq!(call(&l, 1, ORD_WIN_SHOW_POINTER, &[HWND_DESKTOP, 0]), 1);
        assert_eq!(call(&l, 1, ORD_WIN_SHOW_POINTER, &[HWND_DESKTOP, 0]), 1);
        assert_eq!(level(), 2);
        assert_eq!(pointer_messages(&rx), vec![None], "hidden once");
        call(&l, 1, ORD_WIN_SHOW_POINTER, &[HWND_DESKTOP, 1]);
        assert_eq!(pointer_messages(&rx), vec![], "still hidden at level 1");
        call(&l, 1, ORD_WIN_SHOW_POINTER, &[HWND_DESKTOP, 1]);
        assert_eq!(pointer_messages(&rx), vec![Some(PointerShape::System(SystemPointer::Arrow))]);
        call(&l, 1, ORD_WIN_SHOW_POINTER, &[HWND_DESKTOP, 1]);
        assert_eq!(level(), 0, "not below zero");
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_SYS_VALUE, &[HWND_DESKTOP, SV_CXPOINTER as u32]), POINTER_SIZE);
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_SYS_VALUE, &[HWND_DESKTOP, SV_MOUSEPRESENT as u32]), 1);
    }

    #[test]
    fn load_pointer_from_a_resource_and_destroy_it() {
        let l = Loader::new_mock();
        let rx = with_gui(&l);
        let data = mono_pointer(BFT_POINTER, (1, 0));
        l.guest_write_bytes(RES, &data).unwrap();
        l.shared.resource_mgr.lock_or_recover()
            .add(EXE_RESOURCE_MODULE, crate::lx::header::RT_POINTER, 5, RES, data.len() as u32);
        let a = call(&l, 1, ORD_WIN_LOAD_POINTER, &[HWND_DESKTOP, 0, 5]);
        let b = call(&l, 1, ORD_WIN_LOAD_POINTER, &[HWND_DESKTOP, 0, 5]);
        assert!(a != 0 && b != 0 && a != b, "a new copy each time");
        assert_eq!(call(&l, 1, ORD_WIN_LOAD_POINTER, &[HWND_DESKTOP, 0, 6]), 0);
        assert_eq!(last_error(&l, 1), PMERR_RESOURCE_NOT_FOUND);
        assert_eq!(call(&l, 1, ORD_WIN_LOAD_POINTER, &[HWND_DESKTOP, 0, 0x1_0005]), 0);
        assert_eq!(call(&l, 1, ORD_WIN_LOAD_POINTER, &[HWND_DESKTOP, 7, 5]), 0, "another module");
        assert_eq!(call(&l, 1, ORD_WIN_SET_POINTER, &[HWND_DESKTOP, a]), 1);
        let msgs = pointer_messages(&rx);
        assert!(matches!(msgs.as_slice(), [Some(PointerShape::Image(img))]
                         if (img.width, img.height, img.hot_x, img.hot_y) == (4, 2, 1, 1)), "{msgs:?}");
        // Destroying the current pointer shows the arrow again.
        assert_eq!(call(&l, 1, ORD_WIN_DESTROY_POINTER, &[a]), 1);
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_POINTER, &[HWND_DESKTOP]), PointerManager::system_handle(SPTR_ARROW));
        assert_eq!(pointer_messages(&rx), vec![Some(PointerShape::System(SystemPointer::Arrow))]);
        assert_eq!(call(&l, 1, ORD_WIN_SET_POINTER, &[HWND_DESKTOP, a]), 0, "destroyed");
        assert_eq!(call(&l, 1, ORD_WIN_DESTROY_POINTER, &[b]), 1);
    }

    #[test]
    fn default_window_procedure_sets_the_arrow_on_mouse_moves() {
        let l = Loader::new_mock();
        let rx = with_gui(&l);
        let wait = PointerManager::system_handle(SPTR_WAIT);
        call(&l, 1, ORD_WIN_SET_POINTER, &[HWND_DESKTOP, wait]);
        let _ = pointer_messages(&rx);
        assert_eq!(call(&l, 1, ORD_WIN_DEF_WINDOW_PROC, &[0, WM_MOUSEMOVE, 0, 0]), 0);
        assert_eq!(call(&l, 1, ORD_WIN_QUERY_POINTER, &[HWND_DESKTOP]), PointerManager::system_handle(SPTR_ARROW));
        assert_eq!(pointer_messages(&rx), vec![Some(PointerShape::System(SystemPointer::Arrow))]);
    }

    #[test]
    fn every_system_pointer_has_a_shape() {
        for sptr in 0..=SPTR_CPTR + 1 {
            let shape = PointerManager::default().shape(PointerManager::system_handle(sptr));
            assert_eq!(shape.is_some(), valid_sptr(sptr), "SPTR {sptr}");
        }
        assert_eq!(system_shape(SPTR_SIZEWE), PointerShape::System(SystemPointer::SizeWe));
        assert_eq!(system_shape(SPTR_ILLEGAL), PointerShape::System(SystemPointer::Illegal));
        assert_eq!(system_shape(SPTR_FILE), PointerShape::System(SystemPointer::Arrow));
    }
}
