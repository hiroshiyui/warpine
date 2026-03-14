// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 Virtual Filesystem trait, DriveManager, and associated types.
//
// The VfsBackend trait defines the correctness boundary for OS/2 filesystem
// operations with HPFS semantics. Every valid OS/2 filesystem operation must
// succeed with correct behavior; invalid operations return proper OS/2 error
// codes, never crashes. The only failure mode is the host side failing
// (disk full, permissions, etc.).
//
// See doc/developer_guide.md "Filesystem I/O Design" for architecture details.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

// ── OS/2 Error Type ──

/// OS/2 API error code.
///
/// Wraps a `u32` matching the OS/2 error code numbering (0 = NO_ERROR).
/// Use `.0` to extract the raw code for returning to guest RAX.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Os2Error(pub u32);

impl Os2Error {
    pub const NO_ERROR: Self = Self(0);
    pub const INVALID_FUNCTION: Self = Self(1);
    pub const FILE_NOT_FOUND: Self = Self(2);
    pub const PATH_NOT_FOUND: Self = Self(3);
    pub const TOO_MANY_OPEN_FILES: Self = Self(4);
    pub const ACCESS_DENIED: Self = Self(5);
    pub const INVALID_HANDLE: Self = Self(6);
    pub const NOT_ENOUGH_MEMORY: Self = Self(8);
    pub const INVALID_DRIVE: Self = Self(15);
    pub const CURRENT_DIRECTORY: Self = Self(16);
    pub const NOT_SAME_DEVICE: Self = Self(17);
    pub const NO_MORE_FILES: Self = Self(18);
    pub const WRITE_PROTECT: Self = Self(19);
    pub const SHARING_VIOLATION: Self = Self(32);
    pub const LOCK_VIOLATION: Self = Self(33);
    pub const FILE_EXISTS: Self = Self(80);
    pub const CANNOT_MAKE: Self = Self(82);
    pub const INVALID_PARAMETER: Self = Self(87);
    pub const OPEN_FAILED: Self = Self(110);
    pub const BUFFER_OVERFLOW: Self = Self(111);
    pub const DISK_FULL: Self = Self(112);
    pub const NO_MORE_SEARCH_HANDLES: Self = Self(113);
    pub const INVALID_LEVEL: Self = Self(124);
    pub const DIRECTORY_NOT_EMPTY: Self = Self(145);
    pub const FILENAME_EXCED_RANGE: Self = Self(206);
    pub const EA_NOT_FOUND: Self = Self(254);
}

impl fmt::Debug for Os2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.0 {
            0 => "NO_ERROR",
            1 => "INVALID_FUNCTION",
            2 => "FILE_NOT_FOUND",
            3 => "PATH_NOT_FOUND",
            4 => "TOO_MANY_OPEN_FILES",
            5 => "ACCESS_DENIED",
            6 => "INVALID_HANDLE",
            8 => "NOT_ENOUGH_MEMORY",
            15 => "INVALID_DRIVE",
            16 => "CURRENT_DIRECTORY",
            17 => "NOT_SAME_DEVICE",
            18 => "NO_MORE_FILES",
            19 => "WRITE_PROTECT",
            32 => "SHARING_VIOLATION",
            33 => "LOCK_VIOLATION",
            80 => "FILE_EXISTS",
            82 => "CANNOT_MAKE",
            87 => "INVALID_PARAMETER",
            110 => "OPEN_FAILED",
            111 => "BUFFER_OVERFLOW",
            112 => "DISK_FULL",
            113 => "NO_MORE_SEARCH_HANDLES",
            124 => "INVALID_LEVEL",
            145 => "DIRECTORY_NOT_EMPTY",
            206 => "FILENAME_EXCED_RANGE",
            254 => "EA_NOT_FOUND",
            _ => return write!(f, "Os2Error({})", self.0),
        };
        write!(f, "Os2Error({}={})", self.0, name)
    }
}

impl fmt::Display for Os2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OS/2 error {}", self.0)
    }
}

pub type VfsResult<T> = Result<T, Os2Error>;

// ── OS/2 Data Types ──

/// File access mode (DosOpen fsOpenMode bits 0-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    ReadOnly = 0,
    WriteOnly = 1,
    ReadWrite = 2,
}

impl OpenMode {
    pub fn from_raw(fs_open_mode: u32) -> VfsResult<Self> {
        match fs_open_mode & 0x07 {
            0 => Ok(OpenMode::ReadOnly),
            1 => Ok(OpenMode::WriteOnly),
            2 => Ok(OpenMode::ReadWrite),
            _ => Err(Os2Error::INVALID_PARAMETER),
        }
    }
}

/// File sharing mode (DosOpen fsOpenMode bits 4-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharingMode {
    /// Deny read and write access to others.
    DenyReadWrite = 0x10,
    /// Deny write access to others.
    DenyWrite = 0x20,
    /// Deny read access to others.
    DenyRead = 0x30,
    /// Allow full access to others.
    DenyNone = 0x40,
}

impl SharingMode {
    pub fn from_raw(fs_open_mode: u32) -> Self {
        match fs_open_mode & 0x70 {
            0x10 => SharingMode::DenyReadWrite,
            0x20 => SharingMode::DenyWrite,
            0x30 => SharingMode::DenyRead,
            0x40 => SharingMode::DenyNone,
            // Default to DenyNone if unspecified (compatibility behavior)
            _ => SharingMode::DenyNone,
        }
    }
}

/// What to do if the file already exists (DosOpen fsOpenFlags bits 0-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistAction {
    /// Fail if file exists.
    Fail = 0,
    /// Open the existing file.
    Open = 1,
    /// Replace (truncate) the existing file.
    Replace = 2,
}

/// What to do if the file does not exist (DosOpen fsOpenFlags bits 4-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewAction {
    /// Fail if file does not exist.
    Fail = 0,
    /// Create the file.
    Create = 1,
}

/// Combined open flags parsed from DosOpen fsOpenFlags parameter.
#[derive(Debug, Clone, Copy)]
pub struct OpenFlags {
    pub exist_action: ExistAction,
    pub new_action: NewAction,
}

impl OpenFlags {
    pub fn from_raw(fs_open_flags: u32) -> Self {
        let exist_action = match fs_open_flags & 0x0F {
            0 => ExistAction::Fail,
            1 => ExistAction::Open,
            2 => ExistAction::Replace,
            _ => ExistAction::Fail,
        };
        let new_action = match (fs_open_flags >> 4) & 0x0F {
            0 => NewAction::Fail,
            1 => NewAction::Create,
            _ => NewAction::Fail,
        };
        OpenFlags { exist_action, new_action }
    }
}

/// What DosOpen actually did (returned via pulAction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAction {
    /// File existed and was opened.
    Existed = 1,
    /// File did not exist and was created.
    Created = 2,
    /// File existed and was replaced.
    Replaced = 3,
}

/// Seek origin for DosSetFilePtr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekMode {
    /// Seek from beginning of file.
    Begin = 0,
    /// Seek from current position.
    Current = 1,
    /// Seek from end of file.
    End = 2,
}

impl SeekMode {
    pub fn from_raw(mode: u32) -> VfsResult<Self> {
        match mode {
            0 => Ok(SeekMode::Begin),
            1 => Ok(SeekMode::Current),
            2 => Ok(SeekMode::End),
            _ => Err(Os2Error::INVALID_PARAMETER),
        }
    }
}

/// OS/2 file attributes (bitflags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileAttribute(pub u32);

impl FileAttribute {
    pub const NORMAL: Self = Self(0x00);
    pub const READONLY: Self = Self(0x01);
    pub const HIDDEN: Self = Self(0x02);
    pub const SYSTEM: Self = Self(0x04);
    pub const DIRECTORY: Self = Self(0x10);
    pub const ARCHIVE: Self = Self(0x20);

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// File metadata (corresponds to OS/2 FILESTATUS3).
#[derive(Debug, Clone)]
pub struct FileStatus {
    pub creation_date: u16,
    pub creation_time: u16,
    pub last_access_date: u16,
    pub last_access_time: u16,
    pub last_write_date: u16,
    pub last_write_time: u16,
    pub file_size: u32,
    pub file_alloc: u32,
    pub attributes: FileAttribute,
}

/// Directory entry returned by find_first/find_next (corresponds to FILEFINDBUF3).
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub status: FileStatus,
}

/// Extended attribute entry.
#[derive(Debug, Clone)]
pub struct EaEntry {
    pub name: String,
    pub value: Vec<u8>,
    /// fEA flags: 0x80 = critical EA.
    pub flags: u8,
}

/// Filesystem allocation info (DosQueryFSInfo level 1).
#[derive(Debug, Clone)]
pub struct FsAllocate {
    pub id_filesystem: u32,
    pub sectors_per_unit: u32,
    pub total_units: u32,
    pub available_units: u32,
    pub bytes_per_sector: u16,
}

/// Filesystem volume info (DosQueryFSInfo level 2).
#[derive(Debug, Clone)]
pub struct FsVolumeInfo {
    pub serial_number: u32,
    pub label: String,
}

/// Byte-range lock specification for DosSetFileLocks.
#[derive(Debug, Clone, Copy)]
pub struct FileLockRange {
    pub offset: u32,
    pub length: u32,
}

// ── Opaque Handle Types ──

/// Opaque file handle returned by VfsBackend::open().
/// Backends can store any u64 value (fd number, internal index, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VfsFileHandle(pub(crate) u64);

/// Opaque directory search handle returned by VfsBackend::find_first().
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VfsFindHandle(pub(crate) u64);

// ── VfsBackend Trait ──

/// OS/2 filesystem operations with HPFS semantics.
///
/// This trait is the correctness boundary: any implementation must ensure that
/// every valid OS/2 filesystem operation succeeds with correct behavior.
/// Invalid operations return `Os2Error`, never panic.
///
/// Path parameters are relative paths within the volume (no drive letter),
/// with backslashes already converted to forward slashes by the DriveManager.
///
/// Backend implementations must use interior mutability (e.g., `Mutex`) for
/// file state, since the DriveManager holds backends behind `SharedState`'s
/// mutex and individual operations should not hold the DriveManager lock.
pub trait VfsBackend: Send + Sync {
    // ── File Operations ──

    /// Open or create a file. Returns the VFS handle and what action was taken.
    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        sharing: SharingMode,
        flags: OpenFlags,
        attributes: FileAttribute,
    ) -> VfsResult<(VfsFileHandle, OpenAction)>;

    /// Close a file handle.
    fn close(&self, handle: VfsFileHandle) -> VfsResult<()>;

    /// Read from an open file. Returns the number of bytes read.
    fn read(&self, handle: VfsFileHandle, buf: &mut [u8]) -> VfsResult<usize>;

    /// Write to an open file. Returns the number of bytes written.
    fn write(&self, handle: VfsFileHandle, buf: &[u8]) -> VfsResult<usize>;

    /// Seek to a position in the file. Returns the new absolute position.
    fn seek(&self, handle: VfsFileHandle, offset: i64, mode: SeekMode) -> VfsResult<u64>;

    /// Set the file size (truncate or extend).
    fn set_file_size(&self, handle: VfsFileHandle, size: u64) -> VfsResult<()>;

    /// Flush buffered writes to storage.
    fn flush(&self, handle: VfsFileHandle) -> VfsResult<()>;

    // ── Directory Enumeration ──

    /// Start a directory search. Returns the first matching entry.
    /// The pattern supports OS/2 wildcards (`*`, `?`) with HPFS semantics.
    fn find_first(
        &self,
        pattern: &str,
        attributes: FileAttribute,
    ) -> VfsResult<(VfsFindHandle, DirEntry)>;

    /// Get the next matching entry. Returns `NO_MORE_FILES` when exhausted.
    fn find_next(&self, handle: VfsFindHandle) -> VfsResult<DirEntry>;

    /// Close a directory search handle.
    fn find_close(&self, handle: VfsFindHandle) -> VfsResult<()>;

    // ── Directory Management ──

    /// Create a directory.
    fn create_dir(&self, path: &str) -> VfsResult<()>;

    /// Remove an empty directory.
    fn delete_dir(&self, path: &str) -> VfsResult<()>;

    // ── File Management ──

    /// Delete a file.
    fn delete(&self, path: &str) -> VfsResult<()>;

    /// Rename or move a file within the same volume.
    fn rename(&self, old_path: &str, new_path: &str) -> VfsResult<()>;

    /// Copy a file within the same volume.
    fn copy(&self, src_path: &str, dst_path: &str) -> VfsResult<()>;

    // ── Metadata ──

    /// Query file/directory status by path.
    fn query_path_info(&self, path: &str, level: u32) -> VfsResult<FileStatus>;

    /// Query file status by open handle.
    fn query_file_info(&self, handle: VfsFileHandle, level: u32) -> VfsResult<FileStatus>;

    /// Set file status by open handle.
    fn set_file_info(
        &self,
        handle: VfsFileHandle,
        level: u32,
        info: &FileStatus,
    ) -> VfsResult<()>;

    /// Set file/directory status by path.
    fn set_path_info(&self, path: &str, level: u32, info: &FileStatus) -> VfsResult<()>;

    // ── Extended Attributes ──

    /// Get a single extended attribute by name.
    fn get_ea(&self, path: &str, name: &str) -> VfsResult<EaEntry>;

    /// Set a single extended attribute.
    fn set_ea(&self, path: &str, ea: &EaEntry) -> VfsResult<()>;

    /// Enumerate all extended attributes on a file.
    fn enum_ea(&self, path: &str) -> VfsResult<Vec<EaEntry>>;

    // ── Filesystem Information ──

    /// Query filesystem allocation info (DosQueryFSInfo level 1).
    fn query_fs_info_alloc(&self) -> VfsResult<FsAllocate>;

    /// Query filesystem volume info (DosQueryFSInfo level 2).
    fn query_fs_info_volume(&self) -> VfsResult<FsVolumeInfo>;

    /// Set filesystem volume label (DosSetFSInfo level 2).
    fn set_fs_info_volume(&self, label: &str) -> VfsResult<()>;

    /// Filesystem driver name (e.g., "HPFS").
    fn fs_name(&self) -> &str;

    // ── File Locking ──

    /// Apply byte-range locks and unlocks.
    fn set_file_locks(
        &self,
        handle: VfsFileHandle,
        unlock: &[FileLockRange],
        lock: &[FileLockRange],
        timeout_ms: u32,
    ) -> VfsResult<()>;
}

// ── DriveManager ──

/// An open file tracked by DriveManager.
struct FileEntry {
    drive: u8,
    vfs_handle: VfsFileHandle,
}

/// An open directory search tracked by DriveManager.
struct FindEntry {
    drive: u8,
    vfs_handle: VfsFindHandle,
    /// Original search pattern (kept for diagnostics/logging).
    #[allow(dead_code)]
    pattern: String,
}

/// Configuration for a single drive mapping.
#[derive(Debug, Clone)]
pub struct DriveConfig {
    /// Host directory that serves as the volume root.
    pub host_path: PathBuf,
    /// Volume label reported by DosQueryFSInfo.
    pub label: String,
    /// Whether the drive is read-only.
    pub read_only: bool,
}

/// Maps OS/2 drive letters to VfsBackend implementations and owns
/// all file and directory search handle state.
///
/// Replaces `HandleManager`, `HDirManager`, and `translate_path()`.
/// Standard handles (0=stdin, 1=stdout, 2=stderr) are NOT managed here;
/// they remain special-cased in doscalls.rs.
///
/// Pipes and other non-filesystem handles are also not managed here
/// (they continue to use HandleManager during the transition period,
/// and will get their own mechanism in a future step).
pub struct DriveManager {
    drives: [Option<Box<dyn VfsBackend>>; 26],
    /// Drive configurations (host path mappings). Stored separately from
    /// backends so configuration can be set before backends are created.
    drive_configs: [Option<DriveConfig>; 26],
    file_handles: HashMap<u32, FileEntry>,
    find_handles: HashMap<u32, FindEntry>,
    next_file_handle: u32,
    next_find_handle: u32,
    current_disk: u8,          // 0=A, 1=B, 2=C (internal), default 2
    current_dirs: [String; 26],
}

impl DriveManager {
    /// Create a new DriveManager with no drives mounted or configured.
    /// File handles start at 3 (0/1/2 reserved for stdin/stdout/stderr).
    /// Find handles start at 10.
    pub fn new() -> Self {
        const EMPTY_STRING: String = String::new();
        DriveManager {
            drives: std::array::from_fn(|_| None),
            drive_configs: std::array::from_fn(|_| None),
            file_handles: HashMap::new(),
            find_handles: HashMap::new(),
            next_file_handle: 3,
            next_find_handle: 10,
            current_disk: 2, // C:
            current_dirs: [EMPTY_STRING; 26],
        }
    }

    /// Create a DriveManager with default configuration:
    /// C: → `~/.local/share/warpine/drive_c/`
    ///
    /// The directory is created if it does not exist.
    /// Falls back to `./drive_c/` if the home directory cannot be determined.
    pub fn with_default_config() -> Self {
        let mut dm = Self::new();

        let base_dir = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".local/share"))
                    .unwrap_or_else(|_| PathBuf::from("."))
            });
        let drive_c_path = base_dir.join("warpine/drive_c");

        if let Err(e) = std::fs::create_dir_all(&drive_c_path) {
            log::warn!("Failed to create default C: drive directory {}: {}",
                       drive_c_path.display(), e);
        }

        dm.set_drive_config(2, DriveConfig {
            host_path: drive_c_path,
            label: "OS2".to_string(),
            read_only: false,
        });

        dm
    }

    /// Mount a backend on a drive letter (0=A, 1=B, ..., 25=Z).
    pub fn mount(&mut self, drive: u8, backend: Box<dyn VfsBackend>) {
        assert!(drive < 26, "Drive index must be 0-25");
        self.drives[drive as usize] = Some(backend);
    }

    /// Unmount a drive letter.
    pub fn unmount(&mut self, drive: u8) {
        assert!(drive < 26, "Drive index must be 0-25");
        self.drives[drive as usize] = None;
    }

    /// Set the configuration for a drive (host path, label, read-only flag).
    /// This stores the config for later use when creating a backend.
    pub fn set_drive_config(&mut self, drive: u8, config: DriveConfig) {
        assert!(drive < 26, "Drive index must be 0-25");
        self.drive_configs[drive as usize] = Some(config);
    }

    /// Get the configuration for a drive, if set.
    pub fn drive_config(&self, drive: u8) -> Option<&DriveConfig> {
        if drive >= 26 { return None; }
        self.drive_configs[drive as usize].as_ref()
    }

    /// Get the backend for a drive, or INVALID_DRIVE if not mounted.
    pub fn backend(&self, drive: u8) -> VfsResult<&dyn VfsBackend> {
        if drive >= 26 {
            return Err(Os2Error::INVALID_DRIVE);
        }
        self.drives[drive as usize]
            .as_deref()
            .ok_or(Os2Error::INVALID_DRIVE)
    }

    /// Resolve an OS/2 path to (drive_index, relative_path_within_volume).
    ///
    /// Handles drive letter extraction, relative path resolution against
    /// per-drive current directories, and backslash conversion.
    /// Check if a path refers to an OS/2 reserved device name.
    /// Returns the device name (uppercase) if matched, None otherwise.
    /// Device names are matched case-insensitively, with or without extensions.
    pub fn check_device_name(path: &str) -> Option<&'static str> {
        // Extract the filename component
        let filename = path.rsplit(&['\\', '/'][..]).next().unwrap_or(path);
        // Strip extension if present
        let base = filename.split('.').next().unwrap_or(filename);
        match base.to_ascii_uppercase().as_str() {
            "NUL" => Some("NUL"),
            "CON" => Some("CON"),
            "CLOCK$" => Some("CLOCK$"),
            "KBD$" => Some("KBD$"),
            "SCREEN$" => Some("SCREEN$"),
            _ => None,
        }
    }

    pub fn resolve_path(&self, os2_path: &str) -> VfsResult<(u8, String)> {
        // Reject UNC paths (\\server\share)
        if os2_path.starts_with("\\\\") || os2_path.starts_with("//") {
            return Err(Os2Error::PATH_NOT_FOUND);
        }

        let path = os2_path.replace('\\', "/");

        // Extract drive letter or use current drive
        let (drive, rest) = if path.len() >= 2 && path.as_bytes()[1] == b':' {
            let letter = path.as_bytes()[0].to_ascii_uppercase();
            if !(b'A'..=b'Z').contains(&letter) {
                return Err(Os2Error::INVALID_DRIVE);
            }
            (letter - b'A', &path[2..])
        } else {
            (self.current_disk, path.as_str())
        };

        // Verify drive is mounted
        if self.drives[drive as usize].is_none() {
            return Err(Os2Error::INVALID_DRIVE);
        }

        // Strip leading slash
        let rest = rest.trim_start_matches('/');

        // If relative, prepend per-drive current directory
        let resolved = if rest.is_empty() {
            self.current_dirs[drive as usize].clone()
        } else if os2_path.contains('\\') || os2_path.contains('/') {
            // Check if originally absolute (had drive: + slash or just started with slash)
            let was_absolute = if path.len() >= 2 && path.as_bytes()[1] == b':' {
                path.len() > 2 && path.as_bytes()[2] == b'/'
            } else {
                os2_path.starts_with('\\') || os2_path.starts_with('/')
            };
            if was_absolute {
                rest.to_string()
            } else {
                let cur = &self.current_dirs[drive as usize];
                if cur.is_empty() {
                    rest.to_string()
                } else {
                    format!("{}/{}", cur, rest)
                }
            }
        } else {
            // Simple filename, prepend current directory
            let cur = &self.current_dirs[drive as usize];
            if cur.is_empty() {
                rest.to_string()
            } else {
                format!("{}/{}", cur, rest)
            }
        };

        Ok((drive, resolved))
    }

    // ── File Handle Management ──

    /// Open a file and assign an OS/2 handle.
    /// Returns (os2_handle, open_action).
    pub fn open_file(
        &mut self,
        os2_path: &str,
        mode: OpenMode,
        sharing: SharingMode,
        flags: OpenFlags,
        attributes: FileAttribute,
    ) -> VfsResult<(u32, OpenAction)> {
        // Intercept OS/2 device names — they bypass the filesystem
        if let Some(device) = Self::check_device_name(os2_path) {
            log::debug!("Device name '{}' detected in path '{}'", device, os2_path);
            // NUL device: open /dev/null
            if device == "NUL" {
                // Use HandleManager for device handles (not VFS-backed)
                return Err(Os2Error::INVALID_PARAMETER); // caller should handle NUL specially
            }
            // Other devices (CON, KBD$, SCREEN$) — not filesystem-backed
            return Err(Os2Error::INVALID_PARAMETER);
        }

        let (drive, rel_path) = self.resolve_path(os2_path)?;
        let backend = self.drives[drive as usize].as_ref().ok_or(Os2Error::INVALID_DRIVE)?;
        let (vfs_handle, action) = backend.open(&rel_path, mode, sharing, flags, attributes)?;
        let os2_handle = self.next_file_handle;
        self.next_file_handle += 1;
        self.file_handles.insert(os2_handle, FileEntry { drive, vfs_handle });
        Ok((os2_handle, action))
    }

    /// Close a file handle.
    pub fn close_file(&mut self, handle: u32) -> VfsResult<()> {
        let entry = self.file_handles.remove(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        let backend = self.drives[entry.drive as usize].as_ref().ok_or(Os2Error::INVALID_HANDLE)?;
        backend.close(entry.vfs_handle)
    }

    /// Read from an open file.
    pub fn read_file(&self, handle: u32, buf: &mut [u8]) -> VfsResult<usize> {
        let entry = self.file_handles.get(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        let backend = self.drives[entry.drive as usize].as_ref().ok_or(Os2Error::INVALID_HANDLE)?;
        backend.read(entry.vfs_handle, buf)
    }

    /// Write to an open file.
    pub fn write_file(&self, handle: u32, buf: &[u8]) -> VfsResult<usize> {
        let entry = self.file_handles.get(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        let backend = self.drives[entry.drive as usize].as_ref().ok_or(Os2Error::INVALID_HANDLE)?;
        backend.write(entry.vfs_handle, buf)
    }

    /// Seek within an open file.
    pub fn seek_file(&self, handle: u32, offset: i64, mode: SeekMode) -> VfsResult<u64> {
        let entry = self.file_handles.get(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        let backend = self.drives[entry.drive as usize].as_ref().ok_or(Os2Error::INVALID_HANDLE)?;
        backend.seek(entry.vfs_handle, offset, mode)
    }

    /// Flush buffered writes for a file handle.
    pub fn flush_file(&self, handle: u32) -> VfsResult<()> {
        let entry = self.file_handles.get(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        let backend = self.drives[entry.drive as usize].as_ref().ok_or(Os2Error::INVALID_HANDLE)?;
        backend.flush(entry.vfs_handle)
    }

    /// Set the size of an open file (truncate or extend).
    pub fn set_file_size(&self, handle: u32, size: u64) -> VfsResult<()> {
        let entry = self.file_handles.get(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        let backend = self.drives[entry.drive as usize].as_ref().ok_or(Os2Error::INVALID_HANDLE)?;
        backend.set_file_size(entry.vfs_handle, size)
    }

    /// Apply byte-range locks and unlocks on an open file handle.
    pub fn set_file_locks(&self, handle: u32, unlock: &[FileLockRange], lock: &[FileLockRange], timeout_ms: u32) -> VfsResult<()> {
        let entry = self.file_handles.get(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        let backend = self.drives[entry.drive as usize].as_ref().ok_or(Os2Error::INVALID_HANDLE)?;
        backend.set_file_locks(entry.vfs_handle, unlock, lock, timeout_ms)
    }

    /// Flush all open file handles.
    pub fn flush_all(&self) {
        for entry in self.file_handles.values() {
            if let Some(backend) = self.drives[entry.drive as usize].as_ref() {
                let _ = backend.flush(entry.vfs_handle);
            }
        }
    }

    // ── Find Handle Management ──

    /// Start a directory search and assign an OS/2 handle.
    /// Returns (os2_hdir, first_entry).
    pub fn find_first(
        &mut self,
        os2_path: &str,
        attributes: FileAttribute,
    ) -> VfsResult<(u32, DirEntry)> {
        let (drive, rel_path) = self.resolve_path(os2_path)?;
        let backend = self.drives[drive as usize].as_ref().ok_or(Os2Error::INVALID_DRIVE)?;
        let (vfs_handle, entry) = backend.find_first(&rel_path, attributes)?;
        let os2_handle = self.next_find_handle;
        self.next_find_handle += 1;
        self.find_handles.insert(
            os2_handle,
            FindEntry { drive, vfs_handle, pattern: os2_path.to_string() },
        );
        Ok((os2_handle, entry))
    }

    /// Get the next matching entry from a directory search.
    pub fn find_next(&self, handle: u32) -> VfsResult<DirEntry> {
        let entry = self.find_handles.get(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        let backend = self.drives[entry.drive as usize].as_ref().ok_or(Os2Error::INVALID_HANDLE)?;
        backend.find_next(entry.vfs_handle)
    }

    /// Close a directory search handle.
    pub fn find_close(&mut self, handle: u32) -> VfsResult<()> {
        let entry = self.find_handles.remove(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        let backend = self.drives[entry.drive as usize].as_ref().ok_or(Os2Error::INVALID_HANDLE)?;
        backend.find_close(entry.vfs_handle)
    }

    // ── Directory Operations ──

    /// Create a directory.
    pub fn create_dir(&self, os2_path: &str) -> VfsResult<()> {
        let (drive, rel_path) = self.resolve_path(os2_path)?;
        self.backend(drive)?.create_dir(&rel_path)
    }

    /// Remove an empty directory.
    pub fn delete_dir(&self, os2_path: &str) -> VfsResult<()> {
        let (drive, rel_path) = self.resolve_path(os2_path)?;
        self.backend(drive)?.delete_dir(&rel_path)
    }

    /// Delete a file.
    pub fn delete_file(&self, os2_path: &str) -> VfsResult<()> {
        let (drive, rel_path) = self.resolve_path(os2_path)?;
        self.backend(drive)?.delete(&rel_path)
    }

    /// Rename or move a file within the same drive.
    pub fn rename_file(&self, old_path: &str, new_path: &str) -> VfsResult<()> {
        let (drive_old, rel_old) = self.resolve_path(old_path)?;
        let (drive_new, rel_new) = self.resolve_path(new_path)?;
        if drive_old != drive_new {
            return Err(Os2Error::NOT_SAME_DEVICE);
        }
        self.backend(drive_old)?.rename(&rel_old, &rel_new)
    }

    /// Copy a file (may be cross-drive in the future, for now same drive only).
    pub fn copy_file(&self, src_path: &str, dst_path: &str) -> VfsResult<()> {
        let (drive_src, rel_src) = self.resolve_path(src_path)?;
        let (drive_dst, rel_dst) = self.resolve_path(dst_path)?;
        if drive_src != drive_dst {
            return Err(Os2Error::NOT_SAME_DEVICE);
        }
        self.backend(drive_src)?.copy(&rel_src, &rel_dst)
    }

    // ── Metadata ──

    /// Query file/directory status by path.
    pub fn query_path_info(&self, os2_path: &str, level: u32) -> VfsResult<FileStatus> {
        let (drive, rel_path) = self.resolve_path(os2_path)?;
        self.backend(drive)?.query_path_info(&rel_path, level)
    }

    /// Query file status by open handle.
    pub fn query_file_info(&self, handle: u32, level: u32) -> VfsResult<FileStatus> {
        let entry = self.file_handles.get(&handle).ok_or(Os2Error::INVALID_HANDLE)?;
        self.backend(entry.drive)?.query_file_info(entry.vfs_handle, level)
    }

    // ── Drive/Directory State ──

    /// Get the current drive (0=A, 1=B, 2=C, ...).
    pub fn current_disk(&self) -> u8 {
        self.current_disk
    }

    /// Get the current drive as OS/2 convention (1=A, 2=B, 3=C, ...).
    pub fn current_disk_os2(&self) -> u8 {
        self.current_disk + 1
    }

    /// Set the current drive (OS/2 convention: 1=A, 2=B, 3=C, ...).
    pub fn set_current_disk(&mut self, disk_os2: u8) -> VfsResult<()> {
        if disk_os2 == 0 || disk_os2 > 26 {
            return Err(Os2Error::INVALID_DRIVE);
        }
        let idx = disk_os2 - 1;
        if self.drives[idx as usize].is_none() {
            return Err(Os2Error::INVALID_DRIVE);
        }
        self.current_disk = idx;
        Ok(())
    }

    /// Get the current directory for a drive (0-based index).
    /// Returns the path without drive letter or leading backslash.
    pub fn current_dir(&self, drive: u8) -> &str {
        if drive >= 26 { return ""; }
        &self.current_dirs[drive as usize]
    }

    /// Set the current directory for a drive.
    pub fn set_current_dir(&mut self, os2_path: &str) -> VfsResult<()> {
        let (drive, rel_path) = self.resolve_path(os2_path)?;
        // TODO: verify that the path exists and is a directory via the backend
        self.current_dirs[drive as usize] = rel_path;
        Ok(())
    }

    /// Get a bitmask of mounted drives (bit 0 = A:, bit 1 = B:, ...).
    pub fn logical_drive_map(&self) -> u32 {
        let mut map = 0u32;
        for (i, d) in self.drives.iter().enumerate() {
            if d.is_some() {
                map |= 1 << i;
            }
        }
        map
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_os2_error_constants() {
        assert_eq!(Os2Error::NO_ERROR.0, 0);
        assert_eq!(Os2Error::FILE_NOT_FOUND.0, 2);
        assert_eq!(Os2Error::PATH_NOT_FOUND.0, 3);
        assert_eq!(Os2Error::ACCESS_DENIED.0, 5);
        assert_eq!(Os2Error::INVALID_HANDLE.0, 6);
        assert_eq!(Os2Error::INVALID_DRIVE.0, 15);
        assert_eq!(Os2Error::NO_MORE_FILES.0, 18);
        assert_eq!(Os2Error::SHARING_VIOLATION.0, 32);
        assert_eq!(Os2Error::LOCK_VIOLATION.0, 33);
        assert_eq!(Os2Error::FILE_EXISTS.0, 80);
        assert_eq!(Os2Error::INVALID_PARAMETER.0, 87);
        assert_eq!(Os2Error::BUFFER_OVERFLOW.0, 111);
        assert_eq!(Os2Error::DISK_FULL.0, 112);
        assert_eq!(Os2Error::INVALID_LEVEL.0, 124);
        assert_eq!(Os2Error::FILENAME_EXCED_RANGE.0, 206);
    }

    #[test]
    fn test_os2_error_debug() {
        assert_eq!(format!("{:?}", Os2Error::FILE_NOT_FOUND), "Os2Error(2=FILE_NOT_FOUND)");
        assert_eq!(format!("{:?}", Os2Error(999)), "Os2Error(999)");
    }

    #[test]
    fn test_open_mode_from_raw() {
        assert_eq!(OpenMode::from_raw(0x0000).unwrap(), OpenMode::ReadOnly);
        assert_eq!(OpenMode::from_raw(0x0001).unwrap(), OpenMode::WriteOnly);
        assert_eq!(OpenMode::from_raw(0x0002).unwrap(), OpenMode::ReadWrite);
        assert_eq!(OpenMode::from_raw(0x0042).unwrap(), OpenMode::ReadWrite); // extra bits ignored
        assert!(OpenMode::from_raw(0x0003).is_err());
    }

    #[test]
    fn test_sharing_mode_from_raw() {
        assert_eq!(SharingMode::from_raw(0x10), SharingMode::DenyReadWrite);
        assert_eq!(SharingMode::from_raw(0x20), SharingMode::DenyWrite);
        assert_eq!(SharingMode::from_raw(0x30), SharingMode::DenyRead);
        assert_eq!(SharingMode::from_raw(0x40), SharingMode::DenyNone);
        assert_eq!(SharingMode::from_raw(0x00), SharingMode::DenyNone); // default
        assert_eq!(SharingMode::from_raw(0x12), SharingMode::DenyReadWrite); // with access bits
    }

    #[test]
    fn test_open_flags_from_raw() {
        // 0x0012 = CREATE_IF_NEW | REPLACE_IF_EXISTS
        let flags = OpenFlags::from_raw(0x0012);
        assert_eq!(flags.exist_action, ExistAction::Replace);
        assert_eq!(flags.new_action, NewAction::Create);

        // 0x0001 = OPEN_IF_EXISTS
        let flags = OpenFlags::from_raw(0x0001);
        assert_eq!(flags.exist_action, ExistAction::Open);
        assert_eq!(flags.new_action, NewAction::Fail);

        // 0x0011 = OPEN_IF_EXISTS | CREATE_IF_NEW
        let flags = OpenFlags::from_raw(0x0011);
        assert_eq!(flags.exist_action, ExistAction::Open);
        assert_eq!(flags.new_action, NewAction::Create);
    }

    #[test]
    fn test_seek_mode_from_raw() {
        assert_eq!(SeekMode::from_raw(0).unwrap(), SeekMode::Begin);
        assert_eq!(SeekMode::from_raw(1).unwrap(), SeekMode::Current);
        assert_eq!(SeekMode::from_raw(2).unwrap(), SeekMode::End);
        assert!(SeekMode::from_raw(3).is_err());
    }

    #[test]
    fn test_file_attribute_contains() {
        let attr = FileAttribute(0x21); // READONLY | ARCHIVE
        assert!(attr.contains(FileAttribute::READONLY));
        assert!(attr.contains(FileAttribute::ARCHIVE));
        assert!(!attr.contains(FileAttribute::HIDDEN));
        assert!(!attr.contains(FileAttribute::DIRECTORY));
    }

    // ── Mock backend for DriveManager tests ──

    /// Minimal mock backend that tracks open/close calls.
    struct MockBackend;

    impl VfsBackend for MockBackend {
        fn open(&self, _path: &str, _mode: OpenMode, _sharing: SharingMode,
                _flags: OpenFlags, _attrs: FileAttribute) -> VfsResult<(VfsFileHandle, OpenAction)> {
            Ok((VfsFileHandle(42), OpenAction::Created))
        }
        fn close(&self, _handle: VfsFileHandle) -> VfsResult<()> { Ok(()) }
        fn read(&self, _handle: VfsFileHandle, _buf: &mut [u8]) -> VfsResult<usize> { Ok(0) }
        fn write(&self, _handle: VfsFileHandle, _buf: &[u8]) -> VfsResult<usize> { Ok(0) }
        fn seek(&self, _handle: VfsFileHandle, _offset: i64, _mode: SeekMode) -> VfsResult<u64> { Ok(0) }
        fn set_file_size(&self, _handle: VfsFileHandle, _size: u64) -> VfsResult<()> { Ok(()) }
        fn flush(&self, _handle: VfsFileHandle) -> VfsResult<()> { Ok(()) }
        fn find_first(&self, _pattern: &str, _attrs: FileAttribute) -> VfsResult<(VfsFindHandle, DirEntry)> {
            Err(Os2Error::NO_MORE_FILES)
        }
        fn find_next(&self, _handle: VfsFindHandle) -> VfsResult<DirEntry> { Err(Os2Error::NO_MORE_FILES) }
        fn find_close(&self, _handle: VfsFindHandle) -> VfsResult<()> { Ok(()) }
        fn create_dir(&self, _path: &str) -> VfsResult<()> { Ok(()) }
        fn delete_dir(&self, _path: &str) -> VfsResult<()> { Ok(()) }
        fn delete(&self, _path: &str) -> VfsResult<()> { Ok(()) }
        fn rename(&self, _old: &str, _new: &str) -> VfsResult<()> { Ok(()) }
        fn copy(&self, _src: &str, _dst: &str) -> VfsResult<()> { Ok(()) }
        fn query_path_info(&self, _path: &str, _level: u32) -> VfsResult<FileStatus> {
            Err(Os2Error::FILE_NOT_FOUND)
        }
        fn query_file_info(&self, _handle: VfsFileHandle, _level: u32) -> VfsResult<FileStatus> {
            Err(Os2Error::INVALID_HANDLE)
        }
        fn set_file_info(&self, _handle: VfsFileHandle, _level: u32, _info: &FileStatus) -> VfsResult<()> {
            Ok(())
        }
        fn set_path_info(&self, _path: &str, _level: u32, _info: &FileStatus) -> VfsResult<()> {
            Ok(())
        }
        fn get_ea(&self, _path: &str, _name: &str) -> VfsResult<EaEntry> { Err(Os2Error::EA_NOT_FOUND) }
        fn set_ea(&self, _path: &str, _ea: &EaEntry) -> VfsResult<()> { Ok(()) }
        fn enum_ea(&self, _path: &str) -> VfsResult<Vec<EaEntry>> { Ok(Vec::new()) }
        fn query_fs_info_alloc(&self) -> VfsResult<FsAllocate> {
            Ok(FsAllocate { id_filesystem: 0, sectors_per_unit: 1, total_units: 1000, available_units: 500, bytes_per_sector: 512 })
        }
        fn query_fs_info_volume(&self) -> VfsResult<FsVolumeInfo> {
            Ok(FsVolumeInfo { serial_number: 0x12345678, label: "MOCK".to_string() })
        }
        fn set_fs_info_volume(&self, _label: &str) -> VfsResult<()> { Ok(()) }
        fn fs_name(&self) -> &str { "HPFS" }
        fn set_file_locks(&self, _handle: VfsFileHandle, _unlock: &[FileLockRange],
                          _lock: &[FileLockRange], _timeout_ms: u32) -> VfsResult<()> { Ok(()) }
    }

    #[test]
    fn test_drive_manager_new() {
        let dm = DriveManager::new();
        assert_eq!(dm.current_disk(), 2); // C:
        assert_eq!(dm.current_disk_os2(), 3);
        assert_eq!(dm.logical_drive_map(), 0);
        assert!(dm.drive_config(2).is_none()); // no config set
    }

    #[test]
    fn test_drive_manager_with_default_config() {
        let dm = DriveManager::with_default_config();
        assert_eq!(dm.current_disk(), 2); // C:

        // C: should have a config
        let config = dm.drive_config(2).expect("C: config should exist");
        assert!(config.host_path.ends_with("warpine/drive_c"),
                "C: should map to warpine/drive_c, got {:?}", config.host_path);
        assert_eq!(config.label, "OS2");
        assert!(!config.read_only);

        // D: should not have a config
        assert!(dm.drive_config(3).is_none());
    }

    #[test]
    fn test_drive_config_set_and_get() {
        let mut dm = DriveManager::new();
        dm.set_drive_config(3, DriveConfig {
            host_path: PathBuf::from("/tmp/os2_d"),
            label: "DATA".to_string(),
            read_only: true,
        });
        let config = dm.drive_config(3).unwrap();
        assert_eq!(config.host_path, PathBuf::from("/tmp/os2_d"));
        assert_eq!(config.label, "DATA");
        assert!(config.read_only);
    }

    #[test]
    fn test_drive_manager_mount() {
        let mut dm = DriveManager::new();
        dm.mount(2, Box::new(MockBackend)); // C:
        assert!(dm.backend(2).is_ok());
        assert!(dm.backend(3).is_err()); // D: not mounted
        assert_eq!(dm.logical_drive_map(), 0b100); // bit 2 = C:
    }

    #[test]
    fn test_drive_manager_resolve_absolute() {
        let mut dm = DriveManager::new();
        dm.mount(2, Box::new(MockBackend)); // C:

        let (drive, path) = dm.resolve_path("C:\\DIR\\FILE.TXT").unwrap();
        assert_eq!(drive, 2);
        assert_eq!(path, "DIR/FILE.TXT");
    }

    #[test]
    fn test_drive_manager_resolve_relative() {
        let mut dm = DriveManager::new();
        dm.mount(2, Box::new(MockBackend)); // C:
        dm.current_dirs[2] = "WORK".to_string();

        let (drive, path) = dm.resolve_path("test.txt").unwrap();
        assert_eq!(drive, 2);
        assert_eq!(path, "WORK/test.txt");
    }

    #[test]
    fn test_drive_manager_resolve_relative_empty_cwd() {
        let mut dm = DriveManager::new();
        dm.mount(2, Box::new(MockBackend)); // C:

        let (drive, path) = dm.resolve_path("test.txt").unwrap();
        assert_eq!(drive, 2);
        assert_eq!(path, "test.txt");
    }

    #[test]
    fn test_drive_manager_resolve_unmounted_drive() {
        let dm = DriveManager::new();
        let result = dm.resolve_path("D:\\FILE.TXT");
        assert_eq!(result.unwrap_err(), Os2Error::INVALID_DRIVE);
    }

    #[test]
    fn test_drive_manager_reject_unc_paths() {
        let mut dm = DriveManager::new();
        dm.mount(2, Box::new(MockBackend)); // C:

        // UNC paths with backslashes
        assert_eq!(dm.resolve_path("\\\\server\\share\\file").unwrap_err(), Os2Error::PATH_NOT_FOUND);
        // UNC paths with forward slashes
        assert_eq!(dm.resolve_path("//server/share/file").unwrap_err(), Os2Error::PATH_NOT_FOUND);
    }

    #[test]
    fn test_device_name_detection() {
        assert_eq!(DriveManager::check_device_name("NUL"), Some("NUL"));
        assert_eq!(DriveManager::check_device_name("nul"), Some("NUL"));
        assert_eq!(DriveManager::check_device_name("NUL.TXT"), Some("NUL"));
        assert_eq!(DriveManager::check_device_name("C:\\NUL"), Some("NUL"));
        assert_eq!(DriveManager::check_device_name("CON"), Some("CON"));
        assert_eq!(DriveManager::check_device_name("con"), Some("CON"));
        assert_eq!(DriveManager::check_device_name("CLOCK$"), Some("CLOCK$"));
        assert_eq!(DriveManager::check_device_name("KBD$"), Some("KBD$"));
        assert_eq!(DriveManager::check_device_name("SCREEN$"), Some("SCREEN$"));
        assert_eq!(DriveManager::check_device_name("README.TXT"), None);
        assert_eq!(DriveManager::check_device_name("NULLIFY"), None);
    }

    #[test]
    fn test_drive_manager_file_handles() {
        let mut dm = DriveManager::new();
        dm.mount(2, Box::new(MockBackend)); // C:

        let (h1, action) = dm.open_file(
            "test.txt", OpenMode::ReadWrite, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0012), FileAttribute::NORMAL,
        ).unwrap();
        assert_eq!(h1, 3); // first handle after stdin/stdout/stderr
        assert_eq!(action, OpenAction::Created);

        let (h2, _) = dm.open_file(
            "test2.txt", OpenMode::ReadOnly, SharingMode::DenyNone,
            OpenFlags::from_raw(0x0001), FileAttribute::NORMAL,
        ).unwrap();
        assert_eq!(h2, 4); // next handle

        assert!(dm.close_file(h1).is_ok());
        assert!(dm.close_file(h2).is_ok());
        assert_eq!(dm.close_file(h1).unwrap_err(), Os2Error::INVALID_HANDLE); // already closed
    }

    #[test]
    fn test_drive_manager_find_handles() {
        let mut dm = DriveManager::new();
        dm.mount(2, Box::new(MockBackend)); // C:

        // MockBackend::find_first returns NO_MORE_FILES
        let result = dm.find_first("C:\\*.*", FileAttribute::NORMAL);
        assert_eq!(result.unwrap_err(), Os2Error::NO_MORE_FILES);
    }

    #[test]
    fn test_drive_manager_set_current_disk() {
        let mut dm = DriveManager::new();
        dm.mount(2, Box::new(MockBackend)); // C:
        dm.mount(3, Box::new(MockBackend)); // D:

        assert!(dm.set_current_disk(4).is_ok()); // D: (OS/2 convention: 4)
        assert_eq!(dm.current_disk(), 3);
        assert_eq!(dm.current_disk_os2(), 4);

        assert_eq!(dm.set_current_disk(5).unwrap_err(), Os2Error::INVALID_DRIVE); // E: not mounted
        assert_eq!(dm.set_current_disk(0).unwrap_err(), Os2Error::INVALID_DRIVE); // invalid
    }

    #[test]
    fn test_drive_manager_per_drive_current_dir() {
        let mut dm = DriveManager::new();
        dm.mount(2, Box::new(MockBackend)); // C:
        dm.mount(3, Box::new(MockBackend)); // D:

        dm.set_current_dir("C:\\WORK").unwrap();
        dm.set_current_dir("D:\\GAMES").unwrap();

        assert_eq!(dm.current_dir(2), "WORK");
        assert_eq!(dm.current_dir(3), "GAMES");

        // Resolve relative path uses per-drive current dir
        let (_, path) = dm.resolve_path("C:file.txt").unwrap();
        assert_eq!(path, "WORK/file.txt");

        dm.set_current_disk(4).unwrap(); // switch to D:
        let (_, path) = dm.resolve_path("readme.txt").unwrap();
        assert_eq!(path, "GAMES/readme.txt");
    }
}
