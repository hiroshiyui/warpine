// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 VIOCALLS (Video I/O) API implementations.

use super::vm_backend::VcpuBackend;
use log::{debug, warn};

use super::constants::*;
use super::mutex_ext::MutexExt;

impl super::Loader {
    pub(crate) fn handle_viocalls(&self, vcpu: &mut dyn VcpuBackend, _vcpu_id: u32, ordinal: u32) -> super::ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| -> u32 { self.guest_read::<u32>((esp + off) as u32).expect("Stack read OOB") };

        // VIOCALLS ordinals from OS/2 VIOCALLS DLL (see doc/os2_ordinals.md).
        // VIO uses Pascal calling convention: args pushed LEFT-TO-RIGHT,
        // so LAST arg is at ESP+4, first arg at highest offset.
        // Example: VioGetMode(pMode, hvio) → ESP+4=hvio, ESP+8=pMode
        let res = match ordinal {
            // VioWrtTTY(pStr, len, hvio) → ESP+4=hvio, +8=len, +12=pStr
            19 => self.vio_wrt_tty(read_stack(12), read_stack(8), read_stack(4)),
            // VioGetMode(pMode, hvio) → ESP+4=hvio, +8=pMode
            21 => self.vio_get_mode(read_stack(8), read_stack(4)),
            // VioGetCurPos(pRow, pCol, hvio) → ESP+4=hvio, +8=pCol, +12=pRow
            9  => self.vio_get_cur_pos(read_stack(12), read_stack(8), read_stack(4)),
            // VioSetCurPos(row, col, hvio) → ESP+4=hvio, +8=col, +12=row
            15 => self.vio_set_cur_pos(read_stack(12), read_stack(8), read_stack(4)),
            // VioScrollUp(ulr, ulc, lrr, lrc, n, cell, hvio) → ESP+4=hvio, +8=cell, +12=n, +16=lrc, +20=lrr, +24=ulc, +28=ulr
            7  => self.vio_scroll_up(read_stack(28), read_stack(24), read_stack(20), read_stack(16), read_stack(12), read_stack(8)),
            8  => { debug!("  VioPrtSc (stub)"); NO_ERROR },
            // VioWrtCharStrAtt(pStr, len, row, col, pAttr, hvio) → ESP+4=hvio, +8=pAttr, +12=col, +16=row, +20=len, +24=pStr
            48 => self.vio_wrt_char_str_att(read_stack(24), read_stack(20), read_stack(16), read_stack(12), read_stack(8)),
            // VioWrtNCell(pCell, n, row, col, hvio) → ESP+4=hvio, +8=col, +12=row, +16=n, +20=pCell
            52 => self.vio_wrt_n_cell(read_stack(20), read_stack(16), read_stack(12), read_stack(8), read_stack(4)),
            // VioWrtNAttr(pAttr, len, row, col, hvio) → ESP+4=hvio, +8=col, +12=row, +16=len, +20=pAttr
            26 => self.vio_wrt_n_attr(read_stack(20), read_stack(16), read_stack(12), read_stack(8), read_stack(4)),
            // VioReadCellStr(pBuf, pLen, row, col, hvio) → ESP+4=hvio, +8=col, +12=row, +16=pLen, +20=pBuf
            24 => self.vio_read_cell_str(read_stack(20), read_stack(16), read_stack(12), read_stack(8), read_stack(4)),
            // VioSetCurType(pCurInfo, hvio) → ESP+4=hvio, +8=pCurInfo
            32 => self.vio_set_cur_type(read_stack(8), read_stack(4)),
            // VioSetAnsi(mode, hvio) → ESP+4=hvio, +8=mode
            5  => self.vio_set_ansi(read_stack(8), read_stack(4)),
            // VioGetAnsi(pMode, hvio) → ESP+4=hvio, +8=pMode
            3  => self.vio_get_ansi(read_stack(8), read_stack(4)),
            51 => { debug!("  VioSetState (stub)"); NO_ERROR },
            42 => { debug!("  VioSetCp (stub)"); NO_ERROR },
            // VioGetConfig(reserved, pConfig, hvio) → ESP+4=hvio, +8=pConfig, +12=reserved
            46 => self.vio_get_config(read_stack(12), read_stack(8)),
            22 => { debug!("  VioSetMode (stub)"); NO_ERROR },
            31 => { debug!("  VioGetBuf (stub)"); NO_ERROR },
            43 => { debug!("  VioShowBuf (stub)"); NO_ERROR },
            _ => { warn!("Warning: Unknown VIOCALLS Ordinal {}", ordinal); NO_ERROR }
        };
        super::ApiResult::Normal(res)
    }

    /// VioWrtTTY (ordinal 30): write string to display at current cursor position.
    fn vio_wrt_tty(&self, psz: u32, cb: u32, _hvio: u32) -> u32 {
        debug!("  VioWrtTTY(psz=0x{:08X}, cb={})", psz, cb);
        if cb == 0 || psz == 0 { return NO_ERROR; }
        let data: Vec<u8> = (0..cb).filter_map(|i| self.guest_read::<u8>(psz + i)).collect();
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.write_tty(&data, 0x07); // default attribute
        NO_ERROR
    }

    /// Pascal calling convention argument byte count for stack cleanup.
    pub(crate) fn viocalls_arg_bytes(&self, ordinal: u32) -> u64 {
        match ordinal {
            19 => 12, 21 => 8, 9 => 12, 15 => 12, 7 => 28,
            48 => 24, 52 => 20, 26 => 20, 24 => 20, 32 => 8,
            5 => 8, 3 => 8, 51 => 8, 42 => 12, 46 => 12,
            22 => 8, 31 => 12, 43 => 12, 8 => 4,
            _ => 0,
        }
    }

    /// VioGetMode (ordinal 3): get screen mode (rows/cols).
    fn vio_get_mode(&self, p_mode: u32, _hvio: u32) -> u32 {
        debug!("  VioGetMode(p_mode=0x{:08X}, hvio={})", p_mode, _hvio);
        let console = self.shared.console_mgr.lock_or_recover();
        if p_mode != 0 {
            // VIOMODEINFO struct (minimal): length=12, type=1, color=4, col, row, hres, vres
            self.guest_write::<u16>(p_mode, 12);         // cb (struct size)
            self.guest_write::<u8>(p_mode + 2, 1);       // fbType: text mode
            self.guest_write::<u8>(p_mode + 3, 4);       // color: 16 colors
            self.guest_write::<u16>(p_mode + 4, console.cols);
            self.guest_write::<u16>(p_mode + 6, console.rows);
            self.guest_write::<u16>(p_mode + 8, console.cols * 8);  // hres
            self.guest_write::<u16>(p_mode + 10, console.rows * 16); // vres
        }
        NO_ERROR
    }

    /// VioGetCurPos (ordinal 4): get cursor position.
    fn vio_get_cur_pos(&self, p_row: u32, p_col: u32, _hvio: u32) -> u32 {
        debug!("  VioGetCurPos");
        let console = self.shared.console_mgr.lock_or_recover();
        if p_row != 0 { self.guest_write::<u16>(p_row, console.cursor_row); }
        if p_col != 0 { self.guest_write::<u16>(p_col, console.cursor_col); }
        NO_ERROR
    }

    /// VioSetCurPos (ordinal 15): set cursor position.
    fn vio_set_cur_pos(&self, row: u32, col: u32, _hvio: u32) -> u32 {
        debug!("  VioSetCurPos({}, {})", row, col);
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.set_cursor_pos(row as u16, col as u16);
        NO_ERROR
    }

    /// VioScrollUp (ordinal 7): scroll a screen region up.
    fn vio_scroll_up(&self, top: u32, left: u32, bottom: u32, right: u32, lines: u32, _hvio: u32) -> u32 {
        debug!("  VioScrollUp(top={}, left={}, bottom={}, right={}, lines={})", top, left, bottom, right, lines);
        let fill_attr = if lines > 0 {
            // The 'lines' parameter in OS/2 is actually a pointer to a cell (char+attr)
            // But for the common case, we use lines as count and default fill
            0x07
        } else {
            0x07
        };
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.scroll_up(top as u16, bottom as u16, lines as u16, fill_attr);
        NO_ERROR
    }

    /// VioScrollDn (ordinal 8): scroll a screen region down.
    fn vio_scroll_dn(&self, top: u32, _left: u32, bottom: u32, _right: u32, lines: u32, _hvio: u32) -> u32 {
        debug!("  VioScrollDn(top={}, bottom={}, lines={})", top, bottom, lines);
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.scroll_down(top as u16, bottom as u16, lines as u16, 0x07);
        NO_ERROR
    }

    /// VioWrtCharStrAtt (ordinal 26): write attributed character string.
    fn vio_wrt_char_str_att(&self, psz: u32, cb: u32, row: u32, col: u32, p_attr: u32) -> u32 {
        debug!("  VioWrtCharStrAtt(cb={}, row={}, col={})", cb, row, col);
        let attr = if p_attr != 0 { self.guest_read::<u8>(p_attr).unwrap_or(0x07) } else { 0x07 };
        let data: Vec<u8> = (0..cb).filter_map(|i| self.guest_read::<u8>(psz + i)).collect();
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.write_char_str_att(row as u16, col as u16, &data, attr);
        NO_ERROR
    }

    /// VioWrtNCell (ordinal 28): write a cell (char+attr) N times.
    fn vio_wrt_n_cell(&self, p_cell: u32, count: u32, row: u32, col: u32, _hvio: u32) -> u32 {
        debug!("  VioWrtNCell(count={}, row={}, col={})", count, row, col);
        let ch = self.guest_read::<u8>(p_cell).unwrap_or(b' ');
        let attr = self.guest_read::<u8>(p_cell + 1).unwrap_or(0x07);
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.write_n_cell(row as u16, col as u16, (ch, attr), count as u16);
        NO_ERROR
    }

    /// VioWrtNAttr (ordinal 27): write an attribute N times.
    fn vio_wrt_n_attr(&self, p_attr: u32, count: u32, row: u32, col: u32, _hvio: u32) -> u32 {
        debug!("  VioWrtNAttr(count={}, row={}, col={})", count, row, col);
        let attr = self.guest_read::<u8>(p_attr).unwrap_or(0x07);
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.write_n_attr(row as u16, col as u16, attr, count as u16);
        NO_ERROR
    }

    /// VioReadCellStr (ordinal 24): read cell string from screen buffer.
    fn vio_read_cell_str(&self, p_buf: u32, pcb: u32, row: u32, col: u32, _hvio: u32) -> u32 {
        debug!("  VioReadCellStr(row={}, col={})", row, col);
        let max_len = self.guest_read::<u16>(pcb).unwrap_or(0);
        let console = self.shared.console_mgr.lock_or_recover();
        let cells = console.read_cell_str(row as u16, col as u16, max_len);
        let mut offset = 0u32;
        for (ch, attr) in &cells {
            if offset + 2 > max_len as u32 { break; }
            self.guest_write::<u8>(p_buf + offset, *ch);
            self.guest_write::<u8>(p_buf + offset + 1, *attr);
            offset += 2;
        }
        self.guest_write::<u16>(pcb, offset as u16);
        NO_ERROR
    }

    /// VioSetCurType (ordinal 16): set cursor shape/visibility.
    fn vio_set_cur_type(&self, p_cur_data: u32, _hvio: u32) -> u32 {
        debug!("  VioSetCurType");
        if p_cur_data != 0 {
            let attr = self.guest_read::<u16>(p_cur_data + 4).unwrap_or(0);
            let visible = (attr & 0xFFFF) != 0xFFFF; // -1 = hidden
            let mut console = self.shared.console_mgr.lock_or_recover();
            console.set_cursor_type(visible);
        }
        NO_ERROR
    }

    /// VioSetAnsi (ordinal 38): enable/disable ANSI mode.
    fn vio_set_ansi(&self, flag: u32, _hvio: u32) -> u32 {
        debug!("  VioSetAnsi({})", flag);
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.ansi_mode = flag != 0;
        NO_ERROR
    }

    /// VioGetAnsi (ordinal 39): query ANSI mode.
    fn vio_get_ansi(&self, p_flag: u32, _hvio: u32) -> u32 {
        debug!("  VioGetAnsi");
        let console = self.shared.console_mgr.lock_or_recover();
        if p_flag != 0 {
            self.guest_write::<u32>(p_flag, if console.ansi_mode { 1 } else { 0 });
        }
        NO_ERROR
    }

    /// VioGetConfig (ordinal 46): get video adapter configuration.
    fn vio_get_config(&self, _config_id: u32, p_config: u32) -> u32 {
        debug!("  VioGetConfig");
        if p_config != 0 {
            // VIOCONFIGINFO struct (minimal)
            self.guest_write::<u16>(p_config, 10);    // cb
            self.guest_write::<u16>(p_config + 2, 3); // adapter type: VGA
            self.guest_write::<u16>(p_config + 4, 3); // display type: VGA color
            self.guest_write::<u32>(p_config + 6, 0x10000); // adapter memory: 64KB
        }
        NO_ERROR
    }
}
