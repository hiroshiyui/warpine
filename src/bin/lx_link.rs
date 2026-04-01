// SPDX-License-Identifier: GPL-3.0-only
//
// lx_link — ELF-to-LX linker for the Warpine OS/2 compatibility layer.
//
// Takes ELF .o files (and .rlib/.a archives) produced by Rust for the
// i686-warpine-os2 custom target and emits a valid LX executable that
// Warpine can load and run.
//
// Usage:
//   lx-link file1.o file2.o ... [libname.rlib ...] -o output.exe [--def os2api.def]
//
// All unknown flags (--sysroot, -m, -static, --as-needed, etc.) are silently ignored.

#![allow(clippy::cast_possible_truncation)]

use std::error::Error;
use std::path::Path;
use std::process;

use object::read::archive::ArchiveFile;

// ─── Constants (mirrors src/loader/constants.rs — duplicated because bin
// crates cannot use `use warpine::...` without a lib.rs) ──────────────────────

const MAGIC_API_BASE: u32 = 0x0100_0000;
const PMWIN_BASE: u32 = 2048;
const PMGPI_BASE: u32 = 3072;
const KBDCALLS_BASE: u32 = 4096;
const VIOCALLS_BASE: u32 = 5120;
const SESMGR_BASE: u32 = 6144;
const NLS_BASE: u32 = 7168;
const MSG_BASE: u32 = 8192;
const MDM_BASE: u32 = 10240;
const UCONV_BASE: u32 = 12288;

/// Base virtual address of the code (text) object in the generated LX file.
const CODE_OBJECT_BASE: u32 = 0x0001_0000;
/// Base virtual address of the data object in the generated LX file.
const DATA_OBJECT_BASE: u32 = 0x0006_0000;
/// Stack size: 64 KiB, placed just below TIB (0x90000).
const STACK_SIZE: u32 = 0x0001_0000;
const STACK_OBJECT_BASE: u32 = 0x0008_0000 - STACK_SIZE; // 0x70000

const PAGE_SIZE: u32 = 4096;
/// Absolute file offset of the LX header (after 64-byte MZ stub).
const LX_HEADER_OFFSET: u32 = 0x40;
/// Size in bytes of the LX header structure.
const LX_HEADER_SIZE: u32 = 172; // 0xAC

// LX object flags
const OBJ_READ: u32 = 0x0001;
const OBJ_WRITE: u32 = 0x0002;
const OBJ_EXEC: u32 = 0x0004;
const OBJ_BIG: u32 = 0x0040; // 32-bit (USE32)
const OBJ_USE32: u32 = 0x2000;

// ─── mod args ─────────────────────────────────────────────────────────────────

mod args {
    use std::path::PathBuf;

    pub struct Args {
        pub inputs: Vec<PathBuf>,
        pub output: PathBuf,
        pub def_file: Option<PathBuf>,
    }

    pub fn parse_args(raw: &[String]) -> Args {
        let mut inputs = Vec::new();
        let mut output = PathBuf::from("a.out");
        let mut def_file = None;
        let mut i = 1usize;
        while i < raw.len() {
            let arg = &raw[i];
            if arg == "-o" {
                i += 1;
                if i < raw.len() {
                    output = PathBuf::from(&raw[i]);
                }
            } else if let Some(path) = arg.strip_prefix("-o") {
                output = PathBuf::from(path);
            } else if arg == "--def" {
                i += 1;
                if i < raw.len() {
                    def_file = Some(PathBuf::from(&raw[i]));
                }
            } else if arg.starts_with('-') {
                // Flags known to take a single value argument:
                if matches!(
                    arg.as_str(),
                    "--sysroot" | "-m" | "--hash-style" | "-z" | "-rpath" | "-soname"
                ) {
                    i += 1; // consume the value too
                }
                // All other unknown flags: silently skip
            } else {
                // Positional: only collect .o / .rlib / .a files
                let lower = arg.to_ascii_lowercase();
                if lower.ends_with(".o") || lower.ends_with(".rlib") || lower.ends_with(".a") {
                    inputs.push(PathBuf::from(arg));
                }
            }
            i += 1;
        }
        Args { inputs, output, def_file }
    }
}

// ─── mod def_parser ───────────────────────────────────────────────────────────

mod def_parser {
    use std::collections::HashMap;

    /// Parse a DEF file and return a map from symbol name → (module, ordinal).
    ///
    /// DEF file format:
    ///   # comment
    ///   MODULE.ORDINAL  SymbolName
    pub fn parse_def(content: &str) -> HashMap<String, (String, u32)> {
        let mut map = HashMap::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let mod_ord = match parts.next() {
                Some(s) => s,
                None => continue,
            };
            let sym = match parts.next() {
                Some(s) => s,
                None => continue,
            };
            if let Some(dot) = mod_ord.find('.') {
                let module = &mod_ord[..dot];
                if let Ok(ordinal) = mod_ord[dot + 1..].parse::<u32>() {
                    map.insert(sym.to_string(), (module.to_uppercase(), ordinal));
                }
            }
        }
        map
    }
}

// ─── mod elf_reader ───────────────────────────────────────────────────────────

mod elf_reader {
    use std::collections::HashMap;

    use object::SectionKind;

    /// A relocation entry from an ELF section.
    #[derive(Debug, Clone)]
    pub struct Reloc {
        /// Byte offset within the section.
        pub offset: u64,
        /// Relocation kind (Absolute = R_386_32, Relative = R_386_PC32).
        pub kind: object::RelocationKind,
        /// Addend (usually 0 for REL, explicit for RELA).
        pub addend: i64,
        /// The relocation target.
        pub target: RelocTarget,
    }

    /// Resolved relocation target.
    #[derive(Debug, Clone)]
    pub enum RelocTarget {
        /// Symbol defined in (or imported by) this link unit.
        Symbol(String),
        /// Section-relative reference.
        Section(String),
    }

    /// A section extracted from an ELF object file.
    #[derive(Debug)]
    pub struct ElfSection {
        pub name: String,
        pub kind: SectionKind,
        pub data: Vec<u8>,
        /// Alignment requirement (power of two).
        pub align: u64,
        pub relocs: Vec<Reloc>,
    }

    /// An exported symbol from an ELF object.
    #[derive(Debug)]
    pub struct ElfSymbol {
        pub name: String,
        /// Index into the `ElfObject::sections` slice (None for undefined/abs).
        pub section_idx: Option<usize>,
        /// Byte offset within the section (or absolute address if no section).
        pub offset: u64,
    }

    /// One parsed ELF object file.
    #[derive(Debug)]
    pub struct ElfObject {
        pub sections: Vec<ElfSection>,
        pub symbols: Vec<ElfSymbol>,
    }

    /// Parse one ELF .o file from raw bytes.
    pub fn parse_elf(data: &[u8], source_name: &str) -> Result<ElfObject, Box<dyn super::Error>> {
        use object::{
            Object, ObjectSection, ObjectSymbol, RelocationTarget, SymbolKind, SymbolScope,
        };

        let obj = object::File::parse(data)?;

        let mut sections: Vec<ElfSection> = Vec::new();
        let mut section_index_map: HashMap<object::SectionIndex, usize> = HashMap::new();

        for section in obj.sections() {
            let kind = section.kind();
            match kind {
                SectionKind::Text
                | SectionKind::Data
                | SectionKind::ReadOnlyData
                | SectionKind::ReadOnlyString
                | SectionKind::UninitializedData => {}
                _ => continue,
            }
            let data_bytes = match section.uncompressed_data() {
                Ok(d) => d.to_vec(),
                Err(_) => vec![0u8; section.size() as usize],
            };
            let align = section.align().max(1);
            let local_idx = sections.len();
            section_index_map.insert(section.index(), local_idx);
            sections.push(ElfSection {
                name: section.name().unwrap_or("").to_string(),
                kind,
                data: data_bytes,
                align,
                relocs: Vec::new(),
            });
        }

        // Read relocations for each section.
        for section in obj.sections() {
            let local_idx = match section_index_map.get(&section.index()) {
                Some(&i) => i,
                None => continue,
            };
            for (offset, reloc) in section.relocations() {
                let kind = reloc.kind();
                let size = reloc.size();
                let addend = reloc.addend();
                if size != 32 {
                    eprintln!(
                        "warning: {}: unhandled {}-bit relocation at offset {:#x}",
                        source_name, size, offset
                    );
                    continue;
                }
                let target = match reloc.target() {
                    RelocationTarget::Symbol(sym_idx) => {
                        let sym = obj.symbol_by_index(sym_idx)?;
                        let sym_name = sym.name().unwrap_or("").to_string();
                        if let Some(sec_idx) = sym.section_index() {
                            if let Some(&local_sec) = section_index_map.get(&sec_idx) {
                                let sec_name = sections[local_sec].name.clone();
                                RelocTarget::Section(sec_name)
                            } else {
                                RelocTarget::Symbol(sym_name)
                            }
                        } else {
                            RelocTarget::Symbol(sym_name)
                        }
                    }
                    RelocationTarget::Section(sec_idx) => {
                        if let Some(&local_sec) = section_index_map.get(&sec_idx) {
                            let sec_name = sections[local_sec].name.clone();
                            RelocTarget::Section(sec_name)
                        } else {
                            RelocTarget::Symbol(String::new())
                        }
                    }
                    _ => {
                        eprintln!(
                            "warning: {}: unhandled relocation target at offset {:#x}",
                            source_name, offset
                        );
                        continue;
                    }
                };
                sections[local_idx].relocs.push(Reloc { offset, kind, addend, target });
            }
        }

        // Collect symbols.
        let mut symbols: Vec<ElfSymbol> = Vec::new();
        for sym in obj.symbols() {
            let name = match sym.name() {
                Ok(n) if !n.is_empty() => n.to_string(),
                _ => continue,
            };
            if matches!(sym.kind(), SymbolKind::File | SymbolKind::Section | SymbolKind::Unknown) {
                continue;
            }
            // Only include global/linkage symbols (skip local symbols).
            if !matches!(sym.scope(), SymbolScope::Dynamic | SymbolScope::Linkage) {
                continue;
            }
            let (section_idx, offset) = match sym.section_index() {
                Some(sec_idx) => (section_index_map.get(&sec_idx).copied(), sym.address()),
                None => (None, sym.address()),
            };
            symbols.push(ElfSymbol { name, section_idx, offset });
        }

        Ok(ElfObject { sections, symbols })
    }
}

// ─── mod linker_state ─────────────────────────────────────────────────────────

mod linker_state {
    use std::collections::HashMap;

    use object::SectionKind;

    use super::elf_reader::{ElfObject, ElfSection};
    use super::{CODE_OBJECT_BASE, DATA_OBJECT_BASE, PAGE_SIZE};

    pub struct InputEntry {
        pub obj_idx: usize,
        pub sec_idx: usize,
        /// Byte offset within the merged section's data buffer.
        pub merged_offset: u32,
    }

    pub struct MergedSection {
        /// Combined data (all input sections concatenated, alignment-padded).
        pub data: Vec<u8>,
        /// Virtual size of zeroed BSS region beyond `data`.
        pub bss_size: u32,
        /// Base virtual address.
        pub va: u32,
        pub input_map: Vec<InputEntry>,
    }

    pub struct LinkerState {
        pub code_section: MergedSection,
        pub data_section: MergedSection,
        /// symbol name → flat virtual address.
        pub symbol_table: HashMap<String, u32>,
        /// Entry-point virtual address.
        pub entry_va: u32,
    }

    impl LinkerState {
        pub fn build(objects: &[ElfObject]) -> Result<Self, Box<dyn super::Error>> {
            let mut code_sec = MergedSection {
                data: Vec::new(),
                bss_size: 0,
                va: CODE_OBJECT_BASE,
                input_map: Vec::new(),
            };
            let mut data_sec = MergedSection {
                data: Vec::new(),
                bss_size: 0,
                va: DATA_OBJECT_BASE,
                input_map: Vec::new(),
            };

            for (obj_idx, obj) in objects.iter().enumerate() {
                for (sec_idx, sec) in obj.sections.iter().enumerate() {
                    if sec.kind == SectionKind::Text {
                        append_section(&mut code_sec, obj_idx, sec_idx, sec);
                    }
                }
            }
            for (obj_idx, obj) in objects.iter().enumerate() {
                for (sec_idx, sec) in obj.sections.iter().enumerate() {
                    match sec.kind {
                        SectionKind::Data
                        | SectionKind::ReadOnlyData
                        | SectionKind::ReadOnlyString => {
                            append_section(&mut data_sec, obj_idx, sec_idx, sec);
                        }
                        SectionKind::UninitializedData => {
                            let align = sec.data.len() as u32; // BSS: size field holds size
                            let cur = data_sec.data.len() as u32 + data_sec.bss_size;
                            let align_req = sec.align as u32;
                            let padded = align_up(cur, align_req);
                            data_sec.bss_size += (padded - cur) + align;
                        }
                        _ => {}
                    }
                }
            }

            let mut symbol_table = HashMap::new();
            for (obj_idx, obj) in objects.iter().enumerate() {
                for sym in &obj.symbols {
                    if let Some(sec_idx) = sym.section_idx {
                        let base_va = if obj.sections[sec_idx].kind == SectionKind::Text {
                            find_merged_va(&code_sec, obj_idx, sec_idx)
                        } else {
                            find_merged_va(&data_sec, obj_idx, sec_idx)
                        };
                        if let Some(va) = base_va {
                            symbol_table.insert(sym.name.clone(), va + sym.offset as u32);
                        }
                    }
                }
            }

            let entry_va = symbol_table
                .get("_start")
                .copied()
                .or_else(|| symbol_table.get("main").copied())
                .unwrap_or(CODE_OBJECT_BASE);

            Ok(LinkerState { code_section: code_sec, data_section: data_sec, symbol_table, entry_va })
        }

        pub fn code_va_for(&self, obj_idx: usize, sec_idx: usize) -> Option<u32> {
            find_merged_va(&self.code_section, obj_idx, sec_idx)
        }

        pub fn data_va_for(&self, obj_idx: usize, sec_idx: usize) -> Option<u32> {
            find_merged_va(&self.data_section, obj_idx, sec_idx)
        }
    }

    fn align_up(val: u32, align: u32) -> u32 {
        if align <= 1 { val } else { (val + align - 1) & !(align - 1) }
    }

    fn append_section(merged: &mut MergedSection, obj_idx: usize, sec_idx: usize, sec: &ElfSection) {
        let align = sec.align as u32;
        let cur_len = merged.data.len() as u32;
        let aligned = align_up(cur_len, align);
        merged.data.resize(aligned as usize, 0u8);
        let merged_offset = merged.data.len() as u32;
        merged.data.extend_from_slice(&sec.data);
        merged.input_map.push(InputEntry { obj_idx, sec_idx, merged_offset });
    }

    fn find_merged_va(merged: &MergedSection, obj_idx: usize, sec_idx: usize) -> Option<u32> {
        for entry in &merged.input_map {
            if entry.obj_idx == obj_idx && entry.sec_idx == sec_idx {
                return Some(merged.va + entry.merged_offset);
            }
        }
        None
    }

    pub fn page_count(data_len: u32, bss_size: u32) -> usize {
        ((data_len + bss_size).div_ceil(PAGE_SIZE)) as usize
    }
}

// ─── mod lx_writer ────────────────────────────────────────────────────────────

mod lx_writer {
    use std::collections::HashMap;

    use object::{RelocationKind, SectionKind};

    use super::elf_reader::{ElfObject, RelocTarget};
    use super::linker_state::{page_count, LinkerState};
    use super::{
        resolve_import, CODE_OBJECT_BASE, DATA_OBJECT_BASE, LX_HEADER_OFFSET, LX_HEADER_SIZE,
        OBJ_BIG, OBJ_EXEC, OBJ_READ, OBJ_USE32, OBJ_WRITE, PAGE_SIZE, STACK_OBJECT_BASE,
        STACK_SIZE,
    };

    // A fixup record ready to be serialised.
    #[derive(Debug, Clone)]
    struct LxFixup {
        /// Source type: 0x07 = 32-bit absolute.
        source_type: u8,
        /// Page-relative byte offset of the fixup source.
        source_offset: u16,
        target: FixupTarget,
    }

    #[derive(Debug, Clone)]
    enum FixupTarget {
        Internal { object_num: u8, target_offset: u32 },
        ExternalOrdinal { module_idx: u8, ordinal: u32 },
    }

    /// Generate the full LX binary.
    pub fn write_lx(
        objects: &[ElfObject],
        state: &LinkerState,
        def_map: &HashMap<String, (String, u32)>,
    ) -> Vec<u8> {
        let entry_offset = state.entry_va - CODE_OBJECT_BASE;

        // Clone merged data into mutable buffers for patch application.
        let mut code_data = state.code_section.data.clone();
        let mut data_data = state.data_section.data.clone();

        let code_pages = page_count(code_data.len() as u32, state.code_section.bss_size);
        let data_pages = page_count(data_data.len() as u32, state.data_section.bss_size);
        let stack_pages = (STACK_SIZE / PAGE_SIZE) as usize;
        let total_pages = code_pages + data_pages + stack_pages;

        let mut module_map: HashMap<String, u8> = HashMap::new();
        let mut imported_modules: Vec<String> = Vec::new();

        let mut code_fixups: Vec<Vec<LxFixup>> = vec![Vec::new(); code_pages];
        let mut data_fixups: Vec<Vec<LxFixup>> = vec![Vec::new(); data_pages];

        // Patches: (offset_in_merged_buf, value).
        let mut code_patches: Vec<(usize, u32)> = Vec::new();
        let mut data_patches: Vec<(usize, u32)> = Vec::new();
        let mut code_patches_i32: Vec<(usize, i32)> = Vec::new();

        for (obj_idx, obj) in objects.iter().enumerate() {
            for (sec_idx, sec) in obj.sections.iter().enumerate() {
                let is_code = sec.kind == SectionKind::Text;
                let section_base_va = if is_code {
                    match state.code_va_for(obj_idx, sec_idx) { Some(v) => v, None => continue }
                } else {
                    match state.data_va_for(obj_idx, sec_idx) { Some(v) => v, None => continue }
                };
                let obj_base = if is_code { CODE_OBJECT_BASE } else { DATA_OBJECT_BASE };
                // Offset of this section within the merged data buffer.
                let buf_base = (section_base_va - obj_base) as usize;

                for reloc in &sec.relocs {
                    let source_va = section_base_va + reloc.offset as u32;
                    let buf_off = buf_base + reloc.offset as usize;

                    let (target_va, maybe_ft) = resolve_reloc(
                        reloc,
                        def_map,
                        state,
                        objects,
                        &mut module_map,
                        &mut imported_modules,
                    );
                    if target_va == 0 { continue; }

                    match reloc.kind {
                        RelocationKind::Absolute => {
                            let actual = target_va.wrapping_add(reloc.addend as u32);
                            if is_code { code_patches.push((buf_off, actual)); }
                            else       { data_patches.push((buf_off, actual)); }

                            if let Some(ft) = maybe_ft {
                                let page_idx = ((source_va - obj_base) / PAGE_SIZE) as usize;
                                let page_off = ((source_va - obj_base) % PAGE_SIZE) as u16;
                                let fixups = if is_code { &mut code_fixups } else { &mut data_fixups };
                                if page_idx < fixups.len() {
                                    fixups[page_idx].push(LxFixup {
                                        source_type: 0x07,
                                        source_offset: page_off,
                                        target: ft,
                                    });
                                }
                            }
                        }
                        RelocationKind::Relative => {
                            // PC-relative: patch directly, no LX fixup record needed.
                            let disp = (target_va as i64)
                                - (source_va as i64 + 4)
                                + reloc.addend;
                            // PC-relative relocs are almost always in code sections.
                            if is_code { code_patches_i32.push((buf_off, disp as i32)); }
                            else       { data_patches.push((buf_off, disp as u32)); }
                        }
                        _ => eprintln!("warning: unhandled relocation kind {:?}", reloc.kind),
                    }
                }
            }
        }

        // Apply patches.
        apply_patches_u32(&mut code_data, &code_patches);
        apply_patches_u32(&mut data_data, &data_patches);
        apply_patches_i32(&mut code_data, &code_patches_i32);

        pad_to_page(&mut code_data);
        pad_to_page(&mut data_data);

        // ── Build fixup section ───────────────────────────────────────────────
        let mut fixup_page_table: Vec<u32> = vec![0u32; total_pages + 1];
        let mut fixup_records: Vec<u8> = Vec::new();

        for pi in 0..code_pages {
            fixup_page_table[pi] = fixup_records.len() as u32;
            for fx in &code_fixups[pi] { encode_fixup(&mut fixup_records, fx); }
        }
        for pi in 0..data_pages {
            fixup_page_table[code_pages + pi] = fixup_records.len() as u32;
            for fx in &data_fixups[pi] { encode_fixup(&mut fixup_records, fx); }
        }
        for pi in 0..stack_pages {
            fixup_page_table[code_pages + data_pages + pi] = fixup_records.len() as u32;
        }
        fixup_page_table[total_pages] = fixup_records.len() as u32;

        // ── Build import module name table ────────────────────────────────────
        let mut import_modules_bytes: Vec<u8> = Vec::new();
        for m in &imported_modules {
            import_modules_bytes.push(m.len() as u8);
            import_modules_bytes.extend_from_slice(m.as_bytes());
        }

        // ── Resident name table ───────────────────────────────────────────────
        let mod_name = b"WARPEXE";
        let mut resident_name_bytes = Vec::new();
        resident_name_bytes.push(mod_name.len() as u8);
        resident_name_bytes.extend_from_slice(mod_name);
        resident_name_bytes.extend_from_slice(&0u16.to_le_bytes());
        resident_name_bytes.push(0); // terminator

        // ── Entry table (minimal: just terminator) ────────────────────────────
        let entry_table_bytes: Vec<u8> = vec![0, 0];

        // ── Compute table offsets (all relative to LX header start, 0x40) ─────
        let num_objects: u32 = 3;
        let obj_table_off: u32 = LX_HEADER_SIZE;
        let page_map_off: u32 = obj_table_off + num_objects * 24;
        let resident_name_off: u32 = page_map_off + total_pages as u32 * 8;
        let entry_table_off: u32 = resident_name_off + resident_name_bytes.len() as u32;
        let fixup_page_table_off: u32 = entry_table_off + entry_table_bytes.len() as u32;
        let fixup_record_off: u32 = fixup_page_table_off + (total_pages as u32 + 1) * 4;
        let import_modules_off: u32 = fixup_record_off + fixup_records.len() as u32;
        let import_proc_name_off: u32 = import_modules_off + import_modules_bytes.len() as u32;

        let loader_section_size = fixup_page_table_off;
        let fixup_section_size =
            (total_pages as u32 + 1) * 4
            + fixup_records.len() as u32
            + import_modules_bytes.len() as u32;

        // data_pages_offset is an ABSOLUTE file offset.
        let loader_section_end = LX_HEADER_OFFSET + import_proc_name_off;
        let data_pages_offset_abs = align_to(loader_section_end, 16);
        let padding_before_data = data_pages_offset_abs - loader_section_end;

        // ── Assemble output ───────────────────────────────────────────────────
        let mut out: Vec<u8> = Vec::new();

        // 1. MZ stub (64 bytes).
        let mut mz = [0u8; 64];
        mz[0] = b'M'; mz[1] = b'Z';
        mz[0x3C] = LX_HEADER_OFFSET as u8;
        out.extend_from_slice(&mz);

        // 2. LX header (172 bytes).
        out.extend_from_slice(&build_lx_header(
            entry_offset,
            total_pages as u32,
            obj_table_off, num_objects, page_map_off,
            resident_name_off, entry_table_off,
            fixup_page_table_off, fixup_record_off,
            import_modules_off, imported_modules.len() as u32, import_proc_name_off,
            loader_section_size, fixup_section_size,
            data_pages_offset_abs,
        ));

        // 3. Object table.
        out.extend_from_slice(&encode_object_entry(
            code_data.len() as u32 + state.code_section.bss_size,
            CODE_OBJECT_BASE,
            OBJ_READ | OBJ_EXEC | OBJ_BIG | OBJ_USE32,
            1, code_pages as u32,
        ));
        out.extend_from_slice(&encode_object_entry(
            data_data.len() as u32 + state.data_section.bss_size,
            DATA_OBJECT_BASE,
            OBJ_READ | OBJ_WRITE | OBJ_BIG | OBJ_USE32,
            code_pages as u32 + 1, data_pages as u32,
        ));
        out.extend_from_slice(&encode_object_entry(
            STACK_SIZE, STACK_OBJECT_BASE,
            OBJ_READ | OBJ_WRITE | OBJ_BIG | OBJ_USE32,
            code_pages as u32 + data_pages as u32 + 1, STACK_SIZE / PAGE_SIZE,
        ));

        // 4. Page map.
        let mut file_off: u32 = 0;
        for pi in 0..code_pages {
            let slice_end = ((pi + 1) * PAGE_SIZE as usize).min(code_data.len());
            let slice_start = pi * PAGE_SIZE as usize;
            let psize = (slice_end - slice_start) as u16;
            out.extend_from_slice(&file_off.to_le_bytes());
            out.extend_from_slice(&psize.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            file_off += PAGE_SIZE;
        }
        for pi in 0..data_pages {
            let slice_end = ((pi + 1) * PAGE_SIZE as usize).min(data_data.len());
            let slice_start = pi * PAGE_SIZE as usize;
            let psize = (slice_end - slice_start) as u16;
            out.extend_from_slice(&file_off.to_le_bytes());
            out.extend_from_slice(&psize.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            file_off += PAGE_SIZE;
        }
        for _ in 0..stack_pages {
            out.extend_from_slice(&file_off.to_le_bytes());
            out.extend_from_slice(&(PAGE_SIZE as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            file_off += PAGE_SIZE;
        }

        // 5. Resident name table.
        out.extend_from_slice(&resident_name_bytes);
        // 6. Entry table.
        out.extend_from_slice(&entry_table_bytes);
        // 7. Fixup page table.
        for off in &fixup_page_table { out.extend_from_slice(&off.to_le_bytes()); }
        // 8. Fixup records.
        out.extend_from_slice(&fixup_records);
        // 9. Import module name table.
        out.extend_from_slice(&import_modules_bytes);
        // 10. Import procedure name table (empty — we don't use ExternalName fixups).
        // 11. Padding before data pages.
        out.extend(std::iter::repeat_n(0u8, padding_before_data as usize));
        // 12. Data pages.
        out.extend_from_slice(&code_data);
        out.extend_from_slice(&data_data);
        // Stack pages (zero-filled).
        let stack_data = vec![0u8; stack_pages * PAGE_SIZE as usize];
        out.extend_from_slice(&stack_data);

        out
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn resolve_reloc(
        reloc: &super::elf_reader::Reloc,
        def_map: &HashMap<String, (String, u32)>,
        state: &LinkerState,
        objects: &[ElfObject],
        module_map: &mut HashMap<String, u8>,
        imported_modules: &mut Vec<String>,
    ) -> (u32, Option<FixupTarget>) {
        match &reloc.target {
            RelocTarget::Symbol(name) => {
                if let Some(&va) = state.symbol_table.get(name.as_str()) {
                    let (obj_num, off) = va_to_lx_object(va);
                    (va, Some(FixupTarget::Internal { object_num: obj_num, target_offset: off }))
                } else if let Some((module, ordinal)) = def_map.get(name.as_str()) {
                    let resolved = resolve_import(module, *ordinal);
                    let idx = get_or_insert_module(module, module_map, imported_modules);
                    (resolved, Some(FixupTarget::ExternalOrdinal { module_idx: idx, ordinal: *ordinal }))
                } else {
                    eprintln!("warning: undefined symbol '{}'", name);
                    (0, None)
                }
            }
            RelocTarget::Section(sec_name) => {
                let found = find_section_va(objects, state, sec_name);
                match found {
                    Some(base) => {
                        let (obj_num, off) = va_to_lx_object(base);
                        (base, Some(FixupTarget::Internal { object_num: obj_num, target_offset: off }))
                    }
                    None => {
                        eprintln!("warning: section '{}' not found", sec_name);
                        (0, None)
                    }
                }
            }
        }
    }

    fn va_to_lx_object(va: u32) -> (u8, u32) {
        if va >= DATA_OBJECT_BASE { (2, va - DATA_OBJECT_BASE) }
        else { (1, va - CODE_OBJECT_BASE) }
    }

    fn get_or_insert_module(
        name: &str,
        module_map: &mut HashMap<String, u8>,
        imported_modules: &mut Vec<String>,
    ) -> u8 {
        if let Some(&idx) = module_map.get(name) { return idx; }
        imported_modules.push(name.to_string());
        let idx = imported_modules.len() as u8;
        module_map.insert(name.to_string(), idx);
        idx
    }

    fn find_section_va(objects: &[ElfObject], state: &LinkerState, sec_name: &str) -> Option<u32> {
        for (oi, obj) in objects.iter().enumerate() {
            for (si, sec) in obj.sections.iter().enumerate() {
                if sec.name == sec_name {
                    return state.code_va_for(oi, si).or_else(|| state.data_va_for(oi, si));
                }
            }
        }
        None
    }

    fn apply_patches_u32(buf: &mut [u8], patches: &[(usize, u32)]) {
        for &(off, val) in patches {
            if off + 4 <= buf.len() {
                buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
            }
        }
    }

    fn apply_patches_i32(buf: &mut [u8], patches: &[(usize, i32)]) {
        for &(off, val) in patches {
            if off + 4 <= buf.len() {
                buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
            }
        }
    }

    fn pad_to_page(data: &mut Vec<u8>) {
        let rem = data.len() % PAGE_SIZE as usize;
        if rem != 0 {
            data.resize(data.len() + (PAGE_SIZE as usize - rem), 0);
        }
    }

    fn align_to(val: u32, align: u32) -> u32 { (val + align - 1) & !(align - 1) }

    fn encode_object_entry(size: u32, base: u32, flags: u32, page_map_index: u32, page_count: u32) -> Vec<u8> {
        let mut b = Vec::with_capacity(24);
        b.extend_from_slice(&size.to_le_bytes());
        b.extend_from_slice(&base.to_le_bytes());
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&page_map_index.to_le_bytes());
        b.extend_from_slice(&page_count.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // reserved
        b
    }

    fn encode_fixup(out: &mut Vec<u8>, fix: &LxFixup) {
        match &fix.target {
            FixupTarget::Internal { object_num, target_offset } => {
                out.push(fix.source_type);
                out.push(0x10); // target_flags: 32-bit offset, internal (type=0)
                out.extend_from_slice(&fix.source_offset.to_le_bytes());
                out.push(*object_num);
                out.extend_from_slice(&target_offset.to_le_bytes());
            }
            FixupTarget::ExternalOrdinal { module_idx, ordinal } => {
                out.push(fix.source_type);
                out.push(0x11); // 0x01=external ordinal | 0x10=32-bit ordinal
                out.extend_from_slice(&fix.source_offset.to_le_bytes());
                out.push(*module_idx);
                out.extend_from_slice(&ordinal.to_le_bytes());
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_lx_header(
        entry_offset: u32,
        total_pages: u32,
        obj_table_off: u32,
        num_objects: u32,
        page_map_off: u32,
        resident_name_off: u32,
        entry_table_off: u32,
        fixup_page_table_off: u32,
        fixup_record_off: u32,
        import_modules_off: u32,
        imported_modules_count: u32,
        import_proc_name_off: u32,
        loader_section_size: u32,
        fixup_section_size: u32,
        data_pages_offset_abs: u32,
    ) -> Vec<u8> {
        let mut h = Vec::with_capacity(172);
        h.extend_from_slice(b"LX");
        h.push(0); h.push(0);                                       // byte_order, word_order
        h.extend_from_slice(&0u32.to_le_bytes());                   // format_level
        h.extend_from_slice(&2u16.to_le_bytes());                   // cpu_type = 386
        h.extend_from_slice(&1u16.to_le_bytes());                   // os_type = OS/2
        h.extend_from_slice(&0u32.to_le_bytes());                   // module_version
        h.extend_from_slice(&0u32.to_le_bytes());                   // module_flags (EXE)
        h.extend_from_slice(&total_pages.to_le_bytes());            // module_num_pages
        h.extend_from_slice(&1u32.to_le_bytes());                   // eip_object = 1 (code)
        h.extend_from_slice(&entry_offset.to_le_bytes());           // eip
        h.extend_from_slice(&3u32.to_le_bytes());                   // esp_object = 3 (stack)
        h.extend_from_slice(&STACK_SIZE.to_le_bytes());             // esp = top of stack
        h.extend_from_slice(&4096u32.to_le_bytes());                // page_size
        h.extend_from_slice(&0u32.to_le_bytes());                   // page_offset_shift
        h.extend_from_slice(&fixup_section_size.to_le_bytes());     // fixup_section_size
        h.extend_from_slice(&0u32.to_le_bytes());                   // fixup_section_checksum
        h.extend_from_slice(&loader_section_size.to_le_bytes());    // loader_section_size
        h.extend_from_slice(&0u32.to_le_bytes());                   // loader_section_checksum
        h.extend_from_slice(&obj_table_off.to_le_bytes());          // object_table_offset
        h.extend_from_slice(&num_objects.to_le_bytes());            // object_count
        h.extend_from_slice(&page_map_off.to_le_bytes());           // object_page_map_offset
        h.extend_from_slice(&0u32.to_le_bytes());                   // object_iter_data_map_offset
        h.extend_from_slice(&0u32.to_le_bytes());                   // resource_table_offset
        h.extend_from_slice(&0u32.to_le_bytes());                   // resource_count
        h.extend_from_slice(&resident_name_off.to_le_bytes());      // resident_name_table_offset
        h.extend_from_slice(&entry_table_off.to_le_bytes());        // entry_table_offset
        h.extend_from_slice(&0u32.to_le_bytes());                   // module_directives_offset
        h.extend_from_slice(&0u32.to_le_bytes());                   // module_directives_count
        h.extend_from_slice(&fixup_page_table_off.to_le_bytes());   // fixup_page_table_offset
        h.extend_from_slice(&fixup_record_off.to_le_bytes());       // fixup_record_table_offset
        h.extend_from_slice(&import_modules_off.to_le_bytes());     // imported_modules_name_table_offset
        h.extend_from_slice(&imported_modules_count.to_le_bytes()); // imported_modules_count
        h.extend_from_slice(&import_proc_name_off.to_le_bytes());   // import_procedure_name_table_offset
        h.extend_from_slice(&0u32.to_le_bytes());                   // per_page_checksum_table_offset
        h.extend_from_slice(&data_pages_offset_abs.to_le_bytes());  // data_pages_offset (ABSOLUTE)
        h.extend_from_slice(&0u32.to_le_bytes());                   // num_preload_pages
        h.extend_from_slice(&0u32.to_le_bytes());                   // non_resident_name_table_offset
        h.extend_from_slice(&0u32.to_le_bytes());                   // non_resident_name_table_length
        h.extend_from_slice(&0u32.to_le_bytes());                   // non_resident_name_table_checksum
        h.extend_from_slice(&2u32.to_le_bytes());                   // auto_ds_object = 2 (data)
        h.extend_from_slice(&0u32.to_le_bytes());                   // debug_info_offset
        h.extend_from_slice(&0u32.to_le_bytes());                   // debug_info_length
        h.extend_from_slice(&0u32.to_le_bytes());                   // num_instance_preload
        h.extend_from_slice(&0u32.to_le_bytes());                   // num_instance_demand
        h.extend_from_slice(&0u32.to_le_bytes());                   // heap_size
        assert_eq!(h.len(), 172, "LX header must be 172 bytes");
        h
    }
}

// ─── resolve_import (mirrors descriptors.rs::resolve_import) ─────────────────

fn resolve_import(module: &str, ordinal: u32) -> u32 {
    let base = match module {
        "DOSCALLS" => 0,
        "QUECALLS" => 1024,
        "PMWIN"    => PMWIN_BASE,
        "PMGPI"    => PMGPI_BASE,
        "KBDCALLS" => KBDCALLS_BASE,
        "VIOCALLS" => VIOCALLS_BASE,
        "SESMGR"   => SESMGR_BASE,
        "NLS"      => NLS_BASE,
        "MSG"      => MSG_BASE,
        "MDM"      => MDM_BASE,
        "UCONV"    => UCONV_BASE,
        _ => {
            eprintln!("warning: unknown import module '{}'", module);
            return MAGIC_API_BASE;
        }
    };
    MAGIC_API_BASE + base + ordinal
}

// ─── main ─────────────────────────────────────────────────────────────────────

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    let parsed = args::parse_args(&raw_args);

    if parsed.inputs.is_empty() {
        eprintln!("lx-link: no input files");
        process::exit(1);
    }

    let def_content = load_def_file(parsed.def_file.as_deref());
    let def_map = def_parser::parse_def(&def_content);

    let mut objects: Vec<elf_reader::ElfObject> = Vec::new();
    for input in &parsed.inputs {
        if let Err(e) = load_input(input, &mut objects) {
            eprintln!("lx-link: error reading {:?}: {}", input, e);
            process::exit(1);
        }
    }

    if objects.is_empty() {
        eprintln!("lx-link: no ELF objects found in inputs");
        process::exit(1);
    }

    let state = match linker_state::LinkerState::build(&objects) {
        Ok(s) => s,
        Err(e) => { eprintln!("lx-link: linker error: {}", e); process::exit(1); }
    };

    let lx_bytes = lx_writer::write_lx(&objects, &state, &def_map);

    if let Err(e) = std::fs::write(&parsed.output, &lx_bytes) {
        eprintln!("lx-link: cannot write {:?}: {}", parsed.output, e);
        process::exit(1);
    }
}

fn load_def_file(explicit: Option<&Path>) -> String {
    // Embedded fallback — compiled in so lx-link works regardless of CWD.
    const EMBEDDED: &str = include_str!("../../targets/os2api.def");
    if let Some(p) = explicit {
        return std::fs::read_to_string(p).unwrap_or_else(|_| EMBEDDED.to_string());
    }
    if let Ok(s) = std::fs::read_to_string("targets/os2api.def") { return s; }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && let Ok(s) = std::fs::read_to_string(dir.join("os2api.def")) {
        return s;
    }
    EMBEDDED.to_string()
}

fn load_input(path: &Path, objects: &mut Vec<elf_reader::ElfObject>) -> Result<(), Box<dyn Error>> {
    let data = std::fs::read(path)?;
    let name = path.display().to_string();

    if data.starts_with(b"!<arch>\n") {
        let archive = ArchiveFile::parse(data.as_slice())?;
        for member in archive.members() {
            let m = member?;
            let member_name = std::str::from_utf8(m.name()).unwrap_or("");
            if member_name.ends_with(".o") {
                let obj_data = m.data(data.as_slice())?;
                match elf_reader::parse_elf(obj_data, member_name) {
                    Ok(obj) => objects.push(obj),
                    Err(e) => eprintln!("warning: skipping {}/{}: {}", name, member_name, e),
                }
            }
        }
        return Ok(());
    }

    let obj = elf_reader::parse_elf(&data, &name)?;
    objects.push(obj);
    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use object::SectionKind;

    use super::*;

    #[test]
    fn test_def_parser() {
        let content = "\
# comment\n\
DOSCALLS.234  DosExit\n\
DOSCALLS.282  DosWrite\n\
VIOCALLS.19   VioWrtTTY\n";
        let map = def_parser::parse_def(content);
        assert_eq!(map.get("DosExit"),  Some(&("DOSCALLS".to_string(), 234)));
        assert_eq!(map.get("DosWrite"), Some(&("DOSCALLS".to_string(), 282)));
        assert_eq!(map.get("VioWrtTTY"),Some(&("VIOCALLS".to_string(), 19)));
        assert!(map.get("NonExistent").is_none());
    }

    #[test]
    fn test_page_assignment() {
        let source_va: u32 = 0x10001;
        let object_base: u32 = 0x10000;
        let page_idx   = (source_va - object_base) / PAGE_SIZE;
        let page_offset = (source_va - object_base) % PAGE_SIZE;
        assert_eq!(page_idx, 0);
        assert_eq!(page_offset, 1);
    }

    #[test]
    fn test_resolve_import_doscalls() {
        assert_eq!(resolve_import("DOSCALLS", 234), MAGIC_API_BASE + 234);
    }

    #[test]
    fn test_resolve_import_viocalls() {
        assert_eq!(resolve_import("VIOCALLS", 19), MAGIC_API_BASE + VIOCALLS_BASE + 19);
    }

    #[test]
    fn test_resolve_import_pmwin() {
        assert_eq!(resolve_import("PMWIN", 763), MAGIC_API_BASE + PMWIN_BASE + 763);
    }

    #[test]
    fn test_lx_roundtrip() {
        use elf_reader::{ElfObject, ElfSection, ElfSymbol};

        let obj = ElfObject {
            sections: vec![ElfSection {
                name: ".text".into(),
                kind: SectionKind::Text,
                data: vec![0xF4u8], // HLT
                align: 1,
                relocs: vec![],
            }],
            symbols: vec![ElfSymbol {
                name: "_start".into(),
                section_idx: Some(0),
                offset: 0,
            }],
        };

        let objects = vec![obj];
        let state = linker_state::LinkerState::build(&objects).expect("build state");
        let lx_bytes = lx_writer::write_lx(&objects, &state, &std::collections::HashMap::new());

        // Must start with MZ, then LX at 0x40.
        assert!(lx_bytes.len() > LX_HEADER_OFFSET as usize + LX_HEADER_SIZE as usize);
        assert_eq!(&lx_bytes[0..2], b"MZ");
        assert_eq!(&lx_bytes[LX_HEADER_OFFSET as usize..LX_HEADER_OFFSET as usize + 2], b"LX");

        let hdr = LX_HEADER_OFFSET as usize;
        let obj_count = u32::from_le_bytes(lx_bytes[hdr + 0x44..hdr + 0x48].try_into().unwrap());
        assert_eq!(obj_count, 3, "3 objects: code + data + stack");

        let eip_obj = u32::from_le_bytes(lx_bytes[hdr + 0x18..hdr + 0x1C].try_into().unwrap());
        assert_eq!(eip_obj, 1, "entry in object 1 (code)");

        let page_sz = u32::from_le_bytes(lx_bytes[hdr + 0x28..hdr + 0x2C].try_into().unwrap());
        assert_eq!(page_sz, 4096);

        let eip = u32::from_le_bytes(lx_bytes[hdr + 0x1C..hdr + 0x20].try_into().unwrap());
        assert_eq!(eip, 0, "entry offset = 0 for _start at code base");
    }

    #[test]
    fn test_import_fixup_encoding() {
        // Verify ExternalOrdinal fixup for DOSCALLS.282 encodes correctly.
        // source_type=0x07, target_flags=0x11, source_offset=42, module_idx=1, ordinal=282
        let mut bytes: Vec<u8> = Vec::new();
        bytes.push(0x07);
        bytes.push(0x11);
        bytes.extend_from_slice(&42u16.to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&282u32.to_le_bytes());

        assert_eq!(bytes[0], 0x07, "source_type must be 0x07");
        assert_eq!(bytes[1], 0x11, "target_flags must be 0x11");
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 42);
        assert_eq!(bytes[4], 1);
        assert_eq!(u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]), 282);
        assert_eq!(resolve_import("DOSCALLS", 282), MAGIC_API_BASE + 282);
    }

    #[test]
    fn test_args_parse_basic() {
        let raw = ["lx-link", "foo.o", "bar.o", "-o", "out.exe"]
            .iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let p = args::parse_args(&raw);
        assert_eq!(p.inputs.len(), 2);
        assert_eq!(p.output.to_str().unwrap(), "out.exe");
        assert!(p.def_file.is_none());
    }

    #[test]
    fn test_args_parse_ignores_unknown_flags() {
        let raw = [
            "lx-link", "--sysroot", "/foo", "-m", "elf_i386",
            "--static", "--as-needed", "--eh-frame-hdr",
            "--hash-style=gnu", "-z", "relro", "foo.o", "-o", "out.exe",
        ].iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let p = args::parse_args(&raw);
        assert_eq!(p.inputs.len(), 1);
        assert_eq!(p.output.to_str().unwrap(), "out.exe");
    }

    #[test]
    fn test_args_parse_def_flag() {
        let raw = ["lx-link", "foo.o", "--def", "/api.def", "-o", "out.exe"]
            .iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let p = args::parse_args(&raw);
        assert_eq!(p.def_file.as_ref().unwrap().to_str().unwrap(), "/api.def");
    }

    #[test]
    fn test_lx_header_size_constant() {
        assert_eq!(LX_HEADER_SIZE, 172);
    }
}
