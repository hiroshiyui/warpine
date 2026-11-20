// SPDX-License-Identifier: GPL-3.0-or-later
//
// MSG.DLL message helpers, after the IBM Control Program Programming
// Reference: `DosInsertMessage` (MSG.4) and `DosQueryMessageCP` (MSG.8).
// `DosPutMessage` (MSG.5) and `DosGetMessage` (MSG.6) are in `stubs.rs`.
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
// the insertion table. Warpine reads no message files, so
// `DosQueryMessageCP` reports, for a file that exists, the process
// codepage as its one codepage.
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
/// `ERROR_MR_UN_ACC_MSGF` (`<bseerr.h>`).
pub const ERROR_MR_UN_ACC_MSGF: u32 = 318;
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

impl super::Loader {
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

    #[test]
    fn dos_query_message_cp_needs_the_file() {
        let l = Loader::new_mock();
        l.guest_write_bytes(MSG, b"Z:\\NO\\SUCH.MSG\0").unwrap();
        assert_eq!(l.dos_query_message_cp(BUF, 64, MSG, PCB), ERROR_MR_UN_ACC_MSGF);
        assert_eq!(l.dos_query_message_cp(BUF, 64, 0, PCB), ERROR_INVALID_PARAMETER);
    }
}
