// SPDX-License-Identifier: GPL-3.0-or-later
//
// Files for REXX (ADR-0084, ADR-0085): the stream functions' and
// REXXUTIL's file work, by OS/2 name, through the VFS, so it stays inside
// the drives. Both hosts of the interpreter use it: the built-in CMD.EXE
// (`cmd/rexx_host.rs`) and a program's RexxStart (`rexxapi/host.rs`).

use super::codepage::{cp_decode, cp_encode};
use super::mutex_ext::MutexExt;
use super::rexx::interp::{FileInfo, OpenFor, ERROR_INVALID_HANDLE};
use super::vfs::{ExistAction, FileAttribute, FileStatus, NewAction, OpenFlags, OpenMode, Os2Error, SeekMode, SharingMode};
use super::SharedState;

/// The attributes a search matches: files and directories, hidden and
/// system ones too.
const ATTR_ALL_ENTRIES: u32 = 0x37;

/// The process's drives, for an interpreter; names in `codepage`.
pub struct VfsFiles<'a> {
    pub shared: &'a SharedState,
    pub codepage: u32,
}

/// A VFS file status as the interpreter's description of an entry.
fn file_info(name: Vec<u8>, status: &FileStatus) -> FileInfo {
    FileInfo {
        name, size: status.file_size as u64, date: status.last_write_date, time: status.last_write_time,
        attributes: status.attributes.0,
    }
}

impl VfsFiles<'_> {
    /// A name from the script (its codepage) as an OS/2 path.
    fn os2(&self, name: &[u8]) -> String { cp_decode(name, self.codepage) }

    pub fn full_name(&self, name: &[u8]) -> Option<Vec<u8>> {
        let (drive, rel) = self.shared.drive_mgr.lock_or_recover().resolve_path(&self.os2(name)).ok()?;
        let rel = super::vfs::normalize_rel_path(&rel)?;
        Some(cp_encode(&format!("{}:\\{}", (b'A' + drive) as char, rel.replace('/', "\\")), self.codepage))
    }

    pub fn file_open(&self, name: &[u8], mode: OpenFor) -> Result<u32, u32> {
        let (access, flags) = match mode {
            OpenFor::Read => (OpenMode::ReadOnly, OpenFlags { exist_action: ExistAction::Open, new_action: NewAction::Fail }),
            OpenFor::Write => (OpenMode::ReadWrite, OpenFlags { exist_action: ExistAction::Open, new_action: NewAction::Create }),
            OpenFor::Replace => (OpenMode::ReadWrite, OpenFlags { exist_action: ExistAction::Replace, new_action: NewAction::Create }),
        };
        let path = self.os2(name);
        self.shared.drive_mgr.lock_or_recover()
            .open_file(&path, access, SharingMode::DenyNone, flags, FileAttribute::NORMAL)
            .map(|(h, _)| h).map_err(|e| e.0)
    }

    pub fn file_read(&self, handle: u32, pos: u64, buf: &mut [u8]) -> Result<usize, u32> {
        let dm = self.shared.drive_mgr.lock_or_recover();
        if !dm.is_file(handle) { return Err(ERROR_INVALID_HANDLE); }
        dm.seek_file(handle, pos as i64, SeekMode::Begin).map_err(|e| e.0)?;
        dm.read_file(handle, buf).map_err(|e| e.0)
    }

    pub fn file_write(&self, handle: u32, pos: u64, data: &[u8]) -> Result<(), u32> {
        let dm = self.shared.drive_mgr.lock_or_recover();
        if !dm.is_file(handle) { return Err(ERROR_INVALID_HANDLE); }
        dm.seek_file(handle, pos as i64, SeekMode::Begin).map_err(|e| e.0)?;
        let mut done = 0;
        while done < data.len() {
            match dm.write_file(handle, &data[done..]) {
                Ok(0) => return Err(Os2Error::DISK_FULL.0),
                Ok(n) => done += n,
                Err(e) => return Err(e.0),
            }
        }
        Ok(())
    }

    pub fn file_size(&self, handle: u32) -> Result<u64, u32> {
        let dm = self.shared.drive_mgr.lock_or_recover();
        if !dm.is_file(handle) { return Err(ERROR_INVALID_HANDLE); }
        dm.query_file_info(handle, 1).map(|s| s.file_size as u64).map_err(|e| e.0)
    }

    pub fn file_close(&self, handle: u32) {
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        if dm.is_file(handle) { let _ = dm.close_file(handle); }
    }

    pub fn file_info(&self, name: &[u8]) -> Option<FileInfo> {
        let status = self.shared.drive_mgr.lock_or_recover().query_path_info(&self.os2(name), 1).ok()?;
        Some(file_info(self.full_name(name)?, &status))
    }

    pub fn file_find(&self, pattern: &[u8], limit: usize) -> Result<Vec<FileInfo>, u32> {
        let pattern = self.os2(pattern);
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        let (handle, first) = dm.find_first(&pattern, FileAttribute(ATTR_ALL_ENTRIES), 1).map_err(|e| e.0)?;
        let mut found = Vec::new();
        let mut entry = Some(first);
        while let Some(e) = entry {
            if found.len() >= limit { break; }
            if e.name != "." && e.name != ".." { found.push(file_info(cp_encode(&e.name, self.codepage), &e.status)); }
            entry = dm.find_next(handle).ok();
        }
        let _ = dm.find_close(handle);
        Ok(found)
    }

    pub fn file_delete(&self, name: &[u8]) -> u32 {
        let path = self.os2(name);
        self.shared.drive_mgr.lock_or_recover().delete_file(&path).err().map_or(0, |e| e.0)
    }

    pub fn dir_make(&self, name: &[u8]) -> u32 {
        let path = self.os2(name);
        self.shared.drive_mgr.lock_or_recover().create_dir(&path).err().map_or(0, |e| e.0)
    }

    pub fn dir_remove(&self, name: &[u8]) -> u32 {
        let path = self.os2(name);
        self.shared.drive_mgr.lock_or_recover().delete_dir(&path).err().map_or(0, |e| e.0)
    }

    pub fn drives(&self) -> Vec<u8> {
        let map = self.shared.drive_mgr.lock_or_recover().logical_drive_map();
        (0..26u8).filter(|d| map & (1 << d) != 0).map(|d| b'A' + d).collect()
    }
}
