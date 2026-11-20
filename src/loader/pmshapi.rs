// SPDX-License-Identifier: GPL-3.0-or-later
//
// PMSHAPI profile calls (`Prf*`): PrfOpenProfile / PrfCloseProfile,
// PrfQueryProfileInt / String / Size / Data and PrfWriteProfileString /
// Data, after the IBM PM Reference.
//
// The user profile is `C:\OS2\OS2.INI`, the system profile
// `C:\OS2\OS2SYS.INI`; other profiles are the files the program names,
// found through the VFS like `DosOpen`'s. They are stored in Warpine's own
// text format (`profile.rs`). Every call reads the file again and every
// write replaces it atomically, so processes sharing a profile see each
// other's changes; the table lock serialises the threads of one process.

use std::collections::HashMap;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::profile::{self, Profile, StoreError, MAX_PROFILE_VALUE};
use super::vfs::DriveManager;

/// OS/2 path of the user profile (`HINI_USERPROFILE`).
pub const USER_PROFILE_PATH: &str = "C:\\OS2\\OS2.INI";
/// OS/2 path of the system profile (`HINI_SYSTEMPROFILE`).
pub const SYSTEM_PROFILE_PATH: &str = "C:\\OS2\\OS2SYS.INI";

/// First handle `PrfOpenProfile` returns (clear of the `HINI_*` values).
const FIRST_PROFILE_HANDLE: u32 = 0x0001_0000;

/// A profile the process opened with `PrfOpenProfile`.
struct OpenProfile {
    /// Absolute OS/2 path (`X:\DIR\NAME.INI`), independent of later
    /// changes of the current directory.
    path: String,
    /// `(drive, lower-case volume path)`: the file's identity.
    id: (u8, String),
    /// Open count: opening an open file again returns its handle.
    refs: u32,
}

/// Profiles opened by the process (`SharedState::profiles`). Its lock is
/// taken before `drive_mgr`.
pub struct ProfileTable {
    open: HashMap<u32, OpenProfile>,
    next: u32,
}

impl Default for ProfileTable {
    fn default() -> Self {
        ProfileTable { open: HashMap::new(), next: FIRST_PROFILE_HANDLE }
    }
}

/// Why a `Prf*` call failed: the `PMERR_*` code for `WinGetLastError`.
type PrfResult<T> = Result<T, u32>;

/// The identity of OS/2 path `os2_path` and its absolute form.
fn file_id(dm: &DriveManager, os2_path: &str) -> Option<((u8, String), String)> {
    let (drive, rel) = dm.resolve_path(os2_path).ok()?;
    let abs = format!("{}:\\{}", (b'A' + drive) as char, rel.replace('/', "\\"));
    Some(((drive, rel.to_ascii_lowercase()), abs))
}

fn store_error(e: StoreError) -> u32 {
    match e {
        StoreError::Open => PMERR_OPENING_INI_FILE,
        StoreError::Corrupt(_) => PMERR_INI_FILE_CORRUPT,
        StoreError::Write => PMERR_INI_WRITE_FAIL,
    }
}

impl ProfileTable {
    /// The files `hini` reads from, in search order.
    fn read_paths(&self, hini: u32) -> PrfResult<Vec<String>> {
        match hini {
            HINI_PROFILE => Ok(vec![USER_PROFILE_PATH.into(), SYSTEM_PROFILE_PATH.into()]),
            HINI_USERPROFILE => Ok(vec![USER_PROFILE_PATH.into()]),
            HINI_SYSTEMPROFILE => Ok(vec![SYSTEM_PROFILE_PATH.into()]),
            h => self.open.get(&h).map(|p| vec![p.path.clone()]).ok_or(PMERR_INVALID_INI_FILE_HANDLE),
        }
    }

    /// The file `hini` writes to: `HINI_PROFILE` writes the user profile.
    fn write_path(&self, hini: u32) -> PrfResult<String> {
        Ok(match hini {
            HINI_PROFILE | HINI_USERPROFILE => USER_PROFILE_PATH.into(),
            HINI_SYSTEMPROFILE => SYSTEM_PROFILE_PATH.into(),
            h => self.open.get(&h).map(|p| p.path.clone()).ok_or(PMERR_INVALID_INI_FILE_HANDLE)?,
        })
    }

    /// `PrfOpenProfile`: open (creating it when missing) profile file
    /// `os2_path`.
    pub fn open_profile(&mut self, dm: &DriveManager, os2_path: &str) -> PrfResult<u32> {
        let (id, abs) = file_id(dm, os2_path).ok_or(PMERR_OPENING_INI_FILE)?;
        for special in [USER_PROFILE_PATH, SYSTEM_PROFILE_PATH] {
            if file_id(dm, special).is_some_and(|(sid, _)| sid == id) {
                return Err(PMERR_INI_FILE_IS_SYS_OR_USER);
            }
        }
        if let Some((&h, p)) = self.open.iter_mut().find(|(_, p)| p.id == id) {
            p.refs += 1;
            return Ok(h);
        }
        // Read it now so a file that is not a profile fails here.
        let existing = profile::load(dm, &abs).map_err(store_error)?;
        if !profile::exists(dm, &abs) {
            profile::save(dm, &abs, &existing, false).map_err(|_| PMERR_OPENING_INI_FILE)?;
        }
        let h = self.next;
        self.next += 1;
        self.open.insert(h, OpenProfile { path: abs, id, refs: 1 });
        Ok(h)
    }

    /// `PrfCloseProfile`.
    pub fn close_profile(&mut self, hini: u32) -> PrfResult<()> {
        let p = self.open.get_mut(&hini).ok_or(PMERR_INVALID_INI_FILE_HANDLE)?;
        p.refs -= 1;
        if p.refs == 0 {
            self.open.remove(&hini);
        }
        Ok(())
    }

    /// The profiles `hini` reads, loaded.
    fn load_all(&self, dm: &DriveManager, hini: u32) -> PrfResult<Vec<Profile>> {
        self.read_paths(hini)?.iter()
            .map(|p| profile::load(dm, p).map_err(store_error))
            .collect()
    }

    /// The value of `key` of `app` in the profiles of `hini` (first found).
    pub fn query_value(&self, dm: &DriveManager, hini: u32, app: &[u8], key: &[u8])
        -> PrfResult<Vec<u8>>
    {
        self.load_all(dm, hini)?.iter()
            .find_map(|p| p.get(app, key).map(<[u8]>::to_vec))
            .ok_or(PMERR_NOT_IN_IDX)
    }

    /// The application names of `hini` (or, with `app`, its key names) as a
    /// double-NUL-terminated list; names found in several profiles appear
    /// once.
    pub fn query_names(&self, dm: &DriveManager, hini: u32, app: Option<&[u8]>)
        -> PrfResult<Vec<u8>>
    {
        let profiles = self.load_all(dm, hini)?;
        let mut names: Vec<&[u8]> = Vec::new();
        let mut found = app.is_none();
        for p in &profiles {
            let list: Vec<&[u8]> = match app {
                None => p.app_names().collect(),
                Some(a) => match p.key_names(a) {
                    Some(keys) => { found = true; keys }
                    None => continue,
                },
            };
            for n in list {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
        }
        if !found {
            return Err(PMERR_NOT_IN_IDX);
        }
        Ok(profile::name_list(names))
    }

    /// Write (`Some` value) or delete (`None`) `key` of `app`; without a
    /// key, delete the whole application.
    pub fn write(&self, dm: &DriveManager, hini: u32, app: &[u8], key: Option<&[u8]>,
                 value: Option<&[u8]>) -> PrfResult<()>
    {
        let path = self.write_path(hini)?;
        let mut p = profile::load(dm, &path).map_err(store_error)?;
        let changed = match (key, value) {
            (None, _) => p.delete_app(app),
            (Some(k), None) => p.delete_key(app, k),
            (Some(k), Some(v)) => { p.set(app, k, v); true }
        };
        if !changed {
            // Deleting what is not there leaves the file as it is.
            return Ok(());
        }
        let special = hini == HINI_PROFILE || hini == HINI_USERPROFILE || hini == HINI_SYSTEMPROFILE;
        profile::save(dm, &path, &p, special).map_err(store_error)
    }
}

/// Copy as much of `data` as fits into `max` bytes.
fn fit(data: &[u8], max: u32) -> &[u8] {
    &data[..data.len().min(max as usize)]
}

/// A string result truncated to `max` bytes, still NUL-terminated.
fn fit_string(s: &[u8], max: u32) -> Vec<u8> {
    let mut out = s.to_vec();
    out.push(0);
    if out.len() > max as usize {
        out.truncate(max as usize);
        if let Some(last) = out.last_mut() {
            *last = 0;
        }
    }
    out
}

impl super::Loader {
    /// A guest string as bytes (no codepage decoding): `Ok(None)` for a NULL
    /// pointer, `Err` when it is out of guest memory or longer than a
    /// profile value may be.
    fn prf_string(&self, ptr: u32) -> PrfResult<Option<Vec<u8>>> {
        if ptr == 0 {
            return Ok(None);
        }
        let avail = self.shared.guest_mem.size().saturating_sub(ptr as usize);
        let len = avail.min(MAX_PROFILE_VALUE + 1);
        let bytes = self.guest_read_bytes(ptr, len).ok_or(PMERR_INVALID_PARM)?;
        let n = bytes.iter().position(|&b| b == 0).ok_or(PMERR_INVALID_PARM)?;
        Ok(Some(bytes[..n].to_vec()))
    }

    /// Record the failure of a `Prf*` call and return 0 (FALSE / NULLHANDLE).
    fn prf_result(&self, tid: u32, r: PrfResult<u32>) -> u32 {
        r.unwrap_or_else(|code| { self.pm_error(tid, code); 0 })
    }

    /// Write `data` to the guest at `ptr`.
    fn prf_put(&self, ptr: u32, data: &[u8]) -> PrfResult<()> {
        if data.is_empty() {
            return Ok(());
        }
        if ptr == 0 {
            return Err(PMERR_INVALID_PARM);
        }
        self.guest_write_bytes(ptr, data).ok_or(PMERR_INVALID_PARM)
    }

    /// PrfOpenProfile(hab, pszFileName) → HINI.
    pub fn prf_open_profile(&self, tid: u32, _hab: u32, psz_file: u32) -> u32 {
        let r = (|| {
            let name = self.prf_string(psz_file)?.ok_or(PMERR_INVALID_PARM)?;
            let name = super::codepage::cp_decode(&name, self.shared.active_codepage
                .load(std::sync::atomic::Ordering::Relaxed));
            let mut table = self.shared.profiles.lock_or_recover();
            let dm = self.shared.drive_mgr.lock_or_recover();
            table.open_profile(&dm, &name)
        })();
        self.prf_result(tid, r)
    }

    /// PrfCloseProfile(hini) → BOOL.
    pub fn prf_close_profile(&self, tid: u32, hini: u32) -> u32 {
        let r = self.shared.profiles.lock_or_recover().close_profile(hini).map(|()| 1);
        self.prf_result(tid, r)
    }

    /// PrfQueryProfileString(hini, pszApp, pszKey, pszDefault, pBuffer,
    /// ulBufferMax) → bytes returned.
    #[allow(clippy::too_many_arguments)] // the API's six arguments
    pub fn prf_query_profile_string(&self, tid: u32, hini: u32, psz_app: u32, psz_key: u32,
                                    psz_default: u32, p_buf: u32, max: u32) -> u32 {
        let r = (|| {
            let app = self.prf_string(psz_app)?;
            let key = match app { Some(_) => self.prf_string(psz_key)?, None => None };
            let default = self.prf_string(psz_default)?;
            let table = self.shared.profiles.lock_or_recover();
            let dm = self.shared.drive_mgr.lock_or_recover();
            match (&app, &key) {
                (Some(a), Some(k)) => {
                    let found = match table.query_value(&dm, hini, a, k) {
                        Err(code) if code != PMERR_NOT_IN_IDX => return Err(code),
                        r => r,
                    };
                    drop(dm);
                    let text = match (&found, &default) {
                        (Ok(v), _) => profile::value_as_str(v).to_vec(),
                        (Err(_), Some(d)) => d.clone(),
                        (Err(_), None) => Vec::new(),
                    };
                    let out = if found.is_err() && default.is_none() {
                        Vec::new()
                    } else {
                        fit_string(&text, max)
                    };
                    self.prf_put(p_buf, &out)?;
                    if let Err(code) = found {
                        // Not found: the default is returned and the reason
                        // recorded.
                        self.pm_error(tid, code);
                    }
                    Ok(out.len() as u32)
                }
                _ => {
                    // Enumeration: the list without its final NUL, or what
                    // fits.
                    let list = table.query_names(&dm, hini, app.as_deref())?;
                    drop(dm);
                    let out = fit(&list, max);
                    self.prf_put(p_buf, out)?;
                    Ok(if out.len() == list.len() { list.len() as u32 - 1 } else { out.len() as u32 })
                }
            }
        })();
        self.prf_result(tid, r)
    }

    /// PrfQueryProfileInt(hini, pszApp, pszKey, lDefault) → LONG.
    pub fn prf_query_profile_int(&self, tid: u32, hini: u32, psz_app: u32, psz_key: u32,
                                 default: u32) -> u32 {
        let r = (|| {
            let app = self.prf_string(psz_app)?.ok_or(PMERR_INVALID_PARM)?;
            let key = self.prf_string(psz_key)?.ok_or(PMERR_INVALID_PARM)?;
            let table = self.shared.profiles.lock_or_recover();
            let dm = self.shared.drive_mgr.lock_or_recover();
            table.query_value(&dm, hini, &app, &key)
        })();
        match r {
            Ok(v) => profile::parse_profile_int(&v) as u32,
            Err(code) => { self.pm_error(tid, code); default }
        }
    }

    /// PrfQueryProfileSize(hini, pszApp, pszKey, pulDataSize) → BOOL.
    pub fn prf_query_profile_size(&self, tid: u32, hini: u32, psz_app: u32, psz_key: u32,
                                  pul_size: u32) -> u32 {
        let r = (|| {
            if pul_size == 0 { return Err(PMERR_INVALID_PARM); }
            let size = self.prf_query(hini, psz_app, psz_key)?.len() as u32;
            self.guest_write::<u32>(pul_size, size).ok_or(PMERR_INVALID_PARM)?;
            Ok(1)
        })();
        self.prf_result(tid, r)
    }

    /// The data `PrfQueryProfileData` / `Size` return: a value, or a name
    /// list (whole, final NUL included).
    fn prf_query(&self, hini: u32, psz_app: u32, psz_key: u32) -> PrfResult<Vec<u8>> {
        let app = self.prf_string(psz_app)?;
        let key = match app { Some(_) => self.prf_string(psz_key)?, None => None };
        let table = self.shared.profiles.lock_or_recover();
        let dm = self.shared.drive_mgr.lock_or_recover();
        match (app, key) {
            (Some(a), Some(k)) => table.query_value(&dm, hini, &a, &k),
            (app, _) => table.query_names(&dm, hini, app.as_deref()),
        }
    }

    /// PrfQueryProfileData(hini, pszApp, pszKey, pBuffer, pulBufferMax) →
    /// BOOL; `*pulBufferMax` becomes the bytes written.
    pub fn prf_query_profile_data(&self, tid: u32, hini: u32, psz_app: u32, psz_key: u32,
                                  p_buf: u32, pul_max: u32) -> u32 {
        let r = (|| {
            let max = self.guest_read::<u32>(pul_max).filter(|_| pul_max != 0)
                .ok_or(PMERR_INVALID_PARM)?;
            let enumerating = psz_app == 0 || psz_key == 0;
            let data = self.prf_query(hini, psz_app, psz_key)?;
            let out = fit(&data, max);
            self.prf_put(p_buf, out)?;
            // A whole list is counted without its final NUL.
            let count = if enumerating && out.len() == data.len() { out.len() - 1 } else { out.len() };
            self.guest_write::<u32>(pul_max, count as u32).ok_or(PMERR_INVALID_PARM)?;
            Ok(1)
        })();
        self.prf_result(tid, r)
    }

    /// PrfWriteProfileString(hini, pszApp, pszKey, pszData) → BOOL. NULL
    /// data deletes the key, a NULL key the application.
    pub fn prf_write_profile_string(&self, tid: u32, hini: u32, psz_app: u32, psz_key: u32,
                                    psz_data: u32) -> u32 {
        let r = (|| {
            let app = self.prf_string(psz_app)?.ok_or(PMERR_INVALID_PARM)?;
            let key = self.prf_string(psz_key)?;
            // A string is stored with its terminating NUL.
            let data = self.prf_string(psz_data)?.map(|mut d| { d.push(0); d });
            if data.as_ref().is_some_and(|d| d.len() > MAX_PROFILE_VALUE) {
                return Err(PMERR_INVALID_PARM);
            }
            self.prf_write(hini, &app, key.as_deref(), data.as_deref())
        })();
        self.prf_result(tid, r)
    }

    /// PrfWriteProfileData(hini, pszApp, pszKey, pData, ulDataLen) → BOOL.
    /// NULL data deletes the key, a NULL key the application.
    pub fn prf_write_profile_data(&self, tid: u32, hini: u32, psz_app: u32, psz_key: u32,
                                  p_data: u32, len: u32) -> u32 {
        let r = (|| {
            let app = self.prf_string(psz_app)?.ok_or(PMERR_INVALID_PARM)?;
            let key = self.prf_string(psz_key)?;
            let data = if p_data == 0 {
                None
            } else {
                if len as usize > MAX_PROFILE_VALUE { return Err(PMERR_INVALID_PARM); }
                Some(self.guest_read_bytes(p_data, len as usize).ok_or(PMERR_INVALID_PARM)?)
            };
            self.prf_write(hini, &app, key.as_deref(), data.as_deref())
        })();
        self.prf_result(tid, r)
    }

    fn prf_write(&self, hini: u32, app: &[u8], key: Option<&[u8]>, data: Option<&[u8]>)
        -> PrfResult<u32>
    {
        let table = self.shared.profiles.lock_or_recover();
        let dm = self.shared.drive_mgr.lock_or_recover();
        table.write(&dm, hini, app, key, data).map(|()| 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
        use crate::loader::vfs::DriveConfig;
    use crate::loader::vfs_hostdir::HostDirBackend;
    use crate::loader::vm_backend::mock::MockVcpu;
    use crate::loader::{ApiResult, Loader};

    const TID: u32 = 1;
    /// Scratch guest memory for strings and buffers.
    const STR_A: u32 = 0x0010_0000;
    const STR_B: u32 = 0x0010_1000;
    const STR_C: u32 = 0x0010_2000;
    const BUF: u32 = 0x0010_3000;
    const PUL: u32 = 0x0010_8000;

    /// A mock loader whose C: is a temporary directory.
    fn loader() -> (Loader, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let loader = Loader::new_mock();
        {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            dm.set_drive_config(2, DriveConfig { host_path: tmp.path().to_path_buf(),
                                                 label: "T".into(), read_only: false });
            dm.mount(2, Box::new(HostDirBackend::new(tmp.path().to_path_buf()).unwrap()));
        }
        (loader, tmp)
    }

    fn put(l: &Loader, at: u32, s: &[u8]) -> u32 {
        l.guest_write_bytes(at, s).unwrap();
        l.guest_write::<u8>(at + s.len() as u32, 0).unwrap();
        at
    }

    fn last_error(l: &Loader) -> u32 {
        l.shared.pm_errors.take(TID) & 0xFFFF
    }

    fn read(l: &Loader, at: u32, n: u32) -> Vec<u8> {
        l.guest_read_bytes(at, n as usize).unwrap()
    }

    fn write_string(l: &Loader, hini: u32, app: &[u8], key: &[u8], data: &[u8]) -> u32 {
        l.prf_write_profile_string(TID, hini, put(l, STR_A, app), put(l, STR_B, key),
                                   put(l, STR_C, data))
    }

    fn query_string(l: &Loader, hini: u32, app: Option<&[u8]>, key: Option<&[u8]>,
                    default: Option<&[u8]>, max: u32) -> (u32, Vec<u8>) {
        l.guest_write_bytes(BUF, &[0xAA; 64]).unwrap();
        let n = l.prf_query_profile_string(TID, hini, app.map_or(0, |a| put(l, STR_A, a)),
            key.map_or(0, |k| put(l, STR_B, k)), default.map_or(0, |d| put(l, STR_C, d)), BUF, max);
        (n, read(l, BUF, n))
    }

    #[test]
    fn strings_are_written_and_read_back() {
        let (l, tmp) = loader();
        assert_eq!(write_string(&l, HINI_USERPROFILE, b"App", b"Key", b"hello"), 1);
        assert_eq!(query_string(&l, HINI_USERPROFILE, Some(b"App"), Some(b"Key"), None, 64),
                   (6, b"hello\0".to_vec()));
        // HINI_PROFILE searches the user profile too.
        assert_eq!(query_string(&l, HINI_PROFILE, Some(b"App"), Some(b"Key"), None, 64).1, b"hello\0");
        // Not in the system profile.
        assert_eq!(query_string(&l, HINI_SYSTEMPROFILE, Some(b"App"), Some(b"Key"), None, 64).0, 0);
        assert_eq!(last_error(&l), PMERR_NOT_IN_IDX);
        // Stored in Warpine's text format under C:\OS2.
        let text = std::fs::read_to_string(tmp.path().join("OS2/OS2.INI")).unwrap();
        assert!(text.contains("[\"App\"]\n\"Key\" = \"hello\\0\""), "{text}");
        // The size includes the NUL.
        assert_eq!(l.prf_query_profile_size(TID, HINI_USERPROFILE, put(&l, STR_A, b"App"),
                                            put(&l, STR_B, b"Key"), PUL), 1);
        assert_eq!(l.guest_read::<u32>(PUL), Some(6));
    }

    #[test]
    fn missing_keys_return_the_default() {
        let (l, _tmp) = loader();
        assert_eq!(query_string(&l, HINI_PROFILE, Some(b"No"), Some(b"Key"), Some(b"dflt"), 64),
                   (5, b"dflt\0".to_vec()));
        assert_eq!(last_error(&l), PMERR_NOT_IN_IDX);
        assert_eq!(query_string(&l, HINI_PROFILE, Some(b"No"), Some(b"Key"), None, 64).0, 0);
        assert_eq!(l.prf_query_profile_int(TID, HINI_PROFILE, put(&l, STR_A, b"No"),
                                           put(&l, STR_B, b"Key"), -7i32 as u32), -7i32 as u32);
        assert_eq!(last_error(&l), PMERR_NOT_IN_IDX);
    }

    #[test]
    fn long_strings_are_truncated_and_terminated() {
        let (l, _tmp) = loader();
        write_string(&l, HINI_USERPROFILE, b"App", b"Key", b"abcdef");
        assert_eq!(query_string(&l, HINI_USERPROFILE, Some(b"App"), Some(b"Key"), None, 4),
                   (4, b"abc\0".to_vec()));
        assert_eq!(query_string(&l, HINI_USERPROFILE, Some(b"App"), Some(b"Key"), None, 0).0, 0);
    }

    #[test]
    fn integers_are_converted() {
        let (l, _tmp) = loader();
        write_string(&l, HINI_USERPROFILE, b"App", b"N", b"-42xyz");
        write_string(&l, HINI_USERPROFILE, b"App", b"T", b"text");
        let q = |key: &[u8]| l.prf_query_profile_int(TID, HINI_PROFILE, put(&l, STR_A, b"App"),
                                                     put(&l, STR_B, key), 99);
        assert_eq!(q(b"N") as i32, -42);
        assert_eq!(q(b"T"), 0);
    }

    #[test]
    fn enumeration_lists_applications_and_keys() {
        let (l, _tmp) = loader();
        write_string(&l, HINI_USERPROFILE, b"A1", b"k1", b"x");
        write_string(&l, HINI_USERPROFILE, b"A1", b"k2", b"x");
        write_string(&l, HINI_USERPROFILE, b"A2", b"k", b"x");
        write_string(&l, HINI_SYSTEMPROFILE, b"S", b"k", b"x");
        write_string(&l, HINI_SYSTEMPROFILE, b"A1", b"k3", b"x");
        let (n, list) = query_string(&l, HINI_USERPROFILE, None, None, None, 64);
        assert_eq!((n, read(&l, BUF, 7)), (6, b"A1\0A2\0\0".to_vec()));
        assert_eq!(list, b"A1\0A2\0");
        // Both profiles, each name once, user first.
        let (n, _) = query_string(&l, HINI_PROFILE, None, None, None, 64);
        assert_eq!(read(&l, BUF, n + 1), b"A1\0A2\0S\0\0");
        let (n, _) = query_string(&l, HINI_PROFILE, Some(b"A1"), None, None, 64);
        assert_eq!(read(&l, BUF, n + 1), b"k1\0k2\0k3\0\0");
        // Truncated: what fits, without the double NUL.
        let (n, list) = query_string(&l, HINI_USERPROFILE, None, None, None, 4);
        assert_eq!((n, list), (4, b"A1\0A".to_vec()));
        // Keys of a missing application.
        assert_eq!(query_string(&l, HINI_USERPROFILE, Some(b"None"), None, Some(b"d"), 64).0, 0);
        assert_eq!(last_error(&l), PMERR_NOT_IN_IDX);
        // Sizes cover the whole list.
        assert_eq!(l.prf_query_profile_size(TID, HINI_USERPROFILE, 0, 0, PUL), 1);
        assert_eq!(l.guest_read::<u32>(PUL), Some(7));
    }

    #[test]
    fn binary_data_round_trips_and_truncates() {
        let (l, _tmp) = loader();
        let blob = [0u8, 1, 2, 0xFF, b'"', 0];
        l.guest_write_bytes(STR_C, &blob).unwrap();
        assert_eq!(l.prf_write_profile_data(TID, HINI_USERPROFILE, put(&l, STR_A, b"App"),
                                            put(&l, STR_B, b"Blob"), STR_C, blob.len() as u32), 1);
        let query = |max: u32| {
            l.guest_write::<u32>(PUL, max).unwrap();
            let ok = l.prf_query_profile_data(TID, HINI_PROFILE, put(&l, STR_A, b"App"),
                                              put(&l, STR_B, b"Blob"), BUF, PUL);
            let n = l.guest_read::<u32>(PUL).unwrap();
            (ok, read(&l, BUF, n))
        };
        assert_eq!(query(64), (1, blob.to_vec()));
        assert_eq!(query(3), (1, blob[..3].to_vec()));
        // An empty value is a value.
        assert_eq!(l.prf_write_profile_data(TID, HINI_USERPROFILE, put(&l, STR_A, b"App"),
                                            put(&l, STR_B, b"Empty"), STR_C, 0), 1);
        assert_eq!(l.prf_query_profile_size(TID, HINI_USERPROFILE, put(&l, STR_A, b"App"),
                                            put(&l, STR_B, b"Empty"), PUL), 1);
        assert_eq!(l.guest_read::<u32>(PUL), Some(0));
        // Too long, or out of guest memory.
        assert_eq!(l.prf_write_profile_data(TID, HINI_USERPROFILE, put(&l, STR_A, b"App"),
                                            put(&l, STR_B, b"Big"), STR_C,
                                            MAX_PROFILE_VALUE as u32 + 1), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_PARM);
        assert_eq!(l.prf_write_profile_data(TID, HINI_USERPROFILE, put(&l, STR_A, b"App"),
                                            put(&l, STR_B, b"Big"), 0xFFFF_FF00, 0x200), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_PARM);
        // The output buffer out of guest memory.
        l.guest_write::<u32>(PUL, 64).unwrap();
        assert_eq!(l.prf_query_profile_data(TID, HINI_PROFILE, put(&l, STR_A, b"App"),
                                            put(&l, STR_B, b"Blob"), 0xFFFF_FFF0, PUL), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_PARM);
    }

    #[test]
    fn null_data_deletes_a_key_and_null_key_an_application() {
        let (l, _tmp) = loader();
        write_string(&l, HINI_USERPROFILE, b"App", b"K1", b"a");
        write_string(&l, HINI_USERPROFILE, b"App", b"K2", b"b");
        assert_eq!(l.prf_write_profile_string(TID, HINI_USERPROFILE, put(&l, STR_A, b"App"),
                                              put(&l, STR_B, b"K1"), 0), 1);
        assert_eq!(query_string(&l, HINI_USERPROFILE, Some(b"App"), Some(b"K1"), None, 64).0, 0);
        assert_eq!(query_string(&l, HINI_USERPROFILE, Some(b"App"), Some(b"K2"), None, 64).0, 2);
        assert_eq!(l.prf_write_profile_data(TID, HINI_USERPROFILE, put(&l, STR_A, b"App"),
                                            0, 0, 0), 1);
        assert_eq!(query_string(&l, HINI_USERPROFILE, None, None, None, 64), (0, vec![]));
        // Deleting what is not there succeeds.
        assert_eq!(l.prf_write_profile_data(TID, HINI_USERPROFILE, put(&l, STR_A, b"App"),
                                            0, 0, 0), 1);
        // A NULL application cannot be written.
        assert_eq!(l.prf_write_profile_string(TID, HINI_USERPROFILE, 0, 0, 0), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_PARM);
    }

    #[test]
    fn opened_profiles() {
        let (l, tmp) = loader();
        let h = l.prf_open_profile(TID, MOCK_HAB, put(&l, STR_A, b"C:\\MYAPP.INI"));
        assert!(h >= FIRST_PROFILE_HANDLE);
        assert!(tmp.path().join("MYAPP.INI").exists(), "created when missing");
        // Opening it again (any case) gives the same handle.
        assert_eq!(l.prf_open_profile(TID, MOCK_HAB, put(&l, STR_A, b"c:\\myapp.ini")), h);
        assert_eq!(write_string(&l, h, b"App", b"Key", b"v"), 1);
        assert_eq!(query_string(&l, h, Some(b"App"), Some(b"Key"), None, 64).1, b"v\0");
        // The user profile is a different file.
        assert_eq!(query_string(&l, HINI_PROFILE, Some(b"App"), Some(b"Key"), None, 64).0, 0);
        last_error(&l);
        assert_eq!(l.prf_close_profile(TID, h), 1);
        assert_eq!(l.prf_close_profile(TID, h), 1, "opened twice");
        assert_eq!(l.prf_close_profile(TID, h), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_INI_FILE_HANDLE);
        assert_eq!(write_string(&l, h, b"App", b"Key", b"v"), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_INI_FILE_HANDLE);
        assert_eq!(l.prf_close_profile(TID, HINI_USERPROFILE), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_INI_FILE_HANDLE);
        // Reopened, the data is still there.
        let h2 = l.prf_open_profile(TID, MOCK_HAB, put(&l, STR_A, b"C:\\MYAPP.INI"));
        assert_eq!(query_string(&l, h2, Some(b"App"), Some(b"Key"), None, 64).1, b"v\0");
    }

    #[test]
    fn opening_fails_for_system_files_corrupt_files_and_bad_paths() {
        let (l, tmp) = loader();
        assert_eq!(l.prf_open_profile(TID, MOCK_HAB, put(&l, STR_A, b"C:\\OS2\\os2.ini")), 0);
        assert_eq!(last_error(&l), PMERR_INI_FILE_IS_SYS_OR_USER);
        std::fs::write(tmp.path().join("BINARY.INI"), b"\xFF\xFF\xFF\xFF\x14\0\0\0").unwrap();
        assert_eq!(l.prf_open_profile(TID, MOCK_HAB, put(&l, STR_A, b"C:\\BINARY.INI")), 0);
        assert_eq!(last_error(&l), PMERR_INI_FILE_CORRUPT);
        assert_eq!(l.prf_open_profile(TID, MOCK_HAB, put(&l, STR_A, b"Q:\\X.INI")), 0);
        assert_eq!(last_error(&l), PMERR_OPENING_INI_FILE);
        assert_eq!(l.prf_open_profile(TID, MOCK_HAB, put(&l, STR_A, b"C:\\NODIR\\X.INI")), 0);
        assert_eq!(last_error(&l), PMERR_OPENING_INI_FILE);
        assert_eq!(l.prf_open_profile(TID, MOCK_HAB, 0), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_PARM);
    }

    #[test]
    fn bad_handles_and_unterminated_strings_fail() {
        let (l, _tmp) = loader();
        assert_eq!(query_string(&l, 0x1234, Some(b"A"), Some(b"K"), Some(b"d"), 64).0, 0);
        assert_eq!(last_error(&l), PMERR_INVALID_INI_FILE_HANDLE);
        // A string running to the end of guest memory.
        let end = l.shared.guest_mem.size() as u32 - 4;
        l.guest_write_bytes(end, b"ABCD").unwrap();
        assert_eq!(l.prf_write_profile_string(TID, HINI_USERPROFILE, end, 0, 0), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_PARM);
        assert_eq!(l.prf_query_profile_size(TID, HINI_USERPROFILE, 0, 0, 0), 0);
        assert_eq!(last_error(&l), PMERR_INVALID_PARM);
    }

    #[test]
    fn calls_are_dispatched_through_the_registry() {
        let (l, _tmp) = loader();
        let call = |ordinal: u32, args: &[u32]| {
            let mut a = [0u32; 10];
            a[..args.len()].copy_from_slice(args);
            let entry = crate::loader::api_registry::find(PMSHAPI_BASE + ordinal).unwrap();
            let mut vcpu = MockVcpu::new();
            match (entry.handler)(&l, &mut vcpu, TID, a) {
                ApiResult::Normal(v) => v,
                other => panic!("{other:?}"),
            }
        };
        let (app, key) = (put(&l, STR_A, b"App"), put(&l, STR_B, b"Key"));
        put(&l, STR_C, b"5");
        assert_eq!(call(116, &[HINI_USERPROFILE, app, key, STR_C]), 1);
        assert_eq!(call(114, &[HINI_PROFILE, app, key, 0]), 5);
        assert_eq!(call(115, &[HINI_PROFILE, app, key, 0, BUF, 8]), 2);
        assert_eq!(call(101, &[HINI_PROFILE, app, key, PUL]), 1);
        l.guest_write::<u32>(PUL, 8).unwrap();
        assert_eq!(call(117, &[HINI_PROFILE, app, key, BUF, PUL]), 1);
        assert_eq!(call(118, &[HINI_USERPROFILE, app, key, 0, 0]), 1);
        let h = call(102, &[MOCK_HAB, put(&l, STR_C, b"C:\\T.INI")]);
        assert_ne!(h, 0);
        assert_eq!(call(103, &[h]), 1);
        assert_eq!(l.shared.pm_errors.take(TID), 0, "no call failed");
    }
}
