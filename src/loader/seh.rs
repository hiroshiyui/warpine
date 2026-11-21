// SPDX-License-Identifier: GPL-3.0-or-later
//
// OS/2 Structured Exception Handling (SEH) helper methods.
//
// Architecture:
//   • TIB+0x00 (tib_pexchain) is the head of the per-thread handler chain.
//     Initialised to XCPT_CHAIN_END (0xFFFFFFFF) in setup_guest().
//   • Each EXCEPTIONREGISTRATIONRECORD is an 8-byte guest structure:
//       +0x00  prev_structure  (u32) — next record toward XCPT_CHAIN_END
//       +0x04  ExceptionHandler (u32) — handler function pointer
//   • DosSetExceptionHandler pushes a record onto the chain head.
//   • DosUnsetExceptionHandler pops it.
//   • On a hardware fault (IDT exception) or DosRaiseException the dispatcher
//     allocates guest EXCEPTIONREPORTRECORD + CONTEXTRECORD, then calls the
//     first handler via ApiResult::ExceptionDispatch.
//   • Handlers are called with the return address XCPT_RET_TRAP, which
//     vcpu.rs takes like CALLBACK_RET_TRAP; for FrameKind::ExceptionHandler:
//       XCPT_CONTINUE_EXECUTION  → restore FaultContext, resume
//       XCPT_CONTINUE_SEARCH    → invoke next handler in chain (or crash)
//   • DosUnwindException (ADR-0110) returns ApiResult::Unwind: vcpu.rs calls
//     the handlers above the target with EH_UNWINDING one at a time
//     (FrameKind::UnwindHandler, `unwind_step` here), then goes on at
//     pTargetIP or returns.

use super::constants::*;
use super::{ApiResult, FaultContext};
use super::mutex_ext::MutexExt;
use log::{debug, warn};

/// Most records an unwind or a chain walk visits (a cycle in a damaged
/// chain ends there).
const MAX_CHAIN_WALK: usize = 256;

/// A `DosUnwindException` in progress (ADR-0110).
#[derive(Debug)]
pub(crate) struct Unwind {
    /// The record the unwind stops at (`XCPT_CHAIN_END`: the whole chain).
    pub target: u32,
    /// Where the thread goes on afterwards; 0: the call returns.
    pub target_ip: u32,
    /// The report record each handler gets, and whether Warpine made it.
    pub report: u32,
    pub own_report: bool,
    /// The CONTEXTRECORD each handler gets: the caller's state.
    pub ctx: u32,
    /// The registers the thread goes on with (filled in by `vcpu.rs`).
    pub resume: FaultContext,
    /// Guest blocks to free at the end.
    pub allocs: Vec<u32>,
    /// The record whose handler is being called (`XCPT_CHAIN_END`: none).
    pub current: u32,
    /// Handlers called so far.
    pub calls: usize,
}

/// Bytes read at a faulting EIP to recognise a privileged instruction
/// (prefixes, opcode and ModR/M).
const PRIVILEGED_SCAN_LEN: usize = 8;

/// Map an x86 exception vector + error code to an OS/2 XCPT_* exception code.
pub fn xcpt_code_for_vector(vector: u32, _error_code: u32) -> u32 {
    match vector {
        0  => XCPT_INTEGER_DIVIDE_BY_ZERO,
        1  => XCPT_SINGLE_STEP,
        3  => XCPT_BREAKPOINT,
        4  => XCPT_INTEGER_OVERFLOW,
        5  => XCPT_ACCESS_VIOLATION,     // BOUND range check (similar)
        6  => XCPT_ILLEGAL_INSTRUCTION,
        7  => XCPT_ILLEGAL_INSTRUCTION,  // device-not-available (#NM)
        10 => XCPT_ACCESS_VIOLATION,     // invalid TSS (#TS)
        11 => XCPT_ACCESS_VIOLATION,     // segment not present (#NP)
        12 => XCPT_UNABLE_TO_GROW_STACK, // stack fault (#SS)
        13 => XCPT_ACCESS_VIOLATION,     // general protection (#GP)
        14 => XCPT_ACCESS_VIOLATION,     // page fault (#PF)
        16 => XCPT_FLOAT_INVALID_OPERATION, // FPU error (#MF)
        17 => XCPT_DATATYPE_MISALIGNMENT,
        19 => XCPT_FLOAT_INVALID_OPERATION, // SIMD FP (#XM)
        _  => XCPT_FATAL_EXCEPTION,
    }
}

impl super::Loader {
    /// Write an `EXCEPTIONREPORTRECORD` into guest memory at `addr`.
    ///
    /// `params` is a slice of up to 9 additional exception parameters
    /// (e.g. the faulting virtual address for XCPT_ACCESS_VIOLATION).
    pub(crate) fn write_exception_report(
        &self,
        addr: u32,
        xcpt_code: u32,
        flags: u32,
        exc_addr: u32,
        params: &[u32],
    ) {
        let n = params.len().min(9) as u32;
        self.guest_write::<u32>(addr + ERR_NUM,    xcpt_code).unwrap();
        self.guest_write::<u32>(addr + ERR_FLAGS,  flags).unwrap();
        self.guest_write::<u32>(addr + ERR_NESTED, 0).unwrap();
        self.guest_write::<u32>(addr + ERR_ADDR,   exc_addr).unwrap();
        self.guest_write::<u32>(addr + ERR_CPARMS, n).unwrap();
        for (i, &p) in params.iter().take(9).enumerate() {
            self.guest_write::<u32>(addr + ERR_PARAMS + i as u32 * 4, p).unwrap();
        }
        // Zero remaining parameter slots
        for i in params.len()..9 {
            self.guest_write::<u32>(addr + ERR_PARAMS + i as u32 * 4, 0).unwrap();
        }
    }

    /// Write a `CONTEXTRECORD` into guest memory at `addr` from a `FaultContext`.
    pub(crate) fn write_context_record(&self, addr: u32, ctx: &FaultContext) {
        // Zero the floating-point environment and register stack.
        for off in (CTX_FPU_ENV..CTX_GS).step_by(4) {
            self.guest_write::<u32>(addr + off, 0).unwrap();
        }
        self.guest_write::<u32>(addr + CTX_FLAGS,  CONTEXT_FULL).unwrap();
        self.guest_write::<u32>(addr + CTX_GS,     ctx.gs as u32).unwrap();
        self.guest_write::<u32>(addr + CTX_FS,     ctx.fs as u32).unwrap();
        self.guest_write::<u32>(addr + CTX_ES,     ctx.es as u32).unwrap();
        self.guest_write::<u32>(addr + CTX_DS,     ctx.ds as u32).unwrap();
        self.guest_write::<u32>(addr + CTX_EDI,    ctx.edi).unwrap();
        self.guest_write::<u32>(addr + CTX_ESI,    ctx.esi).unwrap();
        self.guest_write::<u32>(addr + CTX_EAX,    ctx.eax).unwrap();
        self.guest_write::<u32>(addr + CTX_EBX,    ctx.ebx).unwrap();
        self.guest_write::<u32>(addr + CTX_ECX,    ctx.ecx).unwrap();
        self.guest_write::<u32>(addr + CTX_EDX,    ctx.edx).unwrap();
        self.guest_write::<u32>(addr + CTX_EBP,    ctx.ebp).unwrap();
        self.guest_write::<u32>(addr + CTX_EIP,    ctx.eip).unwrap();
        self.guest_write::<u32>(addr + CTX_CS,     ctx.cs as u32).unwrap();
        self.guest_write::<u32>(addr + CTX_EFLAGS, ctx.eflags).unwrap();
        self.guest_write::<u32>(addr + CTX_ESP,    ctx.esp).unwrap();
        self.guest_write::<u32>(addr + CTX_SS,     ctx.ss as u32).unwrap();
    }

    /// Restore guest registers from a `CONTEXTRECORD` in guest memory.
    ///
    /// Called when a handler returns XCPT_CONTINUE_EXECUTION and we need to
    /// resume execution at the fault context rather than at the handler call site.
    pub(crate) fn read_context_record(&self, addr: u32) -> FaultContext {
        FaultContext {
            eax:    self.guest_read::<u32>(addr + CTX_EAX).unwrap_or(0),
            ebx:    self.guest_read::<u32>(addr + CTX_EBX).unwrap_or(0),
            ecx:    self.guest_read::<u32>(addr + CTX_ECX).unwrap_or(0),
            edx:    self.guest_read::<u32>(addr + CTX_EDX).unwrap_or(0),
            esi:    self.guest_read::<u32>(addr + CTX_ESI).unwrap_or(0),
            edi:    self.guest_read::<u32>(addr + CTX_EDI).unwrap_or(0),
            ebp:    self.guest_read::<u32>(addr + CTX_EBP).unwrap_or(0),
            esp:    self.guest_read::<u32>(addr + CTX_ESP).unwrap_or(0),
            eip:    self.guest_read::<u32>(addr + CTX_EIP).unwrap_or(0),
            eflags: self.guest_read::<u32>(addr + CTX_EFLAGS).unwrap_or(2),
            cs:     self.guest_read::<u32>(addr + CTX_CS).unwrap_or(0x08) as u16,
            ds:     self.guest_read::<u32>(addr + CTX_DS).unwrap_or(0x10) as u16,
            es:     self.guest_read::<u32>(addr + CTX_ES).unwrap_or(0x10) as u16,
            fs:     self.guest_read::<u32>(addr + CTX_FS).unwrap_or(0x18) as u16,
            gs:     self.guest_read::<u32>(addr + CTX_GS).unwrap_or(0x10) as u16,
            ss:     self.guest_read::<u32>(addr + CTX_SS).unwrap_or(0x10) as u16,
        }
    }

    /// Allocate guest memory for an EXCEPTIONREPORTRECORD and CONTEXTRECORD,
    /// populate them from the fault context, and return their addresses along
    /// with the allocation list (for later freeing).
    pub(crate) fn alloc_exception_records(
        &self,
        fault: &FaultContext,
        xcpt_code: u32,
        flags: u32,
        exc_addr: u32,
        params: &[u32],
    ) -> (u32, u32, Vec<u32>) {
        let mut mem = self.shared.mem_mgr.lock_or_recover();
        let exc_report = mem.alloc(EXCEPTION_REPORT_SIZE).expect("SEH: alloc exc_report OOB");
        let ctx_record = mem.alloc(CONTEXT_RECORD_SIZE).expect("SEH: alloc ctx_record OOB");
        drop(mem);
        self.write_exception_report(exc_report, xcpt_code, flags, exc_addr, params);
        self.write_context_record(ctx_record, fault);
        (exc_report, ctx_record, vec![exc_report, ctx_record])
    }

    /// Free the guest allocations used during exception dispatch.
    pub(crate) fn free_exception_records(&self, allocs: Vec<u32>) {
        let mut mem = self.shared.mem_mgr.lock_or_recover();
        for addr in allocs {
            mem.free(addr);
        }
    }

    /// Try to dispatch a hardware exception through the OS/2 exception handler chain.
    ///
    /// Returns `Some(ApiResult::ExceptionDispatch { ... })` if a handler was found,
    /// or `None` if the chain is exhausted (caller should crash).
    ///
    /// The `fault` context captures the CPU state at exception time.
    pub(crate) fn try_hw_exception_dispatch(
        &self,
        vcpu_id: u32,
        fault: FaultContext,
        vector: u32,
        error_code: u32,
        fault_addr: u32,
    ) -> Option<ApiResult> {
        // A #GP at ring 3 from an instruction only ring 0 may execute is a
        // privileged instruction, as on OS/2 (ADR-0113).
        const GP_VECTOR: u32 = 13;
        let privileged = vector == GP_VECTOR && error_code == 0 && fault.cs & 3 == 3
            && self.guest_read_bytes(fault.eip, PRIVILEGED_SCAN_LEN)
                .or_else(|| self.guest_read_bytes(fault.eip, 1))
                .is_some_and(|code| super::ring::is_privileged_instruction(&code));
        let xcpt_code = if privileged { XCPT_PRIVILEGED_INSTRUCTION } else { xcpt_code_for_vector(vector, error_code) };
        let flags = if xcpt_code == XCPT_ACCESS_VIOLATION || xcpt_code == XCPT_FATAL_EXCEPTION {
            EH_NONCONTINUABLE
        } else {
            0
        };

        // A page fault's access violation says how the address was
        // accessed and which address it was (ExceptionInfo[0], [1]).
        let access = if error_code & PF_ERROR_WRITE != 0 { XCPT_WRITE_ACCESS } else { XCPT_READ_ACCESS };
        let pf_params = [access, fault_addr];
        let params: &[u32] = if vector == 14 { &pf_params } else { &[] };
        debug!("SEH: hardware exception vector={}", vector);
        self.chain_dispatch(vcpu_id, fault, xcpt_code, flags, params)
    }

    /// Dispatch exception `xcpt_code` (raised at `fault`) to the first
    /// handler of thread `vcpu_id`'s chain. `None`: no usable handler.
    pub(crate) fn chain_dispatch(
        &self,
        vcpu_id: u32,
        fault: FaultContext,
        xcpt_code: u32,
        flags: u32,
        params: &[u32],
    ) -> Option<ApiResult> {
        let chain_head = self.exception_chain_addr(vcpu_id)
            .and_then(|head| self.guest_read::<u32>(head))
            .unwrap_or(XCPT_CHAIN_END);
        if chain_head == XCPT_CHAIN_END {
            return None; // No handlers registered
        }
        // Validate that chain_head is a plausible guest address (within 128 MB window).
        if chain_head == 0 || chain_head >= 0x0800_0000 {
            warn!("SEH: chain_head=0x{:08X} is outside guest address space; ignoring", chain_head);
            return None;
        }

        let handler_fn = self.guest_read::<u32>(chain_head + XERREC_HANDLER)
            .unwrap_or(0);
        if handler_fn == 0 {
            warn!("SEH: chain_head=0x{:08X} has null handler", chain_head);
            return None;
        }

        let next_handler = self.guest_read::<u32>(chain_head + XERREC_PREV)
            .unwrap_or(XCPT_CHAIN_END);

        let (exc_report, ctx_record, guest_allocs) =
            self.alloc_exception_records(&fault, xcpt_code, flags, fault.eip, params);

        debug!("SEH: dispatching xcpt=0x{:08X} to handler=0x{:08X} (reg=0x{:08X})",
               xcpt_code, handler_fn, chain_head);

        Some(ApiResult::ExceptionDispatch {
            handler_addr: handler_fn,
            exc_report,
            reg_rec:      chain_head,
            ctx_record,
            saved:        Box::new(fault),
            next_handler,
            guest_allocs,
        })
    }

    /// `DosRaiseException(pexcept)`: the caller's report record goes to the
    /// first handler of the calling thread's chain, with its
    /// `ExceptionAddress` set to the call's return address (ADR-0110).
    /// `vcpu.rs` sets the context so that `XCPT_CONTINUE_EXECUTION` returns
    /// `NO_ERROR` to the caller.
    pub(crate) fn dos_raise_exception(&self, vcpu: &dyn super::vm_backend::VcpuBackend, vcpu_id: u32, p_exc_rec: u32) -> ApiResult {
        debug!("  DosRaiseException(pExcRec=0x{:08X})", p_exc_rec);
        if p_exc_rec == 0 {
            return ApiResult::Normal(ERROR_INVALID_PARAMETER);
        }
        // The exception happened where the call returns to: that is its
        // ExceptionAddress, which Open Watcom's C++ runtime leaves 0 for
        // the system to fill in and unwinds to later (ADR-0110).
        let return_addr = vcpu.get_regs().ok().and_then(|r| self.guest_read::<u32>(r.rsp as u32));
        if return_addr.and_then(|ra| self.guest_write::<u32>(p_exc_rec.wrapping_add(ERR_ADDR), ra)).is_none() {
            return ApiResult::Normal(ERROR_INVALID_PARAMETER);
        }

        // Read the exception code from the caller-provided record.
        let xcpt_code = self.guest_read::<u32>(p_exc_rec.wrapping_add(ERR_NUM)).unwrap_or(XCPT_FATAL_EXCEPTION);
        let _flags    = self.guest_read::<u32>(p_exc_rec.wrapping_add(ERR_FLAGS)).unwrap_or(0);
        let exc_addr  = self.guest_read::<u32>(p_exc_rec.wrapping_add(ERR_ADDR)).unwrap_or(0);

        let chain_head = self.exception_chain_addr(vcpu_id)
            .and_then(|head| self.guest_read::<u32>(head))
            .unwrap_or(XCPT_CHAIN_END);
        if chain_head == XCPT_CHAIN_END {
            warn!("  DosRaiseException: no handlers registered, xcpt=0x{:08X}", xcpt_code);
            return ApiResult::Normal(ERROR_NESTING_NOT_ALLOWED); // no handlers
        }
        if chain_head == 0 || chain_head >= 0x0800_0000 {
            warn!("SEH: chain_head=0x{:08X} outside guest window; no handlers", chain_head);
            return ApiResult::Normal(ERROR_NESTING_NOT_ALLOWED);
        }

        let handler_fn = self.guest_read::<u32>(chain_head + XERREC_HANDLER).unwrap_or(0);
        if handler_fn == 0 {
            return ApiResult::Normal(ERROR_INVALID_PARAMETER);
        }

        let next_handler = self.guest_read::<u32>(chain_head + XERREC_PREV)
            .unwrap_or(XCPT_CHAIN_END);

        // Build a FaultContext representing the current execution point.
        // (EIP/ESP are unknown here — they'll be the API dispatch frame.
        // The caller-provided exc_rec already has the real fault address.)
        let fault = FaultContext {
            eax: 0, ebx: 0, ecx: 0, edx: 0, esi: 0, edi: 0, ebp: 0,
            esp: 0, eip: exc_addr,
            eflags: 2,
            ..FaultContext::flat(self.thread_ring())
        };

        // Allocate CONTEXTRECORD (using zeroed fault); pass caller's exc_rec directly.
        let ctx_record = self.shared.mem_mgr.lock_or_recover()
            .alloc(CONTEXT_RECORD_SIZE).expect("SEH: alloc ctx_record OOB");
        self.write_context_record(ctx_record, &fault);

        debug!("  DosRaiseException: xcpt=0x{:08X} handler=0x{:08X}", xcpt_code, handler_fn);

        ApiResult::ExceptionDispatch {
            handler_addr: handler_fn,
            exc_report:   p_exc_rec,  // use caller's record directly
            reg_rec:      chain_head,
            ctx_record,
            saved:        Box::new(fault),
            next_handler,
            guest_allocs: vec![ctx_record],
        }
    }

    /// `DosUnwindException(phandler, pTargetIP, pERepRec)` (ADR-0110): the
    /// handlers above `phandler` on the calling thread's chain are called
    /// with `EH_UNWINDING`, newest first, and each leaves the chain when it
    /// returns. `phandler` stays; `END_OF_CHAIN` or `UNWIND_ALL` (0) unwinds
    /// the whole chain. Then the thread goes on at `pTargetIP` as if the call
    /// had returned there (its registers and stack as at the return), or
    /// returns when it is NULL. The report record the handlers get is
    /// `pERepRec`, its flags with `EH_UNWINDING` added, or one Warpine makes
    /// (`XCPT_UNWIND`). A `phandler` that is not on the chain is refused,
    /// with nothing unwound.
    ///
    /// The calls are made by `vcpu.rs` (`ApiResult::Unwind`), which knows
    /// the caller's registers; this returns what it needs.
    pub(crate) fn dos_unwind_exception(&self, vcpu_id: u32, p_target: u32, p_target_ip: u32, p_report: u32) -> ApiResult {
        debug!("  DosUnwindException(phandler=0x{p_target:08X}, pTargetIP=0x{p_target_ip:08X}, pERepRec=0x{p_report:08X})");
        let target = if p_target == UNWIND_ALL { XCPT_CHAIN_END } else { p_target };
        let Some(head) = self.exception_chain_addr(vcpu_id) else { return ApiResult::Normal(ERROR_INVALID_PARAMETER) };
        if target != XCPT_CHAIN_END && !self.chain_holds(head, target) {
            warn!("  DosUnwindException: 0x{target:08X} is not on the chain");
            return ApiResult::Normal(ERROR_INVALID_PARAMETER);
        }
        let mut allocs = Vec::new();
        let report = if p_report != 0 {
            let flags = self.guest_read::<u32>(p_report.wrapping_add(ERR_FLAGS));
            if flags.and_then(|f| self.guest_write::<u32>(p_report.wrapping_add(ERR_FLAGS), f | EH_UNWINDING)).is_none() {
                return ApiResult::Normal(ERROR_INVALID_PARAMETER);
            }
            p_report
        } else {
            let Some(rec) = self.shared.mem_mgr.lock_or_recover().alloc(EXCEPTION_REPORT_SIZE) else {
                return ApiResult::Normal(ERROR_NOT_ENOUGH_MEMORY);
            };
            allocs.push(rec);
            // The address is the caller's; vcpu.rs fills it in.
            self.write_exception_report(rec, XCPT_UNWIND, EH_UNWINDING, 0, &[]);
            rec
        };
        let Some(ctx) = self.shared.mem_mgr.lock_or_recover().alloc(CONTEXT_RECORD_SIZE) else {
            self.free_exception_records(allocs);
            return ApiResult::Normal(ERROR_NOT_ENOUGH_MEMORY);
        };
        allocs.push(ctx);
        ApiResult::Unwind(Box::new(Unwind {
            target, target_ip: p_target_ip, report, own_report: p_report == 0, ctx,
            resume: FaultContext::default(), allocs, current: XCPT_CHAIN_END, calls: 0,
        }))
    }

    /// Whether record `rec` is on the chain whose head is at `head`.
    fn chain_holds(&self, head: u32, rec: u32) -> bool {
        let mut cur = self.guest_read::<u32>(head).unwrap_or(XCPT_CHAIN_END);
        for _ in 0..MAX_CHAIN_WALK {
            if cur == rec { return true; }
            if cur == XCPT_CHAIN_END { return false; }
            match self.guest_read::<u32>(cur.wrapping_add(XERREC_PREV)) {
                Some(prev) => cur = prev,
                None => return false,
            }
        }
        false
    }

    /// The next handler `unwind` calls on thread `vcpu_id`: unlink the one
    /// that just returned, then take the head if it is above the target.
    /// `Some((handler, record))`, or None when the unwind is done.
    pub(crate) fn unwind_step(&self, vcpu_id: u32, unwind: &mut Unwind) -> Option<(u32, u32)> {
        let head = self.exception_chain_addr(vcpu_id)?;
        if unwind.current != XCPT_CHAIN_END {
            // The handler that returned leaves the chain (unless it took
            // itself off already).
            if self.guest_read::<u32>(head) == Some(unwind.current) {
                let prev = self.guest_read::<u32>(unwind.current.wrapping_add(XERREC_PREV)).unwrap_or(XCPT_CHAIN_END);
                self.guest_write::<u32>(head, prev);
            }
            unwind.current = XCPT_CHAIN_END;
        }
        loop {
            let rec = self.guest_read::<u32>(head).unwrap_or(XCPT_CHAIN_END);
            if rec == XCPT_CHAIN_END || rec == unwind.target {
                return None;
            }
            if unwind.calls >= MAX_CHAIN_WALK {
                warn!("SEH unwind: more than {MAX_CHAIN_WALK} handlers — a cycle? Stopping");
                return None;
            }
            unwind.calls += 1;
            let prev = self.guest_read::<u32>(rec.wrapping_add(XERREC_PREV)).unwrap_or(XCPT_CHAIN_END);
            match self.guest_read::<u32>(rec.wrapping_add(XERREC_HANDLER)) {
                Some(handler) if handler != 0 => {
                    unwind.current = rec;
                    return Some((handler, rec));
                }
                // No handler to call: the record just leaves.
                _ => { self.guest_write::<u32>(head, prev); }
            }
        }
    }

    /// Address of the exception-handler chain head (`tib_pexchain`) of the
    /// thread running on vCPU `vcpu_id`: each thread has its own TIB, which
    /// its FS selector points at; `None` if the thread is not running.
    pub(crate) fn exception_chain_addr(&self, vcpu_id: u32) -> Option<u32> {
        let tid = super::threads::tid_of_vcpu(vcpu_id);
        let tib = self.shared.threads.table.lock_or_recover().tib_of(tid)?;
        Some(tib + TIB_EXCHAIN_OFFSET)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::{Loader, ApiResult};
    use super::super::constants::*;
    use super::super::mutex_ext::MutexExt;
    use super::xcpt_code_for_vector;

    // ── xcpt_code_for_vector ─────────────────────────────────────────────────

    #[test]
    fn test_xcpt_divide_by_zero() {
        assert_eq!(xcpt_code_for_vector(0, 0), XCPT_INTEGER_DIVIDE_BY_ZERO);
    }

    #[test]
    fn test_xcpt_illegal_instruction() {
        assert_eq!(xcpt_code_for_vector(6, 0), XCPT_ILLEGAL_INSTRUCTION);
    }

    #[test]
    fn test_xcpt_page_fault() {
        assert_eq!(xcpt_code_for_vector(14, 0), XCPT_ACCESS_VIOLATION);
    }

    #[test]
    fn test_xcpt_gp_fault() {
        assert_eq!(xcpt_code_for_vector(13, 0), XCPT_ACCESS_VIOLATION);
    }

    #[test]
    fn test_xcpt_unknown_vector() {
        assert_eq!(xcpt_code_for_vector(255, 0), XCPT_FATAL_EXCEPTION);
    }

    // ── DosSetExceptionHandler / DosUnsetExceptionHandler ───────────────────

    /// Helper: write a minimal EXCEPTIONREGISTRATIONRECORD at `addr`.
    fn write_reg_rec(loader: &Loader, addr: u32, prev: u32, handler: u32) {
        loader.guest_write::<u32>(addr + XERREC_PREV, prev).unwrap();
        loader.guest_write::<u32>(addr + XERREC_HANDLER, handler).unwrap();
    }

    /// Initialise TIB_EXCHAIN_OFFSET (not done by new_mock, only by setup_guest).
    fn init_tib_chain(loader: &Loader) {
        loader.guest_write::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET, XCPT_CHAIN_END).unwrap();
    }

    /// Each thread has its own chain, in its own TIB: a handler a created
    /// thread registers is not seen by thread 1, and the other way round.
    #[test]
    fn test_exception_chains_are_per_thread() {
        use super::super::threads::ThreadMemory;
        let loader = Loader::new_mock();
        init_tib_chain(&loader);
        let (tib2, stack2) = {
            let mut mm = loader.shared.mem_mgr.lock_or_recover();
            (mm.alloc(4096).unwrap(), mm.alloc(4096).unwrap())
        };
        loader.guest_write::<u32>(tib2 + TIB_EXCHAIN_OFFSET, XCPT_CHAIN_END).unwrap();
        // Thread 2 runs on vCPU 2.
        loader.shared.threads.table.lock_or_recover().start(2, ThreadMemory { stack: stack2, tib: tib2, paging: 0 });
        let rec_main = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        let rec_t2 = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();

        assert_eq!(loader.dos_set_exception_handler(2, rec_t2), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(tib2 + TIB_EXCHAIN_OFFSET).unwrap(), rec_t2);
        assert_eq!(loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap(), XCPT_CHAIN_END,
                   "thread 1's chain untouched");
        assert_eq!(loader.dos_set_exception_handler(0, rec_main), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap(), rec_main);
        assert_eq!(loader.guest_read::<u32>(rec_main).unwrap(), XCPT_CHAIN_END, "linked to thread 1's own chain");

        // A fault on thread 2 is dispatched to thread 2's handler.
        write_reg_rec(&loader, rec_t2, XCPT_CHAIN_END, 0x5000);
        let fault = super::super::FaultContext {
            eax: 0, ebx: 0, ecx: 0, edx: 0, esi: 0, edi: 0, ebp: 0, esp: 0x8000, eip: 0x1000,
            eflags: 2, cs: 0x08, ds: 0x10, es: 0x10, fs: 0x18, gs: 0x10, ss: 0x10,
        };
        match loader.try_hw_exception_dispatch(2, fault, 14, 0, 0) {
            Some(ApiResult::ExceptionDispatch { handler_addr, reg_rec, .. }) => {
                assert_eq!((handler_addr, reg_rec), (0x5000, rec_t2));
            }
            _ => panic!("thread 2's handler not found"),
        }

        assert_eq!(loader.dos_unset_exception_handler(2, rec_t2), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(tib2 + TIB_EXCHAIN_OFFSET).unwrap(), XCPT_CHAIN_END);
        assert_eq!(loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap(), rec_main);

        // A thread that does not run, and records outside guest memory, fail
        // instead of panicking.
        assert_eq!(loader.dos_set_exception_handler(7, rec_t2), ERROR_INVALID_PARAMETER);
        assert_eq!(loader.dos_set_exception_handler(0, u32::MAX - 1), ERROR_INVALID_PARAMETER);
        assert_eq!(loader.dos_unset_exception_handler(0, u32::MAX - 1), ERROR_INVALID_PARAMETER);
    }

    #[test]
    fn test_set_exception_handler_pushes_chain() {
        let loader = Loader::new_mock();
        init_tib_chain(&loader);
        // TIB starts with XCPT_CHAIN_END
        assert_eq!(
            loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap(),
            XCPT_CHAIN_END
        );

        let buf = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        write_reg_rec(&loader, buf, 0, 0x1234);

        assert_eq!(loader.dos_set_exception_handler(0, buf), NO_ERROR);

        // TIB chain head is now buf
        assert_eq!(
            loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap(),
            buf
        );
        // buf->prev_structure points to the old head (XCPT_CHAIN_END)
        assert_eq!(
            loader.guest_read::<u32>(buf + XERREC_PREV).unwrap(),
            XCPT_CHAIN_END
        );
    }

    #[test]
    fn test_set_then_unset_exception_handler_restores_chain() {
        let loader = Loader::new_mock();
        init_tib_chain(&loader);
        let buf = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        write_reg_rec(&loader, buf, 0, 0x1234);

        loader.dos_set_exception_handler(0, buf);
        assert_eq!(loader.dos_unset_exception_handler(0, buf), NO_ERROR);

        // Chain is back to XCPT_CHAIN_END
        assert_eq!(
            loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap(),
            XCPT_CHAIN_END
        );
    }

    #[test]
    fn test_set_multiple_handlers_forms_linked_list() {
        let loader = Loader::new_mock();
        init_tib_chain(&loader);
        let mut mem = loader.shared.mem_mgr.lock_or_recover();
        let rec1 = mem.alloc(16).unwrap();
        let rec2 = mem.alloc(16).unwrap();
        drop(mem);

        write_reg_rec(&loader, rec1, 0, 0x1111);
        write_reg_rec(&loader, rec2, 0, 0x2222);

        loader.dos_set_exception_handler(0, rec1);
        loader.dos_set_exception_handler(0, rec2);

        // Chain: TIB → rec2 → rec1 → XCPT_CHAIN_END
        assert_eq!(loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap(), rec2);
        assert_eq!(loader.guest_read::<u32>(rec2 + XERREC_PREV).unwrap(), rec1);
        assert_eq!(loader.guest_read::<u32>(rec1 + XERREC_PREV).unwrap(), XCPT_CHAIN_END);
    }

    #[test]
    fn test_set_handler_null_returns_error() {
        let loader = Loader::new_mock();
        assert_ne!(loader.dos_set_exception_handler(0, 0), NO_ERROR);
    }

    #[test]
    fn test_unset_handler_null_returns_error() {
        let loader = Loader::new_mock();
        assert_ne!(loader.dos_unset_exception_handler(0, 0), NO_ERROR);
    }

    // ── try_hw_exception_dispatch ────────────────────────────────────────────

    #[test]
    fn test_hw_dispatch_no_handlers_returns_none() {
        let loader = Loader::new_mock();
        init_tib_chain(&loader);
        // TIB chain is XCPT_CHAIN_END → no handlers
        let fault = super::super::FaultContext {
            eax: 0, ebx: 0, ecx: 0, edx: 0, esi: 0, edi: 0, ebp: 0,
            esp: 0x1000, eip: 0xDEAD, eflags: 2,
            cs: 0x08, ds: 0x10, es: 0x10, fs: 0x18, gs: 0x10, ss: 0x10,
        };
        assert!(loader.try_hw_exception_dispatch(0, fault, 14, 0, 0).is_none());
    }

    #[test]
    fn test_hw_dispatch_with_handler_returns_some() {
        let loader = Loader::new_mock();
        init_tib_chain(&loader);
        let buf = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        write_reg_rec(&loader, buf, XCPT_CHAIN_END, 0x5000);
        loader.dos_set_exception_handler(0, buf);

        let fault = super::super::FaultContext {
            eax: 0, ebx: 0, ecx: 0, edx: 0, esi: 0, edi: 0, ebp: 0,
            esp: 0x1000, eip: 0xDEAD, eflags: 2,
            cs: 0x08, ds: 0x10, es: 0x10, fs: 0x18, gs: 0x10, ss: 0x10,
        };
        let result = loader.try_hw_exception_dispatch(0, fault, 14, 0, 0);
        assert!(result.is_some());
        if let Some(ApiResult::ExceptionDispatch { handler_addr, next_handler, .. }) = result {
            assert_eq!(handler_addr, 0x5000);
            assert_eq!(next_handler, XCPT_CHAIN_END);
        } else {
            panic!("expected ExceptionDispatch");
        }
    }

    // ── write/read CONTEXTRECORD roundtrip ───────────────────────────────────

    #[test]
    fn test_context_record_roundtrip() {
        let loader = Loader::new_mock();
        let addr = loader.shared.mem_mgr.lock_or_recover()
            .alloc(CONTEXT_RECORD_SIZE).unwrap();
        let ctx = super::super::FaultContext {
            eax: 0x1111, ebx: 0x2222, ecx: 0x3333, edx: 0x4444,
            esi: 0x5555, edi: 0x6666, ebp: 0x7777,
            esp: 0x8888, eip: 0x9999, eflags: 0x0202,
            cs: 0x08, ds: 0x10, es: 0x10, fs: 0x18, gs: 0x10, ss: 0x10,
        };
        loader.write_context_record(addr, &ctx);
        let out = loader.read_context_record(addr);
        assert_eq!(out.eax, ctx.eax);
        assert_eq!(out.eip, ctx.eip);
        assert_eq!(out.esp, ctx.esp);
        assert_eq!(out.eflags, ctx.eflags);
        assert_eq!(out.cs, ctx.cs);
    }

    // ── DosUnwindException (ADR-0110) ────────────────────────────────────────

    /// Records rec1 <- rec2 <- rec3 (the head) with handlers 0x1111, 0x2222
    /// and 0x3333.
    fn three_records(loader: &Loader) -> [u32; 3] {
        init_tib_chain(loader);
        let mut mem = loader.shared.mem_mgr.lock_or_recover();
        let recs = [mem.alloc(16).unwrap(), mem.alloc(16).unwrap(), mem.alloc(16).unwrap()];
        drop(mem);
        for (i, &r) in recs.iter().enumerate() {
            write_reg_rec(loader, r, 0, 0x1111 * (i as u32 + 1));
            loader.dos_set_exception_handler(0, r);
        }
        recs
    }

    fn head(loader: &Loader) -> u32 {
        loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap()
    }

    fn started(result: ApiResult) -> Box<super::Unwind> {
        match result {
            ApiResult::Unwind(u) => u,
            other => panic!("expected an unwind, got {other:?}"),
        }
    }

    #[test]
    fn an_unwind_calls_the_newer_handlers_newest_first_and_unlinks_each() {
        let loader = Loader::new_mock();
        let [rec1, rec2, rec3] = three_records(&loader);
        let mut u = started(loader.dos_unwind_exception(0, rec1, 0x4444, 0));
        assert_eq!((u.target, u.target_ip, u.own_report), (rec1, 0x4444, true));
        assert_eq!(loader.guest_read::<u32>(u.report + ERR_NUM), Some(XCPT_UNWIND));
        assert_eq!(loader.guest_read::<u32>(u.report + ERR_FLAGS), Some(EH_UNWINDING));
        assert_eq!(loader.unwind_step(0, &mut u), Some((0x3333, rec3)));
        assert_eq!(head(&loader), rec3, "on the chain while its handler runs");
        assert_eq!(loader.unwind_step(0, &mut u), Some((0x2222, rec2)));
        assert_eq!(head(&loader), rec2);
        assert_eq!(loader.unwind_step(0, &mut u), None);
        assert_eq!(head(&loader), rec1, "the target stays");
        loader.free_exception_records(std::mem::take(&mut u.allocs));
    }

    #[test]
    fn the_whole_chain_is_unwound_for_end_of_chain_and_unwind_all() {
        for target in [XCPT_CHAIN_END, UNWIND_ALL] {
            let loader = Loader::new_mock();
            three_records(&loader);
            let mut u = started(loader.dos_unwind_exception(0, target, 0, 0));
            let calls: Vec<u32> = std::iter::from_fn(|| loader.unwind_step(0, &mut u).map(|c| c.0)).collect();
            assert_eq!(calls, vec![0x3333, 0x2222, 0x1111]);
            assert_eq!(head(&loader), XCPT_CHAIN_END);
        }
    }

    #[test]
    fn a_given_report_gets_eh_unwinding_and_a_stray_target_is_refused() {
        let loader = Loader::new_mock();
        let [rec1, _, rec3] = three_records(&loader);
        let report = loader.shared.mem_mgr.lock_or_recover().alloc(EXCEPTION_REPORT_SIZE).unwrap();
        loader.write_exception_report(report, 0xE000_0001, EH_NONCONTINUABLE, 0, &[]);
        let u = started(loader.dos_unwind_exception(0, rec1, 0, report));
        assert_eq!((u.report, u.own_report), (report, false));
        assert_eq!(loader.guest_read::<u32>(report + ERR_FLAGS), Some(EH_NONCONTINUABLE | EH_UNWINDING));
        assert_eq!(loader.guest_read::<u32>(report + ERR_NUM), Some(0xE000_0001), "its code stays");
        let stray = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        assert!(matches!(loader.dos_unwind_exception(0, stray, 0, 0), ApiResult::Normal(ERROR_INVALID_PARAMETER)));
        assert_eq!(head(&loader), rec3, "nothing unwound");
    }

    #[test]
    fn a_record_without_a_handler_leaves_without_a_call() {
        let loader = Loader::new_mock();
        let [rec1, rec2, _] = three_records(&loader);
        write_reg_rec(&loader, rec2, rec1, 0);
        let mut u = started(loader.dos_unwind_exception(0, rec1, 0, 0));
        assert_eq!(loader.unwind_step(0, &mut u).map(|c| c.0), Some(0x3333));
        assert_eq!(loader.unwind_step(0, &mut u), None, "rec2 has no handler to call");
        assert_eq!(head(&loader), rec1);
    }
}

