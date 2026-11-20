// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Hidden, system and archive, kept beside a host file (ADR-0091).
//!
//! Linux has no counterpart of OS/2's `FILE_HIDDEN`, `FILE_SYSTEM` and
//! `FILE_ARCHIVED`, so the host-directory VFS (`vfs_hostdir.rs`) keeps
//! them itself, as one byte:
//! - in the extended attribute `user.warpine.attr` where the filesystem
//!   has extended attributes (outside the `user.os2.ea.` names that hold
//!   OS/2 EAs, so `DosEnumAttribute` does not see it);
//! - else in the sidecar file `.os2ea/<name>.attr`, next to the EA sidecar
//!   `.os2ea/<name>.ea`.
//!
//! Nothing stored means the default: archived for a file, nothing for a
//! directory, as before. Storing the default removes the byte.
//! `FILE_READONLY` stays the host's write permission bits, and
//! `FILE_DIRECTORY` the file's kind.

use std::ffi::CString;
use std::path::{Path, PathBuf};

use super::vfs::FileAttribute;

/// The attributes kept here.
pub const KEPT: u32 = FileAttribute::HIDDEN.0 | FileAttribute::SYSTEM.0 | FileAttribute::ARCHIVE.0;
/// The extended attribute that holds them.
pub const ATTR_XATTR: &str = "user.warpine.attr";
/// The folder of sidecar files (EAs and attributes) where there are no
/// extended attributes; never shown to OS/2 programs.
pub const SIDECAR_DIR: &str = ".os2ea";

/// The kept attributes of something with no byte stored.
pub fn default_kept(is_dir: bool) -> u32 {
    if is_dir { 0 } else { FileAttribute::ARCHIVE.0 }
}

/// The sidecar of `host`'s attributes: `.os2ea/<name>.attr` beside it.
pub fn sidecar_path(host: &Path) -> Option<PathBuf> {
    let mut name = host.file_name()?.to_os_string();
    name.push(".attr");
    Some(host.parent()?.join(SIDECAR_DIR).join(name))
}

/// The byte stored for `host`, masked to [`KEPT`]; `None` when there is
/// none (or it is not one byte).
pub fn read(host: &Path, xattr: bool) -> Option<u32> {
    let byte = if xattr {
        let c_path = CString::new(host.as_os_str().as_encoded_bytes()).ok()?;
        let c_name = CString::new(ATTR_XATTR).ok()?;
        let mut buf = [0u8; 2];
        // SAFETY: both strings are NUL-terminated; `buf` is writable for
        // its length, which is passed. The result is the value's length.
        let n = unsafe {
            libc::getxattr(c_path.as_ptr(), c_name.as_ptr(), buf.as_mut_ptr().cast(), buf.len())
        };
        if n != 1 { return None; }
        buf[0]
    } else {
        let data = std::fs::read(sidecar_path(host)?).ok()?;
        if data.len() != 1 { return None; }
        data[0]
    };
    Some(u32::from(byte) & KEPT)
}

/// Keep `attrs` (masked to [`KEPT`]) for `host`; the default removes the
/// byte. Errors when it cannot be stored.
pub fn write(host: &Path, xattr: bool, is_dir: bool, attrs: u32) -> std::io::Result<()> {
    let attrs = attrs & KEPT;
    let keep = attrs != default_kept(is_dir);
    if xattr {
        let c_path = CString::new(host.as_os_str().as_encoded_bytes())?;
        let c_name = CString::new(ATTR_XATTR)?;
        let rc = if keep {
            let byte = [attrs as u8];
            // SAFETY: NUL-terminated strings; `byte` is readable for its
            // length, which is passed.
            unsafe { libc::setxattr(c_path.as_ptr(), c_name.as_ptr(), byte.as_ptr().cast(), 1, 0) }
        } else {
            // SAFETY: NUL-terminated strings.
            unsafe { libc::removexattr(c_path.as_ptr(), c_name.as_ptr()) }
        };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            if keep || e.raw_os_error() != Some(libc::ENODATA) { return Err(e); }
        }
        return Ok(());
    }
    let side = sidecar_path(host).ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if keep {
        if let Some(dir) = side.parent() { std::fs::create_dir_all(dir)?; }
        std::fs::write(&side, [attrs as u8])
    } else {
        match std::fs::remove_file(&side) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// `host_attrs` (from the host's metadata) with the kept attributes put
/// in: the stored byte, or the default.
pub fn apply(host_attrs: u32, stored: Option<u32>, is_dir: bool) -> u32 {
    (host_attrs & !KEPT) | stored.unwrap_or_else(|| default_kept(is_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u32 = FileAttribute::HIDDEN.0;
    const S: u32 = FileAttribute::SYSTEM.0;
    const A: u32 = FileAttribute::ARCHIVE.0;
    const R: u32 = FileAttribute::READONLY.0;

    #[test]
    fn defaults_and_application() {
        assert_eq!(apply(R | A, None, false), R | A, "a file: archived");
        assert_eq!(apply(FileAttribute::DIRECTORY.0, None, true), FileAttribute::DIRECTORY.0);
        assert_eq!(apply(R | A, Some(H), false), R | H, "the stored byte replaces the default");
        assert_eq!(apply(0, Some(H | S | A), false), H | S | A);
    }

    /// Both stores keep the byte, remove it for the default, and read
    /// nothing malformed.
    #[test]
    fn the_byte_is_kept_and_removed() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f.txt");
        std::fs::write(&f, b"x").unwrap();
        // The sidecar.
        assert_eq!(read(&f, false), None);
        write(&f, false, false, H | S | R).unwrap();
        assert_eq!(read(&f, false), Some(H | S), "read-only is not kept here");
        assert!(sidecar_path(&f).unwrap().exists());
        write(&f, false, false, A).unwrap();
        assert_eq!(read(&f, false), None, "the default removes it");
        assert!(!sidecar_path(&f).unwrap().exists());
        std::fs::create_dir_all(d.path().join(SIDECAR_DIR)).unwrap();
        std::fs::write(sidecar_path(&f).unwrap(), [H as u8, 0]).unwrap();
        assert_eq!(read(&f, false), None, "two bytes: malformed");
        std::fs::write(sidecar_path(&f).unwrap(), [0xFF]).unwrap();
        assert_eq!(read(&f, false), Some(KEPT), "masked");
        // The extended attribute, where the filesystem has them.
        if write(&f, true, false, H).is_ok() {
            assert_eq!(read(&f, true), Some(H));
            write(&f, true, false, A).unwrap();
            assert_eq!(read(&f, true), None);
            write(&f, true, false, A).unwrap();
        }
        // A directory's default is nothing.
        write(d.path(), false, true, 0).unwrap();
        assert_eq!(read(d.path(), false), None);
    }
}
