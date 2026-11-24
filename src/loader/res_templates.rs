// SPDX-License-Identifier: GPL-3.0-or-later
//
// The parsers of the resources a running program hands Warpine: dialog
// templates (`DLGTEMPLATE`), menu templates (`MENUTEMPLATE`), accelerator
// tables (`ACCELTABLE`), string and message bundles (`RT_STRING` /
// `RT_MESSAGE`) and `PRESPARAMS` blocks. All of it is untrusted guest data.
//
// The module is pure: no guest memory, no window manager. Code page
// decoding and class-atom names come in as functions, so the fuzz target
// (fuzz/fuzz_targets/res_templates.rs, ADR-0112) includes this file by
// #[path] as bitmap.rs includes bitmap_codec.rs. Callers keep their names:
// `pm_dialog`, `pm_types`, `pm_winmisc` and `pm_presparam` re-export.

use super::constants::{MIS_END, MIS_SEPARATOR, MIS_SUBMENU};

/// A string decoder: the bytes and the code page they are in.
pub type Decode<'a> = &'a dyn Fn(&[u8], u32) -> String;

fn u16_at(data: &[u8], off: usize) -> Option<u16> {
    data.get(off..off.checked_add(2)?).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

// ── DLGTEMPLATE (<pmwin.h>) ──────────────────────────────────────────────────

/// `DLGTEMPLATE.codepage`: codepage of the template's texts.
const DT_CODEPAGE: usize = 4;
/// `DLGTEMPLATE.offadlgti`: offset of the first `DLGTITEM`.
const DT_OFF_ITEMS: usize = 6;
/// `DLGTEMPLATE.iItemFocus`: index of the item that gets the focus.
const DT_ITEM_FOCUS: usize = 10;
/// Size of the `DLGTEMPLATE` header before the items.
pub const DT_HEADER_SIZE: usize = 14;

/// Size of one `DLGTITEM`.
pub const DLGTITEM_SIZE: usize = 30;
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
pub const DI_OFF_PRESPARAMS: usize = 26;
const DI_OFF_CTLDATA: usize = 28;

/// Offset value meaning "none" (`offCtlData`, `offPresParams`, `iItemFocus`).
pub const NO_OFFSET: u16 = 0xFFFF;

/// One `DLGTITEM` of a template, with its strings resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct DlgItem {
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
    /// The item's control data (`CTLDATA`, e.g. a slider's `SLDCDATA`), at
    /// most [`MAX_DLG_CTL_DATA`] bytes; empty when it has none.
    pub ctl_data: Vec<u8>,
}

/// Most control data bytes kept for a dialog item: enough for the
/// structures the built-in controls read from it (`SLDCDATA`, `VSCDATA`).
pub const MAX_DLG_CTL_DATA: usize = 64;

/// A parsed `DLGTEMPLATE`.
#[derive(Debug, Clone, PartialEq)]
pub struct DlgTemplate {
    /// The dialog frame (the first item): title, position, size and style.
    pub dialog: DlgItem,
    /// `FCF_*` frame flags: the template's frame control data (and, from
    /// `pm_dialog::parse_dlg_template`, the border its `FS_*` style asks for).
    pub frame_flags: u32,
    /// Index into `items` of the control that gets the focus first, when the
    /// template names one.
    pub focus: Option<usize>,
    /// The controls, in template order (parents before their children).
    pub items: Vec<DlgItem>,
}

/// The control data at `off` in the template: as many bytes as its leading
/// size field (`cbSize`, a ULONG) gives, bounded by the template's end and
/// [`MAX_DLG_CTL_DATA`].
pub(super) fn ctl_data_at(data: &[u8], off: u16) -> Vec<u8> {
    if off == NO_OFFSET {
        return Vec::new();
    }
    let off = off as usize;
    let len = u32_at(data, off).map_or(0, |cb| (cb as usize).min(MAX_DLG_CTL_DATA));
    data.get(off..).map_or_else(Vec::new, |rest| rest[..len.min(rest.len())].to_vec())
}

/// The string at `off` in the template: at most `cch` bytes (all the way to
/// the end when `cch` is 0), cut at the first NUL, decoded with `codepage`.
fn template_string(data: &[u8], off: u16, cch: u16, codepage: u32, decode: Decode) -> String {
    let off = off as usize;
    if off == NO_OFFSET as usize || off >= data.len() {
        return String::new();
    }
    let end = if cch == 0 { data.len() } else { (off + cch as usize).min(data.len()) };
    let bytes = &data[off..end];
    decode(&bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())], codepage)
}

/// Read the `DLGTITEM` at `off`; returns the item, its `cChildren` and its
/// control data offset. A class given as an atom (`cchClassName` 0) is
/// named by `class_atom(offClassName)`.
fn read_item(data: &[u8], off: usize, codepage: u32, decode: Decode, class_atom: &dyn Fn(u16) -> String)
             -> Option<(DlgItem, u16, u16)> {
    let item = data.get(off..off.checked_add(DLGTITEM_SIZE)?)?;
    let w = |o| u16_at(item, o).unwrap_or(0);
    let class_name = match w(DI_CCH_CLASS) {
        0 => class_atom(w(DI_OFF_CLASS)),
        cch => template_string(data, w(DI_OFF_CLASS), cch, codepage, decode),
    };
    Some((DlgItem {
        class_name,
        text: template_string(data, w(DI_OFF_TEXT), w(DI_CCH_TEXT), codepage, decode),
        x: w(DI_X) as i16,
        y: w(DI_Y) as i16,
        cx: w(DI_CX) as i16,
        cy: w(DI_CY) as i16,
        id: w(DI_ID),
        style: u32_at(item, DI_STYLE).unwrap_or(0),
        parent: None,
        pres_params: match w(DI_OFF_PRESPARAMS) {
            NO_OFFSET => Vec::new(),
            off => data.get(off as usize..).map_or_else(Vec::new, parse_presparams),
        },
        ctl_data: ctl_data_at(data, w(DI_OFF_CTLDATA)),
    }, w(DI_CHILDREN), w(DI_OFF_CTLDATA)))
}

/// Parse a `DLGTEMPLATE` resource. Texts are decoded with the template's
/// codepage, or `fallback_cp` when it gives none. `None` when the data is
/// too short to hold the header and the dialog item.
///
/// The frame control data of the dialog item holds its `FCF_*` flags as one
/// ULONG — that is what the Open Watcom resource compiler writes for
/// `DIALOG ... , FCF_TITLEBAR | FCF_SYSMENU` (verified on
/// `samples/pm_menu_dlg_test`), not a `FRAMECDATA` structure.
pub fn parse_dlg_template(data: &[u8], fallback_cp: u32, decode: Decode, class_atom: &dyn Fn(u16) -> String)
                          -> Option<DlgTemplate> {
    if data.len() < DT_HEADER_SIZE + DLGTITEM_SIZE {
        return None;
    }
    let codepage = match u16_at(data, DT_CODEPAGE)? { 0 => fallback_cp, cp => cp as u32 };
    let first = u16_at(data, DT_OFF_ITEMS)? as usize;
    let (dialog, children, ctl_off) = read_item(data, first, codepage, decode, class_atom)?;
    let frame_flags = match ctl_off {
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
            read_item(data, first + (items.len() + 1) * DLGTITEM_SIZE, codepage, decode, class_atom) else { break };
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

// ── ACCELTABLE ───────────────────────────────────────────────────────────────

/// `ACCELTABLE` layout: `cAccel` (USHORT), `codepage` (USHORT), then
/// `ACCEL { fs, key, cmd }` entries of three USHORTs.
pub const AT_HEADER_SIZE: usize = 4;
pub const ACCEL_SIZE: usize = 6;

/// One `ACCEL` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccelEntry {
    pub flags: u16,
    pub key: u16,
    pub cmd: u16,
}

/// Parse an `ACCELTABLE` resource; entries past the end of `data` are dropped.
pub fn parse_accel_table(data: &[u8]) -> Vec<AccelEntry> {
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

// ── RT_STRING / RT_MESSAGE bundles ───────────────────────────────────────────

/// Strings in one `RT_STRING` / `RT_MESSAGE` bundle.
pub const STRINGS_PER_BUNDLE: u32 = 16;
/// Size of a bundle's codepage header.
const BUNDLE_HEADER: usize = 2;

/// String `index` of a bundle (a USHORT codepage, then 16 entries of a
/// length byte and the string), without its terminating NUL; `None` past
/// the bundle's end.
pub fn bundle_string(data: &[u8], index: usize) -> Option<&[u8]> {
    let mut off = BUNDLE_HEADER;
    for i in 0..STRINGS_PER_BUNDLE as usize {
        let len = *data.get(off)? as usize;
        let text = data.get(off + 1..off + 1 + len)?;
        if i == index {
            return Some(text.strip_suffix(&[0]).unwrap_or(text));
        }
        off += 1 + len;
    }
    None
}

// ── PRESPARAMS ───────────────────────────────────────────────────────────────

/// Most attributes one window keeps; setting another fails.
pub const MAX_PRESPARAMS: usize = 64;
/// Longest value of one attribute, in bytes.
pub const MAX_PRESPARAM_LEN: usize = 1024;
/// Size of `PRESPARAMS.cb` and of `PARAM.id` / `PARAM.cb`.
const ULONG_LEN: usize = 4;

/// The attributes of a `PRESPARAMS` block (`ULONG cb` of the `PARAM`s that
/// follow; each `PARAM` is `ULONG id`, `ULONG cb`, `cb` bytes). Parsing
/// stops at the end of `cb`, of `block`, at a `PARAM` that does not fit, or
/// after [`MAX_PRESPARAMS`] entries; over-long values are skipped.
pub fn parse_presparams(block: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let u32_at = |off: usize| -> Option<u32> {
        Some(u32::from_le_bytes(block.get(off..off.checked_add(ULONG_LEN)?)?.try_into().ok()?))
    };
    let Some(cb) = u32_at(0) else { return Vec::new() };
    let end = (cb as usize).saturating_add(ULONG_LEN).min(block.len());
    let mut out = Vec::new();
    let mut at = ULONG_LEN;
    while out.len() < MAX_PRESPARAMS && at + 2 * ULONG_LEN <= end {
        let (Some(id), Some(len)) = (u32_at(at), u32_at(at + ULONG_LEN)) else { break };
        let start = at + 2 * ULONG_LEN;
        let Some(stop) = start.checked_add(len as usize).filter(|&s| s <= end) else { break };
        if (len as usize) <= MAX_PRESPARAM_LEN {
            out.push((id, block[start..stop].to_vec()));
        }
        at = stop;
    }
    out
}

// ── MENUTEMPLATE ─────────────────────────────────────────────────────────────

/// A single item from a parsed OS/2 MENUTEMPLATE binary resource.
///
/// Stored in `OS2Window::menu_items` for WC_MENU windows created by
/// `WinLoadMenu` / `WinCreateMenu`.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub id: u16,
    /// `afStyle` field from the binary resource (MIS_* flags, bit 15 stripped).
    pub style: u16,
    /// `afAttribute` field (MIA_* flags).
    pub attr: u16,
    pub text: String,
    /// Nested items for `MIS_SUBMENU` entries.
    pub children: Vec<MenuItem>,
    /// What the owner drew for a `MIS_OWNERDRAW` pull-down item
    /// (`pm_owner.rs`): `owner_menu_item_w()` × `MENU_DROPDOWN_ITEM_H`
    /// pels, rows top down; `None` = drawn as text.
    pub image: Option<std::sync::Arc<Vec<u32>>>,
    /// For an item with a submenu: the height of its pull-down's rows, as
    /// the owner measured its `MIS_OWNERDRAW` items (`WM_MEASUREITEM`,
    /// `pm_owner.rs`); 0 = not measured, the default height.
    pub row_h: i32,
}

/// The `MENUTEMPLATE` header (and a submenu's child-block header) as Open
/// Watcom's `wrc` writes it: ULONG size, USHORT codepage, USHORT reserved,
/// USHORT item count.
const MENU_HEADER: usize = 10;
const MENU_CODEPAGE: usize = 4;
const MENU_COUNT: usize = 8;
/// `afStyle`, `afAttribute`, `id`: the fixed part of an item.
const MENU_ITEM_FIXED: usize = 6;
/// The deepest submenu nesting read; deeper submenus are left empty, so
/// a hostile template cannot recurse without end.
pub const MAX_MENU_DEPTH: usize = 16;

/// Parse a `MENUTEMPLATE`; item texts are decoded with the template's
/// codepage, or `fallback_cp` when it gives none. A zero item count falls
/// back to the `MIS_END` sentinel (bit 15 of afStyle) for hand-built
/// templates. An empty list when the data is too short for the header.
pub fn parse_menu_template(data: &[u8], fallback_cp: u32, decode: Decode) -> Vec<MenuItem> {
    if data.len() < MENU_HEADER { return Vec::new(); }
    let count = u16_at(data, MENU_COUNT).unwrap_or(0) as usize;
    let codepage = match u16_at(data, MENU_CODEPAGE).unwrap_or(0) { 0 => fallback_cp, cp => cp as u32 };
    let mut offset = MENU_HEADER;
    parse_menu_items(data, &mut offset, count, codepage, decode, 0)
}

/// Parse `count` items (0: up to the one with `MIS_END`) from `*offset`.
/// Each item is afStyle, afAttribute, id and a NUL-terminated text (absent
/// for `MIS_SEPARATOR`); a `MIS_SUBMENU` item is followed by a child-block
/// header and its children.
fn parse_menu_items(data: &[u8], offset: &mut usize, count: usize, codepage: u32, decode: Decode, depth: usize)
                    -> Vec<MenuItem> {
    let use_count = count > 0;
    let mut items = Vec::new();
    let mut remaining = count;
    loop {
        if use_count && remaining == 0 { break; }
        let (Some(raw_style), Some(attr), Some(id)) =
            (u16_at(data, *offset), u16_at(data, *offset + 2), u16_at(data, *offset + 4)) else { break };
        *offset += MENU_ITEM_FIXED;

        let is_last = !use_count && (raw_style & MIS_END) != 0;
        let style = raw_style & !MIS_END;

        let text = if style & MIS_SEPARATOR == 0 {
            let rest = data.get(*offset..).unwrap_or(&[]);
            let len = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            *offset += (len + 1).min(rest.len());
            decode(&rest[..len], codepage)
        } else {
            String::new()
        };

        let children = if style & MIS_SUBMENU != 0 && *offset + MENU_HEADER <= data.len() {
            let child_count = u16_at(data, *offset + MENU_COUNT).unwrap_or(0) as usize;
            *offset += MENU_HEADER;
            if depth + 1 < MAX_MENU_DEPTH {
                parse_menu_items(data, offset, child_count, codepage, decode, depth + 1)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        items.push(MenuItem { id, style, attr, text, children, image: None, row_h: 0 });
        if use_count { remaining -= 1; }
        if is_last { break; }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latin1(b: &[u8], _cp: u32) -> String { b.iter().map(|&c| c as char).collect() }

    /// A menu template: header, then items; `items` as (style, id, text).
    fn menu(items: &[(u16, u16, &str)]) -> Vec<u8> {
        let mut t = vec![0u8; MENU_HEADER];
        t[MENU_COUNT..MENU_COUNT + 2].copy_from_slice(&(items.len() as u16).to_le_bytes());
        for &(style, id, text) in items {
            t.extend_from_slice(&style.to_le_bytes());
            t.extend_from_slice(&0u16.to_le_bytes());
            t.extend_from_slice(&id.to_le_bytes());
            if style & MIS_SEPARATOR == 0 {
                t.extend_from_slice(text.as_bytes());
                t.push(0);
            }
        }
        t
    }

    #[test]
    fn menus_parse_with_separators() {
        let t = menu(&[(0, 1, "~Open"), (MIS_SEPARATOR, 0, ""), (0, 2, "E~xit")]);
        let items = parse_menu_template(&t, 437, &latin1);
        assert_eq!(items.iter().map(|i| (i.id, i.text.as_str())).collect::<Vec<_>>(),
                   [(1, "~Open"), (0, ""), (2, "E~xit")]);
        assert!(parse_menu_template(&t[..5], 437, &latin1).is_empty(), "no header");
        // A text cut by the end of the data ends the item there.
        let cut = &t[..t.len() - 3];
        assert_eq!(parse_menu_template(cut, 437, &latin1).last().unwrap().text, "E~x");
    }

    #[test]
    fn submenus_nest_and_their_depth_is_bounded() {
        // One submenu item, its child-block header with one child, … nested
        // deeper than MAX_MENU_DEPTH.
        let mut t = vec![0u8; MENU_HEADER];
        t[MENU_COUNT] = 1;
        for level in 0..MAX_MENU_DEPTH + 4 {
            t.extend_from_slice(&MIS_SUBMENU.to_le_bytes());
            t.extend_from_slice(&[0, 0]);
            t.extend_from_slice(&(level as u16).to_le_bytes());
            t.extend_from_slice(b"s\0");
            let mut header = [0u8; MENU_HEADER];
            header[MENU_COUNT] = 1;
            t.extend_from_slice(&header);
        }
        let items = parse_menu_template(&t, 437, &latin1);
        let mut depth = 0;
        let mut level = &items;
        while let Some(first) = level.first() {
            depth += 1;
            level = &first.children;
        }
        assert_eq!(depth, MAX_MENU_DEPTH);
    }

    #[test]
    fn bundles_and_accelerators() {
        let mut b = vec![0u8, 0];
        for s in ["zero", "one"] {
            b.push(s.len() as u8 + 1);
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        assert_eq!(bundle_string(&b, 1), Some(&b"one"[..]));
        assert_eq!(bundle_string(&b, 2), None, "past the end");
        let mut a = vec![2u8, 0, 0, 0];
        a.extend_from_slice(&[1, 0, 2, 0, 3, 0]);
        assert_eq!(parse_accel_table(&a), vec![AccelEntry { flags: 1, key: 2, cmd: 3 }], "the second is cut");
    }

    #[test]
    fn dialog_atoms_go_to_the_class_function() {
        let mut t = vec![0u8; DT_HEADER_SIZE + DLGTITEM_SIZE];
        t[DT_OFF_ITEMS] = DT_HEADER_SIZE as u8;
        let item = DT_HEADER_SIZE;
        t[item + DI_OFF_CLASS] = 3; // cchClassName 0: an atom
        for off in [DI_OFF_TEXT, DI_OFF_PRESPARAMS, DI_OFF_CTLDATA] {
            t[item + off..item + off + 2].copy_from_slice(&NO_OFFSET.to_le_bytes());
        }
        let d = parse_dlg_template(&t, 437, &latin1, &|atom| format!("#{atom}")).unwrap();
        assert_eq!(d.dialog.class_name, "#3");
        assert_eq!(d.frame_flags, 0);
    }
}
