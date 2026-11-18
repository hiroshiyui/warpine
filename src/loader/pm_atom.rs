// SPDX-License-Identifier: GPL-3.0-only
//
// PMWIN atom tables, after the IBM PM Reference ("Atom Functions"):
// `WinCreateAtomTable` (714), `WinDestroyAtomTable` (724), `WinAddAtom`
// (700), `WinDeleteAtom` (721), `WinFindAtom` (744), `WinQueryAtomName`
// (802), `WinQueryAtomLength` (801), `WinQueryAtomUsage` (803) and
// `WinQuerySystemAtomTable` (830).
//
// An atom names a string. String atoms are numbered from `0xC000`; an
// integer atom is a number from 1 to `0xBFFF` given as `MAKEINTATOM(n)`
// (high word `0xFFFF`) or as the string `"#n"`, and stands for itself: it
// is not stored, its name is `"#n"`. Adding a name that is already there
// counts one more use; deleting counts one use off and removes the atom at
// zero. Names are compared without regard to (ASCII) case and kept as the
// program first gave them, as bytes in its codepage.
//
// The system atom table is the process's own (ADR-0020: nothing is shared
// between Warpine processes), so atoms are not a channel between programs.
//
// Bounded: at most `MAX_ATOM_TABLES` tables, names of 1 to `MAX_ATOM_NAME`
// bytes, `0x4000` string atoms per table, usage counts up to `MAX_USAGE`.
//
// Unverified (the Reference is silent): the usage of an integer atom
// (`MAX_USAGE`, as it is never removed), deleting an integer atom
// succeeding, "#" followed by anything but decimal digits being an
// ordinary string name, and `PMERR_HEAP_OUT_OF_MEMORY` for a full table or
// too many tables.

use std::collections::{BTreeSet, HashMap};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::ApiResult;

/// `PMERR_INVALID_HATOMTBL` (`<pmerr.h>`).
pub const PMERR_INVALID_HATOMTBL: u32 = 0x1013;
/// `PMERR_INVALID_ATOM` (`<pmerr.h>`).
pub const PMERR_INVALID_ATOM: u32 = 0x1014;
/// `PMERR_HEAP_OUT_OF_MEMORY` (`<pmerr.h>`).
pub const PMERR_HEAP_OUT_OF_MEMORY: u32 = 0x1011;

/// Longest atom name in bytes.
pub const MAX_ATOM_NAME: usize = 255;
/// Highest integer atom.
pub const MAX_INTEGER_ATOM: u32 = 0xBFFF;
/// First string atom.
pub const FIRST_STRING_ATOM: u32 = 0xC000;
/// Last string atom.
pub const LAST_STRING_ATOM: u32 = 0xFFFF;
/// High word of `MAKEINTATOM(a)` (`MAKEULONG(a, 0xffff)`).
const INT_ATOM_MARK: u32 = 0xFFFF;
/// Highest usage count (and an integer atom's usage).
pub const MAX_USAGE: u32 = 0xFFFF;
/// Most atom tables a process may have, the system table included.
pub const MAX_ATOM_TABLES: usize = 64;
/// Handle of the process's system atom table.
pub const SYSTEM_ATOM_TABLE: u32 = 0x000A_0000;
/// Handle of the first table `WinCreateAtomTable` creates.
const FIRST_ATOM_TABLE: u32 = SYSTEM_ATOM_TABLE + 1;

/// An atom name as a program gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtomName {
    /// `MAKEINTATOM(n)` or `"#n"`.
    Int(u32),
    /// Any other name (bytes in the program's codepage).
    Str(Vec<u8>),
}

impl AtomName {
    /// Parse a name given as bytes: `"#n"` with decimal digits is an
    /// integer atom.
    pub fn from_bytes(name: &[u8]) -> AtomName {
        match name.split_first() {
            Some((b'#', digits)) if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) => {
                // Saturating: a number too large is an invalid integer atom.
                let n = digits.iter().fold(0u32, |n, d| n.saturating_mul(10).saturating_add(u32::from(d - b'0')));
                AtomName::Int(n)
            }
            _ => AtomName::Str(name.to_vec()),
        }
    }
}

#[derive(Debug, Clone)]
struct Atom {
    name: Vec<u8>,
    usage: u32,
}

/// One atom table.
#[derive(Debug)]
pub struct AtomTable {
    atoms: HashMap<u32, Atom>,
    /// Lower-case name → atom.
    by_name: HashMap<Vec<u8>, u32>,
    /// Numbers given out and freed again (used lowest first).
    freed: BTreeSet<u32>,
    /// The lowest number never given out (0: none left).
    top: u32,
}

impl Default for AtomTable {
    fn default() -> Self {
        AtomTable { atoms: HashMap::new(), by_name: HashMap::new(), freed: BTreeSet::new(),
                    top: FIRST_STRING_ATOM }
    }
}

/// Why an atom call failed: the `PMERR_*` code.
pub type AtomResult<T> = Result<T, u32>;

fn fold(name: &[u8]) -> Vec<u8> {
    name.to_ascii_lowercase()
}

/// Check an integer atom's range.
fn int_atom(n: u32) -> AtomResult<u32> {
    if (1..=MAX_INTEGER_ATOM).contains(&n) { Ok(n) } else { Err(PMERR_INVALID_INTEGER_ATOM) }
}

/// Check a string name's length.
fn str_name(name: &[u8]) -> AtomResult<&[u8]> {
    if name.is_empty() || name.len() > MAX_ATOM_NAME { Err(PMERR_INVALID_ATOM_NAME) } else { Ok(name) }
}

impl AtomTable {
    /// `WinAddAtom`: the atom of `name`, one more use.
    pub fn add(&mut self, name: &AtomName) -> AtomResult<u32> {
        let name = match name {
            AtomName::Int(n) => return int_atom(*n),
            AtomName::Str(s) => str_name(s)?,
        };
        let key = fold(name);
        if let Some(&atom) = self.by_name.get(&key) {
            if let Some(a) = self.atoms.get_mut(&atom) {
                a.usage = (a.usage + 1).min(MAX_USAGE);
            }
            return Ok(atom);
        }
        let atom = match self.freed.pop_first() {
            Some(a) => a,
            None if self.top != 0 => {
                let a = self.top;
                self.top = if a == LAST_STRING_ATOM { 0 } else { a + 1 };
                a
            }
            None => return Err(PMERR_HEAP_OUT_OF_MEMORY),
        };
        self.atoms.insert(atom, Atom { name: name.to_vec(), usage: 1 });
        self.by_name.insert(key, atom);
        Ok(atom)
    }

    /// `WinFindAtom`: the atom of `name` (usage unchanged).
    pub fn find(&self, name: &AtomName) -> AtomResult<u32> {
        match name {
            AtomName::Int(n) => int_atom(*n),
            AtomName::Str(s) => self.by_name.get(&fold(str_name(s)?)).copied()
                .ok_or(PMERR_ATOM_NAME_NOT_FOUND),
        }
    }

    /// `WinDeleteAtom`: one use less; the atom goes at zero.
    pub fn delete(&mut self, atom: u32) -> AtomResult<()> {
        if atom <= MAX_INTEGER_ATOM {
            return int_atom(atom).map(|_| ());
        }
        let a = self.atoms.get_mut(&atom).ok_or(PMERR_INVALID_ATOM)?;
        // A count that reached the maximum stays (the atom is locked).
        if a.usage < MAX_USAGE {
            a.usage -= 1;
        }
        if a.usage == 0 {
            let name = fold(&a.name);
            self.atoms.remove(&atom);
            self.by_name.remove(&name);
            self.freed.insert(atom);
        }
        Ok(())
    }

    /// The name of `atom` (`"#n"` for an integer atom).
    pub fn name(&self, atom: u32) -> AtomResult<Vec<u8>> {
        if atom <= MAX_INTEGER_ATOM {
            return int_atom(atom).map(|n| format!("#{n}").into_bytes());
        }
        self.atoms.get(&atom).map(|a| a.name.clone()).ok_or(PMERR_INVALID_ATOM)
    }

    /// The usage count of `atom`.
    pub fn usage(&self, atom: u32) -> AtomResult<u32> {
        if atom <= MAX_INTEGER_ATOM {
            return int_atom(atom).map(|_| MAX_USAGE);
        }
        self.atoms.get(&atom).map(|a| a.usage).ok_or(PMERR_INVALID_ATOM)
    }

    /// Number of string atoms.
    pub fn len(&self) -> usize { self.atoms.len() }

    /// Whether the table holds no string atom.
    pub fn is_empty(&self) -> bool { self.atoms.is_empty() }
}

/// The process's atom tables (`WindowManager::atoms`).
#[derive(Debug)]
pub struct AtomTables {
    tables: HashMap<u32, AtomTable>,
    next: u32,
}

impl Default for AtomTables {
    fn default() -> Self {
        let mut tables = HashMap::new();
        tables.insert(SYSTEM_ATOM_TABLE, AtomTable::default());
        AtomTables { tables, next: FIRST_ATOM_TABLE }
    }
}

impl AtomTables {
    /// `WinCreateAtomTable`: a new, empty table.
    pub fn create(&mut self) -> AtomResult<u32> {
        if self.tables.len() >= MAX_ATOM_TABLES {
            return Err(PMERR_HEAP_OUT_OF_MEMORY);
        }
        let mut h = self.next;
        while h == SYSTEM_ATOM_TABLE || h == 0 || self.tables.contains_key(&h) {
            h = h.wrapping_add(1);
        }
        self.next = h.wrapping_add(1);
        self.tables.insert(h, AtomTable::default());
        Ok(h)
    }

    /// `WinDestroyAtomTable`: the system table cannot be destroyed.
    pub fn destroy(&mut self, h: u32) -> AtomResult<()> {
        if h == SYSTEM_ATOM_TABLE { return Err(PMERR_INVALID_HATOMTBL); }
        self.tables.remove(&h).map(|_| ()).ok_or(PMERR_INVALID_HATOMTBL)
    }

    pub fn get(&self, h: u32) -> AtomResult<&AtomTable> {
        self.tables.get(&h).ok_or(PMERR_INVALID_HATOMTBL)
    }

    pub fn get_mut(&mut self, h: u32) -> AtomResult<&mut AtomTable> {
        self.tables.get_mut(&h).ok_or(PMERR_INVALID_HATOMTBL)
    }
}

impl super::Loader {
    /// The NUL-terminated byte string at `ptr`, at most `max` bytes before
    /// the NUL; `None` when it is out of guest memory or longer.
    pub(crate) fn guest_cstr(&self, ptr: u32, max: usize) -> Option<Vec<u8>> {
        if ptr == 0 { return None; }
        let avail = self.shared.guest_mem.size().saturating_sub(ptr as usize);
        if avail == 0 { return None; }
        let bytes = self.guest_read_bytes(ptr, avail.min(max + 1))?;
        let n = bytes.iter().position(|&b| b == 0)?;
        Some(bytes[..n].to_vec())
    }

    /// The atom name a program passes at `psz`.
    fn atom_name(&self, psz: u32) -> AtomResult<AtomName> {
        if psz >> 16 == INT_ATOM_MARK {
            return Ok(AtomName::Int(psz & 0xFFFF));
        }
        self.guest_cstr(psz, MAX_ATOM_NAME).map(|b| AtomName::from_bytes(&b))
            .ok_or(PMERR_INVALID_ATOM_NAME)
    }

    /// A value, or 0 with the error recorded for `WinGetLastError`.
    fn atom_result(&self, tid: u32, r: AtomResult<u32>) -> ApiResult {
        match r {
            Ok(v) => ApiResult::Normal(v),
            Err(code) => self.pm_fail(tid, code),
        }
    }

    /// `WinCreateAtomTable(cbInitial, cBuckets)` → HATOMTBL. The sizing
    /// hints are not needed.
    pub(super) fn win_create_atom_table(&self, tid: u32) -> ApiResult {
        let r = self.shared.window_mgr.lock_or_recover().atoms.create();
        self.atom_result(tid, r)
    }

    /// `WinDestroyAtomTable(hAtomTbl)` → NULLHANDLE when destroyed, else
    /// the handle.
    pub(super) fn win_destroy_atom_table(&self, tid: u32, h: u32) -> ApiResult {
        match self.shared.window_mgr.lock_or_recover().atoms.destroy(h) {
            Ok(()) => ApiResult::Normal(0),
            Err(code) => { self.pm_error(tid, code); ApiResult::Normal(h) }
        }
    }

    /// `WinQuerySystemAtomTable()` → the process's system atom table.
    pub(super) fn win_query_system_atom_table(&self) -> ApiResult {
        ApiResult::Normal(SYSTEM_ATOM_TABLE)
    }

    /// `WinAddAtom(hAtomTbl, pszAtomName)` → ATOM (0 on error).
    pub(super) fn win_add_atom(&self, tid: u32, h: u32, psz: u32) -> ApiResult {
        let r = (|| {
            let name = self.atom_name(psz)?;
            self.shared.window_mgr.lock_or_recover().atoms.get_mut(h)?.add(&name)
        })();
        self.atom_result(tid, r)
    }

    /// `WinFindAtom(hAtomTbl, pszAtomName)` → ATOM (0 when not there).
    pub(super) fn win_find_atom(&self, tid: u32, h: u32, psz: u32) -> ApiResult {
        let r = (|| {
            let name = self.atom_name(psz)?;
            self.shared.window_mgr.lock_or_recover().atoms.get(h)?.find(&name)
        })();
        self.atom_result(tid, r)
    }

    /// `WinDeleteAtom(hAtomTbl, atom)` → 0 when done, else the atom.
    pub(super) fn win_delete_atom(&self, tid: u32, h: u32, atom: u32) -> ApiResult {
        let r = self.shared.window_mgr.lock_or_recover().atoms.get_mut(h)
            .and_then(|t| t.delete(atom));
        match r {
            Ok(()) => ApiResult::Normal(0),
            Err(code) => { self.pm_error(tid, code); ApiResult::Normal(atom) }
        }
    }

    /// `WinQueryAtomUsage(hAtomTbl, atom)` → the usage count (0 on error).
    pub(super) fn win_query_atom_usage(&self, tid: u32, h: u32, atom: u32) -> ApiResult {
        let r = self.shared.window_mgr.lock_or_recover().atoms.get(h).and_then(|t| t.usage(atom));
        self.atom_result(tid, r)
    }

    /// `WinQueryAtomLength(hAtomTbl, atom)` → the name's length in bytes,
    /// without the NUL (0 on error).
    pub(super) fn win_query_atom_length(&self, tid: u32, h: u32, atom: u32) -> ApiResult {
        let r = self.shared.window_mgr.lock_or_recover().atoms.get(h)
            .and_then(|t| t.name(atom)).map(|n| n.len() as u32);
        self.atom_result(tid, r)
    }

    /// `WinQueryAtomName(hAtomTbl, atom, pchBuffer, cchBufferMax)` → the
    /// length copied, without the NUL; the name is cut to fit and always
    /// NUL-terminated (0 on error or for a zero-sized buffer).
    pub(super) fn win_query_atom_name(&self, tid: u32, h: u32, atom: u32, pch: u32, cch: u32) -> ApiResult {
        let r = (|| {
            let mut name = self.shared.window_mgr.lock_or_recover().atoms.get(h)?.name(atom)?;
            if cch == 0 || pch == 0 { return Err(PMERR_PARAMETER_OUT_OF_RANGE); }
            name.truncate(cch as usize - 1);
            let len = name.len() as u32;
            name.push(0);
            self.guest_write_bytes(pch, &name).ok_or(PMERR_INVALID_STRING_PARM)?;
            Ok(len)
        })();
        self.atom_result(tid, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;

    fn s(name: &str) -> AtomName { AtomName::Str(name.as_bytes().to_vec()) }

    #[test]
    fn names_parse_integer_atoms() {
        assert_eq!(AtomName::from_bytes(b"#123"), AtomName::Int(123));
        assert_eq!(AtomName::from_bytes(b"#0"), AtomName::Int(0));
        assert_eq!(AtomName::from_bytes(b"#99999999999"), AtomName::Int(u32::MAX));
        assert_eq!(AtomName::from_bytes(b"#4294967296"), AtomName::Int(u32::MAX));
        assert_eq!(AtomName::from_bytes(b"#12a"), s("#12a"));
        assert_eq!(AtomName::from_bytes(b"#"), s("#"));
        assert_eq!(AtomName::from_bytes(b"Name"), s("Name"));
    }

    #[test]
    fn string_atoms_count_uses_and_ignore_case() {
        let mut t = AtomTable::default();
        let a = t.add(&s("Hello")).unwrap();
        assert_eq!(a, FIRST_STRING_ATOM);
        assert_eq!(t.add(&s("HELLO")).unwrap(), a);
        assert_eq!(t.usage(a), Ok(2));
        assert_eq!(t.find(&s("hello")), Ok(a));
        assert_eq!(t.name(a).unwrap(), b"Hello");
        let b = t.add(&s("World")).unwrap();
        assert_eq!(b, FIRST_STRING_ATOM + 1);
        t.delete(a).unwrap();
        assert_eq!(t.usage(a), Ok(1));
        t.delete(a).unwrap();
        assert_eq!(t.usage(a), Err(PMERR_INVALID_ATOM));
        assert_eq!(t.find(&s("Hello")), Err(PMERR_ATOM_NAME_NOT_FOUND));
        assert_eq!(t.delete(a), Err(PMERR_INVALID_ATOM));
        // The freed number is used again.
        assert_eq!(t.add(&s("Again")).unwrap(), a);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn integer_atoms_stand_for_themselves() {
        let mut t = AtomTable::default();
        assert_eq!(t.add(&AtomName::Int(5)), Ok(5));
        assert_eq!(t.find(&AtomName::Int(5)), Ok(5));
        assert_eq!(t.name(5).unwrap(), b"#5");
        assert_eq!(t.usage(5), Ok(MAX_USAGE));
        assert_eq!(t.delete(5), Ok(()));
        assert!(t.is_empty());
        assert_eq!(t.add(&AtomName::Int(0)), Err(PMERR_INVALID_INTEGER_ATOM));
        assert_eq!(t.add(&AtomName::Int(MAX_INTEGER_ATOM + 1)), Err(PMERR_INVALID_INTEGER_ATOM));
        assert_eq!(t.name(0), Err(PMERR_INVALID_INTEGER_ATOM));
    }

    #[test]
    fn names_are_bounded() {
        let mut t = AtomTable::default();
        assert_eq!(t.add(&s("")), Err(PMERR_INVALID_ATOM_NAME));
        let long = "x".repeat(MAX_ATOM_NAME + 1);
        assert_eq!(t.add(&s(&long)), Err(PMERR_INVALID_ATOM_NAME));
        assert!(t.add(&s(&long[..MAX_ATOM_NAME])).is_ok());
    }

    #[test]
    fn a_full_table_refuses_more_atoms() {
        let mut t = AtomTable::default();
        for i in 0..=(LAST_STRING_ATOM - FIRST_STRING_ATOM) {
            t.add(&s(&format!("a{i}"))).unwrap();
        }
        assert_eq!(t.add(&s("one more")), Err(PMERR_HEAP_OUT_OF_MEMORY));
        // A name that is there still counts a use.
        assert!(t.add(&s("a0")).is_ok());
    }

    #[test]
    fn usage_saturates_and_locks_the_atom() {
        let mut t = AtomTable::default();
        let a = t.add(&s("x")).unwrap();
        for _ in 0..MAX_USAGE + 5 { t.add(&s("x")).unwrap(); }
        assert_eq!(t.usage(a), Ok(MAX_USAGE));
        t.delete(a).unwrap();
        assert_eq!(t.usage(a), Ok(MAX_USAGE));
    }

    #[test]
    fn tables_are_created_destroyed_and_bounded() {
        let mut tables = AtomTables::default();
        assert!(tables.get(SYSTEM_ATOM_TABLE).is_ok());
        assert_eq!(tables.destroy(SYSTEM_ATOM_TABLE), Err(PMERR_INVALID_HATOMTBL));
        let h = tables.create().unwrap();
        assert_ne!(h, SYSTEM_ATOM_TABLE);
        tables.get_mut(h).unwrap().add(&s("x")).unwrap();
        assert!(tables.get(SYSTEM_ATOM_TABLE).unwrap().is_empty());
        assert_eq!(tables.destroy(h), Ok(()));
        assert_eq!(tables.destroy(h), Err(PMERR_INVALID_HATOMTBL));
        assert_eq!(tables.get(h).err(), Some(PMERR_INVALID_HATOMTBL));
        while tables.create().is_ok() {}
        assert_eq!(tables.create(), Err(PMERR_HEAP_OUT_OF_MEMORY));
        assert_eq!(tables.tables.len(), MAX_ATOM_TABLES);
    }

    const TID: u32 = 1;
    const NAME: u32 = 0x0010_0000;
    const BUF: u32 = 0x0010_1000;

    fn value(r: ApiResult) -> u32 {
        match r { ApiResult::Normal(v) => v, _ => panic!("expected a value") }
    }

    fn last_error(l: &Loader) -> u32 { l.shared.pm_errors.take(TID) & 0xFFFF }

    #[test]
    fn api_calls_read_names_and_write_buffers() {
        let l = Loader::new_mock();
        l.guest_write_bytes(NAME, b"Topic\0").unwrap();
        let sys = value(l.win_query_system_atom_table());
        let a = value(l.win_add_atom(TID, sys, NAME));
        assert_eq!(a, FIRST_STRING_ATOM);
        assert_eq!(value(l.win_find_atom(TID, sys, NAME)), a);
        assert_eq!(value(l.win_query_atom_length(TID, sys, a)), 5);
        assert_eq!(value(l.win_query_atom_name(TID, sys, a, BUF, 4)), 3);
        assert_eq!(l.guest_read_bytes(BUF, 4).unwrap(), b"Top\0");
        // MAKEINTATOM(42).
        let int = (INT_ATOM_MARK << 16) | 42;
        assert_eq!(value(l.win_add_atom(TID, sys, int)), 42);
        assert_eq!(value(l.win_query_atom_name(TID, sys, 42, BUF, 16)), 3);
        assert_eq!(l.guest_read_bytes(BUF, 4).unwrap(), b"#42\0");
        // Errors.
        assert_eq!(value(l.win_add_atom(TID, 0x1234, NAME)), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_HATOMTBL);
        assert_eq!(value(l.win_add_atom(TID, sys, 0)), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_ATOM_NAME);
        assert_eq!(value(l.win_add_atom(TID, sys, INT_ATOM_MARK << 16)), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_INTEGER_ATOM);
        assert_eq!(value(l.win_query_atom_name(TID, sys, a, BUF, 0)), 0);
        assert_eq!(value(l.win_delete_atom(TID, sys, 0xC123)), 0xC123);
        assert_eq!(last_error(&l), PMERR_INVALID_ATOM);
        // An unterminated name at the end of guest memory.
        let end = l.shared.guest_mem.size() as u32 - 2;
        l.guest_write_bytes(end, b"ab").unwrap();
        assert_eq!(value(l.win_add_atom(TID, sys, end)), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_ATOM_NAME);
        // Private tables.
        let h = value(l.win_create_atom_table(TID));
        assert_eq!(value(l.win_find_atom(TID, h, NAME)), 0);
        assert_eq!(last_error(&l), PMERR_ATOM_NAME_NOT_FOUND);
        assert_eq!(value(l.win_destroy_atom_table(TID, h)), 0);
        assert_eq!(value(l.win_destroy_atom_table(TID, h)), h);
        assert_eq!(value(l.win_delete_atom(TID, sys, a)), 0);
        assert_eq!(value(l.win_query_atom_usage(TID, sys, a)), 0);
    }
}
