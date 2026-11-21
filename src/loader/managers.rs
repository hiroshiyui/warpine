// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;
use std::fs::{File, ReadDir};
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
struct AllocBlock {
    addr: u32,
    _size: u32,
}

pub struct MemoryManager {
    allocated: Vec<AllocBlock>,
    free_list: Vec<AllocBlock>,
    pub next_free: u32,
    limit: u32,
    /// The highest address ever handed out: memory below it may hold what
    /// an earlier allocation left (`next_free` drops when the top block is
    /// freed).
    high_water: u32,
}

impl MemoryManager {
    pub fn new(base: u32, limit: u32) -> Self {
        MemoryManager {
            allocated: Vec::new(),
            free_list: Vec::new(),
            next_free: base,
            limit,
            high_water: base,
        }
    }

    pub fn alloc(&mut self, size: u32) -> Option<u32> {
        self.alloc_reporting(size).map(|(addr, _)| addr)
    }

    /// [`Self::alloc`], also telling the bytes that may hold what an
    /// earlier user left: the whole block when it was freed before
    /// (memory never handed out is still zero); 0 otherwise.
    pub fn alloc_reporting(&mut self, size: u32) -> Option<(u32, u32)> {
        let size = (size.checked_add(4095)?) & !4095;
        // First, try to reuse a freed block (first-fit)
        if let Some(idx) = self.free_list.iter().position(|b| b._size >= size) {
            let block = self.free_list.remove(idx);
            let addr = block.addr;
            // If the freed block is larger, split it and return the remainder to the free list
            if block._size > size {
                self.free_list.push(AllocBlock { addr: addr + size, _size: block._size - size });
            }
            self.allocated.push(AllocBlock { addr, _size: size });
            return Some((addr, size));
        }
        // Otherwise bump-allocate
        let end = self.next_free.checked_add(size)?;
        if end > self.limit {
            return None;
        }
        let addr = self.next_free;
        self.allocated.push(AllocBlock { addr, _size: size });
        self.next_free = end;
        let stale = self.high_water.saturating_sub(addr).min(size);
        self.high_water = self.high_water.max(end);
        Some((addr, stale))
    }

    pub fn free(&mut self, addr: u32) -> bool {
        self.free_block(addr).is_some()
    }

    /// Free the block at `addr`; its size (rounded to pages) when it was one.
    pub fn free_block(&mut self, addr: u32) -> Option<u32> {
        if let Some(idx) = self.allocated.iter().position(|b| b.addr == addr) {
            let block = self.allocated.remove(idx);
            let size = block._size;
            self.free_list.push(block);
            // Coalesce: if the top free block is at next_free boundary, reclaim it
            self.free_list.sort_by_key(|b| b.addr);
            while let Some(last) = self.free_list.last() {
                if last.addr + last._size == self.next_free {
                    self.next_free = last.addr;
                    self.free_list.pop();
                } else {
                    break;
                }
            }
            Some(size)
        } else {
            None
        }
    }
}

/// What a pipe-table handle leads to.
#[derive(Clone)]
pub enum PipeObject {
    /// An anonymous pipe end (`DosCreatePipe`): a host pipe.
    Host(Arc<File>),
    /// The server end of a named-pipe instance (`DosCreateNPipe`).
    Server(Arc<super::npipe::ServerInstance>),
    /// The client end of a named pipe (`DosOpen("\\PIPE\\…")`).
    Client(Arc<super::npipe::ClientEnd>),
}

/// One pipe-table handle: the pipe end and the handle's `DosQueryFHState`
/// flags (access mode, `OPEN_FLAGS_NOINHERIT` …).
#[derive(Clone)]
pub struct PipeHandle {
    pub obj: PipeObject,
    pub fh_state: u32,
}

impl PipeHandle {
    /// Whether a child process started by `DosExecPgm` inherits the handle.
    pub fn inheritable(&self) -> bool {
        self.fh_state & super::constants::OPEN_FLAGS_NOINHERIT == 0
    }
}

/// The pipe table: pipe ends by handle number. The numbers come from the
/// process's one handle space (`DriveManager::reserve_pipe_handle`, via
/// `Loader::add_pipe_handle`).
pub struct HandleManager {
    handles: HashMap<u32, PipeHandle>,
}

/// Handle numbers from here on are never files (`DosDupHandle` refuses to
/// put a file there); pipes take numbers from the shared handle space.
pub const PIPE_HANDLE_BASE: u32 = 0x1000;

impl Default for HandleManager {
    fn default() -> Self { Self::new() }
}

impl HandleManager {
    pub fn new() -> Self {
        HandleManager { handles: HashMap::new() }
    }

    pub fn entry(&self, h: u32) -> Option<&PipeHandle> {
        self.handles.get(&h)
    }

    pub fn entry_mut(&mut self, h: u32) -> Option<&mut PipeHandle> {
        self.handles.get_mut(&h)
    }

    /// The host pipe of an anonymous-pipe handle (shared, so a blocking
    /// read or write runs without the manager's lock).
    pub fn host_file(&self, h: u32) -> Option<Arc<File>> {
        match self.handles.get(&h).map(|e| &e.obj) {
            Some(PipeObject::Host(f)) => Some(f.clone()),
            _ => None,
        }
    }

    /// Whether `h` is open in the pipe table.
    pub fn contains(&self, h: u32) -> bool {
        self.handles.contains_key(&h)
    }

    /// Put `entry` at handle number `h` (what `DosDupHandle` does).
    pub fn replace(&mut self, h: u32, entry: PipeHandle) {
        self.handles.insert(h, entry);
    }

    pub fn close(&mut self, h: u32) -> bool {
        self.handles.remove(&h).is_some()
    }

    /// Every open handle, for passing to a child process.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &PipeHandle)> {
        self.handles.iter().map(|(h, e)| (*h, e))
    }

    pub fn flush_all(&mut self) {
        use std::io::Write;
        for entry in self.handles.values() {
            if let PipeObject::Host(f) = &entry.obj {
                let _ = (&**f).flush();
            }
        }
    }
}

pub struct HDirEntry {
    pub iterator: ReadDir,
    pub pattern: String,
}

pub struct HDirManager {
    iterators: HashMap<u32, HDirEntry>,
    next_handle: u32,
}

impl Default for HDirManager {
    fn default() -> Self { Self::new() }
}

impl HDirManager {
    pub fn new() -> Self {
        HDirManager {
            iterators: HashMap::new(),
            next_handle: 10,
        }
    }

    pub fn add(&mut self, it: ReadDir, pattern: String) -> u32 {
        let h = self.next_handle;
        self.iterators.insert(h, HDirEntry { iterator: it, pattern });
        self.next_handle += 1;
        h
    }

    pub fn get_mut(&mut self, h: u32) -> Option<&mut HDirEntry> {
        self.iterators.get_mut(&h)
    }

    pub fn close(&mut self, h: u32) -> bool {
        self.iterators.remove(&h).is_some()
    }
}

pub struct ProcessManager {
    pub current_disk: u8,     // 1=A, 2=B, 3=C (default)
    pub children: HashMap<u32, std::process::Child>,
    /// Background relay threads that forward child stdout to the parent VioManager.
    /// Keyed by the same PID used in `children`.  Must be joined after the child
    /// exits so no output is lost and the thread is properly reaped.
    relay_threads: HashMap<u32, std::thread::JoinHandle<()>>,
    next_pid: u32,
}

impl Default for ProcessManager {
    fn default() -> Self { Self::new() }
}

impl ProcessManager {
    pub fn new() -> Self {
        ProcessManager {
            current_disk: 3, // C:
            children: HashMap::new(),
            relay_threads: HashMap::new(),
            next_pid: super::constants::FIRST_CHILD_PROCESS_ID,
        }
    }

    /// Track a child process under a new id of this process's own
    /// numbering, returning it.
    pub fn add_child(&mut self, child: std::process::Child) -> u32 {
        let pid = self.alloc_pid(&[]);
        self.children.insert(pid, child);
        pid
    }

    /// Track child process `pid` (the id it was started with).
    pub fn insert_child(&mut self, pid: u32, child: std::process::Child) {
        self.children.insert(pid, child);
    }

    /// The next id of this process's own numbering that is none of `avoid`
    /// and no child's (the fallback when the session cannot number
    /// processes, `inherit.rs`).
    pub fn alloc_pid(&mut self, avoid: &[u32]) -> u32 {
        loop {
            let pid = self.next_pid;
            self.next_pid = super::inherit::next_process_id(pid);
            if !avoid.contains(&pid) && !self.children.contains_key(&pid) {
                return pid;
            }
        }
    }

    /// Remove and return a child process by PID.
    pub fn take_child(&mut self, pid: u32) -> Option<std::process::Child> {
        self.children.remove(&pid)
    }

    /// Store the relay thread handle for a child (keyed by PID).
    pub fn add_relay_thread(&mut self, pid: u32, handle: std::thread::JoinHandle<()>) {
        self.relay_threads.insert(pid, handle);
    }

    /// Remove and return the relay thread handle for a child (if any).
    pub fn take_relay_thread(&mut self, pid: u32) -> Option<std::thread::JoinHandle<()>> {
        self.relay_threads.remove(&pid)
    }

    /// Try to wait on any child (for DosWaitChild with DCWA_PROCESSTREE).
    /// Returns (pid, exit_code) for the first finished child found.
    /// A child that ended: its OS/2 pid, host pid and status (its exit
    /// code comes from `SharedState::child_exit_code`).
    pub fn wait_any(&mut self) -> Option<(u32, u32, std::process::ExitStatus)> {
        let mut finished = None;
        for (&pid, child) in self.children.iter_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                finished = Some((pid, child.id(), status));
                break;
            }
        }
        let (pid, host, status) = finished?;
        self.children.remove(&pid);
        Some((pid, host, status))
    }
}

/// A block from `DosAllocSharedMem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedBlock {
    pub addr: u32,
    pub size: u32,
    /// The allocation flags (`PAG_*`, `OBJ_GETTABLE`, `OBJ_GIVEABLE` …).
    pub flags: u32,
    /// `\\SHAREMEM\\…` name (upper case), or none.
    pub name: Option<String>,
}

/// The shared memory blocks of the process, by address and by name.
pub struct SharedMemManager {
    blocks: HashMap<u32, SharedBlock>,
    named_blocks: HashMap<String, u32>,
}

impl Default for SharedMemManager {
    fn default() -> Self { Self::new() }
}

impl SharedMemManager {
    pub fn new() -> Self {
        SharedMemManager { blocks: HashMap::new(), named_blocks: HashMap::new() }
    }

    /// Record a block; a name is kept in upper case (names are not case
    /// sensitive).
    pub fn register(&mut self, mut block: SharedBlock) {
        if let Some(name) = &mut block.name {
            *name = name.to_ascii_uppercase();
            self.named_blocks.insert(name.clone(), block.addr);
        }
        self.blocks.insert(block.addr, block);
    }

    pub fn find_by_name(&self, name: &str) -> Option<u32> {
        self.named_blocks.get(&name.to_ascii_uppercase()).copied()
    }

    /// The block allocated at `addr`.
    pub fn block(&self, addr: u32) -> Option<&SharedBlock> {
        self.blocks.get(&addr)
    }

    /// Forget the block at `addr` (`DosFreeMem`); its name is free again.
    pub fn remove(&mut self, addr: u32) -> Option<SharedBlock> {
        let block = self.blocks.remove(&addr)?;
        if let Some(name) = &block.name {
            self.named_blocks.remove(name);
        }
        Some(block)
    }
}

/// Module handle that stands for the executable in resource calls
/// (`NULLHANDLE` in `GpiLoadBitmap`, `WinLoadString`, `DosGetResource` …).
pub const EXE_RESOURCE_MODULE: u32 = 0;

/// The resources of the executable and of the loaded DLLs.
pub struct ResourceManager {
    // (module handle, type_id, name_id) → (guest_addr, size); the
    // executable is `EXE_RESOURCE_MODULE`, a DLL its module handle.
    resources: HashMap<(u32, u16, u16), (u32, u32)>,
}

impl Default for ResourceManager {
    fn default() -> Self { Self::new() }
}

impl ResourceManager {
    pub fn new() -> Self {
        ResourceManager { resources: HashMap::new() }
    }

    pub fn add(&mut self, hmod: u32, type_id: u16, name_id: u16, guest_addr: u32, size: u32) {
        self.resources.insert((hmod, type_id, name_id), (guest_addr, size));
    }

    /// The resource `type_id` / `name_id` of module `hmod`
    /// (`EXE_RESOURCE_MODULE` for the executable).
    pub fn find(&self, hmod: u32, type_id: u16, name_id: u16) -> Option<(u32, u32)> {
        self.resources.get(&(hmod, type_id, name_id)).copied()
    }

    /// Forget the resources of an unloaded module.
    pub fn remove_module(&mut self, hmod: u32) {
        self.resources.retain(|&(m, _, _), _| m != hmod);
    }
}

/// A DLL module that has been loaded into guest memory.
pub struct LoadedDll {
    /// Module name in UPPERCASE (e.g., "JPOS2DLL")
    pub name: String,
    /// HMODULE handle (opaque u32)
    pub handle: u32,
    /// Guest flat address of each LX object (index = object_num - 1)
    pub object_bases: Vec<u32>,
    /// The size of each object (DosQueryModFromEIP); empty for built-in
    /// modules.
    pub object_sizes: Vec<u32>,
    /// ordinal → guest flat address
    pub exports_by_ordinal: HashMap<u32, u32>,
    /// UPPERCASE name → guest flat address
    pub exports_by_name: HashMap<String, u32>,
    /// Reference count — starts at 1 on first load; DosFreeModule decrements it;
    /// the DLL is unloaded when it reaches 0.
    pub ref_count: u32,
    /// Guest flat address of the DLL's `_DLL_InitTerm` entry point, if present
    /// (LX header eip_object != 0).  `None` for DLLs without an init routine.
    pub initterm_addr: Option<u32>,
    /// True for warpine builtin modules (UCONV, MDM, VIOCALLS, etc.) that have
    /// no real DLL file — DosQueryProcAddr resolves ordinals to thunk stubs.
    pub is_builtin: bool,
}

/// Tracks all user DLLs loaded into guest memory.
pub struct DllManager {
    dlls: Vec<LoadedDll>,
    next_handle: u32,
    /// OS/2 path of each loaded (non-built-in) module, by handle.
    paths: HashMap<u32, String>,
    /// The host file each loaded (non-built-in) module came from, by
    /// handle: the GDB stub finds its debug companion beside it (ADR-0104).
    host_files: HashMap<u32, std::path::PathBuf>,
    /// Modules loaded whose `_DLL_InitTerm(hmod, 0)` has not run yet, in
    /// load order (a module after the ones it imports): (handle, entry).
    pending_init: Vec<(u32, u32)>,
    /// Pages of modules whose `_DLL_InitTerm(hmod, 1)` runs before they
    /// are freed (`WinDeleteLibrary`, ADR-0086), by handle.
    pub pending_free: HashMap<u32, Vec<u32>>,
    /// The program's objects (base, size) and module name
    /// (DosQueryModFromEIP).
    pub exe_objects: Vec<(u32, u32)>,
    pub exe_name: String,
}

impl Default for DllManager {
    fn default() -> Self { Self::new() }
}

impl DllManager {
    pub fn new() -> Self {
        DllManager { dlls: Vec::new(), next_handle: 0x1000, paths: HashMap::new(), host_files: HashMap::new(), pending_init: Vec::new(), pending_free: HashMap::new(),
                     exe_objects: Vec::new(), exe_name: String::new() }
    }

    /// The loaded module whose object holds `address`: its handle, the
    /// object (from 0), the offset in it and the module's name.
    pub fn find_object(&self, address: u32) -> Option<(u32, u32, u32, String)> {
        self.dlls.iter().find_map(|d| {
            d.object_bases.iter().zip(&d.object_sizes).enumerate()
                .find(|&(_, (&base, &size))| address >= base && address - base < size)
                .map(|(i, (&base, _))| (d.handle, i as u32, address - base, d.name.clone()))
        })
    }

    /// Module `handle` was loaded: its `_DLL_InitTerm` at `entry` is to run.
    pub fn add_pending_init(&mut self, handle: u32, entry: u32) {
        self.pending_init.push((handle, entry));
    }

    /// Whether a module's `_DLL_InitTerm` is still to run.
    pub fn has_pending_init(&self) -> bool {
        !self.pending_init.is_empty()
    }

    /// The modules whose `_DLL_InitTerm` is to run now, in order.
    pub fn take_pending_init(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.pending_init)
    }

    /// Allocate a new HMODULE handle value.
    pub fn alloc_handle(&mut self) -> u32 {
        let h = self.next_handle;
        self.next_handle += 1;
        h
    }

    /// Register a loaded DLL and return its handle.
    pub fn register(&mut self, dll: LoadedDll) -> u32 {
        let h = dll.handle;
        self.dlls.push(dll);
        h
    }

    /// Find a loaded DLL by module name (case-insensitive).
    pub fn find_by_name(&self, name: &str) -> Option<&LoadedDll> {
        let upper = name.to_ascii_uppercase();
        self.dlls.iter().find(|d| d.name == upper)
    }

    /// Record the OS/2 path (`DosQueryModuleName`) of loaded module `handle`.
    pub fn set_module_path(&mut self, handle: u32, path: String) {
        self.paths.insert(handle, path);
    }

    /// Record the host file loaded module `handle` came from.
    pub fn set_host_file(&mut self, handle: u32, file: std::path::PathBuf) {
        self.host_files.insert(handle, file);
    }

    /// The loaded modules that came from a host file, in load order: the
    /// file and the address of each of its objects (the GDB stub's library
    /// list, ADR-0104).
    pub fn loaded_files(&self) -> Vec<(std::path::PathBuf, Vec<u32>)> {
        self.dlls.iter()
            .filter(|d| !d.is_builtin)
            .filter_map(|d| self.host_files.get(&d.handle).map(|f| (f.clone(), d.object_bases.clone())))
            .collect()
    }

    /// The OS/2 path recorded for module `handle`.
    pub fn module_path(&self, handle: u32) -> Option<&str> {
        self.paths.get(&handle).map(String::as_str)
    }

    /// Find a loaded DLL by HMODULE handle.
    pub fn find_by_handle(&self, handle: u32) -> Option<&LoadedDll> {
        self.dlls.iter().find(|d| d.handle == handle)
    }

    /// Resolve an import by module name + ordinal → guest flat address.
    pub fn resolve_ordinal(&self, module: &str, ordinal: u32) -> Option<u32> {
        self.find_by_name(module)?.exports_by_ordinal.get(&ordinal).copied()
    }

    /// Resolve an import by module name + export name → guest flat address.
    pub fn resolve_name(&self, module: &str, name: &str) -> Option<u32> {
        let upper = name.to_ascii_uppercase();
        self.find_by_name(module)?.exports_by_name.get(&upper).copied()
    }

    /// Increment the reference count of the DLL identified by `handle`.
    /// Returns `true` if the handle was found, `false` otherwise.
    pub fn increment_refcount(&mut self, handle: u32) -> bool {
        if let Some(dll) = self.dlls.iter_mut().find(|d| d.handle == handle) {
            dll.ref_count += 1;
            true
        } else {
            false
        }
    }

    /// Decrement the reference count of the DLL identified by `handle`.
    ///
    /// Returns `Some((object_bases, initterm_addr))` if the count reached zero
    /// (caller must free those guest memory blocks, optionally after calling
    /// the INITTERM unload routine), or `None` if the DLL is still referenced.
    pub fn decrement_refcount(&mut self, handle: u32) -> Option<(Vec<u32>, Option<u32>)> {
        let idx = self.dlls.iter().position(|d| d.handle == handle)?;
        if self.dlls[idx].ref_count > 0 {
            self.dlls[idx].ref_count -= 1;
        }
        if self.dlls[idx].ref_count == 0 {
            let bases = self.dlls[idx].object_bases.clone();
            let initterm_addr = self.dlls[idx].initterm_addr;
            self.dlls.remove(idx);
            self.paths.remove(&handle);
            self.host_files.remove(&handle);
            Some((bases, initterm_addr))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dll_inits_wait_in_load_order_until_taken() {
        let mut m = DllManager::new();
        assert!(!m.has_pending_init());
        m.add_pending_init(0x1001, 0x5000);
        m.add_pending_init(0x1002, 0x6000);
        assert!(m.has_pending_init());
        assert_eq!(m.take_pending_init(), vec![(0x1001, 0x5000), (0x1002, 0x6000)]);
        assert!(!m.has_pending_init());
    }

    fn user_dll(name: &str, handle: u32, bases: Vec<u32>, builtin: bool) -> LoadedDll {
        LoadedDll { name: name.into(), handle, object_bases: bases, object_sizes: Vec::new(),
                    exports_by_ordinal: HashMap::new(), exports_by_name: HashMap::new(),
                    ref_count: 1, initterm_addr: None, is_builtin: builtin }
    }

    #[test]
    fn loaded_files_list_user_modules_with_their_objects_until_freed() {
        let mut m = DllManager::new();
        m.register(user_dll("A", 0x1001, vec![0x0200_0000, 0x0201_0000], false));
        m.register(user_dll("UCONV", 0x1002, vec![], true));
        m.register(user_dll("B", 0x1003, vec![0x0203_0000], false));
        m.set_host_file(0x1001, "/x/a.dll".into());
        m.set_host_file(0x1003, "/x/b.dll".into());
        assert_eq!(m.loaded_files(), vec![("/x/a.dll".into(), vec![0x0200_0000, 0x0201_0000]),
                                          ("/x/b.dll".into(), vec![0x0203_0000])]);
        assert!(m.decrement_refcount(0x1001).is_some());
        assert_eq!(m.loaded_files(), vec![(std::path::PathBuf::from("/x/b.dll"), vec![0x0203_0000])]);
    }

    use super::*;

    #[test]
    fn test_alloc_basic() {
        let mut mgr = MemoryManager::new(0x1000, 0x10000);
        let a = mgr.alloc(100).unwrap();
        assert_eq!(a, 0x1000);
        // 100 bytes rounds up to 4096
        let b = mgr.alloc(100).unwrap();
        assert_eq!(b, 0x2000);
    }

    #[test]
    fn test_alloc_free_reuse() {
        let mut mgr = MemoryManager::new(0x1000, 0x10000);
        let a = mgr.alloc(4096).unwrap();
        let _b = mgr.alloc(4096).unwrap();
        mgr.free(a);
        // Should reuse the freed block
        let c = mgr.alloc(4096).unwrap();
        assert_eq!(c, a);
    }

    #[test]
    fn test_alloc_free_coalesce_top() {
        let mut mgr = MemoryManager::new(0x1000, 0x10000);
        let a = mgr.alloc(4096).unwrap();
        assert_eq!(mgr.next_free, 0x2000);
        mgr.free(a);
        // After freeing the top block, next_free should be reclaimed
        assert_eq!(mgr.next_free, 0x1000);
    }

    #[test]
    fn test_alloc_overflow() {
        let mut mgr = MemoryManager::new(0xFFFFF000, 0xFFFFFFFF);
        // Requesting a size that would overflow u32 when adding 4095
        assert!(mgr.alloc(0xFFFFF000).is_none());
    }

    #[test]
    fn test_alloc_exceeds_limit() {
        let mut mgr = MemoryManager::new(0x1000, 0x3000);
        let _a = mgr.alloc(4096).unwrap();
        // Only room for one more page
        let _b = mgr.alloc(4096).unwrap();
        assert!(mgr.alloc(4096).is_none());
    }

    #[test]
    fn test_free_nonexistent() {
        let mut mgr = MemoryManager::new(0x1000, 0x10000);
        assert!(!mgr.free(0x9999));
    }

    #[test]
    fn test_resource_manager_find() {
        let mut mgr = ResourceManager::new();
        let exe = EXE_RESOURCE_MODULE;
        mgr.add(exe, 6, 1, 0x10000, 256);
        mgr.add(exe, 4, 100, 0x20000, 512);

        assert_eq!(mgr.find(exe, 6, 1), Some((0x10000, 256)));
        assert_eq!(mgr.find(exe, 4, 100), Some((0x20000, 512)));
        assert_eq!(mgr.find(exe, 6, 2), None);
        assert_eq!(mgr.find(exe, 99, 1), None);
    }

    /// Each module has its own resources: the same type and id in a DLL
    /// and in the executable are different resources, and unloading a DLL
    /// forgets only its own.
    #[test]
    fn test_resource_manager_keys_resources_by_module() {
        let mut mgr = ResourceManager::new();
        let (exe, dll) = (EXE_RESOURCE_MODULE, 7);
        mgr.add(exe, 2, 1, 0x10000, 10);
        mgr.add(dll, 2, 1, 0x30000, 20);
        mgr.add(dll, 5, 3, 0x31000, 30);
        assert_eq!(mgr.find(exe, 2, 1), Some((0x10000, 10)));
        assert_eq!(mgr.find(dll, 2, 1), Some((0x30000, 20)));
        assert_eq!(mgr.find(exe, 5, 3), None, "the DLL's resource is not the executable's");
        assert_eq!(mgr.find(8, 2, 1), None, "unknown module");
        mgr.remove_module(dll);
        assert_eq!(mgr.find(dll, 2, 1), None);
        assert_eq!(mgr.find(dll, 5, 3), None);
        assert_eq!(mgr.find(exe, 2, 1), Some((0x10000, 10)), "the executable's stay");
    }

    #[test]
    fn test_process_manager_defaults() {
        let mgr = ProcessManager::new();
        assert_eq!(mgr.current_disk, 3); // C:
    }

    #[test]
    fn test_shared_mem_manager() {
        let mut mgr = SharedMemManager::new();
        mgr.register(SharedBlock { addr: 0x5000, size: 4096, flags: 0x13,
            name: Some("\\SHAREMEM\\Test".to_string()) });
        assert_eq!(mgr.find_by_name("\\SHAREMEM\\TEST"), Some(0x5000), "names ignore case");
        assert_eq!(mgr.find_by_name("\\SHAREMEM\\OTHER"), None);
        assert_eq!(mgr.block(0x5000).map(|b| b.size), Some(4096));
        // Freeing the block frees its name.
        assert!(mgr.remove(0x5000).is_some());
        assert_eq!(mgr.find_by_name("\\SHAREMEM\\TEST"), None);
        assert!(mgr.remove(0x5000).is_none());
    }

    /// The pipe table: entries at chosen numbers, the inheritance flag.
    #[test]
    fn test_handle_manager_pipe_table() {
        let mut mgr = HandleManager::new();
        let (r, w) = std::io::pipe().unwrap();
        let host = |f: std::io::PipeReader| PipeHandle {
            obj: PipeObject::Host(Arc::new(File::from(std::os::fd::OwnedFd::from(f)))), fh_state: 0 };
        mgr.replace(3, host(r));
        mgr.replace(4, PipeHandle { obj: PipeObject::Host(Arc::new(File::from(std::os::fd::OwnedFd::from(w)))),
                                    fh_state: 1 });
        assert!(mgr.host_file(3).is_some());
        let entry = mgr.entry(4).unwrap().clone();
        assert!(entry.inheritable());
        mgr.replace(1, entry);
        assert!(mgr.contains(1));
        mgr.entry_mut(1).unwrap().fh_state |= crate::loader::constants::OPEN_FLAGS_NOINHERIT;
        assert!(!mgr.entry(1).unwrap().inheritable());
        assert!(mgr.entry(4).unwrap().inheritable(), "the duplicate has its own state");
        assert!(mgr.close(3));
        assert!(!mgr.close(3));
        assert_eq!(mgr.iter().count(), 2);
    }

    fn make_dll(handle: u32, name: &str, bases: Vec<u32>) -> LoadedDll {
        LoadedDll {
            name: name.to_string(),
            handle,
            object_bases: bases,
            object_sizes: Vec::new(),
            exports_by_ordinal: HashMap::new(),
            exports_by_name: HashMap::new(),
            ref_count: 1,
            initterm_addr: None,
            is_builtin: false,
        }
    }

    #[test]
    fn test_dll_manager_refcount_increment() {
        let mut mgr = DllManager::new();
        let h = mgr.alloc_handle();
        mgr.register(make_dll(h, "TESTDLL", vec![0x1000]));
        assert!(mgr.increment_refcount(h));
        assert_eq!(mgr.find_by_handle(h).map(|d| d.ref_count), Some(2));
        assert!(!mgr.increment_refcount(0xDEAD)); // unknown handle
    }

    #[test]
    fn test_dll_manager_refcount_decrement_stays() {
        let mut mgr = DllManager::new();
        let h = mgr.alloc_handle();
        mgr.register(make_dll(h, "TESTDLL", vec![0x1000]));
        mgr.increment_refcount(h); // ref_count = 2
        let result = mgr.decrement_refcount(h); // ref_count = 1 → still loaded
        assert!(result.is_none(), "should not unload at refcount 1");
        assert!(mgr.find_by_handle(h).is_some());
    }

    #[test]
    fn test_dll_manager_refcount_decrement_unloads_no_initterm() {
        let mut mgr = DllManager::new();
        let h = mgr.alloc_handle();
        mgr.register(make_dll(h, "TESTDLL", vec![0x2000, 0x3000]));
        let freed = mgr.decrement_refcount(h); // ref_count 1 → 0
        assert_eq!(freed, Some((vec![0x2000, 0x3000], None)));
        assert!(mgr.find_by_handle(h).is_none(), "DLL should be removed");
    }

    #[test]
    fn test_dll_manager_refcount_decrement_unloads_with_initterm() {
        let mut mgr = DllManager::new();
        let h = mgr.alloc_handle();
        let mut dll = make_dll(h, "TESTDLL", vec![0x4000]);
        dll.initterm_addr = Some(0x4100);
        mgr.register(dll);
        let freed = mgr.decrement_refcount(h);
        assert_eq!(freed, Some((vec![0x4000], Some(0x4100))));
        assert!(mgr.find_by_handle(h).is_none());
    }

    #[test]
    fn test_dll_manager_decrement_unknown_handle() {
        let mut mgr = DllManager::new();
        assert!(mgr.decrement_refcount(0xBEEF).is_none());
    }
}
