// SPDX-License-Identifier: GPL-3.0-or-later

use super::constants::*;
use super::mutex_ext::MutexExt;
use log::debug;

/// A present 32-bit TSS, busy (Intel SDM vol. 3, 3.5): TR is loaded with
/// it directly, never with `LTR`.
const GDT_ACCESS_BUSY_TSS: u8 = 0x8B;
/// 32-bit interrupt gates, present: DPL 0, and DPL 3 for the breakpoint.
const IDT_GATE_DPL0: u16 = 0x8E00;
const IDT_GATE_DPL3: u16 = 0xEE00;
/// The breakpoint exception (`INT3`), which API thunks raise.
const BREAKPOINT_VECTOR: u32 = 3;

/// The PMWIN and PMGPI calls Warpine dispatches, `(name, ordinal)` sorted by
/// name; build.rs reads them from the sub-dispatchers' arms.
mod pm_names {
    include!(concat!(env!("OUT_DIR"), "/pm_dispatch_names.rs"));
}

/// The dispatched calls of a sub-dispatched PM module, or None for any other.
pub(super) fn pm_dispatched(module: &str) -> Option<&'static [(&'static str, u32)]> {
    match module {
        "PMWIN" => Some(pm_names::PMWIN_NAMES),
        "PMGPI" => Some(pm_names::PMGPI_NAMES),
        _ => None,
    }
}

/// The ordinal of a PMWIN or PMGPI call Warpine dispatches, found by name
/// (case-insensitively, as names of programs' own DLLs are).
pub(crate) fn pm_ordinal_by_name(module: &str, name: &str) -> Option<u32> {
    pm_dispatched(module)?.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, o)| o)
}

/// Whether `ordinal` of a PMWIN or PMGPI is dispatched; None for any other module.
pub(crate) fn pm_ordinal_is_dispatched(module: &str, ordinal: u32) -> Option<bool> {
    pm_dispatched(module).map(|t| t.iter().any(|&(_, o)| o == ordinal))
}

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

    /// Resolve an import by procedure name rather than ordinal.
    ///
    /// Used for `FixupTarget::ExternalName` fixups where the linker emits a
    /// name-based fixup instead of an ordinal (e.g. a program that imports
    /// `VioCheckCharType` by name, which `os2386.lib` has only as
    /// `VIO16CHECKCHARTYPE`, ordinal 75).
    ///
    /// Returns 0 if the name cannot be resolved.
    pub(crate) fn resolve_import_by_name(&self, module: &str, proc_name: &str) -> u64 {
        // VIOCALLS name → ordinal table (OS/2 Warp 4 / Warp Server ordinals).
        static VIOCALLS_NAMES: &[(&str, u32)] = &[
            ("VioGetAnsi",        3),
            ("VioSetAnsi",        5),
            ("VioScrollUp",       7),
            ("VioScrollDn",      47),
            ("VioGetCurPos",      9),
            ("VioWrtCellStr",    10),
            ("VioSetCurPos",     15),
            ("VioWrtTTY",        19),
            ("VioGetMode",       21),
            ("VioSetMode",       22),
            ("VioReadCellStr",   24),
            ("VioWrtNAttr",      26),
            ("VioGetBuf",        31),
            ("VioSetCurType",    32),
            ("VioGetCurType",    27),
            ("VioCheckCharType", 75),
            ("VioSetCp",         42),
            ("VioShowBuf",       43),
            ("VioGetConfig",     46),  // was incorrectly 6 (VIO16DEREGISTER); real ord = 46
            ("VioWrtCharStrAtt", 48),
            ("VioSetState",      51),
            ("VioWrtNCell",      52),
        ];
        // KBDCALLS name → ordinal table.
        static KBDCALLS_NAMES: &[(&str, u32)] = &[
            ("KbdCharIn",      4),   // was incorrectly 10 (KbdGetStatus); real ord = 4
            ("KbdStringIn",    9),
            ("KbdGetStatus",  10),
            ("KbdFlushBuffer", 6),
        ];
        // MOUCALLS name → ordinal table (Open Watcom os2386.lib MOU16* exports).
        static MOUCALLS_NAMES: &[(&str, u32)] = &[
            ("MouGetPtrShape",    1),
            ("MouSetPtrShape",    2),
            ("MouGetNumMickeys",  3),
            ("MouGetScaleFact",   6),
            ("MouFlushQue",       7),
            ("MouGetNumButtons",  8),
            ("MouClose",          9),
            ("MouSetScaleFact",  11),
            ("MouGetNumQueEl",   13),
            ("MouGetEventMask",  15),
            ("MouSetEventMask",  16),
            ("MouOpen",          17),
            ("MouRemovePtr",     18),
            ("MouGetPtrPos",     19),
            ("MouReadEventQue",  20),
            ("MouSetPtrPos",     21),
            ("MouGetDevStatus",  22),
            ("MouSynch",         23),
            ("MouSetDevStatus",  25),
            ("MouDrawPtr",       26),
        ];

        let ordinal = match module {
            "VIOCALLS" => {
                VIOCALLS_NAMES.iter().find(|(n, _)| *n == proc_name).map(|(_, o)| *o)
            }
            "KBDCALLS" => {
                KBDCALLS_NAMES.iter().find(|(n, _)| *n == proc_name).map(|(_, o)| *o)
            }
            "MOUCALLS" => {
                MOUCALLS_NAMES.iter().find(|(n, _)| *n == proc_name).map(|(_, o)| *o)
            }
            "PMWIN" | "PMGPI" => pm_ordinal_by_name(module, proc_name),
            _ => {
                // For registry-based modules (DOSCALLS, PMSHAPI, etc.) search api_registry.
                super::api_registry::find_by_module_and_name(module, proc_name)
            }
        };

        if let Some(ord) = ordinal {
            log::debug!("resolve_import_by_name: {}.{} → ordinal {}", module, proc_name, ord);
            self.resolve_import(module, ord)
        } else {
            log::warn!("resolve_import_by_name: {}.{} not found", module, proc_name);
            0
        }
    }

    pub(crate) fn resolve_import(&self, module: &str, ordinal: u32) -> u64 {
        // SOM.DLL exports data too: their addresses, not thunks (som.rs).
        if module == "SOM" && let Some(at) = self.som_data_address(ordinal) {
            return at as u64;
        }
        if let Some(base) = module_base(module) { MAGIC_API_BASE + base as u64 + ordinal as u64 }
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
    ///   [6..4101] 16-bit data tiles (0x30..0x8020) — one per 64KB of guest address space,
    ///   [4102..6149] 16-bit code tiles,
    ///   [6150..6152] ring-0 code, ring-0 data and the TSS (ADR-0113).
    ///
    /// The tiled 16-bit descriptors allow OS/2 16:16 (selector:offset) addressing to work
    /// correctly for LSS, JMP FAR, CALL FAR, and other segmented instructions.
    pub(crate) fn setup_idt(&self) {
        use super::ring::access_byte;
        const NUM_VECTORS: u32 = 32;
        // At ring 3 (ADR-0113) the descriptors programs use are DPL 3, and
        // the gates enter ring 0 through ring 0's own code descriptor.
        let ring3 = self.guest_ring() == 3;
        let user_dpl = if ring3 { 3 } else { 0 };
        let (user_code, user_data) = (access_byte(user_dpl, true), access_byte(user_dpl, false));
        let gate_cs = if ring3 { KERNEL_CODE_SEL } else { 0x08 };

        // ── GDT entries ──

        // Entry 0: null descriptor
        self.guest_write::<u64>(GDT_BASE, 0).unwrap();
        // Entry 1 (selector 0x08): 32-bit code — base=0, limit=4GB, exec/read
        self.guest_write::<u64>(GDT_BASE + 8, Self::make_gdt_entry(0, 0xFFFFF, user_code, 0xCF)).unwrap();
        // Entry 2 (selector 0x10): 32-bit data — base=0, limit=4GB, read/write
        self.guest_write::<u64>(GDT_BASE + 16, Self::make_gdt_entry(0, 0xFFFFF, user_data, 0xCF)).unwrap();
        // Entry 3 (selector 0x18): FS data — the FS window, which each
        // thread's tables map to its own TIB (ADR-0110): reloading FS keeps
        // the thread's TIB, as OS/2's per-thread TIB selector does.
        self.guest_write::<u64>(GDT_BASE + 24, Self::make_gdt_entry(FS_WINDOW, PAGE_SIZE - 1, user_data, 0x40)).unwrap();
        // Entry 4 (selector 0x20): 16-bit data alias — base=0, limit=0xFFFF, byte granular.
        // Used as SS by 16-bit thunk code when it loads a 16-bit stack segment.
        self.guest_write::<u64>(GDT_BASE + 32, Self::make_gdt_entry(0, 0xFFFF, user_data, 0x00)).unwrap();
        // Entry 5 (selector 0x28): 16-bit code alias — base=0, limit=0xFFFF, byte granular,
        // exec+read. Required by Far16 thunk stubs that execute `JMP FAR 0x0028:xxxx` to switch
        // from 32-bit to 16-bit execution mode (e.g., calls into JPOS2DLL Far16 exports).
        self.guest_write::<u64>(GDT_BASE + 40, Self::make_gdt_entry(0, 0xFFFF, user_code, 0x00)).unwrap();

        // Tiled 16-bit read/write data descriptors: GDT[6..4102].
        // Each tile i covers [i*64KB .. (i+1)*64KB), allowing OS/2 16:16 (selector:offset)
        // address arithmetic to work correctly for Far16 thunks (LSS, LES, LDS instructions).
        // Selector for tile i = (TILED_SEL_START_INDEX + i) * 8 = 0x30 + i*8.
        for i in 0..NUM_TILES {
            let base = i * TILE_SIZE;
            // DPL=2 (0xD3) so that OS/2 ring-2 selectors (RPL=2) can be loaded into
            // data segment registers from CPL=0: max(CPL=0,RPL=2)=2 ≤ DPL=2 passes.
            // At ring 3, DPL 3: RPL 2 and 3 selectors both load.
            let access = if ring3 { user_data } else { 0xD3 };
            let entry = Self::make_gdt_entry(base, 0xFFFF, access, 0x00); // 16-bit data
            let entry_addr = GDT_BASE + (TILED_SEL_START_INDEX + i) * 8;
            self.guest_write::<u64>(entry_addr, entry).expect("setup_idt: tile descriptor OOB");
        }
        // Tiled 16-bit execute/read code descriptors: GDT[4102..6150].
        // Same base/limit as data tiles, but with code (exec+read) access.
        // Used by 16:16 far JMP/CALL fixups targeting executable LX objects.
        for i in 0..NUM_CODE_TILES {
            let base = i * TILE_SIZE;
            let entry = Self::make_gdt_entry(base, 0xFFFF, user_code, 0x00); // 16-bit code, exec+read
            let entry_addr = GDT_BASE + (TILED_CODE_START_INDEX + i) * 8;
            self.guest_write::<u64>(entry_addr, entry).expect("setup_idt: code tile descriptor OOB");
        }
        // Ring-0 code and data, and the TSS (ADR-0113): the descriptors an
        // exception from ring 3 enters through. Each thread's tables map
        // the TSS's base, the ring-0 window, to the thread's own page.
        let ring0 = [
            (KERNEL_CODE_SEL, Self::make_gdt_entry(0, 0xFFFFF, 0x9B, 0xCF)),
            (KERNEL_DATA_SEL, Self::make_gdt_entry(0, 0xFFFFF, 0x93, 0xCF)),
            (TSS_SEL, Self::make_gdt_entry(RING0_WINDOW, TSS_SIZE - 1, GDT_ACCESS_BUSY_TSS, 0x00)),
        ];
        for (sel, entry) in ring0 {
            self.guest_write::<u64>(GDT_BASE + u32::from(sel), entry).expect("setup_idt: ring-0 descriptor OOB");
        }
        debug!("GDT: 6 fixed + {} tiled data + {} tiled code + 3 ring-0 descriptors", NUM_TILES, NUM_CODE_TILES);

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
            self.guest_write::<u16>(idt_entry_addr + 2, gate_cs).unwrap(); // code selector
            // `INT3` checks its gate's DPL like `INT n` (Intel SDM vol. 3,
            // 6.12.1.1): the breakpoint gate is DPL 3, so that API thunks
            // keep working from ring 3; a guest's other `INT n` faults.
            let attrs = if i == BREAKPOINT_VECTOR { IDT_GATE_DPL3 } else { IDT_GATE_DPL0 };
            self.guest_write::<u16>(idt_entry_addr + 4, attrs).unwrap();
            self.guest_write::<u16>(idt_entry_addr + 6, offset_hi).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pm_calls_are_found_by_name_in_any_case() {
        assert_eq!(pm_ordinal_by_name("PMWIN", "WinCreateWindow"), Some(909));
        assert_eq!(pm_ordinal_by_name("PMWIN", "WINCREATEWINDOW"), Some(909));
        assert_eq!(pm_ordinal_by_name("PMGPI", "GpiBox"), Some(356));
        assert_eq!(pm_ordinal_by_name("PMWIN", "GpiBox"), None, "each module has its own table");
        assert_eq!(pm_ordinal_by_name("PMWIN", "WinNoSuchCall"), None);
        assert_eq!(pm_ordinal_by_name("DOSCALLS", "DosOpen"), None, "registry modules are not in these tables");
    }

    #[test]
    fn only_dispatched_pm_ordinals_are_known() {
        assert_eq!(pm_ordinal_is_dispatched("PMWIN", 909), Some(true));
        assert_eq!(pm_ordinal_is_dispatched("PMWIN", 0), Some(false));
        assert_eq!(pm_ordinal_is_dispatched("PMGPI", 356), Some(true));
        assert_eq!(pm_ordinal_is_dispatched("VIOCALLS", 3), None);
    }

    #[test]
    fn each_dispatched_pm_call_has_one_name_and_one_ordinal() {
        for module in ["PMWIN", "PMGPI"] {
            let table = pm_dispatched(module).expect("a PM table");
            let names: std::collections::HashSet<_> = table.iter().map(|(n, _)| n.to_ascii_uppercase()).collect();
            let ords: std::collections::HashSet<_> = table.iter().map(|&(_, o)| o).collect();
            assert_eq!(names.len(), table.len(), "{module}: a name dispatched at two ordinals");
            assert_eq!(ords.len(), table.len(), "{module}: an ordinal under two names");
        }
    }
}
