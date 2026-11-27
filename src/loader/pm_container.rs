// SPDX-License-Identifier: GPL-3.0-or-later
//
//! `WC_CONTAINER`: records, field infos and the text, name, detail, tree
//! and icon views (IBM *PM Programming Reference*, "Container Controls";
//! messages, styles and structures from `<pmstddlg.h>`
//! `INCL_WINSTDCNR`).
//!
//! Records and field infos live in guest memory, carved from arenas each
//! container takes from the memory manager ([`RecordHeap`]); the guest's
//! copy is the only one — texts, positions and attributes are read when
//! needed, and only `flRecordAttr`, `ptlIcon` (`CM_ARRANGE`) and edited
//! texts are written back (ADR-0043, ADR-0046). The host keeps the
//! insertion order, each record's parent and the inserted field infos
//! ([`CnrState`]), and accepts only pointers it handed out.
//!
//! Views: `CV_TEXT` (`pszText`), `CV_NAME` (`pszName`), `CV_DETAIL` (one
//! column per visible field info, read at `offStruct` in each record:
//! `CFA_STRING`, `CFA_ULONG`, `CFA_DATE`, `CFA_TIME`, `CFA_BITMAPORICON`),
//! `CV_TREE` (`pszTree`, ADR-0044) and `CV_ICON` (icons at `ptlIcon`,
//! `pszIcon` under them), with `CV_FLOW` for the text and name views, the
//! container title (`CA_CONTAINERTITLE`) and detail column titles
//! (`CA_DETAILSVIEWTITLES`). A `MINIRECORDCORE` container shows `pszIcon`
//! in every view. Pictures (`hptrIcon` / `hbmBitmap` and the mini ones)
//! are drawn in the icon, name and tree views. `CM_FILTER` and direct
//! editing follow ADR-0046. The details view splits into two panes at
//! `pFieldInfoLast` / `xVertSplitbar` ([`detail_panes`]), each scrolling
//! across with its own scroll bar (`CID_HSCROLL`, `CID_RIGHTHSCROLL`), as
//! the icon view does; owners draw items (`CA_OWNERDRAW`, `CFA_OWNER`) by
//! owner calls; the title and column headings can be edited (ADR-0065).
//! Not drawn yet: tree lines.
//!
//! Selection follows `CCS_SINGLESEL` (default) / `CCS_MULTIPLESEL` /
//! `CCS_EXTENDSEL`; the owner gets `WM_CONTROL` `CN_EMPHASIS`
//! (`NOTIFYRECORDEMPHASIS`) and `CN_ENTER` (`NOTIFYRECORDENTER`) through
//! the owner-call queue, `CN_SETFOCUS` / `CN_KILLFOCUS` posted.
//! Unverified (no source): whether `CM_SETRECORDEMPHASIS` sends
//! `CN_EMPHASIS` (Warpine sends it), and whether a record must have all or
//! any of `CM_QUERYRECORDEMPHASIS`'s emphasis bits (Warpine: all).

use std::collections::{HashMap, HashSet};

use super::constants::*;
use super::pm_hook::{CallChain, CallStep, ChainEnd, ChainThen};
use super::pm_owner::{OwnerCall, OwnerCallKind};
use super::pm_types::WindowManager;
use super::{ApiResult, Loader, MutexExt, Painter};

pub const CLASS_CONTAINER: &str = "WC_CONTAINER";

pub const CM_ALLOCDETAILFIELDINFO: u32 = 0x0330;
pub const CM_ALLOCRECORD: u32 = 0x0331;
pub const CM_ARRANGE: u32 = 0x0332;
pub const CM_FILTER: u32 = 0x0334;
pub const CM_FREEDETAILFIELDINFO: u32 = 0x0335;
pub const CM_FREERECORD: u32 = 0x0336;
pub const CM_INSERTDETAILFIELDINFO: u32 = 0x0338;
pub const CM_INSERTRECORD: u32 = 0x0339;
pub const CM_INVALIDATEDETAILFIELDINFO: u32 = 0x033a;
pub const CM_INVALIDATERECORD: u32 = 0x033b;
pub const CM_QUERYCNRINFO: u32 = 0x033d;
pub const CM_QUERYDETAILFIELDINFO: u32 = 0x033e;
pub const CM_QUERYRECORD: u32 = 0x0340;
pub const CM_QUERYRECORDEMPHASIS: u32 = 0x0341;
pub const CM_QUERYRECORDFROMRECT: u32 = 0x0342;
pub const CM_QUERYRECORDRECT: u32 = 0x0343;
pub const CM_QUERYVIEWPORTRECT: u32 = 0x0344;
pub const CM_REMOVEDETAILFIELDINFO: u32 = 0x0345;
pub const CM_REMOVERECORD: u32 = 0x0346;
pub const CM_SCROLLWINDOW: u32 = 0x0347;
pub const CM_SEARCHSTRING: u32 = 0x0348;
pub const CM_SETCNRINFO: u32 = 0x0349;
pub const CM_SETRECORDEMPHASIS: u32 = 0x034a;
pub const CM_SORTRECORD: u32 = 0x034b;
pub const CM_OPENEDIT: u32 = 0x034c;
pub const CM_CLOSEEDIT: u32 = 0x034d;
pub const CM_COLLAPSETREE: u32 = 0x034e;
pub const CM_EXPANDTREE: u32 = 0x034f;
pub const CM_QUERYRECORDINFO: u32 = 0x0350;
pub const CM_INSERTRECORDARRAY: u32 = 0x0351;

pub const CN_DRAGAFTER: u32 = 101;
pub const CN_DRAGLEAVE: u32 = 102;
pub const CN_DRAGOVER: u32 = 103;
pub const CN_DROP: u32 = 104;
pub const CN_ENTER: u32 = 106;
pub const CN_INITDRAG: u32 = 107;
pub const CN_EMPHASIS: u32 = 108;
pub const CN_KILLFOCUS: u32 = 109;
pub const CN_SETFOCUS: u32 = 112;
pub const CN_REALLOCPSZ: u32 = 113;
pub const CN_BEGINEDIT: u32 = 114;
pub const CN_ENDEDIT: u32 = 115;
pub const CN_SCROLL: u32 = 110;
pub const CN_COLLAPSETREE: u32 = 116;
pub const CN_EXPANDTREE: u32 = 117;

pub const CCS_EXTENDSEL: u32 = 0x0000_0001;
pub const CCS_MULTIPLESEL: u32 = 0x0000_0002;
pub const CCS_AUTOPOSITION: u32 = 0x0000_0008;
pub const CCS_READONLY: u32 = 0x0000_0020;
pub const CCS_MINIRECORDCORE: u32 = 0x0000_0040;

pub const CV_TEXT: u32 = 0x0000_0001;
pub const CV_NAME: u32 = 0x0000_0002;
pub const CV_ICON: u32 = 0x0000_0004;
pub const CV_DETAIL: u32 = 0x0000_0008;
pub const CV_FLOW: u32 = 0x0000_0010;
pub const CV_MINI: u32 = 0x0000_0020;
pub const CV_TREE: u32 = 0x0000_0040;

pub const CA_CONTAINERTITLE: u32 = 0x0000_0200;
pub const CA_TITLESEPARATOR: u32 = 0x0000_0400;
pub const CA_TITLELEFT: u32 = 0x0000_0800;
pub const CA_TITLERIGHT: u32 = 0x0000_1000;
pub const CA_DETAILSVIEWTITLES: u32 = 0x0000_8000;
pub const CA_DRAWBITMAP: u32 = 0x0002_0000;
pub const CA_DRAWICON: u32 = 0x0004_0000;
pub const CA_OWNERDRAW: u32 = 0x0000_4000;
pub const CA_TITLEREADONLY: u32 = 0x0008_0000;
pub const CA_ORDEREDTARGETEMPH: u32 = 0x0001_0000;
pub const CA_MIXEDTARGETEMPH: u32 = 0x0020_0000;

pub const CFA_RIGHT: u32 = 0x0000_0002;
pub const CFA_CENTER: u32 = 0x0000_0004;
pub const CFA_INVISIBLE: u32 = 0x0000_0040;
pub const CFA_BITMAPORICON: u32 = 0x0000_0100;
pub const CFA_SEPARATOR: u32 = 0x0000_0200;
pub const CFA_HORZSEPARATOR: u32 = 0x0000_0400;
pub const CFA_STRING: u32 = 0x0000_0800;
pub const CFA_DATE: u32 = 0x0000_2000;
pub const CFA_TIME: u32 = 0x0000_4000;
pub const CFA_FIREADONLY: u32 = 0x0000_8000;
pub const CFA_OWNER: u32 = 0x0000_1000;
pub const CFA_FITITLEREADONLY: u32 = 0x0001_0000;
pub const CFA_ULONG: u32 = 0x0002_0000;

pub const CRA_SELECTED: u32 = 0x0000_0001;
pub const CRA_TARGET: u32 = 0x0000_0002;
pub const CRA_CURSORED: u32 = 0x0000_0004;
pub const CRA_FILTERED: u32 = 0x0000_0010;
pub const CRA_RECORDREADONLY: u32 = 0x0000_0040;
pub const CRA_DISABLED: u32 = 0x0000_1000;
pub const CRA_EXPANDED: u32 = 0x0000_0080;
pub const CRA_COLLAPSED: u32 = 0x0000_0100;

/// The id of a container's vertical scroll bar (`<pmstddlg.h>`).
pub const CID_VSCROLL: u32 = 0x7FF9;
/// The direct-editing MLE.
pub const CID_MLE: u32 = 0x7FFA;
/// `CNREDITDATA.id` of a details-view record (the left window when unsplit).
pub const CID_LEFTDVWND: u32 = 0x7FF7;
pub const CID_RIGHTDVWND: u32 = 0x7FF8;
pub const CID_LEFTCOLTITLEWND: u32 = 0x7FF0;
pub const CID_RIGHTCOLTITLEWND: u32 = 0x7FF1;
pub const CID_HSCROLL: u32 = 0x7FF3;
pub const CID_RIGHTHSCROLL: u32 = 0x7FF4;
pub const CID_CNRTITLEWND: u32 = 0x7FF5;

pub const CMA_PARENT: u16 = 0x0001;
pub const CMA_FIRSTCHILD: u16 = 0x0002;
pub const CMA_LASTCHILD: u16 = 0x0004;
pub const CMA_FIRST: u32 = 0x0010;
pub const CMA_LAST: u32 = 0x0020;
pub const CMA_END: u32 = 0x0040;
pub const CMA_PREV: u32 = 0x0080;
pub const CMA_NEXT: u32 = 0x0100;
pub const CMA_VERTICAL: u32 = 0x0400;
pub const CMA_HORIZONTAL: u32 = 0x0200;
pub const CMA_FILTER: u32 = 0x1000;
/// `CM_QUERYRECORDRECT` extents and `CM_QUERYRECORDFROMRECT` searches.
pub const CMA_ICON: u32 = 0x0800;
pub const CMA_TEXT: u32 = 0x1000;
pub const CMA_COMPLETE: u32 = 0x4000;
pub const CMA_ZORDER: u32 = 0x0008;
pub const CMA_WORKSPACE: u16 = 0x0004;
pub const CMA_TOP: u32 = 0x0001;
pub const CMA_BOTTOM: u32 = 0x0002;
pub const CMA_ARRANGEGRID: u32 = 0x0001;
pub const CMA_ARRANGESELECTED: u32 = 0x0002;
pub const CM_ERASERECORD: u32 = 0x0333;
pub const CM_MOVETREE: u32 = 0x0352;
pub const CM_SETGRIDINFO: u32 = 0x0354;
pub const CM_QUERYGRIDINFO: u32 = 0x0355;
pub const CN_HELP: u32 = 118;
pub const CN_CONTEXTMENU: u32 = 119;
pub const CA_TREELINE: u32 = 0x0040_0000;
use super::pm_input::WM_CONTEXTMENU;
/// `CM_ARRANGE`'s standard arrangement.
pub const CMA_ARRANGESTANDARD: u32 = 0;
pub const CMA_FREE: u16 = 0x0001;
pub const CMA_WINDOW: u16 = 0x0002;
/// `CM_SETCNRINFO` flags.
pub const CMA_CNRTITLE: u32 = 0x0001;
pub const CMA_DELTA: u32 = 0x0002;
pub const CMA_FLWINDOWATTR: u32 = 0x0004;
pub const CMA_LINESPACING: u32 = 0x0008;
pub const CMA_PFIELDINFOLAST: u32 = 0x0010;
pub const CMA_PSORTRECORD: u32 = 0x0020;
pub const CMA_PTLORIGIN: u32 = 0x0040;
pub const CMA_SLBITMAPORICON: u32 = 0x0080;
pub const CMA_XVERTSPLITBAR: u32 = 0x0100;
pub const CMA_PFIELDINFOOBJECT: u32 = 0x0200;
pub const CMA_TREEICON: u32 = 0x0400;
pub const CMA_TREEBITMAP: u32 = 0x0800;
pub const CMA_CXTREEINDENT: u32 = 0x1000;
pub const CMA_CXTREELINE: u32 = 0x2000;
pub const CMA_SLTREEBITMAPORICON: u32 = 0x4000;

pub const PMERR_RECORD_CURRENTLY_INSERTED: u32 = 0x1f04;
pub const PMERR_FI_CURRENTLY_INSERTED: u32 = 0x1f05;

/// `RECORDCORE` / `MINIRECORDCORE` field offsets and sizes.
pub const RC_ATTR: u32 = 4;
pub const RC_NEXT: u32 = 16;
pub const RC_PTLX: u32 = 8;
pub const RC_PTLY: u32 = 12;
pub const RC_PSZICON: u32 = 20;
pub const RC_HPTRICON: u32 = 24;
pub const RC_HPTRMINIICON: u32 = 28;
pub const RC_HBMBITMAP: u32 = 32;
pub const RC_HBMMINIBITMAP: u32 = 36;
/// `RECORDCORE.pTreeItemDesc` (not in a `MINIRECORDCORE`).
pub const RC_PTREEITEMDESC: u32 = 40;
pub const RC_PSZTEXT: u32 = 44;
pub const RC_PSZNAME: u32 = 48;
pub const RC_PSZTREE: u32 = 52;
pub const RECORDCORE_SIZE: u32 = 56;
pub const MINIRECORDCORE_SIZE: u32 = 28;
/// `FIELDINFO` field offsets and size.
pub const FI_FLDATA: u32 = 4;
pub const FI_FLTITLE: u32 = 8;
pub const FI_TITLE: u32 = 12;
pub const FI_OFFSTRUCT: u32 = 16;
pub const FI_NEXT: u32 = 24;
pub const FI_CXWIDTH: u32 = 28;
pub const FIELDINFO_SIZE: u32 = 32;
/// `CNRINFO` size (its fields are laid out in [`CnrInfo::words`]).
pub const CNRINFO_SIZE: u32 = 92;
pub const CNREDITDATA_SIZE: u32 = 28;
/// `RECORDINSERT` / `FIELDINFOINSERT` / `SEARCHSTRING` /
/// `QUERYRECORDRECT` field offsets.
const RI_ORDER: u32 = 4;
const RI_PARENT: u32 = 8;
const RI_INVALIDATE: u32 = 12;
const RI_ZORDER: u32 = 16;
const RI_COUNT: u32 = 20;
const FII_ORDER: u32 = 4;
const FII_INVALIDATE: u32 = 8;
const FII_COUNT: u32 = 12;
const SS_PSZ: u32 = 4;
const SS_PREFIX: u32 = 8;
const SS_CASE: u32 = 12;
const SS_VIEW: u32 = 16;
const QRR_RECORD: u32 = 4;
const QRR_EXTENT: u32 = 12;
const QRFR_RECT: u32 = 4;
const QRFR_SEARCH: u32 = 20;
const CED_RECORD: u32 = 8;
const CED_FIELD: u32 = 12;
const CED_ID: u32 = 24;

/// Bytes of guest memory an arena takes (ADR-0043).
pub const CNR_ARENA_BYTES: u32 = 64 * 1024;
/// Most records and field infos a container holds allocated.
pub const MAX_CNR_ITEMS: usize = 100_000;
/// Most application bytes after a record's core (`cbRecordData`).
pub const MAX_RECORD_DATA: u32 = 64 * 1024;
/// Most bytes one allocation takes.
const MAX_ALLOC_BYTES: u64 = 16 * 1024 * 1024;

const BORDER: i32 = 1;
const ROW_PAD: i32 = 1;
const CELL_PAD: i32 = 4;
const COLUMN_GAP: i32 = 12;
const LINE_H: i32 = 16;
const BORDER_COLOR: u32 = 0x0080_8080;
const FIELD_COLOR: u32 = 0x00FF_FFFF;
/// Target emphasis: its colour and the insertion line's width.
const TARGET_COLOR: u32 = 0x0000_0000;
const TARGET_LINE: i32 = 2;
const SELECT_COLOR: u32 = 0x0000_0080;
const SELECT_TEXT_COLOR: u32 = 0x00FF_FFFF;
const DISABLED_TEXT_COLOR: u32 = 0x0080_8080;
const SEPARATOR_COLOR: u32 = 0x0080_8080;
/// Breadth of the vertical scroll bar.
const SCROLLBAR_W: i32 = 16;
/// The split bar's width, the narrowest pane beside it, and how far a
/// horizontal scroll arrow moves (Warpine's own).
const SPLIT_W: i32 = 4;
const MIN_PANE: i32 = 16;
/// The widest details view, in pels: columns past it start there.
const MAX_DETAIL_WIDTH: i32 = 1 << 24;
const H_LINE: i32 = 16;
/// Tree indentation per level when `cxTreeIndent` is not set.
const TREE_INDENT: i32 = 16;
/// Side of a tree view's expander box.
const EXPANDER: i32 = 9;
/// The expander when it is a picture (`TREEITEMDESC`, `CNRINFO`'s
/// expanded / collapsed handles).
const TREE_PICTURE: i32 = 16;
/// The largest grid square (`CM_SETGRIDINFO`).
const MAX_GRID: i32 = 1024;
/// Icon sizes when `slBitmapOrIcon` is 0: `SV_CXICON` and the mini icon
/// (`CV_MINI`, and Warpine's tree-view default).
pub const ICON_SIZE: i32 = 32;
pub const MINI_ICON_SIZE: i32 = 16;
/// The largest icon size taken from `CNRINFO`.
const MAX_ICON_SIZE: i32 = 256;
/// The gap `CM_ARRANGE` leaves around icon-view items (Warpine's own).
const ARRANGE_GAP: i32 = 8;
/// The narrowest direct-editing MLE.
const EDIT_MIN_W: i32 = 80;

/// What an allocated item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind { Record, Field, Cell }

/// An allocated record or field info: its kind, size in bytes and arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item { pub kind: ItemKind, pub size: u32, arena: u32 }

#[derive(Debug, Clone, PartialEq, Eq)]
struct Arena { base: u32, size: u32, used: u32, live: u32 }

/// A container's records and field infos in guest memory: arenas taken
/// from the memory manager, items carved from the newest one, an arena
/// given back once all of its items are freed (ADR-0043).
#[derive(Debug, Clone, Default)]
pub struct RecordHeap {
    arenas: Vec<Arena>,
    items: HashMap<u32, Item>,
}

impl RecordHeap {
    /// Whether `n` items of `size` bytes fit in the newest arena.
    pub fn fits(&self, size: u32, n: u32) -> bool {
        let need = u64::from(size) * u64::from(n);
        self.arenas.last().is_some_and(|a| u64::from(a.size - a.used) >= need)
    }

    /// Whether `n` more items stay within [`MAX_CNR_ITEMS`].
    pub fn has_room(&self, n: u32) -> bool {
        self.items.len().saturating_add(n as usize) <= MAX_CNR_ITEMS
    }

    pub fn add_arena(&mut self, base: u32, size: u32) {
        self.arenas.push(Arena { base, size, used: 0, live: 0 });
    }

    /// Carve `n` items of `size` bytes (a multiple of 4) from the newest
    /// arena; their addresses, `None` when they do not fit.
    pub fn carve(&mut self, kind: ItemKind, size: u32, n: u32) -> Option<Vec<u32>> {
        if n == 0 || !self.fits(size, n) || !self.has_room(n) {
            return None;
        }
        let a = self.arenas.last_mut()?;
        let start = a.base + a.used;
        a.used += size * n;
        a.live += n;
        let arena = a.base;
        let addrs: Vec<u32> = (0..n).map(|i| start + i * size).collect();
        for &at in &addrs {
            self.items.insert(at, Item { kind, size, arena });
        }
        Some(addrs)
    }

    pub fn item(&self, addr: u32) -> Option<Item> {
        self.items.get(&addr).copied()
    }

    /// Free item `addr`: `None` when it is no item, else the base of its
    /// arena when that arena is now empty (to give back).
    pub fn free(&mut self, addr: u32) -> Option<Option<u32>> {
        let item = self.items.remove(&addr)?;
        let i = self.arenas.iter().position(|a| a.base == item.arena)?;
        self.arenas[i].live -= 1;
        if self.arenas[i].live == 0 {
            return Some(Some(self.arenas.remove(i).base));
        }
        Some(None)
    }

    /// Every arena's base, emptying the heap (the container is destroyed).
    pub fn take_all(&mut self) -> Vec<u32> {
        self.items.clear();
        self.arenas.drain(..).map(|a| a.base).collect()
    }

    /// Items allocated (for tests and bounds).
    pub fn len(&self) -> usize { self.items.len() }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
}

/// The container's `CNRINFO` as set by `CM_SETCNRINFO` (pointers are the
/// application's; the title is read when painted).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CnrInfo {
    pub sort_record: u32,
    pub field_last: u32,
    pub field_object: u32,
    pub title: u32,
    pub window_attr: u32,
    pub origin: (u32, u32),
    pub delta: u32,
    pub bitmap_size: (u32, u32),
    pub tree_size: (u32, u32),
    pub tree_bitmaps: (u32, u32),
    pub tree_icons: (u32, u32),
    pub line_spacing: u32,
    pub tree_indent: u32,
    pub tree_line: u32,
    pub split_bar: u32,
}

impl CnrInfo {
    /// The 23 ULONGs of a `CNRINFO` with `records` and `fields` counted.
    pub fn words(&self, records: u32, fields: u32) -> [u32; 23] {
        [CNRINFO_SIZE, self.sort_record, self.field_last, self.field_object, self.title, self.window_attr,
         self.origin.0, self.origin.1, self.delta, records, self.bitmap_size.0, self.bitmap_size.1,
         self.tree_size.0, self.tree_size.1, self.tree_bitmaps.0, self.tree_bitmaps.1,
         self.tree_icons.0, self.tree_icons.1, self.line_spacing, self.tree_indent, self.tree_line,
         fields, self.split_bar]
    }

    /// Take the fields `CM_SETCNRINFO`'s flags `fl` name from `w`.
    pub fn set_from(&mut self, w: &[u32; 23], fl: u32) {
        let take = |flag: u32| fl & flag != 0;
        if take(CMA_PSORTRECORD) { self.sort_record = w[1]; }
        if take(CMA_PFIELDINFOLAST) { self.field_last = w[2]; }
        if take(CMA_PFIELDINFOOBJECT) { self.field_object = w[3]; }
        if take(CMA_CNRTITLE) { self.title = w[4]; }
        if take(CMA_FLWINDOWATTR) { self.window_attr = w[5]; }
        if take(CMA_PTLORIGIN) { self.origin = (w[6], w[7]); }
        if take(CMA_DELTA) { self.delta = w[8]; }
        if take(CMA_SLBITMAPORICON) { self.bitmap_size = (w[10], w[11]); }
        if take(CMA_SLTREEBITMAPORICON) { self.tree_size = (w[12], w[13]); }
        if take(CMA_TREEBITMAP) { self.tree_bitmaps = (w[14], w[15]); }
        if take(CMA_TREEICON) { self.tree_icons = (w[16], w[17]); }
        if take(CMA_LINESPACING) { self.line_spacing = w[18]; }
        if take(CMA_CXTREEINDENT) { self.tree_indent = w[19]; }
        if take(CMA_CXTREELINE) { self.tree_line = w[20]; }
        if take(CMA_XVERTSPLITBAR) { self.split_bar = w[22]; }
    }
}

/// Why an insertion or removal failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CnrError {
    /// Not a record / field info of this container, or a bad order.
    Invalid,
    /// The record or field info is inserted already.
    Inserted,
}

/// A container: its heap, the inserted records in item order with their
/// parents (0 = top level), the inserted field infos, its `CNRINFO`, the
/// first row shown and the extended-selection anchor.
#[derive(Debug, Clone)]
pub struct CnrState {
    pub heap: RecordHeap,
    pub records: Vec<(u32, u32)>,
    inserted: HashSet<u32>,
    pub fields: Vec<u32>,
    pub info: CnrInfo,
    pub top: usize,
    pub anchor: Option<u32>,
    /// `CCS_MINIRECORDCORE`: records are `MINIRECORDCORE`s.
    pub mini: bool,
    /// A sort waiting for the application's comparisons.
    pub sort: Option<SortJob>,
    next_sort_id: u32,
    /// A filter waiting for the application's answers: its id and the
    /// records asked about, in order.
    pub filter: Option<(u32, Vec<u32>)>,
    /// The direct edit in progress, and the text of a closed one waiting
    /// for the owner's `CN_REALLOCPSZ` answer (encoded, without the NUL).
    pub edit: Option<EditSession>,
    pub edit_text: Option<Vec<u8>>,
    /// Details view: the pels each pane (left, right of the split bar)
    /// is scrolled to the left.
    pub hscroll: [i32; 2],
    /// The split bar is being dragged: where in it the pointer took it.
    pub split_drag: Option<i32>,
    /// A guest cell holding the container title's `PSZ` while it is
    /// edited (its `ppszText`), made at the first title edit.
    pub title_cell: u32,
    /// The icon view's stacking, bottom first (`RECORDINSERT.zOrder`).
    pub zorder: Vec<u32>,
    /// Records erased from the view (`CM_ERASERECORD`) until invalidated.
    pub erased: HashSet<u32>,
    /// The icon view's grid square (`CM_SETGRIDINFO`); `None`: Warpine's.
    pub grid: Option<(i32, i32)>,
    /// The target emphasis of a drag over the container whose owner
    /// accepts the drop.
    pub target: Option<TargetEmph>,
}

/// The target emphasis a container draws while a drag it accepts is over
/// it: around the whole container (no record under the pointer), around a
/// record, or a line after a record (`CN_DRAGAFTER`; `CMA_FIRST`: before
/// the first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetEmph { Whole, Record(u32), After(u32) }

/// Where a drop lands among rows shown top first (`(row index, bottom,
/// height)`, container coordinates) for a pointer at height `y`, with
/// ordered target emphasis: `Some(i)` on row `i` (only `mixed`, over the
/// middle half of a row), `Err(None)` before the first row, `Err(Some(i))`
/// after row `i`. The bands are Warpine's (unverified): a row's upper half
/// puts the drop before it, the lower half after it.
pub fn ordered_spot(rows: &[(usize, i32, i32)], y: i32, mixed: bool) -> Result<usize, Option<usize>> {
    let Some(&(first, first_b, first_h)) = rows.first() else { return Err(None) };
    if y >= first_b + first_h { return Err(first.checked_sub(1)); }
    for &(i, b, h) in rows {
        if y < b || y >= b + h { continue; }
        let quarter = h / 4;
        if mixed && y >= b + quarter && y < b + h - quarter { return Ok(i); }
        return Err(if y >= b + h / 2 { i.checked_sub(1) } else { Some(i) });
    }
    Err(rows.last().map(|r| r.0))
}

/// A direct edit (`CNREDITDATA`): the record (0 for the container title
/// and column headings), the detail field info (0 outside the details
/// view and for the title), where the text's `PSZ` is (`ppszText`) and
/// the window id the edit is in (`CID_CNRTITLEWND` for the title,
/// `CID_LEFTCOLTITLEWND` / `CID_RIGHTCOLTITLEWND` for a heading).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditSession { pub rec: u32, pub fi: u32, pub ppsz: u32, pub id: u32 }

impl EditSession {
    /// The `CNREDITDATA` for container `hwnd` with `cb_text` bytes.
    pub fn block(&self, hwnd: u32, cb_text: u32) -> [u32; 7] {
        [CNREDITDATA_SIZE, hwnd, self.rec, self.fi, self.ppsz, cb_text, self.id]
    }
}

impl CnrState {
    pub fn new(style: u32) -> Self {
        CnrState {
            heap: RecordHeap::default(), records: Vec::new(), inserted: HashSet::new(), fields: Vec::new(),
            info: CnrInfo { window_attr: CV_ICON, ..CnrInfo::default() },
            top: 0, anchor: None, mini: style & CCS_MINIRECORDCORE != 0, sort: None, next_sort_id: 0, filter: None, edit: None, edit_text: None,
            hscroll: [0; 2], split_drag: None, title_cell: 0, zorder: Vec::new(), erased: HashSet::new(), grid: None,
            target: None,
        }
    }

    pub fn core_size(&self) -> u32 {
        if self.mini { MINIRECORDCORE_SIZE } else { RECORDCORE_SIZE }
    }

    pub fn is_inserted(&self, rec: u32) -> bool { self.inserted.contains(&rec) }

    fn is_record(&self, rec: u32) -> bool {
        self.heap.item(rec).is_some_and(|i| i.kind == ItemKind::Record)
    }

    fn is_field(&self, fi: u32) -> bool {
        self.heap.item(fi).is_some_and(|i| i.kind == ItemKind::Field)
    }

    /// Insert `recs` under `parent` (0 = top level) at `order`: `CMA_FIRST`,
    /// `CMA_END` or after an inserted sibling. Returns the record count.
    pub fn insert(&mut self, recs: &[u32], order: u32, parent: u32) -> Result<u32, CnrError> {
        if recs.is_empty() || (parent != 0 && !self.is_inserted(parent)) {
            return Err(CnrError::Invalid);
        }
        let mut seen = HashSet::new();
        for &r in recs {
            if !self.is_record(r) || !seen.insert(r) { return Err(CnrError::Invalid); }
            if self.is_inserted(r) { return Err(CnrError::Inserted); }
        }
        let at = self.position(order, parent).ok_or(CnrError::Invalid)?;
        self.records.splice(at..at, recs.iter().map(|&r| (r, parent)));
        self.inserted.extend(recs.iter().copied());
        self.zorder.extend(recs.iter().copied());
        Ok(self.records.len() as u32)
    }

    /// Where records go in item order for `order` (`CMA_FIRST`, `CMA_END`,
    /// or after a child of `parent`) under `parent`.
    fn position(&self, order: u32, parent: u32) -> Option<usize> {
        match order {
            CMA_END => Some(self.records.len()),
            CMA_FIRST => Some(self.records.iter().position(|&(_, p)| p == parent)
                .or_else(|| self.records.iter().position(|&(r, _)| r == parent).map(|i| i + 1))
                .unwrap_or(self.records.len())),
            after => self.records.iter().position(|&(r, p)| r == after && p == parent).map(|i| i + 1),
        }
    }

    /// Put `recs` at the bottom of the icon view's stacking (`zOrder`
    /// `CMA_BOTTOM`); insertion puts them on top.
    pub fn lower(&mut self, recs: &[u32]) {
        self.zorder.retain(|r| !recs.contains(r));
        self.zorder.splice(0..0, recs.iter().copied());
    }

    /// `CM_MOVETREE`: move `rec` — with `siblings` also the siblings after
    /// it — and their descendants under `parent` (0: the top level) at
    /// `order` (`CMA_FIRST`, `CMA_END`, or after a child of `parent`),
    /// keeping their order. A record cannot move under itself or its
    /// descendants.
    pub fn move_tree(&mut self, rec: u32, parent: u32, order: u32, siblings: bool) -> Result<(), CnrError> {
        if !self.is_inserted(rec) || (parent != 0 && !self.is_inserted(parent)) { return Err(CnrError::Invalid); }
        let old_parent = self.records.iter().find(|&&(r, _)| r == rec).map(|&(_, p)| p).ok_or(CnrError::Invalid)?;
        let tops: Vec<u32> = if siblings {
            self.children(old_parent).skip_while(|&r| r != rec).collect()
        } else {
            vec![rec]
        };
        let moving: HashSet<u32> = self.removal(&tops)?.into_iter().collect();
        if moving.contains(&parent) || moving.contains(&order) { return Err(CnrError::Invalid); }
        let moved: Vec<(u32, u32)> = self.records.iter().filter(|(r, _)| moving.contains(r))
            .map(|&(r, p)| (r, if tops.contains(&r) { parent } else { p })).collect();
        let kept: Vec<(u32, u32)> = self.records.iter().filter(|(r, _)| !moving.contains(r)).copied().collect();
        let saved = std::mem::replace(&mut self.records, kept);
        let Some(at) = self.position(order, parent) else {
            self.records = saved;
            return Err(CnrError::Invalid);
        };
        self.records.splice(at..at, moved);
        Ok(())
    }

    /// The records `recs` and all their descendants, for removal; every
    /// one of `recs` must be inserted.
    pub fn removal(&self, recs: &[u32]) -> Result<Vec<u32>, CnrError> {
        if recs.iter().any(|r| !self.is_inserted(*r)) {
            return Err(CnrError::Invalid);
        }
        let mut out: Vec<u32> = recs.to_vec();
        let mut i = 0;
        while i < out.len() {
            let p = out[i];
            out.extend(self.records.iter().filter(|&&(_, q)| q == p).map(|&(r, _)| r));
            i += 1;
        }
        let mut seen = HashSet::new();
        out.retain(|r| seen.insert(*r));
        Ok(out)
    }

    /// Take `recs` out of the container (not freeing them).
    pub fn remove(&mut self, recs: &[u32]) {
        let gone: HashSet<u32> = recs.iter().copied().collect();
        self.records.retain(|(r, _)| !gone.contains(r));
        self.inserted.retain(|r| !gone.contains(r));
        self.zorder.retain(|r| !gone.contains(r));
        self.erased.retain(|r| !gone.contains(r));
        if self.anchor.is_some_and(|a| gone.contains(&a)) { self.anchor = None; }
    }

    fn children(&self, parent: u32) -> impl Iterator<Item = u32> + '_ {
        self.records.iter().filter(move |&&(_, p)| p == parent).map(|&(r, _)| r)
    }

    /// `CM_QUERYRECORD`: the record `cmd` names relative to `reference`;
    /// `Ok(0)` for none, `Err` for a bad request.
    pub fn query(&self, reference: u32, cmd: u32) -> Result<u32, ()> {
        let parent_of = |r: u32| self.records.iter().find(|&&(x, _)| x == r).map(|&(_, p)| p);
        match cmd {
            CMA_FIRST => Ok(self.children(0).next().unwrap_or(0)),
            CMA_LAST => Ok(self.children(0).last().unwrap_or(0)),
            CMA_NEXT | CMA_PREV => {
                let p = parent_of(reference).ok_or(())?;
                let sibs: Vec<u32> = self.children(p).collect();
                let i = sibs.iter().position(|&r| r == reference).ok_or(())?;
                let j = if cmd == CMA_NEXT { i.checked_add(1) } else { i.checked_sub(1) };
                Ok(j.and_then(|j| sibs.get(j)).copied().unwrap_or(0))
            }
            c if c == u32::from(CMA_PARENT) => parent_of(reference).ok_or(()),
            c if c == u32::from(CMA_FIRSTCHILD) || c == u32::from(CMA_LASTCHILD) => {
                if !self.is_inserted(reference) { return Err(()); }
                let mut kids = self.children(reference);
                Ok(if c == u32::from(CMA_FIRSTCHILD) { kids.next() } else { kids.last() }.unwrap_or(0))
            }
            _ => Err(()),
        }
    }

    /// `CM_QUERYRECORD` with `CMA_FILTER` in `cmd`: the records `shown`
    /// rejects (filtered) are passed over in the direction of the request.
    pub fn query_shown(&self, reference: u32, cmd: u32, shown: &dyn Fn(u32) -> bool) -> Result<u32, ()> {
        let base = cmd & !CMA_FILTER;
        let mut r = self.query(reference, base)?;
        if cmd & CMA_FILTER == 0 { return Ok(r); }
        let step = match base {
            CMA_FIRST | CMA_NEXT => CMA_NEXT,
            CMA_LAST | CMA_PREV => CMA_PREV,
            c if c == u32::from(CMA_FIRSTCHILD) => CMA_NEXT,
            c if c == u32::from(CMA_LASTCHILD) => CMA_PREV,
            _ => return Ok(r),
        };
        let mut left = self.records.len();
        while r != 0 && !shown(r) && left > 0 {
            r = self.query(r, step)?;
            left -= 1;
        }
        Ok(r)
    }

    /// The inserted records in item order, for a filter.
    pub fn all_records(&self) -> Vec<u32> {
        self.records.iter().map(|&(r, _)| r).collect()
    }

    /// Insert field infos `fis` at `order` (`CMA_FIRST`, `CMA_END` or after
    /// an inserted one). Returns the field count.
    pub fn insert_fields(&mut self, fis: &[u32], order: u32) -> Result<u32, CnrError> {
        let mut seen = HashSet::new();
        if fis.is_empty() { return Err(CnrError::Invalid); }
        for &f in fis {
            if !self.is_field(f) || !seen.insert(f) { return Err(CnrError::Invalid); }
            if self.fields.contains(&f) { return Err(CnrError::Inserted); }
        }
        let at = match order {
            CMA_FIRST => 0,
            CMA_END => self.fields.len(),
            after => self.fields.iter().position(|&f| f == after).ok_or(CnrError::Invalid)? + 1,
        };
        self.fields.splice(at..at, fis.iter().copied());
        Ok(self.fields.len() as u32)
    }

    /// The inserted records grouped by parent, each group in item order
    /// (`only`: just the children of that parent).
    pub fn sibling_groups(&self, only: Option<u32>) -> Vec<(u32, Vec<u32>)> {
        let mut index: HashMap<u32, usize> = HashMap::new();
        let mut out: Vec<(u32, Vec<u32>)> = Vec::new();
        for &(r, p) in self.records.iter().filter(|&&(_, p)| only.is_none_or(|o| o == p)) {
            let i = *index.entry(p).or_insert_with(|| { out.push((p, Vec::new())); out.len() - 1 });
            out[i].1.push(r);
        }
        out
    }

    /// Put each group of `groups` (from [`Self::sibling_groups`], sorted)
    /// into the places its records hold; FALSE (nothing changed) when a
    /// group no longer holds the same records.
    pub fn apply_sorted(&mut self, groups: &[(u32, Vec<u32>)]) -> bool {
        let mut slots: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, &(_, p)) in self.records.iter().enumerate() {
            slots.entry(p).or_default().push(i);
        }
        for (p, g) in groups {
            let mut have: Vec<u32> = slots.get(p).map_or(Vec::new(), |v| v.iter().map(|&i| self.records[i].0).collect());
            let mut want = g.clone();
            have.sort_unstable();
            want.sort_unstable();
            if have != want { return false; }
        }
        for (p, g) in groups {
            for (k, &i) in slots.get(p).map_or(&[][..], |v| v.as_slice()).iter().enumerate() {
                self.records[i].0 = g[k];
            }
        }
        true
    }

    pub(crate) fn new_sort_id(&mut self) -> u32 {
        self.next_sort_id = self.next_sort_id.wrapping_add(1);
        self.next_sort_id
    }

    /// `CM_QUERYDETAILFIELDINFO`.
    pub fn query_field(&self, reference: u32, cmd: u32) -> Result<u32, ()> {
        match cmd {
            CMA_FIRST => Ok(self.fields.first().copied().unwrap_or(0)),
            CMA_LAST => Ok(self.fields.last().copied().unwrap_or(0)),
            CMA_NEXT | CMA_PREV => {
                let i = self.fields.iter().position(|&f| f == reference).ok_or(())?;
                let j = if cmd == CMA_NEXT { i.checked_add(1) } else { i.checked_sub(1) };
                Ok(j.and_then(|j| self.fields.get(j)).copied().unwrap_or(0))
            }
            _ => Err(()),
        }
    }
}

/// The records a tree view shows, depth first: each with its depth and
/// whether it has children. Children are shown under `expanded` records
/// only; a record that is not `shown` (filtered) is left out with its
/// subtree.
/// Tree lines (`CA_TREELINE`) of rows at `depths` (tree order): for each
/// row, the levels whose vertical line runs through it (bit `k`: the line
/// at level `k`, that of an ancestor at depth `k + 1` with a later
/// sibling), and whether the row has a later sibling (its own line goes
/// on below it). Depths past 63 count as 63.
pub fn tree_lines(depths: &[u16]) -> Vec<(u64, bool)> {
    let n = depths.len();
    let mut has_next = vec![false; n];
    let mut seen = [false; 64];
    for i in (0..n).rev() {
        let d = usize::from(depths[i].min(63));
        has_next[i] = seen[d];
        seen[d] = true;
        for s in seen.iter_mut().skip(d + 1) { *s = false; }
    }
    let mut path = [false; 64];
    depths.iter().enumerate().map(|(i, &d)| {
        let d = usize::from(d.min(63));
        path[d] = has_next[i];
        let mask = (1..d).filter(|&k| path[k]).fold(0u64, |m, k| m | 1 << (k - 1));
        (mask, has_next[i])
    }).collect()
}

/// Warpine's grid square for icons of `(iw, ih)`: room for the icon and
/// two lines of text, at least 64 pels across, with the arrangement gap.
pub fn default_grid((iw, ih): (i32, i32)) -> (i32, i32) {
    (iw.max(64) + ARRANGE_GAP, ih + icon_text_h(2) + ARRANGE_GAP)
}

/// Grid arrangement (`CMA_ARRANGEGRID`): the `ptlIcon` of `n` items of
/// icon size `(iw, ih)` in squares `cx` × `cy`, row by row from the upper
/// left of a `width` × `height` work area, each icon centred across its
/// square and at the square's top.
pub fn arrange_grid(n: usize, (iw, ih): (i32, i32), (cx, cy): (i32, i32), width: i32, height: i32) -> Vec<(i32, i32)> {
    let cols = (width / cx.max(1)).max(1) as usize;
    (0..n).map(|k| {
        let (c, r) = ((k % cols) as i32, (k / cols) as i32);
        (c * cx + (cx - iw) / 2, height - r * cy - ARRANGE_GAP / 2 - ih)
    }).collect()
}

pub fn tree_order(records: &[(u32, u32)], expanded: &dyn Fn(u32) -> bool, shown: &dyn Fn(u32) -> bool)
                  -> Vec<(u32, u16, bool)> {
    let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(r, p) in records { kids.entry(p).or_default().push(r); }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(u32, u16)> = kids.get(&0).map_or(Vec::new(), |v| v.iter().rev().map(|&r| (r, 0)).collect());
    while let Some((r, d)) = stack.pop() {
        if !seen.insert(r) || !shown(r) { continue; }
        let children = kids.get(&r).filter(|v| !v.is_empty());
        out.push((r, d, children.is_some()));
        if let Some(c) = children && expanded(r) {
            stack.extend(c.iter().rev().map(|&c| (c, d.saturating_add(1))));
        }
    }
    out
}

/// A stable merge sort of sibling groups made one comparison at a time:
/// the container asks the application's comparison function about each
/// pair (one call chain per call, ADR-0026) and feeds the answer back.
#[derive(Debug, Clone)]
pub struct SortJob {
    pub id: u32,
    /// The comparison function and its `pStorage`.
    pub pfn: u32,
    pub storage: u32,
    /// Groups still to sort (last first) and groups sorted.
    todo: Vec<(u32, Vec<u32>)>,
    done: Vec<(u32, Vec<u32>)>,
    parent: u32,
    a: Vec<u32>,
    buf: Vec<u32>,
    width: usize,
    lo: usize,
    i: usize,
    j: usize,
    pending: bool,
}

impl SortJob {
    pub fn new(id: u32, pfn: u32, storage: u32, mut groups: Vec<(u32, Vec<u32>)>) -> Self {
        groups.reverse();
        let mut job = SortJob { id, pfn, storage, todo: groups, done: Vec::new(), parent: 0, a: Vec::new(),
                                buf: Vec::new(), width: 1, lo: 0, i: 0, j: 0, pending: false };
        job.next_group();
        job
    }

    /// Take the next group with two records or more (shorter ones are
    /// sorted already); FALSE when none is left.
    fn next_group(&mut self) -> bool {
        while let Some((p, g)) = self.todo.pop() {
            if g.len() < 2 {
                self.done.push((p, g));
                continue;
            }
            self.parent = p;
            self.a = g;
            self.buf.clear();
            self.width = 1;
            self.start_merge(0);
            return true;
        }
        false
    }

    fn start_merge(&mut self, lo: usize) {
        self.lo = lo;
        self.i = lo;
        self.j = (lo + self.width).min(self.a.len());
    }

    /// The next pair to compare, after `answer` (the comparison of the
    /// pair asked last: negative or 0 keeps the first one first; `None` at
    /// the start). `None` when the sort is done.
    pub fn step(&mut self, answer: Option<i16>) -> Option<(u32, u32)> {
        if let Some(ans) = answer && self.pending {
            self.pending = false;
            if ans <= 0 {
                self.buf.push(self.a[self.i]);
                self.i += 1;
            } else {
                self.buf.push(self.a[self.j]);
                self.j += 1;
            }
        }
        loop {
            if self.a.is_empty() { return None; }
            let n = self.a.len();
            let (mid, hi) = ((self.lo + self.width).min(n), (self.lo + 2 * self.width).min(n));
            if self.i < mid && self.j < hi {
                self.pending = true;
                return Some((self.a[self.i], self.a[self.j]));
            }
            self.buf.extend_from_slice(&self.a[self.i..mid]);
            self.buf.extend_from_slice(&self.a[self.j..hi]);
            if hi < n {
                self.start_merge(hi);
                continue;
            }
            // The pass is over.
            self.a = std::mem::take(&mut self.buf);
            self.width *= 2;
            if self.width >= n {
                let g = std::mem::take(&mut self.a);
                self.done.push((self.parent, g));
                if !self.next_group() { return None; }
            } else {
                self.start_merge(0);
            }
        }
    }

    /// The sorted groups (once [`Self::step`] returned `None`).
    pub fn into_groups(self) -> Vec<(u32, Vec<u32>)> {
        self.done
    }
}

/// The chain asking sort `job`'s comparison function `pfn` about `(a, b)`:
/// `SHORT pfnCompare(PRECORDCORE p1, PRECORDCORE p2, PVOID pStorage)`.
fn sort_chain(hwnd: u32, job: u32, pfn: u32, storage: u32, (a, b): (u32, u32), ret: u32, keep: bool) -> CallChain {
    CallChain::new(super::interrupt::current_vcpu().unwrap_or(0), vec![CallStep::hook(pfn, vec![a, b, storage], false)], false,
                   ChainThen::ContainerSort { hwnd, job, ret, keep })
}

/// The selection mode of container style `style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelMode { Single, Multiple, Extended }

pub fn sel_mode(style: u32) -> SelMode {
    if style & CCS_EXTENDSEL != 0 { SelMode::Extended }
    else if style & CCS_MULTIPLESEL != 0 { SelMode::Multiple }
    else { SelMode::Single }
}

/// The views Warpine draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View { Text, Name, Icon, Tree, Detail }

pub fn view_of(attr: u32) -> View {
    if attr & CV_DETAIL != 0 { View::Detail }
    else if attr & CV_TREE != 0 { View::Tree }
    else if attr & CV_TEXT != 0 { View::Text }
    else if attr & CV_NAME != 0 { View::Name }
    else { View::Icon }
}

/// The offset of the text a record shows in `view` (`pszIcon` for every
/// view of a `MINIRECORDCORE`).
pub fn text_offset(view: View, mini: bool) -> u32 {
    if mini { return RC_PSZICON; }
    match view {
        View::Text => RC_PSZTEXT,
        View::Name => RC_PSZNAME,
        View::Tree => RC_PSZTREE,
        View::Icon | View::Detail => RC_PSZICON,
    }
}

/// Where a view's parts are, in window coordinates: the title band, the
/// detail header band and the rows' area (`(x, y, cx, cy)` each), and the
/// row height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub title: Option<(i32, i32, i32, i32)>,
    pub header: Option<(i32, i32, i32, i32)>,
    pub rows: (i32, i32, i32, i32),
    pub row_h: i32,
}

impl Frame {
    /// `item_h`: the height of a row's picture (0 = text only).
    pub fn new(attr: u32, line_spacing: i32, item_h: i32, cx: i32, cy: i32) -> Frame {
        let row_h = LINE_H.max(item_h.clamp(0, MAX_ICON_SIZE)) + 2 * ROW_PAD + line_spacing.clamp(0, 64);
        let band = LINE_H + 2 * ROW_PAD;
        let (x, w) = (BORDER, (cx - 2 * BORDER).max(0));
        let mut top = cy - BORDER;
        let mut take = |h: i32| { top -= h; Some((x, top, w, h)) };
        let title = if attr & CA_CONTAINERTITLE != 0 {
            take(band + if attr & CA_TITLESEPARATOR != 0 { 1 } else { 0 })
        } else { None };
        let header = if view_of(attr) == View::Detail && attr & CA_DETAILSVIEWTITLES != 0 {
            take(band + 1)
        } else { None };
        let h = (top - BORDER).max(0);
        Frame { title, header, rows: (x, BORDER, w, h), row_h }
    }

    /// Rows that fit in the rows' area (at least one).
    pub fn capacity(&self) -> usize {
        (self.rows.3 / self.row_h).max(1) as usize
    }
}

/// A rectangle `(x, y, cx, cy)` in window coordinates.
pub type Rect = (i32, i32, i32, i32);

/// The rectangle around `a` and `b`.
pub fn union(a: Rect, b: Rect) -> Rect {
    let (l, bt) = (a.0.min(b.0), a.1.min(b.1));
    let (r, t) = ((a.0 + a.2).max(b.0 + b.2), (a.1 + a.3).max(b.1 + b.3));
    (l, bt, r - l, t - bt)
}
/// Shown items: `(index, rectangle)`.
pub type Placed = Vec<(usize, Rect)>;

/// Place `widths.len()` items of a list view starting at item `top`:
/// one column, or with `flow` columns of `capacity` rows left to right,
/// each as wide as its widest item. Returns `(index, (x, y, cx, cy))` for
/// every item wholly or partly shown.
pub fn place_list(f: &Frame, widths: &[i32], top: usize, flow: bool) -> Placed {
    let (rx, ry, rw, rh) = f.rows;
    let cap = f.capacity();
    let row_y = |k: usize| ry + rh - (k as i32 + 1) * f.row_h;
    let mut out = Vec::new();
    if !flow {
        for (k, i) in (top..widths.len()).enumerate() {
            if row_y(k) + f.row_h <= ry { break; }
            out.push((i, (rx, row_y(k), rw, f.row_h)));
        }
        return out;
    }
    let mut x = rx;
    let mut i = top;
    while i < widths.len() && x < rx + rw {
        let end = (i + cap).min(widths.len());
        let col_w = widths[i..end].iter().copied().max().unwrap_or(0) + 2 * CELL_PAD;
        for (k, j) in (i..end).enumerate() {
            out.push((j, (x, row_y(k), col_w, f.row_h)));
        }
        x += col_w + COLUMN_GAP;
        i = end;
    }
    out
}

/// The first item to show so that item `cursor` is visible, given the
/// current first item `top` (flowed views move by whole columns).
pub fn scroll_to(top: usize, cursor: usize, cap: usize, flow: bool, shown: &[(usize, Rect)],
                 rows_right: i32) -> usize {
    if flow {
        let col = |i: usize| i / cap * cap;
        if cursor < top { return col(cursor); }
        let fully = shown.iter().any(|&(i, (x, _, w, _))| i == cursor && x + w <= rows_right);
        if fully { return top; }
        return col(cursor);
    }
    if cursor < top { cursor } else if cursor >= top + cap { cursor + 1 - cap } else { top }
}

/// A `CDATE` (`day`, `month`, `year`) in the country's order and separator
/// (`fsDateFmt` 0 = MDY, 1 = DMY, 2 = YMD).
pub fn format_date(day: u8, month: u8, year: u16, fmt: u32, sep: char) -> String {
    match fmt {
        1 => format!("{day:02}{sep}{month:02}{sep}{year:04}"),
        2 => format!("{year:04}{sep}{month:02}{sep}{day:02}"),
        _ => format!("{month:02}{sep}{day:02}{sep}{year:04}"),
    }
}

/// A `CTIME` in the country's clock (`fsTimeFmt` 0 = 12-hour) and separator.
pub fn format_time(h: u8, m: u8, s: u8, fmt: u8, sep: char) -> String {
    if fmt == 0 {
        let (h12, ap) = match h { 0 => (12, "am"), 1..=11 => (h, "am"), 12 => (12, "pm"), _ => (h - 12, "pm") };
        format!("{h12:2}{sep}{m:02}{sep}{s:02}{ap}")
    } else {
        format!("{h:02}{sep}{m:02}{sep}{s:02}")
    }
}

/// The size records' pictures are drawn at in view attributes `attr`
/// (`CNRINFO.slBitmapOrIcon`, `slTreeBitmapOrIcon` in the tree view; 0 =
/// the system icon size, or the mini icon size with `CV_MINI` — ignored
/// for `MINIRECORDCORE`s — and in the tree view).
pub fn icon_size(info: &CnrInfo, mini_records: bool) -> (i32, i32) {
    let attr = info.window_attr;
    let (sz, default) = if view_of(attr) == View::Tree {
        (info.tree_size, MINI_ICON_SIZE)
    } else if attr & CV_MINI != 0 && !mini_records {
        (info.bitmap_size, MINI_ICON_SIZE)
    } else {
        (info.bitmap_size, ICON_SIZE)
    };
    let pick = |v: u32| if v == 0 { default } else { (v.min(MAX_ICON_SIZE as u32) as i32).max(1) };
    (pick(sz.0), pick(sz.1))
}

/// Whether view attributes `attr` show pictures: the icon and name views,
/// and the tree view unless it is the tree text view (`CV_TREE | CV_TEXT`).
pub fn shows_icons(attr: u32) -> bool {
    match view_of(attr) {
        View::Icon | View::Name => true,
        View::Tree => attr & CV_TEXT == 0,
        View::Text | View::Detail => false,
    }
}

/// Which handle a record's picture is (`hptrIcon` & co.), and whether it is
/// a bitmap: `CA_DRAWBITMAP` without `CA_DRAWICON` draws bitmaps, `CV_MINI`
/// the mini ones (falling back to the normal one when 0, Warpine's
/// choice); a `MINIRECORDCORE` has only `hptrIcon`. Returns the offsets
/// to try, in order.
pub fn picture_fields(attr: u32, mini_records: bool) -> (&'static [u32], bool) {
    if mini_records { return (&[RC_HPTRICON], false); }
    let bitmap = attr & CA_DRAWBITMAP != 0 && attr & CA_DRAWICON == 0;
    let small = attr & CV_MINI != 0 && view_of(attr) != View::Tree;
    match (bitmap, small) {
        (false, false) => (&[RC_HPTRICON], false),
        (false, true) => (&[RC_HPTRMINIICON, RC_HPTRICON], false),
        (true, false) => (&[RC_HBMBITMAP], true),
        (true, true) => (&[RC_HBMMINIBITMAP, RC_HBMBITMAP], true),
    }
}

/// The height of an icon-view text of `lines` lines.
pub fn icon_text_h(lines: usize) -> i32 {
    lines.clamp(1, 64) as i32 * LINE_H + 2 * ROW_PAD
}

/// An icon-view item in window coordinates: the icon at `ptlIcon` (its
/// lower-left corner; the reference does not say which point it is)
/// moved by the work area's origin, the text centred under it.
pub fn icon_item(ptl: (i32, i32), origin: (i32, i32), rows: (i32, i32), (iw, ih): (i32, i32),
                 text_w: i32, lines: usize) -> (Rect, Rect) {
    let x = rows.0.saturating_add(ptl.0.saturating_sub(origin.0));
    let y = rows.1.saturating_add(ptl.1.saturating_sub(origin.1));
    let tw = text_w.saturating_add(2 * CELL_PAD);
    let th = icon_text_h(lines);
    let tx = x.saturating_add(iw / 2).saturating_sub(tw / 2);
    ((x, y, iw, ih), (tx, y.saturating_sub(th), tw, th))
}

/// `CM_ARRANGE`: the `ptlIcon` of each item (`(text width, lines)` each)
/// placed in rows from the upper left of a `width` × `height` work area,
/// left to right, a new row when the next one does not fit (spacing:
/// `ARRANGE_GAP`, Warpine's own; the reference only says CUA spacing).
pub fn arrange_icons(items: &[(i32, usize)], (iw, ih): (i32, i32), width: i32, height: i32, spacing: i32)
                     -> Vec<(i32, i32)> {
    let mut out = Vec::with_capacity(items.len());
    let (mut x, mut top, mut row_h) = (ARRANGE_GAP, height - ARRANGE_GAP, 0);
    for &(tw, lines) in items {
        let w = iw.max(tw.saturating_add(2 * CELL_PAD)).min(width.max(iw));
        let h = ih + icon_text_h(lines);
        if x > ARRANGE_GAP && x.saturating_add(w) > width - ARRANGE_GAP {
            top = top.saturating_sub(row_h + ARRANGE_GAP + spacing.clamp(0, 64));
            x = ARRANGE_GAP;
            row_h = 0;
        }
        out.push((x + (w - iw) / 2, top - ih));
        row_h = row_h.max(h);
        x = x.saturating_add(w + ARRANGE_GAP);
    }
    out
}

/// The icon view's vertical scrolling: items spanning workspace heights
/// `items` (bottom, top) and a work area `h` high. Returns the
/// workspace's `(bottom, top)`: the items' span and the work area at the
/// workspace origin together.
pub fn icon_extent(items: Option<(i32, i32)>, h: i32) -> (i32, i32) {
    let (lo, hi) = items.unwrap_or((0, h));
    (lo.min(0), hi.max(h))
}

/// A detail-view column: its field info, flags, left edge, width, the
/// room it takes (its width and separator) and its pane (0 left, 1 right
/// of the split bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Column { fi: u32, fl_data: u32, fl_title: u32, x: i32, w: i32, adv: i32, pane: usize }

/// A details-view pane: its left edge and width in window coordinates,
/// the columns it holds (`first..end` of the visible columns), their room
/// together, and how many pels it is scrolled to the left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pane { pub x: i32, pub w: i32, pub first: usize, pub end: usize, pub content: i32, pub off: i32 }

/// Where the split bar starts, from the left edge of rows `w` wide:
/// `split_bar` (`CNRINFO.xVertSplitbar`; half the width when not above 0,
/// Warpine's choice), kept so that both panes are `MIN_PANE` wide.
pub fn split_x(split_bar: i32, w: i32) -> i32 {
    let want = if split_bar <= 0 { w / 2 } else { split_bar };
    let lo = MIN_PANE.min(w.max(0));
    want.clamp(lo, (w - SPLIT_W - MIN_PANE).max(lo))
}

/// The panes of a details view whose rows lie at `x`, `w` pels wide, for
/// columns taking `advances` pels each: one pane, or with `left` columns
/// left of the split bar (at `split_bar`, see [`split_x`]) two. `offs`
/// are the panes' scroll positions, kept within their content.
pub fn detail_panes(advances: &[i32], left: Option<usize>, split_bar: i32, (x, w): (i32, i32), offs: [i32; 2]) -> Vec<Pane> {
    let sum = |a: usize, b: usize| advances[a..b].iter().fold(0i32, |t, &v| t.saturating_add(v)).min(MAX_DETAIL_WIDTH);
    let pane = |px: i32, pw: i32, first: usize, end: usize, off: i32| {
        let (content, pw) = (sum(first, end), pw.max(0));
        Pane { x: px, w: pw, first, end, content, off: off.clamp(0, content.saturating_sub(pw).max(0)) }
    };
    match left.filter(|&k| k > 0 && k < advances.len()) {
        None => vec![pane(x, w, 0, advances.len(), offs[0])],
        Some(k) => {
            let sx = split_x(split_bar, w);
            vec![pane(x, sx, 0, k, offs[0]), pane(x + sx + SPLIT_W, w - sx - SPLIT_W, k, advances.len(), offs[1])]
        }
    }
}

/// What a paint or hit test needs, taken under the lock.
struct Snapshot {
    cx: i32,
    cy: i32,
    info: CnrInfo,
    mini: bool,
    top: usize,
    /// Shown records with their sizes: the top-level, unfiltered ones, or
    /// in the tree view the expanded tree depth first.
    rows: Vec<(u32, u32)>,
    /// Each row's depth and whether it has children (tree view).
    tree: Vec<(u16, bool)>,
    fields: Vec<(u32, u32)>,
    /// Icon view: each row's `ptlIcon` and text lines, and the workspace
    /// heights the items span.
    icons: Vec<((i32, i32), Vec<String>)>,
    span: Option<(i32, i32)>,
    /// Icon view: the workspace widths the items span.
    xspan: Option<(i32, i32)>,
    /// Details view: the visible columns (`x` from the first column's
    /// left edge), how many lie left of the split bar, each pane's scroll.
    cols: Vec<Column>,
    left_cols: Option<usize>,
    hscroll: [i32; 2],
    /// Icon view: the rows in drawing order, bottom first (the stacking).
    z: Vec<usize>,
    /// Tree view: each row's expander picture for its state (handle, a
    /// bitmap?), from its `TREEITEMDESC` or `CNRINFO`.
    tree_pics: Vec<Option<(u32, bool)>>,
    /// Records erased from the view (`CM_ERASERECORD`).
    erased: HashSet<u32>,
}

impl Snapshot {
    fn view(&self) -> View { view_of(self.info.window_attr) }

    fn flow(&self) -> bool {
        self.info.window_attr & CV_FLOW != 0 && matches!(self.view(), View::Text | View::Name)
    }

    /// Whether rows `h` pels high need the vertical scroll bar (not in
    /// flowed views; in the icon view when the workspace is taller).
    fn needs_v(&self, h: i32) -> bool {
        if self.view() == View::Icon {
            let (lo, hi) = icon_extent(self.span, h);
            return hi - lo > h;
        }
        !self.flow() && self.rows.len() > (h / self.plain_frame().row_h).max(1) as usize
    }

    /// Whether rows `w` pels wide need horizontal scroll bars: in the
    /// details view when a pane's columns are wider than it, in the icon
    /// view when the workspace is wider than the work area.
    fn needs_h(&self, w: i32) -> bool {
        match self.view() {
            View::Icon => { let (lo, hi) = icon_extent(self.xspan, w); hi - lo > w }
            View::Detail => self.panes_for((BORDER, w)).iter().any(|p| p.content > p.w),
            _ => false,
        }
    }

    /// The scroll bars shown: (vertical, horizontal). A horizontal bar
    /// takes height from the rows, which may then need the vertical one.
    fn bars(&self) -> (bool, bool) {
        let f = self.plain_frame();
        let v0 = self.needs_v(f.rows.3);
        let h = self.needs_h(f.rows.2 - if v0 { SCROLLBAR_W } else { 0 });
        (self.needs_v(f.rows.3 - if h { SCROLLBAR_W } else { 0 }), h)
    }

    /// Whether the rows need the vertical scroll bar.
    fn bar(&self) -> bool { self.bars().0 }

    /// Icon view: the workspace's span (see [`icon_extent`]).
    fn extent(&self) -> (i32, i32) {
        icon_extent(self.span, self.frame().rows.3)
    }

    /// Icon view: the pels of workspace above the work area, and the most.
    fn icon_scroll(&self) -> (i32, i32) {
        let (lo, hi) = self.extent();
        let h = self.frame().rows.3;
        (hi - (self.origin().1 + h), (hi - lo - h).max(0))
    }

    /// Icon view: the pels of workspace left of the work area, and the most.
    fn icon_xscroll(&self) -> (i32, i32) {
        let w = self.frame().rows.2;
        let (lo, hi) = icon_extent(self.xspan, w);
        (self.origin().0 - lo, (hi - lo - w).max(0))
    }

    /// Details view: the panes of rows at `x`, `w` wide.
    fn panes_for(&self, (x, w): (i32, i32)) -> Vec<Pane> {
        let adv: Vec<i32> = self.cols.iter().map(|c| c.adv).collect();
        detail_panes(&adv, self.left_cols, self.info.split_bar as i32, (x, w), self.hscroll)
    }

    /// Details view: the panes of the rows as shown.
    fn panes(&self) -> Vec<Pane> {
        let f = self.frame();
        self.panes_for((f.rows.0, f.rows.2))
    }

    /// Details view: the columns where they are shown (window `x`), each
    /// in its pane, moved by the pane's scrolling.
    fn columns(&self) -> Vec<Column> {
        let mut out = Vec::with_capacity(self.cols.len());
        for (k, p) in self.panes().iter().enumerate() {
            let base = self.cols.get(p.first).map_or(0, |c| c.x);
            for c in &self.cols[p.first..p.end] {
                out.push(Column { x: p.x - p.off + (c.x - base), pane: k, ..*c });
            }
        }
        out
    }

    /// Details view: the split bar's rectangle, through the column
    /// headings and the rows, when the view is split.
    fn split_rect(&self) -> Option<Rect> {
        if self.view() != View::Detail { return None; }
        let panes = self.panes();
        let left = panes.first().filter(|_| panes.len() == 2)?;
        let f = self.frame();
        let top = f.header.map_or(f.rows.1 + f.rows.3, |h| h.1 + h.3);
        Some((left.x + left.w, f.rows.1, SPLIT_W, top - f.rows.1))
    }

    /// The work area's origin in the workspace (`CNRINFO.ptlOrigin`).
    fn origin(&self) -> (i32, i32) {
        (self.info.origin.0 as i32, self.info.origin.1 as i32)
    }

    fn icon(&self) -> (i32, i32) { icon_size(&self.info, self.mini) }

    fn pictures(&self) -> bool { shows_icons(self.info.window_attr) }

    fn plain_frame(&self) -> Frame {
        let item_h = if self.pictures() && self.view() != View::Icon { self.icon().1 } else { 0 };
        Frame::new(self.info.window_attr, self.info.line_spacing as i32, item_h, self.cx, self.cy)
    }

    /// The view's frame, the rows narrowed by the vertical scroll bar and
    /// shortened by the horizontal ones when shown.
    fn frame(&self) -> Frame {
        let mut f = self.plain_frame();
        let (v, h) = self.bars();
        if v { f.rows.2 = (f.rows.2 - SCROLLBAR_W).max(0); }
        if h {
            f.rows.1 += SCROLLBAR_W;
            f.rows.3 = (f.rows.3 - SCROLLBAR_W).max(0);
        }
        f
    }

    /// The first row that can be on top (the last page starts there).
    fn max_top(&self) -> usize {
        if self.flow() { self.rows.len().saturating_sub(1) } else { self.rows.len().saturating_sub(self.frame().capacity()) }
    }

    /// The expander's width: a picture's when a row has one.
    fn expander_w(&self) -> i32 {
        if self.tree_pics.iter().any(Option::is_some) { TREE_PICTURE } else { EXPANDER }
    }

    fn indent(&self) -> i32 {
        match self.info.tree_indent as i32 { i if i > 0 => i.min(256), _ => TREE_INDENT }
    }

    /// Where row `i`'s expander starts, from its rectangle's left edge.
    fn expander_x(&self, i: usize) -> i32 {
        let depth = self.tree.get(i).map_or(0, |t| i32::from(t.0)).min(64);
        depth * self.indent() + CELL_PAD
    }

    /// Where row `i`'s picture starts, from its rectangle's left edge (name
    /// and tree views), if it has one.
    fn icon_inset(&self, i: usize) -> Option<i32> {
        if !self.pictures() { return None; }
        match self.view() {
            View::Name => Some(CELL_PAD),
            View::Tree => Some(self.expander_x(i) + self.expander_w() + CELL_PAD),
            _ => None,
        }
    }

    /// Where row `i`'s text starts, from its rectangle's left edge (tree
    /// view: after its indentation and expander; then after the picture).
    fn text_inset(&self, i: usize) -> i32 {
        if let Some(x) = self.icon_inset(i) { return x + self.icon().0; }
        if self.view() != View::Tree { return 0; }
        self.expander_x(i) + self.expander_w() + CELL_PAD
    }
}

impl WindowManager {
    /// Give a new `WC_CONTAINER` window its state and its scroll bars:
    /// the vertical one (`CID_VSCROLL`) and the horizontal ones of the
    /// left and right panes (`CID_HSCROLL`, `CID_RIGHTHSCROLL`), hidden
    /// until the view needs them.
    pub fn build_container(&mut self, hwnd: u32) {
        let Some(w) = self.get_window_mut(hwnd) else { return };
        if w.class_name != CLASS_CONTAINER || w.container.is_some() { return; }
        w.container = Some(Box::new(CnrState::new(w.style)));
        let hmq = w.hmq;
        for (id, style) in [(CID_VSCROLL, SBS_VERT), (CID_HSCROLL, SBS_HORZ), (CID_RIGHTHSCROLL, SBS_HORZ)] {
            let bar = self.create_window("WC_SCROLLBAR".to_string(), hwnd, hmq);
            if let Some(b) = self.get_window_mut(bar) {
                (b.id, b.owner, b.style, b.visible) = (id, hwnd, style, false);
            }
        }
    }
}

impl Loader {
    fn with_cnr<R>(&self, hwnd: u32, f: impl FnOnce(&mut CnrState, u32) -> R) -> Option<R> {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let w = wm.get_window_mut(hwnd)?;
        let style = w.style;
        w.container.as_deref_mut().map(|s| f(s, style))
    }

    fn rec_attr(&self, rec: u32) -> u32 {
        self.guest_read::<u32>(rec.wrapping_add(RC_ATTR)).unwrap_or(0)
    }

    fn set_rec_attr(&self, rec: u32, attr: u32) {
        let _ = self.guest_write::<u32>(rec.wrapping_add(RC_ATTR), attr);
    }

    fn snapshot(&self, hwnd: u32) -> Option<Snapshot> {
        let (cx, cy, info, mini, top, records, sizes, fields, hscroll, zorder, erased) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let w = wm.get_window(hwnd)?;
            let s = w.container.as_deref()?;
            let size = |a: u32| s.heap.item(a).map_or(0, |i| i.size);
            let sizes: HashMap<u32, u32> = s.records.iter().map(|&(r, _)| (r, size(r))).collect();
            let fields: Vec<(u32, u32)> = s.fields.iter().map(|&f| (f, size(f))).collect();
            (w.cx, w.cy, s.info, s.mini, s.top, s.records.clone(), sizes, fields, s.hscroll, s.zorder.clone(), s.erased.clone())
        };
        let font = Painter::new(self, 0, hwnd).font();
        let shown = |r: u32| self.rec_attr(r) & CRA_FILTERED == 0;
        let size = |r: u32| sizes.get(&r).copied().unwrap_or(0);
        type Rows = (Vec<(u32, u32)>, Vec<(u16, bool)>);
        let (rows, tree): Rows = if view_of(info.window_attr) == View::Tree {
            tree_order(&records, &|r| self.rec_attr(r) & CRA_EXPANDED != 0, &shown)
                .into_iter().map(|(r, d, k)| ((r, size(r)), (d, k))).unzip()
        } else {
            records.iter().filter(|&&(r, p)| p == 0 && shown(r)).map(|&(r, _)| ((r, size(r)), (0, false))).unzip()
        };
        let (icons, span, xspan) = if view_of(info.window_attr) == View::Icon {
            let (iw, ih) = icon_size(&info, mini);
            let mut span: Option<(i32, i32)> = None;
            let mut xspan: Option<(i32, i32)> = None;
            let icons: Vec<((i32, i32), Vec<String>)> = rows.iter().map(|&(r, _)| {
                let rd = |o: u32| self.guest_read::<u32>(r.wrapping_add(o)).unwrap_or(0) as i32;
                let ptl = (rd(RC_PTLX), rd(RC_PTLY));
                let text = self.record_text(hwnd, r, View::Icon, mini);
                let lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).take(64).collect();
                let lo = ptl.1.saturating_sub(icon_text_h(lines.len()));
                let hi = ptl.1.saturating_add(ih);
                span = Some(span.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
                // Across: the icon and its text centred under it.
                let tw = lines.iter().map(|l| crate::gui::font::text_width(l, font)).max().unwrap_or(0)
                    .saturating_add(2 * CELL_PAD);
                let tx = ptl.0.saturating_add(iw / 2).saturating_sub(tw / 2);
                let (xl, xh) = (ptl.0.min(tx), ptl.0.saturating_add(iw).max(tx.saturating_add(tw)));
                xspan = Some(xspan.map_or((xl, xh), |(a, b)| (a.min(xl), b.max(xh))));
                (ptl, lines)
            }).collect();
            (icons, span, xspan)
        } else { (Vec::new(), None, None) };
        // Icon view: the stacking of the shown rows, bottom first.
        let z = if view_of(info.window_attr) == View::Icon {
            let rank: HashMap<u32, usize> = zorder.iter().enumerate().map(|(k, &r)| (r, k)).collect();
            let mut z: Vec<usize> = (0..rows.len()).collect();
            z.sort_by_key(|&i| rank.get(&rows[i].0).copied().unwrap_or(usize::MAX));
            z
        } else { Vec::new() };
        // Tree view: each row's expander picture — its TREEITEMDESC's,
        // else CNRINFO's — for its state; icons unless only bitmaps are
        // drawn (CA_DRAWBITMAP without CA_DRAWICON).
        let tree_pics = if view_of(info.window_attr) == View::Tree {
            let bitmaps_first = info.window_attr & CA_DRAWBITMAP != 0 && info.window_attr & CA_DRAWICON == 0;
            rows.iter().zip(&tree).map(|(&(r, _), &(_, kids))| {
                if !kids { return None; }
                let open = self.rec_attr(r) & CRA_EXPANDED != 0;
                let desc = if mini { 0 } else { self.guest_read::<u32>(r.wrapping_add(RC_PTREEITEMDESC)).unwrap_or(0) };
                let rd = |o: u32| if desc == 0 { 0 } else { self.guest_read::<u32>(desc.wrapping_add(o)).unwrap_or(0) };
                // TREEITEMDESC: hbmExpanded, hbmCollapsed, hptrExpanded, hptrCollapsed.
                let (bm, ptr) = if open { (rd(0), rd(8)) } else { (rd(4), rd(12)) };
                let (cbm, cptr) = if open { (info.tree_bitmaps.0, info.tree_icons.0) } else { (info.tree_bitmaps.1, info.tree_icons.1) };
                let choose = |b: u32, p: u32| {
                    let both = if bitmaps_first { [(b, true), (p, false)] } else { [(p, false), (b, true)] };
                    both.into_iter().find(|&(h, _)| h != 0)
                };
                choose(bm, ptr).or_else(|| choose(cbm, cptr))
            }).collect()
        } else { Vec::new() };
        let mut snap = Snapshot { cx, cy, info, mini, top, rows, tree, fields, icons, span, xspan,
                                  cols: Vec::new(), left_cols: None, hscroll, z, tree_pics, erased };
        if snap.view() == View::Detail {
            snap.cols = self.column_widths(hwnd, &snap, font);
            snap.left_cols = (info.field_last != 0)
                .then(|| snap.cols.iter().position(|c| c.fi == info.field_last).map(|i| i + 1)).flatten();
        }
        Some(snap)
    }

    fn cnr_text(&self, hwnd: u32, ptr: u32) -> String {
        if ptr == 0 { return String::new(); }
        self.read_guest_string_cp(ptr, self.window_codepage(hwnd))
    }

    /// The text record `rec` shows in `view`.
    fn record_text(&self, hwnd: u32, rec: u32, view: View, mini: bool) -> String {
        let p = self.guest_read::<u32>(rec.wrapping_add(text_offset(view, mini))).unwrap_or(0);
        self.cnr_text(hwnd, p)
    }

    /// The text of field info `fi` (with flags `fl_data`) for record `rec`
    /// of `size` bytes; empty when its data lies outside the record.
    fn field_value(&self, hwnd: u32, rec: u32, size: u32, fi: u32, fl_data: u32) -> String {
        let off = self.guest_read::<u32>(fi.wrapping_add(FI_OFFSTRUCT)).unwrap_or(u32::MAX);
        if off.checked_add(4).is_none_or(|end| end > size) {
            return String::new();
        }
        let at = rec.wrapping_add(off);
        let loc = &self.shared.locale;
        if fl_data & CFA_STRING != 0 {
            self.cnr_text(hwnd, self.guest_read::<u32>(at).unwrap_or(0))
        } else if fl_data & CFA_ULONG != 0 {
            self.guest_read::<u32>(at).map_or(String::new(), |v| v.to_string())
        } else if fl_data & CFA_DATE != 0 {
            let (d, m, y) = (self.guest_read::<u8>(at).unwrap_or(0), self.guest_read::<u8>(at + 1).unwrap_or(0),
                             self.guest_read::<u16>(at + 2).unwrap_or(0));
            format_date(d, m, y, loc.date_fmt, loc.date_sep as char)
        } else if fl_data & CFA_TIME != 0 {
            let (h, m, s) = (self.guest_read::<u8>(at).unwrap_or(0), self.guest_read::<u8>(at + 1).unwrap_or(0),
                             self.guest_read::<u8>(at + 2).unwrap_or(0));
            format_time(h, m, s, loc.time_fmt, loc.time_sep as char)
        } else {
            String::new()
        }
    }

    /// The detail-view columns of `snap` (visible field infos; `cxWidth`
    /// 0 = as wide as the title and the widest value), side by side from 0.
    fn column_widths(&self, hwnd: u32, snap: &Snapshot, font: crate::gui::font::FontSpec) -> Vec<Column> {
        let mut x = 0;
        let mut out = Vec::new();
        for &(fi, _) in &snap.fields {
            let rd = |o: u32| self.guest_read::<u32>(fi.wrapping_add(o)).unwrap_or(0);
            let (fl_data, fl_title, width) = (rd(FI_FLDATA), rd(FI_FLTITLE), rd(FI_CXWIDTH));
            if fl_data & CFA_INVISIBLE != 0 { continue; }
            // cxWidth is the guest's ULONG: bounded before it is signed.
            let w = if width != 0 { width.min(super::constants::MAX_WINDOW_COORD as u32) as i32 } else {
                let title = if fl_title & CFA_BITMAPORICON == 0 { self.cnr_text(hwnd, rd(FI_TITLE)) } else { String::new() };
                let mut w = crate::gui::font::text_width(&title, font);
                for &(r, size) in &snap.rows {
                    w = w.max(crate::gui::font::text_width(&self.field_value(hwnd, r, size, fi, fl_data), font));
                }
                w.max(if fl_data & CFA_BITMAPORICON != 0 { LINE_H } else { 0 })
            } + 2 * CELL_PAD;
            let adv = w + if fl_data & CFA_SEPARATOR != 0 { 1 } else { 0 };
            out.push(Column { fi, fl_data, fl_title, x, w, adv, pane: 0 });
            // Columns start within MAX_DETAIL_WIDTH, so sums of a few
            // positions and widths cannot overflow.
            x = x.saturating_add(adv).min(MAX_DETAIL_WIDTH);
        }
        out
    }

    /// Where the shown records of `snap` are: `(row index, rectangle)` (in
    /// the icon view the rectangle around the icon and its text).
    fn placements(&self, hwnd: u32, snap: &Snapshot, font: crate::gui::font::FontSpec) -> (Frame, Placed) {
        let f = snap.frame();
        if snap.view() == View::Icon {
            let placed = self.icon_items(snap, font).into_iter().map(|(i, a, b)| (i, union(a, b))).collect();
            return (f, placed);
        }
        let view = snap.view();
        let flow = snap.flow();
        let widths: Vec<i32> = if flow {
            snap.rows.iter().enumerate().map(|(i, &(r, _))| {
                crate::gui::font::text_width(&self.record_text(hwnd, r, view, snap.mini), font) + snap.text_inset(i)
            }).collect()
        } else { vec![0; snap.rows.len()] };
        let top = snap.top.min(snap.rows.len().saturating_sub(1));
        (f, place_list(&f, &widths, top, flow))
    }

    /// The icon view's items: `(row index, icon rectangle, text rectangle)`
    /// in window coordinates, in drawing order (the stacking, bottom first).
    fn icon_items(&self, snap: &Snapshot, font: crate::gui::font::FontSpec) -> Vec<(usize, Rect, Rect)> {
        let f = snap.frame();
        let icon = snap.icon();
        snap.z.iter().filter_map(|&i| {
            let (ptl, lines) = snap.icons.get(i)?;
            let tw = lines.iter().map(|l| crate::gui::font::text_width(l, font)).max().unwrap_or(0);
            let (a, b) = icon_item(*ptl, snap.origin(), (f.rows.0, f.rows.1), icon, tw, lines.len());
            Some((i, a, b))
        }).collect()
    }

    /// The shown record at window point `(x, y)` (in the icon view the
    /// topmost whose icon or text is there).
    fn record_at(&self, hwnd: u32, snap: &Snapshot, (x, y): (i32, i32)) -> Option<usize> {
        let font = Painter::new(self, 0, hwnd).font();
        let inside = |(rx, ry, rw, rh): Rect| x >= rx && x < rx + rw && y >= ry && y < ry + rh;
        if snap.view() == View::Icon {
            if !inside(snap.frame().rows) { return None; }
            return self.icon_items(snap, font).into_iter().rev().find(|&(_, a, b)| inside(a) || inside(b)).map(|(i, _, _)| i);
        }
        let (_, placed) = self.placements(hwnd, snap, font);
        placed.into_iter().find(|&(_, r)| inside(r)).map(|(i, _)| i)
    }

    /// Change emphasis `mask` of record `rec` (on or off), keeping a single
    /// selection / one cursored record, and tell the owner (`CN_EMPHASIS`).
    fn cnr_set_emphasis(&self, hwnd: u32, rec: u32, on: bool, mask: u32) {
        let Some((style, all)) = self.with_cnr(hwnd, |s, style| (style, s.records.iter().map(|&(r, _)| r).collect::<Vec<_>>()))
        else { return };
        let mut changes: Vec<(u32, u32)> = Vec::new();
        let mut set = |r: u32, bits: u32, on: bool| {
            let old = self.rec_attr(r);
            let new = if on { old | bits } else { old & !bits };
            if new != old {
                self.set_rec_attr(r, new);
                changes.push((r, old ^ new));
            }
        };
        if on {
            let exclusive = mask & CRA_CURSORED
                | if sel_mode(style) == SelMode::Single { mask & CRA_SELECTED } else { 0 };
            if exclusive != 0 {
                for &r in all.iter().filter(|&&r| r != rec) { set(r, exclusive, false); }
            }
        }
        set(rec, mask, on);
        let mut wm = self.shared.window_mgr.lock_or_recover();
        for (r, bits) in changes {
            wm.queue_owner_call(OwnerCall { control: hwnd, kind: OwnerCallKind::ContainerNotify {
                code: CN_EMPHASIS as u16, block: [hwnd, r, bits],
            }});
        }
    }

    /// Read `n` pointers of a guest array at `ptr` (bounded by the item
    /// limit).
    fn read_ptr_array(&self, ptr: u32, n: u32) -> Option<Vec<u32>> {
        if n as usize > MAX_CNR_ITEMS { return None; }
        (0..n).map(|i| self.guest_read::<u32>(ptr.wrapping_add(i.wrapping_mul(4)))).collect()
    }

    /// Follow `preccNextRecord` (`next` offset) from `first` for `n` items.
    fn read_chain(&self, first: u32, n: u32, next: u32) -> Option<Vec<u32>> {
        if n == 0 || n as usize > MAX_CNR_ITEMS { return None; }
        let mut out = Vec::with_capacity(n as usize);
        let mut at = first;
        for _ in 0..n {
            if at == 0 { return None; }
            out.push(at);
            at = self.guest_read::<u32>(at.wrapping_add(next))?;
        }
        Some(out)
    }

    /// `CM_ALLOCRECORD` / `CM_ALLOCDETAILFIELDINFO`: `n` items of `size`
    /// bytes, zeroed, `cb` set to `cb`, chained through `next`. The first,
    /// or 0.
    fn cnr_alloc(&self, hwnd: u32, kind: ItemKind, size: u32, cb: u32, n: u32, next: u32) -> u32 {
        let size = size.next_multiple_of(4);
        let total = u64::from(size) * u64::from(n);
        if n == 0 || total > MAX_ALLOC_BYTES { return 0; }
        let Some((fits, room)) = self.with_cnr(hwnd, |s, _| (s.heap.fits(size, n), s.heap.has_room(n))) else { return 0 };
        if !room { return 0; }
        if !fits {
            let bytes = (total as u32).max(CNR_ARENA_BYTES);
            let Some(base) = self.shared.mem_mgr.lock_or_recover().alloc(bytes) else { return 0 };
            if self.with_cnr(hwnd, |s, _| s.heap.add_arena(base, bytes)).is_none() {
                self.shared.mem_mgr.lock_or_recover().free(base);
                return 0;
            }
        }
        let Some(addrs) = self.with_cnr(hwnd, |s, _| s.heap.carve(kind, size, n)).flatten() else { return 0 };
        for (i, &a) in addrs.iter().enumerate() {
            if let Some(mut m) = self.guest_slice_mut(a, size as usize) { m.fill(0); }
            let _ = self.guest_write::<u32>(a, cb);
            let _ = self.guest_write::<u32>(a + next, addrs.get(i + 1).copied().unwrap_or(0));
        }
        addrs[0]
    }

    /// Free `items` of the container (none may be inserted); FALSE when
    /// one is not an item of `kind`.
    fn cnr_free(&self, hwnd: u32, items: &[u32], kind: ItemKind) -> bool {
        let empty = self.with_cnr(hwnd, |s, _| {
            if items.iter().any(|&a| s.heap.item(a).is_none_or(|i| i.kind != kind)) { return None; }
            if items.iter().any(|&a| s.is_inserted(a) || s.fields.contains(&a)) { return None; }
            Some(items.iter().filter_map(|&a| s.heap.free(a).flatten()).collect::<Vec<u32>>())
        }).flatten();
        let Some(empty) = empty else { return false };
        let mut mm = self.shared.mem_mgr.lock_or_recover();
        for base in empty { mm.free(base); }
        true
    }

    /// Give the arenas of destroyed containers back to the memory manager.
    pub(crate) fn free_pending_guest_blocks(&self) {
        let blocks = std::mem::take(&mut self.shared.window_mgr.lock_or_recover().pending_guest_frees);
        if blocks.is_empty() { return; }
        let mut mm = self.shared.mem_mgr.lock_or_recover();
        for b in blocks { mm.free(b); }
    }

    /// A message for container `hwnd`; `None` when it is not a container
    /// or the message is not one it handles.
    pub(super) fn container_message(&self, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let (style, core, mini) = self.with_cnr(hwnd, |s, style| (style, s.core_size(), s.mini))?;
        let vcpu = super::interrupt::current_vcpu();
        let fail = |code: u32| if let Some(v) = vcpu { self.pm_error(v, code); };
        let ok = |b: bool| ApiResult::Normal(b as u32);
        let minus1 = ApiResult::Normal(u32::MAX);
        let lo = mp2 & 0xFFFF;
        let hi = (mp2 >> 16) as u16;
        Some(match msg {
            WM_BUTTON1DOWN => {
                // A click ends an edit: Alt+click discards it, a plain one
                // keeps it. Alt+click on a record's text starts one.
                let alt = (mp2 >> 16) & KC_ALT != 0;
                if self.cnr_close_edit(hwnd, !alt) { return Some(ok(true)); }
                if self.cnr_split_press(hwnd, mp1) { return Some(ok(true)); }
                if alt && self.cnr_alt_click(hwnd, style, mp1) { return Some(ok(true)); }
                ok(self.cnr_click(hwnd, style, mp1, mp2))
            }
            WM_BUTTON1DBLCLK => {
                self.cnr_click(hwnd, style, mp1, mp2);
                let rec = self.snapshot(hwnd).and_then(|snap| {
                    let at = ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32);
                    self.record_at(hwnd, &snap, at).map(|i| snap.rows[i].0)
                }).unwrap_or(0);
                self.cnr_enter(hwnd, false, rec);
                ok(true)
            }
            WM_CHAR => ok(self.cnr_char(hwnd, style, mp1, mp2)),
            // While the split bar is dragged: it follows the pointer.
            WM_MOUSEMOVE => return self.cnr_split_drag(hwnd, mp1).then_some(ok(true)),
            WM_BUTTON1UP => {
                let held = self.with_cnr(hwnd, |c, _| c.split_drag.take()).flatten().is_some();
                if !held { return None; }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if wm.pressed_control == hwnd { wm.pressed_control = 0; }
                ok(true)
            }
            // The drag button pressed and moved over the container: the owner
            // hears CN_INITDRAG (a CNRDRAGINIT) and may start the drag.
            super::pm_drag::WM_BEGINDRAG => ok(self.cnr_init_drag(hwnd, mp1)),
            // mp1 = cbRecordData, mp2 = number of records.
            CM_ALLOCRECORD => {
                if mp1 > MAX_RECORD_DATA { return Some(ApiResult::Normal(0)); }
                ApiResult::Normal(self.cnr_alloc(hwnd, ItemKind::Record, core + mp1, core, lo, RC_NEXT))
            }
            CM_ALLOCDETAILFIELDINFO => ApiResult::Normal(self.cnr_alloc(hwnd, ItemKind::Field, FIELDINFO_SIZE, FIELDINFO_SIZE,
                                                                        mp1 & 0xFFFF, FI_NEXT)),
            // mp1 = array of records, mp2 = count.
            CM_FREERECORD => {
                let Some(recs) = self.read_ptr_array(mp1, lo) else { return Some(ok(false)) };
                if self.with_cnr(hwnd, |s, _| recs.iter().any(|&r| s.is_inserted(r))) == Some(true) {
                    fail(PMERR_RECORD_CURRENTLY_INSERTED);
                    return Some(ok(false));
                }
                ok(self.cnr_free(hwnd, &recs, ItemKind::Record))
            }
            CM_FREEDETAILFIELDINFO => {
                let Some(fis) = self.read_ptr_array(mp1, lo) else { return Some(ok(false)) };
                if self.with_cnr(hwnd, |s, _| fis.iter().any(|f| s.fields.contains(f))) == Some(true) {
                    fail(PMERR_FI_CURRENTLY_INSERTED);
                    return Some(ok(false));
                }
                ok(self.cnr_free(hwnd, &fis, ItemKind::Field))
            }
            // mp1 = first record (chained), mp2 = PRECORDINSERT.
            CM_INSERTRECORD | CM_INSERTRECORDARRAY => {
                let rd = |o: u32| self.guest_read::<u32>(mp2.wrapping_add(o));
                let (Some(order), Some(parent), Some(inval), Some(n)) = (rd(RI_ORDER), rd(RI_PARENT), rd(RI_INVALIDATE), rd(RI_COUNT))
                else { return Some(ApiResult::Normal(0)) };
                let zorder = rd(RI_ZORDER).unwrap_or(CMA_TOP);
                let recs = if msg == CM_INSERTRECORD { self.read_chain(mp1, n, RC_NEXT) } else { self.read_ptr_array(mp1, n) };
                let Some(recs) = recs.filter(|r| !r.is_empty()) else { return Some(ApiResult::Normal(0)) };
                match self.with_cnr(hwnd, |s, _| s.insert(&recs, order, parent).map(|c| {
                    // zOrder CMA_BOTTOM: under the others (CMA_TOP: on top).
                    if zorder == CMA_BOTTOM { s.lower(&recs); }
                    (c, s.info.sort_record)
                })) {
                    // With a sort function (CNRINFO.pSortRecord) the
                    // parent's children are sorted before the call returns.
                    Some(Ok((count, sort))) if sort != 0 => self.cnr_start_sort(hwnd, sort, 0, Some(parent), count, true),
                    Some(Ok((count, _))) => {
                        self.cnr_autoposition(hwnd);
                        if inval != 0 { self.repaint_control(hwnd); }
                        ApiResult::Normal(count)
                    }
                    Some(Err(CnrError::Inserted)) => { fail(PMERR_RECORD_CURRENTLY_INSERTED); ApiResult::Normal(0) }
                    _ => ApiResult::Normal(0),
                }
            }
            // mp1 = array, mp2 = MPFROM2SHORT(count (0 = all), CMA_FREE | CMA_INVALIDATE).
            CM_REMOVERECORD => {
                let all = lo == 0;
                let recs = if all { self.with_cnr(hwnd, |s, _| s.records.iter().map(|&(r, _)| r).collect::<Vec<_>>()) }
                           else { self.read_ptr_array(mp1, lo) };
                let Some(recs) = recs else { return Some(minus1) };
                let Some(Ok(gone)) = self.with_cnr(hwnd, |s, _| {
                    let gone = s.removal(&recs)?;
                    s.remove(&gone);
                    Ok::<_, CnrError>(gone)
                }) else { return Some(minus1) };
                for &r in &gone {
                    let a = self.rec_attr(r);
                    self.set_rec_attr(r, a & !(CRA_SELECTED | CRA_CURSORED));
                }
                if hi & CMA_FREE != 0 { self.cnr_free(hwnd, &gone, ItemKind::Record); }
                self.cnr_autoposition(hwnd);
                self.cnr_clamp_top(hwnd);
                self.repaint_control(hwnd);
                ApiResult::Normal(self.with_cnr(hwnd, |s, _| s.records.len() as u32).unwrap_or(0))
            }
            // mp1 = reference record, mp2 = MPFROM2SHORT(cmd, order).
            CM_QUERYRECORD => match self.with_cnr(hwnd, |s, _| s.query_shown(mp1, lo, &|r| self.rec_attr(r) & CRA_FILTERED == 0)) {
                Some(Ok(r)) => ApiResult::Normal(r),
                _ => minus1,
            },
            // mp1 = start after (CMA_FIRST = from the start), mp2 = emphasis mask.
            CM_QUERYRECORDEMPHASIS => {
                let Some(all) = self.with_cnr(hwnd, |s, _| s.records.iter().map(|&(r, _)| r).collect::<Vec<_>>()) else { return Some(minus1) };
                let start = if mp1 == CMA_FIRST { 0 } else {
                    match all.iter().position(|&r| r == mp1) { Some(i) => i + 1, None => return Some(minus1) }
                };
                let mask = lo;
                ApiResult::Normal(all[start..].iter().copied().find(|&r| self.rec_attr(r) & mask == mask && mask != 0).unwrap_or(0))
            }
            // mp1 = record, mp2 = MPFROM2SHORT(on, emphasis).
            CM_SETRECORDEMPHASIS => {
                if mp1 == 0 { return Some(ok(true)); }
                if self.with_cnr(hwnd, |s, _| s.is_inserted(mp1)) != Some(true) { return Some(ok(false)); }
                self.cnr_set_emphasis(hwnd, mp1, lo != 0, u32::from(hi));
                if u32::from(hi) & CRA_CURSORED != 0 && lo != 0 { self.cnr_follow(hwnd, mp1); }
                self.repaint_control(hwnd);
                ok(true)
            }
            CM_INVALIDATERECORD | CM_INVALIDATEDETAILFIELDINFO => {
                if msg == CM_INVALIDATERECORD {
                    // Erased records come back: those listed, or all (count 0).
                    let listed = if lo == 0 { None } else { self.read_ptr_array(mp1, lo) };
                    self.with_cnr(hwnd, |s, _| match &listed {
                        Some(recs) => s.erased.retain(|r| !recs.contains(r)),
                        None => s.erased.clear(),
                    });
                    self.cnr_autoposition(hwnd);
                }
                self.cnr_clamp_top(hwnd);
                ok(self.repaint_control(hwnd))
            }
            CM_QUERYRECORDINFO => ok(true),
            // mp1 = PCNRINFO, mp2 = bytes to copy.
            CM_QUERYCNRINFO => {
                let Some(words) = self.with_cnr(hwnd, |s, _| s.info.words(s.records.len() as u32, s.fields.len() as u32))
                else { return Some(ApiResult::Normal(0)) };
                let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
                let n = (mp2 as usize).min(bytes.len());
                if mp1 == 0 || self.guest_write_bytes(mp1, &bytes[..n]).is_none() { return Some(ApiResult::Normal(0)); }
                ApiResult::Normal(n as u32)
            }
            // mp1 = PCNRINFO, mp2 = CMA_* flags.
            CM_SETCNRINFO => {
                let Some(raw) = self.guest_read_bytes(mp1, CNRINFO_SIZE as usize).filter(|_| mp1 != 0) else { return Some(ok(false)) };
                let mut w = [0u32; 23];
                for (i, c) in raw.as_chunks::<4>().0.iter().enumerate() { w[i] = u32::from_le_bytes(*c); }
                self.with_cnr(hwnd, |s, _| { s.info.set_from(&w, mp2); s.top = 0; });
                if mp2 & CMA_FLWINDOWATTR != 0 { self.cnr_autoposition(hwnd); }
                self.repaint_control(hwnd);
                ok(true)
            }
            // mp1 = first field info (chained), mp2 = PFIELDINFOINSERT.
            CM_INSERTDETAILFIELDINFO => {
                let rd = |o: u32| self.guest_read::<u32>(mp2.wrapping_add(o));
                let (Some(order), Some(inval), Some(n)) = (rd(FII_ORDER), rd(FII_INVALIDATE), rd(FII_COUNT))
                else { return Some(ApiResult::Normal(0)) };
                let Some(fis) = self.read_chain(mp1, n, FI_NEXT) else { return Some(ApiResult::Normal(0)) };
                match self.with_cnr(hwnd, |s, _| s.insert_fields(&fis, order)) {
                    Some(Ok(count)) => {
                        if inval != 0 { self.repaint_control(hwnd); }
                        ApiResult::Normal(count)
                    }
                    Some(Err(CnrError::Inserted)) => { fail(PMERR_FI_CURRENTLY_INSERTED); ApiResult::Normal(0) }
                    _ => ApiResult::Normal(0),
                }
            }
            CM_QUERYDETAILFIELDINFO => match self.with_cnr(hwnd, |s, _| s.query_field(mp1, mp2 & 0xFFFF)) {
                Some(Ok(f)) => ApiResult::Normal(f),
                _ => minus1,
            },
            // mp1 = array, mp2 = MPFROM2SHORT(count (0 = all), CMA_FREE | CMA_INVALIDATE).
            CM_REMOVEDETAILFIELDINFO => {
                let fis = if lo == 0 { self.with_cnr(hwnd, |s, _| s.fields.clone()) } else { self.read_ptr_array(mp1, lo) };
                let Some(fis) = fis else { return Some(minus1) };
                let removed = self.with_cnr(hwnd, |s, _| {
                    if fis.iter().any(|f| !s.fields.contains(f)) { return false; }
                    s.fields.retain(|f| !fis.contains(f));
                    true
                });
                if removed != Some(true) { return Some(minus1); }
                if hi & CMA_FREE != 0 { self.cnr_free(hwnd, &fis, ItemKind::Field); }
                self.repaint_control(hwnd);
                ApiResult::Normal(self.with_cnr(hwnd, |s, _| s.fields.len() as u32).unwrap_or(0))
            }
            // mp1 = CMA_VERTICAL / CMA_HORIZONTAL, mp2 = pels.
            CM_SCROLLWINDOW => {
                if mp1 & 0xFFFF == CMA_HORIZONTAL {
                    // The left (or only) pane, or the icon view's work area.
                    let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
                    let pos = match snap.view() {
                        View::Icon => snap.icon_xscroll().0,
                        _ => snap.panes().first().map_or(0, |p| p.off),
                    };
                    return Some(ok(self.cnr_hscroll(hwnd, &snap, 0, i64::from(pos) + i64::from(mp2 as i32))));
                }
                if mp1 & 0xFFFF == CMA_VERTICAL {
                    let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
                    if snap.view() == View::Icon {
                        self.cnr_scroll_icons(hwnd, &snap, i64::from(snap.icon_scroll().0) + i64::from(mp2 as i32));
                        return Some(ok(true));
                    }
                    let rows = (mp2 as i32) / snap.frame().row_h;
                    self.cnr_scroll_to(hwnd, &snap, snap.top as i64 + i64::from(rows));
                }
                ok(true)
            }
            // From a horizontal scroll bar: mp1 = its id, mp2 = MPFROM2SHORT(slider, SB_*).
            WM_HSCROLL => {
                let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
                let pane = usize::from(mp1 & 0xFFFF == CID_RIGHTHSCROLL);
                let (pos, page) = match snap.view() {
                    View::Icon => (snap.icon_xscroll().0, snap.frame().rows.2),
                    _ => snap.panes().get(pane).map_or((0, 0), |p| (p.off, p.w)),
                };
                let (pos, page) = (i64::from(pos), i64::from(page.max(1)));
                let to = match mp2 >> 16 {
                    SB_LINELEFT => pos - i64::from(H_LINE),
                    SB_LINERIGHT => pos + i64::from(H_LINE),
                    SB_PAGELEFT => pos - page,
                    SB_PAGERIGHT => pos + page,
                    SB_SLIDERTRACK | SB_SLIDERPOSITION => i64::from(mp2 as u16 as i16),
                    _ => return Some(ok(false)),
                };
                ok(self.cnr_hscroll(hwnd, &snap, pane, to))
            }
            // From the vertical scroll bar: mp2 = MPFROM2SHORT(slider, SB_*).
            WM_VSCROLL => {
                let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
                if snap.view() == View::Icon {
                    // The icon view scrolls by pels.
                    let (pos, page) = (i64::from(snap.icon_scroll().0), i64::from(snap.frame().rows.3.max(1)));
                    let to = match mp2 >> 16 {
                        SB_LINEUP => pos - i64::from(LINE_H),
                        SB_LINEDOWN => pos + i64::from(LINE_H),
                        SB_PAGEUP => pos - page,
                        SB_PAGEDOWN => pos + page,
                        SB_SLIDERTRACK | SB_SLIDERPOSITION => i64::from(mp2 as u16 as i16),
                        _ => return Some(ok(false)),
                    };
                    self.cnr_scroll_icons(hwnd, &snap, to);
                    return Some(ok(true));
                }
                let page = snap.frame().capacity().saturating_sub(1).max(1) as i64;
                let top = snap.top as i64;
                let to = match mp2 >> 16 {
                    SB_LINEUP => top - 1,
                    SB_LINEDOWN => top + 1,
                    SB_PAGEUP => top - page,
                    SB_PAGEDOWN => top + page,
                    SB_SLIDERTRACK | SB_SLIDERPOSITION => i64::from(mp2 as u16 as i16),
                    _ => return Some(ok(false)),
                };
                self.cnr_scroll_to(hwnd, &snap, to);
                ok(true)
            }
            // mp1 = comparison function, mp2 = pStorage.
            CM_SORTRECORD => {
                if mp1 == 0 { return Some(ok(false)); }
                self.cnr_start_sort(hwnd, mp1, mp2, None, 1, false)
            }
            // mp1 = PCNREDITDATA (pRecord, pFieldInfo, id), mp2 = 0.
            CM_OPENEDIT => {
                let rd = |o: u32| self.guest_read::<u32>(mp1.wrapping_add(o)).unwrap_or(0);
                if mp1 == 0 { fail(PMERR_INVALID_PARAMETERS); return Some(ok(false)); }
                let opened = self.cnr_open_edit(hwnd, style, rd(CED_RECORD), rd(CED_FIELD), rd(CED_ID));
                if !opened { fail(PMERR_INVALID_PARAMETERS); }
                ok(opened)
            }
            CM_CLOSEEDIT => ok(self.cnr_close_edit(hwnd, true)),
            // mp1 = filter function (NULL: show every record), mp2 = pStorage.
            CM_FILTER => self.cnr_start_filter(hwnd, mp1, mp2),
            // mp1 = record (NULL: every record).
            CM_EXPANDTREE | CM_COLLAPSETREE => ok(self.cnr_expand(hwnd, mp1, msg == CM_EXPANDTREE)),
            // mp1 = PSEARCHSTRING, mp2 = start after (CMA_FIRST = from the start).
            CM_SEARCHSTRING => self.cnr_search(hwnd, mini, mp1, mp2).map_or(minus1, ApiResult::Normal),
            // mp1 = PRECTL out, mp2 = PQUERYRECORDRECT.
            CM_QUERYRECORDRECT => {
                let rec = self.guest_read::<u32>(mp2.wrapping_add(QRR_RECORD)).unwrap_or(0);
                let extent = self.guest_read::<u32>(mp2.wrapping_add(QRR_EXTENT)).unwrap_or(0);
                let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
                let Some((x, y, w, h)) = self.record_rect(hwnd, &snap, rec, extent) else { return Some(ok(false)) };
                ok(self.write_rectl(mp1, super::pm_rect::Rectl { x_left: x, y_bottom: y, x_right: x + w, y_top: y + h }))
            }
            // mp1 = search after (CMA_FIRST = from the start), mp2 = PQUERYRECFROMRECT.
            CM_QUERYRECORDFROMRECT => self.cnr_record_from_rect(hwnd, mp1, mp2).map_or(minus1, ApiResult::Normal),
            // mp1 = CMA_ARRANGESTANDARD / CMA_ARRANGEGRID, mp2 = 0 or CMA_ARRANGESELECTED.
            CM_ARRANGE => {
                if ![CMA_ARRANGESTANDARD, CMA_ARRANGEGRID].contains(&mp1) || mp2 & !CMA_ARRANGESELECTED != 0 { return Some(ok(false)); }
                ok(self.cnr_arrange(hwnd, mp1 == CMA_ARRANGEGRID, mp2 & CMA_ARRANGESELECTED != 0))
            }
            // mp1 = PGRIDINFO: the grid square (cxGrid, cyGrid).
            CM_SETGRIDINFO => {
                let (Some(cx), Some(cy)) = (self.guest_read::<i16>(mp1.wrapping_add(4)), self.guest_read::<i16>(mp1.wrapping_add(6)))
                else { return Some(ok(false)) };
                let (cx, cy) = (i32::from(cx), i32::from(cy));
                if mp1 == 0 || !(1..=MAX_GRID).contains(&cx) || !(1..=MAX_GRID).contains(&cy) { return Some(ok(false)); }
                self.with_cnr(hwnd, |s, _| s.grid = Some((cx, cy)));
                ok(true)
            }
            // mp1 = PGRIDINFO out: the square, the rows and columns of the work area.
            CM_QUERYGRIDINFO => {
                let Some(snap) = self.snapshot(hwnd).filter(|_| mp1 != 0) else { return Some(ok(false)) };
                let (cx, cy) = self.with_cnr(hwnd, |s, _| s.grid).flatten().unwrap_or_else(|| default_grid(snap.icon()));
                let rows = snap.frame().rows;
                let (gc, gr) = ((rows.2 / cx).max(1), (rows.3 / cy).max(1));
                let w16 = |o: u32, v: i32| self.guest_write::<i16>(mp1.wrapping_add(o), v.clamp(0, i32::from(i16::MAX)) as i16);
                let ok_all = w16(4, cx).and(w16(6, cy)).and(w16(8, gr)).and(w16(10, gc))
                    .and(self.guest_write::<u32>(mp1.wrapping_add(12), (gr * gc) as u32));
                ok(ok_all.is_some())
            }
            // mp1 = record: erased from the view until it is invalidated.
            CM_ERASERECORD => {
                if self.with_cnr(hwnd, |s, _| s.is_inserted(mp1)) != Some(true) { return Some(ok(false)); }
                self.with_cnr(hwnd, |s, _| s.erased.insert(mp1));
                ok(self.repaint_control(hwnd))
            }
            // mp1 = PTREEMOVE (preccMove, preccNewParent, pRecordOrder, flMoveSiblings).
            CM_MOVETREE => {
                let rd = |o: u32| self.guest_read::<u32>(mp1.wrapping_add(o));
                let (Some(rec), Some(parent), Some(order), Some(sibs)) = (rd(0), rd(4), rd(8), rd(12)) else { return Some(ok(false)) };
                if mp1 == 0 { return Some(ok(false)); }
                self.cnr_close_edit(hwnd, true);
                let moved = self.with_cnr(hwnd, |s, _| s.move_tree(rec, parent, order, sibs != 0)) == Some(Ok(()));
                if moved {
                    self.cnr_clamp_top(hwnd);
                    self.repaint_control(hwnd);
                }
                ok(moved)
            }
            // The context-menu gesture (button 2, Shift+F10) or help (F1)
            // on the container: the owner hears it with the record under
            // the pointer, or the cursored one from the keyboard.
            WM_CONTEXTMENU | WM_HELP => {
                let rec = if msg == WM_CONTEXTMENU && mp2 & 0xFFFF != 0 {
                    let at = ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32);
                    self.snapshot(hwnd).and_then(|snap| self.record_at(hwnd, &snap, at).map(|i| snap.rows[i].0)).unwrap_or(0)
                } else {
                    self.cnr_cursored(hwnd)
                };
                let code = if msg == WM_CONTEXTMENU { CN_CONTEXTMENU } else { CN_HELP };
                self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
                    control: hwnd, kind: OwnerCallKind::ContainerValue { code: code as u16, mp2: rec },
                });
                ok(true)
            }
            // mp1 = PRECTL out, mp2 = MPFROM2SHORT(CMA_WINDOW / CMA_WORKSPACE, right split window).
            CM_QUERYVIEWPORTRECT => {
                if lo == u32::from(CMA_WORKSPACE) {
                    let Some(r) = self.snapshot(hwnd).and_then(|snap| self.cnr_workspace(hwnd, &snap)) else { return Some(ok(false)) };
                    return Some(ok(self.write_rectl(mp1, r)));
                }
                if lo != u32::from(CMA_WINDOW) { return Some(ok(false)); }
                let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
                let (mut x, y, mut w, h) = snap.frame().rows;
                // A split details view: the left or the right pane.
                let panes = if snap.view() == View::Detail { snap.panes() } else { Vec::new() };
                if panes.len() == 2 {
                    let p = panes[usize::from(hi != 0)];
                    (x, w) = (p.x, p.w);
                } else if hi != 0 {
                    return Some(ok(false));
                }
                ok(self.write_rectl(mp1, super::pm_rect::Rectl { x_left: x, y_bottom: y, x_right: x + w, y_top: y + h }))
            }
            _ => return None,
        })
    }

    /// `CM_SEARCHSTRING`: the next record (after `after`, or from the
    /// first with `CMA_FIRST`) whose text in the asked view starts with
    /// (`fsPrefix`) or contains the string; `Some(0)` for none.
    fn cnr_search(&self, hwnd: u32, mini: bool, pss: u32, after: u32) -> Option<u32> {
        let rd = |o: u32| self.guest_read::<u32>(pss.wrapping_add(o));
        let (psz, prefix, case, usview) = (rd(SS_PSZ)?, rd(SS_PREFIX)?, rd(SS_CASE)?, rd(SS_VIEW)?);
        let needle = self.cnr_text(hwnd, psz);
        let rows: Vec<u32> = self.snapshot(hwnd)?.rows.iter().map(|&(r, _)| r).collect();
        let start = if after == CMA_FIRST { 0 } else { rows.iter().position(|&r| r == after)? + 1 };
        let fold = |t: String| if case != 0 { t } else { t.to_lowercase() };
        let needle = fold(needle);
        let view = view_of(usview);
        Some(rows[start..].iter().copied().find(|&r| {
            let t = fold(self.record_text(hwnd, r, view, mini));
            if prefix != 0 { t.starts_with(&needle) } else { t.contains(&needle) }
        }).unwrap_or(0))
    }

    /// Keep the first shown row within the records.
    fn cnr_clamp_top(&self, hwnd: u32) {
        let Some(max) = self.snapshot(hwnd).map(|s| s.max_top()) else { return };
        self.with_cnr(hwnd, |s, _| s.top = s.top.min(max));
    }

    /// Scroll so that row `to` (clamped) is the first shown, telling the
    /// owner (`CN_SCROLL`, a `NOTIFYSCROLL` with the pels scrolled).
    fn cnr_scroll_to(&self, hwnd: u32, snap: &Snapshot, to: i64) {
        let top = to.clamp(0, snap.max_top() as i64) as usize;
        if top == snap.top { return; }
        self.cnr_close_edit(hwnd, false);
        self.with_cnr(hwnd, |s, _| s.top = top);
        let pels = (top as i64 - snap.top as i64) * i64::from(snap.frame().row_h);
        self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
            control: hwnd, kind: OwnerCallKind::ContainerNotify { code: CN_SCROLL as u16, block: [hwnd, pels as i32 as u32, CMA_VERTICAL] },
        });
        self.repaint_control(hwnd);
    }

    /// Where record `rec`'s text for a direct edit is: its `ppszText`, the
    /// `CNREDITDATA.id` and the text's rectangle — `None` when it cannot
    /// be edited: a read-only container (`CCS_READONLY`) or record
    /// (`CRA_RECORDREADONLY`), a record not shown, in the details view a
    /// field info that is not an inserted, writable (`CFA_FIREADONLY`)
    /// `CFA_STRING` column. Without a record, `id` asks for the container
    /// title or a column heading ([`Self::title_edit_target`]).
    fn edit_target(&self, hwnd: u32, style: u32, rec: u32, fi: u32, id: u32) -> Option<(u32, u32, Rect)> {
        if style & CCS_READONLY != 0 { return None; }
        if rec == 0 { return self.title_edit_target(hwnd, fi, id); }
        if self.rec_attr(rec) & CRA_RECORDREADONLY != 0 { return None; }
        self.cnr_follow(hwnd, rec);
        let snap = self.snapshot(hwnd)?;
        let i = snap.rows.iter().position(|&(r, _)| r == rec)?;
        let font = Painter::new(self, 0, hwnd).font();
        let id = self.shared.window_mgr.lock_or_recover().get_window(hwnd).map_or(0, |w| w.id);
        match snap.view() {
            View::Detail => {
                let fl = self.guest_read::<u32>(fi.wrapping_add(FI_FLDATA))?;
                if !snap.fields.iter().any(|&(f, _)| f == fi) || fl & CFA_STRING == 0 || fl & CFA_FIREADONLY != 0 { return None; }
                let off = self.guest_read::<u32>(fi.wrapping_add(FI_OFFSTRUCT))?;
                if off.checked_add(4).is_none_or(|end| end > snap.rows[i].1) { return None; }
                let col = snap.columns().into_iter().find(|c| c.fi == fi)?;
                let (_, placed) = self.placements(hwnd, &snap, font);
                let &(_, row) = placed.iter().find(|&&(k, _)| k == i)?;
                let id = if col.pane == 1 { CID_RIGHTDVWND } else { CID_LEFTDVWND };
                Some((rec.wrapping_add(off), id, (col.x, row.1, col.w, row.3)))
            }
            view => {
                if fi != 0 { return None; }
                let ppsz = rec.wrapping_add(text_offset(view, snap.mini));
                let rect = if view == View::Icon {
                    self.icon_items(&snap, font).into_iter().find(|&(k, _, _)| k == i).map(|(_, _, t)| t)?
                } else {
                    let (_, placed) = self.placements(hwnd, &snap, font);
                    let &(_, row) = placed.iter().find(|&&(k, _)| k == i)?;
                    let inset = snap.text_inset(i);
                    (row.0 + inset, row.1, row.2 - inset, row.3)
                };
                Some((ppsz, id, rect))
            }
        }
    }

    /// Where the container title (`id` `CID_CNRTITLEWND`) or the heading of
    /// details-view column `fi` (`CID_LEFTCOLTITLEWND` /
    /// `CID_RIGHTCOLTITLEWND`) is for a direct edit: its `ppszText`, the
    /// window id (the heading's pane) and the rectangle. The title must be
    /// shown and not `CA_TITLEREADONLY`; its `PSZ` is put in a guest cell
    /// of the container (`CnrState::title_cell`), whose content becomes the
    /// title when the edit is kept. A heading must be shown, a text, and
    /// not `CFA_FITITLEREADONLY`; its `ppszText` is the field info's
    /// `pTitleData`.
    fn title_edit_target(&self, hwnd: u32, fi: u32, id: u32) -> Option<(u32, u32, Rect)> {
        let snap = self.snapshot(hwnd)?;
        let f = snap.frame();
        let attr = snap.info.window_attr;
        if id == CID_CNRTITLEWND {
            let rect = f.title.filter(|_| attr & CA_TITLEREADONLY == 0)?;
            let mut cell = self.with_cnr(hwnd, |c, _| c.title_cell)?;
            if cell == 0 {
                cell = self.cnr_alloc(hwnd, ItemKind::Cell, 8, 0, 1, 4);
                if cell == 0 { return None; }
                self.with_cnr(hwnd, |c, _| c.title_cell = cell);
            }
            self.guest_write::<u32>(cell, snap.info.title)?;
            return Some((cell, CID_CNRTITLEWND, rect));
        }
        if id != CID_LEFTCOLTITLEWND && id != CID_RIGHTCOLTITLEWND { return None; }
        let hd = f.header?;
        let col = snap.columns().into_iter().find(|c| c.fi == fi)?;
        if (col.fl_title | col.fl_data) & CFA_FITITLEREADONLY != 0 || col.fl_title & CFA_BITMAPORICON != 0 { return None; }
        let id = if col.pane == 1 { CID_RIGHTCOLTITLEWND } else { CID_LEFTCOLTITLEWND };
        Some((fi.wrapping_add(FI_TITLE), id, (col.x, hd.1, col.w, hd.3)))
    }

    /// Start a direct edit of record `rec` (`fi`: the details-view column):
    /// an MLE (`CID_MLE`) over the text holding it, all selected, with the
    /// focus; the owner hears `CN_BEGINEDIT`. An edit in progress is kept
    /// first. FALSE when the text cannot be edited.
    fn cnr_open_edit(&self, hwnd: u32, style: u32, rec: u32, fi: u32, want_id: u32) -> bool {
        self.cnr_close_edit(hwnd, true);
        let Some((ppsz, id, (x, y, w, h))) = self.edit_target(hwnd, style, rec, fi, want_id) else { return false };
        let ptr = self.guest_read::<u32>(ppsz).unwrap_or(0);
        let text = self.cnr_text(hwnd, ptr);
        let mle = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let Some((cnr_w, hmq)) = wm.get_window(hwnd).map(|c| (c.cx, c.hmq)) else { return false };
            let mle = match wm.find_child_by_id(hwnd, CID_MLE) {
                Some(m) => m,
                None => {
                    let m = wm.create_window(super::pm_controls::CLASS_MLE.to_string(), hwnd, hmq);
                    if let Some(c) = wm.get_window_mut(m) { (c.id, c.owner) = (CID_MLE, hwnd); }
                    m
                }
            };
            let Some(m) = wm.get_window_mut(mle) else { return false };
            let cx = w.max(EDIT_MIN_W).min((cnr_w - 2 * BORDER).max(1));
            let left = x.clamp(BORDER, (cnr_w - BORDER - cx).max(BORDER));
            (m.x, m.y, m.cx, m.cy, m.visible) = (left, y, cx, h.max(LINE_H + 4), true);
            m.style |= WS_VISIBLE;
            m.edit_anchor = 0;
            m.edit_caret = text.chars().count();
            m.text = text;
            mle
        };
        let s = EditSession { rec, fi, ppsz, id };
        self.with_cnr(hwnd, |c, _| c.edit = Some(s));
        self.queue_edit_note(hwnd, CN_BEGINEDIT, s.block(hwnd, 0));
        self.set_focus_window(mle);
        self.repaint_control(hwnd);
        true
    }

    /// End the direct edit of container `hwnd`, if one is in progress
    /// (FALSE otherwise). Kept (`save`): the owner is asked for room for
    /// the new text (`CN_REALLOCPSZ`, `cbText` counting the NUL), which is
    /// copied into `*ppszText` when it answers TRUE; then, and when
    /// discarded at once, `CN_ENDEDIT`. The MLE is hidden and the focus
    /// goes back to the container.
    pub(crate) fn cnr_close_edit(&self, hwnd: u32, save: bool) -> bool {
        let Some(Some(session)) = self.with_cnr(hwnd, |c, _| c.edit.take()) else { return false };
        let text = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let mle = wm.find_child_by_id(hwnd, CID_MLE);
            mle.and_then(|m| wm.get_window_mut(m)).map(|m| {
                m.visible = false;
                m.style &= !WS_VISIBLE;
                std::mem::take(&mut m.text)
            }).unwrap_or_default()
        };
        if save {
            let mut bytes = super::codepage::cp_encode(&text, self.window_codepage(hwnd));
            bytes.truncate(MAX_RECORD_DATA as usize);
            let cb = bytes.len() as u32 + 1;
            self.with_cnr(hwnd, |c, _| c.edit_text = Some(bytes));
            self.queue_edit_note(hwnd, CN_REALLOCPSZ, session.block(hwnd, cb));
        } else {
            self.queue_edit_note(hwnd, CN_ENDEDIT, session.block(hwnd, 0));
        }
        self.set_focus_window(hwnd);
        self.repaint_control(hwnd);
        true
    }

    /// The owner answered `CN_REALLOCPSZ` (`block`: the `CNREDITDATA` sent)
    /// with `result`: TRUE copies the new text into the string `*ppszText`
    /// points to (at most `cbText` bytes with the NUL); then `CN_ENDEDIT`.
    pub(crate) fn container_realloc_done(&self, hwnd: u32, result: u32, block: [u32; 7]) {
        let text = self.with_cnr(hwnd, |c, _| c.edit_text.take()).flatten();
        let (ppsz, cb) = (block[4], block[5]);
        if result != 0 && let Some(mut bytes) = text {
            let ptr = self.guest_read::<u32>(ppsz).unwrap_or(0);
            bytes.truncate(cb.saturating_sub(1) as usize);
            bytes.push(0);
            if ptr != 0 { let _ = self.guest_write_bytes(ptr, &bytes); }
            // The title: the string the cell now names (the owner may
            // have given it a new one).
            if block[2] == 0 && block[6] == CID_CNRTITLEWND {
                self.with_cnr(hwnd, |c, _| c.info.title = ptr);
            }
        }
        let mut end = block;
        end[5] = if result != 0 { cb } else { 0 };
        self.queue_edit_note(hwnd, CN_ENDEDIT, end);
        self.cnr_autoposition(hwnd);
        self.repaint_control(hwnd);
    }

    fn queue_edit_note(&self, hwnd: u32, code: u32, block: [u32; 7]) {
        self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
            control: hwnd, kind: OwnerCallKind::ContainerEdit { code: code as u16, block },
        });
    }

    /// Alt + button 1 at container point `mp1`: edit the text of the record
    /// there (in the icon view its text, in the details view the column),
    /// or the container title or column heading there.
    fn cnr_alt_click(&self, hwnd: u32, style: u32, mp1: u32) -> bool {
        let at = ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32);
        let Some(snap) = self.snapshot(hwnd) else { return false };
        let inside = |(rx, ry, rw, rh): Rect| at.0 >= rx && at.0 < rx + rw && at.1 >= ry && at.1 < ry + rh;
        let f = snap.frame();
        if f.title.is_some_and(inside) {
            return self.cnr_open_edit(hwnd, style, 0, 0, CID_CNRTITLEWND);
        }
        if f.header.is_some_and(inside) {
            let Some(c) = snap.columns().into_iter().find(|c| at.0 >= c.x && at.0 < c.x + c.w
                && snap.panes().get(c.pane).is_some_and(|p| at.0 >= p.x && at.0 < p.x + p.w)) else { return false };
            return self.cnr_open_edit(hwnd, style, 0, c.fi, CID_LEFTCOLTITLEWND);
        }
        let Some(i) = self.record_at(hwnd, &snap, at) else { return false };
        let rec = snap.rows[i].0;
        let fi = if snap.view() == View::Detail {
            match snap.columns().into_iter().find(|c| at.0 >= c.x && at.0 < c.x + c.w
                && snap.panes().get(c.pane).is_some_and(|p| at.0 >= p.x && at.0 < p.x + p.w)) {
                Some(c) => c.fi,
                None => return false,
            }
        } else { 0 };
        self.cnr_open_edit(hwnd, style, rec, fi, 0)
    }

    /// A key for the direct-editing MLE `mle` of container `hwnd`: Esc
    /// discards the edit. `None`: the MLE handles the key.
    pub(super) fn container_edit_key(&self, hwnd: u32, mle: u32, mp1: u32, mp2: u32) -> Option<ApiResult> {
        let id = self.shared.window_mgr.lock_or_recover().get_window(mle).map_or(0, |w| w.id);
        let fs = mp1 & 0xFFFF;
        if id != CID_MLE || fs & KC_KEYUP != 0 || fs & KC_VIRTUALKEY == 0 || mp2 >> WM_CHAR_VKEY_SHIFT != VK_ESC {
            return None;
        }
        self.cnr_close_edit(hwnd, false).then_some(ApiResult::Normal(1))
    }

    /// Icon view: scroll so that `to` pels of workspace (clamped) are above
    /// the work area, telling the owner (`CN_SCROLL`).
    fn cnr_scroll_icons(&self, hwnd: u32, snap: &Snapshot, to: i64) {
        let (pos, max) = snap.icon_scroll();
        let new = to.clamp(0, i64::from(max)) as i32;
        if new == pos { return; }
        self.cnr_close_edit(hwnd, false);
        let delta = new - pos;
        self.with_cnr(hwnd, |s, _| s.info.origin.1 = (s.info.origin.1 as i32).saturating_sub(delta) as u32);
        self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
            control: hwnd, kind: OwnerCallKind::ContainerNotify { code: CN_SCROLL as u16, block: [hwnd, delta as u32, CMA_VERTICAL] },
        });
        self.repaint_control(hwnd);
    }

    /// `CM_ARRANGE` (standard): place the shown top-level records in rows
    /// from the upper left of the work area (see [`arrange_icons`]),
    /// writing their `ptlIcon`, and scroll back to the workspace origin.
    fn cnr_arrange(&self, hwnd: u32, grid: bool, selected_only: bool) -> bool {
        let Some(snap) = self.snapshot(hwnd) else { return false };
        let font = Painter::new(self, 0, hwnd).font();
        let mut recs: Vec<u32> = if snap.view() == View::Icon { snap.rows.iter().map(|&(r, _)| r).collect() } else {
            let all = self.with_cnr(hwnd, |s, _| s.records.clone()).unwrap_or_default();
            all.into_iter().filter(|&(r, p)| p == 0 && self.rec_attr(r) & CRA_FILTERED == 0).map(|(r, _)| r).collect()
        };
        if selected_only {
            recs.retain(|&r| self.rec_attr(r) & CRA_SELECTED != 0);
        }
        let items: Vec<(i32, usize)> = recs.iter().map(|&r| {
            let text = self.record_text(hwnd, r, View::Icon, snap.mini);
            let lines: Vec<&str> = text.split('\n').take(64).collect();
            (lines.iter().map(|l| crate::gui::font::text_width(l.trim_end_matches('\r'), font)).max().unwrap_or(0), lines.len())
        }).collect();
        let icon = icon_size(&CnrInfo { window_attr: CV_ICON | (snap.info.window_attr & CV_MINI), ..snap.info }, snap.mini);
        let rows = snap.plain_frame().rows;
        let places = if grid {
            let square = self.with_cnr(hwnd, |s, _| s.grid).flatten().unwrap_or_else(|| default_grid(icon));
            arrange_grid(recs.len(), icon, square, rows.2 - SCROLLBAR_W, rows.3)
        } else {
            arrange_icons(&items, icon, rows.2 - SCROLLBAR_W, rows.3, snap.info.line_spacing as i32)
        };
        for (&r, &(px, py)) in recs.iter().zip(&places) {
            let _ = self.guest_write::<u32>(r.wrapping_add(RC_PTLX), px as u32);
            let _ = self.guest_write::<u32>(r.wrapping_add(RC_PTLY), py as u32);
        }
        self.with_cnr(hwnd, |s, _| s.info.origin = (0, 0));
        self.repaint_control(hwnd);
        true
    }

    /// `CCS_AUTOPOSITION`: arrange the icon view after records changed.
    fn cnr_autoposition(&self, hwnd: u32) {
        let auto = self.with_cnr(hwnd, |s, style| style & CCS_AUTOPOSITION != 0 && view_of(s.info.window_attr) == View::Icon);
        if auto == Some(true) { self.cnr_arrange(hwnd, false, false); }
    }

    /// `CM_QUERYRECORDRECT`: record `rec`'s rectangle; in the icon view
    /// `extent` picks the icon (`CMA_ICON`), the text (`CMA_TEXT`) or both,
    /// in the name and tree views the picture alone with `CMA_ICON`.
    fn record_rect(&self, hwnd: u32, snap: &Snapshot, rec: u32, extent: u32) -> Option<Rect> {
        let i = snap.rows.iter().position(|&(r, _)| r == rec)?;
        let font = Painter::new(self, 0, hwnd).font();
        if snap.view() == View::Icon {
            let (_, a, b) = self.icon_items(snap, font).into_iter().find(|&(k, _, _)| k == i)?;
            return Some(match (extent & CMA_ICON != 0, extent & CMA_TEXT != 0) {
                (true, false) => a,
                (false, true) => b,
                _ => union(a, b),
            });
        }
        let (_, placed) = self.placements(hwnd, snap, font);
        let &(_, row) = placed.iter().find(|&&(k, _)| k == i)?;
        if extent & (CMA_ICON | CMA_TEXT) == CMA_ICON && let Some(ix) = snap.icon_inset(i) {
            let (iw, ih) = snap.icon();
            return Some((row.0 + ix, row.1 + (row.3 - ih) / 2, iw, ih));
        }
        Some(row)
    }

    /// `CM_QUERYRECORDFROMRECT`: the next shown record (after `after`, or
    /// from the first with `CMA_FIRST`) lying partly (`CMA_PARTIAL`) or
    /// wholly (`CMA_COMPLETE`) in the `QUERYRECFROMRECT`'s rectangle;
    /// `CMA_ZORDER` searches from the topmost (the last drawn). The
    /// rectangle's x is ignored in the details view. `Some(0)` for none.
    fn cnr_record_from_rect(&self, hwnd: u32, after: u32, pq: u32) -> Option<u32> {
        let rd = |o: u32| self.guest_read::<u32>(pq.wrapping_add(o)).map(|v| v as i32);
        let (l, b, r, t) = (rd(QRFR_RECT)?, rd(QRFR_RECT + 4)?, rd(QRFR_RECT + 8)?, rd(QRFR_RECT + 12)?);
        let search = rd(QRFR_SEARCH)? as u32;
        let snap = self.snapshot(hwnd)?;
        let font = Painter::new(self, 0, hwnd).font();
        let (_, mut placed) = self.placements(hwnd, &snap, font);
        if search & CMA_ZORDER != 0 { placed.reverse(); }
        let detail = snap.view() == View::Detail;
        let complete = search & CMA_COMPLETE != 0;
        let hit = |(x, y, w, h): Rect| {
            let x_ok = detail || if complete { x >= l && x + w <= r } else { x < r && x + w > l };
            let y_ok = if complete { y >= b && y + h <= t } else { y < t && y + h > b };
            x_ok && y_ok
        };
        let order: Vec<u32> = placed.iter().map(|&(i, _)| snap.rows[i].0).collect();
        let start = if after == CMA_FIRST { 0 } else { order.iter().position(|&x| x == after)? + 1 };
        Some(placed[start..].iter().find(|&&(_, rect)| hit(rect)).map_or(0, |&(i, _)| snap.rows[i].0))
    }

    /// Expand (or collapse) record `rec` — every record with children for
    /// 0 — and tell the owner (`CN_EXPANDTREE` / `CN_COLLAPSETREE`, mp2 =
    /// the record). A collapse moves the cursor out of the hidden subtree.
    fn cnr_expand(&self, hwnd: u32, rec: u32, expand: bool) -> bool {
        let Some(targets) = self.with_cnr(hwnd, |s, _| {
            if rec == 0 {
                let parents: HashSet<u32> = s.records.iter().map(|&(_, p)| p).filter(|&p| p != 0).collect();
                Some(s.records.iter().map(|&(r, _)| r).filter(|r| parents.contains(r)).collect::<Vec<_>>())
            } else if s.is_inserted(rec) {
                Some(vec![rec])
            } else { None }
        }).flatten() else { return false };
        let code = if expand { CN_EXPANDTREE } else { CN_COLLAPSETREE };
        for r in targets {
            let a = self.rec_attr(r);
            let new = if expand { (a | CRA_EXPANDED) & !CRA_COLLAPSED } else { (a | CRA_COLLAPSED) & !CRA_EXPANDED };
            if new == a { continue; }
            self.set_rec_attr(r, new);
            if !expand {
                let hidden = self.with_cnr(hwnd, |s, _| s.removal(&[r]).unwrap_or_default()).unwrap_or_default();
                if hidden.iter().skip(1).any(|&d| self.rec_attr(d) & CRA_CURSORED != 0) {
                    self.cnr_set_emphasis(hwnd, r, true, CRA_CURSORED);
                }
            }
            self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
                control: hwnd, kind: OwnerCallKind::ContainerValue { code: code as u16, mp2: r },
            });
        }
        self.cnr_clamp_top(hwnd);
        self.repaint_control(hwnd);
        true
    }

    /// Start sorting the records (`only`: the children of one parent) with
    /// the application's function `pfn`: a call chain asking for each
    /// comparison, then `ret` (`keep`: also when the records changed
    /// meanwhile and the order was left alone).
    fn cnr_start_sort(&self, hwnd: u32, pfn: u32, storage: u32, only: Option<u32>, ret: u32, keep: bool) -> ApiResult {
        let started = self.with_cnr(hwnd, |s, _| {
            let id = s.new_sort_id();
            let mut job = SortJob::new(id, pfn, storage, s.sibling_groups(only));
            match job.step(None) {
                Some(pair) => { s.sort = Some(job); Some((id, pair)) }
                None => { s.sort = None; s.apply_sorted(&job.into_groups()); None }
            }
        });
        match started {
            None => ApiResult::Normal(0),
            Some(None) => { self.repaint_control(hwnd); ApiResult::Normal(ret) }
            Some(Some((job, (a, b)))) => self.start_chain(sort_chain(hwnd, job, pfn, storage, (a, b), ret, keep)),
        }
    }

    /// A comparison of sort `job` returned `result` (a `SHORT`): the chain
    /// asking for the next one, or the end of the sort with its result.
    pub(crate) fn container_sort_step(&self, hwnd: u32, job: u32, ret: u32, keep: bool, result: u32) -> ChainEnd {
        enum Next { Ask(u32, u32, (u32, u32)), Done(bool) }
        let next = self.with_cnr(hwnd, |s, _| {
            let j = s.sort.as_mut().filter(|j| j.id == job)?;
            let (pfn, storage) = (j.pfn, j.storage);
            Some(match j.step(Some(result as u16 as i16)) {
                Some(pair) => Next::Ask(pfn, storage, pair),
                None => {
                    let groups = s.sort.take().map(SortJob::into_groups).unwrap_or_default();
                    Next::Done(s.apply_sorted(&groups))
                }
            })
        }).flatten();
        match next {
            Some(Next::Ask(pfn, storage, pair)) => ChainEnd::Next(Box::new(sort_chain(hwnd, job, pfn, storage, pair, ret, keep))),
            Some(Next::Done(applied)) => {
                self.cnr_autoposition(hwnd);
                self.repaint_control(hwnd);
                ChainEnd::Return(if applied || keep { ret } else { 0 })
            }
            // The container is gone or another sort replaced this one.
            None => ChainEnd::Return(if keep { ret } else { 0 }),
        }
    }

    /// `CM_FILTER`: ask the application's function `pfn` about every
    /// record (a call chain, ADR-0026); those it answers FALSE get
    /// `CRA_FILTERED`. Without a function every record is shown again.
    fn cnr_start_filter(&self, hwnd: u32, pfn: u32, storage: u32) -> ApiResult {
        let Some((id, recs)) = self.with_cnr(hwnd, |s, _| {
            let id = s.new_sort_id();
            let recs = s.all_records();
            s.filter = (pfn != 0 && !recs.is_empty()).then(|| (id, recs.clone()));
            (id, recs)
        }) else { return ApiResult::Normal(0) };
        if pfn == 0 || recs.is_empty() {
            for r in recs {
                let a = self.rec_attr(r);
                if a & CRA_FILTERED != 0 { self.set_rec_attr(r, a & !CRA_FILTERED); }
            }
            self.cnr_autoposition(hwnd);
            self.cnr_clamp_top(hwnd);
            self.repaint_control(hwnd);
            return ApiResult::Normal(1);
        }
        let steps = recs.iter().map(|&r| CallStep::hook(pfn, vec![r, storage], false)).collect();
        self.start_chain(CallChain::new(super::interrupt::current_vcpu().unwrap_or(0), steps, false,
                                        ChainThen::ContainerFilter { hwnd, job: id }))
    }

    /// Filter `job` of container `hwnd` got its answers (`results`, one
    /// per record asked about): mark the records, show the rest.
    pub(crate) fn container_filter_done(&self, hwnd: u32, job: u32, results: &[u32]) -> ChainEnd {
        let Some(Some(recs)) = self.with_cnr(hwnd, |s, _| match s.filter.take() {
            Some((id, recs)) if id == job => Some(recs.into_iter().zip(results).filter(|&(r, _)| s.is_inserted(r)).map(|(r, &k)| (r, k)).collect::<Vec<_>>()),
            other => { s.filter = other; None }
        }) else { return ChainEnd::Return(0) };
        for (r, keep) in recs {
            let a = self.rec_attr(r);
            let new = if keep != 0 { a & !CRA_FILTERED } else { a | CRA_FILTERED };
            if new != a { self.set_rec_attr(r, new); }
        }
        self.cnr_autoposition(hwnd);
        self.cnr_clamp_top(hwnd);
        self.repaint_control(hwnd);
        ChainEnd::Return(1)
    }

    /// Scroll so that record `rec` is shown.
    fn cnr_follow(&self, hwnd: u32, rec: u32) {
        let Some(snap) = self.snapshot(hwnd) else { return };
        let Some(cursor) = snap.rows.iter().position(|&(r, _)| r == rec) else { return };
        let font = Painter::new(self, 0, hwnd).font();
        if snap.view() == View::Icon {
            let Some((_, a, b)) = self.icon_items(&snap, font).into_iter().find(|&(i, _, _)| i == cursor) else { return };
            let (_, ib, _, ih) = union(a, b);
            let (_, ry, _, rh) = snap.frame().rows;
            let pos = i64::from(snap.icon_scroll().0);
            if ib < ry { self.cnr_scroll_icons(hwnd, &snap, pos + i64::from(ry - ib)); }
            else if ib + ih > ry + rh { self.cnr_scroll_icons(hwnd, &snap, pos - i64::from(ib + ih - ry - rh)); }
            return;
        }
        let (f, placed) = self.placements(hwnd, &snap, font);
        let flow = snap.flow();
        let top = scroll_to(snap.top, cursor, f.capacity(), flow, &placed, f.rows.0 + f.rows.2);
        self.with_cnr(hwnd, |s, _| s.top = top);
    }

    /// The shown record at container point `local`, or 0 (white space).
    pub(crate) fn container_record_at(&self, hwnd: u32, local: (i32, i32)) -> u32 {
        self.snapshot(hwnd).and_then(|snap| self.record_at(hwnd, &snap, local).map(|i| snap.rows[i].0)).unwrap_or(0)
    }

    /// What the owner of container `hwnd` is told of a drag at container
    /// point `local`: the notification (`CN_DRAGOVER`, or `CN_DRAGAFTER`
    /// with `CA_ORDEREDTARGETEMPH` / `CA_MIXEDTARGETEMPH` in the name, text
    /// and details views), its record (after which the drop goes for
    /// `CN_DRAGAFTER`; `CMA_FIRST` before the first), and the emphasis
    /// drawn if the owner accepts.
    pub(crate) fn container_drag_target(&self, hwnd: u32, local: (i32, i32)) -> (u32, u32, TargetEmph) {
        let Some(snap) = self.snapshot(hwnd) else { return (CN_DRAGOVER, 0, TargetEmph::Whole) };
        let attr = snap.info.window_attr;
        let ordered = attr & (CA_ORDEREDTARGETEMPH | CA_MIXEDTARGETEMPH) != 0
            && matches!(snap.view(), View::Name | View::Text | View::Detail);
        if !ordered {
            let rec = self.record_at(hwnd, &snap, local).map_or(0, |i| snap.rows[i].0);
            return (CN_DRAGOVER, rec, if rec == 0 { TargetEmph::Whole } else { TargetEmph::Record(rec) });
        }
        let font = Painter::new(self, 0, hwnd).font();
        let (_, placed) = self.placements(hwnd, &snap, font);
        // A flowed view: the column of rows under the pointer.
        let in_column = |r: &Rect| local.0 >= r.0 && local.0 < r.0 + r.2;
        let column = placed.iter().any(|(_, r)| in_column(r));
        let mut rows: Vec<(usize, i32, i32)> = placed.iter().filter(|(_, r)| !column || in_column(r))
            .map(|&(i, r)| (i, r.1, r.3)).collect();
        rows.sort_by_key(|r| r.0);
        match ordered_spot(&rows, local.1, attr & CA_MIXEDTARGETEMPH != 0) {
            Ok(i) => (CN_DRAGOVER, snap.rows[i].0, TargetEmph::Record(snap.rows[i].0)),
            Err(Some(i)) => (CN_DRAGAFTER, snap.rows[i].0, TargetEmph::After(snap.rows[i].0)),
            Err(None) => (CN_DRAGAFTER, CMA_FIRST, TargetEmph::After(CMA_FIRST)),
        }
    }

    /// Show target emphasis `t` in container `hwnd` (`None`: take it away).
    pub(crate) fn set_container_target(&self, hwnd: u32, t: Option<TargetEmph>) {
        if self.with_cnr(hwnd, |s, _| std::mem::replace(&mut s.target, t) != t) == Some(true) {
            self.repaint_control(hwnd);
        }
    }

    /// `CN_INITDRAG` for a drag starting at container point `mp1`: the
    /// record there (or NULL), the pointer in desktop coordinates and its
    /// offset to the record's origin.
    fn cnr_init_drag(&self, hwnd: u32, mp1: u32) -> bool {
        let at = ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32);
        let Some(snap) = self.snapshot(hwnd) else { return false };
        let font = Painter::new(self, 0, hwnd).font();
        let (_, placed) = self.placements(hwnd, &snap, font);
        let hit = placed.iter().find(|&&(_, (x, y, w, h))| at.0 >= x && at.0 < x + w && at.1 >= y && at.1 < y + h);
        let (rec, origin) = hit.map_or((0, at), |&(i, (x, y, _, _))| (snap.rows[i].0, (x, y)));
        let (ox, oy) = self.shared.window_mgr.lock_or_recover().window_origin(hwnd).unwrap_or((0, 0));
        let block = [hwnd, rec, (ox + at.0) as u32, (oy + at.1) as u32,
                     (origin.0 - at.0) as u32, (origin.1 - at.1) as u32];
        self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
            control: hwnd, kind: OwnerCallKind::InitDrag { code: CN_INITDRAG, block },
        });
        true
    }

    /// `CN_ENTER` with a `NOTIFYRECORDENTER` (`fKey`: from the keyboard).
    fn cnr_enter(&self, hwnd: u32, key: bool, rec: u32) {
        self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
            control: hwnd, kind: OwnerCallKind::ContainerNotify { code: CN_ENTER as u16, block: [hwnd, key as u32, rec] },
        });
    }

    /// Make shown row `i` the cursored record and select as the style and
    /// keys `kc` ask (`toggle`: the multiple-selection Space / click).
    fn cnr_pick(&self, hwnd: u32, style: u32, rows: &[(u32, u32)], i: usize, kc: u32, toggle: bool) {
        let rec = rows[i].0;
        let mode = sel_mode(style);
        let (shift, ctrl) = (kc & KC_SHIFT != 0, kc & KC_CTRL != 0);
        if self.rec_attr(rec) & CRA_DISABLED == 0 {
            match mode {
                SelMode::Single => self.cnr_set_emphasis(hwnd, rec, true, CRA_SELECTED),
                SelMode::Multiple => if toggle {
                    let on = self.rec_attr(rec) & CRA_SELECTED == 0;
                    self.cnr_set_emphasis(hwnd, rec, on, CRA_SELECTED);
                },
                SelMode::Extended => {
                    let anchor = self.with_cnr(hwnd, |s, _| s.anchor).flatten()
                        .and_then(|a| rows.iter().position(|&(r, _)| r == a));
                    if shift && let Some(a) = anchor {
                        let (lo, hi) = (a.min(i), a.max(i));
                        for (k, &(r, _)) in rows.iter().enumerate() {
                            self.cnr_set_emphasis(hwnd, r, (lo..=hi).contains(&k), CRA_SELECTED);
                        }
                    } else if ctrl {
                        if toggle {
                            let on = self.rec_attr(rec) & CRA_SELECTED == 0;
                            self.cnr_set_emphasis(hwnd, rec, on, CRA_SELECTED);
                        }
                    } else {
                        for &(r, _) in rows.iter().filter(|&&(r, _)| r != rec) {
                            self.cnr_set_emphasis(hwnd, r, false, CRA_SELECTED);
                        }
                        self.cnr_set_emphasis(hwnd, rec, true, CRA_SELECTED);
                    }
                    if !shift { self.with_cnr(hwnd, |s, _| s.anchor = Some(rec)); }
                }
            }
        }
        self.cnr_set_emphasis(hwnd, rec, true, CRA_CURSORED);
        self.cnr_follow(hwnd, rec);
        self.repaint_control(hwnd);
    }

    fn cnr_click(&self, hwnd: u32, style: u32, mp1: u32, mp2: u32) -> bool {
        if style & WS_DISABLED != 0 { return false; }
        self.set_focus_window(hwnd);
        let Some(snap) = self.snapshot(hwnd) else { return false };
        let at = ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32);
        if let Some(i) = self.record_at(hwnd, &snap, at) {
            // A tree record's expander box opens or closes it.
            if snap.view() == View::Tree && snap.tree[i].1 {
                let ex = BORDER + snap.expander_x(i);
                if (ex - 2..=ex + snap.expander_w() + 1).contains(&at.0) {
                    let open = self.rec_attr(snap.rows[i].0) & CRA_EXPANDED != 0;
                    return self.cnr_expand(hwnd, snap.rows[i].0, !open);
                }
            }
            self.cnr_pick(hwnd, style, &snap.rows, i, mp2 >> 16, true);
        } else {
            self.repaint_control(hwnd);
        }
        true
    }

    /// Keys for a focused container: Up / Down (Left / Right by a column in
    /// a flowed view), PageUp / PageDown, Home / End move the cursor (and
    /// the selection in single and extended mode); Space toggles in
    /// multiple and extended mode; Enter sends `CN_ENTER`.
    fn cnr_char(&self, hwnd: u32, style: u32, mp1: u32, mp2: u32) -> bool {
        let fs = mp1 & 0xFFFF;
        if fs & KC_KEYUP != 0 || style & WS_DISABLED != 0 { return false; }
        let Some(snap) = self.snapshot(hwnd) else { return false };
        let cur = snap.rows.iter().position(|&(r, _)| self.rec_attr(r) & CRA_CURSORED != 0);
        if snap.view() == View::Tree && let Some(i) = cur && let Some(done) = self.cnr_tree_key(hwnd, style, &snap, i, fs, mp2) {
            return done;
        }
        if fs & KC_VIRTUALKEY == 0 { return false; }
        let vk = mp2 >> WM_CHAR_VKEY_SHIFT;
        if matches!(vk, VK_ENTER | VK_NEWLINE) {
            self.cnr_enter(hwnd, true, cur.map_or(0, |i| snap.rows[i].0));
            return true;
        }
        if snap.rows.is_empty() { return matches!(vk, VK_UP | VK_DOWN | VK_HOME | VK_END | VK_PAGEUP | VK_PAGEDOWN); }
        let f = snap.frame();
        let flow = snap.flow();
        let (cap, last, c) = (f.capacity(), snap.rows.len() - 1, cur.unwrap_or(0));
        let target = match vk {
            VK_SPACE => {
                if let Some(i) = cur && sel_mode(style) != SelMode::Single {
                    let on = self.rec_attr(snap.rows[i].0) & CRA_SELECTED == 0;
                    self.cnr_set_emphasis(hwnd, snap.rows[i].0, on, CRA_SELECTED);
                    self.repaint_control(hwnd);
                }
                return true;
            }
            VK_UP => c.saturating_sub(usize::from(cur.is_some())),
            VK_DOWN => if cur.is_some() { (c + 1).min(last) } else { 0 },
            VK_LEFT if flow => c.saturating_sub(cap),
            VK_RIGHT if flow => (c + cap).min(last),
            // The icon view moves through the items in order.
            VK_LEFT if snap.view() == View::Icon => c.saturating_sub(usize::from(cur.is_some())),
            VK_RIGHT if snap.view() == View::Icon => if cur.is_some() { (c + 1).min(last) } else { 0 },
            VK_PAGEUP => c.saturating_sub(cap.saturating_sub(1).max(1)),
            VK_PAGEDOWN => (c + cap.saturating_sub(1).max(1)).min(last),
            VK_HOME => 0,
            VK_END => last,
            _ => return false,
        };
        let kc = fs & (KC_SHIFT | KC_CTRL);
        // Arrow keys move the selection with the cursor except with Ctrl
        // (extended) or in multiple selection.
        let moves_selection = sel_mode(style) == SelMode::Single
            || (sel_mode(style) == SelMode::Extended && kc & KC_CTRL == 0);
        if moves_selection {
            self.cnr_pick(hwnd, style, &snap.rows, target, kc, false);
        } else {
            self.cnr_set_emphasis(hwnd, snap.rows[target].0, true, CRA_CURSORED);
            self.cnr_follow(hwnd, snap.rows[target].0);
            self.repaint_control(hwnd);
        }
        true
    }

    /// Tree keys on the cursored row `i`: `+` / Right expand (Right on an
    /// expanded record goes to its first child), `-` / Left collapse (Left
    /// on a collapsed or childless record goes to its parent). `None` for
    /// other keys.
    fn cnr_tree_key(&self, hwnd: u32, style: u32, snap: &Snapshot, i: usize, fs: u32, mp2: u32) -> Option<bool> {
        let rec = snap.rows[i].0;
        let (depth, kids) = snap.tree[i];
        let open = self.rec_attr(rec) & CRA_EXPANDED != 0;
        let ch = if fs & KC_CHAR != 0 { mp2 & 0xFFFF } else { 0 };
        let vk = if fs & KC_VIRTUALKEY != 0 { mp2 >> WM_CHAR_VKEY_SHIFT } else { 0 };
        let expand = ch == u32::from(b'+') || vk == VK_RIGHT;
        let collapse = ch == u32::from(b'-') || vk == VK_LEFT;
        if expand {
            if kids && !open { return Some(self.cnr_expand(hwnd, rec, true)); }
            if kids && vk == VK_RIGHT && i + 1 < snap.rows.len() {
                self.cnr_pick(hwnd, style, &snap.rows, i + 1, 0, false);
            }
            return Some(true);
        }
        if collapse {
            if kids && open { return Some(self.cnr_expand(hwnd, rec, false)); }
            if vk == VK_LEFT && depth > 0
                && let Some(p) = (0..i).rev().find(|&k| snap.tree[k].0 < depth) {
                self.cnr_pick(hwnd, style, &snap.rows, p, 0, false);
            }
            return Some(true);
        }
        None
    }

    /// Show, place and set the vertical scroll bar of container `hwnd` for
    /// `snap`, or hide it; it paints itself over the container afterwards.
    fn cnr_sync_scrollbar(&self, hwnd: u32, snap: &Snapshot) {
        self.cnr_sync_hscrollbars(hwnd, snap);
        let f = snap.frame();
        let bar = snap.bar();
        let n = snap.rows.len();
        let cap = f.capacity();
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(sb) = wm.find_child_by_id(hwnd, CID_VSCROLL) else { return };
        let Some(w) = wm.get_window_mut(sb) else { return };
        w.visible = bar;
        if !bar { return; }
        (w.x, w.y, w.cx, w.cy) = (snap.cx - BORDER - SCROLLBAR_W, f.rows.1, SCROLLBAR_W, f.rows.3);
        if snap.view() == View::Icon {
            // Pels: the slider shows the work area's share of the workspace.
            let (pos, max) = snap.icon_scroll();
            let (lo, hi) = snap.extent();
            let c16 = |v: i32| v.clamp(0, i32::from(i16::MAX)) as i16;
            w.scrollbar.set_range(c16(pos), 0, c16(max));
            w.scrollbar.thumb_size = Some((c16(f.rows.3) as u16, c16(hi - lo) as u16));
            wm.invalidate_window_only(sb);
            return;
        }
        let clamp = |v: usize| v.min(i16::MAX as usize) as i16;
        w.scrollbar.set_range(clamp(snap.top), 0, clamp(snap.max_top()));
        w.scrollbar.thumb_size = Some((cap.min(u16::MAX as usize) as u16, n.min(u16::MAX as usize) as u16));
        wm.invalidate_window_only(sb);
    }

    /// Show, place and set the horizontal scroll bars: one under each
    /// pane of the details view, one under the icon view's work area.
    fn cnr_sync_hscrollbars(&self, hwnd: u32, snap: &Snapshot) {
        let shown = snap.bars().1;
        let f = snap.frame();
        let c16 = |v: i32| v.clamp(0, i32::from(i16::MAX)) as i16;
        // (x, width, position, most, content) of each bar.
        let spans: Vec<(i32, i32, i32, i32, i32)> = match snap.view() {
            _ if !shown => Vec::new(),
            View::Detail => snap.panes().iter().map(|p| (p.x, p.w, p.off, (p.content - p.w).max(0), p.content)).collect(),
            View::Icon => {
                let (pos, max) = snap.icon_xscroll();
                vec![(f.rows.0, f.rows.2, pos, max, max + f.rows.2)]
            }
            _ => Vec::new(),
        };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        for (k, id) in [CID_HSCROLL, CID_RIGHTHSCROLL].into_iter().enumerate() {
            let Some(sb) = wm.find_child_by_id(hwnd, id) else { continue };
            let Some(w) = wm.get_window_mut(sb) else { continue };
            w.visible = k < spans.len();
            let Some(&(x, width, pos, max, content)) = spans.get(k) else { continue };
            (w.x, w.y, w.cx, w.cy) = (x, BORDER, width, SCROLLBAR_W);
            w.scrollbar.set_range(c16(pos), 0, c16(max));
            w.scrollbar.thumb_size = Some((c16(width) as u16, c16(content) as u16));
            wm.invalidate_window_only(sb);
        }
    }

    /// The cursored record (`CRA_CURSORED`) among the shown ones, or 0.
    fn cnr_cursored(&self, hwnd: u32) -> u32 {
        self.snapshot(hwnd).and_then(|snap| snap.rows.iter().map(|&(r, _)| r)
            .find(|&r| self.rec_attr(r) & CRA_CURSORED != 0)).unwrap_or(0)
    }

    /// `CM_QUERYVIEWPORTRECT(CMA_WORKSPACE)`: the workspace in workspace
    /// coordinates. The icon view's spans everything its items cover and
    /// the work area at the origin; the other views' is their content from
    /// (0, 0): the rows' height, and the columns' width in the details view.
    fn cnr_workspace(&self, hwnd: u32, snap: &Snapshot) -> Option<super::pm_rect::Rectl> {
        let f = snap.frame();
        if snap.view() == View::Icon {
            let (yl, yh) = snap.extent();
            let (xl, xh) = icon_extent(snap.xspan, f.rows.2);
            return Some(super::pm_rect::Rectl { x_left: xl, y_bottom: yl, x_right: xh, y_top: yh });
        }
        let font = Painter::new(self, 0, hwnd).font();
        let (_, placed) = self.placements(hwnd, snap, font);
        let width = if snap.view() == View::Detail {
            snap.panes().iter().map(|p| p.content).fold(0i32, i32::saturating_add)
        } else {
            placed.iter().map(|&(_, r)| r.0 + r.2 - f.rows.0).max().unwrap_or(0).max(f.rows.2)
        };
        let height = (snap.rows.len() as i64 * i64::from(f.row_h)).min(i64::from(i32::MAX)) as i32;
        Some(super::pm_rect::Rectl { x_left: 0, y_bottom: 0, x_right: width, y_top: height })
    }

    /// Button 1 pressed at container point `mp1`: on the split bar it is
    /// taken, to be dragged until the button is released.
    fn cnr_split_press(&self, hwnd: u32, mp1: u32) -> bool {
        let at = ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32);
        let Some((sx, sy, sw, sh)) = self.snapshot(hwnd).and_then(|s| s.split_rect()) else { return false };
        if !(at.0 >= sx && at.0 < sx + sw && at.1 >= sy && at.1 < sy + sh) { return false; }
        self.with_cnr(hwnd, |c, _| c.split_drag = Some(at.0 - sx));
        let mut wm = self.shared.window_mgr.lock_or_recover();
        wm.pressed_control = hwnd;
        wm.drag_pointer = at;
        true
    }

    /// The pointer moved to container point `mp1` while the split bar is
    /// held: move it there (`CNRINFO.xVertSplitbar`, from the rows' left
    /// edge, kept within the rows). FALSE when the bar is not held.
    fn cnr_split_drag(&self, hwnd: u32, mp1: u32) -> bool {
        let at = ((mp1 & 0xFFFF) as i16 as i32, (mp1 >> 16) as i16 as i32);
        let Some(Some(grab)) = self.with_cnr(hwnd, |c, _| c.split_drag) else { return false };
        if self.shared.window_mgr.lock_or_recover().pressed_control != hwnd { return false; }
        let Some(snap) = self.snapshot(hwnd) else { return false };
        let f = snap.frame();
        // At least MIN_PANE: 0 and below would mean "the middle".
        let x = split_x((at.0 - grab - f.rows.0).max(MIN_PANE), f.rows.2);
        self.with_cnr(hwnd, |c, _| c.info.split_bar = x as u32);
        self.repaint_control(hwnd);
        true
    }

    /// Details view: scroll pane `pane` so that `to` pels (clamped) are
    /// left of it; icon view (pane 0): the work area across. The owner
    /// hears `CN_SCROLL` (`CMA_HORIZONTAL`, the pels scrolled).
    fn cnr_hscroll(&self, hwnd: u32, snap: &Snapshot, pane: usize, to: i64) -> bool {
        let (pos, max) = match snap.view() {
            View::Detail => match snap.panes().get(pane) {
                Some(p) => (p.off, (p.content - p.w).max(0)),
                None => return false,
            },
            View::Icon if pane == 0 => snap.icon_xscroll(),
            _ => return false,
        };
        let new = to.clamp(0, i64::from(max)) as i32;
        if new == pos { return true; }
        self.cnr_close_edit(hwnd, false);
        let delta = new - pos;
        self.with_cnr(hwnd, |s, _| {
            if view_of(s.info.window_attr) == View::Icon {
                s.info.origin.0 = (s.info.origin.0 as i32).saturating_add(delta) as u32;
            } else {
                s.hscroll[pane] = new;
            }
        });
        self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
            control: hwnd, kind: OwnerCallKind::ContainerNotify { code: CN_SCROLL as u16, block: [hwnd, delta as u32, CMA_HORIZONTAL] },
        });
        self.repaint_control(hwnd);
        true
    }

    /// Draw handle `h` (a bitmap, else an icon) stretched to `rect`
    /// (surface coordinates `(x, y, cx, cy)`).
    fn paint_picture(&self, p: &Painter, h: u32, bitmap: bool, rect: Rect) {
        if bitmap { self.paint_bitmap_handle(p, rect, h, true); } else { self.paint_icon_handle(p, rect, h, true); }
    }

    /// Draw record `rec`'s picture (see [`picture_fields`]) in `rect`.
    fn paint_record_picture(&self, p: &Painter, rec: u32, snap: &Snapshot, rect: Rect) {
        let (fields, bitmap) = picture_fields(snap.info.window_attr, snap.mini);
        let h = fields.iter().map(|&o| self.guest_read::<u32>(rec.wrapping_add(o)).unwrap_or(0)).find(|&h| h != 0);
        if let Some(h) = h { self.paint_picture(p, h, bitmap, rect); }
    }

    /// The handle a `CFA_BITMAPORICON` field info `fi` finds in record
    /// `rec` of `size` bytes (none outside the record, or 0).
    fn field_handle(&self, rec: u32, size: u32, fi: u32) -> Option<u32> {
        let off = self.guest_read::<u32>(fi.wrapping_add(FI_OFFSTRUCT))?;
        if off.checked_add(4).is_none_or(|end| end > size) { return None; }
        self.guest_read::<u32>(rec.wrapping_add(off)).filter(|&h| h != 0)
    }

    /// Paint container `hwnd` at `(x, y, cx, cy)` in its frame's surface.
    /// Items the owner draws (`CA_OWNERDRAW`: every record's icon and
    /// text; `CFA_OWNER`: the cells of that details column) get their
    /// background and are queued as owner calls (`WM_DRAWITEM`).
    pub(super) fn paint_container(&self, p: &Painter, hwnd: u32, rect: (i32, i32, i32, i32)) {
        self.paint_container_parts(p, hwnd, rect, None);
    }

    /// Paint one part of record `rec` (`fi`: its details-view column,
    /// `part`: `CMA_ICON` / `CMA_TEXT`) as the container draws it: what an
    /// owner left to the container by answering FALSE to `WM_DRAWITEM`.
    pub(crate) fn paint_container_item(&self, p: &Painter, hwnd: u32, rect: (i32, i32, i32, i32), rec: u32, fi: u32, part: u32) {
        self.paint_container_parts(p, hwnd, rect, Some((rec, fi, part)));
    }

    /// Paint the whole container, or with `only` just that item part
    /// (see [`Self::paint_container_item`]).
    fn paint_container_parts(&self, p: &Painter, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32),
                             only: Option<(u32, u32, u32)>) {
        let Some(snap) = self.snapshot(hwnd) else { return };
        let full = only.is_none();
        let want = |rec: u32, fi: u32, part: u32| only.is_none_or(|o| o == (rec, fi, part));
        let font = p.font();
        let attr = snap.info.window_attr;
        let view = view_of(attr);
        let owner_all = full && attr & CA_OWNERDRAW != 0;
        let mut owner_items: Vec<(u32, u32, u32)> = Vec::new();
        let bg = p.own_bg.unwrap_or(FIELD_COLOR);
        let at = |(rx, ry, rw, rh): (i32, i32, i32, i32)| (x + rx, y + ry, x + rx + rw - 1, y + ry + rh - 1);
        if full {
            // The scroll bars first: a bar this paint hides must not keep
            // its place out of the painting.
            self.cnr_sync_scrollbar(hwnd, &snap);
            p.fill((x, y, x + cx - 1, y + cy - 1), bg);
            // The border.
            p.fill((x, y, x + cx - 1, y), BORDER_COLOR);
            p.fill((x, y + cy - 1, x + cx - 1, y + cy - 1), BORDER_COLOR);
            p.fill((x, y, x, y + cy - 1), BORDER_COLOR);
            p.fill((x + cx - 1, y, x + cx - 1, y + cy - 1), BORDER_COLOR);
        }
        let (f, placed) = self.placements(hwnd, &snap, font);
        let fg = p.fg;
        let aligned = |l: i32, r: i32, w: i32, fl: u32| {
            if fl & CFA_RIGHT != 0 { r - CELL_PAD - w } else if fl & CFA_CENTER != 0 { l + (r - l + 1 - w) / 2 } else { l + CELL_PAD }
        };
        // The container title.
        if let Some(t) = f.title.filter(|_| full) {
            let title = self.cnr_text(hwnd, snap.info.title);
            let (l, b, r, _) = at(t);
            let w = crate::gui::font::text_width(&title, font);
            let tx = if attr & CA_TITLELEFT != 0 { l + CELL_PAD }
                     else if attr & CA_TITLERIGHT != 0 { r - CELL_PAD - w } else { l + (r - l + 1 - w) / 2 };
            let sep = (attr & CA_TITLESEPARATOR != 0) as i32;
            p.text(tx, b + sep + ROW_PAD, &title, fg);
            if sep != 0 { p.fill((l, b, r, b), SEPARATOR_COLOR); }
        }
        let focused = self.shared.window_mgr.lock_or_recover().focus_hwnd == hwnd;
        let columns = if view == View::Detail { snap.columns() } else { Vec::new() };
        let panes = if view == View::Detail { snap.panes() } else { Vec::new() };
        // A column's cell from `l` to `r` (surface x), cut to its pane;
        // `None` when none of it shows.
        let in_pane = |c: &Column, l: i32, r: i32| -> Option<(i32, i32)> {
            let pn = panes.get(c.pane)?;
            let (pl, pr) = (l.max(x + pn.x), r.min(x + pn.x + pn.w - 1));
            (pl <= pr).then_some((pl, pr))
        };
        // Detail column titles.
        if let Some(hd) = f.header.filter(|_| full) {
            let (l, b, r, _) = at(hd);
            for c in &columns {
                let (cl, cr) = (x + c.x, x + c.x + c.w - 1);
                let Some((vl, vr)) = in_pane(c, cl, cr + 1) else { continue };
                let title = if c.fl_title & CFA_BITMAPORICON == 0 {
                    self.cnr_text(hwnd, self.guest_read::<u32>(c.fi.wrapping_add(FI_TITLE)).unwrap_or(0))
                } else { String::new() };
                let cp = p.clone_clipped((vl, b, (vr.min(cr) - vl + 1).max(0), hd.3));
                cp.text(aligned(cl, cr, crate::gui::font::text_width(&title, font), c.fl_title), b + 1 + ROW_PAD, &title, fg);
                if c.fl_data & CFA_SEPARATOR != 0 && vr > cr { p.fill((cr + 1, y + f.rows.1, cr + 1, b + hd.3 - 1), SEPARATOR_COLOR); }
            }
            if columns.iter().any(|c| (c.fl_title | c.fl_data) & CFA_HORZSEPARATOR != 0) || !columns.is_empty() {
                p.fill((l, b, r, b), SEPARATOR_COLOR);
            }
        }
        // The records.
        let rows_clip = at(f.rows);
        let rp = p.clone_clipped((rows_clip.0, rows_clip.1, f.rows.2, f.rows.3));
        if view == View::Icon {
            // Icons at their places, each text centred under its icon.
            let surf = |(rx, ry, rw, rh): Rect| (x + rx, y + ry, rw, rh);
            for (i, ir, tr) in self.icon_items(&snap, font) {
                let rec = snap.rows[i].0;
                if only.is_some_and(|o| o.0 != rec) || snap.erased.contains(&rec) { continue; }
                let a = self.rec_attr(rec);
                let selected = a & CRA_SELECTED != 0;
                let color = if selected { SELECT_TEXT_COLOR } else if a & CRA_DISABLED != 0 { DISABLED_TEXT_COLOR } else { fg };
                let (l, b, r, t) = at(tr);
                if owner_all {
                    if selected { rp.fill((l, b, r, t), SELECT_COLOR); }
                    owner_items.extend([(rec, 0, CMA_ICON), (rec, 0, CMA_TEXT)]);
                    continue;
                }
                if want(rec, 0, CMA_ICON) { self.paint_record_picture(&rp, rec, &snap, surf(ir)); }
                if !want(rec, 0, CMA_TEXT) { continue; }
                if selected { rp.fill((l, b, r, t), SELECT_COLOR); }
                for (k, line) in snap.icons[i].1.iter().enumerate() {
                    let lw = crate::gui::font::text_width(line, font);
                    rp.text(l + (r - l + 1 - lw) / 2, t + 1 - ROW_PAD - (k as i32 + 1) * LINE_H, line, color);
                }
                if focused && a & CRA_CURSORED != 0 { super::pm_button::focus_mark(&rp, (l, b, r, t), color); }
            }
        }
        // Tree lines (CA_TREELINE), cxTreeLine pels wide (1 by default).
        let lines = if full && view == View::Tree && attr & CA_TREELINE != 0 {
            tree_lines(&snap.tree.iter().map(|t| t.0).collect::<Vec<_>>())
        } else { Vec::new() };
        let line_w = (snap.info.tree_line as i32).clamp(1, 8);
        let ew = snap.expander_w();
        for &(i, r) in placed.iter().filter(|_| view != View::Icon) {
            let (rec, size) = snap.rows[i];
            if only.is_some_and(|o| o.0 != rec) || snap.erased.contains(&rec) { continue; }
            if let Some(&(mask, has_next)) = lines.get(i) {
                let (l, b, _, t) = at(r);
                let depth = i32::from(snap.tree[i].0.min(63));
                let level_x = |k: i32| l + k * snap.indent() + CELL_PAD + ew / 2;
                let y_mid = b + r.3 / 2;
                let vline = |x: i32, lo: i32, hi: i32| rp.fill((x, lo, x + line_w - 1, hi), SEPARATOR_COLOR);
                for k in 0..depth.saturating_sub(1) {
                    if mask & (1 << k) != 0 { vline(level_x(k), b, t); }
                }
                if depth > 0 {
                    let x = level_x(depth - 1);
                    vline(x, if has_next { b } else { y_mid }, t);
                    let end = l + snap.expander_x(i) + if snap.tree[i].1 { -1 } else { ew - 1 };
                    rp.fill((x, y_mid, end.max(x), y_mid + line_w - 1), SEPARATOR_COLOR);
                }
            }
            let a = self.rec_attr(rec);
            let (l, b, rr, t) = at(r);
            let selected = a & CRA_SELECTED != 0;
            let color = if selected { SELECT_TEXT_COLOR } else if a & CRA_DISABLED != 0 { DISABLED_TEXT_COLOR } else { fg };
            if view == View::Detail {
                let mut right = l;
                for c in &columns {
                    let (cl, cr) = (x + c.x, x + c.x + c.w - 1);
                    let Some((vl, vr)) = in_pane(c, cl, cr) else { continue };
                    right = right.max(vr);
                    if !want(rec, c.fi, CMA_TEXT) { continue; }
                    if selected { rp.fill((vl, b, vr, t), SELECT_COLOR); }
                    if full && (owner_all || c.fl_data & CFA_OWNER != 0) {
                        owner_items.push((rec, c.fi, CMA_TEXT));
                        continue;
                    }
                    let cell = (vl, b, vr - vl + 1, r.3);
                    if c.fl_data & CFA_BITMAPORICON != 0 {
                        // An icon (or bitmap) handle in the record, drawn
                        // in a square the height of a line.
                        if let Some(h) = self.field_handle(rec, size, c.fi) {
                            let cp = rp.clone_clipped(cell);
                            let side = LINE_H.min(c.w);
                            let pic = (cl + (c.w - side) / 2, b + (r.3 - side) / 2, side, side);
                            self.paint_picture(&cp, h, attr & CA_DRAWBITMAP != 0 && attr & CA_DRAWICON == 0, pic);
                        }
                        continue;
                    }
                    let v = self.field_value(hwnd, rec, size, c.fi, c.fl_data);
                    let cp = rp.clone_clipped(cell);
                    cp.text(aligned(cl, cr, crate::gui::font::text_width(&v, font), c.fl_data), b + ROW_PAD, &v, color);
                }
                if full && focused && a & CRA_CURSORED != 0 { super::pm_button::focus_mark(&rp, (l, b, right, t), color); }
            } else {
                let text = self.record_text(hwnd, rec, view, snap.mini);
                let row_l = l;
                if let Some(ix) = snap.icon_inset(i) {
                    if owner_all {
                        owner_items.push((rec, 0, CMA_ICON));
                    } else if want(rec, 0, CMA_ICON) {
                        let (iw, ih) = snap.icon();
                        self.paint_record_picture(&rp, rec, &snap, (row_l + ix, b + (r.3 - ih) / 2, iw, ih));
                    }
                }
                let l = l + snap.text_inset(i);
                let ty = if snap.pictures() { b + (r.3 - LINE_H) / 2 } else { b + ROW_PAD };
                if full && view == View::Tree && snap.tree[i].1
                    && let Some((h, bitmap)) = snap.tree_pics.get(i).copied().flatten() {
                    // The expander as the record's (or CNRINFO's) picture.
                    let ex = row_l + snap.expander_x(i);
                    self.paint_picture(&rp, h, bitmap, (ex, b + (r.3 - ew) / 2, ew, ew));
                } else if full && view == View::Tree && snap.tree[i].1 {
                    // The expander: a box with a minus, or a plus when closed.
                    let ex = row_l + snap.expander_x(i) + (ew - EXPANDER) / 2;
                    let (eb, et) = (b + (r.3 - EXPANDER) / 2, b + (r.3 - EXPANDER) / 2 + EXPANDER - 1);
                    let (er, mid_x, mid_y) = (ex + EXPANDER - 1, ex + EXPANDER / 2, eb + EXPANDER / 2);
                    for edge in [(ex, eb, er, eb), (ex, et, er, et), (ex, eb, ex, et), (er, eb, er, et)] {
                        rp.fill(edge, SEPARATOR_COLOR);
                    }
                    rp.fill((ex + 2, mid_y, er - 2, mid_y), fg);
                    if a & CRA_EXPANDED == 0 { rp.fill((mid_x, eb + 2, mid_x, et - 2), fg); }
                }
                let w = crate::gui::font::text_width(&text, font) + 2 * CELL_PAD;
                let right = (l + w - 1).min(rr);
                if owner_all {
                    if selected { rp.fill((l, b, rr, t), SELECT_COLOR); }
                    owner_items.push((rec, 0, CMA_TEXT));
                    continue;
                }
                if !want(rec, 0, CMA_TEXT) { continue; }
                if selected { rp.fill((l, b, right, t), SELECT_COLOR); }
                rp.text(l + CELL_PAD, ty, &text, color);
                if focused && a & CRA_CURSORED != 0 { super::pm_button::focus_mark(&rp, (l, b, right, t), color); }
            }
        }
        if !full { return; }
        // Column separators through the rows.
        for c in columns.iter().filter(|c| c.fl_data & CFA_SEPARATOR != 0) {
            let cr = x + c.x + c.w;
            if in_pane(c, cr, cr).is_some() { rp.fill((cr, rows_clip.1, cr, rows_clip.3), SEPARATOR_COLOR); }
        }
        // Target emphasis: the drag's (when its owner accepts) and records
        // with CRA_TARGET, an outline; CN_DRAGAFTER's, a line.
        let outline = |(l, b, r, t): (i32, i32, i32, i32)| {
            for edge in [(l, b, r, b), (l, t, r, t), (l, b, l, t), (r, b, r, t)] { rp.fill(edge, TARGET_COLOR); }
        };
        let target = self.with_cnr(hwnd, |s, _| s.target).flatten();
        for &(i, r) in &placed {
            let rec = snap.rows[i].0;
            if target == Some(TargetEmph::Record(rec)) || self.rec_attr(rec) & CRA_TARGET != 0 { outline(at(r)); }
            let (l, b, rr, t) = at(r);
            match target {
                Some(TargetEmph::After(a)) if a == rec => rp.fill((l, b, rr, b + TARGET_LINE - 1), TARGET_COLOR),
                Some(TargetEmph::After(CMA_FIRST)) if i == 0 => rp.fill((l, t - TARGET_LINE + 1, rr, t), TARGET_COLOR),
                _ => {}
            }
        }
        if target == Some(TargetEmph::Whole) { outline(rows_clip); }
        // The split bar between the panes.
        if let Some(sr) = snap.split_rect() {
            let (l, b, r, t) = at(sr);
            p.fill((l, b, r, t), super::CONTROL_FACE_COLOR);
            p.fill((l, b, l, t), SEPARATOR_COLOR);
            p.fill((r, b, r, t), SEPARATOR_COLOR);
        }
        // The owner draws its items now, before the call that painted
        // returns (the drawing so far stays when it answers FALSE).
        if !owner_items.is_empty() {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            wm.owner_calls.retain(|c| !(c.control == hwnd && matches!(c.kind, OwnerCallKind::ContainerDrawItem { .. })));
            for (rec, fi, part) in owner_items {
                wm.queue_owner_call(OwnerCall { control: hwnd, kind: OwnerCallKind::ContainerDrawItem { rec, fi, part } });
            }
        }
    }

    /// Where part `part` (`CMA_ICON` / `CMA_TEXT`) of shown record `rec`
    /// (`fi`: its details-view cell) is, in container coordinates, right
    /// and top exclusive, cut to the rows' area (and the cell's pane).
    pub(crate) fn container_item_rect(&self, hwnd: u32, rec: u32, fi: u32, part: u32) -> Option<(i32, i32, i32, i32)> {
        let snap = self.snapshot(hwnd)?;
        let i = snap.rows.iter().position(|&(r, _)| r == rec)?;
        let font = Painter::new(self, 0, hwnd).font();
        let f = snap.frame();
        let r: Rect = match snap.view() {
            View::Icon => {
                let (_, ir, tr) = self.icon_items(&snap, font).into_iter().find(|&(k, _, _)| k == i)?;
                if part == CMA_ICON { ir } else { tr }
            }
            view => {
                let (_, placed) = self.placements(hwnd, &snap, font);
                let &(_, row) = placed.iter().find(|&&(k, _)| k == i)?;
                if view == View::Detail {
                    let c = snap.columns().into_iter().find(|c| c.fi == fi)?;
                    let pn = snap.panes().get(c.pane).copied()?;
                    let (l, rr) = (c.x.max(pn.x), (c.x + c.w).min(pn.x + pn.w));
                    (l, row.1, rr - l, row.3)
                } else if part == CMA_ICON {
                    let (iw, ih) = snap.icon();
                    (row.0 + snap.icon_inset(i)?, row.1 + (row.3 - ih) / 2, iw, ih)
                } else {
                    let inset = snap.text_inset(i);
                    (row.0 + inset, row.1, row.2 - inset, row.3)
                }
            }
        };
        let (rx, ry, rw, rh) = f.rows;
        let (l, b) = (r.0.max(rx), r.1.max(ry));
        let (rr, t) = ((r.0 + r.2).min(rx + rw), (r.1 + r.3).min(ry + rh));
        (l < rr && b < t).then_some((l, b, rr, t))
    }

    /// The `flRecordAttr` of record `rec`.
    pub(crate) fn container_record_attr(&self, rec: u32) -> u32 { self.rec_attr(rec) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::pm_types::OS2Message;
    use crate::loader::pm_hook::CallStep;


    struct Fx {
        loader: Loader,
        hmq: u32,
        cnr: u32,
    }

    const ID: u32 = 70;
    /// Scratch guest memory for structures and arrays.
    const BUF: u32 = 0x3000;

    /// A 300 × 200 container of `style` at (10, 10) in a client.
    fn fixture(style: u32) -> Fx {
        let loader = Loader::new_mock();
        let (hmq, cnr) = {
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
            let h = wm.create_window(CLASS_CONTAINER.into(), client, hmq);
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy, w.style, w.id, w.owner, w.visible) =
                (10, 10, 300, 200, WS_VISIBLE | style, ID, client, true);
            wm.build_container(h);
            (hmq, h)
        };
        Fx { loader, hmq, cnr }
    }

    impl Fx {
        fn send(&self, msg: u32, mp1: u32, mp2: u32) -> u32 {
            match self.loader.dispatch_builtin_control(self.cnr, msg, mp1, mp2) {
                ApiResult::Normal(v) => v,
                other => panic!("unexpected {other:?}"),
            }
        }
        fn rd(&self, a: u32) -> u32 { self.loader.guest_read::<u32>(a).unwrap() }
        /// Set the view attributes (`CM_SETCNRINFO` `CMA_FLWINDOWATTR`).
        fn set_view(&self, attr: u32) {
            for i in 0..23 { self.wr(BUF + 0x600 + 4 * i, 0); }
            self.wr(BUF + 0x600 + 20, attr);
            assert_eq!(self.send(CM_SETCNRINFO, BUF + 0x600, CMA_FLWINDOWATTR), 1);
        }
        fn wr(&self, a: u32, v: u32) { self.loader.guest_write::<u32>(a, v).unwrap(); }
        /// Allocate `n` records with `extra` bytes; their addresses.
        fn alloc(&self, n: u32, extra: u32) -> Vec<u32> {
            let first = self.send(CM_ALLOCRECORD, extra, n);
            assert_ne!(first, 0);
            let mut out = vec![first];
            while out.len() < n as usize {
                out.push(self.rd(out.last().unwrap() + RC_NEXT));
            }
            out
        }
        /// Insert the chain from `first` (`n` records) at `order`.
        fn insert(&self, first: u32, n: u32, order: u32, parent: u32) -> u32 {
            for (o, v) in [(0, 24), (RI_ORDER, order), (RI_PARENT, parent), (RI_INVALIDATE, 1), (16, 0), (RI_COUNT, n)] {
                self.wr(BUF + o, v);
            }
            self.send(CM_INSERTRECORD, first, BUF)
        }
        fn array(&self, recs: &[u32]) -> u32 {
            for (i, &r) in recs.iter().enumerate() { self.wr(BUF + 0x100 + 4 * i as u32, r); }
            BUF + 0x100
        }
        fn emphasis_notes(&self) -> Vec<(u32, u32)> {
            let mut wm = self.loader.shared.window_mgr.lock_or_recover();
            let calls: Vec<OwnerCall> = std::mem::take(&mut wm.owner_calls);
            calls.into_iter().filter_map(|c| match c.kind {
                OwnerCallKind::ContainerNotify { code, block } if u32::from(code) == CN_EMPHASIS => Some((block[1], block[2])),
                _ => None,
            }).collect()
        }
        fn enter_notes(&self) -> Vec<[u32; 3]> {
            let mut wm = self.loader.shared.window_mgr.lock_or_recover();
            let calls: Vec<OwnerCall> = std::mem::take(&mut wm.owner_calls);
            calls.into_iter().filter_map(|c| match c.kind {
                OwnerCallKind::ContainerNotify { code, block } if u32::from(code) == CN_ENTER => Some(block),
                _ => None,
            }).collect()
        }
        fn key(&self, vk: u32, extra: u32) -> u32 {
            self.send(WM_CHAR, KC_VIRTUALKEY | extra, vk << WM_CHAR_VKEY_SHIFT)
        }
        fn at(x: i32, y: i32) -> u32 { (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16) }
        fn posted(&self) -> Vec<(u32, u32)> {
            let wm = self.loader.shared.window_mgr.lock_or_recover();
            let mq = wm.get_mq(self.hmq).unwrap();
            let msgs: Vec<OS2Message> = mq.lock_or_recover().messages.drain(..).collect();
            msgs.into_iter().filter(|m| m.msg == WM_CONTROL).map(|m| (m.mp1 >> 16, m.mp2)).collect()
        }
    }

    #[test]
    fn records_are_allocated_zeroed_chained_and_sized() {
        let f = fixture(0);
        let r = f.alloc(3, 10);
        assert_eq!(r[1] - r[0], (RECORDCORE_SIZE + 10).next_multiple_of(4), "core plus the application's bytes");
        assert_eq!(f.rd(r[0]), RECORDCORE_SIZE, "cb is the core size");
        assert_eq!((f.rd(r[0] + RC_ATTR), f.rd(r[2] + RC_NEXT), f.rd(r[0] + RC_PSZTEXT)), (0, 0, 0));
        let mini = fixture(CCS_MINIRECORDCORE);
        let m = mini.alloc(2, 0);
        assert_eq!((mini.rd(m[0]), m[1] - m[0]), (MINIRECORDCORE_SIZE, MINIRECORDCORE_SIZE));
        assert_eq!(f.send(CM_ALLOCRECORD, MAX_RECORD_DATA + 1, 1), 0, "too much application data");
        assert_eq!(f.send(CM_ALLOCRECORD, 0, 0), 0, "no records");
        let fi = f.send(CM_ALLOCDETAILFIELDINFO, 2, 0);
        assert_eq!((f.rd(fi), f.rd(f.rd(fi + FI_NEXT)), f.rd(f.rd(fi + FI_NEXT) + FI_NEXT)), (FIELDINFO_SIZE, FIELDINFO_SIZE, 0));
    }

    #[test]
    fn many_single_records_share_an_arena() {
        let f = fixture(0);
        let before = f.loader.shared.mem_mgr.lock_or_recover().next_free;
        let recs: Vec<u32> = (0..500).map(|_| f.send(CM_ALLOCRECORD, 8, 1)).collect();
        let used = f.loader.shared.mem_mgr.lock_or_recover().next_free - before;
        assert_eq!(used, CNR_ARENA_BYTES, "500 records of 64 bytes in one arena");
        assert!(recs.windows(2).all(|w| w[1] == w[0] + 64));
        // Freeing them all gives the arena back.
        assert_eq!(f.send(CM_FREERECORD, f.array(&recs[..100]), 100), 1);
        for chunk in recs[100..].chunks(100) {
            assert_eq!(f.send(CM_FREERECORD, f.array(chunk), chunk.len() as u32), 1);
        }
        assert_eq!(f.loader.shared.mem_mgr.lock_or_recover().next_free, before);
    }

    #[test]
    fn insert_query_remove_and_free() {
        let f = fixture(0);
        let r = f.alloc(4, 0);
        assert_eq!(f.insert(r[0], 3, CMA_END, 0), 3);
        assert_eq!(f.insert(r[3], 1, CMA_FIRST, 0), 4, "the chain stops at the count");
        assert_eq!(f.insert(r[0], 1, CMA_END, 0), 0, "inserted already");
        assert_eq!(f.send(CM_QUERYRECORD, 0, CMA_FIRST), r[3]);
        assert_eq!(f.send(CM_QUERYRECORD, r[3], CMA_NEXT), r[0]);
        assert_eq!(f.send(CM_QUERYRECORD, 0x4444, CMA_NEXT), u32::MAX);
        assert_eq!(f.send(CM_FREERECORD, f.array(&[r[1]]), 1), 0, "inserted records cannot be freed");
        // Remove two and free them; two remain.
        assert_eq!(f.send(CM_REMOVERECORD, f.array(&[r[1], r[2]]), 2 | u32::from(CMA_FREE) << 16), 2);
        assert_eq!(f.send(CM_QUERYRECORD, 0, CMA_LAST), r[0]);
        assert_eq!(f.insert(r[1], 1, CMA_END, 0), 0, "freed: no longer a record");
        // Remove all.
        assert_eq!(f.send(CM_REMOVERECORD, 0, 0), 0);
        assert_eq!(f.send(CM_QUERYRECORD, 0, CMA_FIRST), 0);
    }

    #[test]
    fn single_selection_moves_and_notifies() {
        let f = fixture(0);
        let r = f.alloc(3, 0);
        f.insert(r[0], 3, CMA_END, 0);
        f.send(CM_SETRECORDEMPHASIS, r[0], 1 | CRA_SELECTED << 16);
        assert_eq!(f.emphasis_notes(), vec![(r[0], CRA_SELECTED)]);
        f.send(CM_SETRECORDEMPHASIS, r[2], 1 | CRA_SELECTED << 16);
        assert_eq!(f.emphasis_notes(), vec![(r[0], CRA_SELECTED), (r[2], CRA_SELECTED)], "the old one loses it");
        assert_eq!(f.rd(r[0] + RC_ATTR) & CRA_SELECTED, 0);
        assert_eq!(f.send(CM_QUERYRECORDEMPHASIS, CMA_FIRST, CRA_SELECTED), r[2]);
        assert_eq!(f.send(CM_QUERYRECORDEMPHASIS, r[2], CRA_SELECTED), 0);
        assert_eq!(f.send(CM_SETRECORDEMPHASIS, 0x5555, 1 | CRA_SELECTED << 16), 0, "not a record");
    }

    #[test]
    fn clicks_and_keys_select_and_enter() {
        let f = fixture(CCS_EXTENDSEL);
        let r = f.alloc(4, 0);
        f.insert(r[0], 4, CMA_END, 0);
        f.send(CM_SETCNRINFO, {
            for i in 0..23 { f.wr(BUF + 0x200 + 4 * i, 0); }
            f.wr(BUF + 0x200 + 20, CV_TEXT);
            BUF + 0x200
        }, CMA_FLWINDOWATTR);
        // Rows from the top: 200 - 1 border, 18 pels each; row 1 is y 163..180.
        f.send(WM_BUTTON1DOWN, Fx::at(20, 170), 0);
        let attr = |i: usize| f.rd(r[i] + RC_ATTR);
        assert_eq!(attr(1) & (CRA_SELECTED | CRA_CURSORED), CRA_SELECTED | CRA_CURSORED);
        assert_eq!(f.posted(), vec![(CN_SETFOCUS, f.cnr)]);
        // Shift+Down twice extends the range from the anchor.
        f.key(VK_DOWN, KC_SHIFT);
        f.key(VK_DOWN, KC_SHIFT);
        let selected: Vec<bool> = (0..4).map(|i| attr(i) & CRA_SELECTED != 0).collect();
        assert_eq!(selected, vec![false, true, true, true]);
        assert_ne!(attr(3) & CRA_CURSORED, 0);
        f.emphasis_notes();
        // Enter reports the cursored record from the keyboard; a double
        // click the one under the pointer.
        f.key(VK_NEWLINE, 0);
        assert_eq!(f.enter_notes(), vec![[f.cnr, 1, r[3]]]);
        f.send(WM_BUTTON1DBLCLK, Fx::at(20, 190), 0);
        assert_eq!(f.enter_notes(), vec![[f.cnr, 0, r[0]]]);
    }

    #[test]
    fn cnrinfo_and_detail_fields_through_messages() {
        let f = fixture(0);
        for i in 0..23 { f.wr(BUF + 4 * i, 0); }
        f.wr(BUF + 20, CV_DETAIL | CA_DETAILSVIEWTITLES);
        f.wr(BUF + 16, 0x7000);
        assert_eq!(f.send(CM_SETCNRINFO, BUF, CMA_FLWINDOWATTR | CMA_CNRTITLE), 1);
        let fi = f.send(CM_ALLOCDETAILFIELDINFO, 2, 0);
        let fi2 = f.rd(fi + FI_NEXT);
        for (o, v) in [(0, 16), (FII_ORDER, CMA_END), (FII_INVALIDATE, 1), (FII_COUNT, 2)] { f.wr(BUF + 0x80 + o, v); }
        assert_eq!(f.send(CM_INSERTDETAILFIELDINFO, fi, BUF + 0x80), 2);
        assert_eq!(f.send(CM_QUERYDETAILFIELDINFO, fi, CMA_NEXT), fi2);
        assert_eq!(f.send(CM_FREEDETAILFIELDINFO, f.array(&[fi]), 1), 0, "inserted");
        let r = f.alloc(1, 0);
        f.insert(r[0], 1, CMA_END, 0);
        assert_eq!(f.send(CM_QUERYCNRINFO, BUF + 0x400, CNRINFO_SIZE), CNRINFO_SIZE);
        assert_eq!((f.rd(BUF + 0x400 + 16), f.rd(BUF + 0x400 + 20), f.rd(BUF + 0x400 + 36), f.rd(BUF + 0x400 + 84)),
                   (0x7000, CV_DETAIL | CA_DETAILSVIEWTITLES, 1, 2));
        assert_eq!(f.send(CM_QUERYCNRINFO, BUF + 0x400, 8), 8, "only what fits");
        assert_eq!(f.send(CM_REMOVEDETAILFIELDINFO, f.array(&[fi]), 1 | u32::from(CMA_FREE) << 16), 1);
        assert_eq!(f.send(CM_QUERYDETAILFIELDINFO, 0, CMA_FIRST), fi2);
    }

    #[test]
    fn field_values_stay_inside_their_record() {
        let f = fixture(0);
        let r = f.alloc(1, 12)[0];
        let fi = f.send(CM_ALLOCDETAILFIELDINFO, 1, 0);
        let size = RECORDCORE_SIZE + 12;
        f.wr(fi + FI_OFFSTRUCT, RECORDCORE_SIZE);
        f.wr(r + RECORDCORE_SIZE, 4096);
        assert_eq!(f.loader.field_value(f.cnr, r, size, fi, CFA_ULONG), "4096");
        f.wr(fi + FI_OFFSTRUCT, RECORDCORE_SIZE + 4);
        f.loader.guest_write_bytes(r + RECORDCORE_SIZE + 4, &[9, 3, 0xEA, 0x07]).unwrap();
        assert_eq!(f.loader.field_value(f.cnr, r, size, fi, CFA_DATE).len(), 10);
        f.wr(fi + FI_OFFSTRUCT, size - 2);
        assert_eq!(f.loader.field_value(f.cnr, r, size, fi, CFA_ULONG), "", "past the record's end");
        f.wr(fi + FI_OFFSTRUCT, u32::MAX - 1);
        assert_eq!(f.loader.field_value(f.cnr, r, size, fi, CFA_ULONG), "");
    }

    #[test]
    fn destroying_the_container_gives_its_arenas_back() {
        let f = fixture(0);
        let before = f.loader.shared.mem_mgr.lock_or_recover().next_free;
        let r = f.alloc(2, 0);
        f.insert(r[0], 2, CMA_END, 0);
        assert!(f.loader.shared.mem_mgr.lock_or_recover().next_free > before);
        f.loader.shared.window_mgr.lock_or_recover().destroy_window(f.cnr);
        f.loader.free_pending_guest_blocks();
        assert_eq!(f.loader.shared.mem_mgr.lock_or_recover().next_free, before);
    }

    #[test]
    fn search_finds_by_prefix_or_substring() {
        let f = fixture(0);
        let r = f.alloc(3, 0);
        f.insert(r[0], 3, CMA_END, 0);
        for (i, t) in ["Alpha", "beta", "Gamma alpha"].iter().enumerate() {
            let at = BUF + 0x600 + 0x20 * i as u32;
            f.loader.guest_write_bytes(at, format!("{t}\0").as_bytes()).unwrap();
            f.wr(r[i] + RC_PSZTEXT, at);
        }
        f.loader.guest_write_bytes(BUF + 0x700, b"alpha\0").unwrap();
        for (o, v) in [(0, 20), (SS_PSZ, BUF + 0x700), (SS_PREFIX, 1), (SS_CASE, 0), (SS_VIEW, CV_TEXT)] { f.wr(BUF + 0x780 + o, v); }
        assert_eq!(f.send(CM_SEARCHSTRING, BUF + 0x780, CMA_FIRST), r[0]);
        assert_eq!(f.send(CM_SEARCHSTRING, BUF + 0x780, r[0]), 0, "no other prefix match");
        f.wr(BUF + 0x780 + SS_PREFIX, 0);
        assert_eq!(f.send(CM_SEARCHSTRING, BUF + 0x780, r[0]), r[2], "substring");
        f.wr(BUF + 0x780 + SS_CASE, 1);
        assert_eq!(f.send(CM_SEARCHSTRING, BUF + 0x780, CMA_FIRST), r[2], "case-sensitive");
    }


    /// Run sort `r` (a call chain or an immediate result) to its end,
    /// answering each comparison as `cmp` says; the result and how many
    /// comparisons were asked.
    fn run_sort(f: &Fx, r: ApiResult, cmp: &dyn Fn(u32, u32) -> i16) -> (u32, usize) {
        use crate::loader::pm_hook::ChainEnd;
        let mut chain = match r {
            ApiResult::Normal(v) => return (v, 0),
            ApiResult::CallChain(c) => c,
            other => panic!("{other:?}"),
        };
        let mut asked = 0;
        loop {
            let CallStep::Guest { proc_addr, args, .. } = chain.steps.pop_front().unwrap() else { panic!() };
            assert_eq!((proc_addr, args.len(), args[2]), (SORT_PROC, 3, SORT_STORAGE));
            asked += 1;
            chain.returned(cmp(args[0], args[1]) as u16 as u32, false);
            match f.loader.finish_chain(&chain) {
                ChainEnd::Next(next) => chain = next,
                ChainEnd::Return(v) => return (v, asked),
                other => panic!("{other:?}"),
            }
        }
    }

    const SORT_PROC: u32 = 0x0050_0000;
    const SORT_STORAGE: u32 = 0x1234;

    fn order(f: &Fx) -> Vec<u32> {
        let mut out = Vec::new();
        let mut r = f.send(CM_QUERYRECORD, 0, CMA_FIRST);
        while r != 0 && r != u32::MAX {
            out.push(r);
            r = f.send(CM_QUERYRECORD, r, CMA_NEXT);
        }
        out
    }

    #[test]
    fn the_sort_job_is_a_stable_merge_sort() {
        let keys: Vec<u32> = vec![5, 3, 9, 3, 1, 8, 2, 7, 3, 0, 6];
        // Records 100.. carry keys; equal keys keep their order.
        let recs: Vec<u32> = (0..keys.len() as u32).map(|i| 100 + i).collect();
        let key = |r: u32| keys[(r - 100) as usize];
        let mut job = SortJob::new(1, 0, 0, vec![(0, recs.clone()), (7, vec![200]), (8, vec![])]);
        let mut answer = None;
        let mut asked = 0;
        while let Some((a, b)) = job.step(answer) {
            asked += 1;
            answer = Some(key(a).cmp(&key(b)) as i16);
        }
        assert!(asked <= keys.len() * 4, "n log n comparisons, got {asked}");
        let groups = job.into_groups();
        let mut want = recs.clone();
        want.sort_by_key(|&r| key(r));
        assert_eq!(groups.iter().find(|g| g.0 == 0).unwrap().1, want);
        assert_eq!(groups.len(), 3, "short groups pass through");
        let mut empty = SortJob::new(2, 0, 0, vec![(0, vec![1])]);
        assert_eq!(empty.step(None), None);
    }

    #[test]
    fn tree_order_follows_expansion_and_filters() {
        let records = [(1, 0), (2, 0), (11, 1), (12, 1), (111, 11), (21, 2)];
        let all = tree_order(&records, &|_| true, &|_| true);
        assert_eq!(all, vec![(1, 0, true), (11, 1, true), (111, 2, false), (12, 1, false), (2, 0, true), (21, 1, false)]);
        let closed = tree_order(&records, &|r| r == 1, &|r| r != 12);
        assert_eq!(closed, vec![(1, 0, true), (11, 1, true), (2, 0, true)], "11 and 2 closed, 12 filtered");
    }

    #[test]
    fn sorted_groups_go_back_into_their_places() {
        let (mut s, r) = heap_with(5);
        s.insert(&r[0..3], CMA_END, 0).unwrap();
        s.insert(&r[3..5], CMA_END, r[0]).unwrap();
        let groups = s.sibling_groups(None);
        assert_eq!(groups, vec![(0, r[0..3].to_vec()), (r[0], r[3..5].to_vec())]);
        assert!(s.apply_sorted(&[(0, vec![r[2], r[0], r[1]]), (r[0], vec![r[4], r[3]])]));
        assert_eq!(s.records, vec![(r[2], 0), (r[0], 0), (r[1], 0), (r[4], r[0]), (r[3], r[0])]);
        assert!(!s.apply_sorted(&[(0, vec![r[0], r[1]])]), "a group that changed");
        assert_eq!(s.sibling_groups(Some(r[0])), vec![(r[0], vec![r[4], r[3]])]);
    }

    /// Records whose ULONG after the core is `keys[i]`, inserted in order.
    fn keyed(f: &Fx, keys: &[u32]) -> Vec<u32> {
        let r = f.alloc(keys.len() as u32, 4);
        for (i, &k) in keys.iter().enumerate() { f.wr(r[i] + RECORDCORE_SIZE, k); }
        f.insert(r[0], keys.len() as u32, CMA_END, 0);
        r
    }

    #[test]
    fn cm_sortrecord_asks_the_application_for_each_comparison() {
        let f = fixture(0);
        let r = keyed(&f, &[40, 10, 30, 20]);
        let cmp = |a: u32, b: u32| (f.rd(a + RECORDCORE_SIZE) as i32 - f.rd(b + RECORDCORE_SIZE) as i32) as i16;
        let (ret, asked) = run_sort(&f, f.loader.dispatch_builtin_control(f.cnr, CM_SORTRECORD, SORT_PROC, SORT_STORAGE), &cmp);
        assert_eq!(ret, 1);
        assert!(asked >= 3);
        assert_eq!(order(&f), vec![r[1], r[3], r[2], r[0]]);
        // One record: no call at all.
        let g = fixture(0);
        keyed(&g, &[1]);
        assert!(matches!(g.loader.dispatch_builtin_control(g.cnr, CM_SORTRECORD, SORT_PROC, 0), ApiResult::Normal(1)));
        assert_eq!(g.send(CM_SORTRECORD, 0, 0), 0, "no function");
    }

    #[test]
    fn a_sort_function_in_cnrinfo_sorts_on_insertion() {
        let f = fixture(0);
        for i in 0..23 { f.wr(BUF + 0x200 + 4 * i, 0); }
        f.wr(BUF + 0x200 + 4, SORT_PROC);
        f.send(CM_SETCNRINFO, BUF + 0x200, CMA_PSORTRECORD);
        let r = f.alloc(3, 4);
        for (i, k) in [3u32, 1, 2].iter().enumerate() { f.wr(r[i] + RECORDCORE_SIZE, *k); }
        for (o, v) in [(0, 24), (RI_ORDER, CMA_END), (RI_PARENT, 0), (RI_INVALIDATE, 1), (16, 0), (RI_COUNT, 3)] {
            f.wr(BUF + o, v);
        }
        let cmp = |a: u32, b: u32| (f.rd(a + RECORDCORE_SIZE) as i32 - f.rd(b + RECORDCORE_SIZE) as i32) as i16;
        let res = f.loader.dispatch_builtin_control(f.cnr, CM_INSERTRECORD, r[0], BUF);
        let ApiResult::CallChain(c) = &res else { panic!("expected a sort chain") };
        let Some(CallStep::Guest { proc_addr, args, .. }) = c.steps.front() else { panic!() };
        assert_eq!((*proc_addr, args[2]), (SORT_PROC, 0), "pStorage is NULL when inserting");
        let (count, _) = run_sort_any_storage(&f, res, &cmp);
        assert_eq!(count, 3, "the insertion count");
        assert_eq!(order(&f), vec![r[1], r[2], r[0]]);
    }

    /// [`run_sort`] without checking `pStorage`.
    fn run_sort_any_storage(f: &Fx, r: ApiResult, cmp: &dyn Fn(u32, u32) -> i16) -> (u32, usize) {
        use crate::loader::pm_hook::ChainEnd;
        let mut chain = match r {
            ApiResult::Normal(v) => return (v, 0),
            ApiResult::CallChain(c) => c,
            other => panic!("{other:?}"),
        };
        let mut asked = 0;
        loop {
            let CallStep::Guest { args, .. } = chain.steps.pop_front().unwrap() else { panic!() };
            asked += 1;
            chain.returned(cmp(args[0], args[1]) as u16 as u32, false);
            match f.loader.finish_chain(&chain) {
                ChainEnd::Next(next) => chain = next,
                ChainEnd::Return(v) => return (v, asked),
                other => panic!("{other:?}"),
            }
        }
    }

    const FILTER_PROC: u32 = 0x6100;

    #[test]
    fn cm_filter_asks_about_every_record_and_hides_the_rejected() {
        let f = fixture(0);
        let r = keyed(&f, &[1, 2, 3, 4]);
        // A child of the first record is asked about too.
        let kid = f.alloc(1, 4)[0];
        f.wr(kid + RECORDCORE_SIZE, 6);
        f.insert(kid, 1, CMA_END, r[0]);
        let res = f.loader.dispatch_builtin_control(f.cnr, CM_FILTER, FILTER_PROC, 0x77);
        let ApiResult::CallChain(mut chain) = res else { panic!("expected a filter chain") };
        let mut asked = Vec::new();
        while let Some(CallStep::Guest { proc_addr, args, .. }) = chain.steps.pop_front() {
            assert_eq!((proc_addr, args[1]), (FILTER_PROC, 0x77), "the function with pStorage");
            asked.push(args[0]);
            // Keep the even keys.
            chain.returned(u32::from(f.rd(args[0] + RECORDCORE_SIZE).is_multiple_of(2)), false);
        }
        assert_eq!(asked, vec![r[0], r[1], r[2], r[3], kid]);
        assert_eq!(f.loader.finish_chain(&chain), crate::loader::pm_hook::ChainEnd::Return(1));
        let filtered: Vec<bool> = asked.iter().map(|&x| f.rd(x + RC_ATTR) & CRA_FILTERED != 0).collect();
        assert_eq!(filtered, vec![true, false, true, false, false]);
        // CMA_FILTER passes over filtered records; plain queries do not.
        assert_eq!(f.send(CM_QUERYRECORD, 0, CMA_FIRST | CMA_FILTER), r[1]);
        assert_eq!(f.send(CM_QUERYRECORD, 0, CMA_FIRST), r[0]);
        assert_eq!(f.send(CM_QUERYRECORD, r[1], CMA_NEXT | CMA_FILTER), r[3]);
        assert_eq!(f.send(CM_QUERYRECORD, 0, CMA_LAST | CMA_FILTER), r[3]);
        assert_eq!(f.send(CM_QUERYRECORD, r[3], CMA_PREV | CMA_FILTER), r[1]);
        assert_eq!(f.send(CM_QUERYRECORD, r[0], u32::from(CMA_FIRSTCHILD) | CMA_FILTER), kid);
        // The shown rows are the even ones.
        let shown: Vec<u32> = f.loader.snapshot(f.cnr).unwrap().rows.iter().map(|&(x, _)| x).collect();
        assert_eq!(shown, vec![r[1], r[3]]);
        // No function: everything is shown again, at once.
        assert_eq!(f.send(CM_FILTER, 0, 0), 1);
        assert!(asked.iter().all(|&x| f.rd(x + RC_ATTR) & CRA_FILTERED == 0));
    }

    #[test]
    fn a_stale_filter_marks_nothing_and_skips_removed_records() {
        let f = fixture(0);
        let r = keyed(&f, &[1, 2, 3]);
        let ApiResult::CallChain(mut chain) = f.loader.dispatch_builtin_control(f.cnr, CM_FILTER, FILTER_PROC, 0)
        else { panic!() };
        // The second record is removed while the application answers.
        f.send(CM_REMOVERECORD, f.array(&[r[1]]), 1);
        for _ in 0..3 { chain.steps.pop_front(); chain.returned(0, false); }
        assert_eq!(f.loader.finish_chain(&chain), crate::loader::pm_hook::ChainEnd::Return(1));
        assert_eq!(f.rd(r[1] + RC_ATTR) & CRA_FILTERED, 0, "a removed record is left alone");
        assert_ne!(f.rd(r[2] + RC_ATTR) & CRA_FILTERED, 0, "answers stay with their records");
        // A filter replaced by a newer one does nothing when it ends.
        let ApiResult::CallChain(old) = f.loader.dispatch_builtin_control(f.cnr, CM_FILTER, FILTER_PROC, 0) else { panic!() };
        let _new = f.loader.dispatch_builtin_control(f.cnr, CM_FILTER, FILTER_PROC, 0);
        assert_eq!(f.loader.finish_chain(&old), crate::loader::pm_hook::ChainEnd::Return(0));
    }

    #[test]
    fn records_removed_during_a_sort_leave_the_order_alone() {
        let f = fixture(0);
        let r = keyed(&f, &[3, 2, 1]);
        let ApiResult::CallChain(mut chain) = f.loader.dispatch_builtin_control(f.cnr, CM_SORTRECORD, SORT_PROC, SORT_STORAGE)
        else { panic!() };
        // The comparison function removes a record.
        f.send(CM_REMOVERECORD, f.array(&[r[1]]), 1);
        use crate::loader::pm_hook::ChainEnd;
        let mut steps = 0;
        let ret = loop {
            chain.steps.clear();
            chain.returned(1, false);
            steps += 1;
            match f.loader.finish_chain(&chain) {
                ChainEnd::Next(n) => chain = n,
                ChainEnd::Return(v) => break v,
                other => panic!("{other:?}"),
            }
            assert!(steps < 20);
        };
        assert_eq!(ret, 0, "not applied");
        assert_eq!(order(&f), vec![r[0], r[2]]);
    }

    const CMA_PARTIAL: u32 = 0x2000;
    const CMA_ITEMORDER: u32 = 0x0001;
    const FOLDER: u32 = SYS_HPOINTER_BASE + SPTR_FOLDER;

    impl Fx {
        /// The drawing operations of one paint of the container.
        fn paint_ops(&self) -> Vec<crate::gui::surface::DrawOp> {
            let s = {
                let mut wm = self.loader.shared.window_mgr.lock_or_recover();
                let frame = wm.find_frame_for_hwnd(self.cnr);
                wm.frame_surface(frame).unwrap()
            };
            s.lock_or_recover().record = Some(Vec::new());
            self.loader.dispatch_builtin_control(self.cnr, WM_PAINT, 0, 0);
            s.lock_or_recover().record.take().unwrap()
        }
        fn blits(&self) -> Vec<(i32, i32, u32, u32)> {
            self.paint_ops().into_iter().filter_map(|o| match o {
                crate::gui::surface::DrawOp::Blit { x, y, w, h, src: Some(_), .. } => Some((x, y, w, h)),
                _ => None,
            }).collect()
        }
        /// Records with texts `texts` (pszIcon / pszText / pszName / pszTree
        /// all the same) and icon `hptr`, inserted at the end.
        fn named(&self, texts: &[&str], hptr: u32) -> Vec<u32> {
            let r = self.alloc(texts.len() as u32, 0);
            for (i, t) in texts.iter().enumerate() {
                let at = BUF + 0x800 + 0x40 * i as u32;
                self.loader.guest_write_bytes(at, format!("{t}\0").as_bytes()).unwrap();
                for o in [RC_PSZICON, RC_PSZTEXT, RC_PSZNAME, RC_PSZTREE] { self.wr(r[i] + o, at); }
                self.wr(r[i] + RC_HPTRICON, hptr);
            }
            self.insert(r[0], texts.len() as u32, CMA_END, 0);
            r
        }
        fn place(&self, rec: u32, x: i32, y: i32) {
            self.wr(rec + RC_PTLX, x as u32);
            self.wr(rec + RC_PTLY, y as u32);
        }
        fn ptl(&self, rec: u32) -> (i32, i32) { (self.rd(rec + RC_PTLX) as i32, self.rd(rec + RC_PTLY) as i32) }
        fn notes(&self) -> Vec<(u32, [u32; 7])> {
            let mut wm = self.loader.shared.window_mgr.lock_or_recover();
            std::mem::take(&mut wm.owner_calls).into_iter().filter_map(|c| match c.kind {
                OwnerCallKind::ContainerEdit { code, block } => Some((u32::from(code), block)),
                _ => None,
            }).collect()
        }
    }

    #[test]
    fn icon_sizes_pictures_and_their_handles() {
        let info = |attr: u32| CnrInfo { window_attr: attr, ..CnrInfo::default() };
        assert_eq!(icon_size(&info(CV_ICON), false), (32, 32));
        assert_eq!(icon_size(&info(CV_ICON | CV_MINI), false), (16, 16));
        assert_eq!(icon_size(&info(CV_ICON | CV_MINI), true), (32, 32), "CV_MINI is ignored for MINIRECORDCOREs");
        assert_eq!(icon_size(&info(CV_TREE), false), (16, 16));
        let mut sized = info(CV_NAME);
        sized.bitmap_size = (24, 9999);
        assert_eq!(icon_size(&sized, false), (24, 256));
        assert!(shows_icons(CV_ICON) && shows_icons(CV_NAME) && shows_icons(CV_TREE) && shows_icons(CV_TREE | CV_ICON));
        assert!(!shows_icons(CV_TEXT) && !shows_icons(CV_DETAIL) && !shows_icons(CV_TREE | CV_TEXT));
        assert_eq!(picture_fields(CV_ICON, false), (&[RC_HPTRICON][..], false));
        assert_eq!(picture_fields(CV_ICON | CV_MINI, false), (&[RC_HPTRMINIICON, RC_HPTRICON][..], false));
        assert_eq!(picture_fields(CV_ICON | CA_DRAWBITMAP, false), (&[RC_HBMBITMAP][..], true));
        assert!(!picture_fields(CV_ICON | CA_DRAWBITMAP | CA_DRAWICON, false).1, "CA_DRAWICON wins");
        assert_eq!(picture_fields(CV_NAME | CA_DRAWBITMAP | CV_MINI, false).0, &[RC_HBMMINIBITMAP, RC_HBMBITMAP][..]);
        assert_eq!(picture_fields(CV_ICON | CA_DRAWBITMAP, true), (&[RC_HPTRICON][..], false));
    }

    #[test]
    fn ordered_drops_go_between_rows_and_mixed_ones_onto_their_middle() {
        // Rows 3, 4, 5 shown top first, 10 pels each, from y 30 down to 0.
        let rows = [(3, 20, 10), (4, 10, 10), (5, 0, 10)];
        assert_eq!(ordered_spot(&rows, 35, false), Err(Some(2)), "above: after the row before the first shown");
        assert_eq!(ordered_spot(&rows, 27, false), Err(Some(2)), "upper half: before row 3");
        assert_eq!(ordered_spot(&rows, 22, false), Err(Some(3)), "lower half: after row 3");
        assert_eq!(ordered_spot(&rows, 15, true), Ok(4), "mixed: the middle is the row itself");
        assert_eq!(ordered_spot(&rows, 11, true), Err(Some(4)));
        assert_eq!(ordered_spot(&rows, -5, false), Err(Some(5)), "below: after the last");
        assert_eq!(ordered_spot(&[(0, 0, 10)], 8, false), Err(None), "before the first record: CMA_FIRST");
        assert_eq!(ordered_spot(&[], 8, true), Err(None));
    }

    #[test]
    fn icon_items_and_arrangement_are_placed_from_the_upper_left() {
        // The icon at ptlIcon less the origin, the text centred under it.
        let (icon, text) = icon_item((40, 100), (0, 20), (1, 1), (32, 32), 50, 2);
        assert_eq!(icon, (41, 81, 32, 32));
        assert_eq!(text, (41 + 16 - 29, 81 - (2 * LINE_H + 2), 58, 2 * LINE_H + 2));
        assert_eq!(union((0, 0, 10, 10), (5, -5, 10, 10)), (0, -5, 15, 15));
        // Rows from the top left, a new row when the next one is too wide.
        let places = arrange_icons(&[(10, 1), (100, 1), (10, 1)], (32, 32), 200, 150, 0);
        assert_eq!(places[0], (ARRANGE_GAP, 150 - ARRANGE_GAP - 32));
        let w1 = 100 + 2 * CELL_PAD;
        assert_eq!(places[1], (ARRANGE_GAP + 32 + ARRANGE_GAP + (w1 - 32) / 2, 150 - ARRANGE_GAP - 32));
        let row_h = 32 + icon_text_h(1);
        assert_eq!(places[2], (ARRANGE_GAP, 150 - ARRANGE_GAP - row_h - ARRANGE_GAP - 32), "wrapped");
        // The workspace spans the items and the work area.
        assert_eq!(icon_extent(Some((-50, 80)), 100), (-50, 100));
        assert_eq!(icon_extent(Some((20, 300)), 100), (0, 300));
        assert_eq!(icon_extent(None, 100), (0, 100));
    }

    #[test]
    fn the_icon_view_hits_queries_and_draws_icons() {
        let f = fixture(0);
        let r = f.named(&["alpha", "beta", "gamma"], FOLDER);
        f.place(r[0], 10, 100);
        f.place(r[1], 100, 100);
        f.place(r[2], 105, 105);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert_eq!(snap.view(), View::Icon);
        assert_eq!(snap.icons[0].0, (10, 100));
        // The icon of alpha at window (11, 101); its text under it.
        assert_eq!(f.loader.container_record_at(f.cnr, (20, 110)), r[0]);
        assert_eq!(f.loader.container_record_at(f.cnr, (26, 101 - 8)), r[0], "on the text");
        assert_eq!(f.loader.container_record_at(f.cnr, (200, 110)), 0, "white space");
        // gamma lies over beta and was drawn last: it is hit.
        assert_eq!(f.loader.container_record_at(f.cnr, (110, 110)), r[2]);
        // CM_QUERYRECORDRECT: icon, text, both.
        let rect = |extent: u32| {
            f.wr(BUF + 0x400, 16);
            f.wr(BUF + 0x404, r[0]);
            f.wr(BUF + 0x408, 0);
            f.wr(BUF + 0x40C, extent);
            assert_eq!(f.send(CM_QUERYRECORDRECT, BUF + 0x420, BUF + 0x400), 1);
            (0..4).map(|k| f.rd(BUF + 0x420 + 4 * k) as i32).collect::<Vec<_>>()
        };
        assert_eq!(rect(CMA_ICON), vec![11, 101, 43, 133]);
        let text = rect(CMA_TEXT);
        assert_eq!((text[1], text[3]), (101 - icon_text_h(1), 101));
        assert_eq!(rect(CMA_ICON | CMA_TEXT)[3], 133);
        // CM_QUERYRECORDFROMRECT: partly inside, item order and z-order.
        let from = |after: u32, (l, b, rt, t): (i32, i32, i32, i32), search: u32| {
            for (k, v) in [16u32, l as u32, b as u32, rt as u32, t as u32, search].iter().enumerate() {
                f.wr(BUF + 0x440 + 4 * k as u32, *v);
            }
            f.send(CM_QUERYRECORDFROMRECT, after, BUF + 0x440)
        };
        let area = (90, 90, 200, 200);
        assert_eq!(from(CMA_FIRST, area, CMA_PARTIAL | CMA_ITEMORDER), r[1]);
        assert_eq!(from(r[1], area, CMA_PARTIAL | CMA_ITEMORDER), r[2]);
        assert_eq!(from(r[2], area, CMA_PARTIAL | CMA_ITEMORDER), 0);
        assert_eq!(from(CMA_FIRST, area, CMA_PARTIAL | CMA_ZORDER), r[2], "topmost first");
        assert_eq!(from(CMA_FIRST, (0, 0, 60, 140), CMA_COMPLETE | CMA_ITEMORDER), r[0]);
        assert_eq!(from(CMA_FIRST, (0, 0, 30, 140), CMA_COMPLETE | CMA_ITEMORDER), 0, "alpha sticks out");
        assert_eq!(from(0x1234, area, CMA_PARTIAL), u32::MAX, "an unknown record");
        // Each record's folder icon is drawn at 32 × 32.
        let blits = f.blits();
        assert_eq!(blits.len(), 3, "{blits:?}");
        assert!(blits.iter().all(|&(_, _, w, h)| (w, h) == (32, 32)));
    }

    #[test]
    fn cm_arrange_and_autoposition_write_ptlicon() {
        let f = fixture(0);
        let r = f.named(&["a", "b", "c"], 0);
        assert_eq!(f.send(CM_ARRANGE, CMA_ARRANGESTANDARD, 0), 1);
        let (x0, y0) = f.ptl(r[0]);
        assert_eq!(x0, ARRANGE_GAP);
        assert_eq!(y0, 198 - ARRANGE_GAP - ICON_SIZE, "the top row");
        assert!(f.ptl(r[1]).0 > x0 && f.ptl(r[1]).1 == y0);
        assert_eq!(f.send(CM_ARRANGE, 7, 0), 0, "an unknown arrangement");
        assert_eq!(f.send(CM_ARRANGE, CMA_ARRANGESTANDARD, 0x80), 0, "an unknown flag");
        // CCS_AUTOPOSITION arranges as records come.
        let g = fixture(CCS_AUTOPOSITION);
        let r = g.named(&["a", "b"], 0);
        assert_eq!(g.ptl(r[0]).0, ARRANGE_GAP);
        assert!(g.ptl(r[1]).0 > ARRANGE_GAP);
        let more = g.named(&["c"], 0);
        assert!(g.ptl(more[0]).0 > g.ptl(r[1]).0);
        g.send(CM_REMOVERECORD, g.array(&[r[0]]), 1);
        assert_eq!(g.ptl(r[1]).0, ARRANGE_GAP, "the rest moves up");
    }

    #[test]
    fn the_icon_view_scrolls_by_pels() {
        let f = fixture(0);
        let r = f.named(&["low", "high"], 0);
        f.place(r[0], 10, -300);
        f.place(r[1], 10, 150);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert!(snap.bar());
        // Workspace from -300 - text to 198 (the work area's top); none above.
        let (pos, max) = snap.icon_scroll();
        assert_eq!(pos, 0);
        assert_eq!(max, 300 + icon_text_h(1));
        f.send(WM_VSCROLL, CID_VSCROLL, SB_LINEDOWN << 16);
        let scrolls: Vec<[u32; 3]> = std::mem::take(&mut f.loader.shared.window_mgr.lock_or_recover().owner_calls)
            .into_iter().filter_map(|c| match c.kind {
                OwnerCallKind::ContainerNotify { code, block } if u32::from(code) == CN_SCROLL => Some(block),
                _ => None,
            }).collect();
        assert_eq!(scrolls, vec![[f.cnr, LINE_H as u32, CMA_VERTICAL]]);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert_eq!((snap.origin().1, snap.icon_scroll().0), (-LINE_H, LINE_H));
        f.send(WM_VSCROLL, CID_VSCROLL, SB_PAGEDOWN << 16);
        f.send(WM_VSCROLL, CID_VSCROLL, SB_PAGEDOWN << 16);
        assert_eq!(f.loader.snapshot(f.cnr).unwrap().icon_scroll().0, max, "clamped");
        // Selecting the high record scrolls it back into view.
        f.send(CM_SETRECORDEMPHASIS, r[1], 1 | (CRA_CURSORED << 16));
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert!(snap.icon_scroll().0 < max);
        f.loader.cnr_sync_scrollbar(f.cnr, &snap);
        let wm = f.loader.shared.window_mgr.lock_or_recover();
        let sb = wm.find_child_by_id(f.cnr, CID_VSCROLL).unwrap();
        let w = wm.get_window(sb).unwrap();
        assert!(w.visible);
        assert_eq!(w.scrollbar.last, max as i16);
    }

    #[test]
    fn name_tree_and_detail_views_draw_pictures() {
        let f = fixture(0);
        f.named(&["one", "two"], FOLDER);
        f.set_view(CV_NAME);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert_eq!(snap.frame().row_h, ICON_SIZE + 2 * ROW_PAD, "rows as high as the icons");
        assert_eq!(snap.text_inset(0), CELL_PAD + ICON_SIZE);
        assert_eq!(f.blits().len(), 2);
        f.set_view(CV_NAME | CV_MINI);
        assert!(f.blits().iter().all(|&(_, _, w, h)| (w, h) == (16, 16)));
        f.set_view(CV_TEXT);
        assert!(f.blits().is_empty(), "the text view has no pictures");
        f.set_view(CV_TREE);
        assert!(f.blits().iter().all(|&(_, _, w, _)| w == MINI_ICON_SIZE as u32));
        f.set_view(CV_TREE | CV_TEXT);
        assert!(f.blits().is_empty());
        // A CFA_BITMAPORICON column draws the handle in the record.
        let g = fixture(0);
        let r = g.alloc(1, 4);
        g.wr(r[0] + RECORDCORE_SIZE, FOLDER);
        g.insert(r[0], 1, CMA_END, 0);
        let fi = g.send(CM_ALLOCDETAILFIELDINFO, 1, 0);
        g.wr(fi + FI_FLDATA, CFA_BITMAPORICON);
        g.wr(fi + FI_OFFSTRUCT, RECORDCORE_SIZE);
        for (o, v) in [(0, 16), (4, CMA_END), (8, 1), (12, 1)] { g.wr(BUF + 0x500 + o, v); }
        g.send(CM_INSERTDETAILFIELDINFO, fi, BUF + 0x500);
        g.set_view(CV_DETAIL);
        let b = g.blits();
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].2, b[0].3), (LINE_H as u32, LINE_H as u32));
    }

    #[test]
    fn direct_editing_asks_the_owner_and_copies_the_text() {
        let f = fixture(0);
        f.set_view(CV_TEXT);
        let r = f.named(&["old", "other"], 0);
        // Alt + button 1 on the first record's text opens the editor.
        let top_row = 1 + 198 - 9;
        assert_eq!(f.send(WM_BUTTON1DOWN, Fx::at(20, top_row), KC_ALT << 16), 1);
        let mle = f.loader.shared.window_mgr.lock_or_recover().find_child_by_id(f.cnr, CID_MLE).expect("the MLE");
        {
            let wm = f.loader.shared.window_mgr.lock_or_recover();
            let w = wm.get_window(mle).unwrap();
            assert!(w.visible);
            assert_eq!((w.text.as_str(), w.edit_anchor, w.edit_caret), ("old", 0, 3), "all of it selected");
            assert_eq!(wm.focus_hwnd, mle);
            assert!(w.cx >= EDIT_MIN_W);
        }
        let begin = f.notes();
        let id = ID;
        assert_eq!(begin, vec![(CN_BEGINEDIT, [CNREDITDATA_SIZE, f.cnr, r[0], 0, r[0] + RC_PSZTEXT, 0, id])]);
        // The user types; CM_CLOSEEDIT keeps it: CN_REALLOCPSZ with the
        // bytes the new text needs.
        f.loader.shared.window_mgr.lock_or_recover().get_window_mut(mle).unwrap().text = "renamed".into();
        assert_eq!(f.send(CM_CLOSEEDIT, 0, 0), 1);
        let notes = f.notes();
        assert_eq!(notes, vec![(CN_REALLOCPSZ, [CNREDITDATA_SIZE, f.cnr, r[0], 0, r[0] + RC_PSZTEXT, 8, id])]);
        assert!(!f.loader.shared.window_mgr.lock_or_recover().get_window(mle).unwrap().visible);
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().focus_hwnd, f.cnr);
        // The owner points pszIcon at a buffer of 8 bytes and says TRUE.
        f.wr(r[0] + RC_PSZTEXT, BUF + 0xA00);
        f.loader.container_realloc_done(f.cnr, 1, notes[0].1);
        assert_eq!(f.loader.guest_read_bytes(BUF + 0xA00, 8).unwrap(), b"renamed\0");
        assert_eq!(f.notes(), vec![(CN_ENDEDIT, [CNREDITDATA_SIZE, f.cnr, r[0], 0, r[0] + RC_PSZTEXT, 8, id])]);
        assert_eq!(f.send(CM_CLOSEEDIT, 0, 0), 0, "nothing open");
        // FALSE from the owner: the text stays.
        f.send(CM_OPENEDIT, { f.wr(BUF + 0x480, 28); f.wr(BUF + 0x488, r[1]); f.wr(BUF + 0x48C, 0); BUF + 0x480 }, 0);
        f.notes();
        f.loader.shared.window_mgr.lock_or_recover().get_window_mut(mle).unwrap().text = "zzz".into();
        f.send(CM_CLOSEEDIT, 0, 0);
        let n = f.notes();
        f.loader.container_realloc_done(f.cnr, 0, n[0].1);
        assert_eq!(f.loader.cnr_text(f.cnr, f.rd(r[1] + RC_PSZTEXT)), "other");
        assert_eq!(f.notes()[0].1[5], 0, "CN_ENDEDIT without new text");
        // Esc in the MLE discards: CN_ENDEDIT at once.
        f.send(WM_BUTTON1DOWN, Fx::at(20, top_row), KC_ALT << 16);
        f.notes();
        let esc = f.loader.dispatch_builtin_control(mle, WM_CHAR, KC_VIRTUALKEY, VK_ESC << WM_CHAR_VKEY_SHIFT);
        assert!(matches!(esc, ApiResult::Normal(1)));
        assert_eq!(f.notes().iter().map(|n| n.0).collect::<Vec<_>>(), vec![CN_ENDEDIT]);
    }

    #[test]
    fn read_only_text_is_not_edited() {
        let open = |f: &Fx, rec: u32, fi: u32| {
            f.wr(BUF + 0x480, 28);
            f.wr(BUF + 0x488, rec);
            f.wr(BUF + 0x48C, fi);
            f.send(CM_OPENEDIT, BUF + 0x480, 0)
        };
        let f = fixture(CCS_READONLY);
        let r = f.named(&["x"], 0);
        assert_eq!(open(&f, r[0], 0), 0, "CCS_READONLY");
        let g = fixture(0);
        let r = g.named(&["x", "y"], 0);
        g.wr(r[0] + RC_ATTR, CRA_RECORDREADONLY);
        assert_eq!(open(&g, r[0], 0), 0, "CRA_RECORDREADONLY");
        assert_eq!(open(&g, 0x4444, 0), 0, "not a record");
        assert_eq!(open(&g, r[1], 0), 1);
        g.send(CM_CLOSEEDIT, 0, 0);
        // Details view: only writable CFA_STRING columns inside the record.
        let fis: Vec<u32> = {
            let first = g.send(CM_ALLOCDETAILFIELDINFO, 3, 0);
            let mut v = vec![first];
            while v.len() < 3 { v.push(g.rd(v.last().unwrap() + FI_NEXT)); }
            v
        };
        for (fi, fl, off) in [(fis[0], CFA_STRING, RC_PSZTEXT), (fis[1], CFA_STRING | CFA_FIREADONLY, RC_PSZTEXT), (fis[2], CFA_ULONG, RC_ATTR)] {
            g.wr(fi + FI_FLDATA, fl);
            g.wr(fi + FI_OFFSTRUCT, off);
        }
        for (o, v) in [(0, 16), (4, CMA_END), (8, 1), (12, 3)] { g.wr(BUF + 0x500 + o, v); }
        g.send(CM_INSERTDETAILFIELDINFO, fis[0], BUF + 0x500);
        g.set_view(CV_DETAIL);
        g.notes();
        assert_eq!(open(&g, r[1], fis[1]), 0, "CFA_FIREADONLY");
        assert_eq!(open(&g, r[1], fis[2]), 0, "not a string");
        assert_eq!(open(&g, r[1], 0), 0, "no column");
        assert_eq!(open(&g, r[1], fis[0]), 1);
        assert_eq!(g.notes()[0].1, [CNREDITDATA_SIZE, g.cnr, r[1], fis[0], r[1] + RC_PSZTEXT, 0, CID_LEFTDVWND]);
    }

    #[test]
    fn the_scroll_bar_follows_the_rows() {
        let f = fixture(0);
        f.set_view(CV_TEXT);
        // 200 high: 198 for rows of 18 = 11 rows; 15 records need the bar.
        keyed(&f, &[0; 15]);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert!(snap.bar());
        assert_eq!(snap.frame().rows.2, 298 - SCROLLBAR_W);
        assert_eq!(snap.max_top(), 4);
        f.loader.cnr_sync_scrollbar(f.cnr, &snap);
        let sb = f.loader.shared.window_mgr.lock_or_recover().find_child_by_id(f.cnr, CID_VSCROLL).unwrap();
        {
            let wm = f.loader.shared.window_mgr.lock_or_recover();
            let w = wm.get_window(sb).unwrap();
            assert!(w.visible);
            assert_eq!((w.x, w.cx, w.scrollbar.last, w.scrollbar.thumb_size), (300 - 1 - 16, 16, 4, Some((11, 15))));
        }
        // WM_VSCROLL scrolls, clamped, and tells the owner.
        f.send(WM_VSCROLL, CID_VSCROLL, SB_PAGEDOWN << 16);
        assert_eq!(f.loader.snapshot(f.cnr).unwrap().top, 4);
        let scrolls: Vec<[u32; 3]> = std::mem::take(&mut f.loader.shared.window_mgr.lock_or_recover().owner_calls)
            .into_iter().filter_map(|c| match c.kind {
                OwnerCallKind::ContainerNotify { code, block } if u32::from(code) == CN_SCROLL => Some(block),
                _ => None,
            }).collect();
        assert_eq!(scrolls, vec![[f.cnr, 4 * 18, CMA_VERTICAL]]);
        f.send(WM_VSCROLL, CID_VSCROLL, 2 | SB_SLIDERPOSITION << 16);
        assert_eq!(f.loader.snapshot(f.cnr).unwrap().top, 2);
        f.send(WM_VSCROLL, CID_VSCROLL, SB_LINEUP << 16);
        f.send(WM_VSCROLL, CID_VSCROLL, SB_LINEUP << 16);
        f.send(WM_VSCROLL, CID_VSCROLL, SB_LINEUP << 16);
        assert_eq!(f.loader.snapshot(f.cnr).unwrap().top, 0);
        // Few records: no bar.
        f.send(CM_REMOVERECORD, 0, 0);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert!(!snap.bar());
        f.loader.cnr_sync_scrollbar(f.cnr, &snap);
        assert!(!f.loader.shared.window_mgr.lock_or_recover().get_window(sb).unwrap().visible);
    }

    #[test]
    fn the_tree_view_expands_collapses_and_notifies() {
        let f = fixture(0);
        let r = f.alloc(4, 0);
        f.insert(r[0], 2, CMA_END, 0);
        f.insert(r[2], 2, CMA_END, r[0]);
        for i in 0..23 { f.wr(BUF + 0x200 + 4 * i, 0); }
        f.wr(BUF + 0x200 + 20, CV_TREE);
        f.send(CM_SETCNRINFO, BUF + 0x200, CMA_FLWINDOWATTR);
        let rows = |f: &Fx| f.loader.snapshot(f.cnr).unwrap().rows.iter().map(|&(x, _)| x).collect::<Vec<_>>();
        assert_eq!(rows(&f), vec![r[0], r[1]], "collapsed");
        let values = |f: &Fx| -> Vec<(u32, u32)> {
            std::mem::take(&mut f.loader.shared.window_mgr.lock_or_recover().owner_calls).into_iter().filter_map(|c| match c.kind {
                OwnerCallKind::ContainerValue { code, mp2 } => Some((u32::from(code), mp2)),
                _ => None,
            }).collect()
        };
        assert_eq!(f.send(CM_EXPANDTREE, r[0], 0), 1);
        assert_eq!(rows(&f), vec![r[0], r[2], r[3], r[1]]);
        assert_eq!(values(&f), vec![(CN_EXPANDTREE, r[0])]);
        assert_eq!(f.rd(r[0] + RC_ATTR) & (CRA_EXPANDED | CRA_COLLAPSED), CRA_EXPANDED);
        // Cursor on a child; collapsing moves it to the parent.
        f.send(CM_SETRECORDEMPHASIS, r[3], 1 | CRA_CURSORED << 16);
        f.send(CM_COLLAPSETREE, r[0], 0);
        assert_eq!(rows(&f), vec![r[0], r[1]]);
        assert_ne!(f.rd(r[0] + RC_ATTR) & CRA_CURSORED, 0);
        assert_eq!(values(&f), vec![(CN_COLLAPSETREE, r[0])]);
        // Keys: + opens, Right goes to the first child, Left back, - closes.
        f.send(WM_CHAR, KC_CHAR, u32::from(b'+'));
        assert_eq!(rows(&f).len(), 4);
        f.key(VK_RIGHT, 0);
        assert_ne!(f.rd(r[2] + RC_ATTR) & CRA_CURSORED, 0);
        f.key(VK_LEFT, 0);
        assert_ne!(f.rd(r[0] + RC_ATTR) & CRA_CURSORED, 0);
        f.send(WM_CHAR, KC_CHAR, u32::from(b'-'));
        assert_eq!(rows(&f).len(), 2);
        // A click on the expander box opens it: row 0 at y 181..198, box
        // after a 0 indentation at x 1 + 4 + ... (text inset 0 + 9 + 8).
        let snap = f.loader.snapshot(f.cnr).unwrap();
        let ex = BORDER + snap.expander_x(0);
        values(&f);
        f.send(WM_BUTTON1DOWN, Fx::at(ex + 4, 190), 0);
        assert_eq!(rows(&f).len(), 4, "the expander opened it");
        assert_eq!(f.send(CM_EXPANDTREE, 0x7777, 0), 0, "not a record");
    }

    fn heap_with(n: u32) -> (CnrState, Vec<u32>) {
        let mut s = CnrState::new(0);
        s.heap.add_arena(0x10_0000, CNR_ARENA_BYTES);
        let recs = s.heap.carve(ItemKind::Record, RECORDCORE_SIZE, n).unwrap();
        (s, recs)
    }

    #[test]
    fn the_heap_carves_from_arenas_and_gives_back_empty_ones() {
        let mut h = RecordHeap::default();
        assert!(!h.fits(56, 1), "no arena yet");
        h.add_arena(0x20_0000, 4096);
        let a = h.carve(ItemKind::Record, 64, 3).unwrap();
        assert_eq!(a, vec![0x20_0000, 0x20_0040, 0x20_0080]);
        assert!(h.fits(64, 61) && !h.fits(64, 62));
        assert_eq!(h.item(a[1]).map(|i| (i.kind, i.size)), Some((ItemKind::Record, 64)));
        assert_eq!(h.item(0x20_0004), None, "only item starts are items");
        assert_eq!(h.free(a[0]), Some(None));
        assert_eq!(h.free(a[0]), None, "freed twice");
        assert_eq!(h.free(a[1]), Some(None));
        assert_eq!(h.free(a[2]), Some(Some(0x20_0000)), "the empty arena goes back");
        assert!(h.is_empty() && !h.fits(64, 1));
    }

    #[test]
    fn the_heap_is_bounded() {
        let mut h = RecordHeap::default();
        h.add_arena(0x20_0000, u32::MAX / 2);
        assert!(h.carve(ItemKind::Field, 32, MAX_CNR_ITEMS as u32 + 1).is_none());
        assert!(h.carve(ItemKind::Field, 32, 0).is_none());
        assert!(h.has_room(MAX_CNR_ITEMS as u32));
    }

    #[test]
    fn records_insert_in_order_and_only_once() {
        let (mut s, r) = heap_with(4);
        assert_eq!(s.insert(&r[0..2], CMA_END, 0), Ok(2));
        assert_eq!(s.insert(&[r[2]], CMA_FIRST, 0), Ok(3));
        assert_eq!(s.insert(&[r[3]], r[2], 0), Ok(4), "after a record");
        let order: Vec<u32> = s.records.iter().map(|&(x, _)| x).collect();
        assert_eq!(order, vec![r[2], r[3], r[0], r[1]]);
        assert_eq!(s.insert(&[r[0]], CMA_END, 0), Err(CnrError::Inserted));
        assert_eq!(s.insert(&[0x1234], CMA_END, 0), Err(CnrError::Invalid), "not allocated here");
        assert_eq!(s.insert(&[r[0] + 4], CMA_END, 0), Err(CnrError::Invalid), "inside a record");
    }

    #[test]
    fn duplicate_or_unknown_order_records_are_refused() {
        let (mut s, r) = heap_with(3);
        assert_eq!(s.insert(&[r[0], r[0]], CMA_END, 0), Err(CnrError::Invalid));
        assert_eq!(s.insert(&[r[0]], r[1], 0), Err(CnrError::Invalid), "order record not inserted");
        assert!(s.records.is_empty());
    }

    #[test]
    fn queries_walk_siblings_and_children() {
        let (mut s, r) = heap_with(5);
        s.insert(&r[0..3], CMA_END, 0).unwrap();
        s.insert(&r[3..5], CMA_END, r[1]).unwrap();
        assert_eq!(s.query(0, CMA_FIRST), Ok(r[0]));
        assert_eq!(s.query(0, CMA_LAST), Ok(r[2]));
        assert_eq!(s.query(r[0], CMA_NEXT), Ok(r[1]));
        assert_eq!(s.query(r[1], CMA_NEXT), Ok(r[2]), "children are not siblings");
        assert_eq!(s.query(r[0], CMA_PREV), Ok(0));
        assert_eq!(s.query(r[1], u32::from(CMA_FIRSTCHILD)), Ok(r[3]));
        assert_eq!(s.query(r[1], u32::from(CMA_LASTCHILD)), Ok(r[4]));
        assert_eq!(s.query(r[4], u32::from(CMA_PARENT)), Ok(r[1]));
        assert_eq!(s.query(r[3], CMA_NEXT), Ok(r[4]));
        assert_eq!(s.query(0x999, CMA_NEXT), Err(()));
        assert_eq!(s.query(r[0], 0x7777), Err(()));
    }

    #[test]
    fn removing_a_parent_takes_its_children() {
        let (mut s, r) = heap_with(4);
        s.insert(&r[0..2], CMA_END, 0).unwrap();
        s.insert(&r[2..4], CMA_END, r[0]).unwrap();
        let gone = s.removal(&[r[0]]).unwrap();
        assert_eq!(gone, vec![r[0], r[2], r[3]]);
        s.remove(&gone);
        assert_eq!(s.records, vec![(r[1], 0)]);
        assert!(!s.is_inserted(r[2]));
        assert_eq!(s.removal(&[r[0]]), Err(CnrError::Invalid));
    }

    #[test]
    fn field_infos_insert_and_query() {
        let mut s = CnrState::new(0);
        s.heap.add_arena(0x30_0000, CNR_ARENA_BYTES);
        let f = s.heap.carve(ItemKind::Field, FIELDINFO_SIZE, 3).unwrap();
        assert_eq!(s.insert_fields(&f[0..2], CMA_END), Ok(2));
        assert_eq!(s.insert_fields(&[f[2]], CMA_FIRST), Ok(3));
        assert_eq!(s.fields, vec![f[2], f[0], f[1]]);
        assert_eq!(s.query_field(0, CMA_FIRST), Ok(f[2]));
        assert_eq!(s.query_field(f[0], CMA_NEXT), Ok(f[1]));
        assert_eq!(s.query_field(f[1], CMA_NEXT), Ok(0));
        assert_eq!(s.query_field(f[2], CMA_PREV), Ok(0));
        assert_eq!(s.insert_fields(&[f[0]], CMA_END), Err(CnrError::Inserted));
        let (mut t, r) = heap_with(1);
        assert_eq!(t.insert_fields(&r, CMA_END), Err(CnrError::Invalid), "a record is no field info");
    }

    #[test]
    fn cnrinfo_round_trips_by_flags() {
        let mut info = CnrInfo::default();
        let mut w = [0u32; 23];
        w[4] = 0x5000;
        w[5] = CV_DETAIL | CA_DETAILSVIEWTITLES;
        w[18] = 3;
        info.set_from(&w, CMA_CNRTITLE | CMA_FLWINDOWATTR);
        assert_eq!((info.title, info.window_attr, info.line_spacing), (0x5000, CV_DETAIL | CA_DETAILSVIEWTITLES, 0));
        let out = info.words(7, 2);
        assert_eq!((out[0], out[4], out[9], out[21]), (CNRINFO_SIZE, 0x5000, 7, 2));
    }

    #[test]
    fn views_and_their_texts() {
        assert_eq!(view_of(CV_ICON), View::Icon);
        assert_eq!(view_of(CV_TEXT | CV_FLOW), View::Text);
        assert_eq!(view_of(CV_DETAIL | CV_TEXT), View::Detail);
        assert_eq!(text_offset(View::Name, false), RC_PSZNAME);
        assert_eq!(text_offset(View::Name, true), RC_PSZICON, "mini records show pszIcon");
        assert_eq!(sel_mode(0), SelMode::Single);
        assert_eq!(sel_mode(CCS_EXTENDSEL), SelMode::Extended);
        assert_eq!(sel_mode(CCS_MULTIPLESEL), SelMode::Multiple);
    }

    #[test]
    fn frames_stack_title_header_and_rows() {
        let f = Frame::new(CV_DETAIL | CA_DETAILSVIEWTITLES | CA_CONTAINERTITLE | CA_TITLESEPARATOR, 0, 0, 200, 100);
        assert_eq!(f.title, Some((1, 80, 198, 19)));
        assert_eq!(f.header, Some((1, 61, 198, 19)));
        assert_eq!(f.rows, (1, 1, 198, 60));
        assert_eq!(f.capacity(), 3);
        let plain = Frame::new(CV_TEXT | CA_DETAILSVIEWTITLES, 2, 0, 200, 100);
        assert_eq!((plain.title, plain.header, plain.row_h), (None, None, 20));
    }

    #[test]
    fn lists_place_rows_or_flowed_columns() {
        let f = Frame::new(CV_TEXT, 0, 0, 200, 56);
        assert_eq!(f.capacity(), 3);
        let one = place_list(&f, &[0; 5], 1, false);
        assert_eq!(one.iter().map(|p| p.0).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(one[0].1, (1, 37, 198, 18));
        let flow = place_list(&f, &[40, 50, 30, 20, 60], 0, true);
        assert_eq!(flow.len(), 5);
        assert_eq!(flow[3].1, (1 + 58 + COLUMN_GAP, 37, 68, 18), "second column, as wide as its widest");
        assert_eq!(scroll_to(0, 4, 3, false, &one, 199), 2);
        assert_eq!(scroll_to(3, 1, 3, false, &one, 199), 1);
        assert_eq!(scroll_to(3, 1, 3, true, &flow, 199), 0, "flowed views scroll by columns");
    }

    #[test]
    fn dates_and_times_follow_the_country() {
        assert_eq!(format_date(9, 3, 2026, 0, '-'), "03-09-2026");
        assert_eq!(format_date(9, 3, 2026, 1, '.'), "09.03.2026");
        assert_eq!(format_date(9, 3, 2026, 2, '/'), "2026/03/09");
        assert_eq!(format_time(14, 5, 7, 1, ':'), "14:05:07");
        assert_eq!(format_time(14, 5, 7, 0, ':'), " 2:05:07pm");
        assert_eq!(format_time(0, 0, 0, 0, ':'), "12:00:00am");
    }

    #[test]
    fn huge_column_widths_stay_bounded() {
        let f = fixture(0);
        let fis = detail(&f, [0x8000_0000, u32::MAX, 0x7FFF_FFFF], CA_DETAILSVIEWTITLES, 2);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        for c in &snap.cols {
            assert!(c.w > 0 && c.w <= MAX_WINDOW_COORD + 2 * CELL_PAD, "{c:?}");
            assert!(c.x <= MAX_DETAIL_WIDTH);
        }
        // Painting and scrolling to the far end do not overflow.
        f.paint_ops();
        f.send(WM_HSCROLL, CID_HSCROLL, SB_PAGERIGHT << 16);
        f.send(CM_SCROLLWINDOW, CMA_HORIZONTAL, i32::MAX as u32);
        f.paint_ops();
        let _ = fis;
    }

    #[test]
    fn split_bar_and_panes() {
        assert_eq!(split_x(0, 200), 100, "the middle by default");
        assert_eq!(split_x(5, 200), MIN_PANE);
        assert_eq!(split_x(500, 200), 200 - SPLIT_W - MIN_PANE);
        let one = detail_panes(&[50, 60, 70], None, 0, (1, 100), [500, 0]);
        assert_eq!(one, vec![Pane { x: 1, w: 100, first: 0, end: 3, content: 180, off: 80 }], "scroll kept in the content");
        let two = detail_panes(&[50, 60, 70], Some(1), 40, (1, 200), [10, 20]);
        assert_eq!(two[0], Pane { x: 1, w: 40, first: 0, end: 1, content: 50, off: 10 });
        assert_eq!(two[1], Pane { x: 1 + 40 + SPLIT_W, w: 200 - 40 - SPLIT_W, first: 1, end: 3, content: 130, off: 0 });
        assert_eq!(detail_panes(&[50, 60], Some(2), 40, (1, 200), [0, 0]).len(), 1, "no columns right of the bar");
    }

    /// A details view of `n` records with three fixed-width string
    /// columns (titled "A", "B", "C"; each record's values in its data):
    /// the field infos.
    fn detail(f: &Fx, widths: [u32; 3], attr: u32, n: u32) -> Vec<u32> {
        let first = f.send(CM_ALLOCDETAILFIELDINFO, 3, 0);
        let mut fis = vec![first];
        while fis.len() < 3 { fis.push(f.rd(fis.last().unwrap() + FI_NEXT)); }
        f.loader.guest_write_bytes(BUF + 0xC00, b"A\0B\0C\0value\0").unwrap();
        for (k, &fi) in fis.iter().enumerate() {
            f.wr(fi + FI_FLDATA, CFA_STRING);
            f.wr(fi + FI_OFFSTRUCT, RECORDCORE_SIZE + 4 * k as u32);
            f.wr(fi + FI_CXWIDTH, widths[k]);
            f.wr(fi + FI_TITLE, BUF + 0xC00 + 2 * k as u32);
        }
        for (o, v) in [(0, 16), (4, CMA_END), (8, 1), (12, 3)] { f.wr(BUF + 0x500 + o, v); }
        f.send(CM_INSERTDETAILFIELDINFO, fis[0], BUF + 0x500);
        let recs = f.alloc(n, 12);
        for &r in &recs { for k in 0..3 { f.wr(r + RECORDCORE_SIZE + 4 * k, BUF + 0xC06); } }
        f.insert(recs[0], n, CMA_END, 0);
        f.set_view(CV_DETAIL | attr);
        fis
    }

    fn scroll_notes(f: &Fx) -> Vec<[u32; 3]> {
        std::mem::take(&mut f.loader.shared.window_mgr.lock_or_recover().owner_calls).into_iter().filter_map(|c| match c.kind {
            OwnerCallKind::ContainerNotify { code, block } if u32::from(code) == CN_SCROLL => Some(block),
            _ => None,
        }).collect()
    }

    fn bar_of(f: &Fx, id: u32) -> (bool, i32, i32, i16) {
        let wm = f.loader.shared.window_mgr.lock_or_recover();
        let w = wm.get_window(wm.find_child_by_id(f.cnr, id).unwrap()).unwrap();
        (w.visible, w.x, w.cx, w.scrollbar.last)
    }

    #[test]
    fn wide_details_scroll_across() {
        let f = fixture(0);
        let fis = detail(&f, [150, 150, 100], CA_DETAILSVIEWTITLES, 3);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        // 3 columns of 158, 158, 108 pels (with padding) in 298: a bar.
        assert_eq!(snap.bars(), (false, true));
        assert_eq!(snap.frame().rows.1, BORDER + SCROLLBAR_W, "the rows above the bar");
        f.loader.cnr_sync_scrollbar(f.cnr, &snap);
        let content = 3 * 2 * CELL_PAD + 400;
        assert_eq!(bar_of(&f, CID_HSCROLL), (true, BORDER, 298, (content - 298) as i16));
        assert!(!bar_of(&f, CID_RIGHTHSCROLL).0);
        // A page right: the columns move left; CN_SCROLL across.
        scroll_notes(&f);
        f.send(WM_HSCROLL, CID_HSCROLL, SB_PAGERIGHT << 16);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert_eq!(snap.hscroll[0], content - 298, "clamped to the end");
        assert_eq!(snap.columns()[0].x, BORDER - (content - 298));
        assert_eq!(scroll_notes(&f), vec![[f.cnr, (content - 298) as u32, CMA_HORIZONTAL]]);
        f.send(WM_HSCROLL, CID_HSCROLL, SB_LINELEFT << 16);
        assert_eq!(f.loader.snapshot(f.cnr).unwrap().hscroll[0], content - 298 - H_LINE);
        // CM_SCROLLWINDOW across, by pels.
        assert_eq!(f.send(CM_SCROLLWINDOW, CMA_HORIZONTAL, (-10i32) as u32), 1);
        assert_eq!(f.loader.snapshot(f.cnr).unwrap().hscroll[0], content - 298 - H_LINE - 10);
        // Column headings and cells are drawn inside the rows' width.
        let texts: Vec<i32> = f.paint_ops().into_iter().filter_map(|o| match o {
            crate::gui::surface::DrawOp::Text { x, .. } => Some(x), _ => None }).collect();
        assert!(!texts.is_empty());
        let _ = fis;
        // Narrow columns: no bar.
        let g = fixture(0);
        detail(&g, [20, 20, 20], 0, 2);
        assert_eq!(g.loader.snapshot(g.cnr).unwrap().bars(), (false, false));
    }

    #[test]
    fn a_split_details_view_has_two_panes() {
        let f = fixture(0);
        let fis = detail(&f, [100, 150, 150], CA_DETAILSVIEWTITLES, 3);
        // pFieldInfoLast = the first column, the split bar at 80.
        for i in 0..23 { f.wr(BUF + 0x600 + 4 * i, 0); }
        f.wr(BUF + 0x600 + 8, fis[0]);
        f.wr(BUF + 0x600 + 88, 80);
        assert_eq!(f.send(CM_SETCNRINFO, BUF + 0x600, CMA_PFIELDINFOLAST | CMA_XVERTSPLITBAR), 1);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        let panes = snap.panes();
        assert_eq!(panes.len(), 2);
        assert_eq!((panes[0].x, panes[0].w, panes[0].end), (BORDER, 80, 1));
        assert_eq!(panes[1].x, BORDER + 80 + SPLIT_W);
        let cols = snap.columns();
        assert_eq!((cols[0].pane, cols[1].pane, cols[1].x), (0, 1, panes[1].x));
        // Both panes overflow: two bars, one under each.
        f.loader.cnr_sync_scrollbar(f.cnr, &snap);
        assert_eq!(bar_of(&f, CID_HSCROLL).0..=bar_of(&f, CID_HSCROLL).0, true..=true);
        assert_eq!((bar_of(&f, CID_RIGHTHSCROLL).1, bar_of(&f, CID_RIGHTHSCROLL).2), (panes[1].x, panes[1].w));
        // The right pane scrolls by itself.
        f.send(WM_HSCROLL, CID_RIGHTHSCROLL, SB_LINERIGHT << 16);
        assert_eq!(f.loader.snapshot(f.cnr).unwrap().hscroll, [0, H_LINE]);
        // CM_QUERYVIEWPORTRECT of the right pane.
        assert_eq!(f.send(CM_QUERYVIEWPORTRECT, BUF + 0x700, u32::from(CMA_WINDOW) | 1 << 16), 1);
        assert_eq!((f.rd(BUF + 0x700), f.rd(BUF + 0x708)), (panes[1].x as u32, (panes[1].x + panes[1].w) as u32));
        // Dragging the split bar moves it; CNRINFO reports the new place.
        let (sx, sy, _, sh) = snap.split_rect().unwrap();
        assert_eq!(f.send(WM_BUTTON1DOWN, Fx::at(sx + 1, sy + sh / 2), 0), 1);
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().pressed_control, f.cnr);
        assert_eq!(f.send(WM_MOUSEMOVE, Fx::at(sx + 1 + 40, sy + 5), 0), 1);
        assert_eq!(f.send(WM_BUTTON1UP, Fx::at(sx + 41, sy + 5), 0), 1);
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().pressed_control, 0);
        f.send(CM_QUERYCNRINFO, BUF + 0x400, CNRINFO_SIZE);
        assert_eq!(f.rd(BUF + 0x400 + 88), 120, "xVertSplitbar");
        assert_eq!(f.loader.snapshot(f.cnr).unwrap().panes()[0].w, 120);
        // Moves without the bar held are not the container's.
        assert!(f.loader.container_message(f.cnr, WM_MOUSEMOVE, Fx::at(5, 5), 0).is_none());
        // A drag to the far left stops at the narrowest pane.
        f.send(WM_BUTTON1DOWN, Fx::at(BORDER + 121, sy + 5), 0);
        f.send(WM_MOUSEMOVE, Fx::at(-50, sy + 5), 0);
        f.send(WM_BUTTON1UP, Fx::at(-50, sy + 5), 0);
        assert_eq!(f.loader.snapshot(f.cnr).unwrap().panes()[0].w, MIN_PANE);
    }

    #[test]
    fn the_icon_view_scrolls_across() {
        let f = fixture(0);
        let r = f.named(&["far"], 0);
        f.place(r[0], 600, 50);
        f.send(CM_INVALIDATERECORD, 0, 0);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert!(snap.bars().1, "an icon right of the work area");
        let (pos, max) = snap.icon_xscroll();
        assert_eq!(pos, 0);
        assert!(max >= 600 + ICON_SIZE - 298, "{max}");
        f.send(WM_HSCROLL, CID_HSCROLL, SB_PAGERIGHT << 16);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert_eq!(snap.info.origin.0 as i32, snap.frame().rows.2, "the work area moved a page across");
        assert_eq!(scroll_notes(&f), vec![[f.cnr, snap.frame().rows.2 as u32, CMA_HORIZONTAL]]);
        assert_eq!(f.send(WM_HSCROLL, CID_RIGHTHSCROLL, SB_PAGERIGHT << 16), 0, "no right pane");
    }

    fn draw_calls(f: &Fx) -> Vec<(u32, u32, u32)> {
        f.loader.shared.window_mgr.lock_or_recover().owner_calls.iter().filter_map(|c| match c.kind {
            OwnerCallKind::ContainerDrawItem { rec, fi, part } => Some((rec, fi, part)), _ => None,
        }).collect()
    }

    #[test]
    fn owner_drawn_records_and_columns() {
        let f = fixture(0);
        f.set_view(CV_TEXT | CA_OWNERDRAW);
        let r = f.named(&["one", "two"], 0);
        f.paint_ops();
        assert_eq!(draw_calls(&f), vec![(r[0], 0, CMA_TEXT), (r[1], 0, CMA_TEXT)]);
        // The OWNERITEM: the container, the item's rectangle and the part;
        // pRecord / pFieldInfo after it.
        let call = f.loader.shared.window_mgr.lock_or_recover().owner_calls.iter()
            .find(|c| matches!(c.kind, OwnerCallKind::ContainerDrawItem { .. })).copied().unwrap();
        let client = f.loader.shared.window_mgr.lock_or_recover().get_window(f.cnr).unwrap().owner;
        f.loader.shared.window_mgr.lock_or_recover().get_window_mut(client).unwrap().pfn_wp = 0x5000;
        let crate::loader::pm_owner::OwnerSetup::Call { msg, mp1, block, hps, .. } = f.loader.owner_call_setup(&call)
        else { panic!("a call") };
        assert_eq!((msg, mp1), (WM_DRAWITEM, ID));
        let rect = f.loader.container_item_rect(f.cnr, r[0], 0, CMA_TEXT).unwrap();
        assert_eq!(block.len(), 14);
        assert_eq!((block[0], block[1], block[10], block[12], block[13]), (f.cnr, hps, CMA_TEXT, r[0], 0));
        assert_eq!((block[6] as i32, block[7] as i32, block[8] as i32, block[9] as i32), rect);
        assert_eq!(rect.3, 199, "the top row, below the top border");
        // FALSE: the container draws the text itself.
        let s = {
            let mut wm = f.loader.shared.window_mgr.lock_or_recover();
            let frame = wm.find_frame_for_hwnd(f.cnr);
            wm.frame_surface(frame).unwrap()
        };
        s.lock_or_recover().record = Some(Vec::new());
        f.loader.owner_call_finish(&call, hps, 0, &block);
        let ops = s.lock_or_recover().record.take().unwrap();
        assert!(ops.iter().any(|o| matches!(o, crate::gui::surface::DrawOp::Text { .. })), "{ops:?}");
        // Details view: CFA_OWNER asks for that column's cells only.
        let g = fixture(0);
        let fis = detail(&g, [40, 40, 40], 0, 2);
        g.wr(fis[1] + FI_FLDATA, CFA_STRING | CFA_OWNER);
        let recs: Vec<u32> = g.loader.snapshot(g.cnr).unwrap().rows.iter().map(|&(r, _)| r).collect();
        g.paint_ops();
        assert_eq!(draw_calls(&g), vec![(recs[0], fis[1], CMA_TEXT), (recs[1], fis[1], CMA_TEXT)]);
        let cell = g.loader.container_item_rect(g.cnr, recs[0], fis[1], CMA_TEXT).unwrap();
        let col = g.loader.snapshot(g.cnr).unwrap().columns()[1];
        assert_eq!((cell.0, cell.2), (col.x, col.x + col.w));
        // Icon view with CA_OWNERDRAW: the icon and the text of each.
        let h = fixture(0);
        h.set_view(CV_ICON | CA_OWNERDRAW);
        let r = h.named(&["i"], 0);
        h.send(CM_ARRANGE, CMA_ARRANGESTANDARD, 0);
        h.paint_ops();
        assert_eq!(draw_calls(&h), vec![(r[0], 0, CMA_ICON), (r[0], 0, CMA_TEXT)]);
        let icon = h.loader.container_item_rect(h.cnr, r[0], 0, CMA_ICON).unwrap();
        assert_eq!((icon.2 - icon.0, icon.3 - icon.1), (ICON_SIZE, ICON_SIZE));
    }

    #[test]
    fn the_title_and_column_headings_are_edited() {
        let open = |f: &Fx, fi: u32, id: u32| {
            for i in 0..7 { f.wr(BUF + 0x480 + 4 * i, 0); }
            f.wr(BUF + 0x480, CNREDITDATA_SIZE);
            f.wr(BUF + 0x48C, fi);
            f.wr(BUF + 0x480 + CED_ID, id);
            f.send(CM_OPENEDIT, BUF + 0x480, 0)
        };
        let f = fixture(0);
        let fis = detail(&f, [60, 60, 60], CA_DETAILSVIEWTITLES | CA_CONTAINERTITLE, 1);
        f.loader.guest_write_bytes(BUF + 0xD00, b"Files\0").unwrap();
        for i in 0..23 { f.wr(BUF + 0x600 + 4 * i, 0); }
        f.wr(BUF + 0x600 + 16, BUF + 0xD00);
        f.send(CM_SETCNRINFO, BUF + 0x600, CMA_CNRTITLE);
        f.notes();
        // The title: its PSZ in a cell of the container.
        assert_eq!(open(&f, 0, CID_CNRTITLEWND), 1);
        let n = f.notes();
        let cell = n[0].1[4];
        assert_eq!(n, vec![(CN_BEGINEDIT, [CNREDITDATA_SIZE, f.cnr, 0, 0, cell, 0, CID_CNRTITLEWND])]);
        assert_eq!(f.rd(cell), BUF + 0xD00);
        let mle = f.loader.shared.window_mgr.lock_or_recover().find_child_by_id(f.cnr, CID_MLE).unwrap();
        assert_eq!(f.loader.shared.window_mgr.lock_or_recover().get_window(mle).unwrap().text, "Files");
        f.loader.shared.window_mgr.lock_or_recover().get_window_mut(mle).unwrap().text = "Documents".into();
        f.send(CM_CLOSEEDIT, 0, 0);
        let n = f.notes();
        assert_eq!(n[0].0, CN_REALLOCPSZ);
        // The owner gives the title a new buffer through the cell.
        f.wr(cell, BUF + 0xD40);
        f.loader.container_realloc_done(f.cnr, 1, n[0].1);
        assert_eq!(f.loader.guest_read_bytes(BUF + 0xD40, 10).unwrap(), b"Documents\0");
        f.send(CM_QUERYCNRINFO, BUF + 0x400, CNRINFO_SIZE);
        assert_eq!(f.rd(BUF + 0x400 + 16), BUF + 0xD40, "the new title");
        f.notes();
        // A heading: ppszText = the field info's pTitleData.
        assert_eq!(open(&f, fis[1], CID_LEFTCOLTITLEWND), 1);
        assert_eq!(f.notes()[0].1, [CNREDITDATA_SIZE, f.cnr, 0, fis[1], fis[1] + FI_TITLE, 0, CID_LEFTCOLTITLEWND]);
        f.send(CM_CLOSEEDIT, 0, 0);
        f.notes();
        // Alt+click on the title band edits the title.
        let title = f.loader.snapshot(f.cnr).unwrap().frame().title.unwrap();
        assert_eq!(f.send(WM_BUTTON1DOWN, Fx::at(title.0 + 5, title.1 + 2), KC_ALT << 16), 1);
        assert_eq!(f.notes()[0].1[6], CID_CNRTITLEWND);
        f.send(CM_CLOSEEDIT, 0, 0);
        f.notes();
        // Read-only titles and headings.
        f.wr(fis[2] + FI_FLTITLE, CFA_FITITLEREADONLY);
        assert_eq!(open(&f, fis[2], CID_LEFTCOLTITLEWND), 0, "CFA_FITITLEREADONLY");
        assert_eq!(open(&f, 0x1234, CID_LEFTCOLTITLEWND), 0, "not a column");
        f.set_view(CV_DETAIL | CA_DETAILSVIEWTITLES | CA_CONTAINERTITLE | CA_TITLEREADONLY);
        assert_eq!(open(&f, 0, CID_CNRTITLEWND), 0, "CA_TITLEREADONLY");
        f.set_view(CV_DETAIL);
        assert_eq!(open(&f, fis[0], CID_LEFTCOLTITLEWND), 0, "no headings shown");
        assert_eq!(open(&f, 0, CID_CNRTITLEWND), 0, "no title shown");
        let g = fixture(CCS_READONLY);
        g.set_view(CV_TEXT | CA_CONTAINERTITLE);
        assert_eq!(open(&g, 0, CID_CNRTITLEWND), 0, "CCS_READONLY");
    }

    /// A hidden built-in window draws nothing on a WM_PAINT queued while it
    /// was shown (a container's scroll bar the view stopped needing kept
    /// its picture).
    #[test]
    fn a_hidden_scroll_bar_does_not_paint() {
        let f = fixture(0);
        let sb = f.loader.shared.window_mgr.lock_or_recover().find_child_by_id(f.cnr, CID_VSCROLL).unwrap();
        let s = {
            let mut wm = f.loader.shared.window_mgr.lock_or_recover();
            let w = wm.get_window_mut(sb).unwrap();
            (w.x, w.y, w.cx, w.cy, w.visible) = (280, 1, 16, 100, false);
            let frame = wm.find_frame_for_hwnd(sb);
            wm.frame_surface(frame).unwrap()
        };
        s.lock_or_recover().record = Some(Vec::new());
        f.loader.dispatch_builtin_control(sb, WM_PAINT, 0, 0);
        assert!(s.lock_or_recover().record.take().unwrap().is_empty(), "hidden: nothing drawn");
        f.loader.shared.window_mgr.lock_or_recover().get_window_mut(sb).unwrap().visible = true;
        s.lock_or_recover().record = Some(Vec::new());
        f.loader.dispatch_builtin_control(sb, WM_PAINT, 0, 0);
        assert!(!s.lock_or_recover().record.take().unwrap().is_empty(), "shown: drawn");
    }

    // ── Stage 3: tree look and moves, grid, erasing, stacking, menus ─────

    #[test]
    fn tree_lines_follow_siblings() {
        // a ─ b ─ c   (b has a sibling d after c's subtree)
        //   │   └ c
        //   └ d
        // e
        let lines = tree_lines(&[0, 1, 2, 1, 0]);
        assert_eq!(lines[0], (0, true), "a has a later sibling (e)");
        assert_eq!(lines[1], (0, true), "b goes on to d");
        assert_eq!(lines[2], (0b1, false), "c: the line at level 0 runs through for d");
        assert_eq!(lines[3], (0, false), "d is the last child");
        assert_eq!(lines[4], (0, false));
        assert!(tree_lines(&[0, 70, 0]).len() == 3, "deep levels are bounded");
    }

    #[test]
    fn grid_arrangement_places_rows_of_squares() {
        let p = arrange_grid(5, (32, 32), (80, 70), 250, 200);
        assert_eq!(p.len(), 5);
        assert_eq!(p[0], ((80 - 32) / 2, 200 - ARRANGE_GAP / 2 - 32));
        assert_eq!(p[1].0, 80 + (80 - 32) / 2);
        assert_eq!((p[3].0, p[3].1), (p[0].0, p[0].1 - 70), "three squares a row in 250 pels");
        let (gx, gy) = default_grid((32, 32));
        assert!(gx >= 64 && gy > 32);
    }

    /// Records r0..r5 in a tree: r0 { r2, r3 { r4 } }, r1, r5.
    fn tree_fixture() -> (Fx, Vec<u32>) {
        let f = fixture(0);
        let r = f.alloc(6, 0);
        f.insert(r[0], 2, CMA_END, 0);
        f.insert(r[2], 2, CMA_END, r[0]);
        f.insert(r[4], 1, CMA_END, r[3]);
        f.insert(r[5], 1, CMA_END, 0);
        (f, r)
    }

    fn parent_of(f: &Fx, rec: u32) -> u32 {
        f.loader.with_cnr(f.cnr, |s, _| s.records.iter().find(|&&(x, _)| x == rec).map(|&(_, p)| p)).flatten().unwrap()
    }

    #[test]
    fn cm_movetree_moves_a_subtree() {
        let (f, r) = tree_fixture();
        let mv = |rec: u32, parent: u32, order: u32, sibs: u32| {
            for (o, v) in [(0, rec), (4, parent), (8, order), (12, sibs)] { f.wr(BUF + 0x300 + o, v); }
            f.send(CM_MOVETREE, BUF + 0x300, 0)
        };
        // r3 (with r4) to the top level after r1.
        assert_eq!(mv(r[3], 0, r[1], 0), 1);
        assert_eq!(parent_of(&f, r[3]), 0);
        assert_eq!(parent_of(&f, r[4]), r[3], "its child comes along");
        assert_eq!(f.send(CM_QUERYRECORD, r[1], CMA_NEXT | 1 << 16), r[3]);
        // Under itself or its own child: refused.
        assert_eq!(mv(r[3], r[4], CMA_END, 0), 0);
        assert_eq!(mv(r[3], r[3], CMA_END, 0), 0);
        // r1 and its later siblings (r3, r5) under r0, first.
        assert_eq!(mv(r[1], r[0], CMA_FIRST, 1), 1);
        for x in [r[1], r[3], r[5]] { assert_eq!(parent_of(&f, x), r[0]); }
        assert_eq!(f.send(CM_QUERYRECORD, r[0], u32::from(CMA_FIRSTCHILD) | 1 << 16), r[1]);
        assert_eq!(mv(0x1234, 0, CMA_END, 0), 0, "not a record");
        assert_eq!(mv(r[2], 0, r[2], 0), 0, "after itself");
    }

    #[test]
    fn tree_lines_and_expander_pictures_are_drawn() {
        let (f, r) = tree_fixture();
        f.set_view(CV_TREE | CV_TEXT | CA_TREELINE);
        f.send(CM_EXPANDTREE, 0, 0);
        let plain = f.paint_ops().len();
        // TREEITEMDESC on r0: icons for both states.
        let hptr = crate::loader::pm_pointer::PointerManager::system_handle(3);
        for (o, v) in [(0, 0), (4, 0), (8, hptr), (12, hptr)] { f.wr(BUF + 0x340 + o, v); }
        f.wr(r[0] + RC_PTREEITEMDESC, BUF + 0x340);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert_eq!(snap.tree_pics[0], Some((hptr, false)));
        assert_eq!(snap.expander_w(), TREE_PICTURE);
        assert!(f.paint_ops().len() > plain / 2);
        // A click on the (wider) expander still closes it.
        let ex = BORDER + snap.expander_x(0);
        f.send(WM_BUTTON1DOWN, Fx::at(ex + TREE_PICTURE - 2, 190), 0);
        assert_eq!(f.rd(r[0] + RC_ATTR) & CRA_EXPANDED, 0, "closed by its picture");
        // CNRINFO's bitmaps stand in for records without a TREEITEMDESC.
        for i in 0..23 { f.wr(BUF + 0x600 + 4 * i, 0); }
        f.wr(BUF + 0x600 + 56, 0x77);
        f.wr(BUF + 0x600 + 60, 0x78);
        f.send(CM_SETCNRINFO, BUF + 0x600, CMA_TREEBITMAP);
        f.wr(r[0] + RC_PTREEITEMDESC, 0);
        f.send(CM_EXPANDTREE, r[3], 0);
        let snap = f.loader.snapshot(f.cnr).unwrap();
        assert_eq!(snap.tree_pics[0], Some((0x78, true)), "r0 is closed: hbmCollapsed");
    }

    #[test]
    fn erased_records_come_back_when_invalidated() {
        let f = fixture(0);
        f.set_view(CV_TEXT);
        let r = f.named(&["a", "b"], 0);
        let texts = |f: &Fx| f.paint_ops().iter().filter(|o| matches!(o, crate::gui::surface::DrawOp::Text { .. })).count();
        let both = texts(&f);
        assert_eq!(f.send(CM_ERASERECORD, r[0], 0), 1);
        assert_eq!(texts(&f), both - 1, "erased");
        assert_eq!(f.send(CM_ERASERECORD, 0x1234, 0), 0);
        assert_eq!(f.send(CM_INVALIDATERECORD, f.array(&[r[1]]), 1), 1);
        assert_eq!(texts(&f), both - 1, "another record invalidated");
        f.send(CM_INVALIDATERECORD, 0, 0);
        assert_eq!(texts(&f), both, "all invalidated: back");
    }

    #[test]
    fn insertion_stacks_records_on_top_or_bottom() {
        let f = fixture(0);
        let a = f.named(&["a", "b"], 0);
        let c = f.alloc(1, 0);
        for (o, v) in [(0, 24), (RI_ORDER, CMA_END), (RI_PARENT, 0), (RI_INVALIDATE, 1), (RI_ZORDER, CMA_BOTTOM), (RI_COUNT, 1)] {
            f.wr(BUF + o, v);
        }
        f.send(CM_INSERTRECORD, c[0], BUF);
        for &r in [a[0], a[1], c[0]].iter() { f.place(r, 10, 10); }
        let snap = f.loader.snapshot(f.cnr).unwrap();
        let order: Vec<u32> = snap.z.iter().map(|&i| snap.rows[i].0).collect();
        assert_eq!(order, vec![c[0], a[0], a[1]], "CMA_BOTTOM under the others");
        // Topmost first for CMA_ZORDER.
        for (o, v) in [(0, 24), (4, 0), (8, 0), (12, 300), (16, 300), (QRFR_SEARCH, CMA_PARTIAL | CMA_ZORDER)] { f.wr(BUF + 0x500 + o, v); }
        assert_eq!(f.send(CM_QUERYRECORDFROMRECT, CMA_FIRST, BUF + 0x500), a[1]);
    }

    #[test]
    fn grid_info_and_grid_arrangement() {
        let f = fixture(0);
        let r = f.named(&["a", "b", "c"], 0);
        f.wr(BUF + 0x380, 20);
        f.loader.guest_write::<i16>(BUF + 0x384, 90).unwrap();
        f.loader.guest_write::<i16>(BUF + 0x386, 80).unwrap();
        assert_eq!(f.send(CM_SETGRIDINFO, BUF + 0x380, 0), 1);
        assert_eq!(f.send(CM_QUERYGRIDINFO, BUF + 0x3A0, 0), 1);
        let rd16 = |a: u32| f.loader.guest_read::<i16>(a).unwrap();
        assert_eq!((rd16(BUF + 0x3A4), rd16(BUF + 0x3A6)), (90, 80));
        assert_eq!(rd16(BUF + 0x3AA), (298 / 90) as i16, "columns of the work area");
        assert_eq!(f.send(CM_ARRANGE, CMA_ARRANGEGRID, 0), 1);
        assert_eq!(f.ptl(r[1]).0 - f.ptl(r[0]).0, 90, "one square apart");
        // Only the selected one, with CMA_ARRANGESELECTED.
        f.place(r[2], 200, 5);
        f.place(r[0], 222, 6);
        f.send(CM_SETRECORDEMPHASIS, r[2], 1 | CRA_SELECTED << 16);
        assert_eq!(f.send(CM_ARRANGE, CMA_ARRANGEGRID, CMA_ARRANGESELECTED), 1);
        assert_eq!(f.ptl(r[2]).0, (90 - ICON_SIZE) / 2, "the selected record to the first square");
        assert_eq!(f.ptl(r[0]), (222, 6), "the others stay");
        f.loader.guest_write::<i16>(BUF + 0x384, 0).unwrap();
        assert_eq!(f.send(CM_SETGRIDINFO, BUF + 0x380, 0), 0, "an empty square");
    }

    #[test]
    fn the_workspace_viewport() {
        let f = fixture(0);
        let r = f.named(&["far"], 0);
        f.place(r[0], 500, 300);
        f.send(CM_INVALIDATERECORD, 0, 0);
        assert_eq!(f.send(CM_QUERYVIEWPORTRECT, BUF + 0x700, u32::from(CMA_WORKSPACE)), 1);
        let rd = |o: u32| f.rd(BUF + 0x700 + o) as i32;
        assert_eq!((rd(0), rd(4)), (0, 0));
        assert!(rd(8) >= 500 + ICON_SIZE && rd(12) >= 300 + ICON_SIZE, "{} {}", rd(8), rd(12));
        // A text view: its rows.
        f.set_view(CV_TEXT);
        f.send(CM_QUERYVIEWPORTRECT, BUF + 0x700, u32::from(CMA_WORKSPACE));
        assert_eq!(rd(12), f.loader.snapshot(f.cnr).unwrap().frame().row_h);
    }

    #[test]
    fn context_menu_and_help_reach_the_owner() {
        let f = fixture(0);
        f.set_view(CV_TEXT);
        let r = f.named(&["a", "b"], 0);
        let values = |f: &Fx| -> Vec<(u32, u32)> {
            std::mem::take(&mut f.loader.shared.window_mgr.lock_or_recover().owner_calls).into_iter().filter_map(|c| match c.kind {
                OwnerCallKind::ContainerValue { code, mp2 } => Some((u32::from(code), mp2)),
                _ => None,
            }).collect()
        };
        values(&f);
        // From the pointer: the record under it (row 1 at the top).
        f.send(WM_CONTEXTMENU, Fx::at(20, 189), 1);
        assert_eq!(values(&f), vec![(CN_CONTEXTMENU, r[0])]);
        f.send(WM_CONTEXTMENU, Fx::at(20, 20), 1);
        assert_eq!(values(&f), vec![(CN_CONTEXTMENU, 0)], "over no record");
        // From the keyboard and for help: the cursored record.
        f.send(CM_SETRECORDEMPHASIS, r[1], 1 | CRA_CURSORED << 16);
        values(&f);
        f.send(WM_CONTEXTMENU, 0, 0);
        f.send(WM_HELP, 0, 0);
        assert_eq!(values(&f), vec![(CN_CONTEXTMENU, r[1]), (CN_HELP, r[1])]);
    }
}
