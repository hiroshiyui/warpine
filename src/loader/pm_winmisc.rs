// SPDX-License-Identifier: GPL-3.0-or-later

//! Smaller PMWIN calls: `WinLoadMessage` (and the string-bundle reading it
//! shares with `WinLoadString`), `WinQueryVersion`, `WinSetWindowBits`,
//! `WinSetDlgItemShort` and `WinQueryDlgItemShort`.
//!
//! Behaviour follows the PM Reference pages of the calls:
//! - `RT_STRING` and `RT_MESSAGE` resources hold bundles of 16 strings:
//!   string `id` is entry `id % 16` of bundle `id / 16 + 1`. A bundle is a
//!   USHORT codepage followed by 16 entries of a length byte and the
//!   string (Open Watcom `wrc` counts the terminating NUL in the length and
//!   writes it). `WinLoadString` / `WinLoadMessage` copy at most
//!   `cchMax - 1` bytes plus a NUL and return the length copied.
//! - `WinQueryVersion` returns the version in the low byte and the release
//!   in the next byte (Warpine reports OS/2 Warp 4.5, as
//!   `DosQuerySysInfo(QSV_VERSION_MAJOR / MINOR)`).
//! - `WinSetWindowBits(hwnd, index, flData, flMask)` sets the `flMask`
//!   bits of the ULONG window word `index` to those of `flData`.
//! - `WinSetDlgItemShort` sets a dialog item's text to a number, signed or
//!   unsigned; `WinQueryDlgItemShort` reads it back, FALSE when the text is
//!   not a number of that kind.
//!
//! Unverified (Warpine's own choices): `WinQueryDlgItemShort` skips blanks
//! around the number, takes a leading `+` (and `-` when signed), fails on
//! anything else or out of range, and leaves `*psResult` alone when it
//! fails; a missing string or bundle fails with `PMERR_RESOURCE_NOT_FOUND`.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::WindowManager;
use super::{ApiResult, Loader};

pub use super::res_templates::{bundle_string, STRINGS_PER_BUNDLE};

/// Bundle resource id and index in it of string `id`.
pub fn bundle_of(id: u32) -> (u16, usize) {
    ((id / STRINGS_PER_BUNDLE + 1) as u16, (id % STRINGS_PER_BUNDLE) as usize)
}

/// The text `WinSetDlgItemShort` puts in a dialog item.
pub fn short_text(value: u16, signed: bool) -> String {
    if signed { (value as i16).to_string() } else { value.to_string() }
}

/// The value of a dialog item's text for `WinQueryDlgItemShort`, as the
/// 16 bits stored to `*psResult`.
pub fn parse_short(text: &str, signed: bool) -> Option<u16> {
    let t = text.trim_matches(|c: char| c == ' ' || c == '\t');
    let (negative, digits) = match t.as_bytes().first() {
        Some(b'-') if signed => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) { return None; }
    // Long digit strings overflow i64 parsing: they are out of range anyway.
    let magnitude: i64 = digits.parse().ok()?;
    let v = if negative { -magnitude } else { magnitude };
    let (min, max) = if signed { (i16::MIN as i64, i16::MAX as i64) } else { (0, u16::MAX as i64) };
    (min..=max).contains(&v).then_some(v as u16)
}

impl WindowManager {
    /// Set the `mask` bits of ULONG window word `index` of `hwnd` to those
    /// of `data`; `QWL_STYLE` is the window style (`WS_VISIBLE` included).
    pub fn set_window_bits(&mut self, hwnd: u32, index: i32, data: u32, mask: u32) -> bool {
        let Some(w) = self.get_window_mut(hwnd) else { return false };
        if index == QWL_STYLE {
            let current = (w.style & !WS_VISIBLE) | if w.visible { WS_VISIBLE } else { 0 };
            let style = (current & !mask) | (data & mask);
            w.style = style;
            w.visible = style & WS_VISIBLE != 0;
            self.mark_dirty(hwnd);
        } else {
            let v = w.window_ulong.entry(index).or_insert(0);
            *v = (*v & !mask) | (data & mask);
        }
        true
    }
}

impl Loader {
    /// String `id` of the `rt` bundles (`RT_STRING`, `RT_MESSAGE`) of
    /// module `hmod`, copied to `buf` as `WinLoadString` does. Returns the
    /// length copied, or `None` without such a string.
    pub(super) fn load_bundle_string(&self, hmod: u32, rt: u16, id: u32, cch_max: i32, buf: u32) -> Option<u32> {
        let (bundle, index) = bundle_of(id);
        let found = self.shared.resource_mgr.lock_or_recover().find(hmod, rt, bundle);
        let text = found
            .and_then(|(addr, size)| self.guest_read_bytes(addr, size as usize))
            .and_then(|data| bundle_string(&data, index).map(<[u8]>::to_vec));
        let copied = match (&text, cch_max) {
            (Some(t), n) if n > 0 && buf != 0 => {
                let len = t.len().min(n as usize - 1);
                self.guest_write_bytes(buf, &t[..len])?;
                self.guest_write::<u8>(buf.wrapping_add(len as u32), 0)?;
                len as u32
            }
            _ => {
                if cch_max > 0 && buf != 0 { self.guest_write::<u8>(buf, 0); }
                0
            }
        };
        text.map(|_| copied)
    }

    /// `WinLoadString` (`RT_STRING`) / `WinLoadMessage` (`RT_MESSAGE`).
    pub(super) fn win_load_bundle_string(&self, tid: u32, rt: u16, hmod: u32, id: u32, cch_max: i32, buf: u32) -> ApiResult {
        match self.load_bundle_string(hmod, rt, id, cch_max, buf) {
            Some(n) => ApiResult::Normal(n),
            None => self.pm_fail(tid, PMERR_RESOURCE_NOT_FOUND),
        }
    }

    /// `WinQueryVersion(hab)`.
    pub(super) fn win_query_version(&self) -> ApiResult {
        ApiResult::Normal(OS2_VERSION_MAJOR | (OS2_VERSION_MINOR << 8))
    }

    /// `WinSetWindowBits(hwnd, index, flData, flMask)` (and
    /// `WinSetWindowULong`, which sets all bits).
    pub(super) fn win_set_window_bits(&self, tid: u32, hwnd: u32, index: i32, data: u32, mask: u32) -> ApiResult {
        if !self.pm_check_window_word(tid, hwnd, index, 4) { return ApiResult::Normal(0); }
        let ok = self.shared.window_mgr.lock_or_recover().set_window_bits(hwnd, index, data, mask);
        ApiResult::Normal(ok as u32)
    }

    /// Set the text of item `id` of `hwnd_dlg` (`WinSetDlgItemText`); a
    /// missing item is no error.
    /// False when the dialog has no item `id`.
    pub(super) fn set_dlg_item_text(&self, hwnd_dlg: u32, id: u32, text: String) -> bool {
        let target = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(child) = wm.find_child_by_id(hwnd_dlg, id) else { return false };
            let target = wm.text_window(child);
            if let Some(win) = wm.get_window_mut(target) {
                win.text = text;
            }
            (target != child).then_some(target)
        };
        // The field of a combination box / spin button shows it.
        if let Some(field) = target {
            self.set_field_text_done(field);
        }
        true
    }

    /// `WinSetDlgItemShort(hwndDlg, idItem, usValue, fSigned)`.
    pub(super) fn win_set_dlg_item_short(&self, tid: u32, hwnd_dlg: u32, id: u32, value: u16, signed: bool) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd_dlg, false) { return ApiResult::Normal(0); }
        self.set_dlg_item_text(hwnd_dlg, id, short_text(value, signed));
        ApiResult::Normal(1)
    }

    /// `WinQueryDlgItemShort(hwndDlg, idItem, psResult, fSigned)`.
    pub(super) fn win_query_dlg_item_short(&self, tid: u32, hwnd_dlg: u32, id: u32, presult: u32, signed: bool) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd_dlg, false) { return ApiResult::Normal(0); }
        let text = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(child) = wm.find_child_by_id(hwnd_dlg, id) else { return ApiResult::Normal(0) };
            wm.get_window(wm.text_window(child)).map(|w| w.text.clone()).unwrap_or_default()
        };
        match parse_short(&text, signed) {
            Some(v) if presult != 0 && self.guest_write::<u16>(presult, v).is_some() => ApiResult::Normal(1),
            _ => ApiResult::Normal(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_error::error_id;
    use super::super::vm_backend::mock::MockVcpu;
    use crate::lx::header::{RT_MESSAGE, RT_STRING};

    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_LOAD_MESSAGE: u32 = 779;
    const ORD_WIN_LOAD_STRING: u32 = 781;
    const ORD_WIN_QUERY_DLG_ITEM_SHORT: u32 = 814;
    const ORD_WIN_QUERY_VERSION: u32 = 833;
    const ORD_WIN_QUERY_WINDOW_ULONG: u32 = 843;
    const ORD_WIN_SET_DLG_ITEM_SHORT: u32 = 858;
    const ORD_WIN_SET_WINDOW_BITS: u32 = 874;
    const ORD_WIN_SET_WINDOW_ULONG: u32 = 878;
    const RES: u32 = 0x4000;
    const BUF: u32 = 0x2000;

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

    /// A bundle as `wrc` writes it: codepage 850, entries with the NUL
    /// counted; `strings` by index, the others empty.
    fn bundle(strings: &[(usize, &str)]) -> Vec<u8> {
        let mut out = vec![0x52, 0x03];
        for i in 0..STRINGS_PER_BUNDLE as usize {
            let s = strings.iter().find(|(j, _)| *j == i).map_or("", |(_, s)| s);
            out.push(s.len() as u8 + 1);
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn bundles_hold_sixteen_strings_after_a_codepage() {
        let b = bundle(&[(5, "str five"), (15, "last")]);
        assert_eq!(bundle_string(&b, 5), Some(&b"str five"[..]));
        assert_eq!(bundle_string(&b, 15), Some(&b"last"[..]));
        assert_eq!(bundle_string(&b, 0), Some(&b""[..]));
        assert_eq!(bundle_string(&b, 16), None);
        assert_eq!(bundle_string(&b[..20], 15), None, "cut short");
        assert_eq!(bundle_string(&[], 0), None);
        assert_eq!(bundle_of(5), (1, 5));
        assert_eq!(bundle_of(17), (2, 1));
    }

    #[test]
    fn load_message_and_load_string_read_their_bundles() {
        let loader = Loader::new_mock();
        let msgs = bundle(&[(1, "Seventeen")]);
        let strs = bundle(&[(5, "str five")]);
        loader.guest_write_bytes(RES, &msgs).unwrap();
        loader.guest_write_bytes(RES + 0x100, &strs).unwrap();
        {
            let mut rm = loader.shared.resource_mgr.lock_or_recover();
            rm.add(0, RT_MESSAGE, 2, RES, msgs.len() as u32);
            rm.add(0, RT_STRING, 1, RES + 0x100, strs.len() as u32);
        }
        assert_eq!(call(&loader, ORD_WIN_LOAD_MESSAGE, &[MOCK_HAB, 0, 17, 64, BUF]), 9);
        assert_eq!(loader.guest_read_bytes(BUF, 10).unwrap(), b"Seventeen\0");
        // Cut to cchMax - 1.
        assert_eq!(call(&loader, ORD_WIN_LOAD_MESSAGE, &[MOCK_HAB, 0, 17, 4, BUF]), 3);
        assert_eq!(loader.guest_read_bytes(BUF, 4).unwrap(), b"Sev\0");
        assert_eq!(call(&loader, ORD_WIN_LOAD_STRING, &[MOCK_HAB, 0, 5, 64, BUF]), 8);
        assert_eq!(loader.guest_read_bytes(BUF, 9).unwrap(), b"str five\0");
        // A message is not a string, and a missing bundle is an error.
        assert_eq!(call(&loader, ORD_WIN_LOAD_STRING, &[MOCK_HAB, 0, 17, 64, BUF]), 0);
        assert_eq!(loader.guest_read::<u8>(BUF), Some(0));
        assert_eq!(call(&loader, ORD_WIN_GET_LAST_ERROR, &[MOCK_HAB]), error_id(SEVERITY_ERROR, PMERR_RESOURCE_NOT_FOUND));
        assert_eq!(call(&loader, ORD_WIN_LOAD_MESSAGE, &[MOCK_HAB, 0, 17, 0, BUF]), 0, "no room");
    }

    #[test]
    fn version_is_warp_4_5() {
        let loader = Loader::new_mock();
        assert_eq!(call(&loader, ORD_WIN_QUERY_VERSION, &[MOCK_HAB]), 20 | (45 << 8));
    }

    #[test]
    fn shorts_round_trip_through_text() {
        assert_eq!(short_text(0xFFFF, true), "-1");
        assert_eq!(short_text(0xFFFF, false), "65535");
        assert_eq!(parse_short(" -32768 ", true), Some(0x8000));
        assert_eq!(parse_short("+12", false), Some(12));
        assert_eq!(parse_short("-1", false), None);
        assert_eq!(parse_short("32768", true), None);
        assert_eq!(parse_short("65535", false), Some(0xFFFF));
        assert_eq!(parse_short("65536", false), None);
        assert_eq!(parse_short("99999999999999999999999", false), None);
        assert_eq!(parse_short("12a", true), None);
        assert_eq!(parse_short("", true), None);
        assert_eq!(parse_short("-", true), None);
    }

    #[test]
    fn dialog_item_shorts() {
        let loader = Loader::new_mock();
        let (dlg, item) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let dlg = wm.create_window("App".into(), HWND_DESKTOP, 0);
            let item = wm.create_window("WC_ENTRYFIELD".into(), dlg, 0);
            wm.get_window_mut(item).unwrap().id = 42;
            (dlg, item)
        };
        assert_eq!(call(&loader, ORD_WIN_SET_DLG_ITEM_SHORT, &[dlg, 42, 0xFFFE, 1]), 1);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().get_window(item).unwrap().text, "-2");
        loader.guest_write::<u16>(BUF, 0).unwrap();
        assert_eq!(call(&loader, ORD_WIN_QUERY_DLG_ITEM_SHORT, &[dlg, 42, BUF, 1]), 1);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(0xFFFE));
        assert_eq!(call(&loader, ORD_WIN_QUERY_DLG_ITEM_SHORT, &[dlg, 42, BUF, 0]), 0, "negative, unsigned");
        assert_eq!(call(&loader, ORD_WIN_QUERY_DLG_ITEM_SHORT, &[dlg, 7, BUF, 1]), 0, "no such item");
        assert_eq!(call(&loader, ORD_WIN_SET_DLG_ITEM_SHORT, &[dlg, 42, 40000, 0]), 1);
        assert_eq!(call(&loader, ORD_WIN_QUERY_DLG_ITEM_SHORT, &[dlg, 42, BUF, 0]), 1);
        assert_eq!(loader.guest_read::<u16>(BUF), Some(40000));
        assert_eq!(call(&loader, ORD_WIN_QUERY_DLG_ITEM_SHORT, &[dlg, 42, BUF, 1]), 0, "out of the signed range");
        assert_eq!(call(&loader, ORD_WIN_SET_DLG_ITEM_SHORT, &[0xDEAD, 42, 1, 0]), 0);
    }

    #[test]
    fn window_bits_change_only_the_masked_bits() {
        let loader = Loader::new_mock();
        let (frame, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let frame = wm.create_window("App".into(), HWND_DESKTOP, 0);
            let client = wm.create_window("App".into(), frame, 0);
            wm.frame_to_client.insert(frame, client);
            wm.get_window_mut(client).unwrap().visible = true;
            (frame, client)
        };
        let _ = frame;
        assert_eq!(call(&loader, ORD_WIN_SET_WINDOW_BITS, &[client, QWL_STYLE as u32, WS_DISABLED | 0x10, WS_DISABLED]), 1);
        let style = call(&loader, ORD_WIN_QUERY_WINDOW_ULONG, &[client, QWL_STYLE as u32]);
        assert_eq!(style, WS_VISIBLE | WS_DISABLED, "the unmasked bit stays clear");
        assert_eq!(call(&loader, ORD_WIN_SET_WINDOW_BITS, &[client, QWL_STYLE as u32, 0, WS_VISIBLE]), 1);
        assert!(!loader.shared.window_mgr.lock_or_recover().get_window(client).unwrap().visible);
        // A window word of the frame (frames take any word).
        call(&loader, ORD_WIN_SET_WINDOW_ULONG, &[frame, 0, 0xF0F0]);
        assert_eq!(call(&loader, ORD_WIN_SET_WINDOW_BITS, &[frame, 0, 0x0FFF, 0x00FF]), 1);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_ULONG, &[frame, 0]), 0xF0FF);
        // WinSetWindowULong(QWL_STYLE) sets the style too.
        call(&loader, ORD_WIN_SET_WINDOW_ULONG, &[client, QWL_STYLE as u32, WS_VISIBLE]);
        assert_eq!(call(&loader, ORD_WIN_QUERY_WINDOW_ULONG, &[client, QWL_STYLE as u32]), WS_VISIBLE);
        assert_eq!(call(&loader, ORD_WIN_SET_WINDOW_BITS, &[0xDEAD, 0, 1, 1]), 0);
        assert_eq!(call(&loader, ORD_WIN_SET_WINDOW_BITS, &[client, (QWL_MIN - 1) as u32, 1, 1]), 0);
    }
}
