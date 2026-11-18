// SPDX-License-Identifier: GPL-3.0-only
//
// The switch list (PMSHAPI, after the IBM PM Reference "Switch List
// Functions"): `WinAddSwitchEntry` (120), `WinCreateSwitchEntry` (121),
// `WinChangeSwitchEntry` (123), `WinQuerySwitchEntry` (124),
// `WinQuerySwitchHandle` (125), `WinQuerySwitchList` (126),
// `WinRemoveSwitchEntry` (129) and `WinSwitchToProgram` (131).
//
// The switch list ("Window List") holds an `SWCNTRL` per program window
// the user can switch to. A frame created with `FCF_TASKLIST` gets an entry
// titled with the frame's title, removed when the frame is destroyed;
// programs add, change and remove their own entries.
//
// Scope: each Warpine process keeps its own switch list (ADR-0020: one OS/2
// process per host process, nothing shared unless a feature crosses the
// boundary on purpose). A program sees its own entries only; programs it
// starts with `WinStartApp` keep theirs. A system-wide list would need a
// host mechanism (a session directory like the named pipes' of ADR-0022)
// and a desktop shared by the processes, which Warpine does not have yet.
//
// Unverified (the Reference is silent): handles numbered from
// `FIRST_SWITCH_HANDLE`; `idProcess` 0 meaning the calling process; a title
// cut to `MAXNAMEL` bytes; `WinQuerySwitchList` with a buffer returning the
// number of entries it wrote; `WinQuerySwitchHandle` also finding the entry
// of a client's frame; `PMERR_NO_HANDLE` when no entry is found;
// `PMERR_INVALID_WINDOW` for an `hwnd` that is no window; `WinSwitchToProgram`
// showing and activating the frame without restoring a minimized one;
// entries a program added staying after their window is destroyed.

use std::collections::BTreeMap;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::WindowManager;

/// `PMERR_NO_SPACE` (`<pmerr.h>`).
pub const PMERR_NO_SPACE: u32 = 0x1201;
/// `PMERR_INVALID_SWITCH_HANDLE` (`<pmerr.h>`).
pub const PMERR_INVALID_SWITCH_HANDLE: u32 = 0x1202;
/// `PMERR_NO_HANDLE` (`<pmerr.h>`).
pub const PMERR_NO_HANDLE: u32 = 0x1203;
/// `PMERR_INVALID_WINDOW` (`<pmerr.h>`).
pub const PMERR_INVALID_WINDOW: u32 = 0x1206;

/// `SWL_VISIBLE` (`<pmwin.h>`).
pub const SWL_VISIBLE: u32 = 4;
/// `SWL_JUMPABLE` (`<pmwin.h>`).
pub const SWL_JUMPABLE: u32 = 2;
/// `MAXNAMEL` (`<pmwin.h>`): longest switch-list title.
pub const MAXNAMEL: usize = 60;
/// Bytes of `SWCNTRL.szSwtitle` (`MAXNAMEL + 4`).
const SWTITLE_BYTES: usize = MAXNAMEL + 4;
/// `PROG_PM` (`<pmshl.h>` / `<pmwin.h>`): the program type of a PM frame.
pub const PROG_PM: u32 = 3;

// SWCNTRL field offsets (<pmwin.h>, every field 4 bytes but the title).
const SWC_HWND: usize = 0;
const SWC_HWND_ICON: usize = 4;
const SWC_HPROG: usize = 8;
const SWC_PROCESS: usize = 12;
const SWC_SESSION: usize = 16;
const SWC_VISIBILITY: usize = 20;
const SWC_JUMP: usize = 24;
const SWC_TITLE: usize = 28;
const SWC_PROG_TYPE: usize = SWC_TITLE + SWTITLE_BYTES;
/// `sizeof(SWCNTRL)`.
pub const SWCNTRL_SIZE: usize = SWC_PROG_TYPE + 4;
/// `sizeof(SWENTRY)`: the handle, then the `SWCNTRL`.
pub const SWENTRY_SIZE: usize = 4 + SWCNTRL_SIZE;
/// `SWBLOCK.cswentry` before the entries.
const SWBLOCK_HEADER: usize = 4;

/// Most entries in a process's switch list.
pub const MAX_SWITCH_ENTRIES: usize = 256;
/// The first switch handle.
const FIRST_SWITCH_HANDLE: u32 = 0x0001_0001;

/// An `SWCNTRL`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwControl {
    pub hwnd: u32,
    pub hwnd_icon: u32,
    pub hprog: u32,
    pub process: u32,
    pub session: u32,
    pub visibility: u32,
    pub jump: u32,
    /// Title bytes in the program's codepage, at most `MAXNAMEL`.
    pub title: Vec<u8>,
    pub prog_type: u32,
}

fn field(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl SwControl {
    /// From the bytes of an `SWCNTRL`; the title ends at its first NUL and
    /// is cut to `MAXNAMEL` bytes.
    pub fn from_bytes(b: &[u8; SWCNTRL_SIZE]) -> SwControl {
        let raw = &b[SWC_TITLE..SWC_TITLE + SWTITLE_BYTES];
        let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len()).min(MAXNAMEL);
        SwControl {
            hwnd: field(b, SWC_HWND),
            hwnd_icon: field(b, SWC_HWND_ICON),
            hprog: field(b, SWC_HPROG),
            process: field(b, SWC_PROCESS),
            session: field(b, SWC_SESSION),
            visibility: field(b, SWC_VISIBILITY),
            jump: field(b, SWC_JUMP),
            title: raw[..len].to_vec(),
            prog_type: field(b, SWC_PROG_TYPE),
        }
    }

    /// The bytes of an `SWCNTRL` (title NUL-padded).
    pub fn to_bytes(&self) -> [u8; SWCNTRL_SIZE] {
        let mut b = [0u8; SWCNTRL_SIZE];
        for (at, v) in [(SWC_HWND, self.hwnd), (SWC_HWND_ICON, self.hwnd_icon), (SWC_HPROG, self.hprog),
                        (SWC_PROCESS, self.process), (SWC_SESSION, self.session),
                        (SWC_VISIBILITY, self.visibility), (SWC_JUMP, self.jump),
                        (SWC_PROG_TYPE, self.prog_type)] {
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        let n = self.title.len().min(MAXNAMEL);
        b[SWC_TITLE..SWC_TITLE + n].copy_from_slice(&self.title[..n]);
        b
    }
}

#[derive(Debug, Clone)]
struct Entry {
    ctl: SwControl,
    /// Made for a `FCF_TASKLIST` frame: removed with it.
    auto: bool,
}

/// Why a switch-list call failed: the `PMERR_*` code.
pub type SwitchResult<T> = Result<T, u32>;

/// The process's switch list (`WindowManager::switch_list`), in the order
/// the entries were added.
#[derive(Debug)]
pub struct SwitchList {
    entries: BTreeMap<u32, Entry>,
    next: u32,
}

impl Default for SwitchList {
    fn default() -> Self { SwitchList { entries: BTreeMap::new(), next: FIRST_SWITCH_HANDLE } }
}

impl SwitchList {
    /// Add an entry (`idProcess` 0: this process).
    pub fn add(&mut self, mut ctl: SwControl, auto: bool) -> SwitchResult<u32> {
        if self.entries.len() >= MAX_SWITCH_ENTRIES {
            return Err(PMERR_NO_SPACE);
        }
        if ctl.process == 0 { ctl.process = PROCESS_ID; }
        ctl.title.truncate(MAXNAMEL);
        let h = self.next;
        self.next += 1;
        self.entries.insert(h, Entry { ctl, auto });
        Ok(h)
    }

    /// The entry of a `FCF_TASKLIST` frame.
    pub fn add_frame(&mut self, frame: u32, title: Vec<u8>) -> SwitchResult<u32> {
        self.add(SwControl { hwnd: frame, visibility: SWL_VISIBLE, jump: SWL_JUMPABLE, title,
                             prog_type: PROG_PM, ..Default::default() }, true)
    }

    pub fn remove(&mut self, h: u32) -> SwitchResult<()> {
        self.entries.remove(&h).map(|_| ()).ok_or(PMERR_INVALID_SWITCH_HANDLE)
    }

    pub fn change(&mut self, h: u32, mut ctl: SwControl) -> SwitchResult<()> {
        let e = self.entries.get_mut(&h).ok_or(PMERR_INVALID_SWITCH_HANDLE)?;
        if ctl.process == 0 { ctl.process = PROCESS_ID; }
        ctl.title.truncate(MAXNAMEL);
        e.ctl = ctl;
        Ok(())
    }

    pub fn query(&self, h: u32) -> SwitchResult<&SwControl> {
        self.entries.get(&h).map(|e| &e.ctl).ok_or(PMERR_INVALID_SWITCH_HANDLE)
    }

    /// The entry of window `hwnd` (the first of the windows `hwnds`, in
    /// order, that has one), or else of process `pid`.
    pub fn handle_for(&self, hwnds: &[u32], pid: u32) -> Option<u32> {
        if hwnds.is_empty() {
            return self.entries.iter().find(|(_, e)| e.ctl.process == pid).map(|(&h, _)| h);
        }
        hwnds.iter().find_map(|&w| self.entries.iter().find(|(_, e)| e.ctl.hwnd == w).map(|(&h, _)| h))
    }

    /// All entries, in the order they were added.
    pub fn list(&self) -> impl Iterator<Item = (u32, &SwControl)> {
        self.entries.iter().map(|(&h, e)| (h, &e.ctl))
    }

    pub fn len(&self) -> usize { self.entries.len() }

    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Windows `doomed` are destroyed: their `FCF_TASKLIST` entries go.
    pub fn forget_windows(&mut self, doomed: &[u32]) {
        self.entries.retain(|_, e| !(e.auto && doomed.contains(&e.ctl.hwnd)));
    }
}

/// `SWBLOCK` bytes holding as many of `entries` as fit in `cb` bytes, and
/// how many that is.
pub fn switch_block<'a>(entries: impl Iterator<Item = (u32, &'a SwControl)>, cb: usize) -> (Vec<u8>, u32) {
    let fit = cb.saturating_sub(SWBLOCK_HEADER) / SWENTRY_SIZE;
    let mut out = vec![0u8; SWBLOCK_HEADER];
    let mut n = 0u32;
    for (h, ctl) in entries.take(fit) {
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&ctl.to_bytes());
        n += 1;
    }
    out[..SWBLOCK_HEADER].copy_from_slice(&n.to_le_bytes());
    (out, n)
}

impl WindowManager {
    /// Bring the frame of switch entry `hwnd` forward: shown, on top,
    /// active, `WM_ACTIVATE` to the old and the new frame (as
    /// `WinSetActiveWindow`). False when it is no frame of this process.
    fn switch_to_frame(&mut self, hwnd: u32) -> bool {
        let frame = {
            let c = self.client_to_frame(hwnd);
            if self.frame_to_client.contains_key(&c) { c } else { hwnd }
        };
        let is_frame = self.frame_to_client.contains_key(&frame) || self.is_dialog(frame);
        if !is_frame { return false; }
        if let Some(w) = self.get_window_mut(frame) { w.visible = true; }
        let prev = self.focused_hwnd;
        self.focused_hwnd = frame;
        self.z_push_top(frame);
        self.mark_all_dirty();
        if prev != frame {
            if prev != 0 {
                self.post_msg(self.frame_msg_target(prev), WM_ACTIVATE, 0, frame);
            }
            self.post_msg(self.frame_msg_target(frame), WM_ACTIVATE, 1, prev);
        }
        true
    }
}

impl super::Loader {
    /// The `SWCNTRL` at `p`.
    fn read_swcntrl(&self, p: u32) -> SwitchResult<SwControl> {
        if p == 0 { return Err(PMERR_INVALID_PARM); }
        let bytes = self.guest_read_bytes(p, SWCNTRL_SIZE).ok_or(PMERR_INVALID_PARM)?;
        let arr: [u8; SWCNTRL_SIZE] = bytes.try_into().map_err(|_| PMERR_INVALID_PARM)?;
        let ctl = SwControl::from_bytes(&arr);
        if ctl.hwnd != 0 && !super::pm_error::valid_hwnd(&self.shared.window_mgr.lock_or_recover(), ctl.hwnd) {
            return Err(PMERR_INVALID_WINDOW);
        }
        Ok(ctl)
    }

    /// A handle, or NULLHANDLE with the error recorded.
    fn switch_handle(&self, tid: u32, r: SwitchResult<u32>) -> u32 {
        r.unwrap_or_else(|code| { self.pm_error(tid, code); 0 })
    }

    /// 0, or the error code (also recorded for `WinGetLastError`).
    fn switch_rc(&self, tid: u32, r: SwitchResult<()>) -> u32 {
        match r {
            Ok(()) => 0,
            Err(code) => { self.pm_error(tid, code); code }
        }
    }

    /// `WinAddSwitchEntry(pswctl)` / `WinCreateSwitchEntry(hab, pswctl)` →
    /// HSWITCH.
    pub fn win_add_switch_entry(&self, tid: u32, p: u32) -> u32 {
        let r = self.read_swcntrl(p)
            .and_then(|ctl| self.shared.window_mgr.lock_or_recover().switch_list.add(ctl, false));
        self.switch_handle(tid, r)
    }

    /// `WinRemoveSwitchEntry(hsw)` → 0 or the error.
    pub fn win_remove_switch_entry(&self, tid: u32, h: u32) -> u32 {
        let r = self.shared.window_mgr.lock_or_recover().switch_list.remove(h);
        self.switch_rc(tid, r)
    }

    /// `WinChangeSwitchEntry(hsw, pswctl)` → 0 or the error.
    pub fn win_change_switch_entry(&self, tid: u32, h: u32, p: u32) -> u32 {
        // Each lock is released before the next is taken (read_swcntrl
        // locks the window manager too).
        let r = (|| {
            let known = self.shared.window_mgr.lock_or_recover().switch_list.query(h).is_ok();
            if !known { return Err(PMERR_INVALID_SWITCH_HANDLE); }
            let ctl = self.read_swcntrl(p)?;
            self.shared.window_mgr.lock_or_recover().switch_list.change(h, ctl)
        })();
        self.switch_rc(tid, r)
    }

    /// `WinQuerySwitchEntry(hsw, pswctl)` → 0 or the error.
    pub fn win_query_switch_entry(&self, tid: u32, h: u32, p: u32) -> u32 {
        let r = (|| {
            let bytes = self.shared.window_mgr.lock_or_recover().switch_list.query(h)?.to_bytes();
            if p == 0 { return Err(PMERR_INVALID_PARM); }
            self.guest_write_bytes(p, &bytes).ok_or(PMERR_INVALID_PARM)
        })();
        self.switch_rc(tid, r)
    }

    /// `WinQuerySwitchHandle(hwnd, pid)` → the entry of `hwnd` (or of its
    /// frame), or for NULLHANDLE the first of process `pid` (0: this one).
    pub fn win_query_switch_handle(&self, tid: u32, hwnd: u32, pid: u32) -> u32 {
        let wm = self.shared.window_mgr.lock_or_recover();
        let r = if hwnd != 0 {
            let frame = wm.find_frame_for_hwnd(hwnd);
            wm.switch_list.handle_for(&[hwnd, frame], 0)
        } else {
            wm.switch_list.handle_for(&[], if pid == 0 { PROCESS_ID } else { pid })
        };
        drop(wm);
        self.switch_handle(tid, r.ok_or(PMERR_NO_HANDLE))
    }

    /// `WinQuerySwitchList(hab, pswblk, cbDataLength)` → the number of
    /// entries; with a buffer, the number written to its `SWBLOCK`.
    pub fn win_query_switch_list(&self, tid: u32, p: u32, cb: u32) -> u32 {
        let wm = self.shared.window_mgr.lock_or_recover();
        if p == 0 || cb == 0 {
            return wm.switch_list.len() as u32;
        }
        let (bytes, n) = switch_block(wm.switch_list.list(), cb as usize);
        drop(wm);
        match self.guest_write_bytes(p, &bytes) {
            Some(()) => n,
            None => { self.pm_error(tid, PMERR_INVALID_PARM); 0 }
        }
    }

    /// `WinSwitchToProgram(hsw)` → 0 or the error.
    pub fn win_switch_to_program(&self, tid: u32, h: u32) -> u32 {
        let r = (|| {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let hwnd = wm.switch_list.query(h)?.hwnd;
            if wm.switch_to_frame(hwnd) { Ok(()) } else { Err(PMERR_INVALID_WINDOW) }
        })();
        self.switch_rc(tid, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;

    fn ctl(hwnd: u32, title: &str) -> SwControl {
        SwControl { hwnd, title: title.as_bytes().to_vec(), visibility: SWL_VISIBLE, jump: SWL_JUMPABLE,
                    ..Default::default() }
    }

    #[test]
    fn swcntrl_layout_round_trips() {
        assert_eq!(SWCNTRL_SIZE, 96);
        assert_eq!(SWENTRY_SIZE, 100);
        let c = SwControl { hwnd: 1, hwnd_icon: 2, hprog: 3, process: 4, session: 5, visibility: 6,
                            jump: 7, title: b"Title".to_vec(), prog_type: 9 };
        let b = c.to_bytes();
        assert_eq!(&b[SWC_TITLE..SWC_TITLE + 6], b"Title\0");
        assert_eq!(field(&b, SWC_PROG_TYPE), 9);
        assert_eq!(SwControl::from_bytes(&b), c);
        // An unterminated title is cut to MAXNAMEL.
        let mut long = [b'x'; SWCNTRL_SIZE];
        long[..SWC_TITLE].fill(0);
        assert_eq!(SwControl::from_bytes(&long).title.len(), MAXNAMEL);
    }

    #[test]
    fn entries_are_added_changed_queried_and_removed() {
        let mut list = SwitchList::default();
        let a = list.add(ctl(0x1000, "A"), false).unwrap();
        let b = list.add(ctl(0x2000, "B"), false).unwrap();
        assert_ne!(a, b);
        assert_eq!(list.query(a).unwrap().process, PROCESS_ID);
        list.change(a, ctl(0x1000, "A2")).unwrap();
        assert_eq!(list.query(a).unwrap().title, b"A2");
        assert_eq!(list.handle_for(&[0x2000], 0), Some(b));
        assert_eq!(list.handle_for(&[0x3000, 0x1000], 0), Some(a));
        assert_eq!(list.handle_for(&[], PROCESS_ID), Some(a));
        assert_eq!(list.handle_for(&[], 7), None);
        list.remove(a).unwrap();
        assert_eq!(list.remove(a), Err(PMERR_INVALID_SWITCH_HANDLE));
        assert_eq!(list.query(a), Err(PMERR_INVALID_SWITCH_HANDLE));
        assert_eq!(list.change(a, ctl(0, "")), Err(PMERR_INVALID_SWITCH_HANDLE));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn the_list_is_bounded() {
        let mut list = SwitchList::default();
        for _ in 0..MAX_SWITCH_ENTRIES { list.add(ctl(0, "x"), false).unwrap(); }
        assert_eq!(list.add(ctl(0, "x"), false), Err(PMERR_NO_SPACE));
    }

    #[test]
    fn frame_entries_go_with_their_frame() {
        let mut list = SwitchList::default();
        let auto = list.add_frame(0x1000, b"Frame".to_vec()).unwrap();
        let own = list.add(ctl(0x1000, "Own"), false).unwrap();
        assert_eq!(list.query(auto).unwrap().prog_type, PROG_PM);
        list.forget_windows(&[0x1000]);
        assert!(list.query(auto).is_err());
        assert!(list.query(own).is_ok());
    }

    #[test]
    fn switch_blocks_hold_what_fits() {
        let mut list = SwitchList::default();
        let a = list.add(ctl(1, "A"), false).unwrap();
        list.add(ctl(2, "B"), false).unwrap();
        let (b, n) = switch_block(list.list(), SWBLOCK_HEADER + SWENTRY_SIZE + 10);
        assert_eq!(n, 1);
        assert_eq!(b.len(), SWBLOCK_HEADER + SWENTRY_SIZE);
        assert_eq!(field(&b, 0), 1);
        assert_eq!(field(&b, 4), a);
        assert_eq!(field(&b, 8), 1);
        let (b, n) = switch_block(list.list(), 2);
        assert_eq!((b.len(), n), (SWBLOCK_HEADER, 0));
    }

    const TID: u32 = 1;
    const CTL: u32 = 0x0010_0000;
    const BLOCK: u32 = 0x0010_1000;

    #[test]
    fn api_calls_read_and_write_guest_structures() {
        let l = Loader::new_mock();
        l.guest_write_bytes(CTL, &ctl(0, "Mine").to_bytes()).unwrap();
        let h = l.win_add_switch_entry(TID, CTL);
        assert_ne!(h, 0);
        assert_eq!(l.win_query_switch_list(TID, 0, 0), 1);
        assert_eq!(l.win_query_switch_list(TID, BLOCK, 4096), 1);
        assert_eq!(l.guest_read::<u32>(BLOCK), Some(1));
        assert_eq!(l.guest_read::<u32>(BLOCK + 4), Some(h));
        assert_eq!(l.win_query_switch_handle(TID, 0, 0), h);
        l.guest_write_bytes(CTL, &ctl(0, "Changed").to_bytes()).unwrap();
        assert_eq!(l.win_change_switch_entry(TID, h, CTL), 0);
        l.guest_write_bytes(CTL, &[0u8; SWCNTRL_SIZE]).unwrap();
        assert_eq!(l.win_query_switch_entry(TID, h, CTL), 0);
        assert_eq!(l.guest_read_bytes(CTL + SWC_TITLE as u32, 8).unwrap(), b"Changed\0");
        // A window handle that is no window.
        l.guest_write_bytes(CTL, &ctl(0x7777, "Bad").to_bytes()).unwrap();
        assert_eq!(l.win_add_switch_entry(TID, CTL), 0);
        assert_eq!(l.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_WINDOW);
        assert_eq!(l.win_add_switch_entry(TID, 0), 0);
        // The entry has no frame to switch to.
        assert_eq!(l.win_switch_to_program(TID, h), PMERR_INVALID_WINDOW);
        assert_eq!(l.win_remove_switch_entry(TID, h), 0);
        assert_eq!(l.win_remove_switch_entry(TID, h), PMERR_INVALID_SWITCH_HANDLE);
        assert_eq!(l.win_query_switch_handle(TID, 0, 0), 0);
        assert_eq!(l.shared.pm_errors.take(TID) & 0xFFFF, PMERR_NO_HANDLE);
    }
}
