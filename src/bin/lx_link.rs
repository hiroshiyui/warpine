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

#[path = "../loader/msgfile.rs"]
#[allow(dead_code)] // the reader's lookups are Warpine's
mod msgfile;

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
/// The LX header's size: through `STACKSIZE` (ACh) and the reserved
/// bytes after it, as Open Watcom's linker writes it (0xC4).
const LX_HEADER_SIZE: u32 = 0xC4;

// LX object flags
const OBJ_READ: u32 = 0x0001;
const OBJ_WRITE: u32 = 0x0002;
const OBJ_EXEC: u32 = 0x0004;
/// The object holds resources (reached through the resource table).
const OBJ_RESOURCE: u32 = 0x0008;
/// A DLL's initialisation and termination routine (ADR-0101).
const DLL_ENTRY: &str = "_DLL_InitTerm";
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
        /// Make a DLL (`--dll`, or an output ending in .dll; ADR-0101).
        pub dll: bool,
        /// `--name`: the DLL's module name (default: the output's stem).
        pub module_name: Option<String>,
        /// `--export NAME[.ORDINAL]` (repeatable, or comma-separated).
        pub exports: Vec<(String, Option<u32>)>,
        pub init_instance: bool,
        pub term_instance: bool,
        /// `--debug-file PATH`: where the debug companion goes (ADR-0102;
        /// default: the output with the extension .dbg, written when an
        /// input has debugging information); `--no-debug-file` writes none.
        pub debug_file: Option<PathBuf>,
        pub no_debug_file: bool,
    }

    /// `NAME` or `NAME.ORDINAL`.
    pub fn parse_export(s: &str) -> Option<(String, Option<u32>)> {
        let (name, ordinal) = match s.rsplit_once('.') {
            Some((n, o)) if !o.is_empty() && o.bytes().all(|b| b.is_ascii_digit()) => (n, Some(o.parse().ok()?)),
            _ => (s, None),
        };
        (!name.is_empty()).then(|| (name.to_string(), ordinal))
    }

    pub fn parse_args(raw: &[String]) -> Args {
        let mut inputs = Vec::new();
        let mut output = PathBuf::from("a.out");
        let mut def_file = None;
        let mut gc_sections = false;
        let mut dll = false;
        let mut module_name = None;
        let mut exports = Vec::new();
        let mut init_instance = false;
        let mut term_instance = false;
        let mut debug_file = None;
        let mut no_debug_file = false;
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
            } else if arg == "--dll" {
                dll = true;
            } else if arg == "--initinstance" {
                init_instance = true;
            } else if arg == "--terminstance" {
                term_instance = true;
            } else if arg == "--name" || arg == "--export" {
                i += 1;
                let Some(v) = raw.get(i) else { break };
                if arg == "--name" {
                    module_name = Some(v.clone());
                } else {
                    exports.extend(v.split(',').filter_map(parse_export));
                }
            } else if arg == "--debug-file" {
                i += 1;
                let Some(v) = raw.get(i) else { break };
                debug_file = Some(PathBuf::from(v));
            } else if arg == "--no-debug-file" {
                no_debug_file = true;
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
                // Positional: only collect .o / .rlib / .a / .res / .dll /
                // .msg files
                let lower = arg.to_ascii_lowercase();
                if [".o", ".rlib", ".a", ".res", ".dll", ".msg"].iter().any(|e| lower.ends_with(e)) {
                    inputs.push(PathBuf::from(arg));
                }
            }
            i += 1;
        }
        let dll = dll || output.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll"));
        Args { inputs, output, def_file, gc_sections, dll, module_name, exports, init_instance, term_instance,
               debug_file, no_debug_file }
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

// ─── mod dll_reader ───────────────────────────────────────────────────────────

/// The exports of an LX DLL given as an input (ADR-0101): its module name
/// and its exported names with their ordinals, from the resident and
/// non-resident name tables. A program linked with it imports those names
/// by ordinal; the loader finds the DLL when the program starts.
mod dll_reader {
    /// LX header fields (offsets from the LX header).
    const RESIDENT_NAMES: usize = 0x58;
    const NONRESIDENT_NAMES: usize = 0x88;
    const NONRESIDENT_LEN: usize = 0x8C;
    /// Where the MZ header keeps the LX header's offset.
    const LX_OFFSET_AT: usize = 0x3C;

    /// Name entries from `data[at..]` until a zero length or `end`.
    fn names(data: &[u8], mut at: usize, end: usize, source: &str) -> Result<Vec<(String, u16)>, String> {
        let mut out = Vec::new();
        while at < end.min(data.len()) {
            let len = data[at] as usize;
            if len == 0 { break; }
            let entry = data.get(at + 1..at + 3 + len).ok_or_else(|| format!("{source}: a name table is cut short"))?;
            let name = String::from_utf8_lossy(&entry[..len]).into_owned();
            out.push((name, u16::from_le_bytes([entry[len], entry[len + 1]])));
            at += 3 + len;
        }
        Ok(out)
    }

    fn u32_at(data: &[u8], at: usize, source: &str) -> Result<u32, String> {
        data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| format!("{source}: not an LX file"))
    }

    /// The module name and the (name, ordinal) exports.
    pub fn read_exports(data: &[u8], source: &str) -> Result<(String, Vec<(String, u32)>), String> {
        if !data.starts_with(b"MZ") { return Err(format!("{source}: not an LX file")); }
        let lx = u32_at(data, LX_OFFSET_AT, source)? as usize;
        if data.get(lx..lx + 2) != Some(b"LX") { return Err(format!("{source}: not an LX file")); }
        let resident = names(data, lx + u32_at(data, lx + RESIDENT_NAMES, source)? as usize, data.len(), source)?;
        let nr_at = u32_at(data, lx + NONRESIDENT_NAMES, source)? as usize;
        let nr_len = u32_at(data, lx + NONRESIDENT_LEN, source)? as usize;
        let nonresident = if nr_at == 0 { Vec::new() } else { names(data, nr_at, nr_at + nr_len, source)? };
        let module = resident.iter().find(|(_, o)| *o == 0).map(|(n, _)| n.clone())
            .ok_or_else(|| format!("{source}: no module name"))?;
        let exports = resident.into_iter().chain(nonresident)
            .filter(|(_, o)| *o != 0).map(|(n, o)| (n, u32::from(o))).collect();
        Ok((module.to_ascii_uppercase(), exports))
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
        /// DWARF debugging information (`.debug_*`, not allocated): never
        /// in the program, only in its debug companion (ADR-0102).
        Debug,
        /// Unwind tables (`.eh_frame`) for C++ exceptions (ADR-0103):
        /// kept only for the functions the link keeps, and placed together
        /// between `__EH_FRAME_BEGIN__` and `__EH_FRAME_END__`.
        EhFrame,
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

    /// The largest alignment a section or common symbol may ask for: the
    /// objects start on [`super::OBJECT_ALIGN`] boundaries, so no larger
    /// one could be kept (and an input's own could make the image huge).
    pub const MAX_ALIGN: u64 = super::OBJECT_ALIGN as u64;

    /// `align` of `what`, at least 1, or an error above [`MAX_ALIGN`].
    pub fn alignment(align: u64, what: &str, source_name: &str) -> Result<u64, Box<dyn super::Error>> {
        if align > MAX_ALIGN {
            return Err(format!("{source_name}: {what} asks for an alignment of {align} bytes (at most {MAX_ALIGN})").into());
        }
        Ok(align.max(1))
    }

    /// The zero bytes of an uninitialized section of `size` bytes, or an
    /// error when the input asks for more than `MAX_SECTION_BYTES`.
    pub fn zeroed_section(size: u64, source_name: &str) -> Result<Vec<u8>, Box<dyn super::Error>> {
        if size > MAX_SECTION_BYTES {
            return Err(format!("{source_name}: an uninitialized section of {size} bytes (at most {MAX_SECTION_BYTES})").into());
        }
        Ok(vec![0u8; size as usize])
    }

    /// The class of a section, `None` for one that is not linked (notes,
    /// `.comment` and other unallocated sections but debugging information;
    /// `.eh_frame_hdr`, which the unwinder does without).
    fn classify(name: &str, kind: SectionKind, sh_flags: u64) -> Option<SecClass> {
        if sh_flags & SHF_ALLOC == 0 {
            return name.starts_with(".debug_").then_some(SecClass::Debug);
        }
        if name == ".eh_frame" {
            return Some(SecClass::EhFrame);
        }
        if name.starts_with(".eh_frame") || name.starts_with(".note") {
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
            let align = alignment(section.align(), &format!("section {name}"), source_name)?;
            sections.push(ElfSection { name, class, data, size, align, relocs: Vec::new() });
        }

        for section in obj.sections() {
            let Some(&local) = index_map.get(&section.index()) else { continue };
            for (offset, reloc) in section.relocations() {
                let RelocationFlags::Elf { r_type } = reloc.flags() else {
                    return Err(format!("{source_name}: not an ELF relocation").into());
                };
                let debug = sections[local].class == SecClass::Debug;
                let kind = match r_type {
                    R_386_32 => RelocKind::Abs32,
                    // A PLT reference is a direct call in a static link.
                    R_386_PC32 | R_386_PLT32 => RelocKind::Pc32,
                    // Debugging information may describe what the program
                    // cannot have (thread-local variables): its value is
                    // left as it is.
                    _ if debug => continue,
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
                                    // Debugging information about a section that is not linked.
                                    if debug { sections[local].relocs.push(Reloc { offset, kind, addend, target: RelocTarget::Absolute(0) }); continue; }
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
                        None if debug => RelocTarget::Absolute(0),
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
                SymbolSection::Common => {
                    // A common symbol's value is its alignment (System V ABI).
                    let what = format!("common symbol {name}");
                    let align = alignment(sym.address(), &what, source_name)?;
                    if sym.size() > MAX_SECTION_BYTES {
                        return Err(format!("{source_name}: {what} of {} bytes (at most {MAX_SECTION_BYTES})", sym.size()).into());
                    }
                    commons.push(CommonSymbol { name, size: sym.size(), align });
                }
                SymbolSection::Undefined => {
                    if sym.is_weak() { weak_refs.push(name) } else { undefined.push(name) }
                }
                _ => {}
            }
        }
        Ok(ElfObject { name: source_name.to_string(), sections, symbols, commons, undefined, weak_refs })
    }
}

// ─── mod eh_frame ────────────────────────────────────────────────────────────

/// `.eh_frame` (ADR-0103): the call-frame information C++ exceptions
/// unwind with. An object's `.eh_frame` is a series of records, CIEs
/// (common information: the personality routine) and FDEs (one function
/// each, pointing back at their CIE, with the function's LSDA). The link
/// keeps the FDEs of the functions it keeps, the CIEs they use, and what
/// they refer to.
mod eh_frame {
    use super::elf_reader::{ElfObject, ElfSection, Reloc, RelocTarget, SecClass};
    use std::collections::HashMap;

    /// One record: where it starts and its whole length (with the length
    /// word), and for an FDE the offset of its CIE.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Record { pub offset: usize, pub len: usize, pub cie: Option<usize> }

    /// The records of a section, up to a zero terminator or its end.
    pub fn records(data: &[u8]) -> Result<Vec<Record>, String> {
        let mut out = Vec::new();
        let mut at = 0usize;
        while at + 4 <= data.len() {
            let len = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            if len == 0 { break; }
            if len == 0xFFFF_FFFF { return Err("a 64-bit .eh_frame record".into()); }
            let end = at + 4 + len;
            if end > data.len() || len < 4 { return Err(format!(".eh_frame record at {at:#x} is cut short")); }
            let id = u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
            let cie = if id == 0 { None } else {
                Some((at + 4).checked_sub(id).ok_or_else(|| format!("FDE at {at:#x} points before the section"))?)
            };
            out.push(Record { offset: at, len: 4 + len, cie });
            at = end;
        }
        Ok(out)
    }

    fn relocs_in<'a>(sec: &'a ElfSection, r: &Record) -> impl Iterator<Item = &'a Reloc> {
        let (start, end) = (r.offset as u64, (r.offset + r.len) as u64);
        sec.relocs.iter().filter(move |x| (start..end).contains(&x.offset))
    }

    /// The section an FDE describes: the target of its pc_begin (8 bytes
    /// in), a section of the same object.
    fn fde_target(o: &ElfObject, sec: &ElfSection, r: &Record) -> Option<usize> {
        let reloc = sec.relocs.iter().find(|x| x.offset == r.offset as u64 + 8)?;
        match &reloc.target {
            RelocTarget::Local { section, .. } => Some(*section),
            RelocTarget::Symbol(name) => o.symbols.iter().find(|s| &s.name == name).map(|s| s.section_idx),
            RelocTarget::Absolute(_) => None,
        }
    }

    /// For each function section with an FDE, `(object, section)`, what
    /// its FDE and CIE refer to besides the function itself.
    pub fn dependencies(objects: &[ElfObject]) -> HashMap<(usize, usize), Vec<RelocTarget>> {
        let mut deps: HashMap<(usize, usize), Vec<RelocTarget>> = HashMap::new();
        for (oi, o) in objects.iter().enumerate() {
            for sec in o.sections.iter().filter(|s| s.class == SecClass::EhFrame) {
                let Ok(recs) = records(&sec.data) else { continue };
                for r in recs.iter().filter(|r| r.cie.is_some()) {
                    let Some(target) = fde_target(o, sec, r) else { continue };
                    let cie = recs.iter().find(|c| Some(c.offset) == r.cie);
                    let entry = deps.entry((oi, target)).or_default();
                    for x in relocs_in(sec, r).filter(|x| x.offset != r.offset as u64 + 8)
                        .chain(cie.into_iter().flat_map(|c| relocs_in(sec, c))) {
                        entry.push(x.target.clone());
                    }
                }
            }
        }
        deps
    }

    /// Keep the FDEs of live functions and the CIEs they use: each
    /// `.eh_frame` is rebuilt from those records (an FDE's CIE pointer
    /// recomputed) with its relocations moved along.
    pub fn prune(objects: &mut [ElfObject], live: &[Vec<bool>]) -> Result<(), String> {
        for (oi, o) in objects.iter_mut().enumerate() {
            let name = o.name.clone();
            let keep_fde: Vec<Vec<bool>> = o.sections.iter().map(|sec| {
                if sec.class != SecClass::EhFrame { return Vec::new(); }
                records(&sec.data).unwrap_or_default().iter().map(|r| match r.cie {
                    None => false,
                    Some(_) => fde_target(o, sec, r).is_some_and(|t| live[oi].get(t).copied().unwrap_or(false)),
                }).collect()
            }).collect();
            for (si, sec) in o.sections.iter_mut().enumerate() {
                if sec.class != SecClass::EhFrame { continue; }
                let recs = records(&sec.data).map_err(|e| format!("{name}: {e}"))?;
                let used_cies: Vec<usize> = recs.iter().zip(&keep_fde[si]).filter(|(_, k)| **k).filter_map(|(r, _)| r.cie).collect();
                let mut data = Vec::new();
                let mut moved: HashMap<usize, usize> = HashMap::new();
                for (r, keep) in recs.iter().zip(&keep_fde[si]) {
                    let kept = if r.cie.is_none() { used_cies.contains(&r.offset) } else { *keep };
                    if !kept { continue; }
                    let at = data.len();
                    moved.insert(r.offset, at);
                    data.extend_from_slice(&sec.data[r.offset..r.offset + r.len]);
                    if let Some(cie) = r.cie {
                        let new_cie = *moved.get(&cie).ok_or_else(|| format!("{name}: an FDE before its CIE"))?;
                        let pointer = (at + 4 - new_cie) as u32;
                        data[at + 4..at + 8].copy_from_slice(&pointer.to_le_bytes());
                    }
                }
                let relocs = std::mem::take(&mut sec.relocs).into_iter().filter_map(|mut x| {
                    let r = recs.iter().find(|r| (r.offset as u64..(r.offset + r.len) as u64).contains(&x.offset))?;
                    let new = moved.get(&r.offset)?;
                    x.offset = x.offset - r.offset as u64 + *new as u64;
                    Some(x)
                }).collect();
                sec.size = data.len() as u64;
                sec.data = data;
                sec.relocs = relocs;
            }
        }
        Ok(())
    }
}

// ─── mod debug_elf ───────────────────────────────────────────────────────────

/// The debug companion (ADR-0102): an ELF32 file for GDB beside the LX
/// program, holding the linked code and data at the addresses they have
/// in the program, the global symbols, and the inputs' DWARF sections
/// joined by name with their relocations applied against the final
/// layout. `symbol-file prog.dbg` then gives Warpine's GDB stub source
/// lines, types and variables. A DLL's companion, at the addresses it was
/// linked for, is listed by the stub with the DLL's load addresses
/// (ADR-0104), and GDB moves it there itself.
mod debug_elf {
    use super::elf_reader::{ElfObject, RelocKind, RelocTarget, SecClass};
    use super::linker_state::LinkerState;
    use std::collections::BTreeMap;

    const SHT_PROGBITS: u32 = 1;
    const SHT_SYMTAB: u32 = 2;
    const SHT_STRTAB: u32 = 3;
    const SHT_NOBITS: u32 = 8;
    const SHF_WRITE: u32 = 1;
    const SHF_ALLOC: u32 = 2;
    const SHF_EXECINSTR: u32 = 4;
    const PT_LOAD: u32 = 1;
    const PF_X: u32 = 1;
    const PF_W: u32 = 2;
    const PF_R: u32 = 4;
    const STB_GLOBAL: u8 = 1;
    const STT_OBJECT: u8 = 1;
    const STT_FUNC: u8 = 2;
    const EHDR_SIZE: usize = 52;
    const PHDR_SIZE: usize = 32;
    const SHDR_SIZE: usize = 40;
    const SYM_SIZE: usize = 16;

    /// Whether any object carries debugging information.
    pub fn has_debug_info(objects: &[ElfObject]) -> bool {
        objects.iter().any(|o| o.sections.iter().any(|s| s.class == SecClass::Debug))
    }

    /// Where each input debug section starts in the output section of its
    /// name (`[object][section]`), joining them in input order as `ld`
    /// does, and each output section's size.
    /// `[object][section]` → offset in its output debug section.
    pub type Starts = Vec<Vec<Option<u32>>>;

    pub fn placements(objects: &[ElfObject]) -> Result<(Starts, BTreeMap<String, usize>), String> {
        let mut sizes: BTreeMap<String, usize> = BTreeMap::new();
        let mut start: Starts = objects.iter().map(|o| vec![None; o.sections.len()]).collect();
        for (oi, o) in objects.iter().enumerate() {
            for (si, sec) in o.sections.iter().enumerate().filter(|(_, s)| s.class == SecClass::Debug) {
                let size = sizes.entry(sec.name.clone()).or_default();
                start[oi][si] = Some(u32::try_from(*size).map_err(|_| format!("{} is larger than 4 GB", sec.name))?);
                *size += sec.data.len();
            }
        }
        Ok((start, sizes))
    }

    /// Join the debug sections by name, in input order (as `ld` does), and
    /// apply their relocations: a place in another debug section becomes
    /// its offset in the joined section, a place in the program its
    /// address; a section the link dropped (or an undefined symbol) is 0.
    pub fn join(objects: &[ElfObject], state: &LinkerState) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let (start, sizes) = placements(objects)?;
        let mut sections: BTreeMap<String, Vec<u8>> =
            sizes.into_iter().map(|(name, size)| (name, Vec::with_capacity(size))).collect();
        for o in objects {
            for sec in o.sections.iter().filter(|s| s.class == SecClass::Debug) {
                if let Some(out) = sections.get_mut(&sec.name) { out.extend_from_slice(&sec.data); }
            }
        }
        for (oi, o) in objects.iter().enumerate() {
            for (si, sec) in o.sections.iter().enumerate().filter(|(_, s)| s.class == SecClass::Debug) {
                let base = start[oi][si].unwrap_or(0) as usize;
                for r in &sec.relocs {
                    if r.kind != RelocKind::Abs32 { continue; }
                    let target: i64 = match &r.target {
                        RelocTarget::Local { section, offset } => {
                            let in_debug = start[oi].get(*section).copied().flatten();
                            match in_debug.or_else(|| state.section_va[oi].get(*section).copied().flatten()) {
                                Some(at) => i64::from(at) + *offset as i64,
                                None => { patch(sections.get_mut(&sec.name), base + r.offset as usize, 0); continue; }
                            }
                        }
                        RelocTarget::Symbol(name) => match state.symbol_table.get(name) {
                            Some(&va) => i64::from(va),
                            None => { patch(sections.get_mut(&sec.name), base + r.offset as usize, 0); continue; }
                        },
                        RelocTarget::Absolute(v) => *v as i64,
                    };
                    patch(sections.get_mut(&sec.name), base + r.offset as usize, (target + r.addend) as u32);
                }
            }
        }
        Ok(sections)
    }

    fn patch(section: Option<&mut Vec<u8>>, at: usize, value: u32) {
        if let Some(bytes) = section.and_then(|s| s.get_mut(at..at + 4)) {
            bytes.copy_from_slice(&value.to_le_bytes());
        }
    }

    struct Shdr { name: u32, kind: u32, flags: u32, addr: u32, offset: u32, size: u32, link: u32, info: u32, align: u32, entsize: u32 }

    fn add_name(strtab: &mut Vec<u8>, name: &str) -> u32 {
        let at = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        at
    }

    /// The companion file: `code` and `data` are the program's linked
    /// bytes (relocations applied, as in its pages).
    pub fn write(objects: &[ElfObject], state: &LinkerState, code: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
        let joined = join(objects, state)?;
        let bss_va = state.data_va + data.len() as u32;

        // Contents after the headers: code, data, the debug sections, the
        // symbol and string tables, then the section headers.
        let mut out = vec![0u8; EHDR_SIZE + 2 * PHDR_SIZE];
        let mut shstrtab = vec![0u8];
        let mut shdrs = vec![Shdr { name: 0, kind: 0, flags: 0, addr: 0, offset: 0, size: 0, link: 0, info: 0, align: 0, entsize: 0 }];
        let place = |out: &mut Vec<u8>, bytes: &[u8], align: usize| {
            while !out.len().is_multiple_of(align) { out.push(0); }
            let at = out.len() as u32;
            out.extend_from_slice(bytes);
            at
        };
        let code_off = place(&mut out, code, 16);
        shdrs.push(Shdr { name: add_name(&mut shstrtab, ".text"), kind: SHT_PROGBITS, flags: SHF_ALLOC | SHF_EXECINSTR,
                          addr: state.code_va, offset: code_off, size: code.len() as u32, link: 0, info: 0, align: 16, entsize: 0 });
        let data_off = place(&mut out, data, 16);
        shdrs.push(Shdr { name: add_name(&mut shstrtab, ".data"), kind: SHT_PROGBITS, flags: SHF_ALLOC | SHF_WRITE,
                          addr: state.data_va, offset: data_off, size: data.len() as u32, link: 0, info: 0, align: 16, entsize: 0 });
        shdrs.push(Shdr { name: add_name(&mut shstrtab, ".bss"), kind: SHT_NOBITS, flags: SHF_ALLOC | SHF_WRITE,
                          addr: bss_va, offset: out.len() as u32, size: state.bss_size, link: 0, info: 0, align: 16, entsize: 0 });
        const TEXT: u16 = 1;
        const DATA: u16 = 2;
        const BSS: u16 = 3;
        for (name, bytes) in &joined {
            let off = place(&mut out, bytes, 1);
            shdrs.push(Shdr { name: add_name(&mut shstrtab, name), kind: SHT_PROGBITS, flags: 0, addr: 0, offset: off,
                              size: bytes.len() as u32, link: 0, info: 0, align: 1, entsize: 0 });
        }

        // The global symbols, sorted by address.
        let mut strtab = vec![0u8];
        let mut symtab = vec![0u8; SYM_SIZE];
        let mut symbols: Vec<(&String, &u32)> = state.symbol_table.iter().collect();
        symbols.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)));
        let code_end = state.code_va + code.len() as u32;
        for (name, &va) in symbols {
            let (shndx, kind) = if (state.code_va..code_end).contains(&va) { (TEXT, STT_FUNC) }
                else if (state.data_va..bss_va).contains(&va) { (DATA, STT_OBJECT) }
                else if (bss_va..bss_va + state.bss_size).contains(&va) { (BSS, STT_OBJECT) }
                else { continue };
            symtab.extend_from_slice(&add_name(&mut strtab, name).to_le_bytes());
            symtab.extend_from_slice(&va.to_le_bytes());
            symtab.extend_from_slice(&0u32.to_le_bytes());
            symtab.push((STB_GLOBAL << 4) | kind);
            symtab.push(0);
            symtab.extend_from_slice(&shndx.to_le_bytes());
        }
        let symtab_idx = shdrs.len() as u32;
        let sym_off = place(&mut out, &symtab, 4);
        shdrs.push(Shdr { name: add_name(&mut shstrtab, ".symtab"), kind: SHT_SYMTAB, flags: 0, addr: 0, offset: sym_off,
                          size: symtab.len() as u32, link: symtab_idx + 1, info: 1, align: 4, entsize: SYM_SIZE as u32 });
        let str_off = place(&mut out, &strtab, 1);
        shdrs.push(Shdr { name: add_name(&mut shstrtab, ".strtab"), kind: SHT_STRTAB, flags: 0, addr: 0, offset: str_off,
                          size: strtab.len() as u32, link: 0, info: 0, align: 1, entsize: 0 });
        let shstr_name = add_name(&mut shstrtab, ".shstrtab");
        let shstr_off = place(&mut out, &shstrtab, 1);
        let shstrndx = shdrs.len() as u16;
        shdrs.push(Shdr { name: shstr_name, kind: SHT_STRTAB, flags: 0, addr: 0, offset: shstr_off,
                          size: shstrtab.len() as u32, link: 0, info: 0, align: 1, entsize: 0 });
        while !out.len().is_multiple_of(4) { out.push(0); }
        let shoff = out.len() as u32;
        for h in &shdrs {
            for v in [h.name, h.kind, h.flags, h.addr, h.offset, h.size, h.link, h.info, h.align, h.entsize] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }

        // The ELF header: 32-bit, little-endian, an i386 executable with no
        // entry (GDB is told where the program is, not asked to run it).
        let mut ehdr = Vec::with_capacity(EHDR_SIZE);
        ehdr.extend_from_slice(&[0x7F, b'E', b'L', b'F', 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        ehdr.extend_from_slice(&2u16.to_le_bytes());          // ET_EXEC
        ehdr.extend_from_slice(&3u16.to_le_bytes());          // EM_386
        ehdr.extend_from_slice(&1u32.to_le_bytes());          // EV_CURRENT
        ehdr.extend_from_slice(&0u32.to_le_bytes());          // e_entry
        ehdr.extend_from_slice(&(EHDR_SIZE as u32).to_le_bytes());
        ehdr.extend_from_slice(&shoff.to_le_bytes());
        ehdr.extend_from_slice(&0u32.to_le_bytes());          // e_flags
        for v in [EHDR_SIZE, PHDR_SIZE, 2, SHDR_SIZE, shdrs.len()] { ehdr.extend_from_slice(&(v as u16).to_le_bytes()); }
        ehdr.extend_from_slice(&shstrndx.to_le_bytes());
        out[..EHDR_SIZE].copy_from_slice(&ehdr);
        // Program headers: the code, and the data with its zeros.
        let phdrs = [(code_off, state.code_va, code.len() as u32, code.len() as u32, PF_R | PF_X),
                     (data_off, state.data_va, data.len() as u32, data.len() as u32 + state.bss_size, PF_R | PF_W)];
        for (i, (off, va, filesz, memsz, flags)) in phdrs.into_iter().enumerate() {
            let at = EHDR_SIZE + i * PHDR_SIZE;
            for (j, v) in [PT_LOAD, off, va, va, filesz, memsz, flags, 4096].into_iter().enumerate() {
                out[at + 4 * j..at + 4 * j + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        Ok(out)
    }

    /// The code and data an LX image written by `lx_writer` holds in its
    /// pages (relocations applied): the code object's bytes, then the
    /// data object's, from the start of the data pages.
    pub fn linked_images(lx: &[u8], state: &LinkerState) -> Option<(Vec<u8>, Vec<u8>)> {
        let hdr = super::LX_HEADER_OFFSET as usize;
        let pages = u32::from_le_bytes(lx.get(hdr + 0x80..hdr + 0x84)?.try_into().ok()?) as usize;
        let code = lx.get(pages..pages + state.code.len())?.to_vec();
        let data = lx.get(pages + state.code.len()..pages + state.code.len() + state.data.len())?.to_vec();
        Some((code, data))
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
    use super::{CODE_OBJECT_BASE, DLL_ENTRY, OBJECT_ALIGN, PROGRAM_AREA_END, STACK_SIZE};

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
        /// The entry point: always there for a program; for a DLL its
        /// `_DLL_InitTerm`, if any.
        pub entry_va: Option<u32>,
    }

    pub fn align_up(val: u64, align: u64) -> u64 {
        if align <= 1 { val } else { val.div_ceil(align) * align }
    }

    /// The bounds of the unwind tables (ADR-0103); `__EH_FRAME_END__` is
    /// where the zero terminator starts.
    pub const EH_FRAME_BEGIN: &str = "__EH_FRAME_BEGIN__";
    pub const EH_FRAME_END: &str = "__EH_FRAME_END__";

    /// The start / end symbols of each array, in the order they are laid out.
    pub const ARRAYS: [(ArrayKind, &str, &str); 3] = [
        (ArrayKind::Preinit, "__preinit_array_start", "__preinit_array_end"),
        (ArrayKind::Init, "__init_array_start", "__init_array_end"),
        (ArrayKind::Fini, "__fini_array_start", "__fini_array_end"),
    ];

    impl LinkerState {
        /// Lay out the sections `live` marks (`[object][section]`; all of
        /// them with `None`).
        /// A program: the entry point is `_start` (or `main`) and must exist.
        pub fn build(objects: &[ElfObject], live: Option<&[Vec<bool>]>) -> Result<Self, Box<dyn super::Error>> {
            Self::build_as(objects, live, false)
        }

        /// A DLL (ADR-0101): the entry point is `_DLL_InitTerm` when the
        /// objects define it; there is no stack object.
        pub fn build_dll(objects: &[ElfObject], live: Option<&[Vec<bool>]>) -> Result<Self, Box<dyn super::Error>> {
            Self::build_as(objects, live, true)
        }

        fn build_as(objects: &[ElfObject], live: Option<&[Vec<bool>]>, dll: bool) -> Result<Self, Box<dyn super::Error>> {
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
            // The unwind tables, together, then a zero terminator.
            let eh_start = align_up(data.len() as u64, 4);
            data.resize(eh_start as usize, 0);
            for (oi, o) in objects.iter().enumerate() {
                for (si, s) in o.sections.iter().enumerate() {
                    if s.class == SecClass::EhFrame && is_live(oi, si) { data_off[oi][si] = Some(place(&mut data, &s.data, 4)); }
                }
            }
            let eh_end = data.len() as u64;
            if eh_end > eh_start { data.extend_from_slice(&[0; 4]); }
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
            symbol_table.entry(EH_FRAME_BEGIN.to_string()).or_insert((data_va + eh_start) as u32);
            symbol_table.entry(EH_FRAME_END.to_string()).or_insert((data_va + eh_end) as u32);

            let entry_va = if dll {
                symbol_table.get(DLL_ENTRY).copied()
            } else {
                Some(symbol_table.get("_start").or_else(|| symbol_table.get("main")).copied()
                    .ok_or("no entry point: define _start (or main)")?)
            };

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

    /// What makes a DLL (ADR-0101): its module name, exports and when its
    /// `_DLL_InitTerm` runs.
    #[derive(Debug, Clone, Default)]
    pub struct DllSpec {
        pub name: String,
        /// Exported symbols, with the ordinal asked for (else the next free).
        pub exports: Vec<(String, Option<u32>)>,
        /// Per-process initialisation (`INITINSTANCE`).
        pub init_instance: bool,
        /// Per-process termination (`TERMINSTANCE`).
        pub term_instance: bool,
    }

    /// The output: a program, or a DLL; and the modules that are DLLs given
    /// as inputs (their imports are resolved when the program loads).
    #[derive(Debug, Clone, Default)]
    pub struct Spec {
        pub dll: Option<DllSpec>,
        pub user_modules: std::collections::HashSet<String>,
    }

    /// LX module flags (`<exe386.h>`): a library, per-process
    /// initialisation and termination.
    const MODULE_LIBRARY: u32 = 0x0000_8000;
    const MODULE_INIT_INSTANCE: u32 = 0x0000_0004;
    const MODULE_TERM_INSTANCE: u32 = 0x4000_0000;
    /// LX fixup source types: 32-bit offset and 32-bit self-relative.
    const SRC_OFFSET32: u8 = 0x07;
    const SRC_SELF_REL32: u8 = 0x08;
    /// Entry table: a bundle of 32-bit entries, of unused ordinals; an
    /// entry's flag that it is exported.
    const BUNDLE_32BIT: u8 = 3;
    const BUNDLE_UNUSED: u8 = 0;
    const ENTRY_EXPORTED: u8 = 0x01;

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
        /// An import; `additive` is added to its address.
        ExternalOrdinal { module_idx: u8, ordinal: u32, additive: u32 },
    }

    /// What a relocation's target resolved to.
    enum Resolved {
        /// An address in the program (with an internal fixup).
        Internal(u32),
        /// An import (with an import fixup): an OS/2 API, whose thunk
        /// address is known, or a DLL given as an input, whose is not.
        Import { va: Option<u32>, module: String, ordinal: u32 },
        /// A plain value (absolute symbols, undefined weak ones).
        Value(u32),
    }

    fn resolve(target: &RelocTarget, obj_idx: usize, state: &LinkerState, obj: &ElfObject,
               def_map: &HashMap<String, (String, u32)>, spec: &Spec) -> Result<Resolved, String> {
        Ok(match target {
            RelocTarget::Symbol(name) => {
                if let Some(&va) = state.symbol_table.get(name) {
                    Resolved::Internal(va)
                } else if let Some((module, ordinal)) = def_map.get(name) {
                    let va = resolve_import(module, *ordinal);
                    if va.is_none() && !spec.user_modules.contains(module) {
                        return Err(format!("{name} (module {module} is unknown to lx-link)"));
                    }
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
        spec: &Spec,
    ) -> Result<Vec<u8>, String> {
        let dll = spec.dll.as_ref();
        let mut code_data = state.code.clone();
        let mut data_data = state.data.clone();

        let code_pages = page_count(code_data.len() as u64);
        let data_pages = page_count(data_data.len() as u64 + u64::from(state.bss_size));
        // A DLL runs on its caller's stack: no stack object.
        let stack_pages = if dll.is_some() { 0 } else { (STACK_SIZE / PAGE_SIZE) as usize };
        let resource_object_num: u16 = if dll.is_some() { 3 } else { 4 };
        let (res_blob, res_table) = resource_object(resources, resource_object_num)?;
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
                    let resolved = match resolve(&reloc.target, obj_idx, state, obj, def_map, spec) {
                        Ok(r) => r,
                        Err(name) => { if !undefined.contains(&name) { undefined.push(name); } continue; }
                    };
                    let target_va = match &resolved {
                        Resolved::Internal(va) | Resolved::Value(va) => *va,
                        Resolved::Import { va, .. } => va.unwrap_or(0),
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
                    // The loader writes T for an offset fixup and T - (P + 4)
                    // for a self-relative one, so a PC-relative S + A - P
                    // needs T = S + A + 4. A PC-relative reference within its
                    // own object needs no fixup: the object moves as a whole.
                    let (source_type, fixup_target) = match (reloc.kind, resolved) {
                        (RelocKind::Abs32, Resolved::Internal(va)) => {
                            let (object_num, base) = state.object_of(va);
                            (SRC_OFFSET32, FixupTarget::Internal { object_num, target_offset: value.wrapping_sub(base) })
                        }
                        (RelocKind::Pc32, Resolved::Internal(va)) => {
                            let (object_num, base) = state.object_of(va);
                            if object_num == state.object_of(source_va).0 { continue; }
                            let t = va.wrapping_add(reloc.addend as u32).wrapping_add(4);
                            (SRC_SELF_REL32, FixupTarget::Internal { object_num, target_offset: t.wrapping_sub(base) })
                        }
                        (kind, Resolved::Import { module, ordinal, .. }) => {
                            let module_idx = get_or_insert_module(&module, &mut module_map, &mut imported_modules);
                            let (source_type, additive) = match kind {
                                RelocKind::Abs32 => (SRC_OFFSET32, reloc.addend as u32),
                                RelocKind::Pc32 => (SRC_SELF_REL32, (reloc.addend as u32).wrapping_add(4)),
                            };
                            (source_type, FixupTarget::ExternalOrdinal { module_idx, ordinal, additive })
                        }
                        (_, Resolved::Value(_)) => continue,
                    };
                    let page = (source_va - obj_base) / PAGE_SIZE;
                    let fixups = if is_code { &mut code_fixups } else { &mut data_fixups };
                    if let Some(list) = fixups.get_mut(page as usize) {
                        list.push(LxFixup {
                            source_type,
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
        let (resident_name_bytes, entry_table_bytes) = match dll {
            None => (name_entry(b"WARPEXE", 0).into_iter().chain([0]).collect(), vec![0, 0]),
            Some(d) => export_tables(d, state)?,
        };

        // ── Compute table offsets (all relative to LX header start, 0x40) ─────
        let num_objects: u32 = 2 + u32::from(dll.is_none()) + u32::from(!resources.is_empty());
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

        // 2. LX header (LX_HEADER_SIZE bytes).
        out.extend_from_slice(&build_lx_header(
            total_pages as u32,
            obj_table_off, num_objects, page_map_off,
            resident_name_off, entry_table_off,
            fixup_page_table_off, fixup_record_off,
            import_modules_off, imported_modules.len() as u32, import_proc_name_off,
            loader_section_size, fixup_section_size,
            data_pages_offset_abs,
            if resources.is_empty() { 0 } else { resource_table_off }, resources.len() as u32,
            HeaderKind::of(dll, state),
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
        if dll.is_none() {
            out.extend_from_slice(&encode_object_entry(
                STACK_SIZE, state.stack_va,
                OBJ_READ | OBJ_WRITE | OBJ_BIG | OBJ_USE32,
                code_pages as u32 + data_pages as u32 + 1, STACK_SIZE / PAGE_SIZE,
            ));
        }
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
    fn resource_object(resources: &[ResEntry], object_num: u16) -> Result<(Vec<u8>, Vec<u8>), String> {
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
            table.extend_from_slice(&object_num.to_le_bytes());
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
            FixupTarget::ExternalOrdinal { module_idx, ordinal, additive } => {
                // 0x01 = import by ordinal, 0x10 = 32-bit ordinal; with an
                // additive value, 0x04 = additive and 0x20 = 32 bits of it.
                let flags = if *additive == 0 { 0x11 } else { 0x11 | 0x04 | 0x20 };
                out.push(fix.source_type);
                out.push(flags);
                out.extend_from_slice(&fix.source_offset.to_le_bytes());
                out.push(*module_idx);
                out.extend_from_slice(&ordinal.to_le_bytes());
                if *additive != 0 { out.extend_from_slice(&additive.to_le_bytes()); }
            }
        }
    }

    /// What differs in the header between a program and a DLL.
    struct HeaderKind {
        module_flags: u32,
        eip_object: u32,
        eip: u32,
        esp_object: u32,
        esp: u32,
        /// The main thread's stack (`STACKSIZE`): Warpine makes it sparse.
        stack_size: u32,
    }

    impl HeaderKind {
        fn of(dll: Option<&DllSpec>, state: &LinkerState) -> HeaderKind {
            let (eip_object, eip) = match state.entry_va {
                Some(va) => (1, va - state.code_va),
                None => (0, 0),
            };
            match dll {
                None => HeaderKind { module_flags: 0, eip_object, eip, esp_object: 3, esp: STACK_SIZE, stack_size: STACK_SIZE },
                Some(d) => HeaderKind {
                    module_flags: MODULE_LIBRARY
                        | if d.init_instance { MODULE_INIT_INSTANCE } else { 0 }
                        | if d.term_instance { MODULE_TERM_INSTANCE } else { 0 },
                    eip_object, eip, esp_object: 0, esp: 0, stack_size: 0,
                },
            }
        }
    }

    /// A resident or non-resident name table entry.
    fn name_entry(name: &[u8], ordinal: u16) -> Vec<u8> {
        let mut b = Vec::with_capacity(name.len() + 3);
        b.push(name.len() as u8);
        b.extend_from_slice(name);
        b.extend_from_slice(&ordinal.to_le_bytes());
        b
    }

    /// A DLL's resident name table (its name, then every export) and entry
    /// table (bundles of 32-bit entries, unused ordinals skipped).
    fn export_tables(d: &DllSpec, state: &LinkerState) -> Result<(Vec<u8>, Vec<u8>), String> {
        if d.name.is_empty() || d.name.len() > 127 { return Err(format!("bad module name {:?}", d.name)); }
        let mut taken: std::collections::BTreeMap<u32, (&str, u8, u32)> = std::collections::BTreeMap::new();
        let mut next = 1u32;
        let mut missing = Vec::new();
        for (name, ordinal) in &d.exports {
            let Some(&va) = state.symbol_table.get(name) else { missing.push(name.clone()); continue };
            if name.is_empty() || name.len() > 127 { return Err(format!("export name {name:?} is too long")); }
            let ord = match ordinal { Some(o) => *o, None => { while taken.contains_key(&next) { next += 1; } next } };
            if ord == 0 || ord > 0xFFFF { return Err(format!("export {name}: ordinal {ord} out of range")); }
            let (object, base) = state.object_of(va);
            if taken.insert(ord, (name, object, va - base)).is_some() {
                return Err(format!("ordinal {ord} is exported twice"));
            }
        }
        if !missing.is_empty() {
            missing.sort();
            return Err(format!("exported but not defined: {}", missing.join(", ")));
        }
        let mut names = name_entry(d.name.to_ascii_uppercase().as_bytes(), 0);
        for (ord, (name, _, _)) in &taken { names.extend(name_entry(name.as_bytes(), *ord as u16)); }
        names.push(0);

        let mut entries = Vec::new();
        let mut expected = 1u32;
        let list: Vec<(u32, u8, u32)> = taken.iter().map(|(o, (_, obj, off))| (*o, *obj, *off)).collect();
        let mut i = 0;
        while i < list.len() {
            let (ord, object, _) = list[i];
            let mut gap = ord - expected;
            while gap > 0 {
                let n = gap.min(255);
                entries.extend_from_slice(&[n as u8, BUNDLE_UNUSED]);
                gap -= n;
            }
            let mut run = 1;
            while i + run < list.len() && run < 255 && list[i + run].0 == ord + run as u32 && list[i + run].1 == object { run += 1; }
            entries.extend_from_slice(&[run as u8, BUNDLE_32BIT]);
            entries.extend_from_slice(&u16::from(object).to_le_bytes());
            for &(_, _, offset) in &list[i..i + run] {
                entries.push(ENTRY_EXPORTED);
                entries.extend_from_slice(&offset.to_le_bytes());
            }
            expected = ord + run as u32;
            i += run;
        }
        entries.push(0);
        Ok((names, entries))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_lx_header(
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
        kind: HeaderKind,
    ) -> Vec<u8> {
        let mut h = Vec::with_capacity(LX_HEADER_SIZE as usize);
        h.extend_from_slice(b"LX");
        h.push(0); h.push(0);                                       // byte_order, word_order
        h.extend_from_slice(&0u32.to_le_bytes());                   // format_level
        h.extend_from_slice(&2u16.to_le_bytes());                   // cpu_type = 386
        h.extend_from_slice(&1u16.to_le_bytes());                   // os_type = OS/2
        h.extend_from_slice(&0u32.to_le_bytes());                   // module_version
        h.extend_from_slice(&kind.module_flags.to_le_bytes());      // module_flags
        h.extend_from_slice(&total_pages.to_le_bytes());            // module_num_pages
        h.extend_from_slice(&kind.eip_object.to_le_bytes());        // eip_object (1: code; 0: none)
        h.extend_from_slice(&kind.eip.to_le_bytes());               // eip
        h.extend_from_slice(&kind.esp_object.to_le_bytes());        // esp_object (3: stack; 0 for a DLL)
        h.extend_from_slice(&kind.esp.to_le_bytes());               // esp = top of stack
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
        h.extend_from_slice(&kind.stack_size.to_le_bytes());        // stack_size (the whole stack object)
        h.resize(LX_HEADER_SIZE as usize, 0);                       // reserved
        assert_eq!(h.len(), LX_HEADER_SIZE as usize);
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
    let linked = match link(&parsed, def_map, inputs) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("lx-link: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = std::fs::write(&parsed.output, &linked.lx) {
        eprintln!("lx-link: cannot write {:?}: {}", parsed.output, e);
        process::exit(1);
    }

    // The debug companion (ADR-0102).
    let debug_file = if parsed.no_debug_file {
        None
    } else {
        parsed.debug_file.clone()
            .or_else(|| debug_elf::has_debug_info(&linked.objects).then(|| parsed.output.with_extension("dbg")))
    };
    if let Some(path) = debug_file {
        let written = linked.debug_file()
            .and_then(|elf| std::fs::write(&path, elf).map_err(|e| e.to_string()));
        if let Err(e) = written {
            eprintln!("lx-link: cannot write the debug file {path:?}: {e}");
            process::exit(1);
        }
    }
}

/// What a link made: the LX file, and the objects and layout its debug
/// companion is made from.
pub(crate) struct Linked {
    pub(crate) lx: Vec<u8>,
    objects: Vec<elf_reader::ElfObject>,
    state: linker_state::LinkerState,
}

impl Linked {
    /// The debug companion (ADR-0102).
    pub(crate) fn debug_file(&self) -> Result<Vec<u8>, String> {
        debug_elf::linked_images(&self.lx, &self.state)
            .ok_or_else(|| "the linked image is cut short".to_string())
            .and_then(|(code, data)| debug_elf::write(&self.objects, &self.state, &code, &data))
    }

    /// Whether the objects carry debug information.
    #[allow(dead_code)]
    pub(crate) fn has_debug_info(&self) -> bool {
        debug_elf::has_debug_info(&self.objects)
    }
}

/// Link `inputs` as `parsed` asks, with the API ordinals of `def_map`.
fn link(parsed: &args::Args, mut def_map: std::collections::HashMap<String, (String, u32)>, mut inputs: Inputs) -> Result<Linked, String> {
    let resources = std::mem::take(&mut inputs.resources);
    let messages = std::mem::take(&mut inputs.messages);
    if !messages.is_empty() {
        inputs.objects.push(message_object(&messages).map_err(|e| format!("bad message file: {e}"))?);
    }
    let mut spec = lx_writer::Spec::default();
    // Names a DLL input exports become imports from it; an OS/2 API of
    // the same name stays the API.
    for (module, exports) in std::mem::take(&mut inputs.dlls) {
        for (name, ordinal) in exports {
            def_map.entry(name).or_insert((module.clone(), ordinal));
        }
        spec.user_modules.insert(module);
    }
    if parsed.dll {
        let name = parsed.module_name.clone().unwrap_or_else(|| {
            parsed.output.file_stem().map(|s| s.to_string_lossy().to_ascii_uppercase()).unwrap_or_default()
        });
        spec.dll = Some(lx_writer::DllSpec { name, exports: parsed.exports.clone(),
                                             init_instance: parsed.init_instance, term_instance: parsed.term_instance });
    }
    let objects = select_objects(inputs, &def_map);
    if objects.is_empty() {
        return Err("no ELF objects found in inputs".into());
    }
    let roots: Vec<&str> = match &spec.dll {
        Some(d) => std::iter::once(DLL_ENTRY).chain(d.exports.iter().map(|(n, _)| n.as_str())).collect(),
        None => vec!["_start", "main"],
    };
    let live = parsed.gc_sections.then(|| live_sections(&objects, &roots));
    let mut objects = objects;
    let all_live: Vec<Vec<bool>> = objects.iter().map(|o| vec![true; o.sections.len()]).collect();
    eh_frame::prune(&mut objects, live.as_deref().unwrap_or(&all_live))?;

    let built = if spec.dll.is_some() {
        linker_state::LinkerState::build_dll(&objects, live.as_deref())
    } else {
        linker_state::LinkerState::build(&objects, live.as_deref())
    };
    let state = built.map_err(|e| e.to_string())?;
    let lx = lx_writer::write_lx(&objects, &state, &def_map, &resources, &spec).map_err(|e| e.to_string())?;
    Ok(Linked { lx, objects, state })
}

/// Link the one input `data`, named `name` (its extension says what it
/// is), into a program in memory, with the built-in API ordinals: the
/// fuzz target's entry (ADR-0112). `gc` links with `--gc-sections`.
#[allow(dead_code)]
pub(crate) fn link_in_memory(name: &str, data: Vec<u8>, gc: bool) -> Result<Linked, String> {
    let mut raw = vec!["lx-link".to_string(), name.to_string(), "-o".into(), "fuzz.exe".into()];
    if gc {
        raw.push("--gc-sections".into());
    }
    let parsed = args::parse_args(&raw);
    let mut inputs = Inputs::default();
    add_input(Path::new(name), data, &mut inputs).map_err(|e| e.to_string())?;
    link(&parsed, def_parser::parse_def(EMBEDDED_DEF), inputs)
}

/// The parsed inputs: plain objects are always linked; an archive member
/// only when it defines a symbol something linked needs.
#[derive(Default)]
pub(crate) struct Inputs {
    objects: Vec<elf_reader::ElfObject>,
    members: Vec<elf_reader::ElfObject>,
    /// Resources from .res files, in order.
    resources: Vec<res_reader::ResEntry>,
    /// DLLs given as inputs: module name and exports.
    dlls: Vec<(String, Vec<(String, u32)>)>,
    /// Message files to bind (ADR-0105): file name and bytes.
    messages: Vec<(String, Vec<u8>)>,
}

/// The symbol of the bound message segment, which the SDK's
/// `DosGetMessage` stub passes to MSG.6 (ADR-0105).
const MESSAGE_SEGMENT: &str = "__warpine_message_segment";

/// The message files given to the link, as one object: a data section
/// holding their bound message segment, at [`MESSAGE_SEGMENT`].
fn message_object(messages: &[(String, Vec<u8>)]) -> Result<elf_reader::ElfObject, String> {
    let segment = msgfile::build_segment(messages).map_err(|e| e.to_string())?;
    let size = segment.len() as u64;
    Ok(elf_reader::ElfObject {
        name: "<bound messages>".into(),
        sections: vec![elf_reader::ElfSection { name: ".data.messages".into(), class: elf_reader::SecClass::Data,
                                                data: segment, size, align: 4, relocs: Vec::new() }],
        symbols: vec![elf_reader::ElfSymbol { name: MESSAGE_SEGMENT.into(), section_idx: 0, offset: 0, weak: false }],
        ..Default::default()
    })
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
            .flat_map(|o| o.undefined.iter().chain(o.sections.iter().filter(|s| s.class != elf_reader::SecClass::Debug).flat_map(|s| s.relocs.iter()).filter_map(|r| match &r.target {
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

/// `--gc-sections`: the sections reached from `roots` (a program's entry
/// point; a DLL's `_DLL_InitTerm` and exports) and the init / fini arrays
/// through relocations, as
/// `[object][section]`. Common symbols always stay.
fn live_sections(objects: &[elf_reader::ElfObject], roots: &[&str]) -> Vec<Vec<bool>> {
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
    for root in roots {
        if let Some(&d) = defs.get(root) { work.push(d); }
    }
    for (oi, o) in objects.iter().enumerate() {
        for (si, s) in o.sections.iter().enumerate() {
            if matches!(s.class, SecClass::Array(_)) { work.push((oi, si)); }
        }
    }
    // What each function's unwind entry needs once the function is kept:
    // its LSDA (.gcc_except_table) and its CIE's personality routine.
    let eh_deps = eh_frame::dependencies(objects);
    while let Some((oi, si)) = work.pop() {
        if std::mem::replace(&mut live[oi][si], true) { continue; }
        let deps = eh_deps.get(&(oi, si)).into_iter().flatten();
        for target in objects[oi].sections[si].relocs.iter().map(|r| &r.target).chain(deps) {
            match target {
                RelocTarget::Local { section, .. } => work.push((oi, *section)),
                RelocTarget::Symbol(name) => { if let Some(&d) = defs.get(name.as_str()) { work.push(d); } }
                RelocTarget::Absolute(_) => {}
            }
        }
    }
    // The unwind tables are pruned to the kept functions (eh_frame::prune)
    // and kept themselves.
    for (oi, o) in objects.iter().enumerate() {
        for (si, s) in o.sections.iter().enumerate() {
            if s.class == SecClass::EhFrame { live[oi][si] = true; }
        }
    }
    live
}

/// The API ordinals, compiled in so lx-link works regardless of CWD.
const EMBEDDED_DEF: &str = include_str!("../../targets/os2api.def");

fn load_def_file(explicit: Option<&Path>) -> String {
    const EMBEDDED: &str = EMBEDDED_DEF;
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
    add_input(path, data, inputs)
}

/// Read input `data`, the contents of `path`: a resource file, a DLL, a
/// message file, an archive or an object, by its name and first bytes
/// (the fuzz target's entry, ADR-0112: every input is untrusted).
pub(crate) fn add_input(path: &Path, data: Vec<u8>, inputs: &mut Inputs) -> Result<(), Box<dyn Error>> {
    let name = path.display().to_string();

    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("res")) {
        inputs.resources.extend(res_reader::parse_res(&data, &name)?);
        return Ok(());
    }
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
        inputs.dlls.push(dll_reader::read_exports(&data, &name)?);
        return Ok(());
    }
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("msg")) {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or(name);
        inputs.messages.push((file, data));
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

    /// A section's or common symbol's alignment is at most the objects'
    /// own (64 KB): a crafted one could otherwise grow the image to
    /// gigabytes (found by the lx_link fuzz target).
    #[test]
    fn alignments_are_bounded() {
        use super::elf_reader::{alignment, MAX_ALIGN};
        assert_eq!(alignment(0, "section .text", "a.o").unwrap(), 1);
        assert_eq!(alignment(16, "section .text", "a.o").unwrap(), 16);
        assert_eq!(alignment(MAX_ALIGN, "section .text", "a.o").unwrap(), MAX_ALIGN);
        let e = alignment(1 << 31, "section .text", "a.o").expect_err("too large").to_string();
        assert!(e.contains("a.o: section .text asks for an alignment of 2147483648 bytes"), "{e}");
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
        let lx = lx_writer::write_lx(objects, &state, defs, &[], &lx_writer::Spec::default())?;
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
        let live = live_sections(&objects, &["_start"]);
        assert_eq!(live[0], vec![true, false, true, false], "_start's .text and what it uses");
        let state = LinkerState::build(&objects, Some(&live)).unwrap();
        assert!(lx_writer::write_lx(&objects, &state, &HashMap::new(), &[], &lx_writer::Spec::default()).is_ok());
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
            member("g.o", "g", None), member("f.o", "f", Some("g")), member("h.o", "h", None)], ..Default::default() };
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
        let lx_bytes = lx_writer::write_lx(&objects, &state, &HashMap::new(), &[], &lx_writer::Spec::default()).expect("write");

        // Must start with MZ, then LX at 0x40.
        assert!(lx_bytes.len() > LX_HEADER_OFFSET as usize + LX_HEADER_SIZE as usize);
        assert_eq!(&lx_bytes[0..2], b"MZ");
        assert_eq!(&lx_bytes[LX_HEADER_OFFSET as usize..LX_HEADER_OFFSET as usize + 2], b"LX");

        let hdr = LX_HEADER_OFFSET as usize;
        let obj_count = u32::from_le_bytes(lx_bytes[hdr + 0x44..hdr + 0x48].try_into().unwrap());
        assert_eq!(obj_count, 3, "3 objects: code + data + stack");
        let u32_at = |at: usize| u32::from_le_bytes(lx_bytes[hdr + at..hdr + at + 4].try_into().unwrap());
        assert_eq!(u32_at(0x40), LX_HEADER_SIZE, "the object table follows the header");
        assert_eq!(u32_at(0xAC), STACK_SIZE, "STACKSIZE: the whole stack object");

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
        let lx = lx_writer::write_lx(&objects, &state, &HashMap::new(), &resources, &lx_writer::Spec::default()).expect("write");
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
        let e = lx_writer::write_lx(&objects, &state, &HashMap::new(), &dup, &lx_writer::Spec::default()).unwrap_err();
        assert!(e.contains("given twice"), "{e}");
    }

    /// A DLL (ADR-0101): the library flags, INITINSTANCE / TERMINSTANCE,
    /// no stack object, `_DLL_InitTerm` as the entry, its exports in the
    /// resident names and the entry table, read back by `dll_reader` the
    /// way a program linked with the DLL reads them.
    #[test]
    fn a_dll_has_exports_an_entry_table_and_no_stack() {
        let objects = vec![ElfObject {
            name: "d.o".into(),
            sections: vec![section(".text", SecClass::Code, vec![0x90; 16], vec![]),
                           section(".data", SecClass::Data, vec![0; 8], vec![])],
            symbols: vec![sym("_DLL_InitTerm", 0, 0), sym("Answer", 0, 8), sym("Table", 1, 4)],
            ..Default::default()
        }];
        let state = LinkerState::build_dll(&objects, None).expect("build");
        let spec = lx_writer::Spec {
            dll: Some(lx_writer::DllSpec {
                name: "MYDLL".into(),
                exports: vec![("Answer".into(), Some(3)), ("Table".into(), None)],
                init_instance: true,
                term_instance: true,
            }),
            ..Default::default()
        };
        let lx = lx_writer::write_lx(&objects, &state, &HashMap::new(), &[], &spec).expect("write");
        let hdr = LX_HEADER_OFFSET as usize;
        let u32_at = |o: usize| u32::from_le_bytes(lx[o..o + 4].try_into().unwrap());
        assert_eq!(u32_at(hdr + 0x10), 0x8000 | 0x4 | 0x4000_0000, "library, INITINSTANCE, TERMINSTANCE");
        assert_eq!(u32_at(hdr + 0x44), 2, "code and data, no stack");
        assert_eq!((u32_at(hdr + 0x18), u32_at(hdr + 0x1C)), (1, 0), "_DLL_InitTerm is the entry");
        assert_eq!((u32_at(hdr + 0x20), u32_at(hdr + 0x24)), (0, 0), "no stack");

        let (module, mut exports) = dll_reader::read_exports(&lx, "mydll.dll").expect("read back");
        exports.sort_by_key(|e| e.1);
        assert_eq!(module, "MYDLL");
        assert_eq!(exports, [("Table".to_string(), 1), ("Answer".to_string(), 3)], "the lowest free ordinal");

        // Entry table: a 32-bit bundle in object 2 for ordinal 1, ordinal 2
        // unused (a type-0 bundle), then one in object 1 for ordinal 3.
        let et = hdr + u32_at(hdr + 0x5C) as usize;
        let bundle = |at: usize| (lx[at], lx[at + 1], u16::from_le_bytes([lx[at + 2], lx[at + 3]]),
                                  u32::from_le_bytes(lx[at + 5..at + 9].try_into().unwrap()));
        assert_eq!(bundle(et), (1, 3, 2, 4), "Table: object 2, offset 4");
        assert_eq!(&lx[et + 9..et + 11], &[1, 0], "ordinal 2 unused");
        assert_eq!(bundle(et + 11), (1, 3, 1, 8), "Answer: object 1, offset 8");
        assert_eq!(lx[et + 20], 0, "the end of the table");
    }

    /// Debug sections (ADR-0102) are joined by name in input order; a
    /// relocation to another debug section becomes its offset in the
    /// joined section, one to code its linked address, and one to a
    /// section the link dropped 0.
    #[test]
    fn debug_sections_are_joined_and_relocated() {
        let obj = |name: &str, info_relocs: Vec<Reloc>| ElfObject {
            name: name.into(),
            sections: vec![section(".text", SecClass::Code, vec![0xC3; 8], vec![]),
                           section(".debug_info", SecClass::Debug, vec![0; 8], info_relocs),
                           section(".debug_str", SecClass::Debug, b"main\0x\0".to_vec(), vec![]),
                           section(".text.unused", SecClass::Code, vec![0xC3; 4], vec![])],
            symbols: vec![sym("_start", 0, 0)],
            ..Default::default()
        };
        let a = obj("a.o", vec![reloc(0, RelocKind::Abs32, 5, RelocTarget::Local { section: 2, offset: 0 }),
                                reloc(4, RelocKind::Abs32, 2, RelocTarget::Local { section: 0, offset: 0 })]);
        let mut b = obj("b.o", vec![reloc(0, RelocKind::Abs32, 0, RelocTarget::Local { section: 2, offset: 0 }),
                                    reloc(4, RelocKind::Abs32, 0, RelocTarget::Local { section: 3, offset: 0 })]);
        b.symbols.clear();
        let objects = [a, b];
        let live = live_sections(&objects, &["_start"]);
        let state = LinkerState::build(&objects, Some(&live)).expect("build");
        let joined = debug_elf::join(&objects, &state).expect("join");
        let (start, _) = debug_elf::placements(&objects).expect("placements");
        let info = &joined[".debug_info"];
        let word = |at: usize| u32::from_le_bytes(info[at..at + 4].try_into().unwrap());
        assert_eq!(info.len(), 16, "both objects' .debug_info");
        assert_eq!(start[1][1], Some(8), "b's .debug_info follows a's");
        assert_eq!(start[1][2], Some(7), "b's .debug_str follows a's 7 bytes");
        assert_eq!(word(0), 5, "a's string at its offset 0 + 5");
        assert_eq!(word(4), state.section_va[0][0].unwrap() + 2, "a's code at its address + 2");
        assert_eq!(word(8), 7, "b's string, after a's strings");
        assert_eq!(word(12), 0, "b's .text.unused was dropped");
        assert!(debug_elf::has_debug_info(&objects));
    }

    /// The companion is an ELF32 i386 file GDB can read: the code and
    /// data at their linked addresses, the debug sections, and the global
    /// symbols as functions and objects.
    #[test]
    fn the_debug_companion_is_an_elf_with_code_symbols_and_dwarf() {
        use object::{Object, ObjectSection, ObjectSymbol};
        let objects = vec![ElfObject {
            name: "a.o".into(),
            sections: vec![section(".text", SecClass::Code, vec![0x90, 0xC3], vec![]),
                           section(".data", SecClass::Data, vec![1, 2, 3, 4], vec![]),
                           section(".debug_line", SecClass::Debug, vec![9; 6], vec![])],
            symbols: vec![sym("_start", 0, 0), sym("counter", 1, 0)],
            ..Default::default()
        }];
        let state = LinkerState::build(&objects, None).expect("build");
        let lx = lx_writer::write_lx(&objects, &state, &HashMap::new(), &[], &lx_writer::Spec::default()).expect("lx");
        let (code, data) = debug_elf::linked_images(&lx, &state).expect("images");
        assert_eq!(code, [0x90, 0xC3]);
        let elf = debug_elf::write(&objects, &state, &code, &data).expect("elf");
        let file = object::File::parse(&*elf).expect("parses as ELF");
        assert_eq!(file.architecture(), object::Architecture::I386);
        let text = file.section_by_name(".text").expect(".text");
        assert_eq!((text.address(), text.data().unwrap()), (u64::from(state.code_va), &[0x90u8, 0xC3][..]));
        assert_eq!(file.section_by_name(".data").unwrap().address(), u64::from(state.data_va));
        assert_eq!(file.section_by_name(".debug_line").unwrap().data().unwrap(), &[9; 6]);
        let start = file.symbols().find(|s| s.name() == Ok("_start")).expect("_start");
        assert_eq!((start.address(), start.kind()), (u64::from(state.code_va), object::SymbolKind::Text));
        let counter = file.symbols().find(|s| s.name() == Ok("counter")).expect("counter");
        assert_eq!((counter.address(), counter.kind()), (u64::from(state.data_va), object::SymbolKind::Data));
    }

    /// `--debug-file` names the companion, `--no-debug-file` turns it off.
    #[test]
    fn debug_file_flags_parse() {
        let parse = |a: &[&str]| args::parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let a = parse(&["lx-link", "a.o", "--debug-file", "x.dbg", "-o", "x.exe"]);
        assert_eq!(a.debug_file.as_deref(), Some(std::path::Path::new("x.dbg")));
        assert!(!a.no_debug_file);
        assert!(parse(&["lx-link", "a.o", "--no-debug-file"]).no_debug_file);
    }

    /// An `.eh_frame` with one CIE and two FDEs, for `.text.a` (with an
    /// LSDA in `.gcc_except_table.a`) and `.text.b` (ADR-0103).
    fn eh_object() -> ElfObject {
        let mut eh = Vec::new();
        // CIE at 0: length 16, id 0, version 1, "zR", align 1 / -4, ra 8, aug 1: 0x1b.
        eh.extend_from_slice(&16u32.to_le_bytes());
        eh.extend_from_slice(&0u32.to_le_bytes());
        eh.extend_from_slice(&[1, b'z', b'R', 0, 1, 0x7C, 8, 1, 0x1B]);
        eh.extend_from_slice(&[0, 0, 0]);
        // FDE at 20 (for .text.a): length 20, CIE pointer 24, pc_begin, range, aug 4 (LSDA).
        eh.extend_from_slice(&20u32.to_le_bytes());
        eh.extend_from_slice(&24u32.to_le_bytes());
        eh.extend_from_slice(&[0; 8]);
        eh.extend_from_slice(&[4, 0, 0, 0, 0, 0, 0, 0]);
        // FDE at 44 (for .text.b): length 16, CIE pointer 48.
        eh.extend_from_slice(&16u32.to_le_bytes());
        eh.extend_from_slice(&48u32.to_le_bytes());
        eh.extend_from_slice(&[0; 8]);
        eh.extend_from_slice(&[0, 0, 0, 0]);
        let relocs = vec![
            reloc(28, RelocKind::Pc32, 0, RelocTarget::Local { section: 0, offset: 0 }),
            reloc(37, RelocKind::Abs32, 0, RelocTarget::Local { section: 2, offset: 0 }),
            reloc(52, RelocKind::Pc32, 0, RelocTarget::Local { section: 1, offset: 0 }),
        ];
        ElfObject {
            name: "eh.o".into(),
            sections: vec![section(".text.a", SecClass::Code, vec![0xC3; 4], vec![]),
                           section(".text.b", SecClass::Code, vec![0xC3; 4], vec![]),
                           section(".gcc_except_table.a", SecClass::Data, vec![0xFF; 4], vec![]),
                           section(".eh_frame", SecClass::EhFrame, eh, relocs)],
            symbols: vec![sym("_start", 0, 0), sym("unused", 1, 0)],
            ..Default::default()
        }
    }

    /// A kept function's FDE keeps its LSDA alive; the FDE of a dropped
    /// one goes, the CIE stays, the kept FDE points back at it from its
    /// new place, and its relocations move along. The tables lie between
    /// `__EH_FRAME_BEGIN__` and `__EH_FRAME_END__`, followed by a zero.
    #[test]
    fn eh_frame_keeps_the_kept_functions_records() {
        let mut objects = vec![eh_object()];
        assert_eq!(eh_frame::records(&objects[0].sections[3].data).unwrap().len(), 3);
        let live = live_sections(&objects, &["_start"]);
        assert_eq!(live[0], [true, false, true, true], ".text.a and its LSDA, not .text.b");
        eh_frame::prune(&mut objects, &live).unwrap();
        let eh = &objects[0].sections[3];
        let recs = eh_frame::records(&eh.data).unwrap();
        assert_eq!(recs.len(), 2, "the CIE and .text.a's FDE");
        assert_eq!(recs[1].cie, Some(0), "the FDE's CIE pointer still finds the CIE");
        assert_eq!(eh.relocs.iter().map(|r| r.offset).collect::<Vec<_>>(), [28, 37]);

        let state = LinkerState::build(&objects, Some(&live)).unwrap();
        let begin = state.symbol_table["__EH_FRAME_BEGIN__"];
        let end = state.symbol_table["__EH_FRAME_END__"];
        assert_eq!(end - begin, 44, "the two records");
        assert_eq!(word(&state, end), 0, "the terminator");
        assert_eq!(state.section_va[0][3], Some(begin));
    }

    /// A malformed record is an error naming the object.
    #[test]
    fn a_cut_eh_frame_is_refused() {
        let mut o = eh_object();
        o.sections[3].data.truncate(34);
        let e = eh_frame::prune(&mut [o], &[vec![true; 4]]).unwrap_err();
        assert!(e.contains("eh.o") && e.contains("cut short"), "{e}");
    }

    /// Exporting what nothing defines, or one ordinal twice, is an error.
    #[test]
    fn bad_dll_exports_are_refused() {
        let objects = vec![ElfObject {
            name: "d.o".into(),
            sections: vec![section(".text", SecClass::Code, vec![0x90; 4], vec![])],
            symbols: vec![sym("A", 0, 0), sym("B", 0, 2)],
            ..Default::default()
        }];
        let state = LinkerState::build_dll(&objects, None).expect("build");
        let write = |exports: Vec<(String, Option<u32>)>| {
            let spec = lx_writer::Spec {
                dll: Some(lx_writer::DllSpec { name: "D".into(), exports, init_instance: false, term_instance: false }),
                ..Default::default()
            };
            lx_writer::write_lx(&objects, &state, &HashMap::new(), &[], &spec)
        };
        let e = write(vec![("Missing".into(), None)]).unwrap_err();
        assert!(e.contains("exported but not defined: Missing"), "{e}");
        let e = write(vec![("A".into(), Some(1)), ("B".into(), Some(1))]).unwrap_err();
        assert!(e.contains("ordinal 1 is exported twice"), "{e}");
        assert!(write(vec![("A".into(), Some(1)), ("B".into(), Some(2))]).is_ok());
        // With no entry routine the DLL has no entry point (eip_object 0).
        let lx = write(vec![]).unwrap();
        let hdr = LX_HEADER_OFFSET as usize;
        assert_eq!(u32::from_le_bytes(lx[hdr + 0x18..hdr + 0x1C].try_into().unwrap()), 0);
    }

    /// A program importing from a DLL given as an input: the DLL's module
    /// goes into the import module table, and a call becomes a
    /// self-relative fixup to its ordinal, never a Warpine API thunk.
    #[test]
    fn imports_from_a_user_dll_are_fixups_to_its_module() {
        let objects = [start_object(vec![reloc(1, RelocKind::Pc32, -4, RelocTarget::Symbol("Answer".into()))])];
        let state = LinkerState::build(&objects, None).expect("build");
        let mut defs = HashMap::new();
        defs.insert("Answer".to_string(), ("MYDLL".to_string(), 3));
        let mut spec = lx_writer::Spec::default();
        spec.user_modules.insert("MYDLL".into());
        let lx = lx_writer::write_lx(&objects, &state, &defs, &[], &spec).expect("write");
        let hdr = LX_HEADER_OFFSET as usize;
        let u32_at = |o: usize| u32::from_le_bytes(lx[o..o + 4].try_into().unwrap());
        assert_eq!(u32_at(hdr + 0x74), 1, "one import module");
        let im = hdr + u32_at(hdr + 0x70) as usize;
        assert_eq!(&lx[im..im + 6], b"\x05MYDLL");
        let fx = hdr + u32_at(hdr + 0x6C) as usize;
        assert_eq!(lx[fx], 0x08, "self-relative");
        assert_eq!(lx[fx + 1] & 0x03, 0x01, "an import by ordinal");
        assert_eq!(u16::from_le_bytes([lx[fx + 2], lx[fx + 3]]), 1, "at the call's operand");
        assert_eq!((lx[fx + 4], u16::from_le_bytes([lx[fx + 5], lx[fx + 6]])), (1, 3), "module 1, ordinal 3");
    }

    #[test]
    fn message_files_become_a_bound_segment_object() {
        let app = msgfile::write(&msgfile::compile(b"APP\nAPP0001I: hi\n", 0).unwrap()).unwrap();
        let o = message_object(&[("app.msg".into(), app)]).expect("object");
        assert_eq!(o.symbols.len(), 1);
        assert_eq!((o.symbols[0].name.as_str(), o.symbols[0].offset, o.symbols[0].weak), (MESSAGE_SEGMENT, 0, false));
        let sec = &o.sections[0];
        assert_eq!(sec.class, elf_reader::SecClass::Data);
        assert_eq!(&sec.data[..8], msgfile::SEGMENT_MAGIC);
        assert_eq!(sec.size as usize, sec.data.len());
        assert!(message_object(&[("bad.msg".into(), b"junk".to_vec())]).unwrap_err().contains("bad.msg"));
        let a = args::parse_args(&["lx-link".into(), "x.o".into(), "app.MSG".into(), "-o".into(), "x.exe".into()]);
        assert_eq!(a.inputs, vec![std::path::PathBuf::from("x.o"), "app.MSG".into()]);
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

    /// A link in memory reads its one input by its name and fails
    /// cleanly on bad input (the fuzz target's entry).
    #[test]
    fn a_link_in_memory_refuses_bad_input() {
        for name in ["x.o", "x.res", "x.dll", "x.msg"] {
            assert!(link_in_memory(name, b"not an object".to_vec(), false).is_err(), "{name}");
        }
        assert!(link_in_memory("x.a", b"!<arch>\n".to_vec(), true).is_err(), "an empty archive: no objects");
    }

    #[test]
    fn test_lx_header_size_constant() {
        assert_eq!(LX_HEADER_SIZE, 0xC4, "as Open Watcom's linker writes it");
    }
}
