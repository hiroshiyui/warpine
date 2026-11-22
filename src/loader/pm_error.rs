// SPDX-License-Identifier: GPL-3.0-or-later
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

use super::constants::{HWND_DESKTOP, HWND_OBJECT, PMERR_INVALID_HWND, PMERR_PARAMETER_OUT_OF_RANGE,
                       QWL_MIN, QWS_MIN, SEVERITY_ERROR};
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

    /// Record a whole ERRORID (`WinSetErrorInfo`) as thread `tid`'s last
    /// error.
    pub fn set_id(&self, tid: u32, id: u32) {
        self.last.lock_or_recover().insert(tid, id);
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

/// Whether window-word `index` of `size` bytes (4 for `WinQuery/SetWindowULong`,
/// 2 for `...UShort`) is valid for `hwnd`.
///
/// The PM Reference: valid values are zero through `cbWindowData - size`
/// (the class's own words) plus the reserved `QWL_*` / `QWS_*` indices,
/// which are negative down to `QWL_MIN` / `QWS_MIN`. The upper bound is
/// enforced only where Warpine knows `cbWindowData`: windows of a class the
/// program registered, except frames (Warpine gives a standard frame its
/// client's class, but a frame's words are the frame class's `QWS_FLAGS`
/// ... `QWL_PSTATBLK`).
pub fn window_word_in_range(wm: &WindowManager, hwnd: u32, index: i32, size: u32) -> bool {
    if index < 0 {
        return index >= if size == 4 { QWL_MIN } else { QWS_MIN };
    }
    if wm.frame_to_client.contains_key(&hwnd) { return true; }
    let cb = wm.get_window(hwnd)
        .and_then(|w| wm.get_class(&w.class_name))
        .and_then(|c| c.cb_window_data);
    cb.is_none_or(|cb| index as u64 + size as u64 <= cb as u64)
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

    /// Whether `hwnd` is valid ([`Self::pm_check_hwnd`]) and window-word
    /// `index` of `size` bytes is in range for it ([`window_word_in_range`],
    /// else `PMERR_PARAMETER_OUT_OF_RANGE`).
    pub(super) fn pm_check_window_word(&self, tid: u32, hwnd: u32, index: i32, size: u32) -> bool {
        if !self.pm_check_hwnd(tid, hwnd, false) { return false; }
        let ok = window_word_in_range(&self.shared.window_mgr.lock_or_recover(), hwnd, index, size);
        if !ok {
            self.pm_error(tid, PMERR_PARAMETER_OUT_OF_RANGE);
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

    const ORD_WIN_BEGIN_PAINT: u32 = 703;
    const ORD_WIN_DISMISS_DLG: u32 = 729;
    const ORD_WIN_ENABLE_WINDOW: u32 = 735;
    const ORD_WIN_GET_PS: u32 = 757;
    const ORD_WIN_INVALIDATE_RECT: u32 = 765;
    const ORD_WIN_IS_WINDOW_ENABLED: u32 = 773;
    const ORD_WIN_OPEN_WINDOW_DC: u32 = 794;
    const ORD_WIN_PROCESS_DLG: u32 = 796;
    const ORD_WIN_QUERY_DLG_ITEM_TEXT: u32 = 815;
    const ORD_WIN_QUERY_WINDOW: u32 = 834;
    const ORD_WIN_QUERY_WINDOW_POS: u32 = 837;
    const ORD_WIN_QUERY_WINDOW_RECT: u32 = 840;
    const ORD_WIN_QUERY_WINDOW_ULONG: u32 = 843;
    const ORD_WIN_QUERY_WINDOW_USHORT: u32 = 844;
    const ORD_WIN_SET_ACCEL_TABLE: u32 = 850;
    const ORD_WIN_SET_ACTIVE_WINDOW: u32 = 851;
    const ORD_WIN_SET_CAPTURE: u32 = 852;
    const ORD_WIN_SET_DLG_ITEM_TEXT: u32 = 859;
    const ORD_WIN_SET_FOCUS: u32 = 860;
    const ORD_WIN_SET_WINDOW_ULONG: u32 = 878;
    const ORD_WIN_SET_WINDOW_USHORT: u32 = 879;
    const ORD_WIN_SHOW_WINDOW: u32 = 883;
    const ORD_WIN_STOP_TIMER: u32 = 885;
    const ORD_WIN_UPDATE_WINDOW: u32 = 892;
    const ORD_WIN_VALIDATE_RECT: u32 = 895;
    const ORD_WIN_QUERY_UPDATE_RECT: u32 = 831;
    const ORD_WIN_SCROLL_WINDOW: u32 = 849;
    const ORD_WIN_SEND_DLG_ITEM_MSG: u32 = 903;
    const ORD_WIN_CREATE_WINDOW: u32 = 909;
    const ORD_WIN_REGISTER_CLASS: u32 = 926;
    const ORD_WIN_SUBCLASS_WINDOW: u32 = 929;

    #[test]
    fn more_calls_with_a_bad_hwnd_report_invalid_hwnd() {
        let loader = Loader::new_mock();
        call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        let buf = 0x2000;
        let b = BAD_HWND;
        let cases: &[(&str, u32, Vec<u32>, u32)] = &[
            ("WinShowWindow", ORD_WIN_SHOW_WINDOW, vec![b, 1], 0),
            ("WinQueryWindowRect", ORD_WIN_QUERY_WINDOW_RECT, vec![b, buf], 0),
            ("WinSetWindowULong", ORD_WIN_SET_WINDOW_ULONG, vec![b, 0, 7], 0),
            ("WinQueryWindowULong", ORD_WIN_QUERY_WINDOW_ULONG, vec![b, QWL_STYLE as u32], 0),
            ("WinSetWindowUShort", ORD_WIN_SET_WINDOW_USHORT, vec![b, 0, 7], 0),
            ("WinQueryWindowUShort", ORD_WIN_QUERY_WINDOW_USHORT, vec![b, 0], 0),
            ("WinSendDlgItemMsg", ORD_WIN_SEND_DLG_ITEM_MSG, vec![b, 1, WM_USER, 0, 0], 0),
            ("WinEnableWindow", ORD_WIN_ENABLE_WINDOW, vec![b, 1], 0),
            ("WinIsWindowEnabled", ORD_WIN_IS_WINDOW_ENABLED, vec![b], 0),
            ("WinInvalidateRect", ORD_WIN_INVALIDATE_RECT, vec![b, 0, 0], 0),
            ("WinValidateRect", ORD_WIN_VALIDATE_RECT, vec![b, 0, 0], 0),
            ("WinQueryUpdateRect", ORD_WIN_QUERY_UPDATE_RECT, vec![b, buf], 0),
            ("WinScrollWindow", ORD_WIN_SCROLL_WINDOW, vec![b, 0, 1, 0, 0, 0, 0, 0], RGN_ERROR),
            ("WinSetFocus", ORD_WIN_SET_FOCUS, vec![HWND_DESKTOP, b], 0),
            ("WinQueryWindow", ORD_WIN_QUERY_WINDOW, vec![b, QW_PARENT as u32], 0),
            ("WinQueryWindowPos", ORD_WIN_QUERY_WINDOW_POS, vec![b, buf], 0),
            ("WinSubclassWindow", ORD_WIN_SUBCLASS_WINDOW, vec![b, 0x1234], 0),
            ("WinUpdateWindow", ORD_WIN_UPDATE_WINDOW, vec![b], 0),
            ("WinGetPS", ORD_WIN_GET_PS, vec![b], 0),
            ("WinBeginPaint", ORD_WIN_BEGIN_PAINT, vec![b, 0, buf], 0),
            ("WinOpenWindowDC", ORD_WIN_OPEN_WINDOW_DC, vec![b], 0),
            ("WinSetActiveWindow", ORD_WIN_SET_ACTIVE_WINDOW, vec![HWND_DESKTOP, b], 0),
            ("WinQueryDlgItemText", ORD_WIN_QUERY_DLG_ITEM_TEXT, vec![b, 1, 16, buf], 0),
            ("WinSetDlgItemText", ORD_WIN_SET_DLG_ITEM_TEXT, vec![b, 1, 0], 0),
            ("WinStopTimer", ORD_WIN_STOP_TIMER, vec![MOCK_HAB, b, 1], 0),
            ("WinSetCapture", ORD_WIN_SET_CAPTURE, vec![HWND_DESKTOP, b], 0),
            ("WinDismissDlg", ORD_WIN_DISMISS_DLG, vec![b, 1], 0),
            ("WinProcessDlg", ORD_WIN_PROCESS_DLG, vec![b], DID_CANCEL),
            ("WinSetAccelTable", ORD_WIN_SET_ACCEL_TABLE, vec![MOCK_HAB, 0, b], 0),
            ("WinCreateWindow (parent)", ORD_WIN_CREATE_WINDOW,
             vec![b, WC_STATIC_ATOM, 0, 0, 0, 0, 10, 10, 0, 0, 1, 0, 0], 0),
            ("WinCreateWindow (owner)", ORD_WIN_CREATE_WINDOW,
             vec![HWND_DESKTOP, WC_STATIC_ATOM, 0, 0, 0, 0, 10, 10, b, 0, 1, 0, 0], 0),
        ];
        let windows = loader.shared.window_mgr.lock_or_recover().window_handles().len();
        for (name, ordinal, args, fail) in cases {
            assert_eq!(call(&loader, 0, *ordinal, args), *fail, "{name} fails");
            assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_HWND), "{name}");
        }
        assert_eq!(loader.shared.window_mgr.lock_or_recover().window_handles().len(), windows,
                   "no window was created");
    }

    /// The errors the PM Reference lists for these calls (EDM2's copies of
    /// its WinInvalidateRect, WinBeginPaint, WinCreateWindow and
    /// Win*DlgItemText pages): a flag that is no BOOL, a presentation space
    /// that is none, a thread without a message queue, a dialog item that
    /// does not exist.
    #[test]
    fn documented_pm_errors_are_reported() {
        let loader = Loader::new_mock();
        let buf = 0x2000;
        loader.guest_write_bytes(buf, b"x\0").unwrap();
        // A thread without a queue cannot create a window.
        assert_eq!(call(&loader, 0, ORD_WIN_CREATE_WINDOW, &[HWND_DESKTOP, buf, buf, 0, 0, 0, 10, 10, 0, 0, 1, 0, 0]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_NO_MSG_QUEUE));
        let hmq = call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        let hwnd = loader.shared.window_mgr.lock_or_recover().create_window("App".into(), 0, hmq);
        assert_eq!(call(&loader, 0, ORD_WIN_INVALIDATE_RECT, &[hwnd, 0, 2]), 0, "fIncludeChildren is a BOOL");
        assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_FLAG));
        assert_eq!(call(&loader, 0, ORD_WIN_INVALIDATE_RECT, &[hwnd, 0, 1]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_BEGIN_PAINT, &[hwnd, 0xBAD0, 0]), 0, "no such HPS");
        assert_eq!(last_error(&loader, 0), err(PMERR_INV_HPS));
        assert_eq!(call(&loader, 0, ORD_WIN_SET_DLG_ITEM_TEXT, &[hwnd, 77, buf]), 0, "no item 77");
        assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_HWND));
        loader.guest_write_bytes(buf + 16, b"zz").unwrap();
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_DLG_ITEM_TEXT, &[hwnd, 77, 8, buf + 16]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_HWND));
        assert_eq!(loader.guest_read::<u8>(buf + 16), Some(0), "an empty string");
    }

    /// WinEnableWindow repaints a built-in control whose state changes (a
    /// disabled button's label is greyed), and nothing else: not an
    /// application's window, not a control whose state stays.
    #[test]
    fn win_enable_window_repaints_a_control_whose_state_changes() {
        let loader = Loader::new_mock();
        let hmq = call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        let (button, app) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let app = wm.create_window("App".into(), 0, hmq);
            let button = wm.create_window("WC_BUTTON".into(), app, hmq);
            for h in [app, button] {
                let w = wm.get_window_mut(h).unwrap();
                (w.cx, w.cy) = (40, 20);
            }
            (button, app)
        };
        let invalid = |h: u32| !loader.shared.window_mgr.lock_or_recover().get_window(h).unwrap().update.is_empty();
        assert_eq!(call(&loader, 0, ORD_WIN_ENABLE_WINDOW, &[button, 1]), 1);
        assert!(!invalid(button), "already enabled: no repaint");
        assert_eq!(call(&loader, 0, ORD_WIN_ENABLE_WINDOW, &[button, 0]), 1);
        assert!(invalid(button), "disabled: repainted greyed");
        assert_eq!(call(&loader, 0, ORD_WIN_IS_WINDOW_ENABLED, &[button]), 0);
        assert_eq!(call(&loader, 0, ORD_WIN_ENABLE_WINDOW, &[app, 0]), 1);
        assert!(!invalid(app), "the application paints its own window on WM_ENABLE");
    }

    #[test]
    fn calls_with_good_or_null_handles_record_nothing() {
        let loader = Loader::new_mock();
        let hmq = call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        let hwnd = loader.shared.window_mgr.lock_or_recover().create_window("App".into(), 0, hmq);
        let buf = 0x2000;
        assert_eq!(call(&loader, 0, ORD_WIN_SHOW_WINDOW, &[hwnd, 1]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW_RECT, &[hwnd, buf]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_ENABLE_WINDOW, &[hwnd, 1]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_IS_WINDOW_ENABLED, &[hwnd]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_INVALIDATE_RECT, &[hwnd, 0, 0]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_UPDATE_WINDOW, &[hwnd]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW_POS, &[hwnd, buf]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_SET_FOCUS, &[HWND_DESKTOP, 0]), 1, "NULLHANDLE focus");
        assert_eq!(call(&loader, 0, ORD_WIN_STOP_TIMER, &[MOCK_HAB, 0, 1]), 1, "queue timer");
        assert_eq!(call(&loader, 0, ORD_WIN_SET_CAPTURE, &[HWND_DESKTOP, 0]), 1, "release");
        assert_eq!(call(&loader, 0, ORD_WIN_SET_CAPTURE, &[HWND_DESKTOP, HWND_THREADCAPTURE]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_SET_ACCEL_TABLE, &[MOCK_HAB, 0, 0]), 1, "no queue table");
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW, &[hwnd, QW_FRAMEOWNER as u32]), 0);
        assert_ne!(call(&loader, 0, ORD_WIN_CREATE_WINDOW,
                        &[HWND_DESKTOP, WC_STATIC_ATOM, 0, 0, 0, 0, 10, 10, 0, 0, 1, 0, 0]), 0);
        assert_eq!(last_error(&loader, 0), 0);
    }

    #[test]
    fn query_window_rejects_an_unknown_relationship() {
        let loader = Loader::new_mock();
        let hwnd = loader.shared.window_mgr.lock_or_recover().create_window("App".into(), 0, 1);
        for code in [-1i32, QW_FRAMEOWNER + 1] {
            assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW, &[hwnd, code as u32]), 0);
            assert_eq!(last_error(&loader, 0), err(PMERR_PARAMETER_OUT_OF_RANGE), "lCode {code}");
        }
    }

    #[test]
    fn window_words_outside_the_class_data_are_out_of_range() {
        let loader = Loader::new_mock();
        let name = 0x3000;
        loader.guest_write_bytes(name, b"Words\0");
        // cbWindowData = 8: ULONGs at 0 and 4, USHORTs at 0..=6.
        assert_eq!(call(&loader, 0, ORD_WIN_REGISTER_CLASS, &[MOCK_HAB, name, 0x1234, 0, 8]), 1);
        let hwnd = loader.shared.window_mgr.lock_or_recover().create_window("Words".into(), 0, 1);
        assert_eq!(call(&loader, 0, ORD_WIN_SET_WINDOW_ULONG, &[hwnd, 4, 0xCAFE]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW_ULONG, &[hwnd, 4]), 0xCAFE);
        assert_eq!(call(&loader, 0, ORD_WIN_SET_WINDOW_USHORT, &[hwnd, 6, 0xBEEF]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW_USHORT, &[hwnd, 6]), 0xBEEF);
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW_ULONG, &[hwnd, QWL_STYLE as u32]), 0);
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW_ULONG, &[hwnd, QWL_MIN as u32]), 0);
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW_USHORT, &[hwnd, QWS_MIN as u32]), 0);
        assert_eq!(last_error(&loader, 0), 0, "all in range");

        let cases: &[(&str, u32, Vec<u32>)] = &[
            ("ULONG past the data", ORD_WIN_SET_WINDOW_ULONG, vec![hwnd, 5, 1]),
            ("ULONG past the data", ORD_WIN_QUERY_WINDOW_ULONG, vec![hwnd, 8]),
            ("ULONG below QWL_MIN", ORD_WIN_SET_WINDOW_ULONG, vec![hwnd, (QWL_MIN - 1) as u32, 1]),
            ("ULONG below QWL_MIN", ORD_WIN_QUERY_WINDOW_ULONG, vec![hwnd, (QWL_MIN - 1) as u32]),
            ("USHORT past the data", ORD_WIN_SET_WINDOW_USHORT, vec![hwnd, 7, 1]),
            ("USHORT past the data", ORD_WIN_QUERY_WINDOW_USHORT, vec![hwnd, 8]),
            ("USHORT below QWS_MIN", ORD_WIN_SET_WINDOW_USHORT, vec![hwnd, (QWS_MIN - 1) as u32, 1]),
            ("USHORT below QWS_MIN", ORD_WIN_QUERY_WINDOW_USHORT, vec![hwnd, (QWS_MIN - 1) as u32]),
        ];
        for (name, ordinal, args) in cases {
            assert_eq!(call(&loader, 0, *ordinal, args), 0, "{name} fails");
            assert_eq!(last_error(&loader, 0), err(PMERR_PARAMETER_OUT_OF_RANGE), "{name}");
        }
        assert_eq!(call(&loader, 0, ORD_WIN_QUERY_WINDOW_ULONG, &[hwnd, 4]), 0xCAFE, "unchanged");
    }

    #[test]
    fn frame_and_builtin_window_words_are_not_bounded() {
        let loader = Loader::new_mock();
        let name = 0x3000;
        loader.guest_write_bytes(name, b"Client\0");
        call(&loader, 0, ORD_WIN_REGISTER_CLASS, &[MOCK_HAB, name, 0x1234, 0, 0]);
        let (frame, control) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            // A standard frame carries its client's class name.
            let frame = wm.create_window("Client".into(), 0, 1);
            let client = wm.create_window("Client".into(), frame, 1);
            wm.frame_to_client.insert(frame, client);
            (frame, wm.create_window("WC_STATIC".into(), frame, 1))
        };
        assert_eq!(call(&loader, 0, ORD_WIN_SET_WINDOW_USHORT, &[frame, QWS_CXRESTORE as u32, 300]), 1);
        assert_eq!(call(&loader, 0, ORD_WIN_SET_WINDOW_ULONG, &[control, 0, 1]), 1);
        assert_eq!(last_error(&loader, 0), 0);
    }

    #[test]
    fn send_dlg_item_msg_to_a_missing_item_reports_invalid_hwnd() {
        let loader = Loader::new_mock();
        let dlg = loader.shared.window_mgr.lock_or_recover().create_window("App".into(), 0, 1);
        assert_eq!(call(&loader, 0, ORD_WIN_SEND_DLG_ITEM_MSG, &[dlg, 42, WM_USER, 0, 0]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_HWND));
    }

    #[test]
    fn set_accel_table_with_a_bad_table_reports_invalid_haccel() {
        let loader = Loader::new_mock();
        assert_eq!(call(&loader, 0, ORD_WIN_SET_ACCEL_TABLE, &[MOCK_HAB, 0xBAD, 0]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_HACCEL));
        let haccel = loader.shared.window_mgr.lock_or_recover().add_accel_table(Vec::new());
        assert_eq!(call(&loader, 0, ORD_WIN_SET_ACCEL_TABLE, &[MOCK_HAB, haccel, 0]), 1);
        assert_eq!(last_error(&loader, 0), 0);
    }

    #[test]
    fn get_or_peek_with_a_bad_filter_or_flag_fails() {
        let loader = Loader::new_mock();
        let hmq = call(&loader, 0, ORD_WIN_CREATE_MSG_QUEUE, &[MOCK_HAB, 0]);
        let hwnd = loader.shared.window_mgr.lock_or_recover().create_window("App".into(), 0, hmq);
        call(&loader, 0, ORD_WIN_POST_MSG, &[hwnd, WM_USER, 0, 0]);
        let qmsg = 0x2000;
        assert_eq!(call(&loader, 0, ORD_WIN_PEEK_MSG, &[MOCK_HAB, qmsg, BAD_HWND, 0, 0, PM_NOREMOVE]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_HWND), "WinPeekMsg filter");
        assert_eq!(call(&loader, 0, ORD_WIN_PEEK_MSG, &[MOCK_HAB, qmsg, 0, 0, 0, 0x10]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_FLAG), "WinPeekMsg fl");
        assert_eq!(call(&loader, 0, ORD_WIN_GET_MSG, &[MOCK_HAB, qmsg, BAD_HWND, 0, 0]), 0);
        assert_eq!(last_error(&loader, 0), err(PMERR_INVALID_HWND), "WinGetMsg filter");
        // The message is still there for a valid filter.
        assert_eq!(call(&loader, 0, ORD_WIN_PEEK_MSG, &[MOCK_HAB, qmsg, hwnd, 0, 0, PM_REMOVE]), 1);
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

    #[test]
    fn cursor_rate_is_queried_and_set_through_the_sys_value_calls() {
        const ORD_WIN_QUERY_SYS_VALUE: u32 = 829;
        const ORD_WIN_SET_SYS_VALUE: u32 = 873;
        let loader = Loader::new_mock();
        let query = |i: i32| call(&loader, 0, ORD_WIN_QUERY_SYS_VALUE, &[HWND_DESKTOP, i as u32]);
        assert_eq!(query(SV_CURSORRATE), CARET_BLINK_MS as u32);
        assert_eq!(call(&loader, 0, ORD_WIN_SET_SYS_VALUE, &[HWND_DESKTOP, SV_CURSORRATE as u32, 250]), 1);
        assert_eq!(query(SV_CURSORRATE), 250);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().cursor_rate_ms, 250, "the renderer's blink interval");
        assert_eq!(last_error(&loader, 0), 0);

        // A rate below 1 ms, a value Warpine does not keep, a bad desktop.
        for (args, code) in [([HWND_DESKTOP, SV_CURSORRATE as u32, 0], PMERR_PARAMETER_OUT_OF_RANGE),
                             ([HWND_DESKTOP, SV_CURSORRATE as u32, -5i32 as u32], PMERR_PARAMETER_OUT_OF_RANGE),
                             ([HWND_DESKTOP, SV_CXSCREEN as u32, 10], PMERR_PARAMETER_OUT_OF_RANGE),
                             ([BAD_HWND, SV_CURSORRATE as u32, 100], PMERR_INVALID_HWND)] {
            assert_eq!(call(&loader, 0, ORD_WIN_SET_SYS_VALUE, &args), 0, "{args:x?}");
            assert_eq!(last_error(&loader, 0), err(code), "{args:x?}");
        }
        assert_eq!(query(SV_CURSORRATE), 250, "failed calls change nothing");
        assert_eq!(query(SV_CXSCREEN), desktop_size().0);
    }
}
