// SPDX-License-Identifier: GPL-3.0-only

use std::ptr;
use log::warn;

impl super::Loader {
    // ── Bounds-checked guest memory access helpers ──

    /// Returns a checked raw pointer into guest memory, or None if out of bounds.
    pub fn guest_ptr(&self, offset: u32, len: usize) -> Option<*mut u8> {
        let offset = offset as usize;
        if offset.checked_add(len).map_or(true, |end| end > self.shared.guest_mem_size) {
            return None;
        }
        Some(unsafe { self.shared.guest_mem.add(offset) })
    }

    /// Read a value from guest memory with bounds check.
    pub fn guest_read<T: Copy>(&self, offset: u32) -> Option<T> {
        let ptr = self.guest_ptr(offset, std::mem::size_of::<T>())?;
        Some(unsafe { ptr::read_unaligned(ptr as *const T) })
    }

    /// Write a value to guest memory with bounds check.
    pub fn guest_write<T: Copy>(&self, offset: u32, val: T) -> Option<()> {
        let ptr = self.guest_ptr(offset, std::mem::size_of::<T>())?;
        unsafe { ptr::write_unaligned(ptr as *mut T, val); }
        Some(())
    }

    /// Copy bytes into guest memory with bounds check.
    pub fn guest_write_bytes(&self, offset: u32, data: &[u8]) -> Option<()> {
        let ptr = self.guest_ptr(offset, data.len())?;
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len()); }
        Some(())
    }

    /// Get a mutable slice of guest memory with bounds check.
    pub fn guest_slice_mut(&self, offset: u32, len: usize) -> Option<&mut [u8]> {
        let ptr = self.guest_ptr(offset, len)?;
        Some(unsafe { std::slice::from_raw_parts_mut(ptr, len) })
    }

    /// Read a null-terminated string from guest memory with bounds check and max length.
    pub fn read_guest_string(&self, ptr: u32) -> String {
        const MAX_GUEST_STRING_LEN: usize = 4096;
        let mut s = String::new();
        let base = ptr as usize;
        let mem_size = self.shared.guest_mem_size;
        if base >= mem_size { return s; }
        let max_len = MAX_GUEST_STRING_LEN.min(mem_size - base);
        for i in 0..max_len {
            let byte = self.guest_read::<u8>((base + i) as u32).unwrap_or(0);
            if byte == 0 { break; }
            s.push(byte as char);
        }
        s
    }

    /// Translate an OS/2 path to a sandboxed host path.
    /// Prevents path traversal attacks by canonicalizing and checking containment.
    pub fn translate_path(&self, os2_path: &str) -> Result<std::path::PathBuf, u32> {
        let unix_path = os2_path.replace('\\', "/");
        // Strip drive letter (e.g., "C:" or "D:")
        let stripped = if unix_path.len() >= 2 && unix_path.as_bytes()[1] == b':' {
            &unix_path[2..]
        } else {
            &unix_path
        };
        let relative = stripped.trim_start_matches('/');
        let sandbox_root = std::env::current_dir().map_err(|_| 3u32)?; // ERROR_PATH_NOT_FOUND
        let candidate = sandbox_root.join(relative);
        // Canonicalize to resolve .., symlinks; for new files canonicalize parent
        let resolved = if candidate.exists() {
            candidate.canonicalize().map_err(|_| 3u32)?
        } else {
            let parent = candidate.parent().ok_or(3u32)?;
            if !parent.exists() { return Err(3); }
            let file_name = candidate.file_name().ok_or(3u32)?;
            parent.canonicalize().map_err(|_| 3u32)?.join(file_name)
        };
        // Verify resolved path is under sandbox root
        if !resolved.starts_with(&sandbox_root) {
            warn!("SECURITY: Path traversal blocked: '{}' → '{}'", os2_path, resolved.display());
            return Err(5); // ERROR_ACCESS_DENIED
        }
        Ok(resolved)
    }
}
