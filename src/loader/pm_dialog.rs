// SPDX-License-Identifier: GPL-3.0-only

//! Dialog windows and keyboard accelerators.
//!
//! * [`parse_dlg_template`] reads a `DLGTEMPLATE` resource (`<pmwin.h>`): a
//!   14-byte header, then 30-byte `DLGTITEM`s in tree order — the dialog
//!   frame first, each item followed by its `cChildren` descendants — with
//!   class names, texts and control data in a pool addressed by offsets.
//! * Dialog keyboard handling for `WinDefDlgProc`: Tab / Shift+Tab move the
//!   focus between `WS_TABSTOP` controls, Enter presses the focused push
//!   button or the default (`BS_DEFAULT`) one, Esc posts `DID_CANCEL`.
//! * Accelerator tables (`ACCELTABLE`): parsed by [`parse_accel_table`] and
//!   matched against `WM_CHAR` by [`accel_matches`]; `WinGetMsg` and the
//!   modal dialog loop translate keystrokes through the active window's
//!   table, the queue's table and the system table (Alt+F4 → `SC_CLOSE`,
//!   Alt+F5 / Alt+F9 / Alt+F10 → `SC_RESTORE` / `SC_MINIMIZE` / `SC_MAXIMIZE`).
//!
//! Dialog coordinates: controls keep the template position converted to
//! pixels, relative to the dialog frame's bottom-left corner as in PM. The
//! dialog's surface starts inside the left border, which
//! `WindowManager::get_abs_rect_in_frame` accounts for.

use super::constants::*;
use super::pm_types::{AccelEntry, OS2Message};
use super::{pm_button, ApiResult, Loader, MutexExt};

// ── DLGTEMPLATE layout (<pmwin.h>) ───────────────────────────────────────────

/// `DLGTEMPLATE.codepage`: codepage of the template's texts.
const DT_CODEPAGE: usize = 4;
/// `DLGTEMPLATE.offadlgti`: offset of the first `DLGTITEM`.
const DT_OFF_ITEMS: usize = 6;
/// `DLGTEMPLATE.iItemFocus`: index of the item that gets the focus.
const DT_ITEM_FOCUS: usize = 10;
/// Size of the `DLGTEMPLATE` header before the items.
const DT_HEADER_SIZE: usize = 14;

/// Size of one `DLGTITEM`.
const DLGTITEM_SIZE: usize = 30;
const DI_CHILDREN: usize = 2;
const DI_CCH_CLASS: usize = 4;
const DI_OFF_CLASS: usize = 6;
const DI_CCH_TEXT: usize = 8;
const DI_OFF_TEXT: usize = 10;
const DI_STYLE: usize = 12;
const DI_X: usize = 16;
const DI_Y: usize = 18;
const DI_CX: usize = 20;
const DI_CY: usize = 22;
const DI_ID: usize = 24;
const DI_OFF_PRESPARAMS: usize = 26;
const DI_OFF_CTLDATA: usize = 28;

/// Offset value meaning "none" (`offCtlData`, `offPresParams`, `iItemFocus`).
const NO_OFFSET: u16 = 0xFFFF;
/// High word of a predefined class given as `MAKEINTATOM` (`WC_BUTTON` = 0xFFFF0003).
const CLASS_ATOM_BASE: u32 = 0xFFFF_0000;

/// One `DLGTITEM` of a template, with its strings resolved.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DlgItem {
    pub class_name: String,
    pub text: String,
    /// Position and size in dialog units, relative to the parent item.
    pub x: i16,
    pub y: i16,
    pub cx: i16,
    pub cy: i16,
    pub id: u16,
    pub style: u32,
    /// Index of the parent item in [`DlgTemplate::items`]; `None` = the dialog.
    pub parent: Option<usize>,
    /// Presentation parameters (`offPresParams`: a `PRESPARAMS` block in
    /// the template; `pm_presparam.rs`).
    pub pres_params: Vec<(u32, Vec<u8>)>,
}

/// A parsed `DLGTEMPLATE`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DlgTemplate {
    /// The dialog frame (the first item): title, position, size and style.
    pub dialog: DlgItem,
    /// `FCF_*` frame flags: the template's frame control data plus the border
    /// its `FS_*` style asks for.
    pub frame_flags: u32,
    /// Index into `items` of the control that gets the focus first, when the
    /// template names one.
    pub focus: Option<usize>,
    /// The controls, in template order (parents before their children).
    pub items: Vec<DlgItem>,
}

fn u16_at(data: &[u8], off: usize) -> Option<u16> {
    data.get(off..off + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The string at `off` in the template: at most `cch` bytes (all the way to
/// the end when `cch` is 0), cut at the first NUL, decoded with `codepage`.
fn template_string(data: &[u8], off: u16, cch: u16, codepage: u32) -> String {
    let off = off as usize;
    if off == NO_OFFSET as usize || off >= data.len() {
        return String::new();
    }
    let end = if cch == 0 { data.len() } else { (off + cch as usize).min(data.len()) };
    let bytes = &data[off..end];
    let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
    super::codepage::cp_decode(bytes, codepage)
}

/// Read the `DLGTITEM` at `off`; returns the item and its `cChildren`.
fn read_item(data: &[u8], off: usize, codepage: u32) -> Option<(DlgItem, u16, u16)> {
    let item = data.get(off..off + DLGTITEM_SIZE)?;
    let w = |o| u16_at(item, o).unwrap_or(0);
    let class_name = match w(DI_CCH_CLASS) {
        0 => super::pm_win::resolve_class_atom(CLASS_ATOM_BASE | w(DI_OFF_CLASS) as u32, String::new()),
        cch => template_string(data, w(DI_OFF_CLASS), cch, codepage),
    };
    Some((DlgItem {
        class_name,
        text: template_string(data, w(DI_OFF_TEXT), w(DI_CCH_TEXT), codepage),
        x: w(DI_X) as i16,
        y: w(DI_Y) as i16,
        cx: w(DI_CX) as i16,
        cy: w(DI_CY) as i16,
        id: w(DI_ID),
        style: u32_at(item, DI_STYLE).unwrap_or(0),
        parent: None,
        pres_params: match w(DI_OFF_PRESPARAMS) {
            NO_OFFSET => Vec::new(),
            off => data.get(off as usize..).map_or_else(Vec::new, super::pm_presparam::parse_presparams),
        },
    }, w(DI_CHILDREN), w(DI_OFF_CTLDATA)))
}

/// `FCF_*` border flag for the `FS_*` border style of a dialog.
fn border_flags(style: u32) -> u32 {
    if style & FS_SIZEBORDER != 0 {
        FCF_SIZEBORDER
    } else if style & FS_DLGBORDER != 0 {
        FCF_DLGBORDER
    } else if style & FS_BORDER != 0 {
        FCF_BORDER
    } else {
        0
    }
}

/// Parse a `DLGTEMPLATE` resource. Texts are decoded with the template's
/// codepage, or `fallback_cp` when it gives none. `None` when the data is
/// too short to hold the header and the dialog item.
///
/// The frame control data of the dialog item holds its `FCF_*` flags as one
/// ULONG — that is what the Open Watcom resource compiler writes for
/// `DIALOG ... , FCF_TITLEBAR | FCF_SYSMENU` (verified on
/// `samples/pm_menu_dlg_test`), not a `FRAMECDATA` structure.
pub(super) fn parse_dlg_template(data: &[u8], fallback_cp: u32) -> Option<DlgTemplate> {
    if data.len() < DT_HEADER_SIZE + DLGTITEM_SIZE {
        return None;
    }
    let codepage = match u16_at(data, DT_CODEPAGE)? { 0 => fallback_cp, cp => cp as u32 };
    let first = u16_at(data, DT_OFF_ITEMS)? as usize;
    let (dialog, children, ctl_off) = read_item(data, first, codepage)?;
    let frame_flags = border_flags(dialog.style) | match ctl_off {
        NO_OFFSET => 0,
        off => u32_at(data, off as usize).unwrap_or(0),
    };

    // Items follow the dialog in tree order. `pending` holds, for each open
    // parent, its index (None = the dialog) and how many children it still
    // has; a template can never hold more items than fit in its size.
    let max_items = (data.len() - first) / DLGTITEM_SIZE - 1;
    let mut items: Vec<DlgItem> = Vec::new();
    let mut pending: Vec<(Option<usize>, u16)> = vec![(None, children)];
    while let Some((parent, left)) = pending.last_mut() {
        if *left == 0 {
            pending.pop();
            continue;
        }
        *left -= 1;
        let parent = *parent;
        if items.len() >= max_items {
            break;
        }
        let Some((mut item, grandchildren, _)) =
            read_item(data, first + (items.len() + 1) * DLGTITEM_SIZE, codepage) else { break };
        item.parent = parent;
        items.push(item);
        if grandchildren > 0 {
            pending.push((Some(items.len() - 1), grandchildren));
        }
    }

    // iItemFocus indexes all items, the dialog being item 0.
    let focus = match u16_at(data, DT_ITEM_FOCUS)? {
        NO_OFFSET | 0 => None,
        i => Some(i as usize - 1).filter(|&i| i < items.len()),
    };
    Some(DlgTemplate { dialog, frame_flags, focus, items })
}

// ── Accelerator tables ───────────────────────────────────────────────────────

/// `ACCELTABLE` layout: `cAccel` (USHORT), `codepage` (USHORT), then
/// `ACCEL { fs, key, cmd }` entries of three USHORTs.
pub(super) const AT_HEADER_SIZE: usize = 4;
pub(super) const ACCEL_SIZE: usize = 6;

/// Parse an `ACCELTABLE` resource; entries past the end of `data` are dropped.
pub(super) fn parse_accel_table(data: &[u8]) -> Vec<AccelEntry> {
    let count = u16_at(data, 0).unwrap_or(0) as usize;
    (0..count).map_while(|i| {
        let off = AT_HEADER_SIZE + i * ACCEL_SIZE;
        Some(AccelEntry {
            flags: u16_at(data, off)?,
            key: u16_at(data, off + 2)?,
            cmd: u16_at(data, off + 4)?,
        })
    }).collect()
}

/// The system accelerator table: keystrokes every frame understands.
/// Alt+F4 closes, Alt+F5 restores, Alt+F7 moves, Alt+F8 sizes, Alt+F9
/// minimizes, Alt+F10 maximizes, Alt+Space and Shift+Esc open the system
/// menu (IBM OS/2 1.2 Quick Reference; Alt+F7 also from OS/2 Warp key
/// lists). Alt+F11 (Hide, as the system menu shows it) is left out: no
/// source maps it, and a frame hidden without a window list stays hidden.
pub(super) const SYSTEM_ACCELS: &[AccelEntry] = &[
    AccelEntry { flags: AF_VIRTUALKEY | AF_ALT | AF_SYSCOMMAND, key: VK_F4 as u16, cmd: SC_CLOSE as u16 },
    AccelEntry { flags: AF_VIRTUALKEY | AF_ALT | AF_SYSCOMMAND, key: VK_F5 as u16, cmd: SC_RESTORE as u16 },
    AccelEntry { flags: AF_VIRTUALKEY | AF_ALT | AF_SYSCOMMAND, key: VK_F7 as u16, cmd: SC_MOVE as u16 },
    AccelEntry { flags: AF_VIRTUALKEY | AF_ALT | AF_SYSCOMMAND, key: VK_F8 as u16, cmd: SC_SIZE as u16 },
    AccelEntry { flags: AF_VIRTUALKEY | AF_ALT | AF_SYSCOMMAND, key: VK_SPACE as u16, cmd: SC_SYSMENU as u16 },
    AccelEntry { flags: AF_VIRTUALKEY | AF_SHIFT | AF_SYSCOMMAND, key: VK_ESC as u16, cmd: SC_SYSMENU as u16 },
    AccelEntry { flags: AF_VIRTUALKEY | AF_ALT | AF_SYSCOMMAND, key: VK_F9 as u16, cmd: SC_MINIMIZE as u16 },
    AccelEntry { flags: AF_VIRTUALKEY | AF_ALT | AF_SYSCOMMAND, key: VK_F10 as u16, cmd: SC_MAXIMIZE as u16 },
];

/// Whether `WM_CHAR` `(mp1, mp2)` is the keystroke accelerator `entry`
/// describes. Only key presses match. Ctrl and Alt must be held exactly as
/// the entry asks; Shift too for virtual keys and scan codes, while for a
/// character Shift is part of the character itself. Characters compare
/// without case when Ctrl or Alt is held, since such chords report the
/// key's unshifted character.
pub(super) fn accel_matches(entry: &AccelEntry, mp1: u32, mp2: u32) -> bool {
    let fs = mp1 & 0xFFFF;
    if fs & KC_KEYUP != 0 {
        return false;
    }
    let want = |af: u16| entry.flags & af != 0;
    if want(AF_CONTROL) != (fs & KC_CTRL != 0) || want(AF_ALT) != (fs & KC_ALT != 0) {
        return false;
    }
    if want(AF_LONEKEY) && fs & KC_LONEKEY == 0 {
        return false;
    }
    let key = entry.key as u32;
    if want(AF_VIRTUALKEY) || want(AF_SCANCODE) {
        if want(AF_SHIFT) != (fs & KC_SHIFT != 0) {
            return false;
        }
        return if want(AF_VIRTUALKEY) {
            fs & KC_VIRTUALKEY != 0 && mp2 >> WM_CHAR_VKEY_SHIFT == key
        } else {
            fs & KC_SCANCODE != 0 && mp1 >> WM_CHAR_SCANCODE_SHIFT == key
        };
    }
    if want(AF_SHIFT) && fs & KC_SHIFT == 0 {
        return false;
    }
    let chr = mp2 & 0xFFFF;
    if fs & KC_CHAR == 0 || chr == 0 {
        return false;
    }
    let fold = |c: u32| match char::from_u32(c) {
        Some(ch) if ch.is_ascii_alphabetic() && fs & (KC_CTRL | KC_ALT) != 0 => ch.to_ascii_lowercase() as u32,
        _ => c,
    };
    fold(chr) == fold(key)
}

/// The message an accelerator posts: `(msg, mp1, mp2)` with
/// mp2 = MPFROM2SHORT(CMDSRC_ACCELERATOR, FALSE).
pub(super) fn accel_message(entry: &AccelEntry) -> (u32, u32, u32) {
    let msg = if entry.flags & AF_SYSCOMMAND != 0 {
        WM_SYSCOMMAND
    } else if entry.flags & AF_HELP != 0 {
        WM_HELP
    } else {
        WM_COMMAND
    };
    (msg, entry.cmd as u32, CMDSRC_ACCELERATOR)
}

/// The mnemonic of a control text: the character after the first `~`.
pub(super) fn mnemonic(text: &str) -> Option<char> {
    let mut chars = text.chars();
    chars.find(|&c| c == '~')?;
    chars.next().filter(|&c| c != '~')
}

/// A control text as shown: without its `~` mnemonic marker, and the
/// character index of the mnemonic (to underline), if any.
pub(crate) fn mnemonic_label(text: &str) -> (String, Option<usize>) {
    let mut shown = String::with_capacity(text.len());
    let mut mark = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '~' && mark.is_none() && chars.peek().is_some_and(|&n| n != '~') {
            mark = Some(shown.chars().count());
            continue;
        }
        shown.push(c);
    }
    (shown, mark)
}

/// mp2 of a `WM_COMMAND`: MPFROM2SHORT(source, fPointer).
pub(super) fn command_mp2(source: u32, pointer: bool) -> u32 {
    source | ((pointer as u32) << 16)
}

// ── Dialog behaviour ─────────────────────────────────────────────────────────

/// Whether the virtual key in `(mp1, mp2)` is pressed (not released).
fn vkey_down(mp1: u32, mp2: u32) -> Option<u32> {
    let fs = mp1 & 0xFFFF;
    (fs & KC_VIRTUALKEY != 0 && fs & KC_KEYUP == 0).then_some(mp2 >> WM_CHAR_VKEY_SHIFT)
}

impl Loader {
    /// Queue `msg` for `hwnd` on its message queue.
    pub(super) fn post_to_window(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) {
        let wm = self.shared.window_mgr.lock_or_recover();
        if let Some(mq_arc) = wm.find_hmq_for_hwnd(hwnd).and_then(|hmq| wm.get_mq(hmq)) {
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(OS2Message { hwnd, msg, mp1, mp2, time: 0, x: 0, y: 0 });
            mq.cond.notify_one();
        }
    }

    /// Translate a `WM_CHAR` for `hwnd` through the accelerator tables of
    /// its frame (or dialog), of the message queue and of the system, in that
    /// order. Returns the message to deliver instead — addressed to the
    /// frame's client, or to the dialog — or `None` when nothing matches.
    pub(crate) fn translate_accelerator(&self, hwnd: u32, mp1: u32, mp2: u32) -> Option<OS2Message> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let frame = wm.find_frame_for_hwnd(hwnd);
        let entry = [frame, 0].iter()
            .filter_map(|&h| wm.accel_table_of(h))
            .flat_map(|t| t.iter())
            .chain(SYSTEM_ACCELS.iter())
            .find(|e| accel_matches(e, mp1, mp2))?
            .clone();
        let target = wm.frame_to_client.get(&frame).copied().unwrap_or(frame);
        let (msg, mp1, mp2) = accel_message(&entry);
        Some(OS2Message { hwnd: target, msg, mp1, mp2, time: 0, x: 0, y: 0 })
    }

    /// Replace a keystroke in `msg` with the accelerator it stands for.
    pub(crate) fn translate_queued_accelerator(&self, msg: &mut OS2Message) {
        if msg.msg == WM_CHAR
            && let Some(translated) = self.translate_accelerator(msg.hwnd, msg.mp1, msg.mp2)
        {
            *msg = translated;
        }
    }

    /// Mark dialog `hwnd` dismissed with `result` and wake its modal loop.
    pub(super) fn dismiss_dialog(&self, hwnd: u32, result: u32) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if let Some(win) = wm.get_window_mut(hwnd) {
            win.dialog_dismissed = true;
            win.dialog_result = result;
        }
        if let Some(mq_arc) = wm.find_hmq_for_hwnd(hwnd).and_then(|hmq| wm.get_mq(hmq)) {
            mq_arc.lock_or_recover().cond.notify_one();
        }
    }

    /// The keyboard part of `WinDefDlgProc` for `WM_CHAR` on dialog `hwnd`:
    /// keys the focused control did not use. Returns whether the key was used.
    pub(super) fn dialog_char(&self, hwnd: u32, mp1: u32, mp2: u32) -> bool {
        let fs = mp1 & 0xFFFF;
        if fs & KC_KEYUP != 0 {
            return false;
        }
        // A character no control took: a mnemonic, typed plainly (the focus
        // is on a button) or with Alt.
        if fs & KC_CHAR != 0 && fs & KC_CTRL == 0
            && let Some(ch) = char::from_u32(mp2 & 0xFF).filter(char::is_ascii_alphanumeric)
        {
            return self.dialog_mnemonic(hwnd, ch);
        }
        let Some(vk) = vkey_down(mp1, mp2) else { return false };
        let shift = mp1 & KC_SHIFT != 0;
        match vk {
            VK_TAB | VK_BACKTAB => {
                let backward = vk == VK_BACKTAB || shift;
                let next = self.shared.window_mgr.lock_or_recover().next_tab_stop(hwnd, backward);
                if let Some(next) = next {
                    self.set_focus_window(next);
                }
                true
            }
            VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN => {
                let backward = matches!(vk, VK_LEFT | VK_UP);
                let next = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    wm.next_in_group(hwnd, backward).map(|h| {
                        (h, wm.get_window(h).is_some_and(|w| pm_button::cursor_selects(w.style)))
                    })
                };
                if let Some((next, select)) = next {
                    self.set_focus_window(next);
                    // The cursor moves the selection along an auto-radio group.
                    if select {
                        self.click_button(next, false);
                    }
                }
                next.is_some()
            }
            VK_NEWLINE | VK_ENTER => {
                let button = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let focus = wm.focus_hwnd;
                    if wm.find_frame_for_hwnd(focus) == hwnd && wm.is_push_button(focus) {
                        Some(focus)
                    } else {
                        wm.default_button(hwnd)
                    }
                };
                if let Some(button) = button {
                    self.click_button(button, false);
                }
                button.is_some()
            }
            VK_ESC => {
                // TODO(unverified): the CMDSRC_* code PM reports for Esc.
                self.post_to_window(hwnd, WM_COMMAND, DID_CANCEL, command_mp2(CMDSRC_OTHER, false));
                true
            }
            _ => false,
        }
    }

    /// Mnemonic `ch` in dialog `hwnd`: the control whose text marks `ch`
    /// with `~` is activated — a button is clicked, a static label passes the
    /// focus to the next control that can take it (the field it labels), any
    /// other control takes the focus. Returns whether a control matched.
    fn dialog_mnemonic(&self, hwnd: u32, ch: char) -> bool {
        let (target, is_button) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let controls = wm.builtin_control_descendants(hwnd);
            let Some(i) = controls.iter().position(|&h| {
                wm.get_window(h).is_some_and(|w| {
                    w.style & WS_DISABLED == 0
                        && mnemonic(&w.text).is_some_and(|m| m.eq_ignore_ascii_case(&ch))
                })
            }) else { return false };
            let h = controls[i];
            match wm.get_window(h).map(|w| w.class_name.as_str()) {
                Some("WC_BUTTON") => (Some(h), true),
                Some("WC_STATIC") => (controls[i + 1..].iter().copied().find(|&c| wm.can_take_focus(c)), false),
                _ => (Some(h).filter(|&c| wm.can_take_focus(c)), false),
            }
        };
        match target {
            Some(button) if is_button => self.click_button(button, false),
            Some(control) => { self.set_focus_window(control); }
            None => {}
        }
        true
    }

    /// A button was clicked (`pointer`: with the mouse): a push button posts
    /// `WM_COMMAND` — or `WM_SYSCOMMAND` / `WM_HELP` for `BS_SYSCOMMAND` /
    /// `BS_HELP` — with its id and `CMDSRC_PUSHBUTTON` to its owner; other
    /// buttons notify `WM_CONTROL` / `BN_CLICKED`.
    pub(crate) fn click_button(&self, hwnd: u32, pointer: bool) {
        let (owner, id, style) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let Some(w) = wm.get_window(hwnd) else { return };
            if w.style & WS_DISABLED != 0 { return; }
            (wm.notify_target(hwnd), w.id & 0xFFFF, w.style)
        };
        // Auto check boxes and radio buttons change their state themselves.
        self.select_button(hwnd);
        if owner == 0 {
            return;
        }
        if style & BS_PRIMARYSTYLES == BS_PUSHBUTTON {
            let msg = if style & BS_SYSCOMMAND != 0 {
                WM_SYSCOMMAND
            } else if style & BS_HELP != 0 {
                WM_HELP
            } else {
                WM_COMMAND
            };
            self.post_to_window(owner, msg, id, command_mp2(CMDSRC_PUSHBUTTON, pointer));
        } else {
            self.post_to_window(owner, WM_CONTROL, (BN_CLICKED << 16) | id, hwnd);
        }
    }

    /// `WinDefDlgProc`: default processing of a dialog message.
    pub(crate) fn def_dlg_proc(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> ApiResult {
        match msg {
            // FALSE: the focus chosen when the dialog was created stays.
            WM_INITDLG => ApiResult::Normal(0),
            WM_PAINT => self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0),
            WM_CHAR => ApiResult::Normal(self.dialog_char(hwnd, mp1, mp2) as u32),
            // Any command ends the dialog with the command value as result.
            WM_COMMAND => {
                self.dismiss_dialog(hwnd, mp1 & 0xFFFF);
                ApiResult::Normal(0)
            }
            WM_SYSCOMMAND if mp1 & 0xFFFF == SC_CLOSE => {
                self.post_to_window(hwnd, WM_CLOSE, 0, 0);
                ApiResult::Normal(0)
            }
            // The system menu and keyboard move / size (pm_sysmenu.rs).
            WM_SYSCOMMAND => {
                self.frame_syscommand(hwnd, mp1 & 0xFFFF, mp2);
                ApiResult::Normal(0)
            }
            WM_CLOSE => {
                self.dismiss_dialog(hwnd, DID_CANCEL);
                ApiResult::Normal(0)
            }
            _ => ApiResult::Normal(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `DLGTEMPLATE` Open Watcom `wrc` builds for the About dialog of
    /// `samples/pm_menu_dlg_test/pm_menu_dlg_test.rc` (copied from its .res).
    const ABOUT_DIALOG: &[u8] = &[
        0xa7, 0x00, 0x00, 0x00, 0x52, 0x03, 0x0e, 0x00, 0x01, 0x00, 0xff, 0xff, 0x00, 0x00,
        // DLGTITEM 0: the dialog (WC_FRAME), 2 children
        0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x19, 0x00, 0x68, 0x00,
        0x80, 0x00, 0x00, 0x94, 0x28, 0x00, 0x28, 0x00, 0xb4, 0x00, 0x50, 0x00,
        0x65, 0x00, 0xff, 0xff, 0x82, 0x00,
        // DLGTITEM 1: LTEXT
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x1d, 0x00, 0x86, 0x00,
        0x01, 0x00, 0x00, 0x80, 0x0a, 0x00, 0x2d, 0x00, 0xa0, 0x00, 0x0e, 0x00,
        0x2d, 0x01, 0xff, 0xff, 0xff, 0xff,
        // DLGTITEM 2: DEFPUSHBUTTON
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0xa4, 0x00,
        0x00, 0x04, 0x02, 0x80, 0x3c, 0x00, 0x0a, 0x00, 0x32, 0x00, 0x0e, 0x00,
        0x01, 0x00, 0xff, 0xff, 0xff, 0xff,
        // String pool: title, frame control data, label, button text
        b'A', b'b', b'o', b'u', b't', b' ', b'P', b'M', b' ', b'M', b'e', b'n', b'u', b'/',
        b'D', b'i', b'a', b'l', b'o', b'g', b' ', b'T', b'e', b's', b't', 0x00,
        0x03, 0x00, 0x00, 0x00,
        b'W', b'a', b'r', b'p', b'i', b'n', b'e', b' ', b'P', b'M', b' ', b'M', b'e', b'n',
        b'u', b' ', b'&', b' ', b'D', b'i', b'a', b'l', b'o', b'g', b' ', b'T', b'e', b's',
        b't', 0x00, b'O', b'K', 0x00,
    ];

    #[test]
    fn parses_the_wrc_about_dialog() {
        assert_eq!(ABOUT_DIALOG.len(), 0xa7);
        let t = parse_dlg_template(ABOUT_DIALOG, 437).expect("valid template");
        assert_eq!(t.dialog.text, "About PM Menu/Dialog Test");
        assert_eq!((t.dialog.x, t.dialog.y, t.dialog.cx, t.dialog.cy), (40, 40, 180, 80));
        assert_eq!(t.dialog.id, 101);
        assert_eq!(t.frame_flags, FCF_TITLEBAR | FCF_SYSMENU | FCF_DLGBORDER,
                   "control data flags plus FS_DLGBORDER");
        assert_eq!(t.focus, None);
        assert_eq!(t.items.len(), 2);
        let (label, ok) = (&t.items[0], &t.items[1]);
        assert_eq!((label.class_name.as_str(), label.text.as_str(), label.id), ("WC_STATIC", "Warpine PM Menu & Dialog Test", 301));
        assert_eq!((label.x, label.y, label.cx, label.cy), (10, 45, 160, 14));
        assert_eq!(label.style, WS_VISIBLE | SS_TEXT);
        assert_eq!((ok.class_name.as_str(), ok.text.as_str(), ok.id), ("WC_BUTTON", "OK", 1));
        assert_eq!(ok.style, WS_VISIBLE | WS_TABSTOP | BS_DEFAULT);
        assert_eq!((ok.parent, label.parent), (None, None));
    }

    #[test]
    fn items_carry_their_presparams() {
        use super::super::pm_presparam::PP_FOREGROUNDCOLOR;
        let mut t = ABOUT_DIALOG.to_vec();
        let at = t.len() as u16;
        // PRESPARAMS { cb = 12, PARAM { PP_FOREGROUNDCOLOR, 4, red } }
        for v in [12u32, PP_FOREGROUNDCOLOR, 4, 0x00FF_0000] {
            t.extend_from_slice(&v.to_le_bytes());
        }
        let label_pp = DT_HEADER_SIZE + DLGTITEM_SIZE + DI_OFF_PRESPARAMS;
        t[label_pp..label_pp + 2].copy_from_slice(&at.to_le_bytes());
        // The button's offset points past the end: no parameters.
        let button_pp = DT_HEADER_SIZE + 2 * DLGTITEM_SIZE + DI_OFF_PRESPARAMS;
        t[button_pp..button_pp + 2].copy_from_slice(&0x7000u16.to_le_bytes());
        let parsed = parse_dlg_template(&t, 437).unwrap();
        assert_eq!(parsed.items[0].pres_params, vec![(PP_FOREGROUNDCOLOR, 0x00FF_0000u32.to_le_bytes().to_vec())]);
        assert!(parsed.items[1].pres_params.is_empty());
        assert!(parsed.dialog.pres_params.is_empty(), "NO_OFFSET");
    }

    #[test]
    fn nested_items_keep_their_parent_and_focus_index() {
        let mut t = ABOUT_DIALOG.to_vec();
        // The label (item 1) now owns the button (item 2); the dialog has one child.
        t[14 + 2] = 1;
        t[14 + 30 + 2] = 1;
        // iItemFocus = 2: the button.
        t[10] = 2;
        t[11] = 0;
        let t = parse_dlg_template(&t, 437).unwrap();
        assert_eq!(t.items.len(), 2);
        assert_eq!(t.items[1].parent, Some(0));
        assert_eq!(t.focus, Some(1));
    }

    #[test]
    fn malformed_templates_do_not_panic() {
        assert!(parse_dlg_template(&[], 437).is_none());
        assert!(parse_dlg_template(&ABOUT_DIALOG[..40], 437).is_none(), "shorter than header + dialog");
        // All zero: the item offset 0 points into the header; no children.
        let t = parse_dlg_template(&[0u8; 64], 437).unwrap();
        assert!(t.items.is_empty());
        // More children claimed than the data holds: stop at the end.
        let mut t = ABOUT_DIALOG.to_vec();
        t[14 + 2] = 0xFF;
        t[14 + 3] = 0xFF;
        let parsed = parse_dlg_template(&t, 437).unwrap();
        assert!(parsed.items.len() <= t.len() / DLGTITEM_SIZE);
        // Offsets past the end give empty strings.
        let mut t = ABOUT_DIALOG.to_vec();
        t[14 + 10] = 0xF0;
        t[14 + 11] = 0x7F;
        assert_eq!(parse_dlg_template(&t, 437).unwrap().dialog.text, "");
        // An item offset beyond the data.
        let mut t = ABOUT_DIALOG.to_vec();
        t[6] = 0xF0;
        assert!(parse_dlg_template(&t, 437).is_none());
    }

    #[test]
    fn template_texts_use_the_template_codepage() {
        // "OK" replaced by 0x82 0xA0 (Shift-JIS 'あ') with the codepage set to 932.
        let mut t = ABOUT_DIALOG.to_vec();
        let ok = t.len() - 3;
        t[ok] = 0x82;
        t[ok + 1] = 0xA0;
        t[4] = (932u16 & 0xFF) as u8;
        t[5] = (932u16 >> 8) as u8;
        assert_eq!(parse_dlg_template(&t, 437).unwrap().items[1].text, "あ");
        // Codepage 0: the caller's codepage is used.
        t[4] = 0;
        t[5] = 0;
        assert_eq!(parse_dlg_template(&t, 932).unwrap().items[1].text, "あ");
    }

    #[test]
    fn accel_table_parsing_reads_the_count_first() {
        // cAccel = 2, codepage 850, then two entries; a third would be past the end.
        let data = [2, 0, 0x52, 0x03,
                    0x11, 0x00, b'd', 0x00, 0x10, 0x00,
                    0x02, 0x00, 0x22, 0x00, 0x11, 0x00];
        assert_eq!(parse_accel_table(&data), vec![
            AccelEntry { flags: AF_CHAR | AF_CONTROL, key: b'd' as u16, cmd: 0x10 },
            AccelEntry { flags: AF_VIRTUALKEY, key: VK_F3 as u16, cmd: 0x11 },
        ]);
        let mut short = data.to_vec();
        short[0] = 3;
        assert_eq!(parse_accel_table(&short).len(), 2, "entries past the end are dropped");
        assert!(parse_accel_table(&[]).is_empty());
    }

    fn key(fs: u32, chr: u32, vk: u32, scan: u32) -> (u32, u32) {
        (fs | (1 << WM_CHAR_REPEAT_SHIFT) | (scan << WM_CHAR_SCANCODE_SHIFT), chr | (vk << WM_CHAR_VKEY_SHIFT))
    }

    #[test]
    fn accelerators_match_keys_and_modifiers() {
        let ctrl_d = AccelEntry { flags: AF_CHAR | AF_CONTROL, key: b'd' as u16, cmd: 1 };
        let f3 = AccelEntry { flags: AF_VIRTUALKEY, key: VK_F3 as u16, cmd: 2 };
        let plain_x = AccelEntry { flags: AF_CHAR, key: b'x' as u16, cmd: 3 };
        let scan = AccelEntry { flags: AF_SCANCODE | AF_SHIFT, key: 0x1F, cmd: 4 };

        let (m1, m2) = key(KC_CHAR | KC_CTRL | KC_SCANCODE, b'd' as u32, 0, 0x20);
        assert!(accel_matches(&ctrl_d, m1, m2));
        let (m1, m2) = key(KC_CHAR | KC_CTRL | KC_SHIFT | KC_SCANCODE, b'D' as u32, 0, 0x20);
        assert!(accel_matches(&ctrl_d, m1, m2), "chords compare without case");
        let (m1, m2) = key(KC_CHAR | KC_SCANCODE, b'd' as u32, 0, 0x20);
        assert!(!accel_matches(&ctrl_d, m1, m2), "Ctrl must be held");
        let (m1, m2) = key(KC_CHAR | KC_CTRL | KC_KEYUP, b'd' as u32, 0, 0x20);
        assert!(!accel_matches(&ctrl_d, m1, m2), "key releases never match");

        let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE, 0, VK_F3, 0x3D);
        assert!(accel_matches(&f3, m1, m2));
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SHIFT | KC_SCANCODE, 0, VK_F3, 0x3D);
        assert!(!accel_matches(&f3, m1, m2), "Shift+F3 is another key");
        let (m1, m2) = key(KC_VIRTUALKEY | KC_ALT | KC_SCANCODE, 0, VK_F3, 0x3D);
        assert!(!accel_matches(&f3, m1, m2), "Alt+F3 is another key");

        let (m1, m2) = key(KC_CHAR, b'x' as u32, 0, 0);
        assert!(accel_matches(&plain_x, m1, m2));
        let (m1, m2) = key(KC_CHAR, b'X' as u32, 0, 0);
        assert!(!accel_matches(&plain_x, m1, m2), "a plain character is case-sensitive");

        let (m1, m2) = key(KC_SCANCODE | KC_SHIFT | KC_CHAR, b'S' as u32, 0, 0x1F);
        assert!(accel_matches(&scan, m1, m2));
        let (m1, m2) = key(KC_SCANCODE | KC_CHAR, b's' as u32, 0, 0x1F);
        assert!(!accel_matches(&scan, m1, m2));
    }

    #[test]
    fn system_accelerators_close_restore_minimize_maximize() {
        let (m1, m2) = key(KC_VIRTUALKEY | KC_ALT | KC_SCANCODE, 0, VK_F4, 0x3E);
        let hit = SYSTEM_ACCELS.iter().find(|e| accel_matches(e, m1, m2)).expect("Alt+F4");
        assert_eq!(accel_message(hit), (WM_SYSCOMMAND, SC_CLOSE, CMDSRC_ACCELERATOR));
        for (vk, scan, sc) in [(VK_F5, 0x3F, SC_RESTORE), (VK_F7, 0x41, SC_MOVE), (VK_F8, 0x42, SC_SIZE),
                               (VK_F9, 0x43, SC_MINIMIZE), (VK_F10, 0x44, SC_MAXIMIZE),
                               (VK_SPACE, 0x39, SC_SYSMENU)] {
            let (m1, m2) = key(KC_VIRTUALKEY | KC_ALT | KC_SCANCODE, 0, vk, scan);
            let hit = SYSTEM_ACCELS.iter().find(|e| accel_matches(e, m1, m2)).expect("Alt+Fn");
            assert_eq!(accel_message(hit), (WM_SYSCOMMAND, sc, CMDSRC_ACCELERATOR));
            let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE, 0, vk, scan);
            assert!(!SYSTEM_ACCELS.iter().any(|e| accel_matches(e, m1, m2)), "without Alt: no accelerator");
        }
        // Shift+Esc opens the system menu; Esc alone is left to the window.
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SHIFT | KC_SCANCODE, 0, VK_ESC, 0x01);
        let hit = SYSTEM_ACCELS.iter().find(|e| accel_matches(e, m1, m2)).expect("Shift+Esc");
        assert_eq!(accel_message(hit), (WM_SYSCOMMAND, SC_SYSMENU, CMDSRC_ACCELERATOR));
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE, 0, VK_ESC, 0x01);
        assert!(!SYSTEM_ACCELS.iter().any(|e| accel_matches(e, m1, m2)));
        let plain = AccelEntry { flags: AF_CHAR, key: b'q' as u16, cmd: 7 };
        assert_eq!(accel_message(&plain), (WM_COMMAND, 7, CMDSRC_ACCELERATOR));
        let help = AccelEntry { flags: AF_VIRTUALKEY | AF_HELP, key: VK_F1 as u16, cmd: 9 };
        assert_eq!(accel_message(&help).0, WM_HELP);
        assert_eq!(command_mp2(CMDSRC_PUSHBUTTON, true), CMDSRC_PUSHBUTTON | 0x1_0000);
    }

    // ── Dialog behaviour on a live window manager ──────────────────────────

    use crate::loader::pm_types::WindowManager;

    const ID_ENTRY: u16 = 10;
    const ID_LIST: u16 = 11;
    const ID_LABEL: u16 = 12;
    const ID_CHECK: u16 = 13;

    struct Dlg {
        loader: Loader,
        hmq: u32,
        dlg: u32,
        entry: u32,
        list: u32,
        ok: u32,
        cancel: u32,
        check: u32,
        focus_at_start: u32,
    }

    fn item(class: &str, text: &str, (x, y, cx, cy): (i16, i16, i16, i16), id: u16, style: u32) -> DlgItem {
        DlgItem { class_name: class.into(), text: text.into(), x, y, cx, cy, id, style: WS_VISIBLE | style, parent: None,
                  pres_params: Vec::new() }
    }

    /// A dialog with a label, an entry field, a list box, a check box and
    /// OK (default) / Cancel buttons; every control but the label and the
    /// check box is a tab stop.
    fn dialog() -> Dlg {
        let loader = Loader::new_mock();
        let hmq = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            hmq
        };
        let t = DlgTemplate {
            dialog: item("WC_FRAME", "Test", (10, 10, 200, 120), 100, FS_DLGBORDER),
            frame_flags: FCF_TITLEBAR | FCF_DLGBORDER,
            focus: None,
            items: vec![
                item("WC_STATIC", "Name:", (4, 100, 40, 10), ID_LABEL, SS_TEXT),
                item("WC_ENTRYFIELD", "", (50, 100, 100, 10), ID_ENTRY, WS_TABSTOP),
                item("WC_LISTBOX", "", (4, 40, 100, 50), ID_LIST, WS_TABSTOP),
                item("WC_BUTTON", "Check", (110, 40, 60, 10), ID_CHECK, BS_CHECKBOX),
                item("WC_BUTTON", "OK", (4, 4, 40, 14), DID_OK as u16, WS_TABSTOP | BS_DEFAULT),
                item("WC_BUTTON", "Cancel", (50, 4, 40, 14), DID_CANCEL as u16, WS_TABSTOP),
            ],
        };
        let (dlg, _, focus_at_start) = loader.create_dialog(0, HWND_DESKTOP, 0, 0x1234, Some(t));
        let id = |i: u16| loader.shared.window_mgr.lock_or_recover().find_child_by_id(dlg, i as u32).unwrap();
        let (entry, list, ok, cancel, check) =
            (id(ID_ENTRY), id(ID_LIST), id(DID_OK as u16), id(DID_CANCEL as u16), id(ID_CHECK));
        loader.shared.window_mgr.lock_or_recover().get_mq(hmq).unwrap().lock_or_recover().messages.clear();
        Dlg { loader, hmq, dlg, entry, list, ok, cancel, check, focus_at_start }
    }

    /// Combination boxes and spin buttons in a template get their parts; the
    /// focus and the tab order go to their entry fields.
    #[test]
    fn templates_with_combination_boxes_and_spin_buttons() {
        let loader = Loader::new_mock();
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
        }
        let t = DlgTemplate {
            dialog: item("WC_FRAME", "Test", (10, 10, 200, 120), 100, FS_DLGBORDER),
            frame_flags: FCF_TITLEBAR | FCF_DLGBORDER,
            focus: Some(0),
            items: vec![
                item("WC_COMBOBOX", "", (4, 40, 100, 70), 1, WS_TABSTOP | CBS_DROPDOWN),
                item("WC_SPINBUTTON", "", (110, 90, 60, 12), 2, WS_TABSTOP | SPBS_MASTER),
            ],
        };
        let (dlg, _, focus) = loader.create_dialog(0, HWND_DESKTOP, 0, 0x1234, Some(t));
        let wm = loader.shared.window_mgr.lock_or_recover();
        let combo = wm.find_child_by_id(dlg, 1).unwrap();
        let spin = wm.find_child_by_id(dlg, 2).unwrap();
        let entry = wm.find_child_by_id(combo, CBID_EDIT).expect("the combination box's field");
        assert!(wm.find_child_by_id(combo, CBID_LIST).is_some());
        let field = wm.text_window(spin);
        assert_ne!(field, spin, "the spin button's field");
        assert_eq!(focus, entry, "the template's focus item gives the focus to its field");
        assert_eq!(wm.tab_stops(dlg), vec![entry, field]);
    }

    #[test]
    fn the_first_focus_control_notifies_its_owner_after_wm_initdlg() {
        let d = dialog();
        assert_eq!(d.focus_at_start, d.entry);
        d.loader.queue_dialog_start(d.dlg, d.hmq, d.focus_at_start, 0x55);
        let msgs: Vec<_> = d.take_messages().into_iter().map(|m| (m.hwnd, m.msg, m.mp1, m.mp2)).collect();
        assert_eq!(msgs, [(d.dlg, WM_INITDLG, d.entry, 0x55), (d.dlg, WM_PAINT, 0, 0),
                          (d.dlg, WM_CONTROL, (EN_SETFOCUS << 16) | ID_ENTRY as u32, d.entry)]);
        // Without a focus control there is no notification.
        d.loader.queue_dialog_start(d.dlg, d.hmq, 0, 0);
        assert_eq!(d.take_messages().len(), 2);
    }

    impl Dlg {
        fn wm(&self) -> std::sync::MutexGuard<'_, WindowManager> { self.loader.shared.window_mgr.lock_or_recover() }
        fn focus(&self) -> u32 { self.wm().focus_hwnd }
        fn take_messages(&self) -> Vec<OS2Message> {
            let mq = self.wm().get_mq(self.hmq).unwrap();
            let mut mq = mq.lock_or_recover();
            mq.messages.drain(..).collect()
        }
        fn send(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(hwnd, msg, mp1, mp2) {
                ApiResult::Normal(v) => v,
                _ => panic!("built-in controls answer synchronously"),
            }
        }
        fn vkey(&self, hwnd: u32, vk: u32, extra: u32) -> u32 {
            let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE | extra, 0, vk, 0);
            match self.loader.def_dlg_proc(hwnd, WM_CHAR, m1, m2) {
                ApiResult::Normal(v) => v,
                _ => unreachable!(),
            }
        }
        fn dismissed(&self) -> Option<u32> {
            self.wm().get_window(self.dlg).filter(|w| w.dialog_dismissed).map(|w| w.dialog_result)
        }
    }

    fn command(msgs: &[OS2Message]) -> Option<(u32, u32, u32)> {
        msgs.iter().find(|m| m.msg == WM_COMMAND).map(|m| (m.hwnd, m.mp1, m.mp2))
    }

    #[test]
    fn a_new_dialog_is_active_and_focuses_its_first_tab_stop() {
        let d = dialog();
        assert_eq!(d.wm().focused_hwnd, d.dlg);
        assert_eq!(d.focus_at_start, d.entry);
        assert_eq!(d.focus(), d.entry);
        let wm = d.wm();
        assert_eq!(wm.tab_stops(d.dlg), vec![d.entry, d.list, d.ok, d.cancel]);
        assert_eq!(wm.default_button(d.dlg), Some(d.ok));
        // Controls belong to the dialog: it is their frame and their owner.
        assert_eq!(wm.find_frame_for_hwnd(d.entry), d.dlg);
        assert_eq!(wm.notify_target(d.ok), d.dlg);
        assert_eq!(wm.key_target(), d.entry);
    }

    #[test]
    fn template_focus_overrides_the_first_tab_stop() {
        let loader = Loader::new_mock();
        let mut t = parse_dlg_template(ABOUT_DIALOG, 437).unwrap();
        t.focus = Some(0); // the label cannot take the focus
        let (_, _, focus) = loader.create_dialog(0, HWND_DESKTOP, 0, 0, Some(t.clone()));
        let parent = focus_parent(&loader, focus);
        let ok = loader.shared.window_mgr.lock_or_recover().find_child_by_id(parent, 1).unwrap();
        assert_eq!(focus, ok, "falls back to the first tab stop");
        t.items[0].style |= WS_TABSTOP;
        t.items[0].class_name = "WC_ENTRYFIELD".into();
        t.focus = Some(0);
        let (dlg, _, focus) = loader.create_dialog(0, HWND_DESKTOP, 0, 0, Some(t));
        assert_eq!(focus, loader.shared.window_mgr.lock_or_recover().find_child_by_id(dlg, 301).unwrap());
    }

    fn focus_parent(loader: &Loader, h: u32) -> u32 {
        loader.shared.window_mgr.lock_or_recover().get_window(h).unwrap().parent
    }

    #[test]
    fn tab_and_backtab_cycle_through_the_tab_stops() {
        let d = dialog();
        for expected in [d.list, d.ok, d.cancel, d.entry] {
            assert_eq!(d.vkey(d.dlg, VK_TAB, 0), 1);
            assert_eq!(d.focus(), expected);
        }
        d.vkey(d.dlg, VK_BACKTAB, 0);
        assert_eq!(d.focus(), d.cancel);
        d.vkey(d.dlg, VK_TAB, KC_SHIFT);
        assert_eq!(d.focus(), d.ok, "Shift+Tab also goes back");
        // Releases and other keys are not dialog keys.
        assert_eq!(d.vkey(d.dlg, VK_TAB, KC_KEYUP), 0);
        assert_eq!(d.vkey(d.dlg, VK_F2, 0), 0);
        assert_eq!(d.focus(), d.ok);
    }

    #[test]
    fn keys_an_entry_field_does_not_use_reach_the_dialog() {
        let d = dialog();
        let (m1, m2) = key(KC_VIRTUALKEY | KC_CHAR | KC_SCANCODE, b'\t' as u32, VK_TAB, 0x0F);
        d.send(d.entry, WM_CHAR, m1, m2);
        let (m1, m2) = key(KC_VIRTUALKEY | KC_CHAR | KC_SCANCODE, b'\r' as u32, VK_NEWLINE, 0x1C);
        d.send(d.entry, WM_CHAR, m1, m2);
        let forwarded: Vec<_> = d.take_messages().into_iter().filter(|m| m.msg == WM_CHAR).collect();
        assert_eq!(forwarded.len(), 2);
        assert!(forwarded.iter().all(|m| m.hwnd == d.dlg));
    }

    #[test]
    fn enter_presses_the_focused_or_the_default_push_button() {
        let d = dialog();
        d.vkey(d.dlg, VK_NEWLINE, 0);
        assert_eq!(command(&d.take_messages()),
                   Some((d.dlg, DID_OK, command_mp2(CMDSRC_PUSHBUTTON, false))), "default button");
        d.loader.set_focus_window(d.cancel);
        d.vkey(d.dlg, VK_ENTER, 0);
        assert_eq!(command(&d.take_messages()).map(|c| c.1), Some(DID_CANCEL), "focused push button");
        // Without a default button Enter does nothing.
        d.loader.set_focus_window(d.entry);
        d.wm().get_window_mut(d.ok).unwrap().style &= !BS_DEFAULT;
        assert_eq!(d.vkey(d.dlg, VK_NEWLINE, 0), 0);
        assert!(command(&d.take_messages()).is_none());
    }

    #[test]
    fn escape_cancels() {
        let d = dialog();
        assert_eq!(d.vkey(d.dlg, VK_ESC, 0), 1);
        assert_eq!(command(&d.take_messages()), Some((d.dlg, DID_CANCEL, CMDSRC_OTHER)));
    }

    #[test]
    fn default_processing_ends_the_dialog() {
        let d = dialog();
        d.loader.def_dlg_proc(d.dlg, WM_COMMAND, 42, 0);
        assert_eq!(d.dismissed(), Some(42), "any command ends the dialog with its value");
        let d = dialog();
        d.loader.def_dlg_proc(d.dlg, WM_SYSCOMMAND, SC_CLOSE, CMDSRC_ACCELERATOR);
        assert_eq!(d.dismissed(), None);
        let msgs = d.take_messages();
        assert!(msgs.iter().any(|m| m.hwnd == d.dlg && m.msg == WM_CLOSE), "SC_CLOSE asks to close");
        d.loader.def_dlg_proc(d.dlg, WM_CLOSE, 0, 0);
        assert_eq!(d.dismissed(), Some(DID_CANCEL));
        // A dismissed dialog gives back the focus and the active window.
        d.wm().close_dialog(d.dlg);
        assert_eq!(d.focus(), 0);
        assert_ne!(d.wm().focused_hwnd, d.dlg);
    }

    fn at(x: i32, y: i32) -> u32 {
        (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16)
    }

    #[test]
    fn a_push_button_clicks_when_released_over_itself() {
        let d = dialog();
        let (cx, cy) = d.wm().get_window(d.cancel).map(|w| (w.cx, w.cy)).unwrap();
        d.send(d.cancel, WM_BUTTON1DOWN, at(5, 5), 0);
        assert_eq!(d.wm().pressed_control, d.cancel);
        assert_eq!(d.focus(), d.cancel, "a click takes the focus");
        assert!(command(&d.take_messages()).is_none(), "nothing happens on the press");
        d.send(d.cancel, WM_BUTTON1UP, at(5, 5), 0);
        assert_eq!(d.wm().pressed_control, 0);
        assert_eq!(command(&d.take_messages()),
                   Some((d.dlg, DID_CANCEL, command_mp2(CMDSRC_PUSHBUTTON, true))));

        // Released outside: it pops up without a click.
        d.send(d.cancel, WM_BUTTON1DOWN, at(5, 5), 0);
        d.send(d.cancel, WM_BUTTON1UP, at(cx + 3, cy / 2), 0);
        assert_eq!(d.wm().pressed_control, 0);
        assert!(command(&d.take_messages()).is_none());
        // A release without a press does nothing.
        d.send(d.ok, WM_BUTTON1UP, at(5, 5), 0);
        assert!(command(&d.take_messages()).is_none());
    }

    #[test]
    fn space_clicks_the_focused_button_and_other_keys_go_to_the_dialog() {
        let d = dialog();
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE, 0, VK_SPACE, 0x39);
        assert_eq!(d.send(d.ok, WM_CHAR, m1, m2), 1);
        let (m1, m2) = key(KC_CHAR, b' ' as u32, 0, 0);
        assert_eq!(d.send(d.ok, WM_CHAR, m1, m2), 1, "the typed space is swallowed");
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE | KC_KEYUP, 0, VK_SPACE, 0x39);
        d.send(d.ok, WM_CHAR, m1, m2);
        let msgs = d.take_messages();
        assert_eq!(msgs.iter().filter(|m| m.msg == WM_COMMAND).count(), 1, "one click per press");
        assert_eq!(command(&msgs), Some((d.dlg, DID_OK, command_mp2(CMDSRC_PUSHBUTTON, false))));
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE, 0, VK_ESC, 0x01);
        assert_eq!(d.send(d.ok, WM_CHAR, m1, m2), 0);
        assert!(d.take_messages().iter().any(|m| m.hwnd == d.dlg && m.msg == WM_CHAR));
    }

    #[test]
    fn other_buttons_notify_bn_clicked_and_disabled_buttons_ignore_clicks() {
        let d = dialog();
        d.send(d.check, WM_BUTTON1DOWN, at(2, 2), 0);
        d.send(d.check, WM_BUTTON1UP, at(2, 2), 0);
        let msgs = d.take_messages();
        let ctl = msgs.iter().find(|m| m.msg == WM_CONTROL && m.mp1 >> 16 == BN_CLICKED).expect("WM_CONTROL");
        assert_eq!((ctl.hwnd, ctl.mp1, ctl.mp2), (d.dlg, (BN_CLICKED << 16) | ID_CHECK as u32, d.check));

        d.wm().get_window_mut(d.ok).unwrap().style |= WS_DISABLED;
        assert_eq!(d.send(d.ok, WM_BUTTON1DOWN, at(2, 2), 0), 0);
        assert_eq!(d.wm().pressed_control, 0);
        d.loader.click_button(d.ok, false);
        assert!(d.take_messages().is_empty());
        assert_eq!(d.wm().default_button(d.dlg), None, "a disabled default button is skipped");
    }

    #[test]
    fn dialog_controls_are_hit_and_drawn_inside_the_border() {
        let d = dialog();
        let wm = d.wm();
        let bw = frame_border_width(FCF_DLGBORDER);
        let ok = wm.get_window(d.ok).unwrap();
        let (x, y, cx, cy) = wm.get_abs_rect_in_frame(d.ok);
        assert_eq!((x, y, cx, cy), (ok.x - bw, ok.y, ok.cx, ok.cy), "surface starts inside the border");
        assert_eq!(wm.get_abs_rect_in_frame(d.dlg).0, 0);
        // control_at takes surface coordinates.
        assert_eq!(wm.control_at(d.dlg, x + 1, y + 1), Some((d.ok, 1, 1)));
        assert_eq!(wm.control_at(d.dlg, 0, 0), None);
        // Static text is transparent to the pointer.
        let (lx, ly, _, _) = wm.get_abs_rect_in_frame(wm.find_child_by_id(d.dlg, ID_LABEL as u32).unwrap());
        assert_eq!(wm.control_at(d.dlg, lx + 1, ly + 1), None);
    }

    #[test]
    fn a_dialog_owned_by_a_window_keeps_its_controls_to_itself() {
        let loader = Loader::new_mock();
        let (frame, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            (frame, client)
        };
        let t = parse_dlg_template(ABOUT_DIALOG, 437).unwrap();
        let (dlg, _, ok) = loader.create_dialog(0, client, client, 0, Some(t));
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.find_frame_for_hwnd(ok), dlg, "not the owner's frame");
        assert_eq!(wm.find_frame_for_hwnd(dlg), dlg);
        assert_eq!(wm.find_frame_for_hwnd(client), frame);
        assert_eq!(wm.find_frame_for_hwnd(frame), frame, "parent is the desktop pseudo window");
        assert!(wm.builtin_control_descendants(client).is_empty(), "the dialog's controls are not the client's");
        assert_eq!(wm.control_root(frame), Some(client));
        assert_eq!(wm.control_root(dlg), Some(dlg));
    }

    #[test]
    fn accelerators_translate_through_frame_queue_and_system_tables() {
        let loader = Loader::new_mock();
        let (frame, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            let t = wm.add_accel_table(vec![AccelEntry { flags: AF_VIRTUALKEY, key: VK_F3 as u16, cmd: 300 }]);
            wm.set_window_accel(frame, t);
            let q = wm.add_accel_table(vec![AccelEntry { flags: AF_CHAR | AF_CONTROL, key: b'n' as u16, cmd: 301 }]);
            wm.set_window_accel(0, q);
            (frame, client)
        };
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE, 0, VK_F3, 0x3D);
        let t = loader.translate_accelerator(client, m1, m2).expect("frame table");
        assert_eq!((t.hwnd, t.msg, t.mp1, t.mp2), (client, WM_COMMAND, 300, CMDSRC_ACCELERATOR));
        let (m1, m2) = key(KC_CHAR | KC_CTRL, b'n' as u32, 0, 0x31);
        assert_eq!(loader.translate_accelerator(frame, m1, m2).map(|t| t.mp1), Some(301), "queue table");
        let (m1, m2) = key(KC_VIRTUALKEY | KC_ALT | KC_SCANCODE, 0, VK_F4, 0x3E);
        let t = loader.translate_accelerator(client, m1, m2).expect("system table");
        assert_eq!((t.hwnd, t.msg, t.mp1), (client, WM_SYSCOMMAND, SC_CLOSE));
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE, 0, VK_F5, 0x3F);
        assert!(loader.translate_accelerator(client, m1, m2).is_none());

        // Queued keystrokes become the command; other messages are left alone.
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE, 0, VK_F3, 0x3D);
        let mut msg = OS2Message { hwnd: client, msg: WM_CHAR, mp1: m1, mp2: m2, time: 0, x: 0, y: 0 };
        loader.translate_queued_accelerator(&mut msg);
        assert_eq!((msg.msg, msg.mp1), (WM_COMMAND, 300));
        let mut paint = OS2Message { hwnd: client, msg: WM_PAINT, mp1: m1, mp2: m2, time: 0, x: 0, y: 0 };
        loader.translate_queued_accelerator(&mut paint);
        assert_eq!(paint.msg, WM_PAINT);
    }

    #[test]
    fn mnemonics_are_the_character_after_the_tilde() {
        assert_eq!(mnemonic("~OK"), Some('O'));
        assert_eq!(mnemonic("E~xit"), Some('x'));
        assert_eq!(mnemonic("No mnemonic"), None);
        assert_eq!(mnemonic("trailing~"), None);
        assert_eq!(mnemonic("~~literal"), None);
        assert_eq!(mnemonic_label("E~xit"), ("Exit".to_string(), Some(1)));
        assert_eq!(mnemonic_label("~Name:"), ("Name:".to_string(), Some(0)));
        assert_eq!(mnemonic_label("plain"), ("plain".to_string(), None));
        assert_eq!(mnemonic_label("a~"), ("a~".to_string(), None));
        assert_eq!(mnemonic_label("日~本"), ("日本".to_string(), Some(1)));
    }

    fn chr(d: &Dlg, ch: char, extra: u32) -> u32 {
        let (m1, m2) = key(KC_CHAR | extra, ch as u32, 0, 0);
        match d.loader.def_dlg_proc(d.dlg, WM_CHAR, m1, m2) {
            ApiResult::Normal(v) => v,
            _ => unreachable!(),
        }
    }

    #[test]
    fn mnemonics_focus_the_labelled_field_or_click_the_button() {
        let d = dialog();
        {
            let mut wm = d.wm();
            let label = wm.find_child_by_id(d.dlg, ID_LABEL as u32).unwrap();
            wm.get_window_mut(label).unwrap().text = "~Name:".into();
            wm.get_window_mut(d.cancel).unwrap().text = "~Cancel".into();
        }
        d.loader.set_focus_window(d.ok);
        assert_eq!(chr(&d, 'n', KC_ALT), 1);
        assert_eq!(d.focus(), d.entry, "a label passes the focus to the next control");
        assert_eq!(chr(&d, 'C', 0), 1, "plain letters too, without case");
        assert_eq!(command(&d.take_messages()), Some((d.dlg, DID_CANCEL, command_mp2(CMDSRC_PUSHBUTTON, false))));
        assert_eq!(d.focus(), d.entry, "clicking by mnemonic leaves the focus");
        assert_eq!(chr(&d, 'z', KC_ALT), 0, "no control has it");
        assert_eq!(chr(&d, 'c', KC_CTRL), 0, "Ctrl chords are not mnemonics");
        let (m1, m2) = key(KC_CHAR | KC_KEYUP, b'c' as u32, 0, 0);
        assert!(matches!(d.loader.def_dlg_proc(d.dlg, WM_CHAR, m1, m2), ApiResult::Normal(0)));
        assert!(d.take_messages().is_empty());
        // A disabled control's mnemonic is not taken.
        d.wm().get_window_mut(d.cancel).unwrap().style |= WS_DISABLED;
        assert_eq!(chr(&d, 'c', 0), 0);
    }

    #[test]
    fn arrow_keys_move_within_a_group() {
        let d = dialog();
        // OK starts a group that holds OK and Cancel; the controls before it
        // form the first group.
        d.wm().get_window_mut(d.ok).unwrap().style |= WS_GROUP;
        d.loader.set_focus_window(d.ok);
        assert_eq!(d.vkey(d.dlg, VK_RIGHT, 0), 1);
        assert_eq!(d.focus(), d.cancel);
        d.vkey(d.dlg, VK_DOWN, 0);
        assert_eq!(d.focus(), d.ok, "wraps within the group");
        d.vkey(d.dlg, VK_LEFT, 0);
        assert_eq!(d.focus(), d.cancel);
        d.vkey(d.dlg, VK_UP, 0);
        assert_eq!(d.focus(), d.ok);
        // First group: entry, list, check box (the label cannot take the focus).
        d.loader.set_focus_window(d.check);
        d.vkey(d.dlg, VK_DOWN, 0);
        assert_eq!(d.focus(), d.entry, "never into the next group");
        d.vkey(d.dlg, VK_UP, 0);
        assert_eq!(d.focus(), d.check);
        // Alone in a group: the key is not used.
        d.wm().get_window_mut(d.cancel).unwrap().style |= WS_GROUP;
        d.loader.set_focus_window(d.ok);
        assert_eq!(d.vkey(d.dlg, VK_RIGHT, 0), 0);
        assert_eq!(d.focus(), d.ok);
    }

    /// A dialog with an entry field, a group of three auto-radio buttons
    /// (the last with `BS_NOCURSORSELECT`), a group label, a plain radio
    /// button group of two and OK; returns the dialog and its controls.
    fn radio_dialog() -> (Loader, u32, [u32; 7]) {
        let loader = Loader::new_mock();
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
        }
        let auto = BS_AUTORADIOBUTTON;
        let t = DlgTemplate {
            dialog: item("WC_FRAME", "Radio", (10, 10, 200, 120), 100, FS_DLGBORDER),
            frame_flags: FCF_TITLEBAR | FCF_DLGBORDER,
            focus: None,
            items: vec![
                item("WC_ENTRYFIELD", "", (4, 100, 100, 10), 20, WS_TABSTOP | WS_GROUP),
                item("WC_BUTTON", "~Red", (4, 80, 60, 10), 21, auto | WS_GROUP),
                item("WC_BUTTON", "Green", (4, 70, 60, 10), 22, auto),
                item("WC_BUTTON", "Blue", (4, 60, 60, 10), 23, auto | BS_NOCURSORSELECT),
                item("WC_STATIC", "Size", (80, 80, 40, 10), 24, SS_TEXT | WS_GROUP),
                item("WC_BUTTON", "Small", (80, 70, 60, 10), 25, BS_RADIOBUTTON),
                item("WC_BUTTON", "Large", (80, 60, 60, 10), 26, BS_RADIOBUTTON),
                item("WC_BUTTON", "OK", (4, 4, 40, 14), DID_OK as u16, WS_TABSTOP | WS_GROUP | BS_DEFAULT),
            ],
        };
        let (dlg, _, _) = loader.create_dialog(0, HWND_DESKTOP, 0, 0, Some(t));
        let ids = [20, 21, 22, 23, 25, 26, DID_OK];
        let h = ids.map(|i| loader.shared.window_mgr.lock_or_recover().find_child_by_id(dlg, i).unwrap());
        (loader, dlg, h)
    }

    fn checks(loader: &Loader, hs: &[u32]) -> Vec<u32> {
        let wm = loader.shared.window_mgr.lock_or_recover();
        hs.iter().map(|&h| wm.button_check(h)).collect()
    }

    fn dlg_vkey(loader: &Loader, dlg: u32, vk: u32) -> u32 {
        let (m1, m2) = key(KC_VIRTUALKEY | KC_SCANCODE, 0, vk, 0);
        match loader.def_dlg_proc(dlg, WM_CHAR, m1, m2) {
            ApiResult::Normal(v) => v,
            _ => unreachable!(),
        }
    }

    #[test]
    fn arrow_keys_check_auto_radio_buttons_as_they_move_and_wrap() {
        let (loader, dlg, [_, red, green, blue, ..]) = radio_dialog();
        let focus = || loader.shared.window_mgr.lock_or_recover().focus_hwnd;
        loader.set_focus_window(red);
        assert_eq!(dlg_vkey(&loader, dlg, VK_DOWN), 1);
        assert_eq!(focus(), green);
        assert_eq!(checks(&loader, &[red, green, blue]), [0, 1, 0], "the cursor selects");
        dlg_vkey(&loader, dlg, VK_DOWN);
        assert_eq!(focus(), blue);
        assert_eq!(checks(&loader, &[red, green, blue]), [0, 1, 0], "BS_NOCURSORSELECT: focus only");
        dlg_vkey(&loader, dlg, VK_RIGHT);
        assert_eq!(focus(), red, "wraps around within the group");
        assert_eq!(checks(&loader, &[red, green, blue]), [1, 0, 0]);
        dlg_vkey(&loader, dlg, VK_UP);
        assert_eq!(focus(), blue, "and backwards");
        assert_eq!(checks(&loader, &[red, green, blue]), [1, 0, 0]);
        // The owner hears of each selection.
        let mq = {
            let wm = loader.shared.window_mgr.lock_or_recover();
            wm.get_mq(wm.tid_to_hmq[&0]).unwrap()
        };
        let clicked: Vec<u32> = mq.lock_or_recover().messages.iter()
            .filter(|m| m.msg == WM_CONTROL && m.mp1 >> 16 == BN_CLICKED).map(|m| m.mp1 & 0xFFFF).collect();
        assert_eq!(clicked, [22, 21]);
    }

    #[test]
    fn arrow_keys_only_move_the_focus_over_plain_radio_buttons() {
        let (loader, dlg, [_, _, _, _, small, large, _]) = radio_dialog();
        loader.set_focus_window(small);
        dlg_vkey(&loader, dlg, VK_DOWN);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().focus_hwnd, large,
                   "the static label is not in the enumeration");
        assert_eq!(checks(&loader, &[small, large]), [0, 0]);
    }

    #[test]
    fn tab_enters_a_radio_group_on_its_checked_button() {
        let (loader, dlg, [entry, red, green, blue, small, large, ok]) = radio_dialog();
        let focus = || loader.shared.window_mgr.lock_or_recover().focus_hwnd;
        // No button checked: the group has no tab stop.
        assert_eq!(loader.shared.window_mgr.lock_or_recover().tab_stops(dlg), vec![entry, ok]);
        loader.dispatch_builtin_control(green, BM_SETCHECK, 1, 0);
        loader.dispatch_builtin_control(large, BM_SETCHECK, 1, 0);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().tab_stops(dlg), vec![entry, green, large, ok]);
        loader.set_focus_window(entry);
        dlg_vkey(&loader, dlg, VK_TAB);
        assert_eq!(focus(), green);
        // A click moves the check, and the tab stop with it.
        loader.click_button(blue, true);
        loader.set_focus_window(entry);
        dlg_vkey(&loader, dlg, VK_TAB);
        assert_eq!(focus(), blue);
        dlg_vkey(&loader, dlg, VK_TAB);
        assert_eq!(focus(), large);
        assert_eq!(checks(&loader, &[red, green, blue, small]), [0, 0, 1, 0]);
    }

    #[test]
    fn labels_hide_the_tilde_and_underline_the_mnemonic() {
        use crate::gui::surface::DrawOp;
        let d = dialog();
        let label = d.wm().find_child_by_id(d.dlg, ID_LABEL as u32).unwrap();
        d.wm().get_window_mut(label).unwrap().text = "~Name:".into();
        let surface = d.wm().frame_surface(d.dlg).unwrap();
        surface.lock_or_recover().record = Some(Vec::new());
        d.loader.dispatch_builtin_control(label, WM_PAINT, 0, 0);
        let ops = surface.lock_or_recover().record.take().unwrap();
        let (tx, ty) = ops.iter().find_map(|o| match o {
            DrawOp::Text { x, y, text, .. } if text == "Name:" => Some((*x, *y)),
            _ => None,
        }).expect("the label without its tilde");
        let w = crate::gui::font::text_width("N", crate::gui::font::FontSpec::SYSTEM);
        assert!(ops.iter().any(|o| matches!(o,
            DrawOp::Box { x1, y1, x2, y2, fill: true, .. }
                if *x1 == tx && *x2 == tx + w - 1 && *y1 == *y2 && *y1 > ty)),
            "a one-pel underline below N: {ops:?}");
    }

    #[test]
    fn a_modal_dialog_disables_its_owner_and_is_destroyed_when_it_ends() {
        let loader = Loader::new_mock();
        let (frame, client) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            wm.z_push_top(frame);
            (frame, client)
        };
        let t = parse_dlg_template(ABOUT_DIALOG, 437).unwrap();
        let (dlg, _, ok) = loader.create_dialog(0, HWND_DESKTOP, client, 0, Some(t.clone()));
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        wm.begin_modal_dialog(dlg, true);
        assert!(wm.frame_disabled(frame), "the owner's frame is disabled");
        assert!(!wm.frame_disabled(dlg));
        wm.get_window_mut(dlg).unwrap().dialog_dismissed = true;
        wm.get_window_mut(dlg).unwrap().dialog_result = 7;
        assert_eq!(wm.finish_modal_dialog(dlg), 7);
        assert!(!wm.frame_disabled(frame), "enabled again");
        assert!(!wm.is_window(dlg) && !wm.is_window(ok), "WinDlgBox destroys the dialog");
        assert_eq!(wm.focused_hwnd, frame, "the owner is active again");
        drop(wm);

        // WinProcessDlg: the dialog stays; an owner that was already
        // disabled stays disabled.
        let (dlg, _, _) = loader.create_dialog(0, HWND_DESKTOP, client, 0, Some(t));
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        wm.get_window_mut(frame).unwrap().style |= WS_DISABLED;
        wm.begin_modal_dialog(dlg, false);
        assert_eq!(wm.finish_modal_dialog(dlg), DID_CANCEL, "ended without being dismissed");
        assert!(wm.is_window(dlg));
        assert!(wm.frame_disabled(frame));
        assert_eq!(wm.finish_modal_dialog(0xDEAD), DID_CANCEL);
    }
}
