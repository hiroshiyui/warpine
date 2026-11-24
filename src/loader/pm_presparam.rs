// SPDX-License-Identifier: GPL-3.0-or-later

//! Presentation parameters: `WinSetPresParam`, `WinQueryPresParam`,
//! `WinRemovePresParam`, the `PRESPARAMS` blocks of `WinCreateWindow` and
//! dialog templates, and what the built-in controls take from them.
//!
//! A presentation parameter is an attribute id (`PP_*`, or an application's
//! own from `PP_USER`) with a value of bytes, kept per window
//! ([`PresParams`] in `OS2Window::pres_params`). Following the PM
//! Reference (WinQueryPresParam), a window without a value inherits its
//! owner's, and so on up the owner chain, unless `QPF_NOINHERIT` is given;
//! a frame's client counts as owned by its frame. `QPF_ID1COLORINDEX` /
//! `QPF_ID2COLORINDEX` turn a found colour index (`CLR_*` / `SYSCLR_*`) into
//! the RGB value it stands for; `QPF_PURERGBCOLOR` asks for a pure colour,
//! which every colour of Warpine's 24-bit display is.
//!
//! A change (set or remove) is announced with `WM_PRESPARAMCHANGED`
//! (mp1 = the attribute id) to the window and to every window that inherits
//! the attribute from it. Warpine *posts* the message (PM sends it): the
//! change is made before the windows hear of it either way. Built-in
//! controls and dialogs repaint (they are invalidated); frames recomposite.
//!
//! Storage is bounded: at most [`MAX_PRESPARAMS`] attributes per window, at
//! most [`MAX_PRESPARAM_LEN`] bytes per value, and a `PRESPARAMS` block is
//! read up to [`MAX_PRESPARAMS_BLOCK`] bytes.
//!
//! The built-in controls honour `PP_FOREGROUNDCOLOR` / `PP_BACKGROUNDCOLOR`
//! (and their `*INDEX` forms) and `PP_FONTNAMESIZE` ("10.Helv": a size in
//! points, a dot, a face name mapped through `FontSpec::from_facename`).

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::{default_color, WindowManager};
use super::{ApiResult, Loader};
use crate::gui::font::FontSpec;

// ── <pmwin.h> ────────────────────────────────────────────────────────────────

/// Sent (posted, in Warpine) when a presentation parameter changes; mp1 = id.
pub const WM_PRESPARAMCHANGED: u32 = 0x002f;

pub const PP_FOREGROUNDCOLOR: u32 = 1;
pub const PP_FOREGROUNDCOLORINDEX: u32 = 2;
pub const PP_BACKGROUNDCOLOR: u32 = 3;
pub const PP_BACKGROUNDCOLORINDEX: u32 = 4;
pub const PP_HILITEFOREGROUNDCOLOR: u32 = 5;
pub const PP_HILITEBACKGROUNDCOLOR: u32 = 7;
pub const PP_BORDERCOLOR: u32 = 13;
pub const PP_FONTNAMESIZE: u32 = 15;
pub const PP_BUTTONBACKGROUNDCOLOR: u32 = 58;
/// The first attribute id for an application's own use.
#[cfg_attr(not(test), allow(dead_code))]
pub const PP_USER: u32 = 0x8000;

pub const QPF_NOINHERIT: u32 = 0x0001;
pub const QPF_ID1COLORINDEX: u32 = 0x0002;
pub const QPF_ID2COLORINDEX: u32 = 0x0004;
/// Accepted without effect: every colour of the 24-bit display is pure.
#[cfg_attr(not(test), allow(dead_code))]
pub const QPF_PURERGBCOLOR: u32 = 0x0008;
pub const QPF_VALIDFLAGS: u32 = 0x000F;

// ── Bounds ───────────────────────────────────────────────────────────────────

// The attribute bounds and the PRESPARAMS parser live with the other
// resource parsers (res_templates.rs, fuzzed).
pub use super::res_templates::{parse_presparams, MAX_PRESPARAMS, MAX_PRESPARAM_LEN};
/// Most bytes of a `PRESPARAMS` block read from the guest (`cb` included).
pub const MAX_PRESPARAMS_BLOCK: usize = 16 * 1024;
/// Longest owner chain followed for inheritance (owner loops end here too).
const MAX_OWNER_DEPTH: usize = 64;
/// Size of `PRESPARAMS.cb` and of `PARAM.id` / `PARAM.cb`.
const ULONG_LEN: usize = 4;

/// Size in points to a line height in pels: 96 dpi (4/3 pel per point)
/// times a 1.2 line spacing, so "10.Helv" is the 16-pel system font.
/// Unverified: PM picks the nearest bitmap or outline font size instead.
const LINE_PX_PER_POINT_NUM: u32 = 8;
const LINE_PX_PER_POINT_DEN: u32 = 5;
/// Largest point size accepted in `PP_FONTNAMESIZE`.
pub(super) const MAX_FONT_POINTS: u32 = 120;

/// The presentation parameters of one window, in the order they were set.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PresParams {
    entries: Vec<(u32, Vec<u8>)>,
}

impl PresParams {
    /// The value of attribute `id`.
    pub fn get(&self, id: u32) -> Option<&[u8]> {
        self.entries.iter().find(|(i, _)| *i == id).map(|(_, v)| v.as_slice())
    }

    /// Set attribute `id` to `value`. False (nothing changed) when the value
    /// is too long or the window already has [`MAX_PRESPARAMS`] others.
    pub fn set(&mut self, id: u32, value: &[u8]) -> bool {
        if value.len() > MAX_PRESPARAM_LEN {
            return false;
        }
        if let Some(e) = self.entries.iter_mut().find(|(i, _)| *i == id) {
            e.1 = value.to_vec();
            return true;
        }
        if self.entries.len() >= MAX_PRESPARAMS {
            return false;
        }
        self.entries.push((id, value.to_vec()));
        true
    }

    /// Remove attribute `id`; whether it was there.
    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(i, _)| *i != id);
        self.entries.len() != before
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A `PP_FONTNAMESIZE` value ("10.Helv", NUL-terminated or not) as the font
/// to draw in: the points before the dot give the line height, the rest is
/// the face name (an attribute such as "Helv.Bold" makes it bold through
/// `FontSpec::from_facename`). `None` for a malformed value.
pub fn parse_font_name_size(value: &[u8]) -> Option<FontSpec> {
    let value = &value[..value.iter().position(|&b| b == 0).unwrap_or(value.len())];
    let text = std::str::from_utf8(value).ok()?;
    let (points, face) = text.split_once('.')?;
    let points: u32 = points.trim().parse().ok()?;
    if points == 0 || points > MAX_FONT_POINTS || face.trim().is_empty() {
        return None;
    }
    let px = (points * LINE_PX_PER_POINT_NUM).div_ceil(LINE_PX_PER_POINT_DEN);
    Some(FontSpec::from_facename(face.trim(), false, false, px))
}

/// The `*INDEX` attribute that goes with RGB colour attribute `id`, for the
/// pairs the built-in controls use.
fn index_attr(id: u32) -> Option<u32> {
    match id {
        PP_FOREGROUNDCOLOR => Some(PP_FOREGROUNDCOLORINDEX),
        PP_BACKGROUNDCOLOR => Some(PP_BACKGROUNDCOLORINDEX),
        // Every other pair of <pmwin.h> has the index form right after.
        PP_HILITEFOREGROUNDCOLOR | PP_HILITEBACKGROUNDCOLOR | PP_BORDERCOLOR => Some(id + 1),
        // PP_BUTTONBACKGROUNDCOLOR and the other Warp 4 colours have no
        // index form.
        _ => None,
    }
}

fn le_u32(v: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(v.get(..ULONG_LEN)?.try_into().ok()?))
}

impl WindowManager {
    /// The window `hwnd` inherits presentation parameters from: its owner,
    /// a frame for its client; 0 = none.
    fn pres_owner(&self, hwnd: u32) -> u32 {
        let Some(w) = self.get_window(hwnd) else { return 0 };
        if w.owner != 0 && w.owner != hwnd {
            return w.owner;
        }
        self.frame_to_client.iter().find(|&(_, &c)| c == hwnd).map_or(0, |(&f, _)| f)
    }

    /// WinQueryPresParam's search: `id1` then `id2` (0 = none) on `hwnd`,
    /// then on its owners unless `QPF_NOINHERIT`. Returns the id found and
    /// its value, a colour index already turned into RGB when the flags ask.
    pub fn query_pres_param(&self, hwnd: u32, id1: u32, id2: u32, flags: u32) -> Option<(u32, Vec<u8>)> {
        let mut w = hwnd;
        for _ in 0..MAX_OWNER_DEPTH {
            let pp = &self.get_window(w)?.pres_params;
            for (id, as_index) in [(id1, flags & QPF_ID1COLORINDEX != 0), (id2, flags & QPF_ID2COLORINDEX != 0)] {
                if id == 0 { continue; }
                if let Some(v) = pp.get(id) {
                    let value = match (as_index, le_u32(v)) {
                        (true, Some(index)) => self.index_rgb(index).to_le_bytes().to_vec(),
                        _ => v.to_vec(),
                    };
                    return Some((id, value));
                }
            }
            if flags & QPF_NOINHERIT != 0 { return None; }
            w = self.pres_owner(w);
            if w == 0 || w == HWND_DESKTOP { return None; }
        }
        None
    }

    /// A colour index (`CLR_*`, `SYSCLR_*`) as RGB.
    fn index_rgb(&self, index: u32) -> u32 {
        self.sys_colors.get(index as i32).unwrap_or_else(|| default_color(index))
    }

    /// The RGB colour of `hwnd` for colour attribute `rgb_id` (or its index
    /// form), inherited unless `own_only`.
    pub fn pres_color(&self, hwnd: u32, rgb_id: u32, own_only: bool) -> Option<u32> {
        let flags = QPF_ID2COLORINDEX | if own_only { QPF_NOINHERIT } else { 0 };
        let (_, v) = self.query_pres_param(hwnd, rgb_id, index_attr(rgb_id).unwrap_or(0), flags)?;
        le_u32(&v).map(|rgb| rgb & 0x00FF_FFFF)
    }

    /// The font of `hwnd`'s `PP_FONTNAMESIZE`, inherited.
    pub fn pres_font(&self, hwnd: u32) -> Option<FontSpec> {
        let (_, v) = self.query_pres_param(hwnd, PP_FONTNAMESIZE, 0, 0)?;
        parse_font_name_size(&v)
    }

    /// The windows that hear of a change of attribute `id` on `hwnd`: it and
    /// every window that inherits `id` from it (no value of its own on the
    /// way up).
    pub fn pres_param_audience(&self, hwnd: u32, id: u32) -> Vec<u32> {
        let mut out = vec![hwnd];
        let mut all: Vec<u32> = self.windows().map(|w| w.handle).filter(|&h| h != hwnd).collect();
        all.sort_unstable();
        'windows: for h in all {
            let mut w = h;
            for _ in 0..MAX_OWNER_DEPTH {
                if w == hwnd {
                    out.push(h);
                    continue 'windows;
                }
                match self.get_window(w) {
                    Some(win) if win.pres_params.get(id).is_none() => w = self.pres_owner(w),
                    _ => continue 'windows,
                }
                if w == 0 { continue 'windows; }
            }
        }
        out
    }

    /// Tell `hwnd` and the windows inheriting from it that attribute `id`
    /// changed: post `WM_PRESPARAMCHANGED` to those with a window procedure
    /// (not frames: their client hears of it), repaint built-in controls and
    /// dialogs, recomposite frames.
    pub fn announce_pres_param(&mut self, hwnd: u32, id: u32) {
        for h in self.pres_param_audience(hwnd, id) {
            let Some(w) = self.get_window(h) else { continue };
            let (proc_, builtin_paint) = (w.pfn_wp != 0, w.pfn_wp == 0 || self.is_dialog(h));
            if self.is_frame(h) && !self.is_dialog(h) {
                self.mark_dirty(h);
                continue;
            }
            if proc_ {
                self.post_msg(h, WM_PRESPARAMCHANGED, id, 0);
            }
            if builtin_paint {
                self.invalidate(h, None, true);
            }
        }
    }
}

impl Loader {
    /// WinSetPresParam(HWND hwnd, ULONG id, ULONG cbParam, PVOID pbParam) -> BOOL
    pub(super) fn win_set_pres_param(&self, tid: u32, hwnd: u32, id: u32, cb: u32, pb: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        // Unverified: PM's limits; id 0 names no attribute.
        if id == 0 || cb as usize > MAX_PRESPARAM_LEN {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        let value = if cb == 0 {
            Vec::new()
        } else {
            match self.guest_read_bytes(pb, cb as usize).filter(|_| pb != 0) {
                Some(v) => v,
                None => return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE),
            }
        };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let stored = wm.get_window_mut(hwnd).is_some_and(|w| w.pres_params.set(id, &value));
        if !stored {
            drop(wm);
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        wm.announce_pres_param(hwnd, id);
        ApiResult::Normal(1)
    }

    /// WinQueryPresParam(HWND hwnd, ULONG id1, ULONG id2, PULONG pulId,
    /// ULONG cbBuf, PVOID pbBuf, ULONG fs) -> ULONG bytes returned.
    /// A value longer than `cbBuf` is cut to it (unverified).
    pub(super) fn win_query_pres_param(&self, tid: u32, args: [u32; 7]) -> ApiResult {
        let [hwnd, id1, id2, pul_id, cb_buf, pb_buf, flags] = args;
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        if flags & !QPF_VALIDFLAGS != 0 { return self.pm_fail(tid, PMERR_INVALID_FLAG); }
        let found = self.shared.window_mgr.lock_or_recover().query_pres_param(hwnd, id1, id2, flags);
        let Some((id, value)) = found else { return ApiResult::Normal(0) };
        let n = value.len().min(cb_buf as usize);
        if n > 0 && (pb_buf == 0 || self.guest_write_bytes(pb_buf, &value[..n]).is_none()) {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        if pul_id != 0 && self.guest_write::<u32>(pul_id, id).is_none() {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        ApiResult::Normal(n as u32)
    }

    /// WinRemovePresParam(HWND hwnd, ULONG id) -> BOOL: FALSE when the
    /// window has no such attribute of its own (unverified).
    pub(super) fn win_remove_pres_param(&self, tid: u32, hwnd: u32, id: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, hwnd, false) { return ApiResult::Normal(0); }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let removed = wm.get_window_mut(hwnd).is_some_and(|w| w.pres_params.remove(id));
        if removed {
            wm.announce_pres_param(hwnd, id);
        }
        ApiResult::Normal(removed as u32)
    }

    /// Give `hwnd` the attributes of the `PRESPARAMS` block at guest `addr`
    /// (0 = none), as `WinCreateWindow` does with `pPresParams`. No
    /// `WM_PRESPARAMCHANGED` is sent for them.
    pub(super) fn apply_guest_presparams(&self, hwnd: u32, addr: u32) {
        if addr == 0 { return; }
        let Some(cb) = self.guest_read::<u32>(addr) else { return };
        let len = (cb as usize).saturating_add(ULONG_LEN).min(MAX_PRESPARAMS_BLOCK);
        let Some(block) = self.guest_read_bytes(addr, len) else { return };
        self.apply_presparams(hwnd, &parse_presparams(&block));
    }

    /// Give `hwnd` the attributes `params` (a dialog item's).
    pub(super) fn apply_presparams(&self, hwnd: u32, params: &[(u32, Vec<u8>)]) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if let Some(w) = wm.get_window_mut(hwnd) {
            for (id, v) in params {
                w.pres_params.set(*id, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_error::error_id;
    use super::super::pm_types::OS2Message;
    use super::super::vm_backend::mock::MockVcpu;
    use crate::gui::font::FontFamily;

    const ORD_WIN_SET_PRES_PARAM: u32 = 938;
    const ORD_WIN_QUERY_PRES_PARAM: u32 = 939;
    const ORD_WIN_REMOVE_PRES_PARAM: u32 = 940;
    const ORD_WIN_CREATE_MSG_QUEUE: u32 = 716;
    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const BUF: u32 = 0x3000;
    const OUT_ID: u32 = 0x3100;
    const VALUE: u32 = 0x3200;
    const CLR_BLUE: i32 = 1;

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

    fn block(params: &[(u32, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (id, v) in params {
            body.extend_from_slice(&id.to_le_bytes());
            body.extend_from_slice(&(v.len() as u32).to_le_bytes());
            body.extend_from_slice(v);
        }
        let mut out = (body.len() as u32).to_le_bytes().to_vec();
        out.extend(body);
        out
    }

    /// A queue; window `dlg` (procedure 0x5000) owning built-in `ctl`
    /// (a WC_STATIC), and an unrelated window `other`.
    fn setup() -> (Loader, u32, u32, u32, u32) {
        let loader = Loader::new_mock();
        let hmq = call(&loader, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let dlg = wm.create_window("App".into(), 0, hmq);
        wm.get_window_mut(dlg).unwrap().pfn_wp = 0x5000;
        let ctl = wm.create_window("WC_STATIC".into(), dlg, hmq);
        let w = wm.get_window_mut(ctl).unwrap();
        (w.owner, w.cx, w.cy, w.visible) = (dlg, 40, 16, true);
        let other = wm.create_window("App".into(), 0, hmq);
        drop(wm);
        (loader, hmq, dlg, ctl, other)
    }

    fn set(loader: &Loader, hwnd: u32, id: u32, value: &[u8]) -> u32 {
        loader.guest_write_bytes(VALUE, value).unwrap();
        call(loader, ORD_WIN_SET_PRES_PARAM, &[hwnd, id, value.len() as u32, VALUE])
    }

    fn query(loader: &Loader, hwnd: u32, id1: u32, id2: u32, flags: u32) -> (u32, u32, Vec<u8>) {
        loader.guest_write::<u32>(OUT_ID, 0xDEAD).unwrap();
        let n = call(loader, ORD_WIN_QUERY_PRES_PARAM, &[hwnd, id1, id2, OUT_ID, 64, BUF, flags]);
        let id = loader.guest_read::<u32>(OUT_ID).unwrap();
        (n, id, loader.guest_read_bytes(BUF, n as usize).unwrap())
    }

    fn posted(loader: &Loader, hmq: u32) -> Vec<(u32, u32, u32)> {
        let wm = loader.shared.window_mgr.lock_or_recover();
        let mq = wm.get_mq(hmq).unwrap();
        let mq = mq.lock_or_recover();
        mq.messages.iter().map(|m: &OS2Message| (m.hwnd, m.msg, m.mp1)).collect()
    }

    #[test]
    fn storage_is_bounded() {
        let mut pp = PresParams::default();
        assert!(pp.set(1, &[1, 2, 3, 4]));
        assert!(pp.set(1, &[5]), "replacing keeps one entry");
        assert_eq!((pp.len(), pp.get(1)), (1, Some(&[5u8][..])));
        assert!(!pp.set(2, &vec![0; MAX_PRESPARAM_LEN + 1]));
        for id in 2..=MAX_PRESPARAMS as u32 { assert!(pp.set(id, &[0])); }
        assert!(!pp.set(1000, &[0]), "no more than MAX_PRESPARAMS attributes");
        assert!(pp.remove(2) && !pp.remove(2));
        assert!(pp.set(1000, &[0]));
    }

    #[test]
    fn presparams_blocks_parse_and_stop_at_bad_entries() {
        let b = block(&[(PP_FOREGROUNDCOLOR, &0x00FF_0000u32.to_le_bytes()), (PP_FONTNAMESIZE, b"8.Courier\0")]);
        let p = parse_presparams(&b);
        assert_eq!(p.len(), 2);
        assert_eq!(p[1], (PP_FONTNAMESIZE, b"8.Courier\0".to_vec()));
        // A PARAM claiming more than the block holds ends the parse.
        let mut bad = block(&[(PP_BACKGROUNDCOLOR, &[1, 2, 3, 4])]);
        bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_presparams(&bad).is_empty());
        assert!(parse_presparams(&[1, 2]).is_empty());
        // cb larger than the data: what is there is read.
        let mut big = block(&[(PP_BACKGROUNDCOLOR, &[1, 2, 3, 4])]);
        big[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_presparams(&big).len(), 1);
    }

    #[test]
    fn font_name_size_maps_to_a_face_and_line_height() {
        let f = parse_font_name_size(b"10.Helv\0").unwrap();
        assert_eq!((f.family, f.px, f.bold), (FontFamily::Sans, 16, false));
        let f = parse_font_name_size(b"12.Courier.Bold").unwrap();
        assert_eq!((f.family, f.px, f.bold), (FontFamily::Mono, 20, true));
        assert_eq!(parse_font_name_size(b"Times").map(|f| f.px), None);
        assert_eq!(parse_font_name_size(b"0.Helv"), None);
        assert_eq!(parse_font_name_size(b"10."), None);
        assert_eq!(parse_font_name_size(b"99999.Helv"), None);
    }

    #[test]
    fn set_query_and_inherit_through_the_owner() {
        let (loader, _hmq, dlg, ctl, other) = setup();
        let red = 0x00FF_0000u32.to_le_bytes();
        assert_eq!(set(&loader, dlg, PP_FOREGROUNDCOLOR, &red), 1);
        assert_eq!(query(&loader, dlg, PP_FOREGROUNDCOLOR, 0, 0), (4, PP_FOREGROUNDCOLOR, red.to_vec()));
        // Every colour is pure: the flag changes nothing.
        assert_eq!(query(&loader, dlg, PP_FOREGROUNDCOLOR, 0, QPF_PURERGBCOLOR).2, red.to_vec());
        // The control inherits from its owner, unless QPF_NOINHERIT.
        assert_eq!(query(&loader, ctl, PP_FOREGROUNDCOLOR, 0, 0).2, red.to_vec());
        assert_eq!(query(&loader, ctl, PP_FOREGROUNDCOLOR, 0, QPF_NOINHERIT).0, 0);
        assert_eq!(query(&loader, other, PP_FOREGROUNDCOLOR, 0, 0).0, 0);
        // id2 found on a nearer window wins over id1 further up.
        assert_eq!(set(&loader, ctl, PP_FOREGROUNDCOLORINDEX, &(CLR_BLUE as u32).to_le_bytes()), 1);
        let (n, id, v) = query(&loader, ctl, PP_FOREGROUNDCOLOR, PP_FOREGROUNDCOLORINDEX, QPF_ID2COLORINDEX);
        assert_eq!((n, id, v), (4, PP_FOREGROUNDCOLORINDEX, 0x0000_00FFu32.to_le_bytes().to_vec()));
        // Without the flag the index itself comes back.
        assert_eq!(query(&loader, ctl, PP_FOREGROUNDCOLORINDEX, 0, 0).2, (CLR_BLUE as u32).to_le_bytes().to_vec());
        // A system colour index resolves through the system colours.
        assert_eq!(set(&loader, ctl, PP_BACKGROUNDCOLORINDEX, &(SYSCLR_DIALOGBACKGROUND as u32).to_le_bytes()), 1);
        let want = loader.shared.window_mgr.lock_or_recover().sys_colors.get(SYSCLR_DIALOGBACKGROUND).unwrap();
        assert_eq!(query(&loader, ctl, PP_BACKGROUNDCOLORINDEX, 0, QPF_ID1COLORINDEX).2, want.to_le_bytes().to_vec());
        assert_eq!(loader.shared.window_mgr.lock_or_recover().pres_color(ctl, PP_BACKGROUNDCOLOR, true), Some(want));
        // Removing it shows the owner's value again.
        assert_eq!(call(&loader, ORD_WIN_REMOVE_PRES_PARAM, &[ctl, PP_FOREGROUNDCOLORINDEX]), 1);
        assert_eq!(call(&loader, ORD_WIN_REMOVE_PRES_PARAM, &[ctl, PP_FOREGROUNDCOLORINDEX]), 0);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().pres_color(ctl, PP_FOREGROUNDCOLOR, false), Some(0x00FF_0000));
    }

    #[test]
    fn a_short_buffer_gets_part_of_the_value() {
        let (loader, _hmq, dlg, ..) = setup();
        set(&loader, dlg, PP_FONTNAMESIZE, b"10.Helv\0");
        let n = call(&loader, ORD_WIN_QUERY_PRES_PARAM, &[dlg, PP_FONTNAMESIZE, 0, 0, 3, BUF, 0]);
        assert_eq!(n, 3);
        assert_eq!(loader.guest_read_bytes(BUF, 3).unwrap(), b"10.");
    }

    #[test]
    fn changes_are_announced_to_the_inheriting_windows() {
        let (loader, hmq, dlg, ctl, other) = setup();
        set(&loader, dlg, PP_BACKGROUNDCOLOR, &[0, 0, 0xFF, 0]);
        let msgs = posted(&loader, hmq);
        assert!(msgs.contains(&(dlg, WM_PRESPARAMCHANGED, PP_BACKGROUNDCOLOR)));
        // The built-in control repaints; it has no procedure to be told.
        assert!(msgs.contains(&(ctl, WM_PAINT, 0)));
        assert!(!msgs.iter().any(|m| m.0 == other));
        // A control with its own value does not inherit the change.
        set(&loader, ctl, PP_FONTNAMESIZE, b"8.Helv");
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.pres_param_audience(dlg, PP_FONTNAMESIZE), vec![dlg]);
        assert_eq!(wm.pres_param_audience(dlg, PP_BACKGROUNDCOLOR), vec![dlg, ctl]);
        assert_eq!(wm.pres_font(ctl).map(|f| f.px), Some(13));
    }

    #[test]
    fn errors_are_reported() {
        let (loader, _hmq, dlg, ..) = setup();
        assert_eq!(set(&loader, 0xBAD, PP_FOREGROUNDCOLOR, &[0; 4]), 0);
        assert_eq!(call(&loader, ORD_WIN_GET_LAST_ERROR, &[0]), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
        assert_eq!(call(&loader, ORD_WIN_SET_PRES_PARAM, &[dlg, PP_USER, MAX_PRESPARAM_LEN as u32 + 1, VALUE]), 0);
        assert_eq!(call(&loader, ORD_WIN_GET_LAST_ERROR, &[0]), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_SET_PRES_PARAM, &[dlg, PP_USER, 4, 0]), 0);
        assert_eq!(call(&loader, ORD_WIN_QUERY_PRES_PARAM, &[dlg, PP_USER, 0, 0, 4, BUF, 0x10]), 0);
        assert_eq!(call(&loader, ORD_WIN_GET_LAST_ERROR, &[0]), error_id(SEVERITY_ERROR, PMERR_INVALID_FLAG));
        // An unknown attribute is simply not found.
        assert_eq!(query(&loader, dlg, PP_USER + 5, 0, 0).0, 0);
    }

    #[test]
    fn owner_loops_end() {
        let (loader, _hmq, dlg, ctl, _) = setup();
        loader.shared.window_mgr.lock_or_recover().get_window_mut(dlg).unwrap().owner = ctl;
        assert_eq!(query(&loader, ctl, PP_USER, 0, 0).0, 0);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().pres_param_audience(dlg, PP_USER), vec![dlg, ctl]);
    }

    #[test]
    fn guest_presparams_blocks_are_applied() {
        let (loader, _hmq, _dlg, ctl, _) = setup();
        let b = block(&[(PP_FONTNAMESIZE, b"12.Times New Roman\0")]);
        loader.guest_write_bytes(0x4000, &b).unwrap();
        loader.apply_guest_presparams(ctl, 0x4000);
        let f = loader.shared.window_mgr.lock_or_recover().pres_font(ctl).unwrap();
        assert_eq!((f.family, f.px), (FontFamily::Serif, 20));
        loader.apply_guest_presparams(ctl, 0); // none: nothing happens
    }
}
