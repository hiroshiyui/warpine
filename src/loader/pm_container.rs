// SPDX-License-Identifier: GPL-3.0-only
//
//! `WC_CONTAINER`, first stage: records, field infos and the text, name
//! and detail views (IBM *PM Programming Reference*, "Container Controls";
//! messages, styles and structures from `<pmstddlg.h>`
//! `INCL_WINSTDCNR`).
//!
//! Records and field infos live in guest memory, carved from arenas each
//! container takes from the memory manager ([`RecordHeap`]); the guest's
//! copy is the only one — texts, positions and attributes are read when
//! needed, and only `flRecordAttr` is written back (ADR-0043). The host
//! keeps the insertion order, each record's parent and the inserted field
//! infos ([`CnrState`]), and accepts only pointers it handed out.
//!
//! Views: `CV_TEXT` (`pszText`), `CV_NAME` (`pszName`) and `CV_DETAIL`
//! (one column per visible field info, read at `offStruct` in each record:
//! `CFA_STRING`, `CFA_ULONG`, `CFA_DATE`, `CFA_TIME`), with `CV_FLOW` for
//! the text and name views, the container title (`CA_CONTAINERTITLE`) and
//! detail column titles (`CA_DETAILSVIEWTITLES`). A `MINIRECORDCORE`
//! container shows `pszIcon` in every view. Not drawn yet: icons and
//! bitmaps, the icon view (records are listed by `pszIcon`) and the tree
//! view (top-level records listed by `pszTree`), scroll bars, the split bar.
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
pub const CM_QUERYRECORDRECT: u32 = 0x0343;
pub const CM_QUERYVIEWPORTRECT: u32 = 0x0344;
pub const CM_REMOVEDETAILFIELDINFO: u32 = 0x0345;
pub const CM_REMOVERECORD: u32 = 0x0346;
pub const CM_SCROLLWINDOW: u32 = 0x0347;
pub const CM_SEARCHSTRING: u32 = 0x0348;
pub const CM_SETCNRINFO: u32 = 0x0349;
pub const CM_SETRECORDEMPHASIS: u32 = 0x034a;
pub const CM_SORTRECORD: u32 = 0x034b;
pub const CM_COLLAPSETREE: u32 = 0x034e;
pub const CM_EXPANDTREE: u32 = 0x034f;
pub const CM_QUERYRECORDINFO: u32 = 0x0350;
pub const CM_INSERTRECORDARRAY: u32 = 0x0351;

pub const CN_DRAGLEAVE: u32 = 102;
pub const CN_DRAGOVER: u32 = 103;
pub const CN_DROP: u32 = 104;
pub const CN_ENTER: u32 = 106;
pub const CN_INITDRAG: u32 = 107;
pub const CN_EMPHASIS: u32 = 108;
pub const CN_KILLFOCUS: u32 = 109;
pub const CN_SETFOCUS: u32 = 112;
pub const CN_SCROLL: u32 = 110;
pub const CN_COLLAPSETREE: u32 = 116;
pub const CN_EXPANDTREE: u32 = 117;

pub const CCS_EXTENDSEL: u32 = 0x0000_0001;
pub const CCS_MULTIPLESEL: u32 = 0x0000_0002;
pub const CCS_MINIRECORDCORE: u32 = 0x0000_0040;

pub const CV_TEXT: u32 = 0x0000_0001;
pub const CV_NAME: u32 = 0x0000_0002;
pub const CV_ICON: u32 = 0x0000_0004;
pub const CV_DETAIL: u32 = 0x0000_0008;
pub const CV_FLOW: u32 = 0x0000_0010;
pub const CV_TREE: u32 = 0x0000_0040;

pub const CA_CONTAINERTITLE: u32 = 0x0000_0200;
pub const CA_TITLESEPARATOR: u32 = 0x0000_0400;
pub const CA_TITLELEFT: u32 = 0x0000_0800;
pub const CA_TITLERIGHT: u32 = 0x0000_1000;
pub const CA_DETAILSVIEWTITLES: u32 = 0x0000_8000;

pub const CFA_RIGHT: u32 = 0x0000_0002;
pub const CFA_CENTER: u32 = 0x0000_0004;
pub const CFA_INVISIBLE: u32 = 0x0000_0040;
pub const CFA_BITMAPORICON: u32 = 0x0000_0100;
pub const CFA_SEPARATOR: u32 = 0x0000_0200;
pub const CFA_HORZSEPARATOR: u32 = 0x0000_0400;
pub const CFA_STRING: u32 = 0x0000_0800;
pub const CFA_DATE: u32 = 0x0000_2000;
pub const CFA_TIME: u32 = 0x0000_4000;
pub const CFA_ULONG: u32 = 0x0002_0000;

pub const CRA_SELECTED: u32 = 0x0000_0001;
pub const CRA_CURSORED: u32 = 0x0000_0004;
pub const CRA_FILTERED: u32 = 0x0000_0010;
pub const CRA_DISABLED: u32 = 0x0000_1000;
pub const CRA_EXPANDED: u32 = 0x0000_0080;
pub const CRA_COLLAPSED: u32 = 0x0000_0100;

/// The id of a container's vertical scroll bar (`<pmstddlg.h>`).
pub const CID_VSCROLL: u32 = 0x7FF9;

pub const CMA_PARENT: u16 = 0x0001;
pub const CMA_FIRSTCHILD: u16 = 0x0002;
pub const CMA_LASTCHILD: u16 = 0x0004;
pub const CMA_FIRST: u32 = 0x0010;
pub const CMA_LAST: u32 = 0x0020;
pub const CMA_END: u32 = 0x0040;
pub const CMA_PREV: u32 = 0x0080;
pub const CMA_NEXT: u32 = 0x0100;
pub const CMA_VERTICAL: u32 = 0x0400;
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
pub const RC_PSZICON: u32 = 20;
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
/// `RECORDINSERT` / `FIELDINFOINSERT` / `SEARCHSTRING` /
/// `QUERYRECORDRECT` field offsets.
const RI_ORDER: u32 = 4;
const RI_PARENT: u32 = 8;
const RI_INVALIDATE: u32 = 12;
const RI_COUNT: u32 = 20;
const FII_ORDER: u32 = 4;
const FII_INVALIDATE: u32 = 8;
const FII_COUNT: u32 = 12;
const SS_PSZ: u32 = 4;
const SS_PREFIX: u32 = 8;
const SS_CASE: u32 = 12;
const SS_VIEW: u32 = 16;
const QRR_RECORD: u32 = 4;

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
const SELECT_COLOR: u32 = 0x0000_0080;
const SELECT_TEXT_COLOR: u32 = 0x00FF_FFFF;
const DISABLED_TEXT_COLOR: u32 = 0x0080_8080;
const SEPARATOR_COLOR: u32 = 0x0080_8080;
/// Breadth of the vertical scroll bar.
const SCROLLBAR_W: i32 = 16;
/// Tree indentation per level when `cxTreeIndent` is not set.
const TREE_INDENT: i32 = 16;
/// Side of a tree view's expander box.
const EXPANDER: i32 = 9;

/// What an allocated item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind { Record, Field }

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
}

impl CnrState {
    pub fn new(style: u32) -> Self {
        CnrState {
            heap: RecordHeap::default(), records: Vec::new(), inserted: HashSet::new(), fields: Vec::new(),
            info: CnrInfo { window_attr: CV_ICON, ..CnrInfo::default() },
            top: 0, anchor: None, mini: style & CCS_MINIRECORDCORE != 0, sort: None, next_sort_id: 0,
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
        let at = match order {
            CMA_END => self.records.len(),
            CMA_FIRST => self.records.iter().position(|&(_, p)| p == parent)
                .or_else(|| self.records.iter().position(|&(r, _)| r == parent).map(|i| i + 1))
                .unwrap_or(self.records.len()),
            after => match self.records.iter().position(|&(r, p)| r == after && p == parent) {
                Some(i) => i + 1,
                None => return Err(CnrError::Invalid),
            },
        };
        self.records.splice(at..at, recs.iter().map(|&r| (r, parent)));
        self.inserted.extend(recs.iter().copied());
        Ok(self.records.len() as u32)
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

    fn new_sort_id(&mut self) -> u32 {
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
    pub fn new(attr: u32, line_spacing: i32, cx: i32, cy: i32) -> Frame {
        let row_h = LINE_H + 2 * ROW_PAD + line_spacing.clamp(0, 64);
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

/// A detail-view column: its field info, flags, left edge and width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Column { fi: u32, fl_data: u32, fl_title: u32, x: i32, w: i32 }

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
}

impl Snapshot {
    fn view(&self) -> View { view_of(self.info.window_attr) }

    fn flow(&self) -> bool {
        self.info.window_attr & CV_FLOW != 0 && matches!(self.view(), View::Text | View::Name)
    }

    /// Whether the rows need the vertical scroll bar (not in flowed views).
    fn bar(&self) -> bool {
        !self.flow() && self.rows.len() > self.plain_frame().capacity()
    }

    fn plain_frame(&self) -> Frame {
        Frame::new(self.info.window_attr, self.info.line_spacing as i32, self.cx, self.cy)
    }

    /// The view's frame, the rows narrowed by the scroll bar when shown.
    fn frame(&self) -> Frame {
        let mut f = self.plain_frame();
        if self.bar() { f.rows.2 = (f.rows.2 - SCROLLBAR_W).max(0); }
        f
    }

    /// The first row that can be on top (the last page starts there).
    fn max_top(&self) -> usize {
        if self.flow() { self.rows.len().saturating_sub(1) } else { self.rows.len().saturating_sub(self.frame().capacity()) }
    }

    fn indent(&self) -> i32 {
        match self.info.tree_indent as i32 { i if i > 0 => i.min(256), _ => TREE_INDENT }
    }

    /// Where row `i`'s text starts, from its rectangle's left edge (tree
    /// view: after its indentation and expander).
    fn text_inset(&self, i: usize) -> i32 {
        if self.view() != View::Tree { return 0; }
        let depth = self.tree.get(i).map_or(0, |t| i32::from(t.0)).min(64);
        depth * self.indent() + EXPANDER + 2 * CELL_PAD
    }
}

impl WindowManager {
    /// Give a new `WC_CONTAINER` window its state and its vertical scroll
    /// bar (`CID_VSCROLL`, hidden until the rows need it).
    pub fn build_container(&mut self, hwnd: u32) {
        let Some(w) = self.get_window_mut(hwnd) else { return };
        if w.class_name != CLASS_CONTAINER || w.container.is_some() { return; }
        w.container = Some(Box::new(CnrState::new(w.style)));
        let hmq = w.hmq;
        let bar = self.create_window("WC_SCROLLBAR".to_string(), hwnd, hmq);
        if let Some(b) = self.get_window_mut(bar) {
            (b.id, b.owner, b.style, b.visible) = (CID_VSCROLL, hwnd, SBS_VERT, false);
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
        let (cx, cy, info, mini, top, records, sizes, fields) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let w = wm.get_window(hwnd)?;
            let s = w.container.as_deref()?;
            let size = |a: u32| s.heap.item(a).map_or(0, |i| i.size);
            let sizes: HashMap<u32, u32> = s.records.iter().map(|&(r, _)| (r, size(r))).collect();
            let fields: Vec<(u32, u32)> = s.fields.iter().map(|&f| (f, size(f))).collect();
            (w.cx, w.cy, s.info, s.mini, s.top, s.records.clone(), sizes, fields)
        };
        let shown = |r: u32| self.rec_attr(r) & CRA_FILTERED == 0;
        let size = |r: u32| sizes.get(&r).copied().unwrap_or(0);
        type Rows = (Vec<(u32, u32)>, Vec<(u16, bool)>);
        let (rows, tree): Rows = if view_of(info.window_attr) == View::Tree {
            tree_order(&records, &|r| self.rec_attr(r) & CRA_EXPANDED != 0, &shown)
                .into_iter().map(|(r, d, k)| ((r, size(r)), (d, k))).unzip()
        } else {
            records.iter().filter(|&&(r, p)| p == 0 && shown(r)).map(|&(r, _)| ((r, size(r)), (0, false))).unzip()
        };
        Some(Snapshot { cx, cy, info, mini, top, rows, tree, fields })
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
    /// 0 = as wide as the title and the widest value).
    fn columns(&self, hwnd: u32, snap: &Snapshot, font: crate::gui::font::FontSpec, x0: i32) -> Vec<Column> {
        let mut x = x0;
        let mut out = Vec::new();
        for &(fi, _) in &snap.fields {
            let rd = |o: u32| self.guest_read::<u32>(fi.wrapping_add(o)).unwrap_or(0);
            let (fl_data, fl_title, width) = (rd(FI_FLDATA), rd(FI_FLTITLE), rd(FI_CXWIDTH));
            if fl_data & CFA_INVISIBLE != 0 { continue; }
            let w = if width != 0 { (width as i32).min(super::constants::MAX_WINDOW_COORD) } else {
                let title = if fl_title & CFA_BITMAPORICON == 0 { self.cnr_text(hwnd, rd(FI_TITLE)) } else { String::new() };
                let mut w = crate::gui::font::text_width(&title, font);
                for &(r, size) in &snap.rows {
                    w = w.max(crate::gui::font::text_width(&self.field_value(hwnd, r, size, fi, fl_data), font));
                }
                w
            } + 2 * CELL_PAD;
            out.push(Column { fi, fl_data, fl_title, x, w });
            x = x.saturating_add(w + if fl_data & CFA_SEPARATOR != 0 { 1 } else { 0 });
        }
        out
    }

    /// Where the shown records of `snap` are: `(row index, rectangle)`.
    fn placements(&self, hwnd: u32, snap: &Snapshot, font: crate::gui::font::FontSpec) -> (Frame, Placed) {
        let f = snap.frame();
        let view = snap.view();
        let flow = snap.flow();
        let widths: Vec<i32> = if flow {
            snap.rows.iter().map(|&(r, _)| crate::gui::font::text_width(&self.record_text(hwnd, r, view, snap.mini), font)).collect()
        } else { vec![0; snap.rows.len()] };
        let top = snap.top.min(snap.rows.len().saturating_sub(1));
        (f, place_list(&f, &widths, top, flow))
    }

    /// The shown record at window point `(x, y)`.
    fn record_at(&self, hwnd: u32, snap: &Snapshot, (x, y): (i32, i32)) -> Option<usize> {
        let font = Painter::new(self, 0, hwnd).font();
        let (_, placed) = self.placements(hwnd, snap, font);
        placed.into_iter().find(|&(_, (rx, ry, rw, rh))| x >= rx && x < rx + rw && y >= ry && y < ry + rh).map(|(i, _)| i)
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
            if let Some(m) = self.guest_slice_mut(a, size as usize) { m.fill(0); }
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
            WM_BUTTON1DOWN => ok(self.cnr_click(hwnd, style, mp1, mp2)),
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
                let recs = if msg == CM_INSERTRECORD { self.read_chain(mp1, n, RC_NEXT) } else { self.read_ptr_array(mp1, n) };
                let Some(recs) = recs.filter(|r| !r.is_empty()) else { return Some(ApiResult::Normal(0)) };
                match self.with_cnr(hwnd, |s, _| s.insert(&recs, order, parent).map(|c| (c, s.info.sort_record))) {
                    // With a sort function (CNRINFO.pSortRecord) the
                    // parent's children are sorted before the call returns.
                    Some(Ok((count, sort))) if sort != 0 => self.cnr_start_sort(hwnd, sort, 0, Some(parent), count, true),
                    Some(Ok((count, _))) => {
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
                self.cnr_clamp_top(hwnd);
                self.repaint_control(hwnd);
                ApiResult::Normal(self.with_cnr(hwnd, |s, _| s.records.len() as u32).unwrap_or(0))
            }
            // mp1 = reference record, mp2 = MPFROM2SHORT(cmd, order).
            CM_QUERYRECORD => match self.with_cnr(hwnd, |s, _| s.query(mp1, lo)) {
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
                if mp1 & 0xFFFF == CMA_VERTICAL {
                    let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
                    let rows = (mp2 as i32) / snap.frame().row_h;
                    self.cnr_scroll_to(hwnd, &snap, snap.top as i64 + i64::from(rows));
                }
                ok(true)
            }
            // From the vertical scroll bar: mp2 = MPFROM2SHORT(slider, SB_*).
            WM_VSCROLL => {
                let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
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
            // mp1 = record (NULL: every record).
            CM_EXPANDTREE | CM_COLLAPSETREE => ok(self.cnr_expand(hwnd, mp1, msg == CM_EXPANDTREE)),
            // mp1 = PSEARCHSTRING, mp2 = start after (CMA_FIRST = from the start).
            CM_SEARCHSTRING => self.cnr_search(hwnd, mini, mp1, mp2).map_or(minus1, ApiResult::Normal),
            // mp1 = PRECTL out, mp2 = PQUERYRECORDRECT.
            CM_QUERYRECORDRECT => {
                let rec = self.guest_read::<u32>(mp2.wrapping_add(QRR_RECORD)).unwrap_or(0);
                let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
                let font = Painter::new(self, 0, hwnd).font();
                let (_, placed) = self.placements(hwnd, &snap, font);
                let Some(&(_, (x, y, w, h))) = placed.iter().find(|&&(i, _)| snap.rows[i].0 == rec) else { return Some(ok(false)) };
                ok(self.write_rectl(mp1, super::pm_rect::Rectl { x_left: x, y_bottom: y, x_right: x + w, y_top: y + h }))
            }
            // mp1 = PRECTL out, mp2 = MPFROM2SHORT(CMA_WINDOW / CMA_WORKSPACE, right split window).
            CM_QUERYVIEWPORTRECT => {
                if lo != u32::from(CMA_WINDOW) { return Some(ok(false)); }
                let Some(snap) = self.snapshot(hwnd) else { return Some(ok(false)) };
                let (x, y, w, h) = snap.frame().rows;
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
        self.with_cnr(hwnd, |s, _| s.top = top);
        let pels = (top as i64 - snap.top as i64) * i64::from(snap.frame().row_h);
        self.shared.window_mgr.lock_or_recover().queue_owner_call(OwnerCall {
            control: hwnd, kind: OwnerCallKind::ContainerNotify { code: CN_SCROLL as u16, block: [hwnd, pels as i32 as u32, CMA_VERTICAL] },
        });
        self.repaint_control(hwnd);
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
                self.repaint_control(hwnd);
                ChainEnd::Return(if applied || keep { ret } else { 0 })
            }
            // The container is gone or another sort replaced this one.
            None => ChainEnd::Return(if keep { ret } else { 0 }),
        }
    }

    /// Scroll so that record `rec` is shown.
    fn cnr_follow(&self, hwnd: u32, rec: u32) {
        let Some(snap) = self.snapshot(hwnd) else { return };
        let Some(cursor) = snap.rows.iter().position(|&(r, _)| r == rec) else { return };
        let font = Painter::new(self, 0, hwnd).font();
        let (f, placed) = self.placements(hwnd, &snap, font);
        let flow = snap.flow();
        let top = scroll_to(snap.top, cursor, f.capacity(), flow, &placed, f.rows.0 + f.rows.2);
        self.with_cnr(hwnd, |s, _| s.top = top);
    }

    /// The shown record at container point `local`, or 0 (white space).
    pub(crate) fn container_record_at(&self, hwnd: u32, local: (i32, i32)) -> u32 {
        self.snapshot(hwnd).and_then(|snap| self.record_at(hwnd, &snap, local).map(|i| snap.rows[i].0)).unwrap_or(0)
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
            control: hwnd, kind: OwnerCallKind::ContainerInitDrag { block },
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
                let ex = BORDER + snap.text_inset(i) - EXPANDER - CELL_PAD;
                if (ex - 2..=ex + EXPANDER + 1).contains(&at.0) {
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
        let clamp = |v: usize| v.min(i16::MAX as usize) as i16;
        w.scrollbar.set_range(clamp(snap.top), 0, clamp(snap.max_top()));
        w.scrollbar.thumb_size = Some((cap.min(u16::MAX as usize) as u16, n.min(u16::MAX as usize) as u16));
        wm.invalidate_window_only(sb);
    }

    /// Paint container `hwnd` at `(x, y, cx, cy)` in its frame's surface.
    pub(super) fn paint_container(&self, p: &Painter, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32)) {
        let Some(snap) = self.snapshot(hwnd) else { return };
        let font = p.font();
        let attr = snap.info.window_attr;
        let view = view_of(attr);
        let bg = p.own_bg.unwrap_or(FIELD_COLOR);
        let at = |(rx, ry, rw, rh): (i32, i32, i32, i32)| (x + rx, y + ry, x + rx + rw - 1, y + ry + rh - 1);
        p.fill((x, y, x + cx - 1, y + cy - 1), bg);
        // The border.
        p.fill((x, y, x + cx - 1, y), BORDER_COLOR);
        p.fill((x, y + cy - 1, x + cx - 1, y + cy - 1), BORDER_COLOR);
        p.fill((x, y, x, y + cy - 1), BORDER_COLOR);
        p.fill((x + cx - 1, y, x + cx - 1, y + cy - 1), BORDER_COLOR);
        let (f, placed) = self.placements(hwnd, &snap, font);
        let fg = p.fg;
        let aligned = |l: i32, r: i32, w: i32, fl: u32| {
            if fl & CFA_RIGHT != 0 { r - CELL_PAD - w } else if fl & CFA_CENTER != 0 { l + (r - l + 1 - w) / 2 } else { l + CELL_PAD }
        };
        // The container title.
        if let Some(t) = f.title {
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
        let columns = if view == View::Detail { self.columns(hwnd, &snap, font, BORDER) } else { Vec::new() };
        // Detail column titles.
        if let Some(hd) = f.header {
            let (l, b, r, _) = at(hd);
            for c in &columns {
                let title = if c.fl_title & CFA_BITMAPORICON == 0 {
                    self.cnr_text(hwnd, self.guest_read::<u32>(c.fi.wrapping_add(FI_TITLE)).unwrap_or(0))
                } else { String::new() };
                let (cl, cr) = (x + c.x, x + c.x + c.w - 1);
                let cp = p.clone_clipped((cl, b, (cr - cl + 1).max(0), hd.3));
                cp.text(aligned(cl, cr, crate::gui::font::text_width(&title, font), c.fl_title), b + 1 + ROW_PAD, &title, fg);
                if c.fl_data & CFA_SEPARATOR != 0 { p.fill((cr + 1, y + f.rows.1, cr + 1, b + hd.3 - 1), SEPARATOR_COLOR); }
            }
            if columns.iter().any(|c| (c.fl_title | c.fl_data) & CFA_HORZSEPARATOR != 0) || !columns.is_empty() {
                p.fill((l, b, r, b), SEPARATOR_COLOR);
            }
        }
        // The records.
        let rows_clip = at(f.rows);
        let rp = p.clone_clipped((rows_clip.0, rows_clip.1, f.rows.2, f.rows.3));
        for &(i, r) in &placed {
            let (rec, size) = snap.rows[i];
            let a = self.rec_attr(rec);
            let (l, b, rr, t) = at(r);
            let selected = a & CRA_SELECTED != 0;
            let color = if selected { SELECT_TEXT_COLOR } else if a & CRA_DISABLED != 0 { DISABLED_TEXT_COLOR } else { fg };
            if view == View::Detail {
                let right = columns.last().map_or(rr, |c| (x + c.x + c.w - 1).min(rr));
                if selected { rp.fill((l, b, right, t), SELECT_COLOR); }
                for c in &columns {
                    let (cl, cr) = (x + c.x, x + c.x + c.w - 1);
                    let v = self.field_value(hwnd, rec, size, c.fi, c.fl_data);
                    let cp = rp.clone_clipped((cl, b, (cr - cl + 1).max(0), r.3));
                    cp.text(aligned(cl, cr, crate::gui::font::text_width(&v, font), c.fl_data), b + ROW_PAD, &v, color);
                }
                if focused && a & CRA_CURSORED != 0 { super::pm_button::focus_mark(&rp, (l, b, right, t), color); }
            } else {
                let text = self.record_text(hwnd, rec, view, snap.mini);
                let l = l + snap.text_inset(i);
                if view == View::Tree && snap.tree[i].1 {
                    // The expander: a box with a minus, or a plus when closed.
                    let ex = l - EXPANDER - CELL_PAD;
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
                if selected { rp.fill((l, b, right, t), SELECT_COLOR); }
                rp.text(l + CELL_PAD, b + ROW_PAD, &text, color);
                if focused && a & CRA_CURSORED != 0 { super::pm_button::focus_mark(&rp, (l, b, right, t), color); }
            }
        }
        // Column separators through the rows.
        for c in columns.iter().filter(|c| c.fl_data & CFA_SEPARATOR != 0) {
            let cr = x + c.x + c.w;
            rp.fill((cr, rows_clip.1, cr, rows_clip.3), SEPARATOR_COLOR);
        }
        self.cnr_sync_scrollbar(hwnd, &snap);
    }
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

    #[test]
    fn the_scroll_bar_follows_the_rows() {
        let f = fixture(0);
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
        let ex = BORDER + snap.text_inset(0) - EXPANDER - CELL_PAD;
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
        let f = Frame::new(CV_DETAIL | CA_DETAILSVIEWTITLES | CA_CONTAINERTITLE | CA_TITLESEPARATOR, 0, 200, 100);
        assert_eq!(f.title, Some((1, 80, 198, 19)));
        assert_eq!(f.header, Some((1, 61, 198, 19)));
        assert_eq!(f.rows, (1, 1, 198, 60));
        assert_eq!(f.capacity(), 3);
        let plain = Frame::new(CV_TEXT | CA_DETAILSVIEWTITLES, 2, 200, 100);
        assert_eq!((plain.title, plain.header, plain.row_h), (None, None, 20));
    }

    #[test]
    fn lists_place_rows_or_flowed_columns() {
        let f = Frame::new(CV_TEXT, 0, 200, 56);
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
}
