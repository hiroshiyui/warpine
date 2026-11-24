// SPDX-License-Identifier: GPL-3.0-or-later
//
// PM profiles stored through the VFS: loading and saving Warpine's text
// format (`profile_text.rs`, re-exported here: the model and the format,
// pure, so the fuzz target `profile` includes it alone).

use super::vfs::{DriveManager, ExistAction, FileAttribute, NewAction, OpenFlags, OpenMode,
                 SharingMode, VfsBackend};

pub use super::profile_text::*;

// ── Storage through the VFS ──────────────────────────────────────────────────

/// Why a profile could not be loaded or saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The path is not on a mounted drive or the file cannot be read.
    Open,
    /// The file is not a Warpine profile.
    Corrupt(ParseError),
    /// The file could not be written.
    Write,
}

/// Suffix of the temporary file a profile is written to before it replaces
/// the profile (`rename` is atomic on the host).
const TEMP_SUFFIX: &str = ".~wp";

/// Read a whole file of at most [`MAX_PROFILE_FILE`] bytes; `Ok(None)` when
/// it does not exist.
fn read_file(backend: &dyn VfsBackend, rel: &str) -> Result<Option<Vec<u8>>, StoreError> {
    let flags = OpenFlags { exist_action: ExistAction::Open, new_action: NewAction::Fail };
    let handle = match backend.open(rel, OpenMode::ReadOnly, SharingMode::DenyNone, flags,
                                    FileAttribute::NORMAL) {
        Ok((h, _)) => h,
        Err(e) if e == super::vfs::Os2Error::FILE_NOT_FOUND => return Ok(None),
        Err(_) => return Err(StoreError::Open),
    };
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    let result = loop {
        match backend.read(handle, &mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) if data.len() + n <= MAX_PROFILE_FILE => data.extend_from_slice(&buf[..n]),
            _ => break Err(StoreError::Open),
        }
    };
    let _ = backend.close(handle);
    result.map(|()| Some(data))
}

/// Replace file `rel` with `data`: write a temporary file next to it, then
/// rename it over the file.
fn write_file_atomic(backend: &dyn VfsBackend, rel: &str, data: &[u8]) -> Result<(), StoreError> {
    let tmp = format!("{rel}{TEMP_SUFFIX}");
    let flags = OpenFlags { exist_action: ExistAction::Replace, new_action: NewAction::Create };
    let (handle, _) = backend.open(&tmp, OpenMode::WriteOnly, SharingMode::DenyReadWrite, flags,
                                   FileAttribute::NORMAL).map_err(|_| StoreError::Write)?;
    let mut written = 0;
    let mut ok = true;
    while written < data.len() {
        match backend.write(handle, &data[written..]) {
            Ok(0) | Err(_) => { ok = false; break; }
            Ok(n) => written += n,
        }
    }
    ok &= backend.flush(handle).is_ok();
    ok &= backend.close(handle).is_ok();
    if ok && backend.rename(&tmp, rel).is_ok() {
        return Ok(());
    }
    let _ = backend.delete(&tmp);
    Err(StoreError::Write)
}

/// The backend and volume-relative path of OS/2 path `os2_path`.
fn locate<'a>(dm: &'a DriveManager, os2_path: &str) -> Result<(&'a dyn VfsBackend, String), StoreError> {
    let (drive, rel) = dm.resolve_path(os2_path).map_err(|_| StoreError::Open)?;
    let backend = dm.backend(drive).map_err(|_| StoreError::Open)?;
    Ok((backend, rel))
}

/// Load the profile at OS/2 path `os2_path`; a missing file is an empty
/// profile.
pub fn load(dm: &DriveManager, os2_path: &str) -> Result<Profile, StoreError> {
    let (backend, rel) = locate(dm, os2_path)?;
    match read_file(backend, &rel)? {
        None => Ok(Profile::default()),
        Some(text) => Profile::parse(&text).map_err(StoreError::Corrupt),
    }
}

/// Whether a file exists at OS/2 path `os2_path`.
pub fn exists(dm: &DriveManager, os2_path: &str) -> bool {
    locate(dm, os2_path).is_ok_and(|(backend, rel)| backend.query_path_info(&rel, 1).is_ok())
}

/// Save `profile` at OS/2 path `os2_path`, atomically. With `make_parent`,
/// the parent directory is created first when missing (`C:\OS2`).
pub fn save(dm: &DriveManager, os2_path: &str, profile: &Profile, make_parent: bool)
    -> Result<(), StoreError>
{
    let (backend, rel) = locate(dm, os2_path)?;
    if make_parent
        && let Some(pos) = rel.rfind('/')
        && backend.query_path_info(&rel[..pos], 1).is_err()
    {
        let _ = backend.create_dir(&rel[..pos]);
    }
    write_file_atomic(backend, &rel, &profile.serialize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::vfs::DriveConfig;
    use crate::loader::vfs_hostdir::HostDirBackend;

    fn sample() -> Profile {
        let mut p = Profile::default();
        p.set(b"App", b"Key", b"value\0");
        p.set(b"App", b"Blob", &[0, 1, 0xFF, b'"', b'\\', b'\n']);
        p.set(b"Other \"quoted\"", b"k", b"");
        p
    }

    fn drive(dir: &std::path::Path) -> DriveManager {
        let mut dm = DriveManager::new();
        dm.set_drive_config(2, DriveConfig { host_path: dir.to_path_buf(), label: "T".into(),
                                             read_only: false });
        dm.mount(2, Box::new(HostDirBackend::new(dir.to_path_buf()).unwrap()));
        dm
    }

    #[test]
    fn save_and_load_through_the_vfs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dm = drive(tmp.path());
        assert!(!exists(&dm, "C:\\OS2\\OS2.INI"));
        assert_eq!(load(&dm, "C:\\OS2\\OS2.INI").unwrap(), Profile::default());
        let p = sample();
        assert_eq!(save(&dm, "C:\\OS2\\OS2.INI", &p, false), Err(StoreError::Write),
                   "no parent directory");
        save(&dm, "C:\\OS2\\OS2.INI", &p, true).unwrap();
        assert!(exists(&dm, "C:\\os2\\os2.ini"));
        assert_eq!(load(&dm, "c:\\os2\\OS2.INI").unwrap(), p);
        // Only the profile is left: the temporary file was renamed.
        let names: Vec<_> = std::fs::read_dir(tmp.path().join("OS2")).unwrap()
            .flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("OS2.INI")]);
        // Saving again replaces the file.
        save(&dm, "C:\\OS2\\OS2.INI", &Profile::default(), true).unwrap();
        assert_eq!(load(&dm, "C:\\OS2\\OS2.INI").unwrap(), Profile::default());
    }

    #[test]
    fn load_reports_corrupt_files_and_unmounted_drives() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("BIN.INI"), b"\xFF\xFF\xFF\xFF").unwrap();
        let dm = drive(tmp.path());
        assert!(matches!(load(&dm, "C:\\BIN.INI"), Err(StoreError::Corrupt(_))));
        assert_eq!(load(&dm, "Q:\\X.INI"), Err(StoreError::Open));
        assert_eq!(save(&dm, "Q:\\X.INI", &Profile::default(), true), Err(StoreError::Open));
    }

    #[test]
    fn paths_cannot_leave_the_drive() {
        let tmp = tempfile::TempDir::new().unwrap();
        let inner = tmp.path().join("c");
        std::fs::create_dir(&inner).unwrap();
        let dm = drive(&inner);
        // `..` stops at the drive's root.
        let _ = save(&dm, "C:\\..\\ESCAPE.INI", &sample(), false);
        assert!(!tmp.path().join("ESCAPE.INI").exists());
        assert_eq!(load(&dm, "C:\\..\\..\\ESCAPE.INI").unwrap(), sample());
    }
}
