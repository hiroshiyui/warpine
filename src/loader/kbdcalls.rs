// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 KBDCALLS (Keyboard) API implementations.

use kvm_ioctls::VcpuFd;
use log::{debug, warn};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::console::map_key_to_os2;

impl super::Loader {
    pub(crate) fn handle_kbdcalls(&self, vcpu: &mut VcpuFd, _vcpu_id: u32, ordinal: u32) -> super::ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| -> u32 { self.guest_read::<u32>((esp + off) as u32).expect("Stack read OOB") };

        let res = match ordinal {
            4  => self.kbd_char_in(read_stack(4), read_stack(8), read_stack(12)),
            10 => self.kbd_get_status(read_stack(4), read_stack(8)),
            9  => self.kbd_string_in(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
            _ => { warn!("Warning: Unknown KBDCALLS Ordinal {}", ordinal); NO_ERROR }
        };
        super::ApiResult::Normal(res)
    }

    /// KbdCharIn (ordinal 4): read a character from the keyboard.
    /// Fills KBDKEYINFO struct: charcode(u8), scancode(u8), status(u8), reserved(u8),
    /// shift state(u16), time(u32) — total 8 bytes.
    fn kbd_char_in(&self, p_key_info: u32, wait: u32, _hkbd: u32) -> u32 {
        debug!("  KbdCharIn(wait={})", wait);

        // Ensure raw mode is active
        {
            let mut console = self.shared.console_mgr.lock_or_recover();
            console.enable_raw_mode();
        }

        loop {
            if self.shutting_down() { return ERROR_INVALID_FUNCTION; }

            let byte = {
                let console = self.shared.console_mgr.lock_or_recover();
                console.read_byte()
            };

            if let Some(first_byte) = byte {
                let (charcode, scancode) = {
                    let console = self.shared.console_mgr.lock_or_recover();
                    map_key_to_os2(first_byte, &console)
                };

                if p_key_info != 0 {
                    self.guest_write::<u8>(p_key_info, charcode);      // chChar
                    self.guest_write::<u8>(p_key_info + 1, scancode);  // chScan
                    self.guest_write::<u8>(p_key_info + 2, 0x40);     // fbStatus: final char
                    self.guest_write::<u8>(p_key_info + 3, 0);         // bNlsShift
                    self.guest_write::<u16>(p_key_info + 4, 0);       // fsState (shift state)
                    self.guest_write::<u32>(p_key_info + 6, 0);       // time
                }
                return NO_ERROR;
            }

            // No input available
            if wait == 1 {
                // IO_NOWAIT: return immediately with no data
                if p_key_info != 0 {
                    self.guest_write::<u8>(p_key_info, 0);
                    self.guest_write::<u8>(p_key_info + 1, 0);
                    self.guest_write::<u8>(p_key_info + 2, 0); // status 0 = no char
                    self.guest_write::<u8>(p_key_info + 3, 0);
                    self.guest_write::<u16>(p_key_info + 4, 0);
                    self.guest_write::<u32>(p_key_info + 6, 0);
                }
                return NO_ERROR;
            }

            // IO_WAIT: sleep briefly and retry
            std::thread::sleep(std::time::Duration::from_millis(10));
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

        let mut count = 0u32;
        loop {
            if self.shutting_down() || count >= max_len { break; }

            let byte = {
                let console = self.shared.console_mgr.lock_or_recover();
                console.read_byte()
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
                        console.write_tty(b"\x08 \x08", 0x07);
                    }
                } else if b >= 0x20 {
                    self.guest_write::<u8>(p_buf + count, b);
                    count += 1;
                    // Echo character
                    let mut console = self.shared.console_mgr.lock_or_recover();
                    console.write_tty(&[b], 0x07);
                }
            } else if wait == 1 {
                break; // NOWAIT
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
