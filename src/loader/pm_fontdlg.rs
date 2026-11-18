// SPDX-License-Identifier: GPL-3.0-only

//! `WinFontDlg` / `WinDefFontDlgProc` (PMCTLS): the standard font dialog,
//! over the fonts Warpine has.
//!
//! Warpine's fonts are the three IBM Plex families (`gui/font.rs`), each
//! in four styles and any size; GPI maps OS/2 face names onto them
//! (`FontSpec::from_facename`). The dialog (see `pm_stddlg.rs`) offers:
//!
//! * Name (`DID_NAME`): "IBM Plex Mono", "IBM Plex Sans", "IBM Plex Serif"
//!   — Mono only with `FNTS_FIXEDWIDTHONLY`, Sans and Serif only with
//!   `FNTS_PROPORTIONALONLY`;
//! * Style (`DID_STYLE`): Normal, Italic, Bold, Bold Italic;
//! * Size (`DID_SIZE`, an editable combination box): `pszPtSizeList` or a
//!   default list of point sizes, any whole size from 1 to
//!   `MAX_FONT_POINTS` typed;
//! * Emphasis (`DID_OUTLINE`, `DID_UNDERSCORE`, `DID_STRIKEOUT` check
//!   boxes): passed back in `fsSelection` / `flStyle`; the sample does
//!   not show them (Warpine draws no outline, underline or strikeout);
//! * a sample (`DID_SAMPLE`): `pszPreview` (default "abcdABCD") in the
//!   chosen font, size and `clrFore` / `clrBack` — a static whose
//!   `PP_FONTNAMESIZE` follows the choice;
//! * OK / Cancel and, with `FNTS_RESETBUTTON` / `FNTS_APPLYBUTTON` /
//!   `FNTS_HELPBUTTON`, Reset (back to the initial choice), Apply (writes
//!   the structure without closing — unverified) and Help (does nothing).
//!
//! The initial choice comes from `fAttrs` with `FNTS_INITFROMFATTRS`, else
//! from `pszFamilyname`, `flStyle` and `fxPointSize`. OK writes back the
//! family name (`pszFamilyname`, at most `usFamilyBufLen` bytes),
//! `fxPointSize`, `sNominalPointSize`, `flStyle` (`FATTR_SEL_*`), `flType`
//! (`FTYPE_ITALIC`), `usWeight` / `usWidth`, `lEmHeight` / `lXHeight` /
//! `lExternalLeading` and an `fAttrs` that `GpiCreateLogFont` turns back
//! into the same font (face name with the style words, `lMaxBaselineExt`,
//! `lAveCharWidth`, `FATTR_FONTUSE_OUTLINE`; `usCodePage` kept), with
//! `lReturn` / `lSRC`.
//!
//! Not supported: `FNTS_OWNERDRAWPREVIEW` (Warpine draws the sample),
//! printer fonts (`hpsPrinter`, `FNTS_*FILTER`), `FNTS_BITMAPONLY` (all of
//! Warpine's fonts are outline fonts that scale to any size, so the
//! filter is ignored rather than leaving nothing), and the `FNTM_*`
//! notifications are not sent.

use crate::gui::font::{self, FontFamily, FontSpec};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_presparam::{MAX_FONT_POINTS, PP_BACKGROUNDCOLOR, PP_FONTNAMESIZE, PP_FOREGROUNDCOLOR};
use super::pm_stddlg::*;
use super::{ApiResult, Loader};

// ── <pmstddlg.h>, <os2def.h>, <pmgpi.h> ─────────────────────────────────────

pub const FNTS_CENTER: u32 = 0x0000_0001;
pub const FNTS_CUSTOM: u32 = 0x0000_0002;
pub const FNTS_HELPBUTTON: u32 = 0x0000_0008;
pub const FNTS_APPLYBUTTON: u32 = 0x0000_0010;
pub const FNTS_RESETBUTTON: u32 = 0x0000_0020;
pub const FNTS_MODELESS: u32 = 0x0000_0040;
pub const FNTS_INITFROMFATTRS: u32 = 0x0000_0080;
pub const FNTS_FIXEDWIDTHONLY: u32 = 0x0000_0400;
pub const FNTS_PROPORTIONALONLY: u32 = 0x0000_0800;

pub const FNTS_SUCCESSFUL: u32 = 0;
pub const FNTS_ERR_INVALID_DIALOG: u32 = 3;
pub const FNTS_ERR_INVALID_VERSION: u32 = 10;
pub const FNTS_ERR_DIALOG_LOAD_ERROR: u32 = 12;

pub const FNTF_SCREENFONTSELECTED: u32 = 4;

pub const DID_NAME: u32 = 301;
pub const DID_STYLE: u32 = 302;
pub const DID_SIZE: u32 = 305;
pub const DID_SAMPLE: u32 = 306;
pub const DID_OUTLINE: u32 = 307;
pub const DID_UNDERSCORE: u32 = 308;
pub const DID_STRIKEOUT: u32 = 309;
pub const DID_HELP_BUTTON: u32 = 310;
pub const DID_APPLY_BUTTON: u32 = 311;
pub const DID_RESET_BUTTON: u32 = 312;
pub const DID_NAME_PREFIX: u32 = 313;
pub const DID_STYLE_PREFIX: u32 = 314;
pub const DID_SIZE_PREFIX: u32 = 315;
pub const DID_SAMPLE_GROUPBOX: u32 = 316;
pub const DID_EMPHASIS_GROUPBOX: u32 = 317;

pub const FATTR_SEL_ITALIC: u16 = 0x0001;
pub const FATTR_SEL_UNDERSCORE: u16 = 0x0002;
pub const FATTR_SEL_OUTLINE: u16 = 0x0008;
pub const FATTR_SEL_STRIKEOUT: u16 = 0x0010;
pub const FATTR_SEL_BOLD: u16 = 0x0020;
pub const FATTR_FONTUSE_OUTLINE: u16 = 0x0004;
pub const FACESIZE: usize = 32;
pub const FWEIGHT_NORMAL: u16 = 5;
pub const FWEIGHT_BOLD: u16 = 7;
pub const FWIDTH_NORMAL: u16 = 5;
pub const FTYPE_ITALIC: u32 = 0x01;

/// The emphasis bits the check boxes set.
const EMPHASIS: u16 = FATTR_SEL_UNDERSCORE | FATTR_SEL_OUTLINE | FATTR_SEL_STRIKEOUT;

// FONTDLG field offsets (<pmstddlg.h>; every field naturally aligned).
const FN_CB_SIZE: usize = 0;
const FN_PSZ_TITLE: usize = 12;
const FN_PSZ_PREVIEW: usize = 16;
const FN_PSZ_PT_SIZE_LIST: usize = 20;
const FN_PFN_DLG_PROC: usize = 24;
const FN_PSZ_FAMILYNAME: usize = 28;
const FN_FX_POINT_SIZE: usize = 32;
const FN_FL: usize = 36;
const FN_FL_FLAGS: u32 = 40;
const FN_FL_TYPE: u32 = 44;
const FN_FL_STYLE: usize = 52;
const FN_CLR_FORE: usize = 60;
const FN_CLR_BACK: usize = 64;
const FN_LRETURN: u32 = 72;
const FN_LSRC: u32 = 76;
const FN_LEM_HEIGHT: u32 = 80;
const FN_LX_HEIGHT: u32 = 84;
const FN_LEXTERNAL_LEADING: u32 = 88;
const FN_HMOD: usize = 92;
const FN_FATTRS: usize = 96;
const FN_SNOMINAL_POINT_SIZE: u32 = 152;
const FN_US_WEIGHT: u32 = 154;
const FN_US_WIDTH: u32 = 156;
const FN_X: usize = 158;
const FN_Y: usize = 160;
const FN_US_DLG_ID: usize = 162;
const FN_US_FAMILY_BUF_LEN: usize = 164;
/// `sizeof(FONTDLG)`.
pub const FONTDLG_SIZE: usize = 168;

// FATTRS field offsets (<pmwin.h>).
const FA_RECORD_LENGTH: usize = 0;
const FA_SELECTION: usize = 2;
const FA_FACENAME: usize = 8;
const FA_CODEPAGE: usize = 42;
const FA_MAX_BASELINE_EXT: usize = 44;
const FA_AVE_CHAR_WIDTH: usize = 48;
const FA_FONT_USE: usize = 54;
/// `sizeof(FATTRS)`.
pub const FATTRS_SIZE: usize = 56;

/// Sizes offered without `pszPtSizeList`, and the most read from one.
const DEFAULT_SIZES: &[u32] = &[8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 36, 48, 72];
const MAX_SIZES: usize = 64;
/// The size a choice starts with when the structure gives none.
const DEFAULT_POINTS: u32 = 10;
/// The sample without `pszPreview`.
const DEFAULT_PREVIEW: &str = "abcdABCD";
/// Style names, indexed `bold * 2 + italic`.
pub const STYLES: [&str; 4] = ["Normal", "Italic", "Bold", "Bold Italic"];

// Layout (Warpine's own).
const NAME_W: i32 = 220;
const SIZE_W: i32 = 90;
const CHECK_W: i32 = 110;
const SAMPLE_H: i32 = 96;
const GROUP_PAD: i32 = 8;
const COMBO_DROP_H: i32 = 6 * super::CONTROL_LINE_H + 4;

/// The family name Warpine reports for `f` (as `GpiQueryFontMetrics`).
pub fn family_name(f: FontFamily) -> &'static str {
    match f {
        FontFamily::Sans => "IBM Plex Sans",
        FontFamily::Mono => "IBM Plex Mono",
        FontFamily::Serif => "IBM Plex Serif",
    }
}

/// The families the dialog offers under `fl`, in list order.
pub fn families(fl: u32) -> Vec<FontFamily> {
    [FontFamily::Mono, FontFamily::Sans, FontFamily::Serif].into_iter()
        .filter(|f| match f {
            FontFamily::Mono => fl & FNTS_PROPORTIONALONLY == 0,
            _ => fl & FNTS_FIXEDWIDTHONLY == 0,
        })
        .collect()
}

/// The point sizes of a `pszPtSizeList` ("8 10 12"), ascending, without
/// duplicates; the default list when it gives none.
pub fn parse_sizes(list: Option<&str>) -> Vec<u32> {
    let mut v: Vec<u32> = list.map(|l| l.split([' ', ',', ';', '\t'])
        .filter_map(parse_points).take(MAX_SIZES).collect()).unwrap_or_default();
    if v.is_empty() {
        v = DEFAULT_SIZES.to_vec();
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// A typed point size ("12", "10.5" → 10): 1..=`MAX_FONT_POINTS`.
pub fn parse_points(s: &str) -> Option<u32> {
    let whole = s.trim().split('.').next()?;
    whole.parse::<u32>().ok().filter(|&p| (1..=MAX_FONT_POINTS).contains(&p))
}

/// The font chosen in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontChoice {
    pub family: FontFamily,
    pub bold: bool,
    pub italic: bool,
    /// `FATTR_SEL_UNDERSCORE` / `OUTLINE` / `STRIKEOUT`.
    pub emphasis: u16,
    pub points: u32,
}

impl FontChoice {
    /// `fsSelection` / `flStyle` of the choice.
    pub fn selection(&self) -> u16 {
        let mut s = self.emphasis & EMPHASIS;
        if self.bold { s |= FATTR_SEL_BOLD; }
        if self.italic { s |= FATTR_SEL_ITALIC; }
        s
    }

    /// The face name with the style words ("IBM Plex Serif Bold Italic").
    pub fn face(&self) -> String {
        let style = STYLES[usize::from(self.bold) * 2 + usize::from(self.italic)];
        if style == STYLES[0] {
            family_name(self.family).to_string()
        } else {
            format!("{} {style}", family_name(self.family))
        }
    }

    /// The `PP_FONTNAMESIZE` value that draws the sample ("12.IBM Plex Sans Bold").
    pub fn pres_param(&self) -> Vec<u8> {
        let mut v = format!("{}.{}", self.points, self.face()).into_bytes();
        v.push(0);
        v
    }

    /// The font spec GPI draws the choice with.
    pub fn spec(&self) -> FontSpec {
        super::pm_presparam::parse_font_name_size(&self.pres_param()).unwrap_or(FontSpec::SYSTEM)
    }
}

/// The initial choice: a face name (FATTRS `szFacename` or
/// `pszFamilyname`) with `selection` bits, `points` (0 = default), mapped
/// onto the families offered.
pub fn initial_choice(face: &str, selection: u16, points: u32, offered: &[FontFamily]) -> FontChoice {
    let spec = FontSpec::from_facename(face, selection & FATTR_SEL_BOLD != 0,
                                       selection & FATTR_SEL_ITALIC != 0, 0);
    let family = if offered.contains(&spec.family) { spec.family } else { offered.first().copied().unwrap_or(FontFamily::Sans) };
    let points = if (1..=MAX_FONT_POINTS).contains(&points) { points } else { DEFAULT_POINTS };
    FontChoice { family, bold: spec.bold, italic: spec.italic, emphasis: selection & EMPHASIS, points }
}

/// The `FONTDLG` fields the dialog reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FontDlgIn {
    pub cb_size: u32,
    pub psz_title: u32,
    pub psz_preview: u32,
    pub psz_pt_size_list: u32,
    pub pfn_dlg_proc: u32,
    pub psz_familyname: u32,
    pub fx_point_size: u32,
    pub fl: u32,
    pub fl_style: u32,
    pub clr_fore: u32,
    pub clr_back: u32,
    pub hmod: u32,
    /// `fAttrs`: `fsSelection` and the face name.
    pub fa_selection: u16,
    pub fa_facename: Vec<u8>,
    pub x: i16,
    pub y: i16,
    pub dlg_id: u16,
    pub family_buf_len: u16,
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// Parse the `FONTDLG_SIZE` bytes of a `FONTDLG`; `None` when shorter.
pub fn parse_fontdlg(b: &[u8]) -> Option<FontDlgIn> {
    if b.len() < FONTDLG_SIZE {
        return None;
    }
    let face = &b[FN_FATTRS + FA_FACENAME..FN_FATTRS + FA_FACENAME + FACESIZE];
    let end = face.iter().position(|&c| c == 0).unwrap_or(FACESIZE);
    Some(FontDlgIn {
        cb_size: le32(b, FN_CB_SIZE),
        psz_title: le32(b, FN_PSZ_TITLE),
        psz_preview: le32(b, FN_PSZ_PREVIEW),
        psz_pt_size_list: le32(b, FN_PSZ_PT_SIZE_LIST),
        pfn_dlg_proc: le32(b, FN_PFN_DLG_PROC),
        psz_familyname: le32(b, FN_PSZ_FAMILYNAME),
        fx_point_size: le32(b, FN_FX_POINT_SIZE),
        fl: le32(b, FN_FL),
        fl_style: le32(b, FN_FL_STYLE),
        clr_fore: le32(b, FN_CLR_FORE),
        clr_back: le32(b, FN_CLR_BACK),
        hmod: le32(b, FN_HMOD),
        fa_selection: le16(b, FN_FATTRS + FA_SELECTION),
        fa_facename: face[..end].to_vec(),
        x: le16(b, FN_X) as i16,
        y: le16(b, FN_Y) as i16,
        dlg_id: le16(b, FN_US_DLG_ID),
        family_buf_len: le16(b, FN_US_FAMILY_BUF_LEN),
    })
}

/// The `FATTRS` that selects `choice` (codepage `codepage`).
pub fn fattrs_for(choice: &FontChoice, codepage: u16) -> [u8; FATTRS_SIZE] {
    let mut b = [0u8; FATTRS_SIZE];
    let m = font::metrics(choice.spec());
    b[FA_RECORD_LENGTH..FA_RECORD_LENGTH + 2].copy_from_slice(&(FATTRS_SIZE as u16).to_le_bytes());
    b[FA_SELECTION..FA_SELECTION + 2].copy_from_slice(&choice.selection().to_le_bytes());
    let face = choice.face();
    let n = face.len().min(FACESIZE - 1);
    b[FA_FACENAME..FA_FACENAME + n].copy_from_slice(&face.as_bytes()[..n]);
    b[FA_CODEPAGE..FA_CODEPAGE + 2].copy_from_slice(&codepage.to_le_bytes());
    b[FA_MAX_BASELINE_EXT..FA_MAX_BASELINE_EXT + 4].copy_from_slice(&m.line_height.to_le_bytes());
    b[FA_AVE_CHAR_WIDTH..FA_AVE_CHAR_WIDTH + 4].copy_from_slice(&m.avg_char_width.to_le_bytes());
    b[FA_FONT_USE..FA_FONT_USE + 2].copy_from_slice(&FATTR_FONTUSE_OUTLINE.to_le_bytes());
    b
}

/// The controls of the built-in dialog and its size.
pub fn layout(fl: u32) -> ((i32, i32), Vec<StdControl>) {
    let b = CHROME_BORDER_W;
    let (l, e) = (STD_LABEL_H, STD_ENTRY_H);
    let label = SS_TEXT | DT_LEFT | DT_VCENTER;
    let x0 = b + STD_MARGIN;
    let x1 = x0 + NAME_W + STD_COL_GAP;
    let content_w = NAME_W + STD_COL_GAP + SIZE_W.max(STD_BUTTON_W);
    let mut buttons = vec![(DID_OK, "OK", BS_DEFAULT | WS_GROUP), (DID_CANCEL, "Cancel", 0)];
    if fl & FNTS_RESETBUTTON != 0 { buttons.push((DID_RESET_BUTTON, "Reset", 0)); }
    if fl & FNTS_APPLYBUTTON != 0 { buttons.push((DID_APPLY_BUTTON, "Apply", 0)); }
    if fl & FNTS_HELPBUTTON != 0 { buttons.push((DID_HELP_BUTTON, "Help", BS_HELP)); }
    let buttons_w = buttons.len() as i32 * (STD_BUTTON_W + STD_BUTTON_GAP) - STD_BUTTON_GAP;
    let content_w = content_w.max(buttons_w).max(3 * CHECK_W + 2 * GROUP_PAD);
    let cx = 2 * (b + STD_MARGIN) + content_w;
    let mut v = Vec::new();

    let mut y = b + STD_MARGIN;
    for (i, (id, text, style)) in buttons.into_iter().enumerate() {
        let x = x0 + i as i32 * (STD_BUTTON_W + STD_BUTTON_GAP);
        v.push(StdControl::new("WC_BUTTON", text, (x, y, STD_BUTTON_W, STD_BUTTON_H), id,
                               BS_PUSHBUTTON | WS_TABSTOP | style));
    }
    y += STD_BUTTON_H + STD_MARGIN;
    // Sample: a group box around the static the font is shown in.
    let group_h = SAMPLE_H + l + GROUP_PAD;
    v.push(StdControl::new("WC_STATIC", "Sample", (x0, y, content_w, group_h), DID_SAMPLE_GROUPBOX, SS_GROUPBOX));
    v.push(StdControl::new("WC_STATIC", "", (x0 + GROUP_PAD, y + GROUP_PAD, content_w - 2 * GROUP_PAD, SAMPLE_H),
                           DID_SAMPLE, SS_TEXT | DT_CENTER | DT_VCENTER));
    y += group_h + STD_ROW_GAP;
    // Emphasis: three check boxes in a group box.
    let emph_h = e + l + GROUP_PAD;
    v.push(StdControl::new("WC_STATIC", "Emphasis", (x0, y, content_w, emph_h), DID_EMPHASIS_GROUPBOX, SS_GROUPBOX));
    for (i, (id, text)) in [(DID_OUTLINE, "Outline"), (DID_UNDERSCORE, "Underscore"), (DID_STRIKEOUT, "Strikeout")]
        .into_iter().enumerate() {
        let style = BS_AUTOCHECKBOX | WS_TABSTOP | if i == 0 { WS_GROUP } else { 0 };
        v.push(StdControl::new("WC_BUTTON", text, (x0 + GROUP_PAD + i as i32 * CHECK_W, y + GROUP_PAD / 2, CHECK_W, e),
                               id, style));
    }
    y += emph_h + STD_ROW_GAP;
    // Style row, then the name and size row above it; combination boxes
    // reach down over what their lists cover.
    let combo = |x: i32, y: i32, w: i32, id: u32, kind: u32| StdControl::new(
        "WC_COMBOBOX", "", (x, y - COMBO_DROP_H, w, e + COMBO_DROP_H), id, kind | WS_TABSTOP | WS_GROUP);
    let style_y = y;
    y += e;
    let style_label_y = y;
    y += l + STD_ROW_GAP;
    let name_y = y;
    y += e;
    v.push(StdControl::new("WC_STATIC", "Name:", (x0, y, NAME_W, l), DID_NAME_PREFIX, label));
    v.push(StdControl::new("WC_STATIC", "Size:", (x1, y, SIZE_W, l), DID_SIZE_PREFIX, label));
    v.push(combo(x0, name_y, NAME_W, DID_NAME, CBS_DROPDOWNLIST));
    v.push(combo(x1, name_y, SIZE_W, DID_SIZE, CBS_DROPDOWN));
    v.push(StdControl::new("WC_STATIC", "Style:", (x0, style_label_y, NAME_W, l), DID_STYLE_PREFIX, label));
    v.push(combo(x0, style_y, NAME_W, DID_STYLE, CBS_DROPDOWNLIST));
    y += l + STD_MARGIN;
    let cy = y + CHROME_TITLE_H + b;
    // Creation order = tab order: the name first, the buttons last. The
    // group boxes come before what they hold (painted under it).
    let order = [DID_NAME_PREFIX, DID_NAME, DID_SIZE_PREFIX, DID_SIZE, DID_STYLE_PREFIX, DID_STYLE,
                 DID_EMPHASIS_GROUPBOX, DID_OUTLINE, DID_UNDERSCORE, DID_STRIKEOUT,
                 DID_SAMPLE_GROUPBOX, DID_SAMPLE];
    let mut sorted: Vec<StdControl> = order.iter().filter_map(|id| v.iter().find(|c| c.id == *id).cloned()).collect();
    sorted.extend(v.into_iter().filter(|c| !order.contains(&c.id)));
    ((cx, cy), sorted)
}

/// A font dialog's state.
#[derive(Debug, Clone)]
pub struct FontDlgState {
    /// The program's `FONTDLG`.
    pub pfontdlg: u32,
    pub fl: u32,
    pub cp: u32,
    pub families: Vec<FontFamily>,
    pub sizes: Vec<u32>,
    pub choice: FontChoice,
    /// The choice the dialog started with (Reset).
    pub initial: FontChoice,
    /// The sample's colours (RGB).
    pub colors: (u32, u32),
    pub committed: bool,
    pub modeless: bool,
}

/// RGB of the sample's colours `clrFore` / `clrBack` (`CLR_*` indices or
/// RGB); black on white when they would be the same.
pub fn sample_colors(fore: u32, back: u32) -> (u32, u32) {
    let (f, b) = (super::pm_types::default_color(fore), super::pm_types::default_color(back));
    if f == b { (0x000000, 0xFF_FFFF) } else { (f, b) }
}

impl Loader {
    /// WinFontDlg(hwndP, hwndO, pfntd): open the font dialog. Modal:
    /// TRUE when it has ended (the outcome in `lReturn`); modeless
    /// (`FNTS_MODELESS`): the dialog's handle; NULLHANDLE with `lSRC` set on
    /// an error.
    pub(crate) fn win_font_dlg(&self, tid: u32, hwnd_p: u32, hwnd_o: u32, pfntd: u32) -> ApiResult {
        let Some(fd) = self.guest_read_bytes(pfntd, FONTDLG_SIZE).filter(|_| pfntd != 0)
            .as_deref().and_then(parse_fontdlg) else {
            return ApiResult::Normal(0);
        };
        let fail = |code: u32| {
            let _ = self.guest_write::<u32>(pfntd + FN_LSRC, code);
            ApiResult::Normal(0)
        };
        if (fd.cb_size as usize) < FONTDLG_SIZE {
            return fail(FNTS_ERR_INVALID_VERSION);
        }
        let Some(hmq) = self.shared.window_mgr.lock_or_recover().thread_queue(tid) else {
            return fail(FNTS_ERR_INVALID_DIALOG);
        };
        let cp = self.thread_codepage(tid);
        let read = |p: u32| (p != 0).then(|| self.read_guest_string_cp(p, cp));
        let offered = families(fd.fl);
        let points = fd.fx_point_size >> 16;
        let choice = if fd.fl & FNTS_INITFROMFATTRS != 0 {
            let face = super::codepage::cp_decode(&fd.fa_facename, cp);
            initial_choice(&face, fd.fa_selection, points, &offered)
        } else {
            initial_choice(&read(fd.psz_familyname).unwrap_or_default(), fd.fl_style as u16, points, &offered)
        };
        let sizes = parse_sizes(read(fd.psz_pt_size_list).as_deref());
        let title = read(fd.psz_title).unwrap_or_else(|| "Font".to_string());
        let preview = read(fd.psz_preview).filter(|p| !p.is_empty()).unwrap_or_else(|| DEFAULT_PREVIEW.to_string());
        let modeless = fd.fl & FNTS_MODELESS != 0;

        let hwnd = if fd.fl & FNTS_CUSTOM != 0 {
            let Some(h) = self.std_custom_dialog(tid, hwnd_p, hwnd_o, fd.pfn_dlg_proc, fd.hmod, fd.dlg_id as u32)
            else { return fail(FNTS_ERR_DIALOG_LOAD_ERROR) };
            self.std_center_custom(h, hwnd_p, fd.fl & FNTS_CENTER != 0);
            if fd.psz_title != 0 && let Some(w) = self.shared.window_mgr.lock_or_recover().get_window_mut(h) {
                w.text = title;
            }
            h
        } else {
            let (size, controls) = layout(fd.fl);
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let area = parent_area(&wm, hwnd_p);
            let pos = place(fd.fl & FNTS_CENTER != 0, area, (fd.x as i32, fd.y as i32), size);
            create_std_dialog(&mut wm, hmq, hwnd_o, fd.pfn_dlg_proc, &title, pos, size, &controls)
        };
        let state = FontDlgState {
            pfontdlg: pfntd, fl: fd.fl, cp, families: offered.clone(), sizes: sizes.clone(), choice,
            initial: choice, colors: sample_colors(fd.clr_fore, fd.clr_back), committed: false, modeless,
        };
        self.shared.window_mgr.lock_or_recover().std_dialogs.insert(hwnd, StdDialog::Font(Box::new(state)));

        self.std_set_list(hwnd, DID_NAME, offered.iter().map(|&f| family_name(f).to_string()).collect());
        self.std_set_list(hwnd, DID_STYLE, STYLES.iter().map(|s| s.to_string()).collect());
        self.std_set_list(hwnd, DID_SIZE, sizes.iter().map(u32::to_string).collect());
        self.std_set_item_text(hwnd, DID_SAMPLE, &preview);
        self.font_dlg_show(hwnd, &choice);
        let focus = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.find_child_by_id(hwnd, DID_NAME).map_or(0, |h| wm.focus_window_for(wm.text_window(h)))
        };
        self.std_dialog_start(hwnd, hmq, focus, pfntd, modeless, fd.pfn_dlg_proc)
    }

    /// Show `choice` in the controls and the sample.
    fn font_dlg_show(&self, hwnd: u32, choice: &FontChoice) {
        let Some(s) = self.shared.window_mgr.lock_or_recover().std_dialogs.font(hwnd).cloned() else { return };
        if let Some(i) = s.families.iter().position(|&f| f == choice.family) {
            self.std_select_item(hwnd, DID_NAME, i);
        }
        self.std_select_item(hwnd, DID_STYLE, usize::from(choice.bold) * 2 + usize::from(choice.italic));
        match s.sizes.iter().position(|&p| p == choice.points) {
            Some(i) => self.std_select_item(hwnd, DID_SIZE, i),
            None => self.std_set_item_text(hwnd, DID_SIZE, &choice.points.to_string()),
        }
        for (id, bit) in [(DID_OUTLINE, FATTR_SEL_OUTLINE), (DID_UNDERSCORE, FATTR_SEL_UNDERSCORE),
                          (DID_STRIKEOUT, FATTR_SEL_STRIKEOUT)] {
            let h = self.shared.window_mgr.lock_or_recover().find_child_by_id(hwnd, id);
            if let Some(h) = h {
                self.dispatch_builtin_control(h, BM_SETCHECK, u32::from(choice.emphasis & bit != 0), 0);
            }
        }
        self.font_dlg_preview(hwnd);
    }

    /// Draw the sample in the chosen font.
    fn font_dlg_preview(&self, hwnd: u32) {
        let sample = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some((choice, (fore, back))) = wm.std_dialogs.font(hwnd).map(|s| (s.choice, s.colors)) else { return };
            let Some(h) = wm.find_child_by_id(hwnd, DID_SAMPLE) else { return };
            let Some(w) = wm.get_window_mut(h) else { return };
            w.pres_params.set(PP_FONTNAMESIZE, &choice.pres_param());
            w.pres_params.set(PP_FOREGROUNDCOLOR, &fore.to_le_bytes());
            w.pres_params.set(PP_BACKGROUNDCOLOR, &back.to_le_bytes());
            h
        };
        self.repaint_control(sample);
    }

    /// Change the choice with `f` and show it.
    fn font_dlg_change(&self, hwnd: u32, f: impl FnOnce(&mut FontChoice)) {
        let changed = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(s) = wm.std_dialogs.font_mut(hwnd) else { return };
            let before = s.choice;
            f(&mut s.choice);
            s.choice != before
        };
        if changed {
            self.font_dlg_preview(hwnd);
        }
    }

    /// `WinDefFontDlgProc` (and the messages of a font dialog without a
    /// dialog procedure): the font dialog's default processing.
    pub(crate) fn font_dlg_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> ApiResult {
        let state = self.shared.window_mgr.lock_or_recover().std_dialogs.font(hwnd).cloned();
        let Some(s) = state else { return self.def_dlg_proc(hwnd, msg, mp1, mp2) };
        match msg {
            WM_CONTROL => {
                let (id, code) = (mp1 & 0xFFFF, mp1 >> 16);
                let picked = || self.std_selected(hwnd, id).into_iter().next().map(|x| x.0);
                match (id, code) {
                    (DID_NAME, CBN_LBSELECT) => {
                        if let Some(&fam) = picked().and_then(|i| s.families.get(i)) {
                            self.font_dlg_change(hwnd, |c| c.family = fam);
                        }
                    }
                    (DID_STYLE, CBN_LBSELECT) => {
                        if let Some(i) = picked().filter(|&i| i < STYLES.len()) {
                            self.font_dlg_change(hwnd, |c| (c.bold, c.italic) = (i >= 2, i % 2 == 1));
                        }
                    }
                    (DID_SIZE, CBN_LBSELECT | CBN_EFCHANGE) => {
                        if let Some(p) = parse_points(&self.std_item_text(hwnd, DID_SIZE)) {
                            self.font_dlg_change(hwnd, |c| c.points = p);
                        }
                    }
                    (DID_OUTLINE | DID_UNDERSCORE | DID_STRIKEOUT, BN_CLICKED) => {
                        let bit = match id {
                            DID_OUTLINE => FATTR_SEL_OUTLINE,
                            DID_UNDERSCORE => FATTR_SEL_UNDERSCORE,
                            _ => FATTR_SEL_STRIKEOUT,
                        };
                        let on = self.std_checked(hwnd, id);
                        self.font_dlg_change(hwnd, |c| if on { c.emphasis |= bit } else { c.emphasis &= !bit });
                    }
                    _ => {}
                }
                ApiResult::Normal(0)
            }
            WM_COMMAND => {
                match mp1 & 0xFFFF {
                    DID_OK => self.dismiss_dialog(hwnd, DID_OK),
                    DID_CANCEL => self.dismiss_dialog(hwnd, DID_CANCEL),
                    DID_RESET_BUTTON => {
                        if let Some(st) = self.shared.window_mgr.lock_or_recover().std_dialogs.font_mut(hwnd) {
                            st.choice = st.initial;
                        }
                        self.font_dlg_show(hwnd, &s.initial);
                    }
                    // TODO(unverified): what PM writes and sends for Apply.
                    DID_APPLY_BUTTON => self.font_dlg_write(&s, DID_APPLY_BUTTON),
                    _ => return self.def_dlg_proc(hwnd, msg, mp1, mp2),
                }
                ApiResult::Normal(0)
            }
            WM_CLOSE => {
                self.dismiss_dialog(hwnd, DID_CANCEL);
                ApiResult::Normal(0)
            }
            WM_HELP => ApiResult::Normal(0),
            _ => self.def_dlg_proc(hwnd, msg, mp1, mp2),
        }
    }

    /// The dialog is being dismissed with `result`: write the `FONTDLG` back.
    pub(super) fn font_dlg_commit(&self, hwnd: u32, result: u32) {
        let s = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(st) = wm.std_dialogs.font_mut(hwnd) else { return };
            st.committed = true;
            st.clone()
        };
        self.font_dlg_write(&s, result);
        if s.modeless {
            self.std_hide_modeless(hwnd);
        }
    }

    /// Write `lReturn` = `result`, `lSRC` and, unless cancelled, the choice.
    fn font_dlg_write(&self, s: &FontDlgState, result: u32) {
        let p = s.pfontdlg;
        let _ = self.guest_write::<u32>(p + FN_LRETURN, result);
        let _ = self.guest_write::<u32>(p + FN_LSRC, FNTS_SUCCESSFUL);
        if result == DID_CANCEL {
            return;
        }
        let Some(fd) = self.guest_read_bytes(p, FONTDLG_SIZE).as_deref().and_then(parse_fontdlg) else { return };
        let c = s.choice;
        if fd.psz_familyname != 0 && fd.family_buf_len > 0 {
            self.std_write_string(fd.psz_familyname, fd.family_buf_len as usize, family_name(c.family), s.cp);
        }
        let m = font::metrics(c.spec());
        let _ = self.guest_write::<u32>(p + FN_FX_POINT_SIZE as u32, c.points << 16);
        let _ = self.guest_write::<u32>(p + FN_FL_STYLE as u32, c.selection() as u32);
        let _ = self.guest_write::<u32>(p + FN_FL_TYPE, if c.italic { FTYPE_ITALIC } else { 0 });
        let _ = self.guest_write::<u32>(p + FN_FL_FLAGS, FNTF_SCREENFONTSELECTED);
        let _ = self.guest_write::<i32>(p + FN_LEM_HEIGHT, m.line_height);
        let _ = self.guest_write::<i32>(p + FN_LX_HEIGHT, m.x_height);
        let _ = self.guest_write::<i32>(p + FN_LEXTERNAL_LEADING, 0);
        let _ = self.guest_write::<i16>(p + FN_SNOMINAL_POINT_SIZE, (c.points * 10) as i16);
        let _ = self.guest_write::<u16>(p + FN_US_WEIGHT, if c.bold { FWEIGHT_BOLD } else { FWEIGHT_NORMAL });
        let _ = self.guest_write::<u16>(p + FN_US_WIDTH, FWIDTH_NORMAL);
        let codepage = self.guest_read::<u16>(p + (FN_FATTRS + FA_CODEPAGE) as u32).unwrap_or(0);
        let _ = self.guest_write_bytes(p + FN_FATTRS as u32, &fattrs_for(&c, codepage));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_follow_the_filters() {
        assert_eq!(families(0), [FontFamily::Mono, FontFamily::Sans, FontFamily::Serif]);
        assert_eq!(families(FNTS_FIXEDWIDTHONLY), [FontFamily::Mono]);
        assert_eq!(families(FNTS_PROPORTIONALONLY), [FontFamily::Sans, FontFamily::Serif]);
        assert!(families(FNTS_FIXEDWIDTHONLY | FNTS_PROPORTIONALONLY).is_empty());
        assert_eq!(family_name(FontFamily::Serif), "IBM Plex Serif");
    }

    #[test]
    fn sizes_are_parsed_and_bounded() {
        assert_eq!(parse_sizes(Some("12 8 10 8")), [8, 10, 12]);
        assert_eq!(parse_sizes(Some("9, 10.5; 0 999 x")), [9, 10]);
        assert_eq!(parse_sizes(None), DEFAULT_SIZES);
        assert_eq!(parse_sizes(Some("  ")), DEFAULT_SIZES);
        assert_eq!(parse_points(" 14 "), Some(14));
        assert_eq!(parse_points("0"), None);
        assert_eq!(parse_points(&(MAX_FONT_POINTS + 1).to_string()), None);
        let many = (1..200).map(|n| (n % 100 + 1).to_string()).collect::<Vec<_>>().join(" ");
        assert!(parse_sizes(Some(&many)).len() <= MAX_SIZES);
    }

    #[test]
    fn the_initial_choice_maps_face_names() {
        let all = families(0);
        let c = initial_choice("Times New Roman", FATTR_SEL_BOLD | FATTR_SEL_UNDERSCORE, 14, &all);
        assert_eq!(c, FontChoice { family: FontFamily::Serif, bold: true, italic: false,
                                   emphasis: FATTR_SEL_UNDERSCORE, points: 14 });
        let c = initial_choice("Courier Italic", 0, 0, &all);
        assert_eq!((c.family, c.italic, c.points), (FontFamily::Mono, true, DEFAULT_POINTS));
        let c = initial_choice("IBM Plex Sans", 0, 500, &all);
        assert_eq!((c.family, c.points), (FontFamily::Sans, DEFAULT_POINTS), "out of range");
        let c = initial_choice("Helvetica", 0, 12, &families(FNTS_FIXEDWIDTHONLY));
        assert_eq!(c.family, FontFamily::Mono, "a family not offered falls to the first offered");
    }

    #[test]
    fn a_choice_names_its_face_and_selection() {
        let c = FontChoice { family: FontFamily::Serif, bold: true, italic: true,
                             emphasis: FATTR_SEL_STRIKEOUT, points: 12 };
        assert_eq!(c.face(), "IBM Plex Serif Bold Italic");
        assert_eq!(c.selection(), FATTR_SEL_BOLD | FATTR_SEL_ITALIC | FATTR_SEL_STRIKEOUT);
        assert_eq!(c.pres_param(), b"12.IBM Plex Serif Bold Italic\0");
        let spec = c.spec();
        assert_eq!((spec.family, spec.bold, spec.italic), (FontFamily::Serif, true, true));
        let plain = FontChoice { bold: false, italic: false, emphasis: 0, ..c };
        assert_eq!(plain.face(), "IBM Plex Serif");
        assert!(c.spec().px > plain.spec().px / 2, "same size");
    }

    #[test]
    fn fattrs_select_the_choice_again() {
        let c = FontChoice { family: FontFamily::Mono, bold: true, italic: false, emphasis: 0, points: 20 };
        let b = fattrs_for(&c, 850);
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), FATTRS_SIZE as u16);
        assert_eq!(u16::from_le_bytes([b[2], b[3]]), FATTR_SEL_BOLD);
        assert_eq!(&b[8..8 + 18], b"IBM Plex Mono Bold");
        assert_eq!(b[8 + 18], 0);
        assert_eq!(u16::from_le_bytes([b[42], b[43]]), 850);
        let ext = i32::from_le_bytes(b[44..48].try_into().unwrap());
        assert_eq!(ext, font::metrics(c.spec()).line_height);
        assert!(i32::from_le_bytes(b[48..52].try_into().unwrap()) > 0);
        assert_eq!(u16::from_le_bytes([b[54], b[55]]), FATTR_FONTUSE_OUTLINE);
        // GPI reads the face back as the same family and style.
        let back = FontSpec::from_facename("IBM Plex Mono Bold", false, false, 0);
        assert_eq!((back.family, back.bold), (FontFamily::Mono, true));
    }

    #[test]
    fn fontdlg_is_parsed_at_the_header_offsets() {
        let mut b = vec![0u8; FONTDLG_SIZE];
        b[0..4].copy_from_slice(&(FONTDLG_SIZE as u32).to_le_bytes());
        b[28..32].copy_from_slice(&0x5000u32.to_le_bytes());
        b[32..36].copy_from_slice(&(12u32 << 16).to_le_bytes());
        b[36..40].copy_from_slice(&(FNTS_CENTER | FNTS_INITFROMFATTRS).to_le_bytes());
        b[52..56].copy_from_slice(&(FATTR_SEL_BOLD as u32).to_le_bytes());
        b[60..64].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        b[96 + 2..96 + 4].copy_from_slice(&FATTR_SEL_ITALIC.to_le_bytes());
        b[96 + 8..96 + 15].copy_from_slice(b"Courier");
        b[162..164].copy_from_slice(&400u16.to_le_bytes());
        b[164..166].copy_from_slice(&64u16.to_le_bytes());
        let f = parse_fontdlg(&b).unwrap();
        assert_eq!((f.cb_size, f.psz_familyname, f.fx_point_size >> 16), (168, 0x5000, 12));
        assert_eq!((f.fl, f.fl_style), (FNTS_CENTER | FNTS_INITFROMFATTRS, FATTR_SEL_BOLD as u32));
        assert_eq!((f.clr_fore, f.fa_selection), (0xFFFF_FFFF, FATTR_SEL_ITALIC));
        assert_eq!(f.fa_facename, b"Courier");
        assert_eq!((f.dlg_id, f.family_buf_len), (400, 64));
        assert!(parse_fontdlg(&b[..FONTDLG_SIZE - 1]).is_none());
    }

    #[test]
    fn the_layout_has_the_standard_controls() {
        let ((cx, cy), v) = layout(FNTS_RESETBUTTON | FNTS_APPLYBUTTON | FNTS_HELPBUTTON);
        for id in [DID_NAME, DID_STYLE, DID_SIZE, DID_SAMPLE, DID_OUTLINE, DID_UNDERSCORE, DID_STRIKEOUT,
                   DID_OK, DID_CANCEL, DID_RESET_BUTTON, DID_APPLY_BUTTON, DID_HELP_BUTTON,
                   DID_SAMPLE_GROUPBOX, DID_EMPHASIS_GROUPBOX] {
            assert!(v.iter().any(|c| c.id == id), "control {id}");
        }
        assert_eq!(v[1].id, DID_NAME, "the name list first in tab order");
        for c in &v {
            let (x, y, w, h) = c.rect;
            assert!(x >= CHROME_BORDER_W && x + w <= cx - CHROME_BORDER_W, "{c:?}");
            assert!(y >= CHROME_BORDER_W && y + h <= cy - CHROME_TITLE_H, "{c:?}");
        }
        let (_, plain) = layout(0);
        assert!(!plain.iter().any(|c| c.id == DID_RESET_BUTTON || c.id == DID_HELP_BUTTON));
        let size = plain.iter().find(|c| c.id == DID_SIZE).unwrap();
        assert_eq!(size.style & CBS_DROPDOWN, CBS_DROPDOWN, "a size can be typed");
    }

    #[test]
    fn sample_colours_never_vanish() {
        assert_eq!(sample_colors(-1i32 as u32, -2i32 as u32), (0x000000, 0xFF_FFFF), "CLR_BLACK on CLR_WHITE");
        assert_eq!(sample_colors(0, 0), (0x000000, 0xFF_FFFF), "a zeroed structure");
        assert_eq!(sample_colors(2, 0), (0xFF_0000, 0xFF_FFFF), "CLR_RED on CLR_BACKGROUND");
    }

    // ── The dialog on a mock loader ─────────────────────────────────────────

    use crate::loader::mutex_ext::MutexExt;

    const FD: u32 = 0x0008_0000;
    const FAMILY: u32 = 0x0008_1000;
    const FAMILY_LEN: u16 = 64;

    fn session() -> (Loader, u32) {
        let loader = Loader::new_mock();
        let frame = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), 0, hmq);
            wm.z_push_top(frame);
            frame
        };
        (loader, frame)
    }

    /// A FONTDLG for `family` (`pszFamilyname`) at `points` with `fl`.
    fn fontdlg(l: &Loader, fl: u32, family: &str, points: u32, style: u16) {
        let mut b = vec![0u8; FONTDLG_SIZE];
        b[0..4].copy_from_slice(&(FONTDLG_SIZE as u32).to_le_bytes());
        b[28..32].copy_from_slice(&FAMILY.to_le_bytes());
        b[32..36].copy_from_slice(&(points << 16).to_le_bytes());
        b[36..40].copy_from_slice(&fl.to_le_bytes());
        b[52..56].copy_from_slice(&(style as u32).to_le_bytes());
        b[60..64].copy_from_slice(&(-1i32).to_le_bytes()); // CLR_BLACK
        b[64..68].copy_from_slice(&(-2i32).to_le_bytes()); // CLR_WHITE
        b[96 + 42..96 + 44].copy_from_slice(&850u16.to_le_bytes());
        b[164..166].copy_from_slice(&FAMILY_LEN.to_le_bytes());
        l.guest_write_bytes(FD, &b).unwrap();
        let mut name = family.as_bytes().to_vec();
        name.push(0);
        l.guest_write_bytes(FAMILY, &name).unwrap();
    }

    fn open(l: &Loader, frame: u32) -> u32 {
        match l.win_font_dlg(0, HWND_DESKTOP, frame, FD) {
            ApiResult::DlgRunLoop { hwnd_dlg, .. } => hwnd_dlg,
            other => panic!("expected the modal loop, got {other:?}"),
        }
    }

    fn choice(l: &Loader, dlg: u32) -> FontChoice {
        l.shared.window_mgr.lock_or_recover().std_dialogs.font(dlg).unwrap().choice
    }

    fn sample_font(l: &Loader, dlg: u32) -> Vec<u8> {
        let wm = l.shared.window_mgr.lock_or_recover();
        let h = wm.find_child_by_id(dlg, DID_SAMPLE).unwrap();
        wm.get_window(h).unwrap().pres_params.get(PP_FONTNAMESIZE).unwrap().to_vec()
    }

    #[test]
    fn the_dialog_starts_from_the_structure() {
        let (l, frame) = session();
        fontdlg(&l, FNTS_CENTER, "Times New Roman", 14, FATTR_SEL_BOLD);
        let dlg = open(&l, frame);
        assert_eq!(choice(&l, dlg), FontChoice { family: FontFamily::Serif, bold: true, italic: false,
                                                  emphasis: 0, points: 14 });
        assert_eq!(l.std_item_text(dlg, DID_NAME), "IBM Plex Serif");
        assert_eq!(l.std_item_text(dlg, DID_STYLE), "Bold");
        assert_eq!(l.std_item_text(dlg, DID_SIZE), "14");
        assert_eq!(l.std_item_text(dlg, DID_SAMPLE), DEFAULT_PREVIEW);
        assert_eq!(sample_font(&l, dlg), b"14.IBM Plex Serif Bold\0");
        let wm = l.shared.window_mgr.lock_or_recover();
        let name_entry = wm.find_child_by_id(dlg, DID_NAME).and_then(|h| wm.get_window(h)).unwrap().combo.unwrap().entry;
        assert_eq!(wm.focus_hwnd, name_entry, "the focus on the name list");
        assert_eq!(wm.get_window(dlg).unwrap().text, "Font");
    }

    #[test]
    fn choosing_changes_the_sample_and_ok_writes_the_font_back() {
        let (l, frame) = session();
        fontdlg(&l, 0, "Helvetica", 12, 0);
        let dlg = open(&l, frame);
        assert_eq!(choice(&l, dlg).family, FontFamily::Sans);
        // Mono, Bold Italic, 20 points, underscore.
        l.std_select_item(dlg, DID_NAME, 0);
        l.font_dlg_message(dlg, WM_CONTROL, (CBN_LBSELECT << 16) | DID_NAME, 0);
        l.std_select_item(dlg, DID_STYLE, 3);
        l.font_dlg_message(dlg, WM_CONTROL, (CBN_LBSELECT << 16) | DID_STYLE, 0);
        l.std_set_item_text(dlg, DID_SIZE, "20");
        l.font_dlg_message(dlg, WM_CONTROL, (CBN_EFCHANGE << 16) | DID_SIZE, 0);
        let under = l.shared.window_mgr.lock_or_recover().find_child_by_id(dlg, DID_UNDERSCORE).unwrap();
        l.dispatch_builtin_control(under, BM_SETCHECK, 1, 0);
        l.font_dlg_message(dlg, WM_CONTROL, (BN_CLICKED << 16) | DID_UNDERSCORE, 0);
        let c = choice(&l, dlg);
        assert_eq!(c, FontChoice { family: FontFamily::Mono, bold: true, italic: true,
                                   emphasis: FATTR_SEL_UNDERSCORE, points: 20 });
        assert_eq!(sample_font(&l, dlg), b"20.IBM Plex Mono Bold Italic\0");
        l.font_dlg_message(dlg, WM_COMMAND, DID_OK, 0);
        assert_eq!(l.std_dialog_return(dlg, DID_OK), 1);
        assert_eq!(l.guest_read::<u32>(FD + FN_LRETURN), Some(DID_OK));
        assert_eq!(l.read_guest_string(FAMILY), "IBM Plex Mono");
        assert_eq!(l.guest_read::<u32>(FD + FN_FX_POINT_SIZE as u32), Some(20 << 16));
        assert_eq!(l.guest_read::<u32>(FD + FN_FL_STYLE as u32),
                   Some((FATTR_SEL_BOLD | FATTR_SEL_ITALIC | FATTR_SEL_UNDERSCORE) as u32));
        assert_eq!(l.guest_read::<u32>(FD + FN_FL_TYPE), Some(FTYPE_ITALIC));
        assert_eq!(l.guest_read::<u16>(FD + FN_US_WEIGHT), Some(FWEIGHT_BOLD));
        assert_eq!(l.guest_read::<i16>(FD + FN_SNOMINAL_POINT_SIZE), Some(200));
        assert_eq!(l.read_guest_string(FD + (FN_FATTRS + FA_FACENAME) as u32), "IBM Plex Mono Bold Italic");
        assert_eq!(l.guest_read::<u16>(FD + (FN_FATTRS + FA_CODEPAGE) as u32), Some(850), "codepage kept");
        assert!(l.guest_read::<i32>(FD + FN_LEM_HEIGHT).unwrap() > 0);
    }

    #[test]
    fn reset_cancel_and_short_family_buffers() {
        let (l, frame) = session();
        fontdlg(&l, FNTS_RESETBUTTON | FNTS_FIXEDWIDTHONLY, "Courier", 10, 0);
        let dlg = open(&l, frame);
        assert_eq!(l.shared.window_mgr.lock_or_recover().std_dialogs.font(dlg).unwrap().families, [FontFamily::Mono]);
        l.std_select_item(dlg, DID_STYLE, 2);
        l.font_dlg_message(dlg, WM_CONTROL, (CBN_LBSELECT << 16) | DID_STYLE, 0);
        assert!(choice(&l, dlg).bold);
        l.font_dlg_message(dlg, WM_COMMAND, DID_RESET_BUTTON, 0);
        assert!(!choice(&l, dlg).bold, "back to the start");
        assert_eq!(l.std_item_text(dlg, DID_STYLE), "Normal");
        l.font_dlg_message(dlg, WM_COMMAND, DID_CANCEL, 0);
        assert_eq!(l.guest_read::<u32>(FD + FN_LRETURN), Some(DID_CANCEL));
        assert_eq!(l.read_guest_string(FAMILY), "Courier", "untouched");
        // A family buffer of 5 bytes takes 4 characters and the NUL.
        fontdlg(&l, 0, "Courier", 10, 0);
        l.guest_write::<u16>(FD + FN_US_FAMILY_BUF_LEN as u32, 5).unwrap();
        l.guest_write_bytes(FAMILY + 5, b"ZZ").unwrap();
        let dlg = open(&l, frame);
        l.std_dialog_procless(dlg, WM_COMMAND, DID_OK, 0).unwrap();
        assert_eq!(l.read_guest_string(FAMILY), "IBM ");
        assert_eq!(l.guest_read::<u8>(FAMILY + 5), Some(b'Z'), "nothing past the buffer");
    }

    #[test]
    fn errors_and_modeless() {
        let (l, frame) = session();
        fontdlg(&l, 0, "", 0, 0);
        l.guest_write::<u32>(FD, 40).unwrap();
        assert!(matches!(l.win_font_dlg(0, HWND_DESKTOP, frame, FD), ApiResult::Normal(0)));
        assert_eq!(l.guest_read::<u32>(FD + FN_LSRC), Some(FNTS_ERR_INVALID_VERSION));
        fontdlg(&l, FNTS_CUSTOM, "", 0, 0);
        assert!(matches!(l.win_font_dlg(0, HWND_DESKTOP, frame, FD), ApiResult::Normal(0)));
        assert_eq!(l.guest_read::<u32>(FD + FN_LSRC), Some(FNTS_ERR_DIALOG_LOAD_ERROR));
        fontdlg(&l, FNTS_MODELESS, "", 0, 0);
        let ApiResult::Normal(dlg) = l.win_font_dlg(0, HWND_DESKTOP, frame, FD) else { panic!("modeless") };
        assert_ne!(dlg, 0);
        assert_eq!(choice(&l, dlg).points, DEFAULT_POINTS);
        l.std_dialog_procless(dlg, WM_CLOSE, 0, 0).unwrap();
        assert_eq!(l.guest_read::<u32>(FD + FN_LRETURN), Some(DID_CANCEL));
        assert!(!l.shared.window_mgr.lock_or_recover().get_window(dlg).unwrap().visible);
    }
}
