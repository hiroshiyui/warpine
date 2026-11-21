// SPDX-License-Identifier: GPL-3.0-or-later
//
// MSG.DLL message helpers, after the IBM Control Program Programming
// Reference: `DosGetMessage` (MSG.6), `DosInsertMessage` (MSG.4) and
// `DosQueryMessageCP` (MSG.8). `DosPutMessage` (MSG.5) is in `stubs.rs`.
//
// `DosGetMessage` reads Warpine's own message files (`msgfile.rs`,
// ADR-0105), from the program's bound message segment or from a file.
//
// The 32-bit ordinals are those of the Open Watcom import library
// `os2386.lib` (ADR-0006): MSG.4 `DosInsertMessage`, MSG.5
// `DosPutMessage`, MSG.6 `DOS32TRUEGETMESSAGE`, MSG.8
// `DOS32IQUERYMESSAGECP`; MSG.1–3 and MSG.7 are the 16-bit entry points.
// The library's own `DosGetMessage` (object `msgseg32`) calls MSG.6 with
// the address of the program's message segment as an extra first argument,
// which Warpine skips and removes from the stack on return (its caller
// removes only DosGetMessage's seven arguments; `msg_callee_pop`). The library has no `DosQueryMessageCP` import; a
// program reaches MSG.8 with the four arguments of `<bsedos.h>`'s
// `DosQueryMessageCP` (anything after them is ignored).
//
// `DosInsertMessage` replaces `%1` … `%9` in a message by the strings of
// the insertion table. `DosQueryMessageCP` reports, for a file that
// exists, the process codepage as its one codepage.
//
// Unverified (the Reference is silent): a `%n` without a table string, and
// `%0` / `%%`, are left as they are; a message too long for the buffer is
// cut to it with `ERROR_MR_MSG_TOO_LONG`; the `DosQueryMessageCP` buffer
// holds the number of codepages, the codepages, then the language family
// and dialect (0, 0), all USHORTs, `ERROR_BUFFER_OVERFLOW` when it does
// not fit; a missing file is `ERROR_MR_UN_ACC_MSGF`.

use super::constants::*;
use super::mutex_ext::MutexExt;

/// `ERROR_MR_MSG_TOO_LONG` (`<bseerr.h>`).
pub const ERROR_MR_MSG_TOO_LONG: u32 = 316;
/// `ERROR_MR_MID_NOT_FOUND` (`<bseerr.h>`).
pub const ERROR_MR_MID_NOT_FOUND: u32 = 317;
/// `ERROR_MR_UN_ACC_MSGF` (`<bseerr.h>`).
pub const ERROR_MR_UN_ACC_MSGF: u32 = 318;
/// `ERROR_MR_INV_MSGF_FORMAT` (`<bseerr.h>`).
pub const ERROR_MR_INV_MSGF_FORMAT: u32 = 319;
/// `ERROR_MR_INV_IVCOUNT` (`<bseerr.h>`).
pub const ERROR_MR_INV_IVCOUNT: u32 = 320;

/// Most insertion strings (`%1` … `%9`).
pub const MAX_INSERTIONS: u32 = 9;
/// Longest insertion string taken.
const MAX_INSERTION: usize = 4096;
/// Longest message taken.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// Longest message-file name.
const MAX_MSG_FILE_NAME: usize = 260;
/// Language family and dialect reported by `DosQueryMessageCP`.
const LANGUAGE_UNKNOWN: u16 = 0;

/// MSG ordinal of `DOS32TRUEGETMESSAGE` (the library's `DosGetMessage`).
pub const MSG_TRUE_GET_MESSAGE: u32 = 6;
/// Bytes of the message-segment argument `DosGetMessage`'s stub adds.
const MSG_SEGMENT_ARG_BYTES: u64 = 4;

/// Argument bytes MSG ordinal `ord` removes from the stack on return
/// (beyond the `_System` caller's own cleanup): the stub of Open Watcom's
/// `os2386.lib` (`msgseg32`) pushes the message segment under the return
/// address and its caller removes only `DosGetMessage`'s seven arguments,
/// so MSG.6 removes the segment itself (deduced from the stub's code;
/// unverified against OS/2).
pub fn msg_callee_pop(ord: u32) -> u64 {
    if ord == MSG_TRUE_GET_MESSAGE { MSG_SEGMENT_ARG_BYTES } else { 0 }
}

/// `msg` with `%1` … `%9` replaced by `table[n - 1]`.
pub fn insert_strings(msg: &[u8], table: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(msg.len());
    let mut i = 0;
    while i < msg.len() {
        if msg[i] == b'%'
            && let Some(&d) = msg.get(i + 1)
            && (b'1'..=b'9').contains(&d)
            && let Some(s) = table.get(usize::from(d - b'1'))
        {
            out.extend_from_slice(s);
            i += 2;
            continue;
        }
        out.push(msg[i]);
        i += 1;
    }
    out
}

/// The `DosQueryMessageCP` buffer for `codepages`.
pub fn message_cp_list(codepages: &[u16]) -> Vec<u8> {
    let mut words = vec![codepages.len() as u16];
    words.extend_from_slice(codepages);
    words.extend_from_slice(&[LANGUAGE_UNKNOWN, LANGUAGE_UNKNOWN]);
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// Where `DosGetMessage` found a message, or why it did not.
#[derive(Debug, PartialEq, Eq)]
enum Lookup {
    Found { prefix: Vec<u8>, text: Vec<u8> },
    NoFile,
    BadFile,
    NoMessage,
}

/// The text `DosGetMessage` gives instead of a message, with the error it
/// returns (Warpine's wording; ADR-0105).
fn failure_text(lookup: &Lookup, file: &[u8], number: u32) -> Option<(u32, Vec<u8>)> {
    let name = String::from_utf8_lossy(file);
    let (rc, text) = match lookup {
        Lookup::Found { .. } => return None,
        Lookup::NoFile => (ERROR_MR_UN_ACC_MSGF, format!("SYS0318: The message file {name} was not found.\r\n")),
        Lookup::BadFile => (ERROR_MR_INV_MSGF_FORMAT, format!("SYS0319: The message file {name} is not in Warpine's format.\r\n")),
        Lookup::NoMessage => (ERROR_MR_MID_NOT_FOUND, format!("SYS0317: Message {number:04} was not found in {name}.\r\n")),
    };
    Some((rc, text.into_bytes()))
}

/// Message `number` of a parsed file, with its id prefix when it shows one.
fn lookup_in(file: Result<super::msgfile::MsgFile, super::msgfile::FormatError>, number: u32) -> Lookup {
    let Ok(file) = file else { return Lookup::BadFile };
    match file.get(number) {
        Some(m) if m.kind != super::msgfile::MsgType::Unused => Lookup::Found {
            prefix: if m.kind.shows_id() { file.id_prefix(number) } else { Vec::new() },
            text: m.text.clone(),
        },
        _ => Lookup::NoMessage,
    }
}

impl super::Loader {
    /// DosGetMessage(pTable, cTable, pBuf, cbBuf, msgNumber, pszFile,
    /// pcbMsg), with the program's bound message segment (0: none) the
    /// library stub passes first (ADR-0105).
    ///
    /// The message comes from the segment's file of that name, else from
    /// the file: a name with a drive or directory as it is; a plain name
    /// in the current directory, then along `DPATH`. Errors and warnings
    /// get their id (`ABC0001: `); `%1` … `%9` take the table's strings.
    /// When there is no such file or message, the buffer gets a text
    /// saying so and the call returns the error.
    #[allow(clippy::too_many_arguments)] // the segment and the API's seven arguments
    pub fn dos_get_message(&self, segment: u32, p_table: u32, c_table: u32, p_buf: u32, cb_buf: u32,
                           msg_num: u32, psz_file: u32, pcb_msg: u32) -> u32 {
        if c_table > MAX_INSERTIONS { return ERROR_MR_INV_IVCOUNT; }
        if pcb_msg == 0 || (cb_buf != 0 && p_buf == 0) { return ERROR_INVALID_PARAMETER; }
        let Some(file) = self.guest_cstr(psz_file, MAX_MSG_FILE_NAME) else { return ERROR_INVALID_PARAMETER };
        let mut table = Vec::new();
        for n in 0..c_table {
            let Some(s) = p_table.checked_add(4 * n).and_then(|p| self.guest_read::<u32>(p))
                .and_then(|p| self.guest_cstr(p, MAX_INSERTION))
                else { return ERROR_INVALID_PARAMETER };
            table.push(s);
        }
        let lookup = match self.bound_message_file(segment, &file) {
            Some(bytes) => lookup_in(super::msgfile::parse(&bytes), msg_num),
            None => match self.message_file_on_drive(&file) {
                Some(bytes) => lookup_in(super::msgfile::parse(&bytes), msg_num),
                None => Lookup::NoFile,
            },
        };
        log::debug!("  DosGetMessage({:?}, {msg_num}): {lookup:?}", String::from_utf8_lossy(&file));
        let (mut rc, mut out) = match failure_text(&lookup, &file, msg_num) {
            Some((rc, text)) => (rc, text),
            None => {
                let Lookup::Found { prefix, text } = lookup else { unreachable!("failure_text covers the rest") };
                let mut out = prefix;
                out.extend_from_slice(&insert_strings(&text, &table));
                (NO_ERROR, out)
            }
        };
        if out.len() > cb_buf as usize {
            out.truncate(cb_buf as usize);
            if rc == NO_ERROR { rc = ERROR_MR_MSG_TOO_LONG; }
        }
        if !out.is_empty() && self.guest_write_bytes(p_buf, &out).is_none() { return ERROR_INVALID_PARAMETER; }
        if self.guest_write::<u32>(pcb_msg, out.len() as u32).is_none() { return ERROR_INVALID_PARAMETER; }
        rc
    }

    /// The file named like `file` in the program's bound message segment
    /// at `segment` (bounded reads; anything that is not Warpine's segment
    /// is ignored).
    fn bound_message_file(&self, segment: u32, file: &[u8]) -> Option<Vec<u8>> {
        use super::msgfile::{segment_entries, segment_head_size, segment_name, MAX_FILE, MAX_NAME};
        if segment == 0 { return None; }
        let count = self.guest_read::<u32>(segment.checked_add(8)?)?;
        let head = self.guest_read_bytes(segment, segment_head_size(count))?;
        let want = segment_name(&String::from_utf8_lossy(file));
        segment_entries(&head)?.into_iter().find_map(|(name_off, off, len)| {
            let name = self.guest_cstr(segment.checked_add(name_off)?, MAX_NAME + 1)?;
            if !name.eq_ignore_ascii_case(want.as_bytes()) || len as usize > MAX_FILE { return None; }
            self.guest_read_bytes(segment.checked_add(off)?, len as usize)
        })
    }

    /// A message file's bytes from the drives: `file` as it is when it
    /// names a drive or directory, else in the current directory, then in
    /// each `DPATH` directory.
    fn message_file_on_drive(&self, file: &[u8]) -> Option<Vec<u8>> {
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let name = super::codepage::cp_decode(file, cp);
        if name.is_empty() || name.chars().any(char::is_control) { return None; }
        let mut candidates = vec![name.clone()];
        if !name.contains(['\\', '/', ':']) {
            let dpath = self.environment_value("DPATH").unwrap_or_default();
            candidates.extend(dpath.split(';').map(str::trim).filter(|d| !d.is_empty())
                .map(|d| format!("{}\\{name}", d.trim_end_matches(['\\', '/']))));
        }
        candidates.into_iter().find_map(|c| {
            let host = self.shared.drive_mgr.lock_or_recover().resolve_to_host_path(&c)?;
            let meta = std::fs::metadata(&host).ok()?;
            (meta.is_file() && meta.len() <= super::msgfile::MAX_FILE as u64).then(|| std::fs::read(&host).ok()).flatten()
        })
    }

    /// The value of `name` in the process environment (`pib_pchenv`).
    fn environment_value(&self, name: &str) -> Option<String> {
        /// Bound on the environment walked.
        const MAX_ENVIRONMENT: u32 = 64 * 1024;
        let start = self.guest_read::<u32>(PIB_BASE + PIB_PCHENV_OFFSET)?;
        let prefix = format!("{}=", name.to_ascii_uppercase());
        let mut at = start;
        while at.checked_sub(start)? < MAX_ENVIRONMENT {
            let entry = self.guest_cstr(at, MAX_ENVIRONMENT as usize)?;
            if entry.is_empty() { return None; }
            let text = String::from_utf8_lossy(&entry);
            if text.to_ascii_uppercase().starts_with(&prefix) { return Some(text[prefix.len()..].to_string()); }
            at = at.checked_add(entry.len() as u32 + 1)?;
        }
        None
    }

    /// DosInsertMessage(pTable, cTable, pszMsg, cbMsg, pBuf, cbBuf, pcbMsg).
    #[allow(clippy::too_many_arguments)] // the API's seven arguments
    pub fn dos_insert_message(&self, p_table: u32, c_table: u32, p_msg: u32, cb_msg: u32,
                              p_buf: u32, cb_buf: u32, pcb_msg: u32) -> u32 {
        if c_table > MAX_INSERTIONS { return ERROR_MR_INV_IVCOUNT; }
        if cb_msg as usize > MAX_MESSAGE { return ERROR_MR_MSG_TOO_LONG; }
        if pcb_msg == 0 || (cb_buf != 0 && p_buf == 0) { return ERROR_INVALID_PARAMETER; }
        let Some(msg) = (if cb_msg == 0 { Some(Vec::new()) } else { self.guest_read_bytes(p_msg, cb_msg as usize) })
            else { return ERROR_INVALID_PARAMETER };
        let mut table = Vec::new();
        for n in 0..c_table {
            let Some(s) = p_table.checked_add(4 * n).and_then(|p| self.guest_read::<u32>(p))
                .and_then(|p| self.guest_cstr(p, MAX_INSERTION))
                else { return ERROR_INVALID_PARAMETER };
            table.push(s);
        }
        let mut out = insert_strings(&msg, &table);
        let rc = if out.len() > cb_buf as usize {
            out.truncate(cb_buf as usize);
            ERROR_MR_MSG_TOO_LONG
        } else {
            NO_ERROR
        };
        if !out.is_empty() && self.guest_write_bytes(p_buf, &out).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        if self.guest_write::<u32>(pcb_msg, out.len() as u32).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        rc
    }

    /// DosQueryMessageCP(pb, cb, pszFilename, pcbBuf).
    pub fn dos_query_message_cp(&self, pb: u32, cb: u32, psz_file: u32, pcb_buf: u32) -> u32 {
        let Some(name) = self.guest_cstr(psz_file, MAX_MSG_FILE_NAME) else { return ERROR_INVALID_PARAMETER };
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let name = super::codepage::cp_decode(&name, cp);
        let exists = self.shared.drive_mgr.lock_or_recover().resolve_to_host_path(&name)
            .is_some_and(|p| p.is_file());
        if !exists { return ERROR_MR_UN_ACC_MSGF; }
        let list = message_cp_list(&[u16::try_from(cp).unwrap_or(0)]);
        if list.len() > cb as usize { return ERROR_BUFFER_OVERFLOW; }
        if pb == 0 || self.guest_write_bytes(pb, &list).is_none() { return ERROR_INVALID_PARAMETER; }
        if pcb_buf == 0 || self.guest_write::<u32>(pcb_buf, list.len() as u32).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        NO_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;

    fn v(s: &str) -> Vec<u8> { s.as_bytes().to_vec() }

    #[test]
    fn insertion_replaces_numbered_markers() {
        let t = [v("one"), v("two")];
        assert_eq!(insert_strings(b"a %1 b %2 c %1", &t), v("a one b two c one"));
        assert_eq!(insert_strings(b"%3 %0 %% %", &t), v("%3 %0 %% %"));
        assert_eq!(insert_strings(b"%2%1", &t), v("twoone"));
        assert_eq!(insert_strings(b"", &t), v(""));
    }

    #[test]
    fn only_get_message_removes_its_segment_argument() {
        assert_eq!(msg_callee_pop(MSG_TRUE_GET_MESSAGE), 4);
        for ord in [4, 5, 8] { assert_eq!(msg_callee_pop(ord), 0); }
    }

    #[test]
    fn codepage_lists_are_words() {
        assert_eq!(message_cp_list(&[850]), vec![1, 0, 0x52, 0x03, 0, 0, 0, 0]);
    }

    const TABLE: u32 = 0x0010_0000;
    const S1: u32 = 0x0010_0100;
    const MSG: u32 = 0x0010_0200;
    const BUF: u32 = 0x0010_0300;
    const PCB: u32 = 0x0010_0400;

    #[test]
    fn dos_insert_message_fills_the_buffer() {
        let l = Loader::new_mock();
        l.guest_write_bytes(S1, b"World\0").unwrap();
        l.guest_write::<u32>(TABLE, S1).unwrap();
        l.guest_write_bytes(MSG, b"Hello %1!").unwrap();
        assert_eq!(l.dos_insert_message(TABLE, 1, MSG, 9, BUF, 64, PCB), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PCB), Some(12));
        assert_eq!(l.guest_read_bytes(BUF, 12).unwrap(), b"Hello World!");
        assert_eq!(l.dos_insert_message(TABLE, 1, MSG, 9, BUF, 5, PCB), ERROR_MR_MSG_TOO_LONG);
        assert_eq!(l.guest_read::<u32>(PCB), Some(5));
        assert_eq!(l.dos_insert_message(TABLE, 10, MSG, 9, BUF, 64, PCB), ERROR_MR_INV_IVCOUNT);
        assert_eq!(l.dos_insert_message(TABLE, 1, MSG, 9, BUF, 64, 0), ERROR_INVALID_PARAMETER);
        // A table entry pointing outside guest memory.
        l.guest_write::<u32>(TABLE + 4, 0xFFFF_FFF0).unwrap();
        assert_eq!(l.dos_insert_message(TABLE, 2, MSG, 9, BUF, 64, PCB), ERROR_INVALID_PARAMETER);
        let end = l.shared.guest_mem.size() as u32 - 4;
        assert_eq!(l.dos_insert_message(TABLE, 0, end, 64, BUF, 64, PCB), ERROR_INVALID_PARAMETER);
    }

    const SEG: u32 = 0x0011_0000;
    const NAME: u32 = 0x0010_0500;

    /// A loader with a bound segment holding APP.MSG at `SEG`.
    fn with_segment() -> Loader {
        use crate::loader::msgfile::{build_segment, compile, write};
        let l = Loader::new_mock();
        let app = write(&compile(b"APP\nAPP0001E: File %1 is missing.\nAPP0002P: Go?\nAPP0003?: \n", 0).unwrap()).unwrap();
        l.guest_write_bytes(SEG, &build_segment(&[("app.msg".into(), app)]).unwrap()).unwrap();
        l.guest_write_bytes(S1, b"a.txt\0").unwrap();
        l.guest_write::<u32>(TABLE, S1).unwrap();
        l
    }

    fn get(l: &Loader, count: u32, size: u32, number: u32, file: &[u8]) -> (u32, Vec<u8>) {
        l.guest_write_bytes(NAME, file).unwrap();
        l.guest_write_bytes(NAME + file.len() as u32, b"\0").unwrap();
        let rc = l.dos_get_message(SEG, TABLE, count, BUF, size, number, NAME, PCB);
        let n = l.guest_read::<u32>(PCB).unwrap();
        (rc, l.guest_read_bytes(BUF, n as usize).unwrap_or_default())
    }

    #[test]
    fn dos_get_message_reads_the_bound_segment() {
        let l = with_segment();
        assert_eq!(get(&l, 1, 64, 1, b"APP.MSG"), (NO_ERROR, v("APP0001: File a.txt is missing.\r\n")));
        assert_eq!(get(&l, 0, 64, 2, b"C:\\X\\app.msg"), (NO_ERROR, v("Go?")), "found by its name, in any case");
        assert_eq!(get(&l, 1, 12, 1, b"APP.MSG"), (ERROR_MR_MSG_TOO_LONG, v("APP0001: Fil")));
        let (rc, text) = get(&l, 0, 128, 3, b"APP.MSG");
        assert_eq!(rc, ERROR_MR_MID_NOT_FOUND, "an unused number");
        assert!(String::from_utf8_lossy(&text).contains("0003"));
        assert_eq!(get(&l, 0, 128, 9, b"APP.MSG").0, ERROR_MR_MID_NOT_FOUND);
        let (rc, text) = get(&l, 0, 128, 1, b"OTHER.MSG");
        assert_eq!(rc, ERROR_MR_UN_ACC_MSGF);
        assert!(String::from_utf8_lossy(&text).contains("OTHER.MSG"));
    }

    #[test]
    fn dos_get_message_checks_its_arguments() {
        let l = with_segment();
        l.guest_write_bytes(NAME, b"APP.MSG\0").unwrap();
        assert_eq!(l.dos_get_message(SEG, TABLE, 10, BUF, 64, 1, NAME, PCB), ERROR_MR_INV_IVCOUNT);
        assert_eq!(l.dos_get_message(SEG, TABLE, 1, BUF, 64, 1, NAME, 0), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_get_message(SEG, TABLE, 1, 0, 64, 1, NAME, PCB), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_get_message(SEG, TABLE, 1, BUF, 64, 1, 0, PCB), ERROR_INVALID_PARAMETER);
        l.guest_write::<u32>(TABLE, 0xFFFF_FFF0).unwrap();
        assert_eq!(l.dos_get_message(SEG, TABLE, 1, BUF, 64, 1, NAME, PCB), ERROR_INVALID_PARAMETER);
    }

    #[test]
    fn foreign_or_damaged_segments_are_ignored() {
        let l = with_segment();
        // Another compiler's segment: not Warpine's magic.
        l.guest_write_bytes(SEG, b"\xFFMSGSEG").unwrap();
        assert_eq!(get(&l, 1, 64, 1, b"APP.MSG").0, ERROR_MR_UN_ACC_MSGF);
        // Warpine's magic with entries pointing outside guest memory.
        let l = with_segment();
        l.guest_write::<u32>(SEG + 16, 0xFFFF_FF00).unwrap();
        assert_eq!(get(&l, 1, 64, 1, b"APP.MSG").0, ERROR_MR_UN_ACC_MSGF);
        let l = with_segment();
        l.guest_write::<u32>(SEG + 20, u32::MAX).unwrap();
        assert_eq!(get(&l, 1, 64, 1, b"APP.MSG").0, ERROR_MR_UN_ACC_MSGF, "a size past 16 MB is not read");
        // A segment at the very end of guest memory.
        let end = l.shared.guest_mem.size() as u32 - 4;
        l.guest_write_bytes(NAME, b"APP.MSG\0").unwrap();
        assert_eq!(l.dos_get_message(end, TABLE, 1, BUF, 64, 1, NAME, PCB), ERROR_MR_UN_ACC_MSGF);
    }

    #[test]
    fn failure_texts_name_the_file_and_the_number() {
        let (rc, t) = failure_text(&Lookup::NoMessage, b"X.MSG", 7).unwrap();
        assert_eq!((rc, String::from_utf8(t).unwrap()), (ERROR_MR_MID_NOT_FOUND, "SYS0317: Message 0007 was not found in X.MSG.\r\n".into()));
        assert_eq!(failure_text(&Lookup::BadFile, b"X.MSG", 7).unwrap().0, ERROR_MR_INV_MSGF_FORMAT);
        assert_eq!(failure_text(&Lookup::NoFile, b"X.MSG", 7).unwrap().0, ERROR_MR_UN_ACC_MSGF);
        assert!(failure_text(&Lookup::Found { prefix: vec![], text: vec![] }, b"X.MSG", 7).is_none());
    }

    #[test]
    fn dos_query_message_cp_needs_the_file() {
        let l = Loader::new_mock();
        l.guest_write_bytes(MSG, b"Z:\\NO\\SUCH.MSG\0").unwrap();
        assert_eq!(l.dos_query_message_cp(BUF, 64, MSG, PCB), ERROR_MR_UN_ACC_MSGF);
        assert_eq!(l.dos_query_message_cp(BUF, 64, 0, PCB), ERROR_INVALID_PARAMETER);
    }
}
