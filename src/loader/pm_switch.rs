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
// Scope: on the session's desktop (ADR-0052) the switch list is the
// session's, kept by the desktop server (`desktop/services.rs`, ADR-0056):
// every program sees every entry, changes and removes its own, and a
// program's entries go when it ends. With the desktop in the process
// (`WARPINE_DESKTOP=local`, no session) each process keeps its own list.
//
// The window list: Ctrl+Esc (`SYSTEM_ACCELS`) is `WM_SYSCOMMAND(
// SC_TASKMANAGER)`, for which `WinDefWindowProc` shows the visible entries
// in a pop-up menu (`show_window_list`); the item chosen comes back as one
// of Warpine's own `SC_WINDOWLIST_FIRST + n` and switches to that program.
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
use crate::desktop::proto::{SwitchEntry, WinCall, WinReply, MAX_HANDLES, SERVER_ADDRESS};

/// The first of Warpine's own `WM_SYSCOMMAND` codes for the window list's
/// items (`SC_WINDOWLIST_FIRST + n`: the `n`th entry shown); PM's `SC_*`
/// codes end well below.
pub const SC_WINDOWLIST_FIRST: u32 = 0x8F00;
/// Most entries the window list shows.
pub const MAX_WINDOW_LIST_ITEMS: usize = 64;
/// About half the pop-up's width, to centre it.
const WINDOW_LIST_HALF_WIDTH: i32 = 100;

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
    /// This process's id (`idProcess` 0 means it).
    pub own_process: u32,
}

impl Default for SwitchList {
    fn default() -> Self {
        SwitchList { entries: BTreeMap::new(), next: FIRST_SWITCH_HANDLE, own_process: PROCESS_ID }
    }
}

impl SwitchList {
    /// Add an entry (`idProcess` 0: this process).
    pub fn add(&mut self, mut ctl: SwControl, auto: bool) -> SwitchResult<u32> {
        if self.entries.len() >= MAX_SWITCH_ENTRIES {
            return Err(PMERR_NO_SPACE);
        }
        if ctl.process == 0 { ctl.process = self.own_process; }
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
        if ctl.process == 0 { ctl.process = self.own_process; }
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
        let is_frame = self.is_frame(frame);
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

impl WindowManager {
    /// The `FCF_TASKLIST` entry of a new frame: its title as bytes in the
    /// thread's codepage (this process's list) or as text (the session's,
    /// told without waiting). A full list leaves the frame without one.
    pub fn add_frame_switch(&mut self, frame: u32, title: Vec<u8>, text: String) {
        match &self.session_link {
            Some(link) => {
                let entry = SwitchEntry {
                    hwnd: frame, visibility: SWL_VISIBLE, jump: SWL_JUMPABLE, prog_type: PROG_PM,
                    process: self.switch_list.own_process, title: text, ..Default::default()
                };
                link.notify(SERVER_ADDRESS, WinCall::SwitchAdd { entry, auto: true });
            }
            None => { let _ = self.switch_list.add_frame(frame, title); }
        }
    }

    /// Windows `doomed` are destroyed: their `FCF_TASKLIST` entries go.
    pub fn forget_switch_windows(&mut self, doomed: &[u32]) {
        self.switch_list.forget_windows(doomed);
        if let Some(link) = &self.session_link {
            for chunk in doomed.chunks(MAX_HANDLES) {
                link.notify(SERVER_ADDRESS, WinCall::SwitchForget { hwnds: chunk.to_vec() });
            }
        }
    }
}

impl super::Loader {
    /// The session's desktop link, when the switch list is the session's.
    fn session(&self) -> Option<&std::sync::Arc<crate::desktop::client::DesktopLink>> {
        self.shared.desktop_link.get()
    }

    /// Make `call` on the session's services: the reply, or the error code
    /// (`lost` when the server does not answer).
    fn session_call(&self, link: &crate::desktop::client::DesktopLink, call: WinCall, lost: u32)
        -> SwitchResult<WinReply> {
        match link.call(SERVER_ADDRESS, call) {
            Some(WinReply::Error(code)) => Err(code),
            Some(WinReply::Failed) | None => Err(lost),
            Some(r) => Ok(r),
        }
    }

    /// The session's entry for `ctl` (title decoded from the thread's
    /// codepage; `idProcess` 0 is this process).
    fn to_entry(&self, tid: u32, handle: u32, ctl: &SwControl) -> SwitchEntry {
        let process = if ctl.process == 0 { self.shared.process_id() } else { ctl.process };
        SwitchEntry {
            handle, hwnd: ctl.hwnd, hwnd_icon: ctl.hwnd_icon, hprog: ctl.hprog, process, session: ctl.session,
            visibility: ctl.visibility, jump: ctl.jump, prog_type: ctl.prog_type,
            title: super::codepage::cp_decode(&ctl.title, self.thread_codepage(tid)),
        }
    }

    /// The `SWCNTRL` of a session entry, its title in the thread's
    /// codepage cut to `MAXNAMEL` bytes.
    fn swcntrl_of(&self, tid: u32, e: &SwitchEntry) -> SwControl {
        let mut title = super::codepage::cp_encode(&e.title, self.thread_codepage(tid));
        title.truncate(MAXNAMEL);
        SwControl {
            hwnd: e.hwnd, hwnd_icon: e.hwnd_icon, hprog: e.hprog, process: e.process, session: e.session,
            visibility: e.visibility, jump: e.jump, title, prog_type: e.prog_type,
        }
    }

    /// The session's switch list, as `(handle, SWCNTRL)`s in the thread's
    /// codepage.
    fn session_switches(&self, tid: u32, link: &crate::desktop::client::DesktopLink)
        -> SwitchResult<Vec<(u32, SwControl)>> {
        match self.session_call(link, WinCall::SwitchList, PMERR_NO_HANDLE)? {
            WinReply::Switches(es) => Ok(es.iter().map(|e| (e.handle, self.swcntrl_of(tid, e))).collect()),
            _ => Err(PMERR_NO_HANDLE),
        }
    }

    /// The switch list as `(handle, SWCNTRL)`s: the session's, or this
    /// process's.
    pub(super) fn switch_entries(&self, tid: u32) -> SwitchResult<Vec<(u32, SwControl)>> {
        match self.session() {
            Some(link) => self.session_switches(tid, link),
            None => Ok(self.shared.window_mgr.lock_or_recover().switch_list.list()
                .map(|(h, c)| (h, c.clone())).collect()),
        }
    }

    /// Entry `h`.
    fn switch_query(&self, tid: u32, h: u32) -> SwitchResult<SwControl> {
        match self.session() {
            Some(link) => match self.session_call(link, WinCall::SwitchQuery { handle: h }, PMERR_INVALID_SWITCH_HANDLE)? {
                WinReply::Switches(es) if !es.is_empty() => Ok(self.swcntrl_of(tid, &es[0])),
                _ => Err(PMERR_INVALID_SWITCH_HANDLE),
            },
            None => self.shared.window_mgr.lock_or_recover().switch_list.query(h).cloned(),
        }
    }

    /// The `SWCNTRL` at `p`: its window must be one (of any program on
    /// the session's desktop).
    fn read_swcntrl(&self, p: u32) -> SwitchResult<SwControl> {
        if p == 0 { return Err(PMERR_INVALID_PARM); }
        let bytes = self.guest_read_bytes(p, SWCNTRL_SIZE).ok_or(PMERR_INVALID_PARM)?;
        let arr: [u8; SWCNTRL_SIZE] = bytes.try_into().map_err(|_| PMERR_INVALID_PARM)?;
        let ctl = SwControl::from_bytes(&arr);
        let wm = self.shared.window_mgr.lock_or_recover();
        if ctl.hwnd != 0 && !super::pm_error::valid_hwnd(&wm, ctl.hwnd) && !wm.is_foreign(ctl.hwnd) {
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
        let r = self.read_swcntrl(p).and_then(|ctl| match self.session() {
            Some(link) => {
                let call = WinCall::SwitchAdd { entry: self.to_entry(tid, 0, &ctl), auto: false };
                match self.session_call(link, call, PMERR_NO_SPACE)? {
                    WinReply::Value(h) => Ok(h),
                    _ => Err(PMERR_NO_SPACE),
                }
            }
            None => self.shared.window_mgr.lock_or_recover().switch_list.add(ctl, false),
        });
        self.switch_handle(tid, r)
    }

    /// `WinRemoveSwitchEntry(hsw)` → 0 or the error.
    pub fn win_remove_switch_entry(&self, tid: u32, h: u32) -> u32 {
        let r = match self.session() {
            Some(link) => self.session_call(link, WinCall::SwitchRemove { handle: h }, PMERR_INVALID_SWITCH_HANDLE)
                .map(|_| ()),
            None => self.shared.window_mgr.lock_or_recover().switch_list.remove(h),
        };
        self.switch_rc(tid, r)
    }

    /// `WinChangeSwitchEntry(hsw, pswctl)` → 0 or the error.
    pub fn win_change_switch_entry(&self, tid: u32, h: u32, p: u32) -> u32 {
        // Each lock is released before the next is taken (read_swcntrl
        // locks the window manager too).
        let r = (|| {
            self.switch_query(tid, h)?;
            let ctl = self.read_swcntrl(p)?;
            match self.session() {
                Some(link) => self.session_call(link, WinCall::SwitchChange { entry: self.to_entry(tid, h, &ctl) },
                                                PMERR_INVALID_SWITCH_HANDLE).map(|_| ()),
                None => self.shared.window_mgr.lock_or_recover().switch_list.change(h, ctl),
            }
        })();
        self.switch_rc(tid, r)
    }

    /// `WinQuerySwitchEntry(hsw, pswctl)` → 0 or the error.
    pub fn win_query_switch_entry(&self, tid: u32, h: u32, p: u32) -> u32 {
        let r = (|| {
            let bytes = self.switch_query(tid, h)?.to_bytes();
            if p == 0 { return Err(PMERR_INVALID_PARM); }
            self.guest_write_bytes(p, &bytes).ok_or(PMERR_INVALID_PARM)
        })();
        self.switch_rc(tid, r)
    }

    /// `WinQuerySwitchHandle(hwnd, pid)` → the entry of `hwnd` (or of its
    /// frame), or for NULLHANDLE the first of process `pid` (0: this one).
    pub fn win_query_switch_handle(&self, tid: u32, hwnd: u32, pid: u32) -> u32 {
        let (hwnds, pid) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let hwnds = if hwnd == 0 { Vec::new() } else { vec![hwnd, wm.find_frame_for_hwnd(hwnd)] };
            (hwnds, if pid == 0 { wm.switch_list.own_process } else { pid })
        };
        let r = match self.session() {
            Some(link) => match self.session_call(link, WinCall::SwitchHandle { hwnds, pid }, PMERR_NO_HANDLE) {
                Ok(WinReply::Value(h)) => Ok(h),
                Ok(_) => Err(PMERR_NO_HANDLE),
                Err(e) => Err(e),
            },
            None => self.shared.window_mgr.lock_or_recover().switch_list.handle_for(&hwnds, pid).ok_or(PMERR_NO_HANDLE),
        };
        self.switch_handle(tid, r)
    }

    /// `WinQuerySwitchList(hab, pswblk, cbDataLength)` → the number of
    /// entries; with a buffer, the number written to its `SWBLOCK`.
    pub fn win_query_switch_list(&self, tid: u32, p: u32, cb: u32) -> u32 {
        let entries = match self.switch_entries(tid) {
            Ok(e) => e,
            Err(code) => { self.pm_error(tid, code); return 0; }
        };
        if p == 0 || cb == 0 {
            return entries.len() as u32;
        }
        let (bytes, n) = switch_block(entries.iter().map(|(h, c)| (*h, c)), cb as usize);
        match self.guest_write_bytes(p, &bytes) {
            Some(()) => n,
            None => { self.pm_error(tid, PMERR_INVALID_PARM); 0 }
        }
    }

    /// `WinSwitchToProgram(hsw)` → 0 or the error. Another program's frame
    /// is shown and activated through the desktop server.
    pub fn win_switch_to_program(&self, tid: u32, h: u32) -> u32 {
        let r = (|| {
            let hwnd = self.switch_query(tid, h)?.hwnd;
            if self.shared.window_mgr.lock_or_recover().is_foreign(hwnd) {
                return if self.switch_to_other_program(hwnd) { Ok(()) } else { Err(PMERR_INVALID_WINDOW) };
            }
            if self.shared.window_mgr.lock_or_recover().switch_to_frame(hwnd) { Ok(()) } else { Err(PMERR_INVALID_WINDOW) }
        })();
        self.switch_rc(tid, r)
    }

    /// The window list (`SC_TASKMANAGER`, Ctrl+Esc) for window `hwnd`: the
    /// visible entries of the switch list in a pop-up menu in the middle
    /// of the desktop; the item chosen comes back to `hwnd` as
    /// `WM_SYSCOMMAND(SC_WINDOWLIST_FIRST + n)` ([`Self::window_list_pick`]).
    pub(super) fn show_window_list(&self, tid: u32, hwnd: u32) {
        let Ok(entries) = self.switch_entries(tid) else { return };
        let shown: Vec<(u32, SwControl)> = entries.into_iter()
            .filter(|(_, c)| c.visibility & SWL_VISIBLE != 0)
            .take(MAX_WINDOW_LIST_ITEMS).collect();
        if shown.is_empty() { return; }
        let cp = self.thread_codepage(tid);
        let items: Vec<super::pm_types::MenuItem> = shown.iter().enumerate().map(|(i, (_, c))| {
            super::pm_types::MenuItem {
                id: (SC_WINDOWLIST_FIRST + i as u32) as u16, style: MIS_TEXT | MIS_SYSCOMMAND,
                attr: 0, text: super::codepage::cp_decode(&c.title, cp), children: Vec::new(), image: None,
            }
        }).collect();
        let (dw, dh) = desktop_size();
        let mut wm = self.shared.window_mgr.lock_or_recover();
        wm.window_list = shown.iter().map(|(h, _)| *h).collect();
        let Some(tx) = wm.gui_tx.as_ref() else { return };
        let request = crate::gui::message::PopupRequest {
            owner: hwnd, items, x: dw as i32 / 2 - WINDOW_LIST_HALF_WIDTH, y: dh as i32 / 2,
            position_on_item: false, item: Some(SC_WINDOWLIST_FIRST),
        };
        let _ = tx.send(crate::gui::GUIMessage::PopupMenu(request));
    }

    /// `WM_SYSCOMMAND(cmd)` default processing for an item of the window
    /// list: switch to its program. Returns whether `cmd` was one.
    pub(super) fn window_list_pick(&self, tid: u32, cmd: u32) -> bool {
        if !(SC_WINDOWLIST_FIRST..SC_WINDOWLIST_FIRST + MAX_WINDOW_LIST_ITEMS as u32).contains(&cmd) {
            return false;
        }
        let h = self.shared.window_mgr.lock_or_recover().window_list.get((cmd - SC_WINDOWLIST_FIRST) as usize).copied();
        if let Some(h) = h {
            self.win_switch_to_program(tid, h);
        }
        true
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
    fn entries_of_this_process_carry_its_own_id() {
        let mut list = SwitchList { own_process: 123, ..Default::default() };
        let a = list.add(ctl(0x1000, "A"), false).unwrap();
        assert_eq!(list.query(a).unwrap().process, 123);
        list.change(a, ctl(0x1000, "A2")).unwrap();
        assert_eq!(list.query(a).unwrap().process, 123);
        assert_eq!(list.handle_for(&[], 123), Some(a));
        assert_eq!(list.handle_for(&[], PROCESS_ID), None);
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

    #[test]
    fn the_window_list_shows_visible_entries_and_switches_to_the_one_chosen() {
        use crate::gui::{create_gui_channel, GUIMessage};
        let l = Loader::new_mock();
        let (tx, rx) = create_gui_channel();
        let (frame, other) = {
            let mut wm = l.shared.window_mgr.lock_or_recover();
            wm.gui_tx = Some(tx);
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(TID, hmq);
            wm.register_class("App".into(), 0x1234, 0);
            let mut std = || {
                let f = wm.create_window("App".into(), HWND_DESKTOP, hmq);
                let c = wm.create_window("App".into(), f, hmq);
                wm.frame_to_client.insert(f, c);
                f
            };
            let (a, b) = (std(), std());
            wm.add_frame_switch(a, b"Program A".to_vec(), "Program A".into());
            wm.add_frame_switch(b, b"Program B".to_vec(), "Program B".into());
            let hidden = wm.switch_list.add(SwControl { hwnd: b, visibility: 0, ..ctl(b, "hidden") }, false).unwrap();
            assert_ne!(hidden, 0);
            wm.z_push_top(a);
            wm.focused_hwnd = a;
            (a, b)
        };
        l.show_window_list(TID, frame);
        let Some(GUIMessage::PopupMenu(r)) = rx.try_iter().next() else { panic!("a pop-up menu") };
        assert_eq!(r.owner, frame);
        let titles: Vec<&str> = r.items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(titles, vec!["Program A", "Program B"], "the hidden entry left out");
        assert_eq!(r.items[1].id as u32, SC_WINDOWLIST_FIRST + 1);
        assert_ne!(r.items[1].style & MIS_SYSCOMMAND, 0, "comes back as WM_SYSCOMMAND");
        // Choosing "Program B" makes it the active frame.
        assert!(l.window_list_pick(TID, SC_WINDOWLIST_FIRST + 1));
        assert_eq!(l.shared.window_mgr.lock_or_recover().focused_hwnd, other);
        assert!(!l.window_list_pick(TID, SC_CLOSE), "other commands are not the list's");
    }

    #[test]
    fn ctrl_esc_is_the_window_list() {
        use super::super::pm_dialog::{accel_matches, SYSTEM_ACCELS};
        // WM_CHAR: KC_VIRTUALKEY | KC_CTRL, VK_ESC in mp2's high word.
        let (mp1, mp2) = (KC_VIRTUALKEY | KC_CTRL, VK_ESC << 16);
        let hit = SYSTEM_ACCELS.iter().find(|e| accel_matches(e, mp1, mp2)).expect("Ctrl+Esc");
        assert_eq!(u32::from(hit.cmd), SC_TASKMANAGER);
    }
}
