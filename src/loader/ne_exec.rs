// SPDX-License-Identifier: GPL-3.0-only

use super::constants::*;
use super::MutexExt;
use crate::ne::NeFile;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::Ordering;
use super::vm_backend::{VcpuBackend, GuestSegment};
use log::{info, debug, warn};

impl super::Loader {
    // ── NE (16-bit) loader methods ──

    /// Load an NE (16-bit) executable into guest memory.
    pub fn load_ne<P: AsRef<Path>>(&mut self, ne_file: &NeFile, path: P) -> io::Result<()> {
        let mut file = File::open(path)?;
        let shift = ne_file.header.alignment_shift_count;

        for (i, seg) in ne_file.segment_table.iter().enumerate() {
            let guest_addr = NE_SEGMENT_BASE + (i as u32) * TILE_SIZE;
            let data_len = seg.actual_data_length();
            let file_off = seg.file_offset(shift);

            if data_len > 0 && seg.data_offset != 0 {
                file.seek(SeekFrom::Start(file_off))?;
                let buf = self.guest_slice_mut(guest_addr, data_len as usize)
                    .expect("load_ne: segment target OOB");
                file.read_exact(buf)?;
            }
            debug!("  NE Segment {}: {} bytes at 0x{:08X} ({})",
                i + 1, data_len, guest_addr, if seg.is_code() { "CODE" } else { "DATA" });
        }

        self.apply_ne_fixups(ne_file, &mut file)
    }

    /// Apply NE relocations for all segments.
    fn apply_ne_fixups(&self, ne_file: &NeFile, _file: &mut File) -> io::Result<()> {
        use crate::ne::header::*;

        // Build segment-to-selector/base map (1-based segment numbers)
        let seg_base = |seg_num: u8| -> u32 {
            NE_SEGMENT_BASE + ((seg_num as u32).wrapping_sub(1)) * TILE_SIZE
        };
        let seg_selector = |seg_num: u8| -> u16 {
            let tile_idx = (NE_SEGMENT_BASE / TILE_SIZE) + (seg_num as u32).wrapping_sub(1);
            ((TILED_SEL_START_INDEX + tile_idx) * 8) as u16
        };

        for (seg_idx, relocs) in ne_file.relocations_by_segment.iter().enumerate() {
            let seg_guest_base = seg_base((seg_idx + 1) as u8);

            for reloc in relocs {
                // Resolve the target value
                let (target_offset, target_selector) = match &reloc.target {
                    NeRelocationTarget::InternalRef { segment_num, offset } => {
                        let sel = seg_selector(*segment_num);
                        debug!("  NE fixup: InternalRef seg{}+0x{:04X} -> sel=0x{:04X}, type={}",
                            segment_num, offset, sel, reloc.source_type);
                        (*offset, sel)
                    }
                    NeRelocationTarget::ImportOrdinal { module_index, ordinal } => {
                        let mod_name = ne_file.imported_modules
                            .get((*module_index as usize).wrapping_sub(1))
                            .map(|s| s.as_str())
                            .unwrap_or("?");
                        debug!("  NE fixup: {}.#{} -> thunk", mod_name, ordinal);
                        // CALL FAR requires a code (execute) descriptor — use NE_THUNK_CODE_SELECTOR
                        let thunk_offset = self.resolve_import_16(mod_name, *ordinal);
                        (thunk_offset, NE_THUNK_CODE_SELECTOR)
                    }
                    NeRelocationTarget::ImportName { module_index, name_offset: _ } => {
                        let mod_name = ne_file.imported_modules
                            .get((*module_index as usize).wrapping_sub(1))
                            .map(|s| s.as_str())
                            .unwrap_or("?");
                        warn!("NE import-by-name from {} not implemented", mod_name);
                        (0, NE_THUNK_SELECTOR)
                    }
                    NeRelocationTarget::OsFixup { fixup_type } => {
                        debug!("  NE OS fixup type {}", fixup_type);
                        // OS fixup type 1 = floating-point fixup (NOP on 386+)
                        continue;
                    }
                };

                let is_additive = reloc.flags & RELFLAG_ADDITIVE != 0;

                // Apply the fixup, following the chain for non-additive fixups
                let mut offset = reloc.source_offset;
                loop {
                    let patch_addr = seg_guest_base + offset as u32;
                    // Read the next chain offset BEFORE writing (non-additive fixups store chain pointers)
                    let next_offset = if !is_additive {
                        self.guest_read::<u16>(patch_addr).unwrap_or(0xFFFF)
                    } else {
                        0xFFFF
                    };

                    match reloc.source_type {
                        RELOC_FAR_POINTER => {
                            // Write offset:selector (4 bytes)
                            self.guest_write::<u16>(patch_addr, target_offset);
                            self.guest_write::<u16>(patch_addr + 2, target_selector);
                        }
                        RELOC_SELECTOR => {
                            // Write just the selector (2 bytes)
                            self.guest_write::<u16>(patch_addr, target_selector);
                        }
                        RELOC_OFFSET => {
                            // Write just the offset (2 bytes)
                            self.guest_write::<u16>(patch_addr, target_offset);
                        }
                        RELOC_LOBYTE => {
                            // Write low byte of offset
                            self.guest_write::<u8>(patch_addr, target_offset as u8);
                        }
                        _ => {
                            warn!("Unknown NE relocation source type: {}", reloc.source_type);
                        }
                    }

                    // Follow chain (non-additive: each location stores pointer to next)
                    if is_additive || next_offset == 0xFFFF {
                        break;
                    }
                    offset = next_offset;
                }
            }
        }
        Ok(())
    }

    /// Resolve a 16-bit import to an offset within the NE thunk tile.
    /// Uses the same module base offsets as 32-bit (DOSCALLS=0, KBDCALLS=4096, etc.)
    pub(crate) fn resolve_import_16(&self, module: &str, ordinal: u16) -> u16 {
        let base: u16 = if module == "DOSCALLS" { 0 }
            else if module == "VIOCALLS" { VIOCALLS_BASE as u16 }
            else if module == "KBDCALLS" { KBDCALLS_BASE as u16 }
            else if module == "SESMGR" { SESMGR_BASE as u16 }
            else if module == "NLS" { NLS_BASE as u16 }
            else if module == "MSG" { MSG_BASE as u16 }
            else {
                warn!("Unknown 16-bit import module: {} ordinal {}", module, ordinal);
                STUB_AREA_SIZE as u16 - 1
            };
        base + ordinal
    }

    /// Set up guest memory for an NE executable and return (entry_cs_sel, entry_ip, ss_sel, sp).
    fn setup_guest_ne(&self, ne_file: &NeFile) -> (u16, u16, u16, u16) {
        // Reuse common setup: TIB, PIB, environment, BDA
        let exe_name = self.shared.exe_name.lock_or_recover().clone();
        let os2_exe = if exe_name.is_empty() { String::from("C:\\APP.EXE") }
        else {
            let basename = std::path::Path::new(&exe_name)
                .file_name()
                .map(|f| f.to_string_lossy().to_uppercase())
                .unwrap_or_else(|| "APP.EXE".into());
            format!("C:\\{}", basename)
        };
        let mut env_block: Vec<u8> = Vec::new();
        env_block.extend_from_slice(b"PATH=C:\\\0");
        env_block.extend_from_slice(b"COMSPEC=C:\\CMD.EXE\0");
        env_block.extend_from_slice(b"OS=OS2\0");
        env_block.push(0);
        let cmdline_offset = env_block.len() as u32;
        env_block.extend_from_slice(os2_exe.as_bytes());
        env_block.push(0);
        let env_addr = self.shared.mem_mgr.lock_or_recover()
            .alloc(env_block.len() as u32)
            .expect("setup_guest_ne: env alloc failed");
        let cmdline_addr = env_addr + cmdline_offset;
        self.guest_write_bytes(env_addr, &env_block).expect("setup_guest_ne: env write OOB");

        // TIB/PIB setup
        let tib2_addr = TIB_BASE + 0x40;
        self.guest_write::<u32>(TIB_BASE + 0x0C, tib2_addr).unwrap();
        self.guest_write::<u32>(TIB_BASE + 0x18, TIB_BASE).unwrap();
        self.guest_write::<u32>(TIB_BASE + 0x30, PIB_BASE).unwrap();
        self.guest_write::<u32>(tib2_addr, 1).unwrap();
        self.guest_write::<u32>(tib2_addr + 0x04, 0).unwrap();
        self.guest_write::<u32>(PIB_BASE, 42).unwrap();
        self.guest_write::<u32>(PIB_BASE + 0x0C, cmdline_addr).unwrap();
        self.guest_write::<u32>(PIB_BASE + 0x10, env_addr).unwrap();

        // BDA
        {
            let console = self.shared.console_mgr.lock_or_recover();
            let cols = console.cols;
            let rows = console.rows;
            drop(console);
            self.guest_write::<u8>(0x449, 0x03).unwrap();
            self.guest_write::<u16>(0x44A, cols).unwrap();
            self.guest_write::<u16>(0x44C, cols * rows * 2).unwrap();
            self.guest_write::<u16>(0x44E, 0).unwrap();
            self.guest_write::<u16>(0x450, 0).unwrap();
            self.guest_write::<u8>(0x462, 0).unwrap();
            self.guest_write::<u16>(0x463, 0x3D4).unwrap();
            self.guest_write::<u8>(0x484, (rows - 1) as u8).unwrap();
            self.guest_write::<u16>(0x485, 16).unwrap();
        }

        // Set up 32-bit API stubs (still needed for some internal dispatches)
        self.setup_stubs();

        // Set up the full tiled GDT and IDT using the same path as LX executables.
        // This writes all 6150 GDT entries (including data/code tiles for all NE
        // segment selectors and the NE thunk tile at GDT[NE_THUNK_GDT_INDEX]) and
        // the 32-bit interrupt-gate IDT stubs required by the VMEXIT exception handler.
        self.setup_idt();

        // Set up 16-bit API thunk tile at NE_THUNK_BASE: fill with INT 3.
        // The thunk tile GDT entry (a code tile at NE_THUNK_TILE_INDEX) is already
        // written by setup_idt() as a 16-bit execute/read descriptor (access=0x9B).
        for i in 0..TILE_SIZE {
            self.guest_write::<u8>(NE_THUNK_BASE + i, 0xCC).expect("setup_ne_thunks: write OOB");
        }
        debug!("  NE thunk tile: GDT[{}] selector=0x{:04X} base=0x{:08X}",
            NE_THUNK_GDT_INDEX, NE_THUNK_SELECTOR, NE_THUNK_BASE);

        // Update NE segment GDT entries with accurate byte-granular limits.
        // setup_idt() writes all tiles with limit=0xFFFF; here we tighten each
        // NE segment to its actual allocation size so out-of-bounds accesses fault.
        for (i, seg) in ne_file.segment_table.iter().enumerate() {
            let guest_base = NE_SEGMENT_BASE + (i as u32) * TILE_SIZE;
            let tile_idx = guest_base / TILE_SIZE;
            let gdt_idx = TILED_SEL_START_INDEX + tile_idx;
            let selector = gdt_idx * 8;
            let limit = seg.actual_min_alloc().saturating_sub(1).min(0xFFFF);
            // Data segments use DPL=2 (0xD3) to match the tiled data tile base, allowing
            // OS/2 ring-2 RPL selectors to be loaded without a protection fault.
            let access = if seg.is_code() { 0x9B } else { 0xD3 };
            let gdt_offset = GDT_BASE + gdt_idx * 8;
            self.guest_write::<u64>(gdt_offset,
                Self::make_gdt_entry(guest_base, limit, access, 0x00)).unwrap();
            debug!("  GDT[{}] = NE seg {} ({}) at 0x{:08X}, limit=0x{:04X}, selector=0x{:04X}",
                gdt_idx, i + 1, if seg.is_code() { "CODE" } else { "DATA" },
                guest_base, limit, selector);
        }

        // Return entry point and stack as selectors
        let entry_cs = ne_file.header.entry_cs();
        let entry_ip = ne_file.header.entry_ip();
        let stack_ss = ne_file.header.stack_ss();
        let stack_sp = ne_file.header.stack_sp();

        let cs_sel = if entry_cs > 0 {
            let tile_idx = (NE_SEGMENT_BASE / TILE_SIZE) + (entry_cs as u32 - 1);
            ((TILED_SEL_START_INDEX + tile_idx) * 8) as u16
        } else { 0x08 }; // fallback to 32-bit code

        let ss_sel = if stack_ss > 0 {
            let tile_idx = (NE_SEGMENT_BASE / TILE_SIZE) + (stack_ss as u32 - 1);
            ((TILED_SEL_START_INDEX + tile_idx) * 8) as u16
        } else { 0x10 }; // fallback to 32-bit data

        // SP=0 in 16-bit means top of segment (wrap around)
        let actual_sp = if stack_sp == 0 {
            let ss_seg = &ne_file.segment_table[(stack_ss as usize).wrapping_sub(1)];
            ss_seg.actual_min_alloc() as u16
        } else { stack_sp };

        info!("NE entry: CS:IP = 0x{:04X}:0x{:04X}, SS:SP = 0x{:04X}:0x{:04X}",
            cs_sel, entry_ip, ss_sel, actual_sp);

        (cs_sel, entry_ip, ss_sel, actual_sp)
    }

    /// Run an NE (16-bit) CLI application.
    pub fn setup_and_run_ne_cli(self, ne_file: &NeFile) -> ! {
        let (cs_sel, entry_ip, ss_sel, sp) = self.setup_guest_ne(ne_file);

        let mut vcpu = self.vm.create_vcpu(0).unwrap();
        let mut regs = vcpu.get_regs().unwrap();
        regs.rip = entry_ip as u64;
        regs.rsp = sp as u64;
        regs.rflags = 2;
        vcpu.set_regs(&regs).unwrap();

        // Set up 16-bit protected mode segments
        let mut sregs = vcpu.get_sregs().unwrap();
        // GDT — use the full tiled GDT written by setup_idt()
        sregs.gdt_base  = GDT_BASE as u64;
        sregs.gdt_limit = GDT_SIZE - 1;
        // IDT
        sregs.idt_base  = IDT_BASE as u64;
        sregs.idt_limit = 32 * 8 - 1;
        // CR0: protected mode enabled
        sregs.cr0 = 0x00000011; // PE + ET

        // CS: 16-bit code segment
        sregs.cs = GuestSegment {
            base: self.gdt_entry_base(cs_sel) as u64, limit: self.gdt_entry_limit(cs_sel),
            selector: cs_sel, type_: 11, present: 1, dpl: 0, db: 0, s: 1, l: 0, g: 0, avl: 0, unusable: 0,
        };

        // DS/ES: NE header's auto_data_segment identifies the automatic data segment.
        // Fall back to the stack segment if auto_data_segment is 0 or out of range.
        let auto_ds = ne_file.header.auto_data_segment;
        let ds_sel = if auto_ds > 0 && (auto_ds as usize) <= ne_file.segment_table.len() {
            let tile_idx = (NE_SEGMENT_BASE / TILE_SIZE) + (auto_ds as u32 - 1);
            ((TILED_SEL_START_INDEX + tile_idx) * 8) as u16
        } else {
            ss_sel
        };
        let ds_seg = GuestSegment {
            base: self.gdt_entry_base(ds_sel) as u64, limit: self.gdt_entry_limit(ds_sel),
            selector: ds_sel, type_: 3, present: 1, dpl: 0, db: 0, s: 1, l: 0, g: 0, avl: 0, unusable: 0,
        };
        sregs.ds = ds_seg.clone();
        sregs.es = ds_seg;

        // SS: stack segment
        sregs.ss = GuestSegment {
            base: self.gdt_entry_base(ss_sel) as u64, limit: self.gdt_entry_limit(ss_sel),
            selector: ss_sel, type_: 3, present: 1, dpl: 0, db: 0, s: 1, l: 0, g: 0, avl: 0, unusable: 0,
        };

        // FS/GS: use 32-bit flat data for now
        let flat_seg = GuestSegment {
            base: 0, limit: 0xFFFFFFFF,
            selector: 0x10, type_: 3, present: 1, dpl: 0, db: 1, s: 1, l: 0, g: 1, avl: 0, unusable: 0,
        };
        sregs.fs = flat_seg.clone();
        sregs.gs = flat_seg;

        vcpu.set_sregs(&sregs).unwrap();
        vcpu.enable_software_breakpoints().unwrap();

        info!("Starting NE 16-bit execution at 0x{:04X}:0x{:04X}", cs_sel, entry_ip);
        self.run_vcpu(vcpu, 0, TIB_BASE as u64);

        self.shared.console_mgr.lock_or_recover().disable_raw_mode();
        let code = self.shared.exit_code.load(Ordering::Relaxed);
        std::process::exit(code);
    }

    /// Handle a 16-bit NE API call. Ordinal includes module base offset.
    pub(crate) fn handle_ne_api_call(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, ordinal: u16) -> u32 {
        let regs = vcpu.get_regs().unwrap();
        let sregs = vcpu.get_sregs().unwrap();
        let ss_base = sregs.ss.base as u32;
        let sp = regs.rsp as u16;
        // Stack after CALL FAR: [ret_IP:16] [ret_CS:16] [args...]
        // Args start at SP+4 (Pascal: pushed left-to-right, last arg closest to return addr)
        let arg_base = ss_base + sp as u32 + 4;

        // Read a 16-bit word from the argument area
        let read_arg16 = |off: u32| -> u16 {
            self.guest_read::<u16>(arg_base + off).unwrap_or(0)
        };
        // Read a far pointer (offset:segment) and convert to flat address
        let read_far_ptr = |off: u32| -> u32 {
            let ptr_off = read_arg16(off) as u32;
            let ptr_seg = read_arg16(off + 2);
            self.gdt_entry_base(ptr_seg) + ptr_off
        };

        // Dispatch based on module base + ordinal
        // 16-bit DOSCALLS ordinals (0..4095)
        if (ordinal as u32) < KBDCALLS_BASE {
            match ordinal {
                // DosExit(fTerminate:16, usExitCode:16) — Pascal: last pushed first
                // Stack: [ret] [usExitCode:16] [fTerminate:16]
                5 => {
                    let exit_code = read_arg16(0);
                    let _terminate = read_arg16(2);
                    debug!("  16-bit DosExit(code={})", exit_code);
                    self.shared.exit_code.store(exit_code as i32, Ordering::Relaxed);
                    self.shared.exit_requested.store(true, Ordering::Relaxed);
                    0
                }
                // DosWrite(hf:16, pBuf:far, cbBuf:16, pcbWritten:far) — Pascal
                // Pushed: hf, pBuf(seg:off), cbBuf, pcbWritten(seg:off)
                // Stack (after ret): [pcbWritten_off:16] [pcbWritten_seg:16] [cbBuf:16] [pBuf_off:16] [pBuf_seg:16] [hf:16]
                138 => {
                    let pcb_written = read_far_ptr(0);
                    let cb_buf = read_arg16(4);
                    let p_buf = read_far_ptr(6);
                    let hf = read_arg16(10);
                    debug!("  16-bit DosWrite(hf={}, buf=0x{:08X}, cb={}, pcb=0x{:08X})",
                        hf, p_buf, cb_buf, pcb_written);
                    if let Some(data) = self.guest_slice_mut(p_buf, cb_buf as usize)
                        && (hf == 1 || hf == 2) {
                            match crate::api::doscalls::dos_write(hf as u32, data) {
                                Ok(actual) => {
                                    if pcb_written != 0 {
                                        self.guest_write::<u16>(pcb_written, actual as u16);
                                    }
                                    return 0;
                                }
                                Err(_) => return 1,
                            }
                    }
                    1 // error
                }
                // DosGetInfoSeg(pGlobalSeg:far, pLocalSeg:far) — ordinal 8
                8 => {
                    debug!("  16-bit DosGetInfoSeg (stub)");
                    0
                }
                // DosSetSigHandler — ordinal 41
                41 => { debug!("  16-bit DosSetSigHandler (stub)"); 0 }
                // DosSetVec (exception vector) — ordinal 49
                49 => { debug!("  16-bit DosSetVec (stub)"); 0 }
                // DosGetInfoSeg (16-bit, get GINFOSEG/LINFOSEG selectors) — ordinal 8
                // Already handled above
                // DosGetHugeShift — ordinal 41? No, that's DosSetSigHandler
                // DosGetMachineMode — ordinal 49? No, that's DosSetVec
                // DosGetPID — ordinal 92
                92 => { debug!("  16-bit DosGetInfoSeg (ordinal 92, stub)"); 0 }
                // DosGetEnv — ordinal 94
                94 => {
                    // Returns environment selector and command line offset
                    // Pascal: DosGetEnv(pEnvSel:far, pCmdOffset:far)
                    let p_cmd_offset = read_far_ptr(0);
                    let p_env_sel = read_far_ptr(4);
                    debug!("  16-bit DosGetEnv(pEnvSel=0x{:08X}, pCmdOffset=0x{:08X})", p_env_sel, p_cmd_offset);
                    // Write environment selector (use data segment selector)
                    // and command line offset (0 for now)
                    if p_env_sel != 0 {
                        self.guest_write::<u16>(p_env_sel, 0); // env selector (stub)
                    }
                    if p_cmd_offset != 0 {
                        self.guest_write::<u16>(p_cmd_offset, 0); // cmd offset
                    }
                    0
                }
                _ => {
                    warn!("  [VCPU {}] Unimplemented 16-bit DOSCALLS ordinal {}", vcpu_id, ordinal);
                    0
                }
            }
        } else if (ordinal as u32) >= VIOCALLS_BASE && (ordinal as u32) < VIOCALLS_BASE + 1024 {
            let vio_ord = (ordinal as u32) - VIOCALLS_BASE;
            match vio_ord {
                // VioWrtTTY(pszStr:far, cbStr:USHORT, hvio:HVIO) — Pascal, left-to-right push.
                // Stack after CALL FAR: [ret_IP][ret_CS][hvio][cbStr][pszStr_off][pszStr_seg]
                19 => {
                    let hvio     = read_arg16(0);
                    let cb_str   = read_arg16(2);
                    let p_str    = read_far_ptr(4); // offset at +4, seg at +6
                    debug!("  16-bit VioWrtTTY(pszStr=0x{:08X}, cb={}, hvio={})", p_str, cb_str, hvio);
                    if let Some(data) = self.guest_slice_mut(p_str, cb_str as usize) {
                        let bytes = data.to_vec();
                        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
                        let mut console = self.shared.console_mgr.lock_or_recover();
                        console.write_tty(&bytes, 0x07, cp); // default attribute: light-grey on black
                    }
                    0
                }
                _ => {
                    warn!("  [VCPU {}] Unimplemented 16-bit VIOCALLS ordinal {}", vcpu_id, vio_ord);
                    0
                }
            }
        } else {
            warn!("  [VCPU {}] Unimplemented 16-bit API ordinal {} (module base {})",
                vcpu_id, ordinal, if ordinal as u32 >= KBDCALLS_BASE { "KBDCALLS" } else { "?" });
            0
        }
    }

    /// Return the number of argument bytes for Pascal callee cleanup of a 16-bit API.
    pub(crate) fn ne_api_arg_bytes(&self, ordinal: u16) -> u16 {
        if (ordinal as u32) < KBDCALLS_BASE {
            match ordinal {
                5 => 4,    // DosExit: 2 + 2
                8 => 8,    // DosGetInfoSeg: 4 + 4
                41 => 10,  // DosSetSigHandler: 4 + 2 + 4 (pfnSigHandler:far, fAction:16, pPrevHandler:far, ...)
                49 => 8,   // DosSetVec: 2 + 4 + 4? approximately
                92 => 4,   // DosGetInfoSeg: far pointer
                94 => 8,   // DosGetEnv: 4 + 4 (pEnvSel:far, pCmdOffset:far)
                138 => 12, // DosWrite: 2 + 4 + 2 + 4
                _ => 0,    // Unknown
            }
        } else if (ordinal as u32) < VIOCALLS_BASE + 1024 {
            let vio_ord = ordinal as u32 - VIOCALLS_BASE;
            match vio_ord {
                19 => 8,  // VioWrtTTY: 4 + 2 + 2 (pStr:far, cb:16, hvio:16)
                _ => 0,
            }
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Loader, MutexExt};
    use super::super::constants::*;

    /// ne_api_arg_bytes must return the correct Pascal callee-cleanup byte counts
    /// for all ordinals exercised by ne_hello.exe.
    #[test]
    fn ne_api_arg_bytes_known_ordinals() {
        let loader = Loader::new_mock();
        assert_eq!(loader.ne_api_arg_bytes(5),   4,  "DosExit: fTerminate(2)+usExitCode(2)");
        assert_eq!(loader.ne_api_arg_bytes(138), 12, "DosWrite: hf(2)+pBuf(4)+cb(2)+pcb(4)");
        assert_eq!(loader.ne_api_arg_bytes(41),  10, "DosSetSigHandler");
        assert_eq!(loader.ne_api_arg_bytes(49),  8,  "DosSetVec");
        assert_eq!(loader.ne_api_arg_bytes(92),  4,  "DosGetPID");
        assert_eq!(loader.ne_api_arg_bytes(94),  8,  "DosGetEnv");
        // VioWrtTTY at VIOCALLS_BASE+19
        let vio_ord = (VIOCALLS_BASE + 19) as u16;
        assert_eq!(loader.ne_api_arg_bytes(vio_ord), 8,
            "VioWrtTTY: hvio(2)+cbStr(2)+pszStr_off(2)+pszStr_seg(2)");
    }

    /// resolve_import_16 must map each known module to the correct thunk offset.
    #[test]
    fn resolve_import_16_offset() {
        let loader = Loader::new_mock();
        // DOSCALLS base = 0: ordinal N maps to thunk offset N
        assert_eq!(loader.resolve_import_16("DOSCALLS", 5),   5);
        assert_eq!(loader.resolve_import_16("DOSCALLS", 138), 138);
        // VIOCALLS base: ordinal 19 maps to VIOCALLS_BASE + 19
        let expected = VIOCALLS_BASE as u16 + 19;
        assert_eq!(loader.resolve_import_16("VIOCALLS", 19), expected);
    }

    /// VioWrtTTY dispatch: a call to ordinal VIOCALLS_BASE+19 writes to the console
    /// screen buffer.
    #[test]
    fn ne_vio_wrt_tty_writes_to_screen() {
        use super::super::vm_backend::mock::MockVcpu;
        use super::super::vm_backend::{GuestSegment, GuestSregs, GuestRegs};

        let loader = Loader::new_mock();
        // Write test string into guest memory at flat 0x3000.
        let msg = b"Hi!";
        for (i, &b) in msg.iter().enumerate() {
            loader.guest_write::<u8>(0x3000 + i as u32, b).unwrap();
        }
        // Set up the full GDT so gdt_entry_base works.
        loader.setup_idt();

        // Build mock call frame at SS_base=0, SP=0x200.
        // Stack after CALL FAR: [ret_IP:2][ret_CS:2][hvio:2][cbStr:2][str_off:2][str_seg:2]
        let sp: u32 = 0x200;
        loader.guest_write::<u16>(sp, 0x0050u16).unwrap();      // ret IP
        loader.guest_write::<u16>(sp + 2, 0x00B0u16).unwrap();  // ret CS
        loader.guest_write::<u16>(sp + 4, 0u16).unwrap();       // hvio
        loader.guest_write::<u16>(sp + 6, 3u16).unwrap();       // cbStr = 3
        // String is at flat 0x3000; tile 0, selector 0x30, offset 0x3000
        loader.guest_write::<u16>(sp + 8, 0x3000u16).unwrap();  // pszStr offset
        loader.guest_write::<u16>(sp + 10, 0x0030u16).unwrap(); // pszStr segment (tile 0)

        let mut vcpu = MockVcpu::new();
        vcpu.regs = GuestRegs { rsp: 0x200, ..Default::default() };
        vcpu.sregs = GuestSregs {
            ss: GuestSegment {
                base: 0, limit: 0xFFFF, selector: 0x30,
                type_: 3, present: 1, dpl: 0, db: 0, s: 1, l: 0, g: 0, avl: 0, unusable: 0,
            },
            ..Default::default()
        };

        let vio_ord = (VIOCALLS_BASE + 19) as u16;
        loader.handle_ne_api_call(&mut vcpu, 0, vio_ord);

        // The VioManager screen buffer should contain 'H', 'i', '!'
        let console = loader.shared.console_mgr.lock_or_recover();
        assert_eq!(console.buffer[0].0, 'H', "buffer[0]='H'");
        assert_eq!(console.buffer[1].0, 'i', "buffer[1]='i'");
        assert_eq!(console.buffer[2].0, '!', "buffer[2]='!'");
    }
}
