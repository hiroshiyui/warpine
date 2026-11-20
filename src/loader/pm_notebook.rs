// SPDX-License-Identifier: GPL-3.0-or-later
//
//! `WC_NOTEBOOK`: pages with major and minor tabs, a status line and page
//! buttons (IBM *PM Programming Reference*, "Notebook Controls"; messages,
//! styles and structures from `<pmstddlg.h>` `INCL_WINSTDBOOK`).
//!
//! A notebook keeps an ordered list of pages ([`NotebookState`]). Each page
//! has an id (`BKM_INSERTPAGE` returns it), a style (`BKA_MAJOR` /
//! `BKA_MINOR` give it a tab, `BKA_STATUSTEXTON` shows its status line,
//! `BKA_AUTOPAGESIZE` makes the notebook size its window), application data,
//! a page window (`BKM_SETPAGEWINDOWHWND`; usually a dialog loaded with the
//! notebook as parent, embedded in it — ADR-0042), a tab text and a status
//! text. One page is the top page: its window is shown in the page area,
//! the others' are hidden.
//!
//! Layout ([`Layout`]): major tabs along the side `BKS_MAJORTAB*` names
//! (right by default), minor tabs — those of the top page's major section
//! — along the bottom (top / bottom major tabs: along the right), the status
//! line and two page buttons (previous / next) under the page;
//! `BKS_TABBEDDIALOG` (Warp 4) puts the major tabs in a row on top and has
//! no minor tabs, status line or page buttons. Tabs that do not fit their
//! band scroll: two arrows at the band's end move it by one tab, and
//! turning to a page brings its tab into view. Notebook buttons
//! (`BKM_SETNOTEBOOKBUTTONS`, Warp 4) are push buttons of kind
//! `BS_NOTEBOOKBUTTON` in a row along the notebook's bottom, common to all
//! pages; their `WM_COMMAND` goes to the top page's window (their owner
//! follows the top page), else to the notebook's owner. A tab shows its bitmap
//! (`BKM_SETTABBITMAP`, stretched to the tab) instead of its text, on its
//! colour (`BKM_SETTABCOLOR`). Sizes and colours are Warpine's own;
//! `BKS_SPIRALBIND`, `BKS_ROUNDEDTABS` / `BKS_POLYGONTABS` and the
//! back-page corners are drawn as the defaults.
//!
//! Page windows: `BKM_SETPAGEWINDOWHWND`, or `BKM_SETPAGEINFO` with
//! `BFA_PAGEFROMDLGRES` (a dialog resource) / `BFA_PAGEFROMDLGTEMPLATE` (a
//! `DLGTEMPLATE` in memory), loaded with the notebook as parent and owner
//! at once (`bLoadDlg` TRUE) or when the page is first turned to.
//!
//! Notifications: `WM_CONTROL` to the owner with `BKN_PAGESELECTEDPENDING`
//! and `BKN_PAGESELECTED` (a `PAGESELECTNOTIFY`) and `BKN_PAGEDELETED` (a
//! `DELETENOTIFY`) are sent through the owner-call queue (`pm_owner.rs`) —
//! the owner reads the structure before the call that turned or deleted
//! the page returns —, and `BKN_NEWPAGESIZE` is posted. A turn asks
//! `BKN_PAGESELECTEDPENDING` first; an owner that sets `ulPageIdNew` to 0
//! keeps the current page, and `BKN_PAGESELECTED` follows only a turn made.
//! Keys the notebook does not use go to its owner, so Tab moves from a
//! notebook in a dialog into its top page (`WindowManager::next_tab_stop`).
//!
//! Unverified (no source): whether inserting the first page or deleting
//! the top page selects a page with a notification (Warpine selects
//! silently), whether `BKM_QUERYPAGECOUNT` counts its end pages (Warpine
//! counts both), whether `BKN_PAGESELECTEDPENDING` is also sent for
//! `BKM_TURNTOPAGE` (it is here, which still returns TRUE when the owner
//! refuses), the owner of a loaded page dialog (the notebook here), where
//! the buttons' `WM_COMMAND` goes, `BKA_MAXBUTTONID` as the largest button
//! id accepted, `BKM_SETTABCOLOR` taking an RGB colour, a tab bitmap
//! replacing the text, and the arrows scrolling without turning a page.

use super::constants::*;
use super::pm_owner::{OwnerCall, OwnerCallKind};
use super::pm_types::{default_color, WindowManager};
use super::{ApiResult, Loader, MutexExt, Painter};

pub const CLASS_NOTEBOOK: &str = "WC_NOTEBOOK";

pub const BKM_CALCPAGERECT: u32 = 0x0353;
pub const BKM_DELETEPAGE: u32 = 0x0354;
pub const BKM_INSERTPAGE: u32 = 0x0355;
pub const BKM_INVALIDATETABS: u32 = 0x0356;
pub const BKM_TURNTOPAGE: u32 = 0x0357;
pub const BKM_QUERYPAGECOUNT: u32 = 0x0358;
pub const BKM_QUERYPAGEID: u32 = 0x0359;
pub const BKM_QUERYPAGEDATA: u32 = 0x035a;
pub const BKM_QUERYPAGEWINDOWHWND: u32 = 0x035b;
pub const BKM_QUERYTABBITMAP: u32 = 0x035c;
pub const BKM_QUERYTABTEXT: u32 = 0x035d;
pub const BKM_SETDIMENSIONS: u32 = 0x035e;
pub const BKM_SETPAGEDATA: u32 = 0x035f;
pub const BKM_SETPAGEWINDOWHWND: u32 = 0x0360;
pub const BKM_SETSTATUSLINETEXT: u32 = 0x0361;
pub const BKM_SETTABBITMAP: u32 = 0x0362;
pub const BKM_SETTABTEXT: u32 = 0x0363;
pub const BKM_SETNOTEBOOKCOLORS: u32 = 0x0364;
pub const BKM_QUERYPAGESTYLE: u32 = 0x0365;
pub const BKM_QUERYSTATUSLINETEXT: u32 = 0x0366;
pub const BKM_SETPAGEINFO: u32 = 0x0367;
pub const BKM_QUERYPAGEINFO: u32 = 0x0368;
pub const BKM_SETTABCOLOR: u32 = 0x0374;
pub const BKM_SETNOTEBOOKBUTTONS: u32 = 0x0375;

pub const BKN_PAGESELECTED: u32 = 130;
pub const BKN_NEWPAGESIZE: u32 = 131;
pub const BKN_PAGEDELETED: u32 = 133;
pub const BKN_PAGESELECTEDPENDING: u32 = 134;

pub const BKA_ALL: u32 = 1;
pub const BKA_SINGLE: u32 = 2;
pub const BKA_TAB: u32 = 4;
pub const BKA_LAST: u16 = 0x0002;
pub const BKA_FIRST: u16 = 0x0004;
pub const BKA_NEXT: u16 = 0x0008;
pub const BKA_PREV: u16 = 0x0010;
pub const BKA_TOP: u16 = 0x0020;
pub const BKA_MAJORTAB: u32 = 0x0001;
pub const BKA_MINORTAB: u32 = 0x0002;
pub const BKA_PAGEBUTTON: u32 = 0x0100;
pub const BKA_STATUSTEXTON: u16 = 0x0001;
pub const BKA_MAJOR: u16 = 0x0040;
pub const BKA_MINOR: u16 = 0x0080;
pub const BKA_AUTOPAGESIZE: u16 = 0x0100;
/// `BKM_SETTABCOLOR`: back to the notebook's tab colour.
pub const BKA_AUTOCOLOR: u32 = u32::MAX;
/// The largest notebook button id.
pub const BKA_MAXBUTTONID: u32 = 7999;

pub const BKS_MAJORTABRIGHT: u32 = 0x0000_0010;
pub const BKS_MAJORTABLEFT: u32 = 0x0000_0020;
pub const BKS_MAJORTABTOP: u32 = 0x0000_0040;
pub const BKS_MAJORTABBOTTOM: u32 = 0x0000_0080;
pub const BKS_STATUSTEXTRIGHT: u32 = 0x0000_1000;
pub const BKS_STATUSTEXTCENTER: u32 = 0x0000_2000;
pub const BKS_TABTEXTRIGHT: u32 = 0x0000_4000;
pub const BKS_TABTEXTCENTER: u32 = 0x0000_8000;
pub const BKS_TABBEDDIALOG: u32 = 0x0000_0800;

/// `BKA_*COLOR[INDEX]` attributes of `BKM_SETNOTEBOOKCOLORS`, in pairs
/// (index, RGB) for: page background, major tab background, minor tab
/// background, major tab text, minor tab text.
const BKA_BACKGROUNDPAGECOLORINDEX: u32 = 0x0001;
const BKA_FOREGROUNDMINORCOLOR: u32 = 0x000A;
const NOTEBOOK_COLORS: usize = 5;

/// `BOOKERR_INVALID_PARAMETERS`.
pub const BOOKERR_INVALID_PARAMETERS: u32 = u32::MAX;

/// `BOOKPAGEINFO.fl` flags (Warp 4).
const BFA_PAGEDATA: u32 = 0x0000_0001;
const BFA_PAGEFROMHWND: u32 = 0x0000_0002;
const BFA_PAGEFROMDLGTEMPLATE: u32 = 0x0000_0004;
const BFA_PAGEFROMDLGRES: u32 = 0x0000_0008;
const BFA_STATUSLINE: u32 = 0x0000_0010;
const BFA_MAJORTABBITMAP: u32 = 0x0000_0020;
const BFA_MINORTABBITMAP: u32 = 0x0000_0040;
const BFA_MAJORTABTEXT: u32 = 0x0000_0080;
const BFA_MINORTABTEXT: u32 = 0x0000_0100;
/// `BOOKPAGEINFO` field offsets (ULONGs / pointers, 4 bytes each).
const BPI_FL: u32 = 4;
const BPI_PAGEDATA: u32 = 12;
const BPI_LOADDLG: u32 = 8;
const BPI_HWNDPAGE: u32 = 16;
const BPI_PFNDLGPROC: u32 = 20;
const BPI_IDDLG: u32 = 24;
const BPI_HMODDLG: u32 = 28;
const BPI_CREATEPARAMS: u32 = 32;
const BPI_PDLGT: u32 = 36;
const BPI_CBSTATUSLINE: u32 = 40;
const BPI_PSZSTATUSLINE: u32 = 44;
const BPI_HBMMAJORTAB: u32 = 48;
const BPI_HBMMINORTAB: u32 = 52;
const BPI_CBMAJORTAB: u32 = 56;
const BPI_PSZMAJORTAB: u32 = 60;
const BPI_CBMINORTAB: u32 = 64;
const BPI_PSZMINORTAB: u32 = 68;
/// `BOOKTEXT`: `pString`, `textLen`.
const BOOKTEXT_LEN: u32 = 4;
/// `NOTEBOOKBUTTON`: `pszText`, `idButton`, `hImage`, `flStyle`.
const NOTEBOOK_BUTTON_BYTES: u32 = 16;
/// At most this many notebook buttons.
pub const MAX_NOTEBOOK_BUTTONS: usize = 32;

/// At most this many pages, and this long a tab or status text.
pub const MAX_PAGES: usize = 4096;
const MAX_TEXT: usize = 256;
/// The first page id handed out.
const FIRST_PAGE_ID: u32 = 0x100;

/// Warpine's sizes, in pels.
const MARGIN: i32 = 4;
const BACK_PAGES: i32 = 6;
const DEFAULT_MAJOR_TAB: (i32, i32) = (80, 22);
const DEFAULT_MINOR_TAB: (i32, i32) = (60, 20);
const DEFAULT_PAGE_BUTTON: (i32, i32) = (21, 21);
const TAB_TEXT_PAD: i32 = 6;
const MAX_DIMENSION: i32 = 1024;
/// A tab scroll arrow's short side, and the notebook buttons' height,
/// least width and text padding.
const ARROW: i32 = 16;
const BUTTON_H: i32 = 24;
const BUTTON_MIN_W: i32 = 60;
const BUTTON_PAD: i32 = 12;

/// Colours (RGB).
const PAGE_COLOR: u32 = 0x00F0_F0F0;
const TAB_COLOR: u32 = 0x00C8_C8C8;
const EDGE_COLOR: u32 = 0x0080_8080;
const LIGHT_COLOR: u32 = 0x00FF_FFFF;
const TEXT_COLOR: u32 = 0x0000_0000;
const FOCUS_COLOR: u32 = 0x0000_0080;

/// One page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub id: u32,
    pub style: u16,
    pub data: u32,
    pub hwnd: u32,
    pub tab_text: String,
    pub status: String,
    pub bitmap: u32,
    /// `BKM_SETTABCOLOR` (RGB); `None` = the notebook's tab colour.
    pub tab_color: Option<u32>,
    /// A page dialog to load when the page is first turned to.
    pub dialog: Option<PageDialog>,
}

/// A page dialog `BKM_SETPAGEINFO` names: its procedure, creation
/// parameters and where its template is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageDialog {
    pub proc_addr: u32,
    pub params: u32,
    pub source: DialogSource,
}

/// Where a page dialog's template is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogSource {
    /// Dialog resource `id` of module `hmod`.
    Resource { hmod: u32, id: u32 },
    /// A `DLGTEMPLATE` in guest memory.
    Template(u32),
}

/// A notebook button: its window and text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotebookButton {
    pub hwnd: u32,
    pub text: String,
}

impl Page {
    fn major(&self) -> bool { self.style & BKA_MAJOR != 0 }
    fn minor(&self) -> bool { self.style & BKA_MINOR != 0 && !self.major() }
}

/// A notebook's pages and settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotebookState {
    pub pages: Vec<Page>,
    next_id: u32,
    /// The top page's id (0: no pages).
    pub top: u32,
    pub major_tab: Option<(i32, i32)>,
    pub minor_tab: Option<(i32, i32)>,
    pub page_button: (i32, i32),
    /// `BKM_SETNOTEBOOKCOLORS`, by attribute pair (RGB).
    pub colors: [Option<u32>; NOTEBOOK_COLORS],
    /// The size the page windows were last laid out for.
    pub laid_out: (i32, i32),
    /// The first major / minor tab shown when they do not all fit.
    pub major_first: usize,
    pub minor_first: usize,
    /// `BKM_SETNOTEBOOKBUTTONS`.
    pub buttons: Vec<NotebookButton>,
}

impl Default for NotebookState {
    fn default() -> Self {
        NotebookState {
            pages: Vec::new(), next_id: FIRST_PAGE_ID, top: 0, major_tab: None, minor_tab: None,
            page_button: DEFAULT_PAGE_BUTTON, colors: [None; NOTEBOOK_COLORS], laid_out: (0, 0),
            major_first: 0, minor_first: 0, buttons: Vec::new(),
        }
    }
}

impl NotebookState {
    pub fn index(&self, id: u32) -> Option<usize> {
        (id != 0).then(|| self.pages.iter().position(|p| p.id == id)).flatten()
    }

    pub fn page(&self, id: u32) -> Option<&Page> {
        self.index(id).map(|i| &self.pages[i])
    }

    pub fn page_mut(&mut self, id: u32) -> Option<&mut Page> {
        self.index(id).map(|i| &mut self.pages[i])
    }

    /// `BKM_INSERTPAGE`: a new page with `style` at `order` (`BKA_FIRST`,
    /// `BKA_LAST`, or `BKA_NEXT` / `BKA_PREV` of page `reference`); its id,
    /// or `None` (a bad order or reference, too many pages). The first page
    /// becomes the top page.
    pub fn insert(&mut self, reference: u32, style: u16, order: u16) -> Option<u32> {
        if self.pages.len() >= MAX_PAGES {
            return None;
        }
        let at = match order {
            BKA_FIRST => 0,
            BKA_LAST => self.pages.len(),
            BKA_NEXT => self.index(reference)? + 1,
            BKA_PREV => self.index(reference)?,
            _ => return None,
        };
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        self.pages.insert(at, Page { id, style, ..Page::default() });
        if self.top == 0 {
            self.top = id;
        }
        Some(id)
    }

    /// The pages `BKM_DELETEPAGE` removes for page `id` and `how`
    /// (`BKA_SINGLE`, `BKA_TAB`: the page and the following ones up to the
    /// next tab of its level or above, `BKA_ALL`), in order.
    pub fn deletion(&self, id: u32, how: u32) -> Option<Vec<usize>> {
        if how & BKA_ALL != 0 {
            return Some((0..self.pages.len()).collect());
        }
        let i = self.index(id)?;
        // BKA_SINGLE (also when no flag is given) wins over BKA_TAB.
        if how & BKA_SINGLE != 0 || how & BKA_TAB == 0 {
            return Some(vec![i]);
        }
        let p = &self.pages[i];
        let ends = |q: &Page| if p.major() { q.major() } else { q.major() || q.minor() };
        let end = self.pages[i + 1..].iter().position(ends).map_or(self.pages.len(), |n| i + 1 + n);
        Some((i..end).collect())
    }

    /// Remove pages `which` (indexes, ascending); a removed top page is
    /// replaced by the page after it (else before it). Returns the pages.
    pub fn remove(&mut self, which: &[usize]) -> Vec<Page> {
        let top_at = self.index(self.top);
        let mut removed = Vec::with_capacity(which.len());
        for &i in which.iter().rev() {
            if i < self.pages.len() {
                removed.push(self.pages.remove(i));
            }
        }
        removed.reverse();
        if self.index(self.top).is_none() {
            let at = top_at.map_or(0, |t| t.saturating_sub(which.iter().filter(|&&i| i < t).count()));
            self.top = self.pages.get(at).or(self.pages.last()).map_or(0, |p| p.id);
        }
        removed
    }

    /// `BKM_QUERYPAGEID`: the page `order` of `reference`, optionally only
    /// pages with `filter` (`BKA_MAJOR` / `BKA_MINOR`) style; `Ok(0)` when
    /// there is none, `Err` for a bad reference or order.
    pub fn query_id(&self, reference: u32, order: u16, filter: u16) -> Result<u32, ()> {
        let wanted = |p: &&Page| filter == 0 || p.style & filter != 0;
        let found = match order {
            BKA_FIRST => self.pages.iter().find(wanted),
            BKA_LAST => self.pages.iter().rev().find(wanted),
            BKA_TOP => return Ok(self.top),
            BKA_NEXT => { let i = self.index(reference).ok_or(())?; self.pages[i + 1..].iter().find(wanted) }
            BKA_PREV => { let i = self.index(reference).ok_or(())?; self.pages[..i].iter().rev().find(wanted) }
            _ => return Err(()),
        };
        Ok(found.map_or(0, |p| p.id))
    }

    /// `BKM_QUERYPAGECOUNT`: pages from `first` to `last` (0 = the first /
    /// last page), both included; `None` for a bad id.
    pub fn count(&self, first: u32, last: u32) -> Option<u32> {
        if self.pages.is_empty() {
            return Some(0);
        }
        let a = if first == 0 { 0 } else { self.index(first)? };
        let b = if last == 0 { self.pages.len() - 1 } else { self.index(last)? };
        Some(if b >= a { (b - a + 1) as u32 } else { 0 })
    }

    /// The index of the major page whose section holds the page at `i`.
    fn section_start(&self, i: usize) -> Option<usize> {
        (0..=i).rev().find(|&j| self.pages[j].major())
    }

    /// The major-tab pages, and the minor-tab pages of the top page's section.
    pub fn tabs(&self) -> (Vec<usize>, Vec<usize>) {
        let majors = (0..self.pages.len()).filter(|&i| self.pages[i].major()).collect();
        let minors = match self.index(self.top) {
            Some(t) => {
                let start = self.section_start(t).map_or(0, |s| s + 1);
                let end = self.pages[start..].iter().position(Page::major).map_or(self.pages.len(), |n| start + n);
                (start..end).filter(|&i| self.pages[i].minor()).collect()
            }
            None => Vec::new(),
        };
        (majors, minors)
    }

    /// The tab of the top page's section that is current (drawn attached).
    pub fn current_tabs(&self) -> (Option<usize>, Option<usize>) {
        let Some(t) = self.index(self.top) else { return (None, None) };
        let major = self.section_start(t);
        let minor = (major.map_or(0, |m| m + 1)..=t).rev().find(|&j| self.pages[j].minor());
        (major, minor)
    }
}

/// A rectangle `(x, y, cx, cy)`.
pub type Rect = (i32, i32, i32, i32);

/// How many tabs of `size` pels fit in `room` pels when `n` do not: room
/// is left for the scroll arrows (`arrows` pels), and one tab always
/// shows. `None` when all `n` fit.
pub fn tabs_fitting(n: usize, size: i32, room: i32, arrows: i32) -> Option<usize> {
    if size <= 0 || n as i64 * size as i64 <= room as i64 {
        return None;
    }
    Some(((room - arrows).max(0) / size).max(1) as usize)
}

/// The range of `n` tabs shown from `first` when `fit` of them fit
/// (`None`: all), kept inside `0..n`.
pub fn shown_tabs(first: usize, n: usize, fit: Option<usize>) -> std::ops::Range<usize> {
    match fit {
        None => 0..n,
        Some(v) => {
            let v = v.min(n);
            let f = first.min(n - v);
            f..f + v
        }
    }
}

/// The first tab to show so that tab `pos` is in view, moving the shown
/// range from `first` as little as possible.
pub fn reveal_tab(first: usize, pos: usize, fit: Option<usize>) -> usize {
    match fit {
        None => 0,
        Some(_) if pos < first => pos,
        Some(v) if pos >= first + v => pos + 1 - v,
        Some(_) => first,
    }
}

/// One band of tabs: the tabs shown with their rectangles, the scroll
/// arrows (previous, next) when not all fit, the first tab shown and how
/// many fit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TabBand {
    pub tabs: Vec<(usize, Rect)>,
    pub arrows: Option<(Rect, Rect)>,
    pub first: usize,
    pub fit: Option<usize>,
    /// A column (arrows up / down) rather than a row (left / right).
    pub vertical: bool,
}

/// Where a notebook's parts lie, in its own coordinates (bottom-left 0, 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub page: Rect,
    pub major: TabBand,
    pub minor: TabBand,
    pub status: Option<Rect>,
    pub prev_button: Option<Rect>,
    pub next_button: Option<Rect>,
    /// The notebook buttons, in the order of `NotebookState::buttons`.
    pub buttons: Vec<Rect>,
}

/// Which side the major tabs are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side { Right, Left, Top, Bottom }

fn major_side(style: u32) -> Side {
    if style & BKS_TABBEDDIALOG != 0 || style & BKS_MAJORTABTOP != 0 { Side::Top }
    else if style & BKS_MAJORTABRIGHT != 0 { Side::Right }
    else if style & BKS_MAJORTABLEFT != 0 { Side::Left }
    else if style & BKS_MAJORTABBOTTOM != 0 { Side::Bottom }
    else { Side::Right } // the default
}

/// A column of tabs `size` going down from `top` at `x`, the arrows under
/// it, within `bottom..top`.
fn tab_column(idx: &[usize], first: usize, size: (i32, i32), x: i32, bottom: i32, top: i32) -> TabBand {
    let fit = tabs_fitting(idx.len(), size.1, top - bottom, ARROW);
    let shown = shown_tabs(first, idx.len(), fit);
    let tabs = idx[shown.clone()].iter().enumerate()
        .map(|(n, &i)| (i, (x, top - (n as i32 + 1) * size.1, size.0, size.1))).collect();
    let half = size.0 / 2;
    let arrows = fit.map(|_| ((x, bottom, half, ARROW), (x + half, bottom, size.0 - half, ARROW)));
    TabBand { tabs, arrows, first: shown.start, fit, vertical: true }
}

/// A row of tabs `size` going right from `left` at `y`, the arrows at its
/// right end, within `left..right`.
fn tab_row(idx: &[usize], first: usize, size: (i32, i32), y: i32, left: i32, right: i32) -> TabBand {
    let fit = tabs_fitting(idx.len(), size.0, right - left, 2 * ARROW);
    let shown = shown_tabs(first, idx.len(), fit);
    let tabs = idx[shown.clone()].iter().enumerate()
        .map(|(n, &i)| (i, (left + n as i32 * size.0, y, size.0, size.1))).collect();
    let arrows = fit.map(|_| ((right - 2 * ARROW, y, ARROW, size.1), (right - ARROW, y, ARROW, size.1)));
    TabBand { tabs, arrows, first: shown.start, fit, vertical: false }
}

impl Layout {
    /// The layout of notebook state `s` with `style` at size `cx` × `cy`;
    /// `text_w` measures a tab or button text.
    pub fn new(s: &NotebookState, style: u32, cx: i32, cy: i32, text_w: &dyn Fn(&str) -> i32) -> Layout {
        let (majors, minors) = s.tabs();
        let tabbed = style & BKS_TABBEDDIALOG != 0;
        let side = major_side(style);
        let widest = |idx: &[usize], min: i32| idx.iter()
            .map(|&i| text_w(&s.pages[i].tab_text) + 2 * TAB_TEXT_PAD).max().unwrap_or(0).max(min);
        let major = s.major_tab.unwrap_or((widest(&majors, DEFAULT_MAJOR_TAB.0), DEFAULT_MAJOR_TAB.1));
        let minor = s.minor_tab.unwrap_or((widest(&minors, DEFAULT_MINOR_TAB.0), DEFAULT_MINOR_TAB.1));
        let (mut l, mut b, mut r, mut t) = (MARGIN, MARGIN, cx - MARGIN, cy - MARGIN);
        // Notebook buttons: a row along the bottom, below everything.
        let mut buttons = Vec::with_capacity(s.buttons.len());
        if !s.buttons.is_empty() {
            let mut x = l;
            for button in &s.buttons {
                let w = (text_w(&button.text) + 2 * BUTTON_PAD).max(BUTTON_MIN_W);
                buttons.push((x, b, w, BUTTON_H));
                x += w + MARGIN;
            }
            b += BUTTON_H + MARGIN;
        }
        // Major tabs: a band along their side, one tab per major page.
        let mut major_band = TabBand::default();
        if !majors.is_empty() {
            match side {
                Side::Right | Side::Left => {
                    let x = if side == Side::Right { r - major.0 } else { l };
                    major_band = tab_column(&majors, s.major_first, major, x, b, t);
                    if side == Side::Right { r -= major.0 } else { l += major.0 }
                }
                Side::Top | Side::Bottom => {
                    let y = if side == Side::Top { t - major.1 } else { b };
                    major_band = tab_row(&majors, s.major_first, major, y, l, r);
                    if side == Side::Top { t -= major.1 } else { b += major.1 }
                }
            }
        }
        if tabbed {
            let page = (l + 1, b + 1, (r - l - 2).max(0), (t - b - 2).max(0));
            return Layout { page, major: major_band, minor: TabBand::default(), status: None,
                            prev_button: None, next_button: None, buttons };
        }
        // Minor tabs of the current section: along the bottom (the right
        // for top / bottom major tabs).
        let mut minor_band = TabBand::default();
        if !minors.is_empty() {
            if matches!(side, Side::Right | Side::Left) {
                minor_band = tab_row(&minors, s.minor_first, minor, b, l, r);
                b += minor.1;
            } else {
                minor_band = tab_column(&minors, s.minor_first, minor, r - minor.0, b, t);
                r -= minor.0;
            }
        }
        // Back pages: a few edges on the outer sides of the page.
        r -= BACK_PAGES;
        b += BACK_PAGES;
        // Status line and page buttons along the page's bottom.
        let (bw, bh) = s.page_button;
        let next_button = Some((r - MARGIN - bw, b + MARGIN, bw, bh));
        let prev_button = Some((r - MARGIN - 2 * bw - 2, b + MARGIN, bw, bh));
        let status = Some((l + MARGIN, b + MARGIN, (r - l - 3 * MARGIN - 2 * bw - 2).max(0), bh));
        b += bh + 2 * MARGIN;
        let page = (l + 1, b, (r - l - 2).max(0), (t - b - 1).max(0));
        Layout { page, major: major_band, minor: minor_band, status, prev_button, next_button, buttons }
    }

    /// What is under point `(x, y)`.
    pub fn hit(&self, (x, y): (i32, i32)) -> Option<Hit> {
        let inside = |(rx, ry, rcx, rcy): Rect| x >= rx && x < rx + rcx && y >= ry && y < ry + rcy;
        if let Some(&(i, _)) = self.major.tabs.iter().chain(&self.minor.tabs).find(|(_, rc)| inside(*rc)) {
            return Some(Hit::Tab(i));
        }
        for (band, major) in [(&self.major, true), (&self.minor, false)] {
            if let Some((prev, next)) = band.arrows {
                if inside(prev) { return Some(Hit::Scroll { major, forward: false }); }
                if inside(next) { return Some(Hit::Scroll { major, forward: true }); }
            }
        }
        if self.prev_button.is_some_and(inside) { return Some(Hit::Prev); }
        if self.next_button.is_some_and(inside) { return Some(Hit::Next); }
        None
    }
}

/// What a click on a notebook hit: a tab (its page index), the previous /
/// next page button, or a tab scroll arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit { Tab(usize), Prev, Next, Scroll { major: bool, forward: bool } }

/// A text from guest memory for a tab or status line, at most `MAX_TEXT` bytes.
fn clip_text(mut t: String) -> String {
    super::pm_slider::truncate_utf8(&mut t, MAX_TEXT);
    t
}

impl WindowManager {
    /// Give a new `WC_NOTEBOOK` window its state.
    pub fn build_notebook(&mut self, hwnd: u32) {
        if let Some(w) = self.get_window_mut(hwnd)
            && w.class_name == CLASS_NOTEBOOK && w.notebook.is_none() {
            w.notebook = Some(Box::default());
        }
    }
}

impl Loader {
    fn notebook_info(&self, hwnd: u32) -> Option<(u32, i32, i32, NotebookState)> {
        let wm = self.shared.window_mgr.lock_or_recover();
        let w = wm.get_window(hwnd)?;
        Some((w.style, w.cx, w.cy, (**w.notebook.as_ref()?).clone()))
    }

    fn update_notebook<R>(&self, hwnd: u32, f: impl FnOnce(&mut NotebookState) -> R) -> Option<R> {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        wm.get_window_mut(hwnd)?.notebook.as_deref_mut().map(f)
    }

    fn notebook_layout(&self, hwnd: u32, s: &NotebookState, style: u32, cx: i32, cy: i32) -> Layout {
        let font = Painter::new(self, 0, hwnd).font();
        Layout::new(s, style, cx, cy, &|t| crate::gui::font::text_width(t, font))
    }

    /// Show the top page's window (sized to the page when it asks for it;
    /// a page dialog not loaded yet is loaded first), hide the other pages'
    /// windows, bring the current tabs into view, place the notebook
    /// buttons (owned by the top page's window), and redraw the notebook.
    fn notebook_place_pages(&self, hwnd: u32) {
        self.notebook_load_top_dialog(hwnd);
        let Some((style, cx, cy, s)) = self.notebook_info(hwnd) else { return };
        let lay = self.notebook_layout(hwnd, &s, style, cx, cy);
        let (majors, minors) = s.tabs();
        let (cur_major, cur_minor) = s.current_tabs();
        let pos = |idx: &[usize], cur: Option<usize>| cur.and_then(|c| idx.iter().position(|&i| i == c));
        let major_first = pos(&majors, cur_major).map_or(lay.major.first, |p| reveal_tab(lay.major.first, p, lay.major.fit));
        let minor_first = pos(&minors, cur_minor).map_or(lay.minor.first, |p| reveal_tab(lay.minor.first, p, lay.minor.fit));
        let top_hwnd = s.page(s.top).map_or(0, |p| p.hwnd);
        let mut wm = self.shared.window_mgr.lock_or_recover();
        for p in s.pages.iter().filter(|p| p.hwnd != 0) {
            let top = p.id == s.top;
            let Some(w) = wm.get_window_mut(p.hwnd) else { continue };
            if top && p.style & BKA_AUTOPAGESIZE != 0 {
                (w.x, w.y, w.cx, w.cy) = lay.page;
            }
            w.visible = top;
            if top {
                wm.invalidate_window_only(p.hwnd);
            }
        }
        let button_owner = if top_hwnd != 0 && wm.get_window(top_hwnd).is_some() { top_hwnd } else { wm.notify_target(hwnd) };
        for (button, &rect) in s.buttons.iter().zip(&lay.buttons) {
            if let Some(w) = wm.get_window_mut(button.hwnd) {
                (w.x, w.y, w.cx, w.cy) = rect;
                w.owner = button_owner;
            }
        }
        if let Some(n) = wm.get_window_mut(hwnd).and_then(|w| w.notebook.as_deref_mut()) {
            n.laid_out = (cx, cy);
            n.major_first = major_first;
            n.minor_first = minor_first;
        }
        drop(wm);
        self.repaint_control(hwnd);
    }

    /// Load the top page's dialog when `BKM_SETPAGEINFO` left it for later.
    fn notebook_load_top_dialog(&self, hwnd: u32) {
        let Some((top, dialog)) = self.update_notebook(hwnd, |s| {
            let top = s.top;
            s.page_mut(top).and_then(|p| p.dialog.take()).map(|d| (top, d))
        }).flatten() else { return };
        let dlg = self.notebook_load_dialog(hwnd, dialog);
        self.update_notebook(hwnd, |s| if let Some(p) = s.page_mut(top) { p.hwnd = dlg; });
    }

    /// Load page dialog `d` with notebook `hwnd` as parent and owner; its
    /// window (0 when the template cannot be read).
    fn notebook_load_dialog(&self, hwnd: u32, d: PageDialog) -> u32 {
        let vcpu = super::interrupt::current_vcpu().unwrap_or(0);
        match d.source {
            DialogSource::Resource { hmod, id } => self.load_dialog(vcpu, hwnd, hwnd, d.proc_addr, hmod, id, d.params).0,
            DialogSource::Template(addr) => self.load_dialog_template(vcpu, hwnd, hwnd, d.proc_addr, addr, d.params),
        }
    }

    /// Turn to page `id`: ask the owner `BKN_PAGESELECTEDPENDING` first (an
    /// owner call); the turn itself follows its answer
    /// ([`Self::notebook_turn_answered`]). FALSE for a page that does not exist.
    fn notebook_turn(&self, hwnd: u32, id: u32) -> bool {
        let Some((_, _, _, s)) = self.notebook_info(hwnd) else { return false };
        if s.index(id).is_none() { return false; }
        if s.top != id {
            self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
                control: hwnd, kind: OwnerCallKind::NotebookPending { cur: s.top, new: id },
            });
        }
        true
    }

    /// The owner answered `BKN_PAGESELECTEDPENDING` for a turn from page
    /// `cur` to `new` with `block` (its `PAGESELECTNOTIFY` as it left it;
    /// empty when it was not asked): `ulPageIdNew` 0 keeps the page,
    /// anything else turns to `new` and sends `BKN_PAGESELECTED`.
    pub(super) fn notebook_turn_answered(&self, hwnd: u32, cur: u32, new: u32, block: &[u32]) {
        if block.get(2) == Some(&0) {
            return;
        }
        let Some(turned) = self.update_notebook(hwnd, |s| {
            (s.top == cur && s.index(new).is_some()).then(|| s.top = new).is_some()
        }) else { return };
        if !turned { return; }
        self.notebook_place_pages(hwnd);
        self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
            control: hwnd, kind: OwnerCallKind::NotebookSelected { cur, new },
        });
    }

    /// Read the text at `ptr` (a `PSZ`; 0 = empty) in the notebook's codepage.
    fn notebook_text(&self, hwnd: u32, ptr: u32) -> String {
        if ptr == 0 { return String::new(); }
        clip_text(self.read_guest_string_cp(ptr, self.window_codepage(hwnd)))
    }

    /// Answer a `BOOKTEXT` query at `pbt` with `text`.
    fn notebook_text_out(&self, hwnd: u32, text: &str, pbt: u32) -> u32 {
        let bytes = super::codepage::cp_encode(text, self.window_codepage(hwnd));
        if pbt == 0 { return bytes.len() as u32; }
        let (Some(buf), Some(len)) = (self.guest_read::<u32>(pbt), self.guest_read::<u32>(pbt.wrapping_add(BOOKTEXT_LEN)))
        else { return BOOKERR_INVALID_PARAMETERS };
        self.copy_text_out(&bytes, buf, len as usize)
    }

    /// A message for notebook `hwnd`; `None` when it is not a notebook.
    pub(super) fn notebook_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let (style, cx, cy, s) = self.notebook_info(hwnd)?;
        let ok = |b: bool| ApiResult::Normal(b as u32);
        let err = ApiResult::Normal(BOOKERR_INVALID_PARAMETERS);
        Some(match msg {
            WM_BUTTON1DOWN => {
                if style & WS_DISABLED != 0 { return Some(ok(false)); }
                self.set_focus_window(hwnd);
                let lay = self.notebook_layout(hwnd, &s, style, cx, cy);
                let at = ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32);
                let target = match lay.hit(at) {
                    Some(Hit::Tab(i)) => s.pages.get(i).map(|p| p.id),
                    Some(Hit::Prev) => s.index(s.top).and_then(|t| t.checked_sub(1)).map(|i| s.pages[i].id),
                    Some(Hit::Next) => s.index(s.top).and_then(|t| s.pages.get(t + 1)).map(|p| p.id),
                    Some(Hit::Scroll { major, forward }) => {
                        let band = if major { &lay.major } else { &lay.minor };
                        let first = if forward { band.first + 1 } else { band.first.saturating_sub(1) };
                        self.update_notebook(hwnd, |n| if major { n.major_first = first } else { n.minor_first = first });
                        self.repaint_control(hwnd);
                        None
                    }
                    None => None,
                };
                if let Some(id) = target { self.notebook_turn(hwnd, id); }
                ok(true)
            }
            WM_CHAR => {
                // Keys the notebook does not use (Tab, Enter, Esc...) go to its owner.
                let used = self.notebook_char(hwnd, style, &s, mp1, mp2);
                if !used {
                    let owner = self.shared.window_mgr.lock_or_recover().notify_target(hwnd);
                    if owner != 0 && owner != hwnd {
                        self.post_control_message(owner, WM_CHAR, mp1, mp2);
                    }
                }
                ok(used)
            }
            // mp1 = reference page, mp2 = MPFROM2SHORT(style, order).
            BKM_INSERTPAGE => {
                let id = self.update_notebook(hwnd, |s| s.insert(mp1, mp2 as u16, (mp2 >> 16) as u16)).flatten();
                if id.is_some() { self.repaint_control(hwnd); }
                ApiResult::Normal(id.unwrap_or(0))
            }
            // mp1 = page, mp2 = BKA_SINGLE / BKA_TAB / BKA_ALL.
            BKM_DELETEPAGE => {
                let Some(which) = s.deletion(mp1, mp2 & 0xFFFF) else { return Some(ok(false)) };
                let removed = self.update_notebook(hwnd, |s| s.remove(&which)).unwrap_or_default();
                {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    for p in &removed {
                        wm.queue_owner_call(OwnerCall {
                            control: hwnd,
                            kind: OwnerCallKind::NotebookDeleted { page: p.hwnd, data: p.data, bitmap: p.bitmap },
                        });
                    }
                }
                self.notebook_place_pages(hwnd);
                ok(true)
            }
            BKM_TURNTOPAGE => ok(self.notebook_turn(hwnd, mp1)),
            // mp2 = MPFROM2SHORT(order, style filter).
            BKM_QUERYPAGEID => s.query_id(mp1, mp2 as u16, (mp2 >> 16) as u16).map_or(err, ApiResult::Normal),
            BKM_QUERYPAGECOUNT => s.count(mp1, mp2).map_or(err, ApiResult::Normal),
            BKM_QUERYPAGESTYLE => s.page(mp1).map_or(err, |p| ApiResult::Normal(p.style as u32)),
            BKM_QUERYPAGEDATA => s.page(mp1).map_or(err, |p| ApiResult::Normal(p.data)),
            BKM_SETPAGEDATA => ok(self.update_notebook(hwnd, |s| s.page_mut(mp1).map(|p| p.data = mp2)).flatten().is_some()),
            BKM_QUERYPAGEWINDOWHWND => s.page(mp1).map_or(err, |p| ApiResult::Normal(p.hwnd)),
            BKM_SETPAGEWINDOWHWND => {
                let set = self.update_notebook(hwnd, |s| s.page_mut(mp1).map(|p| p.hwnd = mp2)).flatten().is_some();
                if set { self.notebook_place_pages(hwnd); }
                ok(set)
            }
            BKM_SETTABTEXT => {
                let text = self.notebook_text(hwnd, mp2);
                let set = self.update_notebook(hwnd, |s| s.page_mut(mp1).map(|p| p.tab_text = text)).flatten().is_some();
                if set { self.repaint_control(hwnd); }
                ok(set)
            }
            BKM_QUERYTABTEXT => s.page(mp1).map_or(err, |p| ApiResult::Normal(self.notebook_text_out(hwnd, &p.tab_text, mp2))),
            BKM_SETSTATUSLINETEXT => {
                let text = self.notebook_text(hwnd, mp2);
                let set = self.update_notebook(hwnd, |s| s.page_mut(mp1).map(|p| p.status = text)).flatten().is_some();
                if set { self.repaint_control(hwnd); }
                ok(set)
            }
            BKM_QUERYSTATUSLINETEXT => s.page(mp1).map_or(err, |p| ApiResult::Normal(self.notebook_text_out(hwnd, &p.status, mp2))),
            BKM_SETTABBITMAP => {
                let set = self.update_notebook(hwnd, |s| s.page_mut(mp1).map(|p| p.bitmap = mp2)).flatten().is_some();
                if set { self.repaint_control(hwnd); }
                ok(set)
            }
            BKM_QUERYTABBITMAP => s.page(mp1).map_or(err, |p| ApiResult::Normal(p.bitmap)),
            // mp1 = MPFROM2SHORT(width, height), mp2 = BKA_MAJORTAB / BKA_MINORTAB / BKA_PAGEBUTTON.
            BKM_SETDIMENSIONS => {
                let size = ((mp1 & 0xFFFF) as i32, (mp1 >> 16) as i32);
                if !(1..=MAX_DIMENSION).contains(&size.0) || !(1..=MAX_DIMENSION).contains(&size.1) {
                    return Some(ok(false));
                }
                let set = self.update_notebook(hwnd, |s| match mp2 & 0xFFFF {
                    BKA_MAJORTAB => { s.major_tab = Some(size); true }
                    BKA_MINORTAB => { s.minor_tab = Some(size); true }
                    BKA_PAGEBUTTON => { s.page_button = size; true }
                    _ => false,
                }).unwrap_or(false);
                if set { self.notebook_place_pages(hwnd); }
                ok(set)
            }
            // mp1 = PRECTL, mp2 = TRUE: notebook → page rectangle, FALSE: page → notebook.
            BKM_CALCPAGERECT => {
                let Some(r) = self.read_rectl(mp1) else { return Some(ok(false)) };
                let lay = self.notebook_layout(hwnd, &s, style, cx, cy);
                let (px, py, pcx, pcy) = lay.page;
                let (dl, db, dr, dt) = (px, py, cx - px - pcx, cy - py - pcy);
                let out = if mp2 != 0 {
                    super::pm_rect::Rectl { x_left: r.x_left + dl, y_bottom: r.y_bottom + db,
                                            x_right: r.x_right - dr, y_top: r.y_top - dt }
                } else {
                    super::pm_rect::Rectl { x_left: r.x_left - dl, y_bottom: r.y_bottom - db,
                                            x_right: r.x_right + dr, y_top: r.y_top + dt }
                };
                ok(self.write_rectl(mp1, out))
            }
            // mp1 = colour, mp2 = BKA_* attribute (index or RGB form).
            BKM_SETNOTEBOOKCOLORS => {
                if !(BKA_BACKGROUNDPAGECOLORINDEX..=BKA_FOREGROUNDMINORCOLOR).contains(&mp2) {
                    return Some(ok(false));
                }
                let slot = ((mp2 - 1) / 2) as usize;
                let rgb = if mp2 % 2 == 1 { default_color(mp1) } else { mp1 & 0x00FF_FFFF };
                self.update_notebook(hwnd, |s| s.colors[slot] = Some(rgb));
                self.repaint_control(hwnd);
                ok(true)
            }
            BKM_INVALIDATETABS => ok(self.repaint_control(hwnd)),
            BKM_SETPAGEINFO => ok(self.notebook_set_page_info(hwnd, mp1, mp2)),
            BKM_QUERYPAGEINFO => ok(self.notebook_query_page_info(hwnd, &s, mp1, mp2)),
            // mp1 = page, mp2 = RGB colour or BKA_AUTOCOLOR.
            BKM_SETTABCOLOR => {
                let color = (mp2 != BKA_AUTOCOLOR).then_some(mp2 & 0x00FF_FFFF);
                let set = self.update_notebook(hwnd, |s| s.page_mut(mp1).map(|p| p.tab_color = color)).flatten().is_some();
                if set { self.repaint_control(hwnd); }
                ok(set)
            }
            // mp1 = number of buttons, mp2 = PNOTEBOOKBUTTON array.
            BKM_SETNOTEBOOKBUTTONS => ok(self.notebook_set_buttons(hwnd, mp1, mp2)),
            _ => return None,
        })
    }

    /// `BKM_SETPAGEINFO` (mp1 = page, mp2 = `PBOOKPAGEINFO`): the fields
    /// `fl` names; a page dialog (`BFA_PAGEFROMDLGRES` /
    /// `BFA_PAGEFROMDLGTEMPLATE`) is loaded now or kept for the first turn
    /// to the page, as `bLoadDlg` says.
    fn notebook_set_page_info(&self, hwnd: u32, id: u32, pbpi: u32) -> bool {
        let rd = |o: u32| self.guest_read::<u32>(pbpi.wrapping_add(o));
        let (Some(fl), true) = (rd(BPI_FL), pbpi != 0) else { return false };
        if self.notebook_info(hwnd).and_then(|(_, _, _, s)| s.index(id)).is_none() {
            return false;
        }
        let field = |flag: u32, o: u32| if fl & flag != 0 { rd(o) } else { None };
        let text = |flag: u32, o: u32| (fl & flag != 0).then(|| self.notebook_text(hwnd, rd(o).unwrap_or(0)));
        let (data, page, status) = (field(BFA_PAGEDATA, BPI_PAGEDATA), field(BFA_PAGEFROMHWND, BPI_HWNDPAGE),
                                    text(BFA_STATUSLINE, BPI_PSZSTATUSLINE));
        let tab = text(BFA_MAJORTABTEXT, BPI_PSZMAJORTAB).or_else(|| text(BFA_MINORTABTEXT, BPI_PSZMINORTAB));
        let bitmap = field(BFA_MAJORTABBITMAP, BPI_HBMMAJORTAB).or_else(|| field(BFA_MINORTABBITMAP, BPI_HBMMINORTAB));
        let source = if fl & BFA_PAGEFROMDLGRES != 0 {
            Some(DialogSource::Resource { hmod: rd(BPI_HMODDLG).unwrap_or(0), id: rd(BPI_IDDLG).unwrap_or(0) })
        } else if fl & BFA_PAGEFROMDLGTEMPLATE != 0 {
            rd(BPI_PDLGT).filter(|&a| a != 0).map(DialogSource::Template)
        } else {
            None
        };
        let dialog = source.map(|source| PageDialog {
            proc_addr: rd(BPI_PFNDLGPROC).unwrap_or(0), params: rd(BPI_CREATEPARAMS).unwrap_or(0), source,
        });
        // bLoadDlg TRUE: the dialog now; FALSE: when the page is turned to.
        let (dialog, loaded) = match dialog {
            Some(d) if rd(BPI_LOADDLG).unwrap_or(0) != 0 => (None, Some(self.notebook_load_dialog(hwnd, d))),
            other => (other, None),
        };
        let page = loaded.or(page);
        let set = self.update_notebook(hwnd, |s| s.page_mut(id).map(|p| {
            if dialog.is_some() { p.dialog = dialog; }
            if let Some(v) = data { p.data = v; }
            if let Some(v) = page { p.hwnd = v; }
            if let Some(v) = status { p.status = v; }
            if let Some(v) = tab { p.tab_text = v; }
            if let Some(v) = bitmap { p.bitmap = v; }
        })).flatten().is_some();
        if set { self.notebook_place_pages(hwnd); }
        set
    }

    /// `BKM_SETNOTEBOOKBUTTONS` (`count` `NOTEBOOKBUTTON`s at `array`):
    /// replace the notebook buttons by push buttons of kind
    /// `BS_NOTEBOOKBUTTON` with these texts, ids and styles. FALSE for more
    /// than `MAX_NOTEBOOK_BUTTONS`, an unreadable array or an id above
    /// `BKA_MAXBUTTONID`. `hImage` is not drawn.
    fn notebook_set_buttons(&self, hwnd: u32, count: u32, array: u32) -> bool {
        if count as usize > MAX_NOTEBOOK_BUTTONS || (count != 0 && array == 0) {
            return false;
        }
        let rd = |o: u32| self.guest_read::<u32>(array.wrapping_add(o));
        let mut specs = Vec::with_capacity(count as usize);
        for i in 0..count {
            let at = i * NOTEBOOK_BUTTON_BYTES;
            let (Some(text), Some(id), Some(style)) = (rd(at), rd(at + 4), rd(at + 12)) else { return false };
            if id > BKA_MAXBUTTONID {
                return false;
            }
            specs.push((self.notebook_text(hwnd, text), id, style));
        }
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some(hmq) = wm.get_window(hwnd).filter(|w| w.notebook.is_some()).map(|w| w.hmq) else { return false };
            let old = wm.get_window_mut(hwnd).and_then(|w| w.notebook.as_deref_mut())
                .map(|n| std::mem::take(&mut n.buttons)).unwrap_or_default();
            for b in old {
                wm.destroy_window(b.hwnd);
            }
            let mut buttons = Vec::with_capacity(specs.len());
            for (text, id, style) in specs {
                let b = wm.create_window("WC_BUTTON".into(), hwnd, hmq);
                if let Some(w) = wm.get_window_mut(b) {
                    w.text = text.clone();
                    w.id = id;
                    w.style = WS_VISIBLE | BS_NOTEBOOKBUTTON | (style & !BS_PRIMARYSTYLES);
                    w.visible = true;
                }
                buttons.push(NotebookButton { hwnd: b, text });
            }
            if let Some(n) = wm.get_window_mut(hwnd).and_then(|w| w.notebook.as_deref_mut()) {
                n.buttons = buttons;
            }
        }
        self.notebook_place_pages(hwnd);
        true
    }

    /// `BKM_QUERYPAGEINFO`: fill the fields `fl` names (texts as far as
    /// their `cb*` buffers allow).
    fn notebook_query_page_info(&self, hwnd: u32, s: &NotebookState, id: u32, pbpi: u32) -> bool {
        let Some(p) = s.page(id) else { return false };
        let Some(fl) = (pbpi != 0).then(|| self.guest_read::<u32>(pbpi.wrapping_add(BPI_FL))).flatten() else { return false };
        let wr = |o: u32, v: u32| { self.guest_write::<u32>(pbpi.wrapping_add(o), v); };
        let text_into = |cb: u32, psz: u32, text: &str| {
            let (Some(len), Some(buf)) = (self.guest_read::<u32>(pbpi.wrapping_add(cb)), self.guest_read::<u32>(pbpi.wrapping_add(psz)))
            else { return };
            let bytes = super::codepage::cp_encode(text, self.window_codepage(hwnd));
            self.copy_text_out(&bytes, buf, len as usize);
        };
        if fl & BFA_PAGEDATA != 0 { wr(BPI_PAGEDATA, p.data); }
        if fl & BFA_PAGEFROMHWND != 0 { wr(BPI_HWNDPAGE, p.hwnd); }
        if fl & BFA_STATUSLINE != 0 { text_into(BPI_CBSTATUSLINE, BPI_PSZSTATUSLINE, &p.status); }
        if fl & BFA_MAJORTABTEXT != 0 { text_into(BPI_CBMAJORTAB, BPI_PSZMAJORTAB, &p.tab_text); }
        if fl & BFA_MINORTABTEXT != 0 { text_into(BPI_CBMINORTAB, BPI_PSZMINORTAB, &p.tab_text); }
        if fl & BFA_MAJORTABBITMAP != 0 { wr(BPI_HBMMAJORTAB, p.bitmap); }
        if fl & BFA_MINORTABBITMAP != 0 { wr(BPI_HBMMINORTAB, p.bitmap); }
        true
    }

    /// Keys for a focused notebook: Alt+PageUp / Alt+PageDown (and
    /// PageUp / PageDown) turn one page, Left / Up and Right / Down move to
    /// the previous / next major tab, Home / End to the first / last page.
    fn notebook_char(&self, hwnd: u32, style: u32, s: &NotebookState, mp1: u32, mp2: u32) -> bool {
        let fs = mp1 & 0xFFFF;
        if fs & KC_KEYUP != 0 || fs & KC_VIRTUALKEY == 0 || style & WS_DISABLED != 0 {
            return false;
        }
        let vk = mp2 >> WM_CHAR_VKEY_SHIFT;
        let Some(t) = s.index(s.top) else { return false };
        let (majors, _) = s.tabs();
        let target = match vk {
            VK_PAGEUP => t.checked_sub(1),
            VK_PAGEDOWN => (t + 1 < s.pages.len()).then_some(t + 1),
            VK_HOME => Some(0),
            VK_END => s.pages.len().checked_sub(1),
            VK_LEFT | VK_UP => majors.iter().rev().copied().find(|&m| m < s.section_start(t).unwrap_or(t)),
            VK_RIGHT | VK_DOWN => majors.iter().copied().find(|&m| m > t),
            _ => return false,
        };
        if let Some(i) = target { self.notebook_turn(hwnd, s.pages[i].id); }
        true
    }

    /// Paint notebook `hwnd` at `(x, y, cx, cy)` in its frame's surface;
    /// the top page's window paints itself over the page area afterwards.
    pub(super) fn paint_notebook(&self, p: &Painter, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32)) {
        let Some((style, _, _, s)) = self.notebook_info(hwnd) else { return };
        if s.laid_out != (cx, cy) && s.laid_out != (0, 0) {
            // Resized: the page windows follow, the owner hears of it.
            self.update_notebook(hwnd, |n| n.laid_out = (cx, cy));
            self.notebook_place_pages(hwnd);
            self.shared.window_mgr.lock_or_recover().notify_owner(hwnd, BKN_NEWPAGESIZE, hwnd);
            return;
        }
        let lay = self.notebook_layout(hwnd, &s, style, cx, cy);
        let face = p.bg.unwrap_or(super::CONTROL_FACE_COLOR);
        let page_bg = s.colors[0].unwrap_or(PAGE_COLOR);
        let at = |(rx, ry, rcx, rcy): (i32, i32, i32, i32)| (x + rx, y + ry, x + rx + rcx - 1, y + ry + rcy - 1);
        let outline = |r: (i32, i32, i32, i32), light: u32, dark: u32| {
            let (l, b, rr, t) = at(r);
            p.fill((l, t, rr, t), light);
            p.fill((l, b, l, t), light);
            p.fill((l, b, rr, b), dark);
            p.fill((rr, b, rr, t), dark);
        };
        p.fill((x, y, x + cx - 1, y + cy - 1), face);
        // Back pages: stacked edges right of and below the page.
        if style & BKS_TABBEDDIALOG == 0 {
            let (px, py, pcx, pcy) = lay.page;
            for k in (1..=BACK_PAGES / 2).rev() {
                let d = 2 * k;
                outline((px + d, py - d - lay.status.map_or(0, |st| st.3 + 2 * MARGIN), pcx,
                         pcy + lay.status.map_or(0, |st| st.3 + 2 * MARGIN)), LIGHT_COLOR, EDGE_COLOR);
            }
        }
        // The page (with the status line area below it).
        let (px, py, pcx, pcy) = lay.page;
        let below = lay.status.map_or(0, |st| st.3 + 2 * MARGIN);
        let sheet = (px - 1, py - below, pcx + 2, pcy + below + 1);
        let (sl, sb, sr, st) = at(sheet);
        p.fill((sl, sb, sr, st), page_bg);
        outline(sheet, LIGHT_COLOR, EDGE_COLOR);
        let (cur_major, cur_minor) = s.current_tabs();
        let tab_text = |i: usize, r: (i32, i32, i32, i32), color: u32| {
            let (l, b, rr, t) = at(r);
            // A tab bitmap stands for the text, stretched to the tab.
            let bitmap = s.pages[i].bitmap;
            if bitmap != 0 && self.paint_bitmap_handle(p, (l + 2, b + 2, rr - l - 3, t - b - 3), bitmap, true) {
                return;
            }
            let text = &s.pages[i].tab_text;
            let w = crate::gui::font::text_width(text, p.font());
            let tx = if style & BKS_TABTEXTCENTER != 0 { l + (rr - l + 1 - w) / 2 }
                     else if style & BKS_TABTEXTRIGHT != 0 { rr - TAB_TEXT_PAD - w }
                     else { l + TAB_TEXT_PAD };
            p.text(tx, b + (r.3 - 16) / 2, text, color);
        };
        // Tabs: the current one in the page's colour, the others in their
        // own (BKM_SETTABCOLOR) or the notebook's.
        for (band, current, bg, fg) in [(&lay.major, cur_major, s.colors[1], s.colors[3]),
                                        (&lay.minor, cur_minor, s.colors[2], s.colors[4])] {
            for &(i, r) in &band.tabs {
                let (l, b, rr, t) = at(r);
                let own = s.pages[i].tab_color.or(bg).unwrap_or(TAB_COLOR);
                p.fill((l, b, rr, t), if Some(i) == current { page_bg } else { own });
                outline(r, LIGHT_COLOR, EDGE_COLOR);
                tab_text(i, r, fg.unwrap_or(TEXT_COLOR));
            }
            // Scroll arrows when not all tabs fit.
            if let Some((prev, next)) = band.arrows {
                let labels = if band.vertical { ["^", "v"] } else { ["<", ">"] };
                for (r, label) in [(prev, labels[0]), (next, labels[1])] {
                    let (l, b, rr, t) = at(r);
                    p.fill((l, b, rr, t), face);
                    outline(r, LIGHT_COLOR, EDGE_COLOR);
                    let w = crate::gui::font::text_width(label, p.font());
                    p.text(l + (rr - l + 1 - w) / 2, b + (r.3 - 16) / 2, label, TEXT_COLOR);
                }
            }
        }
        // Status line and page buttons.
        if let (Some(st), Some(top)) = (lay.status, s.page(s.top))
            && top.style & BKA_STATUSTEXTON != 0 {
            let (l, b, rr, _) = at(st);
            let w = crate::gui::font::text_width(&top.status, p.font());
            let tx = if style & BKS_STATUSTEXTCENTER != 0 { l + (rr - l + 1 - w) / 2 }
                     else if style & BKS_STATUSTEXTRIGHT != 0 { rr - w } else { l };
            p.text(tx, b + (st.3 - 16) / 2, &top.status, TEXT_COLOR);
        }
        for (button, label) in [(lay.prev_button, "<"), (lay.next_button, ">")] {
            let Some(r) = button else { continue };
            let (l, b, rr, t) = at(r);
            p.fill((l, b, rr, t), face);
            outline(r, LIGHT_COLOR, EDGE_COLOR);
            let w = crate::gui::font::text_width(label, p.font());
            p.text(l + (rr - l + 1 - w) / 2, b + (r.3 - 16) / 2, label, TEXT_COLOR);
        }
        // The focus: a line under the current major tab.
        let focused = self.shared.window_mgr.lock_or_recover().focus_hwnd == hwnd;
        if focused && let Some(&(_, r)) = lay.major.tabs.iter().find(|(i, _)| Some(*i) == cur_major) {
            let (l, b, rr, _) = at(r);
            p.fill((l + 2, b + 2, rr - 2, b + 2), FOCUS_COLOR);
        }
        // The top page's window paints over the page again.
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if let Some(top) = s.page(s.top) && top.hwnd != 0 && wm.get_window(top.hwnd).is_some_and(|w| w.visible) {
            wm.invalidate_window_only(top.hwnd);
        }
        // So do the notebook buttons.
        for b in &s.buttons {
            if wm.get_window(b.hwnd).is_some() {
                wm.invalidate_window_only(b.hwnd);
            }
        }
        if s.laid_out == (0, 0)
            && let Some(n) = wm.get_window_mut(hwnd).and_then(|w| w.notebook.as_deref_mut()) {
            n.laid_out = (cx, cy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(styles: &[u16]) -> NotebookState {
        let mut s = NotebookState::default();
        for &st in styles {
            s.insert(0, st, BKA_LAST).unwrap();
        }
        s
    }

    fn ids(s: &NotebookState) -> Vec<u32> {
        s.pages.iter().map(|p| p.id).collect()
    }

    #[test]
    fn pages_insert_in_order_and_the_first_is_on_top() {
        let mut s = NotebookState::default();
        let a = s.insert(0, BKA_MAJOR, BKA_LAST).unwrap();
        let c = s.insert(0, 0, BKA_LAST).unwrap();
        let b = s.insert(a, BKA_MINOR, BKA_NEXT).unwrap();
        let z = s.insert(0, BKA_MAJOR, BKA_FIRST).unwrap();
        let y = s.insert(a, 0, BKA_PREV).unwrap();
        assert_eq!(ids(&s), vec![z, y, a, b, c]);
        assert_eq!(s.top, a, "the first page inserted");
        assert_eq!(s.insert(0xDEAD, 0, BKA_NEXT), None, "a bad reference");
        assert_eq!(s.insert(a, 0, 0x4000), None, "a bad order");
        assert!(a >= FIRST_PAGE_ID && ids(&s).iter().all(|&i| i != 0));
    }

    #[test]
    fn page_queries() {
        let s = book(&[BKA_MAJOR, BKA_MINOR, 0, BKA_MAJOR, 0]);
        let p = ids(&s);
        assert_eq!(s.query_id(0, BKA_FIRST, 0), Ok(p[0]));
        assert_eq!(s.query_id(0, BKA_LAST, 0), Ok(p[4]));
        assert_eq!(s.query_id(p[1], BKA_NEXT, 0), Ok(p[2]));
        assert_eq!(s.query_id(p[1], BKA_NEXT, BKA_MAJOR), Ok(p[3]));
        assert_eq!(s.query_id(p[4], BKA_PREV, BKA_MINOR), Ok(p[1]));
        assert_eq!(s.query_id(p[4], BKA_NEXT, 0), Ok(0));
        assert_eq!(s.query_id(0, BKA_TOP, 0), Ok(p[0]));
        assert_eq!(s.query_id(0xBAD, BKA_NEXT, 0), Err(()));
        assert_eq!(s.count(0, 0), Some(5));
        assert_eq!(s.count(p[1], p[3]), Some(3));
        assert_eq!(s.count(p[3], p[1]), Some(0));
        assert_eq!(s.count(0xBAD, 0), None);
    }

    #[test]
    fn deleting_a_tab_takes_its_section() {
        let s = book(&[BKA_MAJOR, BKA_MINOR, 0, BKA_MINOR, 0, BKA_MAJOR, 0]);
        let p = ids(&s);
        assert_eq!(s.deletion(p[0], BKA_TAB), Some(vec![0, 1, 2, 3, 4]), "a major tab: up to the next major");
        assert_eq!(s.deletion(p[1], BKA_TAB), Some(vec![1, 2]), "a minor tab: up to the next tab");
        assert_eq!(s.deletion(p[2], BKA_SINGLE), Some(vec![2]));
        assert_eq!(s.deletion(0, BKA_ALL).map(|v| v.len()), Some(7));
        assert_eq!(s.deletion(0xBAD, BKA_SINGLE), None);
    }

    #[test]
    fn removing_the_top_page_moves_the_top_on() {
        let mut s = book(&[BKA_MAJOR, 0, 0, 0]);
        let p = ids(&s);
        s.top = p[1];
        let gone = s.remove(&[1, 2]);
        assert_eq!(gone.iter().map(|g| g.id).collect::<Vec<_>>(), vec![p[1], p[2]]);
        assert_eq!(s.top, p[3], "the page after the removed ones");
        s.remove(&[1]);
        assert_eq!(s.top, p[0], "else the one before");
        s.remove(&[0]);
        assert_eq!(s.top, 0, "no pages, no top page");
    }

    #[test]
    fn tabs_of_the_top_section() {
        let mut s = book(&[BKA_MAJOR, BKA_MINOR, 0, BKA_MINOR, BKA_MAJOR, BKA_MINOR]);
        let p = ids(&s);
        s.top = p[2];
        assert_eq!(s.tabs(), (vec![0, 4], vec![1, 3]));
        assert_eq!(s.current_tabs(), (Some(0), Some(1)));
        s.top = p[5];
        assert_eq!(s.tabs(), (vec![0, 4], vec![5]));
        assert_eq!(s.current_tabs(), (Some(4), Some(5)));
    }

    #[test]
    fn layout_places_tabs_page_and_buttons() {
        let mut s = book(&[BKA_MAJOR | BKA_STATUSTEXTON, BKA_MINOR, BKA_MAJOR]);
        s.top = s.pages[1].id;
        let w = |t: &str| 8 * t.len() as i32;
        let lay = Layout::new(&s, 0, 400, 300, &w);
        // Major tabs down the right, minor tabs along the bottom.
        assert_eq!(lay.major.tabs.len(), 2);
        assert!(lay.major.arrows.is_none() && lay.minor.arrows.is_none(), "everything fits");
        let (mx, my, mcx, mcy) = lay.major.tabs[0].1;
        assert_eq!((mx + mcx, my + mcy), (400 - MARGIN, 300 - MARGIN));
        assert_eq!(lay.major.tabs[1].1.1, my - mcy, "stacked downwards");
        assert_eq!(lay.minor.tabs.len(), 1);
        assert_eq!(lay.minor.tabs[0].1.1, MARGIN);
        let (px, py, pcx, pcy) = lay.page;
        assert!(px + pcx <= mx && py > lay.minor.tabs[0].1.1 + lay.minor.tabs[0].1.3 && py + pcy <= 300);
        assert!(lay.next_button.unwrap().0 > lay.prev_button.unwrap().0);
        assert_eq!(lay.hit((mx + 1, my + 1)), Some(Hit::Tab(0)));
        let (bx, by, _, _) = lay.next_button.unwrap();
        assert_eq!(lay.hit((bx + 1, by + 1)), Some(Hit::Next));
        assert_eq!(lay.hit((px + 5, py + 5)), None);
        // A tabbed dialog: tabs in a row on top, no buttons or minor tabs.
        let lay = Layout::new(&s, BKS_TABBEDDIALOG, 400, 300, &w);
        assert!(lay.minor.tabs.is_empty() && lay.next_button.is_none() && lay.status.is_none());
        let (t0, t1) = (lay.major.tabs[0].1, lay.major.tabs[1].1);
        assert_eq!((t0.1, t1.1), (t1.1, 300 - MARGIN - t0.3));
        assert_eq!(t1.0, t0.0 + t0.2);
        assert_eq!(lay.page.1 + lay.page.3, t0.1 - 1);
    }

    #[test]
    fn tabs_that_do_not_fit_scroll() {
        assert_eq!(tabs_fitting(3, 20, 100, 16), None, "all fit");
        assert_eq!(tabs_fitting(10, 20, 100, 16), Some(4), "room less the arrows");
        assert_eq!(tabs_fitting(10, 200, 100, 16), Some(1), "at least one");
        assert_eq!(shown_tabs(0, 10, None), 0..10);
        assert_eq!(shown_tabs(3, 10, Some(4)), 3..7);
        assert_eq!(shown_tabs(9, 10, Some(4)), 6..10, "kept inside the tabs");
        assert_eq!(reveal_tab(3, 8, Some(4)), 5, "a tab after the range");
        assert_eq!(reveal_tab(3, 1, Some(4)), 1, "a tab before it");
        assert_eq!(reveal_tab(3, 5, Some(4)), 3, "a tab in view");
        assert_eq!(reveal_tab(3, 5, None), 0);
    }

    #[test]
    fn layout_scrolls_a_long_band_and_places_buttons() {
        let mut s = book(&[BKA_MAJOR; 12]);
        s.major_first = 2;
        s.buttons = vec![NotebookButton { hwnd: 1, text: "OK".into() },
                         NotebookButton { hwnd: 2, text: "Cancel".into() }];
        let w = |t: &str| 8 * t.len() as i32;
        let lay = Layout::new(&s, 0, 400, 200, &w);
        // Buttons: a row along the bottom, below the tabs and the page.
        assert_eq!(lay.buttons.len(), 2);
        assert_eq!(lay.buttons[0], (MARGIN, MARGIN, BUTTON_MIN_W, BUTTON_H));
        assert_eq!(lay.buttons[1].0, MARGIN + BUTTON_MIN_W + MARGIN);
        let above = MARGIN + BUTTON_H + MARGIN;
        // Major tabs on the right: 12 × 22 pels do not fit in 200.
        let fit = lay.major.fit.expect("arrows");
        assert_eq!(fit, ((200 - MARGIN - above - ARROW) / DEFAULT_MAJOR_TAB.1) as usize);
        assert_eq!(lay.major.tabs.len(), fit);
        assert_eq!(lay.major.first, 2);
        assert_eq!(lay.major.tabs[0].0, 2, "the band starts at the third tab");
        let (prev, next) = lay.major.arrows.unwrap();
        assert_eq!(prev.1, above, "arrows at the band's foot");
        assert!(lay.major.vertical);
        assert_eq!(lay.hit((prev.0 + 1, prev.1 + 1)), Some(Hit::Scroll { major: true, forward: false }));
        assert_eq!(lay.hit((next.0 + 1, next.1 + 1)), Some(Hit::Scroll { major: true, forward: true }));
        assert!(lay.major.tabs.iter().all(|(_, r)| r.1 >= prev.1 + ARROW), "tabs above the arrows");
        assert!(lay.page.1 > above, "the page above the buttons");
        // A tabbed dialog: a row, arrows at its right end.
        let lay = Layout::new(&s, BKS_TABBEDDIALOG, 300, 200, &w);
        let (prev, next) = lay.major.arrows.unwrap();
        assert!(!lay.major.vertical);
        assert_eq!((next.0 + next.2, prev.0 + ARROW), (300 - MARGIN, next.0));
        assert!(lay.major.tabs.iter().all(|(_, r)| r.0 + r.2 <= prev.0));
    }

    struct Fx {
        loader: Loader,
        hmq: u32,
        client: u32,
        nb: u32,
    }

    /// What the tests look at of a window.
    struct Win {
        class_name: String, parent: u32, owner: u32, pfn_wp: u32, text: String, visible: bool,
        x: i32, y: i32, cx: i32, cy: i32, id: u32, style: u32,
    }

    /// Scratch guest memory.
    const BUF: u32 = 0x3000;

    /// A 380 × 250 notebook of `style` in a client whose procedure is at
    /// 0x5000 (so owner calls are made).
    fn fixture(style: u32) -> Fx {
        let loader = Loader::new_mock();
        let (hmq, client, nb) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            for h in [frame, client] {
                let w = wm.get_window_mut(h).unwrap();
                (w.cx, w.cy, w.visible) = (400, 300, true);
            }
            wm.get_window_mut(client).unwrap().pfn_wp = 0x5000;
            let nb = wm.create_window(CLASS_NOTEBOOK.into(), client, hmq);
            let w = wm.get_window_mut(nb).unwrap();
            (w.x, w.y, w.cx, w.cy, w.style, w.id, w.owner, w.visible) =
                (8, 8, 380, 250, WS_VISIBLE | style, 1, client, true);
            wm.build_notebook(nb);
            (hmq, client, nb)
        };
        Fx { loader, hmq, client, nb }
    }

    impl Fx {
        fn send(&self, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(self.nb, msg, mp1, mp2) {
                ApiResult::Normal(v) => v,
                other => panic!("unexpected {other:?}"),
            }
        }
        fn insert(&self, style: u16) -> u32 {
            self.send(BKM_INSERTPAGE, 0, (BKA_LAST as u32) << 16 | style as u32)
        }
        fn top(&self) -> u32 { self.send(BKM_QUERYPAGEID, 0, BKA_TOP as u32) }
        fn state(&self) -> NotebookState { self.loader.notebook_info(self.nb).unwrap().3 }
        fn calls(&self) -> Vec<OwnerCallKind> {
            std::mem::take(&mut self.loader.shared.window_mgr.lock_or_recover().owner_calls)
                .into_iter().map(|c| c.kind).collect()
        }
        /// Make the waiting owner calls, answering each with `answer`
        /// (the `PAGESELECTNOTIFY` as the owner leaves it); what was asked.
        fn answer(&self, answer: impl Fn(&[u32]) -> Vec<u32>) -> Vec<OwnerCallKind> {
            let mut asked = Vec::new();
            loop {
                let call = self.loader.shared.window_mgr.lock_or_recover().next_owner_call(self.hmq);
                let Some(call) = call else { break };
                let block = match self.loader.owner_call_setup(&call) {
                    super::super::pm_owner::OwnerSetup::Call { block, .. } => answer(&block),
                    super::super::pm_owner::OwnerSetup::Skip => Vec::new(),
                };
                asked.push(call.kind);
                self.loader.owner_call_finish(&call, 0, 0, &block);
            }
            asked
        }
        fn window(&self, h: u32) -> Win {
            let wm = self.loader.shared.window_mgr.lock_or_recover();
            let w = wm.get_window(h).unwrap();
            Win { class_name: w.class_name.clone(), parent: w.parent, owner: w.owner, pfn_wp: w.pfn_wp,
                  text: w.text.clone(), visible: w.visible, x: w.x, y: w.y, cx: w.cx, cy: w.cy, id: w.id, style: w.style }
        }
        fn posted(&self) -> Vec<(u32, u32, u32, u32)> {
            let wm = self.loader.shared.window_mgr.lock_or_recover();
            let mq = wm.get_mq(self.hmq).unwrap();
            let q = mq.lock_or_recover();
            q.messages.iter().map(|m| (m.hwnd, m.msg, m.mp1, m.mp2)).collect()
        }
    }

    #[test]
    fn a_turn_asks_the_owner_first_and_may_be_refused() {
        let fx = fixture(0);
        let (a, b) = (fx.insert(BKA_MAJOR), fx.insert(BKA_MAJOR));
        fx.calls();
        assert_eq!(fx.send(BKM_TURNTOPAGE, b, 0), 1);
        assert_eq!(fx.top(), a, "not turned before the owner answers");
        // The owner refuses: ulPageIdNew = 0.
        let asked = fx.answer(|blk| { assert_eq!(blk, [fx.nb, a, b]); vec![blk[0], blk[1], 0] });
        assert_eq!(asked, vec![OwnerCallKind::NotebookPending { cur: a, new: b }]);
        assert_eq!(fx.top(), a, "refused");
        // The owner agrees: the page turns, BKN_PAGESELECTED follows.
        fx.send(BKM_TURNTOPAGE, b, 0);
        let asked = fx.answer(|blk| blk.to_vec());
        assert_eq!(asked, vec![OwnerCallKind::NotebookPending { cur: a, new: b },
                               OwnerCallKind::NotebookSelected { cur: a, new: b }]);
        assert_eq!(fx.top(), b);
        // Turning to the top page asks nothing; a bad page fails.
        assert_eq!(fx.send(BKM_TURNTOPAGE, b, 0), 1);
        assert!(fx.calls().is_empty());
        assert_eq!(fx.send(BKM_TURNTOPAGE, 0xBAD, 0), 0);
        // An owner that is not asked (no procedure): the turn is made.
        fx.loader.shared.window_mgr.lock_or_recover().get_window_mut(fx.client).unwrap().pfn_wp = 0;
        fx.send(BKM_TURNTOPAGE, a, 0);
        fx.answer(|blk| blk.to_vec());
        assert_eq!(fx.top(), a);
    }

    #[test]
    fn a_turn_that_is_out_of_date_is_dropped() {
        let fx = fixture(0);
        let (a, b, c) = (fx.insert(BKA_MAJOR), fx.insert(BKA_MAJOR), fx.insert(BKA_MAJOR));
        fx.calls();
        fx.loader.notebook_turn_answered(fx.nb, b, c, &[]);
        assert_eq!(fx.top(), a, "the top page is no longer the turn's from page");
        fx.send(BKM_DELETEPAGE, b, BKA_SINGLE);
        fx.calls();
        fx.loader.notebook_turn_answered(fx.nb, a, b, &[]);
        assert_eq!(fx.top(), a, "the page went away meanwhile");
    }

    #[test]
    fn tab_colours_and_bitmaps() {
        let fx = fixture(0);
        let a = fx.insert(BKA_MAJOR);
        assert_eq!(fx.send(BKM_SETTABCOLOR, a, 0x0012_3456), 1);
        assert_eq!(fx.state().page(a).unwrap().tab_color, Some(0x12_3456));
        assert_eq!(fx.send(BKM_SETTABCOLOR, a, BKA_AUTOCOLOR), 1);
        assert_eq!(fx.state().page(a).unwrap().tab_color, None, "BKA_AUTOCOLOR: the notebook's");
        assert_eq!(fx.send(BKM_SETTABCOLOR, 0xBAD, 0), 0);
        assert_eq!(fx.send(BKM_SETTABBITMAP, a, 0x77), 1);
        assert_eq!(fx.send(BKM_QUERYTABBITMAP, a, 0), 0x77);
    }

    #[test]
    fn notebook_buttons_are_push_buttons_owned_by_the_top_page() {
        let fx = fixture(BKS_TABBEDDIALOG);
        let (a, b) = (fx.insert(BKA_MAJOR), fx.insert(BKA_MAJOR));
        let (pa, pb) = {
            let mut wm = fx.loader.shared.window_mgr.lock_or_recover();
            (wm.create_window(WindowManager::DIALOG_CLASS.into(), fx.nb, fx.hmq),
             wm.create_window(WindowManager::DIALOG_CLASS.into(), fx.nb, fx.hmq))
        };
        fx.send(BKM_SETPAGEWINDOWHWND, a, pa);
        fx.send(BKM_SETPAGEWINDOWHWND, b, pb);
        // NOTEBOOKBUTTON { pszText, idButton, hImage, flStyle } × 2.
        fx.loader.guest_write_bytes(BUF + 0x100, b"Undo\0Help\0").unwrap();
        for (i, (text, id, style)) in [(BUF + 0x100, 10, 0), (BUF + 0x105, 11, BS_HELP)].into_iter().enumerate() {
            for (o, v) in [(0, text), (4, id), (8, 0), (12, style)] {
                fx.loader.guest_write::<u32>(BUF + 16 * i as u32 + o, v).unwrap();
            }
        }
        assert_eq!(fx.send(BKM_SETNOTEBOOKBUTTONS, 2, BUF), 1);
        let buttons = fx.state().buttons;
        assert_eq!(buttons.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Undo", "Help"]);
        let w = fx.window(buttons[1].hwnd);
        assert_eq!((w.class_name.as_str(), w.parent, w.id), ("WC_BUTTON", fx.nb, 11));
        assert_eq!(w.style & BS_PRIMARYSTYLES, BS_NOTEBOOKBUTTON);
        assert!(w.style & BS_HELP != 0 && w.visible);
        assert!(fx.loader.shared.window_mgr.lock_or_recover().is_push_button(buttons[0].hwnd));
        assert_eq!(fx.window(buttons[0].hwnd).owner, pa, "the top page's window");
        let first = fx.window(buttons[0].hwnd);
        assert_eq!((first.x, first.y), (MARGIN, MARGIN), "along the notebook's bottom");
        // A turn hands the buttons to the new top page.
        fx.send(BKM_TURNTOPAGE, b, 0);
        fx.answer(|blk| blk.to_vec());
        assert_eq!(fx.window(buttons[0].hwnd).owner, pb);
        // Clicking one posts WM_COMMAND to it.
        fx.loader.click_button(buttons[0].hwnd, true);
        assert!(fx.posted().iter().any(|&(h, m, mp1, _)| (h, m, mp1) == (pb, WM_COMMAND, 10)));
        // Replacing them destroys the old ones; bad calls fail.
        assert_eq!(fx.send(BKM_SETNOTEBOOKBUTTONS, 1, BUF + 16), 1);
        assert!(fx.loader.shared.window_mgr.lock_or_recover().get_window(buttons[0].hwnd).is_none());
        assert_eq!(fx.state().buttons.len(), 1);
        fx.loader.guest_write::<u32>(BUF + 4, BKA_MAXBUTTONID + 1).unwrap();
        assert_eq!(fx.send(BKM_SETNOTEBOOKBUTTONS, 1, BUF), 0, "an id above BKA_MAXBUTTONID");
        assert_eq!(fx.send(BKM_SETNOTEBOOKBUTTONS, MAX_NOTEBOOK_BUTTONS as u32 + 1, BUF), 0);
        assert_eq!(fx.send(BKM_SETNOTEBOOKBUTTONS, 1, 0), 0);
        assert_eq!(fx.send(BKM_SETNOTEBOOKBUTTONS, 0, 0), 1, "no buttons");
        assert!(fx.state().buttons.is_empty());
    }

    /// Write a `BOOKPAGEINFO` at `BUF` for a page dialog from `fl`.
    fn page_info(fx: &Fx, fl: u32, load: bool, template: u32) {
        for o in (0..76).step_by(4) { fx.loader.guest_write::<u32>(BUF + o, 0).unwrap(); }
        for (o, v) in [(0, 76), (BPI_FL, fl), (BPI_LOADDLG, load as u32), (BPI_PFNDLGPROC, 0x6000),
                       (BPI_IDDLG, 101), (BPI_CREATEPARAMS, 0x1234), (BPI_PDLGT, template)] {
            fx.loader.guest_write::<u32>(BUF + o, v).unwrap();
        }
    }

    #[test]
    fn page_dialogs_load_at_once_or_on_the_first_turn() {
        use super::super::pm_dialog::tests::ABOUT_DIALOG;
        const TEMPLATE: u32 = 0x0009_0000;
        let fx = fixture(0);
        fx.loader.guest_write_bytes(TEMPLATE, ABOUT_DIALOG).unwrap();
        let (a, b) = (fx.insert(BKA_MAJOR | BKA_AUTOPAGESIZE), fx.insert(BKA_MAJOR));
        // Page a, on top: bLoadDlg TRUE, the dialog now.
        page_info(&fx, BFA_PAGEFROMDLGTEMPLATE, true, TEMPLATE);
        assert_eq!(fx.send(BKM_SETPAGEINFO, a, BUF), 1);
        let pa = fx.send(BKM_QUERYPAGEWINDOWHWND, a, 0);
        let w = fx.window(pa);
        assert_eq!((w.class_name.as_str(), w.parent, w.owner, w.pfn_wp), (WindowManager::DIALOG_CLASS, fx.nb, fx.nb, 0x6000));
        assert_eq!(w.text, "About PM Menu/Dialog Test");
        assert!(w.visible, "the top page's window is shown");
        let lay = fx.loader.notebook_layout(fx.nb, &fx.state(), 0, 380, 250);
        assert_eq!((w.x, w.y, w.cx, w.cy), lay.page, "BKA_AUTOPAGESIZE");
        assert!(fx.posted().contains(&(pa, WM_INITDLG, 0, 0x1234)), "WM_INITDLG with the creation parameters");
        // Page b: bLoadDlg FALSE, a resource dialog, loaded on the first turn.
        fx.loader.shared.resource_mgr.lock_or_recover().add(0, crate::lx::header::RT_DIALOG, 101, TEMPLATE,
                                                            ABOUT_DIALOG.len() as u32);
        page_info(&fx, BFA_PAGEFROMDLGRES, false, 0);
        assert_eq!(fx.send(BKM_SETPAGEINFO, b, BUF), 1);
        assert_eq!(fx.send(BKM_QUERYPAGEWINDOWHWND, b, 0), 0, "not loaded yet");
        fx.send(BKM_TURNTOPAGE, b, 0);
        fx.answer(|blk| blk.to_vec());
        let pb = fx.send(BKM_QUERYPAGEWINDOWHWND, b, 0);
        assert_ne!(pb, 0);
        assert_eq!(fx.window(pb).parent, fx.nb);
        assert!(fx.window(pb).visible && !fx.window(pa).visible);
        // Turning back and forth does not load it again.
        fx.send(BKM_TURNTOPAGE, a, 0);
        fx.answer(|blk| blk.to_vec());
        fx.send(BKM_TURNTOPAGE, b, 0);
        fx.answer(|blk| blk.to_vec());
        assert_eq!(fx.send(BKM_QUERYPAGEWINDOWHWND, b, 0), pb);
        assert_eq!(fx.send(BKM_SETPAGEINFO, 0xBAD, BUF), 0, "a bad page");
    }

    #[test]
    fn arrows_scroll_the_tabs_and_a_turn_brings_its_tab_into_view() {
        let fx = fixture(0);
        let pages: Vec<u32> = (0..14).map(|_| fx.insert(BKA_MAJOR)).collect();
        fx.calls();
        let lay = fx.loader.notebook_layout(fx.nb, &fx.state(), 0, 380, 250);
        let fit = lay.major.fit.expect("14 tabs do not fit 250 pels");
        let (_, next) = lay.major.arrows.unwrap();
        fx.send(WM_BUTTON1DOWN, (next.0 as u32 + 2) | (next.1 as u32 + 2) << 16, 0);
        assert_eq!(fx.state().major_first, 1, "one tab on");
        assert_eq!(fx.top(), pages[0], "no page turned");
        fx.send(BKM_TURNTOPAGE, pages[13], 0);
        fx.answer(|blk| blk.to_vec());
        assert_eq!(fx.state().major_first, 14 - fit, "the last tab in view");
        fx.send(BKM_TURNTOPAGE, pages[0], 0);
        fx.answer(|blk| blk.to_vec());
        assert_eq!(fx.state().major_first, 0);
    }

    #[test]
    fn arrows_scroll_a_tabbed_dialog_row() {
        let fx = fixture(BKS_TABBEDDIALOG);
        let pages: Vec<u32> = (0..6).map(|_| fx.insert(BKA_MAJOR)).collect();
        fx.calls();
        let lay = fx.loader.notebook_layout(fx.nb, &fx.state(), BKS_TABBEDDIALOG | WS_VISIBLE, 380, 250);
        let (_, next) = lay.major.arrows.expect("arrows");
        for n in 1..=2 {
            fx.send(WM_BUTTON1DOWN, (next.0 as u32 + 2) | (next.1 as u32 + 2) << 16, 0);
            assert_eq!(fx.state().major_first, n);
        }
        assert_eq!(fx.top(), pages[0]);
    }

    #[test]
    fn keys_the_notebook_does_not_use_go_to_its_owner() {
        let fx = fixture(0);
        fx.insert(BKA_MAJOR);
        let tab = KC_VIRTUALKEY;
        assert_eq!(fx.send(WM_CHAR, tab, VK_TAB << WM_CHAR_VKEY_SHIFT), 0);
        assert!(fx.posted().contains(&(fx.client, WM_CHAR, tab, VK_TAB << WM_CHAR_VKEY_SHIFT)));
        assert_eq!(fx.send(WM_CHAR, tab, VK_END << WM_CHAR_VKEY_SHIFT), 1, "End is the notebook's");
    }
}
