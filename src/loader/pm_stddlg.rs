// SPDX-License-Identifier: GPL-3.0-only

//! What the PMCTLS standard dialogs (`pm_filedlg.rs`, `pm_fontdlg.rs`)
//! share.
//!
//! Both are ordinary `#Dialog` windows of built-in controls, built in
//! memory as `WinMessageBox` builds its box (`pm_msgbox.rs`) — or, with
//! `FDS_CUSTOM` / `FNTS_CUSTOM`, created from the application's own
//! `DLGTEMPLATE` resource — and run by the same modal loop as `WinDlgBox`
//! (`ApiResult::DlgRunLoop`), or modeless. The controls carry the `DID_*`
//! ids of `<pmstddlg.h>`, and the dialog logic finds them by id, so a
//! custom template works as far as it has the standard controls.
//!
//! The state of each dialog lives in [`StdDialogs`] (`WindowManager::
//! std_dialogs`) under the dialog's window handle and goes with it. A
//! dialog without a dialog procedure gets its messages through
//! [`Loader::std_dialog_procless`]; with an application dialog procedure
//! (`pfnDlgProc`, which subclasses it) the procedure passes what it does
//! not handle to `WinDefFileDlgProc` / `WinDefFontDlgProc`, which run the
//! same code. The `FILEDLG` / `FONTDLG` pointer is the dialog's `QWL_USER`
//! window word and `WM_INITDLG`'s mp2, so a dialog procedure can reach it
//! (the PM Guide's samples read it with `WinQueryWindowULong(hwnd,
//! QWL_USER)`).
//!
//! When the dialog is dismissed — by its own OK / Cancel or by the dialog
//! procedure's `WinDismissDlg` — the structure is written back
//! ([`Loader::std_dialog_dismissed`]); a modal call then returns TRUE
//! ([`Loader::std_dialog_return`]), a modeless one returned the dialog's
//! handle when it was created.
//!
//! # Messages to the dialog procedure
//!
//! A dialog with a dialog procedure is sent `FDM_FILTER` / `FDM_VALIDATE` /
//! `FDM_ERROR` (file dialog) and `FNTM_FILTERLIST` / `FNTM_FACENAMECHANGED`
//! / `FNTM_POINTSIZECHANGED` / `FNTM_STYLECHANGED` (font dialog). The
//! dialog logic runs on the host inside `WinDefFileDlgProc` /
//! `WinDefFontDlgProc`, so it queues these messages ([`StdSend`], in
//! [`StdDialogs`]) while it works, and the default procedure, before it
//! returns, sends them synchronously to the dialog procedure as a call
//! chain (ADR-0026, `pm_hook.rs` `ChainThen::StdDialog`): the strings they
//! point to are copied to a guest block freed afterwards, and when every
//! call has returned the dialog acts on the answers
//! ([`Loader::std_chain_done`]) — a file the filter refused leaves the
//! list, a name the validation refused keeps the dialog open — and the
//! default procedure returns its own result. Messages queued while such a
//! chain runs wait for the next call of the default procedure. A dialog
//! without a dialog procedure sends nothing.
//!
//! Layouts and sizes are Warpine's own.

use std::collections::{HashMap, HashSet};

use super::constants::*;
use super::pm_types::WindowManager;
use crate::lx::header::RT_DIALOG;
use super::{Loader, MutexExt};

/// `CCHMAXPATH`: the longest string a standard dialog passes its procedure.
const CCHMAXPATH_STD: usize = 260;

/// `QWL_USER`: the window word that holds the `FILEDLG` / `FONTDLG`.
pub const QWL_USER: i32 = 0;

/// Margin inside the dialog border, gap between rows, between columns.
pub const STD_MARGIN: i32 = 10;
pub const STD_ROW_GAP: i32 = 6;
pub const STD_COL_GAP: i32 = 12;
/// Push buttons.
pub const STD_BUTTON_W: i32 = 80;
pub const STD_BUTTON_H: i32 = 26;
pub const STD_BUTTON_GAP: i32 = 8;
/// Height of a label row and of an entry field (or a combination box's
/// entry row).
pub const STD_LABEL_H: i32 = super::CONTROL_LINE_H + 2;
pub const STD_ENTRY_H: i32 = super::pm_combo::COMBO_ENTRY_H;

/// The state of a standard dialog.
#[derive(Debug)]
pub enum StdDialog {
    File(Box<super::pm_filedlg::FileDlgState>),
    Font(Box<super::pm_fontdlg::FontDlgState>),
}

/// What the answer to a message a standard dialog sends its dialog
/// procedure is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdTag {
    /// `FDM_FILTER` for file `name` of the file list (FALSE: not listed).
    FileFilter(String),
    /// `FDM_VALIDATE` for one of the files OK / Apply takes (FALSE: the
    /// dialog stays).
    FileValidate,
    /// `FNTM_FILTERLIST` for item `text` of list `id` (FALSE: not listed).
    FontFilter { id: u32, text: String },
    /// A message whose answer the dialog does not use (`FDM_ERROR`, the
    /// `FNTM_*CHANGED` notifications).
    Notify,
}

/// A message parameter of a [`StdSend`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdArg {
    Value(u32),
    /// A string, passed as a pointer to it in the dialog's codepage.
    Text(String),
    /// A structure, passed as a pointer to a copy of it.
    Block(Vec<u8>),
}

/// A message a standard dialog sends its dialog procedure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdSend {
    pub msg: u32,
    pub mp1: StdArg,
    pub mp2: StdArg,
    pub tag: StdTag,
}

/// Bytes of the guest block holding the strings of one chain at most.
pub const MAX_STD_SEND_BYTES: usize = 1 << 20;

/// Standard dialogs by window handle, and the file lists
/// (`papszFQFilename`) handed to the program until `WinFreeFileDlgList`.
#[derive(Debug, Default)]
pub struct StdDialogs {
    dialogs: HashMap<u32, StdDialog>,
    file_lists: HashSet<u32>,
    /// Messages to send to each dialog's procedure.
    pending: HashMap<u32, Vec<StdSend>>,
    /// What the messages being sent to each dialog's procedure are for.
    in_flight: HashMap<u32, Vec<StdTag>>,
}

impl StdDialogs {
    pub fn insert(&mut self, hwnd: u32, d: StdDialog) {
        self.dialogs.insert(hwnd, d);
    }
    pub fn get(&self, hwnd: u32) -> Option<&StdDialog> {
        self.dialogs.get(&hwnd)
    }
    pub fn get_mut(&mut self, hwnd: u32) -> Option<&mut StdDialog> {
        self.dialogs.get_mut(&hwnd)
    }
    pub fn file(&self, hwnd: u32) -> Option<&super::pm_filedlg::FileDlgState> {
        match self.dialogs.get(&hwnd) { Some(StdDialog::File(s)) => Some(s), _ => None }
    }
    pub fn file_mut(&mut self, hwnd: u32) -> Option<&mut super::pm_filedlg::FileDlgState> {
        match self.dialogs.get_mut(&hwnd) { Some(StdDialog::File(s)) => Some(s), _ => None }
    }
    pub fn font(&self, hwnd: u32) -> Option<&super::pm_fontdlg::FontDlgState> {
        match self.dialogs.get(&hwnd) { Some(StdDialog::Font(s)) => Some(s), _ => None }
    }
    pub fn font_mut(&mut self, hwnd: u32) -> Option<&mut super::pm_fontdlg::FontDlgState> {
        match self.dialogs.get_mut(&hwnd) { Some(StdDialog::Font(s)) => Some(s), _ => None }
    }
    /// Drop the dialogs among destroyed windows `doomed`.
    pub fn forget_windows(&mut self, doomed: &[u32]) {
        for h in doomed {
            self.dialogs.remove(h);
            self.pending.remove(h);
            self.in_flight.remove(h);
        }
    }
    /// Queue `send` for dialog `hwnd`'s procedure.
    pub fn queue(&mut self, hwnd: u32, send: StdSend) {
        self.pending.entry(hwnd).or_default().push(send);
    }
    /// Drop the queued messages of dialog `hwnd` that `drop` selects.
    pub fn unqueue(&mut self, hwnd: u32, drop: impl Fn(&StdSend) -> bool) {
        if let Some(v) = self.pending.get_mut(&hwnd) {
            v.retain(|s| !drop(s));
        }
    }
    /// The queued messages of dialog `hwnd`, unless some are being sent.
    fn take_pending(&mut self, hwnd: u32) -> Vec<StdSend> {
        if self.in_flight.contains_key(&hwnd) {
            return Vec::new();
        }
        self.pending.remove(&hwnd).unwrap_or_default()
    }
    /// Record file list `papsz` as handed out.
    pub fn add_file_list(&mut self, papsz: u32) {
        self.file_lists.insert(papsz);
    }
    /// Take file list `papsz` back; false when it is not one handed out.
    pub fn take_file_list(&mut self, papsz: u32) -> bool {
        self.file_lists.remove(&papsz)
    }
}

/// Where a standard dialog `cx` × `cy` goes: centred on `area` (x, y, cx,
/// cy: the parent's frame, or the desktop) with `center`, else at `(x, y)`
/// relative to the area's origin; kept on the desktop.
pub fn place(center: bool, area: (i32, i32, i32, i32), (x, y): (i32, i32), (cx, cy): (i32, i32)) -> (i32, i32) {
    let (ax, ay, acx, acy) = area;
    let (px, py) = if center {
        (ax + (acx - cx) / 2, ay + (acy - cy) / 2)
    } else {
        (ax + x, ay + y)
    };
    let (dw, dh) = desktop_size();
    (px.clamp(0, (dw as i32 - cx).max(0)), py.clamp(0, (dh as i32 - cy).max(0)))
}

/// A control of an in-memory standard dialog: class, text, rectangle
/// (dialog coordinates, border included), id and style (`WS_VISIBLE` is
/// added).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdControl {
    pub class: &'static str,
    pub text: String,
    pub rect: (i32, i32, i32, i32),
    pub id: u32,
    pub style: u32,
}

impl StdControl {
    pub fn new(class: &'static str, text: &str, rect: (i32, i32, i32, i32), id: u32, style: u32) -> Self {
        StdControl { class, text: text.to_string(), rect, id, style }
    }
}

/// Create dialog window `title` (`cx` × `cy` at `pos`) on the desktop
/// with `controls`, owned by `owner`, for queue `hmq`, with dialog
/// procedure `pfn` (0 = none). Combination boxes get their parts.
/// Returns the dialog.
#[allow(clippy::too_many_arguments)]
pub fn create_std_dialog(wm: &mut WindowManager, hmq: u32, owner: u32, pfn: u32, title: &str,
                         pos: (i32, i32), size: (i32, i32), controls: &[StdControl]) -> u32 {
    let h = wm.create_window(WindowManager::DIALOG_CLASS.to_string(), HWND_DESKTOP, hmq);
    if let Some(w) = wm.get_window_mut(h) {
        w.owner = owner;
        w.pfn_wp = pfn;
        w.visible = true;
        w.text = title.to_string();
        w.frame_flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_DLGBORDER;
        (w.x, w.y) = pos;
        (w.cx, w.cy) = size;
    }
    for c in controls {
        let ch = wm.create_window(c.class.to_string(), h, hmq);
        if let Some(w) = wm.get_window_mut(ch) {
            (w.x, w.y, w.cx, w.cy) = c.rect;
            w.text = c.text.clone();
            w.id = c.id;
            w.style = c.style | WS_VISIBLE;
            w.visible = true;
            w.owner = h;
        }
        wm.build_combo_or_spin(ch);
    }
    h
}

/// The rectangle a standard dialog of parent `hwnd_p` is placed in: the
/// parent's frame, or the desktop for `HWND_DESKTOP` / no window.
pub fn parent_area(wm: &WindowManager, hwnd_p: u32) -> (i32, i32, i32, i32) {
    let (dw, dh) = desktop_size();
    let desktop = (0, 0, dw as i32, dh as i32);
    if hwnd_p == 0 || hwnd_p == HWND_DESKTOP || wm.get_window(hwnd_p).is_none() {
        return desktop;
    }
    let frame = wm.find_frame_for_hwnd(hwnd_p);
    wm.get_window(frame).map_or(desktop, |w| (w.x, w.y, w.cx, w.cy))
}

impl Loader {
    /// The default procedure of standard dialog `hwnd` returns `result`:
    /// first send the messages it queued to the dialog's procedure (a call
    /// chain that returns `result` when done; see the module comment).
    pub(super) fn std_flush(&self, hwnd: u32, result: super::ApiResult) -> super::ApiResult {
        use super::pm_hook::{CallChain, CallStep, ChainThen};
        let super::ApiResult::Normal(ret) = result else { return result };
        let (pfn, tid, sends) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some((pfn, hmq)) = wm.get_window(hwnd).map(|w| (w.pfn_wp, w.hmq)) else { return result };
            let Some(tid) = wm.queue_thread(hmq) else { return result };
            if pfn == 0 || wm.std_dialogs.get(hwnd).is_none() {
                return result;
            }
            let sends = wm.std_dialogs.take_pending(hwnd);
            if sends.is_empty() {
                return result;
            }
            (pfn, tid, sends)
        };
        // The strings and structures, each NUL-terminated (strings) and
        // 4-byte aligned, in one block.
        let cp = self.window_codepage(hwnd);
        let bytes_of = |a: &StdArg| match a {
            StdArg::Value(_) => None,
            StdArg::Text(t) => {
                let mut b = super::codepage::cp_encode(t, cp);
                b.truncate(CCHMAXPATH_STD - 1);
                b.push(0);
                Some(b)
            }
            StdArg::Block(b) => Some(b.clone()),
        };
        let mut data: Vec<u8> = Vec::new();
        let mut offsets: Vec<(Option<usize>, Option<usize>)> = Vec::with_capacity(sends.len());
        let mut sends: Vec<StdSend> = sends;
        let mut kept = 0;
        for s in &sends {
            let place = |a: &StdArg, data: &mut Vec<u8>| bytes_of(a).map(|b| {
                let at = data.len();
                data.extend_from_slice(&b);
                data.resize(data.len().next_multiple_of(4), 0);
                at
            });
            let before = data.len();
            let o = (place(&s.mp1, &mut data), place(&s.mp2, &mut data));
            if data.len() > MAX_STD_SEND_BYTES {
                data.truncate(before);
                break;
            }
            offsets.push(o);
            kept += 1;
        }
        sends.truncate(kept);
        let buf = if data.is_empty() {
            0
        } else {
            match self.shared.mem_mgr.lock_or_recover().alloc(data.len() as u32) {
                Some(b) => b,
                None => return result,
            }
        };
        if buf != 0 && self.guest_write_bytes(buf, &data).is_none() {
            self.shared.mem_mgr.lock_or_recover().free(buf);
            return result;
        }
        let value = |a: &StdArg, off: Option<usize>| match (a, off) {
            (StdArg::Value(v), _) => *v,
            (_, Some(o)) => buf + o as u32,
            _ => 0,
        };
        let steps: Vec<CallStep> = sends.iter().zip(&offsets)
            .map(|(s, &(o1, o2))| CallStep::send(pfn, hwnd, s.msg, value(&s.mp1, o1), value(&s.mp2, o2)))
            .collect();
        let tags: Vec<StdTag> = sends.into_iter().map(|s| s.tag).collect();
        self.shared.window_mgr.lock_or_recover().std_dialogs.in_flight.insert(hwnd, tags);
        self.start_chain(CallChain::new(tid, steps, false, ChainThen::StdDialog { hwnd, buf, ret }))
    }

    /// The messages standard dialog `hwnd` sent its procedure were
    /// answered with `results`: free their strings at `buf` and act on the
    /// answers.
    pub(crate) fn std_chain_done(&self, hwnd: u32, buf: u32, results: &[u32]) {
        if buf != 0 {
            self.shared.mem_mgr.lock_or_recover().free(buf);
        }
        let tags = self.shared.window_mgr.lock_or_recover().std_dialogs.in_flight.remove(&hwnd);
        // A call skipped (the dialog destroyed meanwhile): nothing to act on.
        let Some(tags) = tags.filter(|t| t.len() == results.len()) else { return };
        let mut refused_files = Vec::new();
        let mut validated: Option<bool> = None;
        let mut refused_items: Vec<(u32, String)> = Vec::new();
        for (tag, &r) in tags.into_iter().zip(results) {
            match tag {
                StdTag::FileFilter(name) if r == 0 => refused_files.push(name),
                StdTag::FileValidate => validated = Some(validated.unwrap_or(true) && r != 0),
                StdTag::FontFilter { id, text } if r == 0 => refused_items.push((id, text)),
                _ => {}
            }
        }
        if !refused_files.is_empty() {
            self.file_dlg_drop_files(hwnd, &refused_files);
        }
        if let Some(ok) = validated {
            self.file_dlg_validated(hwnd, ok);
        }
        if !refused_items.is_empty() {
            self.font_dlg_drop_items(hwnd, &refused_items);
        }
    }

    /// A standard dialog from the program's template `id` of module `hmod`
    /// (`FDS_CUSTOM` / `FNTS_CUSTOM`); `None` without a usable template.
    pub(super) fn std_custom_dialog(&self, tid: u32, parent: u32, owner: u32, pfn: u32,
                                    hmod: u32, id: u32) -> Option<u32> {
        let cp = self.thread_codepage(tid);
        let res = self.shared.resource_mgr.lock_or_recover().find(hmod, RT_DIALOG, id as u16);
        let template = res
            .and_then(|(addr, size)| self.guest_read_bytes(addr, (size as usize).min(MAX_DLG_TEMPLATE)))
            .and_then(|data| super::pm_dialog::parse_dlg_template(&data, cp))?;
        let (h, _, _) = self.create_dialog(tid, parent, owner, pfn, Some(template));
        Some(h)
    }

    /// Centre custom standard dialog `hwnd` on its parent's area when
    /// `center` (the template places it otherwise).
    pub(super) fn std_center_custom(&self, hwnd: u32, hwnd_p: u32, center: bool) {
        if !center {
            return;
        }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let area = parent_area(&wm, hwnd_p);
        let Some(size) = wm.get_window(hwnd).map(|w| (w.cx, w.cy)) else { return };
        let pos = place(true, area, (0, 0), size);
        if let Some(w) = wm.get_window_mut(hwnd) {
            (w.x, w.y) = pos;
        }
    }

    /// The text of control `id` of dialog `dlg` (empty when it has none).
    pub(super) fn std_item_text(&self, dlg: u32, id: u32) -> String {
        let wm = self.shared.window_mgr.lock_or_recover();
        let Some(h) = wm.find_child_by_id(dlg, id) else { return String::new() };
        let h = wm.get_window(h).and_then(|w| w.combo).map_or(h, |c| c.entry);
        wm.get_window(h).map(|w| w.text.clone()).unwrap_or_default()
    }

    /// Set the text of control `id` of dialog `dlg` (a combination box's
    /// entry field for a combination box), all of it selected, and repaint it.
    pub(super) fn std_set_item_text(&self, dlg: u32, id: u32, text: &str) {
        let field = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(h) = wm.find_child_by_id(dlg, id) else { return };
            let h = wm.get_window(h).and_then(|w| w.combo).map_or(h, |c| c.entry);
            let Some(w) = wm.get_window_mut(h) else { return };
            w.text = text.to_string();
            (w.edit_anchor, w.edit_caret, w.edit_scroll) = (0, text.chars().count(), 0);
            h
        };
        self.repaint_control(field);
    }

    /// Replace the items of list box `id` of dialog `dlg` (or of the list
    /// of combination box `id`), nothing selected, scrolled to the top.
    pub(super) fn std_set_list(&self, dlg: u32, id: u32, items: Vec<String>) {
        let list = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(h) = wm.find_child_by_id(dlg, id) else { return };
            let h = wm.get_window(h).and_then(|w| w.combo).map_or(h, |c| c.list);
            let Some(w) = wm.get_window_mut(h) else { return };
            w.listbox_items = items;
            w.item_handles.clear();
            w.listbox_sel = Default::default();
            w.top_row = 0;
            h
        };
        self.repaint_control(list);
    }

    /// Select item `index` of the list of combination box (or list box)
    /// `id` of dialog `dlg`, its text going into the entry field.
    pub(super) fn std_select_item(&self, dlg: u32, id: u32, index: usize) {
        let h = self.shared.window_mgr.lock_or_recover().find_child_by_id(dlg, id);
        if let Some(h) = h {
            self.dispatch_builtin_control(h, LM_SELECTITEM, index as u32, 1);
        }
    }

    /// The selected items of list box `id` of dialog `dlg` (of a
    /// combination box's list), in order.
    pub(super) fn std_selected(&self, dlg: u32, id: u32) -> Vec<(usize, String)> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let Some(h) = wm.find_child_by_id(dlg, id) else { return Vec::new() };
        let h = wm.get_window(h).and_then(|w| w.combo).map_or(h, |c| c.list);
        let Some(w) = wm.get_window(h) else { return Vec::new() };
        let mut out = Vec::new();
        let mut at = None;
        while let Some(i) = w.listbox_sel.next_after(at) {
            if let Some(t) = w.listbox_items.get(i) {
                out.push((i, t.clone()));
            }
            at = Some(i);
        }
        out
    }

    /// The check state of check box `id` of dialog `dlg` (false without one).
    pub(super) fn std_checked(&self, dlg: u32, id: u32) -> bool {
        let wm = self.shared.window_mgr.lock_or_recover();
        wm.find_child_by_id(dlg, id).and_then(|h| wm.get_window(h)).is_some_and(|w| w.check_state == 1)
    }

    /// Start standard dialog `hwnd` (its state registered): `QWL_USER` =
    /// `pstruct`, the focus on `focus`, `WM_INITDLG` (mp2 = `pstruct`) and
    /// a paint queued. Modal: the modal loop runs it (`DlgRunLoop`), and
    /// it is destroyed when it ends; modeless: its handle is returned.
    pub(super) fn std_dialog_start(&self, hwnd: u32, hmq: u32, focus: u32, pstruct: u32,
                                   modeless: bool, pfn: u32) -> super::ApiResult {
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if let Some(w) = wm.get_window_mut(hwnd) {
                w.window_ulong.insert(QWL_USER, pstruct);
            }
            wm.z_push_top(hwnd);
            wm.focused_hwnd = hwnd;
            if focus != 0 {
                wm.focus_hwnd = focus;
            }
            if !modeless {
                wm.begin_modal_dialog(hwnd, true);
            }
        }
        self.queue_dialog_start(hwnd, hmq, focus, pstruct);
        if modeless {
            super::ApiResult::Normal(hwnd)
        } else {
            super::ApiResult::DlgRunLoop { dlg_proc: pfn, hwnd_dlg: hwnd, hmq }
        }
    }

    /// A message to a standard dialog without a dialog procedure; `None`
    /// for other dialogs.
    pub(super) fn std_dialog_procless(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<super::ApiResult> {
        let kind = {
            let wm = self.shared.window_mgr.lock_or_recover();
            match wm.std_dialogs.get(hwnd) {
                Some(StdDialog::File(_)) => 1,
                Some(StdDialog::Font(_)) => 2,
                None => return None,
            }
        };
        Some(if kind == 1 {
            self.file_dlg_message(hwnd, msg, mp1, mp2)
        } else {
            self.font_dlg_message(hwnd, msg, mp1, mp2)
        })
    }

    /// Dialog `hwnd` is being dismissed with `result`: a standard dialog
    /// writes its structure back (once).
    pub(super) fn std_dialog_dismissed(&self, hwnd: u32, result: u32) {
        let kind = {
            let wm = self.shared.window_mgr.lock_or_recover();
            match wm.std_dialogs.get(hwnd) {
                Some(StdDialog::File(s)) if !s.committed => 1,
                Some(StdDialog::Font(s)) if !s.committed => 2,
                _ => return,
            }
        };
        if kind == 1 {
            self.file_dlg_commit(hwnd, result);
        } else {
            self.font_dlg_commit(hwnd, result);
        }
    }

    /// The value a modal dialog call returns when the loop of `hwnd` ends
    /// with `result`: TRUE for `WinFileDlg` / `WinFontDlg` (the outcome is
    /// in `lReturn`), `result` for every other dialog.
    pub(crate) fn std_dialog_return(&self, hwnd: u32, result: u32) -> u32 {
        if self.shared.window_mgr.lock_or_recover().std_dialogs.get(hwnd).is_some() { 1 } else { result }
    }

    /// Take a dismissed modeless standard dialog off the screen.
    pub(super) fn std_hide_modeless(&self, hwnd: u32) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let modeless = wm.get_window(hwnd).is_some_and(|w| !w.destroy_on_dismiss);
        if modeless {
            wm.close_dialog(hwnd);
            if let Some(w) = wm.get_window_mut(hwnd) {
                w.visible = false;
            }
            wm.mark_all_dirty();
        }
    }

    /// Encode `text` in codepage `cp` and write it NUL-terminated into the
    /// `cap`-byte guest buffer at `addr`, cut to fit (a DBCS pair is never
    /// split). Returns whether it fitted whole.
    pub(super) fn std_write_string(&self, addr: u32, cap: usize, text: &str, cp: u32) -> bool {
        if addr == 0 || cap == 0 {
            return false;
        }
        let (bytes, whole) = fit_encoded(text, cp, cap);
        let mut out = bytes;
        out.push(0);
        self.guest_write_bytes(addr, &out).is_some() && whole
    }
}

/// `text` encoded in codepage `cp`, cut at a character boundary to leave
/// room for a NUL in `cap` bytes; and whether it was not cut.
pub fn fit_encoded(text: &str, cp: u32, cap: usize) -> (Vec<u8>, bool) {
    let room = cap.saturating_sub(1);
    let all = super::codepage::cp_encode(text, cp);
    if all.len() <= room {
        return (all, true);
    }
    let mut out = Vec::with_capacity(room);
    for ch in text.chars() {
        let enc = super::codepage::cp_encode(ch.encode_utf8(&mut [0; 4]), cp);
        if out.len() + enc.len() > room {
            break;
        }
        out.extend_from_slice(&enc);
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_centres_or_offsets_and_stays_on_the_desktop() {
        let (dw, dh) = desktop_size();
        let desk = (0, 0, dw as i32, dh as i32);
        assert_eq!(place(true, desk, (0, 0), (200, 100)), ((dw as i32 - 200) / 2, (dh as i32 - 100) / 2));
        assert_eq!(place(true, (100, 100, 400, 300), (0, 0), (200, 100)), (200, 200));
        assert_eq!(place(false, (100, 100, 400, 300), (30, 40), (200, 100)), (130, 140));
        assert_eq!(place(false, desk, (-50, 5000), (200, 100)), (0, dh as i32 - 100), "kept on the desktop");
    }

    #[test]
    fn strings_are_cut_to_fit_their_buffers() {
        assert_eq!(fit_encoded("ABC", 437, 4), (b"ABC".to_vec(), true));
        assert_eq!(fit_encoded("ABCD", 437, 4), (b"ABC".to_vec(), false));
        assert_eq!(fit_encoded("", 437, 1), (Vec::new(), true));
        assert_eq!(fit_encoded("Ä", 850, 2), (vec![0x8E], true));
        // A DBCS character is not split.
        let (b, whole) = fit_encoded("A日", 932, 3);
        assert_eq!((b, whole), (b"A".to_vec(), false));
    }

    #[test]
    fn the_registry_keeps_dialogs_and_file_lists() {
        let mut d = StdDialogs::default();
        d.add_file_list(0x1000);
        assert!(d.take_file_list(0x1000));
        assert!(!d.take_file_list(0x1000), "only once");
        assert!(!d.take_file_list(0x2000), "never handed out");
    }
}
