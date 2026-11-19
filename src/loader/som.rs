// SPDX-License-Identifier: GPL-3.0-only
//
//! SOM.DLL: the System Object Model runtime kernel (ADR-0059, ADR-0060),
//! from the public SOM documentation (*SOM Programming Guide* and
//! *Reference*, "SOMObject", "SOMClass", "SOMClassMgr", the SOM functions
//! and runtime variables; as on EDM/2).
//!
//! - **Objects.** The first word of every SOM object points to its class's
//!   method table; a class is itself an object (an instance of its
//!   metaclass, `SOMClass` by default) and carries its metaclass's instance
//!   data. The guest sees:
//!   - method tables `[classObject, count, entries…]`;
//!   - class objects (the metaclass's method table, then the metaclass's
//!     instance data);
//!   - the `ClassData` of the root classes (`SOMObjectClassData`,
//!     `SOMClassClassData`, `SOMClassMgrClassData`), `SOMClassMgrObject`
//!     and the runtime variables (`SOMMalloc` … `SOM_AssertLevel`),
//!     exported as data.
//! - **Host records.** Every class is also kept here ([`SomRuntime`]):
//!   name, parents, metaclass, its methods, instance layout. Method and
//!   data lookups (`somResolve`, `somClassResolve`, `somDataResolve`)
//!   answer from these records, so a guest that overwrites a method table
//!   cannot steer them.
//! - **Tokens.** A method token names one method introduced by one class
//!   (its index in the runtime's method list plus one); each class maps
//!   the tokens it supports to its procedures, so a token is valid for the
//!   introducing class and every descendant, however many parents they
//!   have. A data token is the introducing class's object: each class
//!   knows where every ancestor's instance data lies in its instances.
//! - **Multiple inheritance.** A class's methods start with its first
//!   parent's; later parents add the methods it lacks, and an override
//!   from a later parent replaces an implementation the earlier ones left
//!   at the introducer's. Instance data of an ancestor reached twice is
//!   laid out once.
//! - **Metaclasses.** `WrpSomMetaclassFor` gives a binding the metaclass
//!   for a new class: the requested one if it descends from the parent's
//!   metaclass, else the parent's if that descends from the requested one,
//!   else a derived metaclass with both as parents, made once.
//! - **Root classes.** The methods of `SOMObject`, `SOMClass` and
//!   `SOMClassMgr` are API thunks of this module, so their method tables
//!   hold thunk addresses like any guest procedure's. `somNew` calls the
//!   object's `somDefaultInit`, whose root implementation calls its
//!   `somInit`; `somFree` and `somDestruct` call its `somUninit` — guest
//!   overrides included — through call chains (ADR-0026).
//! - **Building classes** uses the documented `SOMClass` methods:
//!   `somNew` on a metaclass, `somInitClass` / `somInitMIClass`,
//!   `somAddStaticMethod`, `somOverrideSMethod`, `somClassReady`.
//! - **Memory.** Objects, method tables and `SOMMalloc` blocks come from a
//!   heap of 64 KB arenas ([`SomHeap`]); `somPrintf` formats as C does.
//!
//! Warpine's choices (unverified against IBM's SOM): the layout of method
//! tables and `ClassData` beyond their first word, the token encodings,
//! the module's ordinals (programs import by name), the resolution rule of
//! multiple inheritance, the derived metaclass's name, what `somPrintSelf`
//! and `somDumpSelf` print.

use std::collections::{HashMap, HashSet};

use super::constants::{MAGIC_API_BASE, SOM_BASE, STDOUT_HANDLE};
use super::pm_hook::{CallChain, CallStep, ChainThen};
use super::{ApiResult, Loader, MutexExt};

/// The functions of SOM.DLL (ordinals relative to `SOM_BASE`).
pub const SOM_ENVIRONMENT_NEW: u32 = 1;
pub const SOM_RESOLVE: u32 = 2;
pub const SOM_RESOLVE_BY_NAME: u32 = 3;
pub const SOM_CLASS_RESOLVE: u32 = 4;
pub const SOM_DATA_RESOLVE: u32 = 5;
pub const SOM_GET_GLOBAL_ENVIRONMENT: u32 = 6;
pub const SOM_ID_FROM_STRING: u32 = 7;
pub const SOM_STRING_FROM_ID: u32 = 8;
pub const SOM_COMPARE_IDS: u32 = 9;
pub const SOM_IS_OBJ: u32 = 10;
pub const SOM_PRINTF: u32 = 11;
/// The procedures the runtime variables `SOMMalloc` … `SOMFree` hold.
pub const SOM_MALLOC: u32 = 12;
pub const SOM_CALLOC: u32 = 13;
pub const SOM_REALLOC: u32 = 14;
pub const SOM_FREE_MEM: u32 = 15;
/// `WrpSomMetaclassFor(requested, parent)`: the metaclass of a new class.
pub const SOM_METACLASS_FOR: u32 = 16;
/// The procedures the runtime variables `SOMLoadModule`,
/// `SOMDeleteModule`, `SOMError` and `SOMOutCharRoutine` hold at first.
pub const SOM_DEFAULT_LOAD_MODULE: u32 = 17;
pub const SOM_DEFAULT_DELETE_MODULE: u32 = 18;
pub const SOM_DEFAULT_ERROR: u32 = 19;
pub const SOM_DEFAULT_OUT_CHAR: u32 = 35;
/// `somSetOutChar(routine)`: the routine `somPrintf` writes through.
pub const SOM_SET_OUT_CHAR: u32 = 36;
/// Data exports: their addresses, not thunks (`Loader::som_data_address`).
pub const SOM_OBJECT_CLASS_DATA: u32 = 20;
pub const SOM_CLASS_CLASS_DATA: u32 = 21;
pub const SOM_CLASS_MGR_CLASS_DATA: u32 = 22;
pub const SOM_CLASS_MGR_OBJECT: u32 = 23;
pub const SOM_VAR_MALLOC: u32 = 24;
pub const SOM_VAR_CALLOC: u32 = 25;
pub const SOM_VAR_REALLOC: u32 = 26;
pub const SOM_VAR_FREE: u32 = 27;
pub const SOM_TRACE_LEVEL: u32 = 28;
pub const SOM_WARN_LEVEL: u32 = 29;
pub const SOM_ASSERT_LEVEL: u32 = 30;
pub const SOM_VAR_OUT_CHAR: u32 = 31;
pub const SOM_VAR_LOAD_MODULE: u32 = 32;
pub const SOM_VAR_DELETE_MODULE: u32 = 33;
pub const SOM_VAR_ERROR: u32 = 34;
/// The last data export.
pub const SOM_LAST_DATA: u32 = SOM_VAR_ERROR;
/// The data exports, in the order of the data block.
const DATA_EXPORTS: usize = (SOM_LAST_DATA - SOM_OBJECT_CLASS_DATA + 1) as usize;
/// What an unknown method token resolves to: a procedure returning 0.
pub const SOM_METHOD_ERROR: u32 = 99;
/// The methods of `SOMObject`, `SOMClass` and `SOMClassMgr` (thunks).
pub const SOMOBJECT_METHODS: &[(u32, &str)] = &[
    (100, "somInit"), (101, "somUninit"), (102, "somFree"), (103, "somGetClass"),
    (104, "somGetClassName"), (105, "somGetSize"), (106, "somIsA"), (107, "somIsInstanceOf"),
    (108, "somRespondsTo"), (109, "somPrintSelf"), (110, "somDumpSelf"), (111, "somDumpSelfInt"),
    (112, "somDefaultInit"), (113, "somDestruct"),
];
pub const SOMCLASS_METHODS: &[(u32, &str)] = &[
    (200, "somNew"), (201, "somNewNoInit"), (202, "somGetName"), (203, "somGetParent"),
    (204, "somGetInstanceSize"), (205, "somGetInstancePartSize"), (206, "somGetInstanceToken"),
    (207, "somGetNumMethods"), (208, "somGetNumStaticMethods"), (209, "somDescendedFrom"),
    (210, "somInitClass"), (211, "somAddStaticMethod"), (212, "somOverrideSMethod"),
    (213, "somClassReady"), (214, "somGetMethodToken"), (215, "somSupportsMethod"),
    (216, "somInitMIClass"),
];
pub const SOMCLASSMGR_METHODS: &[(u32, &str)] = &[
    (300, "somFindClass"), (301, "somClassFromId"), (302, "somRegisterClass"),
    (303, "somFindClsInFile"), (304, "somLocateClassFile"), (305, "somLoadClassFile"),
    (306, "somUnregisterClass"), (307, "somUnloadClassFile"),
];

/// Most classes a process defines (Warpine's bound).
pub const MAX_CLASSES: usize = 4096;
/// Most methods of one class, and most methods of all classes.
pub const MAX_METHODS: usize = 4096;
pub const MAX_TOKENS: usize = 65536;
/// Most parents of one class.
pub const MAX_PARENTS: usize = 16;
/// Most objects alive at once.
pub const MAX_INSTANCES: usize = 1 << 20;
/// Most distinct `somId`s.
pub const MAX_IDS: usize = 65536;
/// Largest instance data of one class, in bytes.
pub const MAX_INSTANCE_DATA: u32 = 16 << 20;
/// Longest class or method name, in bytes.
pub const MAX_NAME: usize = 255;
/// Largest `SOMMalloc` block, and most live blocks.
pub const MAX_ALLOC: u32 = 64 << 20;
pub const MAX_BLOCKS: usize = 1 << 20;
/// Longest `somPrintf` output, in bytes.
pub const MAX_PRINT: usize = 65536;
/// The initialization function `somFindClass` asks `SOMLoadModule` to
/// call in a class's DLL (else `<Class>NewClass`).
pub const SOM_INIT_MODULE: &str = "SOMInitModule";
/// A SOM error code's last digit: its severity; 9 is fatal.
const SOM_FATAL: u32 = 9;
/// Bytes of an `Environment` (`_major`, exception id, its parameters).
const ENVIRONMENT_SIZE: u32 = 16;
/// Size of the heap's arenas; larger blocks get pages of their own.
const ARENA_SIZE: u32 = 64 << 10;
const SMALL_MAX: u32 = ARENA_SIZE / 2;
/// Tokens of the root methods the runtime calls itself.
const TOKEN_SOM_INIT: u32 = 1;
const TOKEN_SOM_UNINIT: u32 = 2;
const TOKEN_SOM_DUMP_SELF_INT: u32 = 12;
const TOKEN_SOM_DEFAULT_INIT: u32 = 13;

/// One method a class supports: its token and procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub token: u32,
    pub proc: u32,
}

/// A class: its object, name, place in the hierarchy, methods and instance
/// layout.
#[derive(Debug, Clone, Default)]
pub struct SomClass {
    pub name: String,
    /// Guest copy of the name (`somGetName`).
    pub name_addr: u32,
    /// Parent class objects, left to right (none: `SOMObject` itself).
    pub parents: Vec<u32>,
    /// Metaclass object.
    pub meta: u32,
    /// The methods it supports, inherited ones first.
    pub slots: Vec<Slot>,
    /// Methods this class introduced.
    pub own_methods: usize,
    /// Where each class's instance data lies in an instance: (class
    /// object, offset), this class's last.
    pub parts: Vec<(u32, u32)>,
    /// Size of the class's own instance data.
    pub data_size: u32,
    /// Size of an instance.
    pub instance_size: u32,
    /// Guest method table (0 until `somClassReady`).
    pub mtab: u32,
    pub ready: bool,
}

impl SomClass {
    fn slot(&self, token: u32) -> Option<&Slot> {
        self.slots.iter().find(|s| s.token == token)
    }

    fn offset_of(&self, cls: u32) -> Option<u32> {
        self.parts.iter().find(|p| p.0 == cls).map(|p| p.1)
    }
}

/// The runtime's heap: 64 KB arenas carved into 16-byte-rounded blocks,
/// freed blocks kept by size; large blocks are pages of their own.
#[derive(Debug, Default)]
pub struct SomHeap {
    /// Live blocks and their rounded sizes.
    pub blocks: HashMap<u32, u32>,
    /// Live blocks with pages of their own.
    big: HashSet<u32>,
    /// Freed blocks by rounded size.
    free: HashMap<u32, Vec<u32>>,
    cursor: u32,
    end: u32,
}

impl SomHeap {
    /// The rounded size of a request (`None`: too large).
    pub fn rounded(size: u32) -> Option<u32> {
        let r = size.max(1).checked_add(15)? & !15;
        (r <= MAX_ALLOC).then_some(r)
    }

    /// A small block from the free lists or the current arena.
    fn take(&mut self, rounded: u32) -> Option<u32> {
        if let Some(at) = self.free.get_mut(&rounded).and_then(Vec::pop) { return Some(at); }
        if self.end - self.cursor < rounded { return None; }
        let at = self.cursor;
        self.cursor += rounded;
        Some(at)
    }

    /// A new arena; the rest of the old one becomes a free block.
    fn add_arena(&mut self, at: u32) {
        if self.end > self.cursor { self.free.entry(self.end - self.cursor).or_default().push(self.cursor); }
        (self.cursor, self.end) = (at, at + ARENA_SIZE);
    }
}

/// The process's SOM runtime.
#[derive(Debug, Default)]
pub struct SomRuntime {
    pub initialized: bool,
    /// Classes by class object.
    pub classes: HashMap<u32, SomClass>,
    /// Ready classes by name.
    pub by_name: HashMap<String, u32>,
    /// Method tables → their class.
    pub mtabs: HashMap<u32, u32>,
    /// Every method: its introducing class and name (token = index + 1).
    pub methods: Vec<(u32, String)>,
    /// Derived metaclasses by (requested, parent's metaclass).
    pub derived: HashMap<(u32, u32), u32>,
    /// Objects `somNew` made and `somFree` has not freed.
    pub instances: HashSet<u32>,
    /// `somId`s by string.
    pub ids: HashMap<String, u32>,
    /// The root classes and the class manager.
    pub som_object: u32,
    pub som_class: u32,
    pub som_class_mgr: u32,
    pub mgr: u32,
    /// The global `Environment` and the exported data block.
    pub env: u32,
    pub data: u32,
    pub heap: SomHeap,
    /// Classes in the order they became ready (to tell which a module's
    /// loading made).
    pub ready_order: Vec<u32>,
    /// Classes made while the class manager loaded a module: its handle.
    pub modules: HashMap<u32, u32>,
}

impl SomRuntime {
    /// Whether class `cls` is `ancestor` or descends from it.
    pub fn descends(&self, cls: u32, ancestor: u32) -> bool {
        let mut seen = HashSet::new();
        let mut todo = vec![cls];
        while let Some(c) = todo.pop() {
            if c == ancestor { return true; }
            if seen.len() >= MAX_CLASSES || !seen.insert(c) { continue; }
            if let Some(k) = self.classes.get(&c) { todo.extend(&k.parents); }
        }
        false
    }

    /// The procedure of method `token` of class `cls`.
    pub fn class_method(&self, cls: u32, token: u32) -> Option<u32> {
        self.classes.get(&cls)?.slot(token).map(|s| s.proc)
    }

    /// The token of method `name` as class `cls` supports it.
    pub fn token_of(&self, cls: u32, name: &str) -> Option<u32> {
        let c = self.classes.get(&cls)?;
        c.slots.iter().map(|s| s.token).find(|&t| {
            t.checked_sub(1).and_then(|i| self.methods.get(i as usize)).is_some_and(|m| m.1 == name)
        })
    }

    /// A new token for method `name` introduced by `cls`.
    fn new_token(&mut self, cls: u32, name: &str) -> Option<u32> {
        if self.methods.len() >= MAX_TOKENS { return None; }
        self.methods.push((cls, name.to_string()));
        Some(self.methods.len() as u32)
    }

    /// The introducer's own procedure of method `token`.
    fn default_proc(&self, token: u32) -> Option<u32> {
        let intro = self.methods.get(token.checked_sub(1)? as usize)?.0;
        self.class_method(intro, token)
    }

    /// Lay out class `cls` with `parents` (all ready) and `size` bytes of
    /// instance data of its own: methods and data parts inherited.
    fn inherit(&mut self, cls: u32, parents: &[u32], size: u32) -> Option<()> {
        let first = self.classes.get(parents.first()?)?;
        let mut slots = first.slots.clone();
        let mut parts = first.parts.clone();
        let mut end = first.instance_size;
        for p in &parents[1..] {
            let p = self.classes.get(p)?;
            for s in &p.slots {
                match slots.iter().position(|o| o.token == s.token) {
                    None => slots.push(*s),
                    Some(i) => {
                        let default = self.default_proc(s.token);
                        if Some(slots[i].proc) == default && Some(s.proc) != default { slots[i].proc = s.proc; }
                    }
                }
            }
            for &(pc, _) in &p.parts {
                if parts.iter().any(|q| q.0 == pc) { continue; }
                let at = end.checked_add(3)? & !3;
                parts.push((pc, at));
                end = at.checked_add(self.classes.get(&pc)?.data_size)?;
            }
        }
        if slots.len() > MAX_METHODS { return None; }
        let at = end.checked_add(3)? & !3;
        parts.push((cls, at));
        let instance_size = at.checked_add(size)?.checked_add(3)? & !3;
        let c = self.classes.get_mut(&cls)?;
        (c.parents, c.slots, c.parts, c.own_methods, c.data_size, c.instance_size) =
            (parents.to_vec(), slots, parts, 0, size, instance_size);
        Some(())
    }
}

/// Whether the first `inits` calls of a chain, `_DLL_InitTerm`s, all
/// succeeded (TRUE).
fn som_inits_ok(results: &[u32], inits: u32) -> bool {
    !results.iter().take(inits as usize).any(|&r| r == 0)
}

/// The thunk address of SOM ordinal `ord`.
pub fn thunk(ord: u32) -> u32 {
    (MAGIC_API_BASE + (SOM_BASE + ord) as u64) as u32
}

/// The offsets in the exported data block of data exports 20..=34: each
/// root class's `ClassData` (class object, then a token per method of its
/// own), `SOMClassMgrObject`, the four allocation variables, the three
/// levels, `SOMOutCharRoutine`, `SOMLoadModule`, `SOMDeleteModule`,
/// `SOMError`.
fn data_offsets() -> [u32; DATA_EXPORTS] {
    let sizes = [
        4 + 4 * SOMOBJECT_METHODS.len() as u32,
        4 + 4 * SOMCLASS_METHODS.len() as u32,
        4 + 4 * SOMCLASSMGR_METHODS.len() as u32,
    ];
    let mut offs = [0u32; DATA_EXPORTS];
    let mut at = 0;
    for (i, o) in offs.iter_mut().enumerate() {
        *o = at;
        at += sizes.get(i).copied().unwrap_or(4);
    }
    offs
}

/// Bytes of the exported data block.
fn data_size() -> u32 {
    data_offsets()[DATA_EXPORTS - 1] + 4
}

/// Format `fmt` as C's `printf` does, taking each argument word from
/// `args` (doubles and `long long`s take two) and strings through
/// `string_at`. Conversions `d i u o x X c s p f F e E g G %`, flags,
/// width, precision and `h` / `l` / `ll` / `L` lengths; output at most
/// [`MAX_PRINT`] bytes.
pub fn c_format(fmt: &[u8], args: &mut dyn Iterator<Item = u32>, string_at: &dyn Fn(u32) -> Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut next = || args.next().unwrap_or(0);
    let mut i = 0;
    while i < fmt.len() && out.len() < MAX_PRINT {
        let b = fmt[i];
        i += 1;
        if b != b'%' { out.push(b); continue; }
        let (mut left, mut zero, mut plus, mut space, mut alt) = (false, false, false, false, false);
        while let Some(&f) = fmt.get(i) {
            match f {
                b'-' => left = true,
                b'0' => zero = true,
                b'+' => plus = true,
                b' ' => space = true,
                b'#' => alt = true,
                _ => break,
            }
            i += 1;
        }
        let number = |i: &mut usize, next: &mut dyn FnMut() -> u32| -> Option<i64> {
            if fmt.get(*i) == Some(&b'*') { *i += 1; return Some(next() as i32 as i64); }
            let start = *i;
            let mut v = 0i64;
            while let Some(d) = fmt.get(*i).filter(|d| d.is_ascii_digit()) {
                v = (v * 10 + (d - b'0') as i64).min(4096);
                *i += 1;
            }
            (*i > start).then_some(v)
        };
        let mut width = number(&mut i, &mut next).unwrap_or(0);
        if width < 0 { left = true; width = -width; }
        let width = width.min(4096) as usize;
        let precision = if fmt.get(i) == Some(&b'.') {
            i += 1;
            let p = number(&mut i, &mut next).unwrap_or(0);
            (p >= 0).then_some(p.min(4096) as usize)
        } else { None };
        let (mut half, mut long2) = (false, false);
        while let Some(&l) = fmt.get(i) {
            match l {
                b'h' => half = true,
                b'l' if fmt.get(i + 1) == Some(&b'l') => { long2 = true; i += 1; }
                b'l' => {}
                b'L' => long2 = true,
                _ => break,
            }
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else { out.push(b'%'); break };
        i += 1;
        let wide = |next: &mut dyn FnMut() -> u32| -> u64 {
            if long2 { let lo = next() as u64; lo | (next() as u64) << 32 } else { next() as u64 }
        };
        let (prefix, body): (Vec<u8>, Vec<u8>) = match conv {
            b'd' | b'i' => {
                let w = wide(&mut next);
                let v = if long2 { w as i64 } else if half { w as i16 as i64 } else { w as i32 as i64 };
                let sign = if v < 0 { "-" } else if plus { "+" } else if space { " " } else { "" };
                (sign.into(), digits(v.unsigned_abs().to_string(), precision))
            }
            b'u' | b'o' | b'x' | b'X' => {
                let w = wide(&mut next);
                let v = if long2 { w } else if half { w as u16 as u64 } else { w as u32 as u64 };
                let (s, pre) = match conv {
                    b'o' => (format!("{v:o}"), if alt { "0" } else { "" }),
                    b'x' => (format!("{v:x}"), if alt && v != 0 { "0x" } else { "" }),
                    b'X' => (format!("{v:X}"), if alt && v != 0 { "0X" } else { "" }),
                    _ => (v.to_string(), ""),
                };
                (pre.into(), digits(s, precision))
            }
            b'p' => (Vec::new(), format!("{:08X}", next()).into_bytes()),
            b'c' => (Vec::new(), vec![next() as u8]),
            b's' => {
                let p = next();
                let mut s = if p == 0 { b"(null)".to_vec() } else { string_at(p) };
                if let Some(n) = precision { s.truncate(n); }
                (Vec::new(), s)
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let lo = next() as u64;
                let v = f64::from_bits(lo | (next() as u64) << 32);
                let sign = if v.is_sign_negative() { "-" } else if plus { "+" } else if space { " " } else { "" };
                (sign.into(), float(v.abs(), conv, precision.unwrap_or(6), alt).into_bytes())
            }
            b'%' => { out.push(b'%'); continue; }
            b'n' => { next(); continue; }
            other => { out.extend([b'%', other]); continue; }
        };
        let pad = width.saturating_sub(prefix.len() + body.len());
        let numeric = !matches!(conv, b's' | b'c' | b'p');
        let floating = matches!(conv, b'f' | b'F' | b'e' | b'E' | b'g' | b'G');
        if left {
            out.extend(&prefix);
            out.extend(&body);
            out.extend(std::iter::repeat_n(b' ', pad));
        } else if zero && numeric && (precision.is_none() || floating) {
            out.extend(&prefix);
            out.extend(std::iter::repeat_n(b'0', pad));
            out.extend(&body);
        } else {
            out.extend(std::iter::repeat_n(b' ', pad));
            out.extend(&prefix);
            out.extend(&body);
        }
    }
    out.truncate(MAX_PRINT);
    out
}

/// Integer digits with at least `precision` of them (`.0` of 0: none).
fn digits(s: String, precision: Option<usize>) -> Vec<u8> {
    match precision {
        Some(0) if s == "0" => Vec::new(),
        Some(p) if p > s.len() => format!("{}{s}", "0".repeat(p - s.len())).into_bytes(),
        _ => s.into_bytes(),
    }
}

/// A non-negative double as `%f`, `%e` or `%g` (upper case for `F E G`).
fn float(v: f64, conv: u8, precision: usize, alt: bool) -> String {
    if !v.is_finite() {
        let s = if v.is_nan() { "nan" } else { "inf" };
        return if conv.is_ascii_uppercase() { s.to_uppercase() } else { s.into() };
    }
    let exp = |v: f64, p: usize| -> String {
        let s = format!("{v:.p$e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let e: i32 = e.parse().unwrap_or(0);
        format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.unsigned_abs())
    };
    let s = match conv.to_ascii_lowercase() {
        b'f' => format!("{v:.precision$}"),
        b'e' => exp(v, precision),
        _ => {
            let p = precision.max(1);
            let x: i64 = format!("{v:.*e}", p - 1).split_once('e').and_then(|(_, e)| e.parse().ok()).unwrap_or(0);
            let s = if x < p as i64 && x >= -4 {
                format!("{v:.*}", (p as i64 - 1 - x).max(0) as usize)
            } else {
                exp(v, p - 1)
            };
            if alt { s } else {
                let (m, e) = s.split_at(s.find('e').unwrap_or(s.len()));
                let m = if m.contains('.') { m.trim_end_matches('0').trim_end_matches('.') } else { m };
                format!("{m}{e}")
            }
        }
    };
    if conv.is_ascii_uppercase() { s.to_uppercase() } else { s }
}

impl Loader {
    /// `SOMMalloc`: a zeroed block from the runtime's heap.
    fn som_malloc(&self, size: u32) -> Option<u32> {
        let rounded = SomHeap::rounded(size)?;
        let at = {
            let mut rt = self.shared.som.lock_or_recover();
            if rt.heap.blocks.len() >= MAX_BLOCKS { return None; }
            let at = if rounded > SMALL_MAX {
                let at = self.shared.mem_mgr.lock_or_recover().alloc(rounded)?;
                rt.heap.big.insert(at);
                at
            } else if let Some(at) = rt.heap.take(rounded) {
                at
            } else {
                let arena = self.shared.mem_mgr.lock_or_recover().alloc(ARENA_SIZE)?;
                rt.heap.add_arena(arena);
                rt.heap.take(rounded)?
            };
            rt.heap.blocks.insert(at, rounded);
            at
        };
        if let Some(m) = self.guest_slice_mut(at, rounded as usize) { m.fill(0); }
        Some(at)
    }

    /// `SOMFree`: a block back to the heap (others are ignored).
    fn som_free_mem(&self, at: u32) -> bool {
        let mut rt = self.shared.som.lock_or_recover();
        let Some(size) = rt.heap.blocks.remove(&at) else { return false };
        if rt.heap.big.remove(&at) {
            self.shared.mem_mgr.lock_or_recover().free(at);
        } else {
            rt.heap.free.entry(size).or_default().push(at);
        }
        true
    }

    /// `SOMRealloc`: a block of `size` bytes holding what `at` held.
    fn som_realloc(&self, at: u32, size: u32) -> u32 {
        if at == 0 { return self.som_malloc(size).unwrap_or(0); }
        let Some(old) = self.shared.som.lock_or_recover().heap.blocks.get(&at).copied() else { return 0 };
        if size == 0 { self.som_free_mem(at); return 0; }
        let Some(new) = self.som_malloc(size) else { return 0 };
        if let Some(bytes) = self.guest_read_bytes(at, old.min(size) as usize) {
            let _ = self.guest_write_bytes(new, &bytes);
        }
        self.som_free_mem(at);
        new
    }

    /// A NUL-terminated guest copy of `s`.
    fn som_string(&self, s: &str) -> u32 {
        let Some(at) = self.som_malloc(s.len() as u32 + 1) else { return 0 };
        let _ = self.guest_write_bytes(at, s.as_bytes());
        at
    }

    /// The NUL-terminated bytes at `ptr` (at most `max`).
    fn som_bytes(&self, ptr: u32, max: usize) -> Option<Vec<u8>> {
        if ptr == 0 { return None; }
        let mut out = Vec::new();
        for i in 0..=max as u32 {
            match self.guest_read::<u8>(ptr.checked_add(i)?)? {
                0 => return Some(out),
                b => out.push(b),
            }
        }
        None
    }

    /// The name at `ptr` (at most [`MAX_NAME`] bytes).
    fn som_name(&self, ptr: u32) -> Option<String> {
        self.som_bytes(ptr, MAX_NAME).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The string of `somId` `id` (a pointer to the string's pointer).
    fn som_id_name(&self, id: u32) -> Option<String> {
        if id == 0 { return None; }
        self.som_name(self.guest_read::<u32>(id)?)
    }

    /// Write class `cls`'s method table into guest memory.
    fn som_write_mtab(&self, cls: u32) -> Option<u32> {
        let procs: Vec<u32> = self.shared.som.lock_or_recover().classes.get(&cls)?.slots.iter().map(|s| s.proc).collect();
        let at = self.som_malloc(8 + 4 * procs.len() as u32)?;
        self.guest_write::<u32>(at, cls)?;
        self.guest_write::<u32>(at + 4, procs.len() as u32)?;
        for (i, p) in procs.iter().enumerate() {
            self.guest_write::<u32>(at + 8 + 4 * i as u32, *p)?;
        }
        let mut rt = self.shared.som.lock_or_recover();
        if let Some(c) = rt.classes.get_mut(&cls) { (c.mtab, c.ready) = (at, true); }
        rt.mtabs.insert(at, cls);
        Some(at)
    }

    /// Start the runtime once: the root classes, their method tables and
    /// class objects, the class manager, the global environment and the
    /// exported data.
    pub(crate) fn som_init(&self) {
        {
            let mut rt = self.shared.som.lock_or_recover();
            if rt.initialized { return; }
            rt.initialized = true;
        }
        let (Some(o), Some(c), Some(m), Some(mgr), Some(env), Some(data)) = (
            self.som_malloc(4), self.som_malloc(4), self.som_malloc(4),
            self.som_malloc(4), self.som_malloc(ENVIRONMENT_SIZE), self.som_malloc(data_size()),
        ) else {
            log::warn!("SOM: no memory for the runtime");
            return;
        };
        let names: Vec<u32> = ["SOMObject", "SOMClass", "SOMClassMgr"].iter().map(|n| self.som_string(n)).collect();
        {
            let mut rt = self.shared.som.lock_or_recover();
            let mut root = Vec::new();
            for &(ord, n) in SOMOBJECT_METHODS {
                let token = rt.new_token(o, n).unwrap_or(0);
                root.push(Slot { token, proc: thunk(ord) });
            }
            let mut with = |cls: u32, own: &[(u32, &str)]| -> Vec<Slot> {
                let mut slots = root.clone();
                for &(ord, n) in own { slots.push(Slot { token: rt.new_token(cls, n).unwrap_or(0), proc: thunk(ord) }); }
                slots
            };
            let cls_slots = with(c, SOMCLASS_METHODS);
            let mgr_slots = with(m, SOMCLASSMGR_METHODS);
            for (obj, name, name_addr, parents, slots, own) in [
                (o, "SOMObject", names[0], vec![], root.clone(), SOMOBJECT_METHODS.len()),
                (c, "SOMClass", names[1], vec![o], cls_slots, SOMCLASS_METHODS.len()),
                (m, "SOMClassMgr", names[2], vec![o], mgr_slots, SOMCLASSMGR_METHODS.len()),
            ] {
                // SOMObject's "data" is the method-table word.
                let parts = if obj == o { vec![(o, 0)] } else { vec![(o, 0), (obj, 4)] };
                rt.classes.insert(obj, SomClass {
                    name: name.to_string(), name_addr, parents, meta: c, slots, own_methods: own, parts,
                    data_size: if obj == o { 4 } else { 0 }, instance_size: 4, mtab: 0, ready: false,
                });
                rt.by_name.insert(name.to_string(), obj);
            }
            (rt.som_object, rt.som_class, rt.som_class_mgr, rt.mgr, rt.env, rt.data) = (o, c, m, mgr, env, data);
        }
        let mtabs: Vec<Option<u32>> = [o, c, m].iter().map(|&k| self.som_write_mtab(k)).collect();
        let (Some(_), Some(cls_mtab), Some(mgr_mtab)) = (mtabs[0], mtabs[1], mtabs[2]) else { return };
        // Class objects are instances of SOMClass; the manager of SOMClassMgr.
        for k in [o, c, m] { let _ = self.guest_write::<u32>(k, cls_mtab); }
        let _ = self.guest_write::<u32>(mgr, mgr_mtab);
        self.shared.som.lock_or_recover().instances.extend([o, c, m, mgr]);
        // The exported data: ClassData of the root classes, the manager,
        // the runtime variables.
        let offs = data_offsets();
        let tokens = |cls: u32| -> Vec<u32> {
            let rt = self.shared.som.lock_or_recover();
            (1..=rt.methods.len() as u32).filter(|&t| rt.methods[t as usize - 1].0 == cls).collect()
        };
        for (base, cls) in [(offs[0], o), (offs[1], c), (offs[2], m)] {
            let _ = self.guest_write::<u32>(data + base, cls);
            for (i, t) in tokens(cls).into_iter().enumerate() {
                let _ = self.guest_write::<u32>(data + base + 4 + 4 * i as u32, t);
            }
        }
        let _ = self.guest_write::<u32>(data + offs[3], mgr);
        for (var, f) in [
            (SOM_VAR_MALLOC, SOM_MALLOC), (SOM_VAR_CALLOC, SOM_CALLOC), (SOM_VAR_REALLOC, SOM_REALLOC),
            (SOM_VAR_FREE, SOM_FREE_MEM), (SOM_VAR_OUT_CHAR, SOM_DEFAULT_OUT_CHAR),
            (SOM_VAR_LOAD_MODULE, SOM_DEFAULT_LOAD_MODULE), (SOM_VAR_DELETE_MODULE, SOM_DEFAULT_DELETE_MODULE),
            (SOM_VAR_ERROR, SOM_DEFAULT_ERROR),
        ] {
            let _ = self.guest_write::<u32>(data + offs[(var - SOM_OBJECT_CLASS_DATA) as usize], thunk(f));
        }
    }

    /// The address of SOM data export `ord` (`None`: not a data export).
    pub(crate) fn som_data_address(&self, ord: u32) -> Option<u32> {
        if !(SOM_OBJECT_CLASS_DATA..=SOM_LAST_DATA).contains(&ord) { return None; }
        self.som_init();
        let data = self.shared.som.lock_or_recover().data;
        (data != 0).then(|| data + data_offsets()[(ord - SOM_OBJECT_CLASS_DATA) as usize])
    }

    /// The class of object `obj` (from its method table).
    fn som_class_of(&self, obj: u32) -> Option<u32> {
        let mtab = self.guest_read::<u32>(obj)?;
        self.shared.som.lock_or_recover().mtabs.get(&mtab).copied()
    }

    /// The procedure of method `token` for object `obj`.
    fn som_resolve(&self, obj: u32, token: u32) -> u32 {
        let found = self.som_class_of(obj).and_then(|c| self.shared.som.lock_or_recover().class_method(c, token));
        found.unwrap_or_else(|| {
            log::warn!("SOM: no method {token} for object {obj:#x}");
            thunk(SOM_METHOD_ERROR)
        })
    }

    /// `somDataResolve`: where class `token`'s instance data lies in `obj`.
    fn som_data_resolve(&self, obj: u32, token: u32) -> u32 {
        let cls = self.som_class_of(obj);
        let off = cls.and_then(|c| self.shared.som.lock_or_recover().classes.get(&c)?.offset_of(token));
        match off {
            Some(off) => obj.wrapping_add(off),
            None => {
                log::warn!("SOM: no instance data {token:#x} in object {obj:#x}");
                0
            }
        }
    }

    /// `somIdFromString`: the one `somId` of `name` (a pointer to a pointer
    /// to the string).
    fn som_id(&self, name: &str) -> u32 {
        if let Some(&id) = self.shared.som.lock_or_recover().ids.get(name) { return id; }
        if self.shared.som.lock_or_recover().ids.len() >= MAX_IDS { return 0; }
        let Some(id) = self.som_malloc(4 + name.len() as u32 + 1) else { return 0 };
        let _ = self.guest_write::<u32>(id, id + 4);
        let _ = self.guest_write_bytes(id + 4, name.as_bytes());
        self.shared.som.lock_or_recover().ids.insert(name.to_string(), id);
        id
    }

    /// A new instance of ready class `cls`, zeroed, its method-table word
    /// set (an instance of a metaclass is a new, uninitialized class).
    fn som_new_instance(&self, cls: u32) -> Option<u32> {
        let (size, mtab, is_class) = {
            let rt = self.shared.som.lock_or_recover();
            let c = rt.classes.get(&cls).filter(|c| c.ready)?;
            if rt.instances.len() >= MAX_INSTANCES { return None; }
            (c.instance_size, c.mtab, rt.descends(cls, rt.som_class))
        };
        let obj = self.som_malloc(size)?;
        self.guest_write::<u32>(obj, mtab)?;
        let mut rt = self.shared.som.lock_or_recover();
        rt.instances.insert(obj);
        if is_class && rt.classes.len() < MAX_CLASSES {
            rt.classes.insert(obj, SomClass { meta: cls, ..SomClass::default() });
        }
        Some(obj)
    }

    /// `somInitClass` / `somInitMIClass`: class object `cls`, not yet
    /// initialized, gets its name, parents (none: `SOMObject`) and `size`
    /// bytes of instance data.
    fn som_init_class(&self, cls: u32, name: u32, parents: &[u32], size: u32) -> u32 {
        let Some(name) = self.som_name(name) else { return 0 };
        if size > MAX_INSTANCE_DATA || parents.len() > MAX_PARENTS { return 0; }
        let name_addr = self.som_string(&name);
        let mut rt = self.shared.som.lock_or_recover();
        let mut parents: Vec<u32> = parents.iter().copied().filter(|&p| p != 0).collect();
        if parents.is_empty() { parents.push(rt.som_object); }
        if parents.iter().any(|p| !rt.classes.get(p).is_some_and(|p| p.ready)) { return 0; }
        if !rt.classes.get(&cls).is_some_and(|c| !c.ready && c.name.is_empty()) { return 0; }
        if rt.inherit(cls, &parents, size).is_none() { return 0; }
        if let Some(c) = rt.classes.get_mut(&cls) { (c.name, c.name_addr) = (name, name_addr); }
        0
    }

    /// `somAddStaticMethod(self, methodId, methodDescriptor, method,
    /// redispatchStub, applyStub)` → the method's token (0: failed).
    fn som_add_method(&self, cls: u32, id: u32, method: u32) -> u32 {
        let Some(name) = self.som_id_name(id) else { return 0 };
        let mut rt = self.shared.som.lock_or_recover();
        if !rt.classes.get(&cls).is_some_and(|c| !c.ready && !c.name.is_empty()) { return 0; }
        if let Some(t) = rt.token_of(cls, &name) {
            if let Some(s) = rt.classes.get_mut(&cls).and_then(|c| c.slots.iter_mut().find(|s| s.token == t)) {
                s.proc = method;
            }
            return t;
        }
        if rt.classes.get(&cls).is_some_and(|c| c.slots.len() >= MAX_METHODS) { return 0; }
        let Some(token) = rt.new_token(cls, &name) else { return 0 };
        if let Some(c) = rt.classes.get_mut(&cls) {
            c.slots.push(Slot { token, proc: method });
            c.own_methods += 1;
        }
        token
    }

    /// `somOverrideSMethod(self, methodId, method)`.
    fn som_override(&self, cls: u32, id: u32, method: u32) -> u32 {
        let Some(name) = self.som_id_name(id) else { return 0 };
        let mut rt = self.shared.som.lock_or_recover();
        if !rt.classes.get(&cls).is_some_and(|c| !c.ready && !c.name.is_empty()) { return 0; }
        match rt.token_of(cls, &name) {
            Some(t) => {
                if let Some(s) = rt.classes.get_mut(&cls).and_then(|c| c.slots.iter_mut().find(|s| s.token == t)) {
                    s.proc = method;
                }
            }
            None => log::warn!("SOM: no method {name} to override"),
        }
        0
    }

    /// `somClassReady(self)`: the class's method table is written and it
    /// is registered with the class manager.
    fn som_class_ready(&self, cls: u32) -> u32 {
        let name = {
            let rt = self.shared.som.lock_or_recover();
            match rt.classes.get(&cls) {
                Some(c) if !c.ready && !c.name.is_empty() => c.name.clone(),
                _ => return 0,
            }
        };
        if self.som_write_mtab(cls).is_some() {
            let mut rt = self.shared.som.lock_or_recover();
            rt.by_name.insert(name, cls);
            if rt.ready_order.len() < MAX_CLASSES { rt.ready_order.push(cls); }
        }
        0
    }

    /// `somUnregisterClass`: the class manager forgets class `cls`
    /// (`somFindClass` will look for it again); 0, or 1 if it was not
    /// registered.
    fn som_unregister(&self, cls: u32) -> u32 {
        let mut rt = self.shared.som.lock_or_recover();
        let Some(name) = rt.classes.get(&cls).map(|c| c.name.clone()) else { return 1 };
        if rt.by_name.get(&name) != Some(&cls) { return 1; }
        rt.by_name.remove(&name);
        0
    }

    /// `somUnloadClassFile`: every class the module of class `cls` made is
    /// unregistered and its objects' methods resolve to nothing (their
    /// code goes with the module), then `SOMDeleteModule` frees the
    /// module. Nonzero if the class did not come from a module the class
    /// manager loaded.
    fn som_unload_class_file(&self, vcpu: u32, cls: u32) -> ApiResult {
        let hmod = {
            let mut rt = self.shared.som.lock_or_recover();
            let Some(&hmod) = rt.modules.get(&cls) else { return ApiResult::Normal(1) };
            let made: Vec<u32> = rt.modules.iter().filter(|&(_, &m)| m == hmod).map(|(&c, _)| c).collect();
            for c in made {
                rt.modules.remove(&c);
                let Some((name, mtab)) = rt.classes.get(&c).map(|k| (k.name.clone(), k.mtab)) else { continue };
                if rt.by_name.get(&name) == Some(&c) { rt.by_name.remove(&name); }
                rt.mtabs.remove(&mtab);
                rt.ready_order.retain(|&k| k != c);
            }
            hmod
        };
        let deleter = self.som_var(SOM_VAR_DELETE_MODULE);
        if deleter == thunk(SOM_DEFAULT_DELETE_MODULE) || deleter == 0 {
            return self.dos_free_module(hmod);
        }
        self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(deleter, vec![hmod], false)], false, ChainThen::ReturnLast))
    }

    /// `WrpSomMetaclassFor(requested, parent)`: the metaclass of a new
    /// class with parent `parent` (0: `SOMObject`) that asked for
    /// `requested` (0: none); 0 if `requested` is no metaclass.
    fn som_metaclass_for(&self, requested: u32, parent: u32) -> u32 {
        let (requested, parent_meta, som_class) = {
            let rt = self.shared.som.lock_or_recover();
            let parent = if parent == 0 { rt.som_object } else { parent };
            let parent_meta = rt.classes.get(&parent).map_or(rt.som_class, |p| p.meta);
            (if requested == 0 { parent_meta } else { requested }, parent_meta, rt.som_class)
        };
        let names = {
            let rt = self.shared.som.lock_or_recover();
            let ready = |c: u32| rt.classes.get(&c).is_some_and(|c| c.ready);
            if !ready(requested) || !ready(parent_meta) || !rt.descends(requested, som_class) { return 0; }
            if rt.descends(requested, parent_meta) { return requested; }
            if rt.descends(parent_meta, requested) { return parent_meta; }
            if let Some(&d) = rt.derived.get(&(requested, parent_meta)) { return d; }
            if rt.derived.len() >= MAX_CLASSES { return 0; }
            [requested, parent_meta].map(|c| rt.classes.get(&c).map(|c| c.name.clone()).unwrap_or_default())
        };
        // A derived metaclass: both as parents, no data of its own.
        let Some(d) = self.som_new_instance(som_class) else { return 0 };
        let name = format!("{}+{}", names[0], names[1]);
        let name_addr = self.som_string(&name);
        {
            let mut rt = self.shared.som.lock_or_recover();
            if rt.inherit(d, &[requested, parent_meta], 0).is_none() { return 0; }
            if let Some(c) = rt.classes.get_mut(&d) { (c.name, c.name_addr) = (name, name_addr); }
            rt.derived.insert((requested, parent_meta), d);
        }
        self.som_class_ready(d);
        d
    }

    /// `somPrintf(fmt, …)`: formatted to standard output; at most 9
    /// argument words.
    fn som_printf(&self, vcpu: u32, a: &[u32; 10]) -> ApiResult {
        let Some(fmt) = self.som_bytes(a[0], MAX_PRINT) else { return ApiResult::Normal(0) };
        let string_at = |p: u32| self.som_bytes(p, MAX_PRINT).unwrap_or_default();
        let out = c_format(&fmt, &mut a[1..].iter().copied(), &string_at);
        // A program's own SOMOutCharRoutine gets every character.
        let routine = self.som_var(SOM_VAR_OUT_CHAR);
        if routine == thunk(SOM_DEFAULT_OUT_CHAR) || routine == 0 {
            return ApiResult::Normal(self.write_handle(STDOUT_HANDLE, &out).unwrap_or(0));
        }
        let steps = out.iter().map(|&c| CallStep::hook(routine, vec![c as u32], false)).collect();
        self.start_chain(CallChain::new(vcpu, steps, false, ChainThen::Return(out.len() as u32)))
    }

    /// The value of runtime variable `ord` (`SOMOutCharRoutine` …).
    fn som_var(&self, ord: u32) -> u32 {
        self.som_data_address(ord).and_then(|at| self.guest_read::<u32>(at)).unwrap_or(0)
    }

    /// Load `file` for class `class` as the default `SOMLoadModule` does:
    /// the module, the `_DLL_InitTerm`s of it and the modules loaded with
    /// it, then its initialization function `func(major, minor,
    /// class_ptr)` or, without one, `<class>NewClass(major, minor)`.
    /// Gives the module, the calls to make and how many of them are
    /// `_DLL_InitTerm`s; or an OS/2 error.
    fn som_module_steps(&self, file: &str, func: Option<&str>, class: &str, class_ptr: u32,
                        major: u32, minor: u32) -> Result<(u32, Vec<CallStep>, u32), u32> {
        let Some((module, path)) = self.find_module_file(file) else {
            log::warn!("SOM: no class file {file} for class {class}");
            return Err(super::constants::ERROR_MOD_NOT_FOUND);
        };
        let hmod = self.load_dll(&module, &path).map_err(|e| {
            log::warn!("SOM: loading {file} for class {class} failed: {e}");
            super::constants::ERROR_MOD_NOT_FOUND
        })?;
        let pending = self.shared.dll_mgr.lock_or_recover().take_pending_init();
        let mut steps: Vec<CallStep> = pending.iter().map(|&(m, e)| CallStep::hook(e, vec![m, 0], false)).collect();
        let inits = steps.len() as u32;
        let export = |name: &str| {
            let dlls = self.shared.dll_mgr.lock_or_recover();
            dlls.find_by_handle(hmod)?.exports_by_name.get(&name.to_ascii_uppercase()).copied()
        };
        if let Some(addr) = func.and_then(export) {
            steps.push(CallStep::hook(addr, vec![major, minor, class_ptr], false));
        } else if let Some(addr) = export(&format!("{class}NewClass")) {
            steps.push(CallStep::hook(addr, vec![major, minor], false));
        }
        Ok((hmod, steps, inits))
    }

    /// `somFindClass` / `somFindClsInFile` / `somLoadClassFile`: class
    /// `id` if it is registered (unless `load`), else loaded from `file`
    /// (none: the class's name) through `SOMLoadModule`.
    fn som_find_class(&self, vcpu: u32, id: u32, major: u32, minor: u32, file: Option<u32>, load: bool) -> ApiResult {
        let Some(name) = self.som_id_name(id) else { return ApiResult::Normal(0) };
        if !load && let Some(&cls) = self.shared.som.lock_or_recover().by_name.get(&name) {
            return ApiResult::Normal(cls);
        }
        let file = match file {
            Some(p) => match self.som_name(p) { Some(f) => f, None => return ApiResult::Normal(0) },
            None => name.clone(),
        };
        // A block for the strings and the module handle SOMLoadModule takes.
        let strings = [name.as_str(), file.as_str(), SOM_INIT_MODULE];
        let size = 4 + strings.iter().map(|s| s.len() as u32 + 1).sum::<u32>();
        let Some(block) = self.som_malloc(size) else { return ApiResult::Normal(0) };
        let mut at = block + 4;
        let mut ptrs = Vec::new();
        for s in strings {
            let _ = self.guest_write_bytes(at, s.as_bytes());
            let _ = self.guest_write::<u8>(at + s.len() as u32, 0);
            ptrs.push(at);
            at += s.len() as u32 + 1;
        }
        let loader = self.som_var(SOM_VAR_LOAD_MODULE);
        let first = self.shared.som.lock_or_recover().ready_order.len() as u32;
        if loader == thunk(SOM_DEFAULT_LOAD_MODULE) {
            match self.som_module_steps(&file, Some(SOM_INIT_MODULE), &name, ptrs[0], major, minor) {
                Ok((hmod, steps, inits)) => {
                    let _ = self.guest_write::<u32>(block, hmod);
                    self.start_chain(CallChain::new(vcpu, steps, false, ChainThen::SomClassLoaded { id, block, inits, first }))
                }
                Err(_) => { self.som_free_mem(block); ApiResult::Normal(0) }
            }
        } else {
            // The program's own SOMLoadModule; 0 is success, the module
            // handle in the block's first word.
            let step = CallStep::hook(loader, vec![ptrs[0], ptrs[1], ptrs[2], major, minor, block], false);
            self.start_chain(CallChain::new(vcpu, vec![step], false,
                                            ChainThen::SomClassLoaded { id, block, inits: u32::MAX, first }))
        }
    }

    /// The calls loading a class's file returned: the class, if the
    /// loading succeeded and made it (`inits` `_DLL_InitTerm`s first, or
    /// `u32::MAX`: a program's `SOMLoadModule`, its result last).
    /// The classes that became ready since `first` came from the module
    /// whose handle is in the block's first word.
    pub(crate) fn som_class_loaded(&self, id: u32, block: u32, inits: u32, first: u32, chain: &CallChain) -> u32 {
        let hmod = self.guest_read::<u32>(block).unwrap_or(0);
        self.som_free_mem(block);
        let loaded = if inits == u32::MAX { chain.last == 0 } else { som_inits_ok(&chain.results, inits) };
        if loaded && hmod != 0 {
            let mut rt = self.shared.som.lock_or_recover();
            let made: Vec<u32> = rt.ready_order.iter().skip(first as usize).copied().collect();
            for c in made { rt.modules.insert(c, hmod); }
        }
        let Some(name) = self.som_id_name(id) else { return 0 };
        let cls = self.shared.som.lock_or_recover().by_name.get(&name).copied();
        match (loaded, cls) {
            (true, Some(cls)) => cls,
            (true, None) => { log::warn!("SOM: the class file of {name} did not make the class"); 0 }
            (false, _) => { log::warn!("SOM: loading the class file of {name} failed"); 0 }
        }
    }

    /// The default `SOMLoadModule` made its calls: the module handle goes
    /// to `phandle` and 0 is returned when every `_DLL_InitTerm` succeeded.
    pub(crate) fn som_module_loaded(&self, hmod: u32, phandle: u32, inits: u32, chain: &CallChain) -> u32 {
        if !som_inits_ok(&chain.results, inits) {
            return super::constants::ERROR_INIT_ROUTINE_FAILED;
        }
        if phandle != 0 { let _ = self.guest_write::<u32>(phandle, hmod); }
        0
    }

    /// The text `somPrintSelf` and `somDumpSelf` give an object.
    fn som_describe(&self, obj: u32) -> String {
        let cls = self.som_class_of(obj).unwrap_or(0);
        let name = self.shared.som.lock_or_recover().classes.get(&cls).map(|c| c.name.clone()).unwrap_or_default();
        format!("{{An instance of class {name} at address {obj:08X}}}")
    }

    /// A SOM function or root-class method by module ordinal `ord`, with
    /// its arguments `a` (the receiver first for methods).
    pub(crate) fn som_call(&self, vcpu: u32, ord: u32, a: &[u32; 10]) -> ApiResult {
        self.som_init();
        let ok = ApiResult::Normal;
        let class_of = |obj: u32| self.som_class_of(obj).unwrap_or(0);
        let info = |cls: u32, f: &dyn Fn(&SomClass) -> u32| {
            self.shared.som.lock_or_recover().classes.get(&cls).map_or(0, f)
        };
        match ord {
            SOM_ENVIRONMENT_NEW => ok(self.shared.som.lock_or_recover().mgr),
            SOM_RESOLVE => ok(self.som_resolve(a[0], a[1])),
            SOM_RESOLVE_BY_NAME => {
                let name = self.som_name(a[1]).unwrap_or_default();
                let cls = class_of(a[0]);
                let rt = self.shared.som.lock_or_recover();
                ok(rt.token_of(cls, &name).and_then(|t| rt.class_method(cls, t)).unwrap_or(0))
            }
            SOM_CLASS_RESOLVE => ok(self.shared.som.lock_or_recover().class_method(a[0], a[1]).unwrap_or_else(|| thunk(SOM_METHOD_ERROR))),
            SOM_DATA_RESOLVE => ok(self.som_data_resolve(a[0], a[1])),
            SOM_GET_GLOBAL_ENVIRONMENT => ok(self.shared.som.lock_or_recover().env),
            SOM_ID_FROM_STRING => ok(self.som_name(a[0]).map_or(0, |n| self.som_id(&n))),
            SOM_STRING_FROM_ID => ok(if a[0] == 0 { 0 } else { self.guest_read::<u32>(a[0]).unwrap_or(0) }),
            SOM_COMPARE_IDS => ok((self.som_id_name(a[0]).is_some() && self.som_id_name(a[0]) == self.som_id_name(a[1])) as u32),
            SOM_IS_OBJ => ok(self.shared.som.lock_or_recover().instances.contains(&a[0]) as u32),
            SOM_PRINTF => self.som_printf(vcpu, a),
            SOM_MALLOC => ok(self.som_malloc(a[0]).unwrap_or(0)),
            SOM_CALLOC => ok(a[0].checked_mul(a[1]).and_then(|n| self.som_malloc(n)).unwrap_or(0)),
            SOM_REALLOC => ok(self.som_realloc(a[0], a[1])),
            SOM_FREE_MEM => { self.som_free_mem(a[0]); ok(0) }
            SOM_METACLASS_FOR => ok(self.som_metaclass_for(a[0], a[1])),
            SOM_DEFAULT_LOAD_MODULE => {
                // SOMLoadModule(className, fileName, functionName, major,
                // minor, modHandle *): 0 when loaded.
                let (Some(class), Some(file)) = (self.som_name(a[0]), self.som_name(a[1])) else {
                    return ok(super::constants::ERROR_INVALID_PARAMETER);
                };
                let func = if a[2] == 0 { None } else { self.som_name(a[2]) };
                match self.som_module_steps(&file, func.as_deref(), &class, a[0], a[3], a[4]) {
                    Ok((hmod, steps, inits)) => self.start_chain(CallChain::new(vcpu, steps, false,
                        ChainThen::SomModuleLoaded { hmod, phandle: a[5], inits })),
                    Err(e) => ok(e),
                }
            }
            SOM_DEFAULT_DELETE_MODULE => self.dos_free_module(a[0]),
            SOM_DEFAULT_ERROR => {
                // SOMError(code, fileName, lineNum): a message; a fatal
                // code ends the process.
                let file = self.som_name(a[1]).unwrap_or_default();
                let msg = format!("SOM error {} in file {file}, line {}\n", a[0] as i32, a[2] as i32);
                let _ = self.write_handle(super::constants::STDERR_HANDLE, msg.as_bytes());
                if a[0] % 10 == SOM_FATAL {
                    return self.dos_exit(vcpu, super::threads::EXIT_PROCESS, a[0]);
                }
                ok(0)
            }
            SOM_DEFAULT_OUT_CHAR => {
                let _ = self.write_handle(STDOUT_HANDLE, &[a[0] as u8]);
                ok(1)
            }
            SOM_SET_OUT_CHAR => {
                let routine = if a[0] == 0 { thunk(SOM_DEFAULT_OUT_CHAR) } else { a[0] };
                if let Some(at) = self.som_data_address(SOM_VAR_OUT_CHAR) { let _ = self.guest_write::<u32>(at, routine); }
                ok(0)
            }
            SOM_METHOD_ERROR => ok(0),
            // ── SOMObject ──
            100 | 101 | 111 => ok(0), // somInit / somUninit / somDumpSelfInt: nothing at the root
            102 => {
                // somFree: somUninit (the object's own), then the memory.
                if !self.shared.som.lock_or_recover().instances.contains(&a[0]) { return ok(0); }
                let uninit = self.som_resolve(a[0], TOKEN_SOM_UNINIT);
                self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(uninit, vec![a[0]], false)], false,
                                                ChainThen::SomFree { obj: a[0] }))
            }
            103 => ok(class_of(a[0])),
            104 => ok(info(class_of(a[0]), &|c| c.name_addr)),
            105 => ok(info(class_of(a[0]), &|c| c.instance_size)),
            106 => {
                // The class first: class_of takes the runtime's lock too.
                let cls = class_of(a[0]);
                ok(self.shared.som.lock_or_recover().descends(cls, a[1]) as u32)
            }
            107 => ok((class_of(a[0]) == a[1] && a[1] != 0) as u32),
            108 => {
                let name = self.som_id_name(a[1]).unwrap_or_default();
                let cls = class_of(a[0]);
                ok(self.shared.som.lock_or_recover().token_of(cls, &name).is_some() as u32)
            }
            109 => {
                let _ = self.write_handle(STDOUT_HANDLE, format!("{}\n", self.som_describe(a[0])).as_bytes());
                ok(a[0])
            }
            110 => {
                // somDumpSelf(level): a line, then the object's somDumpSelfInt.
                let indent = "  ".repeat((a[1] as i32).clamp(0, 32) as usize);
                let _ = self.write_handle(STDOUT_HANDLE, format!("{indent}{}\n", self.som_describe(a[0])).as_bytes());
                let int = self.som_resolve(a[0], TOKEN_SOM_DUMP_SELF_INT);
                self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(int, vec![a[0], a[1]], false)], false,
                                                ChainThen::Return(0)))
            }
            112 => {
                // somDefaultInit(ctrl): at the root, the object's somInit
                // (classes written for SOM 1.x initialize there).
                let init = self.som_resolve(a[0], TOKEN_SOM_INIT);
                self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(init, vec![a[0]], false)], false,
                                                ChainThen::Return(0)))
            }
            113 => {
                // somDestruct(doFree, ctrl): the object's somUninit, then
                // its memory when doFree (an octet) is set.
                if !self.shared.som.lock_or_recover().instances.contains(&a[0]) { return ok(0); }
                let uninit = self.som_resolve(a[0], TOKEN_SOM_UNINIT);
                let then = if a[1] & 0xFF != 0 { ChainThen::SomFree { obj: a[0] } } else { ChainThen::Return(0) };
                self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(uninit, vec![a[0]], false)], false, then))
            }
            // ── SOMClass (the receiver is a class object) ──
            200 | 201 => {
                let Some(obj) = self.som_new_instance(a[0]) else { return ok(0) };
                if ord == 201 { return ok(obj); }
                // somNew = somNewNoInit + the object's somDefaultInit; the
                // root's goes straight to somInit.
                let default_init = self.som_resolve(obj, TOKEN_SOM_DEFAULT_INIT);
                let step = if default_init == thunk(112) {
                    CallStep::hook(self.som_resolve(obj, TOKEN_SOM_INIT), vec![obj], false)
                } else {
                    CallStep::hook(default_init, vec![obj, 0], false)
                };
                self.start_chain(CallChain::new(vcpu, vec![step], false, ChainThen::Return(obj)))
            }
            202 => ok(info(a[0], &|c| c.name_addr)),
            203 => ok(info(a[0], &|c| c.parents.first().copied().unwrap_or(0))),
            204 => ok(info(a[0], &|c| c.instance_size)),
            205 => ok(info(a[0], &|c| c.data_size)),
            // The data token of a class is the class itself.
            206 => ok(info(a[0], &|c| if c.parts.is_empty() { 0 } else { a[0] })),
            207 => ok(info(a[0], &|c| c.slots.len() as u32)),
            208 => ok(info(a[0], &|c| c.own_methods as u32)),
            209 => ok(self.shared.som.lock_or_recover().descends(a[0], a[1]) as u32),
            210 => ok(self.som_init_class(a[0], a[1], &[a[2]], a[3])),
            211 => ok(self.som_add_method(a[0], a[1], a[3])),
            212 => ok(self.som_override(a[0], a[1], a[2])),
            213 => ok(self.som_class_ready(a[0])),
            214 | 215 => {
                let name = self.som_id_name(a[1]).unwrap_or_default();
                let token = self.shared.som.lock_or_recover().token_of(a[0], &name).unwrap_or(0);
                ok(if ord == 214 { token } else { (token != 0) as u32 })
            }
            216 => {
                // somInitMIClass(self, inherit_vars, className, parentClasses
                // (a sequence by value: _maximum, _length, _buffer),
                // dataSize, dataAlignment, maxStaticMethods, major, minor).
                let n = a[4] as usize;
                if n == 0 || n > MAX_PARENTS { return ok(0); }
                let parents: Option<Vec<u32>> = (0..n as u32).map(|i| self.guest_read::<u32>(a[5].checked_add(4 * i)?)).collect();
                let Some(parents) = parents else { return ok(0) };
                ok(self.som_init_class(a[0], a[2], &parents, a[6]))
            }
            // ── SOMClassMgr ──
            301 => {
                let name = self.som_id_name(a[1]).unwrap_or_default();
                ok(self.shared.som.lock_or_recover().by_name.get(&name).copied().unwrap_or(0))
            }
            300 => self.som_find_class(vcpu, a[1], a[2], a[3], None, false),
            303 => self.som_find_class(vcpu, a[1], a[2], a[3], Some(a[4]), false),
            // somLocateClassFile: without an interface repository, the
            // class's name.
            304 => ok(self.som_id_name(a[1]).map_or(0, |n| self.som_string(&n))),
            305 => self.som_find_class(vcpu, a[1], a[2], a[3], Some(a[4]), true),
            306 => ok(self.som_unregister(a[1])),
            307 => self.som_unload_class_file(vcpu, a[1]),
            302 => {
                let mut rt = self.shared.som.lock_or_recover();
                if let Some(name) = rt.classes.get(&a[1]).filter(|c| c.ready).map(|c| c.name.clone()) {
                    rt.by_name.insert(name, a[1]);
                }
                ok(0)
            }
            _ => ok(0),
        }
    }

    /// `somFree`'s chain called `somUninit`: the object's memory goes.
    pub(crate) fn som_free_done(&self, obj: u32) -> u32 {
        let known = {
            let mut rt = self.shared.som.lock_or_recover();
            let k = rt.instances.remove(&obj);
            rt.classes.remove(&obj);
            k
        };
        if known { self.som_free_mem(obj); }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(r: ApiResult) -> u32 {
        match r { ApiResult::Normal(v) => v, other => panic!("{other:?}") }
    }

    fn call(l: &Loader, ord: u32, args: &[u32]) -> ApiResult {
        let mut a = [0u32; 10];
        a[..args.len()].copy_from_slice(args);
        l.som_call(0, ord, &a)
    }

    /// Run a chain the way a guest would: each step "returns" 0.
    fn finish(l: &Loader, r: ApiResult) -> u32 {
        match r {
            ApiResult::CallChain(mut c) => {
                while c.steps.pop_front().is_some() { c.returned(0, false); }
                match l.finish_chain(&c) {
                    super::super::pm_hook::ChainEnd::Return(v) => v,
                    other => panic!("{other:?}"),
                }
            }
            ApiResult::Normal(v) => v,
            other => panic!("{other:?}"),
        }
    }

    fn put(l: &Loader, at: u32, s: &str) -> u32 {
        l.guest_write_bytes(at, format!("{s}\0").as_bytes()).unwrap();
        at
    }

    fn root(l: &Loader, ord: u32) -> u32 {
        l.guest_read::<u32>(l.som_data_address(ord).unwrap()).unwrap()
    }

    /// The token of root method `n` in its class's exported ClassData.
    fn root_token(l: &Loader, ord: u32, n: usize) -> u32 {
        l.guest_read::<u32>(l.som_data_address(ord).unwrap() + 4 + 4 * n as u32).unwrap()
    }

    /// A class built as a binding does: an instance of `meta`, its
    /// initialization, methods added or overridden, ready.
    fn new_class(l: &Loader, meta: u32, name: &str, parents: &[u32], size: u32, methods: &[(&str, u32)]) -> u32 {
        let cls = finish(l, call(l, 200, &[meta]));
        assert_ne!(cls, 0);
        let n = put(l, 0x3400, name);
        if parents.len() <= 1 {
            ok(call(l, 210, &[cls, n, parents.first().copied().unwrap_or(0), size, 0, 0, 0]));
        } else {
            for (i, p) in parents.iter().enumerate() { l.guest_write::<u32>(0x3800 + 4 * i as u32, *p).unwrap(); }
            let k = parents.len() as u32;
            ok(call(l, 216, &[cls, 0, n, k, k, 0x3800, size, 4, 0, 0]));
        }
        for &(m, p) in methods {
            let id = ok(call(l, SOM_ID_FROM_STRING, &[put(l, 0x3900, m)]));
            if ok(call(l, 214, &[cls, id])) != 0 {
                ok(call(l, 212, &[cls, id, p]));
            } else {
                assert_ne!(ok(call(l, 211, &[cls, id, 0, p, 0, 0])), 0);
            }
        }
        ok(call(l, 213, &[cls]));
        cls
    }

    fn token(l: &Loader, cls: u32, name: &str) -> u32 {
        let id = ok(call(l, SOM_ID_FROM_STRING, &[put(l, 0x3900, name)]));
        ok(call(l, 214, &[cls, id]))
    }

    #[test]
    fn the_root_classes_and_their_exported_data() {
        let l = Loader::new_mock();
        let som_object = root(&l, SOM_OBJECT_CLASS_DATA);
        assert_ne!(som_object, 0);
        assert_eq!(root_token(&l, SOM_OBJECT_CLASS_DATA, 0), TOKEN_SOM_INIT);
        assert_eq!(root_token(&l, SOM_OBJECT_CLASS_DATA, 1), TOKEN_SOM_UNINIT);
        assert_eq!(root_token(&l, SOM_OBJECT_CLASS_DATA, 11), TOKEN_SOM_DUMP_SELF_INT);
        let mgr = root(&l, SOM_CLASS_MGR_OBJECT);
        assert_eq!(ok(call(&l, SOM_ENVIRONMENT_NEW, &[])), mgr);
        // The manager is an instance of SOMClassMgr, itself a SOMObject.
        let mgr_class = ok(call(&l, 103, &[mgr]));
        assert_eq!(l.read_guest_string(ok(call(&l, 104, &[mgr]))), "SOMClassMgr");
        assert_eq!(ok(call(&l, 106, &[mgr, som_object])), 1, "somIsA SOMObject");
        assert_eq!(ok(call(&l, 203, &[mgr_class])), som_object, "its parent");
        // somResolve answers from the class's methods.
        let get_class = root_token(&l, SOM_OBJECT_CLASS_DATA, 3);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[mgr, get_class])), thunk(103), "somGetClass");
        let find = root_token(&l, SOM_CLASS_MGR_CLASS_DATA, 0);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[mgr, find])), thunk(300), "somFindClass");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[som_object, find])), thunk(SOM_METHOD_ERROR), "not a SOMClassMgr's");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[mgr, 99_999])), thunk(SOM_METHOD_ERROR));
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[0x1234_0000, 1])), thunk(SOM_METHOD_ERROR), "not an object");
        // The runtime variables hold the allocation procedures.
        assert_eq!(root(&l, SOM_VAR_MALLOC), thunk(SOM_MALLOC));
        assert_eq!(root(&l, SOM_VAR_FREE), thunk(SOM_FREE_MEM));
        assert_eq!(root(&l, SOM_TRACE_LEVEL), 0);
        let span = l.som_data_address(SOM_LAST_DATA).unwrap() + 4 - l.som_data_address(SOM_OBJECT_CLASS_DATA).unwrap();
        assert_eq!(span, data_size());
    }

    /// A class built as a binding does: a new class object from SOMClass,
    /// somInitClass, a static method, an override, somClassReady; then an
    /// instance, its data, a parent call and somFree.
    #[test]
    fn a_class_is_built_and_used() {
        let l = Loader::new_mock();
        let som_class = root(&l, SOM_CLASS_CLASS_DATA);
        let som_object = root(&l, SOM_OBJECT_CLASS_DATA);
        let cls = new_class(&l, som_class, "Hello", &[som_object], 8, &[("sayHello", 0x0060_0000), ("somInit", 0x0060_1000)]);
        let tok = token(&l, cls, "sayHello");
        assert_eq!(tok as usize, SOMOBJECT_METHODS.len() + SOMCLASS_METHODS.len() + SOMCLASSMGR_METHODS.len() + 1);
        assert_eq!(ok(call(&l, 204, &[cls])), 12, "4 + its 8 bytes");
        assert_eq!(ok(call(&l, 206, &[cls])), cls, "its data token");
        // An instance: somNew calls its somInit (the override).
        let r = call(&l, 200, &[cls]);
        let ApiResult::CallChain(ref c) = r else { panic!() };
        assert!(matches!(c.steps.front(), Some(CallStep::Guest { proc_addr: 0x0060_1000, .. })));
        let obj = finish(&l, r);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[obj, tok])), 0x0060_0000);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, cls])), obj + 4);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, som_class])), 0, "not its ancestor");
        assert_eq!(ok(call(&l, SOM_CLASS_RESOLVE, &[som_object, TOKEN_SOM_INIT])), thunk(100), "the parent's somInit");
        assert_eq!(l.read_guest_string(ok(call(&l, 104, &[obj]))), "Hello");
        let id = ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3100, "sayHello")]));
        assert_eq!(ok(call(&l, SOM_ID_FROM_STRING, &[0x3100])), id, "one id per string");
        assert_eq!(ok(call(&l, 108, &[obj, id])), 1, "somRespondsTo sayHello");
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 1);
        // somFree: somUninit, then the memory.
        assert_eq!(finish(&l, call(&l, 102, &[obj])), 0);
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 0);
        // A ready class takes no more methods; the manager finds it.
        assert_eq!(ok(call(&l, 211, &[cls, id, 0, 0x0060_2000, 0, 0])), 0);
        let mgr = ok(call(&l, SOM_ENVIRONMENT_NEW, &[]));
        assert_eq!(ok(call(&l, 300, &[mgr, ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3200, "Hello")])), 0, 0])), cls);
    }

    /// The SOM 2.1 initializers the C++ bindings use: somNew calls the
    /// object's somDefaultInit (straight to somInit at the root),
    /// somDefaultInit calls somInit, somDestruct calls somUninit and frees
    /// only when asked.
    #[test]
    fn default_init_and_destruct() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        assert_eq!(root_token(&l, SOM_OBJECT_CLASS_DATA, 12), TOKEN_SOM_DEFAULT_INIT);
        let first = |r: &ApiResult| match r {
            ApiResult::CallChain(c) => match c.steps.front() {
                Some(CallStep::Guest { proc_addr, args, .. }) => (*proc_addr, args.clone()),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        };
        // Only somInit overridden: somNew goes to it.
        let plain = new_class(&l, som_class, "Plain", &[som_object], 0, &[("somInit", 0x0061_0000), ("somUninit", 0x0061_1000)]);
        let r = call(&l, 200, &[plain]);
        assert_eq!(first(&r).0, 0x0061_0000);
        let obj = finish(&l, r);
        // somDefaultInit at the root: the object's somInit.
        let r = call(&l, 112, &[obj, 0]);
        assert_eq!(first(&r), (0x0061_0000, vec![obj]));
        assert_eq!(finish(&l, r), 0);
        // somDestruct without doFree: somUninit, the object stays.
        let r = call(&l, 113, &[obj, 0, 0]);
        assert_eq!(first(&r), (0x0061_1000, vec![obj]));
        finish(&l, r);
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 1);
        // With doFree (only its low byte counts): the memory goes too.
        finish(&l, call(&l, 113, &[obj, 0x0100, 0]));
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 1, "doFree is an octet");
        finish(&l, call(&l, 113, &[obj, 1, 0]));
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 0);
        assert_eq!(ok(call(&l, 113, &[obj, 1, 0])), 0, "not twice");
        // somDefaultInit overridden: somNew calls it with a null control.
        let modern = new_class(&l, som_class, "Modern", &[som_object], 0, &[("somDefaultInit", 0x0062_0000)]);
        let r = call(&l, 200, &[modern]);
        let (proc, args) = first(&r);
        assert_eq!(proc, 0x0062_0000);
        assert_eq!(args[1], 0);
        let obj = finish(&l, r);
        assert_eq!(args[0], obj);
    }

    /// The replaceable runtime variables: somPrintf through a program's
    /// SOMOutCharRoutine, somSetOutChar, SOMError's default, and their
    /// initial procedures.
    #[test]
    fn output_and_error_routines() {
        let l = Loader::new_mock();
        for (var, f) in [(SOM_VAR_OUT_CHAR, SOM_DEFAULT_OUT_CHAR), (SOM_VAR_LOAD_MODULE, SOM_DEFAULT_LOAD_MODULE),
                         (SOM_VAR_DELETE_MODULE, SOM_DEFAULT_DELETE_MODULE), (SOM_VAR_ERROR, SOM_DEFAULT_ERROR)] {
            assert_eq!(root(&l, var), thunk(f));
        }
        // The default routine writes directly.
        let fmt = put(&l, 0x3400, "n=%d\n");
        assert_eq!(ok(call(&l, SOM_PRINTF, &[fmt, 42])), 5);
        // A program's routine gets every character, in order.
        assert_eq!(ok(call(&l, SOM_SET_OUT_CHAR, &[0x0063_0000])), 0);
        assert_eq!(root(&l, SOM_VAR_OUT_CHAR), 0x0063_0000);
        let r = call(&l, SOM_PRINTF, &[fmt, 42]);
        let ApiResult::CallChain(ref c) = r else { panic!("{r:?}") };
        let chars: Vec<u32> = c.steps.iter().map(|s| match s {
            CallStep::Guest { proc_addr: 0x0063_0000, args, .. } => args[0],
            other => panic!("{other:?}"),
        }).collect();
        assert_eq!(chars, b"n=42\n".iter().map(|&b| b as u32).collect::<Vec<_>>());
        assert_eq!(finish(&l, r), 5);
        // somSetOutChar(0) puts the default back.
        ok(call(&l, SOM_SET_OUT_CHAR, &[0]));
        assert_eq!(root(&l, SOM_VAR_OUT_CHAR), thunk(SOM_DEFAULT_OUT_CHAR));
        assert_eq!(ok(call(&l, SOM_DEFAULT_OUT_CHAR, &[b'x' as u32])), 1);
        // SOMError: a warning returns, a fatal code (last digit 9) ends
        // the process with it.
        let file = put(&l, 0x3500, "x.c");
        assert_eq!(ok(call(&l, SOM_DEFAULT_ERROR, &[20012, file, 7])), 0);
        assert!(matches!(call(&l, SOM_DEFAULT_ERROR, &[20019, file, 7]), ApiResult::ExitProcess(20019)));
    }

    /// Class loading: a registered class is found at once; an unknown one
    /// is looked for in a file named after it; a program's SOMLoadModule
    /// is called with the class, the file, "SOMInitModule", the versions
    /// and a handle cell, and the class it made is returned.
    #[test]
    fn classes_are_found_and_loaded() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        let mgr = ok(call(&l, SOM_ENVIRONMENT_NEW, &[]));
        let id = |n: &str, at: u32| ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, at, n)]));
        let known = new_class(&l, som_class, "Known", &[som_object], 0, &[]);
        assert_eq!(finish(&l, call(&l, 300, &[mgr, id("Known", 0x3600), 0, 0])), known);
        // somLocateClassFile: the class's name.
        let missing = id("Nowhere", 0x3620);
        assert_eq!(l.read_guest_string(ok(call(&l, 304, &[mgr, missing, 0, 0]))), "Nowhere");
        // No such file: no class, nothing leaked into a chain.
        assert_eq!(ok(call(&l, 300, &[mgr, missing, 0, 0])), 0);
        let file = put(&l, 0x3640, "NOFILE");
        assert_eq!(ok(call(&l, 303, &[mgr, missing, 0, 0, file])), 0);
        // A program's SOMLoadModule.
        let var = l.som_data_address(SOM_VAR_LOAD_MODULE).unwrap();
        l.guest_write::<u32>(var, 0x0064_0000).unwrap();
        let lazy = id("Lazy", 0x3660);
        let r = call(&l, 303, &[mgr, lazy, 1, 2, file]);
        let ApiResult::CallChain(ref c) = r else { panic!("{r:?}") };
        let Some(CallStep::Guest { proc_addr: 0x0064_0000, args, .. }) = c.steps.front() else { panic!() };
        let args = args.clone();
        assert_eq!(l.read_guest_string(args[0]), "Lazy");
        assert_eq!(l.read_guest_string(args[1]), "NOFILE");
        assert_eq!(l.read_guest_string(args[2]), SOM_INIT_MODULE);
        assert_eq!(&args[3..5], &[1, 2]);
        assert_ne!(args[5], 0, "a handle cell");
        // It "loaded" without making the class: no class.
        assert_eq!(finish(&l, r), 0);
        // It made the class (as SOMInitModule would): that class.
        let r = call(&l, 305, &[mgr, lazy, 0, 0, file]);
        let made = new_class(&l, som_class, "Lazy", &[som_object], 0, &[]);
        assert_eq!(finish(&l, r), made);
        // A failing SOMLoadModule (non-zero): no class, though one exists.
        let r = call(&l, 305, &[mgr, lazy, 0, 0, file]);
        let ApiResult::CallChain(mut c) = r else { panic!() };
        c.steps.pop_front();
        c.returned(1, false);
        assert!(matches!(l.finish_chain(&c), super::super::pm_hook::ChainEnd::Return(0)));
        // The default SOMLoadModule without such a file.
        let (cls_name, file_name) = (put(&l, 0x3680, "Lazy"), put(&l, 0x36A0, "NOFILE"));
        assert_eq!(ok(call(&l, SOM_DEFAULT_LOAD_MODULE, &[cls_name, file_name, 0, 0, 0, 0x36C0])),
                   crate::loader::constants::ERROR_MOD_NOT_FOUND);
        assert!(som_inits_ok(&[1, 5, 0], 2));
        assert!(!som_inits_ok(&[1, 0, 7], 2));
    }

    /// somUnregisterClass forgets a class; somUnloadClassFile forgets the
    /// classes of the module that made one, withdraws their method tables
    /// (their objects resolve nothing) and frees the module.
    #[test]
    fn classes_are_unregistered_and_unloaded() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        let mgr = ok(call(&l, SOM_ENVIRONMENT_NEW, &[]));
        let a = new_class(&l, som_class, "ModA", &[som_object], 0, &[("work", 0x0065_0000)]);
        let b = new_class(&l, som_class, "ModB", &[som_object], 0, &[]);
        assert_eq!(ok(call(&l, 306, &[mgr, b])), 0);
        assert_eq!(ok(call(&l, 306, &[mgr, b])), 1, "not registered any more");
        assert_eq!(ok(call(&l, 307, &[mgr, a])), 1, "no module made it");
        // As if a module 0x1234 had made both.
        l.shared.som.lock_or_recover().modules.extend([(a, 0x1234), (b, 0x1234)]);
        let obj = finish(&l, call(&l, 200, &[a]));
        let work = token(&l, a, "work");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[obj, work])), 0x0065_0000);
        // The default SOMDeleteModule: DosFreeModule of an unknown handle.
        assert_ne!(ok(call(&l, 307, &[mgr, a])), 0);
        let rt = l.shared.som.lock_or_recover();
        assert!(!rt.by_name.contains_key("ModA") && rt.modules.is_empty());
        drop(rt);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[obj, work])), thunk(SOM_METHOD_ERROR), "its code is gone");
        assert_eq!(ok(call(&l, 300, &[mgr, ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3700, "ModA")])), 0, 0])), 0,
                   "looked for again (no file here)");
    }

    /// Two parents: methods of both, the right one's override where the
    /// left one kept the introducer's, each ancestor's data once.
    #[test]
    fn multiple_inheritance_merges_methods_and_data() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        let a = new_class(&l, som_class, "A", &[som_object], 4, &[("fa", 0xA000), ("shared", 0xA100)]);
        let b = new_class(&l, som_class, "B", &[a], 8, &[("fb", 0xB000)]);
        let c = new_class(&l, som_class, "C", &[a], 12, &[("fc", 0xC000), ("shared", 0xC100), ("somInit", 0xC200)]);
        let d = new_class(&l, som_class, "D", &[b, c], 0, &[]);
        for (m, p) in [("fa", 0xA000), ("fb", 0xB000), ("fc", 0xC000), ("shared", 0xC100), ("somInit", 0xC200)] {
            assert_eq!(ok(call(&l, SOM_CLASS_RESOLVE, &[d, token(&l, d, m)])), p, "{m}");
        }
        assert_eq!(token(&l, d, "shared"), token(&l, a, "shared"), "one token, introduced by A");
        // D: SOMObject 0..4, A 4..8, B 8..16, C 16..28.
        assert_eq!(ok(call(&l, 204, &[d])), 28);
        let obj = finish(&l, call(&l, 200, &[d]));
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, a])), obj + 4);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, b])), obj + 8);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, c])), obj + 16);
        assert_eq!(ok(call(&l, 106, &[obj, c])), 1, "somIsA its right parent");
        assert_eq!(ok(call(&l, 209, &[d, b])), 1);
        assert_eq!(ok(call(&l, 203, &[d])), b, "somGetParent: the first");
    }

    /// A metaclass with data and a somInit override; a class whose
    /// requested metaclass is unrelated to its parent's gets a derived one.
    #[test]
    fn metaclasses_and_derived_metaclasses() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        let m_animal = new_class(&l, som_class, "M_Animal", &[som_class], 12, &[("newAnimal", 0x7000), ("somInit", 0x7100)]);
        let kennel = new_class(&l, som_class, "Kennel", &[som_class], 4, &[("newDog", 0x8000), ("somNew", 0x8100)]);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[m_animal, som_object])), m_animal);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[0, som_object])), som_class, "the parent's");
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[som_object, som_object])), 0, "not a metaclass");
        // somNew of a metaclass runs the metaclass's somInit on the new class.
        let r = call(&l, 200, &[m_animal]);
        let ApiResult::CallChain(ref c) = r else { panic!() };
        assert!(matches!(c.steps.front(), Some(CallStep::Guest { proc_addr: 0x7100, .. })));
        drop(r);
        // A class object of M_Animal carries M_Animal's data.
        let animal = new_class(&l, m_animal, "Animal", &[som_object], 8, &[("talk", 0x9000)]);
        assert_eq!(ok(call(&l, 103, &[animal])), m_animal);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[animal, m_animal])), animal + 4);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[0, animal])), m_animal, "inherited");
        // Dog asks for Kennel: Kennel + M_Animal, made once.
        let derived = ok(call(&l, SOM_METACLASS_FOR, &[kennel, animal]));
        assert!(derived != 0 && derived != kennel && derived != m_animal);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[kennel, animal])), derived);
        assert_eq!(l.read_guest_string(ok(call(&l, 202, &[derived]))), "Kennel+M_Animal");
        let dog = new_class(&l, derived, "Dog", &[animal], 8, &[]);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[dog, token(&l, kennel, "newDog")])), 0x8000);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[dog, token(&l, m_animal, "newAnimal")])), 0x7000);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[dog, TOKEN_SOM_INIT])), 0x7100, "M_Animal's somInit");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[dog, token(&l, som_class, "somNew")])), 0x8100, "Kennel's somNew");
        // Kennel's data, then M_Animal's, in the Dog class object.
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[dog, kennel])), dog + 4);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[dog, m_animal])), dog + 8);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[0, dog])), derived);
        // A Dog is an Animal; its class descends from both metaclasses.
        let rex = finish(&l, call(&l, 201, &[dog]));
        assert_eq!(ok(call(&l, 106, &[rex, animal])), 1);
        assert_eq!(ok(call(&l, 106, &[dog, m_animal])), 1);
        assert_eq!(ok(call(&l, 106, &[dog, kennel])), 1);
    }

    #[test]
    fn the_heap_allocates_frees_and_reallocates() {
        let l = Loader::new_mock();
        let a = ok(call(&l, SOM_MALLOC, &[20]));
        let b = ok(call(&l, SOM_MALLOC, &[20]));
        assert!(a != 0 && b == a + 32, "16-byte rounding in one arena");
        l.guest_write_bytes(a, b"abcdef").unwrap();
        let r = ok(call(&l, SOM_REALLOC, &[a, 100]));
        assert_eq!(l.guest_read_bytes(r, 6).unwrap(), b"abcdef");
        assert_eq!(l.shared.som.lock_or_recover().heap.blocks.get(&a), None, "the old block is gone");
        assert_eq!(ok(call(&l, SOM_MALLOC, &[20])), a, "and reused");
        let z = ok(call(&l, SOM_CALLOC, &[4, 8]));
        assert_eq!(l.guest_read_bytes(z, 32).unwrap(), vec![0; 32]);
        assert_eq!(ok(call(&l, SOM_CALLOC, &[u32::MAX, 2])), 0, "overflow");
        assert_eq!(ok(call(&l, SOM_MALLOC, &[MAX_ALLOC + 1])), 0);
        let big = ok(call(&l, SOM_MALLOC, &[100_000]));
        assert_eq!(big % 4096, 0, "pages of its own");
        ok(call(&l, SOM_FREE_MEM, &[big]));
        ok(call(&l, SOM_FREE_MEM, &[0x1234_5678]));
        assert_eq!(ok(call(&l, SOM_REALLOC, &[0x1234_5678, 8])), 0, "not a block");
        assert_eq!(ok(call(&l, SOM_REALLOC, &[r, 0])), 0, "freed");
        assert_eq!(l.shared.som.lock_or_recover().heap.blocks.get(&r), None);
    }

    #[test]
    fn c_format_follows_printf() {
        let strs = |p: u32| -> Vec<u8> { if p == 1 { b"dog".to_vec() } else { b"cat".to_vec() } };
        let f = |fmt: &str, args: &[u32]| String::from_utf8(c_format(fmt.as_bytes(), &mut args.iter().copied(), &strs)).unwrap();
        assert_eq!(f("\tThe %s says %d%%\n", &[1, 42]), "\tThe dog says 42%\n");
        assert_eq!(f("[%5d|%-5d|%05d|%+d|% d]", &[7, 7, 7, 7, 7]), "[    7|7    |00007|+7| 7]");
        assert_eq!(f("%d %u %x %X %#x %o %c", &[(-3i32) as u32, (-1i32) as u32, 255, 255, 255, 8, b'A' as u32]),
                   "-3 4294967295 ff FF 0xff 10 A");
        assert_eq!(f("%.3d|%.2s|%*d|%-*d|%s", &[5, 2, 4, 9, 3, 1, 0]), "005|ca|   9|1  |(null)");
        assert_eq!(f("%hd %lld %p", &[0x1_FFFF, 1, 1, 0xABCD]), "-1 4294967297 0000ABCD");
        let d = 3.25f64.to_bits();
        let (lo, hi) = (d as u32, (d >> 32) as u32);
        assert_eq!(f("%f %.1f %e %g %G", &[lo, hi, lo, hi, lo, hi, lo, hi, lo, hi]),
                   "3.250000 3.2 3.250000e+00 3.25 3.25");
        let big = 1.5e20f64.to_bits();
        assert_eq!(f("%g", &[big as u32, (big >> 32) as u32]), "1.5e+20");
        assert_eq!(f("%y|100%", &[]), "%y|100%", "unknown conversions are kept");
        assert_eq!(f("%d", &[]), "0", "missing arguments are 0");
        assert!(c_format(b"%4096d%4096d", &mut std::iter::empty(), &strs).len() <= MAX_PRINT);
    }

    #[test]
    fn bad_input_is_refused() {
        let l = Loader::new_mock();
        let som_class = root(&l, SOM_CLASS_CLASS_DATA);
        let cls = finish(&l, call(&l, 200, &[som_class]));
        // Instance data too large; an unready parent; no name.
        assert_eq!(ok(call(&l, 210, &[cls, put(&l, 0x3000, "Big"), 0, MAX_INSTANCE_DATA + 1, 0, 0, 0])), 0);
        let other = finish(&l, call(&l, 200, &[som_class]));
        ok(call(&l, 210, &[cls, put(&l, 0x3000, "Kid"), other, 4, 0, 0, 0]));
        assert_eq!(ok(call(&l, 213, &[cls])), 0);
        assert_eq!(ok(call(&l, 200, &[cls])), 0, "not ready: no instances");
        // Too many parents, or none, for somInitMIClass.
        assert_eq!(ok(call(&l, 216, &[cls, 0, 0x3000, 99, 99, 0x3800, 0, 4, 0, 0])), 0);
        assert_eq!(ok(call(&l, 216, &[cls, 0, 0x3000, 0, 0, 0x3800, 0, 4, 0, 0])), 0);
        // somFree of something that is not an object frees nothing.
        assert_eq!(ok(call(&l, 102, &[0x3000])), 0);
        assert_eq!(l.som_data_address(55), None);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[0x3000, som_class])), 0, "not an object");
    }
}
