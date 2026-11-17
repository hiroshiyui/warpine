// SPDX-License-Identifier: GPL-3.0-only

//! Window coordinates and lookups: `WinMapWindowPoints`,
//! `WinWindowFromPoint`, `WinQueryDesktopWindow`, `WinQueryAnchorBlock`,
//! `WinIsChild`, `WinQueryWindowProcess`, `WinMultWindowFromIDs`,
//! `WinEnumDlgItem`, `WinQueryClassName` and `WinQueryClassInfo`.
//!
//! From the PM Reference:
//! - `WinMapWindowPoints(hwndFrom, hwndTo, aptl, cwpt)`: `cwpt` points, from
//!   window (or, for `HWND_DESKTOP`, screen) coordinates into those of
//!   `hwndTo`; a `RECTL` is mapped as two points (`cwpt` 2).
//! - `WinWindowFromPoint(hwndParent, pptl, fChildren)`: the point is relative
//!   to `hwndParent` (`HWND_DESKTOP`: the screen, "all main windows are
//!   tested"); `fChildren` TRUE tests all descendants, FALSE the immediate
//!   children; NULLHANDLE when the point is outside `hwndParent`,
//!   `hwndParent` when it is in none of its children.
//! - `WinQueryDesktopWindow(hab, hdc)`: NULLHANDLE `hdc` is the screen;
//!   "Only the screen device supports windowing" (`PMERR_INV_HDC`). Warpine
//!   returns `HWND_DESKTOP`, which is also the parent it reports for main
//!   windows ("Many of the calls that require a desktop-window handle accept
//!   HWND_DESKTOP instead").
//! - `WinIsChild(hwnd, hwndParent)`: TRUE for a descendant "or is equal to
//!   it"; FALSE for an object window "even if hwndParent is specified as the
//!   desktop".
//! - `WinQueryWindowProcess`: the process and thread "that created the window".
//! - `WinMultWindowFromIDs(hwndParent, ahwnd, idFirst, idLast)`: element
//!   `id - idFirst` is the child with that id or NULLHANDLE; returns the
//!   number of windows found; ids are 0 .. 0xFFFF.
//! - `WinEnumDlgItem(hwndDlg, hwnd, code)`: `EDI_*`; the result "is always an
//!   immediate child of hwndDlg, even if hwnd is not"; `hwnd` may be
//!   NULLHANDLE for `EDI_FIRSTTABITEM` / `EDI_LASTTABITEM`; "If an invalid
//!   handle is passed, the first child window of the dialog is returned";
//!   the next / previous codes wrap around (within the group for the group
//!   codes).
//! - `WinQueryClassName(hwnd, cchMax, pch)`: at most `cchMax - 1` characters
//!   and a null; returns the length without the null; preregistered classes
//!   are named "#nnnnn" after the value of their `WC_*` constant.
//! - `WinQueryClassInfo(hab, pszClassName, pClassInfo)`: an application class
//!   or a preregistered one ("#nnnnn", or the `WC_*` constant itself);
//!   `PMERR_INVALID_INTEGER_ATOM`, `PMERR_INVALID_ATOM_NAME`,
//!   `PMERR_ATOM_NAME_NOT_FOUND`.
//!
//! Where Warpine keeps window positions: a main window (and every dialog)
//! has desktop coordinates; a standard frame's client lies inside the
//! frame's left border, at the frame's bottom edge (its `x`, `y` are 0);
//! other windows are relative to their parent (dialog controls to the
//! dialog, border included).
//!
//! Unverified (no source): hidden windows are skipped by
//! `WinWindowFromPoint` (except a frame's client, which Warpine shows with
//! its frame), disabled ones are not; with `HWND_DESKTOP` it tests
//! the frames and dialogs on the desktop (the windows Warpine shows) and a
//! point off the screen gives NULLHANDLE; `WinMapWindowPoints` with a
//! negative count or an unreadable array fails with
//! `PMERR_PARAMETER_OUT_OF_RANGE`; `WinEnumDlgItem` tab items are visible,
//! enabled `WS_TABSTOP` items and group items visible, enabled ones (the item
//! of `hwnd` always counts), NULLHANDLE when there is no tab item, an
//! unknown code is `PMERR_PARAMETER_OUT_OF_RANGE`; `WinQueryWindowProcess`
//! accepts NULL out pointers, reports the main thread for the desktop and
//! for windows whose queue is gone; `WinQueryClassName` fails with
//! `PMERR_INVALID_HWND` for the desktop and object pseudo windows and with
//! `PMERR_INVALID_STRING_PARM` for `cchMax` ≤ 0 or an unwritable buffer;
//! `WinQueryClassInfo` reports the preregistered classes with `CS_PUBLIC`
//! (plus `CS_FRAME` for the frame class), no window words, and
//! `WinDefWindowProc` as their window procedure (Warpine runs the built-in
//! controls in the host, so there is no guest procedure to subclass);
//! application class names are case-sensitive.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::{group_range, step_from, DeviceContext, WindowManager};
use super::ApiResult;

/// PMWIN ordinal of `WinDefWindowProc`: the window procedure
/// `WinQueryClassInfo` reports for the preregistered classes.
const ORD_WIN_DEF_WINDOW_PROC: u32 = 911;

/// Size of a `CLASSINFO` (`flClassStyle`, `pfnWindowProc`, `cbWindowData`).
const CLASSINFO_SIZE: usize = 12;

/// The preregistered classes Warpine knows: atom (`WC_*` low word) and the
/// class name Warpine gives their windows (`resolve_class_atom`).
const PUBLIC_CLASSES: [(u32, &str); 13] = [
    (WC_FRAME_ATOM & 0xFFFF, "WC_FRAME"),
    (WC_COMBOBOX_ATOM & 0xFFFF, "WC_COMBOBOX"),
    (WC_BUTTON_ATOM & 0xFFFF, "WC_BUTTON"),
    (WC_MENU_ATOM & 0xFFFF, "WC_MENU"),
    (WC_STATIC_ATOM & 0xFFFF, "WC_STATIC"),
    (WC_ENTRYFIELD_ATOM & 0xFFFF, "WC_ENTRYFIELD"),
    (WC_LISTBOX_ATOM & 0xFFFF, "WC_LISTBOX"),
    (WC_SCROLLBAR_ATOM & 0xFFFF, "WC_SCROLLBAR"),
    (WC_TITLEBAR_ATOM & 0xFFFF, "WC_TITLEBAR"),
    (WC_MLE_ATOM & 0xFFFF, "WC_MLE"),
    (WC_SPINBUTTON_ATOM & 0xFFFF, "WC_SPINBUTTON"),
    (WC_CONTAINER_ATOM & 0xFFFF, "WC_CONTAINER"),
    (WC_NOTEBOOK_ATOM & 0xFFFF, "WC_NOTEBOOK"),
];

/// Whether `hwnd` stands for the desktop as a parent.
fn is_desktop(hwnd: u32) -> bool {
    hwnd == 0 || hwnd == HWND_DESKTOP
}

/// The atom of a preregistered class name "#nnnnn" (one to five digits).
fn class_atom_of_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix('#')?;
    if digits.is_empty() || digits.len() > 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The atom of the preregistered class Warpine names `class_name`:
/// "WC_BUTTON" etc., the "WC_ATOM_…" names of other atoms, the internal
/// "#Menu" (a menu) and "#Dialog" (a frame).
fn class_atom_of_window_class(class_name: &str) -> Option<u32> {
    if let Some(&(atom, _)) = PUBLIC_CLASSES.iter().find(|(_, n)| *n == class_name) {
        return Some(atom);
    }
    match class_name {
        "#Menu" => return Some(WC_MENU_ATOM & 0xFFFF),
        WindowManager::DIALOG_CLASS => return Some(WC_FRAME_ATOM & 0xFFFF),
        _ => {}
    }
    let rest = class_name.strip_prefix("WC_ATOM_")?;
    match rest.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => rest.parse().ok(),
    }
}

impl WindowManager {
    /// Where `hwnd`'s bottom-left corner is in its parent's coordinates: its
    /// `x`, `y`, plus the frame border for a standard frame's client.
    fn origin_in_parent(&self, hwnd: u32) -> Option<(i32, i32)> {
        let w = self.get_window(hwnd)?;
        let border = match self.get_window(w.parent) {
            Some(p) if self.frame_to_client.get(&w.parent) == Some(&hwnd) => frame_border_width(p.frame_flags),
            _ => 0,
        };
        Some((w.x.saturating_add(border), w.y))
    }

    /// Desktop coordinates of `hwnd`'s bottom-left corner: (0, 0) for the
    /// desktop and object pseudo windows, `None` for an unknown window. A
    /// main window or a dialog is placed on the desktop; every other
    /// window relative to its parent.
    pub fn window_origin(&self, hwnd: u32) -> Option<(i32, i32)> {
        if is_desktop(hwnd) || hwnd == HWND_OBJECT { return Some((0, 0)); }
        let (mut x, mut y) = (0i32, 0i32);
        let mut h = hwnd;
        for _ in 0..=self.windows().count() {
            let (dx, dy) = self.origin_in_parent(h)?;
            x = x.saturating_add(dx);
            y = y.saturating_add(dy);
            let parent = self.get_window(h)?.parent;
            if self.is_dialog(h) || !self.is_window(parent) {
                return Some((x, y));
            }
            h = parent;
        }
        None
    }

    /// The pseudo parent at the top of `hwnd`'s parent chain: 0 /
    /// `HWND_DESKTOP` for windows on the desktop, `HWND_OBJECT` for object
    /// windows.
    fn root_of(&self, hwnd: u32) -> u32 {
        let mut h = hwnd;
        for _ in 0..=self.windows().count() {
            match self.get_window(h) {
                Some(w) => h = w.parent,
                None => return h,
            }
        }
        0
    }

    /// The windows directly below `parent`, bottom to top. The desktop's
    /// are the main windows: those in the Z-order in its order, then the
    /// others by handle.
    pub fn child_windows(&self, parent: u32) -> Vec<u32> {
        if !is_desktop(parent) {
            return self.get_window(parent).map(|w| w.children.clone()).unwrap_or_default();
        }
        let mut others: Vec<u32> = self.windows()
            .filter(|w| is_desktop(w.parent) && !self.z_order.contains(&w.handle))
            .map(|w| w.handle).collect();
        others.sort_unstable();
        others.into_iter().chain(self.z_order.iter().copied().filter(|&h| self.is_window(h))).collect()
    }

    /// `WinWindowFromPoint`: the window under desktop point `(x, y)` below
    /// `parent` — its topmost visible child (any depth with `deep`), else
    /// `parent`; 0 when the point is not in `parent`.
    pub fn window_from_point(&self, parent: u32, x: i32, y: i32, deep: bool) -> u32 {
        let contains = |h: u32| -> bool {
            let Some(w) = self.get_window(h) else { return false };
            let Some((ox, oy)) = self.window_origin(h) else { return false };
            x >= ox && y >= oy && (x as i64) < ox as i64 + w.cx as i64 && (y as i64) < oy as i64 + w.cy as i64
        };
        let inside = if is_desktop(parent) {
            let (dw, dh) = desktop_size();
            (0..dw as i32).contains(&x) && (0..dh as i32).contains(&y)
        } else {
            contains(parent)
        };
        if !inside { return 0; }
        let mut found = parent;
        for _ in 0..=self.windows().count() {
            let children = if is_desktop(found) {
                // The main windows shown on the desktop.
                self.z_order.clone()
            } else {
                self.child_windows(found)
            };
            // A standard frame's client is drawn whenever its frame is,
            // whatever its own WS_VISIBLE (programs often pass no client style).
            let hit = children.into_iter().rev().find(|&h| {
                self.get_window(h).is_some_and(|w| w.visible || self.frame_to_client.get(&w.parent) == Some(&h))
                    && contains(h)
            });
            match hit {
                Some(h) => found = h,
                None => break,
            }
            if !deep { break; }
        }
        found
    }

    /// The immediate child of `dlg` that is `hwnd` or contains it.
    fn dlg_item_of(&self, dlg: u32, hwnd: u32) -> Option<u32> {
        let mut h = hwnd;
        for _ in 0..=self.windows().count() {
            let parent = self.get_window(h)?.parent;
            if parent == dlg { return Some(h); }
            h = parent;
        }
        None
    }

    /// `WinEnumDlgItem`: the item of `dlg` that `code` (`EDI_*`) names
    /// relative to the item holding `hwnd`; 0 when there is none.
    pub fn enum_dlg_item(&self, dlg: u32, hwnd: u32, code: u32) -> u32 {
        let items = self.child_windows(dlg);
        let Some(&first) = items.first() else { return 0 };
        let enabled = |h: u32| self.get_window(h)
            .is_some_and(|w| w.visible && w.style & WS_DISABLED == 0);
        let tab = |h: u32| enabled(h) && self.get_window(h).is_some_and(|w| w.style & WS_TABSTOP != 0);
        match code {
            EDI_FIRSTTABITEM => return step_from(&items, None, false, tab).unwrap_or(0),
            EDI_LASTTABITEM => return step_from(&items, None, true, tab).unwrap_or(0),
            _ => {}
        }
        // Any other code needs the item of hwnd; without one, the first item.
        let Some(at) = self.dlg_item_of(dlg, hwnd).and_then(|i| items.iter().position(|&h| h == i)) else {
            return first;
        };
        let item = items[at];
        match code {
            EDI_NEXTTABITEM | EDI_PREVTABITEM =>
                step_from(&items, Some(at), code == EDI_PREVTABITEM, tab).unwrap_or(0),
            _ => {
                let range = group_range(&items, at, |h| {
                    self.get_window(h).is_some_and(|w| w.style & WS_GROUP != 0)
                });
                let start = range.start;
                let group = &items[range];
                let member = |h: u32| h == item || enabled(h);
                let found = match code {
                    EDI_FIRSTGROUPITEM => step_from(group, None, false, member),
                    EDI_LASTGROUPITEM => step_from(group, None, true, member),
                    _ => step_from(group, Some(at - start), code == EDI_PREVGROUPITEM, member),
                };
                found.unwrap_or(item)
            }
        }
    }

    /// The name `WinQueryClassName` reports for `hwnd`: "#nnnnn" for a
    /// preregistered class (a frame, a dialog, a built-in control), else the
    /// name its class was registered with.
    pub fn public_class_name(&self, hwnd: u32) -> Option<String> {
        let w = self.get_window(hwnd)?;
        let atom = if self.frame_to_client.contains_key(&hwnd) {
            Some(WC_FRAME_ATOM & 0xFFFF)
        } else {
            class_atom_of_window_class(&w.class_name)
        };
        Some(match atom {
            Some(n) => format!("#{n}"),
            None => w.class_name.clone(),
        })
    }

    /// The thread (OS/2 thread id) whose queue `hwnd` belongs to.
    fn window_thread(&self, hwnd: u32) -> Option<u32> {
        let hmq = self.get_window(hwnd)?.hmq;
        self.tid_to_hmq.iter().find(|&(_, &q)| q == hmq && hmq != 0)
            .map(|(&vcpu, _)| super::threads::tid_of_vcpu(vcpu))
    }
}

impl super::Loader {
    /// Write a POINTL; false for a NULL or unwritable pointer.
    pub(super) fn put_pointl(&self, ptr: u32, (x, y): (i32, i32)) -> bool {
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&x.to_le_bytes());
        bytes[4..].copy_from_slice(&y.to_le_bytes());
        ptr != 0 && self.guest_write_bytes(ptr, &bytes).is_some()
    }

    /// WinMapWindowPoints(HWND hwndFrom, HWND hwndTo, PPOINTL aptl, LONG cwpt) -> BOOL
    pub(super) fn win_map_window_points(&self, tid: u32, from: u32, to: u32, aptl: u32, count: i32) -> ApiResult {
        if !self.pm_check_hwnd(tid, from, false) || !self.pm_check_hwnd(tid, to, false) {
            return ApiResult::Normal(0);
        }
        let Ok(n) = usize::try_from(count) else { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE) };
        let offset = {
            let wm = self.shared.window_mgr.lock_or_recover();
            match (wm.window_origin(from), wm.window_origin(to)) {
                (Some(a), Some(b)) => (a.0.wrapping_sub(b.0), a.1.wrapping_sub(b.1)),
                _ => return self.pm_fail(tid, PMERR_INVALID_HWND),
            }
        };
        if n == 0 { return ApiResult::Normal(1); }
        let Some(mut bytes) = self.guest_read_bytes(aptl, n * 8).filter(|_| aptl != 0) else {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        };
        for p in bytes.as_chunks_mut::<8>().0 {
            let x = i32::from_le_bytes([p[0], p[1], p[2], p[3]]).wrapping_add(offset.0);
            let y = i32::from_le_bytes([p[4], p[5], p[6], p[7]]).wrapping_add(offset.1);
            p[..4].copy_from_slice(&x.to_le_bytes());
            p[4..].copy_from_slice(&y.to_le_bytes());
        }
        if self.guest_write_bytes(aptl, &bytes).is_none() {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        ApiResult::Normal(1)
    }

    /// WinWindowFromPoint(HWND hwndParent, PPOINTL pptl, BOOL fChildren) -> HWND
    pub(super) fn win_window_from_point(&self, tid: u32, parent: u32, pptl: u32, deep: bool) -> ApiResult {
        if !self.pm_check_hwnd(tid, parent, false) { return ApiResult::Normal(0); }
        let Some((x, y)) = self.read_pointl(pptl).filter(|_| pptl != 0) else {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        };
        let wm = self.shared.window_mgr.lock_or_recover();
        if parent == HWND_OBJECT { return ApiResult::Normal(0); }
        let Some((ox, oy)) = wm.window_origin(parent) else { return ApiResult::Normal(0) };
        ApiResult::Normal(wm.window_from_point(parent, ox.wrapping_add(x), oy.wrapping_add(y), deep))
    }

    /// WinQueryDesktopWindow(HAB hab, HDC hdc) -> HWND
    pub(super) fn win_query_desktop_window(&self, tid: u32, hdc: u32) -> ApiResult {
        let screen = hdc == 0 || matches!(self.shared.window_mgr.lock_or_recover().dcs.get(&hdc),
                                          Some(DeviceContext::Window(_)));
        if !screen { return self.pm_fail(tid, PMERR_INV_HDC); }
        ApiResult::Normal(HWND_DESKTOP)
    }

    /// WinQueryAnchorBlock(HWND hwnd) -> HAB: Warpine has one anchor block
    /// (`WinInitialize` returns it to every thread).
    pub(super) fn win_query_anchor_block(&self, tid: u32, hwnd: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        ApiResult::Normal(MOCK_HAB)
    }

    /// WinIsChild(HWND hwnd, HWND hwndParent) -> BOOL
    pub(super) fn win_is_child(&self, tid: u32, hwnd: u32, parent: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) || !self.pm_check_hwnd(tid, parent, false) {
            return ApiResult::Normal(0);
        }
        let wm = self.shared.window_mgr.lock_or_recover();
        let root = wm.root_of(hwnd);
        let child = root != HWND_OBJECT && hwnd != HWND_OBJECT && if is_desktop(parent) {
            is_desktop(root)
        } else {
            wm.is_same_or_descendant(hwnd, parent)
        };
        ApiResult::Normal(child as u32)
    }

    /// WinQueryWindowProcess(HWND hwnd, PPID ppid, PTID ptid) -> BOOL
    pub(super) fn win_query_window_process(&self, tid: u32, hwnd: u32, ppid: u32, ptid: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let thread = self.shared.window_mgr.lock_or_recover().window_thread(hwnd)
            .unwrap_or(super::threads::MAIN_TID);
        for (ptr, value) in [(ppid, PROCESS_ID), (ptid, thread)] {
            if ptr != 0 && self.guest_write::<u32>(ptr, value).is_none() {
                return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
            }
        }
        ApiResult::Normal(1)
    }

    /// WinMultWindowFromIDs(HWND hwndParent, PHWND ahwnd, ULONG idFirst, ULONG idLast) -> LONG
    pub(super) fn win_mult_window_from_ids(&self, tid: u32, parent: u32, ahwnd: u32, first: u32, last: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, parent, false) { return ApiResult::Normal(0); }
        if first > 0xFFFF || last > 0xFFFF || first > last {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        let n = (last - first + 1) as usize;
        let mut found = vec![0u32; n];
        {
            let wm = self.shared.window_mgr.lock_or_recover();
            for h in wm.child_windows(parent) {
                let Some(id) = wm.get_window(h).map(|w| w.id) else { continue };
                // The first window with an id wins, as with WinWindowFromID.
                if (first..=last).contains(&id) && found[(id - first) as usize] == 0 {
                    found[(id - first) as usize] = h;
                }
            }
        }
        let bytes: Vec<u8> = found.iter().flat_map(|h| h.to_le_bytes()).collect();
        if ahwnd == 0 || self.guest_write_bytes(ahwnd, &bytes).is_none() {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        ApiResult::Normal(found.iter().filter(|&&h| h != 0).count() as u32)
    }

    /// WinEnumDlgItem(HWND hwndDlg, HWND hwnd, ULONG code) -> HWND
    pub(super) fn win_enum_dlg_item(&self, tid: u32, dlg: u32, hwnd: u32, code: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, dlg, false) { return ApiResult::Normal(0); }
        if code > EDI_PREVGROUPITEM { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        ApiResult::Normal(self.shared.window_mgr.lock_or_recover().enum_dlg_item(dlg, hwnd, code))
    }

    /// WinQueryClassName(HWND hwnd, LONG cchMax, PCH pch) -> LONG
    pub(super) fn win_query_class_name(&self, tid: u32, hwnd: u32, cch_max: i32, pch: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let Some(name) = self.shared.window_mgr.lock_or_recover().public_class_name(hwnd) else {
            // The desktop and object pseudo windows have no class here.
            return self.pm_fail(tid, PMERR_INVALID_HWND);
        };
        if cch_max <= 0 || pch == 0 { return self.pm_fail(tid, PMERR_INVALID_STRING_PARM); }
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let mut bytes = super::codepage::cp_encode(&name, cp);
        bytes.truncate(cch_max as usize - 1);
        let len = bytes.len() as u32;
        bytes.push(0);
        if self.guest_write_bytes(pch, &bytes).is_none() {
            return self.pm_fail(tid, PMERR_INVALID_STRING_PARM);
        }
        ApiResult::Normal(len)
    }

    /// WinQueryClassInfo(HAB hab, PCSZ pszClassName, PCLASSINFO pClassInfo) -> BOOL
    pub(super) fn win_query_class_info(&self, tid: u32, psz: u32, pci: u32) -> ApiResult {
        let atom = if psz & 0xFFFF_0000 == 0xFFFF_0000 {
            Some(psz & 0xFFFF)
        } else if psz == 0 {
            return self.pm_fail(tid, PMERR_INVALID_ATOM_NAME);
        } else {
            None
        };
        let info = match atom {
            Some(atom) => self.builtin_class_info(atom),
            None => {
                let name = self.read_guest_string(psz);
                if name.is_empty() { return self.pm_fail(tid, PMERR_INVALID_ATOM_NAME); }
                match class_atom_of_name(&name) {
                    Some(atom) => self.builtin_class_info(atom),
                    None => match self.shared.window_mgr.lock_or_recover().get_class(&name) {
                        Some(c) => Some((c.style, c.pfn_wp, c.cb_window_data.unwrap_or(0))),
                        None => return self.pm_fail(tid, PMERR_ATOM_NAME_NOT_FOUND),
                    },
                }
            }
        };
        let Some((style, pfn, cb)) = info else { return self.pm_fail(tid, PMERR_INVALID_INTEGER_ATOM) };
        let mut bytes = [0u8; CLASSINFO_SIZE];
        for (i, v) in [style, pfn, cb].into_iter().enumerate() {
            bytes[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        if pci == 0 || self.guest_write_bytes(pci, &bytes).is_none() {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        ApiResult::Normal(1)
    }

    /// `(flClassStyle, pfnWindowProc, cbWindowData)` of preregistered class
    /// `atom`, `None` for an atom that is not one Warpine knows.
    fn builtin_class_info(&self, atom: u32) -> Option<(u32, u32, u32)> {
        PUBLIC_CLASSES.iter().find(|(a, _)| *a == atom)?;
        let style = if atom == WC_FRAME_ATOM & 0xFFFF { CS_PUBLIC | CS_FRAME } else { CS_PUBLIC };
        Some((style, self.resolve_import("PMWIN", ORD_WIN_DEF_WINDOW_PROC) as u32, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_error::error_id;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::Loader;

    const ORD_WIN_ENUM_DLG_ITEM: u32 = 740;
    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_IS_CHILD: u32 = 768;
    const ORD_WIN_MAP_WINDOW_POINTS: u32 = 788;
    const ORD_WIN_QUERY_ANCHOR_BLOCK: u32 = 800;
    const ORD_WIN_QUERY_CLASS_NAME: u32 = 805;
    const ORD_WIN_QUERY_DESKTOP_WINDOW: u32 = 813;
    const ORD_WIN_QUERY_WINDOW_PROCESS: u32 = 838;
    const ORD_WIN_WINDOW_FROM_POINT: u32 = 900;
    const ORD_WIN_MULT_WINDOW_FROM_IDS: u32 = 917;
    const ORD_WIN_QUERY_CLASS_INFO: u32 = 925;

    const BAD_HWND: u32 = 0xDEAD;
    const BUF: u32 = 0x3000;
    const BUF2: u32 = 0x3200;

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

    fn last_error(loader: &Loader) -> u32 {
        call(loader, ORD_WIN_GET_LAST_ERROR, &[MOCK_HAB])
    }

    fn err(code: u32) -> u32 { error_id(SEVERITY_ERROR, code) }

    /// Geometry: a sizing-border frame at (100, 50), 300×200, with a title
    /// bar; its client (inside the left border) holds a child at (10, 20),
    /// 50×40, which holds a grandchild at (5, 5), 10×10. A dialog at
    /// (500, 400), 200×100, with a button at (8, 10), 60×20.
    struct Tree { frame: u32, client: u32, child: u32, grandchild: u32, dialog: u32, button: u32 }

    fn tree(loader: &Loader) -> Tree {
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hmq = wm.create_mq(437);
        wm.tid_to_hmq.insert(0, hmq);
        wm.register_class("App".into(), 0x1234, 0);
        let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        wm.frame_to_client.insert(frame, client);
        let child = wm.create_window("WC_STATIC".into(), client, hmq);
        let grandchild = wm.create_window("WC_BUTTON".into(), child, hmq);
        let dialog = wm.create_window(WindowManager::DIALOG_CLASS.into(), HWND_DESKTOP, hmq);
        let button = wm.create_window("WC_BUTTON".into(), dialog, hmq);
        let set = |wm: &mut WindowManager, h: u32, r: (i32, i32, i32, i32)| {
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy) = r;
            w.visible = true;
        };
        set(&mut wm, frame, (100, 50, 300, 200));
        wm.get_window_mut(frame).unwrap().frame_flags = FCF_TITLEBAR | FCF_SIZEBORDER;
        wm.layout_frame(frame);
        wm.get_window_mut(client).unwrap().visible = true;
        set(&mut wm, child, (10, 20, 50, 40));
        set(&mut wm, grandchild, (5, 5, 10, 10));
        set(&mut wm, dialog, (500, 400, 200, 100));
        wm.get_window_mut(dialog).unwrap().frame_flags = FCF_DLGBORDER;
        set(&mut wm, button, (8, 10, 60, 20));
        wm.z_push_top(frame);
        wm.z_push_top(dialog);
        Tree { frame, client, child, grandchild, dialog, button }
    }

    fn put_points(loader: &Loader, pts: &[(i32, i32)]) {
        for (i, &p) in pts.iter().enumerate() {
            assert!(loader.put_pointl(BUF + 8 * i as u32, p));
        }
    }

    fn points(loader: &Loader, n: u32) -> Vec<(i32, i32)> {
        (0..n).map(|i| loader.read_pointl(BUF + 8 * i).unwrap()).collect()
    }

    #[test]
    fn window_origins_follow_warpine_placement() {
        let loader = Loader::new_mock();
        let t = tree(&loader);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.window_origin(HWND_DESKTOP), Some((0, 0)));
        assert_eq!(wm.window_origin(t.frame), Some((100, 50)));
        assert_eq!(wm.window_origin(t.client), Some((100 + CHROME_BORDER_W, 50)), "inside the left border");
        assert_eq!(wm.window_origin(t.child), Some((112, 70)));
        assert_eq!(wm.window_origin(t.grandchild), Some((117, 75)));
        assert_eq!(wm.window_origin(t.dialog), Some((500, 400)));
        assert_eq!(wm.window_origin(t.button), Some((508, 410)), "relative to the dialog, border included");
        assert_eq!(wm.window_origin(BAD_HWND), None);
    }

    #[test]
    fn map_window_points_between_windows_and_the_desktop() {
        let loader = Loader::new_mock();
        let t = tree(&loader);
        put_points(&loader, &[(0, 0), (3, 4)]);
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[t.child, HWND_DESKTOP, BUF, 2]), 1);
        assert_eq!(points(&loader, 2), vec![(112, 70), (115, 74)]);
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[HWND_DESKTOP, t.client, BUF, 2]), 1);
        assert_eq!(points(&loader, 2), vec![(10, 20), (13, 24)], "back into the client");
        // A RECTL is two points; client → frame adds the border.
        put_points(&loader, &[(0, 0), (30, 40)]);
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[t.client, t.frame, BUF, 2]), 1);
        assert_eq!(points(&loader, 2), vec![(CHROME_BORDER_W, 0), (30 + CHROME_BORDER_W, 40)]);
        // Between unrelated windows, and to itself.
        put_points(&loader, &[(1, 1)]);
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[t.button, t.grandchild, BUF, 1]), 1);
        assert_eq!(points(&loader, 1), vec![(508 + 1 - 117, 410 + 1 - 75)]);
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[t.child, t.child, BUF, 1]), 1);
        assert_eq!(points(&loader, 1), vec![(508 + 1 - 117, 410 + 1 - 75)], "unchanged");
        // No points; bad arguments.
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[t.child, HWND_DESKTOP, 0, 0]), 1);
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[t.child, HWND_DESKTOP, BUF, (-1i32) as u32]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[t.child, HWND_DESKTOP, 0, 1]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[t.child, HWND_DESKTOP, u32::MAX - 7, 2]), 0);
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[t.child, HWND_DESKTOP, BUF, i32::MAX as u32]), 0);
        assert_eq!(call(&loader, ORD_WIN_MAP_WINDOW_POINTS, &[BAD_HWND, HWND_DESKTOP, BUF, 1]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));
    }

    #[test]
    fn window_from_point_finds_the_topmost_visible_window() {
        let loader = Loader::new_mock();
        let t = tree(&loader);
        let at = |parent: u32, p: (i32, i32), deep: bool| {
            assert!(loader.put_pointl(BUF2, p));
            call(&loader, ORD_WIN_WINDOW_FROM_POINT, &[parent, BUF2, deep as u32])
        };
        // Desktop coordinates: the grandchild at (117..127, 75..85).
        assert_eq!(at(HWND_DESKTOP, (120, 80), true), t.grandchild);
        assert_eq!(at(HWND_DESKTOP, (120, 80), false), t.frame, "main windows only");
        assert_eq!(at(HWND_DESKTOP, (550, 420), true), t.button);
        assert_eq!(at(HWND_DESKTOP, (10, 10), true), HWND_DESKTOP, "no window there");
        assert_eq!(at(HWND_DESKTOP, (-1, 10), true), 0, "off the screen");
        // Relative to the client: the child at (10, 20).
        assert_eq!(at(t.client, (12, 22), false), t.child, "immediate child");
        assert_eq!(at(t.client, (16, 26), false), t.child);
        assert_eq!(at(t.client, (16, 26), true), t.grandchild);
        assert_eq!(at(t.client, (200, 100), true), t.client, "in no child");
        assert_eq!(at(t.client, (-3, 0), true), 0, "outside the parent");
        // The title bar belongs to the frame.
        assert_eq!(at(HWND_DESKTOP, (200, 245), true), t.frame);
        // Hidden windows are skipped.
        loader.shared.window_mgr.lock_or_recover().get_window_mut(t.grandchild).unwrap().visible = false;
        assert_eq!(at(t.client, (16, 26), true), t.child);
        assert_eq!(at(BAD_HWND, (0, 0), true), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));
        assert_eq!(call(&loader, ORD_WIN_WINDOW_FROM_POINT, &[t.client, 0, 1]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
    }

    #[test]
    fn desktop_anchor_block_and_process() {
        let loader = Loader::new_mock();
        let t = tree(&loader);
        assert_eq!(call(&loader, ORD_WIN_QUERY_DESKTOP_WINDOW, &[MOCK_HAB, 0]), HWND_DESKTOP);
        let (mem_dc, win_dc) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            (wm.open_dc(DeviceContext::Memory), wm.open_dc(DeviceContext::Window(t.client)))
        };
        assert_eq!(call(&loader, ORD_WIN_QUERY_DESKTOP_WINDOW, &[MOCK_HAB, win_dc]), HWND_DESKTOP);
        assert_eq!(call(&loader, ORD_WIN_QUERY_DESKTOP_WINDOW, &[MOCK_HAB, mem_dc]), 0, "no windows in memory");
        assert_eq!(last_error(&loader), err(PMERR_INV_HDC));
        assert_eq!(call(&loader, ORD_WIN_QUERY_DESKTOP_WINDOW, &[MOCK_HAB, 0x6666]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INV_HDC));

        assert_eq!(call(&loader, ORD_WIN_QUERY_ANCHOR_BLOCK, &[t.button]), MOCK_HAB);
        assert_eq!(call(&loader, ORD_WIN_QUERY_ANCHOR_BLOCK, &[BAD_HWND]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));

        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_PROCESS, &[t.child, BUF, BUF + 4]), 1);
        assert_eq!(loader.guest_read::<u32>(BUF), Some(PROCESS_ID));
        assert_eq!(loader.guest_read::<u32>(BUF + 4), Some(super::super::threads::MAIN_TID));
        // A window of another thread's queue (vCPU 2 = thread 2).
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let q = wm.create_mq(437);
            wm.tid_to_hmq.insert(2, q);
            wm.get_window_mut(t.button).unwrap().hmq = q;
        }
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_PROCESS, &[t.button, 0, BUF + 4]), 1, "NULL PPID");
        assert_eq!(loader.guest_read::<u32>(BUF + 4), Some(2));
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_PROCESS, &[BAD_HWND, BUF, BUF + 4]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));
    }

    #[test]
    fn is_child_covers_descendants_itself_and_not_object_windows() {
        let loader = Loader::new_mock();
        let t = tree(&loader);
        let is_child = |h, p| call(&loader, ORD_WIN_IS_CHILD, &[h, p]);
        assert_eq!(is_child(t.grandchild, t.frame), 1, "any depth");
        assert_eq!(is_child(t.child, t.child), 1, "itself");
        assert_eq!(is_child(t.frame, t.child), 0);
        assert_eq!(is_child(t.button, t.frame), 0);
        assert_eq!(is_child(t.button, HWND_DESKTOP), 1);
        let obj = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let o = wm.create_window("App".into(), HWND_OBJECT, 0);
            wm.create_window("App".into(), o, 0)
        };
        assert_eq!(is_child(obj, HWND_DESKTOP), 0, "an object window");
        assert_eq!(is_child(obj, HWND_OBJECT), 0);
        assert_eq!(is_child(BAD_HWND, t.frame), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));
    }

    #[test]
    fn mult_window_from_ids_fills_the_array() {
        let loader = Loader::new_mock();
        let t = tree(&loader);
        let (a, b) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.get_window(t.frame).unwrap().hmq;
            let a = wm.create_window("WC_BUTTON".into(), t.dialog, hmq);
            let b = wm.create_window("WC_BUTTON".into(), t.dialog, hmq);
            let dup = wm.create_window("WC_BUTTON".into(), t.dialog, hmq);
            wm.get_window_mut(t.button).unwrap().id = 100;
            wm.get_window_mut(a).unwrap().id = 102;
            wm.get_window_mut(b).unwrap().id = 103;
            wm.get_window_mut(dup).unwrap().id = 103;
            (a, b)
        };
        assert_eq!(call(&loader, ORD_WIN_MULT_WINDOW_FROM_IDS, &[t.dialog, BUF, 100, 104]), 3);
        let got: Vec<u32> = (0..5).map(|i| loader.guest_read::<u32>(BUF + 4 * i).unwrap()).collect();
        assert_eq!(got, vec![t.button, 0, a, b, 0], "the first window with an id");
        // Main windows by id.
        loader.shared.window_mgr.lock_or_recover().get_window_mut(t.frame).unwrap().id = 7;
        assert_eq!(call(&loader, ORD_WIN_MULT_WINDOW_FROM_IDS, &[HWND_DESKTOP, BUF, 7, 7]), 1);
        assert_eq!(loader.guest_read::<u32>(BUF), Some(t.frame));
        // Bad ranges, buffers and handles.
        for (first, last) in [(5, 4), (0, 0x1_0000), (0x1_0000, 0x1_0000)] {
            assert_eq!(call(&loader, ORD_WIN_MULT_WINDOW_FROM_IDS, &[t.dialog, BUF, first, last]), 0);
            assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        }
        assert_eq!(call(&loader, ORD_WIN_MULT_WINDOW_FROM_IDS, &[t.dialog, 0, 100, 101]), 0);
        assert_eq!(call(&loader, ORD_WIN_MULT_WINDOW_FROM_IDS, &[t.dialog, u32::MAX - 3, 100, 101]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_MULT_WINDOW_FROM_IDS, &[BAD_HWND, BUF, 1, 2]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));
    }

    /// A dialog with items: [g1 group+tab, r2, r3 (disabled), g2 group+tab
    /// holding a child, e5 tab, hidden h6 tab].
    fn dialog(loader: &Loader) -> (u32, [u32; 6], u32) {
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hmq = wm.create_mq(437);
        let dlg = wm.create_window(WindowManager::DIALOG_CLASS.into(), HWND_DESKTOP, hmq);
        let styles = [WS_GROUP | WS_TABSTOP, 0, WS_DISABLED, WS_GROUP | WS_TABSTOP, WS_TABSTOP, WS_TABSTOP];
        let items: [u32; 6] = std::array::from_fn(|i| {
            let h = wm.create_window("WC_BUTTON".into(), dlg, hmq);
            let w = wm.get_window_mut(h).unwrap();
            w.style = styles[i];
            w.visible = i != 5;
            h
        });
        let inner = wm.create_window("WC_STATIC".into(), items[3], hmq);
        (dlg, items, inner)
    }

    #[test]
    fn enum_dlg_item_walks_tab_stops_and_groups() {
        let loader = Loader::new_mock();
        let (dlg, [g1, r2, r3, g2, e5, _h6], inner) = dialog(&loader);
        let e = |h: u32, code: u32| call(&loader, ORD_WIN_ENUM_DLG_ITEM, &[dlg, h, code]);
        assert_eq!(e(0, EDI_FIRSTTABITEM), g1);
        assert_eq!(e(0, EDI_LASTTABITEM), e5, "the hidden tab stop does not count");
        assert_eq!(e(g1, EDI_NEXTTABITEM), g2);
        assert_eq!(e(e5, EDI_NEXTTABITEM), g1, "wraps around");
        assert_eq!(e(g1, EDI_PREVTABITEM), e5);
        assert_eq!(e(r2, EDI_NEXTTABITEM), g2, "from an item without WS_TABSTOP");
        assert_eq!(e(inner, EDI_PREVTABITEM), g1, "a window lower down stands for its item");
        // Groups: [g1, r2, r3] and [g2, e5, h6].
        assert_eq!(e(r2, EDI_FIRSTGROUPITEM), g1);
        assert_eq!(e(g1, EDI_LASTGROUPITEM), r2, "the disabled item is skipped");
        assert_eq!(e(g1, EDI_NEXTGROUPITEM), r2);
        assert_eq!(e(r2, EDI_NEXTGROUPITEM), g1, "wraps within the group");
        assert_eq!(e(g1, EDI_PREVGROUPITEM), r2);
        assert_eq!(e(r3, EDI_NEXTGROUPITEM), g1, "from a disabled item");
        assert_eq!(e(e5, EDI_NEXTGROUPITEM), g2);
        assert_eq!(e(inner, EDI_LASTGROUPITEM), e5, "result is an immediate child");
        // An invalid hwnd gives the first item; a bad code fails.
        assert_eq!(e(BAD_HWND, EDI_NEXTGROUPITEM), g1);
        assert_eq!(e(0, EDI_NEXTTABITEM), g1);
        assert_eq!(e(g1, 8), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_ENUM_DLG_ITEM, &[BAD_HWND, g1, EDI_FIRSTTABITEM]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));
        // A window without items.
        assert_eq!(call(&loader, ORD_WIN_ENUM_DLG_ITEM, &[r2, 0, EDI_FIRSTTABITEM]), 0);
    }

    #[test]
    fn class_names_of_public_and_application_classes() {
        let loader = Loader::new_mock();
        let t = tree(&loader);
        let name = |h: u32, max: i32| -> (u32, String) {
            let n = call(&loader, ORD_WIN_QUERY_CLASS_NAME, &[h, max as u32, BUF]);
            (n, loader.read_guest_string(BUF))
        };
        assert_eq!(name(t.frame, 32), (2, "#1".into()), "a standard frame is a WC_FRAME");
        assert_eq!(name(t.dialog, 32), (2, "#1".into()));
        assert_eq!(name(t.button, 32), (2, "#3".into()));
        assert_eq!(name(t.child, 32), (2, "#5".into()));
        assert_eq!(name(t.client, 32), (3, "App".into()));
        assert_eq!(name(t.client, 3), (2, "Ap".into()), "cut to cchMax - 1 with a null");
        assert_eq!(name(t.client, 1), (0, "".into()));
        assert_eq!(call(&loader, ORD_WIN_QUERY_CLASS_NAME, &[t.client, 0, BUF]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_STRING_PARM));
        assert_eq!(call(&loader, ORD_WIN_QUERY_CLASS_NAME, &[t.client, 8, 0]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_STRING_PARM));
        assert_eq!(call(&loader, ORD_WIN_QUERY_CLASS_NAME, &[BAD_HWND, 8, BUF]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));
        assert_eq!(call(&loader, ORD_WIN_QUERY_CLASS_NAME, &[HWND_DESKTOP, 8, BUF]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));
        // Other atoms and the menu.
        let wm = loader.shared.window_mgr.lock_or_recover();
        let mut wm2 = wm;
        let spin = wm2.create_window("WC_SPINBUTTON".into(), t.client, 0);
        let other = wm2.create_window("WC_ATOM_0x0026".into(), t.client, 0);
        let menu = wm2.create_window("#Menu".into(), t.frame, 0);
        assert_eq!(wm2.public_class_name(spin).as_deref(), Some("#32"));
        assert_eq!(wm2.public_class_name(other).as_deref(), Some("#38"));
        assert_eq!(wm2.public_class_name(menu).as_deref(), Some("#4"));
    }

    #[test]
    fn class_info_of_public_and_application_classes() {
        let loader = Loader::new_mock();
        tree(&loader);
        loader.shared.window_mgr.lock_or_recover().set_class_window_data("App", 8);
        let info = |psz: u32| -> Option<(u32, u32, u32)> {
            (call(&loader, ORD_WIN_QUERY_CLASS_INFO, &[MOCK_HAB, psz, BUF2]) == 1).then(|| {
                let r = |i: u32| loader.guest_read::<u32>(BUF2 + 4 * i).unwrap();
                (r(0), r(1), r(2))
            })
        };
        let def_proc = loader.resolve_import("PMWIN", ORD_WIN_DEF_WINDOW_PROC) as u32;
        assert_eq!(info(WC_BUTTON_ATOM), Some((CS_PUBLIC, def_proc, 0)));
        assert_eq!(info(WC_FRAME_ATOM), Some((CS_PUBLIC | CS_FRAME, def_proc, 0)));
        loader.guest_write_bytes(BUF, b"#6\0");
        assert_eq!(info(BUF), Some((CS_PUBLIC, def_proc, 0)), "#6 is WC_ENTRYFIELD");
        loader.guest_write_bytes(BUF, b"App\0");
        assert_eq!(info(BUF), Some((0, 0x1234, 8)));
        // Errors.
        assert_eq!(info(0xFFFF_0026), None, "WC_SLIDER: Warpine has none");
        assert_eq!(last_error(&loader), err(PMERR_INVALID_INTEGER_ATOM));
        loader.guest_write_bytes(BUF, b"#99\0");
        assert_eq!(info(BUF), None);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_INTEGER_ATOM));
        loader.guest_write_bytes(BUF, b"Nope\0");
        assert_eq!(info(BUF), None);
        assert_eq!(last_error(&loader), err(PMERR_ATOM_NAME_NOT_FOUND));
        assert_eq!(info(0), None);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_ATOM_NAME));
        loader.guest_write_bytes(BUF, b"\0");
        assert_eq!(info(BUF), None);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_ATOM_NAME));
        assert_eq!(call(&loader, ORD_WIN_QUERY_CLASS_INFO, &[MOCK_HAB, WC_BUTTON_ATOM, 0]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
    }

    #[test]
    fn class_atom_names() {
        assert_eq!(class_atom_of_name("#1"), Some(1));
        assert_eq!(class_atom_of_name("#32768"), Some(32768));
        assert_eq!(class_atom_of_name("#123456"), None, "at most five digits");
        assert_eq!(class_atom_of_name("#"), None);
        assert_eq!(class_atom_of_name("#Dialog"), None);
        assert_eq!(class_atom_of_name("App"), None);
        assert_eq!(class_atom_of_window_class("WC_ATOM_12"), Some(12));
        assert_eq!(class_atom_of_window_class("App"), None);
    }
}
