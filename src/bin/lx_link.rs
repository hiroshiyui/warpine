// SPDX-License-Identifier: GPL-3.0-or-later
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
const SO32DLL_BASE: u32 = 13312;
const TCP32DLL_BASE: u32 = 14336;
const MOUCALLS_BASE: u32 = 15360;
const PMSHAPI_BASE: u32 = 16384;
const HELPMGR_BASE: u32 = 17408;
const PMSPL_BASE: u32 = 18432;
const PMCTLS_BASE: u32 = 19456;
const WARPINE_BASE: u32 = 20480;
const PMDRAG_BASE: u32 = 21504;
const SOM_BASE: u32 = 22528;
/// SOM is 4096 ordinals wide (ADR-0070); the others 1024.
const REXX_BASE: u32 = SOM_BASE + 4096;
const REXXAPI_BASE: u32 = REXX_BASE + 1024;
const PMWP_BASE: u32 = REXXAPI_BASE + 1024;

/// Base virtual address of the code (text) object: the start of Warpine's
/// program area (`EXE_AREA_START`, ADR-0094). The data object follows on
/// the next [`OBJECT_ALIGN`] boundary after the code, the stack after it.
const CODE_OBJECT_BASE: u32 = 0x0001_0000;
/// Objects start on 64 KiB boundaries.
const OBJECT_ALIGN: u32 = 0x0001_0000;
/// The end of the program area (`EXE_AREA_END`): the API thunks.
const PROGRAM_AREA_END: u32 = MAGIC_API_BASE;
/// Stack size: 64 KiB.
const STACK_SIZE: u32 = 0x0001_0000;

const PAGE_SIZE: u32 = 4096;
/// Absolute file offset of the LX header (after 64-byte MZ stub).
const LX_HEADER_OFFSET: u32 = 0x40;
/// Size in bytes of the LX header structure.
const LX_HEADER_SIZE: u32 = 172; // 0xAC

// LX object flags
const OBJ_READ: u32 = 0x0001;
const OBJ_WRITE: u32 = 0x0002;
const OBJ_EXEC: u32 = 0x0004;
/// The object holds resources (reached through the resource table).
const OBJ_RESOURCE: u32 = 0x0008;
const OBJ_BIG: u32 = 0x0040; // 32-bit (USE32)
const OBJ_USE32: u32 = 0x2000;

// ─── mod args ─────────────────────────────────────────────────────────────────

mod args {
    use std::path::PathBuf;

    pub struct Args {
        pub inputs: Vec<PathBuf>,
        pub output: PathBuf,
        pub def_file: Option<PathBuf>,
        /// `--gc-sections`: link only the sections the program reaches.
        pub gc_sections: bool,
    }

    pub fn parse_args(raw: &[String]) -> Args {
        let mut inputs = Vec::new();
        let mut output = PathBuf::from("a.out");
        let mut def_file = None;
        let mut gc_sections = false;
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
            } else if arg == "--gc-sections" {
                gc_sections = true;
            } else if arg == "--no-gc-sections" {
                gc_sections = false;
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
                // Positional: only collect .o / .rlib / .a / .res files
                let lower = arg.to_ascii_lowercase();
                if lower.ends_with(".o") || lower.ends_with(".rlib") || lower.ends_with(".a") || lower.ends_with(".res") {
                    inputs.push(PathBuf::from(arg));
                }
            }
            i += 1;
        }
        Args { inputs, output, def_file, gc_sections }
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

// ─── mod res_reader ───────────────────────────────────────────────────────────

/// `.res` files in the classic OS/2 format (`os2rc`, ADR-0099): each
/// resource is 0xFF, its type, 0xFF, its name (both numbers), memory
/// flags, its size and its data.
mod res_reader {
    #[derive(Debug, Clone, PartialEq)]
    pub struct ResEntry {
        pub type_id: u16,
        pub name: u16,
        pub data: Vec<u8>,
    }

    /// Size of an entry's header.
    const HEADER: usize = 12;
    /// The mark before a numeric type or name.
    const ORDINAL_MARK: u8 = 0xFF;

    pub fn parse_res(data: &[u8], source: &str) -> Result<Vec<ResEntry>, String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let h = data.get(i..i + HEADER).ok_or_else(|| format!("{source}: a resource header is cut short"))?;
            if h[0] != ORDINAL_MARK || h[3] != ORDINAL_MARK {
                return Err(format!("{source}: resources must have numeric types and names (offset {i})"));
            }
            let type_id = u16::from_le_bytes([h[1], h[2]]);
            let name = u16::from_le_bytes([h[4], h[5]]);
            let size = u32::from_le_bytes([h[8], h[9], h[10], h[11]]) as usize;
            let body = data.get(i + HEADER..).and_then(|r| r.get(..size))
                .ok_or_else(|| format!("{source}: resource {type_id}/{name} is cut short"))?;
            out.push(ResEntry { type_id, name, data: body.to_vec() });
            i += HEADER + size;
        }
        Ok(out)
    }
}

// ─── mod elf_reader ───────────────────────────────────────────────────────────

/// i386 ELF objects as `lx-link` sees them: allocated sections classified
/// by their ELF type and flags (so `.text.*`, `.rodata.str1.1`,
/// `.data.rel.local`, `.bss.*` … all work), relocations with their
/// addends — the implicit ones of REL sections read from the section
/// bytes — and targets resolved per object (a local symbol or a section of
/// *this* object, never a same-named one elsewhere).
mod elf_reader {
    use std::collections::HashMap;

    use object::{
        Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SectionFlags,
        SectionKind, SymbolKind, SymbolSection,
    };

    /// ELF section flags and types used here.
    const SHF_ALLOC: u64 = 0x2;
    const SHF_EXECINSTR: u64 = 0x4;
    const SHF_TLS: u64 = 0x400;
    const SHT_INIT_ARRAY: u32 = 14;
    const SHT_FINI_ARRAY: u32 = 15;
    const SHT_PREINIT_ARRAY: u32 = 16;

    /// i386 relocation types (System V ABI, Intel386 supplement).
    pub const R_386_32: u32 = 1;
    pub const R_386_PC32: u32 = 2;
    pub const R_386_PLT32: u32 = 4;

    /// Where a section goes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SecClass {
        Code,
        /// Initialised data, writable or read-only.
        Data,
        /// Zero-initialised data (`SHT_NOBITS`).
        Bss,
        /// An array of function pointers the start-up code runs.
        Array(ArrayKind),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ArrayKind { Preinit, Init, Fini }

    /// How a relocation is computed (S + A, or S + A − P).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RelocKind { Abs32, Pc32 }

    /// A relocation of a section.
    #[derive(Debug, Clone)]
    pub struct Reloc {
        /// Byte offset within the section.
        pub offset: u64,
        pub kind: RelocKind,
        /// The addend: explicit (RELA) or read from the section (REL).
        pub addend: i64,
        pub target: RelocTarget,
    }

    /// What a relocation refers to.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum RelocTarget {
        /// A global symbol: defined by some object, an OS/2 API import,
        /// or undefined.
        Symbol(String),
        /// A place in a section of the same object (a local symbol, or the
        /// section itself).
        Local { section: usize, offset: u64 },
        /// An absolute value.
        Absolute(u64),
    }

    /// An allocated section of an object.
    #[derive(Debug)]
    pub struct ElfSection {
        pub name: String,
        pub class: SecClass,
        /// The bytes (empty for `Bss`).
        pub data: Vec<u8>,
        /// The size in memory.
        pub size: u64,
        /// Alignment (a power of two).
        pub align: u64,
        pub relocs: Vec<Reloc>,
    }

    /// A global symbol an object defines.
    #[derive(Debug, Clone)]
    pub struct ElfSymbol {
        pub name: String,
        /// The defining section (index into `ElfObject::sections`).
        pub section_idx: usize,
        /// Byte offset within it.
        pub offset: u64,
        pub weak: bool,
    }

    /// A common symbol (`int x;` with `-fcommon`).
    #[derive(Debug, Clone)]
    pub struct CommonSymbol {
        pub name: String,
        pub size: u64,
        pub align: u64,
    }

    /// One parsed object file.
    #[derive(Debug, Default)]
    pub struct ElfObject {
        /// Where it came from, for messages.
        pub name: String,
        pub sections: Vec<ElfSection>,
        /// Global symbols it defines.
        pub symbols: Vec<ElfSymbol>,
        pub commons: Vec<CommonSymbol>,
        /// Global symbols it refers to without defining them (strong
        /// references; weak ones are in `weak_refs`).
        pub undefined: Vec<String>,
        /// Undefined weak symbols: 0 when nothing defines them.
        pub weak_refs: Vec<String>,
    }

    /// The largest uninitialized section accepted: the whole guest
    /// memory of a Warpine process is smaller.
    pub const MAX_SECTION_BYTES: u64 = 256 * 1024 * 1024;

    /// The zero bytes of an uninitialized section of `size` bytes, or an
    /// error when the input asks for more than `MAX_SECTION_BYTES`.
    pub fn zeroed_section(size: u64, source_name: &str) -> Result<Vec<u8>, Box<dyn super::Error>> {
        if size > MAX_SECTION_BYTES {
            return Err(format!("{source_name}: an uninitialized section of {size} bytes (at most {MAX_SECTION_BYTES})").into());
        }
        Ok(vec![0u8; size as usize])
    }

    /// The class of a section, `None` for one that is not linked (not
    /// allocated: debug information, notes, `.comment`; or unwind tables,
    /// which nothing reads on OS/2).
    fn classify(name: &str, kind: SectionKind, sh_flags: u64) -> Option<SecClass> {
        if sh_flags & SHF_ALLOC == 0 || name.starts_with(".eh_frame") || name.starts_with(".note") {
            return None;
        }
        Some(match kind {
            SectionKind::Elf(SHT_PREINIT_ARRAY) => SecClass::Array(ArrayKind::Preinit),
            SectionKind::Elf(SHT_INIT_ARRAY) => SecClass::Array(ArrayKind::Init),
            SectionKind::Elf(SHT_FINI_ARRAY) => SecClass::Array(ArrayKind::Fini),
            SectionKind::UninitializedData => SecClass::Bss,
            _ if sh_flags & SHF_EXECINSTR != 0 => SecClass::Code,
            _ => SecClass::Data,
        })
    }

    /// The name of an i386 relocation type, for messages.
    pub fn reloc_name(r_type: u32) -> &'static str {
        match r_type {
            3 => "R_386_GOT32", 9 => "R_386_GOTOFF", 10 => "R_386_GOTPC", 11 => "R_386_32PLT",
            20 => "R_386_16", 21 => "R_386_PC16", 22 => "R_386_8", 23 => "R_386_PC8",
            43 => "R_386_GOT32X", 14..=19 | 24..=42 => "a thread-local or dynamic relocation",
            _ => "an unknown relocation",
        }
    }

    /// Parse one ELF object from raw bytes.
    pub fn parse_elf(data: &[u8], source_name: &str) -> Result<ElfObject, Box<dyn super::Error>> {
        let obj = object::File::parse(data)?;
        if obj.architecture() != object::Architecture::I386 {
            return Err(format!("{source_name}: not an i386 object ({:?}); compile with -m32", obj.architecture()).into());
        }

        let mut sections: Vec<ElfSection> = Vec::new();
        let mut index_map: HashMap<object::SectionIndex, usize> = HashMap::new();
        for section in obj.sections() {
            let name = section.name().unwrap_or("").to_string();
            let SectionFlags::Elf { sh_flags } = section.flags() else { continue };
            if sh_flags & SHF_TLS != 0 && sh_flags & SHF_ALLOC != 0 {
                return Err(format!("{source_name}: section {name}: thread-local storage is not supported").into());
            }
            let Some(class) = classify(&name, section.kind(), sh_flags) else { continue };
            let size = section.size();
            let data = if class == SecClass::Bss {
                zeroed_section(size, source_name)?;
                Vec::new()
            } else {
                // Compressed sections are not supported: linking zeros in
                // place of code would be silent.
                section.uncompressed_data()
                    .map_err(|e| format!("{source_name}: section {name}: {e}"))?
                    .to_vec()
            };
            index_map.insert(section.index(), sections.len());
            sections.push(ElfSection { name, class, data, size, align: section.align().max(1), relocs: Vec::new() });
        }

        for section in obj.sections() {
            let Some(&local) = index_map.get(&section.index()) else { continue };
            for (offset, reloc) in section.relocations() {
                let RelocationFlags::Elf { r_type } = reloc.flags() else {
                    return Err(format!("{source_name}: not an ELF relocation").into());
                };
                let kind = match r_type {
                    R_386_32 => RelocKind::Abs32,
                    // A PLT reference is a direct call in a static link.
                    R_386_PC32 | R_386_PLT32 => RelocKind::Pc32,
                    other => {
                        let sec = &sections[local].name;
                        return Err(format!(
                            "{source_name}: section {sec} at {offset:#x}: {} (type {other}) is not supported; \
                             compile with -fno-pic (position-independent code needs a GOT, which OS/2 programs do not have)",
                            reloc_name(other)).into());
                    }
                };
                let mut addend = reloc.addend();
                if reloc.has_implicit_addend() {
                    let bytes = sections[local].data.get(offset as usize..offset as usize + 4)
                        .ok_or_else(|| format!("{source_name}: relocation at {offset:#x} outside section {}", sections[local].name))?;
                    addend += i64::from(i32::from_le_bytes(bytes.try_into().unwrap()));
                }
                let target = match reloc.target() {
                    RelocationTarget::Symbol(idx) => {
                        let sym = obj.symbol_by_index(idx)?;
                        let name = sym.name().unwrap_or("").to_string();
                        match sym.section() {
                            SymbolSection::Section(sec) if sym.is_local() || sym.kind() == SymbolKind::Section => {
                                let Some(&s) = index_map.get(&sec) else {
                                    return Err(format!("{source_name}: relocation at {offset:#x} refers to a section that is not linked ({name})").into());
                                };
                                RelocTarget::Local { section: s, offset: sym.address() }
                            }
                            SymbolSection::Absolute => RelocTarget::Absolute(sym.address()),
                            _ => RelocTarget::Symbol(name),
                        }
                    }
                    RelocationTarget::Section(sec) => match index_map.get(&sec) {
                        Some(&s) => RelocTarget::Local { section: s, offset: 0 },
                        None => return Err(format!("{source_name}: relocation at {offset:#x} refers to a section that is not linked").into()),
                    },
                    RelocationTarget::Absolute => RelocTarget::Absolute(0),
                    _ => return Err(format!("{source_name}: relocation at {offset:#x} has an unsupported target").into()),
                };
                if sections[local].class == SecClass::Bss {
                    return Err(format!("{source_name}: relocation in uninitialised section {}", sections[local].name).into());
                }
                sections[local].relocs.push(Reloc { offset, kind, addend, target });
            }
        }

        let mut symbols = Vec::new();
        let mut commons = Vec::new();
        let mut undefined = Vec::new();
        let mut weak_refs = Vec::new();
        for sym in obj.symbols() {
            let name = match sym.name() { Ok(n) if !n.is_empty() => n.to_string(), _ => continue };
            if matches!(sym.kind(), SymbolKind::File | SymbolKind::Section) || sym.is_local() {
                continue;
            }
            match sym.section() {
                SymbolSection::Section(sec) => {
                    if let Some(&s) = index_map.get(&sec) {
                        symbols.push(ElfSymbol { name, section_idx: s, offset: sym.address(), weak: sym.is_weak() });
                    }
                }
                SymbolSection::Common => commons.push(CommonSymbol { name, size: sym.size(), align: sym.address().max(1) }),
                SymbolSection::Undefined => {
                    if sym.is_weak() { weak_refs.push(name) } else { undefined.push(name) }
                }
                _ => {}
            }
        }
        Ok(ElfObject { name: source_name.to_string(), sections, symbols, commons, undefined, weak_refs })
    }
}

// ─── mod linker_state ─────────────────────────────────────────────────────────

/// The program's layout: one code object from 64 KB, the data object
/// (initialised data, the init / fini arrays, then zeros) on the next
/// 64 KB boundary, then the stack — inside Warpine's program area
/// (ADR-0094). Symbols get their addresses here.
mod linker_state {
    use std::collections::HashMap;

    use super::elf_reader::{ArrayKind, ElfObject, SecClass};
    use super::{CODE_OBJECT_BASE, OBJECT_ALIGN, PROGRAM_AREA_END, STACK_SIZE};

    pub struct LinkerState {
        /// The code object's bytes.
        pub code: Vec<u8>,
        /// The data object's initialised bytes; `bss_size` zero bytes follow.
        pub data: Vec<u8>,
        pub bss_size: u32,
        pub code_va: u32,
        pub data_va: u32,
        pub stack_va: u32,
        /// Address of each linked section: `[object][section]`.
        pub section_va: Vec<Vec<Option<u32>>>,
        /// Global symbol → address.
        pub symbol_table: HashMap<String, u32>,
        pub entry_va: u32,
    }

    pub fn align_up(val: u64, align: u64) -> u64 {
        if align <= 1 { val } else { val.div_ceil(align) * align }
    }

    /// The start / end symbols of each array, in the order they are laid out.
    pub const ARRAYS: [(ArrayKind, &str, &str); 3] = [
        (ArrayKind::Preinit, "__preinit_array_start", "__preinit_array_end"),
        (ArrayKind::Init, "__init_array_start", "__init_array_end"),
        (ArrayKind::Fini, "__fini_array_start", "__fini_array_end"),
    ];

    impl LinkerState {
        /// Lay out the sections `live` marks (`[object][section]`; all of
        /// them with `None`).
        pub fn build(objects: &[ElfObject], live: Option<&[Vec<bool>]>) -> Result<Self, Box<dyn super::Error>> {
            let is_live = |oi: usize, si: usize| live.is_none_or(|l| l[oi][si]);
            // Offsets within their object first, addresses once sizes are known.
            let mut code_off: Vec<Vec<Option<u64>>> = objects.iter().map(|o| vec![None; o.sections.len()]).collect();
            let mut data_off = code_off.clone();
            let mut code: Vec<u8> = Vec::new();
            let mut data: Vec<u8> = Vec::new();
            let place = |buf: &mut Vec<u8>, bytes: &[u8], align: u64| -> u64 {
                let at = align_up(buf.len() as u64, align);
                buf.resize(at as usize, 0);
                buf.extend_from_slice(bytes);
                at
            };
            for (oi, o) in objects.iter().enumerate() {
                for (si, s) in o.sections.iter().enumerate() {
                    if s.class == SecClass::Code && is_live(oi, si) { code_off[oi][si] = Some(place(&mut code, &s.data, s.align)); }
                }
            }
            for (oi, o) in objects.iter().enumerate() {
                for (si, s) in o.sections.iter().enumerate() {
                    if s.class == SecClass::Data && is_live(oi, si) { data_off[oi][si] = Some(place(&mut data, &s.data, s.align)); }
                }
            }
            let mut array_bounds: Vec<(u64, u64)> = Vec::new();
            for (kind, _, _) in ARRAYS {
                let start = align_up(data.len() as u64, 4);
                data.resize(start as usize, 0);
                for (oi, o) in objects.iter().enumerate() {
                    for (si, s) in o.sections.iter().enumerate() {
                        if s.class == SecClass::Array(kind) && is_live(oi, si) { data_off[oi][si] = Some(place(&mut data, &s.data, s.align)); }
                    }
                }
                array_bounds.push((start, data.len() as u64));
            }
            // Zeros: uninitialised sections, then the common symbols no
            // object defines (the largest size and alignment of each).
            let mut bss_end = data.len() as u64;
            for (oi, o) in objects.iter().enumerate() {
                for (si, s) in o.sections.iter().enumerate() {
                    if s.class == SecClass::Bss && is_live(oi, si) {
                        let at = align_up(bss_end, s.align);
                        data_off[oi][si] = Some(at);
                        bss_end = at + s.size;
                    }
                }
            }
            let defined: std::collections::HashSet<&str> =
                objects.iter().flat_map(|o| o.symbols.iter().map(|s| s.name.as_str())).collect();
            let mut commons: Vec<(String, u64, u64)> = Vec::new();
            for c in objects.iter().flat_map(|o| &o.commons) {
                if defined.contains(c.name.as_str()) { continue; }
                match commons.iter_mut().find(|(n, _, _)| *n == c.name) {
                    Some(e) => { e.1 = e.1.max(c.size); e.2 = e.2.max(c.align); }
                    None => commons.push((c.name.clone(), c.size, c.align)),
                }
            }
            let mut common_off = Vec::new();
            for (name, size, align) in &commons {
                let at = align_up(bss_end, *align);
                common_off.push((name.clone(), at));
                bss_end = at + size;
            }

            // The layout, inside the program area.
            let code_va = u64::from(CODE_OBJECT_BASE);
            let data_va = align_up(code_va + (code.len() as u64).max(1), u64::from(OBJECT_ALIGN));
            let stack_va = align_up(data_va + bss_end.max(1), u64::from(OBJECT_ALIGN));
            if stack_va + u64::from(STACK_SIZE) > u64::from(PROGRAM_AREA_END) {
                return Err(format!("the program ({} bytes of code, {bss_end} of data) does not fit below {PROGRAM_AREA_END:#x}",
                                   code.len()).into());
            }
            let section_va: Vec<Vec<Option<u32>>> = code_off.iter().zip(&data_off).map(|(c, d)| {
                c.iter().zip(d).map(|(c, d)| c.map(|o| (code_va + o) as u32).or(d.map(|o| (data_va + o) as u32))).collect()
            }).collect();

            // Global symbols: a strong definition wins over a weak one; two
            // strong ones are an error.
            let mut symbol_table: HashMap<String, u32> = HashMap::new();
            let mut weak: HashMap<String, bool> = HashMap::new();
            let mut duplicates = Vec::new();
            for (oi, o) in objects.iter().enumerate() {
                for s in &o.symbols {
                    let Some(base) = section_va[oi][s.section_idx] else { continue };
                    let va = base.wrapping_add(s.offset as u32);
                    match weak.get(&s.name) {
                        None => { symbol_table.insert(s.name.clone(), va); weak.insert(s.name.clone(), s.weak); }
                        Some(true) if !s.weak => { symbol_table.insert(s.name.clone(), va); weak.insert(s.name.clone(), false); }
                        Some(false) if !s.weak => duplicates.push(format!("{} (in {})", s.name, o.name)),
                        _ => {}
                    }
                }
            }
            if !duplicates.is_empty() {
                return Err(format!("symbols defined more than once: {}", duplicates.join(", ")).into());
            }
            for (name, at) in common_off {
                symbol_table.insert(name, (data_va + at) as u32);
            }
            for ((_, start_name, end_name), (start, end)) in ARRAYS.iter().zip(&array_bounds) {
                symbol_table.entry(start_name.to_string()).or_insert((data_va + start) as u32);
                symbol_table.entry(end_name.to_string()).or_insert((data_va + end) as u32);
            }

            let entry_va = symbol_table.get("_start").or_else(|| symbol_table.get("main")).copied()
                .ok_or("no entry point: define _start (or main)")?;

            let bss_size = (bss_end - data.len() as u64) as u32;
            Ok(LinkerState {
                code, data, bss_size,
                code_va: code_va as u32, data_va: data_va as u32, stack_va: stack_va as u32,
                section_va, symbol_table, entry_va,
            })
        }

        /// The object (1 code, 2 data, 3 stack) holding `va`, and its base.
        pub fn object_of(&self, va: u32) -> (u8, u32) {
            if va >= self.stack_va { (3, self.stack_va) }
            else if va >= self.data_va { (2, self.data_va) }
            else { (1, self.code_va) }
        }
    }

    pub fn page_count(bytes: u64) -> usize {
        bytes.div_ceil(u64::from(super::PAGE_SIZE)) as usize
    }
}

// ─── mod lx_writer ────────────────────────────────────────────────────────────

mod lx_writer {
    use std::collections::HashMap;

    use super::elf_reader::{ElfObject, RelocKind, RelocTarget};
    use super::linker_state::{page_count, LinkerState};
    use super::res_reader::ResEntry;
    use super::{
        resolve_import, LX_HEADER_OFFSET, LX_HEADER_SIZE,
        OBJ_BIG, OBJ_EXEC, OBJ_READ, OBJ_RESOURCE, OBJ_USE32, OBJ_WRITE, PAGE_SIZE, STACK_SIZE,
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

    /// What a relocation's target resolved to.
    enum Resolved {
        /// An address in the program (with an internal fixup).
        Internal(u32),
        /// An OS/2 API (with an import fixup).
        Import { va: u32, module: String, ordinal: u32 },
        /// A plain value (absolute symbols, undefined weak ones).
        Value(u32),
    }

    fn resolve(target: &RelocTarget, obj_idx: usize, state: &LinkerState, obj: &ElfObject,
               def_map: &HashMap<String, (String, u32)>) -> Result<Resolved, String> {
        Ok(match target {
            RelocTarget::Symbol(name) => {
                if let Some(&va) = state.symbol_table.get(name) {
                    Resolved::Internal(va)
                } else if let Some((module, ordinal)) = def_map.get(name) {
                    let va = resolve_import(module, *ordinal)
                        .ok_or_else(|| format!("{name} (module {module} is unknown to lx-link)"))?;
                    Resolved::Import { va, module: module.clone(), ordinal: *ordinal }
                } else if obj.weak_refs.iter().any(|w| w == name) {
                    Resolved::Value(0)
                } else {
                    return Err(name.clone());
                }
            }
            RelocTarget::Local { section, offset } => match state.section_va[obj_idx].get(*section).copied().flatten() {
                Some(base) => Resolved::Internal(base.wrapping_add(*offset as u32)),
                None => return Err(format!("a section of {} that is not linked", obj.name)),
            },
            RelocTarget::Absolute(v) => Resolved::Value(*v as u32),
        })
    }

    /// Generate the LX executable; the error lists the undefined symbols.
    pub fn write_lx(
        objects: &[ElfObject],
        state: &LinkerState,
        def_map: &HashMap<String, (String, u32)>,
        resources: &[ResEntry],
    ) -> Result<Vec<u8>, String> {
        let entry_offset = state.entry_va - state.code_va;
        let mut code_data = state.code.clone();
        let mut data_data = state.data.clone();

        let code_pages = page_count(code_data.len() as u64);
        let data_pages = page_count(data_data.len() as u64 + u64::from(state.bss_size));
        let stack_pages = (STACK_SIZE / PAGE_SIZE) as usize;
        let (res_blob, res_table) = resource_object(resources)?;
        let res_pages = page_count(res_blob.len() as u64);
        let total_pages = code_pages + data_pages + stack_pages + res_pages;

        let mut module_map: HashMap<String, u8> = HashMap::new();
        let mut imported_modules: Vec<String> = Vec::new();
        let mut code_fixups: Vec<Vec<LxFixup>> = vec![Vec::new(); code_pages];
        let mut data_fixups: Vec<Vec<LxFixup>> = vec![Vec::new(); data_pages];
        let mut undefined: Vec<String> = Vec::new();

        for (obj_idx, obj) in objects.iter().enumerate() {
            for (sec_idx, sec) in obj.sections.iter().enumerate() {
                let Some(section_va) = state.section_va[obj_idx][sec_idx] else { continue };
                let (_, obj_base) = state.object_of(section_va);
                let is_code = section_va < state.data_va;
                for reloc in &sec.relocs {
                    let source_va = section_va.wrapping_add(reloc.offset as u32);
                    let buf_off = (source_va - obj_base) as usize;
                    let resolved = match resolve(&reloc.target, obj_idx, state, obj, def_map) {
                        Ok(r) => r,
                        Err(name) => { if !undefined.contains(&name) { undefined.push(name); } continue; }
                    };
                    let target_va = match &resolved {
                        Resolved::Internal(va) | Resolved::Value(va) | Resolved::Import { va, .. } => *va,
                    };
                    let value = match reloc.kind {
                        RelocKind::Abs32 => target_va.wrapping_add(reloc.addend as u32),
                        RelocKind::Pc32 => target_va.wrapping_add(reloc.addend as u32).wrapping_sub(source_va),
                    };
                    let buf = if is_code { &mut code_data } else { &mut data_data };
                    let Some(slot) = buf.get_mut(buf_off..buf_off + 4) else {
                        return Err(format!("{}: relocation at {:#x} outside its section", obj.name, reloc.offset));
                    };
                    slot.copy_from_slice(&value.to_le_bytes());
                    if reloc.kind != RelocKind::Abs32 { continue; }
                    let fixup_target = match resolved {
                        Resolved::Internal(va) => {
                            let (object_num, base) = state.object_of(va);
                            FixupTarget::Internal { object_num, target_offset: value.wrapping_sub(base) }
                        }
                        Resolved::Import { module, ordinal, .. } => {
                            if reloc.addend != 0 {
                                return Err(format!("{}: an offset ({}) from an OS/2 API is not supported", obj.name, reloc.addend));
                            }
                            let module_idx = get_or_insert_module(&module, &mut module_map, &mut imported_modules);
                            FixupTarget::ExternalOrdinal { module_idx, ordinal }
                        }
                        Resolved::Value(_) => continue,
                    };
                    let page = (source_va - obj_base) / PAGE_SIZE;
                    let fixups = if is_code { &mut code_fixups } else { &mut data_fixups };
                    if let Some(list) = fixups.get_mut(page as usize) {
                        list.push(LxFixup {
                            source_type: 0x07,
                            source_offset: ((source_va - obj_base) % PAGE_SIZE) as u16,
                            target: fixup_target,
                        });
                    }
                }
            }
        }
        if !undefined.is_empty() {
            undefined.sort();
            return Err(format!("undefined symbols: {}", undefined.join(", ")));
        }

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
        for pi in 0..stack_pages + res_pages {
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
        let num_objects: u32 = if resources.is_empty() { 3 } else { 4 };
        let obj_table_off: u32 = LX_HEADER_SIZE;
        let page_map_off: u32 = obj_table_off + num_objects * 24;
        let resource_table_off: u32 = page_map_off + total_pages as u32 * 8;
        let resident_name_off: u32 = resource_table_off + res_table.len() as u32;
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
            if resources.is_empty() { 0 } else { resource_table_off }, resources.len() as u32,
        ));

        // 3. Object table.
        out.extend_from_slice(&encode_object_entry(
            code_data.len() as u32, state.code_va,
            OBJ_READ | OBJ_EXEC | OBJ_BIG | OBJ_USE32,
            1, code_pages as u32,
        ));
        out.extend_from_slice(&encode_object_entry(
            data_data.len() as u32 + state.bss_size, state.data_va,
            OBJ_READ | OBJ_WRITE | OBJ_BIG | OBJ_USE32,
            code_pages as u32 + 1, data_pages as u32,
        ));
        out.extend_from_slice(&encode_object_entry(
            STACK_SIZE, state.stack_va,
            OBJ_READ | OBJ_WRITE | OBJ_BIG | OBJ_USE32,
            code_pages as u32 + data_pages as u32 + 1, STACK_SIZE / PAGE_SIZE,
        ));
        if !resources.is_empty() {
            // The loader places resource objects itself: base 0.
            out.extend_from_slice(&encode_object_entry(
                res_blob.len() as u32, 0, OBJ_READ | OBJ_RESOURCE,
                (code_pages + data_pages + stack_pages) as u32 + 1, res_pages as u32,
            ));
        }

        // 4. Page map: each page's bytes in the file; pages past the
        // initialised bytes (zeros, the stack) have none.
        let mut file_off: u32 = 0;
        for (pages, len) in [(code_pages, code_data.len()), (data_pages, data_data.len()), (stack_pages, 0),
                             (res_pages, res_blob.len())] {
            for pi in 0..pages {
                let psize = len.saturating_sub(pi * PAGE_SIZE as usize).min(PAGE_SIZE as usize) as u16;
                out.extend_from_slice(&file_off.to_le_bytes());
                out.extend_from_slice(&psize.to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
                file_off += u32::from(psize);
            }
        }

        // 5. Resource table, then the resident name table.
        out.extend_from_slice(&res_table);
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
        // 12. Data pages: the initialised bytes, page after page.
        out.extend_from_slice(&code_data);
        out.extend_from_slice(&data_data);
        out.extend_from_slice(&res_blob);
        Ok(out)
    }

    /// The resource object's bytes (each resource 4-byte aligned) and the
    /// LX resource table: type, name, size, object (4) and offset, 14 bytes
    /// an entry. A type and name given twice is an error.
    fn resource_object(resources: &[ResEntry]) -> Result<(Vec<u8>, Vec<u8>), String> {
        const RESOURCE_OBJECT: u16 = 4;
        let mut blob = Vec::new();
        let mut table = Vec::with_capacity(resources.len() * 14);
        let mut seen = std::collections::HashSet::new();
        for r in resources {
            if !seen.insert((r.type_id, r.name)) {
                return Err(format!("resource type {} id {} is given twice", r.type_id, r.name));
            }
            while blob.len() % 4 != 0 { blob.push(0); }
            let offset = u32::try_from(blob.len()).map_err(|_| "resources larger than 4 GB".to_string())?;
            let size = u32::try_from(r.data.len()).map_err(|_| "a resource larger than 4 GB".to_string())?;
            table.extend_from_slice(&r.type_id.to_le_bytes());
            table.extend_from_slice(&r.name.to_le_bytes());
            table.extend_from_slice(&size.to_le_bytes());
            table.extend_from_slice(&RESOURCE_OBJECT.to_le_bytes());
            table.extend_from_slice(&offset.to_le_bytes());
            blob.extend_from_slice(&r.data);
        }
        Ok((blob, table))
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

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
        resource_table_off: u32,
        resource_count: u32,
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
        h.extend_from_slice(&resource_table_off.to_le_bytes());     // resource_table_offset
        h.extend_from_slice(&resource_count.to_le_bytes());         // resource_count
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

/// The thunk address of `module`'s `ordinal`; `None` for a module Warpine
/// does not provide. Calls reach imports through `call rel32`, so this
/// address is linked in, not only the import fixup.
fn resolve_import(module: &str, ordinal: u32) -> Option<u32> {
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
        "SO32DLL"  => SO32DLL_BASE,
        "TCP32DLL" => TCP32DLL_BASE,
        "MOUCALLS" => MOUCALLS_BASE,
        "PMSHAPI"  => PMSHAPI_BASE,
        "HELPMGR"  => HELPMGR_BASE,
        "PMSPL"    => PMSPL_BASE,
        "PMCTLS"   => PMCTLS_BASE,
        "WARPINE"  => WARPINE_BASE,
        "PMDRAG"   => PMDRAG_BASE,
        "SOM"      => SOM_BASE,
        "REXX"     => REXX_BASE,
        "REXXAPI"  => REXXAPI_BASE,
        "PMWP"     => PMWP_BASE,
        _ => return None,
    };
    Some(MAGIC_API_BASE + base + ordinal)
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

    let mut inputs = Inputs::default();
    for input in &parsed.inputs {
        if let Err(e) = load_input(input, &mut inputs) {
            eprintln!("lx-link: error reading {:?}: {}", input, e);
            process::exit(1);
        }
    }
    let resources = std::mem::take(&mut inputs.resources);
    let objects = select_objects(inputs, &def_map);
    if objects.is_empty() {
        eprintln!("lx-link: no ELF objects found in inputs");
        process::exit(1);
    }
    let live = parsed.gc_sections.then(|| live_sections(&objects));

    let state = match linker_state::LinkerState::build(&objects, live.as_deref()) {
        Ok(s) => s,
        Err(e) => { eprintln!("lx-link: {}", e); process::exit(1); }
    };

    let lx_bytes = match lx_writer::write_lx(&objects, &state, &def_map, &resources) {
        Ok(b) => b,
        Err(e) => { eprintln!("lx-link: {}", e); process::exit(1); }
    };

    if let Err(e) = std::fs::write(&parsed.output, &lx_bytes) {
        eprintln!("lx-link: cannot write {:?}: {}", parsed.output, e);
        process::exit(1);
    }
}

/// The parsed inputs: plain objects are always linked; an archive member
/// only when it defines a symbol something linked needs.
#[derive(Default)]
struct Inputs {
    objects: Vec<elf_reader::ElfObject>,
    members: Vec<elf_reader::ElfObject>,
    /// Resources from .res files, in order.
    resources: Vec<res_reader::ResEntry>,
}

/// The objects to link: the plain ones, then archive members pulled in
/// one by one while they define a symbol that is still undefined (as a
/// static linker does), in archive order.
fn select_objects(inputs: Inputs, def_map: &std::collections::HashMap<String, (String, u32)>) -> Vec<elf_reader::ElfObject> {
    use std::collections::HashSet;
    let Inputs { mut objects, members, .. } = inputs;
    let mut members: Vec<Option<elf_reader::ElfObject>> = members.into_iter().map(Some).collect();
    loop {
        let defined: HashSet<&str> = objects.iter()
            .flat_map(|o| o.symbols.iter().map(|s| s.name.as_str()).chain(o.commons.iter().map(|c| c.name.as_str())))
            .collect();
        let wanted: HashSet<String> = objects.iter()
            .flat_map(|o| o.undefined.iter().chain(o.sections.iter().flat_map(|s| s.relocs.iter()).filter_map(|r| match &r.target {
                elf_reader::RelocTarget::Symbol(n) => Some(n),
                _ => None,
            })))
            .filter(|n| !defined.contains(n.as_str()) && !def_map.contains_key(n.as_str()))
            .cloned().collect();
        let next = members.iter().position(|m| m.as_ref().is_some_and(|m| m.symbols.iter().any(|s| wanted.contains(&s.name))));
        match next {
            Some(i) => objects.push(members[i].take().expect("present")),
            None => return objects,
        }
    }
}

/// `--gc-sections`: the sections reached from the entry point (`_start`,
/// else `main`) and the init / fini arrays through relocations, as
/// `[object][section]`. Common symbols always stay.
fn live_sections(objects: &[elf_reader::ElfObject]) -> Vec<Vec<bool>> {
    use elf_reader::{RelocTarget, SecClass};
    let mut defs: std::collections::HashMap<&str, (usize, usize)> = std::collections::HashMap::new();
    for (oi, o) in objects.iter().enumerate() {
        for s in &o.symbols {
            // A strong definition wins, as in the layout.
            if !s.weak || !defs.contains_key(s.name.as_str()) { defs.insert(&s.name, (oi, s.section_idx)); }
        }
    }
    let mut live: Vec<Vec<bool>> = objects.iter().map(|o| vec![false; o.sections.len()]).collect();
    let mut work: Vec<(usize, usize)> = Vec::new();
    for root in ["_start", "main"] {
        if let Some(&d) = defs.get(root) { work.push(d); break; }
    }
    for (oi, o) in objects.iter().enumerate() {
        for (si, s) in o.sections.iter().enumerate() {
            if matches!(s.class, SecClass::Array(_)) { work.push((oi, si)); }
        }
    }
    while let Some((oi, si)) = work.pop() {
        if std::mem::replace(&mut live[oi][si], true) { continue; }
        for r in &objects[oi].sections[si].relocs {
            match &r.target {
                RelocTarget::Local { section, .. } => work.push((oi, *section)),
                RelocTarget::Symbol(name) => { if let Some(&d) = defs.get(name.as_str()) { work.push(d); } }
                RelocTarget::Absolute(_) => {}
            }
        }
    }
    live
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

fn load_input(path: &Path, inputs: &mut Inputs) -> Result<(), Box<dyn Error>> {
    let data = std::fs::read(path)?;
    let name = path.display().to_string();

    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("res")) {
        inputs.resources.extend(res_reader::parse_res(&data, &name)?);
        return Ok(());
    }

    if data.starts_with(b"!<arch>\n") {
        let archive = ArchiveFile::parse(data.as_slice())?;
        for member in archive.members() {
            let m = member?;
            let member_name = std::str::from_utf8(m.name()).unwrap_or("");
            if !member_name.ends_with(".o") { continue; }
            // A member sits at any offset in the archive; parse a copy,
            // which the allocator aligns as an ELF header needs.
            let obj_data = m.data(data.as_slice())?.to_vec();
            let member_path = format!("{name}({member_name})");
            inputs.members.push(elf_reader::parse_elf(&obj_data, &member_path)?);
        }
        return Ok(());
    }

    inputs.objects.push(elf_reader::parse_elf(&data, &name)?);
    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninitialized_sections_are_bounded() {
        use super::elf_reader::{zeroed_section, MAX_SECTION_BYTES};
        assert_eq!(zeroed_section(16, "a.o").unwrap(), vec![0u8; 16]);
        assert!(zeroed_section(MAX_SECTION_BYTES + 1, "a.o").is_err(), "a crafted header cannot ask for gigabytes");
        assert!(zeroed_section(u64::MAX, "a.o").is_err());
    }

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
        assert!(!map.contains_key("NonExistent"));
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
        assert_eq!(resolve_import("DOSCALLS", 234), Some(MAGIC_API_BASE + 234));
    }

    #[test]
    fn test_resolve_import_viocalls() {
        assert_eq!(resolve_import("VIOCALLS", 19), Some(MAGIC_API_BASE + VIOCALLS_BASE + 19));
    }

    #[test]
    fn test_resolve_import_sockets_and_mouse() {
        assert_eq!(resolve_import("SO32DLL", 19), Some(MAGIC_API_BASE + SO32DLL_BASE + 19));
        assert_eq!(resolve_import("TCP32DLL", 19), Some(MAGIC_API_BASE + TCP32DLL_BASE + 19));
        assert_eq!(resolve_import("MOUCALLS", 17), Some(MAGIC_API_BASE + MOUCALLS_BASE + 17));
    }

    /// Every module the embedded def names resolves, at Warpine's bases
    /// (`constants.rs`): an unknown module would link calls to the wrong
    /// address.
    #[test]
    fn test_every_def_module_resolves() {
        let def = def_parser::parse_def(include_str!("../../targets/os2api.def"));
        for (name, (module, ordinal)) in &def {
            assert!(resolve_import(module, *ordinal).is_some(), "{name}: module {module}");
        }
        assert_eq!(resolve_import("PMDRAG", 1), Some(MAGIC_API_BASE + 21504 + 1));
        assert_eq!(resolve_import("SOM", 1), Some(MAGIC_API_BASE + 22528 + 1));
        assert_eq!(resolve_import("REXX", 1), Some(MAGIC_API_BASE + 26624 + 1));
        assert_eq!(resolve_import("REXXAPI", 1), Some(MAGIC_API_BASE + 27648 + 1));
        assert_eq!(resolve_import("PMWP", 1), Some(MAGIC_API_BASE + 28672 + 1));
        assert_eq!(resolve_import("NOSUCHMOD", 1), None);
    }

    #[test]
    fn test_resolve_import_pmwin() {
        assert_eq!(resolve_import("PMWIN", 763), Some(MAGIC_API_BASE + PMWIN_BASE + 763));
    }

    use elf_reader::{ArrayKind, CommonSymbol, ElfObject, ElfSection, ElfSymbol, Reloc, RelocKind, RelocTarget, SecClass};
    use linker_state::LinkerState;
    use std::collections::HashMap;

    fn section(name: &str, class: SecClass, data: Vec<u8>, relocs: Vec<Reloc>) -> ElfSection {
        let size = data.len() as u64;
        ElfSection { name: name.into(), class, data, size, align: 4, relocs }
    }

    fn sym(name: &str, section_idx: usize, offset: u64) -> ElfSymbol {
        ElfSymbol { name: name.into(), section_idx, offset, weak: false }
    }

    fn reloc(offset: u64, kind: RelocKind, addend: i64, target: RelocTarget) -> Reloc {
        Reloc { offset, kind, addend, target }
    }

    /// One object: `.text` (8 bytes, `_start` at 0) and `.data` (8 bytes).
    fn start_object(relocs: Vec<Reloc>) -> ElfObject {
        ElfObject {
            name: "start.o".into(),
            sections: vec![section(".text", SecClass::Code, vec![0x90; 8], relocs),
                           section(".data", SecClass::Data, vec![0; 8], vec![])],
            symbols: vec![sym("_start", 0, 0)],
            ..Default::default()
        }
    }

    fn word(state: &LinkerState, va: u32) -> u32 {
        let (buf, base) = if va >= state.data_va { (&state.data, state.data_va) } else { (&state.code, state.code_va) };
        let at = (va - base) as usize;
        u32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
    }

    /// Link and return the state with the relocations applied (the LX
    /// writer's image, read back from the file's pages).
    fn link(objects: &[ElfObject], defs: &HashMap<String, (String, u32)>) -> Result<(LinkerState, Vec<u8>), String> {
        let state = LinkerState::build(objects, None).map_err(|e| e.to_string())?;
        let lx = lx_writer::write_lx(objects, &state, defs, &[])?;
        Ok((state, lx))
    }

    /// The code and data object pages of an LX image, as loaded.
    fn loaded(state: &LinkerState, lx: &[u8]) -> LinkerState {
        let hdr = LX_HEADER_OFFSET as usize;
        let data_pages = u32::from_le_bytes(lx[hdr + 0x80..hdr + 0x84].try_into().unwrap()) as usize;
        let code_len = state.code.len();
        LinkerState {
            code: lx[data_pages..data_pages + code_len].to_vec(),
            data: lx[data_pages + code_len..data_pages + code_len + state.data.len()].to_vec(),
            bss_size: state.bss_size, code_va: state.code_va, data_va: state.data_va, stack_va: state.stack_va,
            section_va: state.section_va.clone(), symbol_table: state.symbol_table.clone(), entry_va: state.entry_va,
        }
    }

    /// A relocation against a section or local symbol means that section
    /// of its own object, never a same-named one of another (two `.data`
    /// here), and the REL addend read from the bytes counts.
    #[test]
    fn local_targets_stay_in_their_own_object() {
        let a = start_object(vec![reloc(0, RelocKind::Abs32, 2, RelocTarget::Local { section: 1, offset: 4 })]);
        let mut b = start_object(vec![reloc(4, RelocKind::Abs32, 0, RelocTarget::Local { section: 1, offset: 0 })]);
        b.symbols.clear();
        b.name = "b.o".into();
        let objects = [a, b];
        let (state, lx) = link(&objects, &HashMap::new()).unwrap();
        let img = loaded(&state, &lx);
        let a_data = state.section_va[0][1].unwrap();
        let b_data = state.section_va[1][1].unwrap();
        assert_ne!(a_data, b_data);
        assert_eq!(word(&img, state.section_va[0][0].unwrap()), a_data + 6, "a's .data + 4 + addend 2");
        assert_eq!(word(&img, state.section_va[1][0].unwrap() + 4), b_data, "b's own .data");
    }

    /// PC-relative: S + A − P, the addend (−4 for a call) as given.
    #[test]
    fn pc_relative_relocations_use_the_addend() {
        let a = start_object(vec![reloc(1, RelocKind::Pc32, -4, RelocTarget::Symbol("f".into()))]);
        let b = ElfObject {
            name: "f.o".into(),
            sections: vec![section(".text.f", SecClass::Code, vec![0xC3; 4], vec![])],
            symbols: vec![sym("f", 0, 0)],
            ..Default::default()
        };
        let objects = [a, b];
        let (state, lx) = link(&objects, &HashMap::new()).unwrap();
        let p = state.code_va + 1;
        let f = state.symbol_table["f"];
        assert_eq!(word(&loaded(&state, &lx), p), f.wrapping_sub(4).wrapping_sub(p));
    }

    /// Data follows the code on a 64 KB boundary, the stack follows the
    /// data; a program past the program area is refused.
    #[test]
    fn the_layout_follows_the_sizes() {
        let mut big = start_object(vec![]);
        big.sections[0].data = vec![0x90; 0x6_0001];
        big.sections[0].size = 0x6_0001;
        let state = LinkerState::build(&[big], None).unwrap();
        assert_eq!((state.code_va, state.data_va), (CODE_OBJECT_BASE, 0x8_0000), "past 0x70001");
        assert_eq!(state.stack_va, 0x9_0000);
        let mut huge = start_object(vec![]);
        huge.sections.push(ElfSection { name: ".bss".into(), class: SecClass::Bss, data: vec![], size: 0x0100_0000, align: 4, relocs: vec![] });
        assert!(LinkerState::build(&[huge], None).is_err(), "16 MB of zeros do not fit");
    }

    /// Undefined symbols are an error naming them; an undefined weak one
    /// is 0; OS/2 APIs come from the DEF map.
    #[test]
    fn undefined_weak_and_imported_symbols() {
        let mut a = start_object(vec![
            reloc(0, RelocKind::Abs32, 0, RelocTarget::Symbol("missing".into())),
            reloc(4, RelocKind::Abs32, 0, RelocTarget::Symbol("also_missing".into())),
        ]);
        assert_eq!(link(std::slice::from_ref(&a), &HashMap::new()).err(),
                   Some("undefined symbols: also_missing, missing".into()));
        a.weak_refs = vec!["missing".into(), "also_missing".into()];
        let (state, lx) = link(std::slice::from_ref(&a), &HashMap::new()).unwrap();
        assert_eq!(word(&loaded(&state, &lx), state.code_va), 0);
        let defs = HashMap::from([("missing".to_string(), ("DOSCALLS".to_string(), 282))]);
        a.weak_refs.clear();
        a.sections[0].relocs.truncate(1);
        let (state, lx) = link(std::slice::from_ref(&a), &defs).unwrap();
        assert_eq!(word(&loaded(&state, &lx), state.code_va), MAGIC_API_BASE + 282);
    }

    /// A strong definition wins over a weak one; two strong ones are an
    /// error; a definition wins over a common symbol, and commons merge.
    #[test]
    fn symbol_precedence_and_commons() {
        let mut a = start_object(vec![]);
        a.symbols.push(ElfSymbol { name: "v".into(), section_idx: 1, offset: 0, weak: true });
        a.commons = vec![CommonSymbol { name: "c".into(), size: 4, align: 4 },
                         CommonSymbol { name: "d".into(), size: 4, align: 4 }];
        let mut b = ElfObject {
            name: "b.o".into(),
            sections: vec![section(".data", SecClass::Data, vec![0; 8], vec![])],
            symbols: vec![sym("v", 0, 4), sym("d", 0, 0)],
            commons: vec![CommonSymbol { name: "c".into(), size: 16, align: 8 }],
            ..Default::default()
        };
        let state = LinkerState::build(&[start_object(vec![]), b.clone_for_test()], None).unwrap();
        assert!(state.symbol_table.contains_key("v"));
        let state = LinkerState::build(&[a.clone_for_test(), b.clone_for_test()], None).unwrap();
        assert_eq!(state.symbol_table["v"], state.section_va[1][0].unwrap() + 4, "the strong one");
        assert_eq!(state.symbol_table["d"], state.section_va[1][0].unwrap(), "a definition over a common");
        let c = state.symbol_table["c"];
        assert!(c >= state.data_va + state.data.len() as u32 && c.is_multiple_of(8), "a common in the zeros");
        assert!(state.bss_size >= 16);
        b.symbols[0].weak = false;
        a.symbols[1].weak = false;
        assert!(LinkerState::build(&[a, b], None).is_err(), "two strong definitions");
    }

    /// The init / fini arrays lie in the data object between their start
    /// and end symbols, in order.
    #[test]
    fn init_arrays_get_bounds() {
        let mut a = start_object(vec![]);
        a.sections.push(section(".init_array", SecClass::Array(ArrayKind::Init), vec![1, 0, 0, 0, 2, 0, 0, 0], vec![]));
        a.sections.push(section(".fini_array", SecClass::Array(ArrayKind::Fini), vec![3, 0, 0, 0], vec![]));
        let state = LinkerState::build(&[a], None).unwrap();
        let t = &state.symbol_table;
        assert_eq!(t["__init_array_end"] - t["__init_array_start"], 8);
        assert_eq!(t["__fini_array_end"] - t["__fini_array_start"], 4);
        assert_eq!(t["__preinit_array_end"], t["__preinit_array_start"]);
        assert_eq!(word(&state, t["__init_array_start"]), 1);
        assert_eq!(word(&state, t["__fini_array_start"]), 3);
    }

    /// `--gc-sections`: only sections reached from `_start` and the arrays
    /// are linked, so an unreached reference to a missing symbol is fine.
    #[test]
    fn unreached_sections_are_dropped() {
        let mut a = start_object(vec![reloc(0, RelocKind::Abs32, 0, RelocTarget::Symbol("used".into()))]);
        a.sections.push(section(".text.used", SecClass::Code, vec![0xC3; 4], vec![]));
        a.sections.push(section(".text.unused", SecClass::Code, vec![0xC3; 4],
                                vec![reloc(0, RelocKind::Abs32, 0, RelocTarget::Symbol("nowhere".into()))]));
        a.symbols.push(sym("used", 2, 0));
        a.symbols.push(sym("unused", 3, 0));
        let objects = [a];
        let live = live_sections(&objects);
        assert_eq!(live[0], vec![true, false, true, false], "_start's .text and what it uses");
        let state = LinkerState::build(&objects, Some(&live)).unwrap();
        assert!(lx_writer::write_lx(&objects, &state, &HashMap::new(), &[]).is_ok());
        assert!(!state.symbol_table.contains_key("unused"));
        assert!(link(&objects, &HashMap::new()).is_err(), "without GC, nowhere is undefined");
    }

    /// Archive members are linked only when they define a needed symbol,
    /// and then also what they need in turn.
    #[test]
    fn archive_members_are_pulled_in_on_demand() {
        let a = start_object(vec![reloc(0, RelocKind::Abs32, 0, RelocTarget::Symbol("f".into()))]);
        let member = |name: &str, defines: &str, needs: Option<&str>| ElfObject {
            name: name.into(),
            sections: vec![section(".text", SecClass::Code, vec![0xC3; 4],
                needs.map(|n| vec![reloc(0, RelocKind::Abs32, 0, RelocTarget::Symbol(n.into()))]).unwrap_or_default())],
            symbols: vec![sym(defines, 0, 0)],
            ..Default::default()
        };
        let inputs = Inputs { objects: vec![a], members: vec![
            member("g.o", "g", None), member("f.o", "f", Some("g")), member("h.o", "h", None)], resources: Vec::new() };
        let names: Vec<String> = select_objects(inputs, &HashMap::new()).into_iter().map(|o| o.name).collect();
        assert_eq!(names, ["start.o", "f.o", "g.o"]);
    }

    impl ElfObject {
        fn clone_for_test(&self) -> ElfObject {
            ElfObject {
                name: self.name.clone(),
                sections: self.sections.iter().map(|s| ElfSection {
                    name: s.name.clone(), class: s.class, data: s.data.clone(), size: s.size, align: s.align, relocs: s.relocs.clone(),
                }).collect(),
                symbols: self.symbols.clone(), commons: self.commons.clone(),
                undefined: self.undefined.clone(), weak_refs: self.weak_refs.clone(),
            }
        }
    }

    #[test]
    fn test_lx_roundtrip() {
        let objects = vec![ElfObject {
            name: "a.o".into(),
            sections: vec![section(".text", SecClass::Code, vec![0xF4], vec![])], // HLT
            symbols: vec![sym("_start", 0, 0)],
            ..Default::default()
        }];
        let state = LinkerState::build(&objects, None).expect("build state");
        let lx_bytes = lx_writer::write_lx(&objects, &state, &HashMap::new(), &[]).expect("write");

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

    /// Resources from .res files land in a fourth object flagged
    /// OBJ_RESOURCE, found through the LX resource table (ADR-0099).
    #[test]
    fn resources_get_an_object_and_a_table() {
        use res_reader::ResEntry;
        let objects = vec![ElfObject {
            name: "a.o".into(),
            sections: vec![section(".text", SecClass::Code, vec![0xF4], vec![])],
            symbols: vec![sym("_start", 0, 0)],
            ..Default::default()
        }];
        let state = LinkerState::build(&objects, None).expect("build state");
        let resources = vec![ResEntry { type_id: 3, name: 100, data: vec![1, 2, 3] },
                             ResEntry { type_id: 4, name: 101, data: vec![9; 5000] }];
        let lx = lx_writer::write_lx(&objects, &state, &HashMap::new(), &resources).expect("write");
        let hdr = LX_HEADER_OFFSET as usize;
        let u32_at = |o: usize| u32::from_le_bytes(lx[o..o + 4].try_into().unwrap());
        let u16_at = |o: usize| u16::from_le_bytes(lx[o..o + 2].try_into().unwrap());
        assert_eq!(u32_at(hdr + 0x44), 4, "code, data, stack and resources");
        assert_eq!(u32_at(hdr + 0x54), 2, "resource_count");
        let table = hdr + u32_at(hdr + 0x50) as usize;
        let obj4 = hdr + u32_at(hdr + 0x40) as usize + 3 * 24;
        assert_eq!(u32_at(obj4 + 8), OBJ_READ | OBJ_RESOURCE);
        assert_eq!(u32_at(obj4), 4 + 5000, "the second resource starts 4-byte aligned");
        assert_eq!(u32_at(obj4 + 16), 2, "two pages");
        // Entry 2: type 4, name 101, 5000 bytes, object 4, offset 4.
        let e2 = table + 14;
        assert_eq!((u16_at(e2), u16_at(e2 + 2), u32_at(e2 + 4), u16_at(e2 + 8), u32_at(e2 + 10)), (4, 101, 5000, 4, 4));
        // The object's first page holds the bytes, at the end of the file.
        let first = lx.len() - (4 + 5000);
        assert_eq!(&lx[first..first + 3], &[1, 2, 3]);
        assert_eq!(lx[first + 4], 9);

        let dup = vec![ResEntry { type_id: 3, name: 1, data: vec![] }, ResEntry { type_id: 3, name: 1, data: vec![] }];
        let e = lx_writer::write_lx(&objects, &state, &HashMap::new(), &dup).unwrap_err();
        assert!(e.contains("given twice"), "{e}");
    }

    #[test]
    fn res_files_parse_and_bad_ones_are_refused() {
        let mut res = vec![0xFF, 3, 0, 0xFF, 100, 0, 0x30, 0x10, 2, 0, 0, 0, b'h', b'i'];
        assert_eq!(res_reader::parse_res(&res, "x.res").unwrap(),
                   [res_reader::ResEntry { type_id: 3, name: 100, data: b"hi".to_vec() }]);
        res.pop();
        assert!(res_reader::parse_res(&res, "x.res").unwrap_err().contains("cut short"));
        let named = [b'M', b'E', 0, 0xFF, 1, 0, 0, 0, 0, 0, 0, 0];
        assert!(res_reader::parse_res(&named, "x.res").unwrap_err().contains("numeric"));
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
        assert_eq!(resolve_import("DOSCALLS", 282), Some(MAGIC_API_BASE + 282));
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
