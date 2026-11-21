// SPDX-License-Identifier: GPL-3.0-or-later

use super::MutexExt;
use super::constants::*;
use super::managers::LoadedDll;
use crate::lx::LxFile;
use crate::lx::header::FixupTarget;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use log::{debug, info, warn};

/// Built-in (emulated) module names — handled entirely by the API thunk mechanism.
/// These are never loaded from the host filesystem; imports to them resolve to thunk stubs.
const BUILTIN_MODULES: &[&str] = &[
    "DOSCALLS", "QUECALLS", "PMWIN", "PMGPI", "KBDCALLS",
    "VIOCALLS", "SESMGR", "NLS", "MSG", "MDM", "UCONV", "SO32DLL", "TCP32DLL", "MOUCALLS",
    "PMSHAPI", "HELPMGR", "PMSPL", "PMCTLS", "WARPINE", "PMDRAG", "SOM",
];

impl super::Loader {
    pub fn is_pm_app(&self, lx_file: &LxFile) -> bool {
        lx_file.imported_modules.iter().any(|m| m == "PMWIN" || m == "PMGPI")
    }

    pub fn get_shared(&self) -> Arc<super::SharedState> {
        Arc::clone(&self.shared)
    }

    pub fn load<P: AsRef<Path>>(&mut self, lx_file: &LxFile, path: P) -> io::Result<()> {
        // A program starts in its entry object, on its stack object: both
        // must be objects of the file (the parser lets 0 through, which a
        // DLL may have), or `setup_guest` could not index them.
        let objects = lx_file.object_table.len() as u32;
        for (what, object) in [("entry", lx_file.header.eip_object), ("stack", lx_file.header.esp_object)] {
            if !(1..=objects).contains(&object) {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                    format!("the program's {what} object {object} is not one of its {objects} objects")));
            }
        }
        // Every object inside the program area, none over another
        // (ADR-0094): Warpine's own structures and the allocator's memory
        // lie outside it, so a program can neither be loaded over them nor
        // have them inside its data.
        // Resource objects are reached only through the resource table, never
        // by fixups: they go wherever the allocator puts them (linkers give
        // them base address 0).
        let extents: Vec<(u32, u32, u32)> = lx_file.object_table.iter()
            .filter(|o| o.flags & OBJ_RESOURCE == 0)
            .map(|o| (o.base_address, o.size, o.page_count)).collect();
        check_exe_objects(&extents).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut file = File::open(&path)?;
        let data_pages_base = lx_file.header.data_pages_offset as u64;
        // The program's objects at their base addresses, resource objects
        // where the allocator puts them.
        let mut object_bases: Vec<u32> = Vec::with_capacity(lx_file.object_table.len());
        for (i, o) in lx_file.object_table.iter().enumerate() {
            if o.flags & OBJ_RESOURCE == 0 {
                object_bases.push(o.base_address);
                continue;
            }
            let pages = u64::from(o.page_count) * u64::from(PAGE_SIZE);
            let size = u32::try_from(u64::from(o.size).max(pages)).ok().filter(|&n| n > 0 && n <= MAX_RESOURCE_OBJECT)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData,
                    format!("resource object {} is too large", i + 1)))?;
            let addr = self.alloc_zeroed(size).ok_or_else(|| io::Error::new(io::ErrorKind::OutOfMemory,
                format!("no memory for resource object {}", i + 1)))?;
            object_bases.push(addr);
        }
        // The program's objects, for DosQueryModFromEIP.
        {
            let mut dlls = self.shared.dll_mgr.lock_or_recover();
            dlls.exe_objects = lx_file.object_table.iter().zip(&object_bases).map(|(o, &b)| (b, o.size)).collect();
            dlls.exe_name = path.as_ref().file_stem().map(|s| s.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
        }

        for (i, obj) in lx_file.object_table.iter().enumerate() {
            debug!("  Mapping Object {}...", i + 1);
            let base = object_bases[i];
            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.page_map.len() { break; }
                let page_off = data_pages_base + ((lx_file.page_map[page_idx].data_offset as u64) << lx_file.header.page_offset_shift);
                let target = base as usize + p * 4096;
                let data_size = (lx_file.page_map[page_idx].data_size as usize).min(PAGE_SIZE as usize);
                if data_size > 0 {
                    file.seek(SeekFrom::Start(page_off))?;
                    let slice = self.guest_slice_mut(target as u32, data_size).ok_or_else(|| io::Error::new(
                        io::ErrorKind::InvalidData, format!("object {} page {p} lies outside guest memory", i + 1)))?;
                    file.read_exact(slice)?;
                }
            }
        }

        // Populate resource manager with precomputed guest addresses
        if !lx_file.resources.is_empty() {
            let mut res_mgr = self.shared.resource_mgr.lock_or_recover();
            for res in &lx_file.resources {
                let obj_idx = (res.object_num as usize).wrapping_sub(1);
                if obj_idx < lx_file.object_table.len() {
                    let guest_addr = object_bases[obj_idx].wrapping_add(res.offset);
                    res_mgr.add(super::managers::EXE_RESOURCE_MODULE, res.type_id, res.name_id, guest_addr, res.size);
                    debug!("  Resource: type={} id={} addr=0x{:08X} size={}", res.type_id, res.name_id, guest_addr, res.size);
                }
            }
        }

        // The DLLs the program imports, before its fixups refer to them.
        self.load_dependencies("the program", &lx_file.imported_modules, &mut HashSet::new());
        self.apply_fixups(lx_file, &object_bases)
    }

    /// Load the modules of `imported` that are neither built in nor loaded
    /// yet (and theirs, recursively), for module `owner`; `loading` guards
    /// against cycles.
    fn load_dependencies(&self, owner: &str, imported: &[String], loading: &mut HashSet<String>) {
        for dep_name in imported {
            let dep_upper = dep_name.to_ascii_uppercase();
            // Built-in emulated modules are handled by the thunk mechanism.
            if BUILTIN_MODULES.contains(&dep_upper.as_str()) { continue; }
            // Already loaded — no further action needed (fixups will resolve via dll_mgr).
            {
                let dll_mgr = self.shared.dll_mgr.lock_or_recover();
                if dll_mgr.find_by_name(&dep_upper).is_some() { continue; }
            }
            // Cycle guard.
            if loading.contains(&dep_upper) {
                warn!("DLL '{}': circular import '{}', skipping", owner, dep_upper);
                continue;
            }
            match self.find_dll_path(&dep_upper) {
                Some(dep_path) => {
                    if let Err(e) = self.load_dll_impl(&dep_upper, &dep_path, loading) {
                        warn!("'{}': dependency '{}' failed: {}", owner, dep_upper, e);
                    }
                }
                None => warn!("'{}': dependency '{}' not found on host", owner, dep_upper),
            }
        }
    }

    /// Apply LX fixup records using the given per-object base addresses.
    ///
    /// `object_bases[i]` is the guest flat address of object `i+1` (1-based in LX).
    fn apply_fixups(&self, lx_file: &LxFile, object_bases: &[u32]) -> io::Result<()> {
        for (obj_idx, obj) in lx_file.object_table.iter().enumerate() {
            let base = object_bases[obj_idx];
            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.fixup_records_by_page.len() { break; }
                for record in &lx_file.fixup_records_by_page[page_idx] {
                    let target_addr = match &record.target {
                        FixupTarget::Internal { object_num, target_offset } => {
                            let oi = (*object_num as usize).wrapping_sub(1);
                            if oi < object_bases.len() {
                                object_bases[oi] as usize + *target_offset as usize
                            } else { 0 }
                        }
                        FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal } => {
                            let module = lx_file.imported_modules
                                .get((*module_ordinal as usize).wrapping_sub(1))
                                .map(|s| s.as_str()).unwrap_or("");
                            self.resolve_import(module, *proc_ordinal) as usize
                        }
                        FixupTarget::ExternalName { module_ordinal, proc_name_offset } => {
                            let module = lx_file.imported_modules
                                .get((*module_ordinal as usize).wrapping_sub(1))
                                .map(|s| s.as_str()).unwrap_or("");
                            let proc_name = lx_file.get_proc_name(*proc_name_offset)
                                .unwrap_or_default();
                            self.resolve_import_by_name(module, &proc_name) as usize
                        }
                        _ => 0,
                    };
                    if target_addr == 0 { continue; }
                    // An import with an additive value refers into the
                    // imported object (`Data.field` of imported data).
                    let target_addr = match (&record.target, record.additive) {
                        (FixupTarget::ExternalOrdinal { .. } | FixupTarget::ExternalName { .. }, Some(add)) =>
                            (target_addr as u32).wrapping_add(add) as usize,
                        _ => target_addr,
                    };
                    // Trace VIO/KBD import fixups
                    if let FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal } = &record.target {
                        let module = lx_file.imported_modules.get((*module_ordinal as usize).wrapping_sub(1));
                        if let Some(m) = module
                            && (m == "VIOCALLS" || m == "KBDCALLS") {
                                debug!("  Fixup: {}.{} -> target 0x{:08X}, src_type=0x{:02X}",
                                       m, proc_ordinal, target_addr, record.source_type & 0x0F);
                        }
                    }
                    for &off in &record.source_offsets {
                        let source_phys = base as usize + p * 4096 + off as usize;
                        let src_type = record.source_type & 0x0F;
                        match src_type {
                            0x07 => {
                                self.guest_write::<u32>(source_phys as u32, target_addr as u32).expect("fixup: write OOB");
                            }
                            0x08 => {
                                self.guest_write::<i32>(source_phys as u32, (target_addr as isize - (source_phys as isize + 4)) as i32).expect("fixup: write OOB");
                            }
                            0x02 | 0x03 => {
                                // 16:16 far pointer: derive tile selector from target flat address.
                                // Tile i covers [i*64KB .. (i+1)*64KB); selector = (TILED_SEL_START_INDEX + i) * 8.
                                // Always uses data tile selectors. If a Far16 thunk tries to JMP FAR
                                // to a data tile, the resulting #GP is caught by the VMEXIT handler
                                // which skips the call (Far16 thunks can't work in Warpine's flat model).
                                let tile_index = (target_addr >> 16) as u32;
                                let offset16 = (target_addr & 0xFFFF) as u16;
                                let selector = ((TILED_SEL_START_INDEX + tile_index) * 8) as u16;
                                self.guest_write::<u16>(source_phys as u32, offset16).expect("fixup: 16:16 offset OOB");
                                self.guest_write::<u16>(source_phys as u32 + 2, selector).expect("fixup: 16:16 sel OOB");
                            }
                            0x05 => {
                                self.guest_write::<u16>(source_phys as u32, (target_addr & 0xFFFF) as u16).expect("fixup: 16-bit offset OOB");
                            }
                            0x06 => {
                                self.guest_write::<u32>(source_phys as u32, target_addr as u32).expect("fixup: 16:32 offset OOB");
                                self.guest_write::<u16>(source_phys as u32 + 4, 0x08).expect("fixup: 16:32 selector OOB");
                            }
                            _ => {
                                log::warn!("Unhandled fixup source type 0x{:02X} at 0x{:08X}", src_type, source_phys);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Load a user DLL into guest memory by file path.
    ///
    /// If the DLL is already loaded its reference count is incremented and the
    /// existing handle is returned immediately (no re-parse).  Otherwise the LX
    /// file is parsed, all imported user-DLL dependencies are loaded recursively,
    /// pages are mapped, fixups applied, and the result registered in `dll_mgr`.
    ///
    /// Returns the HMODULE handle on success.
    pub fn load_dll(&self, dll_name: &str, dll_path: &str) -> Result<u32, String> {
        self.load_dll_impl(dll_name, dll_path, &mut HashSet::new())
    }

    /// Internal recursive implementation — `loading` is the set of module names
    /// currently on the call stack, used to break import cycles.
    fn load_dll_impl(
        &self,
        dll_name: &str,
        dll_path: &str,
        loading: &mut HashSet<String>,
    ) -> Result<u32, String> {
        let dll_name_upper = dll_name.to_ascii_uppercase();

        // Fast path: already loaded — bump refcount and return existing handle.
        {
            let mut dll_mgr = self.shared.dll_mgr.lock_or_recover();
            let maybe_handle = dll_mgr.find_by_name(&dll_name_upper).map(|d| d.handle);
            if let Some(h) = maybe_handle {
                dll_mgr.increment_refcount(h);
                return Ok(h);
            }
        }

        info!("DLL load: '{}' from '{}'", dll_name_upper, dll_path);

        // 1. Parse the DLL LX file
        let lx_file = LxFile::open(dll_path)
            .map_err(|e| format!("Failed to parse DLL '{}': {}", dll_path, e))?;

        // Verify it is a library
        if lx_file.header.module_flags & 0x8000 == 0 {
            warn!("DLL load: '{}' does not have LIBRARY flag set", dll_name_upper);
        }

        // 2. Allocate guest memory for each object, zeroed: its zero-filled
        //    pages and the rest of partial pages must not show what an
        //    earlier module or allocation left there.
        let mut object_bases: Vec<u32> = Vec::new();
        for obj in &lx_file.object_table {
            let size = obj.size.max(4096); // at least one page
            let Some(addr) = self.alloc_zeroed(size) else {
                let mut mem = self.shared.mem_mgr.lock_or_recover();
                for &b in &object_bases { mem.free(b); }
                return Err(format!("Out of guest memory for DLL '{}'", dll_name_upper));
            };
            object_bases.push(addr);
            debug!("  DLL object: size=0x{:X} → guest 0x{:08X}", size, addr);
        }

        // 3. Load pages into guest memory
        {
            let mut file = File::open(dll_path)
                .map_err(|e| format!("Cannot open DLL '{}': {}", dll_path, e))?;
            let data_pages_base = lx_file.header.data_pages_offset as u64;
            for (i, obj) in lx_file.object_table.iter().enumerate() {
                let base = object_bases[i];
                let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
                for p in 0..obj.page_count as usize {
                    let page_idx = obj_page_start + p;
                    if page_idx >= lx_file.page_map.len() { break; }
                    let pm = &lx_file.page_map[page_idx];
                    if pm.data_size == 0 { continue; }
                    let page_off = data_pages_base + ((pm.data_offset as u64) << lx_file.header.page_offset_shift);
                    let target = base + (p as u32 * 4096);
                    file.seek(SeekFrom::Start(page_off))
                        .map_err(|e| format!("DLL page seek error: {}", e))?;
                    let slice = self.guest_slice_mut(target, pm.data_size as usize)
                        .ok_or_else(|| format!("DLL page 0x{:08X} out of guest memory bounds", target))?;
                    file.read_exact(slice)
                        .map_err(|e| format!("DLL page read error: {}", e))?;
                }
            }
        }

        // 3.5. Recursively load imported user-DLL dependencies before applying
        //      fixups, so that `resolve_import` finds them in `dll_mgr`.
        loading.insert(dll_name_upper.clone());
        self.load_dependencies(&dll_name_upper, &lx_file.imported_modules, loading);

        // 4. Apply fixups using the allocated (rebased) object addresses.
        //    Dependencies are now in `dll_mgr`, so `resolve_import` can find them.
        self.apply_fixups(&lx_file, &object_bases)
            .map_err(|e| format!("DLL fixup error for '{}': {}", dll_name_upper, e))?;

        // 5. Build export map from entry table + resident and non-resident
        // names tables
        let mut exports_by_ordinal: HashMap<u32, u32> = HashMap::new();
        for exp in &lx_file.exports {
            let obj_idx = (exp.object_num as usize).wrapping_sub(1);
            if obj_idx < object_bases.len() {
                let guest_addr = object_bases[obj_idx] + exp.offset;
                exports_by_ordinal.insert(exp.ordinal, guest_addr);
            }
        }

        let mut exports_by_name: HashMap<String, u32> = HashMap::new();
        for (ordinal, name) in lx_file.resident_names.iter().chain(&lx_file.nonresident_names) {
            if let Some(&addr) = exports_by_ordinal.get(ordinal) {
                exports_by_name.insert(name.to_ascii_uppercase(), addr);
            }
        }

        info!("DLL '{}': {} exports, {} named exports",
              dll_name_upper, exports_by_ordinal.len(), exports_by_name.len());

        // 6. Compute INITTERM entry address (if present).
        let initterm_addr = if lx_file.header.eip_object != 0 {
            let obj_idx = (lx_file.header.eip_object as usize).wrapping_sub(1);
            if obj_idx < object_bases.len() {
                let addr = object_bases[obj_idx] + lx_file.header.eip;
                debug!("DLL '{}': has INITTERM entry at 0x{:08X}", dll_name_upper, addr);
                Some(addr)
            } else {
                None
            }
        } else {
            None
        };

        // Kept for step 8: `object_bases` moves into the DLL record.
        let object_bases_for_res = object_bases.clone();

        // 7. Register in dll_mgr, with its OS/2 path (DosQueryModuleName).
        let os2_path = self.dll_os2_path(&dll_name_upper, dll_path);
        let handle = {
            let mut dll_mgr = self.shared.dll_mgr.lock_or_recover();
            let handle = dll_mgr.alloc_handle();
            dll_mgr.register(LoadedDll {
                name: dll_name_upper.clone(),
                handle,
                object_bases,
                object_sizes: lx_file.object_table.iter().map(|o| o.size.max(4096)).collect(),
                exports_by_ordinal,
                exports_by_name,
                ref_count: 1,
                initterm_addr,
                is_builtin: false,
            });
            dll_mgr.set_module_path(handle, os2_path);
            dll_mgr.set_host_file(handle, std::path::PathBuf::from(dll_path));
            // Its _DLL_InitTerm runs after those of the modules it imports
            // (loaded above): at process start, or in DosLoadModule.
            if let Some(entry) = initterm_addr { dll_mgr.add_pending_init(handle, entry); }
            handle
        };

        // 8. Its resources, found by module handle (GpiLoadBitmap,
        //    WinLoadString, DosGetResource … with this hmod).
        {
            let mut res_mgr = self.shared.resource_mgr.lock_or_recover();
            for res in &lx_file.resources {
                let obj_idx = (res.object_num as usize).wrapping_sub(1);
                if let Some(&base) = object_bases_for_res.get(obj_idx) {
                    res_mgr.add(handle, res.type_id, res.name_id, base + res.offset, res.size);
                }
            }
        }

        // A debugger attached to a running program reads the new module
        // list at the next stop (ADR-0104).
        if let Some(gdb) = self.shared.gdb_state.as_ref() { gdb.libraries_changed(); }

        loading.remove(&dll_name_upper);
        Ok(handle)
    }

    /// The module name and host file of `name` as `DosLoadModule` takes
    /// it: a plain module name is searched for ([`Self::find_dll_path`]);
    /// an OS/2 path (`C:\DIR\X.DLL`) is resolved through the drives,
    /// inside their directories only, and names the module by its file's
    /// base name.
    pub fn find_module_file(&self, name: &str) -> Option<(String, String)> {
        if is_plain_module_name(name) {
            return self.find_dll_path(name).map(|p| (name.to_string(), p));
        }
        if name.len() > 260 || name.chars().any(|c| c.is_control()) { return None; }
        let host = self.shared.drive_mgr.lock_or_recover().resolve_to_host_path(name)?;
        if !host.is_file() { return None; }
        let file = name.rsplit(['\\', '/', ':']).next().filter(|f| !f.is_empty())?;
        let module = file.split('.').next().filter(|m| !m.is_empty())?.to_ascii_uppercase();
        Some((module, host.to_string_lossy().into_owned()))
    }

    /// Search for a DLL by module name (ADR-0092), in this order:
    /// 1. BEGINLIBPATH (`DosSetExtLIBPATH`);
    /// 2. the running program's own directory (Warpine's: OS/2 finds it
    ///    only through a LIBPATH entry);
    /// 3. CONFIG.SYS's `LIBPATH=` (else `C:\OS2\DLL`), `.` being the
    ///    current directory;
    /// 4. ENDLIBPATH.
    ///
    /// Only a plain module name is searched: one with a path separator, a
    /// drive, `..` or a control character is not (it would reach outside
    /// these directories on the host). The directories are OS/2 paths,
    /// resolved through the drives, so none leads outside them.
    pub fn find_dll_path(&self, dll_name: &str) -> Option<String> {
        if !is_plain_module_name(dll_name) {
            log::warn!("module name {dll_name:?} is not a plain name; not searched");
            return None;
        }
        let upper = dll_name.to_ascii_uppercase();
        let stem = upper.strip_suffix(".DLL").unwrap_or(&upper);
        let file = format!("{stem}.DLL");
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);

        let begin = self.shared.begin_libpath.lock_or_recover().clone();
        if let Some(p) = self.find_in_os2_dirs(begin.split(';'), &file) { return Some(p); }

        // The program's own directory, on the host.
        if let Some(exe_dir) = exe_dir(&self.shared.exe_name.lock_or_recover()) {
            for fname in [file.to_ascii_lowercase(), file.clone()] {
                let p = format!("{exe_dir}/{fname}");
                if Path::new(&p).is_file() { return Some(p); }
            }
        }

        let libpath: Vec<String> = self.shared.config_sys().libpath_dirs().iter()
            .map(|d| super::codepage::cp_decode(d, cp)).collect();
        if let Some(p) = self.find_in_os2_dirs(libpath.iter().map(String::as_str), &file) { return Some(p); }

        let end = self.shared.end_libpath.lock_or_recover().clone();
        self.find_in_os2_dirs(end.split(';'), &file)
    }

    /// The host path of `file` in the first of the OS/2 directories `dirs`
    /// that holds it (names matched as the drives match them); blank and
    /// over-long entries are skipped.
    fn find_in_os2_dirs<'a>(&self, dirs: impl IntoIterator<Item = &'a str>, file: &str) -> Option<String> {
        let dm = self.shared.drive_mgr.lock_or_recover();
        dirs.into_iter().map(str::trim)
            .filter(|d| !d.is_empty() && d.len() <= super::config_sys::MAX_LIBPATH_DIR)
            .take(super::config_sys::MAX_LIBPATH_ENTRIES)
            .find_map(|d| {
                let sep = if d.ends_with(['\\', '/', ':']) { "" } else { "\\" };
                let host = dm.resolve_to_host_path(&format!("{d}{sep}{file}"))?;
                host.is_file().then(|| host.to_string_lossy().into_owned())
            })
    }
}

/// `OBJRSRC`: the object holds resources (LX object flags).
pub(crate) const OBJ_RESOURCE: u32 = 0x0008;
/// The largest resource object Warpine allocates.
const MAX_RESOURCE_OBJECT: u32 = 64 * 1024 * 1024;

/// Check an EXE's objects, `(base, virtual size, pages)` each: every
/// object, its pages included, lies wholly in [`EXE_AREA_START`] ..
/// [`EXE_AREA_END`], and no two overlap (ADR-0094). The error names the
/// first object that does not fit.
pub(crate) fn check_exe_objects(objects: &[(u32, u32, u32)]) -> Result<(), String> {
    let mut spans: Vec<(u64, u64, usize)> = Vec::with_capacity(objects.len());
    for (i, &(base, size, pages)) in objects.iter().enumerate() {
        let len = u64::from(size).max(u64::from(pages) * u64::from(PAGE_SIZE));
        let start = u64::from(base);
        let end = (start + len).next_multiple_of(u64::from(PAGE_SIZE));
        if start < u64::from(EXE_AREA_START) || end > u64::from(EXE_AREA_END) {
            return Err(format!(
                "object {} (0x{start:08X}-0x{end:08X}) lies outside the program area 0x{EXE_AREA_START:08X}-0x{EXE_AREA_END:08X}",
                i + 1));
        }
        if let Some(&(_, _, j)) = spans.iter().find(|&&(s, e, _)| start < e && s < end) {
            return Err(format!("object {} overlaps object {}", i + 1, j + 1));
        }
        spans.push((start, end, i));
    }
    Ok(())
}

/// Whether `name` names a module without a path: not empty, no `/`, `\`
/// or `:`, not `.` or `..`, no control characters, at most 255 bytes.
pub(crate) fn is_plain_module_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && name != "." && name != ".."
        && !name.chars().any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
}

/// The host directory of the executable `exe` (as given on the command
/// line): its parent, or the current directory for a bare file name such as
/// `hello.exe` run from its own directory; `None` when there is no executable.
fn exe_dir(exe: &str) -> Option<String> {
    if exe.is_empty() { return None; }
    let parent = Path::new(exe).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    Some(if parent.is_empty() { ".".to_string() } else { parent })
}

#[cfg(test)]
mod tests {
    use super::super::Loader;
    use super::super::MutexExt;

    /// A program whose entry or stack object is not one of its objects is
    /// refused, not run into a panic.
    #[test]
    fn a_program_needs_entry_and_stack_objects() {
        let path = "samples/hello/hello.exe";
        if !std::path::Path::new(path).exists() { return; }
        let lx = crate::lx::LxFile::open(path).expect("hello.exe parses");
        for (eip, esp) in [(0, lx.header.esp_object), (lx.header.eip_object, 0), (99, 1)] {
            let mut bad = crate::lx::LxFile::open(path).unwrap();
            bad.header.eip_object = eip;
            bad.header.esp_object = esp;
            let mut loader = Loader::new_mock();
            let err = loader.load(&bad, path).expect_err("refused");
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{err}");
        }
        assert!(Loader::new_mock().load(&lx, path).is_ok());
    }

    /// Load noinit.dll (samples/dll_noinit) with the mock loader; its 3 exports are registered by ordinal and by name.
    #[test]
    fn test_load_dll_exports_by_name_and_ordinal() {
        let path = "samples/dll_noinit/noinit.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        // Give exe_name so find_dll_path has a directory to search
        *loader.shared.exe_name.lock_or_recover() = "samples/dll_noinit/test.exe".to_string();

        let handle = loader.load_dll("NOINIT", path).expect("load_dll failed");
        assert!(handle > 0, "handle must be nonzero");

        let dll_mgr = loader.shared.dll_mgr.lock_or_recover();
        let dll = dll_mgr.find_by_handle(handle).expect("DLL not registered");
        assert_eq!(dll.name, "NOINIT");
        assert_eq!(dll.exports_by_ordinal.len(), 3, "expected 3 exported ordinals");
        // Names from both tables: NoInitAdd / NoInitTwice non-resident,
        // NoInitAnswer resident.
        for name in ["NOINITADD", "NOINITTWICE", "NOINITANSWER"] {
            assert!(dll.exports_by_name.contains_key(name), "{name} export missing");
        }

        // All export addresses must be within the mock guest memory range
        for (&ord, &addr) in &dll.exports_by_ordinal {
            assert!(addr > 0, "ordinal {} has zero guest address", ord);
        }
    }

    /// find_dll_path locates noinit.dll next to the exe.
    #[test]
    fn test_find_dll_path() {
        let path = "samples/dll_noinit/noinit.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/dll_noinit/test.exe".to_string();

        let found = loader.find_dll_path("NOINIT");
        assert!(found.is_some(), "find_dll_path should find noinit.dll");

        let found_missing = loader.find_dll_path("NONEXISTENT_DLL_ABCD");
        assert!(found_missing.is_none(), "nonexistent DLL should return None");
    }

    /// The executable's directory: its parent, or "." for a bare file name
    /// (a program started from its own directory finds the DLLs beside it).
    #[test]
    fn exe_dir_of_a_bare_name_is_the_current_directory() {
        use super::exe_dir;
        assert_eq!(exe_dir("samples/hello/hello.exe").as_deref(), Some("samples/hello"));
        assert_eq!(exe_dir("/opt/os2/app.exe").as_deref(), Some("/opt/os2"));
        assert_eq!(exe_dir("hello.exe").as_deref(), Some("."));
        assert_eq!(exe_dir(""), None);
    }

    /// Only plain module names are searched on the host: no path, drive,
    /// `..` or control characters (DosLoadModule, somFindClsInFile).
    #[test]
    fn dll_search_takes_only_plain_module_names() {
        use super::is_plain_module_name as plain;
        for ok in ["GREETER", "student.dll", "A-B_C", "x.y"] { assert!(plain(ok), "{ok}"); }
        for bad in ["", ".", "..", "../x", "a/b", "C:\\OS2\\X.DLL", "a\\b", "c:x", "a\nb", &"x".repeat(256)] {
            assert!(!plain(bad), "{bad:?}");
        }
        let l = crate::loader::Loader::new_mock();
        assert_eq!(l.find_dll_path("../../../../../../etc/passwd"), None);
    }

    /// DosLoadModule's names: an OS/2 path is resolved through the drives,
    /// inside them only, and names the module by its file.
    /// The DLL search (ADR-0092): BEGINLIBPATH, the program's directory,
    /// CONFIG.SYS's LIBPATH (`.` the current directory), ENDLIBPATH; no
    /// entry leads outside the drives.
    #[test]
    fn dlls_are_searched_along_the_libpaths() {
        use crate::loader::vfs_hostdir::HostDirBackend;
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("EVIL.DLL"), b"x").unwrap();
        let c = tempfile::TempDir::new().unwrap();
        let exe = tempfile::TempDir::new().unwrap();
        for dir in ["begin", "liba", "libb", "end", "os2/dll"] {
            std::fs::create_dir_all(c.path().join(dir)).unwrap();
        }
        let put = |path: &str| std::fs::write(c.path().join(path), b"x").unwrap();
        put("begin/BOTH.DLL"); put("libb/BOTH.DLL");
        put("libb/Lib.dll"); put("end/END.DLL"); put("libb/END.DLL");
        put("end/ONLYEND.DLL"); put("DOT.DLL"); put("os2/dll/SYS.DLL");
        std::fs::write(exe.path().join("own.dll"), b"x").unwrap();
        put("libb/OWN.DLL");
        let escape = format!("C:\\..\\{}", outside.path().file_name().unwrap().to_string_lossy());
        std::fs::write(c.path().join("CONFIG.SYS"),
            format!("LIBPATH=C:\\NOWHERE\\;{escape};C:\\LIBA;c:\\libb\\;.\r\n")).unwrap();

        let l = crate::loader::Loader::new_mock();
        l.shared.drive_mgr.lock_or_recover().mount(2, Box::new(HostDirBackend::new(c.path().to_path_buf()).unwrap()));
        *l.shared.exe_name.lock_or_recover() = exe.path().join("app.exe").to_string_lossy().into_owned();
        let found = |name: &str| l.find_dll_path(name)
            .map(|p| p.strip_prefix(&*c.path().to_string_lossy()).map(str::to_string).unwrap_or(p));

        assert_eq!(found("LIB").as_deref(), Some("/libb/Lib.dll"), "LIBPATH, names matched as on drive C");
        assert_eq!(found("dot.dll").as_deref(), Some("/DOT.DLL"), "`.`: the current directory");
        assert_eq!(found("SYS"), None, "LIBPATH replaces the default C:\\OS2\\DLL");
        assert_eq!(found("EVIL"), None, "an entry leading out of drive C");
        assert!(found("OWN").unwrap().ends_with("own.dll"), "the program's directory before LIBPATH");
        assert_eq!(found("ONLYEND"), None);
        assert_eq!(found("END").as_deref(), Some("/libb/END.DLL"));

        *l.shared.begin_libpath.lock_or_recover() = "C:\\BEGIN".into();
        *l.shared.end_libpath.lock_or_recover() = " ; C:\\END".into();
        assert_eq!(found("BOTH").as_deref(), Some("/begin/BOTH.DLL"), "BEGINLIBPATH first");
        assert_eq!(found("ONLYEND").as_deref(), Some("/end/ONLYEND.DLL"), "ENDLIBPATH last");
        assert_eq!(found("END").as_deref(), Some("/libb/END.DLL"), "LIBPATH before ENDLIBPATH");
        *l.shared.begin_libpath.lock_or_recover() = escape.clone();
        assert_eq!(found("EVIL"), None, "BEGINLIBPATH leading out of drive C");
    }

    /// Without a LIBPATH statement, C:\OS2\DLL is searched.
    #[test]
    fn libpath_defaults_to_the_system_dll_directory() {
        use crate::loader::vfs_hostdir::HostDirBackend;
        let c = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(c.path().join("OS2/DLL")).unwrap();
        std::fs::write(c.path().join("OS2/DLL/SYS.DLL"), b"x").unwrap();
        let l = crate::loader::Loader::new_mock();
        l.shared.drive_mgr.lock_or_recover().mount(2, Box::new(HostDirBackend::new(c.path().to_path_buf()).unwrap()));
        assert!(l.find_dll_path("SYS").is_some_and(|p| p.ends_with("OS2/DLL/SYS.DLL")));
    }

    /// An EXE's objects must lie in the program area and not overlap
    /// (ADR-0094); an object reaching Warpine's system area, the API
    /// thunks or past guest memory is refused, without overflow.
    #[test]
    fn exe_objects_must_fit_the_program_area() {
        use super::check_exe_objects;
        use crate::loader::constants::{EXE_AREA_END, EXE_AREA_START, SYSTEM_AREA_BASE, TIB_BASE};
        // Open Watcom's layout: code at 64 KB, data after it.
        assert_eq!(check_exe_objects(&[(0x1_0000, 0x2CCB, 3), (0x2_0000, 0x2000, 2)]), Ok(()));
        // A data object of more than 1 MB, as npipe_cut_test once had: fine
        // now that nothing of Warpine's lies below 16 MB.
        assert_eq!(check_exe_objects(&[(0x1_0000, 0x3000, 3), (0x2_0000, 0x10_1000, 1)]), Ok(()));
        assert_eq!(check_exe_objects(&[(EXE_AREA_END - 0x1000, 0x1000, 1)]), Ok(()), "up to the end");
        for (objects, what) in [
            (vec![(0, 0x1000, 1)], "page 0"),
            (vec![(EXE_AREA_START - 0x1000, 0x1000, 1)], "below 64 KB"),
            (vec![(EXE_AREA_END - 0x1000, 0x1001, 1)], "into the API thunks"),
            (vec![(SYSTEM_AREA_BASE, 0x1000, 1)], "the system area"),
            (vec![(TIB_BASE, 0x10, 1)], "the TIB"),
            (vec![(0x1_0000, 0x10, 0x1_0000)], "pages beyond the size"),
            (vec![(u32::MAX - 0xFFF, 0x1000, 1)], "at the top of 4 GB"),
            (vec![(0x1_0000, u32::MAX, 1)], "a huge size"),
        ] {
            assert!(check_exe_objects(&objects).is_err(), "{what}");
        }
        assert_eq!(check_exe_objects(&[(0x1_0000, 0x2000, 2), (0x1_1000, 0x1000, 1)]),
                   Err("object 2 overlaps object 1".into()));
        assert_eq!(check_exe_objects(&[]), Ok(()));
    }

    #[test]
    fn module_files_by_os2_path() {
        use crate::loader::vfs_hostdir::HostDirBackend;
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("evil.dll"), b"x").unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("libs")).unwrap();
        std::fs::write(tmp.path().join("libs/Greeter.DLL"), b"x").unwrap();
        let l = crate::loader::Loader::new_mock();
        l.shared.drive_mgr.lock_or_recover().mount(2, Box::new(HostDirBackend::new(tmp.path().to_path_buf()).unwrap()));
        let (module, path) = l.find_module_file("C:\\LIBS\\GREETER.DLL").expect("found through drive C");
        assert_eq!(module, "GREETER");
        assert!(path.ends_with("libs/Greeter.DLL"), "{path}");
        assert_eq!(l.find_module_file("C:\\LIBS\\NONE.DLL"), None);
        assert_eq!(l.find_module_file("C:\\LIBS"), None, "a directory");
        let up = format!("C:\\..\\{}\\evil.dll", outside.path().file_name().unwrap().to_string_lossy());
        assert_eq!(l.find_module_file(&up), None, "outside the drive");
        assert_eq!(l.find_module_file("Q:\\X.DLL"), None, "no such drive");
    }

    /// Verify BUILTIN_MODULES covers all modules handled by the thunk mechanism.
    #[test]
    fn test_builtin_modules_are_complete() {
        use super::BUILTIN_MODULES;
        // Every module base defined in descriptors.rs must appear in BUILTIN_MODULES.
        let required = ["DOSCALLS", "QUECALLS", "PMWIN", "PMGPI",
                        "KBDCALLS", "VIOCALLS", "SESMGR", "NLS", "MSG", "MDM", "UCONV",
                        "SO32DLL", "TCP32DLL", "MOUCALLS", "PMSHAPI", "HELPMGR",
                        "PMSPL", "PMCTLS", "WARPINE", "PMDRAG", "SOM"];
        for m in &required {
            assert!(BUILTIN_MODULES.contains(m),
                    "BUILTIN_MODULES is missing '{}'", m);
        }
    }

    /// Second load_dll call returns the same handle with incremented refcount.
    #[test]
    fn test_load_dll_refcount_on_second_load() {
        let path = "samples/dll_noinit/noinit.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/dll_noinit/test.exe".to_string();

        let h1 = loader.load_dll("NOINIT", path).expect("first load");
        let h2 = loader.load_dll("NOINIT", path).expect("second load");
        assert_eq!(h1, h2, "same handle on second load");

        let dll_mgr = loader.shared.dll_mgr.lock_or_recover();
        let dll = dll_mgr.find_by_handle(h1).expect("dll must still be registered");
        assert_eq!(dll.ref_count, 2, "refcount should be 2 after two loads");
    }

    /// DosFreeModule decrements refcount; DLL stays loaded until refcount = 0.
    #[test]
    fn test_dos_free_module_refcount() {
        let path = "samples/dll_noinit/noinit.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/dll_noinit/test.exe".to_string();

        let h = loader.load_dll("NOINIT", path).expect("load");
        let h2 = loader.load_dll("NOINIT", path).expect("second load"); // refcount = 2
        assert_eq!(h, h2);

        // First free: refcount drops to 1, DLL still loaded
        loader.dos_free_module(h);
        {
            let dll_mgr = loader.shared.dll_mgr.lock_or_recover();
            assert!(dll_mgr.find_by_handle(h).is_some(), "DLL should still be loaded after first free");
            assert_eq!(dll_mgr.find_by_handle(h).unwrap().ref_count, 1);
        }

        // Second free: refcount drops to 0, DLL unloaded
        loader.dos_free_module(h);
        {
            let dll_mgr = loader.shared.dll_mgr.lock_or_recover();
            assert!(dll_mgr.find_by_handle(h).is_none(), "DLL should be unloaded after second free");
        }
    }

    /// noinit.dll has eip_object == 0 so initterm_addr must be None.
    #[test]
    fn test_load_dll_initterm_none_without_an_entry_point() {
        let path = "samples/dll_noinit/noinit.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/dll_noinit/test.exe".to_string();

        let handle = loader.load_dll("NOINIT", path).expect("load_dll failed");
        let dll_mgr = loader.shared.dll_mgr.lock_or_recover();
        let dll = dll_mgr.find_by_handle(handle).unwrap();
        assert_eq!(dll.initterm_addr, None,
            "noinit.dll has no INITTERM entry — initterm_addr should be None");
    }

    /// DosLoadModule for a DLL without INITTERM must return ApiResult::Normal(NO_ERROR)
    /// and immediately write the handle to *phmod.
    #[test]
    fn test_dos_load_module_no_initterm_returns_normal() {
        use super::super::ApiResult;
        use super::super::constants::NO_ERROR;

        let path = "samples/dll_noinit/noinit.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/dll_noinit/test.exe".to_string();

        let phmod = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();
        let mod_name_buf = loader.shared.mem_mgr.lock_or_recover().alloc(32).unwrap();
        loader.guest_write_bytes(mod_name_buf, b"NOINIT\0");

        let result = loader.dos_load_module(0, 0, mod_name_buf, phmod);
        assert!(matches!(result, ApiResult::Normal(c) if c == NO_ERROR),
            "no-INITTERM DLL must return Normal(NO_ERROR), got {:?}", result);

        let written_handle = loader.guest_read::<u32>(phmod).unwrap();
        assert!(written_handle > 0, "phmod must be written immediately for no-INITTERM DLL");
    }
}
