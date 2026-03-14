// SPDX-License-Identifier: GPL-3.0-only
//
// HostDirBackend: implements VfsBackend using a host directory with HPFS semantics.
//
// This is the first (and primary) backend for warpine's virtual filesystem.
// It maps an OS/2 drive to an isolated host directory, providing:
// - Case-insensitive, case-preserving filename lookup
// - Sandbox enforcement (paths cannot escape the volume root)
// - Long filename support (up to 254 characters, HPFS limit)
// - File sharing mode enforcement
// - Proper OS/2 error codes for all failure modes

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use std::ffi::CString;

use log::{debug, warn};

use super::vfs::*;

// ── Helper: DOS date/time conversion ──

/// Convert a SystemTime to OS/2 DOS date and time.
fn systemtime_to_dos(st: SystemTime) -> (u16, u16) {
    use std::time::UNIX_EPOCH;
    let secs = st.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    // Convert Unix timestamp to broken-down time
    // Simple conversion: days since epoch → year/month/day
    let days = (secs / 86400) as i64;
    let time_of_day = (secs % 86400) as u32;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since 1970-01-01 → year/month/day
    // Using a simple algorithm (no leap second handling)
    let (year, month, day) = days_to_ymd(days);

    // DOS date: bits 15-9 = year-1980, bits 8-5 = month, bits 4-0 = day
    let dos_year = (year as i32 - 1980).max(0) as u16;
    let dos_date = (dos_year << 9) | ((month as u16) << 5) | (day as u16);
    // DOS time: bits 15-11 = hour, bits 10-5 = minute, bits 4-0 = seconds/2
    let dos_time = ((hours as u16) << 11) | ((minutes as u16) << 5) | ((seconds as u16) / 2);

    (dos_date, dos_time)
}

fn days_to_ymd(days_since_epoch: i64) -> (i32, u32, u32) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days_since_epoch + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + (era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Build a FileStatus from std::fs::Metadata.
fn metadata_to_file_status(meta: &fs::Metadata) -> FileStatus {
    let (cdate, ctime) = meta.created()
        .map(|t| systemtime_to_dos(t))
        .unwrap_or((0, 0));
    let (adate, atime) = meta.accessed()
        .map(|t| systemtime_to_dos(t))
        .unwrap_or((0, 0));
    let (wdate, wtime) = meta.modified()
        .map(|t| systemtime_to_dos(t))
        .unwrap_or((0, 0));

    let size = meta.len() as u32;
    let mut attrs = 0u32;
    if meta.is_dir() { attrs |= FileAttribute::DIRECTORY.0; }
    if meta.permissions().readonly() { attrs |= FileAttribute::READONLY.0; }
    // Default to ARCHIVE for regular files
    if meta.is_file() { attrs |= FileAttribute::ARCHIVE.0; }

    FileStatus {
        creation_date: cdate,
        creation_time: ctime,
        last_access_date: adate,
        last_access_time: atime,
        last_write_date: wdate,
        last_write_time: wtime,
        file_size: size,
        file_alloc: (size + 511) & !511, // round up to 512-byte sectors
        attributes: FileAttribute(attrs),
    }
}

// ── Case-insensitive path resolution ──

/// Resolve a single path component case-insensitively within a directory.
///
/// Strategy (from WINE's `lookup_unix_name()`):
/// 1. Try exact match with stat() — fast path
/// 2. Fall back to readdir() + case-insensitive comparison
fn resolve_component_case_insensitive(dir: &Path, component: &str) -> Option<String> {
    // Fast path: exact match
    let exact = dir.join(component);
    if exact.exists() {
        return Some(component.to_string());
    }

    // Fallback: scan directory entries case-insensitively
    let entries = fs::read_dir(dir).ok()?;
    let lower = component.to_ascii_lowercase();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.to_ascii_lowercase() == lower {
            return Some(name_str.into_owned());
        }
    }

    None
}

/// Resolve a full relative path case-insensitively, walking from root.
/// Returns the canonical host path if found, or None.
fn resolve_path_case_insensitive(root: &Path, rel_path: &str) -> Option<PathBuf> {
    if rel_path.is_empty() {
        return Some(root.to_path_buf());
    }

    let mut current = root.to_path_buf();
    for component in rel_path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            // Go up, but never above root
            if current != root {
                current = current.parent()?.to_path_buf();
            }
            continue;
        }
        match resolve_component_case_insensitive(&current, component) {
            Some(resolved) => current = current.join(resolved),
            None => return None,
        }
    }
    Some(current)
}

// ── OS/2 wildcard matching (HPFS semantics) ──

/// Match a filename against an OS/2 wildcard pattern (case-insensitive).
///
/// HPFS wildcard rules:
/// - `*` matches any sequence of characters (including dots)
/// - `?` matches any single character
/// - `*.*` matches ALL files (including files without dots) — unlike DOS/FAT
/// - `.` in the pattern only matches `.` in the name (no implicit dot insertion)
/// - Matching is case-insensitive
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();

    // HPFS special case: "*.*" matches everything (including files without dots)
    if p == "*.*" {
        return true;
    }

    let p_chars: Vec<char> = p.chars().collect();
    let n_chars: Vec<char> = n.chars().collect();
    wildcard_match_recursive(&p_chars, &n_chars)
}

fn wildcard_match_recursive(pattern: &[char], name: &[char]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            // '*' matches zero or more characters
            wildcard_match_recursive(&pattern[1..], name) ||
            (!name.is_empty() && wildcard_match_recursive(pattern, &name[1..]))
        }
        (Some('?'), Some(_)) => {
            wildcard_match_recursive(&pattern[1..], &name[1..])
        }
        (Some(pc), Some(nc)) if *pc == *nc => {
            wildcard_match_recursive(&pattern[1..], &name[1..])
        }
        _ => false,
    }
}

// ── Attribute filtering ──

/// Check if a directory entry's attributes pass the OS/2 DosFindFirst filter.
///
/// OS/2 attribute filtering rules:
/// - Normal files (no special attributes) are always included
/// - Hidden, system, and directory entries are only included if the
///   corresponding bit is set in the attribute filter
/// - The filter acts as an "include these types too" mask, not "must have"
fn attributes_match(entry_attrs: FileAttribute, filter: FileAttribute) -> bool {
    // These attribute types are excluded by default — only included if filter requests them
    let gated = FileAttribute::HIDDEN.0 | FileAttribute::SYSTEM.0 | FileAttribute::DIRECTORY.0;
    let entry_gated = entry_attrs.0 & gated;

    // If the entry has any gated attributes, the filter must include them
    if entry_gated != 0 && (entry_gated & filter.0) != entry_gated {
        return false;
    }
    true
}

// ── Sharing mode tracking ──

/// Tracks open files and their sharing modes for violation detection.
struct SharingTable {
    /// Maps canonical host path → list of (VfsFileHandle, OpenMode, SharingMode)
    entries: HashMap<PathBuf, Vec<(u64, OpenMode, SharingMode)>>,
}

impl SharingTable {
    fn new() -> Self {
        SharingTable { entries: HashMap::new() }
    }

    /// Check if a new open request is compatible with existing opens.
    /// Returns Ok(()) if allowed, Err(SHARING_VIOLATION) if not.
    fn check_and_add(
        &mut self,
        path: &Path,
        handle_id: u64,
        mode: OpenMode,
        sharing: SharingMode,
    ) -> VfsResult<()> {
        let existing = self.entries.entry(path.to_path_buf()).or_default();

        for &(_, existing_mode, existing_sharing) in existing.iter() {
            // Check if the new open conflicts with an existing open
            if !is_sharing_compatible(existing_mode, existing_sharing, mode, sharing) {
                return Err(Os2Error::SHARING_VIOLATION);
            }
        }

        existing.push((handle_id, mode, sharing));
        Ok(())
    }

    /// Remove a handle from the sharing table.
    fn remove(&mut self, path: &Path, handle_id: u64) {
        if let Some(entries) = self.entries.get_mut(path) {
            entries.retain(|&(id, _, _)| id != handle_id);
            if entries.is_empty() {
                self.entries.remove(path);
            }
        }
    }
}

/// Check if two open modes and sharing modes are compatible.
fn is_sharing_compatible(
    existing_mode: OpenMode, existing_sharing: SharingMode,
    new_mode: OpenMode, new_sharing: SharingMode,
) -> bool {
    // Check: does the existing open's sharing mode allow the new open's access mode?
    let existing_allows_new = match existing_sharing {
        SharingMode::DenyReadWrite => false, // deny all
        SharingMode::DenyWrite => {
            new_mode == OpenMode::ReadOnly
        }
        SharingMode::DenyRead => {
            new_mode == OpenMode::WriteOnly
        }
        SharingMode::DenyNone => true,
    };

    // Check: does the new open's sharing mode allow the existing open's access mode?
    let new_allows_existing = match new_sharing {
        SharingMode::DenyReadWrite => false,
        SharingMode::DenyWrite => {
            existing_mode == OpenMode::ReadOnly
        }
        SharingMode::DenyRead => {
            existing_mode == OpenMode::WriteOnly
        }
        SharingMode::DenyNone => true,
    };

    existing_allows_new && new_allows_existing
}

// ── Find state ──

/// Internal state for an active directory search.
struct FindState {
    entries: Vec<DirEntry>,
    position: usize,
}

// ── Open file state ──

/// Tracks an open file with its host path (for sharing table cleanup).
struct OpenFileState {
    file: File,
    host_path: PathBuf,
}

// ── HostDirBackend ──

/// VfsBackend implementation using a host directory as the volume root.
///
/// Provides HPFS-compatible semantics on top of the Linux filesystem:
/// - Case-insensitive, case-preserving filename lookup
/// - File sharing mode enforcement
/// - Long filenames (up to 254 characters)
/// - Sandbox enforcement (paths cannot escape the volume root)
/// - Proper OS/2 error codes
///
/// Uses interior mutability (`Mutex`) for thread safety, as required by
/// the VfsBackend trait contract.
pub struct HostDirBackend {
    root: PathBuf,
    files: Mutex<HashMap<u64, OpenFileState>>,
    finds: Mutex<HashMap<u64, FindState>>,
    sharing: Mutex<SharingTable>,
    next_id: AtomicU64,
}

impl HostDirBackend {
    /// Create a new HostDirBackend rooted at the given directory.
    /// The directory must exist.
    pub fn new(root: PathBuf) -> VfsResult<Self> {
        let root = root.canonicalize().map_err(|_| Os2Error::PATH_NOT_FOUND)?;
        if !root.is_dir() {
            return Err(Os2Error::PATH_NOT_FOUND);
        }
        Ok(HostDirBackend {
            root,
            files: Mutex::new(HashMap::new()),
            finds: Mutex::new(HashMap::new()),
            sharing: Mutex::new(SharingTable::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// Allocate a new unique handle ID.
    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Resolve a relative path to a host path, with case-insensitive lookup
    /// and sandbox enforcement.
    ///
    /// For existing files: resolves case-insensitively and canonicalizes.
    /// For new files: resolves the parent case-insensitively, preserves the
    /// filename as given (case-preserving on creation).
    fn resolve_existing(&self, rel_path: &str) -> VfsResult<PathBuf> {
        let resolved = resolve_path_case_insensitive(&self.root, rel_path)
            .ok_or(Os2Error::FILE_NOT_FOUND)?;
        self.enforce_sandbox(&resolved)?;
        Ok(resolved)
    }

    /// Resolve a path for creating a new file: parent must exist (case-insensitive),
    /// filename is preserved as-is.
    fn resolve_for_create(&self, rel_path: &str) -> VfsResult<PathBuf> {
        if rel_path.is_empty() {
            return Err(Os2Error::PATH_NOT_FOUND);
        }

        // Split into parent dir and filename
        let (parent_rel, filename) = match rel_path.rfind('/') {
            Some(pos) => (&rel_path[..pos], &rel_path[pos + 1..]),
            None => ("", rel_path),
        };

        // Validate filename length (HPFS: 254 chars max)
        if filename.len() > 254 {
            return Err(Os2Error::FILENAME_EXCED_RANGE);
        }

        let parent = if parent_rel.is_empty() {
            self.root.clone()
        } else {
            resolve_path_case_insensitive(&self.root, parent_rel)
                .ok_or(Os2Error::PATH_NOT_FOUND)?
        };

        if !parent.is_dir() {
            return Err(Os2Error::PATH_NOT_FOUND);
        }

        let full = parent.join(filename);
        self.enforce_sandbox(&full)?;
        Ok(full)
    }

    /// Verify that a path does not escape the volume root.
    fn enforce_sandbox(&self, path: &Path) -> VfsResult<()> {
        // For existing paths, canonicalize and check prefix
        let check_path = if path.exists() {
            path.canonicalize().map_err(|_| Os2Error::PATH_NOT_FOUND)?
        } else {
            // For new files, canonicalize the parent
            let parent = path.parent().ok_or(Os2Error::PATH_NOT_FOUND)?;
            if !parent.exists() {
                return Err(Os2Error::PATH_NOT_FOUND);
            }
            let canon_parent = parent.canonicalize().map_err(|_| Os2Error::PATH_NOT_FOUND)?;
            let filename = path.file_name().ok_or(Os2Error::PATH_NOT_FOUND)?;
            canon_parent.join(filename)
        };

        if !check_path.starts_with(&self.root) {
            warn!("SECURITY: Path traversal blocked: {} (root={})",
                  check_path.display(), self.root.display());
            return Err(Os2Error::ACCESS_DENIED);
        }
        Ok(())
    }

    /// Convert a std::io::Error to an Os2Error.
    fn map_io_error(e: &std::io::Error) -> Os2Error {
        use std::io::ErrorKind;
        match e.kind() {
            ErrorKind::NotFound => Os2Error::FILE_NOT_FOUND,
            ErrorKind::PermissionDenied => Os2Error::ACCESS_DENIED,
            ErrorKind::AlreadyExists => Os2Error::FILE_EXISTS,
            ErrorKind::DirectoryNotEmpty => Os2Error::DIRECTORY_NOT_EMPTY,
            _ => {
                // Check for disk full via raw OS error
                if let Some(os_err) = e.raw_os_error() {
                    if os_err == libc::ENOSPC { return Os2Error::DISK_FULL; }
                    if os_err == libc::ENAMETOOLONG { return Os2Error::FILENAME_EXCED_RANGE; }
                }
                Os2Error::ACCESS_DENIED
            }
        }
    }

    // ── Extended Attribute helpers (xattr-based) ──

    /// The xattr namespace prefix for OS/2 extended attributes.
    /// Each EA is stored as `user.os2.ea.{NAME}` with value = `[flags_u8][data...]`.
    const EA_XATTR_PREFIX: &'static str = "user.os2.ea.";

    /// Get a single extended attribute from a host path via xattr.
    fn xattr_get_ea(host_path: &Path, ea_name: &str) -> VfsResult<EaEntry> {
        let xattr_name = format!("{}{}", Self::EA_XATTR_PREFIX, ea_name);
        let c_path = CString::new(host_path.as_os_str().as_encoded_bytes())
            .map_err(|_| Os2Error::INVALID_PARAMETER)?;
        let c_xattr = CString::new(xattr_name.as_bytes())
            .map_err(|_| Os2Error::INVALID_PARAMETER)?;

        // First call: get size
        let size = unsafe {
            libc::getxattr(c_path.as_ptr(), c_xattr.as_ptr(), std::ptr::null_mut(), 0)
        };
        if size < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return if errno == libc::ENODATA || errno == libc::ENOTSUP {
                Err(Os2Error::EA_NOT_FOUND)
            } else {
                Err(Os2Error::ACCESS_DENIED)
            };
        }
        if size == 0 {
            return Ok(EaEntry { name: ea_name.to_string(), value: Vec::new(), flags: 0 });
        }

        // Second call: get data
        let mut buf = vec![0u8; size as usize];
        let got = unsafe {
            libc::getxattr(c_path.as_ptr(), c_xattr.as_ptr(),
                          buf.as_mut_ptr() as *mut libc::c_void, buf.len())
        };
        if got < 0 {
            return Err(Os2Error::ACCESS_DENIED);
        }
        buf.truncate(got as usize);

        // First byte is flags, rest is value
        let flags = if !buf.is_empty() { buf[0] } else { 0 };
        let value = if buf.len() > 1 { buf[1..].to_vec() } else { Vec::new() };

        Ok(EaEntry { name: ea_name.to_string(), value, flags })
    }

    /// Set a single extended attribute on a host path via xattr.
    fn xattr_set_ea(host_path: &Path, ea: &EaEntry) -> VfsResult<()> {
        let xattr_name = format!("{}{}", Self::EA_XATTR_PREFIX, ea.name);
        let c_path = CString::new(host_path.as_os_str().as_encoded_bytes())
            .map_err(|_| Os2Error::INVALID_PARAMETER)?;
        let c_xattr = CString::new(xattr_name.as_bytes())
            .map_err(|_| Os2Error::INVALID_PARAMETER)?;

        // Encode: [flags_u8][value_bytes...]
        let mut data = Vec::with_capacity(1 + ea.value.len());
        data.push(ea.flags);
        data.extend_from_slice(&ea.value);

        let rc = unsafe {
            libc::setxattr(c_path.as_ptr(), c_xattr.as_ptr(),
                          data.as_ptr() as *const libc::c_void, data.len(), 0)
        };
        if rc < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return if errno == libc::ENOTSUP {
                debug!("xattr not supported on {}, EA '{}' not stored",
                       host_path.display(), ea.name);
                Err(Os2Error::ACCESS_DENIED)
            } else if errno == libc::ENOSPC || errno == libc::E2BIG {
                Err(Os2Error::DISK_FULL)
            } else {
                Err(Os2Error::ACCESS_DENIED)
            };
        }
        Ok(())
    }

    /// Remove a single extended attribute from a host path.
    fn xattr_remove_ea(host_path: &Path, ea_name: &str) -> VfsResult<()> {
        let xattr_name = format!("{}{}", Self::EA_XATTR_PREFIX, ea_name);
        let c_path = CString::new(host_path.as_os_str().as_encoded_bytes())
            .map_err(|_| Os2Error::INVALID_PARAMETER)?;
        let c_xattr = CString::new(xattr_name.as_bytes())
            .map_err(|_| Os2Error::INVALID_PARAMETER)?;

        let rc = unsafe { libc::removexattr(c_path.as_ptr(), c_xattr.as_ptr()) };
        if rc < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno != libc::ENODATA {
                return Err(Os2Error::ACCESS_DENIED);
            }
        }
        Ok(())
    }

    /// List all OS/2 extended attributes on a host path.
    fn xattr_list_eas(host_path: &Path) -> VfsResult<Vec<EaEntry>> {
        let c_path = CString::new(host_path.as_os_str().as_encoded_bytes())
            .map_err(|_| Os2Error::INVALID_PARAMETER)?;

        // First call: get buffer size
        let size = unsafe { libc::listxattr(c_path.as_ptr(), std::ptr::null_mut(), 0) };
        if size < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::ENOTSUP {
                return Ok(Vec::new());
            }
            return Err(Os2Error::ACCESS_DENIED);
        }
        if size == 0 {
            return Ok(Vec::new());
        }

        // Second call: get names (null-separated list)
        let mut buf = vec![0u8; size as usize];
        let got = unsafe {
            libc::listxattr(c_path.as_ptr(), buf.as_mut_ptr() as *mut libc::c_char, buf.len())
        };
        if got < 0 {
            return Err(Os2Error::ACCESS_DENIED);
        }
        buf.truncate(got as usize);

        // Parse null-separated xattr names, filter for our prefix
        let prefix = Self::EA_XATTR_PREFIX;
        let mut eas = Vec::new();
        for name_bytes in buf.split(|&b| b == 0) {
            if name_bytes.is_empty() { continue; }
            let name = match std::str::from_utf8(name_bytes) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if let Some(ea_name) = name.strip_prefix(prefix) {
                if !ea_name.is_empty() {
                    match Self::xattr_get_ea(host_path, ea_name) {
                        Ok(ea) => eas.push(ea),
                        Err(_) => {} // skip unreadable EAs
                    }
                }
            }
        }

        Ok(eas)
    }
}

impl VfsBackend for HostDirBackend {
    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        sharing: SharingMode,
        flags: OpenFlags,
        _attributes: FileAttribute,
    ) -> VfsResult<(VfsFileHandle, OpenAction)> {
        // Try case-insensitive resolution of existing file
        let existing = resolve_path_case_insensitive(&self.root, path);
        let (host_path, action) = match (&existing, flags.exist_action, flags.new_action) {
            // File exists
            (Some(p), ExistAction::Open, _) => {
                self.enforce_sandbox(p)?;
                (p.clone(), OpenAction::Existed)
            }
            (Some(p), ExistAction::Replace, _) => {
                self.enforce_sandbox(p)?;
                (p.clone(), OpenAction::Replaced)
            }
            (Some(_), ExistAction::Fail, _) => {
                return Err(Os2Error::FILE_EXISTS);
            }
            // File does not exist
            (None, _, NewAction::Create) => {
                let p = self.resolve_for_create(path)?;
                (p, OpenAction::Created)
            }
            (None, _, NewAction::Fail) => {
                return Err(Os2Error::FILE_NOT_FOUND);
            }
        };

        // Check sharing mode compatibility
        let handle_id = self.alloc_id();
        {
            let mut sharing_table = self.sharing.lock().unwrap();
            sharing_table.check_and_add(&host_path, handle_id, mode, sharing)?;
        }

        // Build OpenOptions
        let mut opts = OpenOptions::new();
        match mode {
            OpenMode::ReadOnly => { opts.read(true); }
            OpenMode::WriteOnly => { opts.write(true); }
            OpenMode::ReadWrite => { opts.read(true).write(true); }
        }
        if action == OpenAction::Created {
            opts.create(true);
            // Creating a file requires write permission on Linux
            opts.write(true);
        }
        if action == OpenAction::Replaced {
            opts.truncate(true).create(true);
            // Truncating requires write permission on Linux
            opts.write(true);
        }

        let file = opts.open(&host_path).map_err(|e| {
            // Clean up sharing table on failure
            let mut sharing_table = self.sharing.lock().unwrap();
            sharing_table.remove(&host_path, handle_id);
            Self::map_io_error(&e)
        })?;

        self.files.lock().unwrap().insert(handle_id, OpenFileState { file, host_path });
        Ok((VfsFileHandle(handle_id), action))
    }

    fn close(&self, handle: VfsFileHandle) -> VfsResult<()> {
        let entry = self.files.lock().unwrap().remove(&handle.0)
            .ok_or(Os2Error::INVALID_HANDLE)?;
        self.sharing.lock().unwrap().remove(&entry.host_path, handle.0);
        Ok(())
    }

    fn read(&self, handle: VfsFileHandle, buf: &mut [u8]) -> VfsResult<usize> {
        let mut files = self.files.lock().unwrap();
        let entry = files.get_mut(&handle.0).ok_or(Os2Error::INVALID_HANDLE)?;
        entry.file.read(buf).map_err(|e| Self::map_io_error(&e))
    }

    fn write(&self, handle: VfsFileHandle, buf: &[u8]) -> VfsResult<usize> {
        let mut files = self.files.lock().unwrap();
        let entry = files.get_mut(&handle.0).ok_or(Os2Error::INVALID_HANDLE)?;
        entry.file.write(buf).map_err(|e| Self::map_io_error(&e))
    }

    fn seek(&self, handle: VfsFileHandle, offset: i64, mode: SeekMode) -> VfsResult<u64> {
        let mut files = self.files.lock().unwrap();
        let entry = files.get_mut(&handle.0).ok_or(Os2Error::INVALID_HANDLE)?;
        let seek_from = match mode {
            SeekMode::Begin => SeekFrom::Start(offset as u64),
            SeekMode::Current => SeekFrom::Current(offset),
            SeekMode::End => SeekFrom::End(offset),
        };
        entry.file.seek(seek_from).map_err(|e| Self::map_io_error(&e))
    }

    fn set_file_size(&self, handle: VfsFileHandle, size: u64) -> VfsResult<()> {
        let files = self.files.lock().unwrap();
        let entry = files.get(&handle.0).ok_or(Os2Error::INVALID_HANDLE)?;
        entry.file.set_len(size).map_err(|e| Self::map_io_error(&e))
    }

    fn flush(&self, handle: VfsFileHandle) -> VfsResult<()> {
        let mut files = self.files.lock().unwrap();
        let entry = files.get_mut(&handle.0).ok_or(Os2Error::INVALID_HANDLE)?;
        entry.file.flush().map_err(|e| Self::map_io_error(&e))
    }

    fn find_first(
        &self,
        pattern: &str,
        attr_filter: FileAttribute,
    ) -> VfsResult<(VfsFindHandle, DirEntry)> {
        // Split pattern into directory and filename pattern
        let (dir_part, file_pattern) = match pattern.rfind('/') {
            Some(pos) => (&pattern[..pos], &pattern[pos + 1..]),
            None => ("", pattern),
        };

        let dir_path = if dir_part.is_empty() {
            self.root.clone()
        } else {
            resolve_path_case_insensitive(&self.root, dir_part)
                .ok_or(Os2Error::PATH_NOT_FOUND)?
        };
        self.enforce_sandbox(&dir_path)?;

        if !dir_path.is_dir() {
            return Err(Os2Error::PATH_NOT_FOUND);
        }

        // Collect all matching entries (wildcard + attribute filter)
        let read_dir = fs::read_dir(&dir_path).map_err(|e| Self::map_io_error(&e))?;
        let mut entries = Vec::new();

        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if wildcard_match(file_pattern, &name) {
                if let Ok(meta) = entry.metadata() {
                    let status = metadata_to_file_status(&meta);
                    if attributes_match(status.attributes, attr_filter) {
                        entries.push(DirEntry { name, status });
                    }
                }
            }
        }

        // Also check for "." and ".." if pattern matches
        // "." and ".." are directories, so they require DIRECTORY in the attr filter
        if wildcard_match(file_pattern, ".") && attr_filter.contains(FileAttribute::DIRECTORY) {
            if let Ok(meta) = fs::metadata(&dir_path) {
                entries.insert(0, DirEntry {
                    name: ".".to_string(),
                    status: metadata_to_file_status(&meta),
                });
            }
        }
        if wildcard_match(file_pattern, "..") && attr_filter.contains(FileAttribute::DIRECTORY) {
            let parent = dir_path.parent().unwrap_or(&dir_path);
            if let Ok(meta) = fs::metadata(parent) {
                let insert_pos = if entries.first().is_some_and(|e| e.name == ".") { 1 } else { 0 };
                entries.insert(insert_pos, DirEntry {
                    name: "..".to_string(),
                    status: metadata_to_file_status(&meta),
                });
            }
        }

        if entries.is_empty() {
            return Err(Os2Error::NO_MORE_FILES);
        }

        let first = entries[0].clone();
        let handle_id = self.alloc_id();
        self.finds.lock().unwrap().insert(handle_id, FindState {
            entries,
            position: 1, // first entry already returned
        });

        Ok((VfsFindHandle(handle_id), first))
    }

    fn find_next(&self, handle: VfsFindHandle) -> VfsResult<DirEntry> {
        let mut finds = self.finds.lock().unwrap();
        let state = finds.get_mut(&handle.0).ok_or(Os2Error::INVALID_HANDLE)?;

        if state.position >= state.entries.len() {
            return Err(Os2Error::NO_MORE_FILES);
        }

        let entry = state.entries[state.position].clone();
        state.position += 1;
        Ok(entry)
    }

    fn find_close(&self, handle: VfsFindHandle) -> VfsResult<()> {
        self.finds.lock().unwrap().remove(&handle.0)
            .ok_or(Os2Error::INVALID_HANDLE)?;
        Ok(())
    }

    fn create_dir(&self, path: &str) -> VfsResult<()> {
        let host_path = self.resolve_for_create(path)?;
        fs::create_dir(&host_path).map_err(|e| Self::map_io_error(&e))
    }

    fn delete_dir(&self, path: &str) -> VfsResult<()> {
        let host_path = self.resolve_existing(path)?;
        fs::remove_dir(&host_path).map_err(|e| Self::map_io_error(&e))
    }

    fn delete(&self, path: &str) -> VfsResult<()> {
        let host_path = self.resolve_existing(path)?;
        fs::remove_file(&host_path).map_err(|e| Self::map_io_error(&e))
    }

    fn rename(&self, old_path: &str, new_path: &str) -> VfsResult<()> {
        let old_host = self.resolve_existing(old_path)?;
        let new_host = self.resolve_for_create(new_path)?;
        fs::rename(&old_host, &new_host).map_err(|e| Self::map_io_error(&e))
    }

    fn copy(&self, src_path: &str, dst_path: &str) -> VfsResult<()> {
        let src_host = self.resolve_existing(src_path)?;
        let dst_host = self.resolve_for_create(dst_path)?;
        fs::copy(&src_host, &dst_host).map_err(|e| Self::map_io_error(&e))?;
        Ok(())
    }

    fn query_path_info(&self, path: &str, _level: u32) -> VfsResult<FileStatus> {
        let host_path = self.resolve_existing(path)?;
        let meta = fs::metadata(&host_path).map_err(|e| Self::map_io_error(&e))?;
        Ok(metadata_to_file_status(&meta))
    }

    fn query_file_info(&self, handle: VfsFileHandle, _level: u32) -> VfsResult<FileStatus> {
        let files = self.files.lock().unwrap();
        let entry = files.get(&handle.0).ok_or(Os2Error::INVALID_HANDLE)?;
        let meta = entry.file.metadata().map_err(|e| Self::map_io_error(&e))?;
        Ok(metadata_to_file_status(&meta))
    }

    fn set_file_info(
        &self,
        _handle: VfsFileHandle,
        _level: u32,
        _info: &FileStatus,
    ) -> VfsResult<()> {
        // Stub: setting file info not yet implemented
        Ok(())
    }

    fn set_path_info(&self, _path: &str, _level: u32, _info: &FileStatus) -> VfsResult<()> {
        // Stub: setting path info not yet implemented
        Ok(())
    }

    fn get_ea(&self, path: &str, name: &str) -> VfsResult<EaEntry> {
        let host_path = self.resolve_existing(path)?;
        Self::xattr_get_ea(&host_path, name)
    }

    fn set_ea(&self, path: &str, ea: &EaEntry) -> VfsResult<()> {
        let host_path = self.resolve_existing(path)?;
        if ea.value.is_empty() && ea.flags == 0 {
            // Empty value with no flags = delete the EA
            Self::xattr_remove_ea(&host_path, &ea.name)
        } else {
            Self::xattr_set_ea(&host_path, ea)
        }
    }

    fn enum_ea(&self, path: &str) -> VfsResult<Vec<EaEntry>> {
        let host_path = self.resolve_existing(path)?;
        Self::xattr_list_eas(&host_path)
    }

    fn query_fs_info_alloc(&self) -> VfsResult<FsAllocate> {
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let c_path = std::ffi::CString::new(self.root.to_string_lossy().as_bytes())
            .map_err(|_| Os2Error::PATH_NOT_FOUND)?;
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
        if rc != 0 {
            return Err(Os2Error::ACCESS_DENIED);
        }

        Ok(FsAllocate {
            id_filesystem: 0,
            sectors_per_unit: (stat.f_frsize / 512).max(1) as u32,
            total_units: stat.f_blocks as u32,
            available_units: stat.f_bavail as u32,
            bytes_per_sector: 512,
        })
    }

    fn query_fs_info_volume(&self) -> VfsResult<FsVolumeInfo> {
        // Try to read volume label from .vol_label file
        let label_path = self.root.join(".vol_label");
        let label = fs::read_to_string(&label_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "OS2".to_string());

        // Generate a serial number from the root path
        let serial = {
            let path_str = self.root.to_string_lossy();
            let mut hash: u32 = 0;
            for b in path_str.bytes() {
                hash = hash.wrapping_mul(31).wrapping_add(b as u32);
            }
            hash
        };

        Ok(FsVolumeInfo {
            serial_number: serial,
            label,
        })
    }

    fn set_fs_info_volume(&self, label: &str) -> VfsResult<()> {
        let label_path = self.root.join(".vol_label");
        fs::write(&label_path, label).map_err(|e| Self::map_io_error(&e))
    }

    fn fs_name(&self) -> &str {
        "HPFS"
    }

    fn set_file_locks(
        &self,
        handle: VfsFileHandle,
        unlock: &[FileLockRange],
        lock: &[FileLockRange],
        _timeout_ms: u32,
    ) -> VfsResult<()> {
        use std::os::unix::io::AsRawFd;

        let files = self.files.lock().unwrap();
        let entry = files.get(&handle.0).ok_or(Os2Error::INVALID_HANDLE)?;
        let fd = entry.file.as_raw_fd();

        // Process unlocks first
        for range in unlock {
            let flock = libc::flock {
                l_type: libc::F_UNLCK as i16,
                l_whence: libc::SEEK_SET as i16,
                l_start: range.offset as libc::off_t,
                l_len: range.length as libc::off_t,
                l_pid: 0,
            };
            let rc = unsafe { libc::fcntl(fd, libc::F_SETLK, &flock) };
            if rc < 0 {
                return Err(Os2Error::LOCK_VIOLATION);
            }
        }

        // Process locks
        for range in lock {
            let flock = libc::flock {
                l_type: libc::F_WRLCK as i16,
                l_whence: libc::SEEK_SET as i16,
                l_start: range.offset as libc::off_t,
                l_len: range.length as libc::off_t,
                l_pid: 0,
            };
            let rc = unsafe { libc::fcntl(fd, libc::F_SETLK, &flock) };
            if rc < 0 {
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if errno == libc::EACCES || errno == libc::EAGAIN {
                    return Err(Os2Error::LOCK_VIOLATION);
                }
                return Err(Os2Error::ACCESS_DENIED);
            }
        }

        Ok(())
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    fn create_temp_backend() -> (tempfile::TempDir, HostDirBackend) {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = HostDirBackend::new(tmp.path().to_path_buf()).unwrap();
        (tmp, backend)
    }

    // ── Wildcard matching ──

    #[test]
    fn test_wildcard_star_star() {
        // HPFS: *.* matches all files, including those without dots
        assert!(wildcard_match("*.*", "test.txt"));
        assert!(wildcard_match("*.*", "README.MD"));
        assert!(wildcard_match("*.*", "Makefile")); // HPFS: matches even without dot
    }

    #[test]
    fn test_wildcard_star_ext() {
        assert!(wildcard_match("*.txt", "test.txt"));
        assert!(wildcard_match("*.txt", "TEST.TXT")); // case-insensitive
        assert!(!wildcard_match("*.txt", "test.doc"));
    }

    #[test]
    fn test_wildcard_question() {
        assert!(wildcard_match("test.???", "test.txt"));
        assert!(wildcard_match("test.???", "test.doc"));
        assert!(!wildcard_match("test.???", "test.java"));
    }

    #[test]
    fn test_wildcard_star_all() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*", "test.txt"));
        assert!(wildcard_match("*", ".hidden"));
    }

    // ── Case-insensitive path resolution ──

    #[test]
    fn test_case_insensitive_lookup() {
        let (tmp, _backend) = create_temp_backend();
        // Create a file with mixed case
        let test_file = tmp.path().join("HelloWorld.TXT");
        File::create(&test_file).unwrap();

        // Should find it with different casing
        let resolved = resolve_path_case_insensitive(tmp.path(), "helloworld.txt");
        assert!(resolved.is_some());
        let resolved = resolved.unwrap();
        assert!(resolved.exists());
        assert!(resolved.to_string_lossy().contains("HelloWorld.TXT"));
    }

    #[test]
    fn test_case_insensitive_nested() {
        let (tmp, _backend) = create_temp_backend();
        fs::create_dir(tmp.path().join("MyDir")).unwrap();
        File::create(tmp.path().join("MyDir/Test.Txt")).unwrap();

        let resolved = resolve_path_case_insensitive(tmp.path(), "mydir/test.txt");
        assert!(resolved.is_some());
        assert!(resolved.unwrap().exists());
    }

    #[test]
    fn test_case_preserving_creation() {
        let (_tmp, backend) = create_temp_backend();

        // Create a file with specific casing
        let (handle, action) = backend.open(
            "MyFile.TXT", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        assert_eq!(action, OpenAction::Created);
        backend.close(handle).unwrap();

        // Verify the file exists with the exact casing given
        let host_path = backend.root.join("MyFile.TXT");
        assert!(host_path.exists());
    }

    // ── File operations (simulates file_test sequence) ──

    #[test]
    fn test_file_test_gate() {
        // This test mirrors samples/file_test/file_test.c exactly:
        // 1. DosOpen("test.txt", CREATE_IF_NEW | REPLACE_IF_EXISTS, READWRITE | DENYNONE)
        // 2. DosWrite("Warpine File Test Data")
        // 3. DosClose
        // 4. DosOpen("test.txt", OPEN_IF_EXISTS, READONLY | DENYWRITE)
        // 5. DosRead → verify 22 bytes
        // 6. DosClose
        let (_tmp, backend) = create_temp_backend();

        // 1. Create file
        let (h1, action) = backend.open(
            "test.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), // CREATE_IF_NEW | REPLACE_IF_EXISTS
            FileAttribute::NORMAL,
        ).unwrap();
        assert_eq!(action, OpenAction::Created);

        // 2. Write data
        let msg = b"Warpine File Test Data";
        let written = backend.write(h1, msg).unwrap();
        assert_eq!(written, 22);

        // 3. Close
        backend.close(h1).unwrap();

        // 4. Reopen for reading
        let (h2, action) = backend.open(
            "test.txt", OpenMode::ReadOnly, SharingMode::DenyWrite,
            OpenFlags::from_raw(0x0001), // OPEN_IF_EXISTS
            FileAttribute::NORMAL,
        ).unwrap();
        assert_eq!(action, OpenAction::Existed);

        // 5. Read back
        let mut buf = [0u8; 100];
        let read_count = backend.read(h2, &mut buf).unwrap();
        assert_eq!(read_count, 22);
        assert_eq!(&buf[..22], msg);

        // 6. Close
        backend.close(h2).unwrap();
    }

    // ── Sharing mode enforcement ──

    #[test]
    fn test_sharing_deny_write() {
        let (_tmp, backend) = create_temp_backend();

        // Open for reading with DENY_WRITE
        let (h1, _) = backend.open(
            "share_test.txt", OpenMode::ReadOnly, SharingMode::DenyWrite,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();

        // Another read-only open should succeed (not denied)
        let result = backend.open(
            "share_test.txt", OpenMode::ReadOnly, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0001), FileAttribute::NORMAL,
        );
        assert!(result.is_ok());
        let (h2, _) = result.unwrap();

        // A write open should fail (SHARING_VIOLATION)
        let result = backend.open(
            "share_test.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0001), FileAttribute::NORMAL,
        );
        assert_eq!(result.unwrap_err(), Os2Error::SHARING_VIOLATION);

        backend.close(h1).unwrap();
        backend.close(h2).unwrap();

        // After closing, write should succeed
        let result = backend.open(
            "share_test.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0001), FileAttribute::NORMAL,
        );
        assert!(result.is_ok());
        backend.close(result.unwrap().0).unwrap();
    }

    // ── Directory operations ──

    #[test]
    fn test_create_and_delete_dir() {
        let (_tmp, backend) = create_temp_backend();

        backend.create_dir("TestDir").unwrap();
        let info = backend.query_path_info("TestDir", 1).unwrap();
        assert!(info.attributes.contains(FileAttribute::DIRECTORY));

        backend.delete_dir("TestDir").unwrap();
        assert_eq!(backend.query_path_info("TestDir", 1).unwrap_err(), Os2Error::FILE_NOT_FOUND);
    }

    // ── Directory enumeration ──

    #[test]
    fn test_find_first_next() {
        let (_tmp, backend) = create_temp_backend();

        // Create some files
        backend.open("alpha.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL).map(|(h, _)| backend.close(h)).unwrap().unwrap();
        backend.open("beta.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL).map(|(h, _)| backend.close(h)).unwrap().unwrap();
        backend.open("gamma.doc", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL).map(|(h, _)| backend.close(h)).unwrap().unwrap();

        // Find *.txt
        let (fh, first) = backend.find_first("*.txt", FileAttribute::NORMAL).unwrap();
        let mut names = vec![first.name];
        loop {
            match backend.find_next(fh) {
                Ok(entry) => names.push(entry.name),
                Err(e) if e == Os2Error::NO_MORE_FILES => break,
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }
        backend.find_close(fh).unwrap();

        names.sort();
        assert_eq!(names, vec!["alpha.txt", "beta.txt"]);
    }

    // ── Sandbox enforcement ──

    #[test]
    fn test_sandbox_blocks_traversal() {
        let (_tmp, backend) = create_temp_backend();

        // Try to escape via ..
        let result = backend.open(
            "../../etc/passwd", OpenMode::ReadOnly, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0001), FileAttribute::NORMAL,
        );
        // Should fail with FILE_NOT_FOUND (.. past root resolves to root)
        // or ACCESS_DENIED if canonicalization reveals escape
        assert!(result.is_err());
    }

    // ── File metadata ──

    #[test]
    fn test_query_path_info() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "info_test.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.write(h, b"hello").unwrap();
        backend.close(h).unwrap();

        let status = backend.query_path_info("info_test.txt", 1).unwrap();
        assert_eq!(status.file_size, 5);
        assert!(status.attributes.contains(FileAttribute::ARCHIVE));
        assert!(!status.attributes.contains(FileAttribute::DIRECTORY));
    }

    // ── Filesystem info ──

    #[test]
    fn test_query_fs_info() {
        let (_tmp, backend) = create_temp_backend();

        let alloc = backend.query_fs_info_alloc().unwrap();
        assert_eq!(alloc.bytes_per_sector, 512);
        assert!(alloc.total_units > 0);
        assert!(alloc.available_units > 0);

        let vol = backend.query_fs_info_volume().unwrap();
        assert_eq!(vol.label, "OS2"); // default label

        assert_eq!(backend.fs_name(), "HPFS");
    }

    // ── Rename and copy ──

    #[test]
    fn test_rename_file() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "old.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.write(h, b"data").unwrap();
        backend.close(h).unwrap();

        backend.rename("old.txt", "new.txt").unwrap();

        assert_eq!(backend.query_path_info("old.txt", 1).unwrap_err(), Os2Error::FILE_NOT_FOUND);
        let status = backend.query_path_info("new.txt", 1).unwrap();
        assert_eq!(status.file_size, 4);
    }

    #[test]
    fn test_copy_file() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "src.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.write(h, b"copy me").unwrap();
        backend.close(h).unwrap();

        backend.copy("src.txt", "dst.txt").unwrap();

        let status = backend.query_path_info("dst.txt", 1).unwrap();
        assert_eq!(status.file_size, 7);
    }

    // ── HPFS wildcard semantics ──

    #[test]
    fn test_wildcard_star_dot_star_matches_all() {
        // HPFS: *.* matches everything including files without dots
        assert!(wildcard_match("*.*", "Makefile"));
        assert!(wildcard_match("*.*", "README"));
        assert!(wildcard_match("*.*", "test.txt"));
        assert!(wildcard_match("*.*", ".hidden"));
    }

    #[test]
    fn test_wildcard_no_dot_pattern() {
        // Pattern without dot should not match files with dots (unless * covers it)
        assert!(wildcard_match("test*", "test.txt"));
        assert!(wildcard_match("test*", "testing"));
        assert!(!wildcard_match("test", "test.txt"));
    }

    // ── Attribute filtering ──

    #[test]
    fn test_attr_filter_normal_files() {
        // Normal files (ARCHIVE) always pass
        assert!(attributes_match(FileAttribute::ARCHIVE, FileAttribute::NORMAL));
        assert!(attributes_match(FileAttribute::ARCHIVE, FileAttribute::ARCHIVE));
    }

    #[test]
    fn test_attr_filter_directory_excluded_by_default() {
        // Directories excluded unless DIRECTORY bit set in filter
        assert!(!attributes_match(FileAttribute::DIRECTORY, FileAttribute::NORMAL));
        assert!(attributes_match(FileAttribute::DIRECTORY,
            FileAttribute(FileAttribute::DIRECTORY.0 | FileAttribute::NORMAL.0)));
    }

    #[test]
    fn test_attr_filter_hidden_excluded_by_default() {
        assert!(!attributes_match(FileAttribute::HIDDEN, FileAttribute::NORMAL));
        assert!(attributes_match(FileAttribute::HIDDEN,
            FileAttribute(FileAttribute::HIDDEN.0)));
    }

    #[test]
    fn test_find_first_attr_filter_dirs() {
        let (_tmp, backend) = create_temp_backend();

        // Create a file and a directory
        let (h, _) = backend.open(
            "file.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.close(h).unwrap();
        backend.create_dir("subdir").unwrap();

        // Without DIRECTORY in filter, should only get the file
        let (fh, first) = backend.find_first("*", FileAttribute::NORMAL).unwrap();
        let mut names = vec![first.name];
        while let Ok(entry) = backend.find_next(fh) {
            names.push(entry.name);
        }
        backend.find_close(fh).unwrap();
        assert!(names.contains(&"file.txt".to_string()));
        assert!(!names.contains(&"subdir".to_string()));

        // With DIRECTORY in filter, should get both
        let attr_with_dir = FileAttribute(FileAttribute::DIRECTORY.0);
        let (fh2, first2) = backend.find_first("*", attr_with_dir).unwrap();
        let mut names2 = vec![first2.name];
        while let Ok(entry) = backend.find_next(fh2) {
            names2.push(entry.name);
        }
        backend.find_close(fh2).unwrap();
        assert!(names2.contains(&"file.txt".to_string()));
        assert!(names2.contains(&"subdir".to_string()));
    }

    // ── Sharing compatibility ──

    #[test]
    fn test_sharing_compatibility() {
        // DenyNone allows anything
        assert!(is_sharing_compatible(
            OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenMode::ReadWrite, SharingMode::DenyNone,
        ));

        // DenyReadWrite blocks everything
        assert!(!is_sharing_compatible(
            OpenMode::ReadOnly, SharingMode::DenyReadWrite,
            OpenMode::ReadOnly, SharingMode::DenyNone,
        ));

        // DenyWrite allows read-only
        assert!(is_sharing_compatible(
            OpenMode::ReadOnly, SharingMode::DenyWrite,
            OpenMode::ReadOnly, SharingMode::DenyNone,
        ));

        // DenyWrite blocks write
        assert!(!is_sharing_compatible(
            OpenMode::ReadOnly, SharingMode::DenyWrite,
            OpenMode::WriteOnly, SharingMode::DenyNone,
        ));
    }

    // ── Extended Attributes ──

    #[test]
    fn test_ea_set_and_get() {
        let (_tmp, backend) = create_temp_backend();

        // Create a file to attach EAs to
        let (h, _) = backend.open(
            "ea_test.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.close(h).unwrap();

        // Set an EA
        let ea = EaEntry {
            name: ".TYPE".to_string(),
            value: b"Plain Text".to_vec(),
            flags: 0,
        };
        backend.set_ea("ea_test.txt", &ea).unwrap();

        // Get it back
        let got = backend.get_ea("ea_test.txt", ".TYPE").unwrap();
        assert_eq!(got.name, ".TYPE");
        assert_eq!(got.value, b"Plain Text");
        assert_eq!(got.flags, 0);
    }

    #[test]
    fn test_ea_critical_flag() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "critical.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.close(h).unwrap();

        // Set a critical EA (flags = 0x80)
        let ea = EaEntry {
            name: ".LONGNAME".to_string(),
            value: b"A Very Long Filename.txt".to_vec(),
            flags: 0x80,
        };
        backend.set_ea("critical.txt", &ea).unwrap();

        let got = backend.get_ea("critical.txt", ".LONGNAME").unwrap();
        assert_eq!(got.flags, 0x80);
        assert_eq!(got.value, b"A Very Long Filename.txt");
    }

    #[test]
    fn test_ea_not_found() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "noea.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.close(h).unwrap();

        let result = backend.get_ea("noea.txt", ".NONEXISTENT");
        assert_eq!(result.unwrap_err(), Os2Error::EA_NOT_FOUND);
    }

    #[test]
    fn test_ea_enum() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "multi_ea.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.close(h).unwrap();

        // Set multiple EAs
        backend.set_ea("multi_ea.txt", &EaEntry {
            name: ".TYPE".to_string(), value: b"Plain Text".to_vec(), flags: 0,
        }).unwrap();
        backend.set_ea("multi_ea.txt", &EaEntry {
            name: ".SUBJECT".to_string(), value: b"Test file".to_vec(), flags: 0,
        }).unwrap();

        // Enumerate
        let eas = backend.enum_ea("multi_ea.txt").unwrap();
        assert_eq!(eas.len(), 2);
        let names: Vec<&str> = eas.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&".TYPE"));
        assert!(names.contains(&".SUBJECT"));
    }

    #[test]
    fn test_ea_delete() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "del_ea.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.close(h).unwrap();

        // Set then delete an EA (empty value + flags=0 means delete)
        backend.set_ea("del_ea.txt", &EaEntry {
            name: ".TYPE".to_string(), value: b"data".to_vec(), flags: 0,
        }).unwrap();
        assert!(backend.get_ea("del_ea.txt", ".TYPE").is_ok());

        backend.set_ea("del_ea.txt", &EaEntry {
            name: ".TYPE".to_string(), value: Vec::new(), flags: 0,
        }).unwrap();
        assert_eq!(backend.get_ea("del_ea.txt", ".TYPE").unwrap_err(), Os2Error::EA_NOT_FOUND);
    }

    #[test]
    fn test_ea_overwrite() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "overwrite_ea.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.close(h).unwrap();

        // Set EA
        backend.set_ea("overwrite_ea.txt", &EaEntry {
            name: ".TYPE".to_string(), value: b"Original".to_vec(), flags: 0,
        }).unwrap();

        // Overwrite with new value
        backend.set_ea("overwrite_ea.txt", &EaEntry {
            name: ".TYPE".to_string(), value: b"Updated".to_vec(), flags: 0x80,
        }).unwrap();

        let got = backend.get_ea("overwrite_ea.txt", ".TYPE").unwrap();
        assert_eq!(got.value, b"Updated");
        assert_eq!(got.flags, 0x80);
    }

    // ── Filesystem information ──

    #[test]
    fn test_set_and_get_volume_label() {
        let (_tmp, backend) = create_temp_backend();

        // Default label
        let vol = backend.query_fs_info_volume().unwrap();
        assert_eq!(vol.label, "OS2");

        // Set new label
        backend.set_fs_info_volume("MYVOLUME").unwrap();

        // Read it back
        let vol = backend.query_fs_info_volume().unwrap();
        assert_eq!(vol.label, "MYVOLUME");
    }

    // ── File locking ──

    #[test]
    fn test_file_lock_basic() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "lock_test.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.write(h, b"test data for locking").unwrap();

        // Lock bytes 0-10
        let lock_range = FileLockRange { offset: 0, length: 10 };
        backend.set_file_locks(h, &[], &[lock_range], 0).unwrap();

        // Unlock
        backend.set_file_locks(h, &[lock_range], &[], 0).unwrap();

        backend.close(h).unwrap();
    }

    #[test]
    fn test_file_lock_invalid_handle() {
        let (_tmp, backend) = create_temp_backend();

        let lock_range = FileLockRange { offset: 0, length: 10 };
        let result = backend.set_file_locks(VfsFileHandle(999), &[], &[lock_range], 0);
        assert_eq!(result.unwrap_err(), Os2Error::INVALID_HANDLE);
    }

    #[test]
    fn test_ea_case_insensitive_path() {
        let (_tmp, backend) = create_temp_backend();

        let (h, _) = backend.open(
            "CaseEA.TXT", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        backend.close(h).unwrap();

        // Set EA using original case
        backend.set_ea("CaseEA.TXT", &EaEntry {
            name: ".TYPE".to_string(), value: b"test".to_vec(), flags: 0,
        }).unwrap();

        // Read EA using different case (case-insensitive path resolution)
        let got = backend.get_ea("caseea.txt", ".TYPE").unwrap();
        assert_eq!(got.value, b"test");
    }
}
