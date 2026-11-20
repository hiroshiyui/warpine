// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXXUTIL (ADR-0084): our own implementations, from IBM's public
// documentation, of a subset of the utility functions OS/2's REXXUTIL
// package offers, under their familiar names. As on an OS/2 system where
// SysLoadFuncs has run once, they are registered from the start
// (unverified); RxFuncAdd, RxFuncQuery and RxFuncDrop (built-in
// functions) manage the registrations, and SysDropFuncs removes them.
//
// Files go through the host, so they stay inside the drives. Functions
// REXXUTIL has that are not here (screen and keyboard ones, EAs, INI
// files, Workplace Shell objects) are Error 43, as unregistered ones are.

use std::collections::HashMap;
use std::rc::Rc;

use super::builtins::{bad, count, next_random, opt, req, whole_out, Args};
use super::expr::is_valid_symbol;
use super::interp::{Interp, OpenFor, MAX_STEM_ENTRIES, MAX_STRING, R};
use super::number::Num;
use super::program::Program;
use crate::loader::constants::{OS2_VERSION_MAJOR, OS2_VERSION_MINOR};

/// The REXXUTIL module's name, as RxFuncAdd takes it.
const MODULE: &[u8] = b"REXXUTIL";

/// The REXXUTIL functions there are.
const FUNCTIONS: [&str; 14] = [
    "SYSBOOTDRIVE", "SYSCLS", "SYSDRIVEMAP", "SYSDROPFUNCS", "SYSFILEDELETE", "SYSFILESEARCH", "SYSFILETREE",
    "SYSLOADFUNCS", "SYSMKDIR", "SYSOS2VER", "SYSRMDIR", "SYSSEARCHPATH", "SYSSLEEP", "SYSTEMPFILENAME",
];

/// The most entries SysFileTree and SysFileSearch give.
const MAX_FOUND: usize = 65536;
/// How deep SysFileTree's `S` option goes into subdirectories.
const MAX_TREE_DEPTH: usize = 32;
/// The longest SysSleep (one day), in seconds.
const MAX_SLEEP_SECONDS: u64 = 86_400;
/// How often SysSleep looks whether the program should stop.
const SLEEP_SLICE_MS: u64 = 50;
/// The most filter characters SysTempFileName replaces.
const MAX_TEMP_DIGITS: u32 = 5;
/// The OS/2 attributes SysFileTree shows, in its order: A D H R S.
const ATTRIBUTES: [(u32, u8); 5] = [(0x20, b'A'), (0x10, b'D'), (0x02, b'H'), (0x01, b'R'), (0x04, b'S')];
const ATTR_DIRECTORY: u32 = 0x10;
/// REXXUTIL's return codes: done, not enough memory, a file not opened.
const RC_OK: &[u8] = b"0";
const RC_NO_MEMORY: &[u8] = b"2";
const RC_NOT_OPENED: &[u8] = b"3";

/// Every REXXUTIL function, registered under its own name.
pub fn all_registered() -> HashMap<Vec<u8>, Vec<u8>> {
    FUNCTIONS.iter().map(|f| (f.as_bytes().to_vec(), f.as_bytes().to_vec())).collect()
}

/// Call REXXUTIL function `entry`, or `None` if there is none.
pub fn call(interp: &mut Interp, entry: &[u8], args: Args) -> Option<R<Vec<u8>>> {
    let f: fn(&mut Interp, &[u8], Args) -> R<Vec<u8>> = match entry {
        b"SYSBOOTDRIVE" => sys_boot_drive,
        b"SYSCLS" => sys_cls,
        b"SYSDRIVEMAP" => sys_drive_map,
        b"SYSDROPFUNCS" => sys_drop_funcs,
        b"SYSFILEDELETE" => |i, n, a| path_op(i, n, a, |h, p| h.file_delete(p)),
        b"SYSFILESEARCH" => sys_file_search,
        b"SYSFILETREE" => sys_file_tree,
        b"SYSLOADFUNCS" => sys_load_funcs,
        b"SYSMKDIR" => |i, n, a| path_op(i, n, a, |h, p| h.dir_make(p)),
        b"SYSOS2VER" => sys_os2_ver,
        b"SYSRMDIR" => |i, n, a| path_op(i, n, a, |h, p| h.dir_remove(p)),
        b"SYSSEARCHPATH" => sys_search_path,
        b"SYSSLEEP" => sys_sleep,
        b"SYSTEMPFILENAME" => sys_temp_file_name,
        _ => return None,
    };
    Some(f(interp, entry, args))
}

// ── Registration (built-in functions) ────────────────────────────────────

/// RxFuncAdd(name, module, entry): 0 when registered; 1 for a module or
/// function this REXX does not have (unverified).
pub(super) fn rx_func_add(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 3, 3)?;
    let entry = req(a, 2).to_ascii_uppercase();
    if !req(a, 1).eq_ignore_ascii_case(MODULE) || !FUNCTIONS.iter().any(|f| f.as_bytes() == entry) {
        return Ok(b"1".to_vec());
    }
    i.functions.insert(req(a, 0).to_ascii_uppercase(), entry);
    Ok(b"0".to_vec())
}

/// RxFuncQuery(name): 0 when registered, else 1.
pub(super) fn rx_func_query(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    Ok(if i.functions.contains_key(&req(a, 0).to_ascii_uppercase()) { b"0".to_vec() } else { b"1".to_vec() })
}

/// RxFuncDrop(name): 0 when it was registered, else 1.
pub(super) fn rx_func_drop(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    Ok(if i.functions.remove(&req(a, 0).to_ascii_uppercase()).is_some() { b"0".to_vec() } else { b"1".to_vec() })
}

fn sys_load_funcs(i: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    i.functions.extend(all_registered());
    Ok(Vec::new())
}

fn sys_drop_funcs(i: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    i.functions.retain(|_, entry| !FUNCTIONS.iter().any(|f| f.as_bytes() == entry.as_slice()));
    Ok(Vec::new())
}

// ── System ───────────────────────────────────────────────────────────────

/// SysOS2Ver(): the OS/2 version Warpine reports, as `2.45`.
fn sys_os2_ver(_: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    Ok(format!("{}.{:02}", OS2_VERSION_MAJOR / 10, OS2_VERSION_MINOR).into_bytes())
}

/// SysBootDrive(): `C:`, drive C being the one Warpine's system is on.
fn sys_boot_drive(_: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    Ok(b"C:".to_vec())
}

fn sys_cls(i: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    i.host.clear_screen();
    Ok(Vec::new())
}

/// SysSleep(seconds): wait, waking early when the program must stop.
/// Fractions of a second are accepted (unverified).
fn sys_sleep(i: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    let secs = Num::parse(req(a, 0)).filter(|s| !s.is_negative()).ok_or_else(|| bad(n, "a number of seconds was needed"))?;
    let ms = secs.mul(&Num::from_i64(1000), 18).ok().and_then(|m| m.trunc().to_whole(18)).unwrap_or(i64::MAX);
    let mut left = (ms.max(0) as u64).min(MAX_SLEEP_SECONDS * 1000);
    while left > 0 && !i.host.interrupted() {
        let slice = left.min(SLEEP_SLICE_MS);
        std::thread::sleep(std::time::Duration::from_millis(slice));
        left -= slice;
    }
    Ok(Vec::new())
}

/// SysDriveMap([drive], [USED | FREE | LOCAL | REMOTE | DETACHED]): the
/// drives from `drive` (C: by default), as `C: D:`. Warpine's drives are
/// all local.
fn sys_drive_map(i: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 0, 2)?;
    let start = match opt(a, 0) {
        Some([c, rest @ ..]) if c.is_ascii_alphabetic() && matches!(rest, [] | [b':']) => c.to_ascii_uppercase(),
        Some(_) => return Err(bad(n, "a drive letter was needed")),
        None => b'C',
    };
    let used = i.host.drives();
    let wanted: Vec<u8> = match opt(a, 1).map(<[u8]>::to_ascii_uppercase).as_deref() {
        None | Some(b"USED") | Some(b"LOCAL") => used,
        Some(b"FREE") => (b'A'..=b'Z').filter(|l| !used.contains(l)).collect(),
        Some(b"REMOTE") | Some(b"DETACHED") => Vec::new(),
        Some(_) => return Err(bad(n, "an unknown option")),
    };
    let letters: Vec<String> = wanted.into_iter().filter(|&l| l >= start).map(|l| format!("{}:", l as char)).collect();
    Ok(letters.join(" ").into_bytes())
}

// ── Files ────────────────────────────────────────────────────────────────

/// SysFileDelete, SysMkDir, SysRmDir: the OS/2 error code (0 when done).
fn path_op(i: &mut Interp, n: &[u8], a: Args, op: fn(&mut dyn super::interp::Host, &[u8]) -> u32) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    Ok(whole_out(op(i.host, req(a, 0).trim_ascii()) as i64))
}

/// A stem argument: its name in upper case, ending with a period.
fn stem_arg(n: &[u8], a: Args, k: usize) -> R<Vec<u8>> {
    let mut stem = req(a, k).trim_ascii().to_ascii_uppercase();
    if !stem.ends_with(b".") { stem.push(b'.'); }
    if !is_valid_symbol(&stem) || stem.first().is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
        return Err(bad(n, "a stem name was needed"));
    }
    Ok(stem)
}

/// Set `stem.1` … `stem.n` to `items` and `stem.0` to n.
fn fill_stem(i: &mut Interp, stem: &[u8], items: Vec<Vec<u8>>) -> R<()> {
    let total = items.len();
    for (k, item) in items.into_iter().enumerate() {
        i.set_compound(&[stem, (k + 1).to_string().as_bytes()].concat(), stem.len(), item)?;
    }
    i.set_compound(&[stem, b"0"].concat(), stem.len(), whole_out(total as i64))
}

/// `path` split after its directory part (`C:\DIR\`, `C:`, or nothing).
fn split_dir(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|&c| matches!(c, b'\\' | b'/' | b':')) {
        Some(k) => (&path[..=k], &path[k + 1..]),
        None => (&path[..0], path),
    }
}

/// The `ADHRS` string of OS/2 attributes.
fn attribute_letters(attributes: u32) -> Vec<u8> {
    ATTRIBUTES.iter().map(|&(bit, letter)| if attributes & bit != 0 { letter } else { b'-' }).collect()
}

/// Whether `attributes` fit a SysFileTree mask (`*` any, `+` set, `-`
/// clear, for A D H R S).
fn mask_fits(mask: &[u8], attributes: u32) -> bool {
    mask.iter().zip(ATTRIBUTES).all(|(&m, (bit, _))| match m {
        b'+' => attributes & bit != 0,
        b'-' => attributes & bit == 0,
        _ => true,
    })
}

/// What SysFileTree's options ask for.
struct TreeOptions {
    files: bool,
    dirs: bool,
    recurse: bool,
    short_time: bool,
    names_only: bool,
    mask: Vec<u8>,
}

/// SysFileTree(spec, stem, [options], [tattrib], [nattrib]): the files
/// and directories `spec` matches, one per entry of `stem`: date, time,
/// size, attributes and full name (options F files, D directories, B
/// both, S subdirectories too, T time as YY/MM/DD/HH/MM, O names only).
/// Changing attributes (`nattrib`) is not supported (Error 40). Returns 0,
/// or 2 when more was found than it gives.
fn sys_file_tree(i: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 2, 5)?;
    let stem = stem_arg(n, a, 1)?;
    let options = opt(a, 2).unwrap_or(b"B").to_ascii_uppercase();
    if let Some(c) = options.iter().find(|c| !b"FDBSTO".contains(c)) {
        return Err(bad(n, &format!("an unknown option {}", *c as char)));
    }
    let only_files = options.contains(&b'F');
    let only_dirs = options.contains(&b'D');
    let mask = opt(a, 3).unwrap_or(b"*****").to_vec();
    if mask.len() != ATTRIBUTES.len() || mask.iter().any(|c| !b"*+-".contains(c)) {
        return Err(bad(n, "an attribute mask is five of * + -"));
    }
    if opt(a, 4).is_some_and(|m| m != b"*****") { return Err(bad(n, "changing attributes is not supported")); }
    let opts = TreeOptions {
        files: !only_dirs || only_files, dirs: !only_files || only_dirs, recurse: options.contains(&b'S'),
        short_time: options.contains(&b'T'), names_only: options.contains(&b'O'), mask,
    };
    let spec = req(a, 0).trim_ascii();
    let Some(full) = i.host.full_name(if spec.is_empty() { b"*" } else { spec }) else {
        fill_stem(i, &stem, Vec::new())?;
        return Ok(RC_OK.to_vec());
    };
    let (dir, pattern) = split_dir(&full);
    let (dir, pattern) = (dir.to_vec(), if pattern.is_empty() { b"*".to_vec() } else { pattern.to_vec() });
    let mut found = Vec::new();
    let complete = walk(i, &dir, &pattern, &opts, 0, &mut found);
    fill_stem(i, &stem, found)?;
    Ok(if complete { RC_OK } else { RC_NO_MEMORY }.to_vec())
}

/// SysFileTree's search of `dir` (and, with `S`, its subdirectories).
/// Returns false when it stopped at the limit.
fn walk(i: &mut Interp, dir: &[u8], pattern: &[u8], opts: &TreeOptions, depth: usize, found: &mut Vec<Vec<u8>>) -> bool {
    let room = MAX_FOUND.min(MAX_STEM_ENTRIES / 2).saturating_sub(found.len());
    let entries = i.host.file_find(&[dir, pattern].concat(), room + 1).unwrap_or_default();
    for e in &entries {
        let is_dir = e.attributes & ATTR_DIRECTORY != 0;
        if (is_dir && !opts.dirs) || (!is_dir && !opts.files) || !mask_fits(&opts.mask, e.attributes) { continue; }
        if found.len() >= room.min(MAX_FOUND) { return false; }
        found.push(tree_line(dir, e, opts));
    }
    if opts.recurse && depth < MAX_TREE_DEPTH {
        let subdirs = i.host.file_find(&[dir, b"*"].concat(), MAX_FOUND).unwrap_or_default();
        for d in subdirs.iter().filter(|d| d.attributes & ATTR_DIRECTORY != 0) {
            let sub = [dir, &d.name, b"\\"].concat();
            if !walk(i, &sub, pattern, opts, depth + 1, found) { return false; }
        }
    }
    true
}

/// One line of SysFileTree's result (layout unverified).
fn tree_line(dir: &[u8], e: &super::interp::FileInfo, opts: &TreeOptions) -> Vec<u8> {
    let name = [dir, &e.name].concat();
    if opts.names_only { return name; }
    let (d, t) = (e.date, e.time);
    let (year, month, day) = (1980 + (d >> 9) as u32, ((d >> 5) & 0x0F) as u32, (d & 0x1F) as u32);
    let (hour, minute) = ((t >> 11) as u32, ((t >> 5) & 0x3F) as u32);
    let mut line = if opts.short_time {
        format!("{:02}/{month:02}/{day:02}/{hour:02}/{minute:02}", year % 100)
    } else {
        let (h12, ap) = match hour { 0 => (12, 'a'), 1..=11 => (hour, 'a'), 12 => (12, 'p'), _ => (hour - 12, 'p') };
        format!("{month:2}/{day:02}/{:02}  {h12:2}:{minute:02}{ap}", year % 100)
    }.into_bytes();
    line.extend_from_slice(format!("  {:10}  ", e.size).as_bytes());
    line.extend(attribute_letters(e.attributes));
    line.extend_from_slice(b"  ");
    line.extend(name);
    line
}

/// SysFileSearch(target, file, stem, [options]): the lines of `file`
/// containing `target` (options C: case counts, N: each line after its
/// number and a blank, layout unverified). Returns 0, 2 when more was
/// found than it gives, or 3 when the file could not be opened.
fn sys_file_search(i: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 3, 4)?;
    let stem = stem_arg(n, a, 2)?;
    let options = opt(a, 3).unwrap_or_default().to_ascii_uppercase();
    if options.iter().any(|c| !b"CN".contains(c)) { return Err(bad(n, "an unknown option")); }
    let (case, numbers) = (options.contains(&b'C'), options.contains(&b'N'));
    let target = if case { req(a, 0).to_vec() } else { req(a, 0).to_ascii_uppercase() };
    let Some(full) = i.host.full_name(req(a, 1).trim_ascii()) else { return Ok(RC_NOT_OPENED.to_vec()) };
    let Ok(h) = i.host.file_open(&full, OpenFor::Read) else { return Ok(RC_NOT_OPENED.to_vec()) };
    let mut found = Vec::new();
    let mut complete = true;
    let (mut pos, mut number, mut partial) = (0u64, 0u64, Vec::new());
    let mut buf = vec![0u8; 64 * 1024];
    let check = |line: &[u8], number: u64, found: &mut Vec<Vec<u8>>| -> bool {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let hay = if case { line.to_vec() } else { line.to_ascii_uppercase() };
        if target.is_empty() || hay.windows(target.len()).any(|w| w == target) {
            if found.len() >= MAX_FOUND { return false; }
            found.push(if numbers { [number.to_string().as_bytes(), b" ", line].concat() } else { line.to_vec() });
        }
        true
    };
    while let Ok(got) = i.host.file_read(h, pos, &mut buf) {
        if got == 0 { break; }
        pos += got as u64;
        for &b in &buf[..got] {
            if b == b'\n' {
                number += 1;
                if !check(&partial, number, &mut found) { complete = false; break; }
                partial.clear();
            } else if partial.len() < MAX_STRING {
                partial.push(b);
            }
        }
        if !complete { break; }
    }
    if complete && !partial.is_empty() { number += 1; complete = check(&partial, number, &mut found); }
    i.host.file_close(h);
    fill_stem(i, &stem, found)?;
    Ok(if complete { RC_OK } else { RC_NO_MEMORY }.to_vec())
}

/// SysSearchPath(variable, file): the full name of `file` found in the
/// current directory (first, unverified) or a directory the environment
/// variable lists, or an empty string.
fn sys_search_path(i: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 2, 2)?;
    let file = req(a, 1).trim_ascii();
    let path = i.host.get_env(req(a, 0)).unwrap_or_default();
    let mut dirs: Vec<Vec<u8>> = vec![Vec::new()];
    dirs.extend(path.split(|&b| b == b';').map(<[u8]>::trim_ascii).filter(|d| !d.is_empty()).map(|d| {
        if d.ends_with(b"\\") || d.ends_with(b"/") || d.ends_with(b":") { d.to_vec() } else { [d, b"\\"].concat() }
    }));
    for dir in dirs {
        if let Some(info) = i.host.file_info(&[&dir[..], file].concat()) && info.attributes & ATTR_DIRECTORY == 0 {
            return Ok(info.name);
        }
    }
    Ok(Vec::new())
}

/// SysTempFileName(template, [filter]): `template` with its filter
/// characters (`?` by default, at most five) replaced by digits so that
/// it names no file yet; an empty string when every name is taken.
fn sys_temp_file_name(i: &mut Interp, n: &[u8], a: Args) -> R<Vec<u8>> {
    count(n, a, 1, 2)?;
    let template = req(a, 0).trim_ascii().to_vec();
    let filter = match opt(a, 1) {
        None => b'?',
        Some([c]) => *c,
        Some(_) => return Err(bad(n, "the filter must be one character")),
    };
    let places = template.iter().filter(|&&c| c == filter).count() as u32;
    if places > MAX_TEMP_DIGITS { return Err(bad(n, "at most five filter characters")); }
    let combinations = 10u64.pow(places);
    let first = next_random(i) % combinations;
    for k in 0..combinations {
        let mut number = (first + k) % combinations;
        let mut name = template.clone();
        for c in name.iter_mut().rev().filter(|c| **c == filter) {
            *c = b'0' + (number % 10) as u8;
            number /= 10;
        }
        if i.host.file_info(&name).is_none() { return Ok(name); }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_and_masks() {
        assert_eq!(attribute_letters(0x21), b"A--R-");
        assert_eq!(attribute_letters(0x10), b"-D---");
        assert!(mask_fits(b"*+***", 0x10));
        assert!(!mask_fits(b"*-***", 0x10));
        assert!(mask_fits(b"+***-", 0x20));
    }

    #[test]
    fn tree_lines_are_laid_out() {
        let e = super::super::interp::FileInfo {
            name: b"CONFIG.SYS".to_vec(), size: 1234,
            date: ((2026 - 1980) << 9 | 11 << 5 | 20) as u16, time: (15 << 11 | 7 << 5) as u16, attributes: 0x20,
        };
        let opts = |short_time, names_only| TreeOptions { files: true, dirs: true, recurse: false, short_time, names_only, mask: b"*****".to_vec() };
        assert_eq!(String::from_utf8(tree_line(b"C:\\", &e, &opts(false, false))).unwrap(), "11/20/26   3:07p        1234  A----  C:\\CONFIG.SYS");
        assert_eq!(String::from_utf8(tree_line(b"C:\\", &e, &opts(true, false))).unwrap(), "26/11/20/15/07        1234  A----  C:\\CONFIG.SYS");
        assert_eq!(tree_line(b"C:\\", &e, &opts(false, true)), b"C:\\CONFIG.SYS");
        assert_eq!(split_dir(b"C:\\A\\*.TXT"), (&b"C:\\A\\"[..], &b"*.TXT"[..]));
    }
}
