// SPDX-License-Identifier: GPL-3.0-or-later
//
//! The privilege level guests run at: ring 3 (ADR-0113).
//!
//! OS/2 runs applications at ring 3. Warpine's guests ran at ring 0, which
//! let them load CR3 or CR0 and rewrite their own page tables. Every
//! thread of a program, 32-bit or 16-bit, now runs paged at ring 3:
//! - the descriptors programs use (0x08, 0x10, 0x18, the 16-bit aliases and
//!   tiles) are DPL 3, and their selectors carry RPL 3 (0x0B, 0x13, 0x1B);
//! - the IDT's gates enter ring 0 through `KERNEL_CODE_SEL`, on the ring-0
//!   stack each thread's TSS names (`RING0_WINDOW`);
//! - an exception's frame, which the processor pushes on that ring-0
//!   stack, is moved to the guest's own stack before Warpine handles it
//!   ([`super::Loader::lower_ring3_frame`]), so everything after sees what
//!   it saw at ring 0;
//! - a `#GP` from an instruction only ring 0 may execute is
//!   `XCPT_PRIVILEGED_INSTRUCTION`, as on OS/2 ([`is_privileged_instruction`]);
//! - EFLAGS Warpine takes from a context record or a saved context cannot
//!   raise the I/O privilege level or set IF ([`guest_eflags`]).
//!
//! 16-bit (NE) programs too (`ne_exec.rs`). The pages a ring-3 guest must
//! not reach are supervisor-only (`paging.rs`), and Warpine does not write
//! them on a guest's behalf (`guest_mem::protected`).

use super::constants::*;
use super::vm_backend::{GuestSegment, VcpuBackend};

/// The ring every guest thread runs at, and the RPL of its selectors.
pub const USER_RING: u8 = 3;

/// A selector as a guest thread holds it: with RPL 3 (0x08 is 0x0B).
pub const fn user_selector(selector: u16) -> u16 {
    selector | USER_RING as u16
}

/// Descriptor access bytes (Intel SDM vol. 3, 3.4.5): present, the
/// descriptor's DPL, code or data, and its type.
pub const fn access_byte(dpl: u8, code: bool) -> u8 {
    0x80 | (dpl << 5) | 0x10 | if code { 0x0B } else { 0x03 }
}

/// The EFLAGS bits a guest may set through a context record or a saved
/// context: the arithmetic flags, TF, DF, OF, AC and ID. IOPL, IF, NT, RF,
/// VM, VIF and VIP stay as Warpine runs guests (all clear); bit 1 is
/// always set.
pub fn guest_eflags(eflags: u32) -> u64 {
    const CF: u32 = 1 << 0;
    const RESERVED_ONE: u32 = 1 << 1;
    const PF: u32 = 1 << 2;
    const AF: u32 = 1 << 4;
    const ZF: u32 = 1 << 6;
    const SF: u32 = 1 << 7;
    const TF: u32 = 1 << 8;
    const DF: u32 = 1 << 10;
    const OF: u32 = 1 << 11;
    const AC: u32 = 1 << 18;
    const ID: u32 = 1 << 21;
    const ALLOWED: u32 = CF | PF | AF | ZF | SF | TF | DF | OF | AC | ID;
    u64::from((eflags & ALLOWED) | RESERVED_ONE)
}

/// Whether `code` (the bytes at a faulting EIP) starts with an instruction
/// only ring 0 (or a sufficient IOPL) may execute: `HLT`, `CLI`, `STI`,
/// port I/O, moves to and from control and debug registers, `CLTS`,
/// `INVD`, `WBINVD`, `WRMSR`, `RDMSR`, `LGDT` / `LIDT` / `LMSW` /
/// `INVLPG`, `LLDT` / `LTR`. Prefixes (operand size, address size,
/// segment, `REP`) are skipped. A guest at ring 3 executing one gets a #GP
/// with error code 0.
pub fn is_privileged_instruction(code: &[u8]) -> bool {
    const PREFIXES: [u8; 10] = [0x66, 0x67, 0x2E, 0x36, 0x3E, 0x26, 0x64, 0x65, 0xF2, 0xF3];
    let start = code.iter().take(4).take_while(|b| PREFIXES.contains(b)).count();
    let Some(&op) = code.get(start) else { return false };
    let modrm_reg = |at: usize| code.get(at).map(|m| (m >> 3) & 7);
    match op {
        0xF4 | 0xFA | 0xFB => true,                     // HLT, CLI, STI
        0xE4..=0xE7 | 0xEC..=0xEF | 0x6C..=0x6F => true, // IN, OUT, INS, OUTS
        0x0F => match code.get(start + 1) {
            Some(0x06 | 0x08 | 0x09 | 0x30 | 0x32) => true,  // CLTS, INVD, WBINVD, WRMSR, RDMSR
            Some(0x20..=0x23) => true,                       // MOV to / from CRn, DRn
            Some(0x00) => matches!(modrm_reg(start + 2), Some(2 | 3)), // LLDT, LTR
            Some(0x01) => matches!(modrm_reg(start + 2), Some(2 | 3 | 6 | 7)), // LGDT, LIDT, LMSW, INVLPG
            _ => false,
        },
        _ => false,
    }
}

/// The CPU's frame on the ring-0 stack for an exception from ring 3, as
/// Warpine's stubs leave it (from the stack pointer up): the vector, the
/// error code (or the stub's 0), EIP, CS, EFLAGS, ESP, SS.
const RING3_FRAME_DWORDS: u32 = 7;
/// What stays of it, moved to the guest's stack: the first five dwords,
/// as an exception at ring 0 leaves them.
const RING0_FRAME_DWORDS: u32 = 5;

impl super::Loader {
    /// The segment the GDT descriptor of `selector` describes, as the
    /// processor would load it; None for a selector outside the GDT or a
    /// descriptor that is not present.
    pub(crate) fn gdt_segment(&self, selector: u16) -> Option<GuestSegment> {
        let index = u32::from(selector) / 8;
        if index == 0 || index >= GDT_ENTRY_COUNT || selector & 4 != 0 {
            return None;
        }
        let d = self.guest_read::<u64>(GDT_BASE + index * 8)?;
        let access = (d >> 40) as u8;
        let flags = (d >> 52) as u8;
        let raw_limit = (d & 0xFFFF) as u32 | (((d >> 48) & 0xF) as u32) << 16;
        let granular = flags & 0x8 != 0;
        let segment = GuestSegment {
            base: (((d >> 16) & 0xFF_FFFF) | (((d >> 56) & 0xFF) << 24)),
            limit: if granular { (raw_limit << 12) | 0xFFF } else { raw_limit },
            selector,
            type_: access & 0xF,
            present: (access >> 7) & 1,
            dpl: (access >> 5) & 3,
            db: (flags >> 2) & 1,
            s: (access >> 4) & 1,
            l: 0,
            g: u8::from(granular),
            avl: flags & 1,
            unusable: 0,
        };
        (segment.present == 1).then_some(segment)
    }

    /// At an exception stub (ring 0), move the frame of an exception from
    /// ring 3 from the thread's ring-0 stack to the guest's own stack, and
    /// put the vCPU back at ring 3 with that stack: the state an exception
    /// at ring 0 leaves, which the rest of the exception handling reads.
    ///
    /// Ok(false): no ring-3 frame (the thread runs at ring 0). Err: the
    /// frame cannot be read or the guest's stack cannot take it, as a
    /// fault at ring 0 would have ended in a triple fault.
    pub(crate) fn lower_ring3_frame(&self, vcpu: &mut dyn VcpuBackend) -> Result<bool, String> {
        let mut regs = vcpu.get_regs()?;
        let rsp = regs.rsp as u32;
        let frame_len = RING3_FRAME_DWORDS * 4;
        // The frame lies wholly in the ring-0 window, which this thread's
        // tables map to its ring-0 page (the page after its TLM page).
        let inside = rsp.checked_sub(RING0_WINDOW).filter(|&o| o + frame_len <= PAGE_SIZE);
        let Some(offset) = inside else { return Ok(false) };
        let ring0_page = super::paging::ring0_page_of_tlm(super::guest_mem::thread_tlm_backing())
            .ok_or_else(|| "an exception on the ring-0 stack of a thread without a ring-0 page".to_string())?;
        let at = ring0_page + offset;
        let mut frame = [0u32; RING3_FRAME_DWORDS as usize];
        for (i, word) in frame.iter_mut().enumerate() {
            *word = self.guest_read::<u32>(at + 4 * i as u32).ok_or_else(|| "the ring-0 stack cannot be read".to_string())?;
        }
        let [_vector, _error, _eip, cs, _eflags, esp, ss] = frame;
        if cs & 3 != 3 {
            return Ok(false);
        }
        let code = self.gdt_segment(cs as u16).ok_or_else(|| format!("CS 0x{cs:04X} of the exception is no segment"))?;
        let stack = self.gdt_segment(ss as u16).ok_or_else(|| format!("SS 0x{ss:04X} of the exception is no segment"))?;
        // A 16-bit stack (a tile's) addresses SS.base + SP.
        let (new_esp, linear) = if stack.db == 0 {
            let sp = (esp as u16).wrapping_sub((RING0_FRAME_DWORDS * 4) as u16);
            ((esp & 0xFFFF_0000) | u32::from(sp), (stack.base as u32).wrapping_add(u32::from(sp)))
        } else {
            let sp = esp.wrapping_sub(RING0_FRAME_DWORDS * 4);
            (sp, (stack.base as u32).wrapping_add(sp))
        };
        for (i, &word) in frame[..RING0_FRAME_DWORDS as usize].iter().enumerate() {
            self.guest_write::<u32>(linear.wrapping_add(4 * i as u32), word)
                .ok_or_else(|| format!("the guest's stack at 0x{linear:08X} cannot take the exception's frame"))?;
        }
        regs.rsp = u64::from(new_esp);
        vcpu.set_regs(&regs)?;
        let mut sregs = vcpu.get_sregs()?;
        sregs.cs = code;
        sregs.ss = stack;
        vcpu.set_sregs(&sregs)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::vm_backend::{GuestRegs, GuestSregs};

    #[test]
    fn guest_eflags_keep_only_what_a_program_may_set() {
        assert_eq!(guest_eflags(0), 0x2);
        assert_eq!(guest_eflags(0x3000 | 0x200 | 0x4000 | 0x1_0000 | 0x2_0000 | 0x18_0000), 0x2,
                   "IOPL, IF, NT, RF, VM, VIF, VIP");
        assert_eq!(guest_eflags(0x0000_0CD5), 0x0000_0CD7, "CF PF AF ZF SF DF OF (and bit 1)");
        assert_eq!(guest_eflags(0x0004_0100), 0x0004_0102, "AC, TF");
        assert_eq!(guest_eflags(0x0020_0000), 0x0020_0002, "ID");
    }

    #[test]
    fn privileged_instructions_are_recognised() {
        let yes: [&[u8]; 16] = [
            &[0xF4], &[0xFA], &[0xFB], &[0xE4, 0x80], &[0xEE], &[0x66, 0xEF], &[0xF3, 0x6C],
            &[0x0F, 0x22, 0xD8], &[0x0F, 0x20, 0xC0], &[0x0F, 0x23, 0xC0], &[0x0F, 0x06],
            &[0x0F, 0x09], &[0x0F, 0x30], &[0x0F, 0x01, 0x15, 0, 0, 0, 0], &[0x0F, 0x00, 0xD8],
            &[0x0F, 0x01, 0x38],
        ];
        for c in yes {
            assert!(is_privileged_instruction(c), "{c:02X?}");
        }
        let no: [&[u8]; 9] = [
            &[], &[0x90], &[0xCC], &[0xCD, 0x21], &[0x0F, 0x01, 0x05, 0, 0, 0, 0] /* SGDT */,
            &[0x0F, 0x00, 0xC8] /* STR */, &[0x8B, 0xC0], &[0x0F, 0xA2] /* CPUID */, &[0x0F, 0x31] /* RDTSC */,
        ];
        for c in no {
            assert!(!is_privileged_instruction(c), "{c:02X?}");
        }
    }

    #[test]
    fn access_bytes_and_selectors() {
        assert_eq!(access_byte(0, true), 0x9B);
        assert_eq!(access_byte(0, false), 0x93);
        assert_eq!(access_byte(3, true), 0xFB);
        assert_eq!(access_byte(3, false), 0xF3);
        assert_eq!(user_selector(0x10), 0x13);
        assert_eq!(user_selector(0x0B), 0x0B, "already RPL 3");
    }

    /// A GDT descriptor decodes to the segment the processor loads.
    #[test]
    fn gdt_descriptors_decode() {
        let loader = super::super::Loader::new_mock();
        loader.setup_idt();
        let cs = loader.gdt_segment(0x0B).expect("code");
        assert_eq!((cs.base, cs.limit, cs.type_, cs.dpl, cs.db, cs.s, cs.g, cs.present), (0, 0xFFFF_FFFF, 0xB, 3, 1, 1, 1, 1));
        let fs = loader.gdt_segment(0x1B).expect("FS");
        assert_eq!((fs.base, fs.limit, fs.dpl, fs.g), (u64::from(FS_WINDOW), PAGE_SIZE - 1, 3, 0));
        let tile = loader.gdt_segment((TILED_SEL_START + 8) as u16 | 3).expect("a tile");
        assert_eq!((tile.base, tile.limit, tile.db, tile.dpl), (u64::from(TILE_SIZE), 0xFFFF, 0, 3));
        assert!(loader.gdt_segment(0).is_none(), "the null selector");
        assert!(loader.gdt_segment(0x0F).is_none(), "an LDT selector");
        assert!(loader.gdt_segment((GDT_ENTRY_COUNT * 8) as u16).is_none(), "past the GDT");
    }

    /// An exception from ring 3: the frame on the ring-0 stack moves to
    /// the guest's stack as ring 0 would have left it, and the vCPU is at
    /// ring 3 on that stack again.
    #[test]
    fn a_ring3_frame_moves_to_the_guests_stack() {
        let loader = super::super::Loader::new_mock();
        loader.setup_idt();
        let block = loader.alloc_paging_block(TIB_BASE).expect("a block");
        super::super::guest_mem::set_thread_tlm_backing(super::super::paging::tlm_page_of(u64::from(block)).unwrap());
        let ring0 = super::super::paging::ring0_page_of_tlm(super::super::guest_mem::thread_tlm_backing()).unwrap();
        let r0_sp = RING0_STACK_TOP - 4 * RING3_FRAME_DWORDS;
        let guest_esp = 0x0003_0000;
        let frame = [14, 6, 0x0001_2345, 0x0B, 0x246, guest_esp, 0x13];
        for (i, w) in frame.iter().enumerate() {
            loader.system_write::<u32>(ring0 + (r0_sp - RING0_WINDOW) + 4 * i as u32, *w).unwrap();
        }
        let mut vcpu = MockVcpu::new();
        vcpu.regs = GuestRegs { rsp: u64::from(r0_sp), ..Default::default() };
        vcpu.sregs = GuestSregs::default();
        assert_eq!(loader.lower_ring3_frame(&mut vcpu), Ok(true));
        let regs = vcpu.get_regs().unwrap();
        assert_eq!(regs.rsp, u64::from(guest_esp - 20));
        for (i, w) in frame[..5].iter().enumerate() {
            assert_eq!(loader.guest_read::<u32>(guest_esp - 20 + 4 * i as u32), Some(*w), "dword {i}");
        }
        let s = vcpu.get_sregs().unwrap();
        assert_eq!((s.cs.selector, s.cs.dpl, s.ss.selector, s.ss.dpl), (0x0B, 3, 0x13, 3));
        // A stack pointer outside the ring-0 window: no ring-3 frame.
        let mut vcpu = MockVcpu::new();
        vcpu.regs = GuestRegs { rsp: u64::from(guest_esp), ..Default::default() };
        assert_eq!(loader.lower_ring3_frame(&mut vcpu), Ok(false));
        // A guest stack that cannot take the frame.
        let mut vcpu = MockVcpu::new();
        loader.system_write::<u32>(ring0 + (r0_sp - RING0_WINDOW) + 20, 0xFFFF_FF00).unwrap();
        vcpu.regs = GuestRegs { rsp: u64::from(r0_sp), ..Default::default() };
        assert!(loader.lower_ring3_frame(&mut vcpu).is_err());
        super::super::guest_mem::set_thread_tlm_backing(0);
    }
}
