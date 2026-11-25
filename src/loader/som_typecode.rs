// SPDX-License-Identifier: GPL-3.0-or-later
//
//! SOM TypeCodes (ADR-0128): `TypeCodeNew`, `TypeCode_alignment`,
//! `_copy`, `_equal`, `_free`, `_kind`, `_param_count`, `_parameter`,
//! `_print`, `_setAlignment` and `_size` (SOM Programming Reference,
//! "TypeCode functions"), and the `TC_*` constants.
//!
//! - **Records.** A TypeCode is a `SOMMalloc` block in the guest (Warpine's
//!   own layout: kind, parameter count, one word per parameter, then the
//!   strings it owns), so `TypeCode_parameter` can point an `any` at a
//!   parameter. Every TypeCode is also kept here ([`TypeCodeRec`]), and
//!   every function answers from these records: a guest that overwrites a
//!   block cannot steer them, and an address that is no TypeCode is never
//!   read.
//! - **Ownership.** `TypeCodeNew` copies its strings and takes the
//!   TypeCodes it is given (the Reference's example builds a struct from
//!   nested `TypeCodeNew` calls and frees only the outer one); one already
//!   taken, or given twice, is copied instead, and a constant is shared.
//!   `TypeCode_free` frees a TypeCode with those it took; freeing one
//!   another TypeCode holds, or a constant, does nothing.
//! - **Sizes** are those of the C types of the SDK's compilers (i386):
//!   `double` aligned to 4, `any` 8 bytes, a sequence 12, an enum 4.
//! - **Bounds.** Records nest at most [`MAX_TC_DEPTH`] deep, a struct,
//!   union or enum has at most [`MAX_TC_MEMBERS`] members, and a process
//!   holds at most [`MAX_TYPECODES`] TypeCodes.

use std::collections::HashMap;

use super::som_dispatch::tk;
use super::{ApiResult, Loader, MutexExt};

/// Warpine's own ordinals for the TypeCode functions (ADR-0123: no public
/// source gives IBM's).
pub const TC_NEW: u32 = 3109;
pub const TC_ALIGNMENT: u32 = 3110;
pub const TC_COPY: u32 = 3111;
pub const TC_EQUAL: u32 = 3112;
pub const TC_FREE: u32 = 3113;
pub const TC_KIND: u32 = 3114;
pub const TC_PARAM_COUNT: u32 = 3115;
/// `WrpTypeCode_parameter(result, tc, ev, index)`: `TypeCode_parameter`
/// with its `any` result stored at `result` (the SDK's inline
/// `TypeCode_parameter` calls it, so no compiler's struct-return
/// convention is involved).
pub const TC_PARAMETER: u32 = 3116;
pub const TC_PRINT: u32 = 3117;
pub const TC_SET_ALIGNMENT: u32 = 3118;
pub const TC_SIZE: u32 = 3119;
/// `WrpTypeCodeConstant(kind)`: the `TC_*` constant of a kind.
pub const TC_CONSTANT: u32 = 3120;

/// A union member's flag in `TypeCodeNew`: a labelled case, or the
/// default one (SOM Programming Reference).
pub const TCREGULAR_CASE: u32 = 1;
pub const TCDEFAULT_CASE: u32 = 2;

/// The exception `TypeCode_parameter` raises for an index out of range:
/// CORBA's `TypeCode::Bounds`, by the name C's mapping gives it
/// (`ex_TypeCode_Bounds`; Warpine's choice of spelling).
pub const EX_TYPECODE_BOUNDS: &str = "::TypeCode::Bounds";
/// `USER_EXCEPTION` (`exception_type`).
const USER_EXCEPTION: u32 = 1;

/// The deepest nesting of TypeCodes.
pub const MAX_TC_DEPTH: u32 = 32;
/// The most members of one struct, union or enum.
pub const MAX_TC_MEMBERS: usize = 1024;
/// The most TypeCodes a process holds.
pub const MAX_TYPECODES: usize = 65536;
/// The longest string a TypeCode keeps.
pub const MAX_TC_STRING: usize = 4096;
/// `TypeCode_print`'s longest text.
const MAX_TC_TEXT: usize = super::som::MAX_PRINT;

/// A block's header: the kind and the parameter count, then one word per
/// parameter.
const BLOCK_HEADER: u32 = 8;
/// The id of `TC_Object` (Warpine's choice).
const OBJECT_ID: &str = "::SOMObject";

/// One parameter of a TypeCode (SOM Programming Reference, Table 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TcParam {
    Str(Vec<u8>),
    Tc(u32),
    Long(i32),
}

impl TcParam {
    /// The kind of the `TC_*` constant an `any` names it by.
    fn kind(&self) -> u32 {
        match self {
            TcParam::Str(_) => tk::STRING,
            TcParam::Tc(_) => tk::TYPECODE,
            TcParam::Long(_) => tk::LONG,
        }
    }
}

/// A TypeCode as the kernel keeps it.
#[derive(Debug, Clone)]
pub struct TypeCodeRec {
    pub kind: u32,
    pub params: Vec<TcParam>,
    /// A union's members' flags ([`TCREGULAR_CASE`], [`TCDEFAULT_CASE`]),
    /// which are not parameters.
    pub flags: Vec<u32>,
    /// What `TypeCode_setAlignment` set.
    pub alignment: Option<u32>,
    /// A `TC_*` constant: never freed nor changed.
    pub constant: bool,
    /// The TypeCode that took this one.
    pub owner: Option<u32>,
    /// Where its first parameter word lies.
    pub values: u32,
}

/// The TypeCodes of a process, with the constants and the guest copy of
/// [`EX_TYPECODE_BOUNDS`].
#[derive(Debug, Default)]
pub struct TypeCodes {
    pub recs: HashMap<u32, TypeCodeRec>,
    pub constants: HashMap<u32, u32>,
    pub bounds_name: u32,
}

/// What a TypeCode's parameters will be, before its strings are read.
enum Arg { Str(u32), Tc(u32), Long(u32) }

/// The parameters `TypeCodeNew(kind, …)` takes, read word by word from
/// `word(i)` (`i` from 1, after the kind), and a union's flags; `None` for
/// a call that is not well formed.
fn parse_new(kind: u32, word: &dyn Fn(u32) -> Option<u32>) -> Option<(Vec<Arg>, Vec<u32>)> {
    let mut i = 1;
    let mut next = || { let w = word(i); i += 1; w };
    let mut args = Vec::new();
    let mut flags = Vec::new();
    let count = |v: u32| ((v as i32) >= 0).then_some(Arg::Long(v));
    match kind {
        tk::NULL..=tk::PRINCIPAL => {}
        tk::OBJREF | tk::SELF => args.push(Arg::Str(next()?)),
        tk::STRING => args.push(count(next()?)?),
        tk::SEQUENCE | tk::ARRAY => { args.push(Arg::Tc(next()?)); args.push(count(next()?)?); }
        tk::POINTER => args.push(Arg::Tc(next()?)),
        tk::FOREIGN => { args.push(Arg::Str(next()?)); args.push(Arg::Str(next()?)); args.push(count(next()?)?); }
        tk::STRUCT => {
            args.push(Arg::Str(next()?));
            loop {
                let name = next()?;
                if name == 0 { break; }
                if args.len() / 2 >= MAX_TC_MEMBERS { return None; }
                args.push(Arg::Str(name));
                args.push(Arg::Tc(next()?));
            }
        }
        tk::UNION => {
            args.push(Arg::Str(next()?));
            args.push(Arg::Tc(next()?));
            loop {
                let flag = next()?;
                if flag == 0 { break; }
                if flag != TCREGULAR_CASE && flag != TCDEFAULT_CASE { return None; }
                if flags.len() >= MAX_TC_MEMBERS { return None; }
                flags.push(flag);
                args.push(Arg::Long(next()?));
                args.push(Arg::Str(next()?));
                args.push(Arg::Tc(next()?));
            }
        }
        tk::ENUM => {
            args.push(Arg::Str(next()?));
            loop {
                let id = next()?;
                if id == 0 { break; }
                if args.len() > MAX_TC_MEMBERS { return None; }
                args.push(Arg::Str(id));
            }
        }
        _ => return None,
    }
    Some((args, flags))
}

/// How deep TypeCode `tc` nests (1 for one without nested TypeCodes);
/// `None` for one that is no TypeCode or nests deeper than the limit.
fn depth(recs: &HashMap<u32, TypeCodeRec>, tc: u32, level: u32) -> Option<u32> {
    if level > MAX_TC_DEPTH { return None; }
    let rec = recs.get(&tc)?;
    let mut deepest = 0;
    for p in &rec.params {
        if let TcParam::Tc(c) = p { deepest = deepest.max(depth(recs, *c, level + 1)?); }
    }
    Some(deepest + 1)
}

fn round_up(v: u32, align: u32) -> Option<u32> {
    v.checked_add(align - 1).map(|x| x / align * align)
}

/// The size and alignment of an instance of TypeCode `tc`'s type (the
/// alignment `TypeCode_setAlignment` set, else its own); `None` for no
/// TypeCode, or a size beyond 32 bits.
pub fn layout(recs: &HashMap<u32, TypeCodeRec>, tc: u32, level: u32) -> Option<(u32, u32)> {
    if level > MAX_TC_DEPTH { return None; }
    let rec = recs.get(&tc)?;
    let child = |i: usize| match rec.params.get(i) {
        Some(TcParam::Tc(c)) => layout(recs, *c, level + 1),
        _ => None,
    };
    let long = |i: usize| match rec.params.get(i) { Some(TcParam::Long(v)) => *v as u32, _ => 0 };
    let (size, align) = match rec.kind {
        tk::NULL | tk::VOID => (0, 1),
        tk::SHORT | tk::USHORT => (2, 2),
        tk::BOOLEAN | tk::CHAR | tk::OCTET => (1, 1),
        // double: 4-aligned, as the SDK's i386 compilers lay it out.
        tk::DOUBLE => (8, 4),
        tk::ANY => (8, 4),
        tk::SEQUENCE => (12, 4),
        tk::ARRAY => {
            let (s, a) = child(0)?;
            (s.checked_mul(long(1))?, a)
        }
        tk::STRUCT => {
            let (mut offset, mut align) = (0u32, 1u32);
            for m in (2..rec.params.len()).step_by(2) {
                let (s, a) = child(m)?;
                offset = round_up(offset, a)?.checked_add(s)?;
                align = align.max(a);
            }
            (round_up(offset, align)?, align)
        }
        tk::UNION => {
            let (ds, da) = child(1)?;
            let (mut biggest, mut ua) = (0u32, 1u32);
            for m in (4..rec.params.len()).step_by(3) {
                let (s, a) = child(m)?;
                biggest = biggest.max(s);
                ua = ua.max(a);
            }
            let align = da.max(ua);
            (round_up(round_up(ds, ua)?.checked_add(biggest)?, align)?, align)
        }
        tk::FOREIGN => {
            let s = long(2);
            (s, if s >= 4 { 4 } else if s >= 2 { 2 } else { 1 })
        }
        // long, ulong, float, TypeCode, Principal, objref, enum, string,
        // pointer, self: a word.
        _ => (4, 4),
    };
    Some((size, rec.alignment.unwrap_or(align)))
}

/// Whether TypeCodes `a` and `b` describe the same type with the same
/// alignment.
pub fn equal(recs: &HashMap<u32, TypeCodeRec>, a: u32, b: u32, level: u32) -> bool {
    if level > MAX_TC_DEPTH { return false; }
    let (Some(ra), Some(rb)) = (recs.get(&a), recs.get(&b)) else { return false };
    if ra.kind != rb.kind || ra.flags != rb.flags || ra.params.len() != rb.params.len() { return false; }
    if layout(recs, a, level).map(|l| l.1) != layout(recs, b, level).map(|l| l.1) { return false; }
    ra.params.iter().zip(&rb.params).all(|(x, y)| match (x, y) {
        (TcParam::Tc(x), TcParam::Tc(y)) => equal(recs, *x, *y, level + 1),
        _ => x == y,
    })
}

/// A string as C source writes it.
fn c_string(s: &[u8], out: &mut String) {
    out.push('"');
    for &b in s {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('"');
}

/// `TypeCode_print`'s text for `tc`: the `TypeCodeNew` call that makes it
/// (the Reference's description; the spacing is Warpine's), `NULL` for no
/// TypeCode.
pub fn text(recs: &HashMap<u32, TypeCodeRec>, tc: u32, level: u32, out: &mut String) {
    let Some(rec) = recs.get(&tc).filter(|_| level <= MAX_TC_DEPTH && out.len() <= MAX_TC_TEXT) else {
        out.push_str("NULL");
        return;
    };
    out.push_str("TypeCodeNew (");
    out.push_str(tk::NAMES.get(rec.kind as usize).copied().unwrap_or("tk_null"));
    let mut flags = rec.flags.iter();
    for (i, p) in rec.params.iter().enumerate() {
        out.push_str(", ");
        // A union member starts with its flag.
        if rec.kind == tk::UNION && i >= 2 && (i - 2) % 3 == 0 {
            let flag = flags.next().copied().unwrap_or(TCREGULAR_CASE);
            out.push_str(if flag == TCDEFAULT_CASE { "TCDEFAULT_CASE, " } else { "TCREGULAR_CASE, " });
        }
        match p {
            TcParam::Str(s) => c_string(s, out),
            TcParam::Long(v) => out.push_str(&v.to_string()),
            TcParam::Tc(c) => text(recs, *c, level + 1, out),
        }
    }
    if matches!(rec.kind, tk::STRUCT | tk::UNION | tk::ENUM) { out.push_str(", NULL"); }
    out.push(')');
}

/// The TypeCodes `tc` took, all the way down (itself first).
fn owned_tree(recs: &HashMap<u32, TypeCodeRec>, tc: u32) -> Vec<u32> {
    let mut out = vec![tc];
    let mut i = 0;
    while i < out.len() && out.len() <= MAX_TYPECODES {
        let at = out[i];
        if let Some(rec) = recs.get(&at) {
            for p in &rec.params {
                if let TcParam::Tc(c) = p && recs.get(c).is_some_and(|r| r.owner == Some(at)) { out.push(*c); }
            }
        }
        i += 1;
    }
    out
}

impl Loader {
    /// A TypeCode function by its ordinal, with its argument words `a`
    /// (`TypeCodeNew` reads its own from the stack: [`Self::tc_new`]).
    pub(crate) fn tc_call(&self, vcpu: u32, ord: u32, a: &[u32; 10]) -> ApiResult {
        self.som_init();
        let ok = ApiResult::Normal;
        let tcs = || self.shared.som.lock_or_recover();
        match ord {
            TC_ALIGNMENT => ok(layout(&tcs().typecodes.recs, a[0], 0).map_or(0, |l| l.1)),
            TC_SIZE => ok(layout(&tcs().typecodes.recs, a[0], 0).map_or(0, |l| l.0)),
            TC_KIND => ok(tcs().typecodes.recs.get(&a[0]).map_or(tk::NULL, |r| r.kind)),
            TC_PARAM_COUNT => ok(tcs().typecodes.recs.get(&a[0]).map_or(0, |r| r.params.len() as u32)),
            TC_EQUAL => ok(equal(&tcs().typecodes.recs, a[0], a[2], 0) as u32),
            TC_COPY => ok(self.tc_copy(a[0], 0).unwrap_or(0)),
            TC_FREE => { self.tc_free(a[0]); ok(0) }
            TC_SET_ALIGNMENT => {
                // A power of two up to 64 (Warpine's bounds); others, and
                // constants, are left alone.
                let align = a[2] & 0xFFFF;
                if align.is_power_of_two() && align <= 64
                    && let Some(rec) = tcs().typecodes.recs.get_mut(&a[0]).filter(|r| !r.constant) {
                    rec.alignment = Some(align);
                }
                ok(0)
            }
            TC_PARAMETER => ok(self.tc_parameter(a[0], a[1], a[2], a[3])),
            TC_PRINT => {
                let mut out = String::new();
                text(&tcs().typecodes.recs, a[0], 0, &mut out);
                out.truncate(MAX_TC_TEXT - 1);
                out.push('\n');
                self.som_output(vcpu, out.into_bytes())
            }
            TC_CONSTANT => ok(self.tc_constant(a[0])),
            _ => ok(0),
        }
    }

    /// `TypeCodeNew(kind, …)`: its arguments are the stack words from
    /// `args` (the first, `kind`, at `args`); 0 when they are not well
    /// formed or name something that is no TypeCode.
    pub(crate) fn tc_new(&self, args: u32) -> u32 {
        self.som_init();
        let word = |i: u32| {
            let at = args.checked_add(i.checked_mul(4)?)?;
            self.guest_read_buffer(at, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let Some(kind) = word(0) else { return 0 };
        let Some((parsed, flags)) = parse_new(kind, &word) else { return 0 };
        // Strings copied, TypeCodes checked.
        let mut params = Vec::with_capacity(parsed.len());
        for p in parsed {
            params.push(match p {
                Arg::Str(ptr) => match self.som_bytes(ptr, MAX_TC_STRING) {
                    Some(s) => TcParam::Str(s),
                    None => return 0,
                },
                Arg::Tc(tc) => TcParam::Tc(tc),
                Arg::Long(v) => TcParam::Long(v as i32),
            });
        }
        // Children taken, or copied when held already or given twice.
        let mut copies = Vec::new();
        let mut taken = Vec::new();
        for p in params.iter_mut() {
            let TcParam::Tc(c) = p else { continue };
            let (known, constant, held) = {
                let rt = self.shared.som.lock_or_recover();
                let rec = rt.typecodes.recs.get(c);
                let deep_enough = depth(&rt.typecodes.recs, *c, 1).is_some_and(|d| d < MAX_TC_DEPTH);
                (rec.is_some() && deep_enough, rec.is_some_and(|r| r.constant), rec.is_some_and(|r| r.owner.is_some()))
            };
            if !known { self.tc_release(&copies); return 0; }
            if constant { continue; }
            if held || taken.contains(c) {
                match self.tc_copy(*c, 1) {
                    Some(copy) => { copies.push(copy); *c = copy; }
                    None => { self.tc_release(&copies); return 0; }
                }
            } else {
                taken.push(*c);
            }
        }
        match self.tc_store(kind, params, flags, None, false) {
            Some(tc) => tc,
            None => { self.tc_release(&copies); 0 }
        }
    }

    /// A new TypeCode with these parameters: its guest block written and
    /// its record kept; the TypeCodes it names that are not constants
    /// become its own. `None` when the heap or the record count runs out,
    /// or a child was taken meanwhile.
    fn tc_store(&self, kind: u32, params: Vec<TcParam>, flags: Vec<u32>, alignment: Option<u32>, constant: bool) -> Option<u32> {
        let strings: u32 = params.iter().map(|p| match p { TcParam::Str(s) => s.len() as u32 + 1, _ => 0 }).sum();
        let size = BLOCK_HEADER + 4 * params.len() as u32 + strings;
        if self.shared.som.lock_or_recover().typecodes.recs.len() >= MAX_TYPECODES { return None; }
        let block = self.som_malloc(size)?;
        let values = block + BLOCK_HEADER;
        let mut text_at = values + 4 * params.len() as u32;
        let mut bytes = Vec::with_capacity(size as usize);
        bytes.extend(kind.to_le_bytes());
        bytes.extend((params.len() as u32).to_le_bytes());
        let mut strings_out = Vec::with_capacity(strings as usize);
        for p in &params {
            let word = match p {
                TcParam::Str(s) => {
                    let at = text_at;
                    strings_out.extend_from_slice(s);
                    strings_out.push(0);
                    text_at += s.len() as u32 + 1;
                    at
                }
                TcParam::Tc(c) => *c,
                TcParam::Long(v) => *v as u32,
            };
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(strings_out);
        let _ = self.guest_write_bytes(block, &bytes);
        let mut rt = self.shared.som.lock_or_recover();
        let recs = &mut rt.typecodes.recs;
        // Every child still there, and the free ones not taken meanwhile.
        let fine = params.iter().all(|p| match p {
            TcParam::Tc(c) => recs.get(c).is_some_and(|r| r.constant || r.owner.is_none()),
            _ => true,
        });
        if !fine {
            drop(rt);
            self.som_free_mem(block);
            return None;
        }
        for p in &params {
            if let TcParam::Tc(c) = p && let Some(r) = recs.get_mut(c).filter(|r| !r.constant) { r.owner = Some(block); }
        }
        recs.insert(block, TypeCodeRec { kind, params, flags, alignment, constant, owner: None, values });
        Some(block)
    }

    /// `TypeCode_copy`: a TypeCode like `tc` that refers to none of its
    /// TypeCodes (the constants excepted, which live as long as the
    /// process) or strings.
    fn tc_copy(&self, tc: u32, level: u32) -> Option<u32> {
        if level > MAX_TC_DEPTH { return None; }
        let rec = self.shared.som.lock_or_recover().typecodes.recs.get(&tc)?.clone();
        let mut copies = Vec::new();
        let mut params = rec.params;
        for p in params.iter_mut() {
            let TcParam::Tc(c) = p else { continue };
            let constant = self.shared.som.lock_or_recover().typecodes.recs.get(c).is_some_and(|r| r.constant);
            if constant { continue; }
            match self.tc_copy(*c, level + 1) {
                Some(copy) => { copies.push(copy); *c = copy; }
                None => { self.tc_release(&copies); return None; }
            }
        }
        let made = self.tc_store(rec.kind, params, rec.flags, rec.alignment, false);
        if made.is_none() { self.tc_release(&copies); }
        made
    }

    /// Free TypeCodes nobody took yet (a failed `TypeCodeNew`'s copies).
    fn tc_release(&self, tcs: &[u32]) {
        for &tc in tcs { self.tc_free(tc); }
    }

    /// `TypeCode_free`: `tc` and the TypeCodes it took; nothing for a
    /// constant, one another TypeCode holds, or no TypeCode.
    fn tc_free(&self, tc: u32) {
        let blocks = {
            let mut rt = self.shared.som.lock_or_recover();
            let recs = &mut rt.typecodes.recs;
            if !recs.get(&tc).is_some_and(|r| !r.constant && r.owner.is_none()) { return; }
            let tree = owned_tree(recs, tc);
            for b in &tree { recs.remove(b); }
            tree
        };
        for b in blocks { self.som_free_mem(b); }
    }

    /// `TypeCode_parameter(tc, ev, index)`: the `any` at `result` names
    /// parameter `index` (`_type` a `TC_*` constant, `_value` its word in
    /// the block); an index out of range, or a TypeCode without
    /// parameters, raises `TypeCode::Bounds` in `ev` and gives an empty
    /// `any`. `result` back.
    fn tc_parameter(&self, result: u32, tc: u32, ev: u32, index: u32) -> u32 {
        let found = {
            let rt = self.shared.som.lock_or_recover();
            rt.typecodes.recs.get(&tc).and_then(|r| {
                let p = r.params.get(index as usize)?;
                Some((p.kind(), r.values + 4 * index))
            })
        };
        let (ty, value) = match found {
            Some((kind, value)) => (self.tc_constant(kind), value),
            None => {
                let name = self.tc_bounds_name();
                self.som_set_exception(ev, USER_EXCEPTION, name, 0);
                (0, 0)
            }
        };
        if result != 0 {
            let _ = self.guest_write::<u32>(result, ty);
            let _ = self.guest_write::<u32>(result.wrapping_add(4), value);
        }
        result
    }

    /// The guest copy of [`EX_TYPECODE_BOUNDS`], made once.
    fn tc_bounds_name(&self) -> u32 {
        let have = self.shared.som.lock_or_recover().typecodes.bounds_name;
        if have != 0 { return have; }
        let Some(at) = self.som_malloc(EX_TYPECODE_BOUNDS.len() as u32 + 1) else { return 0 };
        let _ = self.guest_write_bytes(at, EX_TYPECODE_BOUNDS.as_bytes());
        let mut rt = self.shared.som.lock_or_recover();
        if rt.typecodes.bounds_name == 0 { rt.typecodes.bounds_name = at; return at; }
        let kept = rt.typecodes.bounds_name;
        drop(rt);
        self.som_free_mem(at);
        kept
    }

    /// The `TC_*` constant of `kind`: the basic kinds (`tk_null` …
    /// `tk_Principal`), `TC_Object` (an object reference to
    /// `::SOMObject`) and `TC_string` (unbounded); 0 for another kind.
    /// Each is made once.
    pub(crate) fn tc_constant(&self, kind: u32) -> u32 {
        let params = match kind {
            tk::NULL..=tk::PRINCIPAL => Vec::new(),
            tk::OBJREF => vec![TcParam::Str(OBJECT_ID.as_bytes().to_vec())],
            tk::STRING => vec![TcParam::Long(0)],
            _ => return 0,
        };
        if let Some(&tc) = self.shared.som.lock_or_recover().typecodes.constants.get(&kind) { return tc; }
        let Some(tc) = self.tc_store(kind, params, Vec::new(), None, true) else { return 0 };
        let mut rt = self.shared.som.lock_or_recover();
        if let Some(&kept) = rt.typecodes.constants.get(&kind) {
            // Another thread made it first.
            rt.typecodes.recs.remove(&tc);
            drop(rt);
            self.som_free_mem(tc);
            return kept;
        }
        rt.typecodes.constants.insert(kind, tc);
        tc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(r: ApiResult) -> u32 {
        match r { ApiResult::Normal(v) => v, other => panic!("{other:?}") }
    }

    /// Guest scratch for strings and argument lists.
    const SCRATCH: u32 = 0x0030_0000;

    struct Guest<'a> { l: &'a Loader, next: u32 }

    impl<'a> Guest<'a> {
        fn new(l: &'a Loader) -> Self { Guest { l, next: SCRATCH } }
        fn string(&mut self, s: &str) -> u32 {
            let at = self.next;
            self.l.guest_write_bytes(at, format!("{s}\0").as_bytes()).unwrap();
            self.next += (s.len() as u32 + 4) & !3;
            at
        }
        /// `TypeCodeNew` with these argument words.
        fn new_tc(&mut self, words: &[u32]) -> u32 {
            let at = self.next;
            for (i, w) in words.iter().enumerate() { self.l.guest_write::<u32>(at + 4 * i as u32, *w).unwrap(); }
            self.next += 4 * words.len() as u32;
            self.l.tc_new(at)
        }
    }

    fn call(l: &Loader, ord: u32, args: &[u32]) -> u32 {
        let mut a = [0u32; 10];
        a[..args.len()].copy_from_slice(args);
        ok(l.tc_call(0, ord, &a))
    }

    fn printed(l: &Loader, tc: u32) -> String {
        let mut out = String::new();
        text(&l.shared.som.lock_or_recover().typecodes.recs, tc, 0, &mut out);
        out
    }

    /// The Reference's example, `struct node { long count; struct node
    /// *next; }`, built from nested calls: its kind, parameters, size,
    /// alignment and printed form; freed with everything it took.
    #[test]
    fn a_self_referential_struct_from_nested_calls() {
        let l = Loader::new_mock();
        let mut g = Guest::new(&l);
        let (node, count, next) = (g.string("node"), g.string("count"), g.string("next"));
        let long = g.new_tc(&[tk::LONG]);
        let selfref = g.new_tc(&[tk::SELF, node]);
        let ptr = g.new_tc(&[tk::POINTER, selfref]);
        let tc = g.new_tc(&[tk::STRUCT, node, count, long, next, ptr, 0]);
        assert_ne!(tc, 0);
        assert_eq!(call(&l, TC_KIND, &[tc]), tk::STRUCT);
        assert_eq!(call(&l, TC_PARAM_COUNT, &[tc]), 5, "2N+1");
        assert_eq!((call(&l, TC_SIZE, &[tc]), call(&l, TC_ALIGNMENT, &[tc])), (8, 4));
        assert_eq!(printed(&l, tc), "TypeCodeNew (tk_struct, \"node\", \"count\", TypeCodeNew (tk_long), \
                                     \"next\", TypeCodeNew (tk_pointer, TypeCodeNew (tk_self, \"node\")), NULL)");
        // Freeing a TypeCode the struct took does nothing; freeing the
        // struct frees all four.
        call(&l, TC_FREE, &[long]);
        assert_eq!(call(&l, TC_KIND, &[long]), tk::LONG);
        call(&l, TC_FREE, &[tc]);
        for t in [tc, long, selfref, ptr] { assert_eq!(call(&l, TC_PARAM_COUNT, &[t]), 0); }
        assert!(l.shared.som.lock_or_recover().typecodes.recs.is_empty());
    }

    /// Sizes and alignments follow the SDK's i386 C layout.
    #[test]
    fn sizes_and_alignments_follow_c() {
        let l = Loader::new_mock();
        let mut g = Guest::new(&l);
        let (s, a, b, c, u, d) = (g.string("s"), g.string("a"), g.string("b"), g.string("c"), g.string("u"), g.string("d"));
        let k = |g: &mut Guest, kind| g.new_tc(&[kind]);
        let size = |tc| (call(&l, TC_SIZE, &[tc]), call(&l, TC_ALIGNMENT, &[tc]));
        for (kind, want) in [(tk::NULL, (0, 1)), (tk::SHORT, (2, 2)), (tk::LONG, (4, 4)), (tk::DOUBLE, (8, 4)),
                             (tk::BOOLEAN, (1, 1)), (tk::ANY, (8, 4)), (tk::TYPECODE, (4, 4))] {
            let tc = k(&mut g, kind);
            assert_eq!(size(tc), want, "{}", tk::NAMES[kind as usize]);
        }
        // struct { char a; double b; short c; }: 1 + 3 + 8 + 2 + 2 = 16.
        let (ch, db, sh) = (k(&mut g, tk::CHAR), k(&mut g, tk::DOUBLE), k(&mut g, tk::SHORT));
        let st = g.new_tc(&[tk::STRUCT, s, a, ch, b, db, c, sh, 0]);
        assert_eq!(size(st), (16, 4));
        // char x[3] in a struct after a short: 2 + 3 + pad 1 = 6.
        let (ch, sh) = (k(&mut g, tk::CHAR), k(&mut g, tk::SHORT));
        let arr = g.new_tc(&[tk::ARRAY, ch, 3]);
        assert_eq!(size(arr), (3, 1));
        let st2 = g.new_tc(&[tk::STRUCT, s, a, sh, b, arr, 0]);
        assert_eq!(size(st2), (6, 2));
        // union u switch (short) { case 1: char a; default: double d; }:
        // the discriminator padded to the union's 4, then 8.
        let (sw, ch, db) = (k(&mut g, tk::SHORT), k(&mut g, tk::CHAR), k(&mut g, tk::DOUBLE));
        let un = g.new_tc(&[tk::UNION, u, sw, TCREGULAR_CASE, 1, a, ch, TCDEFAULT_CASE, 0, d, db, 0]);
        assert_eq!(call(&l, TC_PARAM_COUNT, &[un]), 8, "3N+2");
        assert_eq!(size(un), (12, 4));
        assert!(printed(&l, un).contains("TCREGULAR_CASE, 1, \"a\", TypeCodeNew (tk_char), TCDEFAULT_CASE, 0, \"d\""));
        // A sequence is { maximum, length, buffer }; a string a pointer.
        let el = k(&mut g, tk::LONG);
        assert_eq!(size(g.new_tc(&[tk::SEQUENCE, el, 0])), (12, 4));
        assert_eq!(size(g.new_tc(&[tk::STRING, 10])), (4, 4));
        assert_eq!(size(g.new_tc(&[tk::FOREIGN, a, b, 3])), (3, 2));
        // setAlignment changes the alignment only, and only a power of
        // two up to 64.
        call(&l, TC_SET_ALIGNMENT, &[st, 0, 8]);
        assert_eq!(size(st), (16, 8));
        call(&l, TC_SET_ALIGNMENT, &[st, 0, 3]);
        assert_eq!(size(st), (16, 8));
    }

    /// TypeCodeNew refuses what is not well formed, and anything that is
    /// no TypeCode reads as nothing.
    #[test]
    fn malformed_calls_and_unknown_typecodes() {
        let l = Loader::new_mock();
        let mut g = Guest::new(&l);
        let name = g.string("x");
        assert_eq!(g.new_tc(&[99]), 0, "no such kind");
        assert_eq!(g.new_tc(&[tk::POINTER, 0x0040_0000]), 0, "no TypeCode");
        assert_eq!(g.new_tc(&[tk::STRING, (-1i32) as u32]), 0, "negative bound");
        assert_eq!(g.new_tc(&[tk::OBJREF, 0]), 0, "no id");
        let long = g.new_tc(&[tk::LONG]);
        assert_eq!(g.new_tc(&[tk::UNION, name, long, 7, 0, name, long, 0]), 0, "bad flag");
        // A string in protected memory is not copied.
        assert_eq!(g.new_tc(&[tk::OBJREF, super::super::constants::SYSTEM_AREA_BASE]), 0);
        // An unknown address: tk_null, nothing to size, not equal to itself.
        let bogus = 0x0040_0000;
        assert_eq!(call(&l, TC_KIND, &[bogus]), tk::NULL);
        assert_eq!(call(&l, TC_SIZE, &[bogus]), 0);
        assert_eq!(call(&l, TC_EQUAL, &[bogus, 0, bogus]), 0);
        assert_eq!(call(&l, TC_COPY, &[bogus]), 0);
        assert_eq!(printed(&l, bogus), "NULL");
        // Nesting stops at the limit.
        let mut tc = g.new_tc(&[tk::LONG]);
        let mut made = 1;
        loop {
            let p = g.new_tc(&[tk::POINTER, tc]);
            if p == 0 { break; }
            tc = p;
            made += 1;
        }
        assert_eq!(made, MAX_TC_DEPTH);
    }

    /// A TypeCode given twice, or one another holds, is copied; copies
    /// equal their originals and outlive them; constants are shared and
    /// never freed.
    #[test]
    fn ownership_copies_and_constants() {
        let l = Loader::new_mock();
        let mut g = Guest::new(&l);
        let (s, a, b) = (g.string("s"), g.string("a"), g.string("b"));
        let long = g.new_tc(&[tk::LONG]);
        let st = g.new_tc(&[tk::STRUCT, s, a, long, b, long, 0]);
        let recs = || l.shared.som.lock_or_recover().typecodes.recs.clone();
        let members: Vec<u32> = recs()[&st].params.iter().filter_map(|p| if let TcParam::Tc(c) = p { Some(*c) } else { None }).collect();
        assert_eq!(members[0], long, "taken");
        assert_ne!(members[1], long, "the second use copied");
        let other = g.new_tc(&[tk::POINTER, long]);
        assert_ne!(recs()[&other].params[0], TcParam::Tc(long), "held by st: copied");
        let copy = call(&l, TC_COPY, &[st]);
        assert!(copy != 0 && copy != st);
        assert_eq!(call(&l, TC_EQUAL, &[copy, 0, st]), 1);
        call(&l, TC_SET_ALIGNMENT, &[copy, 0, 16]);
        assert_eq!(call(&l, TC_EQUAL, &[copy, 0, st]), 0, "alignment counts");
        call(&l, TC_FREE, &[st]);
        assert_eq!(call(&l, TC_SIZE, &[copy]), 8, "the copy lives on");
        // Constants: one per kind, shared, never freed or realigned.
        let tc_long = call(&l, TC_CONSTANT, &[tk::LONG]);
        assert_eq!(call(&l, TC_CONSTANT, &[tk::LONG]), tc_long);
        let p = g.new_tc(&[tk::POINTER, tc_long]);
        assert_eq!(recs()[&p].params[0], TcParam::Tc(tc_long));
        call(&l, TC_FREE, &[p]);
        call(&l, TC_FREE, &[tc_long]);
        call(&l, TC_SET_ALIGNMENT, &[tc_long, 0, 8]);
        assert_eq!(call(&l, TC_ALIGNMENT, &[tc_long]), 4);
        assert_eq!(printed(&l, call(&l, TC_CONSTANT, &[tk::OBJREF])), "TypeCodeNew (tk_objref, \"::SOMObject\")");
        assert_eq!(call(&l, TC_CONSTANT, &[tk::STRUCT]), 0);
    }

    /// TypeCode_parameter: an any naming the parameter's word, or the
    /// Bounds exception.
    #[test]
    fn parameters_as_any_and_the_bounds_exception() {
        let l = Loader::new_mock();
        let mut g = Guest::new(&l);
        let (color, red, green) = (g.string("color"), g.string("red"), g.string("green"));
        let en = g.new_tc(&[tk::ENUM, color, red, green, 0]);
        assert_eq!(call(&l, TC_PARAM_COUNT, &[en]), 3, "N+1");
        let (result, ev) = (0x0031_0000, 0x0031_0100);
        assert_eq!(call(&l, TC_PARAMETER, &[result, en, ev, 2]), result);
        let ty = l.guest_read::<u32>(result).unwrap();
        assert_eq!(ty, call(&l, TC_CONSTANT, &[tk::STRING]));
        let value = l.guest_read::<u32>(result + 4).unwrap();
        let s = l.guest_read::<u32>(value).unwrap();
        assert_eq!(l.read_guest_string(s), "green");
        assert_eq!(l.guest_read::<u32>(ev).unwrap(), 0, "no exception");
        // Out of range, and a basic kind: Bounds.
        let long = g.new_tc(&[tk::LONG]);
        for (tc, i) in [(en, 3), (long, 0)] {
            call(&l, TC_PARAMETER, &[result, tc, ev, i]);
            assert_eq!(l.guest_read::<u32>(ev).unwrap(), USER_EXCEPTION);
            let name = l.guest_read::<u32>(ev + 4).unwrap();
            assert_eq!(l.read_guest_string(name), EX_TYPECODE_BOUNDS);
            assert_eq!((l.guest_read::<u32>(result).unwrap(), l.guest_read::<u32>(result + 4).unwrap()), (0, 0));
        }
        // A long parameter: TC_long, its value.
        let bounded = g.new_tc(&[tk::STRING, 80]);
        call(&l, TC_PARAMETER, &[result, bounded, 0, 0]);
        assert_eq!(l.guest_read::<u32>(result).unwrap(), call(&l, TC_CONSTANT, &[tk::LONG]));
        assert_eq!(l.guest_read::<u32>(l.guest_read::<u32>(result + 4).unwrap()).unwrap(), 80);
    }

    /// The parser's member limit.
    #[test]
    fn a_struct_with_too_many_members_is_refused() {
        let words: Vec<u32> = std::iter::once(0x100).chain(std::iter::repeat_n(0x200, 2 * MAX_TC_MEMBERS + 3)).collect();
        let word = |i: u32| words.get(i as usize).copied();
        assert!(parse_new(tk::STRUCT, &word).is_none());
        let fits: Vec<u32> = std::iter::once(0x100).chain(std::iter::repeat_n(0x200, 2 * MAX_TC_MEMBERS + 1)).chain([0]).collect();
        let word = |i: u32| fits.get(i as usize).copied();
        assert_eq!(parse_new(tk::STRUCT, &word).map(|(a, _)| a.len()), Some(2 * MAX_TC_MEMBERS + 1));
    }
}
