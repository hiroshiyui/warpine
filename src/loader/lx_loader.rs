// SPDX-License-Identifier: GPL-3.0-only

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
    "VIOCALLS", "SESMGR", "NLS", "MSG", "MDM",
];

impl super::Loader {
    pub fn is_pm_app(&self, lx_file: &LxFile) -> bool {
        lx_file.imported_modules.iter().any(|m| m == "PMWIN" || m == "PMGPI")
    }

    pub fn get_shared(&self) -> Arc<super::SharedState> {
        Arc::clone(&self.shared)
    }

    pub fn load<P: AsRef<Path>>(&mut self, lx_file: &LxFile, path: P) -> io::Result<()> {
        let mut file = File::open(path)?;
        let data_pages_base = lx_file.header.data_pages_offset as u64;
        // Use preferred base addresses from the object table for the main exe
        let object_bases: Vec<u32> = lx_file.object_table.iter().map(|o| o.base_address).collect();

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
                    res_mgr.add(res.type_id, res.name_id, guest_addr, res.size);
                    debug!("  Resource: type={} id={} addr=0x{:08X} size={}", res.type_id, res.name_id, guest_addr, res.size);
                }
            }
        }

        self.apply_fixups(lx_file, &object_bases)
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
                        _ => 0,
                    };
                    if target_addr == 0 { continue; }
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

        // 2. Allocate guest memory for each object
        let mut object_bases: Vec<u32> = Vec::new();
        {
            let mut mem = self.shared.mem_mgr.lock_or_recover();
            for obj in &lx_file.object_table {
                let size = obj.size.max(4096); // at least one page
                let addr = mem.alloc(size)
                    .ok_or_else(|| format!("Out of guest memory for DLL '{}'", dll_name_upper))?;
                object_bases.push(addr);
                debug!("  DLL object: size=0x{:X} → guest 0x{:08X}", size, addr);
            }
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
        for dep_name in &lx_file.imported_modules {
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
                warn!("DLL '{}': circular import '{}', skipping", dll_name_upper, dep_upper);
                continue;
            }
            match self.find_dll_path(&dep_upper) {
                Some(dep_path) => {
                    if let Err(e) = self.load_dll_impl(&dep_upper, &dep_path, loading) {
                        warn!("DLL '{}': dependency '{}' failed: {}", dll_name_upper, dep_upper, e);
                    }
                }
                None => {
                    warn!("DLL '{}': dependency '{}' not found on host", dll_name_upper, dep_upper);
                }
            }
        }

        // 4. Apply fixups using the allocated (rebased) object addresses.
        //    Dependencies are now in `dll_mgr`, so `resolve_import` can find them.
        self.apply_fixups(&lx_file, &object_bases)
            .map_err(|e| format!("DLL fixup error for '{}': {}", dll_name_upper, e))?;

        // 5. Build export map from entry table + non-resident names table
        let mut exports_by_ordinal: HashMap<u32, u32> = HashMap::new();
        for exp in &lx_file.exports {
            let obj_idx = (exp.object_num as usize).wrapping_sub(1);
            if obj_idx < object_bases.len() {
                let guest_addr = object_bases[obj_idx] + exp.offset;
                exports_by_ordinal.insert(exp.ordinal, guest_addr);
            }
        }

        let mut exports_by_name: HashMap<String, u32> = HashMap::new();
        for (ordinal, name) in &lx_file.nonresident_names {
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

        // 7. Register in dll_mgr
        let handle = {
            let mut dll_mgr = self.shared.dll_mgr.lock_or_recover();
            let handle = dll_mgr.alloc_handle();
            dll_mgr.register(LoadedDll {
                name: dll_name_upper.clone(),
                handle,
                object_bases,
                exports_by_ordinal,
                exports_by_name,
                ref_count: 1,
                initterm_addr,
            });
            handle
        };

        loading.remove(&dll_name_upper);
        Ok(handle)
    }

    /// Search for a DLL by module name on the host filesystem.
    ///
    /// Search order:
    /// 1. Same directory as the running exe
    /// 2. `C:\OS2\DLL\` via the DriveManager host path
    pub fn find_dll_path(&self, dll_name: &str) -> Option<String> {
        // Strip .DLL extension if present, then construct filename
        let upper = dll_name.to_ascii_uppercase();
        let stem = upper.strip_suffix(".DLL").unwrap_or(&upper);
        let filename_lower = format!("{}.dll", stem.to_ascii_lowercase());
        let filename_upper = format!("{}.DLL", stem);

        // 1. Next to the running exe
        let exe_dir = {
            let exe = self.shared.exe_name.lock_or_recover();
            Path::new(exe.as_str())
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        };
        if !exe_dir.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::super::Loader;
    use super::super::MutexExt;

    /// Load jpos2dll.dll with mock loader; verify 7 exports are registered.
    #[test]
    fn test_load_dll_jpos2dll() {
        let path = "samples/4os2/jpos2dll.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        // Give exe_name so find_dll_path has a directory to search
        *loader.shared.exe_name.lock_or_recover() = "samples/4os2/4os2.exe".to_string();

        let handle = loader.load_dll("JPOS2DLL", path).expect("load_dll failed");
        assert!(handle > 0, "handle must be nonzero");

        let dll_mgr = loader.shared.dll_mgr.lock_or_recover();
        let dll = dll_mgr.find_by_handle(handle).expect("DLL not registered");
        assert_eq!(dll.name, "JPOS2DLL");
        assert_eq!(dll.exports_by_ordinal.len(), 7, "expected 7 exported ordinals");
        assert!(dll.exports_by_name.contains_key("SENDKEYS"), "SENDKEYS export missing");
        assert!(dll.exports_by_name.contains_key("QUERYEXTLIBPATH"), "QUERYEXTLIBPATH export missing");
        assert!(dll.exports_by_name.contains_key("KEYSTACKHOOKPROC"), "KEYSTACKHOOKPROC export missing");

        // All export addresses must be within the mock guest memory range
        for (&ord, &addr) in &dll.exports_by_ordinal {
            assert!(addr > 0, "ordinal {} has zero guest address", ord);
        }
    }

    /// find_dll_path locates jpos2dll.dll next to the exe.
    #[test]
    fn test_find_dll_path() {
        let path = "samples/4os2/jpos2dll.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/4os2/4os2.exe".to_string();

        let found = loader.find_dll_path("JPOS2DLL");
        assert!(found.is_some(), "find_dll_path should find jpos2dll.dll");

        let found_missing = loader.find_dll_path("NONEXISTENT_DLL_ABCD");
        assert!(found_missing.is_none(), "nonexistent DLL should return None");
    }

    /// Verify BUILTIN_MODULES covers all modules handled by the thunk mechanism.
    #[test]
    fn test_builtin_modules_are_complete() {
        use super::BUILTIN_MODULES;
        // Every module base defined in descriptors.rs must appear in BUILTIN_MODULES.
        let required = ["DOSCALLS", "QUECALLS", "PMWIN", "PMGPI",
                        "KBDCALLS", "VIOCALLS", "SESMGR", "NLS", "MSG", "MDM"];
        for m in &required {
            assert!(BUILTIN_MODULES.contains(m),
                    "BUILTIN_MODULES is missing '{}'", m);
        }
    }

    /// Second load_dll call returns the same handle with incremented refcount.
    #[test]
    fn test_load_dll_refcount_on_second_load() {
        let path = "samples/4os2/jpos2dll.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/4os2/4os2.exe".to_string();

        let h1 = loader.load_dll("JPOS2DLL", path).expect("first load");
        let h2 = loader.load_dll("JPOS2DLL", path).expect("second load");
        assert_eq!(h1, h2, "same handle on second load");

        let dll_mgr = loader.shared.dll_mgr.lock_or_recover();
        let dll = dll_mgr.find_by_handle(h1).expect("dll must still be registered");
        assert_eq!(dll.ref_count, 2, "refcount should be 2 after two loads");
    }

    /// DosFreeModule decrements refcount; DLL stays loaded until refcount = 0.
    #[test]
    fn test_dos_free_module_refcount() {
        let path = "samples/4os2/jpos2dll.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/4os2/4os2.exe".to_string();

        let h = loader.load_dll("JPOS2DLL", path).expect("load");
        let h2 = loader.load_dll("JPOS2DLL", path).expect("second load"); // refcount = 2
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

    /// jpos2dll.dll has eip_object == 0 so initterm_addr must be None.
    #[test]
    fn test_load_dll_initterm_none_for_jpos2dll() {
        let path = "samples/4os2/jpos2dll.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/4os2/4os2.exe".to_string();

        let handle = loader.load_dll("JPOS2DLL", path).expect("load_dll failed");
        let dll_mgr = loader.shared.dll_mgr.lock_or_recover();
        let dll = dll_mgr.find_by_handle(handle).unwrap();
        assert_eq!(dll.initterm_addr, None,
            "jpos2dll has no INITTERM entry — initterm_addr should be None");
    }

    /// DosLoadModule for a DLL without INITTERM must return ApiResult::Normal(NO_ERROR)
    /// and immediately write the handle to *phmod.
    #[test]
    fn test_dos_load_module_no_initterm_returns_normal() {
        use super::super::ApiResult;
        use super::super::constants::NO_ERROR;

        let path = "samples/4os2/jpos2dll.dll";
        if !std::path::Path::new(path).exists() { return; }

        let loader = Loader::new_mock();
        *loader.shared.exe_name.lock_or_recover() = "samples/4os2/4os2.exe".to_string();

        let phmod = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();
        let mod_name_buf = loader.shared.mem_mgr.lock_or_recover().alloc(32).unwrap();
        loader.guest_write_bytes(mod_name_buf, b"JPOS2DLL\0");

        let result = loader.dos_load_module(0, 0, mod_name_buf, phmod);
        assert!(matches!(result, ApiResult::Normal(c) if c == NO_ERROR),
            "no-INITTERM DLL must return Normal(NO_ERROR), got {:?}", result);

        let written_handle = loader.guest_read::<u32>(phmod).unwrap();
        assert!(written_handle > 0, "phmod must be written immediately for no-INITTERM DLL");
    }
}
