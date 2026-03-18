// SPDX-License-Identifier: GPL-3.0-only

use super::constants::*;
use super::{ApiResult, CallbackFrame, MutexExt};
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

    pub(crate) fn run_vcpu(&self, mut vcpu: Box<dyn VcpuBackend>, vcpu_id: u32, tib_base: u64) {
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
        sregs.gdt_limit = GDT_SIZE - 1; // 4 standard + NUM_TILES tiled 16-bit data entries
        sregs.idt_base  = IDT_BASE as u64;
        sregs.idt_limit = 32 * 8 - 1;
        vcpu.set_sregs(&sregs).unwrap();

        vcpu.enable_software_breakpoints().unwrap();

        debug!("  [VCPU {}] Started at EIP=0x{:08X}", vcpu_id, vcpu.get_regs().unwrap().rip);

        let mut callback_stack: Vec<CallbackFrame> = Vec::new();

        loop {
            // Check if shutdown has been requested
            if self.shared.exit_requested.load(Ordering::Relaxed) {
                return;
            }
            let res = vcpu.run();
            if let Err(e) = res {
                error!("  [VCPU {}] KVM Run failed: {}", vcpu_id, e);
                self.shared.exit_code.store(1, Ordering::Relaxed);
                self.shared.exit_requested.store(true, Ordering::Relaxed);
                return;
            }
            let exit = res.unwrap();
            match exit {
                VmExit::Debug => {
                    let rip = vcpu.get_regs().unwrap().rip;
                    // Check if this is from an IDT exception handler stub
                    if rip >= IDT_HANDLER_BASE as u64 && rip < IDT_HANDLER_BASE as u64 + 32 * 16 {
                        let regs = vcpu.get_regs().unwrap();
                        // Stack layout: [vector_num] [error_code_or_fake] [fault_EIP] [fault_CS] [fault_EFLAGS]
                        let vector = self.guest_read::<u32>(regs.rsp as u32).unwrap_or(0xFF);
                        let error_code = self.guest_read::<u32>(regs.rsp as u32 + 4).unwrap_or(0);
                        let fault_eip = self.guest_read::<u32>(regs.rsp as u32 + 8).unwrap_or(0);
                        let fault_cs = self.guest_read::<u32>(regs.rsp as u32 + 12).unwrap_or(0);
                        let fault_eflags = self.guest_read::<u32>(regs.rsp as u32 + 16).unwrap_or(0);

                        let sregs = vcpu.get_sregs().unwrap();
                        error!("  [VCPU {}] CPU Exception #{} at EIP=0x{:08X} CS=0x{:04X} EFLAGS=0x{:08X} ErrorCode=0x{:X}",
                               vcpu_id, vector, fault_eip, fault_cs, fault_eflags, error_code);
                        error!("    EAX=0x{:08X} EBX=0x{:08X} ECX=0x{:08X} EDX=0x{:08X}",
                               regs.rax, regs.rbx, regs.rcx, regs.rdx);
                        error!("    ESI=0x{:08X} EDI=0x{:08X} EBP=0x{:08X} ESP=0x{:08X}",
                               regs.rsi, regs.rdi, regs.rbp, regs.rsp);
                        error!("    CS=0x{:04X} DS=0x{:04X} SS=0x{:04X} ES=0x{:04X} FS=0x{:04X} GS=0x{:04X}",
                               sregs.cs.selector, sregs.ds.selector, sregs.ss.selector,
                               sregs.es.selector, sregs.fs.selector, sregs.gs.selector);
                        // Dump bytes at the fault EIP to help diagnose the faulting instruction
                        let mut fault_bytes = [0u8; 16];
                        for (i, b) in fault_bytes.iter_mut().enumerate() {
                            *b = self.guest_read::<u8>(fault_eip + i as u32).unwrap_or(0xCC);
                        }
                        error!("    Bytes at EIP: {:02X?}", fault_bytes);
                        self.shared.exit_code.store(1, Ordering::Relaxed);
                        self.shared.exit_requested.store(true, Ordering::Relaxed);
                        return;
                    }
                    // 16-bit NE API thunk: breakpoint in the NE thunk tile
                    if rip >= NE_THUNK_BASE as u64 && rip < (NE_THUNK_BASE + TILE_SIZE) as u64 {
                        let ordinal = (rip - NE_THUNK_BASE as u64) as u16;
                        {
                            let regs_dbg = vcpu.get_regs().unwrap();
                            let sregs_dbg = vcpu.get_sregs().unwrap();
                            let ss_base_dbg = sregs_dbg.ss.base as u32;
                            let sp_dbg = regs_dbg.rsp as u16;
                            let ret_ip_dbg = self.guest_read::<u16>(ss_base_dbg + sp_dbg as u32).unwrap_or(0);
                            let ret_cs_dbg = self.guest_read::<u16>(ss_base_dbg + sp_dbg as u32 + 2).unwrap_or(0);
                            debug!("  [VCPU {}] 16-bit API call: ordinal {} at 0x{:08X}, ret=0x{:04X}:0x{:04X}, SP=0x{:04X}",
                                vcpu_id, ordinal, rip, ret_cs_dbg, ret_ip_dbg, sp_dbg);
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
                        warn!("  [VCPU {}] Guest breakpoint at EIP=0x{:08X}.", vcpu_id, rip);
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
                }
                VmExit::MmioWrite { addr } => {
                    // Guest write to unmapped memory — silently ignore
                    warn!("  [VCPU {}] MMIO write at 0x{:08X} — ignoring", vcpu_id, addr);
                }
                VmExit::Shutdown => {
                    let regs = vcpu.get_regs().unwrap();
                    let sregs = vcpu.get_sregs().unwrap();
                    error!("  [VCPU {}] Guest shutdown (triple fault)", vcpu_id);
                    error!("    EIP=0x{:08X} ESP=0x{:08X} EAX=0x{:08X} EBX=0x{:08X}", regs.rip, regs.rsp, regs.rax, regs.rbx);
                    error!("    ECX=0x{:08X} EDX=0x{:08X} ESI=0x{:08X} EDI=0x{:08X}", regs.rcx, regs.rdx, regs.rsi, regs.rdi);
                    error!("    EBP=0x{:08X} EFLAGS=0x{:08X} CR0=0x{:08X} CR2=0x{:08X}", regs.rbp, regs.rflags, sregs.cr0, sregs.cr2);
                    error!("    CS=0x{:04X} DS=0x{:04X} SS=0x{:04X} FS=0x{:04X}", sregs.cs.selector, sregs.ds.selector, sregs.ss.selector, sregs.fs.selector);
                    self.shared.exit_code.store(1, Ordering::Relaxed);
                    self.shared.exit_requested.store(true, Ordering::Relaxed);
                    return;
                }
                VmExit::Other(e) => {
                    let rip = vcpu.get_regs().unwrap().rip;
                    error!("  [VCPU {}] Unhandled VMEXIT: {} at EIP=0x{:08X}", vcpu_id, e, rip);
                    self.shared.exit_code.store(1, Ordering::Relaxed);
                    self.shared.exit_requested.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
}
