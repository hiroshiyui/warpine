// SPDX-License-Identifier: GPL-3.0-or-later
//
// Starting applications (PMSHAPI), after the IBM PM Reference:
// `WinStartApp` (119) and `WinTerminateApp` (130).
//
// `WinStartApp(hwndNotify, pDetails, pszParams, pReserved, fbOptions)`
// starts the program a `PROGDETAILS` names. Every OS/2 process is a
// Warpine process of its own (ADR-0020), so, as for `DosExecPgm`
// (`process.rs`), Warpine runs itself on the program, which gets its
// command line and environment through the host `exec` (`inherit.rs`,
// ADR-0022): `WARPINE_OS2ARG0` the executable as named, `WARPINE_OS2ARGS`
// the parameters, `WARPINE_OS2ENV` this process's environment with the
// `PROGDETAILS` strings added (a name given there replaces the inherited
// value). The program runs in a session of its own: it does not inherit
// open handles (no `WARPINE_HANDLES`; the host descriptors stay
// close-on-exec), its standard input is empty and its output goes where
// this process's goes; it starts in `pszStartupDir`. A watcher thread
// polls the host process and, when it ends, posts
// `WM_APPTERMINATENOTIFY(happ, rc)` to `hwndNotify`.
//
// `WinTerminateApp(happ)` ends an application this process started (the
// host process is killed; the notification still follows).
//
// Unverified (the Reference is silent or this is a choice of Warpine's):
// the parameters are `PROGDETAILS.pszParameters` with
// `SAF_INSTALLEDCMDLINE`, otherwise `pszParams`, and those of
// `PROGDETAILS` when `pszParams` is NULL; the `PROGDETAILS` environment is
// merged into the inherited one; `Length` is not checked (the fields up
// to `pszEnvironment` are read); `SAF_MAXIMIZED` / `SAF_MINIMIZED` /
// `SAF_BACKGROUND`, the title, icon and `swpInitial` are not applied; a
// program started with `SAF_STARTCHILDAPP` is not ended with this
// process; a killed program's code is 1; the program categories other
// than default, full screen, windowable VIO and PM fail with
// `PMERR_INVALID_PROGRAM_CATEGORY`, a missing or non-OS/2 executable with
// `PMERR_INVALID_APPL`, a host start failure with `PMERR_CANNOT_START`.

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use log::{debug, warn};

use super::constants::*;
use super::inherit;
use super::mutex_ext::MutexExt;
use super::pm_switch::PROG_PM;

/// `WM_APPTERMINATENOTIFY` (`<pmwin.h>`): mp1 the HAPP, mp2 the return code.
pub const WM_APPTERMINATENOTIFY: u32 = 0x002E;

// fbOptions (<pmwin.h>, INCL_WINPROGRAMLIST).
pub const SAF_INSTALLEDCMDLINE: u32 = 0x0001;
pub const SAF_STARTCHILDAPP: u32 = 0x0002;
pub const SAF_VALIDFLAGS: u32 = 0x001F;

// PROGCATEGORY (<pmwin.h>).
pub const PROG_DEFAULT: u32 = 0;
pub const PROG_FULLSCREEN: u32 = 1;
pub const PROG_WINDOWABLEVIO: u32 = 2;

/// `PMERR_INVALID_PROGRAM_CATEGORY` (`<pmerr.h>`).
pub const PMERR_INVALID_PROGRAM_CATEGORY: u32 = 0x152F;
/// `PMERR_INVALID_APPL` (`<pmerr.h>`).
pub const PMERR_INVALID_APPL: u32 = 0x1530;
/// `PMERR_CANNOT_START` (`<pmerr.h>`).
pub const PMERR_CANNOT_START: u32 = 0x1531;
/// `PMERR_INVALID_HAPP` (`<pmerr.h>`).
pub const PMERR_INVALID_HAPP: u32 = 0x1533;

// PROGDETAILS field offsets (<pmwin.h>): Length, PROGTYPE (progc,
// fbVisible), then pointers.
const PD_PROGC: u32 = 4;
const PD_EXECUTABLE: u32 = 16;
const PD_PARAMETERS: u32 = 20;
const PD_STARTUP_DIR: u32 = 24;
const PD_ENVIRONMENT: u32 = 32;
/// Bytes of `PROGDETAILS` read: up to `pszEnvironment` (`swpInitial`,
/// which follows, is not used).
const PD_READ: usize = 36;

/// `pib_pchenv`: offset of the environment pointer in the PIB.
const PIB_ENV_OFFSET: u32 = 0x10;
/// Longest executable path or start-up directory taken.
const MAX_APP_PATH: usize = 260;
/// How often the watcher looks whether the program ended.
const APP_POLL: Duration = Duration::from_millis(50);
/// Most applications running at once from one process.
pub const MAX_APPS: usize = 64;
/// The first HAPP.
const FIRST_HAPP: u32 = 0x0002_0001;
/// Return code reported for a program that ended without one (killed).
const KILLED_RC: u32 = 1;

/// An application `WinStartApp` started.
struct App {
    child: Child,
    notify: u32,
}

/// Applications started by this process (`WindowManager::apps`).
pub struct AppTable {
    apps: HashMap<u32, App>,
    next: u32,
}

impl Default for AppTable {
    fn default() -> Self { AppTable { apps: HashMap::new(), next: FIRST_HAPP } }
}

impl std::fmt::Debug for AppTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppTable").field("apps", &self.apps.keys().collect::<Vec<_>>()).finish()
    }
}

impl AppTable {
    fn insert(&mut self, child: Child, notify: u32) -> u32 {
        let h = self.next;
        self.next += 1;
        self.apps.insert(h, App { child, notify });
        h
    }

    /// Whether application `happ` ended: its host pid and status (`None`:
    /// it cannot be known) and the window to notify, the application then
    /// forgotten.
    fn poll(&mut self, happ: u32) -> Option<(u32, Option<std::process::ExitStatus>, u32)> {
        let app = self.apps.get_mut(&happ)?;
        let host = app.child.id();
        match app.child.try_wait() {
            Ok(Some(status)) => {
                let notify = app.notify;
                self.apps.remove(&happ);
                Some((host, Some(status), notify))
            }
            Ok(None) => None,
            Err(e) => {
                warn!("WinStartApp: cannot wait for application {happ:#x}: {e}");
                let notify = self.apps.remove(&happ).map_or(0, |a| a.notify);
                Some((host, None, notify))
            }
        }
    }

    /// Whether `happ` is still running.
    pub fn contains(&self, happ: u32) -> bool { self.apps.contains_key(&happ) }

    pub fn len(&self) -> usize { self.apps.len() }

    pub fn is_empty(&self) -> bool { self.apps.is_empty() }

    /// `WinTerminateApp`: kill the application's host process.
    fn kill(&mut self, happ: u32) -> Result<(), u32> {
        let app = self.apps.get_mut(&happ).ok_or(PMERR_INVALID_HAPP)?;
        app.child.kill().map_err(|_| PMERR_INVALID_HAPP)
    }
}

/// The parameters an application gets (see the module comment).
pub fn app_parameters(options: u32, installed: Option<Vec<u8>>, given: Option<Vec<u8>>) -> Vec<u8> {
    if options & SAF_INSTALLEDCMDLINE != 0 {
        installed.unwrap_or_default()
    } else {
        given.or(installed).unwrap_or_default()
    }
}

/// The environment `base` with the strings of `extra` added; a name in
/// `extra` (compared without regard to ASCII case) replaces the one in
/// `base`.
pub fn merge_env(base: Vec<Vec<u8>>, extra: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let name = |s: &[u8]| -> Vec<u8> {
        s.split(|&b| b == b'=').next().unwrap_or(&[]).to_ascii_uppercase()
    };
    let mut out = base;
    for s in extra {
        let n = name(&s);
        match out.iter_mut().find(|o| name(o) == n) {
            Some(o) => *o = s,
            None => out.push(s),
        }
    }
    out
}

/// Whether the file at `path` starts like an OS/2 executable (an MZ stub,
/// or a bare LX / NE header).
fn is_executable(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut sig = [0u8; 2];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut sig)).is_ok()
        && matches!(&sig, b"MZ" | b"LX" | b"NE")
}

impl super::Loader {
    /// The bytes of a NUL-terminated string field of a structure (none for
    /// a NULL pointer), bounded.
    fn app_string(&self, ptr: u32, max: usize) -> Result<Option<Vec<u8>>, u32> {
        if ptr == 0 { return Ok(None); }
        self.guest_cstr(ptr, max).map(Some).ok_or(PMERR_INVALID_PARM)
    }

    /// `WinStartApp(hwndNotify, pDetails, pszParams, pReserved, fbOptions)`
    /// → HAPP (NULLHANDLE on failure).
    pub fn win_start_app(&self, tid: u32, notify: u32, p_details: u32, psz_params: u32,
                         options: u32) -> u32 {
        match self.start_app(tid, notify, p_details, psz_params, options) {
            Ok(h) => h,
            Err(code) => { self.pm_error(tid, code); 0 }
        }
    }

    fn start_app(&self, tid: u32, notify: u32, p_details: u32, psz_params: u32, options: u32)
        -> Result<u32, u32>
    {
        if notify != 0 && !self.pm_check_hwnd(tid, notify, false) {
            return Err(PMERR_INVALID_HWND);
        }
        if options & !SAF_VALIDFLAGS != 0 {
            return Err(PMERR_INVALID_FLAG);
        }
        if p_details == 0 { return Err(PMERR_INVALID_PARM); }
        let pd = self.guest_read_bytes(p_details, PD_READ).ok_or(PMERR_INVALID_PARM)?;
        let field = |at: u32| u32::from_le_bytes(pd[at as usize..at as usize + 4].try_into().unwrap_or([0; 4]));
        let category = field(PD_PROGC);
        if !matches!(category, PROG_DEFAULT | PROG_FULLSCREEN | PROG_WINDOWABLEVIO | PROG_PM) {
            return Err(PMERR_INVALID_PROGRAM_CATEGORY);
        }
        let exe = self.app_string(field(PD_EXECUTABLE), MAX_APP_PATH)?
            .filter(|e| !e.is_empty()).ok_or(PMERR_INVALID_APPL)?;
        let installed = self.app_string(field(PD_PARAMETERS), inherit::MAX_ARGS)?;
        let given = self.app_string(psz_params, inherit::MAX_ARGS)?;
        let params = app_parameters(options, installed, given);
        let start_dir = self.app_string(field(PD_STARTUP_DIR), MAX_APP_PATH)?;
        let env_ptr = field(PD_ENVIRONMENT);
        let extra_env = if env_ptr != 0 { self.read_env_block(env_ptr) } else { Vec::new() };

        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let exe_name = super::codepage::cp_decode(&exe, cp);
        let prog_path = self.resolve_program(&exe_name)
            .filter(|p| p.is_file() && is_executable(p)).ok_or(PMERR_INVALID_APPL)?;
        // The child's OS/2 current drive and directories: this process's,
        // with the start directory (if it resolves) as the current one.
        let start_dir = start_dir.filter(|d| !d.is_empty()).map(|d| super::codepage::cp_decode(&d, cp));
        let cwd = inherit::cwd_strings(&self.shared.drive_mgr.lock_or_recover(), start_dir.as_deref());

        if self.shared.window_mgr.lock_or_recover().apps.len() >= MAX_APPS {
            return Err(PMERR_CANNOT_START);
        }
        let warpine = std::env::current_exe().map_err(|_| PMERR_CANNOT_START)?;
        let mut cmd = Command::new(warpine);
        // Bounded nesting and number of children (inherit.rs).
        // A slot of the session's THREADS= limit too (no PM error of its
        // own in <pmerr.h>: PMERR_CANNOT_START).
        let slot = self.admit_child(&mut cmd).map_err(|_| PMERR_CANNOT_START)?;
        cmd.arg(&prog_path);
        // A session of its own: nothing of this process's start-up or
        // handles, only what is set here.
        inherit::clear_parent_vars(&mut cmd);
        // A scripted session (tests) drives this process's desktop only.
        cmd.env_remove(crate::gui::gui_script::SCRIPT_ENV);
        cmd.env(inherit::ARG0_ENV, inherit::escape(&exe));
        cmd.env(inherit::ARGS_ENV, inherit::escape(&params));
        let own_env_ptr = self.guest_read::<u32>(PIB_BASE + PIB_ENV_OFFSET).unwrap_or(0);
        let own_env = if own_env_ptr != 0 { self.read_env_block(own_env_ptr) } else { Vec::new() };
        let mut block = inherit::join_env(&merge_env(own_env, extra_env));
        block.pop(); // the strings with their NULs; the reader adds the end
        cmd.env(inherit::ENV_ENV, inherit::escape(&block));
        match self.shared.npipes.lock_or_recover().dir.path() {
            Some(dir) => { cmd.env(super::npipe::PIPE_DIR_ENV, dir); }
            None => { cmd.env_remove(super::npipe::PIPE_DIR_ENV); }
        }
        match category {
            PROG_FULLSCREEN => { cmd.env(crate::gui::vio_window::VIO_ENV, "fullscreen"); }
            PROG_WINDOWABLEVIO => { cmd.env(crate::gui::vio_window::VIO_ENV, "window"); }
            _ => {}
        }
        cmd.env(inherit::CWD_ENV, inherit::escape(&cwd));
        cmd.stdin(Stdio::null());
        debug!("WinStartApp: {} {:?}", prog_path.display(), String::from_utf8_lossy(&params));
        let spawned = cmd.spawn();
        self.child_started(slot, spawned.as_ref().ok().map(std::process::Child::id));
        let child = spawned.map_err(|e| {
            warn!("WinStartApp: cannot start {}: {e}", prog_path.display());
            PMERR_CANNOT_START
        })?;
        let happ = self.shared.window_mgr.lock_or_recover().apps.insert(child, notify);
        let shared = Arc::clone(&self.shared);
        std::thread::Builder::new().name(format!("app-{happ:x}")).spawn(move || loop {
            std::thread::sleep(APP_POLL);
            let mut wm = shared.window_mgr.lock_or_recover();
            if !wm.apps.contains(happ) { break; }
            if let Some((host, status, hwnd)) = wm.apps.poll(happ) {
                // Its whole return code (exit_report.rs); the report's pipe closes either way.
                let rc = match status {
                    Some(s) => shared.child_exit_code(host, s).unwrap_or(KILLED_RC),
                    None => { shared.exit_reports.lock_or_recover().remove(&host); KILLED_RC }
                };
                if hwnd != 0 && wm.get_window(hwnd).is_some() {
                    wm.post_msg(hwnd, WM_APPTERMINATENOTIFY, happ, rc);
                }
                break;
            }
        }).map_err(|_| PMERR_CANNOT_START)?;
        Ok(happ)
    }

    /// `WinTerminateApp(happ)` → BOOL.
    pub fn win_terminate_app(&self, tid: u32, happ: u32) -> u32 {
        match self.shared.window_mgr.lock_or_recover().apps.kill(happ) {
            Ok(()) => 1,
            Err(code) => { self.pm_error(tid, code); 0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<u8> { s.as_bytes().to_vec() }

    #[test]
    fn parameters_follow_the_options() {
        assert_eq!(app_parameters(0, Some(v("inst")), Some(v("given"))), v("given"));
        assert_eq!(app_parameters(0, Some(v("inst")), None), v("inst"));
        assert_eq!(app_parameters(0, None, None), v(""));
        assert_eq!(app_parameters(SAF_INSTALLEDCMDLINE, Some(v("inst")), Some(v("given"))), v("inst"));
        assert_eq!(app_parameters(SAF_INSTALLEDCMDLINE, None, Some(v("given"))), v(""));
    }

    #[test]
    fn program_environment_overrides_by_name() {
        let base = vec![v("PATH=C:\\OS2"), v("Mode=old")];
        let merged = merge_env(base, vec![v("MODE=new"), v("EXTRA=1")]);
        assert_eq!(merged, vec![v("PATH=C:\\OS2"), v("MODE=new"), v("EXTRA=1")]);
    }

    #[test]
    fn executables_are_recognised_by_their_signature() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("a.exe");
        std::fs::write(&exe, b"MZ\x90\0").unwrap();
        assert!(is_executable(&exe));
        let txt = dir.path().join("a.txt");
        std::fs::write(&txt, b"hello").unwrap();
        assert!(!is_executable(&txt));
        assert!(!is_executable(&dir.path().join("missing")));
    }

    #[test]
    fn the_table_reports_ended_applications_once() {
        let mut t = AppTable::default();
        let child = Command::new("true").spawn().unwrap();
        let h = t.insert(child, 0x1234);
        assert!(t.contains(h));
        let mut ended = None;
        for _ in 0..200 {
            if let Some(r) = t.poll(h) { ended = Some(r); break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (_, status, notify) = ended.unwrap();
        assert_eq!((status.and_then(|s| s.code()), notify), (Some(0), 0x1234));
        assert!(!t.contains(h));
        assert_eq!(t.poll(h), None);
        assert_eq!(t.kill(h), Err(PMERR_INVALID_HAPP));
        assert!(t.is_empty());
    }

    #[test]
    fn a_killed_application_reports_the_killed_code() {
        let mut t = AppTable::default();
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let h = t.insert(child, 0);
        assert_eq!(t.kill(h), Ok(()));
        let mut ended = None;
        for _ in 0..200 {
            if let Some(r) = t.poll(h) { ended = Some(r); break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (_, status, _) = ended.unwrap();
        assert_eq!(status.and_then(|s| s.code()), None, "killed: no exit code, so KILLED_RC");
    }

    const TID: u32 = 1;
    const PD: u32 = 0x0010_0000;
    const EXE: u32 = 0x0010_1000;

    #[test]
    fn start_app_checks_its_arguments() {
        let l = crate::loader::Loader::new_mock();
        let err = |l: &crate::loader::Loader| l.shared.pm_errors.take(TID) & 0xFFFF;
        assert_eq!(l.win_start_app(TID, 0, 0, 0, 0), 0);
        assert_eq!(err(&l), PMERR_INVALID_PARM);
        assert_eq!(l.win_start_app(TID, 0x7777, PD, 0, 0), 0);
        assert_eq!(err(&l), PMERR_INVALID_HWND);
        assert_eq!(l.win_start_app(TID, 0, PD, 0, 0x100), 0);
        assert_eq!(err(&l), PMERR_INVALID_FLAG);
        let mut pd = [0u8; 72];
        pd[0..4].copy_from_slice(&72u32.to_le_bytes());
        pd[4..8].copy_from_slice(&4u32.to_le_bytes()); // PROG_REAL (DOS)
        l.guest_write_bytes(PD, &pd).unwrap();
        assert_eq!(l.win_start_app(TID, 0, PD, 0, 0), 0);
        assert_eq!(err(&l), PMERR_INVALID_PROGRAM_CATEGORY);
        pd[4..8].copy_from_slice(&0u32.to_le_bytes());
        l.guest_write_bytes(PD, &pd).unwrap();
        assert_eq!(l.win_start_app(TID, 0, PD, 0, 0), 0, "no executable");
        assert_eq!(err(&l), PMERR_INVALID_APPL);
        l.guest_write_bytes(EXE, b"C:\\NOWHERE\\MISSING.EXE\0").unwrap();
        pd[16..20].copy_from_slice(&EXE.to_le_bytes());
        l.guest_write_bytes(PD, &pd).unwrap();
        assert_eq!(l.win_start_app(TID, 0, PD, 0, 0), 0);
        assert_eq!(err(&l), PMERR_INVALID_APPL);
        assert_eq!(l.win_terminate_app(TID, FIRST_HAPP), 0);
        assert_eq!(err(&l), PMERR_INVALID_HAPP);
    }
}
