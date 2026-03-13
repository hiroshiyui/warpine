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

impl HandleManager {
    pub fn new() -> Self {
        HandleManager {
            handles: HashMap::new(),
            next_handle: 3,
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

    pub fn close(&mut self, h: u32) -> bool {
        self.handles.remove(&h).is_some()
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

pub struct ResourceManager {
    // (type_id, name_id) → (guest_addr, size)
    resources: HashMap<(u16, u16), (u32, u32)>,
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
}
