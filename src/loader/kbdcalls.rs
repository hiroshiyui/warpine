// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 KBDCALLS (Keyboard) API implementations.

use super::vm_backend::VcpuBackend;
use log::{debug, warn};
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::console::{map_key_to_os2, HostInput};

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
            _ => { warn!("Warning: Unknown KBDCALLS Ordinal {}", ordinal); NO_ERROR }
        };
        super::ApiResult::Normal(res)
    }

    /// Pascal calling convention argument byte count for stack cleanup.
    pub(crate) fn kbdcalls_arg_bytes(&self, ordinal: u32) -> u64 {
        match ordinal {
            4 => 12, 10 => 8, 9 => 16,
            _ => 0,
        }
    }

    /// KbdCharIn (ordinal 4): read a character from the keyboard.
    /// Fills KBDKEYINFO struct: charcode(u8), scancode(u8), status(u8), reserved(u8),
    /// shift state(u16), time(u32) — total 8 bytes.
    fn kbd_char_in(&self, p_key_info: u32, wait: u32, _hkbd: u32) -> u32 {
        debug!("  KbdCharIn(wait={})", wait);

        if self.shared.use_sdl_text.load(Ordering::Relaxed) {
            // SDL text mode: block on the keyboard condvar queue.
            let mut queue = self.shared.kbd_queue.lock().unwrap();
            let ki = loop {
                if self.shutting_down() { return ERROR_INVALID_FUNCTION; }
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
        // The same path as DosExit(EXIT_PROCESS): it also wakes threads
        // waiting for the process to end (`threads.rs`).
        self.request_process_exit(EXIT_CODE_KBD_END_OF_INPUT as u32);
        self.shared.kbd_cond.notify_all();
        ERROR_KBD_NO_DEVICE
    }

    /// `KbdCharIn` from the host's standard input (`read` gives one byte or
    /// says why it has none).
    fn kbd_char_in_host(&self, p_key_info: u32, wait: u32, mut read: impl FnMut() -> HostInput) -> u32 {
        loop {
            if self.shutting_down() { return ERROR_INVALID_FUNCTION; }

            let byte = match read() {
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

    /// KbdGetStatus (ordinal 10): get keyboard status.
    fn kbd_get_status(&self, p_status: u32, _hkbd: u32) -> u32 {
        debug!("  KbdGetStatus");
        if p_status != 0 {
            // KBDINFO struct: cb(u16), fsMask(u16), chTurnAround(u16), fsInterim(u16), fsState(u16)
            self.guest_write::<u16>(p_status, 10);     // cb
            self.guest_write::<u16>(p_status + 2, 0x06); // fsMask: binary+raw mode
            self.guest_write::<u16>(p_status + 4, 0x0D); // turn-around char: CR
            self.guest_write::<u16>(p_status + 6, 0);    // fsInterim
            self.guest_write::<u16>(p_status + 8, 0);    // fsState (shift state)
        }
        NO_ERROR
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

            let byte = match read() {
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

    /// A waiting KbdCharIn at the end of piped input ends the process.
    #[test]
    fn test_kbd_char_in_waiting_at_end_of_input_ends_the_process() {
        let loader = Loader::new_mock();
        loader.shared.exit_code.store(99, Ordering::Relaxed);
        let mut reads = vec![HostInput::Empty, HostInput::EndOfInput].into_iter();
        let rc = loader.kbd_char_in_host(0x2000, IO_WAIT, || reads.next().unwrap());
        assert_ne!(rc, 0);
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), EXIT_CODE_KBD_END_OF_INPUT);
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
        // Nothing left, waiting: the process ends.
        assert_ne!(loader.kbd_string_in_host(buf, len, 40, IO_WAIT, || HostInput::EndOfInput), 0);
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
    }
}
