// SPDX-License-Identifier: GPL-3.0-or-later
//
// OS/2 KBDCALLS (Keyboard) API implementations.

use super::vm_backend::VcpuBackend;
use log::{debug, warn};
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::console::{map_key_to_os2, HostInput};

/// `KBDINFO`'s size, its `cb`.
const KBDINFO_SIZE: u16 = 10;
/// The most typed-ahead bytes `KbdFlushBuffer` reads from the terminal.
const MAX_FLUSHED_BYTES: usize = 4096;

impl super::Loader {
    pub(crate) fn handle_kbdcalls(&self, vcpu: &mut dyn VcpuBackend, _vcpu_id: u32, ordinal: u32) -> super::ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| -> u32 { self.guest_read::<u32>((esp + off) as u32).expect("Stack read OOB") };

        // KBDCALLS uses Pascal calling convention: last arg at ESP+4.
        let res = match ordinal {
            // KbdCharIn(pKeyInfo, wait, hkbd) → ESP+4=hkbd, +8=wait, +12=pKeyInfo
            4  => self.kbd_char_in(read_stack(12), read_stack(8), read_stack(4)),
            // KbdGetStatus(pInfo, hkbd) → ESP+4=hkbd, +8=pInfo
            10 => self.kbd_get_status(read_stack(8), read_stack(4)),
            // KbdStringIn(pBuf, pLen, wait, hkbd) → ESP+4=hkbd, +8=wait, +12=pLen, +16=pBuf
            9  => self.kbd_string_in(read_stack(16), read_stack(12), read_stack(8), read_stack(4)),
            // KbdPeek(pKeyInfo, hkbd) → ESP+4=hkbd, +8=pKeyInfo
            22 => self.kbd_peek(read_stack(8), read_stack(4)),
            // KbdFlushBuffer(hkbd) → ESP+4=hkbd
            13 => self.kbd_flush_buffer(read_stack(4)),
            // KbdSetStatus(pInfo, hkbd) → ESP+4=hkbd, +8=pInfo
            11 => self.kbd_set_status(read_stack(8), read_stack(4)),
            // KbdGetCp(ulReserved, pidCP, hkbd) → ESP+4=hkbd, +8=pidCP, +12=ulReserved
            3  => self.kbd_get_cp(read_stack(12), read_stack(8), read_stack(4)),
            // KbdSetCp(usReserved, idCP, hkbd) → ESP+4=hkbd, +8=idCP, +12=usReserved
            5  => self.kbd_set_cp(read_stack(12), read_stack(8), read_stack(4)),
            // KbdGetHWID(pHwid, hkbd) → ESP+4=hkbd, +8=pHwid
            24 => self.kbd_get_hwid(read_stack(8), read_stack(4)),
            // KbdOpen(phkbd) → ESP+4=phkbd
            23 => self.kbd_open(read_stack(4)),
            // KbdClose(hkbd) → ESP+4=hkbd
            17 => self.kbd_close(read_stack(4)),
            // KbdGetFocus(fWait, hkbd) → ESP+4=hkbd, +8=fWait; KbdFreeFocus(hkbd) → ESP+4=hkbd
            12 => self.kbd_focus(read_stack(4)),
            18 => self.kbd_focus(read_stack(4)),
            // KbdXlate(pKbdTrans, hkbd) → ESP+4=hkbd, +8=pKbdTrans
            14 => self.kbd_xlate(read_stack(8), read_stack(4)),
            // KbdSynch(fWait) → ESP+4=fWait
            7  => { debug!("  KbdSynch"); NO_ERROR },
            _ => { warn!("Warning: Unknown KBDCALLS Ordinal {}", ordinal); NO_ERROR }
        };
        super::ApiResult::Normal(res)
    }

    /// Pascal calling convention argument byte count for stack cleanup.
    pub(crate) fn kbdcalls_arg_bytes(&self, ordinal: u32) -> u64 {
        match ordinal {
            4 => 12, 10 => 8, 9 => 16,
            22 => 8, 13 => 4, 11 => 8, 3 => 12, 5 => 12, 24 => 8,
            23 => 4, 17 => 4, 12 => 8, 18 => 4, 7 => 4, 14 => 8,
            _ => 0,
        }
    }

    /// KbdCharIn (ordinal 4): read a character from the keyboard.
    /// Fills KBDKEYINFO struct: charcode(u8), scancode(u8), status(u8), reserved(u8),
    /// shift state(u16), time(u32) at +6 — total 10 bytes.
    fn kbd_char_in(&self, p_key_info: u32, wait: u32, _hkbd: u32) -> u32 {
        debug!("  KbdCharIn(wait={})", wait);

        if self.shared.use_sdl_text.load(Ordering::Relaxed) {
            // SDL text mode: block on the keyboard condvar queue.
            let mut queue = self.shared.kbd_queue.lock().unwrap();
            let ki = loop {
                if self.stop_or_restart() { return ERROR_INVALID_FUNCTION; }
                if let Some(ki) = queue.pop_front() { break ki; }
                if wait == 1 {
                    // IO_NOWAIT: return immediately with status 0 (no char)
                    if p_key_info != 0 {
                        self.guest_write::<u8>(p_key_info, 0);
                        self.guest_write::<u8>(p_key_info + 1, 0);
                        self.guest_write::<u8>(p_key_info + 2, 0);
                        self.guest_write::<u8>(p_key_info + 3, 0);
                        self.guest_write::<u16>(p_key_info + 4, 0);
                        self.guest_write::<u32>(p_key_info + 6, 0);
                    }
                    return NO_ERROR;
                }
                // IO_WAIT: sleep on condvar; recheck every 50 ms for shutdown
                let (new_queue, _) = self.shared.kbd_cond
                    .wait_timeout(queue, Duration::from_millis(50))
                    .unwrap();
                queue = new_queue;
            };
            if p_key_info != 0 {
                // fbStatus: 0x40 = final ASCII char; 0x02 = secondary conversion
                // (extended/function key).  4OS2's GetKeystroke checks fbStatus & 0x02
                // together with chChar == 0 to distinguish extended keys: when both are
                // true it returns (chScan | 0x100) so arrow keys and F-keys are
                // recognised.  Regular printable keys and backspace keep 0x40.
                let fb_status: u8 = if ki.ch == 0 { 0x02 } else { 0x40 };
                self.guest_write::<u8>(p_key_info,     ki.ch);
                self.guest_write::<u8>(p_key_info + 1, ki.scan);
                self.guest_write::<u8>(p_key_info + 2, fb_status);
                self.guest_write::<u8>(p_key_info + 3, 0);
                self.guest_write::<u16>(p_key_info + 4, ki.state);
                self.guest_write::<u32>(p_key_info + 6, 0);
            }
            return NO_ERROR;
        }

        // Terminal (termios) path — ensure raw mode is active
        {
            let mut console = self.shared.console_mgr.lock_or_recover();
            console.enable_raw_mode();
            // A key KbdPeek read is the next one.
            if let Some((ch, scan)) = console.kbd.peeked.take() {
                drop(console);
                self.write_key_info(p_key_info, ch, scan, 0x40, 0);
                return NO_ERROR;
            }
        }
        self.kbd_char_in_host(p_key_info, wait, || self.shared.console_mgr.lock_or_recover().read_input())
    }

    /// End the process because a waiting keyboard read found the end of a
    /// piped (non-terminal) standard input: no key will ever come, and
    /// waiting is `KbdCharIn`'s contract, so Warpine ends the program as
    /// `DosExit(EXIT_PROCESS, EXIT_CODE_KBD_END_OF_INPUT)` would — its own
    /// choice; the KBD reference lists no end-of-input error.
    fn kbd_end_of_input(&self) -> u32 {
        debug!("  keyboard read at the end of piped input: ending the process");
        // As DosExit(EXIT_PROCESS) would: thread 1 runs the exit list
        // (TC_EXIT) and ends the process; this call, on thread 1, gives up
        // so that it can (another thread gets the error and is stopped).
        super::signals::request_termination(&self.shared, EXIT_CODE_KBD_END_OF_INPUT as u32, TC_EXIT);
        self.shared.kbd_cond.notify_all();
        self.interrupt_restart();
        ERROR_KBD_NO_DEVICE
    }

    /// `KbdCharIn` from the host's standard input (`read` gives one byte or
    /// says why it has none).
    fn kbd_char_in_host(&self, p_key_info: u32, wait: u32, mut read: impl FnMut() -> HostInput) -> u32 {
        loop {
            if self.stop_or_restart() { return ERROR_INVALID_FUNCTION; }

            let byte = match read() {
                // Ctrl+C is a signal, not a character (cooked mode).
                HostInput::Byte(b) if self.keyboard_signal(b) => None,
                HostInput::Byte(b) => Some(b),
                HostInput::Empty => None,
                // A non-waiting call still just reports "no character".
                HostInput::EndOfInput if wait != IO_NOWAIT => return self.kbd_end_of_input(),
                HostInput::EndOfInput => None,
            };

            if let Some(first_byte) = byte {
                let (charcode, scancode) = {
                    let console = self.shared.console_mgr.lock_or_recover();
                    map_key_to_os2(first_byte, &console)
                };

                if p_key_info != 0 {
                    self.guest_write::<u8>(p_key_info, charcode);
                    self.guest_write::<u8>(p_key_info + 1, scancode);
                    self.guest_write::<u8>(p_key_info + 2, 0x40);
                    self.guest_write::<u8>(p_key_info + 3, 0);
                    self.guest_write::<u16>(p_key_info + 4, 0);
                    self.guest_write::<u32>(p_key_info + 6, 0);
                }
                return NO_ERROR;
            }

            if wait == IO_NOWAIT {
                if p_key_info != 0 {
                    self.guest_write::<u8>(p_key_info, 0);
                    self.guest_write::<u8>(p_key_info + 1, 0);
                    self.guest_write::<u8>(p_key_info + 2, 0);
                    self.guest_write::<u8>(p_key_info + 3, 0);
                    self.guest_write::<u16>(p_key_info + 4, 0);
                    self.guest_write::<u32>(p_key_info + 6, 0);
                }
                return NO_ERROR;
            }

            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// KbdGetStatus (ordinal 10): the logical keyboard's status
    /// (`KBDINFO`), as `KbdSetStatus` left it.
    fn kbd_get_status(&self, p_status: u32, _hkbd: u32) -> u32 {
        debug!("  KbdGetStatus");
        if p_status != 0 {
            let k = self.shared.console_mgr.lock_or_recover().kbd.clone();
            for (off, v) in [(0, KBDINFO_SIZE), (2, k.mask), (4, k.turnaround), (6, k.interim), (8, k.shift)] {
                self.guest_write::<u16>(p_status + off, v);
            }
        }
        NO_ERROR
    }

    /// Fill a `KBDKEYINFO` (chChar, chScan, fbStatus, bNlsShift, fsState,
    /// time; 10 bytes).
    fn write_key_info(&self, p: u32, ch: u8, scan: u8, status: u8, shift: u16) {
        if p == 0 { return; }
        self.guest_write::<u8>(p, ch);
        self.guest_write::<u8>(p + 1, scan);
        self.guest_write::<u8>(p + 2, status);
        self.guest_write::<u8>(p + 3, 0);
        self.guest_write::<u16>(p + 4, shift);
        self.guest_write::<u32>(p + 6, 0);
    }

    /// KbdPeek (ordinal 22): the next key without taking it; a record of
    /// zeros (fbStatus 0) when none is waiting.
    fn kbd_peek(&self, p_key_info: u32, hkbd: u32) -> u32 {
        debug!("  KbdPeek");
        if !self.shared.console_mgr.lock_or_recover().kbd.valid(hkbd) { return ERROR_KBD_INVALID_HANDLE; }
        if self.shared.use_sdl_text.load(Ordering::Relaxed) {
            let next = self.shared.kbd_queue.lock().unwrap().front().cloned();
            match next {
                Some(ki) => self.write_key_info(p_key_info, ki.ch, ki.scan, if ki.ch == 0 { 0x02 } else { 0x40 }, ki.state),
                None => self.write_key_info(p_key_info, 0, 0, 0, 0),
            }
            return NO_ERROR;
        }
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.enable_raw_mode();
        if console.kbd.peeked.is_none()
            && let HostInput::Byte(b) = console.read_input()
            && !self.keyboard_signal(b)
        {
            let key = map_key_to_os2(b, &console);
            console.kbd.peeked = Some(key);
        }
        let peeked = console.kbd.peeked;
        drop(console);
        match peeked {
            Some((ch, scan)) => self.write_key_info(p_key_info, ch, scan, 0x40, 0),
            None => self.write_key_info(p_key_info, 0, 0, 0, 0),
        }
        NO_ERROR
    }

    /// KbdFlushBuffer (ordinal 13): drop every key typed ahead.
    fn kbd_flush_buffer(&self, hkbd: u32) -> u32 {
        debug!("  KbdFlushBuffer");
        if !self.shared.console_mgr.lock_or_recover().kbd.valid(hkbd) { return ERROR_KBD_INVALID_HANDLE; }
        self.shared.kbd_queue.lock().unwrap().clear();
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.kbd.peeked = None;
        if !self.shared.use_sdl_text.load(Ordering::Relaxed) && console.is_raw_mode() {
            // What the terminal holds; bounded, in case input never pauses.
            for _ in 0..MAX_FLUSHED_BYTES {
                if !matches!(console.read_input(), HostInput::Byte(_)) { break; }
            }
        }
        NO_ERROR
    }

    /// KbdSetStatus (ordinal 11): set the echo and input modes, the
    /// turnaround character, the interim and shift states (`KBDINFO`, whose
    /// `fsMask` says what to change). Warpine records them for
    /// `KbdGetStatus`; its reads do not act on them yet.
    fn kbd_set_status(&self, p_info: u32, hkbd: u32) -> u32 {
        use super::vio_text::{KEYBOARD_ASCII_MODE, KEYBOARD_BINARY_MODE, KEYBOARD_ECHO_OFF, KEYBOARD_ECHO_ON};
        const MODIFY_STATE: u16 = 0x0010;
        const MODIFY_INTERIM: u16 = 0x0020;
        const MODIFY_TURNAROUND: u16 = 0x0040;
        const TWO_BYTE_TURNAROUND: u16 = 0x0080;
        let field = |off: u32| self.guest_read::<u16>(p_info + off);
        let (Some(cb), Some(mask), Some(turn), Some(interim), Some(shift)) =
            (field(0), field(2), field(4), field(6), field(8)) else { return ERROR_KBD_PARAMETER };
        debug!("  KbdSetStatus(mask=0x{mask:04X})");
        if cb != KBDINFO_SIZE { return ERROR_KBD_INVALID_LENGTH; }
        let echo = mask & (KEYBOARD_ECHO_ON | KEYBOARD_ECHO_OFF);
        let mode = mask & (KEYBOARD_BINARY_MODE | KEYBOARD_ASCII_MODE);
        if echo == KEYBOARD_ECHO_ON | KEYBOARD_ECHO_OFF { return ERROR_KBD_INVALID_ECHO_MASK; }
        if mode == KEYBOARD_BINARY_MODE | KEYBOARD_ASCII_MODE { return ERROR_KBD_INVALID_INPUT_MASK; }
        let mut console = self.shared.console_mgr.lock_or_recover();
        if !console.kbd.valid(hkbd) { return ERROR_KBD_INVALID_HANDLE; }
        let k = &mut console.kbd;
        if echo != 0 { k.mask = (k.mask & !(KEYBOARD_ECHO_ON | KEYBOARD_ECHO_OFF)) | echo; }
        if mode != 0 { k.mask = (k.mask & !(KEYBOARD_BINARY_MODE | KEYBOARD_ASCII_MODE)) | mode; }
        if mask & MODIFY_STATE != 0 { k.shift = shift; }
        if mask & MODIFY_INTERIM != 0 { k.interim = interim; }
        if mask & MODIFY_TURNAROUND != 0 {
            k.turnaround = if mask & TWO_BYTE_TURNAROUND != 0 { turn } else { turn & 0x00FF };
        }
        NO_ERROR
    }

    /// KbdGetCp (ordinal 3): the keyboard codepage `KbdSetCp` set (0, the
    /// default table, until then).
    fn kbd_get_cp(&self, reserved: u32, p_cp: u32, hkbd: u32) -> u32 {
        if reserved != 0 { return ERROR_KBD_PARAMETER; }
        let console = self.shared.console_mgr.lock_or_recover();
        if !console.kbd.valid(hkbd) { return ERROR_KBD_INVALID_HANDLE; }
        let cp = console.kbd.codepage;
        drop(console);
        debug!("  KbdGetCp -> {cp}");
        match self.guest_write::<u16>(p_cp, cp) {
            Some(()) => NO_ERROR,
            None => ERROR_KBD_PARAMETER,
        }
    }

    /// KbdSetCp (ordinal 5): the keyboard codepage, 0 or one Warpine knows
    /// (`ERROR_KBD_INVALID_CODEPAGE`); "does not affect the display or
    /// process code pages".
    fn kbd_set_cp(&self, _reserved: u32, cp: u32, hkbd: u32) -> u32 {
        let cp = cp & 0xFFFF;
        debug!("  KbdSetCp({cp})");
        if cp != 0 && !super::stubs::is_supported_codepage(cp) { return ERROR_KBD_INVALID_CODEPAGE; }
        let mut console = self.shared.console_mgr.lock_or_recover();
        if !console.kbd.valid(hkbd) { return ERROR_KBD_INVALID_HANDLE; }
        console.kbd.codepage = cp as u16;
        NO_ERROR
    }

    /// KbdGetHWID (ordinal 24): the keyboard's hardware ID (`KBDHWID`: cb,
    /// idKbd, two reserved words). `cb` on entry is the room given; Warpine
    /// reports an enhanced (101/102-key) keyboard, ID 0xAB41.
    fn kbd_get_hwid(&self, p_hwid: u32, hkbd: u32) -> u32 {
        const KBDHWID_SIZE: u16 = 8;
        /// The ID an enhanced keyboard answers (Warpine's choice: it has no
        /// keyboard hardware to ask).
        const ENHANCED_KEYBOARD_ID: u16 = 0xAB41;
        if !self.shared.console_mgr.lock_or_recover().kbd.valid(hkbd) { return ERROR_KBD_INVALID_HANDLE; }
        let Some(cb) = self.guest_read::<u16>(p_hwid) else { return ERROR_KBD_PARAMETER };
        debug!("  KbdGetHWID(cb={cb})");
        // cb itself and idKbd at least.
        if cb < 4 { return ERROR_KBD_PARAMETER; }
        let cb = cb.min(KBDHWID_SIZE);
        let fields = [cb, ENHANCED_KEYBOARD_ID, 0, 0];
        for (off, v) in (0u32..).step_by(2).zip(fields).take(usize::from(cb / 2)) {
            self.guest_write::<u16>(p_hwid + off, v);
        }
        NO_ERROR
    }

    /// KbdXlate (ordinal 14): translate the `KBDTRANS` at `p` — its
    /// `chScan` and `fsState` — into `chChar`, with `fbStatus` and
    /// `fsXlate` saying the character is final (US layout,
    /// `vio_text::translate_scan`; a key without a character gives 0 and
    /// the extended-key status). An unknown scan code is translated to 0
    /// and not marked final.
    fn kbd_xlate(&self, p: u32, hkbd: u32) -> u32 {
        const KT_CHAR: u32 = 0;
        const KT_SCAN: u32 = 1;
        const KT_STATUS: u32 = 2;
        const KT_STATE: u32 = 4;
        const KT_XLATE: u32 = 12;
        const KBDTRF_FINAL_CHAR_IN: u8 = 0x40;
        /// fbStatus bit 1: an extended key (`KbdCharIn` sets it too).
        const KBDTRF_EXTENDED: u8 = 0x02;
        /// fsXlate bit 0: the translation is complete.
        const XLATE_COMPLETE: u16 = 0x0001;
        if !self.shared.console_mgr.lock_or_recover().kbd.valid(hkbd) { return ERROR_KBD_INVALID_HANDLE; }
        let (Some(scan), Some(state)) = (self.guest_read::<u8>(p + KT_SCAN), self.guest_read::<u16>(p + KT_STATE)) else {
            return ERROR_KBD_PARAMETER;
        };
        let translated = super::vio_text::translate_scan(scan, state);
        debug!("  KbdXlate(scan=0x{scan:02X}, state=0x{state:04X}) -> {translated:?}");
        let ch = translated.unwrap_or(0);
        let status = match translated {
            Some(0) => KBDTRF_FINAL_CHAR_IN | KBDTRF_EXTENDED,
            Some(_) => KBDTRF_FINAL_CHAR_IN,
            None => 0,
        };
        let xlate = if translated.is_some() { XLATE_COMPLETE } else { 0 };
        let ok = self.guest_write::<u8>(p + KT_CHAR, ch).is_some()
            && self.guest_write::<u8>(p + KT_STATUS, status).is_some()
            && self.guest_write::<u16>(p + KT_XLATE, xlate).is_some();
        if ok { NO_ERROR } else { ERROR_KBD_PARAMETER }
    }

    /// KbdOpen (ordinal 23): a new logical keyboard handle. Every handle
    /// reads the one keyboard.
    fn kbd_open(&self, p_hkbd: u32) -> u32 {
        let Some(h) = self.shared.console_mgr.lock_or_recover().kbd.open() else { return ERROR_KBD_PARAMETER };
        debug!("  KbdOpen -> {h}");
        if self.guest_write::<u16>(p_hkbd, h).is_none() {
            self.shared.console_mgr.lock_or_recover().kbd.close(u32::from(h));
            return ERROR_KBD_PARAMETER;
        }
        NO_ERROR
    }

    /// KbdClose (ordinal 17): close a handle `KbdOpen` gave out.
    fn kbd_close(&self, hkbd: u32) -> u32 {
        debug!("  KbdClose({hkbd})");
        if self.shared.console_mgr.lock_or_recover().kbd.close(hkbd) { NO_ERROR } else { ERROR_KBD_INVALID_HANDLE }
    }

    /// KbdGetFocus (ordinal 12) / KbdFreeFocus (ordinal 18): bind the
    /// physical keyboard to a logical one and free it. With one keyboard
    /// for the process the binding is always there; the handle is checked.
    fn kbd_focus(&self, hkbd: u32) -> u32 {
        debug!("  Kbd focus({hkbd})");
        if self.shared.console_mgr.lock_or_recover().kbd.valid(hkbd) { NO_ERROR } else { ERROR_KBD_INVALID_HANDLE }
    }

    /// KbdStringIn (ordinal 9): read a string from keyboard.
    /// Uses repeated KbdCharIn with echo.
    fn kbd_string_in(&self, p_buf: u32, p_length: u32, wait: u32, _hkbd: u32) -> u32 {
        debug!("  KbdStringIn");
        let max_len = if p_length != 0 {
            self.guest_read::<u16>(p_length).unwrap_or(0) as u32
        } else {
            return ERROR_INVALID_FUNCTION;
        };

        // Ensure raw mode
        {
            let mut console = self.shared.console_mgr.lock_or_recover();
            console.enable_raw_mode();
        }
        self.kbd_string_in_host(p_buf, p_length, max_len, wait,
                                || self.shared.console_mgr.lock_or_recover().read_input())
    }

    /// `KbdStringIn` from the host's standard input. At the end of piped
    /// input a partly read line is returned as if Enter ended it; with
    /// nothing read, a waiting call ends the process (`kbd_end_of_input`).
    fn kbd_string_in_host(&self, p_buf: u32, p_length: u32, max_len: u32, wait: u32,
                          mut read: impl FnMut() -> HostInput) -> u32 {
        let mut count = 0u32;
        loop {
            if self.shutting_down() || count >= max_len { break; }
            // A signal: with nothing typed yet the call is made again after
            // it; a partly typed line is returned as it is.
            if self.interrupted() {
                if count == 0 { self.interrupt_restart(); }
                break;
            }

            let byte = match read() {
                HostInput::Byte(b) if self.keyboard_signal(b) => None,
                HostInput::Byte(b) => Some(b),
                HostInput::Empty => None,
                HostInput::EndOfInput if count > 0 || wait == IO_NOWAIT => break,
                HostInput::EndOfInput => return self.kbd_end_of_input(),
            };

            if let Some(b) = byte {
                if b == 0x0D {
                    // Enter: done
                    break;
                } else if b == 0x08 || b == 0x7F {
                    // Backspace
                    if count > 0 {
                        count -= 1;
                        // Echo backspace
                        let mut console = self.shared.console_mgr.lock_or_recover();
                        console.write_tty(b"\x08 \x08", 437);
                    }
                } else if b >= 0x20 {
                    self.guest_write::<u8>(p_buf + count, b);
                    count += 1;
                    // Echo character
                    let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
                    let mut console = self.shared.console_mgr.lock_or_recover();
                    console.write_tty(&[b], cp);
                }
            } else if wait == IO_NOWAIT {
                break;
            } else {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        // Write actual length
        if p_length != 0 {
            self.guest_write::<u16>(p_length + 2, count as u16); // cchIn (actual count read)
        }
        NO_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Loader, ApiResult, KbdKeyInfo};
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::constants::ERROR_INVALID_FUNCTION;
    use std::sync::atomic::Ordering;

    /// Push Pascal-convention args: args[0] at ESP+4, args[1] at ESP+8, …
    fn write_stack(loader: &Loader, esp: u32, args: &[u32]) {
        for (i, &arg) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, arg).unwrap();
        }
    }

    // ── kbdcalls_arg_bytes ───────────────────────────────────────────────────

    #[test]
    fn test_kbdcalls_arg_bytes() {
        let loader = Loader::new_mock();
        assert_eq!(loader.kbdcalls_arg_bytes(4),  12); // KbdCharIn:   3 args × 4
        assert_eq!(loader.kbdcalls_arg_bytes(9),  16); // KbdStringIn: 4 args × 4
        assert_eq!(loader.kbdcalls_arg_bytes(10),  8); // KbdGetStatus: 2 args × 4
        assert_eq!(loader.kbdcalls_arg_bytes(99),  0); // unknown → 0
    }

    // ── KbdGetStatus (ordinal 10) ────────────────────────────────────────────

    #[test]
    fn test_kbd_get_status_writes_kbdinfo_struct() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp = 0x1000u32;
        vcpu.regs.rsp = esp as u64;
        let p_status = 0x2000u32;
        // Pascal: KbdGetStatus(pInfo, hkbd) → ESP+4=hkbd, ESP+8=pInfo
        write_stack(&loader, esp, &[0, p_status]);
        let result = loader.handle_kbdcalls(&mut vcpu, 0, 10);
        assert!(matches!(result, ApiResult::Normal(0)));
        assert_eq!(loader.guest_read::<u16>(p_status),     Some(10));   // cb = struct size
        assert_eq!(loader.guest_read::<u16>(p_status + 2), Some(0x06)); // fsMask: binary+raw
        assert_eq!(loader.guest_read::<u16>(p_status + 4), Some(0x0D)); // turn-around: CR
        assert_eq!(loader.guest_read::<u16>(p_status + 6), Some(0));    // fsInterim
        assert_eq!(loader.guest_read::<u16>(p_status + 8), Some(0));    // fsState
    }

    #[test]
    fn test_kbd_get_status_null_pointer_no_crash() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp = 0x1000u32;
        vcpu.regs.rsp = esp as u64;
        write_stack(&loader, esp, &[0, 0]); // pStatus = null
        let result = loader.handle_kbdcalls(&mut vcpu, 0, 10);
        assert!(matches!(result, ApiResult::Normal(0)));
    }

    // ── KbdCharIn (ordinal 4) — SDL mode ───────────────────────────────────

    #[test]
    fn test_kbd_char_in_nowait_empty_queue_returns_zeroed_struct() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        let esp = 0x1000u32;
        vcpu.regs.rsp = esp as u64;
        let p_key_info = 0x2000u32;
        // Pascal: KbdCharIn(pKeyInfo, wait, hkbd) → ESP+4=hkbd, +8=wait(1=NOWAIT), +12=pKeyInfo
        write_stack(&loader, esp, &[0, 1, p_key_info]);
        let result = loader.handle_kbdcalls(&mut vcpu, 0, 4);
        assert!(matches!(result, ApiResult::Normal(0)));
        // All fields zeroed (no char available)
        assert_eq!(loader.guest_read::<u8>(p_key_info),     Some(0)); // ch
        assert_eq!(loader.guest_read::<u8>(p_key_info + 1), Some(0)); // scan
        assert_eq!(loader.guest_read::<u8>(p_key_info + 2), Some(0)); // fbStatus
        assert_eq!(loader.guest_read::<u16>(p_key_info + 4), Some(0)); // state
    }

    #[test]
    fn test_kbd_char_in_sdl_prequeued_key_delivered() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        // Pre-queue a key before the call
        {
            let mut q = loader.shared.kbd_queue.lock().unwrap();
            q.push_back(KbdKeyInfo { ch: b'A', scan: 0x1E, state: 0x0020 });
        }
        loader.shared.kbd_cond.notify_all();
        let esp = 0x1000u32;
        vcpu.regs.rsp = esp as u64;
        let p_key_info = 0x2000u32;
        // wait=0 (IO_WAIT) — key already in queue so returns immediately
        write_stack(&loader, esp, &[0, 0, p_key_info]);
        let result = loader.handle_kbdcalls(&mut vcpu, 0, 4);
        assert!(matches!(result, ApiResult::Normal(0)));
        assert_eq!(loader.guest_read::<u8>(p_key_info),     Some(b'A')); // ch
        assert_eq!(loader.guest_read::<u8>(p_key_info + 1), Some(0x1E)); // scan
        assert_eq!(loader.guest_read::<u8>(p_key_info + 2), Some(0x40)); // fbStatus = final char
        assert_eq!(loader.guest_read::<u16>(p_key_info + 4), Some(0x0020)); // shift state
    }

    #[test]
    fn test_kbd_char_in_sdl_extended_key_uses_fb_status_02() {
        // Extended keys (ch=0, e.g. arrow keys) must use fbStatus=0x02 so that
        // 4OS2's GetKeystroke loop can detect them via (fbStatus & 2) and return
        // the scan code as (chScan | 0x100) rather than looping forever on ch=0.
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        {
            let mut q = loader.shared.kbd_queue.lock().unwrap();
            // Up-arrow: ch=0, scan=0x48
            q.push_back(KbdKeyInfo { ch: 0, scan: 0x48, state: 0 });
        }
        loader.shared.kbd_cond.notify_all();
        let esp = 0x1000u32;
        vcpu.regs.rsp = esp as u64;
        let p_key_info = 0x2000u32;
        write_stack(&loader, esp, &[0, 0, p_key_info]);
        let result = loader.handle_kbdcalls(&mut vcpu, 0, 4);
        assert!(matches!(result, ApiResult::Normal(0)));
        assert_eq!(loader.guest_read::<u8>(p_key_info),     Some(0));    // ch = 0
        assert_eq!(loader.guest_read::<u8>(p_key_info + 1), Some(0x48)); // scan = Up
        assert_eq!(loader.guest_read::<u8>(p_key_info + 2), Some(0x02)); // fbStatus = extended
    }

    // ── KbdStringIn (ordinal 9) ──────────────────────────────────────────────

    #[test]
    fn test_kbd_string_in_null_length_returns_error() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp = 0x1000u32;
        vcpu.regs.rsp = esp as u64;
        // Pascal: KbdStringIn(pBuf, pLen, wait, hkbd) → ESP+4=hkbd, +8=wait, +12=pLen, +16=pBuf
        // pLen = 0 (null) → must return ERROR_INVALID_FUNCTION
        write_stack(&loader, esp, &[0, 0, 0, 0x2000]); // hkbd=0, wait=0, pLen=0, pBuf=0x2000
        let result = loader.handle_kbdcalls(&mut vcpu, 0, 9);
        assert!(matches!(result, ApiResult::Normal(ERROR_INVALID_FUNCTION)));
    }

    // ── End of piped input (terminal path) ───────────────────────────────────

    use super::super::console::HostInput;
    use super::super::constants::{EXIT_CODE_KBD_END_OF_INPUT, IO_NOWAIT, IO_WAIT};

    /// A waiting KbdCharIn at the end of piped input ends the process: as
    /// DosExit(EXIT_PROCESS) would, thread 1 is asked to run the exit list
    /// (TC_EXIT); the call on thread 1 gives up to let it.
    #[test]
    fn test_kbd_char_in_waiting_at_end_of_input_ends_the_process() {
        use super::super::interrupt;
        let loader = Loader::new_mock();
        loader.shared.exit_code.store(99, Ordering::Relaxed);
        let mut reads = vec![HostInput::Empty, HostInput::EndOfInput].into_iter();
        interrupt::begin_call(super::super::threads::MAIN_TID, true);
        let rc = loader.kbd_char_in_host(0x2000, IO_WAIT, || reads.next().unwrap());
        assert!(interrupt::end_call(), "thread 1 goes on to its exit list");
        assert_ne!(rc, 0);
        let mut signals = loader.shared.threads.signals.lock().unwrap();
        assert_eq!(signals.take_termination(),
                   Some((EXIT_CODE_KBD_END_OF_INPUT as u32, super::super::constants::TC_EXIT)));
        drop(signals);
        // With thread 1 gone, the process ends at once.
        let loader = Loader::new_mock();
        loader.shared.threads.table.lock().unwrap().end(super::super::threads::MAIN_TID);
        let rc = loader.kbd_char_in_host(0x2000, IO_WAIT, || HostInput::EndOfInput);
        assert_ne!(rc, 0);
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), EXIT_CODE_KBD_END_OF_INPUT);
    }

    /// Ctrl+C read from the keyboard (here: piped input) is XCPT_SIGNAL_INTR,
    /// not a character.
    #[test]
    fn test_kbd_char_in_ctrl_c_is_a_signal() {
        let loader = Loader::new_mock();
        let p = 0x2000u32;
        let mut reads = vec![HostInput::Byte(0x03), HostInput::Byte(b'x')].into_iter();
        assert_eq!(loader.kbd_char_in_host(p, IO_WAIT, || reads.next().unwrap()), 0);
        assert_eq!(loader.guest_read::<u8>(p), Some(b'x'));
        let mut signals = loader.shared.threads.signals.lock().unwrap();
        assert_eq!(signals.take(), Some(super::super::constants::XCPT_SIGNAL_INTR));
    }

    /// A non-waiting KbdCharIn at the end of input reports "no character".
    #[test]
    fn test_kbd_char_in_nowait_at_end_of_input_returns_no_character() {
        let loader = Loader::new_mock();
        let p = 0x2000u32;
        loader.guest_write::<u32>(p, 0xFFFF_FFFF).unwrap();
        let rc = loader.kbd_char_in_host(p, IO_NOWAIT, || HostInput::EndOfInput);
        assert_eq!(rc, 0);
        assert_eq!(loader.guest_read::<u32>(p), Some(0), "zeroed KBDKEYINFO");
        assert!(!loader.shared.exit_requested.load(Ordering::Relaxed));
    }

    #[test]
    fn test_kbd_char_in_delivers_a_piped_byte() {
        let loader = Loader::new_mock();
        let p = 0x2000u32;
        let rc = loader.kbd_char_in_host(p, IO_WAIT, || HostInput::Byte(b'q'));
        assert_eq!(rc, 0);
        assert_eq!(loader.guest_read::<u8>(p), Some(b'q'));
        assert_eq!(loader.guest_read::<u8>(p + 2), Some(0x40), "final character");
    }

    /// KbdStringIn: a partly read line is returned at the end of input; the
    /// next waiting call ends the process.
    #[test]
    fn test_kbd_string_in_at_end_of_input() {
        let loader = Loader::new_mock();
        loader.shared.console_mgr.lock().unwrap().enable_sdl_mode(); // echo stays in the buffer
        let (buf, len) = (0x3000u32, 0x2000u32);
        let mut reads = vec![HostInput::Byte(b'h'), HostInput::Byte(b'i'), HostInput::EndOfInput].into_iter();
        assert_eq!(loader.kbd_string_in_host(buf, len, 40, IO_WAIT, || reads.next().unwrap()), 0);
        assert_eq!(loader.guest_read::<u16>(len + 2), Some(2));
        assert_eq!((loader.guest_read::<u8>(buf), loader.guest_read::<u8>(buf + 1)), (Some(b'h'), Some(b'i')));
        assert!(!loader.shared.exit_requested.load(Ordering::Relaxed));
        // Nothing left, not waiting: an empty line.
        assert_eq!(loader.kbd_string_in_host(buf, len, 40, IO_NOWAIT, || HostInput::EndOfInput), 0);
        assert_eq!(loader.guest_read::<u16>(len + 2), Some(0));
        // Nothing left, waiting: the process ends (thread 1 runs its exit list).
        assert_ne!(loader.kbd_string_in_host(buf, len, 40, IO_WAIT, || HostInput::EndOfInput), 0);
        assert!(loader.shared.threads.signals.lock().unwrap().termination_pending());
    }

    /// A signal while KbdStringIn waits: with nothing typed the call is
    /// made again after it; a partly typed line is returned.
    #[test]
    fn test_kbd_string_in_interrupted() {
        use super::super::interrupt;
        use super::super::threads::MAIN_TID;
        let loader = Loader::new_mock();
        loader.shared.console_mgr.lock().unwrap().enable_sdl_mode();
        loader.dos_set_signal_exception_focus(super::super::constants::SIG_SETFOCUS, 0);
        let (buf, len) = (0x3000u32, 0x2000u32);
        interrupt::begin_call(MAIN_TID, true);
        let mut reads = vec![HostInput::Byte(0x03)].into_iter().chain(std::iter::repeat(HostInput::Empty));
        loader.kbd_string_in_host(buf, len, 40, IO_WAIT, || reads.next().unwrap());
        assert!(interrupt::end_call(), "made again");
        loader.shared.threads.signals.lock().unwrap().take();
        loader.shared.threads.signals.lock().unwrap().acknowledge(super::super::constants::XCPT_SIGNAL_INTR);
        interrupt::begin_call(MAIN_TID, true);
        let mut reads = vec![HostInput::Byte(b'a'), HostInput::Byte(0x03)].into_iter()
            .chain(std::iter::repeat(HostInput::Empty));
        assert_eq!(loader.kbd_string_in_host(buf, len, 40, IO_WAIT, || reads.next().unwrap()), 0);
        assert!(!interrupt::end_call(), "a partly typed line is returned");
        assert_eq!(loader.guest_read::<u16>(len + 2), Some(1));
    }

    /// KbdSetStatus checks KBDINFO's length and masks and keeps what it
    /// sets; KbdGetHWID, KbdSetCp and the handles answer as documented.
    #[test]
    fn kbd_status_codepage_hwid_and_handles() {
        use super::super::constants::*;
        let loader = Loader::new_mock();
        let p = 0x0020_0000;
        let info = |cb: u16, mask: u16, turn: u16| {
            for (off, v) in [(0u32, cb), (2, mask), (4, turn), (6, 0), (8, 0)] { loader.guest_write::<u16>(p + off, v).unwrap(); }
        };
        info(10, 0x0003, 0);
        assert_eq!(loader.kbd_set_status(p, 0), ERROR_KBD_INVALID_ECHO_MASK);
        info(10, 0x000C, 0);
        assert_eq!(loader.kbd_set_status(p, 0), ERROR_KBD_INVALID_INPUT_MASK);
        info(8, 0x0001, 0);
        assert_eq!(loader.kbd_set_status(p, 0), ERROR_KBD_INVALID_LENGTH);
        info(10, 0x0001 | 0x0040, 0x121B);
        assert_eq!(loader.kbd_set_status(p, 7), ERROR_KBD_INVALID_HANDLE);
        assert_eq!(loader.kbd_set_status(p, 0), NO_ERROR);
        assert_eq!(loader.kbd_get_status(p, 0), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(p + 2), Some(0x0001 | 0x0004), "echo on, still binary");
        assert_eq!(loader.guest_read::<u16>(p + 4), Some(0x1B), "a one-byte turnaround");

        assert_eq!(loader.kbd_set_cp(0, 12345, 0), ERROR_KBD_INVALID_CODEPAGE);
        assert_eq!(loader.kbd_set_cp(0, 850, 0), NO_ERROR);
        assert_eq!(loader.kbd_get_cp(0, p, 0), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(p), Some(850));
        assert_eq!(loader.kbd_get_cp(1, p, 0), ERROR_KBD_PARAMETER, "the reserved word");

        loader.guest_write::<u16>(p, 2).unwrap();
        assert_eq!(loader.kbd_get_hwid(p, 0), ERROR_KBD_PARAMETER, "no room for idKbd");
        loader.guest_write::<u16>(p, 6).unwrap();
        assert_eq!(loader.kbd_get_hwid(p, 0), NO_ERROR);
        assert_eq!(loader.guest_read::<u16>(p), Some(6));
        assert_eq!(loader.guest_read::<u16>(p + 2), Some(0xAB41));

        assert_eq!(loader.kbd_open(p), NO_ERROR);
        let h = u32::from(loader.guest_read::<u16>(p).unwrap());
        assert_eq!(loader.kbd_focus(h), NO_ERROR);
        assert_eq!(loader.kbd_close(h), NO_ERROR);
        assert_eq!(loader.kbd_close(h), ERROR_KBD_INVALID_HANDLE);
        assert_eq!(loader.kbd_focus(h), ERROR_KBD_INVALID_HANDLE);
    }

    /// KbdPeek leaves the key for KbdCharIn (the SDL key queue).
    #[test]
    fn kbd_peek_leaves_the_key() {
        use super::super::constants::NO_ERROR;
        use super::super::KbdKeyInfo;
        let loader = Loader::new_mock();
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        let p = 0x0020_0000;
        assert_eq!(loader.kbd_peek(p, 0), NO_ERROR);
        assert_eq!(loader.guest_read::<u8>(p + 2), Some(0), "no key: fbStatus 0");
        loader.shared.kbd_queue.lock().unwrap().push_back(KbdKeyInfo { ch: b'q', scan: 0x10, state: 0 });
        assert_eq!(loader.kbd_peek(p, 0), NO_ERROR);
        assert_eq!(loader.guest_read::<u8>(p), Some(b'q'));
        assert_eq!(loader.guest_read::<u8>(p + 2), Some(0x40));
        assert_eq!(loader.shared.kbd_queue.lock().unwrap().len(), 1, "still there");
        assert_eq!(loader.kbd_flush_buffer(0), NO_ERROR);
        assert!(loader.shared.kbd_queue.lock().unwrap().is_empty());
    }
}
