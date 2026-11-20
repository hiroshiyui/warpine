// SPDX-License-Identifier: GPL-3.0-or-later
//
//! `DosSearchPath` (DOSCALLS 228) and `DosQueryModuleName` (DOSCALLS 320).
//!
//! Behaviour from IBM's *Control Program Programming Reference* and
//! `<bsedos.h>`:
//! - `DosSearchPath(flag, pszPathOrName, pszFilename, pBuf, cbBuf)` looks
//!   for `pszFilename` in each directory of a semicolon-separated path, in
//!   order, and writes the full name of the first match. Flag bits:
//!   `SEARCH_CUR_DIRECTORY` searches the current directory first,
//!   `SEARCH_ENVIRONMENT` takes the path from the environment variable named
//!   by `pszPathOrName` (`ERROR_ENVVAR_NOT_FOUND`), `SEARCH_IGNORENETERRS`
//!   goes on after a network error; bits 3–31 are reserved. Paths that
//!   contain semicolons are quoted. A name with wildcards is matched with
//!   `DosFindFirst` and returned with its wildcards (ready for
//!   `DosFindFirst`); without, the result is fully qualified (ready for
//!   `DosOpen`). Errors: `ERROR_FILE_NOT_FOUND`, `ERROR_BUFFER_OVERFLOW`,
//!   `ERROR_INVALID_PARAMETER`, `ERROR_ENVVAR_NOT_FOUND`.
//! - `DosQueryModuleName(hmod, cbName, pch)` writes the fully qualified
//!   drive, path, file name and extension of a module
//!   (`ERROR_INVALID_HANDLE`, `ERROR_BAD_LENGTH`).
//!
//! Warpine's choices where the documents say nothing:
//! - reserved flag bits are `ERROR_INVALID_PARAMETER`, as are NULL string
//!   pointers;
//! - a path element that cannot be resolved (unknown drive, UNC) is skipped
//!   whatever `SEARCH_IGNORENETERRS` says (Warpine has no network drives);
//! - only files match, not directories (hidden and system files do);
//! - the directory part of the result is normalised (`.` and `..`
//!   removed) and uses `\`; the file name keeps the caller's spelling;
//! - a result that does not fit `cbBuf` (with its NUL) is
//!   `ERROR_BUFFER_OVERFLOW` and nothing is written;
//! - module handle 0 — what Warpine passes the executable as its module
//!   handle — is the executable, named as in its command line
//!   (`C:\NAME.EXE`); a DLL next to the executable is in the same
//!   directory, any other DLL has the OS/2 path of its file on a drive,
//!   else `C:\OS2\DLL\NAME.DLL`; a built-in module (DOSCALLS, PMWIN, …)
//!   loaded with `DosLoadModule` reports `C:\OS2\DLL\NAME.DLL`, where OS/2
//!   keeps its system DLLs;
//! - a name that does not fit `cbName` (with its NUL) is `ERROR_BAD_LENGTH`
//!   and nothing is written; an unwritable buffer `ERROR_INVALID_PARAMETER`.

use std::path::Path;

use super::codepage::cp_encode;
use super::constants::*;
use super::mutex_ext::MutexExt;
use super::vfs::FileAttribute;
use log::debug;

/// Directory of the system DLLs that built-in modules report.
const SYSTEM_DLL_DIR: &str = "C:\\OS2\\DLL\\";
/// Longest environment block `DosSearchPath` reads for `SEARCH_ENVIRONMENT`.
const MAX_ENV_BLOCK: u32 = 64 * 1024;
/// Most path elements `DosSearchPath` tries.
const MAX_PATH_ELEMENTS: usize = 256;
/// Files `DosSearchPath` matches: any but directories.
const SEARCH_ATTRS: FileAttribute = FileAttribute(0x07);

/// The elements of a `;`-separated path; `"…"` quotes a `;`. Empty
/// elements are dropped.
pub fn split_path(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in path.chars() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                let e = cur.trim();
                if !e.is_empty() { out.push(e.to_string()); }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let e = cur.trim();
    if !e.is_empty() { out.push(e.to_string()); }
    out.truncate(MAX_PATH_ELEMENTS);
    out
}

/// `dir` and `name` joined with one `\`.
pub fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with(['\\', '/', ':']) { format!("{dir}{name}") } else { format!("{dir}\\{name}") }
}

/// Fully qualified `X:\DIR\…\NAME` from a resolved drive index and the
/// volume-relative path of the directory, `.` and `..` removed.
pub fn qualify(drive: u8, rel_dir: &str, name: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in rel_dir.split(['/', '\\']) {
        match p {
            "" | "." => {}
            ".." => { parts.pop(); }
            p => parts.push(p),
        }
    }
    let letter = (b'A' + drive.min(25)) as char;
    if parts.is_empty() {
        format!("{letter}:\\{name}")
    } else {
        format!("{letter}:\\{}\\{name}", parts.join("\\"))
    }
}

fn has_wildcards(name: &str) -> bool {
    name.contains(['*', '?'])
}

impl super::Loader {
    /// The executable's OS/2 name, as in its command line (`C:\NAME.EXE`).
    pub(crate) fn os2_exe_path(&self) -> String {
        let exe_name = self.shared.exe_name.lock_or_recover().clone();
        if exe_name.is_empty() {
            return String::from("C:\\APP.EXE");
        }
        let basename = Path::new(&exe_name).file_name()
            .map(|f| f.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "APP.EXE".into());
        format!("C:\\{basename}")
    }

    /// The OS/2 path a DLL loaded from host file `host_path` reports.
    pub(crate) fn dll_os2_path(&self, name: &str, host_path: &str) -> String {
        let host = Path::new(host_path);
        let file = host.file_name()
            .map(|f| f.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| format!("{name}.DLL"));
        // Next to the executable: the executable's own directory (C:\).
        let exe_name = self.shared.exe_name.lock_or_recover().clone();
        let exe_dir = Path::new(&exe_name).parent()
            .map(|p| if p.as_os_str().is_empty() { Path::new(".") } else { p })
            .and_then(|p| p.canonicalize().ok());
        let dll_dir = host.parent()
            .map(|p| if p.as_os_str().is_empty() { Path::new(".") } else { p })
            .and_then(|p| p.canonicalize().ok());
        if !exe_name.is_empty() && exe_dir.is_some() && exe_dir == dll_dir {
            return format!("C:\\{file}");
        }
        if let Some(p) = self.shared.drive_mgr.lock_or_recover().host_to_os2_path(host) {
            return p;
        }
        format!("{SYSTEM_DLL_DIR}{file}")
    }

    /// The value of environment variable `name` (case-insensitive) in the
    /// process's environment block (at most `MAX_ENV_BLOCK` bytes read).
    fn env_value(&self, name: &str) -> Option<String> {
        let start = self.guest_read::<u32>(PIB_BASE + PIB_PCHENV_OFFSET)?;
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let prefix = format!("{}=", name.to_ascii_uppercase());
        let mut entry: Vec<u8> = Vec::new();
        for i in 0..MAX_ENV_BLOCK {
            let b = self.guest_read::<u8>(start.checked_add(i)?)?;
            if b != 0 {
                entry.push(b);
                continue;
            }
            if entry.is_empty() {
                break; // the NUL that ends the block
            }
            let text = super::codepage::cp_decode(&entry, cp);
            if text.to_ascii_uppercase().starts_with(&prefix) {
                return Some(text[prefix.len()..].to_string());
            }
            entry.clear();
        }
        None
    }

    /// The full name of `name` in directory `dir`, if a file matches there.
    fn search_in(&self, dir: &str, name: &str) -> Option<String> {
        let candidate = join_path(dir, name);
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        let (drive, rel) = dm.resolve_path(&candidate).ok()?;
        let found = if has_wildcards(name) {
            match dm.find_first(&candidate, SEARCH_ATTRS, 1) {
                Ok((h, _)) => { let _ = dm.find_close(h); true }
                Err(_) => false,
            }
        } else {
            dm.query_path_info(&candidate, 1)
                .is_ok_and(|s| !s.attributes.contains(FileAttribute::DIRECTORY))
        };
        if !found {
            return None;
        }
        // The directory part of the resolved path, then the caller's name.
        let rel_dir = rel.rsplit_once('/').map_or("", |(d, _)| d);
        let name_part = name.rsplit(['\\', '/']).next().unwrap_or(name);
        Some(qualify(drive, rel_dir, name_part))
    }

    /// DosSearchPath (ordinal 228).
    pub fn dos_search_path(&self, flag: u32, psz_path: u32, psz_name: u32, p_buf: u32, cb_buf: u32) -> u32 {
        const VALID: u32 = SEARCH_CUR_DIRECTORY | SEARCH_ENVIRONMENT | SEARCH_IGNORENETERRS;
        if flag & !VALID != 0 || psz_path == 0 || psz_name == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let path_or_name = self.read_guest_string(psz_path);
        let name = self.read_guest_string(psz_name);
        let path = if flag & SEARCH_ENVIRONMENT != 0 {
            match self.env_value(&path_or_name) {
                Some(v) => v,
                None => return ERROR_ENVVAR_NOT_FOUND,
            }
        } else {
            path_or_name
        };
        if name.is_empty() {
            return ERROR_FILE_NOT_FOUND;
        }
        let mut dirs = split_path(&path);
        if flag & SEARCH_CUR_DIRECTORY != 0 {
            dirs.insert(0, ".".to_string());
        }
        let Some(found) = dirs.iter().find_map(|d| self.search_in(d, &name)) else {
            debug!("  DosSearchPath('{}', '{}') not found", path, name);
            return ERROR_FILE_NOT_FOUND;
        };
        debug!("  DosSearchPath('{}', '{}') -> '{}'", path, name, found);
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let mut bytes = cp_encode(&found, cp);
        bytes.push(0);
        if bytes.len() as u64 > cb_buf as u64 {
            return ERROR_BUFFER_OVERFLOW;
        }
        match self.guest_write_bytes(p_buf, &bytes) {
            Some(()) => NO_ERROR,
            None => ERROR_INVALID_PARAMETER,
        }
    }

    /// The OS/2 name of module `hmod`.
    pub(crate) fn module_name(&self, hmod: u32) -> Option<String> {
        if hmod == EXE_MODULE_HANDLE {
            return Some(self.os2_exe_path());
        }
        let dll_mgr = self.shared.dll_mgr.lock_or_recover();
        let dll = dll_mgr.find_by_handle(hmod)?;
        if dll.is_builtin {
            return Some(format!("{SYSTEM_DLL_DIR}{}.DLL", dll.name));
        }
        Some(dll_mgr.module_path(hmod).map(str::to_string)
            .unwrap_or_else(|| format!("{SYSTEM_DLL_DIR}{}.DLL", dll.name)))
    }

    /// DosQueryModuleName (ordinal 320).
    pub fn dos_query_module_name(&self, hmod: u32, cb_name: u32, pch: u32) -> u32 {
        let Some(name) = self.module_name(hmod) else { return ERROR_INVALID_HANDLE };
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let mut bytes = cp_encode(&name, cp);
        bytes.push(0);
        if bytes.len() as u64 > cb_name as u64 {
            return ERROR_BAD_LENGTH;
        }
        match self.guest_write_bytes(pch, &bytes) {
            Some(()) => NO_ERROR,
            None => ERROR_INVALID_PARAMETER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Loader;
    use super::super::managers::LoadedDll;
    use super::super::vfs::DriveConfig;
    use super::super::vfs_hostdir::HostDirBackend;
    use std::collections::HashMap;

    const STR_A: u32 = 0x0010_0000;
    const STR_B: u32 = 0x0010_1000;
    const BUF: u32 = 0x0010_2000;
    const ENV: u32 = 0x0010_4000;

    /// A mock loader whose C: is a temporary directory with
    /// `BIN\TOOL.EXE`, `LIB\A.DLL`, `LIB\B.DLL`, `ROOT.TXT` and a
    /// directory `LIB\SUB.DIR`; the environment has `MYPATH` and `EMPTY`.
    fn loader() -> (Loader, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        for d in ["BIN", "LIB", "LIB/SUB.DIR"] {
            std::fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        for f in ["BIN/TOOL.EXE", "LIB/A.DLL", "LIB/B.DLL", "ROOT.TXT"] {
            std::fs::write(tmp.path().join(f), b"x").unwrap();
        }
        let loader = Loader::new_mock();
        {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            dm.set_drive_config(2, DriveConfig { host_path: tmp.path().to_path_buf(),
                                                 label: "T".into(), read_only: false });
            dm.mount(2, Box::new(HostDirBackend::new(tmp.path().to_path_buf()).unwrap()));
        }
        loader.guest_write_bytes(ENV, b"PATH=C:\\\0MyPath=C:\\NOPE;C:\\LIB\0EMPTY=\0\0").unwrap();
        loader.guest_write::<u32>(PIB_BASE + PIB_PCHENV_OFFSET, ENV).unwrap();
        (loader, tmp)
    }

    fn put(l: &Loader, at: u32, s: &str) -> u32 {
        l.guest_write_bytes(at, s.as_bytes()).unwrap();
        l.guest_write::<u8>(at + s.len() as u32, 0).unwrap();
        at
    }

    fn search(l: &Loader, flag: u32, path: &str, name: &str, cb: u32) -> Result<String, u32> {
        let rc = l.dos_search_path(flag, put(l, STR_A, path), put(l, STR_B, name), BUF, cb);
        if rc != NO_ERROR { return Err(rc); }
        Ok(l.read_guest_string(BUF))
    }

    #[test]
    fn path_helpers() {
        assert_eq!(split_path("a;;b ; \"c;d\";"), vec!["a", "b", "c;d"]);
        assert!(split_path(";;").is_empty());
        assert_eq!(split_path(&"x;".repeat(1000)).len(), MAX_PATH_ELEMENTS);
        assert_eq!(join_path("C:\\A", "F"), "C:\\A\\F");
        assert_eq!(join_path("C:\\A\\", "F"), "C:\\A\\F");
        assert_eq!(join_path("C:", "F"), "C:F");
        assert_eq!(qualify(2, "a/./b/../c", "F.TXT"), "C:\\a\\c\\F.TXT");
        assert_eq!(qualify(3, "", "F"), "D:\\F");
        assert_eq!(qualify(2, "../..", "F"), "C:\\F");
    }

    #[test]
    fn search_path_finds_the_first_match_in_order() {
        let (l, _tmp) = loader();
        assert_eq!(search(&l, SEARCH_PATH, "C:\\BIN;C:\\LIB", "A.DLL", 260), Ok("C:\\LIB\\A.DLL".into()));
        assert_eq!(search(&l, SEARCH_PATH, "C:\\NOPE;Q:\\X;\\\\srv\\share;C:\\BIN\\", "tool.exe", 260),
                   Ok("C:\\BIN\\tool.exe".into()));
        assert_eq!(search(&l, SEARCH_PATH, "C:\\BIN", "A.DLL", 260), Err(ERROR_FILE_NOT_FOUND));
        assert_eq!(search(&l, SEARCH_PATH, "C:\\LIB", "SUB.DIR", 260), Err(ERROR_FILE_NOT_FOUND),
                   "directories do not match");
        assert_eq!(search(&l, SEARCH_PATH, "", "ROOT.TXT", 260), Err(ERROR_FILE_NOT_FOUND));
        assert_eq!(search(&l, SEARCH_PATH, "C:\\", "", 260), Err(ERROR_FILE_NOT_FOUND));
        assert_eq!(search(&l, SEARCH_PATH, "C:\\BIN\\..\\LIB", "B.DLL", 260), Ok("C:\\LIB\\B.DLL".into()));
        assert_eq!(search(&l, SEARCH_PATH, "C:\\", "LIB\\A.DLL", 260), Ok("C:\\LIB\\A.DLL".into()));
    }

    #[test]
    fn search_path_current_directory_and_environment() {
        let (l, _tmp) = loader();
        l.shared.drive_mgr.lock_or_recover().set_current_dir("C:\\LIB").unwrap();
        assert_eq!(search(&l, SEARCH_PATH, "C:\\BIN", "A.DLL", 260), Err(ERROR_FILE_NOT_FOUND));
        assert_eq!(search(&l, SEARCH_CUR_DIRECTORY, "C:\\BIN", "A.DLL", 260), Ok("C:\\LIB\\A.DLL".into()));
        assert_eq!(search(&l, SEARCH_PATH, ".;C:\\BIN", "A.DLL", 260), Ok("C:\\LIB\\A.DLL".into()));
        assert_eq!(search(&l, SEARCH_ENVIRONMENT, "MYPATH", "B.DLL", 260), Ok("C:\\LIB\\B.DLL".into()));
        assert_eq!(search(&l, SEARCH_ENVIRONMENT | SEARCH_IGNORENETERRS, "mypath", "B.DLL", 260),
                   Ok("C:\\LIB\\B.DLL".into()));
        assert_eq!(search(&l, SEARCH_ENVIRONMENT, "NOSUCH", "B.DLL", 260), Err(ERROR_ENVVAR_NOT_FOUND));
        assert_eq!(search(&l, SEARCH_ENVIRONMENT, "EMPTY", "B.DLL", 260), Err(ERROR_FILE_NOT_FOUND));
    }

    #[test]
    fn search_path_wildcards_buffer_and_flags() {
        let (l, _tmp) = loader();
        assert_eq!(search(&l, SEARCH_PATH, "C:\\BIN;C:\\LIB", "*.DLL", 260), Ok("C:\\LIB\\*.DLL".into()));
        assert_eq!(search(&l, SEARCH_PATH, "C:\\LIB", "?.DLL", 260), Ok("C:\\LIB\\?.DLL".into()));
        assert_eq!(search(&l, SEARCH_PATH, "C:\\BIN", "*.DLL", 260), Err(ERROR_FILE_NOT_FOUND));
        // "C:\LIB\A.DLL" + NUL is 13 bytes.
        assert_eq!(search(&l, SEARCH_PATH, "C:\\LIB", "A.DLL", 13), Ok("C:\\LIB\\A.DLL".into()));
        l.guest_write::<u8>(BUF, 0x55).unwrap();
        assert_eq!(search(&l, SEARCH_PATH, "C:\\LIB", "A.DLL", 12), Err(ERROR_BUFFER_OVERFLOW));
        assert_eq!(l.guest_read::<u8>(BUF), Some(0x55), "nothing written");
        assert_eq!(search(&l, 8, "C:\\LIB", "A.DLL", 260), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(l.dos_search_path(0, 0, put(&l, STR_B, "A.DLL"), BUF, 260), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_search_path(0, put(&l, STR_A, "C:\\LIB"), 0, BUF, 260), ERROR_INVALID_PARAMETER);
        let rc = l.dos_search_path(0, put(&l, STR_A, "C:\\LIB"), put(&l, STR_B, "A.DLL"), u32::MAX - 4, 260);
        assert_eq!(rc, ERROR_INVALID_PARAMETER, "unwritable buffer");
    }

    fn register(l: &Loader, name: &str, builtin: bool, path: Option<&str>) -> u32 {
        let mut dm = l.shared.dll_mgr.lock_or_recover();
        let handle = dm.alloc_handle();
        dm.register(LoadedDll {
            name: name.into(), handle, object_bases: vec![], exports_by_ordinal: HashMap::new(),
            exports_by_name: HashMap::new(), ref_count: 1, initterm_addr: None, is_builtin: builtin,
        });
        if let Some(p) = path { dm.set_module_path(handle, p.into()); }
        handle
    }

    fn module_name(l: &Loader, hmod: u32, cb: u32) -> Result<String, u32> {
        let rc = l.dos_query_module_name(hmod, cb, BUF);
        if rc != NO_ERROR { return Err(rc); }
        Ok(l.read_guest_string(BUF))
    }

    #[test]
    fn query_module_name_of_exe_dlls_and_builtins() {
        let (l, _tmp) = loader();
        *l.shared.exe_name.lock_or_recover() = "samples/x/myapp.exe".into();
        assert_eq!(module_name(&l, EXE_MODULE_HANDLE, 260), Ok("C:\\MYAPP.EXE".into()));
        let dll = register(&l, "MYDLL", false, Some("C:\\LIB\\MYDLL.DLL"));
        assert_eq!(module_name(&l, dll, 260), Ok("C:\\LIB\\MYDLL.DLL".into()));
        let builtin = register(&l, "UCONV", true, None);
        assert_eq!(module_name(&l, builtin, 260), Ok("C:\\OS2\\DLL\\UCONV.DLL".into()));
        assert_eq!(module_name(&l, 0xDEAD, 260), Err(ERROR_INVALID_HANDLE));
        // "C:\MYAPP.EXE" + NUL is 13 bytes.
        assert_eq!(module_name(&l, EXE_MODULE_HANDLE, 13), Ok("C:\\MYAPP.EXE".into()));
        assert_eq!(module_name(&l, EXE_MODULE_HANDLE, 12), Err(ERROR_BAD_LENGTH));
        assert_eq!(l.dos_query_module_name(EXE_MODULE_HANDLE, 260, u32::MAX - 2), ERROR_INVALID_PARAMETER);
        // The last DosFreeModule forgets the path.
        assert!(l.shared.dll_mgr.lock_or_recover().decrement_refcount(dll).is_some());
        assert_eq!(module_name(&l, dll, 260), Err(ERROR_INVALID_HANDLE));
        assert!(l.shared.dll_mgr.lock_or_recover().module_path(dll).is_none());
    }

    #[test]
    fn dll_paths_follow_where_the_file_is() {
        let (l, tmp) = loader();
        let exe = tmp.path().join("BIN/TOOL.EXE");
        *l.shared.exe_name.lock_or_recover() = exe.to_string_lossy().into_owned();
        // Next to the executable: the executable's directory, C:\.
        let beside = tmp.path().join("BIN/near.dll");
        std::fs::write(&beside, b"x").unwrap();
        assert_eq!(l.dll_os2_path("NEAR", &beside.to_string_lossy()), "C:\\NEAR.DLL");
        // Elsewhere on drive C: its OS/2 path.
        let on_c = tmp.path().join("LIB/A.DLL");
        assert_eq!(l.dll_os2_path("A", &on_c.to_string_lossy()), "C:\\LIB\\A.DLL");
        // Outside every drive: the system DLL directory.
        let outside = tempfile::TempDir::new().unwrap();
        let far = outside.path().join("far.dll");
        std::fs::write(&far, b"x").unwrap();
        assert_eq!(l.dll_os2_path("FAR", &far.to_string_lossy()), "C:\\OS2\\DLL\\FAR.DLL");
    }

    #[test]
    fn host_paths_map_back_to_drives() {
        let (l, tmp) = loader();
        let dm = l.shared.drive_mgr.lock_or_recover();
        assert_eq!(dm.host_to_os2_path(&tmp.path().join("LIB/A.DLL")).as_deref(), Some("C:\\LIB\\A.DLL"));
        assert_eq!(dm.host_to_os2_path(tmp.path()).as_deref(), Some("C:\\"));
        assert_eq!(dm.host_to_os2_path(&tmp.path().join("missing")), None);
        assert_eq!(dm.host_to_os2_path(std::path::Path::new("/")), None);
    }
}
