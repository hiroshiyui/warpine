// SPDX-License-Identifier: GPL-3.0-or-later
//
// os2rc — the OS/2 resource compiler of Warpine's C SDK (ADR-0099).
//
//   os2rc [-I dir]... [-D name[=value]]... input.rc [-o output.res]
//
// Compiles an OS/2 resource script into a `.res` file in the classic
// OS/2 format (each resource: 0xFF, type, 0xFF, name, memory flags, size,
// data), which `lx-link` embeds in the program. The binary layouts are the
// ones Warpine's loader reads, and the output matches what the Open Watcom
// resource compiler writes with `-zm` for the same script (the oracle
// test in tests/integration.rs compares them resource by resource).
//
// Statements: ICON, POINTER, BITMAP, FONT, DEFAULTICON, RESOURCE,
// DLGINCLUDE, RCDATA, MENU, ACCELTABLE, STRINGTABLE, MESSAGETABLE,
// DLGTEMPLATE / WINDOWTEMPLATE (DIALOG, FRAME, WINDOW, CONTROL and the
// control statements), CODEPAGE and RCINCLUDE; a C preprocessor
// (#define, #include, #if …) in front. Header files (.h) contribute only
// their preprocessor directives, so a script can include the SDK's os2.h.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process;

#[path = "../cpp.rs"]
#[allow(dead_code)]
mod cpp;

use cpp::{err, Error as RcError, Eval, Kind, Loc, Preprocessor, Result, Tok, Val};

/// The names a resource script sees defined.
const PREDEFINED: &[&str] = &["RC_INVOKED", "__WARPINE_RC__", "__OS2__"];

/// Included resource scripts emit their lines; headers only their
/// directives, so a script can include the SDK's os2.h.
fn is_script(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("rc" | "dlg"))
}

// ─── Constants ────────────────────────────────────────────────────────────────

mod consts {
    // Resource types (<pmwin.h> RT_*).
    pub const RT_POINTER: u16 = 1;
    pub const RT_BITMAP: u16 = 2;
    pub const RT_MENU: u16 = 3;
    pub const RT_DIALOG: u16 = 4;
    pub const RT_STRING: u16 = 5;
    pub const RT_FONT: u16 = 7;
    pub const RT_ACCELTABLE: u16 = 8;
    pub const RT_RCDATA: u16 = 9;
    pub const RT_MESSAGE: u16 = 10;
    pub const RT_DLGINCLUDE: u16 = 11;
    pub const RT_DEFAULTICON: u16 = 22;
    /// The name the default-icon copy of pointer 1 (and DEFAULTICON) gets.
    pub const DEFAULTICON_NAME: u16 = 22;
    /// The pointer name DEFAULTICON's file gets.
    pub const DEFAULTICON_POINTER_NAME: u16 = 999;

    // Memory flags of a .res entry.
    pub const MF_MOVEABLE: u16 = 0x0010;
    pub const MF_PURE: u16 = 0x0020;
    pub const MF_PRELOAD: u16 = 0x0040;
    pub const MF_DISCARDABLE: u16 = 0x1000;
    pub const DEFAULT_FLAGS: u16 = MF_DISCARDABLE | MF_PURE | MF_MOVEABLE;
    /// Accelerator tables and bitmaps are not discardable.
    pub const ACCEL_FLAGS: u16 = MF_PURE | MF_MOVEABLE;
    pub const BITMAP_FLAGS: u16 = MF_PURE | MF_MOVEABLE;

    /// The codepage of resources before any CODEPAGE statement.
    pub const DEFAULT_CODEPAGE: u16 = 850;

    // Menus.
    pub const MIS_TEXT: u16 = 0x0001;
    pub const MIS_SEPARATOR: u16 = 0x0004;
    pub const MIS_SUBMENU: u16 = 0x0010;
    pub const MIA_DISABLED: u16 = 0x4000;
    /// The id of a MENUITEM SEPARATOR.
    pub const SEPARATOR_ID: u16 = 0xFFFF;
    /// The word after a menu template's codepage.
    pub const MENU_TEMPLATE_WORD: u16 = 4;
    pub const MENU_HEADER_SIZE: usize = 10;

    // Accelerators.
    pub const AF_CHAR: u16 = 0x0001;
    pub const AF_VIRTUALKEY: u16 = 0x0002;
    pub const AF_SCANCODE: u16 = 0x0004;
    pub const AF_SHIFT: u16 = 0x0008;
    pub const AF_CONTROL: u16 = 0x0010;
    pub const AF_ALT: u16 = 0x0020;
    pub const AF_LONEKEY: u16 = 0x0040;
    pub const AF_SYSCOMMAND: u16 = 0x0100;
    pub const AF_HELP: u16 = 0x0200;

    // Window and control styles.
    pub const WS_VISIBLE: u32 = 0x8000_0000;
    pub const WS_CLIPSIBLINGS: u32 = 0x1000_0000;
    pub const WS_SAVEBITS: u32 = 0x0400_0000;
    pub const WS_GROUP: u32 = 0x0001_0000;
    pub const WS_TABSTOP: u32 = 0x0002_0000;
    pub const FS_DLGBORDER: u32 = 0x0000_0080;
    pub const BS_PUSHBUTTON: u32 = 0;
    pub const BS_CHECKBOX: u32 = 1;
    pub const BS_AUTOCHECKBOX: u32 = 2;
    pub const BS_RADIOBUTTON: u32 = 3;
    pub const BS_AUTORADIOBUTTON: u32 = 4;
    pub const BS_DEFAULT: u32 = 0x0400;
    pub const SS_TEXT: u32 = 1;
    pub const SS_GROUPBOX: u32 = 2;
    pub const SS_ICON: u32 = 3;
    pub const DT_CENTER: u32 = 0x0100;
    pub const DT_RIGHT: u32 = 0x0200;
    pub const ES_AUTOSCROLL: u32 = 0x0004;
    pub const CBS_SIMPLE: u32 = 1;
    pub const MLS_BORDER: u32 = 0x0002;

    // Window class atoms: the low word of WC_* (0xFFFF00nn).
    pub const CLASS_ATOM_HIGH: u32 = 0xFFFF_0000;
    pub const ATOM_FRAME: u16 = 1;
    pub const ATOM_COMBOBOX: u16 = 2;
    pub const ATOM_BUTTON: u16 = 3;
    pub const ATOM_STATIC: u16 = 5;
    pub const ATOM_ENTRYFIELD: u16 = 6;
    pub const ATOM_LISTBOX: u16 = 7;
    pub const ATOM_MLE: u16 = 10;
    pub const ATOM_SPINBUTTON: u16 = 0x20;
    pub const ATOM_CONTAINER: u16 = 0x25;
    pub const ATOM_SLIDER: u16 = 0x26;
    pub const ATOM_VALUESET: u16 = 0x27;
    pub const ATOM_NOTEBOOK: u16 = 0x28;

    // Dialog templates.
    pub const DLG_HEADER_SIZE: usize = 14;
    pub const DLGTITEM_SIZE: usize = 30;
    pub const DLG_STATUS: u16 = 1;
    pub const NO_OFFSET: u16 = 0xFFFF;
    /// The text of an ICON control: 0xFF and the pointer's resource id.
    pub const RESOURCE_TEXT_MARK: u8 = 0xFF;

    // String and message tables.
    pub const STRINGS_PER_BUNDLE: u16 = 16;
    pub const MAX_STRING: usize = 254;
}
use consts::*;

// ─── Resources ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
struct Resource {
    type_id: u16,
    name: u16,
    flags: u16,
    data: Vec<u8>,
}

/// A menu item.
#[derive(Debug, Clone, PartialEq)]
struct MenuItem {
    style: u16,
    attr: u16,
    id: u16,
    text: Vec<u8>,
    children: Vec<MenuItem>,
}

/// A window class: a predefined one by atom, or a registered one by name.
#[derive(Debug, Clone, PartialEq)]
enum Class {
    Atom(u16),
    Name(Vec<u8>),
}

/// A dialog item's text: a string, or (ICON controls) a resource id.
#[derive(Debug, Clone, PartialEq)]
enum Text {
    Str(Vec<u8>),
    ResId(u16),
}

/// A dialog template item and its children.
#[derive(Debug, Clone, PartialEq)]
struct DlgItem {
    class: Class,
    text: Text,
    style: u32,
    x: i16,
    y: i16,
    cx: i16,
    cy: i16,
    id: u16,
    ctl_data: Option<Vec<u8>>,
    pres_params: Vec<(u32, Vec<u8>)>,
    children: Vec<DlgItem>,
}

/// A control statement: its class, default style and whether it has text.
struct ControlKind {
    keyword: &'static str,
    class: u16,
    style: u32,
    has_text: bool,
}

const CONTROLS: &[ControlKind] = &[
    ControlKind { keyword: "AUTOCHECKBOX", class: ATOM_BUTTON, style: BS_AUTOCHECKBOX | WS_TABSTOP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "AUTORADIOBUTTON", class: ATOM_BUTTON, style: BS_AUTORADIOBUTTON | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "CHECKBOX", class: ATOM_BUTTON, style: BS_CHECKBOX | WS_TABSTOP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "RADIOBUTTON", class: ATOM_BUTTON, style: BS_RADIOBUTTON | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "PUSHBUTTON", class: ATOM_BUTTON, style: BS_PUSHBUTTON | WS_TABSTOP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "DEFPUSHBUTTON", class: ATOM_BUTTON,
                  style: BS_PUSHBUTTON | BS_DEFAULT | WS_TABSTOP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "LTEXT", class: ATOM_STATIC, style: SS_TEXT | WS_GROUP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "CTEXT", class: ATOM_STATIC, style: SS_TEXT | DT_CENTER | WS_GROUP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "RTEXT", class: ATOM_STATIC, style: SS_TEXT | DT_RIGHT | WS_GROUP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "GROUPBOX", class: ATOM_STATIC, style: SS_GROUPBOX | WS_GROUP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "ENTRYFIELD", class: ATOM_ENTRYFIELD, style: ES_AUTOSCROLL | WS_TABSTOP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "LISTBOX", class: ATOM_LISTBOX, style: WS_TABSTOP | WS_VISIBLE, has_text: false },
    ControlKind { keyword: "COMBOBOX", class: ATOM_COMBOBOX, style: CBS_SIMPLE | WS_TABSTOP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "MLE", class: ATOM_MLE, style: MLS_BORDER | WS_TABSTOP | WS_VISIBLE, has_text: true },
    ControlKind { keyword: "SPINBUTTON", class: ATOM_SPINBUTTON, style: WS_TABSTOP | WS_VISIBLE, has_text: false },
    ControlKind { keyword: "SLIDER", class: ATOM_SLIDER, style: WS_TABSTOP | WS_VISIBLE, has_text: false },
    ControlKind { keyword: "VALUESET", class: ATOM_VALUESET, style: WS_TABSTOP | WS_VISIBLE, has_text: false },
    ControlKind { keyword: "CONTAINER", class: ATOM_CONTAINER, style: WS_TABSTOP | WS_VISIBLE, has_text: false },
    ControlKind { keyword: "NOTEBOOK", class: ATOM_NOTEBOOK, style: WS_TABSTOP | WS_VISIBLE, has_text: false },
];

/// What a frame-like statement (DIALOG, FRAME, WINDOW) defaults to.
const DIALOG_STYLE: u32 = FS_DLGBORDER | WS_SAVEBITS | WS_CLIPSIBLINGS;
const FRAME_STYLE: u32 = WS_VISIBLE;

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    files: &'a [PathBuf],
    search: &'a [PathBuf],
    /// Whether ICON, BITMAP, … may read their files.
    reads_files: bool,
    codepage: u16,
    out: Vec<Resource>,
    /// String and message table bundles, by (type, bundle id).
    bundles: BTreeMap<(u16, u16), Bundle>,
}

/// A bundle of 16 strings: the codepage and memory flags of the block
/// that started it, and the strings given so far.
struct Bundle {
    codepage: u16,
    flags: u16,
    strings: Vec<Option<Vec<u8>>>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a Tok> { self.toks.get(self.pos) }
    fn loc(&self) -> Loc { self.peek().or(self.toks.last()).map_or(Loc::default(), |t| t.loc) }
    fn at(&self, p: &str) -> bool { self.peek().is_some_and(|t| t.is_punct(p)) }
    fn at_keyword(&self, k: &str) -> bool { self.peek().is_some_and(|t| t.is_keyword(k)) }

    fn eat(&mut self, p: &str) -> bool {
        if self.at(p) { self.pos += 1; true } else { false }
    }

    fn expect(&mut self, p: &str) -> Result<()> {
        if self.eat(p) { Ok(()) } else { err(self.loc(), format!("expected {p}")) }
    }

    fn keyword(&mut self) -> Result<String> {
        match self.peek().and_then(Tok::ident) {
            Some(k) => { self.pos += 1; Ok(k.to_ascii_uppercase()) }
            None => err(self.loc(), "expected a keyword"),
        }
    }

    fn begin(&mut self) -> Result<()> {
        if self.at_keyword("BEGIN") || self.at("{") { self.pos += 1; Ok(()) } else { err(self.loc(), "expected BEGIN") }
    }

    fn at_end(&self) -> bool { self.at_keyword("END") || self.at("}") }

    fn end(&mut self) -> Result<()> {
        if self.at_end() { self.pos += 1; Ok(()) } else { err(self.loc(), "expected END") }
    }

    /// An integer expression.
    fn expr(&mut self) -> Result<Val> {
        let mut ev = Eval { toks: self.toks, pos: self.pos, unknown_is_zero: false };
        let v = ev.ternary()?;
        self.pos = ev.pos;
        Ok(v)
    }

    fn number<T: TryFrom<i64>>(&mut self, what: &str, min: i64, max: i64) -> Result<T> {
        let loc = self.loc();
        let (v, _) = self.expr()?;
        if v < min || v > max {
            return err(loc, format!("{what} {v} is out of range"));
        }
        // Negative values keep their bit pattern (-1 as 0xFFFF).
        let wrapped = if v < 0 { v + max + 1 } else { v };
        T::try_from(wrapped).map_err(|_| RcError { loc: Some(loc), msg: format!("{what} {v} is out of range") })
    }

    fn word(&mut self, what: &str) -> Result<u16> { self.number(what, -0x8000, 0xFFFF) }
    fn coord(&mut self) -> Result<i16> { Ok(self.number::<u16>("a coordinate", -0x8000, 0x7FFF)? as i16) }

    fn long(&mut self, what: &str) -> Result<u32> { self.number(what, -0x8000_0000, 0xFFFF_FFFF) }

    /// A style: alternatives joined by |, each possibly NOT (clearing its
    /// bits); `default` is the starting value.
    fn style(&mut self, default: u32) -> Result<u32> {
        let mut set = default;
        loop {
            if self.at_keyword("NOT") {
                self.pos += 1;
                let loc = self.loc();
                let mut ev = Eval { toks: self.toks, pos: self.pos, unknown_is_zero: false };
                let v = ev.binary(3)?; // above |, so NOT binds to one alternative
                self.pos = ev.pos;
                let v = u32::try_from(v.0 & 0xFFFF_FFFF).map_err(|_| RcError { loc: Some(loc), msg: "bad style".into() })?;
                set &= !v;
            } else {
                let loc = self.loc();
                let mut ev = Eval { toks: self.toks, pos: self.pos, unknown_is_zero: false };
                let v = ev.binary(3)?;
                self.pos = ev.pos;
                if v.0 < -0x8000_0000 || v.0 > 0xFFFF_FFFF { return err(loc, "style out of range"); }
                set |= (v.0 & 0xFFFF_FFFF) as u32;
            }
            if !self.eat("|") { return Ok(set); }
        }
    }

    /// A string: adjacent quoted strings joined.
    fn string(&mut self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut any = false;
        while let Some(Kind::Str(s)) = self.peek().map(|t| &t.kind) {
            out.extend_from_slice(s);
            self.pos += 1;
            any = true;
        }
        if any { Ok(out) } else { err(self.loc(), "expected a string") }
    }

    fn at_string(&self) -> bool { matches!(self.peek().map(|t| &t.kind), Some(Kind::Str(_))) }

    /// The memory and load options after a resource's name.
    fn options(&mut self, mut flags: u16) -> u16 {
        while let Some(k) = self.peek().and_then(Tok::ident).map(str::to_ascii_uppercase) {
            match k.as_str() {
                "PRELOAD" => flags |= MF_PRELOAD,
                "LOADONCALL" => flags &= !MF_PRELOAD,
                "MOVEABLE" => flags |= MF_MOVEABLE,
                "FIXED" => flags &= !MF_MOVEABLE,
                "DISCARDABLE" => flags |= MF_DISCARDABLE,
                "PURE" => flags |= MF_PURE,
                "IMPURE" => flags &= !MF_PURE,
                "SEGALIGN" => {}
                _ => break,
            }
            self.pos += 1;
        }
        flags
    }

    /// A file name: a string, or the rest of the line as written.
    fn file_name(&mut self) -> Result<String> {
        if self.at_string() {
            return Ok(String::from_utf8_lossy(&self.string()?).into_owned());
        }
        let Some(first) = self.peek() else { return err(self.loc(), "expected a file name") };
        let mut name = String::new();
        while let Some(t) = self.peek() {
            if t.loc != first.loc || (!name.is_empty() && t.space_before) { break; }
            name.push_str(&t.text);
            self.pos += 1;
        }
        Ok(name)
    }

    fn read_file(&self, name: &str, loc: Loc) -> Result<Vec<u8>> {
        if !self.reads_files {
            return err(loc, format!("cannot read {name}: no files here"));
        }
        let here = self.files.get(loc.file).and_then(|f| f.parent()).map(Path::to_path_buf);
        let candidates = std::iter::once(PathBuf::from(name))
            .chain(here.map(|d| d.join(name)))
            .chain(self.search.iter().map(|d| d.join(name)));
        // Regular files only: a device such as /dev/zero never ends.
        for c in candidates.filter(|c| c.is_file()) {
            if let Ok(data) = std::fs::read(&c) { return Ok(data); }
        }
        err(loc, format!("cannot read {name}"))
    }

    fn push(&mut self, type_id: u16, name: u16, flags: u16, data: Vec<u8>) {
        self.out.push(Resource { type_id, name, flags, data });
    }

    fn statements(&mut self) -> Result<()> {
        while self.peek().is_some() {
            let loc = self.loc();
            let k = self.keyword()?;
            match k.as_str() {
                "ICON" | "POINTER" | "BITMAP" | "FONT" => {
                    let name = self.word("a resource id")?;
                    let type_id = match k.as_str() { "BITMAP" => RT_BITMAP, "FONT" => RT_FONT, _ => RT_POINTER };
                    let flags = self.options(if type_id == RT_BITMAP { BITMAP_FLAGS } else { DEFAULT_FLAGS });
                    let file = self.file_name()?;
                    let data = self.read_file(&file, loc)?;
                    if type_id == RT_POINTER && name == 1 {
                        self.push(RT_DEFAULTICON, DEFAULTICON_NAME, flags, data.clone());
                    }
                    self.push(type_id, name, flags, data);
                }
                "DEFAULTICON" => {
                    let file = self.file_name()?;
                    let data = self.read_file(&file, loc)?;
                    self.push(RT_DEFAULTICON, DEFAULTICON_NAME, DEFAULT_FLAGS, data.clone());
                    self.push(RT_POINTER, DEFAULTICON_POINTER_NAME, DEFAULT_FLAGS, data);
                }
                "RESOURCE" => {
                    let type_id = self.word("a resource type")?;
                    self.eat(",");
                    let name = self.word("a resource id")?;
                    let flags = self.options(DEFAULT_FLAGS);
                    let file = self.file_name()?;
                    let data = self.read_file(&file, loc)?;
                    self.push(type_id, name, flags, data);
                }
                "DLGINCLUDE" => {
                    let name = self.word("a resource id")?;
                    let flags = self.options(DEFAULT_FLAGS);
                    let mut file = self.file_name()?.into_bytes();
                    file.push(0);
                    self.push(RT_DLGINCLUDE, name, flags, file);
                }
                "CODEPAGE" => self.codepage = self.word("a codepage")?,
                "RCDATA" => {
                    let name = self.word("a resource id")?;
                    let flags = self.options(DEFAULT_FLAGS);
                    self.begin()?;
                    let data = self.data_list(true)?;
                    self.end()?;
                    self.push(RT_RCDATA, name, flags, data);
                }
                "MENU" => {
                    let name = self.word("a resource id")?;
                    let flags = self.options(DEFAULT_FLAGS);
                    let items = self.menu_items()?;
                    let data = encode_menu(&items, self.codepage);
                    self.push(RT_MENU, name, flags, data);
                }
                "ACCELTABLE" => {
                    let name = self.word("a resource id")?;
                    let flags = self.options(ACCEL_FLAGS);
                    let data = self.accel_table()?;
                    self.push(RT_ACCELTABLE, name, flags, data);
                }
                "STRINGTABLE" | "MESSAGETABLE" => {
                    let type_id = if k == "STRINGTABLE" { RT_STRING } else { RT_MESSAGE };
                    let flags = self.options(DEFAULT_FLAGS);
                    self.string_table(type_id, flags)?;
                }
                "DLGTEMPLATE" | "WINDOWTEMPLATE" => {
                    let name = self.word("a resource id")?;
                    let flags = self.options(DEFAULT_FLAGS);
                    self.begin()?;
                    let item = self.dlg_item()?;
                    if !self.at_end() {
                        return err(self.loc(), "a dialog template holds one top-level window");
                    }
                    self.end()?;
                    let data = encode_dialog(&item, self.codepage, loc)?;
                    self.push(RT_DIALOG, name, flags, data);
                }
                "HELPTABLE" | "HELPSUBTABLE" | "ASSOCTABLE" | "FONTDIR" => {
                    return err(loc, format!("{k} is not supported yet"));
                }
                other => return err(loc, format!("unknown statement {other}")),
            }
        }
        Ok(())
    }

    /// RCDATA and CTLDATA items, separated by commas: strings (with their
    /// NUL) and numbers, 2 bytes each; in RCDATA a number with an L suffix
    /// takes 4 (CTLDATA keeps every number to 2, as wrc does).
    fn data_list(&mut self, longs: bool) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        while !self.at_end() && self.peek().is_some() && !self.at_attribute() {
            if self.at_string() {
                out.extend(self.string()?);
                out.push(0);
            } else {
                let loc = self.loc();
                let (v, long) = self.expr()?;
                if long && longs {
                    if !(-0x8000_0000..=0xFFFF_FFFF).contains(&v) { return err(loc, "value out of range"); }
                    out.extend_from_slice(&(v as u32).to_le_bytes());
                } else {
                    if !(-0x8000..=0xFFFF).contains(&v) && !(long && !longs && (-0x8000_0000..=0xFFFF_FFFF).contains(&v)) {
                        return err(loc, "value out of range (add L for 32 bits)");
                    }
                    out.extend_from_slice(&(v as u16).to_le_bytes());
                }
            }
            if !self.eat(",") { break; }
        }
        Ok(out)
    }

    fn menu_items(&mut self) -> Result<Vec<MenuItem>> {
        self.begin()?;
        let mut items = Vec::new();
        while !self.at_end() {
            let loc = self.loc();
            let k = self.keyword()?;
            match k.as_str() {
                "MENUITEM" if self.at_keyword("SEPARATOR") => {
                    self.pos += 1;
                    items.push(MenuItem { style: MIS_SEPARATOR, attr: MIA_DISABLED, id: SEPARATOR_ID,
                                          text: Vec::new(), children: Vec::new() });
                }
                "MENUITEM" | "SUBMENU" => {
                    let text = self.string()?;
                    self.expect(",")?;
                    let id = self.word("a menu item id")?;
                    let mut style = None;
                    let mut attr = 0;
                    if self.eat(",") {
                        if !self.at(",") { style = Some(self.style(0)? as u16); }
                        if self.eat(",") { attr = self.style(0)? as u16; }
                    }
                    let mut style = style.unwrap_or(MIS_TEXT);
                    let children = if k == "SUBMENU" {
                        style |= MIS_SUBMENU;
                        self.menu_items()?
                    } else {
                        Vec::new()
                    };
                    items.push(MenuItem { style, attr, id, text, children });
                }
                other => return err(loc, format!("expected MENUITEM or SUBMENU, found {other}")),
            }
        }
        self.end()?;
        Ok(items)
    }

    fn accel_table(&mut self) -> Result<Vec<u8>> {
        self.begin()?;
        let mut entries: Vec<[u16; 3]> = Vec::new();
        while !self.at_end() {
            let loc = self.loc();
            let (mut fs, key) = if self.at_string() {
                let s = self.string()?;
                match s.as_slice() {
                    [b'^', c] => (AF_CONTROL, u16::from(*c)),
                    [c] => (0, u16::from(*c)),
                    _ => return err(loc, "an accelerator key is one character, or ^ and one"),
                }
            } else {
                (0, self.word("a key")?)
            };
            self.expect(",")?;
            let cmd = self.word("a command")?;
            while self.eat(",") {
                let loc = self.loc();
                fs |= match self.keyword()?.as_str() {
                    "CHAR" => AF_CHAR,
                    "VIRTUALKEY" => AF_VIRTUALKEY,
                    "SCANCODE" => AF_SCANCODE,
                    "SHIFT" => AF_SHIFT,
                    "CONTROL" => AF_CONTROL,
                    "ALT" => AF_ALT,
                    "LONEKEY" => AF_LONEKEY,
                    "SYSCOMMAND" => AF_SYSCOMMAND,
                    "HELP" => AF_HELP,
                    other => return err(loc, format!("unknown accelerator option {other}")),
                };
            }
            if fs & AF_VIRTUALKEY == 0 { fs |= AF_CHAR; }
            entries.push([fs, key, cmd]);
        }
        self.end()?;
        let count = u16::try_from(entries.len()).map_err(|_| RcError { loc: Some(self.loc()), msg: "too many accelerators".into() })?;
        let mut out = Vec::with_capacity(4 + entries.len() * 6);
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.codepage.to_le_bytes());
        for e in entries { for w in e { out.extend_from_slice(&w.to_le_bytes()); } }
        Ok(out)
    }

    fn string_table(&mut self, type_id: u16, flags: u16) -> Result<()> {
        self.begin()?;
        while !self.at_end() {
            let loc = self.loc();
            let id = self.word("a string id")?;
            self.eat(",");
            let s = self.string()?;
            if s.len() > MAX_STRING {
                return err(loc, format!("string {id} is longer than {MAX_STRING} bytes"));
            }
            let bundle = id / STRINGS_PER_BUNDLE + 1;
            let codepage = self.codepage;
            let entry = self.bundles.entry((type_id, bundle))
                .or_insert_with(|| Bundle { codepage, flags, strings: vec![None; STRINGS_PER_BUNDLE as usize] });
            let slot = &mut entry.strings[(id % STRINGS_PER_BUNDLE) as usize];
            if slot.is_some() {
                return err(loc, format!("string {id} is defined twice"));
            }
            *slot = Some(s);
        }
        self.end()
    }

    fn at_attribute(&self) -> bool { self.at_keyword("CTLDATA") || self.at_keyword("PRESPARAMS") }

    /// A class: a string names one, a number is a predefined one's atom.
    fn class(&mut self) -> Result<Class> {
        if self.at_string() { return Ok(Class::Name(self.string()?)); }
        let loc = self.loc();
        let v = self.long("a window class")?;
        if v >> 16 == CLASS_ATOM_HIGH >> 16 || v <= 0xFFFF {
            Ok(Class::Atom(v as u16))
        } else {
            err(loc, "a window class is a WC_* value or a string")
        }
    }

    /// One dialog item (DIALOG, FRAME, WINDOW, CONTROL or a control
    /// statement), with its attributes and children.
    fn dlg_item(&mut self) -> Result<DlgItem> {
        let loc = self.loc();
        let k = self.keyword()?;
        let frame_like = matches!(k.as_str(), "DIALOG" | "FRAME" | "WINDOW");
        let control = CONTROLS.iter().find(|c| c.keyword == k);
        if !frame_like && k != "CONTROL" && k != "ICON" && control.is_none() {
            return err(loc, format!("unknown dialog item {k}"));
        }

        let text = if k == "ICON" {
            let id = self.word("a pointer resource id")?;
            self.expect(",")?;
            Text::ResId(id)
        } else if control.is_some_and(|c| !c.has_text) {
            Text::Str(Vec::new())
        } else {
            let t = self.string()?;
            self.expect(",")?;
            Text::Str(t)
        };
        let id = self.word("an item id")?;
        self.expect(",")?;
        let x = self.coord()?;
        self.expect(",")?;
        let y = self.coord()?;
        self.expect(",")?;
        let cx = self.coord()?;
        self.expect(",")?;
        let cy = self.coord()?;

        let (class, default_style) = match k.as_str() {
            "DIALOG" => (Class::Atom(ATOM_FRAME), DIALOG_STYLE),
            "FRAME" => (Class::Atom(ATOM_FRAME), FRAME_STYLE),
            "ICON" => (Class::Atom(ATOM_STATIC), SS_ICON | WS_VISIBLE),
            "WINDOW" | "CONTROL" => {
                self.expect(",")?;
                (self.class()?, 0)
            }
            _ => {
                let c = control.expect("checked above");
                (Class::Atom(c.class), c.style)
            }
        };
        let mut style = default_style;
        let mut frame_flags = 0;
        if self.eat(",") {
            if !self.at(",") { style = self.style(default_style)?; }
            if frame_like && self.eat(",") { frame_flags = self.long("frame flags")?; }
        }
        let mut ctl_data = frame_like.then(|| frame_flags.to_le_bytes().to_vec());

        let mut pres_params = Vec::new();
        loop {
            if self.at_keyword("CTLDATA") {
                self.pos += 1;
                ctl_data = Some(self.data_list(false)?);
            } else if self.at_keyword("PRESPARAMS") {
                self.pos += 1;
                let id = self.long("a presentation parameter")?;
                let mut value = Vec::new();
                while self.eat(",") {
                    if self.at_string() {
                        value.extend(self.string()?);
                        value.push(0);
                    } else {
                        value.extend_from_slice(&self.long("a presentation parameter value")?.to_le_bytes());
                    }
                }
                pres_params.push((id, value));
            } else {
                break;
            }
        }

        let mut children = Vec::new();
        if self.at_keyword("BEGIN") || self.at("{") {
            self.pos += 1;
            while !self.at_end() {
                children.push(self.dlg_item()?);
            }
            self.end()?;
        }
        Ok(DlgItem { class, text, style, x, y, cx, cy, id, ctl_data, pres_params, children })
    }

    /// The string and message bundles, after the other resources.
    fn finish(mut self) -> Vec<Resource> {
        for ((type_id, bundle), Bundle { codepage, flags, strings }) in std::mem::take(&mut self.bundles) {
            let mut data = codepage.to_le_bytes().to_vec();
            for s in strings {
                let s = s.unwrap_or_default();
                data.push((s.len() + 1) as u8);
                data.extend_from_slice(&s);
                data.push(0);
            }
            self.push(type_id, bundle, flags, data);
        }
        self.out
    }
}

// ─── Encoders ─────────────────────────────────────────────────────────────────

/// A menu template: size, codepage, a word 4, the item count, the items;
/// a submenu's items follow its item as a template of their own.
fn encode_menu(items: &[MenuItem], codepage: u16) -> Vec<u8> {
    let mut body = Vec::new();
    for item in items {
        body.extend_from_slice(&item.style.to_le_bytes());
        body.extend_from_slice(&item.attr.to_le_bytes());
        body.extend_from_slice(&item.id.to_le_bytes());
        if item.style & MIS_SEPARATOR == 0 {
            body.extend_from_slice(&item.text);
            body.push(0);
        }
        if item.style & MIS_SUBMENU != 0 {
            body.extend(encode_menu(&item.children, codepage));
        }
    }
    let mut out = Vec::with_capacity(MENU_HEADER_SIZE + body.len());
    out.extend_from_slice(&((MENU_HEADER_SIZE + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&codepage.to_le_bytes());
    out.extend_from_slice(&MENU_TEMPLATE_WORD.to_le_bytes());
    out.extend_from_slice(&(items.len() as u16).to_le_bytes());
    out.extend(body);
    out
}

/// The items of a dialog in template order (depth first), each with its
/// number of children.
fn flatten<'a>(item: &'a DlgItem, out: &mut Vec<&'a DlgItem>) {
    out.push(item);
    for c in &item.children { flatten(c, out); }
}

/// A dialog template: the 14-byte header, the 30-byte items, then a pool
/// with each item's class name, text, control data and presentation
/// parameters, item by item.
fn encode_dialog(root: &DlgItem, codepage: u16, loc: Loc) -> Result<Vec<u8>> {
    let mut items = Vec::new();
    flatten(root, &mut items);
    let mut pool: Vec<u8> = Vec::new();
    let pool_start = DLG_HEADER_SIZE + items.len() * DLGTITEM_SIZE;
    let mut records = Vec::with_capacity(items.len() * DLGTITEM_SIZE);
    let offset = |pool: &Vec<u8>| -> Result<u16> {
        u16::try_from(pool_start + pool.len()).map_err(|_| RcError { loc: Some(loc), msg: "dialog template larger than 64 KB".into() })
    };
    for item in &items {
        let (cch_class, off_class) = match &item.class {
            Class::Atom(a) => (0u16, *a),
            Class::Name(n) => {
                let off = offset(&pool)?;
                pool.extend_from_slice(n);
                pool.push(0);
                (n.len() as u16, off)
            }
        };
        let off_text = offset(&pool)?;
        let cch_text = match &item.text {
            Text::Str(s) => {
                pool.extend_from_slice(s);
                pool.push(0);
                s.len() as u16
            }
            Text::ResId(id) => {
                pool.push(RESOURCE_TEXT_MARK);
                pool.extend_from_slice(&id.to_le_bytes());
                3
            }
        };
        let off_ctl = match &item.ctl_data {
            Some(d) => { let o = offset(&pool)?; pool.extend_from_slice(d); o }
            None => NO_OFFSET,
        };
        let off_pp = if item.pres_params.is_empty() { NO_OFFSET } else {
            let o = offset(&pool)?;
            let total: usize = item.pres_params.iter().map(|(_, v)| 8 + v.len()).sum();
            pool.extend_from_slice(&(total as u32).to_le_bytes());
            for (id, v) in &item.pres_params {
                pool.extend_from_slice(&id.to_le_bytes());
                pool.extend_from_slice(&(v.len() as u32).to_le_bytes());
                pool.extend_from_slice(v);
            }
            o
        };
        for w in [0u16, item.children.len() as u16, cch_class, off_class, cch_text, off_text] {
            records.extend_from_slice(&w.to_le_bytes());
        }
        records.extend_from_slice(&item.style.to_le_bytes());
        for w in [item.x as u16, item.y as u16, item.cx as u16, item.cy as u16, item.id, off_pp, off_ctl] {
            records.extend_from_slice(&w.to_le_bytes());
        }
    }
    let total = pool_start + pool.len();
    let total16 = u16::try_from(total).map_err(|_| RcError { loc: Some(loc), msg: "dialog template larger than 64 KB".into() })?;
    let mut out = Vec::with_capacity(total);
    for w in [total16, 0, codepage, DLG_HEADER_SIZE as u16, DLG_STATUS, NO_OFFSET, 0] {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend(records);
    out.extend(pool);
    Ok(out)
}

/// A .res file: each resource as 0xFF, type, 0xFF, name, flags, size, data.
fn write_res(resources: &[Resource]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in resources {
        out.push(0xFF);
        out.extend_from_slice(&r.type_id.to_le_bytes());
        out.push(0xFF);
        out.extend_from_slice(&r.name.to_le_bytes());
        out.extend_from_slice(&r.flags.to_le_bytes());
        out.extend_from_slice(&(r.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&r.data);
    }
    out
}

/// Compile the script at `input`.
fn compile(input: &Path, include_dirs: &[PathBuf], defines: &[String]) -> std::result::Result<Vec<Resource>, String> {
    let text = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    compile_source(&text, input, include_dirs, defines, true)
}

/// Compile the script `text` alone into a .res file: no `#include`,
/// `RCINCLUDE` or resource file is read (the fuzz target's entry,
/// ADR-0112).
#[allow(dead_code)]
pub(crate) fn compile_in_memory(text: &[u8]) -> std::result::Result<Vec<u8>, String> {
    compile_source(text, Path::new("input.rc"), &[], &[], false).map(|r| write_res(&r))
}

/// Compile `text`, the script at `input`; files are read only when
/// `reads_files`.
fn compile_source(text: &[u8], input: &Path, include_dirs: &[PathBuf], defines: &[String], reads_files: bool)
                  -> std::result::Result<Vec<Resource>, String> {
    let mut pre = Preprocessor::new(include_dirs.to_vec(), PREDEFINED);
    pre.reads_files = reads_files;
    pre.emits = is_script;
    pre.include_keyword = Some("RCINCLUDE");
    let describe = |e: RcError, files: &[PathBuf]| match e.loc {
        Some(l) if l.line > 0 => format!("{}:{}: {}", files.get(l.file).map_or("?".into(), |f| f.display().to_string()), l.line, e.msg),
        Some(l) => format!("{}: {}", files.get(l.file).map_or("?".into(), |f| f.display().to_string()), e.msg),
        None => e.msg,
    };
    for d in defines {
        pre.define_cmdline(d).map_err(|e| e.msg)?;
    }
    if let Err(e) = pre.run_text(text, input, true, None) {
        return Err(describe(e, &pre.files));
    }
    let toks = std::mem::take(&mut pre.out);
    let mut parser = Parser { toks: &toks, pos: 0, files: &pre.files, search: include_dirs, reads_files, codepage: DEFAULT_CODEPAGE,
                              out: Vec::new(), bundles: BTreeMap::new() };
    match parser.statements() {
        Ok(()) => Ok(parser.finish()),
        Err(e) => Err(describe(e, &pre.files)),
    }
}

fn usage() -> ! {
    eprintln!("usage: os2rc [-I dir]... [-D name[=value]]... input.rc [-o output.res]");
    process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut include_dirs = Vec::new();
    let mut defines = Vec::new();
    let mut input = None;
    let mut output = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let mut value = |flag: &str| -> String {
            if a.len() > flag.len() { a[flag.len()..].to_string() } else { i += 1; args.get(i).cloned().unwrap_or_else(|| usage()) }
        };
        if a.starts_with("-I") { include_dirs.push(PathBuf::from(value("-I"))); }
        else if a.starts_with("-D") { defines.push(value("-D")); }
        else if a.starts_with("-o") { output = Some(PathBuf::from(value("-o"))); }
        else if a.starts_with('-') { usage(); }
        else if input.is_none() { input = Some(PathBuf::from(a)); }
        else { usage(); }
        i += 1;
    }
    let Some(input) = input else { usage() };
    let output = output.unwrap_or_else(|| input.with_extension("res"));
    match compile(&input, &include_dirs, &defines) {
        Ok(resources) => {
            if let Err(e) = std::fs::write(&output, write_res(&resources)) {
                eprintln!("os2rc: {}: {e}", output.display());
                process::exit(1);
            }
        }
        Err(msg) => {
            eprintln!("os2rc: {msg}");
            process::exit(1);
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile `script` (and `files`, written next to it) in a temporary
    /// directory, with the SDK's headers on the include path.
    fn compile_text(script: &str, files: &[(&str, &[u8])]) -> std::result::Result<Vec<Resource>, String> {
        let dir = std::env::temp_dir().join(format!("os2rc-test-{}-{:?}", process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        for (name, data) in files {
            std::fs::write(dir.join(name), data).expect("write a file");
        }
        let rc = dir.join("test.rc");
        std::fs::write(&rc, script).expect("write the script");
        let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/include");
        let out = compile(&rc, &[sdk], &[]);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    /// A script compiled in memory reads no file: neither an include nor
    /// a resource's file; what it defines itself compiles.
    #[test]
    fn a_script_compiled_in_memory_reads_no_file() {
        let res = compile_in_memory(b"#define ID 7\nRCDATA ID\nBEGIN\n  1, 2\nEND\n").expect("compiles");
        // One resource: type RT_RCDATA (0xFF, 9), name 7 (0xFF, 7).
        assert_eq!(&res[..6], &[0xFF, 9, 0, 0xFF, 7, 0]);
        let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/include/os2.h");
        let include = format!("#include \"{}\"\n", sdk.display());
        assert!(compile_in_memory(include.as_bytes()).is_err(), "no include");
        let icon = format!("ICON 1 \"{}\"\n", sdk.display());
        assert!(compile_in_memory(icon.as_bytes()).expect_err("no resource file").contains("no files"));
    }

    /// A resource's file must be a regular file: a device such as
    /// /dev/zero would be read for ever.
    #[test]
    fn a_resource_file_must_be_a_regular_file() {
        if !Path::new("/dev/zero").exists() { return; }
        let e = compile_text("ICON 1 \"/dev/zero\"\n", &[]).expect_err("not a regular file");
        assert!(e.contains("cannot read /dev/zero"), "{e}");
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
    }

    /// The scripts below were compiled by Open Watcom's wrc -zm -bt=os2;
    /// these are the .res files it wrote.
    const WRC_MENU: &str = "ff0300ff640030104b0000004b00000052030400020011000000c8007e46696c65002c000000520304000300\
        01000000c9007e41626f75742e2e2e0004000040ffff01000040ca00457e78697400020000000800233700";
    const WRC_DIALOG: &str = "ff0400ff65003010e7000000e700000052030e000100ffff000000000400000001000500a400800000942800\
        2800b40050006500ae00aa0000000000000005000500c100010001800a002d00a0000e00ffffc700ffff0000000000000300\
        0200d700000403803c000a0032000e000100ffffffff000000000400da000000df000000008001000200030004006600ffff\
        e00000000000000005000300e4000300008001000100000000006700ffffffff41626f757400030000000f0000000f000000\
        07000000382e48656c76004c6162656c000c00000001000000040000000000ff004f4b004469616c000008000200ff0500";
    const WRC_TABLES: &str = "ff0800ff01003000160000000300b501110064000a00220024000b00110043000c00ff0900ff090030100b00\
        0000010002000000616200ffffff0500ff010030102d000000b501010001000100010001000c537472696e672066697665000\
        100010001000100010001000100010001000100ff0500ff020030102f000000b50101000e7365636f6e642062756e646c6500\
        01000100010001000100010001000100010001000100010001000100ff0a00ff0100301025000000b5010100046d73670001\
        000100010001000100010001000100010001000100010001000100";

    #[test]
    fn menus_match_wrc() {
        let r = compile_text("MENU 100\nBEGIN\n  SUBMENU \"~File\", 200\n  BEGIN\n    MENUITEM \"~About...\", 201\n\
                              MENUITEM SEPARATOR\n    MENUITEM \"E~xit\", 202, 0x0001, 0x4000\n  END\n\
                              MENUITEM \"#7\", 8, 0x0002\nEND\n", &[]).expect("compile");
        assert_eq!(write_res(&r), hex(WRC_MENU));
    }

    #[test]
    fn dialogs_with_presparams_ctldata_class_names_and_icons_match_wrc() {
        let r = compile_text("DLGTEMPLATE 101\nBEGIN\n  DIALOG \"About\", 101, 40, 40, 180, 80, 0x80000000, 3\n\
                              PRESPARAMS 15, \"8.Helv\"\n  BEGIN\n    LTEXT \"Label\", -1, 10, 45, 160, 14\n\
                              PRESPARAMS 1, 0x00FF0000L\n    DEFPUSHBUTTON \"OK\", 1, 60, 10, 50, 14, 0x00010000\n\
                              CONTROL \"\", 102, 1, 2, 3, 4, \"Dial\", 0x80000000\n      CTLDATA 8, 2L\n\
                              ICON 5, 103, 1, 1, 0, 0\n  END\nEND\n", &[]).expect("compile");
        assert_eq!(write_res(&r), hex(WRC_DIALOG));
    }

    #[test]
    fn accelerators_strings_messages_and_rcdata_match_wrc() {
        let r = compile_text("CODEPAGE 437\nACCELTABLE 1\nBEGIN\n  \"d\", 10, CONTROL\n  0x24, 11, VIRTUALKEY, ALT\n\
                              \"^C\", 12\nEND\nSTRINGTABLE\nBEGIN\n  5, \"String five\"\n  17 \"second\" \" bundle\"\nEND\n\
                              MESSAGETABLE\nBEGIN\n  1, \"msg\"\nEND\nRCDATA 9\nBEGIN\n  1, 2L, \"ab\", -1\nEND\n", &[])
            .expect("compile");
        assert_eq!(write_res(&r), hex(WRC_TABLES));
    }

    #[test]
    fn the_sdk_header_gives_the_window_constants() {
        let r = compile_text("#include <os2.h>\nDLGTEMPLATE 1\nBEGIN\n  DIALOG \"\", 1, 0, 0, 1, 1, WS_VISIBLE | NOT WS_SAVEBITS, \
                              FCF_TITLEBAR | FCF_SYSMENU\n  BEGIN\n    CONTROL \"x\", 2, 0, 0, 1, 1, WC_BUTTON, BS_PUSHBUTTON | WS_VISIBLE\n\
                              END\nEND\n", &[]).expect("compile");
        let d = &r[0].data;
        let style = u32::from_le_bytes(d[26..30].try_into().unwrap());
        assert_eq!(style, WS_VISIBLE | FS_DLGBORDER | WS_CLIPSIBLINGS, "defaults, plus WS_VISIBLE, minus WS_SAVEBITS");
        assert_eq!(u16::from_le_bytes(d[44 + 6..44 + 8].try_into().unwrap()), ATOM_BUTTON, "WC_BUTTON as an atom");
    }

    #[test]
    fn the_preprocessor_handles_conditionals_and_function_like_macros() {
        let r = compile_text("#define TWICE(x) ((x) * 2)\n#define BASE 10\n#if defined(BASE) && BASE > 5\n\
                              #define ID TWICE(BASE) + 1\n#else\n#define ID 1\n#endif\n#ifndef NOT_DEFINED\n\
                              STRINGTABLE\nBEGIN\n  ID, \"twenty-one\"\nEND\n#endif\n", &[]).expect("compile");
        assert_eq!((r[0].type_id, r[0].name), (RT_STRING, 2), "string 21 is in bundle 2");
        assert_eq!(&r[0].data[2 + 5 * 2..2 + 5 * 2 + 12], b"\x0btwenty-one\0");
    }

    #[test]
    fn files_and_icons() {
        let r = compile_text("#include \"ids.h\"\nPOINTER ID_PTR ptr.ptr\nBITMAP 2 \"pic.bmp\"\nRESOURCE 300, 4 blob\n",
                             &[("ids.h", b"#define ID_PTR 1\nint ignored_c_code;\n"), ("ptr.ptr", b"PT"),
                               ("pic.bmp", b"BM"), ("blob", b"raw")]).expect("compile");
        let got: Vec<(u16, u16, u16, &[u8])> = r.iter().map(|x| (x.type_id, x.name, x.flags, x.data.as_slice())).collect();
        assert_eq!(got, [(RT_DEFAULTICON, DEFAULTICON_NAME, DEFAULT_FLAGS, &b"PT"[..]), (RT_POINTER, 1, DEFAULT_FLAGS, b"PT"),
                         (RT_BITMAP, 2, BITMAP_FLAGS, b"BM"), (300, 4, DEFAULT_FLAGS, b"raw")]);
    }

    #[test]
    fn mistakes_are_reported_with_their_line() {
        let e = compile_text("MENU 1\nBEGIN\n  MENUITEM \"x\", UNDEFINED_ID\nEND\n", &[]).unwrap_err();
        assert!(e.contains("test.rc:3") && e.contains("UNDEFINED_ID"), "{e}");
        let e = compile_text("TOOLBAR 1\n", &[]).unwrap_err();
        assert!(e.contains("unknown statement TOOLBAR"), "{e}");
        let e = compile_text(&format!("STRINGTABLE\nBEGIN\n 1, \"{}\"\nEND\n", "x".repeat(300)), &[]).unwrap_err();
        assert!(e.contains("longer than 254"), "{e}");
        let e = compile_text("STRINGTABLE\nBEGIN\n 1, \"a\"\n 1, \"b\"\nEND\n", &[]).unwrap_err();
        assert!(e.contains("defined twice"), "{e}");
        let e = compile_text("#if 1\nRCDATA 1\nBEGIN\nEND\n", &[]).unwrap_err();
        assert!(e.contains("#if without #endif"), "{e}");
        let e = compile_text("BITMAP 1 missing.bmp\n", &[]).unwrap_err();
        assert!(e.contains("cannot read missing.bmp"), "{e}");
    }

}
