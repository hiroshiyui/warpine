// SPDX-License-Identifier: GPL-3.0-or-later

use super::constants::*;
use super::{ApiResult, CallbackFrame, FaultContext, FrameKind, MutexExt, pm_minmax};
use super::crash_dump::CrashContext;
use super::gdb_stub::{GdbResumeCmd, GdbStopInfo, GdbVcpuStopReason};
use crate::lx::LxFile;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use super::vm_backend::{GuestSegment, VcpuBackend, VmExit};
use super::threads::{tid_of_vcpu, EndOutcome, ThreadEnd, MAIN_TID, MAIN_VCPU_ID};
use super::exitlist::ExitListStart;
use super::signals::{SignalAction, SIGNAL_EXIT_CODE};
use super::interrupt;
use log::{info, debug, warn, error};

/// Length of a near `jmp rel32`.
const ENTRY_JMP_LEN: usize = 5;
/// How much of `__OS2Main` is looked at.
const OW_MAIN_SCAN_LEN: usize = 0x60;
/// Prologue of the broken `__OS2Main`: `push ebp; mov ebp,esp; sub esp,8`.
const OW_SHIFTED_PROLOGUE: [u8; 6] = [0x55, 0x89, 0xE5, 0x83, 0xEC, 0x08];
/// Its argument loads: `mov eax,[ebp+0x10]` (cmd taken from `[esp+12]`),
/// `mov ebx,[ebp+4]` (hmod from the return-address slot), `push eax`.
const OW_SHIFTED_ARGS: [u8; 7] = [0x8B, 0x45, 0x10, 0x8B, 0x5D, 0x04, 0x50];

/// The task register of a thread (ADR-0113): its TSS, at the ring-0
/// window its tables map to its own ring-0 page.
pub(crate) fn task_register() -> GuestSegment {
    const BUSY_TSS_32: u8 = 11;
    GuestSegment {
        base: u64::from(RING0_WINDOW), limit: TSS_SIZE - 1, selector: TSS_SEL,
        type_: BUSY_TSS_32, present: 1, ..Default::default()
    }
}

/// The initial stack of an executable, from `[esp]` up: return address
/// (Warpine's exit trap), module handle, reserved, environment, command
/// line (see `create_initial_vcpu`).
fn exe_entry_stack(env_addr: u32, cmdline_addr: u32) -> [u32; 5] {
    [EXIT_TRAP_ADDR, EXE_MODULE_HANDLE, 0, env_addr, cmdline_addr]
}

/// The target of the `jmp rel32` at `at` (`code` = its bytes).
fn jmp_target(at: u32, code: &[u8]) -> Option<u32> {
    if code.len() < ENTRY_JMP_LEN || code[0] != 0xE9 { return None; }
    let rel = i32::from_le_bytes([code[1], code[2], code[3], code[4]]);
    Some(at.wrapping_add(ENTRY_JMP_LEN as u32).wrapping_add(rel as u32))
}

/// Whether `code` is the broken Open Watcom `__OS2Main` (see
/// `Loader::entry_misreads_stack`).
fn is_shifted_ow_main(code: &[u8]) -> bool {
    code.starts_with(&OW_SHIFTED_PROLOGUE)
        && code.windows(OW_SHIFTED_ARGS.len()).any(|w| w == OW_SHIFTED_ARGS)
}

/// Finish the memory access a vCPU exited for, so that it may be pointed
/// elsewhere at once: KVM completes an MMIO exit on the next `KVM_RUN`,
/// and changing the registers before that loses the change (KVM API). One
/// `KVM_RUN` with `immediate_exit` set — what a kick sets — completes it
/// and returns at once without entering the guest.
fn finish_mmio(vcpu: &mut dyn VcpuBackend) {
    vcpu.kicker().kick();
    match vcpu.run() {
        Ok(VmExit::Interrupted) => {}
        other => warn!("  finishing an MMIO access returned {:?}", other.map(|e| format!("{e:?}"))),
    }
}

/// Exits to memory past guest RAM in a row, with no other exit between
/// them, after which a thread ends as a trap (`TC_TRAP`): about a second
/// at the 1 ms each such exit sleeps.
const MMIO_STORM_LIMIT: u32 = 1000;

impl super::Loader {
    pub(crate) fn setup_guest(&self, lx_file: &LxFile) -> (u64, u64, u64) {
        let entry_eip = lx_file.object_table[lx_file.header.eip_object as usize - 1].base_address as u64 + lx_file.header.eip as u64;
        let entry_esp = lx_file.object_table[lx_file.header.esp_object as usize - 1].base_address as u64 + lx_file.header.esp as u64;

        let tib_base = TIB_BASE as u64;
        // Build OS/2 environment block: null-terminated KEY=VALUE strings, double-null terminated,
        // followed by the program name (null-terminated).
        // The program's OS/2 name (also what DosQueryModuleName reports).
        // The environment (a parent's, else Warpine's default), the
        // program's full name, then the command line (`inherit.rs`).
        let (env_block, cmdline_offset) = self.startup_block();
        let cmdline_offset = cmdline_offset as u32;
        // Allocate env block via memory manager so it's properly tracked
        let env_addr = self.shared.mem_mgr.lock_or_recover()
            .alloc(env_block.len() as u32)
            .expect("setup_guest: env alloc failed");
        let cmdline_addr = env_addr + cmdline_offset;
        debug!("  Environment block ({} bytes) at 0x{:08X}: {:02X?}",
            env_block.len(), env_addr, env_block);
        self.guest_write_bytes(env_addr, &env_block).expect("setup_guest: env write OOB");
        // TIB2 is placed right after TIB (TIB is ~0x40 bytes, TIB2 at TIB_BASE + 0x40)
        let tib2_addr = TIB_BASE + 0x40;
        // tib_pexchain (TIB+0x00): head of the SEH handler chain; XCPT_CHAIN_END = no handlers.
        self.guest_write::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET, XCPT_CHAIN_END)
            .expect("setup_guest: TIB.pexchain OOB");
        self.guest_write::<u32>(TIB_BASE + 0x0C, tib2_addr).expect("setup_guest: TIB.ptib2 OOB"); // tib_ptib2
        self.guest_write::<u32>(TIB_BASE + 0x18, TIB_BASE).expect("setup_guest: TIB self-ptr OOB");
        self.guest_write::<u32>(TIB_BASE + 0x30, PIB_BASE).expect("setup_guest: TIB->PIB OOB");
        // TIB2 fields
        self.guest_write::<u32>(tib2_addr, 1).expect("setup_guest: TIB2.tid OOB"); // tib2_ultid = thread 1
        self.guest_write::<u32>(tib2_addr + 0x04, 0).expect("setup_guest: TIB2.pri OOB"); // tib2_ulpri = normal
        self.write_pib_process_ids();
        self.guest_write::<u32>(PIB_BASE + PIB_PCHCMD_OFFSET, cmdline_addr).expect("setup_guest: PIB pchcmd OOB");
        self.guest_write::<u32>(PIB_BASE + PIB_PCHENV_OFFSET, env_addr).expect("setup_guest: PIB pchenv OOB");

        // Initialize BIOS Data Area (BDA) at flat address 0x400 with VGA 80x25 text mode.
        // Some OS/2 code and C runtime libraries read BDA directly for screen dimensions.
        {
            let console = self.shared.console_mgr.lock_or_recover();
            let cols = console.cols;
            let rows = console.rows;
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

        // Thread 1's stack: its bounds in the TIB, sparse (ADR-0114).
        let stack_object = &lx_file.object_table[lx_file.header.esp_object as usize - 1];
        let object_bytes = stack_object.size.max(stack_object.page_count.saturating_mul(PAGE_SIZE));
        if let Some(stack) = super::vmem::MainStack::of(stack_object.base_address, object_bytes, entry_esp as u32, lx_file.header.stack_size) {
            self.sparse_main_stack(stack);
        }

        self.setup_stubs();
        self.setup_idt();
        // Other Warpine processes may send signal exceptions (signals.rs).
        self.listen_for_signals();
        (entry_eip, entry_esp, tib_base)
    }

    pub(crate) fn create_initial_vcpu(&self, entry_eip: u64, entry_esp: u64) -> Box<dyn VcpuBackend> {
        let mut vcpu = self.vm.create_vcpu(0).unwrap();
        // Set up 32-bit flat segment registers, CR0/CR4, GDT, IDT, and
        // thread 1's page tables (thread-local memory, ADR-0088).
        // Every thread runs paged at ring 3 (ADR-0113); the arena is empty
        // when thread 1 starts, so this fails only if guest memory is gone.
        let Some(cr3) = self.alloc_paging_block(TIB_BASE) else {
            error!("no memory for thread 1's page tables: the program cannot start");
            std::process::exit(1);
        };
        self.setup_vcpu_segments_32bit(&mut *vcpu, TIB_BASE as u64, cr3);
        let mut regs = vcpu.get_regs().unwrap();
        regs.rip = entry_eip;
        regs.rsp = entry_esp - 20;
        regs.rflags = 2;
        vcpu.set_regs(&regs).unwrap();

        // OS/2 initial stack of an executable (cdecl call of the entry
        // point): [esp] return address, [esp+4] module handle, [esp+8]
        // reserved (0), [esp+12] environment, [esp+16] command line — the
        // `pib_pchenv` / `pib_pchcmd` strings. Open Watcom's start-up code
        // (`__OS2Main( hmod, reserved, env, cmd )`, cstrto32.asm / main2o32.c)
        // reads it so; see `entry_misreads_stack` for a broken snapshot.
        let cmdline_addr = self.guest_read::<u32>(PIB_BASE + PIB_PCHCMD_OFFSET).unwrap_or(ENV_ADDR);
        let env_addr = self.guest_read::<u32>(PIB_BASE + PIB_PCHENV_OFFSET).unwrap_or(ENV_ADDR);
        let sp = regs.rsp as u32;
        for (slot, value) in exe_entry_stack(env_addr, cmdline_addr).into_iter().enumerate() {
            self.guest_write::<u32>(sp + 4 * slot as u32, value).expect("create_initial_vcpu: stack write OOB");
        }
        // DLLs the program imports initialize first: the program starts in
        // WARPINE.6, which calls their _DLL_InitTerm and "returns" to the
        // entry point with the entry stack as above.
        if self.shared.dll_mgr.lock_or_recover().has_pending_init() {
            let mut regs = vcpu.get_regs().unwrap();
            regs.rsp -= 4;
            self.guest_write::<u32>(regs.rsp as u32, entry_eip as u32).expect("create_initial_vcpu: stack write OOB");
            regs.rip = MAGIC_API_BASE + (WARPINE_BASE + WRP_START_PROCESS) as u64;
            vcpu.set_regs(&regs).unwrap();
        }
        if self.entry_misreads_stack(entry_eip as u32) {
            warn!("this program was built with an Open Watcom snapshot whose start-up code reads the \
                   entry stack one slot off (Open Watcom #1649, fixed 2026-08-11): its argv and \
                   environment are wrong; rebuild it with a newer Open Watcom");
        }
        vcpu
    }

    /// The length of the breakpoint instruction a 32-bit program executed
    /// at `rip` (`INT3` or `INT 3`), or None: 16-bit code, or no such
    /// instruction there (`seh::breakpoint_length`).
    fn program_breakpoint(&self, vcpu: &dyn VcpuBackend, rip: u64) -> Option<u32> {
        let sregs = vcpu.get_sregs().ok()?;
        if sregs.cs.db == 0 || sregs.cs.base != 0 {
            return None;
        }
        let at = u32::try_from(rip).ok()?;
        let code = self.guest_read_bytes(at, 2).or_else(|| self.guest_read_bytes(at, 1))?;
        super::seh::breakpoint_length(&code)
    }

    /// Whether the executable's entry point is the start-up code of the
    /// Open Watcom snapshots that declared `__OS2Main` "no return" (Open
    /// Watcom issue #1649): `jmp __OS2Main`, which then takes its four
    /// arguments one slot too low — the command line from `[esp+12]`, where
    /// the environment is. Only diagnosed: on OS/2 itself such programs
    /// fail at start-up.
    fn entry_misreads_stack(&self, entry: u32) -> bool {
        let Some(jmp) = self.guest_read_bytes(entry, ENTRY_JMP_LEN) else { return false };
        let Some(target) = jmp_target(entry, &jmp) else { return false };
        self.guest_read_bytes(target, OW_MAIN_SCAN_LEN).is_some_and(|code| is_shifted_ow_main(&code))
    }

    pub fn setup_and_run_cli(self, lx_file: &LxFile) -> ! {
        let (entry_eip, entry_esp, tib_base) = self.setup_guest(lx_file);
        let vcpu = self.create_initial_vcpu(entry_eip, entry_esp);
        self.run_vcpu(vcpu, super::threads::MAIN_VCPU_ID, tib_base);
        // Thread 1 may end by itself while other threads still run.
        self.wait_for_process_end();
        // Restore terminal before process::exit() which skips all destructors.
        // Must restore termios FIRST so OPOST is active, then emit ANSI reset.
        self.shared.npipes.lock_or_recover().close_all();
        self.shared.pipe_sems.lock_or_recover().stop_all();
        self.shared.leave_thread_session();
        self.shared.console_mgr.lock_or_recover().disable_raw_mode();
        let code = self.shared.exit_code.load(Ordering::Relaxed);
        // The whole code for the parent (exit_report.rs).
        super::exit_report::report(code as u32);
        std::process::exit(code);
    }

    pub fn setup_and_spawn_vcpu(self: Arc<Self>, lx_file: &LxFile) {
        let (entry_eip, entry_esp, tib_base) = self.setup_guest(lx_file);
        let vcpu = self.create_initial_vcpu(entry_eip, entry_esp);
        let loader = self;
        thread::spawn(move || {
            loader.run_vcpu(vcpu, MAIN_VCPU_ID, tib_base);
        });
    }

    /// Legacy run method for backwards compatibility
    pub fn run(self, lx_file: &LxFile, gui_sender: crate::gui::GUISender) -> ! {
        self.shared.window_mgr.lock_or_recover().gui_tx = Some(gui_sender);

        let (entry_eip, entry_esp, tib_base) = self.setup_guest(lx_file);
        let vcpu = self.create_initial_vcpu(entry_eip, entry_esp);
        self.run_vcpu(vcpu, super::threads::MAIN_VCPU_ID, tib_base);
        // Thread 1 may end by itself while other threads still run.
        self.wait_for_process_end();
        self.shared.npipes.lock_or_recover().close_all();
        self.shared.pipe_sems.lock_or_recover().stop_all();
        self.shared.leave_thread_session();
        self.shared.console_mgr.lock_or_recover().disable_raw_mode();
        let code = self.shared.exit_code.load(Ordering::Relaxed);
        // The whole code for the parent (exit_report.rs).
        super::exit_report::report(code as u32);
        std::process::exit(code);
    }

    /// Configure vCPU segment registers, CR0/CR4, GDT, and IDT for 32-bit flat mode.
    ///
    /// Called by the LX (32-bit) execution paths.  The NE (16-bit) path sets up
    /// its own segment registers in `setup_and_run_ne_cli` before calling
    /// `run_vcpu`, so this helper must NOT be called from there.
    ///
    /// `cr3`: the thread's paging block (`paging.rs`). Every thread runs
    /// paged, at ring 3 (ADR-0113).
    pub(crate) fn setup_vcpu_segments_32bit(&self, vcpu: &mut dyn VcpuBackend, tib_base: u64, cr3: u32) {
        use super::ring::{user_selector, USER_RING};
        let mut sregs = vcpu.get_sregs().unwrap();
        sregs.cs.base = 0; sregs.cs.limit = 0xFFFFFFFF; sregs.cs.g = 1; sregs.cs.db = 1; sregs.cs.present = 1; sregs.cs.type_ = 11; sregs.cs.s = 1;
        sregs.cs.selector = user_selector(0x08); sregs.cs.dpl = USER_RING;
        let mut ds = sregs.cs.clone(); ds.type_ = 3; ds.selector = user_selector(0x10);
        sregs.ds = ds.clone(); sregs.es = ds.clone(); sregs.gs = ds.clone(); sregs.ss = ds.clone();
        // The thread sees its TIB through the FS window, as a reload of
        // selector 0x18 would (ADR-0110).
        let fs_base = u64::from(FS_WINDOW) + (tib_base & 0xFFF);
        let mut fs = ds; fs.base = fs_base; fs.limit = 0xFFF; fs.selector = user_selector(0x18); fs.g = 0; sregs.fs = fs;
        // PE=1 (protected mode), clear EM/TS to enable FPU, set NE for native FPU errors
        sregs.cr0 = (sregs.cr0 | 1 | (1 << 5)) & !(1u64 << 2) & !(1u64 << 3);
        // Also set CR4.OSFXSR to enable SSE instructions
        sregs.cr4 |= 1 << 9;
        // Paging with the thread's own tables (ADR-0088); CR0.WP: read-only
        // pages hold for Warpine's ring-0 stubs too (ADR-0113).
        sregs.cr3 = cr3 as u64;
        sregs.cr4 |= super::paging::CR4_PSE;
        sregs.cr0 |= super::paging::CR0_PG | super::paging::CR0_WP;
        // The thread's TSS, in its ring-0 window (ADR-0113).
        sregs.tr = task_register();
        // Set up GDT and IDT registers
        sregs.gdt_base  = GDT_BASE as u64;
        sregs.gdt_limit = GDT_SIZE - 1;
        sregs.idt_base  = IDT_BASE as u64;
        sregs.idt_limit = 32 * 8 - 1;
        vcpu.set_sregs(&sregs).unwrap();
    }

    /// Run a guest thread on `vcpu` until it ends, then end the thread:
    /// free what Warpine allocated for it, keep its vCPU for reuse, wake
    /// `DosWaitThread`, and end the process when it was the last thread
    /// (or thread 1 returning from the program's entry point).
    pub(crate) fn run_vcpu(&self, mut vcpu: Box<dyn VcpuBackend>, vcpu_id: u32, tib_base: u64) {
        // Exit-list routines start from the thread's initial stack.
        let initial_esp = vcpu.get_regs().map_or(0, |r| r.rsp as u32);
        // Others may now kick this thread out of guest code.
        self.shared.threads.table.lock_or_recover().set_kicker(tid_of_vcpu(vcpu_id), vcpu.kicker());
        // This host thread's accesses to the thread-local page go where the
        // guest thread's own do (paging.rs).
        // Only a CR3 that is a valid block counts (paging.rs).
        let paged = vcpu.get_sregs().ok().filter(|s| s.cr0 & super::paging::CR0_PG != 0);
        super::guest_mem::set_thread_tlm_backing(paged.and_then(|s| super::paging::tlm_page_of(s.cr3)).unwrap_or(0));
        super::guest_mem::set_thread_tib(tib_base as u32);
        // Its stacking changes go in the Report after each call (ADR-0108).
        super::pm_types::set_api_thread(true);
        let end = self.vcpu_loop(&mut *vcpu, vcpu_id, initial_esp, false);
        self.end_thread(vcpu, vcpu_id, end, initial_esp);
    }

    /// Block until the process ends: the last thread ended or one asked
    /// for the process to end (and its exit list has run).
    pub(crate) fn wait_for_process_end(&self) {
        self.shared.threads.wait_for_process_end(|| self.shared.exit_requested.load(Ordering::Relaxed));
    }

    /// Give back what a thread was given: its stack's page states (a sparse
    /// stack's region), then its stack and TIB, then its paging block.
    ///
    /// The page states go first, while the stack is still allocated: once
    /// it is freed another thread may be given the same address
    /// (`DosCreateThread` for a thread waiting on this one's end), and
    /// writing the old region's pages "not present" then would unmap the
    /// new thread's stack — it faulted at its first push and never ran
    /// (thread_control_test's CREATE_SUSPENDED thread, CI once).
    pub(crate) fn release_thread_memory(&self, stack: u32, tib: u32, paging: u32) {
        self.free_page_states(stack);
        let mut mem_mgr = self.shared.mem_mgr.lock_or_recover();
        mem_mgr.free(stack);
        mem_mgr.free(tib);
        drop(mem_mgr);
        if paging != 0 {
            self.free_paging_block(paging);
        }
    }

    /// The end of thread `vcpu_id`'s vCPU loop (see `run_vcpu`).
    fn end_thread(&self, mut vcpu: Box<dyn VcpuBackend>, vcpu_id: u32, mut end: ThreadEnd, initial_esp: u32) {
        let tid = tid_of_vcpu(vcpu_id);
        self.shared.threads.forget_must_complete(tid);
        // Thread 1 ending as the last thread ends the process: its exit list
        // runs first, on this thread (decided under the table lock, so that
        // a thread ending at the same time cannot also be the last).
        if let ThreadEnd::Thread(rc) = end && tid == MAIN_TID {
            let claimed = {
                let table = self.shared.threads.table.lock_or_recover();
                table.running_count() == 1 && table.is_running(MAIN_TID)
                    && self.claim_exit_list(vcpu_id, rc, TC_EXIT, initial_esp)
            };
            if claimed {
                end = if self.next_exit_routine(&mut *vcpu) {
                    self.vcpu_loop(&mut *vcpu, vcpu_id, initial_esp, true)
                } else {
                    ThreadEnd::Process
                };
            }
        }
        // A tracking it was in ends (pm_track.rs): the capture goes back.
        let track = self.shared.window_mgr.lock_or_recover().tracks.get(&vcpu_id).copied();
        if let Some(s) = track { self.finish_track(vcpu_id, &s); }
        // A thread's PM message queue goes with it, so that input for its
        // windows is dropped instead of waiting for a reader that is gone.
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let refused = wm.thread_queue(vcpu_id).map_or_else(Vec::new, |hmq| {
                // Its windows go too, without WM_DESTROY: there is no thread
                // left to run their procedures.
                wm.destroy_queue_windows(hmq);
                let refused = wm.drain_sent(hmq);
                wm.destroy_mq(vcpu_id, hmq);
                refused
            });
            // So do its hook and send-call state (pm_hook.rs).
            wm.hooks.forget_thread(vcpu_id);
            drop(wm);
            // Messages other programs sent to its windows will not run, and
            // its own sends to other programs are given up (pm_remote.rs).
            self.refuse_sent(refused);
            self.forget_remote_sends(vcpu_id);
        }
        // Arenas of containers destroyed with them (pm_container.rs).
        self.free_pending_guest_blocks();
        // Mutex semaphores it still owns lose their owner (ERROR_SEM_OWNER_DIED
        // for the next requester); before its vCPU can go to a new thread.
        self.shared.sem_mgr.lock_or_recover().owner_ended(vcpu_id, tid);
        let (outcome, memory) = {
            let mut table = self.shared.threads.table.lock_or_recover();
            let ended = table.end(tid);
            // Only created threads' vCPUs are reused (thread 1's runs on the
            // initial vCPU, which the NE path sets up for 16-bit code).
            if vcpu_id != MAIN_VCPU_ID {
                table.put_vcpu(vcpu);
            }
            ended
        };
        if let Some(mem) = memory {
            self.release_thread_memory(mem.stack, mem.tib, mem.paging);
        }
        super::guest_mem::set_thread_tlm_backing(0);
        // A critical section the thread was in ends with it.
        self.shared.threads.release_crit_sec(tid);
        // So does its slot of the session's THREADS= limit.
        self.shared.thread_slots.lock_or_recover().release_thread();
        match end {
            ThreadEnd::Thread(rc) => {
                info!("  [VCPU {}] Thread {} ended (rc={}).", vcpu_id, tid, rc);
                if outcome == EndOutcome::LastThread {
                    self.end_process(rc);
                }
            }
            ThreadEnd::Process => {
                // Every path that ends a loop this way has already asked
                // for the process to end; make sure it does — unless the
                // thread stopped for another one running the exit list.
                if !self.stopped_by_exit_list(vcpu_id) {
                    self.shared.exit_requested.store(true, Ordering::Relaxed);
                }
            }
        }
        self.shared.threads.ended.notify_all();
    }

    /// An unhandled exception ends the process with exit code 1 — after its
    /// exit list (`TC_EXCEPTION`); in an exit-list routine it ends just
    /// that routine. `None`: the vCPU now runs a routine.
    /// `tc`: the termination code exit-list routines get (`TC_EXCEPTION`,
    /// `TC_KILLPROCESS` for a signal).
    fn end_by_exception(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, initial_esp: u32,
                        callback_stack: &mut Vec<CallbackFrame>, tc: u32) -> Option<ThreadEnd> {
        const EXCEPTION_EXIT_CODE: u32 = 1;
        if self.runs_exit_list(vcpu_id) {
            return (!self.next_exit_routine(vcpu)).then_some(ThreadEnd::Process);
        }
        match self.begin_exit_list(vcpu, vcpu_id, EXCEPTION_EXIT_CODE, tc, initial_esp, callback_stack) {
            ExitListStart::Running => None,
            ExitListStart::Finished => Some(ThreadEnd::Process),
            ExitListStart::NotStarted => {
                if !self.stopped_by_exit_list(vcpu_id) {
                    self.shared.exit_code.store(EXCEPTION_EXIT_CODE as i32, Ordering::Relaxed);
                    self.shared.exit_requested.store(true, Ordering::Relaxed);
                }
                Some(ThreadEnd::Process)
            }
        }
    }

    /// A fault no exception handler can see — a triple fault (`TC_TRAP`), a
    /// KVM error or an exit Warpine cannot handle (`TC_HARDERROR`) — on
    /// thread `vcpu_id`: the process ends with exit code 1, after its exit
    /// list where that is safe:
    /// - on another thread, thread 1 runs the exit list (asked for like a
    ///   termination from outside); the faulting thread waits for the end
    ///   (at most `KILL_GRACE`) and is not used again;
    /// - on thread 1, when `restart` holds the segment state it started
    ///   with (32-bit code), which is put back; a vCPU that fails again
    ///   in a routine ends the process;
    /// - in an exit-list routine, the routine has ended (the next one runs
    ///   when the vCPU can run again).
    ///
    /// `None`: the vCPU now runs an exit-list routine.
    fn end_by_fault(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, initial_esp: u32,
                    restart: Option<&super::vm_backend::GuestSregs>,
                    callback_stack: &mut Vec<CallbackFrame>, tc: u32) -> Option<ThreadEnd> {
        const FAULT_EXIT_CODE: u32 = 1;
        // The segment state the thread started with (its page tables are
        // out of its reach at ring 3). A fault the vCPU was still
        // delivering is dropped.
        let reset = |vcpu: &mut dyn VcpuBackend| restart.is_some_and(|s| {
            vcpu.clear_pending_events().is_ok() && vcpu.set_sregs(s).is_ok()
        });
        if self.runs_exit_list(vcpu_id) {
            if reset(vcpu) && self.next_exit_routine(vcpu) { return None; }
            self.request_process_exit(FAULT_EXIT_CODE);
            return Some(ThreadEnd::Process);
        }
        if tid_of_vcpu(vcpu_id) != MAIN_TID {
            self.wait_for_thread_1_to_end(FAULT_EXIT_CODE, tc);
            return Some(ThreadEnd::Process);
        }
        if reset(vcpu) {
            match self.begin_exit_list(vcpu, vcpu_id, FAULT_EXIT_CODE, tc, initial_esp, callback_stack) {
                ExitListStart::Running => return None,
                ExitListStart::Finished => return Some(ThreadEnd::Process),
                ExitListStart::NotStarted => {}
            }
        }
        if !self.stopped_by_exit_list(vcpu_id) {
            self.request_process_exit(FAULT_EXIT_CODE);
        }
        Some(ThreadEnd::Process)
    }

    /// Run thread `vcpu_id` until it ends. `initial_esp` is where its
    /// stack started (exit-list routines start there); `resumed`: the loop
    /// goes on after the thread's own end to run the exit list.
    fn vcpu_loop(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, initial_esp: u32, resumed: bool) -> ThreadEnd {
        let tid = tid_of_vcpu(vcpu_id);
        // The segment state an exit-list routine can run with after a fault
        // (32-bit threads only; see `end_by_fault`).
        let restart_sregs = vcpu.get_sregs().ok().filter(|s| s.cs.db != 0 && s.ss.db != 0);

        vcpu.enable_software_breakpoints().unwrap();

        debug!("  [VCPU {}] Started at EIP=0x{:08X}", vcpu_id, vcpu.get_regs().unwrap().rip);

        // If a GDB stub is attached, pause at the entry point and wait for the
        // first 'continue' or 'step' command before executing any guest code.
        if !resumed && let Some(gdb) = self.shared.gdb_state.as_ref() {
            gdb.started.store(true, Ordering::Release);
            let regs  = vcpu.get_regs().unwrap();
            let sregs = vcpu.get_sregs().unwrap();
            gdb.notify_stopped(GdbStopInfo {
                reason: GdbVcpuStopReason::Initial,
                regs,
                sregs,
            });
            match gdb.wait_for_resume() {
                GdbResumeCmd::Kill => return ThreadEnd::Process,
                GdbResumeCmd::Step => {
                    vcpu.set_single_step(true).unwrap();
                }
                GdbResumeCmd::Continue => {}
            }
        }

        let mut callback_stack: Vec<CallbackFrame> = Vec::new();
        // The TLB epoch this vCPU's TLB is clean for (ADR-0114).
        let mut tlb_epoch = self.shared.tlb_epoch.load(Ordering::Acquire);
        // Exits to memory past guest RAM in a row (see MMIO_STORM_LIMIT).
        let mut mmio_run: u32 = 0;

        loop {
            // Another thread is in a critical section (DosEnterCritSec):
            // do not enter the guest until it ends. Giving up means the
            // thread is to end, which the checks below do.
            self.wait_crit_sec(vcpu_id);
            // Check if shutdown has been requested
            if self.shared.exit_requested.load(Ordering::Relaxed) {
                return ThreadEnd::Process;
            }
            // Another thread runs the exit list: every other thread ends.
            if self.stopped_by_exit_list(vcpu_id) {
                info!("  [VCPU {}] Thread {} stopped for exit-list processing.", vcpu_id, tid);
                return ThreadEnd::Process;
            }
            // DosSuspendThread: do not enter the guest until resumed (a
            // killed thread too: it ends when resumed), then look at
            // everything again (a critical section may have begun).
            if self.shared.threads.wait_while_suspended(tid, || self.shutting_down()) {
                continue;
            }
            // DosKillThread: end like DosExit(EXIT_THREAD).
            if self.shared.threads.table.lock_or_recover().is_killed(tid) {
                info!("  [VCPU {}] Thread {} killed.", vcpu_id, tid);
                return ThreadEnd::Thread(NO_ERROR);
            }
            // A signal exception for thread 1 (DosSendSignalException).
            match self.signal_action(&*vcpu, vcpu_id) {
                SignalAction::None => {}
                SignalAction::Dispatch(dispatch) => {
                    setup_exception_dispatch(&mut *vcpu, self, &mut callback_stack, dispatch);
                    continue;
                }
                SignalAction::EndProcess => {
                    match self.begin_exit_list(&mut *vcpu, vcpu_id, SIGNAL_EXIT_CODE, TC_KILLPROCESS,
                                               initial_esp, &mut callback_stack) {
                        ExitListStart::Running => continue,
                        ExitListStart::Finished => return ThreadEnd::Process,
                        ExitListStart::NotStarted => {}
                    }
                    self.end_process(SIGNAL_EXIT_CODE);
                    return ThreadEnd::Process;
                }
                SignalAction::Terminate { rc, tc, exit_list } => {
                    // The process ends as DosExit(EXIT_PROCESS) would, its exit
                    // list with `tc` (a closed session, the end of piped input,
                    // a fault on another thread).
                    if exit_list {
                        match self.begin_exit_list(&mut *vcpu, vcpu_id, rc, tc, initial_esp, &mut callback_stack) {
                            ExitListStart::Running => continue,
                            ExitListStart::Finished => return ThreadEnd::Process,
                            ExitListStart::NotStarted => {}
                        }
                    }
                    self.end_process(rc);
                    return ThreadEnd::Process;
                }
            }

            // GDB Ctrl-C, or a module loaded or freed (ADR-0104): pause
            // the vCPU and wait for a resume command.
            if let Some(gdb) = self.shared.gdb_state.as_ref()
                && gdb.stop_requested.swap(false, Ordering::AcqRel) {
                let regs  = vcpu.get_regs().unwrap();
                let sregs = vcpu.get_sregs().unwrap();
                let library = gdb.library_event.swap(false, Ordering::AcqRel);
                gdb.notify_stopped(GdbStopInfo {
                    reason: if library { GdbVcpuStopReason::Library } else { GdbVcpuStopReason::Interrupt },
                    regs,
                    sregs,
                });
                match gdb.wait_for_resume() {
                    GdbResumeCmd::Kill => return ThreadEnd::Process,
                    GdbResumeCmd::Step => {
                        vcpu.set_single_step(true).unwrap();
                    }
                    GdbResumeCmd::Continue => {
                        vcpu.set_single_step(false).unwrap();
                    }
                }
            }

            // The allocator's allocations and frees into the page tables
            // (ADR-0114).
            self.sync_allocations();
            // Pages lost access since this vCPU last ran (ADR-0114):
            // reload CR3 so that its TLB forgets them.
            // So did the memory this thread freed (free memory is not
            // present), which only this thread's TLB is flushed for.
            let epoch = self.shared.tlb_epoch.load(Ordering::Acquire);
            let freed_here = super::managers::take_freed_here();
            if epoch != tlb_epoch || freed_here {
                tlb_epoch = epoch;
                flush_tlb(&mut *vcpu);
            }
            let res = vcpu.run();
            if let Err(e) = res {
                let report = self.collect_crash_report(
                    &*vcpu, vcpu_id,
                    CrashContext::KvmRunError { description: e.clone() },
                );
                self.dump_crash_report(&report);
                // KVM_RUN failed (TC_HARDERROR). The exit list is tried on
                // this vCPU with its starting segment state put back; if it
                // fails again, `end_by_fault` ends the process.
                match self.end_by_fault(&mut *vcpu, vcpu_id, initial_esp, restart_sregs.as_ref(),
                                        &mut callback_stack, TC_HARDERROR) {
                    Some(end) => return end,
                    None => continue,
                }
            }
            let exit = res.unwrap();
            // Memory past guest RAM reads as zeros, but a run of such
            // exits with nothing else between them (kicks aside) is a guest that makes
            // no progress (a page walk through a CR3 or table entry the
            // guest pointed past its memory, a loop over memory it does
            // not have): it ends as a trap instead of spinning forever.
            if matches!(exit, VmExit::MmioRead { .. } | VmExit::MmioWrite { .. }) {
                mmio_run += 1;
                if mmio_run >= MMIO_STORM_LIMIT {
                    mmio_run = 0;
                    let report = self.collect_crash_report(&*vcpu, vcpu_id, CrashContext::UnhandledVmexit {
                        description: format!("{MMIO_STORM_LIMIT} accesses in a row to memory past guest RAM"),
                    });
                    self.dump_crash_report(&report);
                    finish_mmio(&mut *vcpu);
                    match self.end_by_fault(&mut *vcpu, vcpu_id, initial_esp, restart_sregs.as_ref(),
                                            &mut callback_stack, TC_TRAP) {
                        Some(end) => return end,
                        None => continue,
                    }
                }
            } else if !matches!(exit, VmExit::Interrupted) {
                // A kick (the paging watch's, every 250 ms) is no progress.
                mmio_run = 0;
            }
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
                                GdbResumeCmd::Kill => return ThreadEnd::Process,
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
                                GdbResumeCmd::Kill => return ThreadEnd::Process,
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
                        // An exception from ring 3 (ADR-0113): its frame
                        // moves to the guest's stack, so what follows reads
                        // it as at ring 0.
                        if let Err(e) = self.lower_ring3_frame(&mut *vcpu) {
                            let report = self.collect_crash_report(&*vcpu, vcpu_id, CrashContext::UnhandledVmexit {
                                description: format!("an exception at ring 3 that cannot be delivered: {e}"),
                            });
                            self.dump_crash_report(&report);
                            match self.end_by_fault(&mut *vcpu, vcpu_id, initial_esp, restart_sregs.as_ref(),
                                                    &mut callback_stack, TC_TRAP) {
                                Some(end) => return end,
                                None => continue,
                            }
                        }
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
                        if vector == 13 && fault_cs & !3 == 0x08 {
                            let err_sel_index = error_code / 8;
                            let is_tiled = (TILED_SEL_START_INDEX..TILED_SEL_START_INDEX + NUM_TILES).contains(&err_sel_index)
                                || (TILED_CODE_START_INDEX..TILED_CODE_START_INDEX + NUM_CODE_TILES).contains(&err_sel_index);
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
                                            if ss_val & !3 == 0x10 {
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
                                        regs.rflags = super::ring::guest_eflags(fault_eflags);
                                        vcpu.set_regs(&regs).unwrap();

                                        let mut sregs = vcpu.get_sregs().unwrap();
                                        // Restore all segments to 32-bit flat, at ring 3.
                                        let ring = super::ring::USER_RING;
                                        sregs.cs.selector = super::ring::user_selector(0x08);
                                        sregs.cs.base = 0;
                                        sregs.cs.limit = 0xFFFFFFFF;
                                        sregs.cs.db = 1;
                                        sregs.cs.type_ = 0x0B;
                                        sregs.cs.dpl = ring;
                                        for seg in [&mut sregs.ds, &mut sregs.es, &mut sregs.ss] {
                                            seg.selector = s_ds as u16; // typically 0x10 (0x13 at ring 3)
                                            seg.base = 0;
                                            seg.limit = 0xFFFFFFFF;
                                            seg.db = 1;
                                            seg.type_ = 0x03;
                                            seg.dpl = ring;
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

                        // ── SEH dispatch ────────────────────────────────────
                        // Reconstruct the fault-time register state.  The IDT
                        // stub only pushed vector + error_code + INT3 so all
                        // general-purpose registers still reflect the fault context.
                        // The pre-fault ESP is the current frame_base + 20 (the five
                        // dwords pushed by the exception stub and CPU, both variants).
                        let fault_ctx = FaultContext {
                            eax:    regs.rax as u32,
                            ebx:    regs.rbx as u32,
                            ecx:    regs.rcx as u32,
                            edx:    regs.rdx as u32,
                            esi:    regs.rsi as u32,
                            edi:    regs.rdi as u32,
                            ebp:    regs.rbp as u32,
                            esp:    frame_base + 20,  // pre-fault stack pointer
                            eip:    fault_eip,
                            eflags: fault_eflags,
                            cs:     fault_cs as u16,
                            ds:     sregs.ds.selector,
                            es:     sregs.es.selector,
                            fs:     sregs.fs.selector,
                            gs:     sregs.gs.selector,
                            ss:     sregs.ss.selector,
                        };

                        // A page fault in memory DosAllocMem allocated
                        // (ADR-0114): a stale TLB entry runs the
                        // instruction again; a guard page, now without its
                        // guard, raises XCPT_GUARD_PAGE_VIOLATION, which
                        // with no handler goes on where it faulted.
                        if vector == PAGE_FAULT_VECTOR {
                            let write = error_code & PF_ERROR_WRITE != 0;
                            let addr = sregs.cr2 as u32;
                            match self.page_fault_kind(addr, write) {
                                Some(super::vmem::FaultKind::Retry) => {
                                    let mut r = vcpu.get_regs().unwrap();
                                    resume_at(&mut r, &fault_ctx);
                                    vcpu.set_regs(&r).unwrap();
                                    continue;
                                }
                                Some(super::vmem::FaultKind::Guard) => {
                                    let access = if write { XCPT_WRITE_ACCESS } else { XCPT_READ_ACCESS };
                                    let dispatch = if guest_stack_ok(regs.rsp) {
                                        self.chain_dispatch(vcpu_id, fault_ctx.clone(), XCPT_GUARD_PAGE_VIOLATION, 0, &[access, addr])
                                    } else { None };
                                    if let Some(exc_dispatch) = dispatch {
                                        setup_exception_dispatch(&mut *vcpu, self, &mut callback_stack, exc_dispatch);
                                    } else {
                                        let mut r = vcpu.get_regs().unwrap();
                                        resume_at(&mut r, &fault_ctx);
                                        vcpu.set_regs(&r).unwrap();
                                    }
                                    continue;
                                }
                                // A thread's stack reached its bottom pages
                                // (now committed, for the handler).
                                Some(super::vmem::FaultKind::StackExhausted) => {
                                    let dispatch = if guest_stack_ok(regs.rsp) {
                                        self.chain_dispatch(vcpu_id, fault_ctx.clone(), XCPT_UNABLE_TO_GROW_STACK, 0, &[])
                                    } else { None };
                                    if let Some(exc_dispatch) = dispatch {
                                        setup_exception_dispatch(&mut *vcpu, self, &mut callback_stack, exc_dispatch);
                                        continue;
                                    }
                                    let report = self.collect_crash_report(&*vcpu, vcpu_id, CrashContext::GuestException {
                                        vector, error_code, fault_eip, fault_cs, fault_eflags,
                                    });
                                    self.dump_crash_report(&report);
                                    match self.end_by_exception(&mut *vcpu, vcpu_id, initial_esp, &mut callback_stack, TC_EXCEPTION) {
                                        Some(end) => return end,
                                        None => continue,
                                    }
                                }
                                // Memory another thread allocated just now:
                                // its page is present once the allocation
                                // is in the tables.
                                None if error_code & PF_ERROR_PRESENT == 0 && self.allocated_and_present(addr) => {
                                    debug!("  [VCPU {}] #PF at 0x{:08X}: allocated meanwhile, again", vcpu_id, addr);
                                    let mut r = vcpu.get_regs().unwrap();
                                    resume_at(&mut r, &fault_ctx);
                                    vcpu.set_regs(&r).unwrap();
                                    continue;
                                }
                                Some(super::vmem::FaultKind::Violation) | None => {}
                            }
                        }

                        // A handler call is built below the faulting stack:
                        // with no room there, no handler runs.
                        let dispatch = if guest_stack_ok(regs.rsp) {
                            self.try_hw_exception_dispatch(vcpu_id, fault_ctx, vector, error_code, sregs.cr2 as u32)
                        } else { None };
                        match dispatch {
                            Some(exc_dispatch) => {
                                // Install exception handler call on the guest stack.
                                // We reuse the IDT-stub frame area; the pre-fault ESP
                                // is carried in saved.esp and will be used on
                                // XCPT_CONTINUE_EXECUTION.
                                setup_exception_dispatch(
                                    &mut *vcpu, self, &mut callback_stack, exc_dispatch,
                                );
                                continue;
                            }
                            None => {
                                // No handlers: fall through to crash report.
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
                                match self.end_by_exception(&mut *vcpu, vcpu_id, initial_esp, &mut callback_stack, TC_EXCEPTION) {
                                    Some(end) => return end,
                                    None => continue,
                                }
                            }
                        }
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
                        // A blocking 16-bit call can be interrupted too (a
                        // kill, the process end; 16-bit code takes no
                        // signal-handler call): it is made again at the
                        // thunk, where the top of the loop ends the thread.
                        interrupt::begin_call(tid, false);
                        let result = self.handle_ne_api_call(&mut *vcpu, vcpu_id, ordinal);
                        if interrupt::end_call() {
                            debug!("  [VCPU {}] 16-bit call {} interrupted; it is made again", vcpu_id, ordinal);
                            continue;
                        }
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
                            // The thread function returned: the thread ends with
                            // its return value; thread 1 returning from the
                            // program's entry point ends the process.
                            let rc = vcpu.get_regs().map_or(0, |r| r.rax as u32);
                            if self.runs_exit_list(vcpu_id) {
                                // An exit-list routine returned instead of
                                // calling DosExitList(EXLST_EXIT): go on.
                                warn!("  [VCPU {}] exit-list routine returned without EXLST_EXIT", vcpu_id);
                                if self.next_exit_routine(&mut *vcpu) { continue; }
                                return ThreadEnd::Process;
                            }
                            if tid == MAIN_TID {
                                info!("  [VCPU {}] Main thread returned; the process ends.", vcpu_id);
                                match self.begin_exit_list(&mut *vcpu, vcpu_id, NO_ERROR, TC_EXIT, initial_esp, &mut callback_stack) {
                                    ExitListStart::Running => continue,
                                    ExitListStart::Finished => return ThreadEnd::Process,
                                    ExitListStart::NotStarted => {}
                                }
                                self.end_process(NO_ERROR);
                                return ThreadEnd::Process;
                            }
                            return ThreadEnd::Thread(rc);
                        }
                        if rip == CALLBACK_RET_TRAP as u64 || rip == XCPT_RET_TRAP as u64 {
                            // Exception and unwind handler calls the guest left
                            // without returning (a handler that longjmp'd out):
                            // a trap on a stack above where they were made
                            // ends them (ADR-0110).
                            let trap_esp = vcpu.get_regs().map_or(0, |r| r.rsp);
                            drop_abandoned_handler_frames(self, &mut callback_stack, trap_esp);
                            if let Some(frame) = callback_stack.pop() {
                                let mut regs = vcpu.get_regs().unwrap();
                                let result = regs.rax as u32;
                                regs.rip = frame.saved_rip;
                                regs.rsp = frame.saved_rsp;
                                match frame.kind {
                                    FrameKind::PmCallback => {
                                        // Return the window-proc result as-is.
                                        // _System is caller-cleanup; the guest caller
                                        // will do `add esp, N` itself.
                                        regs.rax = result as u64;
                                    }
                                    FrameKind::WmCreate { h_frame } => {
                                        // WM_CREATE returned; pass the frame window handle
                                        // back to the WinCreateStdWindow caller.
                                        // (Non-zero WM_CREATE return = veto creation; we
                                        // ignore vetoes for now and always return h_frame.)
                                        regs.rax = h_frame as u64;
                                    }
                                    FrameKind::MinMaxFrame { frame, op, target, ret } => {
                                        // FALSE: the client lets the frame change state.
                                        if result == 0 {
                                            self.shared.window_mgr.lock_or_recover()
                                                .finish_minmax(frame, op, target);
                                        }
                                        regs.rax = ret as u64;
                                    }
                                    FrameKind::DlgRunLoop { dlg_proc, hwnd_dlg, hmq } => {
                                        // Check whether the dialog was dismissed while the
                                        // last message callback was executing.
                                        let dismissed = {
                                            let wm = self.shared.window_mgr.lock_or_recover();
                                            wm.get_window(hwnd_dlg)
                                                .map(|w| (w.dialog_dismissed, w.dialog_result))
                                        };
                                        if let Some((true, _)) = dismissed {
                                            // Dialog dismissed: off the screen, its owner
                                            // enabled again, destroyed (with WM_DESTROY)
                                            // for WinDlgBox; then back to the caller.
                                            finish_dlg_loop(
                                                &mut *vcpu, self, &mut callback_stack,
                                                hwnd_dlg, frame.saved_rip, frame.saved_rsp,
                                            );
                                            continue;
                                        } else {
                                            // Not yet dismissed: dispatch the next queued message.
                                            if setup_dlg_dispatch(
                                                &mut *vcpu, self, &mut callback_stack,
                                                dlg_proc, hwnd_dlg, hmq,
                                                frame.saved_rip, frame.saved_rsp,
                                            ) {
                                                continue; // skip vcpu.set_regs — setup_dlg_dispatch did it
                                            }
                                            // setup_dlg_dispatch returned false = returning to caller.
                                            // regs were set by setup_dlg_dispatch; skip set below.
                                            continue;
                                        }
                                    }
                                    FrameKind::OwnerDraw { chain, call, hps, block_at, block_len } => {
                                        // The owner drew (or measured): finish the
                                        // call, then on with the chain.
                                        let block: Vec<u32> = (0..block_len)
                                            .map(|i| self.guest_read::<u32>(block_at + 4 * i).unwrap_or(0))
                                            .collect();
                                        self.owner_call_finish(&call, hps, result, &block);
                                        continue_owner_draw(
                                            &mut *vcpu, self, &mut callback_stack,
                                            chain, frame.saved_rip, frame.saved_rsp,
                                        );
                                        continue;
                                    }
                                    FrameKind::CallChain { mut chain, stops } => {
                                        // A hook or broadcast call returned: on
                                        // with the chain (pm_hook.rs).
                                        chain.returned(result, stops);
                                        continue_call_chain(
                                            &mut *vcpu, self, &mut callback_stack,
                                            chain, frame.saved_rip, frame.saved_rsp,
                                        );
                                        continue;
                                    }
                                    FrameKind::DestroyWindows { mut chain } => {
                                        // WM_DESTROY returned: on to the next window,
                                        // or back to the caller when all are done.
                                        let next = self.advance_destroy(&mut chain);
                                        continue_destroy(
                                            &mut *vcpu, self, &mut callback_stack,
                                            chain, next, frame.saved_rip, frame.saved_rsp,
                                        );
                                        continue;
                                    }
                                    FrameKind::InitTerm { hmod, phmod } => {
                                        // _DLL_InitTerm returns 1 on success, 0 on failure.
                                        if result != 0 {
                                            debug!("  [VCPU {}] _DLL_InitTerm(hmod={:#x}) succeeded", vcpu_id, hmod);
                                            if phmod != 0 {
                                                self.guest_write::<u32>(phmod, hmod);
                                            }
                                            regs.rax = NO_ERROR as u64;
                                        } else {
                                            warn!("  [VCPU {}] _DLL_InitTerm(hmod={:#x}) returned 0 — init failed", vcpu_id, hmod);
                                            if phmod != 0 {
                                                self.guest_write::<u32>(phmod, 0);
                                            }
                                            regs.rax = ERROR_INIT_ROUTINE_FAILED as u64;
                                        }
                                    }
                                    FrameKind::InitTermUnload { hmod, object_bases } => {
                                        // Unload-time INITTERM: OS/2 does not allow a DLL to
                                        // refuse unloading, so the return value is ignored.
                                        debug!("  [VCPU {}] _DLL_InitTerm(hmod={:#x}, flag=1) unload complete (ret={})", vcpu_id, hmod, result);
                                        self.free_module_objects(object_bases);
                                        regs.rax = NO_ERROR as u64;
                                    }
                                    FrameKind::UnwindHandler { unwind } => {
                                        drive_unwind(&mut *vcpu, self, vcpu_id, &mut callback_stack, unwind);
                                        continue;
                                    }
                                    FrameKind::ExceptionHandler {
                                        saved, next_handler, exc_report, ctx_record, guest_allocs,
                                    } => {
                                        if result == XCPT_CONTINUE_EXECUTION {
                                            // Handler fixed the exception — restore the fault
                                            // context from the CONTEXTRECORD (handler may have
                                            // modified it) and resume at the fault address.
                                            let ctx = self.read_context_record(ctx_record);
                                            regs.rip    = ctx.eip as u64;
                                            regs.rsp    = ctx.esp as u64;
                                            regs.rax    = ctx.eax as u64;
                                            regs.rbx    = ctx.ebx as u64;
                                            regs.rcx    = ctx.ecx as u64;
                                            regs.rdx    = ctx.edx as u64;
                                            regs.rsi    = ctx.esi as u64;
                                            regs.rdi    = ctx.edi as u64;
                                            regs.rbp    = ctx.ebp as u64;
                                            regs.rflags = super::ring::guest_eflags(ctx.eflags);
                                            // A signal continued is acknowledged.
                                            self.signal_continued(exc_report);
                                            self.free_exception_records(guest_allocs);
                                            debug!("  [VCPU {}] SEH: XCPT_CONTINUE_EXECUTION at EIP=0x{:08X}", vcpu_id, ctx.eip);
                                        } else {
                                            // XCPT_CONTINUE_SEARCH (or any other value):
                                            // try the next handler in the chain.
                                            if next_handler == XCPT_CHAIN_END {
                                                // Chain exhausted — unhandled exception.
                                                // A guard page's default action (ADR-0114):
                                                // the guard is gone; go on where it faulted.
                                                if self.continues_by_default(exc_report) {
                                                    debug!("  [VCPU {}] SEH: guard page, no handler took it: going on", vcpu_id);
                                                    self.free_exception_records(guest_allocs);
                                                    resume_at(&mut regs, &saved);
                                                    vcpu.set_regs(&regs).unwrap();
                                                    continue;
                                                }
                                                warn!("  [VCPU {}] SEH: unhandled exception (chain exhausted)", vcpu_id);
                                                self.free_exception_records(guest_allocs);
                                                let tc = self.termination_code(exc_report);
                                                match self.end_by_exception(&mut *vcpu, vcpu_id, initial_esp, &mut callback_stack, tc) {
                                                    Some(end) => return end,
                                                    None => continue,
                                                }
                                            }
                                            let handler_fn = self.guest_read::<u32>(next_handler + XERREC_HANDLER).unwrap_or(0);
                                            let next_next  = self.guest_read::<u32>(next_handler + XERREC_PREV).unwrap_or(XCPT_CHAIN_END);
                                            if handler_fn == 0 {
                                                warn!("  [VCPU {}] SEH: null handler at 0x{:08X}, giving up", vcpu_id, next_handler);
                                                self.free_exception_records(guest_allocs);
                                                let tc = self.termination_code(exc_report);
                                                match self.end_by_exception(&mut *vcpu, vcpu_id, initial_esp, &mut callback_stack, tc) {
                                                    Some(end) => return end,
                                                    None => continue,
                                                }
                                            }
                                            debug!("  [VCPU {}] SEH: XCPT_CONTINUE_SEARCH → next handler=0x{:08X} at 0x{:08X}", vcpu_id, next_handler, handler_fn);
                                            // Re-issue the exception dispatch to the next handler.
                                            let exc_dispatch = ApiResult::ExceptionDispatch {
                                                handler_addr: handler_fn,
                                                exc_report,
                                                reg_rec: next_handler,
                                                ctx_record,
                                                saved,
                                                next_handler: next_next,
                                                guest_allocs,
                                            };
                                            setup_exception_dispatch(
                                                &mut *vcpu, self, &mut callback_stack, exc_dispatch,
                                            );
                                            continue;
                                        }
                                    }
                                }
                                vcpu.set_regs(&regs).unwrap();
                                continue;
                            } else {
                                error!("  [VCPU {}] CALLBACK_RET_TRAP with empty callback stack!", vcpu_id);
                                return ThreadEnd::Process;
                            }
                        }
                        // A call from a thread that was running when another
                        // entered a critical section waits for the section to
                        // end. Giving up: the thread ends at the top of the
                        // loop, and RIP still points at the API thunk.
                        if !self.wait_crit_sec(vcpu_id) {
                            continue;
                        }
                        let ordinal = (rip - MAGIC_API_BASE) as u32;
                        // The host builds frames below the guest's stack
                        // pointer (owner calls, call chains, dialog calls):
                        // one too low, or outside guest memory, ends the
                        // process as a fault would, never the host.
                        let esp = vcpu.get_regs().unwrap().rsp;
                        if !guest_stack_ok(esp) {
                            log::warn!("[VCPU {}] API call {} with ESP 0x{:08X} outside the usable stack: ending the process",
                                       vcpu_id, ordinal, esp);
                            match self.end_by_exception(&mut *vcpu, vcpu_id, initial_esp, &mut callback_stack, TC_EXCEPTION) {
                                Some(end) => return end,
                                None => continue,
                            }
                        }
                        // A blocking call may be interrupted (interrupt.rs): a
                        // signal for thread 1 only when it can take it now.
                        let signals_ok = tid == MAIN_TID && self.can_take_signal(&*vcpu);
                        interrupt::begin_call(tid, signals_ok);
                        let api_result = self.handle_api_call_ex(&mut *vcpu, vcpu_id, ordinal);
                        if interrupt::end_call() && matches!(api_result, ApiResult::Normal(_)) {
                            // Interrupted, to be made again: the guest stays at
                            // the thunk with the call's stack; the top of the
                            // loop delivers the signal (or ends the thread).
                            debug!("  [VCPU {}] call {} interrupted; it is made again", vcpu_id, ordinal);
                            continue;
                        }
                        match api_result {
                            ApiResult::ExitThread(rc) => return ThreadEnd::Thread(rc),
                            ApiResult::ExitProcess(rc) => {
                                // Thread 1 runs the exit list (exitlist.rs).
                                if self.exit_on_thread_1(vcpu_id, rc) {
                                    return ThreadEnd::Process;
                                }
                                match self.begin_exit_list(&mut *vcpu, vcpu_id, rc, TC_EXIT, initial_esp, &mut callback_stack) {
                                    ExitListStart::Running => continue,
                                    ExitListStart::Finished => return ThreadEnd::Process,
                                    ExitListStart::NotStarted => {}
                                }
                                self.end_process(rc);
                                return ThreadEnd::Process;
                            }
                            ApiResult::ExitListNext => {
                                if self.next_exit_routine(&mut *vcpu) { continue; }
                                return ThreadEnd::Process;
                            }
                            ApiResult::Normal(res) => {
                                let mut regs = vcpu.get_regs().unwrap();
                                regs.rax = res as u64;
                                regs.rip = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for return address") as u64;
                                regs.rsp += 4;
                                // VIO/KBD use Pascal calling convention (callee cleanup).
                                // Pop the arguments from the stack after the return address.
                                if (VIOCALLS_BASE..SESMGR_BASE).contains(&ordinal) {
                                    regs.rsp += self.viocalls_arg_bytes(ordinal - VIOCALLS_BASE);
                                } else if (KBDCALLS_BASE..VIOCALLS_BASE).contains(&ordinal) {
                                    regs.rsp += self.kbdcalls_arg_bytes(ordinal - KBDCALLS_BASE);
                                } else if (MOUCALLS_BASE..PMSHAPI_BASE).contains(&ordinal) {
                                    regs.rsp += self.moucalls_arg_bytes(ordinal - MOUCALLS_BASE);
                                } else if (MSG_BASE..MDM_BASE).contains(&ordinal) {
                                    regs.rsp += super::msgcalls::msg_callee_pop(ordinal - MSG_BASE);
                                }
                                vcpu.set_regs(&regs).unwrap();
                                // Owner drawing queued by the call runs before
                                // its result reaches the caller (pm_owner.rs).
                                if let Some(hmq) = self.begin_owner_chain_for_thread(vcpu_id) {
                                    let chain = Box::new(super::pm_owner::OwnerChain {
                                        hmq, then: super::pm_owner::OwnerThen::Return(res),
                                    });
                                    continue_owner_draw(&mut *vcpu, self, &mut callback_stack,
                                                        chain, regs.rip, regs.rsp);
                                }
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
                                    kind: FrameKind::PmCallback,
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
                            ApiResult::MinMaxFrame { wnd_proc, client, frame, op, target, ret } => {
                                // Send WM_MINMAXFRAME (mp1 = PSWP) to the client;
                                // FrameKind::MinMaxFrame finishes the change.
                                let mut regs = vcpu.get_regs().unwrap();
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for WM_MINMAXFRAME return address");
                                callback_stack.push(CallbackFrame {
                                    saved_rip: return_addr as u64,
                                    saved_rsp: regs.rsp + 4,
                                    kind: FrameKind::MinMaxFrame { frame, op, target, ret },
                                });
                                // The SWP lies just below the caller's stack, above
                                // the callback's return address and arguments.
                                regs.rsp -= 20 + pm_minmax::SWP_STRUCT_SIZE as u64;
                                let sp = regs.rsp as u32;
                                let pswp = sp + 20;
                                for (i, v) in pm_minmax::minmax_swp(frame, op, target).into_iter().enumerate() {
                                    self.guest_write::<u32>(pswp + 4 * i as u32, v).expect("Callback stack write OOB");
                                }
                                for (o, v) in [(0, CALLBACK_RET_TRAP), (4, client), (8, WM_MINMAXFRAME), (12, pswp), (16, 0)] {
                                    self.guest_write::<u32>(sp + o, v).expect("Callback stack write OOB");
                                }
                                regs.rip = wnd_proc as u64;
                                vcpu.set_regs(&regs).unwrap();
                            }
                            ApiResult::WmCreateCallback { wnd_proc, hwnd, h_frame } => {
                                // Dispatch WM_CREATE synchronously; after the callback
                                // returns CALLBACK_RET_TRAP, FrameKind::WmCreate writes
                                // h_frame into EAX as WinCreateStdWindow's return value.
                                let mut regs = vcpu.get_regs().unwrap();
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for WM_CREATE return address");
                                callback_stack.push(CallbackFrame {
                                    saved_rip: return_addr as u64,
                                    saved_rsp: regs.rsp + 4,
                                    kind: FrameKind::WmCreate { h_frame },
                                });
                                regs.rsp -= 20;
                                let sp = regs.rsp as u32;
                                self.guest_write::<u32>(sp,      CALLBACK_RET_TRAP).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp +  4, hwnd).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp +  8, WM_CREATE).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp + 12, 0).expect("Callback stack write OOB"); // mp1 (CREATESTRUCT*)
                                self.guest_write::<u32>(sp + 16, 0).expect("Callback stack write OOB"); // mp2 (CTLDATA*)
                                regs.rip = wnd_proc as u64;
                                vcpu.set_regs(&regs).unwrap();
                            }
                            ApiResult::DlgRunLoop { dlg_proc, hwnd_dlg, hmq } => {
                                let regs = vcpu.get_regs().unwrap();
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for DlgRunLoop return address");
                                let saved_rip = return_addr as u64;
                                let saved_rsp = regs.rsp + 4; // past return addr; caller cleans args
                                // Restore regs so setup_dlg_dispatch sees a clean state.
                                vcpu.set_regs(&regs).unwrap();
                                if setup_dlg_dispatch(
                                    &mut *vcpu, self, &mut callback_stack,
                                    dlg_proc, hwnd_dlg, hmq,
                                    saved_rip, saved_rsp,
                                ) {
                                    continue;
                                }
                                // setup_dlg_dispatch returned false (dismissed immediately or
                                // no queue) — registers already set, fall through.
                            }
                            ApiResult::DestroyWindows { chain, hwnd, wnd_proc } => {
                                let regs = vcpu.get_regs().unwrap();
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for WinDestroyWindow return address");
                                continue_destroy(
                                    &mut *vcpu, self, &mut callback_stack,
                                    chain, Some((hwnd, wnd_proc)),
                                    return_addr as u64, regs.rsp + 4,
                                );
                            }
                            ApiResult::CallChain(chain) => {
                                let regs = vcpu.get_regs().unwrap();
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for call-chain return address");
                                continue_call_chain(
                                    &mut *vcpu, self, &mut callback_stack,
                                    chain, return_addr as u64, regs.rsp + 4,
                                );
                            }
                            ApiResult::ExceptionDispatch { .. } => {
                                let mut regs = vcpu.get_regs().unwrap();
                                // Pop the API return address from the guest stack
                                // (the standard API dispatch leaves this on top).
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for ExceptionDispatch return address");
                                regs.rsp += 4;
                                // Rebuild the FaultContext so XCPT_CONTINUE_EXECUTION returns
                                // to the DosRaiseException caller (i.e. normal API return).
                                if let ApiResult::ExceptionDispatch {
                                    handler_addr, exc_report, reg_rec, ctx_record,
                                    mut saved, next_handler, guest_allocs,
                                } = api_result {
                                    // For DosRaiseException the "saved" context EIP/ESP should
                                    // point back to the API caller so XCPT_CONTINUE_EXECUTION
                                    // effectively returns NO_ERROR from DosRaiseException.
                                    saved.eip = return_addr;
                                    saved.esp = regs.rsp as u32;
                                    // The caller's registers too: continuing must not
                                    // clobber the callee-saved EBX / ESI / EDI / EBP, and
                                    // DosRaiseException then returns NO_ERROR in EAX.
                                    saved.eax = NO_ERROR;
                                    saved.ebx = regs.rbx as u32;
                                    saved.ecx = regs.rcx as u32;
                                    saved.edx = regs.rdx as u32;
                                    saved.esi = regs.rsi as u32;
                                    saved.edi = regs.rdi as u32;
                                    saved.ebp = regs.rbp as u32;
                                    saved.eflags = regs.rflags as u32;
                                    // Write the context into the CONTEXTRECORD so the
                                    // handler sees (and may change) it.
                                    self.write_context_record(ctx_record, &saved);
                                    setup_exception_dispatch(
                                        &mut *vcpu, self, &mut callback_stack,
                                        ApiResult::ExceptionDispatch {
                                            handler_addr, exc_report, reg_rec, ctx_record,
                                            saved, next_handler, guest_allocs,
                                        },
                                    );
                                }
                            }
                            ApiResult::Unwind(mut unwind) => {
                                // DosUnwindException: the caller's state, which
                                // the handlers see and the thread goes on with
                                // (at pTargetIP, or the return address).
                                let regs = vcpu.get_regs().unwrap();
                                let sregs = vcpu.get_sregs().unwrap();
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for DosUnwindException return address");
                                let caller = FaultContext {
                                    eax: NO_ERROR, ebx: regs.rbx as u32, ecx: regs.rcx as u32, edx: regs.rdx as u32,
                                    esi: regs.rsi as u32, edi: regs.rdi as u32, ebp: regs.rbp as u32,
                                    esp: (regs.rsp + 4) as u32, eip: return_addr, eflags: regs.rflags as u32,
                                    cs: sregs.cs.selector, ds: sregs.ds.selector, es: sregs.es.selector,
                                    fs: sregs.fs.selector, gs: sregs.gs.selector, ss: sregs.ss.selector,
                                };
                                self.write_context_record(unwind.ctx, &caller);
                                if unwind.own_report {
                                    self.guest_write::<u32>(unwind.report + ERR_ADDR, return_addr);
                                }
                                let target_ip = unwind.target_ip;
                                unwind.resume = FaultContext { eip: if target_ip != 0 { target_ip } else { return_addr }, ..caller };
                                drive_unwind(&mut *vcpu, self, vcpu_id, &mut callback_stack, unwind);
                            }
                            ApiResult::CallGuest { addr, hmod, phmod, object_bases } => {
                                let mut regs = vcpu.get_regs().unwrap();
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for INITTERM return address");
                                // Distinguish load-time (flag=0, object_bases empty) from
                                // unload-time (flag=1, object_bases carries pages to free).
                                let (kind, flag) = if object_bases.is_empty() {
                                    (FrameKind::InitTerm { hmod, phmod }, 0u32)
                                } else {
                                    (FrameKind::InitTermUnload { hmod, object_bases }, 1u32)
                                };
                                callback_stack.push(CallbackFrame {
                                    saved_rip: return_addr as u64,
                                    saved_rsp: regs.rsp + 4,
                                    kind,
                                });
                                // _DLL_InitTerm(hmod: u32, flag: u32) — _System, 2 args.
                                // Push: [CALLBACK_RET_TRAP][hmod][flag] = 12 bytes.
                                regs.rsp -= 12;
                                let sp = regs.rsp as u32;
                                self.guest_write::<u32>(sp,     CALLBACK_RET_TRAP).expect("INITTERM stack write OOB");
                                self.guest_write::<u32>(sp + 4, hmod).expect("INITTERM stack write OOB");
                                self.guest_write::<u32>(sp + 8, flag).expect("INITTERM stack write OOB");
                                regs.rip = addr as u64;
                                vcpu.set_regs(&regs).unwrap();
                            }
                        }
                    }
                    // A breakpoint instruction in a 32-bit program's own
                    // code: XCPT_BREAKPOINT to its handlers, with EIP past
                    // the instruction, as the processor's trap frame has
                    // it (Control Program Reference); unhandled, it ends
                    // the process like any other exception.
                    else if let Some(len) = self.program_breakpoint(&*vcpu, rip) {
                        let regs = vcpu.get_regs().unwrap();
                        let sregs = vcpu.get_sregs().unwrap();
                        let after = (rip as u32).wrapping_add(len);
                        let fault_ctx = FaultContext {
                            eax: regs.rax as u32, ebx: regs.rbx as u32, ecx: regs.rcx as u32, edx: regs.rdx as u32,
                            esi: regs.rsi as u32, edi: regs.rdi as u32, ebp: regs.rbp as u32,
                            esp: regs.rsp as u32, eip: after, eflags: regs.rflags as u32,
                            cs: sregs.cs.selector, ds: sregs.ds.selector, es: sregs.es.selector,
                            fs: sregs.fs.selector, gs: sregs.gs.selector, ss: sregs.ss.selector,
                        };
                        let dispatch = if guest_stack_ok(regs.rsp) {
                            self.chain_dispatch(vcpu_id, fault_ctx, XCPT_BREAKPOINT, 0, &[])
                        } else { None };
                        if let Some(exc_dispatch) = dispatch {
                            debug!("  [VCPU {}] INT3 at 0x{:08X}: XCPT_BREAKPOINT", vcpu_id, rip);
                            setup_exception_dispatch(&mut *vcpu, self, &mut callback_stack, exc_dispatch);
                            continue;
                        }
                        let report = self.collect_crash_report(&*vcpu, vcpu_id, CrashContext::UnexpectedBreakpoint);
                        self.dump_crash_report(&report);
                        match self.end_by_exception(&mut *vcpu, vcpu_id, initial_esp, &mut callback_stack, TC_EXCEPTION) {
                            Some(end) => return end,
                            None => continue,
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
                        return ThreadEnd::Process;
                    }
                }
                // Kicked out of the guest (VcpuKick): the top of the loop
                // looks at the flags; guest state is as it was.
                VmExit::Interrupted => continue,
                VmExit::Hlt => {
                    // The guest stopped its processor: a trap, its exit
                    // list runs as after any other (TC_TRAP).
                    info!("  [VCPU {}] Guest HLT.", vcpu_id);
                    match self.end_by_fault(&mut *vcpu, vcpu_id, initial_esp, restart_sregs.as_ref(),
                                            &mut callback_stack, TC_TRAP) {
                        Some(end) => return end,
                        None => continue,
                    }
                }
                VmExit::MmioRead { addr, size } => {
                    // zeros already filled by kvm_backend
                    warn!("  [VCPU {}] MMIO read at 0x{:08X} ({} bytes) — returning zeros",
                           vcpu_id, addr, size);
                    // Rate-limit repeated accesses to unmapped memory to avoid
                    // spinning the VCPU thread at 100% CPU.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    // The top of the loop may point the vCPU elsewhere (a
                    // termination's exit list): finish the access first.
                    finish_mmio(&mut *vcpu);
                }
                VmExit::MmioWrite { addr } => {
                    // Guest write to unmapped memory — silently ignore
                    warn!("  [VCPU {}] MMIO write at 0x{:08X} — ignoring", vcpu_id, addr);
                    // Rate-limit same as MmioRead.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    finish_mmio(&mut *vcpu);
                }
                VmExit::Shutdown => {
                    let report = self.collect_crash_report(
                        &*vcpu, vcpu_id, CrashContext::TripleFault,
                    );
                    self.dump_crash_report(&report);
                    // The guest's own fault: its exit list runs (TC_TRAP),
                    // on this vCPU with its starting segment state put back.
                    match self.end_by_fault(&mut *vcpu, vcpu_id, initial_esp, restart_sregs.as_ref(),
                                            &mut callback_stack, TC_TRAP) {
                        Some(end) => return end,
                        None => continue,
                    }
                }
                VmExit::Other(e) => {
                    let report = self.collect_crash_report(
                        &*vcpu, vcpu_id,
                        CrashContext::UnhandledVmexit { description: e.clone() },
                    );
                    self.dump_crash_report(&report);
                    // An exit the guest caused that Warpine cannot handle
                    // (port I/O …): like a trap.
                    match self.end_by_fault(&mut *vcpu, vcpu_id, initial_esp, restart_sregs.as_ref(),
                                            &mut callback_stack, TC_TRAP) {
                        Some(end) => return end,
                        None => continue,
                    }
                }
            }
        }
    }
}

/// Set up a guest call to an OS/2 exception handler.
///
/// Pushes a `FrameKind::ExceptionHandler` onto `callback_stack`, then adjusts
/// the vCPU registers to call `handler_addr` with the four standard OS/2
/// exception-handler arguments (cdecl/caller-cleanup convention):
///
/// ```text
/// [ESP+0]  XCPT_RET_TRAP       ← return address (an unwinder recognises it, ADR-0110)
/// [ESP+4]  pExceptionReport
/// [ESP+8]  pRegistrationRecord
/// [ESP+12] pContextRecord
/// [ESP+16] pDispatcherContext (0)
/// ```
fn setup_exception_dispatch(
    vcpu: &mut dyn super::vm_backend::VcpuBackend,
    loader: &super::Loader,
    callback_stack: &mut Vec<CallbackFrame>,
    exc_dispatch: ApiResult,
) {
    let ApiResult::ExceptionDispatch {
        handler_addr, exc_report, reg_rec, ctx_record,
        saved, next_handler, guest_allocs,
    } = exc_dispatch else { return };

    let mut regs = vcpu.get_regs().unwrap();

    // Push the callback frame so we know what to do when the handler returns.
    callback_stack.push(CallbackFrame {
        saved_rip: saved.eip as u64,
        saved_rsp: saved.esp as u64,
        kind: FrameKind::ExceptionHandler {
            saved,
            next_handler,
            exc_report,
            ctx_record,
            guest_allocs,
        },
    });

    // Build the handler call on the guest stack: 5 × u32 = 20 bytes.
    regs.rsp -= 20;
    let sp = regs.rsp as u32;
    loader.guest_write::<u32>(sp,      XCPT_RET_TRAP).expect("SEH stack write OOB");
    loader.guest_write::<u32>(sp +  4, exc_report).expect("SEH stack write OOB");
    loader.guest_write::<u32>(sp +  8, reg_rec).expect("SEH stack write OOB");
    loader.guest_write::<u32>(sp + 12, ctx_record).expect("SEH stack write OOB");
    loader.guest_write::<u32>(sp + 16, 0).expect("SEH stack write OOB"); // pDispCtx
    regs.rip = handler_addr as u64;
    vcpu.set_regs(&regs).unwrap();
}

/// Go on with `unwind` (ADR-0110): call its next handler with the
/// four exception-handler arguments under a `FrameKind::UnwindHandler`
/// frame, below the caller's stack; or, when it is done, free its records
/// and go on with the registers it resumes with.
fn drive_unwind(
    vcpu: &mut dyn super::vm_backend::VcpuBackend,
    loader: &super::Loader,
    vcpu_id: u32,
    callback_stack: &mut Vec<CallbackFrame>,
    mut unwind: Box<super::seh::Unwind>,
) {
    let mut regs = vcpu.get_regs().unwrap();
    let r = unwind.resume.clone();
    match loader.unwind_step(vcpu_id, &mut unwind) {
        Some((handler, rec)) => {
            let (report, ctx) = (unwind.report, unwind.ctx);
            callback_stack.push(CallbackFrame {
                saved_rip: r.eip as u64,
                saved_rsp: r.esp as u64,
                kind: FrameKind::UnwindHandler { unwind },
            });
            let sp = r.esp.wrapping_sub(20);
            for (i, v) in [XCPT_RET_TRAP, report, rec, ctx, 0].into_iter().enumerate() {
                loader.guest_write::<u32>(sp + 4 * i as u32, v).expect("unwind stack write OOB");
            }
            regs.rsp = sp as u64;
            regs.rip = handler as u64;
        }
        None => {
            loader.free_exception_records(std::mem::take(&mut unwind.allocs));
            regs.rip = r.eip as u64;
            regs.rsp = r.esp as u64;
            regs.rax = r.eax as u64;
            regs.rbx = r.ebx as u64;
            regs.rcx = r.ecx as u64;
            regs.rdx = r.edx as u64;
            regs.rsi = r.esi as u64;
            regs.rdi = r.edi as u64;
            regs.rbp = r.ebp as u64;
            regs.rflags = super::ring::guest_eflags(r.eflags);
        }
    }
    vcpu.set_regs(&regs).unwrap();
}

/// Drop the exception and unwind handler frames on top of `callback_stack`
/// that the guest left without returning: a handler call is made below
/// the stack it returns to, so a trap whose stack lies above that is
/// another call's, and those frames' handlers are gone (a handler that
/// longjmp'd out, ADR-0110). Their records are freed.
fn drop_abandoned_handler_frames(loader: &super::Loader, callback_stack: &mut Vec<CallbackFrame>, trap_esp: u64) {
    while let Some(top) = callback_stack.last()
        && matches!(top.kind, FrameKind::ExceptionHandler { .. } | FrameKind::UnwindHandler { .. })
        && trap_esp > top.saved_rsp
    {
        let Some(frame) = callback_stack.pop() else { break };
        debug!("dropping an abandoned handler frame (returns to ESP 0x{:08X}, trap at 0x{:08X})", frame.saved_rsp, trap_esp);
        match frame.kind {
            FrameKind::ExceptionHandler { guest_allocs, .. } => loader.free_exception_records(guest_allocs),
            FrameKind::UnwindHandler { mut unwind } => loader.free_exception_records(std::mem::take(&mut unwind.allocs)),
            _ => {}
        }
    }
}

/// Drive the modal dialog message loop for WinDlgBox / WinProcessDlg.
///
/// This function either:
/// * Dispatches the next message from `hmq` to its window's procedure —
///   `dlg_proc` for the dialog — (pushes a `FrameKind::DlgRunLoop` frame onto
///   `callback_stack`, sets guest IP to the procedure, then returns — caller
///   must `continue` the VMEXIT loop). Messages for built-in controls are
///   handled on the spot.
/// * Returns to the WinDlgBox/WinProcessDlg caller (`saved_rip`/`saved_rsp`)
///   with EAX = dialog result (sets registers and returns — caller should
///   NOT `continue` independently; `vcpu.set_regs` is already done).
///
/// Use `setup_dlg_dispatch_returns_to_caller` as a boolean:
/// The function sets `*dispatched = false` when returning to caller, and
/// `true` when dispatching a message.  Caller should `continue` iff `true`.
/// Bytes of a window-procedure call frame on the guest stack: the return
/// address and the four arguments (hwnd, msg, mp1, mp2).
const DLG_CALL_FRAME_BYTES: u64 = 20;

/// The lowest guest stack pointer below which the host builds frames: more
/// than the largest frame it puts there (an owner call's block,
/// `MAX_OWNER_BLOCK_BYTES` with its return address and arguments).
pub(crate) const MIN_GUEST_ESP: u64 = 0x2000;

/// Whether the guest's stack pointer `esp` leaves room for the host's
/// frames below it and its return address above it in guest memory.
/// Make KVM drop the TLB of a paged vCPU (ADR-0114): a CR3 that differs
/// only in its write-through bit makes KVM reload the vCPU's paging, and
/// its translations with it; the next flush toggles it back.
fn flush_tlb(vcpu: &mut dyn VcpuBackend) {
    let Ok(mut sregs) = vcpu.get_sregs() else { return };
    if sregs.cr0 & super::paging::CR0_PG == 0 {
        return;
    }
    sregs.cr3 ^= super::paging::CR3_PWT;
    if let Err(e) = vcpu.set_sregs(&sregs) {
        warn!("TLB flush: {e}");
    }
}

/// Point `regs` at `ctx`: the thread goes on where it faulted, with the
/// registers it had there (EFLAGS as a program may set them).
fn resume_at(regs: &mut super::vm_backend::GuestRegs, ctx: &FaultContext) {
    regs.rip = u64::from(ctx.eip);
    regs.rsp = u64::from(ctx.esp);
    regs.rax = u64::from(ctx.eax);
    regs.rbx = u64::from(ctx.ebx);
    regs.rcx = u64::from(ctx.ecx);
    regs.rdx = u64::from(ctx.edx);
    regs.rsi = u64::from(ctx.esi);
    regs.rdi = u64::from(ctx.edi);
    regs.rbp = u64::from(ctx.ebp);
    regs.rflags = super::ring::guest_eflags(ctx.eflags);
}

pub(crate) fn guest_stack_ok(esp: u64) -> bool {
    (MIN_GUEST_ESP..=u64::from(GUEST_MEM_SIZE) - 16).contains(&esp)
}

#[allow(clippy::too_many_arguments)]
fn setup_dlg_dispatch(
    vcpu: &mut dyn super::vm_backend::VcpuBackend,
    loader: &super::Loader,
    callback_stack: &mut Vec<CallbackFrame>,
    dlg_proc: u32,
    hwnd_dlg: u32,
    hmq: u32,
    saved_rip: u64,
    saved_rsp: u64,
) -> bool {
    use super::mutex_ext::MutexExt;

    // End the modal loop: return to the WinDlgBox / WinProcessDlg caller with
    // the dialog result (DID_CANCEL unless it was dismissed), after taking
    // the dialog off the screen and enabling its owner again.
    let finish = |vcpu: &mut dyn super::vm_backend::VcpuBackend, callback_stack: &mut Vec<CallbackFrame>| {
        finish_dlg_loop(vcpu, loader, callback_stack, hwnd_dlg, saved_rip, saved_rsp);
        false
    };

    let mq_arc = loader.shared.window_mgr.lock_or_recover().get_mq(hmq);
    let Some(mq_arc) = mq_arc else { return finish(vcpu, callback_stack) };

    let (cond, wait_lock) = {
        let mq = mq_arc.lock_or_recover();
        (Arc::clone(&mq.cond), Arc::clone(&mq.lock))
    };

    loop {
        // Check if the dialog has been dismissed by WinDismissDlg.
        // Dismissed, or destroyed (DID_CANCEL).
        let (ended, owner) = {
            let wm = loader.shared.window_mgr.lock_or_recover();
            (wm.get_window(hwnd_dlg).is_none_or(|w| w.dialog_dismissed), wm.queue_thread(hmq))
        };
        // A thread killed (DosKillThread) leaves its modal loop, which ends
        // the dialog as DID_CANCEL; the thread then ends at its next pass.
        let killed = owner.is_some_and(|v| loader.shared.threads.table.lock_or_recover().is_killed(tid_of_vcpu(v)));
        if ended || killed || loader.shutting_down() {
            return finish(vcpu, callback_stack);
        }
        // A signal for thread 1, or a request to end the process, is taken
        // between messages (ADR-0093): see `leave_dlg_loop_for_signal`.
        if owner.is_some_and(|v| loader.signal_waits_for(v)) {
            leave_dlg_loop_for_signal(vcpu, loader, callback_stack, dlg_proc, hwnd_dlg, hmq, saved_rip, saved_rsp);
            return true;
        }

        // A message another program sent runs first, then the loop goes on
        // (pm_remote.rs).
        if let Some(sent) = loader.take_sent(hmq)
            && let ApiResult::CallChain(chain) = loader.run_sent(owner.unwrap_or(0), sent,
                super::pm_remote::AfterSent::ModalLoop { dlg_proc, hwnd_dlg, hmq }) {
            continue_call_chain(vcpu, loader, callback_stack, chain, saved_rip, saved_rsp);
            return true;
        }

        // The next message in PM's order, as WinGetMsg takes it.
        let msg_opt = loader.shared.window_mgr.lock_or_recover().next_queued(hmq, true, |_, _| true);
        // A pointer event waits for its window's WM_HITTEST answer.
        if msg_opt.is_none()
            && let Some(ask) = loader.shared.window_mgr.lock_or_recover().take_hit_ask(hmq)
            && let ApiResult::CallChain(chain) = loader.hit_test_chain(owner.unwrap_or(0), ask,
                super::pm_remote::AfterSent::ModalLoop { dlg_proc, hwnd_dlg, hmq }) {
            continue_call_chain(vcpu, loader, callback_stack, chain, saved_rip, saved_rsp);
            return true;
        }

        if let Some(msg) = msg_opt {
            // WM_QUIT terminates the dialog loop with DID_CANCEL.
            if msg.msg == WM_QUIT {
                return finish(vcpu, callback_stack);
            }

            // Like WinGetMsg + WinCallMsgFilter + WinDispatchMsg:
            // accelerators are translated, the message-filter hooks see the
            // message (a chain; the loop goes on when they returned,
            // pm_hook.rs), and each message goes to its own window's
            // procedure.
            let mut msg = msg;
            loader.translate_queued_accelerator(&mut msg);
            let pqmsg = (saved_rsp - 4) as u32 - super::pm_hook::CHAIN_BLOCK_BYTES;
            if let Some(chain) = loader.modal_filter_chain(dlg_proc, hwnd_dlg, hmq, &msg, pqmsg) {
                continue_call_chain(vcpu, loader, callback_stack, chain, saved_rip, saved_rsp);
                return true;
            }
            if dispatch_dlg_message(vcpu, loader, callback_stack, dlg_proc, hwnd_dlg, hmq, &msg,
                                    saved_rip, saved_rsp) {
                return true;
            }
            continue;
        }

        // No message; block on the condvar for up to 100 ms.
        let guard = wait_lock.lock_or_recover();
        let _ = cond.wait_timeout(guard, std::time::Duration::from_millis(100)).unwrap();
    }
}

/// Leave the modal loop of dialog `hwnd_dlg` for the vCPU loop so that
/// thread 1 takes a signal (ADR-0093): as if a message's procedure had been
/// called and were about to return — the guest at `CALLBACK_RET_TRAP` on
/// the stack a message call would use, under a `FrameKind::DlgRunLoop`
/// frame. The vCPU loop delivers the signal with that point as its
/// context; when a handler continues, the trap pops the frame and the
/// loop goes on. A signal whose default action applies ends the process
/// there (its exit list runs) as anywhere else.
#[allow(clippy::too_many_arguments)]
fn leave_dlg_loop_for_signal(
    vcpu: &mut dyn super::vm_backend::VcpuBackend,
    loader: &super::Loader,
    callback_stack: &mut Vec<CallbackFrame>,
    dlg_proc: u32,
    hwnd_dlg: u32,
    hmq: u32,
    saved_rip: u64,
    saved_rsp: u64,
) {
    callback_stack.push(CallbackFrame {
        saved_rip,
        saved_rsp,
        kind: FrameKind::DlgRunLoop { dlg_proc, hwnd_dlg, hmq },
    });
    let mut regs = vcpu.get_regs().unwrap();
    regs.rsp = saved_rsp - 4 - DLG_CALL_FRAME_BYTES;
    loader.guest_write::<u32>(regs.rsp as u32, CALLBACK_RET_TRAP).expect("DlgRunLoop stack OOB");
    regs.rip = CALLBACK_RET_TRAP as u64;
    vcpu.set_regs(&regs).unwrap();
}

/// Dispatch `msg`, taken by the modal loop of dialog `hwnd_dlg`, to its
/// window's procedure (the dialog procedure for the dialog, the owner's
/// window procedure for a repaint behind the dialog) under a
/// `FrameKind::DlgRunLoop` frame, or to the built-in controls. TRUE when a
/// guest call was set up (the caller `continue`s the VMEXIT loop), FALSE
/// when the message was handled on the host and the loop takes the next.
#[allow(clippy::too_many_arguments)]
fn dispatch_dlg_message(
    vcpu: &mut dyn super::vm_backend::VcpuBackend,
    loader: &super::Loader,
    callback_stack: &mut Vec<CallbackFrame>,
    dlg_proc: u32,
    hwnd_dlg: u32,
    hmq: u32,
    msg: &super::pm_types::OS2Message,
    saved_rip: u64,
    saved_rsp: u64,
) -> bool {
    use super::mutex_ext::MutexExt;
    let wnd_proc = loader.shared.window_mgr.lock_or_recover()
        .get_window(msg.hwnd).map_or(0, |w| w.pfn_wp);
    if wnd_proc == 0 {
        if !(msg.msg == WM_PAINT && loader.repaint_control(msg.hwnd)) {
            loader.dispatch_builtin_control(msg.hwnd, msg.msg, msg.mp1, msg.mp2);
        }
        // Owner drawing the control queued runs now; the loop goes
        // on when it is done (pm_owner.rs).
        if loader.shared.window_mgr.lock_or_recover().begin_owner_chain(hmq) {
            let chain = Box::new(super::pm_owner::OwnerChain {
                hmq, then: super::pm_owner::OwnerThen::DlgLoop { dlg_proc, hwnd_dlg, hmq },
            });
            continue_owner_draw(vcpu, loader, callback_stack, chain, saved_rip, saved_rsp);
            return true;
        }
        return false;
    }

    // Push the DlgRunLoop frame so the loop continues after the callback.
    callback_stack.push(CallbackFrame {
        saved_rip,
        saved_rsp,
        kind: FrameKind::DlgRunLoop { dlg_proc, hwnd_dlg, hmq },
    });

    // Set up the guest stack for a standard WndProc call, always at
    // the same place: just below the WinDlgBox / WinProcessDlg
    // return address (`saved_rsp - 4`). The procedure's `ret`
    // leaves rsp 16 bytes below where its frame began, so building
    // the next call from the current rsp would sink the stack by 16
    // bytes for every message dispatched during the dialog.
    let mut regs = vcpu.get_regs().unwrap();
    regs.rsp = saved_rsp - 4 - DLG_CALL_FRAME_BYTES;
    let sp = regs.rsp as u32;
    loader.guest_write::<u32>(sp,      CALLBACK_RET_TRAP).expect("DlgRunLoop stack OOB");
    loader.guest_write::<u32>(sp +  4, msg.hwnd).expect("DlgRunLoop stack OOB");
    loader.guest_write::<u32>(sp +  8, msg.msg).expect("DlgRunLoop stack OOB");
    loader.guest_write::<u32>(sp + 12, msg.mp1).expect("DlgRunLoop stack OOB");
    loader.guest_write::<u32>(sp + 16, msg.mp2).expect("DlgRunLoop stack OOB");
    regs.rip = wnd_proc as u64;
    vcpu.set_regs(&regs).unwrap();
    true
}

/// End a modal dialog loop and return to the WinDlgBox / WinProcessDlg
/// caller (`saved_rip` / `saved_rsp`) with the dialog result: the dialog
/// leaves the screen, its owner is enabled again, and for WinDlgBox it is
/// destroyed first — with `WM_DESTROY`, which may call the guest
/// (`continue_destroy`). The registers are set either way; the caller
/// `continue`s the VMEXIT loop.
fn finish_dlg_loop(
    vcpu: &mut dyn super::vm_backend::VcpuBackend,
    loader: &super::Loader,
    callback_stack: &mut Vec<CallbackFrame>,
    hwnd_dlg: u32,
    saved_rip: u64,
    saved_rsp: u64,
) {
    let (result, destroy) = loader.shared.window_mgr.lock_or_recover().end_modal_dialog(hwnd_dlg);
    // WinFileDlg / WinFontDlg return TRUE (their outcome is in lReturn).
    let result = loader.std_dialog_return(hwnd_dlg, result);
    let outcome = if destroy { loader.destroy_dialog(hwnd_dlg, result) } else { ApiResult::Normal(result) };
    match outcome {
        ApiResult::DestroyWindows { chain, hwnd, wnd_proc } => continue_destroy(
            vcpu, loader, callback_stack, chain, Some((hwnd, wnd_proc)), saved_rip, saved_rsp,
        ),
        other => {
            let result = match other { ApiResult::Normal(v) => v, _ => result };
            let mut regs = vcpu.get_regs().unwrap();
            regs.rip = saved_rip;
            regs.rsp = saved_rsp;
            regs.rax = result as u64;
            vcpu.set_regs(&regs).unwrap();
        }
    }
}

/// Go on with a window destruction (`pm_destroy.rs`): with `next` =
/// `Some((hwnd, wnd_proc))`, call `wnd_proc(hwnd, WM_DESTROY, 0, 0)` under a
/// `FrameKind::DestroyWindows` frame that resumes the chain on return;
/// with `None` (the chain has ended and the windows are gone), return the
/// chain's result to the caller at `saved_rip` / `saved_rsp`. Every call
/// is built just below `saved_rsp`, so a long chain does not use up stack.
fn continue_destroy(
    vcpu: &mut dyn super::vm_backend::VcpuBackend,
    loader: &super::Loader,
    callback_stack: &mut Vec<CallbackFrame>,
    chain: Box<super::pm_destroy::DestroyChain>,
    next: Option<(u32, u32)>,
    saved_rip: u64,
    saved_rsp: u64,
) {
    let mut regs = vcpu.get_regs().unwrap();
    match next {
        Some((hwnd, wnd_proc)) => {
            callback_stack.push(CallbackFrame {
                saved_rip,
                saved_rsp,
                kind: FrameKind::DestroyWindows { chain },
            });
            // _System window procedure: return address + 4 arguments.
            regs.rsp = saved_rsp - 20;
            let sp = regs.rsp as u32;
            loader.guest_write::<u32>(sp,      CALLBACK_RET_TRAP).expect("WM_DESTROY stack OOB");
            loader.guest_write::<u32>(sp +  4, hwnd).expect("WM_DESTROY stack OOB");
            loader.guest_write::<u32>(sp +  8, WM_DESTROY).expect("WM_DESTROY stack OOB");
            loader.guest_write::<u32>(sp + 12, 0).expect("WM_DESTROY stack OOB");
            loader.guest_write::<u32>(sp + 16, 0).expect("WM_DESTROY stack OOB");
            regs.rip = wnd_proc as u64;
        }
        None => {
            regs.rip = saved_rip;
            regs.rsp = saved_rsp;
            regs.rax = chain.result as u64;
        }
    }
    vcpu.set_regs(&regs).unwrap();
}

/// Go on with an owner-draw chain (`pm_owner.rs`): send the next owner
/// call of the chain's queue under a `FrameKind::OwnerDraw` frame, its
/// structure and arguments just below `saved_rsp`; calls without a guest
/// procedure are finished at once. When none is left the chain ends with
/// its `then`: the API result back to `saved_rip` / `saved_rsp`, or the
/// modal dialog loop again.
fn continue_owner_draw(
    vcpu: &mut dyn super::vm_backend::VcpuBackend,
    loader: &super::Loader,
    callback_stack: &mut Vec<CallbackFrame>,
    chain: Box<super::pm_owner::OwnerChain>,
    saved_rip: u64,
    saved_rsp: u64,
) {
    use super::pm_owner::{OwnerCallKind, OwnerSetup, OwnerThen, MAX_OWNER_BLOCK_BYTES, OWNER_BLOCK_BYTES};
    loop {
        let next = loader.shared.window_mgr.lock_or_recover().next_owner_call(chain.hmq);
        let Some(call) = next else {
            loader.shared.window_mgr.lock_or_recover().end_owner_chain(chain.hmq);
            match chain.then {
                OwnerThen::Return(result) => {
                    let mut regs = vcpu.get_regs().unwrap();
                    regs.rip = saved_rip;
                    regs.rsp = saved_rsp;
                    regs.rax = result as u64;
                    vcpu.set_regs(&regs).unwrap();
                }
                OwnerThen::DlgLoop { dlg_proc, hwnd_dlg, hmq } => {
                    setup_dlg_dispatch(vcpu, loader, callback_stack, dlg_proc, hwnd_dlg, hmq,
                                       saved_rip, saved_rsp);
                }
            }
            return;
        };
        match loader.owner_call_setup(&call) {
            OwnerSetup::Skip => loader.owner_call_finish(&call, 0, 0, &[]),
            OwnerSetup::Call { wnd_proc, owner, msg, mp1, mp2, block, hps } => {
                // The structure lies below the slot under saved_rsp, the
                // call's return address and arguments below it.
                // Room for the structure (more than the usual block for a
                // WM_FORMATFRAME's SWP array), 16-byte aligned.
                let room = (block.len() as u32 * 4).next_multiple_of(16).clamp(OWNER_BLOCK_BYTES, MAX_OWNER_BLOCK_BYTES);
                let block_at = (saved_rsp - 4) as u32 - room;
                let block_len = block.len().min((room / 4) as usize) as u32;
                for (i, w) in block.iter().take(block_len as usize).enumerate() {
                    loader.guest_write::<u32>(block_at + 4 * i as u32, *w).expect("owner-draw stack OOB");
                }
                let mp2 = if block_len > 0 { block_at } else { mp2 };
                // WM_FORMATFRAME: mp2 → the client RECTL at the block's
                // start, mp1 → the SWP array after it.
                let mp1 = match call.kind {
                    OwnerCallKind::FrameFormat { count: Some(_) } => block_at + super::pm_frame::FORMAT_RECTL_BYTES,
                    _ => mp1,
                };
                // A container's OWNERITEM: hItem → the CNRDRAWITEMINFO
                // after it (block words 12 and 13).
                if let OwnerCallKind::ContainerDrawItem { .. } = call.kind
                    && block_len >= 14 {
                    loader.guest_write::<u32>(block_at + 44, block_at + 48).expect("owner-draw stack OOB");
                }
                callback_stack.push(CallbackFrame {
                    saved_rip,
                    saved_rsp,
                    kind: FrameKind::OwnerDraw { chain, call, hps, block_at, block_len },
                });
                let mut regs = vcpu.get_regs().unwrap();
                regs.rsp = (block_at - 20) as u64;
                let sp = regs.rsp as u32;
                for (o, v) in [(0, CALLBACK_RET_TRAP), (4, owner), (8, msg), (12, mp1), (16, mp2)] {
                    loader.guest_write::<u32>(sp + o, v).expect("owner-draw stack OOB");
                }
                regs.rip = wnd_proc as u64;
                vcpu.set_regs(&regs).unwrap();
                return;
            }
        }
    }
}

/// Go on with a chain of guest calls (`pm_hook.rs`): make its next call
/// under a `FrameKind::CallChain` frame — a structure the call passes just
/// below the API caller's stack (`saved_rsp`), the return address and the
/// arguments below it —, handle messages for built-in windows on the spot,
/// and once no call is left finish the chain: return its result to
/// `saved_rip` / `saved_rsp`, or run the PMWIN API again (its arguments are
/// still on the stack above the return address at `saved_rsp - 4`).
fn continue_call_chain(
    vcpu: &mut dyn super::vm_backend::VcpuBackend,
    loader: &super::Loader,
    callback_stack: &mut Vec<CallbackFrame>,
    mut chain: Box<super::pm_hook::CallChain>,
    saved_rip: u64,
    saved_rsp: u64,
) {
    use super::pm_hook::{CallStep, ChainEnd, CHAIN_BLOCK_BYTES, MAX_CHAIN_ARGS, MAX_CHAIN_NESTING};
    while let Some(step) = chain.steps.pop_front() {
        match step {
            CallStep::Builtin { hwnd, msg, mp1, mp2 } => {
                match loader.dispatch_builtin_control(hwnd, msg, mp1, mp2) {
                    ApiResult::Normal(v) => chain.returned(v, false),
                    other => debug!("call chain: built-in result {:?} of msg 0x{:X} for {} dropped", other, msg, hwnd),
                }
            }
            CallStep::Guest { proc_addr, mut args, block, block_arg, stop_on_true, window } => {
                if window != 0 && !loader.shared.window_mgr.lock_or_recover().is_window(window) {
                    continue; // destroyed by an earlier call
                }
                args.truncate(MAX_CHAIN_ARGS);
                // The call goes below the API caller's stack, which the
                // guest chose: a call that nests too deep, or whose frame
                // cannot be written there, is skipped as if it returned 0
                // rather than ending the process (security audit
                // 2026-11-25; somDispatch can dispatch itself, each level
                // a host frame).
                let frame = (saved_rsp as u32).checked_sub(4 + CHAIN_BLOCK_BYTES).and_then(|block_at| {
                    let sp = block_at.checked_sub(4 * (args.len() as u32 + 1))?;
                    loader.guest_slice_mut(sp, (block_at + CHAIN_BLOCK_BYTES - sp) as usize).map(|_| (block_at, sp))
                });
                let Some((block_at, sp)) = frame.filter(|_| callback_stack.len() < MAX_CHAIN_NESTING) else {
                    warn!("call chain: a call to {proc_addr:#x} skipped (nested {} deep, or no room for its frame below {saved_rsp:#x})",
                          callback_stack.len());
                    chain.returned(0, false);
                    continue;
                };
                let words = block.len().min((CHAIN_BLOCK_BYTES / 4) as usize);
                for (i, w) in block.iter().take(words).enumerate() {
                    let _ = loader.guest_write::<u32>(block_at + 4 * i as u32, *w);
                }
                if words > 0 && block_arg < args.len() {
                    args[block_arg] = block_at;
                }
                callback_stack.push(CallbackFrame {
                    saved_rip,
                    saved_rsp,
                    kind: FrameKind::CallChain { chain, stops: stop_on_true },
                });
                let mut regs = vcpu.get_regs().unwrap();
                let _ = loader.guest_write::<u32>(sp, CALLBACK_RET_TRAP);
                for (i, a) in args.iter().enumerate() {
                    let _ = loader.guest_write::<u32>(sp + 4 * (i as u32 + 1), *a);
                }
                regs.rsp = sp as u64;
                regs.rip = proc_addr as u64;
                vcpu.set_regs(&regs).unwrap();
                return;
            }
        }
    }
    let mut regs = vcpu.get_regs().unwrap();
    match loader.finish_chain(&chain) {
        ChainEnd::Return(v) => {
            regs.rip = saved_rip;
            regs.rsp = saved_rsp;
            regs.rax = v as u64;
        }
        ChainEnd::Rerun(ordinal) => {
            // Back at the API thunk with the return address on top of the
            // stack: the call is made again.
            regs.rip = MAGIC_API_BASE + (PMWIN_BASE + ordinal) as u64;
            regs.rsp = saved_rsp - 4;
        }
        ChainEnd::Exit(result) => {
            // Into DosExit(EXIT_PROCESS, result), as if the program had
            // called it from where the chain began.
            let sp = saved_rsp as u32 - 12;
            for (i, v) in [saved_rip as u32, super::threads::EXIT_PROCESS, result].into_iter().enumerate() {
                loader.guest_write::<u32>(sp + 4 * i as u32, v).expect("exit stack OOB");
            }
            regs.rip = MAGIC_API_BASE + super::pm_hook::ORD_DOS_EXIT as u64;
            regs.rsp = sp as u64;
        }
        ChainEnd::Next(next) => {
            // One more round of calls (a container sort's next comparison).
            continue_call_chain(vcpu, loader, callback_stack, next, saved_rip, saved_rsp);
            return;
        }
        ChainEnd::ModalLoop { dlg_proc, hwnd_dlg, hmq, msg } => {
            // The message-filter hooks of a modal loop returned: dispatch
            // what they let through, then on with the loop.
            if let Some(msg) = msg
                && dispatch_dlg_message(vcpu, loader, callback_stack, dlg_proc, hwnd_dlg, hmq, &msg,
                                        saved_rip, saved_rsp)
            {
                return;
            }
            setup_dlg_dispatch(vcpu, loader, callback_stack, dlg_proc, hwnd_dlg, hmq, saved_rip, saved_rsp);
            return;
        }
    }
    vcpu.set_regs(&regs).unwrap();
}

#[cfg(test)]
mod entry_tests {
    use super::*;

    #[test]
    fn exe_entry_stack_is_return_hmod_reserved_env_cmd() {
        let st = exe_entry_stack(0x2000_0000, 0x2000_0123);
        assert_eq!(st, [EXIT_TRAP_ADDR, EXE_MODULE_HANDLE, 0, 0x2000_0000, 0x2000_0123]);
        // Open Watcom's __OS2Main(hmod, reserved, env, cmd) after `jmp`:
        // env at [esp+12], cmd at [esp+16].
        assert_eq!(st[3], 0x2000_0000);
        assert_eq!(st[4], 0x2000_0123);
    }

    #[test]
    fn jmp_targets_are_relative_to_the_next_instruction() {
        assert_eq!(jmp_target(0x10000, &[0xE9, 0x10, 0, 0, 0]), Some(0x10015));
        assert_eq!(jmp_target(0x10000, &[0xE9, 0xFB, 0xFF, 0xFF, 0xFF]), Some(0x10000));
        assert_eq!(jmp_target(0x10000, &[0xEB, 0x10, 0, 0, 0]), None, "short jmp is not it");
        assert_eq!(jmp_target(0x10000, &[0xE9, 0x10]), None, "too short");
        assert_eq!(jmp_target(0xFFFF_FFF0, &[0xE9, 0x20, 0, 0, 0]), Some(0x15), "wraps, no overflow");
    }

    /// `__OS2Main` of the Open Watcom snapshot of March 2026 (`wdis` of
    /// clib3r.lib main2o32): arguments read one slot low.
    const SHIFTED: &[u8] = &[
        0x55, 0x89, 0xE5, 0x83, 0xEC, 0x08, 0xB8, 0x01, 0, 0, 0, 0xE8, 0, 0, 0, 0,
        0xA1, 0, 0, 0, 0, 0x83, 0xC0, 0x03, 0x24, 0xFC, 0x31, 0xD2, 0x2B, 0xE0, 0x89, 0xE6,
        0x8B, 0x1D, 0, 0, 0, 0, 0x89, 0xF0, 0xE8, 0, 0, 0, 0, 0xA1, 0, 0, 0, 0,
        0x89, 0x86, 0xFC, 0, 0, 0, 0x89, 0xF0, 0x8B, 0x4D, 0x0C, 0xE8, 0, 0, 0, 0,
        0x8B, 0x45, 0x10, 0x8B, 0x5D, 0x04, 0x50, 0x89, 0xF2, 0x8D, 0x45, 0xF8, 0xE8, 0, 0, 0, 0,
    ];
    /// The same after the fix (November 2026 snapshot): `[ebp+0x18]` …
    const FIXED: &[u8] = &[
        0x53, 0x51, 0x52, 0x56, 0x55, 0x89, 0xE5, 0x83, 0xEC, 0x08, 0xB8, 0x01, 0, 0, 0,
        0x8B, 0x4D, 0x20, 0xE8, 0, 0, 0, 0, 0x8B, 0x45, 0x24, 0x8B, 0x5D, 0x18, 0x50,
    ];

    #[test]
    fn the_broken_open_watcom_start_up_is_recognised() {
        assert!(is_shifted_ow_main(SHIFTED));
        assert!(!is_shifted_ow_main(FIXED));
        assert!(!is_shifted_ow_main(&SHIFTED[..20]), "argument loads not seen");
        assert!(!is_shifted_ow_main(&[]));
    }

    #[test]
    fn a_loader_checks_the_entry_jump() {
        let loader = super::super::Loader::new_mock();
        const ENTRY: u32 = 0x0001_0000;
        const MAIN: u32 = 0x0001_0100;
        let rel = (MAIN - (ENTRY + ENTRY_JMP_LEN as u32)).to_le_bytes();
        loader.guest_write_bytes(ENTRY, &[0xE9, rel[0], rel[1], rel[2], rel[3]]).unwrap();
        loader.guest_write_bytes(MAIN, SHIFTED).unwrap();
        assert!(loader.entry_misreads_stack(ENTRY));
        loader.guest_write_bytes(MAIN, FIXED).unwrap();
        assert!(!loader.entry_misreads_stack(ENTRY));
        assert!(!loader.entry_misreads_stack(MAIN), "no jmp at the entry point");
        assert!(!loader.entry_misreads_stack(u32::MAX - 2), "out of guest memory");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0110: exception and unwind handler frames a trap's stack lies
    /// above were left by their handler (a longjmp out); others stay.
    #[test]
    fn abandoned_handler_frames_are_dropped_on_a_later_trap() {
        let loader = super::super::Loader::new_mock();
        let xcpt = |saved_rsp: u64| CallbackFrame {
            saved_rip: 0, saved_rsp,
            kind: FrameKind::ExceptionHandler {
                saved: Box::default(), next_handler: XCPT_CHAIN_END, exc_report: 0, ctx_record: 0,
                guest_allocs: Vec::new(),
            },
        };
        let mut stack = vec![CallbackFrame { saved_rip: 0, saved_rsp: 0x9000, kind: FrameKind::PmCallback }, xcpt(0x4000)];
        drop_abandoned_handler_frames(&loader, &mut stack, 0x3FF0);
        assert_eq!(stack.len(), 2, "a trap below it: the handler is returning");
        drop_abandoned_handler_frames(&loader, &mut stack, 0x8FF0);
        assert_eq!(stack.len(), 1, "a trap above it: the handler was left");
        assert!(matches!(stack[0].kind, FrameKind::PmCallback), "other frames stay");
        drop_abandoned_handler_frames(&loader, &mut stack, 0xA000);
        assert_eq!(stack.len(), 1, "never other kinds");
    }
    use super::super::mutex_ext::MutexExt;
    use super::super::pm_types::OS2Message;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::vm_backend::VcpuBackend;

    /// A thread runs paged on its own block (paging.rs), with CR0.WP and
    /// its TSS (ADR-0113).
    #[test]
    fn a_thread_runs_paged_on_its_own_block() {
        use super::super::paging::{CR0_PG, CR4_PSE};
        let loader = super::super::Loader::new_mock();
        let block = loader.alloc_paging_block(TIB_BASE).expect("a block");
        let mut vcpu = MockVcpu::new();
        loader.setup_vcpu_segments_32bit(&mut vcpu, TIB_BASE as u64, block);
        let s = vcpu.get_sregs().unwrap();
        assert_eq!(s.cr3, block as u64);
        assert_ne!(s.cr0 & CR0_PG, 0);
        assert_ne!(s.cr4 & CR4_PSE, 0);
        assert_ne!(s.cr0 & super::super::paging::CR0_WP, 0, "write protect: the thunks are read-only at ring 0 too");
        assert_eq!(s.cr0 & 1, 1, "protected mode");
        assert_eq!((s.tr.base, s.tr.limit, s.tr.selector), (u64::from(RING0_WINDOW), TSS_SIZE - 1, TSS_SEL),
                   "TR: the thread's TSS in its ring-0 window");
        assert_eq!((s.tr.type_, s.tr.present, s.tr.s), (11, 1, 0), "a present, busy 32-bit TSS");
    }

    /// The GDT's ring-0 descriptors and the TSS descriptor, and the IDT's
    /// gates: only the breakpoint gate is DPL 3, so API thunks work from
    /// ring 3 while a guest's other `INT n` faults (ADR-0113).
    #[test]
    fn the_gdt_has_ring0_descriptors_and_only_the_breakpoint_gate_is_dpl3() {
        let loader = super::super::Loader::new_mock();
        loader.setup_idt();
        let desc = |sel: u16| loader.guest_read::<u64>(GDT_BASE + u32::from(sel)).unwrap();
        let access = |sel: u16| (desc(sel) >> 40) as u8;
        assert_eq!(access(KERNEL_CODE_SEL), 0x9B, "ring-0 code: present, DPL 0, execute/read");
        assert_eq!(access(KERNEL_DATA_SEL), 0x93, "ring-0 data: present, DPL 0, read/write");
        assert_eq!(access(TSS_SEL), 0x8B, "TSS: present, DPL 0, busy 32-bit");
        assert_eq!(loader.gdt_entry_base(TSS_SEL), RING0_WINDOW);
        assert_eq!(loader.gdt_entry_limit(TSS_SEL), TSS_SIZE - 1);
        assert_eq!(loader.gdt_entry_base(KERNEL_CODE_SEL), 0);
        for (sel, want) in [(0x08u16, 0xFBu8), (0x10, 0xF3), (0x18, 0xF3)] {
            assert_eq!(access(sel), want, "selector {sel:#x}: DPL 3, for ring 3");
        }
        for v in 0..32u32 {
            let attrs = loader.guest_read::<u16>(IDT_BASE + v * 8 + 4).unwrap();
            let dpl = (attrs >> 13) & 3;
            assert_eq!(dpl, if v == 3 { 3 } else { 0 }, "gate {v}'s DPL");
            assert_eq!(attrs & 0x9F00, 0x8E00, "gate {v}: a present 32-bit interrupt gate");
        }
    }

    /// At ring 3 (ADR-0113) the descriptors programs use are DPL 3, the
    /// gates enter ring 0 through ring 0's code descriptor, and a paged
    /// thread starts with RPL-3 selectors.
    #[test]
    fn at_ring_3_program_descriptors_are_dpl3_and_gates_enter_ring_0() {
        let loader = super::super::Loader::new_mock();
        loader.setup_idt();
        let access = |sel: u32| (loader.guest_read::<u64>(GDT_BASE + sel).unwrap() >> 40) as u8;
        for (sel, want) in [(0x08, 0xFB), (0x10, 0xF3), (0x18, 0xF3), (0x20, 0xF3), (0x28, 0xFB),
                            (TILED_SEL_START, 0xF3), (TILED_CODE_START, 0xFB)] {
            assert_eq!(access(sel), want, "selector {sel:#x}: DPL 3");
        }
        assert_eq!(access(u32::from(KERNEL_CODE_SEL)), 0x9B, "ring 0's code stays DPL 0");
        for v in 0..32u32 {
            let cs = loader.guest_read::<u16>(IDT_BASE + v * 8 + 2).unwrap();
            assert_eq!(cs, KERNEL_CODE_SEL, "gate {v} enters ring 0");
        }
        let block = loader.alloc_paging_block(TIB_BASE).expect("a block");
        let mut vcpu = MockVcpu::new();
        loader.setup_vcpu_segments_32bit(&mut vcpu, TIB_BASE as u64, block);
        let s = vcpu.get_sregs().unwrap();
        assert_eq!((s.cs.selector, s.cs.dpl), (0x0B, 3));
        for seg in [&s.ds, &s.es, &s.gs, &s.ss] {
            assert_eq!((seg.selector, seg.dpl), (0x13, 3));
        }
        assert_eq!((s.fs.selector, s.fs.dpl), (0x1B, 3));
    }

    /// A chain call is skipped, as if it returned 0, when its frame has no
    /// room below the guest's stack or calls nest too deep (security audit
    /// 2026-11-25): the process goes on instead of the host panicking.
    #[test]
    fn a_chain_call_without_room_or_nested_too_deep_is_skipped() {
        use super::super::constants::DYNAMIC_ALLOC_BASE;
        use super::super::pm_hook::{CallChain, CallStep, ChainThen, MAX_CHAIN_NESTING};
        let loader = super::super::Loader::new_mock();
        let chain = || Box::new(CallChain::new(0, vec![CallStep::hook(0x1_0000, vec![1, 2], false)], false, ChainThen::ReturnLast));
        let mut vcpu = MockVcpu::new();
        let mut stack = Vec::new();
        // Just above the paging arena (protected), and near address 0.
        for saved_rsp in [u64::from(DYNAMIC_ALLOC_BASE) + 8, 8] {
            continue_call_chain(&mut vcpu, &loader, &mut stack, chain(), 0x4242, saved_rsp);
            assert!(stack.is_empty(), "{saved_rsp:#x}: no frame pushed");
            let r = vcpu.get_regs().unwrap();
            assert_eq!((r.rip, r.rsp, r.rax), (0x4242, saved_rsp, 0), "{saved_rsp:#x}: back to the caller");
        }
        let ok_rsp = 0x8_0000u64;
        for _ in 0..MAX_CHAIN_NESTING {
            stack.push(CallbackFrame { saved_rip: 0, saved_rsp: 0, kind: FrameKind::CallChain {
                chain: Box::new(CallChain::new(0, Vec::new(), false, ChainThen::ReturnLast)), stops: false } });
        }
        continue_call_chain(&mut vcpu, &loader, &mut stack, chain(), 0x4242, ok_rsp);
        assert_eq!(stack.len(), MAX_CHAIN_NESTING, "too deep: skipped");
        assert_eq!(vcpu.get_regs().unwrap().rip, 0x4242);
        stack.clear();
        continue_call_chain(&mut vcpu, &loader, &mut stack, chain(), 0x4242, ok_rsp);
        assert_eq!(stack.len(), 1, "with room: the call is made");
        assert_eq!(vcpu.get_regs().unwrap().rip, 0x1_0000);
    }

    /// Every message the modal loop dispatches is called at the same stack
    /// position: a procedure returning (which leaves rsp 16 bytes below its
    /// frame) does not make the next call sink lower.
    #[test]
    fn modal_loop_calls_do_not_sink_the_stack() {
        let loader = super::super::Loader::new_mock();
        let (hmq, dlg) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            wm.register_class("Dlg".into(), 0x1234, 0);
            let dlg = wm.create_window("Dlg".into(), 0, hmq);
            let mq = wm.get_mq(hmq).unwrap();
            for msg in [0x1000, 0x1001, 0x1002] {
                mq.lock_or_recover().messages.push_back(
                    OS2Message { hwnd: dlg, msg, mp1: 0, mp2: 0, time: 0, x: 0, y: 0 });
            }
            (hmq, dlg)
        };
        let mut vcpu = MockVcpu::new();
        let saved_rsp: u64 = 0x8_0000;
        vcpu.regs.rsp = saved_rsp - 4; // at the API's return address
        let mut stack = Vec::new();
        let mut sps = Vec::new();
        for _ in 0..3 {
            assert!(setup_dlg_dispatch(&mut vcpu, &loader, &mut stack, 0x1234, dlg, hmq, 0x4000, saved_rsp));
            let sp = vcpu.get_regs().unwrap().rsp;
            sps.push(sp);
            // The procedure returns: `ret` pops the return address.
            vcpu.regs.rsp = sp + 4;
            stack.pop();
        }
        assert_eq!(sps, vec![saved_rsp - 4 - DLG_CALL_FRAME_BYTES; 3]);
    }

    /// A signal for thread 1 waiting in a modal loop leaves the loop for
    /// the vCPU loop (ADR-0093): the guest at the callback trap under a
    /// loop frame, where the signal is dispatched to its handlers; a held
    /// signal, or one for a dialog of another thread, does not.
    #[test]
    fn a_signal_leaves_the_modal_loop_for_its_handlers() {
        use super::super::constants::{SIG_SETFOCUS, XCPT_SIGNAL_INTR};
        use super::super::signals::SignalAction;
        let loader = super::super::Loader::new_mock();
        let (hmq, dlg) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(MAIN_VCPU_ID, hmq);
            wm.register_class("Dlg".into(), 0x1234, 0);
            (hmq, wm.create_window("Dlg".into(), 0, hmq))
        };
        let mut vcpu = MockVcpu::new();
        vcpu.sregs.cs.db = 1;
        vcpu.sregs.ss.db = 1;
        let saved_rsp: u64 = 0x8_0000;
        vcpu.regs.rsp = saved_rsp - 4;
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        loader.post_signal(XCPT_SIGNAL_INTR);
        let mut stack = Vec::new();
        assert!(setup_dlg_dispatch(&mut vcpu, &loader, &mut stack, 0x1234, dlg, hmq, 0x4000, saved_rsp));
        let regs = vcpu.get_regs().unwrap();
        assert_eq!((regs.rip, regs.rsp), (CALLBACK_RET_TRAP as u64, saved_rsp - 4 - DLG_CALL_FRAME_BYTES));
        assert!(matches!(stack.last(), Some(CallbackFrame { kind: FrameKind::DlgRunLoop { .. }, saved_rip: 0x4000, .. })));
        // The vCPU loop then gives the signal to the handlers (none here:
        // the default action, the end of the process).
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID), SignalAction::EndProcess));
        // Now held: the loop stays (it waits for a message, then for the
        // dialog to end; here it is dismissed).
        loader.post_signal(XCPT_SIGNAL_INTR);
        assert!(!loader.signal_waits_for(MAIN_VCPU_ID), "held until acknowledged");
        assert!(!loader.signal_waits_for(2), "not thread 1");
    }

    /// An owner-draw chain calls the owner's procedure with the structure
    /// on the guest stack below the caller's, one call after the other, and
    /// then returns the API's result to the caller.
    #[test]
    fn owner_draw_calls_run_below_the_callers_stack_then_return_the_result() {
        use super::super::pm_owner::{OwnerCall, OwnerCallKind, OwnerChain, OwnerThen, OWNER_BLOCK_BYTES};
        let loader = super::super::Loader::new_mock();
        let (hmq, client, buttons) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            wm.register_class("App".into(), 0x5000, 0);
            let client = wm.create_window("App".into(), 0, hmq);
            let mut buttons = Vec::new();
            for id in [1, 2] {
                let b = wm.create_window("WC_BUTTON".into(), client, hmq);
                let w = wm.get_window_mut(b).unwrap();
                (w.id, w.owner, w.style, w.cx, w.cy) = (id, client, BS_USERBUTTON, 40, 20);
                wm.queue_owner_call(OwnerCall { control: b, kind: OwnerCallKind::UserButton });
                buttons.push(b);
            }
            assert!(wm.begin_owner_chain(hmq));
            (hmq, client, buttons)
        };
        let mut vcpu = MockVcpu::new();
        let saved_rsp: u64 = 0x8_0000;
        let mut stack = Vec::new();
        let chain = Box::new(OwnerChain { hmq, then: OwnerThen::Return(77) });
        continue_owner_draw(&mut vcpu, &loader, &mut stack, chain, 0x4000, saved_rsp);
        for &button in &buttons {
            let regs = vcpu.get_regs().unwrap();
            assert_eq!(regs.rip, 0x5000, "the owner's window procedure");
            let sp = regs.rsp as u32;
            let block = saved_rsp as u32 - 4 - OWNER_BLOCK_BYTES;
            assert_eq!(sp, block - 20);
            let arg = |o: u32| loader.guest_read::<u32>(sp + o).unwrap();
            assert_eq!((arg(0), arg(4), arg(8)), (CALLBACK_RET_TRAP, client, WM_CONTROL));
            assert_eq!(arg(12) >> 16, BN_PAINT);
            assert_eq!(arg(16), block, "mp2 = the USERBUTTON");
            assert_eq!(loader.guest_read::<u32>(block).unwrap(), button);
            // The procedure returns.
            let Some(CallbackFrame { kind: FrameKind::OwnerDraw { chain, call, hps, .. }, saved_rip, saved_rsp }) = stack.pop()
            else { panic!("an owner-draw frame") };
            loader.owner_call_finish(&call, hps, 0, &[]);
            continue_owner_draw(&mut vcpu, &loader, &mut stack, chain, saved_rip, saved_rsp);
        }
        let regs = vcpu.get_regs().unwrap();
        assert_eq!((regs.rip, regs.rsp, regs.rax), (0x4000, saved_rsp, 77));
        assert!(stack.is_empty());
        assert!(!loader.shared.window_mgr.lock_or_recover().owner_drawing.contains(&hmq), "the chain ended");
    }

    /// A call chain makes its calls just below the caller's stack, the
    /// structure a call passes above its arguments; a TRUE result of a
    /// stopping call skips the rest, and WinSendMsg after its send hooks is
    /// made again from the API thunk with the caller's return address on
    /// top of the stack.
    #[test]
    fn call_chain_calls_hooks_below_the_callers_stack_then_reruns_or_returns() {
        use super::super::pm_hook::{CallChain, CallStep, ChainThen, CHAIN_BLOCK_BYTES, ORD_WIN_SEND_MSG};
        let loader = super::super::Loader::new_mock();
        let saved_rsp: u64 = 0x8_0000;
        let mut vcpu = MockVcpu::new();
        let mut stack = Vec::new();
        let steps = vec![
            CallStep::hook_with_block(0x7000, vec![1, 0, 3], vec![10, 11, 12], 1, false),
            CallStep::hook(0x7100, vec![4], true),
            CallStep::hook(0x7200, vec![5], true),
        ];
        let chain = Box::new(CallChain::new(0, steps, true, ChainThen::SendAgain));
        continue_call_chain(&mut vcpu, &loader, &mut stack, chain, 0x4000, saved_rsp);
        let block = saved_rsp as u32 - 4 - CHAIN_BLOCK_BYTES;
        let regs = vcpu.get_regs().unwrap();
        assert_eq!((regs.rip, regs.rsp as u32), (0x7000, block - 16));
        let rd = |a: u32| loader.guest_read::<u32>(a).unwrap();
        assert_eq!([rd(block - 16), rd(block - 12), rd(block - 8), rd(block - 4)],
                   [CALLBACK_RET_TRAP, 1, block, 3], "the block's address replaces argument 1");
        assert_eq!([rd(block), rd(block + 4), rd(block + 8)], [10, 11, 12]);
        // Each call returns; the second one returns TRUE.
        for result in [0, 1] {
            let Some(CallbackFrame { kind: FrameKind::CallChain { mut chain, stops }, saved_rip, saved_rsp }) = stack.pop()
            else { panic!("a call-chain frame") };
            chain.returned(result, stops);
            continue_call_chain(&mut vcpu, &loader, &mut stack, chain, saved_rip, saved_rsp);
        }
        assert!(stack.is_empty(), "the third hook is not called");
        let regs = vcpu.get_regs().unwrap();
        assert_eq!((regs.rip, regs.rsp), (MAGIC_API_BASE + (PMWIN_BASE + ORD_WIN_SEND_MSG) as u64, saved_rsp - 4));
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        assert!(!wm.hooks.in_hook(0) && wm.hooks.take_send_hooks_done(0));
        drop(wm);
        // A chain that returns a value.
        let chain = Box::new(CallChain::new(0, vec![], false, ChainThen::Return(1)));
        continue_call_chain(&mut vcpu, &loader, &mut stack, chain, 0x4000, saved_rsp);
        let regs = vcpu.get_regs().unwrap();
        assert_eq!((regs.rip, regs.rsp, regs.rax), (0x4000, saved_rsp, 1));
    }

    /// The modal loop calls the message-filter hooks with each message in
    /// a QMSG below the caller's stack (MSGF_DIALOGBOX, MSGF_MESSAGEBOX for
    /// a message box); TRUE swallows it, FALSE dispatches what the hook left
    /// in the QMSG.
    #[test]
    fn modal_loop_calls_message_filter_hooks() {
        use super::super::pm_hook::{CHAIN_BLOCK_BYTES, HK_MSGFILTER, HMQ_CURRENT, MSGF_DIALOGBOX, MSGF_MESSAGEBOX};
        let loader = super::super::Loader::new_mock();
        let (hmq, dlg) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(0, hmq);
            wm.register_class("Dlg".into(), 0x1234, 0);
            let dlg = wm.create_window("Dlg".into(), 0, hmq);
            let mq = wm.get_mq(hmq).unwrap();
            for msg in [0x1000, 0x1001] {
                mq.lock_or_recover().messages.push_back(
                    OS2Message { hwnd: dlg, msg, mp1: 7, mp2: 8, time: 0, x: 0, y: 0 });
            }
            (hmq, dlg)
        };
        assert!(matches!(loader.win_set_hook(0, HMQ_CURRENT, HK_MSGFILTER, 0x7000), ApiResult::Normal(1)));
        let mut vcpu = MockVcpu::new();
        let saved_rsp: u64 = 0x8_0000;
        let qmsg = saved_rsp as u32 - 4 - CHAIN_BLOCK_BYTES;
        let rd = |a: u32| loader.guest_read::<u32>(a).unwrap();
        let mut stack = Vec::new();
        let filter_return = |vcpu: &mut MockVcpu, stack: &mut Vec<CallbackFrame>, result: u32| {
            let Some(CallbackFrame { kind: FrameKind::CallChain { mut chain, stops }, saved_rip, saved_rsp }) = stack.pop()
            else { panic!("a call-chain frame") };
            chain.returned(result, stops);
            continue_call_chain(vcpu, &loader, stack, chain, saved_rip, saved_rsp);
        };
        // The first message: the hook sees it and swallows it.
        assert!(setup_dlg_dispatch(&mut vcpu, &loader, &mut stack, 0x1234, dlg, hmq, 0x4000, saved_rsp));
        let sp = vcpu.get_regs().unwrap().rsp as u32;
        assert_eq!(vcpu.get_regs().unwrap().rip, 0x7000);
        assert_eq!([rd(sp + 4), rd(sp + 8), rd(sp + 12)], [MOCK_HAB, qmsg, MSGF_DIALOGBOX]);
        assert_eq!([rd(qmsg), rd(qmsg + 4), rd(qmsg + 8)], [dlg, 0x1000, 7]);
        filter_return(&mut vcpu, &mut stack, 1);
        // The loop goes on with the second message, which the hook changes
        // and lets through: the dialog procedure gets the changed message.
        assert_eq!(vcpu.get_regs().unwrap().rip, 0x7000);
        assert_eq!(rd(qmsg + 4), 0x1001);
        loader.guest_write::<u32>(qmsg + 8, 99).unwrap();
        filter_return(&mut vcpu, &mut stack, 0);
        let regs = vcpu.get_regs().unwrap();
        assert_eq!(regs.rip, 0x1234, "the dialog procedure");
        let sp = regs.rsp as u32;
        assert_eq!([rd(sp + 4), rd(sp + 8), rd(sp + 12)], [dlg, 0x1001, 99]);
        assert!(matches!(stack.pop(), Some(CallbackFrame { kind: FrameKind::DlgRunLoop { .. }, .. })));
        assert!(!loader.shared.window_mgr.lock_or_recover().hooks.in_hook(0));
        // A message box passes MSGF_MESSAGEBOX.
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.get_window_mut(dlg).unwrap().message_box = true;
            wm.get_mq(hmq).unwrap().lock_or_recover().messages.push_back(
                OS2Message { hwnd: dlg, msg: 0x1002, mp1: 0, mp2: 0, time: 0, x: 0, y: 0 });
        }
        assert!(setup_dlg_dispatch(&mut vcpu, &loader, &mut stack, 0x1234, dlg, hmq, 0x4000, saved_rsp));
        let sp = vcpu.get_regs().unwrap().rsp as u32;
        assert_eq!(rd(sp + 12), MSGF_MESSAGEBOX);
    }

    // ── Thread ends ──────────────────────────────────────────────────────

    use super::super::threads::{ThreadMemory, WaitOutcome, FIRST_CREATED_TID};

    const STACK_TOP: u32 = 0x8_0000;

    /// A mock vCPU stopped at `rip` (an INT 3 trap) with `rax`, the stack at
    /// `STACK_TOP`.
    fn vcpu_at(rip: u32, rax: u32) -> Box<MockVcpu> {
        let mut vcpu = MockVcpu::new();
        vcpu.regs.rip = rip as u64;
        vcpu.regs.rax = rax as u64;
        vcpu.regs.rsp = STACK_TOP as u64;
        vcpu.push_exit(VmExit::Debug);
        Box::new(vcpu)
    }

    /// A mock vCPU calling `DosExit(action, result)`.
    fn vcpu_calling_dos_exit(loader: &super::super::Loader, action: u32, result: u32) -> Box<MockVcpu> {
        const DOS_EXIT: u32 = 234;
        for (i, v) in [0x1234u32, action, result].into_iter().enumerate() {
            loader.guest_write::<u32>(STACK_TOP + 4 * i as u32, v).unwrap();
        }
        vcpu_at(MAGIC_API_BASE as u32 + DOS_EXIT, 0)
    }

    fn start_thread(loader: &super::super::Loader, tid: u32) {
        loader.shared.threads.table.lock_or_recover().start(tid, ThreadMemory { stack: 0, tib: 0, paging: 0 });
    }

    #[test]
    fn a_returning_thread_ends_alone() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        loader.run_vcpu(vcpu_at(EXIT_TRAP_ADDR, 7), FIRST_CREATED_TID, 0);
        assert!(!loader.shutting_down(), "the main thread still runs");
        let mut table = loader.shared.threads.table.lock_or_recover();
        assert_eq!(table.wait(MAIN_TID, FIRST_CREATED_TID), WaitOutcome::Ended(FIRST_CREATED_TID));
        assert!(table.take_vcpu().is_ok(), "the vCPU is kept for the next thread");
    }

    #[test]
    fn the_guest_stack_bound_covers_every_host_frame() {
        let largest = u64::from(super::super::pm_owner::MAX_OWNER_BLOCK_BYTES) + 4 + 20;
        assert!(MIN_GUEST_ESP > largest);
        assert!(MIN_GUEST_ESP > u64::from(super::super::pm_hook::CHAIN_BLOCK_BYTES) + 4 + DLG_CALL_FRAME_BYTES);
        assert!(!guest_stack_ok(0) && !guest_stack_ok(MIN_GUEST_ESP - 1) && guest_stack_ok(MIN_GUEST_ESP));
        assert!(guest_stack_ok(u64::from(GUEST_MEM_SIZE) - 16) && !guest_stack_ok(u64::from(GUEST_MEM_SIZE)));
    }

    /// An API call made with a stack pointer too low for the host's frames
    /// ends the process as a fault does; the host does not panic.
    #[test]
    fn an_api_call_with_no_stack_room_ends_the_process() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        let mut vcpu = vcpu_calling_dos_exit(&loader, EXIT_THREAD, 3);
        vcpu.regs.rsp = 0x40;
        loader.run_vcpu(vcpu, FIRST_CREATED_TID, 0);
        assert!(loader.shutting_down(), "the process ends");
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 1, "as an exception ends it");
        loader.wait_for_process_end();
    }

    #[test]
    fn dos_exit_thread_ends_only_the_caller() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        loader.run_vcpu(vcpu_calling_dos_exit(&loader, EXIT_THREAD, 3), FIRST_CREATED_TID, 0);
        assert!(!loader.shutting_down());
        assert!(!loader.shared.threads.table.lock_or_recover().is_running(FIRST_CREATED_TID));
    }

    #[test]
    fn the_last_thread_ending_ends_the_process_with_its_code() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        // Thread 1 ends first with EXIT_THREAD; the process lives on.
        loader.run_vcpu(vcpu_calling_dos_exit(&loader, EXIT_THREAD, 1), MAIN_VCPU_ID, 0);
        assert!(!loader.shutting_down());
        loader.run_vcpu(vcpu_calling_dos_exit(&loader, EXIT_THREAD, 9), FIRST_CREATED_TID, 0);
        assert!(loader.shutting_down());
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 9);
        loader.wait_for_process_end();
    }

    #[test]
    fn dos_exit_process_ends_everything_and_the_first_code_wins() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        loader.run_vcpu(vcpu_calling_dos_exit(&loader, EXIT_PROCESS, 4), FIRST_CREATED_TID, 0);
        assert!(loader.shutting_down());
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 4);
        // Another thread ending afterwards does not change the code.
        loader.run_vcpu(vcpu_calling_dos_exit(&loader, EXIT_THREAD, 8), MAIN_VCPU_ID, 0);
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn the_main_thread_returning_ends_the_process() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        loader.run_vcpu(vcpu_at(EXIT_TRAP_ADDR, 5), MAIN_VCPU_ID, 0);
        assert!(loader.shutting_down());
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_killed_thread_ends_at_its_next_exit() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        assert_eq!(loader.dos_kill_thread(MAIN_VCPU_ID, FIRST_CREATED_TID), NO_ERROR);
        // The mock would halt (ending the process) if the thread ran on.
        loader.run_vcpu(Box::new(MockVcpu::new()), FIRST_CREATED_TID, 0);
        assert!(!loader.shutting_down());
        assert_eq!(loader.dos_kill_thread(MAIN_VCPU_ID, FIRST_CREATED_TID), ERROR_INVALID_THREADID);
        assert_eq!(loader.dos_kill_thread(MAIN_VCPU_ID, MAIN_VCPU_ID + 1), ERROR_INVALID_THREADID,
                   "a thread cannot kill itself");
    }

    #[test]
    fn a_thread_ending_destroys_its_message_queue() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        let hmq = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(FIRST_CREATED_TID, hmq);
            hmq
        };
        loader.run_vcpu(vcpu_at(EXIT_TRAP_ADDR, 0), FIRST_CREATED_TID, 0);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert!(wm.get_mq(hmq).is_none());
        assert_eq!(wm.thread_queue(FIRST_CREATED_TID), None);
    }

    #[test]
    fn dos_wait_thread_reports_ended_threads() {
        let loader = super::super::Loader::new_mock();
        const PTID: u32 = 0x7_0000;
        start_thread(&loader, FIRST_CREATED_TID);
        start_thread(&loader, FIRST_CREATED_TID + 1);
        loader.guest_write::<u32>(PTID, FIRST_CREATED_TID).unwrap();
        assert_eq!(loader.dos_wait_thread(MAIN_VCPU_ID, PTID, DCWW_NOWAIT_OPT), ERROR_THREAD_NOT_TERMINATED);
        assert_eq!(loader.dos_wait_thread(MAIN_VCPU_ID, PTID, 7), ERROR_INVALID_PARAMETER);
        loader.run_vcpu(vcpu_at(EXIT_TRAP_ADDR, 0), FIRST_CREATED_TID, 0);
        assert_eq!(loader.dos_wait_thread(MAIN_VCPU_ID, PTID, DCWW_WAIT_OPT), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PTID), Some(FIRST_CREATED_TID));
        // Reported once.
        assert_eq!(loader.dos_wait_thread(MAIN_VCPU_ID, PTID, DCWW_WAIT_OPT), ERROR_INVALID_THREADID);
        // Any thread: the other one, once it ends.
        let other = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(20));
                loader.run_vcpu(vcpu_at(EXIT_TRAP_ADDR, 0), FIRST_CREATED_TID + 1, 0);
            });
            loader.guest_write::<u32>(PTID, 0).unwrap();
            loader.dos_wait_thread(MAIN_VCPU_ID, PTID, DCWW_WAIT_OPT)
        });
        assert_eq!(other, NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PTID), Some(FIRST_CREATED_TID + 1));
        // Thread 1 and the caller itself cannot be waited for.
        loader.guest_write::<u32>(PTID, MAIN_TID).unwrap();
        assert_eq!(loader.dos_wait_thread(FIRST_CREATED_TID, PTID, DCWW_WAIT_OPT), ERROR_INVALID_THREADID);
    }

    #[test]
    fn dos_create_thread_sets_up_tib_and_frees_it_at_the_end() {
        let loader = super::super::Loader::new_mock();
        const PTID: u32 = 0x7_0000;
        // The mock vCPU halts at once: the thread ends (with the process).
        assert_eq!(loader.dos_create_thread(MAIN_VCPU_ID, PTID, 0x1_0000, 0x55, CREATE_READY, 0x2000), NO_ERROR);
        let tid = loader.guest_read::<u32>(PTID).unwrap();
        assert_eq!(tid, FIRST_CREATED_TID);
        loader.wait_for_process_end();
        // Wait until the thread's end is recorded.
        let mut tries = 0;
        while loader.shared.threads.table.lock_or_recover().is_running(tid) && tries < 200 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            tries += 1;
        }
        let mut table = loader.shared.threads.table.lock_or_recover();
        assert!(!table.is_running(tid));
        assert!(table.take_vcpu().is_ok(), "the vCPU went back to the table");
        assert_eq!(table.next_tid(), Some(FIRST_CREATED_TID + 1));
    }

    #[test]
    fn dos_create_thread_rejects_a_stack_too_small_for_its_frame() {
        let loader = super::super::Loader::new_mock();
        assert_eq!(loader.dos_create_thread(MAIN_VCPU_ID, 0x7_0000, 0x1_0000, 0, STACK_COMMITTED, 4), ERROR_INVALID_PARAMETER);
    }

    const DCWW_WAIT_OPT: u32 = super::super::threads::DCWW_WAIT;
    const DCWW_NOWAIT_OPT: u32 = super::super::threads::DCWW_NOWAIT;
    const EXIT_THREAD: u32 = super::super::threads::EXIT_THREAD;
    const EXIT_PROCESS: u32 = super::super::threads::EXIT_PROCESS;

    // ── Exit lists ───────────────────────────────────────────────────────

    const DOS_EXIT_API: u32 = MAGIC_API_BASE as u32 + 234;
    const ROUTINE_A: u32 = 0x1_0000;
    const ROUTINE_B: u32 = 0x1_0100;
    const ROUTINE_C: u32 = 0x1_0200;

    fn add_routine(loader: &super::super::Loader, order: u32, pfn: u32) {
        let r = loader.dos_exit_list(MAIN_VCPU_ID, EXLST_ADD | order << 8, pfn);
        assert!(matches!(r, ApiResult::Normal(NO_ERROR)), "{r:?}");
    }

    fn exit_list_len(loader: &super::super::Loader) -> usize {
        loader.shared.threads.exit_list.lock_or_recover().len()
    }

    /// The vCPU is at a routine: `pfn(tc)` from the initial stack.
    fn assert_routine(loader: &super::super::Loader, vcpu: &MockVcpu, pfn: u32, tc: u32) {
        assert_eq!(vcpu.regs.rip, pfn as u64);
        assert_eq!(vcpu.regs.rsp, (STACK_TOP - 8) as u64, "the stack starts over");
        assert_eq!(loader.guest_read::<u32>(STACK_TOP - 8), Some(EXIT_TRAP_ADDR));
        assert_eq!(loader.guest_read::<u32>(STACK_TOP - 4), Some(tc));
    }

    #[test]
    fn exit_list_routines_run_in_order_then_the_process_ends() {
        let loader = super::super::Loader::new_mock();
        add_routine(&loader, 0x20, ROUTINE_A);
        add_routine(&loader, 0x10, ROUTINE_B);
        add_routine(&loader, 0x20, ROUTINE_C);
        let mut vcpu = MockVcpu::new();
        vcpu.regs.rsp = STACK_TOP as u64;
        let mut frames = Vec::new();
        let start = loader.begin_exit_list(&mut vcpu, MAIN_VCPU_ID, 7, TC_EXIT, STACK_TOP, &mut frames);
        assert_eq!(start, ExitListStart::Running);
        assert!(!loader.shutting_down(), "the exit-list thread goes on");
        assert!(loader.runs_exit_list(MAIN_VCPU_ID));
        assert!(loader.stopped_by_exit_list(FIRST_CREATED_TID));
        // Other host threads give up their waits.
        assert!(std::thread::scope(|s| s.spawn(|| loader.shutting_down()).join().unwrap()));
        for pfn in [ROUTINE_B, ROUTINE_C, ROUTINE_A] {
            assert_routine(&loader, &vcpu, pfn, TC_EXIT);
            // Another thread cannot end the routine; DosExitList(EXLST_EXIT) on it does.
            assert!(matches!(loader.dos_exit_list(FIRST_CREATED_TID, EXLST_EXIT, 0),
                             ApiResult::Normal(ERROR_INVALID_FUNCTION)));
            assert!(matches!(loader.dos_exit_list(MAIN_VCPU_ID, EXLST_EXIT, 0), ApiResult::ExitListNext));
            assert!(!loader.shutting_down() || pfn == ROUTINE_A);
            let more = loader.next_exit_routine(&mut vcpu);
            assert_eq!(more, pfn != ROUTINE_A);
        }
        assert!(loader.shutting_down());
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 7);
        // It never runs again.
        add_routine(&loader, 0, ROUTINE_A);
        assert_eq!(loader.begin_exit_list(&mut vcpu, MAIN_VCPU_ID, 0, TC_EXIT, STACK_TOP, &mut frames),
                   ExitListStart::NotStarted);
    }

    #[test]
    fn exlst_exit_outside_exit_list_processing_fails() {
        let loader = super::super::Loader::new_mock();
        assert!(matches!(loader.dos_exit_list(MAIN_VCPU_ID, EXLST_EXIT, 0),
                         ApiResult::Normal(ERROR_INVALID_FUNCTION)));
        assert!(matches!(loader.dos_exit_list(MAIN_VCPU_ID, 0x1_0001, ROUTINE_A),
                         ApiResult::Normal(ERROR_INVALID_DATA)));
    }

    #[test]
    fn dos_exit_process_runs_the_exit_list_before_the_process_ends() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        // A routine that returns (EXIT_TRAP) and one calling DosExit: each
        // ends like DosExitList(EXLST_EXIT).
        add_routine(&loader, 1, EXIT_TRAP_ADDR);
        add_routine(&loader, 2, DOS_EXIT_API);
        let mut vcpu = vcpu_calling_dos_exit(&loader, EXIT_PROCESS, 4);
        vcpu.push_exit(VmExit::Debug);
        vcpu.push_exit(VmExit::Debug);
        loader.run_vcpu(vcpu, MAIN_VCPU_ID, 0);
        assert!(loader.shutting_down());
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 4,
                   "both routines ran, the list ended the process (a halt would leave 0)");
        assert_eq!(exit_list_len(&loader), 0);
    }

    /// DosExit(EXIT_PROCESS) on another thread hands the exit list to
    /// thread 1 (exitlist_thread.exe covers the hand-over itself); not on
    /// thread 1, with an empty list or once thread 1 has ended.
    #[test]
    fn only_another_thread_with_routines_hands_the_exit_list_to_thread_1() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        assert!(!loader.exit_on_thread_1(FIRST_CREATED_TID, 4), "an empty list");
        add_routine(&loader, 0, ROUTINE_A);
        assert!(!loader.exit_on_thread_1(MAIN_VCPU_ID, 4), "thread 1 runs the list itself");
        loader.run_vcpu(vcpu_calling_dos_exit(&loader, EXIT_THREAD, 1), MAIN_VCPU_ID, 0);
        assert!(!loader.exit_on_thread_1(FIRST_CREATED_TID, 4), "thread 1 has ended");
        assert!(!loader.shared.threads.signals.lock_or_recover().termination_pending());
    }

    #[test]
    fn thread_1_ending_last_runs_the_exit_list() {
        let loader = super::super::Loader::new_mock();
        add_routine(&loader, 0, EXIT_TRAP_ADDR);
        let mut vcpu = vcpu_calling_dos_exit(&loader, EXIT_THREAD, 6);
        vcpu.push_exit(VmExit::Debug);
        loader.run_vcpu(vcpu, MAIN_VCPU_ID, 0);
        assert!(loader.shutting_down());
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 6);
        assert_eq!(exit_list_len(&loader), 0);
    }

    #[test]
    fn thread_1_returning_runs_the_exit_list() {
        let loader = super::super::Loader::new_mock();
        add_routine(&loader, 0, EXIT_TRAP_ADDR);
        let mut vcpu = vcpu_at(EXIT_TRAP_ADDR, 5);
        vcpu.push_exit(VmExit::Debug);
        loader.run_vcpu(vcpu, MAIN_VCPU_ID, 0);
        assert!(loader.shutting_down());
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 0);
        assert_eq!(exit_list_len(&loader), 0);
    }

    #[test]
    fn no_exit_list_once_thread_1_has_ended() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        add_routine(&loader, 0, ROUTINE_A);
        loader.run_vcpu(vcpu_calling_dos_exit(&loader, EXIT_THREAD, 1), MAIN_VCPU_ID, 0);
        assert!(!loader.shutting_down());
        loader.run_vcpu(vcpu_calling_dos_exit(&loader, EXIT_PROCESS, 2), FIRST_CREATED_TID, 0);
        assert!(loader.shutting_down());
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 2);
        assert_eq!(exit_list_len(&loader), 1, "the routine never ran");
    }

    #[test]
    fn other_threads_stop_while_the_exit_list_runs() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        add_routine(&loader, 0, ROUTINE_A);
        assert!(loader.claim_exit_list(MAIN_VCPU_ID, 0, TC_EXIT, STACK_TOP));
        // Thread 2 would otherwise run (and halt the process).
        loader.run_vcpu(vcpu_at(EXIT_TRAP_ADDR, 0), FIRST_CREATED_TID, 0);
        assert!(!loader.shared.exit_requested.load(Ordering::Relaxed), "the process goes on");
        assert!(!loader.shared.threads.table.lock_or_recover().is_running(FIRST_CREATED_TID));
        // Thread 2 asking for the process end meanwhile changes nothing.
        loader.end_process(9);
        assert!(!loader.shared.exit_requested.load(Ordering::Relaxed));
    }

    // ── Kicks, suspension, signals ──────────────────────────────────────

    #[test]
    fn an_interrupted_exit_resumes_the_guest_untouched() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        let mut vcpu = MockVcpu::new();
        vcpu.regs.rip = EXIT_TRAP_ADDR as u64;
        vcpu.regs.rax = 7;
        vcpu.regs.rsp = STACK_TOP as u64;
        vcpu.push_exit(VmExit::Interrupted);
        vcpu.push_exit(VmExit::Interrupted);
        vcpu.push_exit(VmExit::Debug);
        loader.run_vcpu(Box::new(vcpu), FIRST_CREATED_TID, 0);
        // The thread went on to its own end with its registers intact.
        let mut table = loader.shared.threads.table.lock_or_recover();
        assert_eq!(table.wait(MAIN_TID, FIRST_CREATED_TID), WaitOutcome::Ended(FIRST_CREATED_TID));
        drop(table);
        assert!(!loader.shutting_down());
    }

    #[test]
    fn killing_a_thread_kicks_it_out_of_the_guest() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        let vcpu = vcpu_at(0x1_0000, 0);
        let kick = vcpu.kick.clone();
        loader.shared.threads.table.lock_or_recover().set_kicker(FIRST_CREATED_TID, kick.clone());
        assert_eq!(loader.dos_kill_thread(MAIN_VCPU_ID, FIRST_CREATED_TID), NO_ERROR);
        assert_eq!(kick.kicks.load(Ordering::SeqCst), 1);
        // The kicked vCPU leaves the guest and the thread ends.
        loader.run_vcpu(vcpu, FIRST_CREATED_TID, 0);
        assert!(!loader.shared.threads.table.lock_or_recover().is_running(FIRST_CREATED_TID));
    }

    #[test]
    fn a_critical_section_and_the_process_end_kick_the_other_threads() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        let (main, other) = (MockVcpu::new(), MockVcpu::new());
        {
            let mut table = loader.shared.threads.table.lock_or_recover();
            table.set_kicker(MAIN_TID, main.kick.clone());
            table.set_kicker(FIRST_CREATED_TID, other.kick.clone());
        }
        assert_eq!(loader.dos_enter_crit_sec(MAIN_VCPU_ID), NO_ERROR);
        assert_eq!(main.kick.kicks.load(Ordering::SeqCst), 0, "not the owner");
        assert_eq!(other.kick.kicks.load(Ordering::SeqCst), 1);
        loader.dos_exit_crit_sec(MAIN_VCPU_ID);
        loader.request_process_exit(0);
        assert_eq!(main.kick.kicks.load(Ordering::SeqCst), 1);
        assert_eq!(other.kick.kicks.load(Ordering::SeqCst), 2);
        // A thread that ended is no longer kicked.
        loader.shared.threads.table.lock_or_recover().end(FIRST_CREATED_TID);
        loader.request_process_exit(0);
        assert_eq!(other.kick.kicks.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_suspended_thread_waits_until_resumed() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        let vcpu = vcpu_at(EXIT_TRAP_ADDR, 4);
        let kick = vcpu.kick.clone();
        loader.shared.threads.table.lock_or_recover().set_kicker(FIRST_CREATED_TID, kick.clone());
        assert_eq!(loader.dos_suspend_thread(MAIN_VCPU_ID, FIRST_CREATED_TID), NO_ERROR);
        assert_eq!(loader.dos_suspend_thread(MAIN_VCPU_ID, FIRST_CREATED_TID), NO_ERROR);
        assert_eq!(kick.kicks.load(Ordering::SeqCst), 2);
        // Killed while suspended: it ends only once resumed.
        assert_eq!(loader.dos_kill_thread(MAIN_VCPU_ID, FIRST_CREATED_TID), NO_ERROR);
        std::thread::scope(|s| {
            let runner = s.spawn(|| loader.run_vcpu(vcpu, FIRST_CREATED_TID, 0));
            std::thread::sleep(std::time::Duration::from_millis(60));
            assert!(loader.shared.threads.table.lock_or_recover().is_running(FIRST_CREATED_TID));
            assert_eq!(loader.dos_resume_thread(FIRST_CREATED_TID), NO_ERROR);
            std::thread::sleep(std::time::Duration::from_millis(60));
            assert!(loader.shared.threads.table.lock_or_recover().is_running(FIRST_CREATED_TID),
                    "suspended twice: still waiting");
            assert_eq!(loader.dos_resume_thread(FIRST_CREATED_TID), NO_ERROR);
            runner.join().unwrap();
        });
        assert!(!loader.shared.threads.table.lock_or_recover().is_running(FIRST_CREATED_TID));
        assert_eq!(loader.dos_resume_thread(FIRST_CREATED_TID), ERROR_INVALID_THREADID);
    }

    #[test]
    fn a_suspended_thread_stops_when_the_process_ends() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        loader.dos_suspend_thread(MAIN_VCPU_ID, FIRST_CREATED_TID);
        std::thread::scope(|s| {
            let runner = s.spawn(|| loader.run_vcpu(vcpu_at(0x1_0000, 0), FIRST_CREATED_TID, 0));
            std::thread::sleep(std::time::Duration::from_millis(30));
            loader.request_process_exit(3);
            runner.join().unwrap();
        });
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn suspend_and_resume_errors() {
        let loader = super::super::Loader::new_mock();
        start_thread(&loader, FIRST_CREATED_TID);
        assert_eq!(loader.dos_suspend_thread(MAIN_VCPU_ID, MAIN_TID), ERROR_INVALID_THREADID, "itself");
        assert_eq!(loader.dos_suspend_thread(MAIN_VCPU_ID, 99), ERROR_INVALID_THREADID);
        assert_eq!(loader.dos_resume_thread(FIRST_CREATED_TID), ERROR_NOT_FROZEN);
        assert_eq!(loader.dos_resume_thread(99), ERROR_INVALID_THREADID);
        // Thread 1 can be suspended by another thread.
        assert_eq!(loader.dos_suspend_thread(FIRST_CREATED_TID, MAIN_TID), NO_ERROR);
        assert_eq!(loader.dos_resume_thread(MAIN_TID), NO_ERROR);
        assert_eq!(loader.dos_resume_thread(MAIN_TID), ERROR_NOT_FROZEN);
    }

    #[test]
    fn dos_create_thread_can_start_suspended() {
        let loader = super::super::Loader::new_mock();
        const PTID: u32 = 0x7_0000;
        // The mock vCPU halts (ending the process) as soon as it runs.
        assert_eq!(loader.dos_create_thread(MAIN_VCPU_ID, PTID, 0x1_0000, 0,
                                            CREATE_SUSPENDED | STACK_COMMITTED, 0x2000), NO_ERROR);
        let tid = loader.guest_read::<u32>(PTID).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(!loader.shutting_down(), "the thread has not run");
        assert_eq!(loader.shared.threads.table.lock_or_recover().suspend_count(tid), 1);
        assert_eq!(loader.dos_resume_thread(tid), NO_ERROR);
        loader.wait_for_process_end();
    }

    #[test]
    fn a_signal_calls_thread_1s_handler_on_its_stack() {
        let loader = super::super::Loader::new_mock();
        // Handler = the exit trap, so that the "call" ends the thread.
        let rec = 0x6000u32;
        loader.guest_write::<u32>(rec, XCPT_CHAIN_END);
        loader.guest_write::<u32>(rec + 4, EXIT_TRAP_ADDR);
        loader.guest_write::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET, rec);
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        assert_eq!(loader.dos_send_signal_exception(PROCESS_ID, XCPT_SIGNAL_INTR), NO_ERROR);
        let mut vcpu = vcpu_at(0x1_0000, 0);
        vcpu.sregs.cs.db = 1;
        vcpu.sregs.ss.db = 1;
        loader.run_vcpu(vcpu, MAIN_VCPU_ID, 0);
        let sp = STACK_TOP - 20;
        assert_eq!(loader.guest_read::<u32>(sp), Some(XCPT_RET_TRAP), "a handler returns to XCPT_RET_TRAP (ADR-0110)");
        let report = loader.guest_read::<u32>(sp + 4).unwrap();
        assert_eq!(loader.guest_read::<u32>(report + ERR_NUM), Some(XCPT_SIGNAL));
        assert_eq!(loader.guest_read::<u32>(report + ERR_PARAMS), Some(XCPT_SIGNAL_INTR));
        let ctx = loader.guest_read::<u32>(sp + 12).unwrap();
        assert_eq!(loader.guest_read::<u32>(ctx + CTX_EIP), Some(0x1_0000));
        assert_eq!(loader.guest_read::<u32>(ctx + CTX_ESP), Some(STACK_TOP));
    }

    #[test]
    fn an_unwanted_signal_ends_the_process_through_its_exit_list() {
        let loader = super::super::Loader::new_mock();
        // No focus: Ctrl+C ends the process (its default action).
        loader.post_signal(XCPT_SIGNAL_INTR);
        let mut vcpu = vcpu_at(0x1_0000, 0);
        vcpu.sregs.cs.db = 1;
        vcpu.sregs.ss.db = 1;
        loader.run_vcpu(vcpu, MAIN_VCPU_ID, 0);
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), SIGNAL_EXIT_CODE as i32);
    }
}
