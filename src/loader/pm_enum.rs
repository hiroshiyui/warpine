// SPDX-License-Identifier: GPL-3.0-or-later

//! Window enumeration: `WinBeginEnumWindows` (702), `WinGetNextWindow`
//! (756), `WinEndEnumWindows` (737).
//!
//! `WinBeginEnumWindows(hwnd)` takes a snapshot of `hwnd`'s children, top
//! first, and returns a handle (`HENUM`) that `WinGetNextWindow` reads one
//! window at a time (NULLHANDLE at the end) until `WinEndEnumWindows` frees
//! it. The children of the desktop include the windows of the other
//! programs on the session's desktop, frames in the session's stacking
//! order (`Loader::desktop_windows`, `pm_remote.rs`, ADR-0055); another
//! program's window can be enumerated too.
//!
//! Warpine's choices (unverified against OS/2):
//! - a window of this program destroyed after the snapshot is skipped;
//! - at most `MAX_ENUMS` enumerations are open at once; the next
//!   `WinBeginEnumWindows` fails with `PMERR_HEAP_OUT_OF_MEMORY`;
//! - the children of `HWND_OBJECT` are this program's object windows,
//!   newest first.

use std::collections::{HashMap, VecDeque};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::{ApiResult, Loader};

/// Most enumerations one program may have open at once.
pub const MAX_ENUMS: usize = 64;
/// The first `HENUM` handed out.
const FIRST_HENUM: u32 = 0x00E0_0001;

/// Open enumerations: each a snapshot of windows still to return.
#[derive(Debug)]
pub struct EnumTable {
    lists: HashMap<u32, VecDeque<u32>>,
    next: u32,
}

impl Default for EnumTable {
    fn default() -> Self {
        EnumTable { lists: HashMap::new(), next: FIRST_HENUM }
    }
}

impl EnumTable {
    /// A new enumeration of `windows`: its handle, `None` when
    /// `MAX_ENUMS` are open.
    pub fn begin(&mut self, windows: Vec<u32>) -> Option<u32> {
        if self.lists.len() >= MAX_ENUMS {
            return None;
        }
        // Handles are never 0 and not reused while open.
        while self.next == 0 || self.lists.contains_key(&self.next) {
            self.next = self.next.wrapping_add(1);
        }
        let h = self.next;
        self.next = self.next.wrapping_add(1);
        self.lists.insert(h, windows.into());
        Some(h)
    }

    /// The next window of enumeration `h` that is still `alive` (0 at the
    /// end), `None` for a handle that is no open enumeration.
    pub fn next(&mut self, h: u32, alive: impl Fn(u32) -> bool) -> Option<u32> {
        let list = self.lists.get_mut(&h)?;
        while let Some(w) = list.pop_front() {
            if alive(w) {
                return Some(w);
            }
        }
        Some(0)
    }

    /// End enumeration `h`: whether it was open.
    pub fn end(&mut self, h: u32) -> bool {
        self.lists.remove(&h).is_some()
    }
}

impl Loader {
    /// `WinBeginEnumWindows(hwnd)`: see the module comment.
    pub(super) fn win_begin_enum_windows(&self, tid: u32, hwnd: u32) -> ApiResult {
        let windows = if hwnd == 0 || hwnd == HWND_DESKTOP {
            self.desktop_windows()
        } else if hwnd == HWND_OBJECT {
            let wm = self.shared.window_mgr.lock_or_recover();
            let mut v: Vec<u32> = wm.windows().filter(|w| w.parent == HWND_OBJECT).map(|w| w.handle).collect();
            v.sort_unstable_by(|a, b| b.cmp(a));
            v
        } else if self.shared.window_mgr.lock_or_recover().is_foreign(hwnd) {
            match self.remote_children(hwnd) {
                Some(v) => v,
                None => return self.pm_fail(tid, PMERR_INVALID_HWND),
            }
        } else {
            if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
            let mut v = self.shared.window_mgr.lock_or_recover().child_windows(hwnd);
            v.reverse();
            v
        };
        match self.shared.window_mgr.lock_or_recover().enums.begin(windows) {
            Some(h) => ApiResult::Normal(h),
            None => self.pm_fail(tid, PMERR_HEAP_OUT_OF_MEMORY),
        }
    }

    /// `WinGetNextWindow(henum)`: the next window, NULLHANDLE at the end.
    pub(super) fn win_get_next_window(&self, tid: u32, henum: u32) -> ApiResult {
        let next = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let mut enums = std::mem::take(&mut wm.enums);
            // Other programs' windows are returned as they were.
            let next = enums.next(henum, |w| wm.is_foreign(w) || wm.is_window(w));
            wm.enums = enums;
            next
        };
        match next {
            Some(w) => ApiResult::Normal(w),
            None => self.pm_fail(tid, PMERR_INVALID_HENUM),
        }
    }

    /// `WinEndEnumWindows(henum)`.
    pub(super) fn win_end_enum_windows(&self, tid: u32, henum: u32) -> ApiResult {
        if self.shared.window_mgr.lock_or_recover().enums.end(henum) {
            ApiResult::Normal(1)
        } else {
            self.pm_fail(tid, PMERR_INVALID_HENUM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_enumeration_returns_its_snapshot_skipping_the_dead_then_null() {
        let mut t = EnumTable::default();
        let h = t.begin(vec![3, 2, 1]).unwrap();
        assert_ne!(h, 0);
        assert_eq!(t.next(h, |w| w != 2), Some(3));
        assert_eq!(t.next(h, |w| w != 2), Some(1), "a window gone since is skipped");
        assert_eq!(t.next(h, |_| true), Some(0));
        assert_eq!(t.next(h, |_| true), Some(0), "stays at the end");
        assert!(t.end(h));
        assert_eq!(t.next(h, |_| true), None, "ended");
        assert!(!t.end(h));
    }

    #[test]
    fn open_enumerations_are_bounded() {
        let mut t = EnumTable::default();
        let hs: Vec<u32> = (0..MAX_ENUMS).map(|_| t.begin(vec![]).unwrap()).collect();
        assert_eq!(t.begin(vec![]), None);
        assert!(t.end(hs[5]));
        let again = t.begin(vec![7]).unwrap();
        assert!(!hs.iter().enumerate().any(|(i, &h)| i != 5 && h == again), "no open handle reused");
    }
}
