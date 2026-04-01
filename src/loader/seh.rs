// SPDX-License-Identifier: GPL-3.0-only
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
//   • In vcpu.rs the CALLBACK_RET_TRAP handler processes FrameKind::ExceptionHandler:
//       XCPT_CONTINUE_EXECUTION  → restore FaultContext, resume
//       XCPT_CONTINUE_SEARCH    → invoke next handler in chain (or crash)

use super::constants::*;
use super::{ApiResult, FaultContext};
use super::mutex_ext::MutexExt;
use log::{debug, warn};

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
        // Zero the FPU env/save area (offsets 0x04–0x8B).
        for off in (0x04..0x8Cu32).step_by(4) {
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
        fault: FaultContext,
        vector: u32,
        error_code: u32,
    ) -> Option<ApiResult> {
        let xcpt_code = xcpt_code_for_vector(vector, error_code);
        let flags = if xcpt_code == XCPT_ACCESS_VIOLATION || xcpt_code == XCPT_FATAL_EXCEPTION {
            EH_NONCONTINUABLE
        } else {
            0
        };

        // Access violation passes the faulting address as a parameter.
        let params: &[u32] = if vector == 14 { &[0, fault.eip] } else { &[] };

        let chain_head = self.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET)
            .unwrap_or(XCPT_CHAIN_END);
        if chain_head == XCPT_CHAIN_END {
            return None; // No handlers registered
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

        debug!("SEH: dispatching vector={} xcpt=0x{:08X} to handler=0x{:08X} (reg=0x{:08X})",
               vector, xcpt_code, handler_fn, chain_head);

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

    /// Walk the exception handler chain starting at `reg_rec`, calling each handler
    /// with `EH_UNWINDING` set.  Used by `DosUnwindException`.
    ///
    /// Stops when `reg_rec == stop_at` (exclusive; if 0 walk entire chain).
    /// After walking, restores the stack to the target frame by setting
    /// `TIB_EXCHAIN` to `stop_at`.
    pub(crate) fn dos_raise_exception(&self, p_exc_rec: u32) -> ApiResult {
        debug!("  DosRaiseException(pExcRec=0x{:08X})", p_exc_rec);
        if p_exc_rec == 0 {
            return ApiResult::Normal(ERROR_INVALID_PARAMETER);
        }

        // Read the exception code from the caller-provided record.
        let xcpt_code = self.guest_read::<u32>(p_exc_rec + ERR_NUM).unwrap_or(XCPT_FATAL_EXCEPTION);
        let _flags    = self.guest_read::<u32>(p_exc_rec + ERR_FLAGS).unwrap_or(0);
        let exc_addr  = self.guest_read::<u32>(p_exc_rec + ERR_ADDR).unwrap_or(0);

        let chain_head = self.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET)
            .unwrap_or(XCPT_CHAIN_END);
        if chain_head == XCPT_CHAIN_END {
            warn!("  DosRaiseException: no handlers registered, xcpt=0x{:08X}", xcpt_code);
            return ApiResult::Normal(ERROR_NESTING_NOT_ALLOWED); // no handlers
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
            cs: 0x08, ds: 0x10, es: 0x10, fs: 0x18, gs: 0x10, ss: 0x10,
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

    /// `DosUnwindException` — walk the exception handler chain with EH_UNWINDING,
    /// stopping at `p_reg_rec` (or the chain end if 0), then restore execution
    /// at the target by popping TIB_EXCHAIN to `p_reg_rec`.
    ///
    /// The full unwind protocol is complex (involves calling each handler with
    /// EH_UNWINDING so it can perform cleanup).  This implementation performs
    /// the TIB chain truncation synchronously and delegates the handler-call
    /// side to the caller's compiler-generated unwind tables (Watcom uses
    /// setjmp/longjmp internally so this is usually sufficient).
    pub(crate) fn dos_unwind_exception(
        &self,
        p_reg_rec: u32,
        _reserved: u32,
        p_exc_rec: u32,
        _data: u32,
    ) -> u32 {
        debug!("  DosUnwindException(pRegRec=0x{:08X}, pExcRec=0x{:08X})", p_reg_rec, p_exc_rec);

        // Walk the chain calling handlers with EH_UNWINDING until we reach the target.
        let mut cur = self.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET)
            .unwrap_or(XCPT_CHAIN_END);

        while cur != XCPT_CHAIN_END && cur != p_reg_rec {
            debug!("  DosUnwindException: skipping handler at 0x{:08X}", cur);
            // NOTE: a full implementation would invoke each handler with EH_UNWINDING.
            // For Watcom Open Watcom apps that use __try/__finally via setjmp this
            // is handled by the C runtime before calling DosUnwindException, so the
            // skip-and-truncate approach is sufficient for the common case.
            cur = self.guest_read::<u32>(cur + XERREC_PREV).unwrap_or(XCPT_CHAIN_END);
        }

        // Truncate TIB chain to the target record.
        self.guest_write::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET, p_reg_rec).unwrap();
        NO_ERROR
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

        assert_eq!(loader.dos_set_exception_handler(buf), NO_ERROR);

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

        loader.dos_set_exception_handler(buf);
        assert_eq!(loader.dos_unset_exception_handler(buf), NO_ERROR);

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

        loader.dos_set_exception_handler(rec1);
        loader.dos_set_exception_handler(rec2);

        // Chain: TIB → rec2 → rec1 → XCPT_CHAIN_END
        assert_eq!(loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap(), rec2);
        assert_eq!(loader.guest_read::<u32>(rec2 + XERREC_PREV).unwrap(), rec1);
        assert_eq!(loader.guest_read::<u32>(rec1 + XERREC_PREV).unwrap(), XCPT_CHAIN_END);
    }

    #[test]
    fn test_set_handler_null_returns_error() {
        let loader = Loader::new_mock();
        assert_ne!(loader.dos_set_exception_handler(0), NO_ERROR);
    }

    #[test]
    fn test_unset_handler_null_returns_error() {
        let loader = Loader::new_mock();
        assert_ne!(loader.dos_unset_exception_handler(0), NO_ERROR);
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
        assert!(loader.try_hw_exception_dispatch(fault, 14, 0).is_none());
    }

    #[test]
    fn test_hw_dispatch_with_handler_returns_some() {
        let loader = Loader::new_mock();
        init_tib_chain(&loader);
        let buf = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        write_reg_rec(&loader, buf, XCPT_CHAIN_END, 0x5000);
        loader.dos_set_exception_handler(buf);

        let fault = super::super::FaultContext {
            eax: 0, ebx: 0, ecx: 0, edx: 0, esi: 0, edi: 0, ebp: 0,
            esp: 0x1000, eip: 0xDEAD, eflags: 2,
            cs: 0x08, ds: 0x10, es: 0x10, fs: 0x18, gs: 0x10, ss: 0x10,
        };
        let result = loader.try_hw_exception_dispatch(fault, 14, 0);
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

    // ── DosUnwindException ───────────────────────────────────────────────────

    #[test]
    fn test_dos_unwind_truncates_chain() {
        let loader = Loader::new_mock();
        init_tib_chain(&loader);
        let mut mem = loader.shared.mem_mgr.lock_or_recover();
        let rec1 = mem.alloc(16).unwrap();
        let rec2 = mem.alloc(16).unwrap();
        drop(mem);
        write_reg_rec(&loader, rec1, XCPT_CHAIN_END, 0x1111);
        write_reg_rec(&loader, rec2, 0, 0x2222);
        loader.dos_set_exception_handler(rec1);
        loader.dos_set_exception_handler(rec2);

        // Unwind to rec1 — chain should be truncated to rec1.
        assert_eq!(loader.dos_unwind_exception(rec1, 0, 0, 0), NO_ERROR);
        assert_eq!(
            loader.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET).unwrap(),
            rec1
        );
    }
}

