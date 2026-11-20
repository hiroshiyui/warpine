// SPDX-License-Identifier: GPL-3.0-or-later

//! System colours: `WinQuerySysColor` and `WinSetSysColors`.
//!
//! From the PM Reference:
//! - `WinQuerySysColor(hwndDesktop, clr, lReserved)` returns the RGB value
//!   of system colour index `clr` (`SYSCLR_*`; `lReserved` "must be 0");
//!   errors `PMERR_INVALID_HWND`, `PMERR_PARAMETER_OUT_OF_RANGE`.
//! - `WinSetSysColors(hwndDesktop, flOptions, flFormat, lStart, cData,
//!   pData)`: `LCOL_RESET` resets all system colours to their defaults
//!   "before processing the remainder of the data"; `LCOL_PURECOLOR` asks for
//!   no dithering; `LCOLF_INDRGB` gives (index, RGB) pairs ("For LCOLF_INDRGB
//!   it must be an even number"), `LCOLF_CONSECRGB` RGB values for the
//!   indices from `lStart` upwards. RGB values are `R * 65536 + G * 256 + B`.
//!   "This function sends all main windows in the system a WM_SYSCOLORCHANGE
//!   message"; afterwards "all windows in the system are invalidated".
//!   Errors `PMERR_INVALID_HWND`, `PMERR_INVALID_FLAG`,
//!   `PMERR_PARAMETER_OUT_OF_RANGE`.
//! - `WM_SYSCOLORCHANGE`: mp1 is `flOptions` "copied from the parameter of
//!   the WinSetSysColors function", mp2 reserved (0).
//!
//! The defaults are the colours Warpine draws with where they correspond
//! (title bars, the desktop, dialog and control faces, selections, the
//! menus); the other indices take the defaults the PM Reference lists.
//! Warpine re-reads the table for title bars (`SYSCLR_ACTIVETITLE`,
//! `SYSCLR_INACTIVETITLE`, `SYSCLR_ACTIVETITLETEXT`,
//! `SYSCLR_INACTIVETITLETEXT`) and wherever a `SYSCLR_*` value is used as a
//! colour (`WinFillRect`, `GpiSetColor` / `GpiSetBackColor`, `WinDrawText`).
//!
//! Unverified (no source): a failing call changes nothing (Warpine checks
//! every index before it sets any); RGB values are masked to 24 bits;
//! `lReserved` other than 0 is `PMERR_PARAMETER_OUT_OF_RANGE`; the
//! `hwndDesktop` argument may be any valid window or NULLHANDLE (as in
//! Warpine's pointer calls); `WM_SYSCOLORCHANGE`
//! is posted (not sent) to the main windows (a frame's message goes to its
//! client, as all frame messages in Warpine); `WinQuerySysColor` returns 0 on
//! error; `LCOL_PURECOLOR` changes nothing (Warpine draws in true colour).

use std::sync::atomic::{AtomicU32, Ordering};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::WindowManager;
use super::ApiResult;

/// The most `WinSetSysColors` data entries Warpine reads in one call.
pub const MAX_SYS_COLOR_DATA: u32 = 4096;

/// The default value of every system colour index, in index order.
const DEFAULT_SYS_COLORS: [(i32, u32); SYSCLR_CSYSCOLORS as usize] = [
    (SYSCLR_SHADOWHILITEBGND,      0x0080_8080),
    (SYSCLR_SHADOWHILITEFGND,      0x00FF_FFFF),
    (SYSCLR_SHADOWTEXT,            0x0080_8080),
    (SYSCLR_ENTRYFIELD,            0x00FF_FFFF), // Warpine's entry fields are white
    (SYSCLR_MENUDISABLEDTEXT,      0x0080_8080),
    (SYSCLR_MENUHILITE,            0x00FF_FFFF), // Warpine: white on navy
    (SYSCLR_MENUHILITEBGND,        0x0000_0080),
    (SYSCLR_PAGEBACKGROUND,        0x00FF_FFFF),
    (SYSCLR_FIELDBACKGROUND,       0x00CC_CCCC),
    (SYSCLR_BUTTONLIGHT,           0x00FF_FFFF),
    (SYSCLR_BUTTONMIDDLE,          0x00D4_D0C8), // Warpine's control face
    (SYSCLR_BUTTONDARK,            0x0080_8080),
    (SYSCLR_BUTTONDEFAULT,         0x0000_0000),
    (SYSCLR_TITLEBOTTOM,           0x0080_8080),
    (SYSCLR_SHADOW,                0x0080_8080),
    (SYSCLR_ICONTEXT,              0x0000_0000),
    (SYSCLR_DIALOGBACKGROUND,      0x00D4_D0C8), // Warpine's dialog face
    (SYSCLR_HILITEFOREGROUND,      0x00FF_FFFF),
    (SYSCLR_HILITEBACKGROUND,      0x0000_0080), // Warpine's selection colour
    (SYSCLR_INACTIVETITLETEXTBGND, 0x0040_4040), // Warpine's inactive title bar
    (SYSCLR_ACTIVETITLETEXTBGND,   0x0000_0080), // Warpine's active title bar
    (SYSCLR_INACTIVETITLETEXT,     0x00FF_FFFF), // Warpine: white on both
    (SYSCLR_ACTIVETITLETEXT,       0x00FF_FFFF),
    (SYSCLR_OUTPUTTEXT,            0x0000_0000),
    (SYSCLR_WINDOWSTATICTEXT,      0x0000_0080),
    (SYSCLR_SCROLLBAR,             0x00A0_A0A0), // Warpine's scroll-bar track
    (SYSCLR_BACKGROUND,            0x0040_8040), // Warpine's desktop
    (SYSCLR_ACTIVETITLE,           0x0000_0080),
    (SYSCLR_INACTIVETITLE,         0x0040_4040),
    (SYSCLR_MENU,                  0x00D4_D0C8), // Warpine's menu face
    (SYSCLR_WINDOW,                0x00FF_FFFF),
    (SYSCLR_WINDOWFRAME,           0x0080_8080),
    (SYSCLR_MENUTEXT,              0x0000_0000),
    (SYSCLR_WINDOWTEXT,            0x0000_0000),
    (SYSCLR_TITLETEXT,             0x00FF_FFFF),
    (SYSCLR_ACTIVEBORDER,          0x00CC_CCCC), // Warpine's border face
    (SYSCLR_INACTIVEBORDER,        0x00CC_CCCC),
    (SYSCLR_APPWORKSPACE,          0x00FF_FFE0),
    (SYSCLR_HELPBACKGROUND,        0x00FF_FFFF),
    (SYSCLR_HELPTEXT,              0x0000_0080),
    (SYSCLR_HELPHILITE,            0x0000_8080),
];

/// The table slot of system colour index `clr`, `None` outside
/// `SYSCLR_SHADOWHILITEBGND..=SYSCLR_HELPHILITE`.
pub fn sys_color_slot(clr: i32) -> Option<usize> {
    (SYSCLR_SHADOWHILITEBGND..=SYSCLR_HELPHILITE).contains(&clr)
        .then(|| (clr - SYSCLR_SHADOWHILITEBGND) as usize)
}

/// The system colours, shared by the window manager and every presentation
/// space (which resolve `SYSCLR_*` colour values through it). Lock-free, so
/// it can be read with any lock held.
#[derive(Debug)]
pub struct SysColorTable {
    colors: [AtomicU32; SYSCLR_CSYSCOLORS as usize],
}

impl Default for SysColorTable {
    fn default() -> Self {
        SysColorTable { colors: std::array::from_fn(|i| AtomicU32::new(DEFAULT_SYS_COLORS[i].1)) }
    }
}

impl SysColorTable {
    /// The RGB value of `clr`, `None` for a value that is no system colour.
    pub fn get(&self, clr: i32) -> Option<u32> {
        sys_color_slot(clr).map(|i| self.colors[i].load(Ordering::Relaxed))
    }

    /// Set `clr` to `rgb` (masked to 24 bits); false for no system colour.
    pub fn set(&self, clr: i32, rgb: u32) -> bool {
        let Some(i) = sys_color_slot(clr) else { return false };
        self.colors[i].store(rgb & 0x00FF_FFFF, Ordering::Relaxed);
        true
    }

    /// `LCOL_RESET`: every colour back to its default.
    pub fn reset(&self) {
        for (slot, &(_, rgb)) in self.colors.iter().zip(DEFAULT_SYS_COLORS.iter()) {
            slot.store(rgb, Ordering::Relaxed);
        }
    }

    /// The default value of `clr`.
    pub fn default_of(clr: i32) -> Option<u32> {
        sys_color_slot(clr).map(|i| DEFAULT_SYS_COLORS[i].1)
    }
}

/// What a `WinSetSysColors` call asks for, checked: `(index, rgb)` pairs.
/// `Err(PMERR_*)` when the options, format or data are invalid.
pub fn parse_sys_colors(options: u32, format: u32, start: i32, data: &[u32]) -> Result<Vec<(i32, u32)>, u32> {
    if options & !(LCOL_RESET | LCOL_PURECOLOR) != 0 {
        return Err(PMERR_INVALID_FLAG);
    }
    let pairs: Vec<(i32, u32)> = match format {
        LCOLF_INDRGB => {
            if !data.len().is_multiple_of(2) { return Err(PMERR_PARAMETER_OUT_OF_RANGE); }
            data.as_chunks::<2>().0.iter().map(|&[clr, rgb]| (clr as i32, rgb)).collect()
        }
        LCOLF_CONSECRGB => data.iter().enumerate()
            .map(|(i, &rgb)| (start.saturating_add(i as i32), rgb)).collect(),
        _ => return Err(PMERR_INVALID_FLAG),
    };
    if pairs.iter().any(|&(clr, _)| sys_color_slot(clr).is_none()) {
        return Err(PMERR_PARAMETER_OUT_OF_RANGE);
    }
    // A starting index outside the table is wrong even without data.
    if format == LCOLF_CONSECRGB && !data.is_empty() && sys_color_slot(start).is_none() {
        return Err(PMERR_PARAMETER_OUT_OF_RANGE);
    }
    Ok(pairs)
}

impl WindowManager {
    /// After a system colour change: post `WM_SYSCOLORCHANGE(options, 0)` to
    /// every main window (a frame's goes to its client), then have every
    /// frame and dialog repaint (`WM_PAINT` to the window that paints it).
    pub fn broadcast_sys_color_change(&mut self, options: u32) {
        let mut main: Vec<u32> = self.windows()
            .filter(|w| w.parent == 0 || w.parent == HWND_DESKTOP)
            .map(|w| w.handle).collect();
        main.sort_unstable();
        for &h in &main {
            self.post_msg(self.frame_msg_target(h), WM_SYSCOLORCHANGE, options, 0);
        }
        let mut frames: Vec<u32> = self.windows().map(|w| w.handle).filter(|&h| self.is_frame(h)).collect();
        frames.sort_unstable();
        for h in frames {
            self.post_msg(self.frame_msg_target(h), WM_PAINT, 0, 0);
        }
        self.mark_all_dirty();
    }
}

impl super::Loader {
    /// Whether `hwnd` may name the desktop: as in the pointer calls, any
    /// valid window handle or NULLHANDLE; else `PMERR_INVALID_HWND`.
    pub(super) fn check_desktop_arg(&self, tid: u32, hwnd: u32) -> bool {
        self.pm_check_hwnd(tid, hwnd, true)
    }

    /// WinQuerySysColor(HWND hwndDesktop, LONG clr, LONG lReserved) -> LONG
    pub(super) fn win_query_sys_color(&self, tid: u32, hwnd: u32, clr: i32, reserved: u32) -> ApiResult {
        if !self.check_desktop_arg(tid, hwnd) { return ApiResult::Normal(0); }
        if reserved != 0 { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        match self.shared.window_mgr.lock_or_recover().sys_colors.get(clr) {
            Some(rgb) => ApiResult::Normal(rgb),
            None => self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE),
        }
    }

    /// WinSetSysColors(HWND hwndDesktop, ULONG flOptions, ULONG flFormat,
    /// LONG clrFirst, ULONG cColorData, PLONG pColorData) -> BOOL
    pub(super) fn win_set_sys_colors(&self, tid: u32, args: [u32; 6]) -> ApiResult {
        let [hwnd, options, format, start, count, data_ptr] = args;
        if !self.check_desktop_arg(tid, hwnd) { return ApiResult::Normal(0); }
        if count > MAX_SYS_COLOR_DATA { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        let data: Vec<u32> = if count == 0 {
            Vec::new()
        } else {
            match self.guest_read_bytes(data_ptr, count as usize * 4).filter(|_| data_ptr != 0) {
                Some(bytes) => bytes.as_chunks::<4>().0.iter().map(|&b| u32::from_le_bytes(b)).collect(),
                None => return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE),
            }
        };
        let pairs = match parse_sys_colors(options, format, start as i32, &data) {
            Ok(p) => p,
            Err(code) => return self.pm_fail(tid, code),
        };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if options & LCOL_RESET != 0 { wm.sys_colors.reset(); }
        for (clr, rgb) in pairs {
            wm.sys_colors.set(clr, rgb);
        }
        wm.broadcast_sys_color_change(options);
        ApiResult::Normal(1)
    }

    /// A colour value of `WinFillRect` as RGB: a `SYSCLR_*` index through
    /// the system colours, anything else through the default colour table.
    pub(crate) fn map_color(&self, clr: u32) -> u32 {
        self.shared.window_mgr.lock_or_recover().sys_colors.get(clr as i32)
            .unwrap_or_else(|| super::pm_types::default_color(clr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_error::error_id;
    use super::super::pm_types::OS2Message;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::Loader;

    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_QUERY_SYS_COLOR: u32 = 826;
    const ORD_WIN_SET_SYS_COLORS: u32 = 871;

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

    fn query(loader: &Loader, clr: i32) -> u32 {
        call(loader, ORD_WIN_QUERY_SYS_COLOR, &[HWND_DESKTOP, clr as u32, 0])
    }

    const DATA: u32 = 0x3000;

    fn put_data(loader: &Loader, values: &[u32]) {
        for (i, &v) in values.iter().enumerate() {
            loader.guest_write::<u32>(DATA + 4 * i as u32, v).unwrap();
        }
    }

    #[test]
    fn slots_cover_the_documented_range() {
        assert_eq!(sys_color_slot(SYSCLR_SHADOWHILITEBGND), Some(0));
        assert_eq!(sys_color_slot(SYSCLR_HELPHILITE), Some(SYSCLR_CSYSCOLORS as usize - 1));
        assert_eq!(sys_color_slot(-51), None);
        assert_eq!(sys_color_slot(-9), None);
        assert_eq!(sys_color_slot(0), None);
        // The default table is in index order with no gaps.
        for (i, &(clr, _)) in DEFAULT_SYS_COLORS.iter().enumerate() {
            assert_eq!(sys_color_slot(clr), Some(i), "{clr}");
        }
    }

    #[test]
    fn defaults_follow_warpine_drawing_and_the_reference() {
        let loader = Loader::new_mock();
        assert_eq!(query(&loader, SYSCLR_ACTIVETITLE), 0x0000_0080, "Warpine's navy title bar");
        assert_eq!(query(&loader, SYSCLR_INACTIVETITLE), 0x0040_4040);
        assert_eq!(query(&loader, SYSCLR_WINDOW), 0x00FF_FFFF);
        assert_eq!(query(&loader, SYSCLR_BACKGROUND), 0x0040_8040, "Warpine's desktop");
        assert_eq!(query(&loader, SYSCLR_HELPHILITE), 0x0000_8080, "the reference's default");
        assert_eq!(query(&loader, SYSCLR_APPWORKSPACE), 0x00FF_FFE0);
        assert_eq!(last_error(&loader), 0);
    }

    #[test]
    fn query_rejects_bad_indices_handles_and_reserved_values() {
        let loader = Loader::new_mock();
        for clr in [-51, -9, 0, 5] {
            assert_eq!(query(&loader, clr), 0, "{clr}");
            assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
        }
        assert_eq!(call(&loader, ORD_WIN_QUERY_SYS_COLOR, &[0xDEAD, SYSCLR_WINDOW as u32, 0]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
        assert_eq!(call(&loader, ORD_WIN_QUERY_SYS_COLOR, &[HWND_DESKTOP, SYSCLR_WINDOW as u32, 1]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_QUERY_SYS_COLOR, &[0, SYSCLR_WINDOW as u32, 0]), 0x00FF_FFFF,
                   "NULLHANDLE stands for the desktop");
    }

    #[test]
    fn set_with_index_pairs_and_consecutive_values_then_reset() {
        let loader = Loader::new_mock();
        put_data(&loader, &[SYSCLR_WINDOW as u32, 0x0012_3456, SYSCLR_ACTIVETITLE as u32, 0xFF00_8000]);
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS, &[HWND_DESKTOP, 0, LCOLF_INDRGB, 0, 4, DATA]), 1);
        assert_eq!(query(&loader, SYSCLR_WINDOW), 0x0012_3456);
        assert_eq!(query(&loader, SYSCLR_ACTIVETITLE), 0x0000_8000, "masked to 24 bits");
        // Consecutive from SYSCLR_HELPBACKGROUND: HELPBACKGROUND, HELPTEXT, HELPHILITE.
        put_data(&loader, &[0x11, 0x22, 0x33]);
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS,
                        &[HWND_DESKTOP, 0, LCOLF_CONSECRGB, SYSCLR_HELPBACKGROUND as u32, 3, DATA]), 1);
        assert_eq!([query(&loader, SYSCLR_HELPBACKGROUND), query(&loader, SYSCLR_HELPTEXT),
                    query(&loader, SYSCLR_HELPHILITE)], [0x11, 0x22, 0x33]);
        // LCOL_RESET first, then the data.
        put_data(&loader, &[SYSCLR_MENU as u32, 0x0000_00FF]);
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS, &[HWND_DESKTOP, LCOL_RESET, LCOLF_INDRGB, 0, 2, DATA]), 1);
        assert_eq!(query(&loader, SYSCLR_WINDOW), 0x00FF_FFFF, "reset");
        assert_eq!(query(&loader, SYSCLR_HELPHILITE), 0x0000_8080, "reset");
        assert_eq!(query(&loader, SYSCLR_MENU), 0x0000_00FF, "set after the reset");
        // A reset alone needs no data.
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS, &[HWND_DESKTOP, LCOL_RESET, LCOLF_INDRGB, 0, 0, 0]), 1);
        assert_eq!(query(&loader, SYSCLR_MENU), 0x00D4_D0C8);
    }

    #[test]
    fn set_rejects_bad_arguments_and_changes_nothing() {
        let loader = Loader::new_mock();
        let err = |code| error_id(SEVERITY_ERROR, code);
        put_data(&loader, &[SYSCLR_WINDOW as u32, 0x0012_3456, 7, 0x0000_0001]);
        // Second pair names no system colour: nothing is set.
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS, &[HWND_DESKTOP, 0, LCOLF_INDRGB, 0, 4, DATA]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(query(&loader, SYSCLR_WINDOW), 0x00FF_FFFF);
        // An odd count of index/RGB entries.
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS, &[HWND_DESKTOP, 0, LCOLF_INDRGB, 0, 3, DATA]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        // Consecutive values running past SYSCLR_HELPHILITE.
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS,
                        &[HWND_DESKTOP, 0, LCOLF_CONSECRGB, SYSCLR_HELPHILITE as u32, 2, DATA]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        // Unknown option bits and formats.
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS, &[HWND_DESKTOP, 0x80, LCOLF_INDRGB, 0, 2, DATA]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_FLAG));
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS, &[HWND_DESKTOP, 0, LCOLF_RGB, 0, 2, DATA]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_FLAG));
        // Data that is not in guest memory, too much data, a bad handle.
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS, &[HWND_DESKTOP, 0, LCOLF_INDRGB, 0, 2, 0]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS,
                        &[HWND_DESKTOP, 0, LCOLF_INDRGB, 0, 2, u32::MAX - 3]), 0);
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS,
                        &[HWND_DESKTOP, 0, LCOLF_INDRGB, 0, MAX_SYS_COLOR_DATA + 2, DATA]), 0);
        assert_eq!(last_error(&loader), err(PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS, &[0xDEAD, 0, LCOLF_INDRGB, 0, 2, DATA]), 0);
        assert_eq!(last_error(&loader), err(PMERR_INVALID_HWND));
        assert_eq!(query(&loader, SYSCLR_WINDOW), 0x00FF_FFFF);
    }

    #[test]
    fn a_change_is_posted_to_main_windows_and_repaints_frames() {
        let loader = Loader::new_mock();
        let (hmq, frame, client, child) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            let child = wm.create_window("WC_BUTTON".into(), client, hmq);
            wm.z_push_top(frame);
            (hmq, frame, client, child)
        };
        put_data(&loader, &[SYSCLR_WINDOW as u32, 0x0001_0203]);
        assert_eq!(call(&loader, ORD_WIN_SET_SYS_COLORS,
                        &[HWND_DESKTOP, LCOL_PURECOLOR, LCOLF_INDRGB, 0, 2, DATA]), 1);
        let wm = loader.shared.window_mgr.lock_or_recover();
        let msgs: Vec<OS2Message> = wm.get_mq(hmq).unwrap().lock_or_recover().messages.iter().cloned().collect();
        let change: Vec<_> = msgs.iter().filter(|m| m.msg == WM_SYSCOLORCHANGE).collect();
        assert_eq!(change.len(), 1, "one main window");
        assert_eq!((change[0].hwnd, change[0].mp1, change[0].mp2), (client, LCOL_PURECOLOR, 0),
                   "the frame's message goes to its client, mp1 = flOptions");
        assert!(msgs.iter().any(|m| m.msg == WM_PAINT && m.hwnd == client), "repainted");
        assert!(!msgs.iter().any(|m| m.hwnd == child), "children get nothing");
        assert!(wm.dirty.contains(&frame));
    }

    #[test]
    fn sysclr_values_draw_in_the_system_colour() {
        let loader = Loader::new_mock();
        assert_eq!(loader.map_color(SYSCLR_WINDOW as u32), 0x00FF_FFFF);
        loader.shared.window_mgr.lock_or_recover().sys_colors.set(SYSCLR_WINDOW, 0x0000_FF00);
        assert_eq!(loader.map_color(SYSCLR_WINDOW as u32), 0x0000_FF00);
        assert_eq!(loader.map_color(CLR_BLACK as u32), 0, "other values as before");
        // A presentation space resolves them too.
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hps = wm.create_ps(0);
        let ps = wm.ps_map.get_mut(&hps).unwrap();
        ps.set_back_color(SYSCLR_WINDOW as u32);
        assert_eq!(ps.back_color, 0x0000_FF00);
        assert_eq!(ps.back_color_attr, SYSCLR_WINDOW as u32, "the value as set is kept");
    }
}
