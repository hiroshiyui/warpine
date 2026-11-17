// SPDX-License-Identifier: GPL-3.0-only
//
// PM last-error store: the `PMERR_*` codes that `WinGetLastError` reports.
//
// IBM's reference: "This call retrieves the last error code and resets it to
// zero." Errors are kept per thread ("The last error for the process and
// thread on which this function call is made will be returned"), as a 32-bit
// ERRORID whose high 16 bits are a severity and whose low 16 bits are the
// error code.

use std::collections::HashMap;
use std::sync::Mutex;

use super::constants::{HWND_DESKTOP, HWND_OBJECT, PMERR_INVALID_HWND, SEVERITY_ERROR};
use super::mutex_ext::MutexExt;
use super::pm_types::WindowManager;
use super::ApiResult;

/// The ERRORID of `code` with `severity` (`MAKEERRORID` of `<os2def.h>`).
pub fn error_id(severity: u32, code: u32) -> u32 {
    (severity << 16) | (code & 0xFFFF)
}

/// Last PM error of every thread, keyed by thread (vCPU) id.
///
/// Its lock is a leaf: it is taken alone, so errors may be recorded while
/// the window manager is locked.
#[derive(Default)]
pub struct PmErrorStore {
    last: Mutex<HashMap<u32, u32>>,
}

impl PmErrorStore {
    /// Record `PMERR_*` `code` as thread `tid`'s last error. Every code
    /// is recorded with `SEVERITY_ERROR`. A later success does not clear it.
    pub fn set(&self, tid: u32, code: u32) {
        self.last.lock_or_recover().insert(tid, error_id(SEVERITY_ERROR, code));
    }

    /// Thread `tid`'s last error (0 = none), reset to 0 (`WinGetLastError`).
    pub fn take(&self, tid: u32) -> u32 {
        self.last.lock_or_recover().remove(&tid).unwrap_or(0)
    }
}

/// Whether `hwnd` names a window a PM call may be given: an existing window
/// or the desktop / object-window pseudo-handles, which Warpine does not
/// keep as windows.
pub fn valid_hwnd(wm: &WindowManager, hwnd: u32) -> bool {
    hwnd == HWND_DESKTOP || hwnd == HWND_OBJECT || wm.is_window(hwnd)
}

impl super::Loader {
    /// Record `PMERR_*` `code` as the last error of thread `tid`.
    pub(super) fn pm_error(&self, tid: u32, code: u32) {
        self.shared.pm_errors.set(tid, code);
    }

    /// Record `code` for thread `tid` and return the failure value 0
    /// (FALSE / NULLHANDLE).
    pub(super) fn pm_fail(&self, tid: u32, code: u32) -> ApiResult {
        self.pm_error(tid, code);
        ApiResult::Normal(0)
    }

    /// Whether `hwnd` is a valid window for a call of thread `tid`
    /// ([`valid_hwnd`]; with `allow_null`, NULLHANDLE too); if not,
    /// `PMERR_INVALID_HWND` becomes the thread's last error.
    pub(super) fn pm_check_hwnd(&self, tid: u32, hwnd: u32, allow_null: bool) -> bool {
        let ok = (allow_null && hwnd == 0) || valid_hwnd(&self.shared.window_mgr.lock_or_recover(), hwnd);
        if !ok {
            self.pm_error(tid, PMERR_INVALID_HWND);
        }
        ok
    }

    /// Whether thread `tid` has a message queue.
    pub(super) fn has_msg_queue(&self, tid: u32) -> bool {
        self.shared.window_mgr.lock_or_recover().thread_queue(tid).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::super::constants::*;
    use super::super::mutex_ext::MutexExt;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::{ApiResult, Loader};
    use super::*;

    const ORD_WIN_CREATE_MSG_QUEUE: u32 = 716;
    const ORD_WIN_DESTROY_MSG_QUEUE: u32 = 726;
    const ORD_WIN_DESTROY_WINDOW: u32 = 728;
    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_QUERY_WINDOW_TEXT: u32 = 841;
    const ORD_WIN_SET_WINDOW_POS: u32 = 875;
    const ORD_WIN_SET_WINDOW_TEXT: u32 = 877;
    const ORD_WIN_WINDOW_FROM_ID: u32 = 899;
    const ORD_WIN_GET_MSG: u32 = 915;
    const ORD_WIN_PEEK_MSG: u32 = 918;
    const ORD_WIN_POST_MSG: u32 = 919;
    const ORD_WIN_SEND_MSG: u32 = 920;

    /// A handle that is no window.
    const BAD_HWND: u32 = 0xDEAD;

    fn call(loader: &Loader, tid: u32, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
        match loader.handle_pmwin_call(&mut vcpu, tid, ordinal) {
            ApiResult::Normal(v) => v,
            other => panic!("PMWIN ordinal {} returned {:?}", ordinal, other),
        }
    }

    fn last_error(loader: &Loader, tid: u32) -> u32 {
        call(loader, tid, ORD_WIN_GET_LAST_ERROR, &[MOCK_HAB])
    }

    fn err(code: u32) -> u32 {
        error_id(SEVERITY_ERROR, code)
    }

    #[test]
    fn error_id_puts_severity_in_the_high_word() {
        assert_eq!(error_id(SEVERITY_ERROR, PMERR_INVALID_HWND), 0x0008_1001);
        assert_eq!(error_id(SEVERITY_WARNING, 0x1_2345), 0x0004_2345, "code is 16 bits");
        assert_eq!(error_id(SEVERITY_NOERROR, 0), 0);
    }

    #[test]
    fn store_is_per_thread_and_reading_resets() {
        let store = PmErrorStore::default();
        assert_eq!(store.take(1), 0, "no error yet");
        store.set(1, PMERR_INVALID_HMQ);
        store.set(2, PMERR_INVALID_HWND);
        store.set(1, PMERR_NO_MSG_QUEUE);
        assert_eq!(store.take(1), err(PMERR_NO_MSG_QUEUE), "the last one wins");
        assert_eq!(store.take(1), 0, "reset by reading");
        assert_eq!(store.take(2), err(PMERR_INVALID_HWND), "other thread untouched");
    }

    #[test]
    fn win_get_last_error_starts_at_zero() {
        let loader = Loader::new_mock();
        assert_eq!(last_error(&loader, 0), 0);
    }

    #[test]
    fn second_message_queue_reports_already_exists() {
        let loader = Loader::new_mock();
        assert_ne!(call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]), 0);
        assert_eq!(last_error(&loader, 0), 0, "success records nothing");
        assert_eq!(call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]), 0);
        assert_eq!(last_error(&loader, 1), 0, "another thread sees no error");
        assert_eq!(last_error(&loader, 0), err(PMERR_MSG_QUEUE_ALREADY_EXISTS));
        assert_eq!(last_error(&loader, 0), 0, "reset by WinGetLastError");
    }

    #[test]
    fn destroying_a_foreign_or_bad_queue_reports_invalid_hmq() {
        let loader = Loader::new_mock();
        let hmq = call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        assert_eq!(call(&loader, 1, ORD_WIN_DESTROY_MSG_QUEUE, &[hmq]), 0);
        assert_eq!(last_error(&loader, 1), err(PMERR_INVALID_HMQ), "not the creator");
        assert_eq!(call(&loader, 0, ORD_WIN_DESTROY_MSG_QUEUE, &[hmq + 99]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_HMQ), "no such queue");
        assert_eq!(call(&loader, 0, ORD_WIN_DESTROY_MSG_QUEUE, &[hmq]), 1);
        assert_eq!(last_error(&loader, 0), 0);
    }

    #[test]
    fn a_success_does_not_clear_the_last_error() {
        let loader = Loader::new_mock();
        call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        let hwnd = loader.shared.window_mgr.lock_or_recover().create_window("App".into(), 0, 1);
        assert_eq!(call(&loader, 0, ORD_WIN_SET_WINDOW_TEXT, &[hwnd, 0]), 1);
        assert_eq!(last_error(&loader, 0), err(PMERR_MSG_QUEUE_ALREADY_EXISTS));
    }

    #[test]
    fn calls_with_a_bad_hwnd_report_invalid_hwnd() {
        let loader = Loader::new_mock();
        let hmq = call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        let buf = 0x2000;
        let cases: &[(&str, u32, Vec<u32>)] = &[
            ("WinDestroyWindow", ORD_WIN_DESTROY_WINDOW, vec![BAD_HWND]),
            ("WinSetWindowPos", ORD_WIN_SET_WINDOW_POS, vec![BAD_HWND, 0, 0, 0, 10, 10, SWP_SIZE]),
            ("WinSetWindowText", ORD_WIN_SET_WINDOW_TEXT, vec![BAD_HWND, 0]),
            ("WinQueryWindowText", ORD_WIN_QUERY_WINDOW_TEXT, vec![BAD_HWND, 16, buf]),
            ("WinWindowFromID", ORD_WIN_WINDOW_FROM_ID, vec![BAD_HWND, 1]),
            ("WinPostMsg", ORD_WIN_POST_MSG, vec![BAD_HWND, WM_USER, 0, 0]),
            ("WinSendMsg", ORD_WIN_SEND_MSG, vec![BAD_HWND, WM_USER, 0, 0]),
        ];
        for (name, ordinal, args) in cases {
            assert_eq!(call(&loader, 0, *ordinal, args), 0, "{name} fails");
            assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_HWND), "{name}");
        }
        // Nothing reached the queue.
        let mq = loader.shared.window_mgr.lock_or_recover().get_mq(hmq).unwrap();
        assert!(mq.lock_or_recover().messages.is_empty());
    }

    #[test]
    fn calls_with_a_good_hwnd_record_nothing() {
        let loader = Loader::new_mock();
        let hmq = call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        let hwnd = loader.shared.window_mgr.lock_or_recover().create_window("App".into(), 0, hmq);
        assert_eq!(call(&loader, 0, ORD_WIN_SET_WINDOW_POS, &[hwnd, 0, 0, 0, 10, 10, SWP_SIZE]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_POST_MSG, &[hwnd, WM_USER, 0, 0]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_WINDOW_FROM_ID, &[HWND_DESKTOP, 42]), 0, "no such child");
        assert_eq!(last_error(&loader, 0), 0, "the desktop is a valid parent");
        assert_eq!(call(&loader, 0, ORD_WIN_DESTROY_WINDOW, &[hwnd]), 1);
        assert_eq!(last_error(&loader, 0), 0);
    }

    #[test]
    fn get_or_peek_without_a_queue_reports_no_msg_queue() {
        let loader = Loader::new_mock();
        let qmsg = 0x2000;
        assert_eq!(call(&loader, 0, ORD_WIN_PEEK_MSG, &[MOCK_HAB, qmsg, 0, 0, 0, PM_REMOVE]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_NO_MSG_QUEUE), "WinPeekMsg");
        assert_eq!(call(&loader, 0, ORD_WIN_GET_MSG, &[MOCK_HAB, qmsg, 0, 0, 0]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_NO_MSG_QUEUE), "WinGetMsg");
        call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        assert_eq!(call(&loader, 0, ORD_WIN_PEEK_MSG, &[MOCK_HAB, qmsg, 0, 0, 0, PM_REMOVE]), 0);
        assert_eq!(last_error(&loader, 0), 0, "an empty queue is no error");
    }
}
