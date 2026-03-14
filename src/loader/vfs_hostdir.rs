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

use log::warn;

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

// ── OS/2 wildcard matching ──

/// Match a filename against an OS/2 wildcard pattern (case-insensitive).
/// Supports `*` (match any sequence) and `?` (match any single character).
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let n: Vec<char> = name.to_ascii_lowercase().chars().collect();
    wildcard_match_recursive(&p, &n)
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
        _attributes: FileAttribute,
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

        // Collect all matching entries
        let read_dir = fs::read_dir(&dir_path).map_err(|e| Self::map_io_error(&e))?;
        let mut entries = Vec::new();

        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if wildcard_match(file_pattern, &name) {
                if let Ok(meta) = entry.metadata() {
                    entries.push(DirEntry {
                        name,
                        status: metadata_to_file_status(&meta),
                    });
                }
            }
        }

        // Also check for "." and ".." if pattern matches
        if wildcard_match(file_pattern, ".") {
            if let Ok(meta) = fs::metadata(&dir_path) {
                entries.insert(0, DirEntry {
                    name: ".".to_string(),
                    status: metadata_to_file_status(&meta),
                });
            }
        }
        if wildcard_match(file_pattern, "..") {
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

    fn get_ea(&self, _path: &str, _name: &str) -> VfsResult<EaEntry> {
        // EAs deferred to Step 3
        Err(Os2Error::EA_NOT_FOUND)
    }

    fn set_ea(&self, _path: &str, _ea: &EaEntry) -> VfsResult<()> {
        // EAs deferred to Step 3
        Ok(())
    }

    fn enum_ea(&self, _path: &str) -> VfsResult<Vec<EaEntry>> {
        // EAs deferred to Step 3
        Ok(Vec::new())
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

    fn fs_name(&self) -> &str {
        "HPFS"
    }

    fn set_file_locks(
        &self,
        _handle: VfsFileHandle,
        _unlock: &[FileLockRange],
        _lock: &[FileLockRange],
        _timeout_ms: u32,
    ) -> VfsResult<()> {
        // File locking deferred to Step 4
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
        assert!(wildcard_match("*.*", "test.txt"));
        assert!(wildcard_match("*.*", "README.MD"));
        assert!(!wildcard_match("*.*", "Makefile")); // no dot
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
}
