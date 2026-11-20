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
        let mut file = File::open(&path)?;
        let data_pages_base = lx_file.header.data_pages_offset as u64;
        // Use preferred base addresses from the object table for the main exe
        let object_bases: Vec<u32> = lx_file.object_table.iter().map(|o| o.base_address).collect();
        // The program's objects, for DosQueryModFromEIP.
        {
            let mut dlls = self.shared.dll_mgr.lock_or_recover();
            dlls.exe_objects = lx_file.object_table.iter().map(|o| (o.base_address, o.size)).collect();
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
                if lx_file.page_map[page_idx].data_size > 0 {
                    file.seek(SeekFrom::Start(page_off))?;
                    file.read_exact(self.guest_slice_mut(target as u32, lx_file.page_map[page_idx].data_size as usize).expect("load: page target OOB"))?;
                }
            }
        }

        // Populate resource manager with precomputed guest addresses
        if !lx_file.resources.is_empty() {
            let mut res_mgr = self.shared.resource_mgr.lock_or_recover();
            for res in &lx_file.resources {
                let obj_idx = (res.object_num as usize).wrapping_sub(1);
                if obj_idx < lx_file.object_table.len() {
                    let guest_addr = lx_file.object_table[obj_idx].base_address + res.offset;
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

    /// Search for a DLL by module name on the host filesystem.
    ///
    /// Search order:
    /// 1. Same directory as the running exe
    /// 2. `C:\OS2\DLL\` via the DriveManager host path
    ///
    /// Only a plain module name is searched: one with a path separator, a
    /// drive, `..` or a control character is not (it would reach outside
    /// these directories on the host).
    pub fn find_dll_path(&self, dll_name: &str) -> Option<String> {
        if !is_plain_module_name(dll_name) {
            log::warn!("module name {dll_name:?} is not a plain name; not searched");
            return None;
        }
        // Strip .DLL extension if present, then construct filename
        let upper = dll_name.to_ascii_uppercase();
        let stem = upper.strip_suffix(".DLL").unwrap_or(&upper);
        let filename_lower = format!("{}.dll", stem.to_ascii_lowercase());
        let filename_upper = format!("{}.DLL", stem);

        // 1. Next to the running exe
        let exe_dir = exe_dir(&self.shared.exe_name.lock_or_recover());
        if let Some(exe_dir) = exe_dir {
            for fname in &[filename_lower.as_str(), filename_upper.as_str()] {
                let p = format!("{}/{}", exe_dir, fname);
                if Path::new(&p).exists() { return Some(p); }
            }
        }

        // 2. C:\OS2\DLL\ via DriveManager host path
        let c_host = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            dm.drive_config(2).map(|cfg| cfg.host_path.clone())
        };
        if let Some(c_root) = c_host {
            for subdir in &["os2/dll", "OS2/DLL"] {
                for fname in &[filename_lower.as_str(), filename_upper.as_str()] {
                    let p = format!("{}/{}/{}", c_root.display(), subdir, fname);
                    if Path::new(&p).exists() { return Some(p); }
                }
            }
        }

        None
    }
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
