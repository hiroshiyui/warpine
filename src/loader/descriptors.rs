// SPDX-License-Identifier: GPL-3.0-only

use super::constants::*;
use super::mutex_ext::MutexExt;
use log::debug;

impl super::Loader {
    /// Read GDT entry base address from guest memory.
    pub(crate) fn gdt_entry_base(&self, selector: u16) -> u32 {
        let gdt_idx = (selector / 8) as u32;
        let entry = self.guest_read::<u64>(GDT_BASE + gdt_idx * 8).unwrap_or(0);
        let base_lo = ((entry >> 16) & 0xFFFF) as u32;
        let base_mid = ((entry >> 32) & 0xFF) as u32;
        let base_hi = ((entry >> 56) & 0xFF) as u32;
        base_lo | (base_mid << 16) | (base_hi << 24)
    }

    /// Read GDT entry limit from guest memory.
    pub(crate) fn gdt_entry_limit(&self, selector: u16) -> u32 {
        let gdt_idx = (selector / 8) as u32;
        let entry = self.guest_read::<u64>(GDT_BASE + gdt_idx * 8).unwrap_or(0);
        let limit_lo = (entry & 0xFFFF) as u32;
        let limit_hi = ((entry >> 48) & 0x0F) as u32;
        limit_lo | (limit_hi << 16)
    }

    pub(crate) fn resolve_import(&self, module: &str, ordinal: u32) -> u64 {
        if module == "DOSCALLS" { MAGIC_API_BASE + ordinal as u64 }
        else if module == "QUECALLS" { MAGIC_API_BASE + 1024 + ordinal as u64 }
        else if module == "PMWIN" { MAGIC_API_BASE + PMWIN_BASE as u64 + ordinal as u64 }
        else if module == "PMGPI" { MAGIC_API_BASE + PMGPI_BASE as u64 + ordinal as u64 }
        else if module == "KBDCALLS" { MAGIC_API_BASE + KBDCALLS_BASE as u64 + ordinal as u64 }
        else if module == "VIOCALLS" { MAGIC_API_BASE + VIOCALLS_BASE as u64 + ordinal as u64 }
        else if module == "SESMGR" { MAGIC_API_BASE + SESMGR_BASE as u64 + ordinal as u64 }
        else if module == "NLS" { MAGIC_API_BASE + NLS_BASE as u64 + ordinal as u64 }
        else if module == "MSG" { MAGIC_API_BASE + MSG_BASE as u64 + ordinal as u64 }
        else if module == "MDM" { MAGIC_API_BASE + MDM_BASE as u64 + ordinal as u64 }
        else {
            // Check loaded user DLLs
            let dll_mgr = self.shared.dll_mgr.lock_or_recover();
            if let Some(addr) = dll_mgr.resolve_ordinal(module, ordinal) {
                return addr as u64;
            }
            drop(dll_mgr);
            log::warn!("Unknown import module: {} ordinal {}", module, ordinal);
            MAGIC_API_BASE + (STUB_AREA_SIZE as u64 - 1)
        }
    }

    pub(crate) fn setup_stubs(&self) {
        for i in 0..STUB_AREA_SIZE {
            self.guest_write::<u8>(MAGIC_API_BASE as u32 + i, 0xCC).expect("setup_stubs: write OOB");
        }
    }

    /// Build a GDT descriptor entry.
    pub(crate) fn make_gdt_entry(base: u32, limit: u32, access: u8, flags: u8) -> u64 {
        let mut entry: u64 = 0;
        entry |= (limit & 0xFFFF) as u64;
        entry |= ((base & 0xFFFF) as u64) << 16;
        entry |= (((base >> 16) & 0xFF) as u64) << 32;
        entry |= (access as u64) << 40;
        entry |= ((((limit >> 16) & 0x0F) as u64) | ((flags as u64) & 0xF0)) << 48;
        entry |= (((base >> 24) & 0xFF) as u64) << 56;
        entry
    }

    /// Set up GDT (with 16-bit tiled segments) and IDT for CPU exception handling.
    ///
    /// GDT layout:
    ///   [0] null, [1] 32-bit code (0x08), [2] 32-bit data (0x10), [3] FS data (0x18),
    ///   [4] 16-bit data alias (0x20, base=0, limit=0xFFFF) — SS for 16-bit stack,
    ///   [5] 16-bit code alias (0x28, base=0, limit=0xFFFF) — CS target for Far16 thunks,
    ///   [6..4101] 16-bit data tiles (0x30..0x8020) — one per 64KB of guest address space.
    ///
    /// The tiled 16-bit descriptors allow OS/2 16:16 (selector:offset) addressing to work
    /// correctly for LSS, JMP FAR, CALL FAR, and other segmented instructions.
    pub(crate) fn setup_idt(&self) {
        const NUM_VECTORS: u32 = 32;

        // ── GDT entries ──

        // Entry 0: null descriptor
        self.guest_write::<u64>(GDT_BASE, 0).unwrap();
        // Entry 1 (selector 0x08): 32-bit code — base=0, limit=4GB, exec/read
        self.guest_write::<u64>(GDT_BASE + 8, Self::make_gdt_entry(0, 0xFFFFF, 0x9B, 0xCF)).unwrap();
        // Entry 2 (selector 0x10): 32-bit data — base=0, limit=4GB, read/write
        self.guest_write::<u64>(GDT_BASE + 16, Self::make_gdt_entry(0, 0xFFFFF, 0x93, 0xCF)).unwrap();
        // Entry 3 (selector 0x18): FS data — base set via sregs
        self.guest_write::<u64>(GDT_BASE + 24, Self::make_gdt_entry(0, 0xFFFFF, 0x93, 0xCF)).unwrap();
        // Entry 4 (selector 0x20): 16-bit data alias — base=0, limit=0xFFFF, byte granular.
        // Used as SS by 16-bit thunk code when it loads a 16-bit stack segment.
        self.guest_write::<u64>(GDT_BASE + 32, Self::make_gdt_entry(0, 0xFFFF, 0x93, 0x00)).unwrap();
        // Entry 5 (selector 0x28): 16-bit code alias — base=0, limit=0xFFFF, byte granular,
        // exec+read. Required by Far16 thunk stubs that execute `JMP FAR 0x0028:xxxx` to switch
        // from 32-bit to 16-bit execution mode (e.g., calls into JPOS2DLL Far16 exports).
        self.guest_write::<u64>(GDT_BASE + 40, Self::make_gdt_entry(0, 0xFFFF, 0x9B, 0x00)).unwrap();

        // Tiled 16-bit read/write data descriptors: GDT[6..4102].
        // Each tile i covers [i*64KB .. (i+1)*64KB), allowing OS/2 16:16 (selector:offset)
        // address arithmetic to work correctly for Far16 thunks (LSS, LES, LDS instructions).
        // Selector for tile i = (TILED_SEL_START_INDEX + i) * 8 = 0x30 + i*8.
        for i in 0..NUM_TILES {
            let base = i * TILE_SIZE;
            // DPL=2 (0xD3) so that OS/2 ring-2 selectors (RPL=2) can be loaded into
            // data segment registers from CPL=0: max(CPL=0,RPL=2)=2 ≤ DPL=2 passes.
            let entry = Self::make_gdt_entry(base, 0xFFFF, 0xD3, 0x00); // 16-bit data, DPL=2
            let entry_addr = GDT_BASE + (TILED_SEL_START_INDEX + i) * 8;
            self.guest_write::<u64>(entry_addr, entry).expect("setup_idt: tile descriptor OOB");
        }
        // Tiled 16-bit execute/read code descriptors: GDT[4102..6150].
        // Same base/limit as data tiles, but with code (exec+read) access.
        // Used by 16:16 far JMP/CALL fixups targeting executable LX objects.
        for i in 0..NUM_CODE_TILES {
            let base = i * TILE_SIZE;
            let entry = Self::make_gdt_entry(base, 0xFFFF, 0x9B, 0x00); // 16-bit code, exec+read
            let entry_addr = GDT_BASE + (TILED_CODE_START_INDEX + i) * 8;
            self.guest_write::<u64>(entry_addr, entry).expect("setup_idt: code tile descriptor OOB");
        }
        debug!("GDT: 6 fixed + {} tiled data + {} tiled code descriptors", NUM_TILES, NUM_CODE_TILES);

        // ── IDT with exception handler stubs ──
        for i in 0..NUM_VECTORS {
            let handler_addr = IDT_HANDLER_BASE + i * 16;  // 16 bytes per handler
            // For exceptions with error codes (#DF=8, #TS=10, #NP=11, #SS=12, #GP=13, #PF=14, #AC=17):
            //   CPU pushes: [error_code] [EIP] [CS] [EFLAGS]
            // For exceptions without error codes:
            //   CPU pushes: [EIP] [CS] [EFLAGS]
            let has_error_code = matches!(i, 8 | 10 | 11 | 12 | 13 | 14 | 17);
            let mut off = 0u32;
            if !has_error_code {
                // PUSH 0 as fake error code to unify stack layout
                self.guest_write::<u8>(handler_addr + off, 0x6A).unwrap(); // PUSH imm8
                self.guest_write::<u8>(handler_addr + off + 1, 0x00).unwrap();
                off += 2;
            }
            // PUSH imm8 <vector number>
            self.guest_write::<u8>(handler_addr + off, 0x6A).unwrap();
            self.guest_write::<u8>(handler_addr + off + 1, i as u8).unwrap();
            off += 2;
            // INT 3
            self.guest_write::<u8>(handler_addr + off, 0xCC).unwrap();

            // IDT entry: 32-bit interrupt gate
            let idt_entry_addr = IDT_BASE + i * 8;
            let offset_lo = (handler_addr & 0xFFFF) as u16;
            let offset_hi = ((handler_addr >> 16) & 0xFFFF) as u16;
            self.guest_write::<u16>(idt_entry_addr, offset_lo).unwrap();
            self.guest_write::<u16>(idt_entry_addr + 2, 0x08).unwrap(); // code selector
            self.guest_write::<u16>(idt_entry_addr + 4, 0x8E00).unwrap(); // P=1, DPL=0, 32-bit int gate
            self.guest_write::<u16>(idt_entry_addr + 6, offset_hi).unwrap();
        }
    }
}
