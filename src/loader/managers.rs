// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::fs::{File, ReadDir};

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
}

impl MemoryManager {
    pub fn new(base: u32, limit: u32) -> Self {
        MemoryManager {
            allocated: Vec::new(),
            free_list: Vec::new(),
            next_free: base,
            limit,
        }
    }

    pub fn alloc(&mut self, size: u32) -> Option<u32> {
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
            return Some(addr);
        }
        // Otherwise bump-allocate
        let end = self.next_free.checked_add(size)?;
        if end > self.limit {
            return None;
        }
        let addr = self.next_free;
        self.allocated.push(AllocBlock { addr, _size: size });
        self.next_free = end;
        Some(addr)
    }

    pub fn free(&mut self, addr: u32) -> bool {
        if let Some(idx) = self.allocated.iter().position(|b| b.addr == addr) {
            let block = self.allocated.remove(idx);
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
            true
        } else {
            false
        }
    }
}

pub struct HandleManager {
    handles: HashMap<u32, File>,
    next_handle: u32,
}

/// Pipe/legacy handle base — offset to avoid collision with VFS file handles.
/// VFS handles occupy 3..PIPE_HANDLE_BASE-1, pipe handles occupy PIPE_HANDLE_BASE+.
pub const PIPE_HANDLE_BASE: u32 = 0x1000;

impl Default for HandleManager {
    fn default() -> Self { Self::new() }
}

impl HandleManager {
    pub fn new() -> Self {
        HandleManager {
            handles: HashMap::new(),
            next_handle: PIPE_HANDLE_BASE,
        }
    }

    pub fn add(&mut self, file: File) -> u32 {
        let h = self.next_handle;
        self.handles.insert(h, file);
        self.next_handle += 1;
        h
    }

    pub fn get_mut(&mut self, h: u32) -> Option<&mut File> {
        self.handles.get_mut(&h)
    }

    pub fn get(&self, h: u32) -> Option<&File> {
        self.handles.get(&h)
    }

    pub fn insert(&mut self, file: File) -> u32 {
        self.add(file)
    }

    pub fn replace(&mut self, h: u32, file: File) {
        self.handles.insert(h, file);
    }

    pub fn close(&mut self, h: u32) -> bool {
        self.handles.remove(&h).is_some()
    }

    pub fn flush_all(&mut self) {
        use std::io::Write;
        for file in self.handles.values_mut() {
            let _ = file.flush();
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
    pub current_dir: String,  // OS/2-style current directory (without drive letter), e.g. "\"
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
            current_dir: String::from("\\"),
            children: HashMap::new(),
            relay_threads: HashMap::new(),
            next_pid: 100,
        }
    }

    /// Get the OS/2 current directory path without the leading backslash.
    /// OS/2 DosQueryCurrentDir returns the path without the drive letter or leading backslash.
    pub fn current_dir_no_leading_slash(&self) -> &str {
        self.current_dir.trim_start_matches('\\')
    }

    /// Track a child process, returning its assigned PID.
    pub fn add_child(&mut self, child: std::process::Child) -> u32 {
        let pid = self.next_pid;
        self.children.insert(pid, child);
        self.next_pid += 1;
        pid
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
    pub fn wait_any(&mut self) -> Option<(u32, i32)> {
        let mut finished = None;
        for (&pid, child) in self.children.iter_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                finished = Some((pid, status.code().unwrap_or(1)));
                break;
            }
        }
        if let Some((pid, code)) = finished {
            self.children.remove(&pid);
            Some((pid, code))
        } else {
            None
        }
    }
}

pub struct SharedMemManager {
    named_blocks: HashMap<String, u32>,
}

impl Default for SharedMemManager {
    fn default() -> Self { Self::new() }
}

impl SharedMemManager {
    pub fn new() -> Self {
        SharedMemManager { named_blocks: HashMap::new() }
    }

    pub fn register(&mut self, name: String, addr: u32) {
        self.named_blocks.insert(name, addr);
    }

    pub fn find_by_name(&self, name: &str) -> Option<u32> {
        self.named_blocks.get(name).copied()
    }
}

pub struct ResourceManager {
    // (type_id, name_id) → (guest_addr, size)
    resources: HashMap<(u16, u16), (u32, u32)>,
}

impl Default for ResourceManager {
    fn default() -> Self { Self::new() }
}

impl ResourceManager {
    pub fn new() -> Self {
        ResourceManager { resources: HashMap::new() }
    }

    pub fn add(&mut self, type_id: u16, name_id: u16, guest_addr: u32, size: u32) {
        self.resources.insert((type_id, name_id), (guest_addr, size));
    }

    pub fn find(&self, type_id: u16, name_id: u16) -> Option<(u32, u32)> {
        self.resources.get(&(type_id, name_id)).copied()
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
}

/// Tracks all user DLLs loaded into guest memory.
pub struct DllManager {
    dlls: Vec<LoadedDll>,
    next_handle: u32,
}

impl Default for DllManager {
    fn default() -> Self { Self::new() }
}

impl DllManager {
    pub fn new() -> Self {
        DllManager { dlls: Vec::new(), next_handle: 0x1000 }
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
            Some((bases, initterm_addr))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
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
        mgr.add(6, 1, 0x10000, 256);
        mgr.add(4, 100, 0x20000, 512);

        assert_eq!(mgr.find(6, 1), Some((0x10000, 256)));
        assert_eq!(mgr.find(4, 100), Some((0x20000, 512)));
        assert_eq!(mgr.find(6, 2), None);
        assert_eq!(mgr.find(99, 1), None);
    }

    #[test]
    fn test_process_manager_defaults() {
        let mgr = ProcessManager::new();
        assert_eq!(mgr.current_disk, 3); // C:
        assert_eq!(mgr.current_dir, "\\");
        assert_eq!(mgr.current_dir_no_leading_slash(), "");
    }

    #[test]
    fn test_process_manager_current_dir() {
        let mut mgr = ProcessManager::new();
        mgr.current_dir = String::from("\\TOOLS\\BIN");
        assert_eq!(mgr.current_dir_no_leading_slash(), "TOOLS\\BIN");
    }

    #[test]
    fn test_shared_mem_manager() {
        let mut mgr = SharedMemManager::new();
        mgr.register("\\SHAREMEM\\TEST".to_string(), 0x5000);
        assert_eq!(mgr.find_by_name("\\SHAREMEM\\TEST"), Some(0x5000));
        assert_eq!(mgr.find_by_name("\\SHAREMEM\\OTHER"), None);
    }

    fn make_dll(handle: u32, name: &str, bases: Vec<u32>) -> LoadedDll {
        LoadedDll {
            name: name.to_string(),
            handle,
            object_bases: bases,
            exports_by_ordinal: HashMap::new(),
            exports_by_name: HashMap::new(),
            ref_count: 1,
            initterm_addr: None,
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
