// SPDX-License-Identifier: GPL-3.0-only

//! Input state: `WinGetKeyState`, `WinGetPhysKeyState`, `WinQueryMsgPos`,
//! `WinQueryMsgTime`, `WinQueryPointerPos` and `WinSetPointerPos`.
//!
//! From the PM Reference:
//! - `WinGetKeyState(hwndDesktop, vkey)` "returns the state of the key at
//!   the time that the last message obtained from the queue was posted":
//!   0x8000 "The key is down", 0x0001 "The key has been pressed an odd
//!   number of times since the system has been started"; `VK_BUTTON1` ..
//!   `VK_BUTTON3` give the pointing-device buttons.
//! - `WinGetPhysKeyState(hwndDesktop, sc)` takes a hardware scan code and
//!   returns "the asynchronous (interrupt level) state", "not synchronized
//!   to the processing of input": 0x8000 down, 0x0001 "pressed since the
//!   last time this function was issued".
//! - `WinQueryMsgPos` / `WinQueryMsgTime`: the pointer position (screen
//!   coordinates) and time of "the last message obtained from the current
//!   message queue" — the `QMSG` `ptl` and `time` values; the time is in
//!   milliseconds "from the time the system is started".
//! - `WinQueryPointerPos` / `WinSetPointerPos`: the pointer in screen
//!   coordinates, "relative to the desktop's lower left corner".
//!
//! How Warpine keeps the state: the GUI thread records every key press and
//! release by scan code and the pointer position as SDL reports them
//! (`InputState::physical_key`, `InputState::pointer`) — the physical state.
//! Keyboard and pointer input reach programs through the system input queue
//! (`pm_input.rs`); when a thread reads an event from it, its `WM_CHAR`
//! virtual key or `WM_BUTTONn*` button updates the synchronous state
//! (`InputState::read_input`) that `WinGetKeyState` reports. Each input
//! event carries the time and pointer position of its arrival; other
//! messages are stamped when they are read.
//!
//! Unverified (no source): one synchronous key state for the whole process
//! rather than one per queue; a double click counts as a press; `vkey` /
//! `sc` above 0xFF and a NULL `PPOINTL` fail with
//! `PMERR_PARAMETER_OUT_OF_RANGE`; `WinQueryMsgPos` / `WinQueryMsgTime` on
//! a thread without a message queue fail with `PMERR_NO_MSG_QUEUE` (FALSE /
//! 0) and give (0, 0) / 0 before the first message; `WinSetPointerPos`
//! keeps the pointer on the desktop (clamped) and moves the host pointer
//! only in an interactive session (not in a scripted or headless one);
//! keys pressed before Warpine started count as up, Caps Lock and Num Lock
//! as not toggled.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::OS2Message;
use super::ApiResult;
use crate::gui::GUIMessage;

/// Number of virtual keys and of scan codes Warpine tracks (one byte each).
const KEY_SLOTS: usize = 256;

/// Key, button and pointer state (a field of `WindowManager`).
#[derive(Debug, Clone)]
pub struct InputState {
    /// By virtual key: as of the input read last from the system input
    /// queue (`KEY_STATE_DOWN`, `KEY_STATE_TOGGLED`).
    sync_vk: [u16; KEY_SLOTS],
    /// By scan code: the keyboard now (`KEY_STATE_DOWN`, and
    /// `KEY_STATE_TOGGLED` = pressed since the last `WinGetPhysKeyState`).
    phys_scan: [u16; KEY_SLOTS],
    /// The pointer position: desktop coordinates, bottom-left origin.
    pub pointer: (i32, i32),
}

impl Default for InputState {
    fn default() -> Self {
        InputState { sync_vk: [0; KEY_SLOTS], phys_scan: [0; KEY_SLOTS], pointer: (0, 0) }
    }
}

/// Record a press (`down`) or release of a key in `slot`. A press of a key
/// that is up flips the toggle bit; an auto-repeated press does not.
fn record(slot: &mut u16, down: bool) {
    let mask = KEY_STATE_DOWN as u16;
    if down {
        if *slot & mask == 0 { *slot ^= KEY_STATE_TOGGLED as u16; }
        *slot |= mask;
    } else {
        *slot &= !mask;
    }
}

impl InputState {
    /// `WinGetKeyState`: the state of virtual key `vk` (0 for a value that
    /// is no virtual key).
    pub fn key_state(&self, vk: u32) -> u32 {
        self.sync_vk.get(vk as usize).copied().unwrap_or(0) as u32
    }

    /// `WinGetPhysKeyState`: the state of the key with scan code `sc`;
    /// reading it clears its "pressed since the last call" bit.
    pub fn phys_key_state(&mut self, sc: u32) -> u32 {
        let Some(slot) = self.phys_scan.get_mut(sc as usize) else { return 0 };
        let v = *slot as u32;
        *slot &= !(KEY_STATE_TOGGLED as u16);
        v
    }

    /// The GUI thread saw the key with scan code `sc` go down or up. A
    /// press sets its "pressed since the last call" bit.
    pub fn physical_key(&mut self, sc: u8, down: bool) {
        let slot = &mut self.phys_scan[sc as usize];
        if down {
            *slot |= (KEY_STATE_DOWN | KEY_STATE_TOGGLED) as u16;
        } else {
            *slot &= !(KEY_STATE_DOWN as u16);
        }
    }

    /// A thread read input message `m` from the system input queue: its
    /// virtual key (`WM_CHAR` with `KC_VIRTUALKEY`) or pointer button
    /// (`WM_BUTTONn*`) changes the synchronous state.
    pub fn read_input(&mut self, m: &OS2Message) {
        let button = |vk: u32, down: bool| (vk, down);
        let (vk, down) = match m.msg {
            WM_CHAR => {
                let flags = m.mp1 & 0xFFFF;
                if flags & KC_VIRTUALKEY == 0 { return; }
                let vk = (m.mp2 >> WM_CHAR_VKEY_SHIFT) & 0xFF;
                let down = flags & KC_KEYUP == 0;
                // A repeated press is no new press (KC_PREVDOWN).
                if down && flags & KC_PREVDOWN != 0 && self.sync_vk[vk as usize] & KEY_STATE_DOWN as u16 != 0 {
                    return;
                }
                // Shift+Tab arrives as VK_BACKTAB; the Tab key is down too.
                if vk == VK_BACKTAB { record(&mut self.sync_vk[VK_TAB as usize], down); }
                (vk, down)
            }
            WM_BUTTON1DOWN | WM_BUTTON1DBLCLK => button(VK_BUTTON1, true),
            WM_BUTTON1UP => button(VK_BUTTON1, false),
            WM_BUTTON2DOWN | WM_BUTTON2DBLCLK => button(VK_BUTTON2, true),
            WM_BUTTON2UP => button(VK_BUTTON2, false),
            WM_BUTTON3DOWN | WM_BUTTON3DBLCLK => button(VK_BUTTON3, true),
            WM_BUTTON3UP => button(VK_BUTTON3, false),
            _ => return,
        };
        record(&mut self.sync_vk[vk as usize], down);
    }
}

impl super::Loader {
    /// WinGetKeyState(HWND hwndDesktop, LONG vkey) -> LONG
    pub(super) fn win_get_key_state(&self, tid: u32, hwnd: u32, vk: u32) -> ApiResult {
        if !self.check_desktop_arg(tid, hwnd) { return ApiResult::Normal(0); }
        if vk > 0xFF { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        ApiResult::Normal(self.shared.window_mgr.lock_or_recover().key_state.key_state(vk))
    }

    /// WinGetPhysKeyState(HWND hwndDesktop, LONG sc) -> LONG
    pub(super) fn win_get_phys_key_state(&self, tid: u32, hwnd: u32, sc: u32) -> ApiResult {
        if !self.check_desktop_arg(tid, hwnd) { return ApiResult::Normal(0); }
        if sc > 0xFF { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        ApiResult::Normal(self.shared.window_mgr.lock_or_recover().key_state.phys_key_state(sc))
    }

    /// The `(time, pointer position)` of the message the calling thread's
    /// queue read last; `None` without a queue (`PMERR_NO_MSG_QUEUE`).
    fn last_message(&self, tid: u32) -> Option<(u32, (i32, i32))> {
        let mq = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.thread_queue(tid).and_then(|h| wm.get_mq(h))
        };
        let Some(mq) = mq else {
            self.pm_error(tid, PMERR_NO_MSG_QUEUE);
            return None;
        };
        let q = mq.lock_or_recover();
        Some((q.last_msg_time, q.last_msg_pos))
    }

    /// WinQueryMsgPos(HAB hab, PPOINTL pptl) -> BOOL
    pub(super) fn win_query_msg_pos(&self, tid: u32, pptl: u32) -> ApiResult {
        let Some((_, pos)) = self.last_message(tid) else { return ApiResult::Normal(0) };
        if !self.put_pointl(pptl, pos) { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        ApiResult::Normal(1)
    }

    /// WinQueryMsgTime(HAB hab) -> ULONG
    pub(super) fn win_query_msg_time(&self, tid: u32) -> ApiResult {
        ApiResult::Normal(self.last_message(tid).map_or(0, |(time, _)| time))
    }

    /// WinQueryPointerPos(HWND hwndDesktop, PPOINTL pptl) -> BOOL
    pub(super) fn win_query_pointer_pos(&self, tid: u32, hwnd: u32, pptl: u32) -> ApiResult {
        if !self.check_desktop_arg(tid, hwnd) { return ApiResult::Normal(0); }
        let pos = self.shared.window_mgr.lock_or_recover().key_state.pointer;
        if !self.put_pointl(pptl, pos) { return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE); }
        ApiResult::Normal(1)
    }

    /// WinSetPointerPos(HWND hwndDesktop, LONG x, LONG y) -> BOOL: the
    /// pointer moves to desktop point `(x, y)`, kept on the desktop; the GUI
    /// thread moves the host pointer there.
    pub(super) fn win_set_pointer_pos(&self, tid: u32, hwnd: u32, x: i32, y: i32) -> ApiResult {
        if !self.check_desktop_arg(tid, hwnd) { return ApiResult::Normal(0); }
        let (dw, dh) = desktop_size();
        let pos = (x.clamp(0, dw as i32 - 1), y.clamp(0, dh as i32 - 1));
        let mut wm = self.shared.window_mgr.lock_or_recover();
        wm.key_state.pointer = pos;
        if let Some(tx) = &wm.gui_tx {
            let _ = tx.send(GUIMessage::SetPointerPos { x: pos.0, y: pos.1 });
        }
        ApiResult::Normal(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pm_error::error_id;
    use super::super::pm_input::InputEvent;
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::Loader;

    const ORD_WIN_GET_KEY_STATE: u32 = 752;
    const ORD_WIN_GET_LAST_ERROR: u32 = 753;
    const ORD_WIN_GET_PHYS_KEY_STATE: u32 = 758;
    const ORD_WIN_QUERY_MSG_POS: u32 = 818;
    const ORD_WIN_QUERY_MSG_TIME: u32 = 819;
    const ORD_WIN_QUERY_POINTER_POS: u32 = 823;
    const ORD_WIN_SET_POINTER_POS: u32 = 867;
    const ORD_WIN_GET_MSG: u32 = 915;
    const ORD_WIN_POST_MSG: u32 = 919;

    const PTL: u32 = 0x3000;
    const QMSG: u32 = 0x3100;
    /// Scan code of the left Shift key.
    const SC_LSHIFT: u8 = 0x2A;

    fn call(loader: &Loader, ordinal: u32, args: &[u32]) -> u32 {
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
        match loader.handle_pmwin_call(&mut vcpu, 0, ordinal) {
            ApiResult::Normal(v) => v,
            other => panic!("PMWIN ordinal {} returned {:?}", ordinal, other),
        }
    }

    fn last_error(loader: &Loader) -> u32 {
        call(loader, ORD_WIN_GET_LAST_ERROR, &[MOCK_HAB])
    }

    fn pointl(loader: &Loader, ptr: u32) -> (i32, i32) {
        loader.read_pointl(ptr).unwrap()
    }

    /// A queue for thread 0 with a frame and its client; returns the client.
    fn window(loader: &Loader) -> u32 {
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let hmq = wm.create_mq(437);
        wm.tid_to_hmq.insert(0, hmq);
        let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        wm.frame_to_client.insert(frame, client);
        wm.get_window_mut(client).unwrap().visible = true;
        wm.focused_hwnd = frame;
        client
    }

    /// The `WM_CHAR` of a virtual key going down or up.
    fn vkey(vk: u32, up: bool) -> InputEvent {
        let flags = KC_VIRTUALKEY | KC_SCANCODE | if up { KC_KEYUP } else { 0 };
        InputEvent::Key { mp1: flags | (1 << WM_CHAR_REPEAT_SHIFT), mp2: vk << WM_CHAR_VKEY_SHIFT }
    }

    fn get_msg(loader: &Loader) -> u32 {
        call(loader, ORD_WIN_GET_MSG, &[MOCK_HAB, QMSG, 0, 0, 0]);
        loader.guest_read::<u32>(QMSG + 4).unwrap()
    }

    #[test]
    fn record_flips_the_toggle_on_each_new_press() {
        let mut slot = 0u16;
        record(&mut slot, true);
        assert_eq!(slot as u32, KEY_STATE_DOWN | KEY_STATE_TOGGLED);
        record(&mut slot, true);
        assert_eq!(slot as u32, KEY_STATE_DOWN | KEY_STATE_TOGGLED, "still down: no new press");
        record(&mut slot, false);
        assert_eq!(slot as u32, KEY_STATE_TOGGLED);
        record(&mut slot, true);
        record(&mut slot, false);
        assert_eq!(slot, 0, "pressed twice: even");
    }

    #[test]
    fn key_state_follows_the_input_read_not_the_input_waiting() {
        let loader = Loader::new_mock();
        let client = window(&loader);
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.post_input(vkey(VK_SHIFT, false));
            wm.post_input(vkey(VK_F5, false));
            wm.post_input(vkey(VK_SHIFT, true));
        }
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_SHIFT]), 0, "nothing read yet");
        assert_eq!(get_msg(&loader), WM_CHAR);
        assert_eq!(loader.guest_read::<u32>(QMSG).unwrap(), client);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_SHIFT]),
                   KEY_STATE_DOWN | KEY_STATE_TOGGLED);
        assert_eq!(get_msg(&loader), WM_CHAR);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_SHIFT]) & KEY_STATE_DOWN,
                   KEY_STATE_DOWN, "Shift still down while F5 is processed");
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_F5]) & KEY_STATE_DOWN, KEY_STATE_DOWN);
        assert_eq!(get_msg(&loader), WM_CHAR);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_SHIFT]), KEY_STATE_TOGGLED,
                   "up, pressed once");
        // A program's own WM_CHAR is no input.
        call(&loader, ORD_WIN_POST_MSG, &[client, WM_CHAR, KC_VIRTUALKEY, VK_CTRL << 16]);
        assert_eq!(get_msg(&loader), WM_CHAR);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_CTRL]), 0);
    }

    #[test]
    fn buttons_and_back_tab_count_as_keys() {
        let loader = Loader::new_mock();
        let client = window(&loader);
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.post_input(InputEvent::Window { hwnd: client, msg: WM_BUTTON1DOWN, mp1: 0, mp2: 0 });
            wm.post_input(InputEvent::Window { hwnd: client, msg: WM_BUTTON1UP, mp1: 0, mp2: 0 });
            wm.post_input(InputEvent::Window { hwnd: client, msg: WM_BUTTON2DBLCLK, mp1: 0, mp2: 0 });
            wm.post_input(vkey(VK_BACKTAB, false));
        }
        assert_eq!(get_msg(&loader), WM_BUTTON1DOWN);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_BUTTON1]) & KEY_STATE_DOWN, KEY_STATE_DOWN);
        assert_eq!(get_msg(&loader), WM_BUTTON1UP);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_BUTTON1]), KEY_STATE_TOGGLED);
        assert_eq!(get_msg(&loader), WM_BUTTON2DBLCLK);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_BUTTON2]) & KEY_STATE_DOWN, KEY_STATE_DOWN);
        assert_eq!(get_msg(&loader), WM_CHAR);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_TAB]) & KEY_STATE_DOWN, KEY_STATE_DOWN);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_BACKTAB]) & KEY_STATE_DOWN, KEY_STATE_DOWN);
    }

    #[test]
    fn physical_state_is_now_and_its_press_bit_resets_on_read() {
        let loader = Loader::new_mock();
        let sc = SC_LSHIFT as u32;
        assert_eq!(call(&loader, ORD_WIN_GET_PHYS_KEY_STATE, &[HWND_DESKTOP, sc]), 0);
        loader.shared.window_mgr.lock_or_recover().key_state.physical_key(SC_LSHIFT, true);
        assert_eq!(call(&loader, ORD_WIN_GET_PHYS_KEY_STATE, &[HWND_DESKTOP, sc]),
                   KEY_STATE_DOWN | KEY_STATE_TOGGLED);
        assert_eq!(call(&loader, ORD_WIN_GET_PHYS_KEY_STATE, &[HWND_DESKTOP, sc]), KEY_STATE_DOWN,
                   "pressed bit cleared by the last call");
        loader.shared.window_mgr.lock_or_recover().key_state.physical_key(SC_LSHIFT, false);
        assert_eq!(call(&loader, ORD_WIN_GET_PHYS_KEY_STATE, &[HWND_DESKTOP, sc]), 0);
        // Pressed and released between two calls.
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.key_state.physical_key(SC_LSHIFT, true);
            wm.key_state.physical_key(SC_LSHIFT, false);
        }
        assert_eq!(call(&loader, ORD_WIN_GET_PHYS_KEY_STATE, &[HWND_DESKTOP, sc]), KEY_STATE_TOGGLED);
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, VK_SHIFT]), 0,
                   "the synchronous state did not see it");
    }

    #[test]
    fn key_state_arguments_are_checked() {
        let loader = Loader::new_mock();
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[HWND_DESKTOP, 0x100]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_GET_PHYS_KEY_STATE, &[HWND_DESKTOP, 0x1_0000]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_GET_KEY_STATE, &[0xDEAD, VK_SHIFT]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
        assert_eq!(call(&loader, ORD_WIN_GET_PHYS_KEY_STATE, &[0xDEAD, 0x2A]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
    }

    #[test]
    fn input_messages_carry_their_arrival_time_and_pointer_position() {
        let loader = Loader::new_mock();
        let client = window(&loader);
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.key_state.pointer = (120, 340);
            wm.post_input(InputEvent::Window { hwnd: client, msg: WM_BUTTON1DOWN, mp1: 0, mp2: 0 });
            wm.key_state.pointer = (5, 6);
        }
        let before = super::super::stubs::ms_count();
        assert_eq!(get_msg(&loader), WM_BUTTON1DOWN);
        // QMSG.ptl is a POINTL at +20, then ulReserved.
        assert_eq!(pointl(&loader, QMSG + 20), (120, 340), "where the pointer was when it arrived");
        let time = loader.guest_read::<u32>(QMSG + 16).unwrap();
        assert!(time != 0 && time.wrapping_sub(before) < 1000 || before.wrapping_sub(time) < 1000);
        assert_eq!(call(&loader, ORD_WIN_QUERY_MSG_TIME, &[MOCK_HAB]), time);
        assert_eq!(call(&loader, ORD_WIN_QUERY_MSG_POS, &[MOCK_HAB, PTL]), 1);
        assert_eq!(pointl(&loader, PTL), (120, 340));
        // A posted message is stamped when it is read.
        call(&loader, ORD_WIN_POST_MSG, &[client, WM_USER, 0, 0]);
        assert_eq!(get_msg(&loader), WM_USER);
        assert_eq!(pointl(&loader, QMSG + 20), (5, 6));
        assert_ne!(loader.guest_read::<u32>(QMSG + 16).unwrap(), 0);
        assert_eq!(call(&loader, ORD_WIN_QUERY_MSG_POS, &[MOCK_HAB, PTL]), 1);
        assert_eq!(pointl(&loader, PTL), (5, 6));
        assert_eq!(call(&loader, ORD_WIN_QUERY_MSG_POS, &[MOCK_HAB, 0]), 0, "NULL PPOINTL");
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
    }

    #[test]
    fn message_queries_need_a_queue() {
        let loader = Loader::new_mock();
        assert_eq!(call(&loader, ORD_WIN_QUERY_MSG_TIME, &[MOCK_HAB]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_NO_MSG_QUEUE));
        assert_eq!(call(&loader, ORD_WIN_QUERY_MSG_POS, &[MOCK_HAB, PTL]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_NO_MSG_QUEUE));
        window(&loader);
        assert_eq!(call(&loader, ORD_WIN_QUERY_MSG_POS, &[MOCK_HAB, PTL]), 1, "a queue, no message yet");
        assert_eq!(pointl(&loader, PTL), (0, 0));
    }

    #[test]
    fn pointer_position_is_queried_and_set_on_the_desktop() {
        let loader = Loader::new_mock();
        let (tx, rx) = crate::gui::message::create_gui_channel();
        loader.shared.window_mgr.lock_or_recover().gui_tx = Some(tx);
        loader.shared.window_mgr.lock_or_recover().key_state.pointer = (10, 20);
        assert_eq!(call(&loader, ORD_WIN_QUERY_POINTER_POS, &[HWND_DESKTOP, PTL]), 1);
        assert_eq!(pointl(&loader, PTL), (10, 20));
        assert_eq!(call(&loader, ORD_WIN_SET_POINTER_POS, &[HWND_DESKTOP, 300, 200]), 1);
        assert_eq!(call(&loader, ORD_WIN_QUERY_POINTER_POS, &[HWND_DESKTOP, PTL]), 1);
        assert_eq!(pointl(&loader, PTL), (300, 200));
        assert!(matches!(rx.try_recv(), Ok(GUIMessage::SetPointerPos { x: 300, y: 200 })));
        // Kept on the desktop.
        let (dw, dh) = desktop_size();
        assert_eq!(call(&loader, ORD_WIN_SET_POINTER_POS, &[HWND_DESKTOP, (-5i32) as u32, 99_999]), 1);
        assert_eq!(call(&loader, ORD_WIN_QUERY_POINTER_POS, &[HWND_DESKTOP, PTL]), 1);
        assert_eq!(pointl(&loader, PTL), (0, dh as i32 - 1));
        assert_eq!(call(&loader, ORD_WIN_SET_POINTER_POS, &[HWND_DESKTOP, dw + 7, 0]), 1);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().key_state.pointer, (dw as i32 - 1, 0));
        // Errors.
        assert_eq!(call(&loader, ORD_WIN_QUERY_POINTER_POS, &[HWND_DESKTOP, 0]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_PARAMETER_OUT_OF_RANGE));
        assert_eq!(call(&loader, ORD_WIN_SET_POINTER_POS, &[0xDEAD, 1, 1]), 0);
        assert_eq!(last_error(&loader), error_id(SEVERITY_ERROR, PMERR_INVALID_HWND));
    }
}
