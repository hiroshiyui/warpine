// SPDX-License-Identifier: GPL-3.0-only
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
//! no minor tabs, status line or page buttons. Sizes and colours are
//! Warpine's own; tab bitmaps (`BKM_SETTABBITMAP`), tab colours and
//! notebook buttons (`BKM_SETNOTEBOOKBUTTONS`) are kept but not drawn.
//!
//! Notifications: `WM_CONTROL` to the owner with `BKN_PAGESELECTED` (a
//! `PAGESELECTNOTIFY`) and `BKN_PAGEDELETED` (a `DELETENOTIFY`) are sent
//! through the owner-call queue (`pm_owner.rs`) — the owner reads the
//! structure before the call that turned or deleted the page returns —
//! and `BKN_NEWPAGESIZE` is posted. Unverified (no source): whether
//! inserting the first page or deleting the top page selects a page with
//! a notification (Warpine selects silently), `BKN_PAGESELECTEDPENDING`
//! (not sent), and whether `BKM_QUERYPAGECOUNT` counts its end pages
//! (Warpine counts both).

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
const BFA_STATUSLINE: u32 = 0x0000_0010;
const BFA_MAJORTABBITMAP: u32 = 0x0000_0020;
const BFA_MINORTABBITMAP: u32 = 0x0000_0040;
const BFA_MAJORTABTEXT: u32 = 0x0000_0080;
const BFA_MINORTABTEXT: u32 = 0x0000_0100;
/// `BOOKPAGEINFO` field offsets (ULONGs / pointers, 4 bytes each).
const BPI_FL: u32 = 4;
const BPI_PAGEDATA: u32 = 12;
const BPI_HWNDPAGE: u32 = 16;
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
}

impl Default for NotebookState {
    fn default() -> Self {
        NotebookState {
            pages: Vec::new(), next_id: FIRST_PAGE_ID, top: 0, major_tab: None, minor_tab: None,
            page_button: DEFAULT_PAGE_BUTTON, colors: [None; NOTEBOOK_COLORS], laid_out: (0, 0),
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

/// Where a notebook's parts lie, in its own coordinates (bottom-left 0, 0;
/// rectangles `(x, y, cx, cy)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub page: (i32, i32, i32, i32),
    pub major_tabs: Vec<(usize, (i32, i32, i32, i32))>,
    pub minor_tabs: Vec<(usize, (i32, i32, i32, i32))>,
    pub status: Option<(i32, i32, i32, i32)>,
    pub prev_button: Option<(i32, i32, i32, i32)>,
    pub next_button: Option<(i32, i32, i32, i32)>,
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

impl Layout {
    /// The layout of notebook state `s` with `style` at size `cx` × `cy`;
    /// `text_w` measures a tab text.
    pub fn new(s: &NotebookState, style: u32, cx: i32, cy: i32, text_w: &dyn Fn(&str) -> i32) -> Layout {
        let (majors, minors) = s.tabs();
        let tabbed = style & BKS_TABBEDDIALOG != 0;
        let side = major_side(style);
        let widest = |idx: &[usize], min: i32| idx.iter()
            .map(|&i| text_w(&s.pages[i].tab_text) + 2 * TAB_TEXT_PAD).max().unwrap_or(0).max(min);
        let major = s.major_tab.unwrap_or((widest(&majors, DEFAULT_MAJOR_TAB.0), DEFAULT_MAJOR_TAB.1));
        let minor = s.minor_tab.unwrap_or((widest(&minors, DEFAULT_MINOR_TAB.0), DEFAULT_MINOR_TAB.1));
        let (mut l, mut b, mut r, mut t) = (MARGIN, MARGIN, cx - MARGIN, cy - MARGIN);
        let mut major_tabs = Vec::new();
        let mut minor_tabs = Vec::new();
        // Major tabs: a band along their side, one tab per major page.
        if !majors.is_empty() {
            match side {
                Side::Right | Side::Left => {
                    let x = if side == Side::Right { r - major.0 } else { l };
                    for (n, &i) in majors.iter().enumerate() {
                        let y = t - (n as i32 + 1) * major.1;
                        major_tabs.push((i, (x, y, major.0, major.1)));
                    }
                    if side == Side::Right { r -= major.0 } else { l += major.0 }
                }
                Side::Top | Side::Bottom => {
                    let y = if side == Side::Top { t - major.1 } else { b };
                    for (n, &i) in majors.iter().enumerate() {
                        major_tabs.push((i, (l + n as i32 * major.0, y, major.0, major.1)));
                    }
                    if side == Side::Top { t -= major.1 } else { b += major.1 }
                }
            }
        }
        if tabbed {
            let page = (l + 1, b + 1, (r - l - 2).max(0), (t - b - 2).max(0));
            return Layout { page, major_tabs, minor_tabs, status: None, prev_button: None, next_button: None };
        }
        // Minor tabs of the current section: along the bottom (the right
        // for top / bottom major tabs).
        if !minors.is_empty() {
            if matches!(side, Side::Right | Side::Left) {
                for (n, &i) in minors.iter().enumerate() {
                    minor_tabs.push((i, (l + n as i32 * minor.0, b, minor.0, minor.1)));
                }
                b += minor.1;
            } else {
                for (n, &i) in minors.iter().enumerate() {
                    let y = t - (n as i32 + 1) * minor.1;
                    minor_tabs.push((i, (r - minor.0, y, minor.0, minor.1)));
                }
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
        Layout { page, major_tabs, minor_tabs, status, prev_button, next_button }
    }

    /// The page or tab under point `(x, y)`: a tab's page index, or the
    /// previous (`-1`) / next (`1`) page button.
    pub fn hit(&self, (x, y): (i32, i32)) -> Option<Hit> {
        let inside = |(rx, ry, rcx, rcy): (i32, i32, i32, i32)| x >= rx && x < rx + rcx && y >= ry && y < ry + rcy;
        if let Some(&(i, _)) = self.major_tabs.iter().chain(&self.minor_tabs).find(|(_, rc)| inside(*rc)) {
            return Some(Hit::Tab(i));
        }
        if self.prev_button.is_some_and(inside) { return Some(Hit::Prev); }
        if self.next_button.is_some_and(inside) { return Some(Hit::Next); }
        None
    }
}

/// What a click on a notebook hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit { Tab(usize), Prev, Next }

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

    /// Show the top page's window (sized to the page when it asks for it),
    /// hide the other pages' windows, and redraw the notebook.
    fn notebook_place_pages(&self, hwnd: u32) {
        let Some((style, cx, cy, s)) = self.notebook_info(hwnd) else { return };
        let lay = self.notebook_layout(hwnd, &s, style, cx, cy);
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
        if let Some(n) = wm.get_window_mut(hwnd).and_then(|w| w.notebook.as_deref_mut()) {
            n.laid_out = (cx, cy);
        }
        drop(wm);
        self.repaint_control(hwnd);
    }

    /// Make page `id` the top page and send `BKN_PAGESELECTED`.
    fn notebook_turn(&self, hwnd: u32, id: u32) -> bool {
        let Some(cur) = self.update_notebook(hwnd, |s| {
            s.index(id)?;
            Some(std::mem::replace(&mut s.top, id))
        }).flatten() else { return false };
        self.notebook_place_pages(hwnd);
        if cur != id {
            self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
                control: hwnd, kind: OwnerCallKind::NotebookSelected { cur, new: id },
            });
        }
        true
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
                    None => None,
                };
                if let Some(id) = target { self.notebook_turn(hwnd, id); }
                ok(true)
            }
            WM_CHAR => ok(self.notebook_char(hwnd, style, &s, mp1, mp2)),
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
            BKM_SETTABBITMAP => ok(self.update_notebook(hwnd, |s| s.page_mut(mp1).map(|p| p.bitmap = mp2)).flatten().is_some()),
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
            // Kept but not drawn.
            BKM_SETTABCOLOR | BKM_SETNOTEBOOKBUTTONS => ok(true),
            _ => return None,
        })
    }

    /// `BKM_SETPAGEINFO` (mp1 = page, mp2 = `PBOOKPAGEINFO`): the fields
    /// `fl` names. Loading a page dialog (`BFA_PAGEFROMDLGRES` /
    /// `BFA_PAGEFROMDLGTEMPLATE`) is not supported.
    fn notebook_set_page_info(&self, hwnd: u32, id: u32, pbpi: u32) -> bool {
        let rd = |o: u32| self.guest_read::<u32>(pbpi.wrapping_add(o));
        let (Some(fl), true) = (rd(BPI_FL), pbpi != 0) else { return false };
        let field = |flag: u32, o: u32| if fl & flag != 0 { rd(o) } else { None };
        let text = |flag: u32, o: u32| (fl & flag != 0).then(|| self.notebook_text(hwnd, rd(o).unwrap_or(0)));
        let (data, page, status) = (field(BFA_PAGEDATA, BPI_PAGEDATA), field(BFA_PAGEFROMHWND, BPI_HWNDPAGE),
                                    text(BFA_STATUSLINE, BPI_PSZSTATUSLINE));
        let tab = text(BFA_MAJORTABTEXT, BPI_PSZMAJORTAB).or_else(|| text(BFA_MINORTABTEXT, BPI_PSZMINORTAB));
        let bitmap = field(BFA_MAJORTABBITMAP, BPI_HBMMAJORTAB).or_else(|| field(BFA_MINORTABBITMAP, BPI_HBMMINORTAB));
        let set = self.update_notebook(hwnd, |s| s.page_mut(id).map(|p| {
            if let Some(v) = data { p.data = v; }
            if let Some(v) = page { p.hwnd = v; }
            if let Some(v) = status { p.status = v; }
            if let Some(v) = tab { p.tab_text = v; }
            if let Some(v) = bitmap { p.bitmap = v; }
        })).flatten().is_some();
        if set { self.notebook_place_pages(hwnd); }
        set
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
            let (l, b, rr, _t) = at(r);
            let text = &s.pages[i].tab_text;
            let w = crate::gui::font::text_width(text, p.font());
            let tx = if style & BKS_TABTEXTCENTER != 0 { l + (rr - l + 1 - w) / 2 }
                     else if style & BKS_TABTEXTRIGHT != 0 { rr - TAB_TEXT_PAD - w }
                     else { l + TAB_TEXT_PAD };
            p.text(tx, b + (r.3 - 16) / 2, text, color);
        };
        for &(i, r) in &lay.major_tabs {
            let current = Some(i) == cur_major;
            let (l, b, rr, t) = at(r);
            p.fill((l, b, rr, t), if current { page_bg } else { s.colors[1].unwrap_or(TAB_COLOR) });
            outline(r, LIGHT_COLOR, EDGE_COLOR);
            tab_text(i, r, s.colors[3].unwrap_or(TEXT_COLOR));
        }
        for &(i, r) in &lay.minor_tabs {
            let current = Some(i) == cur_minor;
            let (l, b, rr, t) = at(r);
            p.fill((l, b, rr, t), if current { page_bg } else { s.colors[2].unwrap_or(TAB_COLOR) });
            outline(r, LIGHT_COLOR, EDGE_COLOR);
            tab_text(i, r, s.colors[4].unwrap_or(TEXT_COLOR));
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
        if focused && let Some(&(_, r)) = lay.major_tabs.iter().find(|(i, _)| Some(*i) == cur_major) {
            let (l, b, rr, _) = at(r);
            p.fill((l + 2, b + 2, rr - 2, b + 2), FOCUS_COLOR);
        }
        // The top page's window paints over the page again.
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if let Some(top) = s.page(s.top) && top.hwnd != 0 && wm.get_window(top.hwnd).is_some_and(|w| w.visible) {
            wm.invalidate_window_only(top.hwnd);
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
        assert_eq!(lay.major_tabs.len(), 2);
        let (mx, my, mcx, mcy) = lay.major_tabs[0].1;
        assert_eq!((mx + mcx, my + mcy), (400 - MARGIN, 300 - MARGIN));
        assert_eq!(lay.major_tabs[1].1.1, my - mcy, "stacked downwards");
        assert_eq!(lay.minor_tabs.len(), 1);
        assert_eq!(lay.minor_tabs[0].1.1, MARGIN);
        let (px, py, pcx, pcy) = lay.page;
        assert!(px + pcx <= mx && py > lay.minor_tabs[0].1.1 + lay.minor_tabs[0].1.3 && py + pcy <= 300);
        assert!(lay.next_button.unwrap().0 > lay.prev_button.unwrap().0);
        assert_eq!(lay.hit((mx + 1, my + 1)), Some(Hit::Tab(0)));
        let (bx, by, _, _) = lay.next_button.unwrap();
        assert_eq!(lay.hit((bx + 1, by + 1)), Some(Hit::Next));
        assert_eq!(lay.hit((px + 5, py + 5)), None);
        // A tabbed dialog: tabs in a row on top, no buttons or minor tabs.
        let lay = Layout::new(&s, BKS_TABBEDDIALOG, 400, 300, &w);
        assert!(lay.minor_tabs.is_empty() && lay.next_button.is_none() && lay.status.is_none());
        let (t0, t1) = (lay.major_tabs[0].1, lay.major_tabs[1].1);
        assert_eq!((t0.1, t1.1), (t1.1, 300 - MARGIN - t0.3));
        assert_eq!(t1.0, t0.0 + t0.2);
        assert_eq!(lay.page.1 + lay.page.3, t0.1 - 1);
    }
}
