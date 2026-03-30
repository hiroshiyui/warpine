// SPDX-License-Identifier: GPL-3.0-only

use super::constants::*;
use super::{ApiResult, CallbackFrame, MutexExt};
use super::crash_dump::CrashContext;
use super::gdb_stub::{GdbResumeCmd, GdbStopInfo, GdbVcpuStopReason};
use crate::lx::LxFile;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use super::vm_backend::{VcpuBackend, VmExit};
use log::{info, debug, warn, error};

impl super::Loader {
    pub(crate) fn setup_guest(&self, lx_file: &LxFile) -> (u64, u64, u64) {
        let entry_eip = lx_file.object_table[lx_file.header.eip_object as usize - 1].base_address as u64 + lx_file.header.eip as u64;
        let entry_esp = lx_file.object_table[lx_file.header.esp_object as usize - 1].base_address as u64 + lx_file.header.esp as u64;

        let tib_base = TIB_BASE as u64;
        // Build OS/2 environment block: null-terminated KEY=VALUE strings, double-null terminated,
        // followed by the program name (null-terminated).
        let exe_name = self.shared.exe_name.lock_or_recover().clone();
        let os2_exe = if exe_name.is_empty() {
            String::from("C:\\APP.EXE")
        } else {
            // Convert Unix path to OS/2 style: C:\path\to\exe
            let basename = std::path::Path::new(&exe_name)
                .file_name()
                .map(|f| f.to_string_lossy().to_uppercase())
                .unwrap_or_else(|| "APP.EXE".into());
            format!("C:\\{}", basename)
        };
        let mut env_block: Vec<u8> = Vec::new();
        env_block.extend_from_slice(b"PATH=C:\\\0");
        env_block.extend_from_slice(b"COMSPEC=C:\\4OS2.EXE\0");
        env_block.extend_from_slice(b"OS=OS2\0");
        env_block.extend_from_slice(b"TMP=C:\\TMP\0");
        env_block.push(0); // double-null terminates the environment
        let cmdline_offset = env_block.len() as u32;
        env_block.extend_from_slice(os2_exe.as_bytes());
        env_block.push(0);
        // Allocate env block via memory manager so it's properly tracked
        let env_addr = self.shared.mem_mgr.lock_or_recover()
            .alloc(env_block.len() as u32)
            .expect("setup_guest: env alloc failed");
        let cmdline_addr = env_addr + cmdline_offset;
        debug!("  Environment block ({} bytes) at 0x{:08X}: {:02X?}",
            env_block.len(), env_addr, &env_block);
        self.guest_write_bytes(env_addr, &env_block).expect("setup_guest: env write OOB");
        // TIB2 is placed right after TIB (TIB is ~0x40 bytes, TIB2 at TIB_BASE + 0x40)
        let tib2_addr = TIB_BASE + 0x40;
        self.guest_write::<u32>(TIB_BASE + 0x0C, tib2_addr).expect("setup_guest: TIB.ptib2 OOB"); // tib_ptib2
        self.guest_write::<u32>(TIB_BASE + 0x18, TIB_BASE).expect("setup_guest: TIB self-ptr OOB");
        self.guest_write::<u32>(TIB_BASE + 0x30, PIB_BASE).expect("setup_guest: TIB->PIB OOB");
        // TIB2 fields
        self.guest_write::<u32>(tib2_addr + 0x00, 1).expect("setup_guest: TIB2.tid OOB"); // tib2_ultid = thread 1
        self.guest_write::<u32>(tib2_addr + 0x04, 0).expect("setup_guest: TIB2.pri OOB"); // tib2_ulpri = normal
        self.guest_write::<u32>(PIB_BASE + 0x00, 42).expect("setup_guest: PIB PID OOB");
        self.guest_write::<u32>(PIB_BASE + 0x0C, cmdline_addr).expect("setup_guest: PIB pchcmd OOB");
        self.guest_write::<u32>(PIB_BASE + 0x10, env_addr).expect("setup_guest: PIB pchenv OOB");

        // Initialize BIOS Data Area (BDA) at flat address 0x400 with VGA 80x25 text mode.
        // Some OS/2 code and C runtime libraries read BDA directly for screen dimensions.
        {
            let console = self.shared.console_mgr.lock_or_recover();
            let cols = console.cols as u16;
            let rows = console.rows as u16;
            drop(console);
            self.guest_write::<u8>(0x449, 0x03).unwrap();         // Current video mode: VGA 80x25 color text
            self.guest_write::<u16>(0x44A, cols).unwrap();         // Number of columns
            self.guest_write::<u16>(0x44C, cols * rows * 2).unwrap(); // Page size (chars * 2 bytes each)
            self.guest_write::<u16>(0x44E, 0).unwrap();            // Current page offset
            self.guest_write::<u16>(0x450, 0).unwrap();            // Cursor position page 0 (row=0, col=0)
            self.guest_write::<u8>(0x462, 0).unwrap();             // Current display page
            self.guest_write::<u16>(0x463, 0x3D4).unwrap();        // CRTC base I/O port (color)
            self.guest_write::<u8>(0x484, (rows - 1) as u8).unwrap(); // Number of rows - 1
            self.guest_write::<u16>(0x485, 16).unwrap();           // Character height (16 scanlines)
            debug!("BDA initialized: VGA mode 0x03, {}x{} text", cols, rows);
        }

        self.setup_stubs();
        self.setup_idt();
        (entry_eip, entry_esp, tib_base)
    }

    pub(crate) fn create_initial_vcpu(&self, entry_eip: u64, entry_esp: u64) -> Box<dyn VcpuBackend> {
        let mut vcpu = self.vm.create_vcpu(0).unwrap();
        // Set up 32-bit flat segment registers, CR0/CR4, GDT, IDT.
        self.setup_vcpu_segments_32bit(&mut *vcpu, TIB_BASE as u64);
        let mut regs = vcpu.get_regs().unwrap();
        regs.rip = entry_eip;
        regs.rsp = entry_esp - 20;
        regs.rflags = 2;
        vcpu.set_regs(&regs).unwrap();

        // OS/2 initial stack: [return_addr] [hmod] [reserved] [env_ptr] [cmdline_ptr]
        // PIB layout: +0x0C = pib_pchcmd, +0x10 = pib_pchenv
        let cmdline_addr = self.guest_read::<u32>(PIB_BASE + 0x0C).unwrap_or(ENV_ADDR);
        let env_addr = self.guest_read::<u32>(PIB_BASE + 0x10).unwrap_or(ENV_ADDR);
        let sp = regs.rsp as u32;
        self.guest_write::<u32>(sp, EXIT_TRAP_ADDR).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 4, 0).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 8, 0).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 12, env_addr).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 16, cmdline_addr).expect("create_initial_vcpu: stack write OOB");
        vcpu
    }

    pub fn setup_and_run_cli(self, lx_file: &LxFile) -> ! {
        let (entry_eip, entry_esp, tib_base) = self.setup_guest(lx_file);
        let vcpu = self.create_initial_vcpu(entry_eip, entry_esp);
        self.run_vcpu(vcpu, 0, tib_base);
        // Restore terminal before process::exit() which skips all destructors.
        // Must restore termios FIRST so OPOST is active, then emit ANSI reset.
        self.shared.console_mgr.lock_or_recover().disable_raw_mode();
        let code = self.shared.exit_code.load(Ordering::Relaxed);
        std::process::exit(code);
    }

    pub fn setup_and_spawn_vcpu(self: Arc<Self>, lx_file: &LxFile) {
        let (entry_eip, entry_esp, tib_base) = self.setup_guest(lx_file);
        let vcpu = self.create_initial_vcpu(entry_eip, entry_esp);
        let loader = self;
        thread::spawn(move || {
            loader.run_vcpu(vcpu, 0, tib_base);
        });
    }

    /// Legacy run method for backwards compatibility
    pub fn run(self, lx_file: &LxFile, gui_sender: crate::gui::GUISender) -> ! {
        self.shared.window_mgr.lock_or_recover().gui_tx = Some(gui_sender);

        let (entry_eip, entry_esp, tib_base) = self.setup_guest(lx_file);
        let vcpu = self.create_initial_vcpu(entry_eip, entry_esp);
        self.run_vcpu(vcpu, 0, tib_base);
        self.shared.console_mgr.lock_or_recover().disable_raw_mode();
        let code = self.shared.exit_code.load(Ordering::Relaxed);
        std::process::exit(code);
    }

    /// Configure vCPU segment registers, CR0/CR4, GDT, and IDT for 32-bit flat mode.
    ///
    /// Called by the LX (32-bit) execution paths.  The NE (16-bit) path sets up
    /// its own segment registers in `setup_and_run_ne_cli` before calling
    /// `run_vcpu`, so this helper must NOT be called from there.
    pub(crate) fn setup_vcpu_segments_32bit(&self, vcpu: &mut dyn VcpuBackend, tib_base: u64) {
        let mut sregs = vcpu.get_sregs().unwrap();
        sregs.cs.base = 0; sregs.cs.limit = 0xFFFFFFFF; sregs.cs.g = 1; sregs.cs.db = 1; sregs.cs.present = 1; sregs.cs.type_ = 11; sregs.cs.s = 1; sregs.cs.selector = 0x08;
        let mut ds = sregs.cs.clone(); ds.type_ = 3; ds.selector = 0x10;
        sregs.ds = ds.clone(); sregs.es = ds.clone(); sregs.gs = ds.clone(); sregs.ss = ds.clone();
        let mut fs = ds; fs.base = tib_base; fs.limit = 0xFFF; fs.selector = 0x18; sregs.fs = fs;
        // PE=1 (protected mode), clear EM/TS to enable FPU, set NE for native FPU errors
        sregs.cr0 = (sregs.cr0 | 1 | (1 << 5)) & !(1u64 << 2) & !(1u64 << 3);
        // Also set CR4.OSFXSR to enable SSE instructions
        sregs.cr4 |= 1 << 9;
        // Set up GDT and IDT registers
        sregs.gdt_base  = GDT_BASE as u64;
        sregs.gdt_limit = GDT_SIZE - 1;
        sregs.idt_base  = IDT_BASE as u64;
        sregs.idt_limit = 32 * 8 - 1;
        vcpu.set_sregs(&sregs).unwrap();
    }

    pub(crate) fn run_vcpu(&self, mut vcpu: Box<dyn VcpuBackend>, vcpu_id: u32, _tib_base: u64) {

        vcpu.enable_software_breakpoints().unwrap();

        debug!("  [VCPU {}] Started at EIP=0x{:08X}", vcpu_id, vcpu.get_regs().unwrap().rip);

        // If a GDB stub is attached, pause at the entry point and wait for the
        // first 'continue' or 'step' command before executing any guest code.
        if let Some(gdb) = self.shared.gdb_state.as_ref() {
            let regs  = vcpu.get_regs().unwrap();
            let sregs = vcpu.get_sregs().unwrap();
            gdb.notify_stopped(GdbStopInfo {
                reason: GdbVcpuStopReason::Initial,
                regs,
                sregs,
            });
            match gdb.wait_for_resume() {
                GdbResumeCmd::Kill => return,
                GdbResumeCmd::Step => {
                    vcpu.set_single_step(true).unwrap();
                }
                GdbResumeCmd::Continue => {}
            }
        }

        let mut callback_stack: Vec<CallbackFrame> = Vec::new();

        loop {
            // Check if shutdown has been requested
            if self.shared.exit_requested.load(Ordering::Relaxed) {
                return;
            }

            // GDB Ctrl-C: pause the vCPU and wait for a resume command.
            if let Some(gdb) = self.shared.gdb_state.as_ref() {
                if gdb.stop_requested.swap(false, Ordering::Relaxed) {
                    let regs  = vcpu.get_regs().unwrap();
                    let sregs = vcpu.get_sregs().unwrap();
                    gdb.notify_stopped(GdbStopInfo {
                        reason: GdbVcpuStopReason::Interrupt,
                        regs,
                        sregs,
                    });
                    match gdb.wait_for_resume() {
                        GdbResumeCmd::Kill => return,
                        GdbResumeCmd::Step => {
                            vcpu.set_single_step(true).unwrap();
                        }
                        GdbResumeCmd::Continue => {
                            vcpu.set_single_step(false).unwrap();
                        }
                    }
                }
            }

            let res = vcpu.run();
            if let Err(e) = res {
                let report = self.collect_crash_report(
                    &*vcpu, vcpu_id,
                    CrashContext::KvmRunError { description: e.clone() },
                );
                self.dump_crash_report(&report);
                self.shared.exit_code.store(1, Ordering::Relaxed);
                self.shared.exit_requested.store(true, Ordering::Relaxed);
                return;
            }
            let exit = res.unwrap();
            match exit {
                VmExit::Debug => {
                    // ── GDB single-step or software breakpoint ────────────────
                    if let Some(gdb) = self.shared.gdb_state.as_ref() {
                        let regs  = vcpu.get_regs().unwrap();
                        let rip32 = regs.rip as u32;

                        // Check if this is a GDB software breakpoint (INT 3 we
                        // wrote into guest memory).  RIP points *after* the 0xCC,
                        // so the breakpoint address is RIP-1.
                        let is_gdb_bp = {
                            let bps = gdb.sw_breakpoints.lock().unwrap();
                            bps.contains_key(&(rip32.wrapping_sub(1)))
                        };

                        // If single-step was active this is a step stop, not a BP.
                        // We detect by checking whether the matching slot exists AND
                        // single-step flag is off (gdbstub will have turned it off
                        // before any resume, but we recheck via KVM here — instead
                        // we just use the presence in the sw_bp map as the criterion).
                        if is_gdb_bp {
                            // Restore the original byte and rewind RIP to the
                            // breakpoint address so GDB sees the correct PC.
                            let bp_addr = rip32.wrapping_sub(1);
                            let orig = {
                                let bps = gdb.sw_breakpoints.lock().unwrap();
                                *bps.get(&bp_addr).unwrap()
                            };
                            self.shared.guest_mem.write::<u8>(bp_addr, orig);
                            let mut regs_w = regs.clone();
                            regs_w.rip = bp_addr as u64;
                            vcpu.set_regs(&regs_w).unwrap();
                            // Disable single-step (it may have been on from a prior step).
                            vcpu.set_single_step(false).unwrap();

                            let sregs = vcpu.get_sregs().unwrap();
                            gdb.notify_stopped(GdbStopInfo {
                                reason: GdbVcpuStopReason::SwBreakpoint,
                                regs:   regs_w,
                                sregs,
                            });
                            match gdb.wait_for_resume() {
                                GdbResumeCmd::Kill => return,
                                GdbResumeCmd::Step => {
                                    vcpu.set_single_step(true).unwrap();
                                }
                                GdbResumeCmd::Continue => {
                                    // Re-install the INT3 so the breakpoint persists,
                                    // and single-step over the real instruction first.
                                    self.shared.guest_mem.write::<u8>(bp_addr, 0xCC);
                                    vcpu.set_single_step(true).unwrap();
                                }
                            }
                            continue;
                        }

                        // Is this a single-step stop (not a GDB SW breakpoint)?
                        // Single-step stops happen at addresses that are NOT in our
                        // API thunk range and NOT in the IDT handler range.
                        let rip = regs.rip;
                        let flat_rip_ss = vcpu.get_sregs().unwrap().cs.base + rip;
                        let in_api_range = rip >= MAGIC_API_BASE && rip < MAGIC_API_BASE + STUB_AREA_SIZE as u64;
                        let in_idt_range = rip >= IDT_HANDLER_BASE as u64 && rip < IDT_HANDLER_BASE as u64 + 32 * 16;
                        let in_ne_range  = flat_rip_ss >= NE_THUNK_BASE as u64 && flat_rip_ss < (NE_THUNK_BASE + TILE_SIZE) as u64;
                        if !in_api_range && !in_idt_range && !in_ne_range {
                            // Single-step stop.
                            vcpu.set_single_step(false).unwrap();
                            let sregs = vcpu.get_sregs().unwrap();
                            gdb.notify_stopped(GdbStopInfo {
                                reason: GdbVcpuStopReason::SingleStep,
                                regs,
                                sregs,
                            });
                            match gdb.wait_for_resume() {
                                GdbResumeCmd::Kill => return,
                                GdbResumeCmd::Step => {
                                    vcpu.set_single_step(true).unwrap();
                                }
                                GdbResumeCmd::Continue => {}
                            }
                            continue;
                        }
                    }

                    let rip = vcpu.get_regs().unwrap().rip;
                    // In 16-bit mode KVM reports rip as the segment offset, not the flat address.
                    // Compute flat_rip = CS.base + rip for range checks (harmless in 32-bit: CS.base=0).
                    let flat_rip = vcpu.get_sregs().unwrap().cs.base + rip;
                    // Check if this is from an IDT exception handler stub
                    if rip >= IDT_HANDLER_BASE as u64 && rip < IDT_HANDLER_BASE as u64 + 32 * 16 {
                        let regs = vcpu.get_regs().unwrap();
                        let sregs = vcpu.get_sregs().unwrap();

                        // When SS has D/B=0 (16-bit stack segment — e.g. a tiled Far16
                        // descriptor), the CPU uses SP (the low 16 bits of ESP) as the
                        // effective stack pointer.  All pushes/pops address
                        // SS.base + SP, NOT SS.base + ESP.  The full RSP reported by KVM
                        // still holds the original 32-bit ESP with its upper 16 bits
                        // unchanged, so reading the exception frame from `regs.rsp as u32`
                        // would land in completely the wrong memory and produce all-zero
                        // values.  Detect this case and compute the correct address.
                        let frame_base = if sregs.ss.db == 0 {
                            sregs.ss.base as u32 + regs.rsp as u16 as u32
                        } else {
                            regs.rsp as u32
                        };

                        // Stack layout: [vector_num] [error_code_or_fake] [fault_EIP] [fault_CS] [fault_EFLAGS]
                        let vector     = self.guest_read::<u32>(frame_base).unwrap_or(0xFF);
                        let error_code = self.guest_read::<u32>(frame_base + 4).unwrap_or(0);
                        let fault_eip  = self.guest_read::<u32>(frame_base + 8).unwrap_or(0);
                        let fault_cs   = self.guest_read::<u32>(frame_base + 12).unwrap_or(0);
                        let fault_eflags = self.guest_read::<u32>(frame_base + 16).unwrap_or(0);

                        // ── Far16 thunk bypass ───────────────────────────────
                        // Watcom __Far16 thunks do `JMP FAR <tiled_sel>:<off>`
                        // to enter 16-bit mode.  Warpine cannot execute 16-bit
                        // code, so we intercept the #GP, fully unwind the
                        // thunk, and return 0 to the caller.
                        //
                        // Thunk prologue pattern (Watcom):
                        //   PUSH EBP/EDI/EBX/EDX/ES/DS   (6 saves, 24 bytes)
                        //   MOV EBP, ESP                  (EBP = top of saves)
                        //   PUSH SS; PUSH EBP             ([EBP-4]=SS, [EBP-8]=EBP)
                        //   ...param conversion, 16-bit SS:SP switch...
                        //   66 EA xx xx xx xx              JMP FAR ptr16:16  ← #GP
                        //
                        // To unwind: find the saved EBP on the 32-bit stack
                        // (self-referential: value at addr == addr+8, with
                        // [addr+4]==0x10 as saved SS), then read the saved
                        // registers and return address from [EBP+0..EBP+24].
                        if vector == 13 && fault_cs == 0x08 {
                            let err_sel_index = error_code / 8;
                            let is_tiled = (err_sel_index >= TILED_SEL_START_INDEX
                                && err_sel_index < TILED_SEL_START_INDEX + NUM_TILES)
                                || (err_sel_index >= TILED_CODE_START_INDEX
                                && err_sel_index < TILED_CODE_START_INDEX + NUM_CODE_TILES);
                            if is_tiled {
                                let b0 = self.guest_read::<u8>(fault_eip).unwrap_or(0);
                                let b1 = self.guest_read::<u8>(fault_eip + 1).unwrap_or(0);
                                let b2 = self.guest_read::<u8>(fault_eip + 2).unwrap_or(0);

                                // Two known Far16 thunk patterns that fault with a tiled selector:
                                //   Pattern A (Watcom __Far16 JMP):  66 EA <off16> <sel16>
                                //   Pattern B (LSS stack-switch):    66 0F B2 24 24
                                //                                     66 EA <off16> <sel16>
                                // Pattern B faults at the LSS (trying to load a DPL=2 tile
                                // selector into SS at CPL=0) before ever reaching the JMP FAR.
                                // Both patterns use the same Watcom thunk frame layout so the
                                // self-referential EBP scan and unwind path are identical.
                                let is_jmp_far = b0 == 0x66 && b1 == 0xEA;
                                // `66 0F B2 24 24` = LSS SP, [ESP] with 16-bit operand override
                                let is_lss_sp_esp = b0 == 0x66 && b1 == 0x0F && b2 == 0xB2;

                                if is_jmp_far || is_lss_sp_esp {
                                    // For the JMP FAR variant the target is inline.
                                    // For the LSS variant the JMP FAR follows at +5.
                                    let jmp_base = if is_jmp_far { fault_eip + 2 } else { fault_eip + 7 };
                                    let target_off = self.guest_read::<u16>(jmp_base).unwrap_or(0);
                                    let target_sel = self.guest_read::<u16>(jmp_base + 2).unwrap_or(0);

                                    // Scan the 32-bit stack for the self-referential
                                    // PUSH EBP pattern: val == addr+8, [addr+4]==0x10.
                                    let stack_upper = (regs.rsp as u32) & 0xFFFF0000;
                                    let scan_start = stack_upper | 0xFF00; // near top of page
                                    let scan_end   = stack_upper;
                                    let mut saved_ebp: Option<u32> = None;
                                    let mut addr = scan_start;
                                    while addr >= scan_end + 8 {
                                        let val = self.guest_read::<u32>(addr).unwrap_or(0);
                                        if val == addr + 8 {
                                            let ss_val = self.guest_read::<u16>(addr + 4).unwrap_or(0);
                                            if ss_val == 0x10 {
                                                saved_ebp = Some(val);
                                                break;
                                            }
                                        }
                                        addr = addr.wrapping_sub(4);
                                    }

                                    if let Some(ebp) = saved_ebp {
                                        // Read saved registers from [EBP+0..EBP+24]
                                        let s_ds  = self.guest_read::<u32>(ebp).unwrap_or(0x10);
                                        let _s_es = self.guest_read::<u32>(ebp + 4).unwrap_or(0x10);
                                        let s_edx = self.guest_read::<u32>(ebp + 8).unwrap_or(0);
                                        let s_ebx = self.guest_read::<u32>(ebp + 12).unwrap_or(0);
                                        let s_edi = self.guest_read::<u32>(ebp + 16).unwrap_or(0);
                                        let s_ebp = self.guest_read::<u32>(ebp + 20).unwrap_or(0);
                                        let ret_addr = self.guest_read::<u32>(ebp + 24).unwrap_or(0);

                                        log::warn!("[VCPU {}] Bypassing Far16 thunk ({}): \
                                                   JMP FAR 0x{:04X}:0x{:04X} \
                                                   at EIP=0x{:08X} → return to 0x{:08X}",
                                                  vcpu_id,
                                                  if is_lss_sp_esp { "LSS+JMP" } else { "JMP" },
                                                  target_sel, target_off, fault_eip, ret_addr);

                                        let mut regs = vcpu.get_regs().unwrap();
                                        regs.rip = ret_addr as u64;
                                        regs.rsp = (ebp + 28) as u64; // past saved regs + return addr
                                        regs.rax = 0; // return "success" / null
                                        regs.rbx = s_ebx as u64;
                                        regs.rdx = s_edx as u64;
                                        regs.rdi = s_edi as u64;
                                        regs.rbp = s_ebp as u64;
                                        regs.rflags = fault_eflags as u64;
                                        vcpu.set_regs(&regs).unwrap();

                                        let mut sregs = vcpu.get_sregs().unwrap();
                                        // Restore all segments to 32-bit flat
                                        sregs.cs.selector = 0x08;
                                        sregs.cs.base = 0;
                                        sregs.cs.limit = 0xFFFFFFFF;
                                        sregs.cs.db = 1;
                                        sregs.cs.type_ = 0x0B;
                                        for seg in [&mut sregs.ds, &mut sregs.es, &mut sregs.ss] {
                                            seg.selector = s_ds as u16; // typically 0x10
                                            seg.base = 0;
                                            seg.limit = 0xFFFFFFFF;
                                            seg.db = 1;
                                            seg.type_ = 0x03;
                                        }
                                        vcpu.set_sregs(&sregs).unwrap();
                                        continue;
                                    } else {
                                        log::warn!("[VCPU {}] Far16 thunk at EIP=0x{:08X}: \
                                                   could not find saved frame, crashing",
                                                  vcpu_id, fault_eip);
                                    }
                                }
                            }
                        }

                        let report = self.collect_crash_report(
                            &*vcpu, vcpu_id,
                            CrashContext::GuestException {
                                vector,
                                error_code,
                                fault_eip,
                                fault_cs,
                                fault_eflags,
                            },
                        );
                        self.dump_crash_report(&report);
                        self.shared.exit_code.store(1, Ordering::Relaxed);
                        self.shared.exit_requested.store(true, Ordering::Relaxed);
                        return;
                    }
                    // 16-bit NE API thunk: breakpoint in the NE thunk tile
                    // Use flat_rip (CS.base + rip) since rip is a segment offset in 16-bit mode.
                    if flat_rip >= NE_THUNK_BASE as u64 && flat_rip < (NE_THUNK_BASE + TILE_SIZE) as u64 {
                        let ordinal = (flat_rip - NE_THUNK_BASE as u64) as u16;
                        {
                            let regs_dbg = vcpu.get_regs().unwrap();
                            let sregs_dbg = vcpu.get_sregs().unwrap();
                            let ss_base_dbg = sregs_dbg.ss.base as u32;
                            let sp_dbg = regs_dbg.rsp as u16;
                            let ret_ip_dbg = self.guest_read::<u16>(ss_base_dbg + sp_dbg as u32).unwrap_or(0);
                            let ret_cs_dbg = self.guest_read::<u16>(ss_base_dbg + sp_dbg as u32 + 2).unwrap_or(0);
                            debug!("  [VCPU {}] 16-bit API call: ordinal {} at flat=0x{:08X}, ret=0x{:04X}:0x{:04X}, SP=0x{:04X}",
                                vcpu_id, ordinal, flat_rip, ret_cs_dbg, ret_ip_dbg, sp_dbg);
                        }
                        let result = self.handle_ne_api_call(&mut *vcpu, vcpu_id, ordinal);
                        let mut regs = vcpu.get_regs().unwrap();
                        regs.rax = result as u64;
                        // Pop far return address (IP + CS = 4 bytes) and restore CS:IP
                        let sregs = vcpu.get_sregs().unwrap();
                        let ss_base = sregs.ss.base as u32;
                        let sp = regs.rsp as u16;
                        let ret_ip = self.guest_read::<u16>(ss_base + sp as u32).unwrap_or(0);
                        let ret_cs_sel = self.guest_read::<u16>(ss_base + sp as u32 + 2).unwrap_or(0);
                        regs.rsp = (sp.wrapping_add(4)) as u64; // pop IP + CS
                        // Pascal callee cleanup: pop arguments
                        let arg_bytes = self.ne_api_arg_bytes(ordinal);
                        regs.rsp = ((regs.rsp as u16).wrapping_add(arg_bytes)) as u64;
                        // Set return CS:IP
                        regs.rip = ret_ip as u64;
                        vcpu.set_regs(&regs).unwrap();
                        // Update CS segment register to the return code segment
                        let mut sregs = vcpu.get_sregs().unwrap();
                        sregs.cs.base     = self.gdt_entry_base(ret_cs_sel) as u64;
                        sregs.cs.limit    = self.gdt_entry_limit(ret_cs_sel);
                        sregs.cs.selector = ret_cs_sel;
                        sregs.cs.type_    = 11; // code, exec+read
                        sregs.cs.db       = 0; // 16-bit
                        sregs.cs.present  = 1;
                        sregs.cs.s        = 1;
                        vcpu.set_sregs(&sregs).unwrap();
                        continue;
                    }
                    if rip >= MAGIC_API_BASE && rip < MAGIC_API_BASE + STUB_AREA_SIZE as u64 {
                        if rip == EXIT_TRAP_ADDR as u64 {
                            info!("  [VCPU {}] Guest requested thread exit.", vcpu_id);
                            self.shared.exit_requested.store(true, Ordering::Relaxed);
                            return;
                        }
                        if rip == CALLBACK_RET_TRAP as u64 {
                            // Return from a PM callback
                            if let Some(frame) = callback_stack.pop() {
                                let mut regs = vcpu.get_regs().unwrap();
                                let result = regs.rax as u32;
                                regs.rip = frame.saved_rip;
                                regs.rsp = frame.saved_rsp;
                                regs.rax = result as u64;
                                // _System calling convention is caller-cleanup, so the guest
                                // caller will do `add esp, N` itself — we must NOT pop args here.
                                vcpu.set_regs(&regs).unwrap();
                                continue;
                            } else {
                                error!("  [VCPU {}] CALLBACK_RET_TRAP with empty callback stack!", vcpu_id);
                                return;
                            }
                        }
                        let ordinal = (rip - MAGIC_API_BASE) as u32;
                        let api_result = self.handle_api_call_ex(&mut *vcpu, vcpu_id, ordinal);
                        match api_result {
                            ApiResult::Normal(res) => {
                                let mut regs = vcpu.get_regs().unwrap();
                                regs.rax = res as u64;
                                regs.rip = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for return address") as u64;
                                regs.rsp += 4;
                                // VIO/KBD use Pascal calling convention (callee cleanup).
                                // Pop the arguments from the stack after the return address.
                                if ordinal >= VIOCALLS_BASE && ordinal < SESMGR_BASE {
                                    regs.rsp += self.viocalls_arg_bytes(ordinal - VIOCALLS_BASE);
                                } else if ordinal >= KBDCALLS_BASE && ordinal < VIOCALLS_BASE {
                                    regs.rsp += self.kbdcalls_arg_bytes(ordinal - KBDCALLS_BASE);
                                }
                                vcpu.set_regs(&regs).unwrap();
                            }
                            ApiResult::Callback { wnd_proc, hwnd, msg, mp1, mp2 } => {
                                let mut regs = vcpu.get_regs().unwrap();
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for callback return address");
                                // Save current state; saved_rsp is past the return address.
                                // The caller will clean up its own args (_System is caller-cleanup).
                                callback_stack.push(CallbackFrame {
                                    saved_rip: return_addr as u64,
                                    saved_rsp: regs.rsp + 4,
                                });
                                // Set up guest stack for callback: push ret addr + 4 args = 20 bytes
                                regs.rsp -= 20;
                                let sp = regs.rsp as u32;
                                self.guest_write::<u32>(sp, CALLBACK_RET_TRAP).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp + 4, hwnd).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp + 8, msg).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp + 12, mp1).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp + 16, mp2).expect("Callback stack write OOB");
                                regs.rip = wnd_proc as u64;
                                vcpu.set_regs(&regs).unwrap();
                            }
                        }
                    }
                    else {
                        let sregs_dbg = vcpu.get_sregs().unwrap();
                        warn!("[VCPU {}] Unexpected breakpoint: raw_rip=0x{:08X} flat_rip=0x{:08X} cs.sel=0x{:04X} cs.base=0x{:08X} cs.db={}",
                            vcpu_id, rip, flat_rip, sregs_dbg.cs.selector, sregs_dbg.cs.base, sregs_dbg.cs.db);
                        let report = self.collect_crash_report(
                            &*vcpu, vcpu_id, CrashContext::UnexpectedBreakpoint,
                        );
                        self.dump_crash_report(&report);
                        self.shared.exit_requested.store(true, Ordering::Relaxed);
                        return;
                    }
                }
                VmExit::Hlt => {
                    info!("  [VCPU {}] Guest HLT.", vcpu_id);
                    self.shared.exit_requested.store(true, Ordering::Relaxed);
                    return;
                }
                VmExit::MmioRead { addr, size } => {
                    // zeros already filled by kvm_backend
                    warn!("  [VCPU {}] MMIO read at 0x{:08X} ({} bytes) — returning zeros",
                           vcpu_id, addr, size);
                    // Rate-limit repeated accesses to unmapped memory to avoid
                    // spinning the VCPU thread at 100% CPU.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                VmExit::MmioWrite { addr } => {
                    // Guest write to unmapped memory — silently ignore
                    warn!("  [VCPU {}] MMIO write at 0x{:08X} — ignoring", vcpu_id, addr);
                    // Rate-limit same as MmioRead.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                VmExit::Shutdown => {
                    let report = self.collect_crash_report(
                        &*vcpu, vcpu_id, CrashContext::TripleFault,
                    );
                    self.dump_crash_report(&report);
                    self.shared.exit_code.store(1, Ordering::Relaxed);
                    self.shared.exit_requested.store(true, Ordering::Relaxed);
                    return;
                }
                VmExit::Other(e) => {
                    let report = self.collect_crash_report(
                        &*vcpu, vcpu_id,
                        CrashContext::UnhandledVmexit { description: e.clone() },
                    );
                    self.dump_crash_report(&report);
                    self.shared.exit_code.store(1, Ordering::Relaxed);
                    self.shared.exit_requested.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
}
