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
            // VioScrollUp(ulr, ulc, lrr, lrc, n, pCell, hvio) → ESP+4=hvio, +8=pCell, +12=n, +16=lrc, +20=lrr, +24=ulc, +28=ulr
            7  => self.vio_scroll_up(read_stack(28), read_stack(24), read_stack(20), read_stack(16), read_stack(12), read_stack(8), read_stack(4)),
            // VioScrollDn(ulr, ulc, lrr, lrc, n, pCell, hvio) — same layout as VioScrollUp
            8  => self.vio_scroll_dn(read_stack(28), read_stack(24), read_stack(20), read_stack(16), read_stack(12), read_stack(8), read_stack(4)),
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
            // VioGetCurType(pCurInfo, hvio) → ESP+4=hvio, +8=pCurInfo
            33 => self.vio_get_cur_type(read_stack(8), read_stack(4)),
            // VioSetAnsi(mode, hvio) → ESP+4=hvio, +8=mode
            5  => self.vio_set_ansi(read_stack(8), read_stack(4)),
            // VioGetAnsi(pMode, hvio) → ESP+4=hvio, +8=pMode
            3  => self.vio_get_ansi(read_stack(8), read_stack(4)),
            51 => { debug!("  VioSetState (stub)"); NO_ERROR },
            42 => { debug!("  VioSetCp (stub)"); NO_ERROR },
            // VioGetConfig(reserved, pConfig, hvio) → ESP+4=hvio, +8=pConfig, +12=reserved
            46 => self.vio_get_config(read_stack(12), read_stack(8)),
            // VioSetMode(pMode, hvio) → ESP+4=hvio, +8=pMode
            22 => self.vio_set_mode(read_stack(8), read_stack(4)),
            31 => { debug!("  VioGetBuf (stub)"); NO_ERROR },
            43 => { debug!("  VioShowBuf (stub)"); NO_ERROR },
            // VioCheckCharType(pType, usRow, usCol, hvio) → ESP+4=hvio, +8=usCol, +12=usRow, +16=pType
            39 => self.vio_check_char_type(read_stack(16), read_stack(12), read_stack(8), read_stack(4)),
            // VioWrtCellStr(pchCells, cb, usRow, usCol, hvio) — two possible ordinals:
            //   ordinal 10: as used by Open Watcom VIOCALLS.LIB (os2v2 target)
            //   ordinal 28: from older/alternative ordinal table
            // Both stubs return NO_ERROR; real implementation would write to VIO buffer.
            10 | 28 => { debug!("  VioWrtCellStr (stub)"); NO_ERROR },
            _ => { warn!("Warning: Unknown VIOCALLS Ordinal {}", ordinal); NO_ERROR }
        };
        super::ApiResult::Normal(res)
    }

    /// VioWrtTTY (ordinal 30): write string to display at current cursor position.
    fn vio_wrt_tty(&self, psz: u32, cb: u32, _hvio: u32) -> u32 {
        debug!("  VioWrtTTY(psz=0x{:08X}, cb={})", psz, cb);
        if cb == 0 || psz == 0 { return NO_ERROR; }
        let data: Vec<u8> = (0..cb).filter_map(|i| self.guest_read::<u8>(psz + i)).collect();
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.write_tty(&data, 0x07, cp);
        NO_ERROR
    }

    /// Pascal calling convention argument byte count for stack cleanup.
    pub(crate) fn viocalls_arg_bytes(&self, ordinal: u32) -> u64 {
        match ordinal {
            19 => 12, 21 => 8, 9 => 12, 15 => 12, 7 => 28, 8 => 28,
            48 => 24, 52 => 20, 26 => 20, 24 => 20, 32 => 8, 33 => 8,
            5 => 8, 3 => 8, 51 => 8, 42 => 12, 46 => 12,
            22 => 8, 31 => 12, 43 => 12,
            10 => 20, // VioWrtCellStr — ordinal as used by Open Watcom VIOCALLS.LIB
            28 => 20, // VioWrtCellStr — ordinal from alternative table
            39 => 16, // VioCheckCharType(pType, usRow, usCol, hvio) — 4 args × 4 bytes
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

    /// VioSetMode (ordinal 22): set screen mode (rows/cols).
    ///
    /// OS/2 VIOMODEINFO layout (first 12 bytes required):
    ///   +0 cb(u16)  +2 fbType(u8)  +3 color(u8)  +4 col(u16)  +6 row(u16)
    ///   +8 hres(u16)  +10 vres(u16)
    ///
    /// Only text mode (fbType == 1) is supported.  The SDL2 text window will
    /// resize automatically on the next rendered frame.
    fn vio_set_mode(&self, p_mode: u32, _hvio: u32) -> u32 {
        debug!("  VioSetMode(p_mode=0x{:08X})", p_mode);
        if p_mode == 0 { return ERROR_INVALID_FUNCTION; }

        let cb = self.guest_read::<u16>(p_mode).unwrap_or(0);
        if cb < 8 { return ERROR_INVALID_LEVEL; }

        let fb_type = self.guest_read::<u8>(p_mode + 2).unwrap_or(0);
        if fb_type != 1 {
            // Only text mode (fbType=1) is supported.
            return ERROR_INVALID_FUNCTION;
        }

        let new_cols = self.guest_read::<u16>(p_mode + 4).unwrap_or(80);
        let new_rows = self.guest_read::<u16>(p_mode + 6).unwrap_or(25);

        if new_cols < 1 || new_rows < 1 || new_cols > 255 || new_rows > 255 {
            return ERROR_INVALID_LEVEL;
        }

        let mut console = self.shared.console_mgr.lock_or_recover();
        console.resize(new_rows, new_cols);
        debug!("  VioSetMode → {}×{} text mode", new_cols, new_rows);
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
    /// `p_cell` points to a 2-byte (char, attr) fill cell; NULL → space/grey.
    #[allow(clippy::too_many_arguments)]
    fn vio_scroll_up(&self, top: u32, left: u32, bottom: u32, right: u32, lines: u32, p_cell: u32, _hvio: u32) -> u32 {
        debug!("  VioScrollUp(top={}, left={}, bottom={}, right={}, lines={}, p_cell=0x{:08X})", top, left, bottom, right, lines, p_cell);
        let (fill_cell, fill_raw) = if p_cell != 0 {
            let ch_byte = self.guest_read::<u8>(p_cell).unwrap_or(b' ');
            let attr    = self.guest_read::<u8>(p_cell + 1).unwrap_or(0x07);
            let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
            ((super::console::VioManager::decode_vio_byte(ch_byte, cp), attr), ch_byte)
        } else {
            ((' ', 0x07), b' ')
        };
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.scroll_up(top as u16, bottom as u16, lines as u16, fill_cell, fill_raw);
        NO_ERROR
    }

    /// VioScrollDn (ordinal 8): scroll a screen region down.
    /// `p_cell` points to a 2-byte (char, attr) fill cell; NULL → space/grey.
    #[allow(clippy::too_many_arguments)]
    fn vio_scroll_dn(&self, top: u32, _left: u32, bottom: u32, _right: u32, lines: u32, p_cell: u32, _hvio: u32) -> u32 {
        debug!("  VioScrollDn(top={}, bottom={}, lines={}, p_cell=0x{:08X})", top, bottom, lines, p_cell);
        let (fill_cell, fill_raw) = if p_cell != 0 {
            let ch_byte = self.guest_read::<u8>(p_cell).unwrap_or(b' ');
            let attr    = self.guest_read::<u8>(p_cell + 1).unwrap_or(0x07);
            let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
            ((super::console::VioManager::decode_vio_byte(ch_byte, cp), attr), ch_byte)
        } else {
            ((' ', 0x07), b' ')
        };
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.scroll_down(top as u16, bottom as u16, lines as u16, fill_cell, fill_raw);
        NO_ERROR
    }

    /// VioWrtCharStrAtt (ordinal 26): write attributed character string.
    fn vio_wrt_char_str_att(&self, psz: u32, cb: u32, row: u32, col: u32, p_attr: u32) -> u32 {
        debug!("  VioWrtCharStrAtt(cb={}, row={}, col={})", cb, row, col);
        let attr = if p_attr != 0 { self.guest_read::<u8>(p_attr).unwrap_or(0x07) } else { 0x07 };
        let data: Vec<u8> = (0..cb).filter_map(|i| self.guest_read::<u8>(psz + i)).collect();
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.write_char_str_att(row as u16, col as u16, &data, attr, cp);
        NO_ERROR
    }

    /// VioWrtNCell (ordinal 28): write a cell (char+attr) N times.
    fn vio_wrt_n_cell(&self, p_cell: u32, count: u32, row: u32, col: u32, _hvio: u32) -> u32 {
        debug!("  VioWrtNCell(count={}, row={}, col={})", count, row, col);
        let ch_byte = self.guest_read::<u8>(p_cell).unwrap_or(b' ');
        let attr = self.guest_read::<u8>(p_cell + 1).unwrap_or(0x07);
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let ch = super::console::VioManager::decode_vio_byte(ch_byte, cp);
        let mut console = self.shared.console_mgr.lock_or_recover();
        console.write_n_cell(row as u16, col as u16, ch_byte, (ch, attr), count as u16);
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
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let console = self.shared.console_mgr.lock_or_recover();
        let cells = console.read_cell_str(row as u16, col as u16, max_len);
        let mut offset = 0u32;
        for (ch, attr) in &cells {
            if offset + 2 > max_len as u32 { break; }
            // Re-encode the stored Unicode char back to the active codepage byte.
            let byte = if ch.is_ascii() {
                *ch as u8
            } else {
                let s: String = std::iter::once(*ch).collect();
                *super::codepage::cp_encode(&s, cp).first().unwrap_or(&b'?')
            };
            self.guest_write::<u8>(p_buf + offset, byte);
            self.guest_write::<u8>(p_buf + offset + 1, *attr);
            offset += 2;
        }
        self.guest_write::<u16>(pcb, offset as u16);
        NO_ERROR
    }

    /// VioSetCurType (ordinal 16): set cursor shape/visibility.
    ///
    /// VIOCURSORINFO layout: yStart(u16) cEnd(u16) cx(u16) attr(u16).
    /// attr == 0xFFFF means hidden.
    fn vio_set_cur_type(&self, p_cur_data: u32, _hvio: u32) -> u32 {
        debug!("  VioSetCurType");
        if p_cur_data != 0 {
            let y_start = self.guest_read::<u16>(p_cur_data).unwrap_or(14);
            let c_end   = self.guest_read::<u16>(p_cur_data + 2).unwrap_or(15);
            let attr    = self.guest_read::<u16>(p_cur_data + 6).unwrap_or(0);
            let visible = attr != 0xFFFF;
            let mut console = self.shared.console_mgr.lock_or_recover();
            console.set_cursor_type(visible);
            console.set_cursor_shape(y_start as u8, c_end as u8);
        }
        NO_ERROR
    }

    /// VioGetCurType (ordinal 33): read current cursor shape/visibility.
    ///
    /// VIOCURSORINFO layout: yStart(u16) cEnd(u16) cx(u16) attr(u16).
    /// attr == 0xFFFF means hidden; 0 means normal.
    fn vio_get_cur_type(&self, p_cur_data: u32, _hvio: u32) -> u32 {
        debug!("  VioGetCurType");
        if p_cur_data != 0 {
            let console = self.shared.console_mgr.lock_or_recover();
            let attr: u16 = if console.cursor_visible { 0 } else { 0xFFFF };
            self.guest_write::<u16>(p_cur_data,     console.cursor_start as u16);
            self.guest_write::<u16>(p_cur_data + 2, console.cursor_end   as u16);
            self.guest_write::<u16>(p_cur_data + 4, 0); // cx: default width
            self.guest_write::<u16>(p_cur_data + 6, attr);
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

    /// VioCheckCharType (ordinal 39): classify a screen cell as SBCS, DBCS lead, or DBCS trail.
    ///
    /// Scans from column 0 of the queried row left-to-right using `annotate_dbcs()` so
    /// mid-row queries correctly reflect the DBCS pairing state.  Writes:
    ///   0 → SBCS cell
    ///   2 → DBCS lead cell
    ///   3 → DBCS trail cell
    fn vio_check_char_type(&self, p_type: u32, row: u32, col: u32, _hvio: u32) -> u32 {
        use crate::gui::text_renderer::{annotate_dbcs, CellKind};
        debug!("  VioCheckCharType(p_type=0x{:08X}, row={}, col={})", p_type, row, col);
        let console = self.shared.console_mgr.lock_or_recover();
        let rows = console.rows as u32;
        let cols = console.cols as u32;
        if row >= rows { return ERROR_VIO_ROW; }
        if col >= cols { return ERROR_VIO_COL; }
        let cp = console.codepage as u32;
        let row_start = (row * cols) as usize;
        let row_bytes = &console.raw_bytes[row_start..row_start + cols as usize];
        let kinds = annotate_dbcs(row_bytes, cp, cols as u16);
        if p_type != 0 {
            let type_val: u16 = match kinds[col as usize] {
                CellKind::Sbcs     => 0,
                CellKind::DbcsLead => 2,
                CellKind::DbcsTail => 3,
            };
            let _ = self.guest_write::<u16>(p_type, type_val);
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

#[cfg(test)]
mod tests {
    use super::super::{Loader, ApiResult};
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::mutex_ext::MutexExt;

    /// Write `args` to the Pascal call stack at `esp`: last arg at esp+4,
    /// second-to-last at esp+8, etc. (matches OS/2 Pascal calling convention).
    fn write_stack(loader: &Loader, esp: u32, args: &[u32]) {
        for (i, &arg) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, arg).unwrap();
        }
    }

    #[test]
    fn test_vio_get_mode_writes_viomodeinfo() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32    = 0x1000;
        let p_mode: u32 = 0x2000;
        vcpu.regs.rsp = esp as u64;
        // VioGetMode(pMode, hvio) → ESP+4=hvio, +8=pMode
        write_stack(&loader, esp, &[/*hvio*/0, /*pMode*/p_mode]);

        let result = loader.handle_viocalls(&mut vcpu, 0, 21);
        assert!(matches!(result, ApiResult::Normal(0)));

        // VIOMODEINFO: cb=12, fbType=1 (text), color=4 (16-colour)
        assert_eq!(loader.guest_read::<u16>(p_mode).unwrap(),     12);  // cb
        assert_eq!(loader.guest_read::<u8>(p_mode + 2).unwrap(),   1);  // fbType
        assert_eq!(loader.guest_read::<u8>(p_mode + 3).unwrap(),   4);  // color
        // cols/rows are terminal-dependent; just check they are sane values
        let cols = loader.guest_read::<u16>(p_mode + 4).unwrap();
        let rows = loader.guest_read::<u16>(p_mode + 6).unwrap();
        assert!(cols >= 80, "cols={cols}");
        assert!(rows >= 25, "rows={rows}");
        // hres = cols*8, vres = rows*16
        assert_eq!(loader.guest_read::<u16>(p_mode + 8).unwrap(),  cols * 8);
        assert_eq!(loader.guest_read::<u16>(p_mode + 10).unwrap(), rows * 16);
    }

    #[test]
    fn test_vio_get_mode_null_ptr_is_noop() {
        // p_mode == 0 → handler must not write anything (no panic/OOB)
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        write_stack(&loader, esp, &[0, 0]); // hvio=0, pMode=0 (null)
        let result = loader.handle_viocalls(&mut vcpu, 0, 21);
        assert!(matches!(result, ApiResult::Normal(0)));
    }

    #[test]
    fn test_vio_set_and_get_cur_pos() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;

        // VioSetCurPos(row=5, col=10, hvio=0) → ESP+4=hvio, +8=col, +12=row
        write_stack(&loader, esp, &[0, 10, 5]);
        loader.handle_viocalls(&mut vcpu, 0, 15);

        // VioGetCurPos(pRow, pCol, hvio) → ESP+4=hvio, +8=pCol, +12=pRow
        let p_row: u32 = 0x2000;
        let p_col: u32 = 0x2002;
        write_stack(&loader, esp, &[0, p_col, p_row]);
        let result = loader.handle_viocalls(&mut vcpu, 0, 9);
        assert!(matches!(result, ApiResult::Normal(0)));

        assert_eq!(loader.guest_read::<u16>(p_row).unwrap(), 5);
        assert_eq!(loader.guest_read::<u16>(p_col).unwrap(), 10);
    }

    #[test]
    fn test_vio_wrt_tty_null_or_zero_len_is_noop() {
        // Both null pointer and zero length should return NO_ERROR without panic
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;

        // pStr=NULL, len=0
        write_stack(&loader, esp, &[0, 0, 0]); // hvio, len, pStr
        let result = loader.handle_viocalls(&mut vcpu, 0, 19);
        assert!(matches!(result, ApiResult::Normal(0)));
    }

    #[test]
    fn test_vio_get_config_writes_struct() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32      = 0x1000;
        let p_config: u32 = 0x2000;
        vcpu.regs.rsp = esp as u64;
        // VioGetConfig(reserved, pConfig, hvio) → ESP+4=hvio, +8=pConfig, +12=reserved
        write_stack(&loader, esp, &[0, p_config, 0]);
        let result = loader.handle_viocalls(&mut vcpu, 0, 46);
        assert!(matches!(result, ApiResult::Normal(0)));

        assert_eq!(loader.guest_read::<u16>(p_config).unwrap(),     10); // cb
        assert_eq!(loader.guest_read::<u16>(p_config + 2).unwrap(),  3); // VGA adapter
        assert_eq!(loader.guest_read::<u16>(p_config + 4).unwrap(),  3); // VGA color display
    }

    /// VioScrollDn (ordinal 8): dispatch must call the real implementation, not a stub.
    /// Before the fix, ordinal 8 was mislabelled "VioPrtSc" with only 4 arg-bytes,
    /// which corrupted the Pascal calling-convention stack by 24 bytes on every call.
    #[test]
    fn test_vio_scroll_dn_ordinal_and_arg_bytes() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;

        // VioScrollDn(ulr, ulc, lrr, lrc, n, pCell, hvio)
        // Pascal layout (last arg at esp+4): hvio, pCell, n, lrc, lrr, ulc, ulr
        write_stack(&loader, esp, &[/*hvio*/0, /*pCell*/0, /*n*/2, /*lrc*/79, /*lrr*/24, /*ulc*/0, /*ulr*/0]);
        let result = loader.handle_viocalls(&mut vcpu, 0, 8);
        // Must return NO_ERROR (not a stub panic/wrong ordinal)
        assert!(matches!(result, ApiResult::Normal(0)));

        // Arg-byte count must match VioScrollDn's 7 args × 4 bytes = 28,
        // so the vCPU loop adjusts rsp by 28 (not 4 as the old bug had it).
        assert_eq!(loader.viocalls_arg_bytes(8), 28,
            "Wrong arg-byte count for VioScrollDn (ordinal 8): stack corruption bug");
    }

    /// VioScrollUp/VioScrollDn: pCell pointer is read and used as fill char+attr.
    #[test]
    fn test_vio_scroll_up_p_cell_fill() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32    = 0x1000;
        let p_cell: u32 = 0x3000;
        vcpu.regs.rsp = esp as u64;

        // Write a custom fill cell: '.' (0x2E) with attr 0x4F (white on red)
        loader.guest_write::<u8>(p_cell,     b'.').unwrap();
        loader.guest_write::<u8>(p_cell + 1, 0x4F).unwrap();

        // Seed row 1 with some content, then scroll it up so row 0 gets it
        // and the bottom (row 24) should be filled with '.' / 0x4F.
        {
            let mut con = loader.shared.console_mgr.lock_or_recover();
            con.enable_sdl2_mode(); // fix to 80x25
            let cols = con.cols as usize;
            con.buffer[cols] = ('Z', 0x07); // row 1, col 0
        }

        // VioScrollUp(ulr=0, ulc=0, lrr=24, lrc=79, n=1, pCell=p_cell, hvio=0)
        // Pascal layout: hvio, pCell, n, lrc, lrr, ulc, ulr
        write_stack(&loader, esp, &[/*hvio*/0, /*pCell*/p_cell, /*n*/1, /*lrc*/79, /*lrr*/24, /*ulc*/0, /*ulr*/0]);
        let result = loader.handle_viocalls(&mut vcpu, 0, 7);
        assert!(matches!(result, ApiResult::Normal(0)));

        let con = loader.shared.console_mgr.lock_or_recover();
        // Row 0 should now contain what was in row 1
        assert_eq!(con.buffer[0], ('Z', 0x07), "row 0 should have row-1 content after scroll-up");
        // Last row should be filled with the custom fill cell
        let last_row_start = 24 * con.cols as usize;
        assert_eq!(con.buffer[last_row_start], ('.', 0x4F), "bottom row should be filled with pCell value");
    }

    /// VioScrollDn with pCell: fill row at top with the custom cell.
    #[test]
    fn test_vio_scroll_dn_p_cell_fill() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32    = 0x1000;
        let p_cell: u32 = 0x3000;
        vcpu.regs.rsp = esp as u64;

        loader.guest_write::<u8>(p_cell,     b'*').unwrap();
        loader.guest_write::<u8>(p_cell + 1, 0x1A).unwrap(); // green on blue

        {
            let mut con = loader.shared.console_mgr.lock_or_recover();
            con.enable_sdl2_mode();
            con.buffer[0] = ('Q', 0x07); // row 0, col 0
        }

        // VioScrollDn: row 0 → row 1, top row filled with '*'/0x1A
        write_stack(&loader, esp, &[/*hvio*/0, /*pCell*/p_cell, /*n*/1, /*lrc*/79, /*lrr*/24, /*ulc*/0, /*ulr*/0]);
        let result = loader.handle_viocalls(&mut vcpu, 0, 8);
        assert!(matches!(result, ApiResult::Normal(0)));

        let con = loader.shared.console_mgr.lock_or_recover();
        let cols = con.cols as usize;
        assert_eq!(con.buffer[cols], ('Q', 0x07), "row 1 should have row-0 content after scroll-dn");
        assert_eq!(con.buffer[0], ('*', 0x1A), "top row should be filled with pCell value");
    }

    #[test]
    fn test_vio_get_cur_type_reflects_set() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32        = 0x1000;
        let p_cur_data: u32 = 0x2000;
        vcpu.regs.rsp = esp as u64;

        // Set cursor to yStart=6, cEnd=7, attr=0 (visible block)
        loader.guest_write::<u16>(p_cur_data,     6).unwrap();   // yStart
        loader.guest_write::<u16>(p_cur_data + 2, 7).unwrap();   // cEnd
        loader.guest_write::<u16>(p_cur_data + 4, 0).unwrap();   // cx
        loader.guest_write::<u16>(p_cur_data + 6, 0).unwrap();   // attr = visible
        write_stack(&loader, esp, &[0, p_cur_data]); // hvio=0, pCurInfo=p_cur_data
        loader.handle_viocalls(&mut vcpu, 0, 32);  // VioSetCurType

        // Now read it back with VioGetCurType (ordinal 33)
        let p_out: u32 = 0x3000;
        write_stack(&loader, esp, &[0, p_out]);
        let result = loader.handle_viocalls(&mut vcpu, 0, 33);
        assert!(matches!(result, ApiResult::Normal(0)));

        assert_eq!(loader.guest_read::<u16>(p_out).unwrap(),     6,      "yStart");
        assert_eq!(loader.guest_read::<u16>(p_out + 2).unwrap(), 7,      "cEnd");
        assert_eq!(loader.guest_read::<u16>(p_out + 6).unwrap(), 0,      "attr (visible)");

        // Hide cursor via VioSetCurType (attr = 0xFFFF)
        loader.guest_write::<u16>(p_cur_data + 6, 0xFFFF).unwrap();
        write_stack(&loader, esp, &[0, p_cur_data]);
        loader.handle_viocalls(&mut vcpu, 0, 32);

        // VioGetCurType should now report hidden
        write_stack(&loader, esp, &[0, p_out]);
        loader.handle_viocalls(&mut vcpu, 0, 33);
        assert_eq!(loader.guest_read::<u16>(p_out + 6).unwrap(), 0xFFFF, "attr (hidden)");
    }

    #[test]
    fn test_vio_set_mode_resizes_console() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32    = 0x1000;
        let p_mode: u32 = 0x2000;
        vcpu.regs.rsp = esp as u64;

        // Build a VIOMODEINFO: cb=12, fbType=1 (text), color=4, cols=132, rows=50
        loader.guest_write::<u16>(p_mode,      12).unwrap(); // cb
        loader.guest_write::<u8>(p_mode + 2,    1).unwrap(); // fbType = text
        loader.guest_write::<u8>(p_mode + 3,    4).unwrap(); // color = 16
        loader.guest_write::<u16>(p_mode + 4, 132).unwrap(); // cols
        loader.guest_write::<u16>(p_mode + 6,  50).unwrap(); // rows

        // VioSetMode(pMode, hvio) → ESP+4=hvio, +8=pMode
        write_stack(&loader, esp, &[/*hvio*/0, /*pMode*/p_mode]);
        let result = loader.handle_viocalls(&mut vcpu, 0, 22);
        assert!(matches!(result, super::super::ApiResult::Normal(0)));

        let con = loader.shared.console_mgr.lock_or_recover();
        assert_eq!(con.cols, 132, "cols should be updated to 132");
        assert_eq!(con.rows, 50,  "rows should be updated to 50");
        assert_eq!(con.buffer.len(), 132 * 50, "buffer resized");
    }

    #[test]
    fn test_vio_set_mode_invalid_type_rejected() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32    = 0x1000;
        let p_mode: u32 = 0x2000;
        vcpu.regs.rsp = esp as u64;

        // fbType = 0 (graphics mode) — not supported
        loader.guest_write::<u16>(p_mode,     12).unwrap();
        loader.guest_write::<u8>(p_mode + 2,   0).unwrap(); // graphics
        loader.guest_write::<u8>(p_mode + 3,   4).unwrap();
        loader.guest_write::<u16>(p_mode + 4, 80).unwrap();
        loader.guest_write::<u16>(p_mode + 6, 25).unwrap();
        write_stack(&loader, esp, &[0, p_mode]);
        let result = loader.handle_viocalls(&mut vcpu, 0, 22);
        assert!(!matches!(result, super::super::ApiResult::Normal(0)),
            "graphics mode must be rejected");
    }

    #[test]
    fn test_vio_set_mode_roundtrip_via_get_mode() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32    = 0x1000;
        let p_mode: u32 = 0x2000;
        vcpu.regs.rsp = esp as u64;

        // Set 40×12
        loader.guest_write::<u16>(p_mode,     12).unwrap();
        loader.guest_write::<u8>(p_mode + 2,   1).unwrap();
        loader.guest_write::<u8>(p_mode + 3,   4).unwrap();
        loader.guest_write::<u16>(p_mode + 4, 40).unwrap();
        loader.guest_write::<u16>(p_mode + 6, 12).unwrap();
        write_stack(&loader, esp, &[0, p_mode]);
        loader.handle_viocalls(&mut vcpu, 0, 22);

        // Read back with VioGetMode
        let p_out: u32 = 0x3000;
        write_stack(&loader, esp, &[0, p_out]);
        let result = loader.handle_viocalls(&mut vcpu, 0, 21);
        assert!(matches!(result, super::super::ApiResult::Normal(0)));
        assert_eq!(loader.guest_read::<u16>(p_out + 4).unwrap(), 40);
        assert_eq!(loader.guest_read::<u16>(p_out + 6).unwrap(), 12);
        assert_eq!(loader.guest_read::<u16>(p_out + 8).unwrap(),  40 * 8);
        assert_eq!(loader.guest_read::<u16>(p_out + 10).unwrap(), 12 * 16);
    }

    #[test]
    fn test_vio_ansi_set_and_get() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32    = 0x1000;
        let p_flag: u32 = 0x2000;
        vcpu.regs.rsp = esp as u64;

        // VioSetAnsi(mode=1, hvio=0)
        write_stack(&loader, esp, &[0, 1]);
        loader.handle_viocalls(&mut vcpu, 0, 5);

        // VioGetAnsi(pMode, hvio=0)
        write_stack(&loader, esp, &[0, p_flag]);
        loader.handle_viocalls(&mut vcpu, 0, 3);
        assert_eq!(loader.guest_read::<u32>(p_flag).unwrap(), 1);

        // VioSetAnsi(mode=0)
        write_stack(&loader, esp, &[0, 0]);
        loader.handle_viocalls(&mut vcpu, 0, 5);
        loader.handle_viocalls(&mut vcpu, 0, 3); // VioGetAnsi again (same p_flag)
        write_stack(&loader, esp, &[0, p_flag]);
        loader.handle_viocalls(&mut vcpu, 0, 3);
        assert_eq!(loader.guest_read::<u32>(p_flag).unwrap(), 0);
    }

    // ── VioCheckCharType ──────────────────────────────────────────────────────

    #[test]
    fn test_vio_check_char_type_sbcs_returns_zero() {
        let loader = Loader::new_mock();
        // Default codepage is CP437 (SBCS) — every cell must be type 0.
        let p_type: u32 = 0x2000;
        assert_eq!(loader.vio_check_char_type(p_type, 0, 0, 0), 0);
        assert_eq!(loader.guest_read::<u16>(p_type), Some(0));
    }

    #[test]
    fn test_vio_check_char_type_dbcs_lead_and_trail() {
        use std::sync::atomic::Ordering;
        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(936, Ordering::Relaxed);
        // Plant a CP936 DBCS pair (0xC4 0xE3) at row 0, col 0.
        {
            let mut console = loader.shared.console_mgr.lock_or_recover();
            console.codepage = 936;
            console.raw_bytes[0] = 0xC4;
            console.raw_bytes[1] = 0xE3;
        }
        let p_type: u32 = 0x2000;
        // Col 0 = DBCS lead → type 2
        assert_eq!(loader.vio_check_char_type(p_type, 0, 0, 0), 0);
        assert_eq!(loader.guest_read::<u16>(p_type), Some(2));
        // Col 1 = DBCS tail → type 3
        assert_eq!(loader.vio_check_char_type(p_type, 0, 1, 0), 0);
        assert_eq!(loader.guest_read::<u16>(p_type), Some(3));
    }

    #[test]
    fn test_vio_check_char_type_oob_row_returns_error() {
        let loader = Loader::new_mock();
        let rows = loader.shared.console_mgr.lock_or_recover().rows;
        // row >= rows is out of bounds.
        assert_ne!(loader.vio_check_char_type(0x2000, rows as u32, 0, 0), 0);
    }

    /// VioWrtCellStr: stub must return NO_ERROR and its Pascal arg-byte count
    /// must be 20 (5 args × 4 bytes) for BOTH ordinals (10 = Open Watcom
    /// VIOCALLS.LIB, 28 = alternative table). The wrong ordinal (0 arg-bytes)
    /// was the root cause of the `Passed: 127928197` stack-corruption bug.
    #[test]
    fn test_vio_wrt_cell_str_stub_and_arg_bytes() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;

        // VioWrtCellStr(pchCells, cb, usRow, usCol, hvio)
        // Pascal layout (last arg at esp+4): hvio, usCol, usRow, cb, pchCells
        write_stack(&loader, esp, &[/*hvio*/0, /*usCol*/0u32, /*usRow*/0u32, /*cb*/2u32, /*pchCells*/0x3000u32]);

        // Ordinal 10 (Open Watcom VIOCALLS.LIB os2v2 ordinal)
        let result = loader.handle_viocalls(&mut vcpu, 0, 10);
        assert!(matches!(result, ApiResult::Normal(0)), "VioWrtCellStr (ord 10) must return NO_ERROR");
        assert_eq!(loader.viocalls_arg_bytes(10), 20,
            "Wrong arg-byte count for VioWrtCellStr (ordinal 10): stack corruption bug");

        // Ordinal 28 (alternative ordinal table)
        let result = loader.handle_viocalls(&mut vcpu, 0, 28);
        assert!(matches!(result, ApiResult::Normal(0)), "VioWrtCellStr (ord 28) must return NO_ERROR");
        assert_eq!(loader.viocalls_arg_bytes(28), 20,
            "Wrong arg-byte count for VioWrtCellStr (ordinal 28): stack corruption bug");
    }

    /// VioCheckCharType (ordinal 39): Pascal arg-byte count must be 16
    /// (4 args × 4 bytes). Missing this entry was part of the dbcs_test
    /// stack-corruption bug.
    #[test]
    fn test_vio_check_char_type_arg_bytes() {
        let loader = Loader::new_mock();
        assert_eq!(loader.viocalls_arg_bytes(39), 16,
            "Wrong arg-byte count for VioCheckCharType (ordinal 39): stack corruption bug");
    }
}
