// SPDX-License-Identifier: GPL-3.0-only
//
// Guest physical memory — sole access point for reads and writes into the
// KVM guest address space.  All operations are bounds-checked.  No other
// module may hold a raw `*mut u8` pointer into guest memory or perform direct
// pointer arithmetic on it outside of this file.

use std::ptr;
use log::warn;

// ── GuestMemory ───────────────────────────────────────────────────────────────

/// Owns the `mmap`-backed host memory region that backs the KVM guest physical
/// address space.
///
/// Every read and write into guest memory **must** go through the methods on
/// this type; direct pointer arithmetic outside this file is not permitted.
/// This is the primary enforcement point for the hypervisor-escape prevention
/// rule: the guest can never cause the host to access memory outside this
/// allocation because every access is bounds-checked before dereferencing.
///
/// # Safety
///
/// `Send + Sync` is safe because:
/// - All public methods perform bounds checking before dereferencing.
/// - The `*mut u8` field is never exposed to callers.
/// - The allocation is exclusively owned by this type; no other object holds a
///   live pointer to the same region (the KVM kernel side accesses it via the
///   mapped physical region, which is controlled by the hypervisor, not a raw
///   pointer visible to safe Rust code).
pub struct GuestMemory {
    ptr:  *mut u8,
    size: usize,
}

unsafe impl Send for GuestMemory {}
unsafe impl Sync for GuestMemory {}

impl GuestMemory {
    /// Allocate `size` bytes of anonymous, zero-initialised memory for use as
    /// the KVM guest physical address space.
    ///
    /// Panics if `mmap` fails (unrecoverable — no guest memory → no execution).
    pub fn alloc(size: usize) -> Self {
        let raw = unsafe {
            libc::mmap(
                ptr::null_mut(), size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1, 0,
            )
        };
        if raw == libc::MAP_FAILED {
            panic!(
                "Failed to mmap {} bytes for guest memory: {}",
                size,
                std::io::Error::last_os_error()
            );
        }
        let p = raw as *mut u8;
        unsafe { ptr::write_bytes(p, 0, size); }
        GuestMemory { ptr: p, size }
    }

    /// Total size of the guest memory region in bytes.
    pub fn size(&self) -> usize { self.size }

    /// Host virtual address of the base of the region.
    ///
    /// This value is passed to KVM (`KVM_SET_USER_MEMORY_REGION`) so the
    /// hypervisor knows where the guest physical pages live in host VA space.
    /// It must not be used to derive pointers for direct access — use
    /// [`ptr_at`] instead.
    pub fn host_base_addr(&self) -> u64 { self.ptr as u64 }

    /// Returns a checked raw pointer at `offset` for an access of `len` bytes,
    /// or `None` if `offset..offset+len` would exceed the allocation bounds.
    pub fn ptr_at(&self, offset: u32, len: usize) -> Option<*mut u8> {
        let off = offset as usize;
        if off.checked_add(len).map_or(true, |end| end > self.size) {
            return None;
        }
        Some(unsafe { self.ptr.add(off) })
    }

    /// Read a `Copy` value from guest memory at `offset`.
    /// Returns `None` if the access would be out of bounds.
    pub fn read<T: Copy>(&self, offset: u32) -> Option<T> {
        let p = self.ptr_at(offset, std::mem::size_of::<T>())?;
        Some(unsafe { ptr::read_unaligned(p as *const T) })
    }

    /// Write a `Copy` value to guest memory at `offset`.
    /// Returns `None` if the access would be out of bounds.
    pub fn write<T: Copy>(&self, offset: u32, val: T) -> Option<()> {
        let p = self.ptr_at(offset, std::mem::size_of::<T>())?;
        unsafe { ptr::write_unaligned(p as *mut T, val); }
        Some(())
    }

    /// Copy `data` into guest memory starting at `offset`.
    /// Returns `None` if the write would be out of bounds.
    pub fn write_bytes(&self, offset: u32, data: &[u8]) -> Option<()> {
        let p = self.ptr_at(offset, data.len())?;
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), p, data.len()); }
        Some(())
    }

    /// Return a mutable byte slice into guest memory at `offset..offset+len`.
    /// Returns `None` if the range would be out of bounds.
    pub fn slice_mut(&self, offset: u32, len: usize) -> Option<&mut [u8]> {
        let p = self.ptr_at(offset, len)?;
        Some(unsafe { std::slice::from_raw_parts_mut(p, len) })
    }
}

impl Drop for GuestMemory {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.size); }
    }
}

// ── Loader helper methods (thin wrappers over GuestMemory) ───────────────────

impl super::Loader {
    /// Returns a checked raw pointer into guest memory, or `None` if OOB.
    pub fn guest_ptr(&self, offset: u32, len: usize) -> Option<*mut u8> {
        self.shared.guest_mem.ptr_at(offset, len)
    }

    /// Read a value from guest memory with bounds check.
    pub fn guest_read<T: Copy>(&self, offset: u32) -> Option<T> {
        self.shared.guest_mem.read::<T>(offset)
    }

    /// Write a value to guest memory with bounds check.
    pub fn guest_write<T: Copy>(&self, offset: u32, val: T) -> Option<()> {
        self.shared.guest_mem.write::<T>(offset, val)
    }

    /// Copy bytes into guest memory with bounds check.
    pub fn guest_write_bytes(&self, offset: u32, data: &[u8]) -> Option<()> {
        self.shared.guest_mem.write_bytes(offset, data)
    }

    /// Get a mutable slice of guest memory with bounds check.
    pub fn guest_slice_mut(&self, offset: u32, len: usize) -> Option<&mut [u8]> {
        self.shared.guest_mem.slice_mut(offset, len)
    }

    /// Read a null-terminated string from guest memory.
    pub fn read_guest_string(&self, ptr: u32) -> String {
        const MAX_GUEST_STRING_LEN: usize = 4096;
        let mut s = String::new();
        let base = ptr as usize;
        let mem_size = self.shared.guest_mem.size();
        if base >= mem_size { return s; }
        let max_len = MAX_GUEST_STRING_LEN.min(mem_size - base);
        for i in 0..max_len {
            let byte = self.shared.guest_mem.read::<u8>((base + i) as u32).unwrap_or(0);
            if byte == 0 { break; }
            s.push(byte as char);
        }
        s
    }

    /// Translate an OS/2 path to a sandboxed host path.
    /// Prevents path traversal attacks by canonicalizing and checking containment.
    /// Relative paths are resolved against the OS/2 current directory from ProcessManager.
    pub fn translate_path(&self, os2_path: &str) -> Result<std::path::PathBuf, u32> {
        use super::mutex_ext::MutexExt;

        let unix_path = os2_path.replace('\\', "/");
        // Strip drive letter (e.g., "C:" or "D:")
        let stripped = if unix_path.len() >= 2 && unix_path.as_bytes()[1] == b':' {
            &unix_path[2..]
        } else {
            &unix_path
        };

        // If the path is relative (doesn't start with /), prepend the OS/2 current directory
        let resolved_relative = if !stripped.starts_with('/') && !stripped.is_empty() {
            let proc_mgr = self.shared.process_mgr.lock_or_recover();
            let cur_dir = proc_mgr.current_dir.replace('\\', "/");
            let cur_dir = cur_dir.trim_start_matches('/');
            if cur_dir.is_empty() {
                stripped.to_string()
            } else {
                format!("{}/{}", cur_dir, stripped)
            }
        } else {
            stripped.trim_start_matches('/').to_string()
        };

        let relative = resolved_relative.trim_start_matches('/');
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
