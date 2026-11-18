// SPDX-License-Identifier: GPL-3.0-only

//! `WinFileDlg` / `WinDefFileDlgProc` / `WinFreeFileDlgList` (PMCTLS): the
//! standard Open and Save As dialog, browsing Warpine's drives.
//!
//! The dialog (see `pm_stddlg.rs` for what the standard dialogs share) has
//! the controls of `<pmstddlg.h>`: the file-name entry field
//! (`DID_FILENAME_ED`), the type-of-file filter (`DID_FILTER_CB`: "<All
//! Files>" and `pszIType` / `papszITypeList`, matched against a file's
//! `.TYPE` extended attribute), the drive list (`DID_DRIVE_CB`: the mounted
//! drives, or those of `papszIDriveList`), the directories
//! (`DID_DIRECTORY_LB`: ".." and the subdirectories) and the files
//! (`DID_FILES_LB`) of the current directory, OK / Cancel and, with
//! `FDS_HELPBUTTON` / `FDS_APPLYBUTTON`, Help and Apply; a static above the
//! lists shows the current directory.
//!
//! * Everything is read through the drive manager (`vfs.rs`): only the
//!   mounted drives, paths below their roots (`..` stops at the root), no
//!   hidden or system files, at most [`MAX_DIR_ENTRIES`] entries per
//!   directory.
//! * `szFullFile` starts the dialog: its drive and directory (an unknown
//!   drive or a directory that does not exist leaves the current one —
//!   unverified: PM may report `FDS_ERR_INVALID_*` instead) and its last
//!   part, a file name (shown in the entry field) or a wildcard pattern
//!   (the file filter, default `*`).
//! * A click on a file puts its name in the entry field; a double click (or
//!   Enter in the file list) is OK. A double click on a directory goes into
//!   it; picking a drive or a type changes the lists.
//! * OK takes the entry field: a pattern becomes the filter, a directory is
//!   entered, anything else must be a valid name in an existing directory
//!   and ends the dialog (`DID_OK`); the name takes the case of an
//!   existing file it matches. The Open dialog does not require the file
//!   to exist (unverified: the application validates). With
//!   `FDS_MULTIPLESEL` (an extended-selection file list) several selected
//!   files are returned together.
//! * Write-back: `lReturn` (`DID_OK` / `DID_CANCEL`), `lSRC`
//!   (`FDS_SUCCESSFUL`), `szFullFile` (the first file, in the dialog
//!   thread's queue codepage), `ulFQFCount`; with `FDS_MULTIPLESEL`
//!   `papszFQFilename` points at a list Warpine allocates (one block: the
//!   pointers then `CCHMAXPATH`-byte names) that `WinFreeFileDlgList`
//!   frees. Apply writes the same without closing (unverified).
//! * `FDS_SAVEAS_DIALOG` titles it Save As and disables the file list
//!   unless `FDS_ENABLEFILELB`; `FDS_CENTER` centres it on its parent,
//!   otherwise `x` / `y` place it relative to the parent; `pszTitle` and
//!   `pszOKButton` replace the title and the OK text.
//! * Not supported: the `FDM_FILTER` / `FDM_VALIDATE` / `FDM_ERROR`
//!   messages are not sent (every file passes the filter and every valid
//!   name is accepted), `FDS_INCLUDE_EAS` / `FDS_PRELOAD_VOLINFO` /
//!   `FDS_FILTERUNION` / `FDS_NATIONAL_LANGUAGE` are accepted and ignored,
//!   the Help button does nothing (no help viewer), there is no read-only
//!   check box (`DID_READ_ONLY`), and Warpine does not ask before a
//!   Save As over an existing file.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_stddlg::*;
use super::vfs::{DriveManager, FileAttribute};
use super::{ApiResult, Loader};

// ── <pmstddlg.h> ─────────────────────────────────────────────────────────────

pub const FDS_CENTER: u32 = 0x0000_0001;
pub const FDS_CUSTOM: u32 = 0x0000_0002;
pub const FDS_HELPBUTTON: u32 = 0x0000_0008;
pub const FDS_APPLYBUTTON: u32 = 0x0000_0010;
pub const FDS_MODELESS: u32 = 0x0000_0040;
pub const FDS_OPEN_DIALOG: u32 = 0x0000_0100;
pub const FDS_SAVEAS_DIALOG: u32 = 0x0000_0200;
pub const FDS_MULTIPLESEL: u32 = 0x0000_0400;
pub const FDS_ENABLEFILELB: u32 = 0x0000_0800;

pub const FDS_SUCCESSFUL: u32 = 0;
pub const FDS_ERR_DIALOG_LOAD_ERROR: u32 = 12;
pub const FDS_ERR_INVALID_VERSION: u32 = 10;

pub const DID_FILENAME_TXT: u32 = 257;
pub const DID_FILENAME_ED: u32 = 258;
pub const DID_DRIVE_TXT: u32 = 259;
pub const DID_DRIVE_CB: u32 = 260;
pub const DID_FILTER_TXT: u32 = 261;
pub const DID_FILTER_CB: u32 = 262;
pub const DID_DIRECTORY_TXT: u32 = 263;
pub const DID_DIRECTORY_LB: u32 = 264;
pub const DID_FILES_TXT: u32 = 265;
pub const DID_FILES_LB: u32 = 266;
pub const DID_HELP_PB: u32 = 267;
pub const DID_APPLY_PB: u32 = 268;

/// `CCHMAXPATH` (`<bsedos.h>`): a full path with its NUL.
pub const CCHMAXPATH: usize = 260;

/// Id of the static that shows the current directory (Warpine's own; the
/// header names no such control).
pub const FILEDLG_PATH_ID: u32 = 0x7F01;

// FILEDLG field offsets (<pmstddlg.h>; every field naturally aligned).
const FD_CB_SIZE: usize = 0;
const FD_FL: usize = 4;
const FD_LRETURN: u32 = 12;
const FD_LSRC: u32 = 16;
const FD_PSZ_TITLE: usize = 20;
const FD_PSZ_OK_BUTTON: usize = 24;
const FD_PFN_DLG_PROC: usize = 28;
const FD_PSZ_ITYPE: usize = 32;
const FD_PAPSZ_ITYPE_LIST: usize = 36;
const FD_PSZ_IDRIVE: usize = 40;
const FD_PAPSZ_IDRIVE_LIST: usize = 44;
const FD_HMOD: usize = 48;
const FD_SZ_FULL_FILE: usize = 52;
const FD_PAPSZ_FQ_FILENAME: u32 = 312;
const FD_UL_FQF_COUNT: u32 = 316;
const FD_US_DLG_ID: usize = 320;
const FD_X: usize = 322;
const FD_Y: usize = 324;
/// `sizeof(FILEDLG)`.
pub const FILEDLG_SIZE: usize = 328;

/// Entries read from one directory at most.
pub const MAX_DIR_ENTRIES: usize = 4096;
/// Types read from `papszITypeList` / drives from `papszIDriveList` at most.
const MAX_TYPES: usize = 64;
const MAX_DRIVES: usize = 26;
/// Files returned together at most (`FDS_MULTIPLESEL`).
pub const MAX_FQF_FILES: usize = 1024;
/// Path components at most (each at least two bytes in `CCHMAXPATH`).
const MAX_COMPONENTS: usize = CCHMAXPATH / 2;
/// The `.TYPE` extended attribute and its value encodings (`<bsedos.h>`).
const TYPE_EA: &str = ".TYPE";
const EAT_MVMT: u16 = 0xFFDF;
const EAT_ASCII: u16 = 0xFFFD;

/// The filter entry that shows every file.
pub const ALL_FILES: &str = "<All Files>";

// Layout (Warpine's own).
const COL_W: i32 = 200;
const LIST_H: i32 = 168;
const COMBO_DROP_H: i32 = 6 * super::CONTROL_LINE_H + 4;

/// The `FILEDLG` fields the dialog reads (pointers unresolved).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDlgIn {
    pub cb_size: u32,
    pub fl: u32,
    pub psz_title: u32,
    pub psz_ok_button: u32,
    pub pfn_dlg_proc: u32,
    pub psz_itype: u32,
    pub papsz_itype_list: u32,
    pub psz_idrive: u32,
    pub papsz_idrive_list: u32,
    pub hmod: u32,
    /// `szFullFile` up to its NUL (at most `CCHMAXPATH` bytes).
    pub full_file: Vec<u8>,
    pub dlg_id: u16,
    pub x: i16,
    pub y: i16,
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// Parse the `FILEDLG_SIZE` bytes of a `FILEDLG`; `None` when shorter.
pub fn parse_filedlg(b: &[u8]) -> Option<FileDlgIn> {
    if b.len() < FILEDLG_SIZE {
        return None;
    }
    let full = &b[FD_SZ_FULL_FILE..FD_SZ_FULL_FILE + CCHMAXPATH];
    let end = full.iter().position(|&c| c == 0).unwrap_or(CCHMAXPATH);
    Some(FileDlgIn {
        cb_size: le32(b, FD_CB_SIZE),
        fl: le32(b, FD_FL),
        psz_title: le32(b, FD_PSZ_TITLE),
        psz_ok_button: le32(b, FD_PSZ_OK_BUTTON),
        pfn_dlg_proc: le32(b, FD_PFN_DLG_PROC),
        psz_itype: le32(b, FD_PSZ_ITYPE),
        papsz_itype_list: le32(b, FD_PAPSZ_ITYPE_LIST),
        psz_idrive: le32(b, FD_PSZ_IDRIVE),
        papsz_idrive_list: le32(b, FD_PAPSZ_IDRIVE_LIST),
        hmod: le32(b, FD_HMOD),
        full_file: full[..end].to_vec(),
        dlg_id: le16(b, FD_US_DLG_ID),
        x: le16(b, FD_X) as i16,
        y: le16(b, FD_Y) as i16,
    })
}

// ── Paths and patterns ──────────────────────────────────────────────────────

/// Whether `s` holds a wildcard (`*` or `?`).
pub fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?'])
}

/// OS/2 wildcard match, ignoring case: `*` any run, `?` one character.
/// (A name without a dot also matches a pattern ending in `.*`.)
pub fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().flat_map(char::to_uppercase).collect();
    let n: Vec<char> = name.chars().flat_map(char::to_uppercase).collect();
    if glob(&p, &n) {
        return true;
    }
    // "*.*" and "NAME.*" also take names without an extension.
    !name.contains('.') && p.ends_with(&['.', '*']) && glob(&p[..p.len() - 2], &n)
}

/// Iterative glob with back-tracking to the last `*`.
fn glob(p: &[char], n: &[char]) -> bool {
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Whether `c` may appear in a file name (HPFS rules, Warpine's reading:
/// no control characters, no `\ / : * ? " < > |`).
fn name_char_ok(c: char) -> bool {
    !c.is_control() && !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
}

/// Whether `comp` is a valid name (the last one may be a pattern).
fn component_ok(comp: &str, last: bool) -> bool {
    !comp.is_empty() && comp.trim() == comp.trim_start() && !comp.ends_with(' ')
        && comp.chars().all(|c| name_char_ok(c) || (last && matches!(c, '*' | '?')))
}

/// `text` typed in the dialog, resolved against drive `cur_drive` and its
/// directory `cur_dir`; `drive_dir` gives the current directory of another
/// drive. Returns the drive and the path components (`.` and `..`
/// resolved, `..` stopping at the root); `None` for an invalid path (a bad
/// drive letter, invalid characters, too long).
pub fn resolve_input(text: &str, cur_drive: u8, cur_dir: &[String],
                     drive_dir: impl Fn(u8) -> Vec<String>) -> Option<(u8, Vec<String>)> {
    let text = text.trim();
    if text.is_empty() || text.len() >= CCHMAXPATH {
        return None;
    }
    let (drive, rest) = match text.as_bytes() {
        [l, b':', ..] => {
            let l = l.to_ascii_uppercase();
            if !l.is_ascii_uppercase() { return None; }
            (l - b'A', &text[2..])
        }
        _ => (cur_drive, text),
    };
    let absolute = rest.starts_with(['\\', '/']);
    let mut comps: Vec<String> = if absolute {
        Vec::new()
    } else if drive == cur_drive {
        cur_dir.to_vec()
    } else {
        drive_dir(drive)
    };
    let parts: Vec<&str> = rest.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    for (i, part) in parts.iter().enumerate() {
        match *part {
            "." => {}
            ".." => { comps.pop(); }
            p if component_ok(p, i + 1 == parts.len()) => comps.push(p.to_string()),
            _ => return None,
        }
        if comps.len() > MAX_COMPONENTS { return None; }
    }
    (full_path(drive, &comps).len() < CCHMAXPATH).then_some((drive, comps))
}

/// `C:\DIR\NAME` for drive `drive` (0 = A) and the components.
pub fn full_path(drive: u8, comps: &[String]) -> String {
    format!("{}:\\{}", (b'A' + drive) as char, comps.join("\\"))
}

/// A directory's path relative to its drive's root, as the drive manager
/// takes it.
fn rel(comps: &[String]) -> String {
    comps.join("/")
}

/// The type names of a `.TYPE` extended attribute value: a multi-valued
/// `EAT_MVMT` list of `EAT_ASCII` strings, or a single `EAT_ASCII` string.
pub fn type_names(value: &[u8]) -> Vec<String> {
    let rd16 = |at: usize| value.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let ascii = |at: usize| -> Option<(String, usize)> {
        if rd16(at)? != EAT_ASCII { return None; }
        let len = rd16(at + 2)? as usize;
        let bytes = value.get(at + 4..at + 4 + len)?;
        Some((String::from_utf8_lossy(bytes).into_owned(), at + 4 + len))
    };
    match rd16(0) {
        Some(EAT_MVMT) => {
            let count = rd16(4).unwrap_or(0) as usize;
            let mut at = 6;
            let mut out = Vec::new();
            for _ in 0..count.min(MAX_TYPES) {
                let Some((s, next)) = ascii(at) else { break };
                out.push(s);
                at = next;
            }
            out
        }
        Some(EAT_ASCII) => ascii(0).map(|(s, _)| vec![s]).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Sort names as the lists show them: ignoring case.
pub fn sort_names(v: &mut [String]) {
    v.sort_by_cached_key(|s| (s.to_uppercase(), s.clone()));
}

/// What the dialog starts with, from `szFullFile`: the drive, the
/// directory, and the entry field's text (a file name) or the filter (a
/// pattern). An empty `szFullFile` starts in the current directory.
pub fn initial_place(full_file: &str, cur_drive: u8, cur_dir: &[String],
                     drive_dir: impl Fn(u8) -> Vec<String>) -> (u8, Vec<String>, String, String) {
    let all = "*".to_string();
    let Some((drive, mut comps)) = resolve_input(full_file, cur_drive, cur_dir, drive_dir) else {
        return (cur_drive, cur_dir.to_vec(), String::new(), all);
    };
    // "C:\DIR\" names the directory itself.
    let names_dir = full_file.trim_end().ends_with(['\\', '/']) || full_file.trim_end().ends_with(':');
    if names_dir {
        return (drive, comps, String::new(), all);
    }
    let last = comps.pop().unwrap_or_default();
    if has_wildcard(&last) {
        (drive, comps, String::new(), last)
    } else {
        (drive, comps, last, all)
    }
}

/// One directory's subdirectories and files.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Listing {
    pub dirs: Vec<String>,
    pub files: Vec<String>,
}

/// Read directory `dir` of `drive`: `None` when it does not exist.
/// Subdirectories and the files matching `pattern` (and, when `type_name`
/// is given, carrying that `.TYPE`), sorted.
pub fn list_directory(dm: &DriveManager, drive: u8, dir: &[String], pattern: &str,
                      type_name: Option<&str>) -> Option<Listing> {
    let backend = dm.backend(drive).ok()?;
    let r = rel(dir);
    let search = if r.is_empty() { "*".to_string() } else { format!("{r}/*") };
    let mut entries = Vec::new();
    match backend.find_first(&search, FileAttribute::DIRECTORY) {
        Ok((h, first)) => {
            entries.push(first);
            while entries.len() < MAX_DIR_ENTRIES {
                match backend.find_next(h) {
                    Ok(e) => entries.push(e),
                    Err(_) => break,
                }
            }
            let _ = backend.find_close(h);
        }
        Err(e) if e == super::vfs::Os2Error::NO_MORE_FILES => {}
        Err(_) => return None,
    }
    let mut out = Listing::default();
    for e in entries {
        if e.status.attributes.contains(FileAttribute::DIRECTORY) {
            if e.name != "." && e.name != ".." {
                out.dirs.push(e.name);
            }
        } else if wildcard_match(pattern, &e.name) {
            let typed = type_name.is_none_or(|t| {
                let path = if r.is_empty() { e.name.clone() } else { format!("{r}/{}", e.name) };
                backend.get_ea(&path, TYPE_EA).is_ok_and(|ea| type_names(&ea.value).iter().any(|n| n == t))
            });
            if typed {
                out.files.push(e.name);
            }
        }
    }
    sort_names(&mut out.dirs);
    sort_names(&mut out.files);
    Some(out)
}

/// Existing directory `dir` of `drive` with each name in the case it has
/// on the drive (a typed "docs" shows as "Docs").
pub fn on_disk_case(dm: &DriveManager, drive: u8, dir: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(dir.len());
    for name in dir {
        let found = list_directory(dm, drive, &out, "*", None)
            .and_then(|l| l.dirs.into_iter().find(|d| d.eq_ignore_ascii_case(&name)));
        out.push(found.unwrap_or(name));
    }
    out
}

// ── Layout ──────────────────────────────────────────────────────────────────

/// The controls of the built-in dialog and its size.
pub fn layout(fl: u32, ok_text: &str) -> ((i32, i32), Vec<StdControl>) {
    let b = CHROME_BORDER_W;
    let save_as = is_save_as(fl);
    let (l, e) = (STD_LABEL_H, STD_ENTRY_H);
    let cx = 2 * (b + STD_MARGIN) + 2 * COL_W + STD_COL_GAP;
    let full_w = 2 * COL_W + STD_COL_GAP;
    let x0 = b + STD_MARGIN;
    let x1 = x0 + COL_W + STD_COL_GAP;
    let mut v = Vec::new();
    let label = SS_TEXT | DT_LEFT | DT_VCENTER;

    // Bottom up: buttons, lists, combination boxes, path, file name.
    let mut y = b + STD_MARGIN;
    let mut buttons = vec![(DID_OK, ok_text, BS_DEFAULT | WS_GROUP), (DID_CANCEL, "Cancel", 0)];
    if fl & FDS_APPLYBUTTON != 0 { buttons.push((DID_APPLY_PB, "Apply", 0)); }
    if fl & FDS_HELPBUTTON != 0 { buttons.push((DID_HELP_PB, "Help", BS_HELP)); }
    for (i, (id, text, style)) in buttons.into_iter().enumerate() {
        let x = x0 + i as i32 * (STD_BUTTON_W + STD_BUTTON_GAP);
        v.push(StdControl::new("WC_BUTTON", text, (x, y, STD_BUTTON_W, STD_BUTTON_H), id,
                               BS_PUSHBUTTON | WS_TABSTOP | style));
    }
    y += STD_BUTTON_H + STD_MARGIN;
    let mut files_style = WS_TABSTOP | WS_GROUP;
    if fl & FDS_MULTIPLESEL != 0 { files_style |= LS_EXTENDEDSEL; }
    if save_as && fl & FDS_ENABLEFILELB == 0 { files_style |= WS_DISABLED; }
    v.push(StdControl::new("WC_LISTBOX", "", (x0, y, COL_W, LIST_H), DID_DIRECTORY_LB, WS_TABSTOP | WS_GROUP));
    v.push(StdControl::new("WC_LISTBOX", "", (x1, y, COL_W, LIST_H), DID_FILES_LB, files_style));
    y += LIST_H;
    v.push(StdControl::new("WC_STATIC", "Directory:", (x0, y, COL_W, l), DID_DIRECTORY_TXT, label));
    v.push(StdControl::new("WC_STATIC", "File:", (x1, y, COL_W, l), DID_FILES_TXT, label));
    y += l + STD_ROW_GAP;
    // A combination box's window reaches down over what its list covers.
    let combo = |x: i32, id: u32| StdControl::new("WC_COMBOBOX", "", (x, y - COMBO_DROP_H, COL_W, e + COMBO_DROP_H), id,
                                                  CBS_DROPDOWNLIST | WS_TABSTOP | WS_GROUP);
    v.push(combo(x0, DID_FILTER_CB));
    v.push(combo(x1, DID_DRIVE_CB));
    y += e;
    v.push(StdControl::new("WC_STATIC", "Type of file:", (x0, y, COL_W, l), DID_FILTER_TXT, label));
    v.push(StdControl::new("WC_STATIC", "Drive:", (x1, y, COL_W, l), DID_DRIVE_TXT, label));
    y += l + STD_ROW_GAP;
    v.push(StdControl::new("WC_STATIC", "", (x0, y, full_w, l), FILEDLG_PATH_ID, label));
    y += l + STD_ROW_GAP;
    v.push(StdControl::new("WC_ENTRYFIELD", "", (x0, y, full_w, e), DID_FILENAME_ED, WS_TABSTOP | WS_GROUP));
    y += e;
    let caption = if save_as { "Save as filename:" } else { "Open filename:" };
    v.push(StdControl::new("WC_STATIC", caption, (x0, y, full_w, l), DID_FILENAME_TXT, label));
    y += l + STD_MARGIN;
    let cy = y + CHROME_TITLE_H + b;
    // Tab order follows creation order: entry field first.
    v.reverse();
    ((cx, cy), v)
}

// ── State ───────────────────────────────────────────────────────────────────

/// A file dialog's state.
#[derive(Debug, Clone, Default)]
pub struct FileDlgState {
    /// The program's `FILEDLG`.
    pub pfiledlg: u32,
    pub fl: u32,
    /// Codepage of the strings written back.
    pub cp: u32,
    pub drive: u8,
    pub dir: Vec<String>,
    /// The file filter (wildcard pattern).
    pub pattern: String,
    /// The types of the filter list (without "<All Files>") and the one
    /// chosen (0 = all files).
    pub types: Vec<String>,
    pub type_sel: usize,
    /// Drives offered (0 = A).
    pub drives: Vec<u8>,
    /// The files OK accepted (full paths).
    pub accepted: Vec<String>,
    /// The structure has been written back.
    pub committed: bool,
    /// The dialog is modeless (it hides itself when dismissed).
    pub modeless: bool,
}

impl Loader {
    /// Strings of a NULL-terminated `PAPSZ` array, at most `max`.
    fn read_psz_array(&self, papsz: u32, max: usize, cp: u32) -> Vec<String> {
        let mut out = Vec::new();
        if papsz == 0 {
            return out;
        }
        for i in 0..max as u32 {
            let Some(p) = papsz.checked_add(4 * i).and_then(|a| self.guest_read::<u32>(a)) else { break };
            if p == 0 { break; }
            out.push(self.read_guest_string_cp(p, cp));
        }
        out
    }

    fn psz_or(&self, psz: u32, cp: u32, default: &str) -> String {
        if psz == 0 { default.to_string() } else { self.read_guest_string_cp(psz, cp) }
    }

    /// WinFileDlg(hwndP, hwndO, pfild): open the dialog. Modal: returns
    /// TRUE when it has ended (the outcome in `lReturn`); modeless
    /// (`FDS_MODELESS`): the dialog's handle. NULLHANDLE with `lSRC` set
    /// on an error.
    pub(crate) fn win_file_dlg(&self, tid: u32, hwnd_p: u32, hwnd_o: u32, pfild: u32) -> ApiResult {
        let Some(fd) = self.guest_read_bytes(pfild, FILEDLG_SIZE).filter(|_| pfild != 0)
            .as_deref().and_then(parse_filedlg) else {
            return ApiResult::Normal(0);
        };
        let fail = |code: u32| {
            let _ = self.guest_write::<u32>(pfild + FD_LSRC, code);
            ApiResult::Normal(0)
        };
        if (fd.cb_size as usize) < FILEDLG_SIZE {
            return fail(FDS_ERR_INVALID_VERSION);
        }
        let cp = self.thread_codepage(tid);
        let Some(hmq) = self.shared.window_mgr.lock_or_recover().thread_queue(tid) else {
            return fail(FDS_ERR_INVALID_DIALOG);
        };

        // Where it starts, and the drives it offers.
        let (cur_drive, drive_dirs, mounted) = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            let dirs: Vec<Vec<String>> = (0..26u8).map(|d| split_dir(dm.current_dir(d))).collect();
            (dm.current_disk(), dirs, dm.logical_drive_map())
        };
        let drive_dir = |d: u8| drive_dirs.get(d as usize).cloned().unwrap_or_default();
        let idrive = (fd.psz_idrive != 0).then(|| self.read_guest_string_cp(fd.psz_idrive, cp));
        let start_drive = idrive.as_deref().and_then(drive_letter).filter(|&d| mounted & (1 << d) != 0)
            .unwrap_or(cur_drive);
        let full_file = super::codepage::cp_decode(&fd.full_file, cp);
        let (mut drive, mut dir, name, pattern) =
            initial_place(&full_file, start_drive, &drive_dir(start_drive), drive_dir);
        let listed = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            list_directory(&dm, drive, &dir, &pattern, None).is_some()
        };
        if mounted & (1 << drive) == 0 || !listed {
            // TODO(unverified): PM may fail with FDS_ERR_INVALID_DRIVE /
            // FDS_ERR_INVALID_PATHFILE instead.
            drive = start_drive;
            dir = drive_dir(start_drive);
        }
        let mut drives: Vec<u8> = self.read_psz_array(fd.papsz_idrive_list, MAX_DRIVES, cp).iter()
            .filter_map(|s| drive_letter(s)).filter(|&d| mounted & (1 << d) != 0).collect();
        if drives.is_empty() {
            drives = (0..26u8).filter(|&d| mounted & (1 << d) != 0).collect();
        }
        if !drives.contains(&drive) {
            drives.push(drive);
            drives.sort_unstable();
        }
        let mut types = self.read_psz_array(fd.papsz_itype_list, MAX_TYPES, cp);
        let itype = (fd.psz_itype != 0).then(|| self.read_guest_string_cp(fd.psz_itype, cp));
        if let Some(t) = &itype && !types.contains(t) {
            types.insert(0, t.clone());
        }
        let type_sel = itype.as_ref().and_then(|t| types.iter().position(|x| x == t)).map_or(0, |i| i + 1);

        let save_as = is_save_as(fd.fl);
        let title = self.psz_or(fd.psz_title, cp, if save_as { "Save As" } else { "Open" });
        let ok_text = self.psz_or(fd.psz_ok_button, cp, "OK");
        let modeless = fd.fl & FDS_MODELESS != 0;

        // The dialog: the program's template, or the built-in one.
        let hwnd = if fd.fl & FDS_CUSTOM != 0 {
            match self.std_custom_dialog(tid, hwnd_p, hwnd_o, fd.pfn_dlg_proc, fd.hmod, fd.dlg_id as u32) {
                Some(h) => h,
                None => return fail(FDS_ERR_DIALOG_LOAD_ERROR),
            }
        } else {
            let (size, controls) = layout(fd.fl, &ok_text);
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let area = parent_area(&wm, hwnd_p);
            let pos = place(fd.fl & FDS_CENTER != 0, area, (fd.x as i32, fd.y as i32), size);
            create_std_dialog(&mut wm, hmq, hwnd_o, fd.pfn_dlg_proc, &title, pos, size, &controls)
        };
        if fd.fl & FDS_CUSTOM != 0 {
            self.std_center_custom(hwnd, hwnd_p, fd.fl & FDS_CENTER != 0);
            if fd.psz_title != 0 && let Some(w) = self.shared.window_mgr.lock_or_recover().get_window_mut(hwnd) {
                w.text = title;
            }
            if fd.psz_ok_button != 0 {
                self.std_set_item_text(hwnd, DID_OK, &ok_text);
            }
        }
        let state = FileDlgState {
            pfiledlg: pfild, fl: fd.fl, cp, drive, dir, pattern, types, type_sel, drives,
            accepted: Vec::new(), committed: false, modeless,
        };
        self.shared.window_mgr.lock_or_recover().std_dialogs.insert(hwnd, StdDialog::File(Box::new(state)));

        // Fill the controls.
        let (types_items, drive_items, sel_drive) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let s = wm.std_dialogs.file(hwnd).expect("just registered");
            let mut t = vec![ALL_FILES.to_string()];
            t.extend(s.types.iter().cloned());
            let d: Vec<String> = s.drives.iter().map(|&d| format!("{}:", (b'A' + d) as char)).collect();
            (t, d, s.drives.iter().position(|&d| d == s.drive).unwrap_or(0))
        };
        self.std_set_list(hwnd, DID_FILTER_CB, types_items);
        self.std_select_item(hwnd, DID_FILTER_CB, type_sel);
        self.std_set_list(hwnd, DID_DRIVE_CB, drive_items);
        self.std_select_item(hwnd, DID_DRIVE_CB, sel_drive);
        self.file_dlg_refresh(hwnd);
        let entry_text = if name.is_empty() {
            self.shared.window_mgr.lock_or_recover().std_dialogs.file(hwnd).map(|s| s.pattern.clone()).unwrap_or_default()
        } else {
            name
        };
        self.std_set_item_text(hwnd, DID_FILENAME_ED, &entry_text);
        let focus = self.shared.window_mgr.lock_or_recover().find_child_by_id(hwnd, DID_FILENAME_ED).unwrap_or(0);
        self.std_dialog_start(hwnd, hmq, focus, pfild, modeless, fd.pfn_dlg_proc)
    }

    /// Fill the directory and file lists and the path for the dialog's
    /// current place.
    fn file_dlg_refresh(&self, hwnd: u32) {
        let Some(s) = self.shared.window_mgr.lock_or_recover().std_dialogs.file(hwnd).cloned() else { return };
        let type_name = s.type_sel.checked_sub(1).and_then(|i| s.types.get(i)).map(String::as_str);
        let listing = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            list_directory(&dm, s.drive, &s.dir, &s.pattern, type_name).unwrap_or_default()
        };
        let mut dirs = Vec::new();
        if !s.dir.is_empty() {
            dirs.push("..".to_string());
        }
        dirs.extend(listing.dirs);
        self.std_set_list(hwnd, DID_DIRECTORY_LB, dirs);
        self.std_set_list(hwnd, DID_FILES_LB, listing.files);
        self.std_set_item_text(hwnd, FILEDLG_PATH_ID, &full_path(s.drive, &s.dir));
    }

    /// Go to directory `dir` of `drive` when it exists; returns whether it did.
    fn file_dlg_go(&self, hwnd: u32, drive: u8, dir: Vec<String>) -> bool {
        let dir = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            if list_directory(&dm, drive, &dir, "*", None).is_none() {
                return false;
            }
            on_disk_case(&dm, drive, dir)
        };
        let drive_index = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(s) = wm.std_dialogs.file_mut(hwnd) else { return false };
            let changed = s.drive != drive;
            s.drive = drive;
            s.dir = dir;
            changed.then(|| s.drives.iter().position(|&d| d == drive)).flatten()
        };
        if let Some(i) = drive_index {
            self.std_select_item(hwnd, DID_DRIVE_CB, i);
        }
        self.file_dlg_refresh(hwnd);
        true
    }

    /// `WinDefFileDlgProc` (and the messages of a file dialog without a
    /// dialog procedure): the file dialog's default processing.
    pub(crate) fn file_dlg_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> ApiResult {
        let state = self.shared.window_mgr.lock_or_recover().std_dialogs.file(hwnd).cloned();
        let Some(s) = state else { return self.def_dlg_proc(hwnd, msg, mp1, mp2) };
        match msg {
            WM_CONTROL => {
                let (id, code) = (mp1 & 0xFFFF, mp1 >> 16);
                match (id, code) {
                    (DID_FILES_LB, LN_SELECT) => {
                        let sel = self.std_selected(hwnd, DID_FILES_LB);
                        let cursor = self.list_cursor_text(hwnd, DID_FILES_LB);
                        if let Some(t) = cursor.or_else(|| sel.first().map(|x| x.1.clone())) {
                            self.std_set_item_text(hwnd, DID_FILENAME_ED, &t);
                        }
                    }
                    (DID_FILES_LB, LN_ENTER) => self.file_dlg_ok(hwnd, false),
                    (DID_DIRECTORY_LB, LN_ENTER) => {
                        if let Some((_, name)) = self.std_selected(hwnd, DID_DIRECTORY_LB).into_iter().next() {
                            let mut dir = s.dir.clone();
                            if name == ".." { dir.pop(); } else { dir.push(name); }
                            self.file_dlg_go(hwnd, s.drive, dir);
                        }
                    }
                    (DID_DRIVE_CB, CBN_LBSELECT) | (DID_DRIVE_CB, CBN_ENTER) => {
                        let picked = self.std_selected(hwnd, DID_DRIVE_CB).into_iter().next()
                            .and_then(|(_, t)| drive_letter(&t));
                        if let Some(d) = picked.filter(|&d| d != s.drive) {
                            let dir = split_dir(self.shared.drive_mgr.lock_or_recover().current_dir(d));
                            if !self.file_dlg_go(hwnd, d, dir) {
                                self.file_dlg_go(hwnd, d, Vec::new());
                            }
                        }
                    }
                    (DID_FILTER_CB, CBN_LBSELECT) | (DID_FILTER_CB, CBN_ENTER) => {
                        let sel = self.std_selected(hwnd, DID_FILTER_CB).into_iter().next().map_or(0, |x| x.0);
                        if sel != s.type_sel {
                            if let Some(st) = self.shared.window_mgr.lock_or_recover().std_dialogs.file_mut(hwnd) {
                                st.type_sel = sel;
                            }
                            self.file_dlg_refresh(hwnd);
                        }
                    }
                    _ => {}
                }
                ApiResult::Normal(0)
            }
            WM_COMMAND => {
                match mp1 & 0xFFFF {
                    DID_OK => self.file_dlg_ok(hwnd, false),
                    DID_APPLY_PB => self.file_dlg_ok(hwnd, true),
                    DID_CANCEL => self.dismiss_dialog(hwnd, DID_CANCEL),
                    _ => return self.def_dlg_proc(hwnd, msg, mp1, mp2),
                }
                ApiResult::Normal(0)
            }
            WM_CLOSE => {
                self.dismiss_dialog(hwnd, DID_CANCEL);
                ApiResult::Normal(0)
            }
            // No help viewer (helpmgr.rs).
            WM_HELP => ApiResult::Normal(0),
            _ => self.def_dlg_proc(hwnd, msg, mp1, mp2),
        }
    }

    /// The text of the item with the cursor in list `id` of `dlg`.
    fn list_cursor_text(&self, dlg: u32, id: u32) -> Option<String> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let w = wm.find_child_by_id(dlg, id).and_then(|h| wm.get_window(h))?;
        w.listbox_sel.cursor().and_then(|i| w.listbox_items.get(i).cloned())
    }

    /// OK (or, with `apply`, Apply): act on the entry field / selection.
    fn file_dlg_ok(&self, hwnd: u32, apply: bool) {
        let Some(s) = self.shared.window_mgr.lock_or_recover().std_dialogs.file(hwnd).cloned() else { return };
        if s.committed {
            return;
        }
        let selected = self.std_selected(hwnd, DID_FILES_LB);
        let files: Vec<String> = if s.fl & FDS_MULTIPLESEL != 0 && selected.len() > 1 {
            selected.into_iter().take(MAX_FQF_FILES).map(|(_, n)| {
                let mut c = s.dir.clone();
                c.push(n);
                full_path(s.drive, &c)
            }).collect()
        } else {
            let text = self.std_item_text(hwnd, DID_FILENAME_ED);
            let text = if text.trim().is_empty() {
                match selected.into_iter().next() { Some((_, n)) => n, None => return }
            } else {
                text
            };
            match self.file_dlg_resolve(hwnd, &s, &text) {
                Some(path) => vec![path],
                None => return,
            }
        };
        let accepted = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(st) = wm.std_dialogs.file_mut(hwnd) else { return };
            st.accepted = files;
            st.accepted.clone()
        };
        if apply {
            // TODO(unverified): what PM writes and sends for Apply.
            self.file_dlg_write(&s, DID_APPLY_PB, &accepted);
        } else {
            self.dismiss_dialog(hwnd, DID_OK);
        }
    }

    /// The entry field's `text`: `Some(full path)` of a file to return;
    /// `None` when it was a pattern, a directory or not valid (the dialog
    /// stays, its lists changed for a pattern or a directory).
    fn file_dlg_resolve(&self, hwnd: u32, s: &FileDlgState, text: &str) -> Option<String> {
        let drive_dir = |d: u8| split_dir(self.shared.drive_mgr.lock_or_recover().current_dir(d));
        let Some((drive, mut comps)) = resolve_input(text, s.drive, &s.dir, drive_dir) else {
            log::info!("WinFileDlg: '{text}' is not a valid file name");
            return None;
        };
        let mounted = self.shared.drive_mgr.lock_or_recover().logical_drive_map();
        if mounted & (1 << drive) == 0 {
            return None;
        }
        let last = comps.last().cloned().unwrap_or_default();
        if has_wildcard(&last) {
            comps.pop();
            if let Some(st) = self.shared.window_mgr.lock_or_recover().std_dialogs.file_mut(hwnd) {
                st.pattern = last.clone();
            }
            if !self.file_dlg_go(hwnd, drive, comps) {
                self.file_dlg_refresh(hwnd);
            }
            self.std_set_item_text(hwnd, DID_FILENAME_ED, &last);
            return None;
        }
        if comps.is_empty() || self.file_dlg_go(hwnd, drive, comps.clone()) {
            // A directory (or a drive's root): entered.
            if comps.is_empty() {
                self.file_dlg_go(hwnd, drive, Vec::new());
            }
            let pattern = self.shared.window_mgr.lock_or_recover().std_dialogs.file(hwnd).map(|s| s.pattern.clone());
            self.std_set_item_text(hwnd, DID_FILENAME_ED, &pattern.unwrap_or_default());
            return None;
        }
        // A file in an existing directory, in the case it has there.
        let name = comps.pop().unwrap_or_default();
        let listing = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            list_directory(&dm, drive, &comps, "*", None)
        };
        let Some(listing) = listing else {
            log::info!("WinFileDlg: the directory of '{text}' does not exist");
            return None;
        };
        let name = listing.files.into_iter().find(|f| f.eq_ignore_ascii_case(&name)).unwrap_or(name);
        comps.push(name);
        Some(full_path(drive, &comps))
    }

    /// The dialog is being dismissed with `result`: write the `FILEDLG`
    /// back and, when modeless, take the dialog off the screen.
    pub(super) fn file_dlg_commit(&self, hwnd: u32, result: u32) {
        let s = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(st) = wm.std_dialogs.file_mut(hwnd) else { return };
            st.committed = true;
            st.clone()
        };
        let mut files = s.accepted.clone();
        if result == DID_OK && files.is_empty() {
            // Dismissed by the dialog procedure: whatever the entry field names.
            let text = self.std_item_text(hwnd, DID_FILENAME_ED);
            let drive_dir = |d: u8| split_dir(self.shared.drive_mgr.lock_or_recover().current_dir(d));
            if let Some((d, c)) = resolve_input(&text, s.drive, &s.dir, drive_dir) {
                files.push(full_path(d, &c));
            }
        }
        self.file_dlg_write(&s, result, &files);
        if s.modeless {
            self.std_hide_modeless(hwnd);
        }
    }

    /// Write `lReturn` = `result`, `lSRC` and, for OK / Apply, the files.
    fn file_dlg_write(&self, s: &FileDlgState, result: u32, files: &[String]) {
        let p = s.pfiledlg;
        let _ = self.guest_write::<u32>(p + FD_LRETURN, result);
        let _ = self.guest_write::<u32>(p + FD_LSRC, FDS_SUCCESSFUL);
        if result == DID_CANCEL || files.is_empty() {
            return;
        }
        self.std_write_string(p + FD_SZ_FULL_FILE as u32, CCHMAXPATH, &files[0], s.cp);
        let _ = self.guest_write::<u32>(p + FD_UL_FQF_COUNT, files.len() as u32);
        if s.fl & FDS_MULTIPLESEL != 0 {
            let list = self.alloc_file_list(files, s.cp);
            let _ = self.guest_write::<u32>(p + FD_PAPSZ_FQ_FILENAME, list);
        }
    }

    /// Allocate a file list for `papszFQFilename`: `files.len()` pointers,
    /// then a `CCHMAXPATH`-byte name for each; 0 when memory runs out.
    fn alloc_file_list(&self, files: &[String], cp: u32) -> u32 {
        let n = files.len().min(MAX_FQF_FILES);
        let size = n * (4 + CCHMAXPATH);
        let Some(block) = self.shared.mem_mgr.lock_or_recover().alloc(size as u32) else { return 0 };
        let names = block + 4 * n as u32;
        for (i, f) in files.iter().take(n).enumerate() {
            let at = names + (i * CCHMAXPATH) as u32;
            let _ = self.guest_write::<u32>(block + 4 * i as u32, at);
            self.std_write_string(at, CCHMAXPATH, f, cp);
        }
        self.shared.window_mgr.lock_or_recover().std_dialogs.add_file_list(block);
        block
    }

    /// WinFreeFileDlgList(papszFQFilename): free a list a file dialog
    /// returned. FALSE for anything else.
    pub fn win_free_file_dlg_list(&self, papsz: u32) -> u32 {
        if papsz == 0 || !self.shared.window_mgr.lock_or_recover().std_dialogs.take_file_list(papsz) {
            return 0;
        }
        self.shared.mem_mgr.lock_or_recover().free(papsz) as u32
    }
}

/// Whether `fl` asks for the Save As dialog: `FDS_SAVEAS_DIALOG` without
/// `FDS_OPEN_DIALOG` (with neither or both it is an Open dialog —
/// unverified).
pub fn is_save_as(fl: u32) -> bool {
    fl & FDS_SAVEAS_DIALOG != 0 && fl & FDS_OPEN_DIALOG == 0
}

/// The drive (0 = A) of "C:" / "c" / "C:\…"; `None` without a letter.
pub fn drive_letter(s: &str) -> Option<u8> {
    let b = s.trim().as_bytes();
    let l = b.first()?.to_ascii_uppercase();
    (l.is_ascii_uppercase() && (b.len() == 1 || b[1] == b':')).then(|| l - b'A')
}

/// The components of a drive manager directory (`A/B` or `A\B`).
fn split_dir(dir: &str) -> Vec<String> {
    dir.split(['/', '\\']).filter(|p| !p.is_empty()).map(str::to_string).collect()
}

/// `FDS_ERR_INVALID_DIALOG`: no message queue to run the dialog on.
pub const FDS_ERR_INVALID_DIALOG: u32 = 3;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::vfs_hostdir::HostDirBackend;

    fn comps(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wildcards_match_ignoring_case() {
        assert!(wildcard_match("*", "readme.txt"));
        assert!(wildcard_match("*.TXT", "readme.txt"));
        assert!(wildcard_match("*.txt", "A.B.TXT"));
        assert!(!wildcard_match("*.txt", "readme.doc"));
        assert!(wildcard_match("r?ad*", "READ"));
        assert!(!wildcard_match("r?ad*", "rad"));
        assert!(wildcard_match("*.*", "makefile"), "a name without a dot matches *.*");
        assert!(wildcard_match("make.*", "make"));
        assert!(!wildcard_match("*.c", "make"));
        assert!(wildcard_match("a*b*c", "aXXbYYc"));
        assert!(!wildcard_match("a*b*c", "aXXbYY"));
        assert!(has_wildcard("*.c") && has_wildcard("a?") && !has_wildcard("abc"));
    }

    #[test]
    fn input_resolves_against_the_current_directory() {
        let cur = comps(&["WORK"]);
        let other = |d: u8| if d == 3 { comps(&["DDIR"]) } else { Vec::new() };
        assert_eq!(resolve_input("a.txt", 2, &cur, other), Some((2, comps(&["WORK", "a.txt"]))));
        assert_eq!(resolve_input("..\\b.txt", 2, &cur, other), Some((2, comps(&["b.txt"]))));
        assert_eq!(resolve_input("..\\..\\..\\b.txt", 2, &cur, other), Some((2, comps(&["b.txt"]))),
                   ".. stops at the root");
        assert_eq!(resolve_input("\\x\\.\\y", 2, &cur, other), Some((2, comps(&["x", "y"]))));
        assert_eq!(resolve_input("d:f.c", 2, &cur, other), Some((3, comps(&["DDIR", "f.c"]))));
        assert_eq!(resolve_input("D:\\", 2, &cur, other), Some((3, Vec::new())));
        assert_eq!(resolve_input("C:/a/b", 2, &cur, other), Some((2, comps(&["a", "b"]))));
        assert_eq!(resolve_input("*.c", 2, &cur, other), Some((2, comps(&["WORK", "*.c"]))));
        assert_eq!(resolve_input("a*\\b", 2, &cur, other), None, "a pattern only last");
        assert_eq!(resolve_input("a|b", 2, &cur, other), None);
        assert_eq!(resolve_input("1:\\x", 2, &cur, other), None);
        assert_eq!(resolve_input("   ", 2, &cur, other), None);
        assert_eq!(resolve_input(&"x".repeat(300), 2, &cur, other), None, "longer than CCHMAXPATH");
        let deep = vec!["a"; 200].join("\\");
        assert_eq!(resolve_input(&deep, 2, &[], other), None, "too long");
        assert_eq!(full_path(2, &comps(&["WORK", "a.txt"])), "C:\\WORK\\a.txt");
        assert_eq!(full_path(3, &[]), "D:\\");
    }

    #[test]
    fn the_initial_place_comes_from_sz_full_file() {
        let cur = comps(&["HOME"]);
        let none = |_| Vec::new();
        assert_eq!(initial_place("", 2, &cur, none), (2, cur.clone(), String::new(), "*".into()));
        assert_eq!(initial_place("C:\\DATA\\*.TXT", 2, &cur, none),
                   (2, comps(&["DATA"]), String::new(), "*.TXT".into()));
        assert_eq!(initial_place("C:\\DATA\\notes.txt", 2, &cur, none),
                   (2, comps(&["DATA"]), "notes.txt".into(), "*".into()));
        assert_eq!(initial_place("C:\\DATA\\", 2, &cur, none), (2, comps(&["DATA"]), String::new(), "*".into()));
        assert_eq!(initial_place("new.txt", 2, &cur, none), (2, comps(&["HOME"]), "new.txt".into(), "*".into()));
        assert_eq!(initial_place("a<b", 2, &cur, none), (2, cur.clone(), String::new(), "*".into()), "invalid");
    }

    #[test]
    fn drive_letters() {
        assert_eq!(drive_letter("C:"), Some(2));
        assert_eq!(drive_letter("d"), Some(3));
        assert_eq!(drive_letter("c:\\x"), Some(2));
        assert_eq!(drive_letter("CD"), None);
        assert_eq!(drive_letter(""), None);
        assert_eq!(drive_letter("1:"), None);
        assert_eq!(split_dir("A/B\\C"), comps(&["A", "B", "C"]));
        assert!(split_dir("").is_empty());
    }

    #[test]
    fn type_attributes_are_decoded() {
        // EAT_MVMT, codepage 0, two EAT_ASCII entries.
        let mut v = vec![0xDF, 0xFF, 0, 0, 2, 0];
        for t in ["Plain Text", "C Code"] {
            v.extend_from_slice(&[0xFD, 0xFF]);
            v.extend_from_slice(&(t.len() as u16).to_le_bytes());
            v.extend_from_slice(t.as_bytes());
        }
        assert_eq!(type_names(&v), ["Plain Text", "C Code"]);
        let single = [0xFD, 0xFF, 3, 0, b'B', b'i', b'n'];
        assert_eq!(type_names(&single), ["Bin"]);
        // Truncated or foreign values give what is complete.
        assert_eq!(type_names(&v[..v.len() - 1]), ["Plain Text"]);
        assert!(type_names(&[0xDE, 0xFF]).is_empty());
        assert!(type_names(&[]).is_empty());
        let mut huge = vec![0xDF, 0xFF, 0, 0, 0xFF, 0xFF];
        huge.extend_from_slice(&[0xFD, 0xFF, 0, 0]);
        assert_eq!(type_names(&huge).len(), 1, "a count past the data stops at the data");
    }

    #[test]
    fn filedlg_is_parsed_at_the_header_offsets() {
        let mut b = vec![0u8; FILEDLG_SIZE];
        b[0..4].copy_from_slice(&(FILEDLG_SIZE as u32).to_le_bytes());
        b[4..8].copy_from_slice(&(FDS_OPEN_DIALOG | FDS_CENTER).to_le_bytes());
        b[20..24].copy_from_slice(&0x1234u32.to_le_bytes());
        b[48..52].copy_from_slice(&7u32.to_le_bytes());
        b[52..58].copy_from_slice(b"C:\\*.C");
        b[320..322].copy_from_slice(&300u16.to_le_bytes());
        b[322..324].copy_from_slice(&(-5i16).to_le_bytes());
        b[324..326].copy_from_slice(&40i16.to_le_bytes());
        let f = parse_filedlg(&b).unwrap();
        assert_eq!((f.cb_size, f.fl, f.psz_title, f.hmod), (328, FDS_OPEN_DIALOG | FDS_CENTER, 0x1234, 7));
        assert_eq!(f.full_file, b"C:\\*.C");
        assert_eq!((f.dlg_id, f.x, f.y), (300, -5, 40));
        assert!(parse_filedlg(&b[..FILEDLG_SIZE - 1]).is_none());
        // An unterminated szFullFile stops at CCHMAXPATH.
        b[52..52 + CCHMAXPATH].fill(b'x');
        assert_eq!(parse_filedlg(&b).unwrap().full_file.len(), CCHMAXPATH);
    }

    #[test]
    fn the_layout_has_the_standard_controls() {
        let ((cx, cy), v) = layout(FDS_OPEN_DIALOG, "Open");
        let ids: Vec<u32> = v.iter().map(|c| c.id).collect();
        for id in [DID_FILENAME_TXT, DID_FILENAME_ED, DID_FILTER_CB, DID_DRIVE_CB, DID_DIRECTORY_LB,
                   DID_FILES_LB, DID_OK, DID_CANCEL, FILEDLG_PATH_ID] {
            assert!(ids.contains(&id), "control {id}");
        }
        assert!(!ids.contains(&DID_HELP_PB) && !ids.contains(&DID_APPLY_PB));
        assert_eq!(v[0].id, DID_FILENAME_TXT, "top first");
        assert_eq!(v.iter().find(|c| c.id == DID_OK).unwrap().text, "Open");
        // Everything inside the dialog.
        for c in &v {
            let (x, y, w, h) = c.rect;
            assert!(x >= CHROME_BORDER_W && x + w <= cx - CHROME_BORDER_W, "{c:?}");
            assert!(y >= CHROME_BORDER_W && y + h <= cy - CHROME_TITLE_H, "{c:?}");
        }
        let files = |fl| layout(fl, "OK").1.into_iter().find(|c| c.id == DID_FILES_LB).unwrap().style;
        assert_eq!(files(FDS_SAVEAS_DIALOG) & WS_DISABLED, WS_DISABLED, "Save As: file list disabled");
        assert_eq!(files(FDS_SAVEAS_DIALOG | FDS_ENABLEFILELB) & WS_DISABLED, 0);
        assert_eq!(files(FDS_MULTIPLESEL) & LS_EXTENDEDSEL, LS_EXTENDEDSEL);
        let (_, v) = layout(FDS_HELPBUTTON | FDS_APPLYBUTTON | FDS_SAVEAS_DIALOG, "OK");
        assert_eq!(v.iter().find(|c| c.id == DID_HELP_PB).unwrap().style & BS_HELP, BS_HELP);
        assert!(v.iter().any(|c| c.id == DID_APPLY_PB));
        assert_eq!(v[0].text, "Save as filename:");
    }

    /// A drive manager with a fresh drive D.
    fn drives(root: &std::path::Path) -> DriveManager {
        let mut dm = DriveManager::new();
        dm.mount(3, Box::new(HostDirBackend::new(root.to_path_buf()).unwrap()));
        dm
    }

    #[test]
    fn directories_list_subdirectories_and_matching_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("Sub/Deep")).unwrap();
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        for f in ["b.txt", "A.TXT", "c.doc", "Sub/inner.txt"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        let dm = drives(root);
        let l = list_directory(&dm, 3, &[], "*", None).unwrap();
        assert_eq!(l.dirs, ["alpha", "Sub"]);
        assert_eq!(l.files, ["A.TXT", "b.txt", "c.doc"]);
        let l = list_directory(&dm, 3, &[], "*.txt", None).unwrap();
        assert_eq!(l.files, ["A.TXT", "b.txt"]);
        let l = list_directory(&dm, 3, &comps(&["sub"]), "*", None).unwrap();
        assert_eq!((l.dirs, l.files), (comps(&["Deep"]), comps(&["inner.txt"])), "case-insensitive path");
        assert!(list_directory(&dm, 3, &comps(&["nope"]), "*", None).is_none());
        assert!(list_directory(&dm, 4, &[], "*", None).is_none(), "unmounted drive");
        let empty = list_directory(&dm, 3, &comps(&["Sub", "Deep"]), "*", None).unwrap();
        assert_eq!(empty, Listing::default());
        // No type attributes on these files.
        assert!(list_directory(&dm, 3, &[], "*", Some("Plain Text")).unwrap().files.is_empty());
        // A path that leaves the drive is refused by the drive itself.
        assert!(list_directory(&dm, 3, &comps(&["..", ".."]), "*", None).is_none_or(|l| !l.dirs.contains(&"proc".to_string())));
    }

    // ── The dialog on a mock loader ─────────────────────────────────────────

    use crate::loader::mutex_ext::MutexExt;

    const FD: u32 = 0x0008_0000;
    const STR: u32 = 0x0008_1000;

    /// A mock loader with a message queue for thread 0, an owner frame and
    /// drive D (current) on a fresh directory with files and folders.
    fn session() -> (Loader, u32, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Docs/Old")).unwrap();
        for f in ["alpha.txt", "Beta.TXT", "gamma.doc", "Docs/inside.txt"] {
            std::fs::write(tmp.path().join(f), b"x").unwrap();
        }
        let loader = Loader::new_mock();
        {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            dm.mount(3, Box::new(HostDirBackend::new(tmp.path().to_path_buf()).unwrap()));
            dm.set_current_disk(4).unwrap();
        }
        let frame = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), 0, hmq);
            wm.z_push_top(frame);
            frame
        };
        (loader, frame, tmp)
    }

    /// Write a FILEDLG with `fl` and `full_file`.
    fn filedlg(l: &Loader, fl: u32, full_file: &str) {
        let mut b = vec![0u8; FILEDLG_SIZE];
        b[0..4].copy_from_slice(&(FILEDLG_SIZE as u32).to_le_bytes());
        b[4..8].copy_from_slice(&fl.to_le_bytes());
        b[52..52 + full_file.len()].copy_from_slice(full_file.as_bytes());
        l.guest_write_bytes(FD, &b).unwrap();
    }

    fn open(l: &Loader, frame: u32) -> u32 {
        match l.win_file_dlg(0, HWND_DESKTOP, frame, FD) {
            ApiResult::DlgRunLoop { hwnd_dlg, .. } => hwnd_dlg,
            other => panic!("expected the modal loop, got {other:?}"),
        }
    }

    fn items(l: &Loader, dlg: u32, id: u32) -> Vec<String> {
        let wm = l.shared.window_mgr.lock_or_recover();
        let h = wm.find_child_by_id(dlg, id).unwrap();
        let h = wm.get_window(h).unwrap().combo.map_or(h, |c| c.list);
        wm.get_window(h).unwrap().listbox_items.clone()
    }

    fn full_file(l: &Loader) -> String {
        l.read_guest_string(FD + FD_SZ_FULL_FILE as u32)
    }

    fn type_into(l: &Loader, dlg: u32, text: &str) {
        l.std_set_item_text(dlg, DID_FILENAME_ED, text);
    }

    fn dismissed(l: &Loader, dlg: u32) -> Option<u32> {
        l.shared.window_mgr.lock_or_recover().get_window(dlg)
            .and_then(|w| w.dialog_dismissed.then_some(w.dialog_result))
    }

    #[test]
    fn the_open_dialog_lists_the_directory_named_by_sz_full_file() {
        let (l, frame, _tmp) = session();
        filedlg(&l, FDS_OPEN_DIALOG | FDS_CENTER, "D:\\*.txt");
        let dlg = open(&l, frame);
        assert_eq!(items(&l, dlg, DID_FILES_LB), ["alpha.txt", "Beta.TXT"]);
        assert_eq!(items(&l, dlg, DID_DIRECTORY_LB), ["Docs"], "no .. at the root");
        assert_eq!(items(&l, dlg, DID_FILTER_CB), [ALL_FILES]);
        assert!(items(&l, dlg, DID_DRIVE_CB).contains(&"D:".to_string()));
        assert_eq!(l.std_item_text(dlg, DID_DRIVE_CB), "D:");
        assert_eq!(l.std_item_text(dlg, DID_FILENAME_ED), "*.txt");
        assert_eq!(l.std_item_text(dlg, FILEDLG_PATH_ID), "D:\\");
        let wm = l.shared.window_mgr.lock_or_recover();
        let d = wm.get_window(dlg).unwrap();
        assert_eq!((d.text.as_str(), d.pfn_wp), ("Open", 0));
        assert_eq!(d.window_ulong.get(&QWL_USER), Some(&FD), "QWL_USER holds the FILEDLG");
        assert_eq!(wm.focus_hwnd, wm.find_child_by_id(dlg, DID_FILENAME_ED).unwrap());
        assert_ne!(wm.get_window(frame).unwrap().style & WS_DISABLED, 0, "modal: owner disabled");
    }

    #[test]
    fn ok_returns_the_typed_file_in_the_case_found_on_disk() {
        let (l, frame, _tmp) = session();
        filedlg(&l, FDS_OPEN_DIALOG, "");
        let dlg = open(&l, frame);
        type_into(&l, dlg, "beta.txt");
        l.file_dlg_message(dlg, WM_COMMAND, DID_OK, 0);
        assert_eq!(dismissed(&l, dlg), Some(DID_OK));
        assert_eq!(full_file(&l), "D:\\Beta.TXT");
        assert_eq!(l.guest_read::<u32>(FD + FD_LRETURN), Some(DID_OK));
        assert_eq!(l.guest_read::<u32>(FD + FD_UL_FQF_COUNT), Some(1));
        assert_eq!(l.std_dialog_return(dlg, DID_OK), 1, "WinFileDlg returns TRUE");
        // A new name in an existing directory is fine for Save As too.
        filedlg(&l, FDS_SAVEAS_DIALOG, "D:\\Docs\\");
        let dlg = open(&l, frame);
        assert_eq!(l.shared.window_mgr.lock_or_recover().get_window(dlg).unwrap().text, "Save As");
        type_into(&l, dlg, "new file.txt");
        l.file_dlg_message(dlg, WM_COMMAND, DID_OK, 0);
        assert_eq!(full_file(&l), "D:\\Docs\\new file.txt");
    }

    #[test]
    fn patterns_directories_and_bad_names_keep_the_dialog_open() {
        let (l, frame, _tmp) = session();
        filedlg(&l, FDS_OPEN_DIALOG, "");
        let dlg = open(&l, frame);
        type_into(&l, dlg, "*.doc");
        l.file_dlg_message(dlg, WM_COMMAND, DID_OK, 0);
        assert_eq!(dismissed(&l, dlg), None);
        assert_eq!(items(&l, dlg, DID_FILES_LB), ["gamma.doc"]);
        type_into(&l, dlg, "docs");
        l.file_dlg_message(dlg, WM_COMMAND, DID_OK, 0);
        assert_eq!(dismissed(&l, dlg), None);
        assert_eq!(l.std_item_text(dlg, FILEDLG_PATH_ID), "D:\\Docs");
        assert_eq!(items(&l, dlg, DID_DIRECTORY_LB), ["..", "Old"]);
        assert!(items(&l, dlg, DID_FILES_LB).is_empty(), "the *.doc filter stays");
        assert_eq!(l.std_item_text(dlg, DID_FILENAME_ED), "*.doc");
        for bad in ["a|b", "nowhere\\x.txt", "Z:\\x"] {
            type_into(&l, dlg, bad);
            l.file_dlg_message(dlg, WM_COMMAND, DID_OK, 0);
            assert_eq!(dismissed(&l, dlg), None, "{bad}");
        }
        // A double click on ".." goes up.
        l.std_select_item(dlg, DID_DIRECTORY_LB, 0);
        l.file_dlg_message(dlg, WM_CONTROL, (LN_ENTER << 16) | DID_DIRECTORY_LB, 0);
        assert_eq!(l.std_item_text(dlg, FILEDLG_PATH_ID), "D:\\");
    }

    #[test]
    fn picking_a_file_fills_the_entry_field_and_a_double_click_is_ok() {
        let (l, frame, _tmp) = session();
        filedlg(&l, FDS_OPEN_DIALOG, "D:\\");
        let dlg = open(&l, frame);
        l.std_select_item(dlg, DID_FILES_LB, 2);
        l.file_dlg_message(dlg, WM_CONTROL, (LN_SELECT << 16) | DID_FILES_LB, 0);
        assert_eq!(l.std_item_text(dlg, DID_FILENAME_ED), "gamma.doc");
        l.file_dlg_message(dlg, WM_CONTROL, (LN_ENTER << 16) | DID_FILES_LB, 0);
        assert_eq!(dismissed(&l, dlg), Some(DID_OK));
        assert_eq!(full_file(&l), "D:\\gamma.doc");
    }

    #[test]
    fn cancel_leaves_sz_full_file_alone() {
        let (l, frame, _tmp) = session();
        filedlg(&l, FDS_OPEN_DIALOG, "D:\\keep.txt");
        let dlg = open(&l, frame);
        l.file_dlg_message(dlg, WM_CLOSE, 0, 0);
        assert_eq!(dismissed(&l, dlg), Some(DID_CANCEL));
        assert_eq!(l.guest_read::<u32>(FD + FD_LRETURN), Some(DID_CANCEL));
        assert_eq!(full_file(&l), "D:\\keep.txt");
        assert_eq!(l.std_dialog_return(dlg, DID_CANCEL), 1);
        // Destroyed with the modal loop's end: the state goes too.
        l.shared.window_mgr.lock_or_recover().finish_modal_dialog(dlg);
        assert!(l.shared.window_mgr.lock_or_recover().std_dialogs.get(dlg).is_none());
        assert_eq!(l.std_dialog_return(dlg, DID_CANCEL), DID_CANCEL);
    }

    #[test]
    fn several_selected_files_come_back_as_a_list_to_free() {
        let (l, frame, _tmp) = session();
        filedlg(&l, FDS_OPEN_DIALOG | FDS_MULTIPLESEL, "D:\\");
        let dlg = open(&l, frame);
        l.std_select_item(dlg, DID_FILES_LB, 0);
        l.std_select_item(dlg, DID_FILES_LB, 2);
        l.file_dlg_message(dlg, WM_COMMAND, DID_OK, 0);
        assert_eq!(l.guest_read::<u32>(FD + FD_UL_FQF_COUNT), Some(2));
        assert_eq!(full_file(&l), "D:\\alpha.txt");
        let list = l.guest_read::<u32>(FD + FD_PAPSZ_FQ_FILENAME).unwrap();
        assert_ne!(list, 0);
        let names: Vec<String> = (0..2).map(|i| l.read_guest_string(l.guest_read::<u32>(list + 4 * i).unwrap())).collect();
        assert_eq!(names, ["D:\\alpha.txt", "D:\\gamma.doc"]);
        assert_eq!(l.win_free_file_dlg_list(list), 1);
        assert_eq!(l.win_free_file_dlg_list(list), 0, "freed already");
        assert_eq!(l.win_free_file_dlg_list(STR), 0, "not a file list");
    }

    #[test]
    fn a_modeless_dialog_returns_its_handle_and_hides_when_done() {
        let (l, frame, _tmp) = session();
        filedlg(&l, FDS_OPEN_DIALOG | FDS_MODELESS, "D:\\alpha.txt");
        let dlg = match l.win_file_dlg(0, HWND_DESKTOP, frame, FD) {
            ApiResult::Normal(h) => h,
            other => panic!("modeless returns at once: {other:?}"),
        };
        assert_eq!(l.shared.window_mgr.lock_or_recover().get_window(frame).unwrap().style & WS_DISABLED, 0);
        assert_eq!(l.std_item_text(dlg, DID_FILENAME_ED), "alpha.txt");
        // Through the default procedure, as the program's loop dispatches it.
        l.std_dialog_procless(dlg, WM_COMMAND, DID_OK, 0).unwrap();
        assert_eq!(full_file(&l), "D:\\alpha.txt");
        let wm = l.shared.window_mgr.lock_or_recover();
        assert!(!wm.get_window(dlg).unwrap().visible && !wm.z_order.contains(&dlg));
    }

    #[test]
    fn a_dialog_procedure_subclasses_and_win_dismiss_dlg_writes_back() {
        let (l, frame, _tmp) = session();
        filedlg(&l, FDS_SAVEAS_DIALOG | FDS_HELPBUTTON, "D:\\");
        l.guest_write::<u32>(FD + FD_PFN_DLG_PROC as u32, 0x0040_0000).unwrap();
        l.guest_write_bytes(STR, b"Export\0").unwrap();
        l.guest_write::<u32>(FD + FD_PSZ_TITLE as u32, STR).unwrap();
        let dlg = match l.win_file_dlg(0, HWND_DESKTOP, frame, FD) {
            ApiResult::DlgRunLoop { hwnd_dlg, dlg_proc, .. } => { assert_eq!(dlg_proc, 0x0040_0000); hwnd_dlg }
            other => panic!("{other:?}"),
        };
        {
            let wm = l.shared.window_mgr.lock_or_recover();
            let d = wm.get_window(dlg).unwrap();
            assert_eq!((d.pfn_wp, d.text.as_str()), (0x0040_0000, "Export"));
            assert!(wm.find_child_by_id(dlg, DID_HELP_PB).is_some());
        }
        assert!(l.std_dialog_procless(dlg, WM_HELP, 0, 0).is_some());
        type_into(&l, dlg, "out.dat");
        // The program's procedure ends it itself (WinDismissDlg).
        l.dismiss_dialog(dlg, DID_OK);
        assert_eq!(full_file(&l), "D:\\out.dat");
        assert_eq!(l.guest_read::<u32>(FD + FD_LRETURN), Some(DID_OK));
    }

    /// The About dialog of `pm_dialog.rs`'s tests (Open Watcom `wrc`
    /// output) with its label turned into the file-name entry field
    /// (`WC_ENTRYFIELD`, id `DID_FILENAME_ED`) next to its OK button.
    fn custom_template() -> Vec<u8> {
        let mut t: Vec<u8> = vec![
            0xa7, 0x00, 0x00, 0x00, 0x52, 0x03, 0x0e, 0x00, 0x01, 0x00, 0xff, 0xff, 0x00, 0x00,
            0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x19, 0x00, 0x68, 0x00,
            0x80, 0x00, 0x00, 0x94, 0x28, 0x00, 0x28, 0x00, 0xb4, 0x00, 0x50, 0x00,
            0x65, 0x00, 0xff, 0xff, 0x82, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x1d, 0x00, 0x86, 0x00,
            0x01, 0x00, 0x00, 0x80, 0x0a, 0x00, 0x2d, 0x00, 0xa0, 0x00, 0x0e, 0x00,
            0x2d, 0x01, 0xff, 0xff, 0xff, 0xff,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0xa4, 0x00,
            0x00, 0x04, 0x02, 0x80, 0x3c, 0x00, 0x0a, 0x00, 0x32, 0x00, 0x0e, 0x00,
            0x01, 0x00, 0xff, 0xff, 0xff, 0xff,
        ];
        t.extend_from_slice(b"About PM Menu/Dialog Test\0\x03\0\0\0Warpine PM Menu & Dialog Test\0OK\0");
        // Item 1: predefined class 6 (WC_ENTRYFIELD), id DID_FILENAME_ED.
        let item1 = 14 + 30;
        t[item1 + 6] = 6;
        t[item1 + 24..item1 + 26].copy_from_slice(&(DID_FILENAME_ED as u16).to_le_bytes());
        t
    }

    #[test]
    fn a_custom_template_is_filled_by_control_id() {
        const RES: u32 = 0x0009_0000;
        const DLG_ID: u16 = 300;
        let (l, frame, _tmp) = session();
        let t = custom_template();
        l.guest_write_bytes(RES, &t).unwrap();
        l.shared.resource_mgr.lock_or_recover().add(0, crate::lx::header::RT_DIALOG, DLG_ID, RES, t.len() as u32);
        filedlg(&l, FDS_OPEN_DIALOG | FDS_CUSTOM | FDS_CENTER, "D:\\alpha.txt");
        l.guest_write::<u16>(FD + FD_US_DLG_ID as u32, DLG_ID).unwrap();
        l.guest_write_bytes(STR, b"Load\0").unwrap();
        l.guest_write::<u32>(FD + FD_PSZ_OK_BUTTON as u32, STR).unwrap();
        let dlg = open(&l, frame);
        assert_eq!(l.std_item_text(dlg, DID_FILENAME_ED), "alpha.txt");
        assert_eq!(l.std_item_text(dlg, DID_OK), "Load", "pszOKButton on the template's OK");
        {
            let wm = l.shared.window_mgr.lock_or_recover();
            let d = wm.get_window(dlg).unwrap();
            let (dw, _) = desktop_size();
            assert_eq!(d.x, (dw as i32 - d.cx) / 2, "FDS_CENTER moves the template's dialog");
            assert!(wm.find_child_by_id(dlg, DID_FILES_LB).is_none(), "no file list in this template");
        }
        type_into(&l, dlg, "gamma.doc");
        l.file_dlg_message(dlg, WM_COMMAND, DID_OK, 0);
        assert_eq!(full_file(&l), "D:\\gamma.doc");
    }

    #[test]
    fn bad_structures_fail_with_l_src() {
        let (l, frame, _tmp) = session();
        filedlg(&l, FDS_OPEN_DIALOG, "");
        l.guest_write::<u32>(FD, 100).unwrap();
        assert!(matches!(l.win_file_dlg(0, HWND_DESKTOP, frame, FD), ApiResult::Normal(0)));
        assert_eq!(l.guest_read::<u32>(FD + FD_LSRC), Some(FDS_ERR_INVALID_VERSION));
        assert!(matches!(l.win_file_dlg(0, HWND_DESKTOP, frame, 0), ApiResult::Normal(0)));
        assert!(matches!(l.win_file_dlg(0, HWND_DESKTOP, frame, 0xFFFF_FF00), ApiResult::Normal(0)));
        // A custom template that does not exist.
        filedlg(&l, FDS_OPEN_DIALOG | FDS_CUSTOM, "");
        assert!(matches!(l.win_file_dlg(0, HWND_DESKTOP, frame, FD), ApiResult::Normal(0)));
        assert_eq!(l.guest_read::<u32>(FD + FD_LSRC), Some(FDS_ERR_DIALOG_LOAD_ERROR));
        // No message queue on thread 5.
        filedlg(&l, FDS_OPEN_DIALOG, "");
        assert!(matches!(l.win_file_dlg(5, HWND_DESKTOP, frame, FD), ApiResult::Normal(0)));
        assert_eq!(l.guest_read::<u32>(FD + FD_LSRC), Some(FDS_ERR_INVALID_DIALOG));
    }
}
