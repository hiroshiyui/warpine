// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 Presentation Manager PMWIN API handler methods.

use std::sync::Arc;
use std::thread;
use super::vm_backend::VcpuBackend;
use log::{debug, info, warn};

use super::constants::*;

// ── Built-in class atom → canonical class name ────────────────────────────────

/// Bound guest-supplied window geometry `(x, y, cx, cy)` to
/// ±`MAX_WINDOW_COORD` (sizes to `0..=MAX_WINDOW_COORD`), so the arithmetic
/// on it elsewhere — hit tests, layout, the compositor — cannot overflow.
pub(super) fn bound_window_geometry(x: i32, y: i32, cx: i32, cy: i32) -> (i32, i32, i32, i32) {
    let pos = |v: i32| v.clamp(-MAX_WINDOW_COORD, MAX_WINDOW_COORD);
    let size = |v: i32| v.clamp(0, MAX_WINDOW_COORD);
    (pos(x), pos(y), size(cx), size(cy))
}

/// Resolve a WinCreateWindow `pszClass` argument.
///
/// OS/2 PM encodes built-in class atoms as `MAKEINTATOM(n) = 0xFFFF0000 | n`.
/// If `ptr` carries the `0xFFFF` high word (or is a small bare atom < 0x10000),
/// we map it to the canonical class-name string.  Otherwise `string` contains
/// the already-read guest class-name string.
pub(super) fn resolve_class_atom(ptr: u32, string: String) -> String {
    match ptr {
        WC_FRAME_ATOM      => "WC_FRAME".to_string(),
        WC_COMBOBOX_ATOM   => "WC_COMBOBOX".to_string(),
        WC_BUTTON_ATOM     => "WC_BUTTON".to_string(),
        WC_MENU_ATOM       => "WC_MENU".to_string(),
        WC_STATIC_ATOM     => "WC_STATIC".to_string(),
        WC_ENTRYFIELD_ATOM => "WC_ENTRYFIELD".to_string(),
        WC_LISTBOX_ATOM    => "WC_LISTBOX".to_string(),
        WC_SCROLLBAR_ATOM  => "WC_SCROLLBAR".to_string(),
        WC_TITLEBAR_ATOM   => "WC_TITLEBAR".to_string(),
        WC_MLE_ATOM        => "WC_MLE".to_string(),
        WC_SPINBUTTON_ATOM => "WC_SPINBUTTON".to_string(),
        WC_CONTAINER_ATOM  => "WC_CONTAINER".to_string(),
        WC_NOTEBOOK_ATOM   => "WC_NOTEBOOK".to_string(),
        WC_SLIDER_ATOM     => "WC_SLIDER".to_string(),
        WC_VALUESET_ATOM   => "WC_VALUESET".to_string(),
        n if (n & 0xFFFF_0000) == 0xFFFF_0000
                           => format!("WC_ATOM_0x{:04X}", n & 0xFFFF),
        n if n < 0x10000   => format!("WC_ATOM_{}", n),
        _                  => string,
    }
}
use super::mutex_ext::MutexExt;
use super::pm_types::OS2Message;
use super::ApiResult;
use crate::gui::GUIMessage;
use crate::gui::surface::{DrawOp, Mix};
use super::pm_types::{DeviceContext, PsOrigin, TimerStop};
use crate::lx::header::{RT_STRING, RT_MESSAGE, RT_MENU, RT_DIALOG, RT_ACCELTABLE};
use super::pm_types::MenuItem;

impl super::Loader {
    pub(crate) fn handle_pmwin_call(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, ordinal: u32) -> ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| -> u32 { self.guest_read::<u32>((esp + off) as u32).expect("Stack read OOB") };
        // The five arguments after `hab` of the rectangle calls; ones a call
        // does not take are read without failing (and ignored).
        let rect_args = || -> [u32; 5] {
            std::array::from_fn(|i| self.guest_read::<u32>((esp + 8 + 4 * i as u64) as u32).unwrap_or(0))
        };

        match ordinal {
            763 => {
                // WinInitialize
                debug!("  [VCPU {}] WinInitialize called.", vcpu_id);
                ApiResult::Normal(MOCK_HAB)
            }
            888 => {
                // WinTerminate
                debug!("  [VCPU {}] WinTerminate called.", vcpu_id);
                ApiResult::Normal(1) // TRUE
            }
            716 => {
                // WinCreateMsgQueue
                debug!("  [VCPU {}] WinCreateMsgQueue called.", vcpu_id);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                // One queue per thread: a second one fails with NULLHANDLE.
                if wm.thread_queue(vcpu_id).is_some() {
                    return self.pm_fail(vcpu_id, PMERR_MSG_QUEUE_ALREADY_EXISTS);
                }
                // The queue codepage starts as the process codepage.
                let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
                let hmq = wm.create_mq(cp);
                wm.tid_to_hmq.insert(vcpu_id, hmq);
                ApiResult::Normal(hmq)
            }
            726 => {
                // WinDestroyMsgQueue(HMQ hmq): only the thread that created
                // the queue may destroy it (else FALSE, PMERR_INVALID_HMQ).
                let hmq = read_stack(4);
                debug!("  [VCPU {}] WinDestroyMsgQueue hmq={}", vcpu_id, hmq);
                let destroyed = self.shared.window_mgr.lock_or_recover().destroy_mq(vcpu_id, hmq);
                if !destroyed { return self.pm_fail(vcpu_id, PMERR_INVALID_HMQ); }
                ApiResult::Normal(1)
            }
            926 => {
                // WinRegisterClass
                let _hab = read_stack(4);
                let psz_class_name_ptr = read_stack(8);
                let pfn_wp = read_stack(12);
                let style = read_stack(16);
                let cb_window_data = read_stack(20);
                let name = self.read_guest_string(psz_class_name_ptr);
                debug!("  [VCPU {}] WinRegisterClass: name='{}', pfn_wp=0x{:08X}", vcpu_id, name, pfn_wp);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.register_class(name.clone(), pfn_wp, style);
                wm.set_class_window_data(&name, cb_window_data);
                ApiResult::Normal(1) // TRUE
            }
            908 => {
                // WinCreateStdWindow
                let parent = read_stack(4);
                let style = read_stack(8);
                let pfc_flags_ptr = read_stack(12);
                let psz_class_name_ptr = read_stack(16);
                let psz_title_ptr = read_stack(20);
                let client_style = read_stack(24);
                let hmod = read_stack(28);
                let id_resources = read_stack(32);
                let phwnd_client_ptr = read_stack(36);
                let class_name = self.read_guest_string(psz_class_name_ptr);
                let title = if psz_title_ptr != 0 {
                    self.read_guest_string_cp(psz_title_ptr, self.thread_codepage(vcpu_id))
                } else { "Warpine Window".to_string() };
                // VDR-C1: read the flCreateFlags (FCF_* bits) from the PULONG argument.
                let fl_create_flags = if pfc_flags_ptr != 0 {
                    self.guest_read::<u32>(pfc_flags_ptr).unwrap_or(0)
                } else { 0 };
                debug!("  [VCPU {}] WinCreateStdWindow: class='{}', title='{}', parent=0x{:08X}, style=0x{:08X}, fcf=0x{:08X}", vcpu_id, class_name, title, parent, style, fl_create_flags);

                // FCF_MENU: the frame loads its menu bar from the RT_MENU resource
                // whose id is the idResources argument. A missing resource simply
                // leaves the frame without a menu.
                let fcf_menu_items = if fl_create_flags & FCF_MENU != 0 {
                    self.load_menu_resource(hmod, id_resources, vcpu_id)
                } else { None };
                // FCF_ACCELTABLE: the frame's accelerator table is the RT_ACCELTABLE
                // resource with the same id.
                let fcf_accel = if fl_create_flags & FCF_ACCELTABLE != 0 {
                    self.load_accel_table(hmod, id_resources)
                } else { None };

                // The switch-list title, in the thread's codepage (taken
                // before the window manager is locked).
                let tasklist_title = (fl_create_flags & FCF_TASKLIST != 0)
                    .then(|| super::codepage::cp_encode(&title, self.thread_codepage(vcpu_id)));
                let (h_frame, h_client, pfn_wp_client) = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    let h_frame = wm.create_window(class_name.clone(), parent, hmq);
                    let h_client = wm.create_window(class_name.clone(), h_frame, hmq);
                    wm.frame_to_client.insert(h_frame, h_client);
                    // Initialise both windows to the default SDL window size so that
                    // WinQueryWindowRect returns correct dimensions before the first resize.
                    // Default placement: cascade from the desktop's top-left corner
                    // so successive windows do not stack exactly on top of each other.
                    let (def_x, def_y) = default_frame_pos(
                        wm.frame_to_client.len() - 1, FRAME_DEFAULT_CX, FRAME_DEFAULT_CY, desktop_size());
                    if let Some(win) = wm.get_window_mut(h_frame)  {
                        win.x = def_x; win.y = def_y;
                        win.cx = FRAME_DEFAULT_CX; win.cy = FRAME_DEFAULT_CY;
                        win.frame_flags = fl_create_flags;
                        win.style = style;
                        win.visible = style & WS_VISIBLE != 0;
                        win.text = title.clone();
                    }
                    // The client takes flClientStyle (the PM Guide's samples
                    // pass WS_VISIBLE there to "make client window visible").
                    let class_style = wm.class_window_style(&class_name);
                    if let Some(win) = wm.get_window_mut(h_client) {
                        win.style = client_style | class_style;
                        win.visible = client_style & WS_VISIBLE != 0;
                    }
                    if let Some(items) = fcf_menu_items {
                        let h_menu = wm.create_window("#Menu".to_string(), h_frame, 0);
                        if let Some(win) = wm.get_window_mut(h_menu) {
                            win.menu_items = items;
                            win.id = FID_MENU;
                            win.owner = h_frame;
                        }
                        if let Some(win) = wm.get_window_mut(h_frame) { win.menu_hwnd = h_menu; }
                    }
                    if let Some(haccel) = fcf_accel {
                        wm.set_window_accel(h_frame, haccel);
                    }
                    // FCF_VERTSCROLL / FCF_HORZSCROLL (pm_frame.rs).
                    Self::add_frame_scroll_bars(&mut wm, h_frame, fl_create_flags, hmq);
                    // Client fills the frame below the title bar and menu bar.
                    wm.layout_frame(h_frame);
                    // VDR-B1/E1: register in Z-order stack (top-most) and set focus.
                    wm.z_push_top(h_frame);
                    wm.focused_hwnd = h_frame;
                    // FCF_TASKLIST: a switch-list entry with the frame's
                    // title, removed with the frame (pm_switch.rs). A full
                    // list leaves the frame without one.
                    if let Some(t) = tasklist_title {
                        let _ = wm.switch_list.add_frame(h_frame, t);
                    }

                    // The frame is composited once it has a surface.
                    wm.frame_surface(h_frame);

                    // The new client is invalid: it paints on creation,
                    // the frame having it erase its background first.
                    wm.invalidate(h_client, None, false);
                    wm.queue_erase_background(h_client);

                    let pfn_wp = wm.get_window(h_client).map(|w| w.pfn_wp).unwrap_or(0);
                    (h_frame, h_client, pfn_wp)
                };

                if phwnd_client_ptr != 0 {
                    self.guest_write::<u32>(phwnd_client_ptr, h_client);
                }

                // Dispatch WM_CREATE synchronously to the client window procedure
                // before returning h_frame to the guest.  This mirrors real OS/2 PM
                // behaviour: controls and timers set up in WM_CREATE are ready before
                // WinCreateStdWindow returns.
                if pfn_wp_client != 0 {
                    debug!("  [VCPU {}] WinCreateStdWindow: dispatching WM_CREATE to pfn_wp=0x{:08X}", vcpu_id, pfn_wp_client);
                    return ApiResult::WmCreateCallback {
                        wnd_proc: pfn_wp_client,
                        hwnd: h_client,
                        h_frame,
                    };
                }

                ApiResult::Normal(h_frame)
            }
            915 => {
                // WinGetMsg(HAB hab, PQMSG pqmsg, HWND hwndFilter, ULONG msgFirst, ULONG msgLast):
                // wait for a message; FALSE when it is WM_QUIT.
                if !self.has_msg_queue(vcpu_id) { return self.pm_fail(vcpu_id, PMERR_NO_MSG_QUEUE); }
                // A filter that is no window: FALSE, PMERR_INVALID_HWND.
                if !self.pm_check_hwnd(vcpu_id, read_stack(12), true) { return ApiResult::Normal(0); }
                // The input hooks (pm_hook.rs) see the message before it is
                // returned and accelerators are translated.
                let pqmsg = read_stack(8);
                let hooks = self.input_hooks(vcpu_id, pqmsg);
                match self.next_message(vcpu_id, pqmsg, read_stack(12), read_stack(16),
                                        read_stack(20), PM_REMOVE, true, hooks.is_empty()) {
                    Some(_) if !hooks.is_empty() =>
                        self.input_hook_chain(vcpu_id, hooks, ordinal, pqmsg, PM_REMOVE, false),
                    Some(msg) => ApiResult::Normal((msg != WM_QUIT) as u32),
                    None => ApiResult::Normal(0),
                }
            }
            918 => {
                // WinPeekMsg(HAB hab, PQMSG pqmsg, HWND hwndFilter, ULONG msgFirst, ULONG msgLast, ULONG fl):
                // TRUE when a message is there, removed with PM_REMOVE; never waits.
                if !self.has_msg_queue(vcpu_id) { return self.pm_fail(vcpu_id, PMERR_NO_MSG_QUEUE); }
                if !self.pm_check_hwnd(vcpu_id, read_stack(12), true) { return ApiResult::Normal(0); }
                // fl is PM_REMOVE or PM_NOREMOVE; other bits are invalid.
                if read_stack(24) & !PM_REMOVE != 0 { return self.pm_fail(vcpu_id, PMERR_INVALID_FLAG); }
                let (pqmsg, fl) = (read_stack(8), read_stack(24));
                let hooks = self.input_hooks(vcpu_id, pqmsg);
                let found = self.next_message(vcpu_id, pqmsg, read_stack(12), read_stack(16),
                                              read_stack(20), fl, false, hooks.is_empty());
                if found.is_some() && !hooks.is_empty() {
                    return self.input_hook_chain(vcpu_id, hooks, ordinal, pqmsg, fl, true);
                }
                ApiResult::Normal(found.is_some() as u32)
            }
            912 => {
                // WinDispatchMsg
                debug!("  [VCPU {}] WinDispatchMsg called.", vcpu_id);
                let _hab = read_stack(4);
                let pqmsg_ptr = read_stack(8);
                if pqmsg_ptr == 0 { return ApiResult::Normal(0); }

                let (hwnd, msg, mp1, mp2) = (
                    self.guest_read::<u32>(pqmsg_ptr).unwrap_or(0),
                    self.guest_read::<u32>(pqmsg_ptr + 4).unwrap_or(0),
                    self.guest_read::<u32>(pqmsg_ptr + 8).unwrap_or(0),
                    self.guest_read::<u32>(pqmsg_ptr + 12).unwrap_or(0),
                );

                let pfn_wp = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    wm.get_window(hwnd).map(|w| w.pfn_wp).unwrap_or(0)
                };

                if pfn_wp != 0 {
                    debug!("  [VCPU {}] Callback: msg={} to pfn_wp 0x{:08X}", vcpu_id, msg, pfn_wp);
                    return ApiResult::Callback { wnd_proc: pfn_wp, hwnd, msg, mp1, mp2 };
                }
                // pfn_wp == 0: route to built-in control handler (WC_BUTTON, WC_STATIC, …)
                // A queued WM_PAINT for a control (creation, focus change) is
                // painted and presented; nothing else will present it.
                if msg == WM_PAINT && self.repaint_control(hwnd) {
                    return ApiResult::Normal(0);
                }
                self.dispatch_builtin_control(hwnd, msg, mp1, mp2)
            }
            919 => {
                // WinPostMsg
                let hwnd = read_stack(4);
                let msg = read_stack(8);
                let mp1 = read_stack(12);
                let mp2 = read_stack(16);
                if !self.pm_check_hwnd(vcpu_id, hwnd, true) { return ApiResult::Normal(0); }
                let wm = self.shared.window_mgr.lock_or_recover();
                let hmq = wm.find_hmq_for_hwnd(hwnd);
                // FALSE when there is no queue to post to (e.g. destroyed).
                let Some(mq_arc) = hmq.and_then(|hmq| wm.get_mq(hmq)) else {
                    return ApiResult::Normal(0);
                };
                let mut mq = mq_arc.lock_or_recover();
                mq.messages.push_back(OS2Message {
                    hwnd, msg, mp1, mp2, time: 0, x: 0, y: 0,
                });
                // Wake WinGetMsg if it's waiting on the condvar
                mq.cond.notify_one();
                ApiResult::Normal(1)
            }
            920 => {
                // WinSendMsg(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2):
                // synchronous, after the send hooks (pm_message.rs).
                self.win_send_msg(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16))
            }
            902 => {
                // WinPostQueueMsg(HMQ hmq, ULONG msg, MPARAM mp1, MPARAM mp2) -> BOOL
                self.win_post_queue_msg(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16))
            }
            901 => {
                // WinBroadcastMsg(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2, ULONG rgf) -> BOOL
                self.win_broadcast_msg(vcpu_id, read_stack(4), read_stack(8), read_stack(12),
                                       read_stack(16), read_stack(20))
            }
            761 => {
                // WinInSendMsg(HAB hab) -> BOOL
                self.win_in_send_msg(vcpu_id)
            }
            897 => {
                // WinWaitMsg(HAB hab, ULONG msgFirst, ULONG msgLast) -> BOOL
                self.win_wait_msg(vcpu_id, read_stack(8), read_stack(12))
            }
            978 => {
                // WinWaitEventSem(HEV hev, ULONG ulTimeout) -> APIRET: as
                // DosWaitEventSem (pm_message.rs: no sent messages to process).
                ApiResult::Normal(self.dos_wait_event_sem(read_stack(4), read_stack(8)))
            }
            979 => {
                // WinRequestMutexSem(HMTX hmtx, ULONG ulTimeout) -> APIRET:
                // as DosRequestMutexSem.
                ApiResult::Normal(self.dos_request_mutex_sem(vcpu_id, read_stack(4), read_stack(8)))
            }
            928 => {
                // WinSetHook(HAB hab, HMQ hmq, LONG iHook, PFN pfnHook, HMODULE hmod) -> BOOL
                self.win_set_hook(vcpu_id, read_stack(8), read_stack(12) as i32, read_stack(16))
            }
            927 => {
                // WinReleaseHook(HAB hab, HMQ hmq, LONG iHook, PFN pfnHook, HMODULE hmod) -> BOOL
                self.win_release_hook(vcpu_id, read_stack(8), read_stack(12) as i32, read_stack(16))
            }
            905 => {
                // WinCallMsgFilter(HAB hab, PQMSG pqmsg, ULONG msgf) -> BOOL
                self.win_call_msg_filter(vcpu_id, read_stack(8), read_stack(12))
            }
            911 => {
                // WinDefWindowProc
                let hwnd = read_stack(4);
                let msg = read_stack(8);
                let mp1 = read_stack(12);
                let mp2 = read_stack(16);

                // Frame formatting messages of frame windows (pm_frame.rs).
                if let Some(r) = self.frame_default_message(hwnd, msg, mp1, mp2) {
                    return ApiResult::Normal(r);
                }
                match msg {
                    // The system menu and keyboard move / size (pm_sysmenu.rs).
                    WM_SYSCOMMAND if self.frame_syscommand(hwnd, mp1 & 0xFFFF, mp2) => {}
                    WM_CLOSE => { self.post_wm_quit(hwnd); }
                    // Help: the help hooks of a frame (pm_message.rs).
                    WM_HELP => return self.help_hook(vcpu_id, hwnd, mp1, mp2),
                    // "The default window procedure sets the pointer shape":
                    // the arrow (pm_pointer.rs).
                    WM_MOUSEMOVE => { self.default_mouse_pointer(); }
                    // As WinBeginPaint + WinEndPaint would: the window is valid.
                    // (Its children keep their own update rectangles.)
                    WM_PAINT => { self.shared.window_mgr.lock_or_recover().begin_paint(hwnd); }
                    // Frame controls changed: show the frame's current menu
                    // (standard frames; dialogs do not take menus yet).
                    WM_UPDATEFRAME => {
                        let is_frame = self.shared.window_mgr.lock_or_recover()
                            .frame_to_client.contains_key(&hwnd);
                        if is_frame && mp1 & FCF_MENU != 0 {
                            self.update_frame_menu(hwnd);
                        }
                        return ApiResult::Normal(is_frame as u32);
                    }
                    // The system command Close (system menu, Alt+F4) asks the
                    // window to close.
                    WM_SYSCOMMAND if mp1 & 0xFFFF == SC_CLOSE => {
                        self.post_to_window(hwnd, WM_CLOSE, 0, 0);
                    }
                    // Hide (the hide button): the frame becomes invisible
                    // (PM Reference, WM_SYSCOMMAND: "Sets the visibility
                    // state of the frame window to off").
                    WM_SYSCOMMAND if mp1 & 0xFFFF == SC_HIDE => {
                        let mut wm = self.shared.window_mgr.lock_or_recover();
                        let frame = wm.client_to_frame(hwnd);
                        if wm.is_frame(frame) { wm.hide_frame(frame); }
                    }
                    // Minimize, maximize, restore: done by frames with
                    // minimize / maximize buttons (FID_MINMAX), else nothing.
                    WM_SYSCOMMAND => {
                        if let Some(op) = super::pm_minmax::syscommand_state(mp1 & 0xFFFF) {
                            let prepared = {
                                let mut wm = self.shared.window_mgr.lock_or_recover();
                                let frame = wm.client_to_frame(hwnd);
                                if wm.has_minmax_buttons(frame) {
                                    wm.begin_minmax(frame, op, None, None).map(|t| (frame, t))
                                } else { None }
                            };
                            if let Some((frame, target)) = prepared {
                                return self.send_minmax(frame, op, target, 0);
                            }
                        }
                    }
                    WM_BUTTON1DOWN | WM_BUTTON1DBLCLK | WM_BUTTON1UP => {
                        // If this frame has a menu, delegate mouse events to the menu
                        // control handler so it can handle menu-bar clicks and dropdown
                        // item selection.
                        //
                        // The input queue (pm_input.rs) delivers a frame's pointer messages to
                        // its client (frame_to_client),
                        // so WM_BUTTON1DOWN arrives with hwnd = client_hwnd.  We must also
                        // check the frame window (via client_to_frame) for menu_hwnd.
                        let (menu_hwnd, frame_cy) = {
                            let wm = self.shared.window_mgr.lock_or_recover();
                            // First try hwnd directly, then its frame ancestor.
                            let mut mh = wm.get_window(hwnd).map(|w| w.menu_hwnd).unwrap_or(0);
                            let mut frame_h = hwnd;
                            if mh == 0 {
                                frame_h = wm.client_to_frame(hwnd);
                                if frame_h != hwnd {
                                    mh = wm.get_window(frame_h).map(|w| w.menu_hwnd).unwrap_or(0);
                                }
                            }
                            let cy = wm.get_window(frame_h).map(|w| w.cy).unwrap_or(0);
                            (mh, cy)
                        };
                        // The second press of a double click acts as a press.
                        if menu_hwnd != 0 && msg != WM_BUTTON1UP {
                            let click_y = ((mp1 >> 16) & 0xFFFF) as i32;
                            // Check if click is in the menu bar or open dropdown.
                            // Title bar occupies [frame_cy-CHROME_TITLE_H, frame_cy).
                            // Menu bar occupies [frame_cy-CHROME_TITLE_H-MENU_BAR_HEIGHT, frame_cy-CHROME_TITLE_H).
                            // Dropdown is further below; we forward any click at or below menu bar top edge.
                            let menu_y1 = frame_cy - CHROME_TITLE_H - MENU_BAR_HEIGHT as i32;
                            // Only intercept clicks that are at or above menu_y1 (in bar),
                            // or check if there's an open dropdown covering the click area.
                            let open_item = {
                                let wm = self.shared.window_mgr.lock_or_recover();
                                wm.get_window(menu_hwnd)
                                    .and_then(|w| w.window_ulong.get(&-1)).copied().unwrap_or(0)
                            };
                            if click_y >= menu_y1 || open_item > 0 {
                                return self.dispatch_builtin_control(menu_hwnd, WM_BUTTON1DOWN, mp1, 0);
                            }
                        }
                    }
                    _ => {}
                }
                ApiResult::Normal(0)
            }
            703 => {
                // WinBeginPaint(HWND hwnd, HPS hps, PRECTL prclPaint) → HPS
                // With hps = 0 a presentation space is created for the paint
                // and freed by WinEndPaint; otherwise the given one is used.
                let hwnd = read_stack(4);
                let hps_in = read_stack(8);
                let prcl = read_stack(12);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let hps = if hps_in != 0 && wm.ps_map.contains_key(&hps_in) {
                    hps_in
                } else {
                    let h = wm.create_ps(hwnd);
                    if let Some(ps) = wm.ps_map.get_mut(&h) { ps.origin = PsOrigin::Paint; }
                    h
                };
                drop(wm);
                // prclPaint receives the update rectangle, which is emptied
                // (pm_paint.rs).
                self.begin_paint_rect(hwnd, prcl);
                ApiResult::Normal(hps)
            }
            738 => {
                // WinEndPaint
                let hps = read_stack(4);
                let (frame_hwnd, controls) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let ps_hwnd = wm.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0);
                    (wm.client_to_frame(ps_hwnd), wm.builtin_control_descendants(ps_hwnd))
                };
                // Child controls share the window's FrameBuffer, so whatever the
                // window just painted (typically a full background fill) has
                // covered them. Repaint them on top before presenting.
                for hwnd_ctl in controls {
                    self.dispatch_builtin_control(hwnd_ctl, WM_PAINT, 0, 0);
                }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.mark_dirty(frame_hwnd);
                // A presentation space WinBeginPaint created ends with the paint.
                if wm.ps_map.get(&hps).is_some_and(|ps| ps.origin == PsOrigin::Paint) {
                    wm.remove_ps(hps);
                }
                ApiResult::Normal(1)
            }
            757 => {
                // WinGetPS(HWND hwnd) → HPS (freed by WinReleasePS)
                let hwnd = read_stack(4);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if wm.get_window(hwnd).is_none() { return ApiResult::Normal(0); }
                let hps = wm.create_ps(hwnd);
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.origin = PsOrigin::Cached; }
                ApiResult::Normal(hps)
            }
            848 => {
                // WinReleasePS(HPS hps) → BOOL
                let hps = read_stack(4);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let ok = wm.ps_map.get(&hps).is_some_and(|ps| ps.origin == PsOrigin::Cached);
                if ok { wm.remove_ps(hps); }
                ApiResult::Normal(ok as u32)
            }
            794 => {
                // WinOpenWindowDC(HWND hwnd) → HDC (one per window)
                let hwnd = read_stack(4);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if wm.get_window(hwnd).is_none() { return ApiResult::Normal(0); }
                let existing = wm.dcs.iter()
                    .find(|&(_, &dc)| dc == DeviceContext::Window(hwnd)).map(|(&h, _)| h);
                let hdc = existing.unwrap_or_else(|| wm.open_dc(DeviceContext::Window(hwnd)));
                ApiResult::Normal(hdc)
            }
            835 => {
                // WinQueryWindowDC(HWND hwnd) → HDC (0 if none was opened)
                let hwnd = read_stack(4);
                let wm = self.shared.window_mgr.lock_or_recover();
                let hdc = wm.dcs.iter()
                    .find(|&(_, &dc)| dc == DeviceContext::Window(hwnd)).map_or(0, |(&h, _)| h);
                ApiResult::Normal(hdc)
            }
            730 => {
                // WinDrawBitmap(HPS hpsDst, HBITMAP hbm, PRECTL pwrcSrc, PPOINTL pptlDst,
                //               LONG clrFore, LONG clrBack, ULONG fl) → BOOL
                // pptlDst is a RECTL with DBM_STRETCH. DBM_INVERT and DBM_XOR
                // select the raster operation; DBM_HALFTONE / DBM_IMAGEATTRS
                // (monochrome colouring) are ignored.
                let (hps, hbm, psrc, pdst, fl) =
                    (read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(28));
                let rect = |p: u32| -> Option<(i64, i64, i64, i64)> {
                    let v = |i: u32| self.guest_read::<i32>(p.wrapping_add(4 * i)).map(|v| v as i64);
                    Some((v(0)?, v(1)?, v(2)?, v(3)?))
                };
                let src = if psrc != 0 {
                    match rect(psrc) { Some(r) => Some(r), None => return ApiResult::Normal(0) }
                } else { None };
                let dst = if fl & DBM_STRETCH != 0 {
                    rect(pdst)
                } else {
                    let size = src.map(|(x0, y0, x1, y1)| (x1 - x0, y1 - y0)).or_else(|| {
                        let wm = self.shared.window_mgr.lock_or_recover();
                        wm.bitmaps.get(&hbm).map(|b| {
                            let s = b.surface.lock_or_recover();
                            (s.width as i64, s.height as i64)
                        })
                    });
                    match (self.guest_read::<i32>(pdst), self.guest_read::<i32>(pdst.wrapping_add(4)), size) {
                        (Some(x), Some(y), Some((w, h))) => Some((x as i64, y as i64, x as i64 + w, y as i64 + h)),
                        _ => None,
                    }
                };
                let Some(dst) = dst else { return ApiResult::Normal(0) };
                let rop = if fl & DBM_INVERT != 0 { ROP_NOTSRCCOPY }
                    else if fl & DBM_XOR != 0 { ROP_SRCINVERT } else { ROP_SRCCOPY };
                ApiResult::Normal(self.draw_bitmap(hps, hbm, src, dst, rop))
            }
            753 => {
                // WinGetLastError(HAB hab): this thread's last PMERR_*, reset to 0.
                ApiResult::Normal(self.shared.pm_errors.take(vcpu_id))
            }
            789 => {
                // WinMessageBox(HWND hwndParent, HWND hwndOwner, PCSZ pszText,
                //               PCSZ pszCaption, ULONG idWindow, ULONG flStyle)
                //
                // A modal PM dialog (pm_msgbox.rs) run by the dialog loop;
                // other windows keep running. A program without a PM desktop
                // (a text-mode program) gets MBID_OK at once.
                let _hwnd_parent    = read_stack(4);
                let hwnd_owner      = read_stack(8);
                let psz_text_ptr    = read_stack(12);
                let psz_caption_ptr = read_stack(16);
                let _id             = read_stack(20);
                let style           = read_stack(24);

                let cp      = self.thread_codepage(vcpu_id);
                let text    = self.read_guest_string_cp(psz_text_ptr, cp);
                let caption = self.read_guest_string_cp(psz_caption_ptr, cp);
                info!("  [VCPU {}] WinMessageBox '{}' : '{}'", vcpu_id, caption, text);
                let has_desktop = self.shared.window_mgr.lock_or_recover().gui_tx.is_some();
                if !has_desktop {
                    return ApiResult::Normal(MBID_OK);
                }
                self.message_box(vcpu_id, hwnd_owner, caption, &text, style)
            }
            883 => {
                // WinShowWindow(HWND hwnd, BOOL fShow)
                let hwnd = read_stack(4);
                let show = read_stack(8) != 0;
                debug!("  [VCPU {}] WinShowWindow hwnd={} show={}", vcpu_id, hwnd, show);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    win.visible = show;
                }
                wm.mark_dirty(hwnd); // VDR-B4
                wm.frame_visibility_changed(hwnd);
                ApiResult::Normal(1)
            }
            840 => {
                // WinQueryWindowRect(HWND hwnd, PRECTL prcl)
                let hwnd = read_stack(4);
                let prcl_ptr = read_stack(8);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                if prcl_ptr != 0 {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let (cx, cy) = wm.get_window(hwnd)
                        .map(|w| (w.cx, w.cy))
                        .unwrap_or((640, 480));
                    self.guest_write::<i32>(prcl_ptr, 0);       // xLeft
                    self.guest_write::<i32>(prcl_ptr + 4, 0);   // yBottom
                    self.guest_write::<i32>(prcl_ptr + 8, cx);  // xRight
                    self.guest_write::<i32>(prcl_ptr + 12, cy); // yTop
                }
                ApiResult::Normal(1) // TRUE
            }
            728 => {
                // WinDestroyWindow(HWND hwnd): WM_DESTROY to the window and
                // its descendants, then they go (`pm_destroy.rs`).
                self.win_destroy_window(vcpu_id, read_stack(4))
            }
            772 => {
                // WinIsWindow(HAB hab, HWND hwnd)
                ApiResult::Normal(self.shared.window_mgr.lock_or_recover().is_window(read_stack(8)) as u32)
            }
            884 => {
                // WinStartTimer(HAB hab, HWND hwnd, ULONG idTimer, ULONG dtTimeout)
                let _hab = read_stack(4);
                let hwnd = read_stack(8);
                let id_timer = read_stack(12);
                let dt_timeout = read_stack(16);
                debug!("  [VCPU {}] WinStartTimer: hwnd={}, id={}, timeout={}ms", vcpu_id, hwnd, id_timer, dt_timeout);

                let stop = Arc::new(TimerStop::default());
                let stop_clone = stop.clone();
                let shared = Arc::clone(&self.shared);

                // Stop any existing timer with the same id (immediate: its
                // thread sleeps on its TimerStop).
                let old = self.shared.window_mgr.lock_or_recover().timers.remove(&(hwnd, id_timer));
                if let Some((old_stop, old_handle)) = old {
                    old_stop.stop();
                    if let Some(h) = old_handle { let _ = h.join(); }
                }

                let timeout = std::time::Duration::from_millis(dt_timeout as u64);
                let join_handle = thread::spawn(move || {
                    while !stop_clone.is_stopped()
                        && !shared.exit_requested.load(std::sync::atomic::Ordering::Relaxed) {
                        if stop_clone.sleep(timeout) { break; }
                        let wm = shared.window_mgr.lock_or_recover();
                        let hmq = wm.find_hmq_for_hwnd(hwnd);
                        if let Some(hmq) = hmq
                            && let Some(mq_arc) = wm.get_mq(hmq) {
                                let mut mq = mq_arc.lock_or_recover();
                                mq.messages.push_back(OS2Message {
                                    hwnd, msg: WM_TIMER, mp1: id_timer, mp2: 0,
                                    time: 0, x: 0, y: 0,
                                });
                                mq.cond.notify_one();
                        }
                    }
                });
                {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    wm.timers.insert((hwnd, id_timer), (stop, Some(join_handle)));
                }
                ApiResult::Normal(id_timer) // Return the timer ID
            }
            885 => {
                // WinStopTimer(HAB hab, HWND hwnd, ULONG idTimer)
                let _hab = read_stack(4);
                let hwnd = read_stack(8);
                let id_timer = read_stack(12);
                if !self.pm_check_hwnd(vcpu_id, hwnd, true) { return ApiResult::Normal(0); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some((stop, handle)) = wm.timers.remove(&(hwnd, id_timer)) {
                    stop.stop();
                    drop(wm); // Release lock before joining
                    if let Some(h) = handle { let _ = h.join(); }
                }
                ApiResult::Normal(1)
            }
            701 => {
                // WinAlarm(HWND hwndDesktop, ULONG rgfType)
                // Just a beep - stub it
                ApiResult::Normal(1)
            }
            707 => {
                // WinCloseClipbrd(HAB hab)
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.clipboard_open = false;
                ApiResult::Normal(1)
            }
            857 => {
                // WinSetCp(HMQ hmq, ULONG idCodePage) -> BOOL
                let hmq = read_stack(4);
                let cp  = read_stack(8);
                let ok = self.win_set_cp(hmq, cp);
                debug!("  [VCPU {}] WinSetCp hmq={} cp={} -> {}", vcpu_id, hmq, cp, ok);
                ApiResult::Normal(ok as u32)
            }
            811 => {
                // WinQueryCpList(HAB hab, ULONG ccpMax, PULONG prgcp) -> ULONG:
                // the codepages a message queue can use; up to ccpMax are
                // written, and the number available is returned.
                let max = read_stack(8) as usize;
                let list = read_stack(12);
                let cps = super::stubs::pm_codepages();
                if list != 0 {
                    for (i, &cp) in cps.iter().take(max).enumerate() {
                        self.guest_write::<u32>(list + 4 * i as u32, cp);
                    }
                }
                ApiResult::Normal(cps.len() as u32)
            }
            810 => {
                // WinQueryCp(HMQ hmq) -> ULONG (0 on error)
                let hmq = read_stack(4);
                let cp = self.win_query_cp(hmq);
                debug!("  [VCPU {}] WinQueryCp hmq={} -> {}", vcpu_id, hmq, cp);
                ApiResult::Normal(cp)
            }
            907 => {
                // WinCreateMenu(HWND hwndParent, PVOID pvmt)
                let hwnd_parent = read_stack(4);
                let pvmt        = read_stack(8);
                let items = if pvmt != 0 {
                    // pvmt points to an inline MENUTEMPLATE — parse it.
                    // We don't know the exact size; use a generous bound.
                    let items = self.parse_menu_template(pvmt, 4096);
                    debug!("  [VCPU {}] WinCreateMenu: {} top-level items from pvmt=0x{:08X}",
                           vcpu_id, items.len(), pvmt);
                    items
                } else {
                    Vec::new()
                };
                // Shown once the frame gets WM_UPDATEFRAME(FCF_MENU).
                ApiResult::Normal(self.create_menu_window(hwnd_parent, items))
            }
            910 => {
                // WinDefDlgProc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2)
                self.def_dlg_proc(read_stack(4), read_stack(8), read_stack(12), read_stack(16))
            }
            729 => {
                // WinDismissDlg(HWND hwndDlg, ULONG usResult)
                let hwnd   = read_stack(4);
                let result = read_stack(8);
                debug!("  [VCPU {}] WinDismissDlg hwnd={} result={}", vcpu_id, hwnd, result);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                self.dismiss_dialog(hwnd, result);
                ApiResult::Normal(1)
            }
            923 => {
                // WinDlgBox(hwndParent, hwndOwner, pfnDlgProc, hmod, idDlg, pCreateParams)
                let pfn_dlg_proc = read_stack(12);
                if pfn_dlg_proc == 0 {
                    return ApiResult::Normal(DID_ERROR);
                }
                let (hwnd_dlg, hmq) = self.load_dialog(vcpu_id, read_stack(4), read_stack(8),
                    pfn_dlg_proc, read_stack(16), read_stack(20), read_stack(24));
                // Modal: the owner is disabled until the dialog ends, and
                // WinDlgBox destroys the dialog before it returns.
                self.shared.window_mgr.lock_or_recover().begin_modal_dialog(hwnd_dlg, true);
                ApiResult::DlgRunLoop { dlg_proc: pfn_dlg_proc, hwnd_dlg, hmq }
            }
            733 => {
                // WinEmptyClipbrd(HAB hab)
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.clipboard.clear();
                ApiResult::Normal(1)
            }
            743 => {
                // WinFillRect(HPS hps, PRECTL prcl, LONG lColor)
                let hps = read_stack(4);
                let prcl = read_stack(8);
                let color_idx = read_stack(12);
                let color = self.map_color(color_idx);
                let (x_left, y_bottom, x_right, y_top) = (
                    self.guest_read::<i32>(prcl).unwrap_or(0),
                    self.guest_read::<i32>(prcl + 4).unwrap_or(0),
                    self.guest_read::<i32>(prcl + 8).unwrap_or(0),
                    self.guest_read::<i32>(prcl + 12).unwrap_or(0),
                );
                // Into whatever the PS draws into (its window or a bitmap),
                // through its clip region.
                let Some(t) = self.ps_target(hps) else { return ApiResult::Normal(0) };
                // RECTL is exclusive at the top and right.
                self.draw_ps(&t, DrawOp::Box {
                    x1: x_left, y1: y_bottom, x2: x_right - 1, y2: y_top - 1,
                    color, fill: true, mix: Mix::Overpaint,
                });
                ApiResult::Normal(1)
            }
            766 => {
                // WinInvalidateRegion(HWND hwnd, HRGN hrgn, BOOL fIncludeChildren) (pm_region.rs)
                self.win_invalidate_region(vcpu_id, read_stack(4), read_stack(8), read_stack(12))
            }
            896 => {
                // WinValidateRegion(HWND hwnd, HRGN hrgn, BOOL fIncludeClippedChildren) (pm_region.rs)
                self.win_validate_region(vcpu_id, read_stack(4), read_stack(8), read_stack(12))
            }
            832 => {
                // WinQueryUpdateRegion(HWND hwnd, HRGN hrgn) (pm_region.rs)
                self.win_query_update_region(vcpu_id, read_stack(4), read_stack(8))
            }
            742 => {
                // WinExcludeUpdateRegion(HPS hps, HWND hwnd) (pm_region.rs)
                self.win_exclude_update_region(vcpu_id, read_stack(4), read_stack(8))
            }
            765 => {
                // WinInvalidateRect(HWND hwnd, PRECTL prcl, BOOL fIncludeChildren) (pm_paint.rs)
                self.win_invalidate_rect(vcpu_id, read_stack(4), read_stack(8), read_stack(12))
            }
            895 => {
                // WinValidateRect(HWND hwnd, PRECTL prcl, BOOL fIncludeClippedChildren) (pm_paint.rs)
                self.win_validate_rect(vcpu_id, read_stack(4), read_stack(8), read_stack(12))
            }
            831 => {
                // WinQueryUpdateRect(HWND hwnd, PRECTL prcl) (pm_paint.rs)
                self.win_query_update_rect(vcpu_id, read_stack(4), read_stack(8))
            }
            849 => {
                // WinScrollWindow(hwnd, lDx, lDy, prclScroll, prclClip, hrgnUpdate, prclUpdate, flOptions)
                self.win_scroll_window(vcpu_id, read_stack(4), read_stack(8) as i32, read_stack(12) as i32,
                    read_stack(16), read_stack(20), read_stack(24), read_stack(28), read_stack(32))
            }
            731 => {
                // WinDrawBorder(hps, prcl, cx, cy, clrFore, clrBack, flCmd) (pm_paint.rs)
                self.win_draw_border(vcpu_id, read_stack(4), read_stack(8), read_stack(12) as i32,
                    read_stack(16) as i32, read_stack(20), read_stack(24), read_stack(28))
            }
            776 => {
                // WinLoadAccelTable(HAB hab, HMODULE hmod, ULONG idAccelTable)
                ApiResult::Normal(self.load_accel_table(read_stack(8), read_stack(12)).unwrap_or(0))
            }
            924 => {
                // WinLoadDlg(hwndParent, hwndOwner, pfnDlgProc, hmod, idDlg, pCreateParams)
                let (hwnd_dlg, _) = self.load_dialog(vcpu_id, read_stack(4), read_stack(8),
                    read_stack(12), read_stack(16), read_stack(20), read_stack(24));
                ApiResult::Normal(hwnd_dlg)
            }
            778 => {
                // WinLoadMenu(HWND hwndFrame, HMODULE hmod, ULONG idMenu)
                let hwnd_frame = read_stack(4);
                let hmod       = read_stack(8);
                let id_menu    = read_stack(12);
                debug!("  [VCPU {}] WinLoadMenu hwnd_frame={} id={}", vcpu_id, hwnd_frame, id_menu);

                let items = self.load_menu_resource(hmod, id_menu, vcpu_id).unwrap_or_default();
                // As in PM, the frame shows the new menu once it is sent
                // WM_UPDATEFRAME(FCF_MENU).
                ApiResult::Normal(self.create_menu_window(hwnd_frame, items))
            }
            781 => {
                // WinLoadString(HAB hab, HMODULE hmod, ULONG id, LONG cchMax, PSZ pszBuffer) (pm_winmisc.rs)
                self.win_load_bundle_string(vcpu_id, RT_STRING, read_stack(8), read_stack(12),
                                            read_stack(16) as i32, read_stack(20))
            }
            779 => {
                // WinLoadMessage(HAB hab, HMODULE hmod, ULONG id, LONG cchMax, PSZ pszBuffer) (pm_winmisc.rs)
                self.win_load_bundle_string(vcpu_id, RT_MESSAGE, read_stack(8), read_stack(12),
                                            read_stack(16) as i32, read_stack(20))
            }
            793 => {
                // WinOpenClipbrd(HAB hab)
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.clipboard_open = true;
                ApiResult::Normal(1)
            }
            851 => {
                // WinSetActiveWindow(HWND hwndDesktop, HWND hwnd) -> BOOL
                // VDR-E2: set focused_hwnd and raise to top of Z-order;
                // post WM_ACTIVATE to both the old and new active windows (VDR-D4).
                let _hwnd_desktop = read_stack(4);
                let hwnd = read_stack(8);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                let (prev, new_frame) = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let frame = {
                        let candidate = wm.client_to_frame(hwnd);
                        if wm.frame_to_client.contains_key(&candidate) { candidate } else { hwnd }
                    };
                    let prev = wm.focused_hwnd;
                    if wm.frame_to_client.contains_key(&frame) {
                        wm.focused_hwnd = frame;
                        wm.z_push_top(frame);
                        (prev, frame)
                    } else {
                        (0, 0)
                    }
                };
                if new_frame != 0 && prev != new_frame {
                    // WM_ACTIVATE goes to each frame's client, as for a click
                    // and for minimizing (`frame_msg_target`): FALSE to the
                    // old frame first, then TRUE to the new one.
                    let wm = self.shared.window_mgr.lock_or_recover();
                    if prev != 0 {
                        wm.post_msg(wm.frame_msg_target(prev), WM_ACTIVATE, 0, new_frame);
                    }
                    wm.post_msg(wm.frame_msg_target(new_frame), WM_ACTIVATE, 1, prev);
                }
                ApiResult::Normal(if new_frame != 0 { 1 } else { 0 })
            }
            860 => {
                // WinSetFocus(HWND hwndDesktop, HWND hwndNewFocus) -> BOOL
                // The focus window receives keyboard input; NULLHANDLE means
                // no window has the focus (keys go to the active frame's client).
                // A desktop window means the same (PM Reference, WinSetFocus
                // remarks: no window on that desktop receives the focus).
                let _hwnd_desktop = read_stack(4);
                let hwnd = read_stack(8);
                if !self.pm_check_hwnd(vcpu_id, hwnd, true) { return ApiResult::Normal(0); }
                let hwnd = if hwnd == HWND_DESKTOP { 0 } else { hwnd };
                let exists = hwnd == 0
                    || self.shared.window_mgr.lock_or_recover().get_window(hwnd).is_some();
                if exists {
                    self.set_focus_window(hwnd);
                }
                ApiResult::Normal(exists as u32)
            }
            817 => {
                // WinQueryFocus(HWND hwndDesktop) -> HWND
                // The focus window, or the active frame's client when no
                // window has taken the focus explicitly.
                let wm = self.shared.window_mgr.lock_or_recover();
                let target = wm.key_target();
                ApiResult::Normal(wm.frame_to_client.get(&target).copied().unwrap_or(target))
            }
            799 => {
                // WinQueryActiveWindow(HWND hwndDesktop) -> HWND
                // VDR-E2: return focused_hwnd.
                let _hwnd_desktop = read_stack(4);
                let wm = self.shared.window_mgr.lock_or_recover();
                ApiResult::Normal(wm.focused_hwnd)
            }
            937 => {
                // WinPopupMenu(HWND hwndParent, HWND hwndOwner, HWND hwndMenu, LONG x, LONG y, LONG idItem, ULONG fs)
                let args = [read_stack(4), read_stack(8), read_stack(12), read_stack(16),
                            read_stack(20), read_stack(24), read_stack(28)];
                self.win_popup_menu(vcpu_id, args)
            }
            796 => {
                // WinProcessDlg(HWND hwndDlg) — run the modal dialog loop
                let hwnd_dlg = read_stack(4);
                debug!("  [VCPU {}] WinProcessDlg hwnd={}", vcpu_id, hwnd_dlg);
                if !self.pm_check_hwnd(vcpu_id, hwnd_dlg, false) { return ApiResult::Normal(DID_CANCEL); }
                let (pfn_wp, hmq) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let pfn = wm.get_window(hwnd_dlg).map(|w| w.pfn_wp).unwrap_or(0);
                    let hmq = wm.find_hmq_for_hwnd(hwnd_dlg).unwrap_or(0);
                    (pfn, hmq)
                };
                if pfn_wp == 0 || hmq == 0 {
                    return ApiResult::Normal(DID_CANCEL);
                }
                // Modal until dismissed; the dialog stays for WinDestroyWindow.
                self.shared.window_mgr.lock_or_recover().begin_modal_dialog(hwnd_dlg, false);
                ApiResult::DlgRunLoop { dlg_proc: pfn_wp, hwnd_dlg, hmq }
            }
            804 => {
                // WinQueryCapture(HWND hwndDesktop) -> HWND
                let _hwnd_desktop = read_stack(4);
                let wm = self.shared.window_mgr.lock_or_recover();
                ApiResult::Normal(wm.capture_hwnd)
            }
            806 => {
                // WinQueryClipbrdData(HAB hab, ULONG fmt)
                let _hab = read_stack(4);
                let fmt = read_stack(8);
                let wm = self.shared.window_mgr.lock_or_recover();
                let data = wm.clipboard.get(&fmt).copied().unwrap_or(0);
                ApiResult::Normal(data)
            }
            815 => {
                // WinQueryDlgItemText(HWND hwndDlg, ULONG idItem, LONG cchBufferMax, PCSZ pchBuffer)
                let hwnd_dlg = read_stack(4);
                let id_item = read_stack(8);
                let cch_max = read_stack(12) as usize;
                let buffer_ptr = read_stack(16);
                if !self.pm_check_hwnd(vcpu_id, hwnd_dlg, false) { return ApiResult::Normal(0); }
                let cp = self.window_codepage(hwnd_dlg);
                let wm = self.shared.window_mgr.lock_or_recover();
                let text = wm.find_child_by_id(hwnd_dlg, id_item)
                    .and_then(|h| wm.get_window(wm.text_window(h)))
                    .map(|w| w.text.clone())
                    .unwrap_or_default();
                drop(wm);
                let bytes = super::codepage::cp_encode(&text, cp);
                let copy_len = bytes.len().min(cch_max.saturating_sub(1));
                self.guest_write_bytes(buffer_ptr, &bytes[..copy_len]);
                self.guest_write::<u8>(buffer_ptr + copy_len as u32, 0);
                ApiResult::Normal(copy_len as u32)
            }
            828 => {
                // WinQuerySysPointer(HWND hwndDesktop, LONG iptr, BOOL fCopy)
                self.win_query_sys_pointer(vcpu_id, read_stack(4), read_stack(8), read_stack(12) != 0)
            }
            829 => {
                // WinQuerySysValue(HWND hwndDesktop, LONG iSysValue)
                let _hwnd = read_stack(4);
                let sys_val = read_stack(8) as i32;
                let v = self.pointer_sys_value(sys_val).unwrap_or_else(|| self.query_sys_value(sys_val));
                ApiResult::Normal(v as u32)
            }
            821 => {
                // WinQueryPointer(HWND hwndDesktop)
                self.win_query_pointer(vcpu_id, read_stack(4))
            }
            881 => {
                // WinShowPointer(HWND hwndDesktop, BOOL fShow)
                self.win_show_pointer(vcpu_id, read_stack(4), read_stack(8) != 0)
            }
            780 => {
                // WinLoadPointer(HWND hwndDesktop, HMODULE hmod, ULONG idres)
                self.win_load_pointer(vcpu_id, read_stack(4), read_stack(8), read_stack(12))
            }
            727 => {
                // WinDestroyPointer(HPOINTER hptr)
                self.win_destroy_pointer(vcpu_id, read_stack(4))
            }
            841 => {
                // WinQueryWindowText(HWND hwnd, LONG cchBufferMax, PCH pchBuffer)
                let hwnd = read_stack(4);
                let cch_max = read_stack(8) as usize;
                let buffer_ptr = read_stack(12);
                self.pm_check_hwnd(vcpu_id, hwnd, false);
                let cp = self.window_codepage(hwnd);
                let wm = self.shared.window_mgr.lock_or_recover();
                let text = wm.get_window(wm.text_window(hwnd)).map(|w| w.text.clone()).unwrap_or_default();
                drop(wm);
                let bytes = super::codepage::cp_encode(&text, cp);
                let copy_len = bytes.len().min(cch_max.saturating_sub(1));
                self.guest_write_bytes(buffer_ptr, &bytes[..copy_len]);
                self.guest_write::<u8>(buffer_ptr + copy_len as u32, 0);
                ApiResult::Normal(copy_len as u32)
            }
            834 => {
                // WinQueryWindow(HWND hwnd, LONG lCode)
                let hwnd = read_stack(4);
                let code = read_stack(8) as i32;
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                if !(QW_NEXT..=QW_FRAMEOWNER).contains(&code) {
                    return self.pm_fail(vcpu_id, PMERR_PARAMETER_OUT_OF_RANGE);
                }
                ApiResult::Normal(self.shared.window_mgr.lock_or_recover().query_window(hwnd, code))
            }
            843 => {
                // WinQueryWindowULong(HWND hwnd, LONG index)
                let hwnd = read_stack(4);
                let index = read_stack(8) as i32;
                if !self.pm_check_window_word(vcpu_id, hwnd, index, 4) { return ApiResult::Normal(0); }
                let wm = self.shared.window_mgr.lock_or_recover();
                let val = wm.get_window(hwnd).map_or(0, |w| if index == QWL_STYLE {
                    // The style, with WS_VISIBLE as the window is now.
                    (w.style & !WS_VISIBLE) | if w.visible { WS_VISIBLE } else { 0 }
                } else {
                    w.window_ulong.get(&index).copied().unwrap_or(0)
                });
                ApiResult::Normal(val)
            }
            844 => {
                // WinQueryWindowUShort(HWND hwnd, LONG index)
                let hwnd = read_stack(4);
                let index = read_stack(8) as i32;
                if !self.pm_check_window_word(vcpu_id, hwnd, index, 2) { return ApiResult::Normal(0); }
                let val = self.shared.window_mgr.lock_or_recover().query_window_ushort(hwnd, index);
                ApiResult::Normal(val as u32)
            }
            903 => {
                // WinSendDlgItemMsg(HWND hwndDlg, ULONG idItem, ULONG msg, MPARAM mp1, MPARAM mp2)
                let hwnd_dlg = read_stack(4);
                let id_item  = read_stack(8);
                let msg      = read_stack(12);
                let mp1      = read_stack(16);
                let mp2      = read_stack(20);
                if !self.pm_check_hwnd(vcpu_id, hwnd_dlg, false) { return ApiResult::Normal(0); }
                let (child_hwnd, pfn_wp) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    if let Some(ch) = wm.find_child_by_id(hwnd_dlg, id_item) {
                        (ch, wm.get_window(ch).map(|w| w.pfn_wp).unwrap_or(0))
                    } else { (0, 0) }
                };
                // No such item: as WinSendMsg(WinWindowFromID(...)) to NULLHANDLE.
                if child_hwnd == 0 { return self.pm_fail(vcpu_id, PMERR_INVALID_HWND); }
                if pfn_wp != 0 {
                    return ApiResult::Callback { wnd_proc: pfn_wp, hwnd: child_hwnd, msg, mp1, mp2 };
                }
                self.dispatch_builtin_control(child_hwnd, msg, mp1, mp2)
            }
            850 => {
                // WinSetAccelTable(HAB hab, HACCEL haccel, HWND hwnd)
                let _hab = read_stack(4);
                let haccel = read_stack(8);
                let hwnd = read_stack(12);
                debug!("  [VCPU {}] WinSetAccelTable haccel={} hwnd={}", vcpu_id, haccel, hwnd);
                if !self.pm_check_hwnd(vcpu_id, hwnd, true) { return ApiResult::Normal(0); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if haccel != 0 && wm.accel_table(haccel).is_none() {
                    drop(wm);
                    return self.pm_fail(vcpu_id, PMERR_INVALID_HACCEL);
                }
                wm.set_window_accel(hwnd, haccel);
                ApiResult::Normal(1)
            }
            852 => {
                // WinSetCapture(HWND hwndDesktop, HWND hwnd)
                let _hwnd_desktop = read_stack(4);
                let hwnd = read_stack(8);
                debug!("  [VCPU {}] WinSetCapture hwnd={}", vcpu_id, hwnd);
                if hwnd != HWND_THREADCAPTURE && !self.pm_check_hwnd(vcpu_id, hwnd, true) {
                    return ApiResult::Normal(0);
                }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.capture_hwnd = hwnd;
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::SetMouseCapture(hwnd));
                }
                ApiResult::Normal(1)
            }
            854 => {
                // WinSetClipbrdData(HAB hab, ULONG ulData, ULONG fmt, ULONG rgfFmtInfo)
                let _hab = read_stack(4);
                let data = read_stack(8);
                let fmt = read_stack(12);
                let _flags = read_stack(16);
                // For CF_TEXT, also bridge the text to the host system clipboard.
                if fmt == CF_TEXT && data != 0 {
                    let text = self.read_guest_string_cp(data, self.thread_codepage(vcpu_id));
                    debug!("  [VCPU {}] WinSetClipbrdData CF_TEXT: {:?}", vcpu_id, text);
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    wm.clipboard_text = text.clone();
                    if let Some(ref sender) = wm.gui_tx {
                        let _ = sender.send(GUIMessage::SetClipboardText(text));
                    }
                    wm.clipboard.insert(fmt, data);
                } else {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    wm.clipboard.insert(fmt, data);
                }
                ApiResult::Normal(1)
            }
            859 => {
                // WinSetDlgItemText(HWND hwndDlg, ULONG idItem, PCSZ pszText)
                let hwnd_dlg = read_stack(4);
                let id_item = read_stack(8);
                let psz_text = read_stack(12);
                if !self.pm_check_hwnd(vcpu_id, hwnd_dlg, false) { return ApiResult::Normal(0); }
                let text = self.read_guest_string_cp(psz_text, self.window_codepage(hwnd_dlg));
                self.set_dlg_item_text(hwnd_dlg, id_item, text);
                ApiResult::Normal(1)
            }
            858 => {
                // WinSetDlgItemShort(HWND hwndDlg, ULONG idItem, USHORT usValue, BOOL fSigned) (pm_winmisc.rs)
                self.win_set_dlg_item_short(vcpu_id, read_stack(4), read_stack(8), read_stack(12) as u16, read_stack(16) != 0)
            }
            814 => {
                // WinQueryDlgItemShort(HWND hwndDlg, ULONG idItem, PSHORT psResult, BOOL fSigned) (pm_winmisc.rs)
                self.win_query_dlg_item_short(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16) != 0)
            }
            873 => {
                // WinSetSysValue(HWND hwndDesktop, LONG iSysValue, LONG lValue)
                let hwnd = read_stack(4);
                let index = read_stack(8) as i32;
                let value = read_stack(12) as i32;
                self.set_sys_value(vcpu_id, hwnd, index, value)
            }
            715 => {
                // WinCreateCursor(HWND hwnd, LONG x, LONG y, LONG cx, LONG cy, ULONG fs, PRECTL prclClip)
                let rect = (read_stack(8) as i32, read_stack(12) as i32,
                            read_stack(16) as i32, read_stack(20) as i32);
                self.win_create_cursor(vcpu_id, read_stack(4), rect, read_stack(24), read_stack(28))
            }
            880 => {
                // WinShowCursor(HWND hwnd, BOOL fShow) (pm_cursor.rs)
                self.win_show_cursor(vcpu_id, read_stack(4), read_stack(8) != 0)
            }
            725 => {
                // WinDestroyCursor(HWND hwnd) (pm_cursor.rs)
                self.win_destroy_cursor(vcpu_id, read_stack(4))
            }
            812 => {
                // WinQueryCursorInfo(HWND hwndDesktop, PCURSORINFO pCursorInfo) (pm_cursor.rs)
                self.win_query_cursor_info(vcpu_id, read_stack(4), read_stack(8))
            }
            866 => {
                // WinSetPointer(HWND hwndDesktop, HPOINTER hptrNew)
                self.win_set_pointer(vcpu_id, read_stack(4), read_stack(8))
            }
            875 => {
                // WinSetWindowPos(HWND hwnd, HWND hwndInsertBehind, LONG x, LONG y, LONG cx, LONG cy, ULONG fl)
                let hwnd         = read_stack(4);
                let hwnd_behind  = read_stack(8);
                let (x, y, cx, cy) = bound_window_geometry(
                    read_stack(12) as i32, read_stack(16) as i32,
                    read_stack(20) as i32, read_stack(24) as i32);
                let fl = read_stack(28);
                debug!("  [VCPU {}] WinSetWindowPos hwnd={} behind={} x={} y={} cx={} cy={} fl=0x{:04X}",
                       vcpu_id, hwnd, hwnd_behind, x, y, cx, cy, fl);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                // pm_frame.rs; a state change asks the client first.
                if let Some((op, target)) = self.set_window_pos(hwnd, hwnd_behind, x, y, cx, cy, fl) {
                    return self.send_minmax(hwnd, op, target, 1);
                }
                ApiResult::Normal(1)
            }
            863 => {
                // WinSetMultWindowPos(HAB hab, PSWP pswp, ULONG cswp) (pm_frame.rs)
                self.win_set_mult_window_pos(vcpu_id, read_stack(8), read_stack(12))
            }
            877 => {
                // WinSetWindowText(HWND hwnd, PCSZ pszText)
                let hwnd = read_stack(4);
                let psz_text = read_stack(8);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                let text = self.read_guest_string_cp(psz_text, self.window_codepage(hwnd));
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let target = wm.text_window(hwnd);
                if let Some(win) = wm.get_window_mut(target) {
                    win.text = text;
                }
                wm.mark_dirty(hwnd); // VDR-B4: title bar text changed
                drop(wm);
                // The field of a combination box / spin button shows it.
                if target != hwnd {
                    self.set_field_text_done(target);
                }
                ApiResult::Normal(1)
            }
            878 => {
                // WinSetWindowULong(HWND hwnd, LONG index, ULONG ul)
                let hwnd = read_stack(4);
                let index = read_stack(8) as i32;
                let value = read_stack(12);
                // All bits of the word; QWL_STYLE is the style (pm_winmisc.rs).
                self.win_set_window_bits(vcpu_id, hwnd, index, value, u32::MAX)
            }
            874 => {
                // WinSetWindowBits(HWND hwnd, LONG index, ULONG flData, ULONG flMask) (pm_winmisc.rs)
                self.win_set_window_bits(vcpu_id, read_stack(4), read_stack(8) as i32, read_stack(12), read_stack(16))
            }
            833 => {
                // WinQueryVersion(HAB hab) (pm_winmisc.rs)
                self.win_query_version()
            }
            906 => {
                // WinCreateFrameControls(HWND hwndFrame, PFRAMECDATA pfcdata, PCSZ pszTitle) (pm_frame.rs)
                self.win_create_frame_controls(vcpu_id, read_stack(4), read_stack(8), read_stack(12))
            }
            704 => {
                // WinCalcFrameRect(HWND hwndFrame, PRECTL prcl, BOOL fClient) (pm_frame.rs)
                self.win_calc_frame_rect(vcpu_id, read_stack(4), read_stack(8), read_stack(12) != 0)
            }
            745 => {
                // WinFlashWindow(HWND hwnd, BOOL fFlash) (pm_frame.rs)
                self.win_flash_window(vcpu_id, read_stack(4), read_stack(8) != 0)
            }
            713 => {
                // WinCreateAccelTable(HAB hab, PACCELTABLE pAccelTable) (pm_accel.rs)
                self.win_create_accel_table(vcpu_id, read_stack(8))
            }
            723 => {
                // WinDestroyAccelTable(HACCEL haccel) (pm_accel.rs)
                self.win_destroy_accel_table(vcpu_id, read_stack(4))
            }
            709 => {
                // WinCopyAccelTable(HACCEL haccel, PACCELTABLE pAccelTable, ULONG cbCopyMax) (pm_accel.rs)
                self.win_copy_accel_table(vcpu_id, read_stack(4), read_stack(8), read_stack(12))
            }
            798 => {
                // WinQueryAccelTable(HAB hab, HWND hwnd) (pm_accel.rs)
                self.win_query_accel_table(vcpu_id, read_stack(8))
            }
            879 => {
                // WinSetWindowUShort(HWND hwnd, LONG index, USHORT us)
                let hwnd = read_stack(4);
                let index = read_stack(8) as i32;
                let value = read_stack(12) as u16;
                if !self.pm_check_window_word(vcpu_id, hwnd, index, 2) { return ApiResult::Normal(0); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    win.window_ushort.insert(index, value);
                }
                ApiResult::Normal(1)
            }
            904 => {
                // WinTranslateAccel(HAB hab, HWND hwnd, HACCEL haccel, PQMSG pqmsg):
                // a WM_CHAR in *pqmsg that is a key of table `haccel` becomes the
                // command it stands for, addressed to `hwnd`.
                let hwnd = read_stack(8);
                let haccel = read_stack(12);
                let pqmsg = read_stack(16);
                if pqmsg == 0 { return ApiResult::Normal(0); }
                if self.guest_read::<u32>(pqmsg + 4) != Some(WM_CHAR) { return ApiResult::Normal(0); }
                let mp1 = self.guest_read::<u32>(pqmsg + 8).unwrap_or(0);
                let mp2 = self.guest_read::<u32>(pqmsg + 12).unwrap_or(0);
                let entry = self.shared.window_mgr.lock_or_recover().accel_table(haccel)
                    .and_then(|t| t.iter().find(|e| super::pm_dialog::accel_matches(e, mp1, mp2)).cloned());
                let Some(entry) = entry else { return ApiResult::Normal(0) };
                let (msg, mp1, mp2) = super::pm_dialog::accel_message(&entry);
                self.guest_write::<u32>(pqmsg, hwnd);
                self.guest_write::<u32>(pqmsg + 4, msg);
                self.guest_write::<u32>(pqmsg + 8, mp1);
                self.guest_write::<u32>(pqmsg + 12, mp2);
                ApiResult::Normal(1)
            }
            892 => {
                // WinUpdateWindow(HWND hwnd)
                // Trigger a present buffer
                let hwnd = read_stack(4);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let frame_hwnd = wm.client_to_frame(hwnd);
                wm.mark_dirty(frame_hwnd);
                ApiResult::Normal(1)
            }
            899 => {
                // WinWindowFromID(HWND hwndParent, ULONG id)
                let hwnd_parent = read_stack(4);
                let id = read_stack(8);
                if !self.pm_check_hwnd(vcpu_id, hwnd_parent, false) { return ApiResult::Normal(0); }
                let wm = self.shared.window_mgr.lock_or_recover();
                let result = wm.find_child_by_id(hwnd_parent, id).unwrap_or(0);
                ApiResult::Normal(result)
            }
            909 => {
                // WinCreateWindow(hwndParent, pszClass, pszName, flStyle,
                //                 x, y, cx, cy, hwndOwner, hwndInsertBehind,
                //                 id, pCtlData, pPresParams)
                let hwnd_parent     = read_stack(4);
                let psz_class       = read_stack(8);
                let psz_name        = read_stack(12);
                let fl_style        = read_stack(16);
                let (x, y, cx, cy)  = bound_window_geometry(
                    read_stack(20) as i32, read_stack(24) as i32,
                    read_stack(28) as i32, read_stack(32) as i32);
                let hwnd_owner      = read_stack(36);
                // hwndInsertBehind (+40) — ignored for now
                let id              = read_stack(44);
                let p_ctl_data      = read_stack(48);
                let p_pres_params   = read_stack(52);
                if !self.pm_check_hwnd(vcpu_id, hwnd_parent, false)
                    || !self.pm_check_hwnd(vcpu_id, hwnd_owner, true) {
                    return ApiResult::Normal(0);
                }

                // MAKEINTATOM atoms have high word 0xFFFF; also accept bare
                // small integers (< 0x10000) for robustness.
                let class_name = if (psz_class & 0xFFFF_0000) == 0xFFFF_0000 || psz_class < 0x10000 {
                    resolve_class_atom(psz_class, String::new())
                } else {
                    resolve_class_atom(psz_class, self.read_guest_string(psz_class))
                };
                let text = if psz_name != 0 { self.read_guest_string_cp(psz_name, self.thread_codepage(vcpu_id)) } else { String::new() };
                debug!("  [VCPU {}] WinCreateWindow class='{}' text='{}' parent={} ({},{}) {}x{} id={} style=0x{:08X}",
                       vcpu_id, class_name, text, hwnd_parent, x, y, cx, cy, id, fl_style);

                let hwnd = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    let h = wm.create_window(class_name.clone(), hwnd_parent, hmq);
                    // CS_CLIPCHILDREN & co. are window styles too (pm_paint_style.rs).
                    let class_style = wm.class_window_style(&class_name);
                    if let Some(win) = wm.get_window_mut(h) {
                        win.text = text;
                        win.x = x; win.y = y;
                        win.cx = cx; win.cy = cy;
                        win.id = id;
                        win.owner = hwnd_owner;
                        win.style = fl_style | class_style;
                        win.visible = fl_style & WS_VISIBLE != 0;
                    }
                    h
                };
                // Presentation parameters, before the window first paints.
                self.apply_guest_presparams(hwnd, p_pres_params);
                if class_name == "WC_SCROLLBAR" {
                    self.scrollbar_init(hwnd, p_ctl_data);
                }
                // A slider's scales (SLDCDATA), a value set's rows and
                // columns (VSCDATA).
                self.init_slider_or_valueset(hwnd, p_ctl_data);
                // The parts of a combination box / spin button.
                self.build_compound_control(hwnd);
                // The client of a WC_FRAME window (pm_frame.rs).
                self.adopt_frame_client(hwnd);

                // A visible new window is invalid: it draws itself on creation.
                if fl_style & WS_VISIBLE != 0 {
                    self.shared.window_mgr.lock_or_recover().invalidate_window_only(hwnd);
                }
                ApiResult::Normal(hwnd)
            }
            775 => {
                // WinIsWindowVisible(HWND hwnd) -> BOOL (pm_window.rs)
                self.win_is_window_visible(vcpu_id, read_stack(4))
            }
            842 => {
                // WinQueryWindowTextLength(HWND hwnd) -> LONG
                self.win_query_window_text_length(vcpu_id, read_stack(4))
            }
            865 => {
                // WinSetParent(HWND hwnd, HWND hwndNewParent, BOOL fRedraw) -> BOOL
                self.win_set_parent(vcpu_id, read_stack(4), read_stack(8))
            }
            864 => {
                // WinSetOwner(HWND hwnd, HWND hwndNewOwner) -> BOOL
                self.win_set_owner(vcpu_id, read_stack(4), read_stack(8))
            }
            839 => {
                // WinQueryWindowPtr(HWND hwnd, LONG index) -> PVOID
                self.win_query_window_ptr(vcpu_id, read_stack(4), read_stack(8) as i32)
            }
            876 => {
                // WinSetWindowPtr(HWND hwnd, LONG index, PVOID p) -> BOOL
                self.win_set_window_ptr(vcpu_id, read_stack(4), read_stack(8) as i32, read_stack(12))
            }
            929 => {
                // WinSubclassWindow(HWND hwnd, PFNWP pfnwp) → PFNWP (old proc)
                //
                // Replace hwnd's window procedure with pfnwp and return the
                // previous one.  The caller is responsible for chaining.
                let hwnd   = read_stack(4);
                let pfn_wp = read_stack(8);
                debug!("  [VCPU {}] WinSubclassWindow hwnd={} pfn_wp=0x{:08X}", vcpu_id, hwnd, pfn_wp);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    let old = win.pfn_wp;
                    win.pfn_wp = pfn_wp;
                    return ApiResult::Normal(old);
                }
                ApiResult::Normal(0)
            }
            735 => {
                // WinEnableWindow(HWND hwnd, BOOL fEnable) -> BOOL
                //
                // Sets or clears the WS_DISABLED style bit and returns TRUE:
                // the PM Reference defines the result as "enabled state
                // successfully updated", not the previous state.
                let hwnd    = read_stack(4);
                let enable  = read_stack(8) != 0;
                debug!("  [VCPU {}] WinEnableWindow hwnd={} enable={}", vcpu_id, hwnd, enable);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }

                {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    if let Some(win) = wm.get_window_mut(hwnd) {
                        if enable {
                            win.style &= !WS_DISABLED;
                        } else {
                            win.style |= WS_DISABLED;
                        }
                    }
                }

                // Notify the window proc of the state change via WM_ENABLE.
                {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    if let Some(hmq) = wm.find_hmq_for_hwnd(hwnd)
                        && let Some(mq_arc) = wm.get_mq(hmq) {
                            let mut mq = mq_arc.lock_or_recover();
                            mq.messages.push_back(OS2Message {
                                hwnd,
                                msg: WM_ENABLE,
                                mp1: enable as u32,
                                mp2: 0,
                                time: 0, x: 0, y: 0,
                            });
                            mq.cond.notify_one();
                    }
                }
                ApiResult::Normal(1)
            }
            // ── Presentation parameters (pm_presparam.rs) ──
            938 => {
                // WinSetPresParam(HWND hwnd, ULONG id, ULONG cbParam, PVOID pbParam) -> BOOL
                self.win_set_pres_param(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16))
            }
            939 => {
                // WinQueryPresParam(HWND hwnd, ULONG id1, ULONG id2, PULONG pulId, ULONG cbBuf, PVOID pbBuf, ULONG fs) -> ULONG
                self.win_query_pres_param(vcpu_id, std::array::from_fn(|i| read_stack(4 + 4 * i as u64)))
            }
            940 => {
                // WinRemovePresParam(HWND hwnd, ULONG id) -> BOOL
                self.win_remove_pres_param(vcpu_id, read_stack(4), read_stack(8))
            }
            784 => {
                // WinLockWindowUpdate(HWND hwndDesktop, HWND hwndLockUpdate) -> BOOL (pm_paint.rs)
                self.win_lock_window_update(vcpu_id, read_stack(4), read_stack(8))
            }
            736 => {
                // WinEnableWindowUpdate(HWND hwnd, BOOL fEnable)
                // Controls whether the window redraws itself.  Stub — we have
                // no deferred-update queue, so just return TRUE.
                ApiResult::Normal(1)
            }
            773 => {
                // WinIsWindowEnabled(HWND hwnd) -> BOOL
                let hwnd = read_stack(4);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                let wm = self.shared.window_mgr.lock_or_recover();
                let enabled = wm.get_window(hwnd)
                    .map(|w| w.style & WS_DISABLED == 0)
                    .unwrap_or(false);
                ApiResult::Normal(enabled as u32)
            }
            837 => {
                // WinQueryWindowPos(HWND hwnd, PSWP pswp) -> BOOL
                //
                // Fills in the whole 36-byte SWP structure (<pmwin.h>) with
                // the window's current position and size.
                //
                //   LONG   fl;               +0  SWP_MOVE | SWP_SIZE, plus
                //                                SWP_MINIMIZE / SWP_MAXIMIZE and
                //                                SWP_ACTIVATE / SWP_DEACTIVATE
                //                                (`WindowManager::query_swp_flags`)
                //   LONG   cy;               +4  height
                //   LONG   cx;               +8  width
                //   LONG   y;                +12 y position
                //   LONG   x;                +16 x position
                //   HWND   hwndInsertBehind; +20 (always 0 here)
                //   HWND   hwnd;             +24 window handle
                //   ULONG  ulReserved1;      +28 0
                //   ULONG  ulReserved2;      +32 0
                let hwnd     = read_stack(4);
                let pswp_ptr = read_stack(8);
                if !self.pm_check_hwnd(vcpu_id, hwnd, false) { return ApiResult::Normal(0); }
                if pswp_ptr == 0 {
                    return ApiResult::Normal(0); // FALSE — null pointer
                }
                let wm = self.shared.window_mgr.lock_or_recover();
                let (x, y, cx, cy) = wm.get_window(hwnd)
                    .map(|w| (w.x, w.y, w.cx, w.cy))
                    .unwrap_or((0, 0, 0, 0));
                // fl: position and size, minimized / maximized, active.
                let fl = wm.query_swp_flags(hwnd);
                drop(wm);
                self.guest_write::<u32>(pswp_ptr,      fl);                  // fl
                self.guest_write::<i32>(pswp_ptr +  4, cy);                  // cy
                self.guest_write::<i32>(pswp_ptr +  8, cx);                  // cx
                self.guest_write::<i32>(pswp_ptr + 12, y);                   // y
                self.guest_write::<i32>(pswp_ptr + 16, x);                   // x
                self.guest_write::<u32>(pswp_ptr + 20, 0);                   // hwndInsertBehind
                self.guest_write::<u32>(pswp_ptr + 24, hwnd);                // hwnd
                self.guest_write::<u32>(pswp_ptr + 28, 0);                   // ulReserved1
                self.guest_write::<u32>(pswp_ptr + 32, 0);                   // ulReserved2
                ApiResult::Normal(1) // TRUE
            }
            913 => {
                // WinDrawText(HPS hps, LONG cchText, PCH lpchText, PRECTL prcl, LONG clrFore, LONG clrBack, ULONG flCmd)
                self.win_draw_text(vcpu_id, read_stack(4), read_stack(8) as i32, read_stack(12), read_stack(16),
                                   read_stack(20), read_stack(24), read_stack(28))
            }
            // ── Rectangle helpers (pm_rect.rs); the first argument is hab ──
            868 => {
                // WinSetRect(HAB hab, PRECTL prcl, LONG xLeft, LONG yBottom, LONG xRight, LONG yTop)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            869 => {
                // WinSetRectEmpty(HAB hab, PRECTL prcl)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            710 => {
                // WinCopyRect(HAB hab, PRECTL prclDst, PRECTL prclSrc)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            792 => {
                // WinOffsetRect(HAB hab, PRECTL prcl, LONG cx, LONG cy)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            762 => {
                // WinInflateRect(HAB hab, PRECTL prcl, LONG cx, LONG cy)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            797 => {
                // WinPtInRect(HAB hab, PRECTL prcl, PPOINTL pptl)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            764 => {
                // WinIntersectRect(HAB hab, PRECTL prclDst, PRECTL prclSrc1, PRECTL prclSrc2)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            891 => {
                // WinUnionRect(HAB hab, PRECTL prclDst, PRECTL prclSrc1, PRECTL prclSrc2)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            887 => {
                // WinSubtractRect(HAB hab, PRECTL prclDst, PRECTL prclSrc1, PRECTL prclSrc2)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            770 => {
                // WinIsRectEmpty(HAB hab, PRECTL prcl)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            741 => {
                // WinEqualRect(HAB hab, PRECTL prcl1, PRECTL prcl2)
                ApiResult::Normal(self.win_rect_call(ordinal, rect_args()))
            }
            // ── Coordinates and lookups (pm_query.rs) ──
            788 => {
                // WinMapWindowPoints(HWND hwndFrom, HWND hwndTo, PPOINTL aptl, LONG cwpt) -> BOOL
                self.win_map_window_points(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16) as i32)
            }
            900 => {
                // WinWindowFromPoint(HWND hwndParent, PPOINTL pptl, BOOL fChildren) -> HWND
                self.win_window_from_point(vcpu_id, read_stack(4), read_stack(8), read_stack(12) != 0)
            }
            813 => {
                // WinQueryDesktopWindow(HAB hab, HDC hdc) -> HWND
                self.win_query_desktop_window(vcpu_id, read_stack(8))
            }
            800 => {
                // WinQueryAnchorBlock(HWND hwnd) -> HAB
                self.win_query_anchor_block(vcpu_id, read_stack(4))
            }
            768 => {
                // WinIsChild(HWND hwnd, HWND hwndParent) -> BOOL
                self.win_is_child(vcpu_id, read_stack(4), read_stack(8))
            }
            838 => {
                // WinQueryWindowProcess(HWND hwnd, PPID ppid, PTID ptid) -> BOOL
                self.win_query_window_process(vcpu_id, read_stack(4), read_stack(8), read_stack(12))
            }
            917 => {
                // WinMultWindowFromIDs(HWND hwndParent, PHWND ahwnd, ULONG idFirst, ULONG idLast) -> LONG
                self.win_mult_window_from_ids(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16))
            }
            740 => {
                // WinEnumDlgItem(HWND hwndDlg, HWND hwnd, ULONG code) -> HWND
                self.win_enum_dlg_item(vcpu_id, read_stack(4), read_stack(8), read_stack(12))
            }
            805 => {
                // WinQueryClassName(HWND hwnd, LONG cchMax, PCH pch) -> LONG
                self.win_query_class_name(vcpu_id, read_stack(4), read_stack(8) as i32, read_stack(12))
            }
            925 => {
                // WinQueryClassInfo(HAB hab, PCSZ pszClassName, PCLASSINFO pClassInfo) -> BOOL
                self.win_query_class_info(vcpu_id, read_stack(8), read_stack(12))
            }
            // ── Input state (pm_keystate.rs) ──
            752 => {
                // WinGetKeyState(HWND hwndDesktop, LONG vkey) -> LONG
                self.win_get_key_state(vcpu_id, read_stack(4), read_stack(8))
            }
            758 => {
                // WinGetPhysKeyState(HWND hwndDesktop, LONG sc) -> LONG
                self.win_get_phys_key_state(vcpu_id, read_stack(4), read_stack(8))
            }
            818 => {
                // WinQueryMsgPos(HAB hab, PPOINTL pptl) -> BOOL
                self.win_query_msg_pos(vcpu_id, read_stack(8))
            }
            819 => {
                // WinQueryMsgTime(HAB hab) -> ULONG
                self.win_query_msg_time(vcpu_id)
            }
            823 => {
                // WinQueryPointerPos(HWND hwndDesktop, PPOINTL pptl) -> BOOL
                self.win_query_pointer_pos(vcpu_id, read_stack(4), read_stack(8))
            }
            867 => {
                // WinSetPointerPos(HWND hwndDesktop, LONG x, LONG y) -> BOOL
                self.win_set_pointer_pos(vcpu_id, read_stack(4), read_stack(8) as i32, read_stack(12) as i32)
            }
            // ── System colours (pm_syscolor.rs) ──
            826 => {
                // WinQuerySysColor(HWND hwndDesktop, LONG clr, LONG lReserved) -> LONG
                self.win_query_sys_color(vcpu_id, read_stack(4), read_stack(8) as i32, read_stack(12))
            }
            871 => {
                // WinSetSysColors(HWND hwndDesktop, ULONG flOptions, ULONG flFormat, LONG clrFirst, ULONG cColorData, PLONG pColorData) -> BOOL
                self.win_set_sys_colors(vcpu_id, std::array::from_fn(|i| read_stack(4 + 4 * i as u64)))
            }
            // ── Atom tables (pm_atom.rs) ──
            714 => {
                // WinCreateAtomTable(ULONG cbInitial, ULONG cBuckets) -> HATOMTBL
                self.win_create_atom_table(vcpu_id)
            }
            724 => {
                // WinDestroyAtomTable(HATOMTBL hAtomTbl) -> HATOMTBL
                self.win_destroy_atom_table(vcpu_id, read_stack(4))
            }
            830 => {
                // WinQuerySystemAtomTable() -> HATOMTBL
                self.win_query_system_atom_table()
            }
            700 => {
                // WinAddAtom(HATOMTBL hAtomTbl, PCSZ pszAtomName) -> ATOM
                self.win_add_atom(vcpu_id, read_stack(4), read_stack(8))
            }
            744 => {
                // WinFindAtom(HATOMTBL hAtomTbl, PCSZ pszAtomName) -> ATOM
                self.win_find_atom(vcpu_id, read_stack(4), read_stack(8))
            }
            721 => {
                // WinDeleteAtom(HATOMTBL hAtomTbl, ATOM atom) -> ATOM
                self.win_delete_atom(vcpu_id, read_stack(4), read_stack(8))
            }
            801 => {
                // WinQueryAtomLength(HATOMTBL hAtomTbl, ATOM atom) -> ULONG
                self.win_query_atom_length(vcpu_id, read_stack(4), read_stack(8))
            }
            802 => {
                // WinQueryAtomName(HATOMTBL hAtomTbl, ATOM atom, PSZ pchBuffer, ULONG cchBufferMax) -> ULONG
                self.win_query_atom_name(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16))
            }
            803 => {
                // WinQueryAtomUsage(HATOMTBL hAtomTbl, ATOM atom) -> ULONG
                self.win_query_atom_usage(vcpu_id, read_stack(4), read_stack(8))
            }
            // ── Dynamic Data Exchange (pm_dde.rs) ──
            718 => {
                // WinDdeInitiate(HWND hwndClient, PCSZ pszAppName, PCSZ pszTopicName, PCONVCONTEXT pcctxt) -> BOOL
                self.win_dde_initiate(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16))
            }
            720 => {
                // WinDdeRespond(HWND hwndClient, HWND hwndServer, PCSZ pszAppName, PCSZ pszTopicName, PCONVCONTEXT pcctxt) -> MRESULT
                self.win_dde_respond(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16),
                                     read_stack(20))
            }
            719 => {
                // WinDdePostMsg(HWND hwndTo, HWND hwndFrom, ULONG wm, PDDESTRUCT pddest, ULONG flOptions) -> BOOL
                self.win_dde_post_msg(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(16),
                                      read_stack(20))
            }
            _ => {
                warn!("Warning: Unknown PMWIN Ordinal {} on VCPU {}", ordinal, vcpu_id);
                ApiResult::Normal(0)
            }
        }
    }

    // ── Binary resource parser helpers ───────────────────────────────────────────

    /// Read a NUL-terminated guest string starting at `base + *offset`, advancing
    /// `*offset` past the terminating NUL, and decode it with `codepage`.
    /// Stops at `base + max_size`.
    fn read_nul_str_at(&self, base: u32, offset: &mut u32, max_size: u32, codepage: u32) -> String {
        let mut bytes = Vec::new();
        loop {
            if *offset >= max_size { break; }
            let b = self.guest_read::<u8>(base + *offset).unwrap_or(0);
            *offset += 1;
            if b == 0 { break; }
            bytes.push(b);
        }
        super::codepage::cp_decode(&bytes, codepage)
    }

    /// Parse exactly `count` MENUTEMPLATE items from the flat byte stream.
    ///
    /// Each item is: u16 afStyle + u16 afAttribute + u16 id + NUL-terminated
    /// text (absent when `MIS_SEPARATOR` is set).  SUBMENU items are followed
    /// by a 10-byte child-block header (u32 total_size, u16 codepage, u16
    /// reserved, u16 child_count) then the children recursively.
    ///
    /// When `count == 0` the function falls back to the MIS_END sentinel
    /// (bit 15 of afStyle) for compatibility with hand-built templates that
    /// omit the count field.
    fn parse_menu_items(&self, base: u32, size: u32, offset: &mut u32, count: usize, codepage: u32) -> Vec<MenuItem> {
        use super::constants::{MIS_END, MIS_SEPARATOR, MIS_SUBMENU};
        let use_count = count > 0;
        let mut items = Vec::new();
        let mut remaining = count;
        loop {
            if use_count && remaining == 0 { break; }
            if *offset + 6 > size { break; }
            let raw_style = self.guest_read::<u16>(base + *offset).unwrap_or(0);
            let attr      = self.guest_read::<u16>(base + *offset + 2).unwrap_or(0);
            let id        = self.guest_read::<u16>(base + *offset + 4).unwrap_or(0);
            *offset += 6;

            let is_last = !use_count && (raw_style & MIS_END) != 0;
            let style   = raw_style & !MIS_END;

            // Text is present unless the item is a pure separator.
            let text = if style & MIS_SEPARATOR == 0 {
                self.read_nul_str_at(base, offset, size, codepage)
            } else {
                String::new()
            };

            // Recurse into submenu: wrc prepends a 10-byte child-block header
            // (u32 total_size + u16 codepage + u16 reserved + u16 child_count).
            let children = if style & MIS_SUBMENU != 0 {
                if *offset + 10 <= size {
                    let child_count = self.guest_read::<u16>(base + *offset + 8).unwrap_or(0) as usize;
                    *offset += 10;
                    self.parse_menu_items(base, size, offset, child_count, codepage)
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            items.push(MenuItem { id, style, attr, text, children, image: None });
            if use_count { remaining -= 1; }
            if is_last { break; }
        }
        items
    }

    /// Parse a MENUTEMPLATE binary resource at `guest_addr` (size `size`);
    /// item texts are decoded with the template's codepage.
    ///
    /// The wrc (Open Watcom) OS/2 MENUTEMPLATE header is 10 bytes:
    ///   u32 total_size, u16 codepage, u16 reserved, u16 item_count.
    /// Items start at byte 10.  A zero item_count falls back to MIS_END
    /// sentinel parsing for hand-built inline templates.
    ///
    /// Returns the top-level item list, or an empty `Vec` on parse failure.
    fn parse_menu_template(&self, guest_addr: u32, size: u32) -> Vec<MenuItem> {
        if size < 10 { return Vec::new(); }
        // Header: u32 total_size, u16 codepage, u16 reserved, u16 item_count
        let item_count = self.guest_read::<u16>(guest_addr + 8).unwrap_or(0) as usize;
        // Item texts are in the template's codepage; 0 means the process codepage.
        let codepage = match self.guest_read::<u16>(guest_addr + 4).unwrap_or(0) {
            0 => self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed),
            cp => cp as u32,
        };
        let mut offset = 10u32;
        self.parse_menu_items(guest_addr, size, &mut offset, item_count, codepage)
    }

    /// Parse the RT_MENU resource `id_menu` of module `hmod` (0 = the
    /// executable) into a menu item tree; `None` when there is none.
    pub(super) fn load_menu_resource(&self, hmod: u32, id_menu: u32, vcpu_id: u32) -> Option<Vec<MenuItem>> {
        let res = self.shared.resource_mgr.lock_or_recover().find(hmod, RT_MENU, id_menu as u16);
        match res {
            Some((guest_addr, size)) => {
                let parsed = self.parse_menu_template(guest_addr, size);
                debug!("  [VCPU {}] RT_MENU id={}: parsed {} top-level items",
                       vcpu_id, id_menu, parsed.len());
                Some(parsed)
            }
            None => {
                debug!("  [VCPU {}] no RT_MENU resource for id={}", vcpu_id, id_menu);
                None
            }
        }
    }

    /// The process codepage (`DosSetProcessCp`).
    pub(crate) fn process_codepage(&self) -> u32 {
        self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Codepage of PM text belonging to window `hwnd`: its message queue's
    /// (`WinSetCp`), else the process codepage.
    pub(crate) fn window_codepage(&self, hwnd: u32) -> u32 {
        self.shared.window_mgr.lock_or_recover().queue_codepage(hwnd)
            .unwrap_or_else(|| self.process_codepage())
    }

    /// WinQuerySysValue: the fixed values of [`sys_value`] plus the ones
    /// `WinSetSysValue` can change (`SV_CURSORRATE`, milliseconds).
    pub(crate) fn query_sys_value(&self, index: i32) -> i32 {
        match index {
            SV_CURSORRATE => self.shared.window_mgr.lock_or_recover().cursor_rate_ms as i32,
            _ => sys_value(index),
        }
    }

    /// WinSetSysValue: set system value `index` for the desktop `hwnd`.
    /// Warpine keeps only `SV_CURSORRATE` (the caret blink interval, a
    /// positive number of milliseconds); any other index, or a rate below 1,
    /// fails with `PMERR_PARAMETER_OUT_OF_RANGE` (TODO(unverified): which
    /// rates PM accepts; Warpine's choice for the values it does not keep).
    /// A handle other than `HWND_DESKTOP` fails with `PMERR_INVALID_HWND`.
    pub(crate) fn set_sys_value(&self, tid: u32, hwnd: u32, index: i32, value: i32) -> ApiResult {
        if hwnd != HWND_DESKTOP {
            return self.pm_fail(tid, PMERR_INVALID_HWND);
        }
        match index {
            SV_CURSORRATE if value > 0 => {
                self.shared.window_mgr.lock_or_recover().cursor_rate_ms = value as u32;
                ApiResult::Normal(1)
            }
            _ => self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE),
        }
    }

    /// Codepage of PM text passed by thread `vcpu_id` where no window says
    /// otherwise (a window being created, a message box): its message
    /// queue's, else the process codepage.
    pub(crate) fn thread_codepage(&self, vcpu_id: u32) -> u32 {
        let wm = self.shared.window_mgr.lock_or_recover();
        wm.tid_to_hmq.get(&vcpu_id).and_then(|&hmq| wm.get_mq(hmq))
            .map(|mq| mq.lock_or_recover().codepage)
            .unwrap_or_else(|| self.process_codepage())
    }

    /// WinSetCp: set the codepage of message queue `hmq`. Fails for a
    /// codepage warpine cannot convert or an unknown queue, changing nothing.
    ///
    /// The value is per queue and independent of the process codepage
    /// (`DosSetProcessCp`), as in PM: text of the queue's windows (window
    /// text, list items, typed characters) and of presentation spaces created
    /// afterwards for them is converted with it.
    fn win_set_cp(&self, hmq: u32, cp: u32) -> bool {
        if !super::stubs::is_pm_codepage(cp) { return false; }
        let Some(mq) = self.shared.window_mgr.lock_or_recover().get_mq(hmq) else { return false };
        mq.lock_or_recover().codepage = cp;
        true
    }

    /// WinQueryCp: codepage of message queue `hmq`, or 0 for an unknown queue.
    fn win_query_cp(&self, hmq: u32) -> u32 {
        let Some(mq) = self.shared.window_mgr.lock_or_recover().get_mq(hmq) else { return 0 };
        mq.lock_or_recover().codepage
    }

    /// Attach `hwnd_menu` as the menu bar of `hwnd_frame` and return the menu
    /// it replaces. The client area shrinks (or grows) to make room for the
    /// bar; when its size changes the FrameBuffer is resized and the client is
    /// told via WM_SIZE + WM_PAINT so it redraws at the new size.
    /// Create a menu window (`WinLoadMenu` / `WinCreateMenu`) with `items`:
    /// parent and owner `parent`, identity `FID_MENU`. It is not attached to
    /// a frame here; see [`Self::update_frame_menu`].
    fn create_menu_window(&self, parent: u32, items: Vec<super::pm_types::MenuItem>) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let h = wm.create_window("#Menu".to_string(), parent, 0);
        if let Some(win) = wm.get_window_mut(h) {
            win.menu_items = items;
            win.id = FID_MENU;
            win.owner = parent;
        }
        h
    }

    /// Minimize, maximize or restore `frame` (`op`) to `target`, prepared by
    /// `WindowManager::begin_minmax`: `WM_MINMAXFRAME` goes to its client
    /// first, which can stop the change by returning TRUE. A client without
    /// a window procedure is not asked. The API returns `ret`.
    pub(super) fn send_minmax(&self, frame: u32, op: u32, target: super::pm_minmax::WinRect, ret: u32) -> ApiResult {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let client = wm.frame_to_client.get(&frame).copied().unwrap_or(0);
        let wnd_proc = wm.get_window(client).map_or(0, |w| w.pfn_wp);
        if wnd_proc != 0 {
            return ApiResult::MinMaxFrame { wnd_proc, client, frame, op, target, ret };
        }
        wm.finish_minmax(frame, op, target);
        ApiResult::Normal(ret)
    }

    /// WM_UPDATEFRAME(FCF_MENU) on `frame`: its menu bar becomes its
    /// topmost `FID_MENU` child menu, or none without one, and the frame
    /// is laid out again.
    fn update_frame_menu(&self, frame: u32) {
        let menu = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.get_window(frame).and_then(|f| f.children.iter().rev().copied().find(|&c| {
                wm.get_window(c).is_some_and(|w| w.id == FID_MENU && w.class_name == "#Menu")
            })).unwrap_or(0)
        };
        self.attach_menu_to_frame(frame, menu);
    }

    pub(super) fn attach_menu_to_frame(&self, hwnd_frame: u32, hwnd_menu: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let client = wm.frame_to_client.get(&hwnd_frame).copied();
        let old_size = client.and_then(|c| wm.get_window(c)).map(|w| (w.cx, w.cy));
        let prev = wm.attach_menu(hwnd_frame, hwnd_menu);
        let new_size = client.and_then(|c| wm.get_window(c)).map(|w| (w.cx, w.cy));
        if let (Some(client), Some(old), Some(new)) = (client, old_size, new_size)
            && old != new
        {
            // The surface is the frame below its title and menu bars.
            let surface = wm.layout_frame(hwnd_frame);
            wm.resize_surface(hwnd_frame, surface.0, surface.1);
            wm.post_size(client, old, new);
        }
        prev
    }

    /// WinGetMsg / WinPeekMsg: take the first message of the calling
    /// thread's queue that passes the filters (`hwnd_filter`: that window or
    /// one of its descendants, 0 = any; `first`..`last`: a message range, see
    /// [`msg_in_range`]), write it to `pqmsg` as a QMSG with any accelerator
    /// keystroke translated, and return its message number. The message is
    /// left in the queue unless `fl` is `PM_REMOVE`. With `wait`, block
    /// until one arrives; `None` when there is none (or on shutdown).
    /// Without `translate` the message is written as it was queued (the
    /// input hooks see it first and the accelerator is translated after
    /// them, `pm_hook.rs`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn next_message(&self, vcpu_id: u32, pqmsg: u32, hwnd_filter: u32, first: u32, last: u32,
                    fl: u32, wait: bool, translate: bool) -> Option<u32> {
        let (hmq, mq_arc) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
            (hmq, wm.get_mq(hmq)?)
        };
        let (cond, wait_lock) = {
            let mq = mq_arc.lock_or_recover();
            (Arc::clone(&mq.cond), Arc::clone(&mq.lock))
        };
        let remove = fl & PM_REMOVE != 0;
        loop {
            if self.stop_or_restart() { return None; }
            // Window-manager lock before the queue lock, as everywhere else.
            let found = self.shared.window_mgr.lock_or_recover().next_queued(hmq, remove, |wm, m| {
                // WM_QUIT passes every filter, so a filtered loop still ends.
                m.msg == WM_QUIT || (msg_in_range(m.msg, first, last)
                    && (hwnd_filter == 0 || wm.is_same_or_descendant(m.hwnd, hwnd_filter)))
            });
            if let Some(mut msg) = found {
                // Keystrokes that are accelerators arrive as the command
                // they stand for.
                if translate { self.translate_queued_accelerator(&mut msg); }
                if pqmsg != 0 {
                    self.guest_write::<u32>(pqmsg, msg.hwnd);
                    self.guest_write::<u32>(pqmsg + 4, msg.msg);
                    self.guest_write::<u32>(pqmsg + 8, msg.mp1);
                    self.guest_write::<u32>(pqmsg + 12, msg.mp2);
                    self.guest_write::<u32>(pqmsg + 16, msg.time);
                    // ptl is a POINTL (two LONGs), then ulReserved.
                    self.guest_write::<i32>(pqmsg + 20, msg.x);
                    self.guest_write::<i32>(pqmsg + 24, msg.y);
                    self.guest_write::<u32>(pqmsg + 28, 0);
                }
                return Some(msg.msg);
            }
            if !wait { return None; }
            // Block on the queue's condvar instead of spinning.
            let guard = wait_lock.lock_or_recover();
            let _ = cond.wait_timeout(guard, std::time::Duration::from_millis(100)).unwrap();
        }
    }

    /// Load accelerator table `id` (RT_ACCELTABLE) of module `hmod` and
    /// return its handle, or `None` without such a resource.
    pub(super) fn load_accel_table(&self, hmod: u32, id: u32) -> Option<u32> {
        let (addr, size) = self.shared.resource_mgr.lock_or_recover().find(hmod, RT_ACCELTABLE, id as u16)?;
        let data = self.guest_read_bytes(addr, size as usize)?;
        let entries = super::pm_dialog::parse_accel_table(&data);
        debug!("  WinLoadAccelTable id={}: {} entries", id, entries.len());
        // ACCELTABLE.codepage follows cAccel.
        let codepage = data.get(2..4).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
        Some(self.shared.window_mgr.lock_or_recover().create_accel_table(entries, codepage))
    }

    /// WinDlgBox / WinLoadDlg: create dialog `id_dlg` (RT_DIALOG) of module
    /// `hmod` and queue WM_INITDLG (mp1 = the control given the focus, mp2 =
    /// `p_create_params`) and WM_PAINT for it. Without a usable template the
    /// dialog is an empty window. Returns `(hwnd_dlg, hmq)`.
    #[allow(clippy::too_many_arguments)]
    fn load_dialog(&self, vcpu_id: u32, hwnd_parent: u32, hwnd_owner: u32, pfn_dlg_proc: u32,
                   hmod: u32, id_dlg: u32, p_create_params: u32) -> (u32, u32) {
        debug!("  [VCPU {}] load dialog: parent={} owner={} idDlg={} pfnDlgProc=0x{:08X}",
               vcpu_id, hwnd_parent, hwnd_owner, id_dlg, pfn_dlg_proc);
        let codepage = self.thread_codepage(vcpu_id);
        let res = self.shared.resource_mgr.lock_or_recover().find(hmod, RT_DIALOG, id_dlg as u16);
        let template = res
            .and_then(|(addr, size)| self.guest_read_bytes(addr, (size as usize).min(MAX_DLG_TEMPLATE)))
            .and_then(|data| super::pm_dialog::parse_dlg_template(&data, codepage));
        if template.is_none() {
            warn!("  [VCPU {}] dialog {}: no usable RT_DIALOG resource", vcpu_id, id_dlg);
        }
        let (hwnd_dlg, hmq, focus) = self.create_dialog(
            vcpu_id, hwnd_parent, hwnd_owner, pfn_dlg_proc, template);
        self.queue_dialog_start(hwnd_dlg, hmq, focus, p_create_params);
        (hwnd_dlg, hmq)
    }

    /// Queue a new dialog's first messages: WM_INITDLG (mp1 = `focus`, mp2
    /// = `p_create_params`), WM_PAINT, then the focus notification of the
    /// control given the focus (`EN_SETFOCUS`...) to its owner.
    pub(super) fn queue_dialog_start(&self, hwnd_dlg: u32, hmq: u32, focus: u32, p_create_params: u32) {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if let Some(mq_arc) = wm.get_mq(hmq) {
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(OS2Message {
                hwnd: hwnd_dlg, msg: WM_INITDLG, mp1: focus, mp2: p_create_params, time: 0, x: 0, y: 0,
            });
            mq.cond.notify_one();
        }
        wm.invalidate_window_only(hwnd_dlg);
        wm.notify_focus(focus, true);
    }

    /// Create a dialog window and its controls from `template` (an empty
    /// dialog when `None`), make it the active window and give the focus to
    /// its first control. Returns `(hwnd_dlg, hmq, focus control or 0)`.
    pub(super) fn create_dialog(&self, vcpu_id: u32, hwnd_parent: u32, hwnd_owner: u32, pfn_dlg_proc: u32,
                     template: Option<super::pm_dialog::DlgTemplate>) -> (u32, u32, u32) {
        use super::constants::{dlg_units_to_px_x as px_x, dlg_units_to_px_y as px_y};
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
        // Template coordinates are relative to the dialog's parent: the
        // desktop for HWND_DESKTOP, otherwise the parent window's frame.
        let frame = wm.find_frame_for_hwnd(hwnd_parent);
        let (origin_x, origin_y) = if wm.is_frame(frame) {
            wm.get_window(frame).map_or((0, 0), |w| (w.x, w.y))
        } else {
            (0, 0)
        };
        let h = wm.create_window(super::pm_types::WindowManager::DIALOG_CLASS.to_string(), hwnd_parent, hmq);
        let mut controls = Vec::new();
        if let Some(win) = wm.get_window_mut(h) {
            win.pfn_wp = pfn_dlg_proc;
            win.owner = hwnd_owner;
            win.visible = true; // dialogs are shown when created
            if let Some(t) = &template {
                // The compositor draws the chrome the template asks for.
                win.frame_flags = t.frame_flags;
                win.style = t.dialog.style;
                win.text = t.dialog.text.clone();
                win.id = t.dialog.id as u32;
                win.x = origin_x + px_x(t.dialog.x);
                win.y = origin_y + px_y(t.dialog.y);
                win.cx = px_x(t.dialog.cx);
                win.cy = px_y(t.dialog.cy);
                for (id, v) in &t.dialog.pres_params { win.pres_params.set(*id, v); }
            }
        }
        if let Some(t) = &template {
            for item in &t.items {
                let parent = item.parent.map_or(h, |i| controls[i]);
                let ch = wm.create_window(item.class_name.clone(), parent, hmq);
                if let Some(cwin) = wm.get_window_mut(ch) {
                    cwin.text = item.text.clone();
                    cwin.x = px_x(item.x);
                    cwin.y = px_y(item.y);
                    cwin.cx = px_x(item.cx);
                    cwin.cy = px_y(item.cy);
                    cwin.id = item.id as u32;
                    cwin.style = item.style;
                    cwin.visible = item.style & WS_VISIBLE != 0;
                    // Controls notify the dialog.
                    cwin.owner = h;
                    for (id, v) in &item.pres_params { cwin.pres_params.set(*id, v); }
                }
                // The parts of a combination box / spin button, the state
                // of a slider / value set (from the item's control data).
                wm.build_combo_or_spin(ch);
                wm.build_slider_or_valueset(ch, &item.ctl_data);
                controls.push(ch);
            }
        }
        wm.z_push_top(h);
        // A new dialog is the active window; z_remove hands activation back
        // to the window underneath when it is dismissed.
        wm.focused_hwnd = h;
        let preferred = template.as_ref().and_then(|t| t.focus).map(|i| controls[i]);
        let focus = wm.initial_dialog_focus(h, preferred).unwrap_or(0);
        if focus != 0 {
            wm.focus_hwnd = focus;
        }
        // The compositor reads position and size from the window; the
        // surface is allocated on the first draw (sized from cx × cy).
        (h, hmq, focus)
    }
}

/// Whether message `msg` passes the `msgFirst` / `msgLast` filter of
/// WinGetMsg / WinPeekMsg: both 0 = every message; `first <= last` = that
/// range; `first > last` = every message except those strictly between
/// `last` and `first` (TODO(unverified): the exact bounds of the exclusion).
fn msg_in_range(msg: u32, first: u32, last: u32) -> bool {
    match (first, last) {
        (0, 0) => true,
        (f, l) if f <= l => (f..=l).contains(&msg),
        (f, l) => msg <= l || msg >= f,
    }
}

/// WinQuerySysValue: the system value `index` (`SV_*`), describing the
/// desktop and the chrome Warpine draws; 0 for values it does not keep.
pub(super) fn sys_value(index: i32) -> i32 {
    let (dw, dh) = desktop_size();
    match index {
        SV_CXSCREEN | SV_CXFULLSCREEN => dw as i32,
        SV_CYSCREEN => dh as i32,
        // The full-screen client height leaves out the title bar.
        SV_CYFULLSCREEN => dh as i32 - CHROME_TITLE_H,
        SV_CYTITLEBAR => CHROME_TITLE_H,
        SV_CYMENU => MENU_BAR_HEIGHT as i32,
        // Frame scroll bars (pm_frame.rs).
        SV_CXVSCROLL | SV_CYHSCROLL => super::pm_frame::FRAME_SCROLLBAR_W,
        SV_CXSIZEBORDER | SV_CYSIZEBORDER | SV_CXDLGFRAME | SV_CYDLGFRAME => CHROME_BORDER_W,
        SV_CXBORDER | SV_CYBORDER => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Loader, ApiResult};
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::mutex_ext::MutexExt;
    use std::sync::atomic::Ordering;

    /// WinFillRect on a memory presentation space fills the bitmap selected
    /// into it (it used to look for the PS's window and draw nothing).
    #[test]
    fn test_win_fill_rect_fills_the_bitmap_of_a_memory_ps() {
        use crate::gui::surface::Surface;
        /// `CLR_RED` (`<pmgpi.h>`).
        const CLR_RED: u32 = 2;
        const ORD_WIN_FILL_RECT: u32 = 743;
        let loader = Loader::new_mock();
        let (hps, hbm) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hbm = wm.create_bitmap(Surface::new(8, 8), 24, 1);
            let hps = wm.create_ps(0);
            wm.ps_map.get_mut(&hps).unwrap().bitmap = hbm;
            (hps, hbm)
        };
        let prcl = 0x0008_0000;
        for (i, v) in [1i32, 1, 4, 4].into_iter().enumerate() {
            loader.guest_write::<i32>(prcl + 4 * i as u32, v).unwrap();
        }
        let mut vcpu = MockVcpu::new();
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_FILL_RECT, &[hps, prcl, CLR_RED]), 1);
        let wm = loader.shared.window_mgr.lock_or_recover();
        let s = wm.bitmaps[&hbm].surface.lock_or_recover();
        assert_eq!(s.pel(1, 1), Some(0xFF_0000));
        assert_eq!(s.pel(3, 3), Some(0xFF_0000));
        assert_ne!(s.pel(4, 4), Some(0xFF_0000), "RECTL exclusive at the top and right");
        drop(s);
        drop(wm);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_FILL_RECT, &[0xDEAD, prcl, 0]), 0, "no such PS");
    }

    #[test]
    fn test_sys_values_match_the_desktop_and_chrome() {
        use super::super::constants::*;
        let (dw, dh) = desktop_size();
        assert_eq!(super::sys_value(SV_CXSCREEN), dw as i32);
        assert_eq!(super::sys_value(SV_CYSCREEN), dh as i32);
        assert_eq!(super::sys_value(SV_CXFULLSCREEN), dw as i32);
        assert_eq!(super::sys_value(SV_CYFULLSCREEN), dh as i32 - CHROME_TITLE_H);
        assert_eq!(super::sys_value(SV_CYTITLEBAR), CHROME_TITLE_H);
        assert_eq!(super::sys_value(SV_CYMENU), MENU_BAR_HEIGHT as i32);
        assert_eq!(super::sys_value(SV_CXSIZEBORDER), CHROME_BORDER_W);
        assert_eq!(super::sys_value(SV_CYDLGFRAME), CHROME_BORDER_W);
        assert_eq!(super::sys_value(SV_CXBORDER), 1);
        assert_eq!(super::sys_value(-1), 0, "unknown index");
    }

    #[test]
    fn test_window_geometry_is_bounded() {
        use super::super::constants::MAX_WINDOW_COORD as M;
        assert_eq!(super::bound_window_geometry(10, -20, 300, 200), (10, -20, 300, 200));
        assert_eq!(super::bound_window_geometry(i32::MAX, i32::MIN, i32::MAX, -5), (M, -M, M, 0));
        // x + cx stays far from overflow.
        let (x, _, cx, _) = super::bound_window_geometry(i32::MAX, 0, i32::MAX, 0);
        assert!(x.checked_add(cx).is_some());
    }

    fn write_stack(loader: &Loader, esp: u32, args: &[u32]) {
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
    }

    /// WinLoadMenu (778) with a wrc-format RT_MENU binary resource must parse
    /// a two-item top-level menu and store the items in the menu window.
    ///
    /// The wrc OS/2 MENUTEMPLATE header is 10 bytes:
    ///   u32 total_size, u16 codepage, u16 reserved, u16 item_count.
    #[test]
    fn test_win_load_menu_parses_items() {
        let loader = Loader::new_mock();

        // Build a wrc-format MENUTEMPLATE in guest RAM at 0x5000.
        // Header: u32 total_size=0x20, u16 codepage=850, u16 reserved=4, u16 count=2
        // Item 1: style=MIS_TEXT(0x0001), attr=0, id=100, text="File\0"
        // Item 2: style=MIS_TEXT(0x0001), attr=0, id=200, text="Help\0"
        let base: u32 = 0x5000;
        // total_size = 10 header + 11 + 11 = 32 = 0x20
        let data: &[u8] = &[
            0x20, 0x00, 0x00, 0x00, // u32 total_size = 32
            0x52, 0x03,             // u16 codepage = 850
            0x04, 0x00,             // u16 reserved = 4
            0x02, 0x00,             // u16 item_count = 2
            // Item 1: style=0x0001, attr=0x0000, id=100, text="File\0"
            0x01, 0x00, 0x00, 0x00, 0x64, 0x00, b'F', b'i', b'l', b'e', 0x00,
            // Item 2: style=0x0001, attr=0, id=200, text="Help\0"
            0x01, 0x00, 0x00, 0x00, 0xC8, 0x00, b'H', b'e', b'l', b'p', 0x00,
        ];
        for (i, &b) in data.iter().enumerate() {
            loader.guest_write::<u8>(base + i as u32, b).unwrap();
        }

        let items = loader.parse_menu_template(base, data.len() as u32);
        assert_eq!(items.len(), 2, "Expected 2 top-level items");
        assert_eq!(items[0].id, 100);
        assert_eq!(items[0].text, "File");
        assert_eq!(items[1].id, 200);
        assert_eq!(items[1].text, "Help");
    }

    /// Menu item texts are decoded with the template's codepage (the process
    /// codepage when the template gives 0), not byte by byte as Latin-1.
    #[test]
    fn test_menu_template_texts_use_the_template_codepage() {
        let loader = Loader::new_mock();
        let base: u32 = 0x5000;
        let menu = |cp: u16| -> Vec<u8> {
            let mut d = vec![0x13, 0x00, 0x00, 0x00, cp as u8, (cp >> 8) as u8, 0x04, 0x00, 0x01, 0x00];
            // One item: MIS_TEXT, id 1, text 0x82 0xA0 (CP932 'あ', CP850 'éá').
            d.extend_from_slice(&[0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x82, 0xA0, 0x00]);
            d
        };
        let parse = |data: &[u8]| {
            for (i, &b) in data.iter().enumerate() {
                loader.guest_write::<u8>(base + i as u32, b).unwrap();
            }
            loader.parse_menu_template(base, data.len() as u32)[0].text.clone()
        };
        assert_eq!(parse(&menu(932)), "あ");
        assert_eq!(parse(&menu(850)), "éá");
        loader.shared.active_codepage.store(932, Ordering::Relaxed);
        assert_eq!(parse(&menu(0)), "あ", "codepage 0: the process codepage");
    }

    /// WinCreateStdWindow (908) with FCF_MENU must load the RT_MENU resource
    /// named by idResources, attach it to the frame, and size the client so it
    /// ends below the menu bar. Without the resource the frame has no menu.
    #[test]
    fn test_win_create_std_window_fcf_menu_loads_menu_resource() {
        use super::super::mutex_ext::MutexExt;
        use super::super::constants::{FCF_TITLEBAR, FCF_MENU, MENU_BAR_HEIGHT, CHROME_TITLE_H};
        let loader = Loader::new_mock();
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(TEST_CODEPAGE);
            wm.tid_to_hmq.insert(0, hmq);
        }
        // One-item wrc MENUTEMPLATE ("File", id 100) registered as RT_MENU id 7.
        let base: u32 = 0x5000;
        let data: &[u8] = &[
            0x15, 0x00, 0x00, 0x00, 0x52, 0x03, 0x04, 0x00, 0x01, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x64, 0x00, b'F', b'i', b'l', b'e', 0x00,
        ];
        for (i, &b) in data.iter().enumerate() {
            loader.guest_write::<u8>(base + i as u32, b).unwrap();
        }
        loader.shared.resource_mgr.lock_or_recover().add(0, crate::lx::header::RT_MENU, 7, base, data.len() as u32);
        // Class name string and flCreateFlags in guest memory.
        let (p_class, p_flags, p_client) = (0x6000u32, 0x6100u32, 0x6200u32);
        for (i, &b) in b"MenuTest\0".iter().enumerate() {
            loader.guest_write::<u8>(p_class + i as u32, b).unwrap();
        }
        loader.guest_write::<u32>(p_flags, FCF_TITLEBAR | FCF_MENU).unwrap();

        let create = |id_resources: u32| {
            let mut vcpu = MockVcpu::new();
            let esp: u32 = 0x1000;
            vcpu.regs.rsp = esp as u64;
            write_stack(&loader, esp, &[0, 0, p_flags, p_class, 0, 0, 0, id_resources, p_client]);
            loader.handle_pmwin_call(&mut vcpu, 0, 908);
            loader.guest_read::<u32>(p_client).unwrap()
        };

        let client = create(7);
        {
            let wm = loader.shared.window_mgr.lock_or_recover();
            let frame = wm.client_to_frame(client);
            let menu_hwnd = wm.get_window(frame).unwrap().menu_hwnd;
            assert_ne!(menu_hwnd, 0, "FCF_MENU must attach a menu");
            let menu = wm.get_window(menu_hwnd).unwrap();
            assert_eq!(wm.find_child_by_id(frame, super::super::constants::FID_MENU), Some(menu_hwnd));
            assert_eq!(menu.menu_items.len(), 1);
            assert_eq!(menu.menu_items[0].text, "File");
            assert_eq!(wm.get_window(client).unwrap().cy,
                       480 - CHROME_TITLE_H - MENU_BAR_HEIGHT as i32);
            assert_eq!(menu.y, wm.get_window(client).unwrap().cy);
        }

        // idResources with no RT_MENU resource: no menu, client only loses the title bar.
        let client = create(99);
        let wm = loader.shared.window_mgr.lock_or_recover();
        let frame = wm.client_to_frame(client);
        assert_eq!(wm.get_window(frame).unwrap().menu_hwnd, 0);
        assert_eq!(wm.get_window(client).unwrap().cy, 480 - CHROME_TITLE_H);
    }

    /// Replacing a frame's menu as the PM reference describes: WinLoadMenu /
    /// WinCreateMenu create an FID_MENU child of the frame, which the frame
    /// shows once it is sent WM_UPDATEFRAME(FCF_MENU); destroying the shown
    /// menu takes the bar away.
    #[test]
    fn test_menus_are_shown_by_wm_updateframe_and_replaced() {
        use super::super::constants::*;
        let loader = Loader::new_mock();
        let (frame, client, _, _) = window_with_entry(&loader);
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let w = wm.get_window_mut(frame).unwrap();
            (w.cx, w.cy, w.frame_flags) = (640, 480, FCF_TITLEBAR);
            wm.layout_frame(frame);
        }
        // One-item wrc MENUTEMPLATE ("File", id 100), as RT_MENU 7 and inline.
        let base: u32 = 0x5000;
        let data: &[u8] = &[
            0x15, 0x00, 0x00, 0x00, 0x52, 0x03, 0x04, 0x00, 0x01, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x64, 0x00, b'F', b'i', b'l', b'e', 0x00,
        ];
        for (i, &b) in data.iter().enumerate() {
            loader.guest_write::<u8>(base + i as u32, b).unwrap();
        }
        loader.shared.resource_mgr.lock_or_recover().add(0, crate::lx::header::RT_MENU, 7, base, data.len() as u32);
        let mut vcpu = MockVcpu::new();
        let mut call = |ord: u32, args: &[u32]| call_pmwin(&loader, &mut vcpu, ord, args);
        let shown = || loader.shared.window_mgr.lock_or_recover().get_window(frame).unwrap().menu_hwnd;
        let client_cy = || loader.shared.window_mgr.lock_or_recover().get_window(client).unwrap().cy;
        let full = 480 - CHROME_TITLE_H;

        let loaded = call(778, &[frame, 0, 7]);
        assert_ne!(loaded, 0);
        assert_eq!(shown(), 0, "not shown before WM_UPDATEFRAME");
        assert_eq!(call(899, &[frame, FID_MENU]), loaded, "WinWindowFromID(frame, FID_MENU)");
        assert_eq!(loader.shared.window_mgr.lock_or_recover().get_window(loaded).unwrap().owner, frame);

        assert_eq!(call(911, &[frame, WM_UPDATEFRAME, FCF_MENU, 0]), 1);
        assert_eq!(shown(), loaded);
        assert_eq!(client_cy(), full - MENU_BAR_HEIGHT as i32, "the client makes room for the bar");

        // A second menu replaces it on the next WM_UPDATEFRAME.
        let created = call(907, &[frame, base]);
        assert_eq!(shown(), loaded);
        call(911, &[frame, WM_UPDATEFRAME, FCF_MENU, 0]);
        assert_eq!(shown(), created);

        // Destroying the shown menu removes the bar.
        assert_eq!(call(728, &[created]), 1);
        assert_eq!(shown(), 0);
        assert_eq!(client_cy(), full);
        call(911, &[frame, WM_UPDATEFRAME, FCF_MENU, 0]);
        assert_eq!(shown(), loaded, "the remaining FID_MENU child");

        // Other flags and windows that are not frames leave it alone.
        call(911, &[frame, WM_UPDATEFRAME, FCF_TITLEBAR, 0]);
        assert_eq!(shown(), loaded);
        assert_eq!(call(911, &[client, WM_UPDATEFRAME, FCF_MENU, 0]), 0);
    }

    /// parse_menu_template with a SUBMENU item must recurse into the child
    /// block (10-byte sub-header) and return the nested structure.
    #[test]
    fn test_win_load_menu_submenu() {
        let loader = Loader::new_mock();

        // MENUTEMPLATE with 1 top-level SUBMENU "File" (id=200) containing
        // 2 children: "About" (id=201) and "Exit" (id=202).
        //
        // Main header (10 bytes): total=66, codepage=850, reserved=4, count=1
        // File item (17 bytes): style=MIS_TEXT|MIS_SUBMENU, attr=0, id=200, "~File\0"
        // Child sub-header (10 bytes): total=44, codepage=850, reserved=4, count=2
        // About item (16 bytes): style=MIS_TEXT, attr=0, id=201, "~About\0"
        // Exit item (15 bytes): style=MIS_TEXT, attr=0, id=202, "Exit\0"
        let base: u32 = 0x5000;
        let data: &[u8] = &[
            // Main header: count=1
            0x44, 0x00, 0x00, 0x00,  // total_size=68
            0x52, 0x03,               // codepage=850
            0x04, 0x00,               // reserved=4
            0x01, 0x00,               // item_count=1
            // File SUBMENU item: style=MIS_TEXT|MIS_SUBMENU=0x0011
            0x11, 0x00, 0x00, 0x00, 0xC8, 0x00,  // style, attr, id=200
            b'~', b'F', b'i', b'l', b'e', 0x00,   // "~File\0"
            // Child sub-header: count=2
            0x27, 0x00, 0x00, 0x00,  // total_size=39
            0x52, 0x03,               // codepage=850
            0x04, 0x00,               // reserved=4
            0x02, 0x00,               // child_count=2
            // About item: style=MIS_TEXT
            0x01, 0x00, 0x00, 0x00, 0xC9, 0x00,  // style, attr, id=201
            b'~', b'A', b'b', b'o', b'u', b't', 0x00, // "~About\0"
            // Exit item: style=MIS_TEXT
            0x01, 0x00, 0x00, 0x00, 0xCA, 0x00,  // style, attr, id=202
            b'E', b'x', b'i', b't', 0x00,          // "Exit\0"
        ];
        for (i, &b) in data.iter().enumerate() {
            loader.guest_write::<u8>(base + i as u32, b).unwrap();
        }

        let items = loader.parse_menu_template(base, data.len() as u32);
        assert_eq!(items.len(), 1, "Expected 1 top-level item");
        assert_eq!(items[0].id, 200);
        assert_eq!(items[0].text, "~File");
        assert_eq!(items[0].children.len(), 2, "Expected 2 children");
        assert_eq!(items[0].children[0].id, 201);
        assert_eq!(items[0].children[0].text, "~About");
        assert_eq!(items[0].children[1].id, 202);
        assert_eq!(items[0].children[1].text, "Exit");
    }

    /// WinDismissDlg (729) must set dialog_dismissed and dialog_result on the window.
    #[test]
    fn test_win_dismiss_dlg_sets_flag() {
        use super::super::mutex_ext::MutexExt;
        let loader = Loader::new_mock();
        let hwnd = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(TEST_CODEPAGE);
            wm.tid_to_hmq.insert(0, hmq);
            wm.create_window("#Dialog".to_string(), 0, hmq)
        };
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        write_stack(&loader, esp, &[hwnd, 42]);
        let result = loader.handle_pmwin_call(&mut vcpu, 0, 729); // WinDismissDlg
        assert!(matches!(result, ApiResult::Normal(1)));
        let wm = loader.shared.window_mgr.lock_or_recover();
        let win = wm.get_window(hwnd).unwrap();
        assert!(win.dialog_dismissed, "dialog_dismissed should be true");
        assert_eq!(win.dialog_result, 42, "dialog_result should be 42");
    }

    /// DLGTEMPLATE geometry is in dialog units; the created dialog and its
    /// children must be scaled to pixels.
    #[test]
    fn test_create_dialog_scales_dialog_units_to_pixels() {
        use super::super::mutex_ext::MutexExt;
        use super::super::pm_dialog::{DlgItem, DlgTemplate};
        let loader = Loader::new_mock();
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(TEST_CODEPAGE);
            wm.tid_to_hmq.insert(0, hmq);
        }
        use super::super::constants::{dlg_units_to_px_x as dx, dlg_units_to_px_y as dy,
                                      HWND_DESKTOP, FCF_TITLEBAR, FCF_DLGBORDER, WS_VISIBLE};
        let item = |class: &str, text: &str, x, y, cx, cy, id| DlgItem {
            class_name: class.to_string(), text: text.to_string(),
            x, y, cx, cy, id, style: WS_VISIBLE, parent: None, pres_params: Vec::new(), ctl_data: Vec::new(),
        };
        let template = DlgTemplate {
            dialog: item("WC_FRAME", "About", 40, 40, 180, 80, 101),
            frame_flags: FCF_TITLEBAR | FCF_DLGBORDER,
            focus: None,
            items: vec![item("WC_BUTTON", "OK", 70, 10, 40, 14, 1)],
        };
        let (hwnd_dlg, _, _) = loader.create_dialog(0, HWND_DESKTOP, 0, 0, Some(template.clone()));
        // One horizontal unit is 1/4 of the system font's average character
        // width, one vertical unit 1/8 of its 16-px line height (= 2 px).
        let avg_w = crate::gui::font::metrics(crate::gui::font::FontSpec::SYSTEM).avg_char_width;
        assert_eq!(dx(4), avg_w);
        assert_eq!(dy(8), 16);
        assert!(dx(180) > 180 && dx(180) < 360, "dialogs are scaled, but no longer by a fixed 8-px cell");
        {
            let wm = loader.shared.window_mgr.lock_or_recover();
            let dlg = wm.get_window(hwnd_dlg).unwrap();
            // Parent is the desktop: template position is desktop-relative.
            assert_eq!((dlg.x, dlg.y, dlg.cx, dlg.cy), (dx(40), dy(40), dx(180), dy(80)));
            assert_eq!(dlg.frame_flags & (FCF_TITLEBAR | FCF_DLGBORDER), FCF_TITLEBAR | FCF_DLGBORDER,
                       "dialogs use the standard compositor chrome");
            let child = wm.get_window(dlg.children[0]).unwrap();
            assert_eq!((child.x, child.y, child.cx, child.cy), (dx(70), dy(10), dx(40), dy(14)));
        }

        // Parent is a window: the template position is relative to its frame.
        let client = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let frame = wm.create_window("F".to_string(), 0, 0);
            let client = wm.create_window("C".to_string(), frame, 0);
            wm.frame_to_client.insert(frame, client);
            let f = wm.get_window_mut(frame).unwrap();
            f.x = 300; f.y = 200;
            client
        };
        let (hwnd_dlg, _, _) = loader.create_dialog(0, client, client, 0, Some(template));
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let dlg = wm.get_window(hwnd_dlg).unwrap();
        assert_eq!((dlg.x, dlg.y), (300 + dx(40), 200 + dy(40)));
        // The newest dialog is the active window until it is removed.
        assert_eq!(wm.focused_hwnd, hwnd_dlg);
        wm.z_remove(hwnd_dlg);
        assert_ne!(wm.focused_hwnd, hwnd_dlg);
    }

    /// Frames created without an explicit position cascade from the desktop's
    /// top-left corner and always stay on the desktop.
    #[test]
    fn test_default_frame_pos_cascades_and_stays_on_desktop() {
        use super::super::constants::{default_frame_pos, FRAME_CASCADE_ORIGIN as O,
                                      FRAME_CASCADE_STEP as STEP, FRAME_CASCADE_SLOTS};
        let desk = (1024, 768);
        // First frame: inset from the top-left corner (OS/2 y is from the bottom).
        assert_eq!(default_frame_pos(0, 640, 480, desk), (O, 768 - 480 - O));
        assert_eq!(default_frame_pos(1, 640, 480, desk), (O + STEP, 768 - 480 - O - STEP));
        // The cascade wraps round.
        assert_eq!(default_frame_pos(FRAME_CASCADE_SLOTS as usize, 640, 480, desk),
                   default_frame_pos(0, 640, 480, desk));
        // A frame as large as (or larger than) the desktop is pinned to the origin.
        assert_eq!(default_frame_pos(3, 1024, 768, desk), (0, 0));
        assert_eq!(default_frame_pos(3, 2000, 2000, desk), (0, 0));
        for n in 0..20 {
            let (x, y) = default_frame_pos(n, 640, 480, desk);
            assert!(x >= 0 && y >= 0 && x + 640 <= 1024 && y + 480 <= 768, "frame {n} off-desktop");
        }
    }

    /// WinSendDlgItemMsg (903) routes to a child control's built-in handler.
    #[test]
    fn test_win_send_dlg_item_msg_routes_to_child() {
        use super::super::mutex_ext::MutexExt;
        use super::super::constants::LM_INSERTITEM;
        let loader = Loader::new_mock();
        let (dlg_hwnd, _list_hwnd) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(TEST_CODEPAGE);
            wm.tid_to_hmq.insert(0, hmq);
            let dlg = wm.create_window("#Dialog".to_string(), 0, hmq);
            let list = wm.create_window("WC_LISTBOX".to_string(), dlg, hmq);
            wm.get_window_mut(list).unwrap().id = 101;
            (dlg, list)
        };
        // Write "ItemA\0" into guest RAM at 0x3000
        let text_ptr: u32 = 0x3000;
        for (i, &b) in b"ItemA\0".iter().enumerate() {
            loader.guest_write::<u8>(text_ptr + i as u32, b).unwrap();
        }
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        // WinSendDlgItemMsg(dlg, id=101, LM_INSERTITEM, LIT_END, text_ptr)
        write_stack(&loader, esp, &[dlg_hwnd, 101, LM_INSERTITEM, 0xFFFF_FFFF, text_ptr]);
        let result = loader.handle_pmwin_call(&mut vcpu, 0, 903);
        // Should return index 0 (first inserted item)
        assert!(matches!(result, ApiResult::Normal(0)), "Expected index 0, got {:?}", result);
    }

    /// Attaching a menu through the loader-level helper (the path WinLoadMenu
    /// takes) stores the menu hwnd on the frame and returns the menu it
    /// replaces. There is no PMWIN export for this; ordinal 857 is WinSetCp.
    #[test]
    fn test_attach_menu_to_frame() {
        use super::super::mutex_ext::MutexExt;
        let loader = Loader::new_mock();
        let (frame_hwnd, menu_hwnd, menu2_hwnd) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let frame = wm.create_window("WC_FRAME".to_string(), 0, 0);
            let menu  = wm.create_window("#Menu".to_string(), frame, 0);
            let menu2 = wm.create_window("#Menu".to_string(), frame, 0);
            (frame, menu, menu2)
        };
        assert_eq!(loader.attach_menu_to_frame(frame_hwnd, menu_hwnd), 0); // previous menu was 0
        assert_eq!(loader.shared.window_mgr.lock_or_recover()
            .get_window(frame_hwnd).unwrap().menu_hwnd, menu_hwnd);
        // Replacing the menu hands back the old one.
        assert_eq!(loader.attach_menu_to_frame(frame_hwnd, menu2_hwnd), menu_hwnd);
        assert_eq!(loader.shared.window_mgr.lock_or_recover()
            .get_window(frame_hwnd).unwrap().menu_hwnd, menu2_hwnd);
    }

    /// Call a PMWIN API with `args` on the stack and return its plain result.
    fn call_pmwin(loader: &Loader, vcpu: &mut MockVcpu, ordinal: u32, args: &[u32]) -> u32 {
        call_pmwin_on(loader, vcpu, 0, ordinal, args)
    }

    /// [`call_pmwin`] from thread `tid`.
    fn call_pmwin_on(loader: &Loader, vcpu: &mut MockVcpu, tid: u32, ordinal: u32, args: &[u32]) -> u32 {
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        write_stack(loader, esp, args);
        match loader.handle_pmwin_call(vcpu, tid, ordinal) {
            ApiResult::Normal(v) => v,
            other => panic!("PMWIN ordinal {} returned {:?}", ordinal, other),
        }
    }

    /// Queue codepage for tests that create a queue without caring about it.
    const TEST_CODEPAGE: u32 = 437;

    const ORD_WIN_CREATE_MSG_QUEUE: u32 = 716;
    const ORD_WIN_QUERY_CP: u32 = 810;
    const ORD_WIN_SET_CP: u32 = 857;

    /// A new queue reports the process codepage in force when it was created.
    #[test]
    fn test_win_query_cp_starts_as_process_codepage() {
        use std::sync::atomic::Ordering;
        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(850, Ordering::Relaxed);
        let mut vcpu = MockVcpu::new();
        let hmq = call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_CP, &[hmq]), 850);
    }

    /// One message queue per thread: a second WinCreateMsgQueue fails;
    /// WinDestroyMsgQueue destroys only the calling thread's own queue, drops
    /// its messages (posting to its windows then fails), and the thread can
    /// create a new queue afterwards.
    #[test]
    fn test_message_queue_lifecycle() {
        const ORD_WIN_DESTROY_MSG_QUEUE: u32 = 726;
        const ORD_WIN_POST_MSG: u32 = 919;
        use super::super::constants::WM_USER;
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let hmq = call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        assert_ne!(hmq, 0);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]), 0,
                   "the thread already has a queue");
        let hwnd = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.create_window("App".into(), 0, hmq)
        };
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_POST_MSG, &[hwnd, WM_USER, 0, 0]), 1);

        // Another thread may not destroy it, nor may a wrong handle.
        assert_eq!(call_pmwin_on(&loader, &mut vcpu, 1, ORD_WIN_DESTROY_MSG_QUEUE, &[hmq]), 0,
                   "another thread's queue");
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_DESTROY_MSG_QUEUE, &[hmq + 99]), 0);

        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_DESTROY_MSG_QUEUE, &[hmq]), 1);
        assert!(loader.shared.window_mgr.lock_or_recover().get_mq(hmq).is_none(), "gone with its messages");
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_DESTROY_MSG_QUEUE, &[hmq]), 0, "already destroyed");
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_POST_MSG, &[hwnd, WM_USER, 0, 0]), 0,
                   "nothing to post to");

        let again = call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        assert_ne!(again, 0, "a new queue after the old one is destroyed");
        assert_ne!(again, hmq);
    }

    /// WinSetCp (857) with a supported codepage returns TRUE, WinQueryCp (810)
    /// reports it, and the process codepage is left alone.
    #[test]
    fn test_win_set_cp_supported() {
        use std::sync::atomic::Ordering;
        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(437, Ordering::Relaxed);
        let mut vcpu = MockVcpu::new();
        let hmq = call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_CP, &[hmq, 932]), 1);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_CP, &[hmq]), 932);
        assert_eq!(loader.shared.active_codepage.load(Ordering::Relaxed), 437);
    }

    /// CP1004 is a PM codepage: a queue may use it, the process may not.
    #[test]
    fn test_win_set_cp_accepts_1004_and_cp_list_reports_it() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let hmq = call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_CP, &[hmq, 1004]), 1);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_CP, &[hmq]), 1004);
        assert_ne!(loader.dos_set_process_cp(1004), 0, "DosSetProcessCp refuses a PM-only codepage");

        // WinQueryCpList(hab, ccpMax, prgcp): the number available; at most ccpMax written.
        let list = 0x13300;
        let all = super::super::stubs::pm_codepages();
        assert!(all.contains(&1004) && all.contains(&850) && all.is_sorted());
        loader.guest_write::<u32>(list + 8, 0xFFFF_FFFF).unwrap();
        assert_eq!(call_pmwin(&loader, &mut vcpu, 811, &[0, 2, list]) as usize, all.len());
        assert_eq!(loader.guest_read::<u32>(list), Some(all[0]));
        assert_eq!(loader.guest_read::<u32>(list + 4), Some(all[1]));
        assert_eq!(loader.guest_read::<u32>(list + 8), Some(0xFFFF_FFFF), "ccpMax respected");
        assert_eq!(call_pmwin(&loader, &mut vcpu, 811, &[0, 0, 0]) as usize, all.len(), "count only");
    }

    /// Window text is converted with the codepage of the window's message
    /// queue, not the process codepage.
    #[test]
    fn test_window_text_uses_the_queue_codepage() {
        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(437, Ordering::Relaxed);
        let (frame, client, _, entry) = window_with_entry(&loader);
        {
            let wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.find_hmq_for_hwnd(client).unwrap();
            wm.get_mq(hmq).unwrap().lock_or_recover().codepage = 932;
        }
        let mut vcpu = MockVcpu::new();
        // "日本" in CP932.
        let (text, buf) = (0x13400u32, 0x13500u32);
        for (i, b) in [0x93u8, 0xFA, 0x96, 0x7B, 0].iter().enumerate() {
            loader.guest_write::<u8>(text + i as u32, *b).unwrap();
        }
        assert_eq!(call_pmwin(&loader, &mut vcpu, 877, &[frame, text]), 1); // WinSetWindowText
        assert_eq!(loader.shared.window_mgr.lock_or_recover().get_window(frame).unwrap().text, "日本");
        assert_eq!(call_pmwin(&loader, &mut vcpu, 841, &[frame, 16, buf]), 4); // WinQueryWindowText
        assert_eq!(loader.guest_read_bytes(buf, 5).unwrap(), vec![0x93, 0xFA, 0x96, 0x7B, 0]);
        assert_eq!(loader.window_codepage(entry), 932);
        assert_eq!(loader.thread_codepage(0), 932, "the calling thread's queue");
        assert_eq!(loader.thread_codepage(99), 437, "no queue: the process codepage");
        assert_eq!(loader.window_codepage(0xDEAD), 437);
    }

    /// The codepage belongs to the queue: setting one leaves another unchanged.
    #[test]
    fn test_win_set_cp_is_per_queue() {
        use std::sync::atomic::Ordering;
        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(437, Ordering::Relaxed);
        let mut vcpu = MockVcpu::new();
        let hmq_a = call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        // Another thread's queue (one queue per thread).
        let hmq_b = call_pmwin_on(&loader, &mut vcpu, 1, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        assert_ne!(hmq_a, hmq_b);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_CP, &[hmq_a, 850]), 1);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_CP, &[hmq_a]), 850);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_CP, &[hmq_b]), 437);
    }

    /// An unsupported codepage returns FALSE and changes nothing.
    #[test]
    fn test_win_set_cp_unsupported_codepage() {
        use std::sync::atomic::Ordering;
        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(437, Ordering::Relaxed);
        let mut vcpu = MockVcpu::new();
        let hmq = call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_CP, &[hmq, 12345]), 0);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_CP, &[hmq]), 437);
        assert_eq!(loader.shared.active_codepage.load(Ordering::Relaxed), 437);
    }

    /// An unknown queue handle returns FALSE from WinSetCp and 0 from
    /// WinQueryCp, and must not be mistaken for a frame window (the old
    /// WinSetMenu arm at this ordinal would have attached a "menu").
    #[test]
    fn test_win_set_cp_invalid_queue() {
        use super::super::mutex_ext::MutexExt;
        let loader = Loader::new_mock();
        let frame = loader.shared.window_mgr.lock_or_recover()
            .create_window("WC_FRAME".to_string(), 0, 0);
        let mut vcpu = MockVcpu::new();
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_CP, &[0xDEAD, 850]), 0);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_CP, &[0xDEAD]), 0);
        // A window handle is not a queue handle.
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_CP, &[frame, 850]), 0);
        assert_eq!(loader.shared.window_mgr.lock_or_recover()
            .get_window(frame).unwrap().menu_hwnd, 0);
    }

    /// WinQueryWindowText (ordinal 841) must encode the stored UTF-8 window
    /// title back into the guest's active codepage before writing to guest RAM.
    #[test]
    fn test_win_query_window_text_encodes_to_active_codepage() {
        let loader = Loader::new_mock();
        // Store a window with a non-ASCII title (CP850: é = 0x82)
        let hwnd = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let h = wm.create_window("WC_FRAME".to_string(), 0, 1);
            wm.get_window_mut(h).unwrap().text = "caf\u{00E9}".to_string(); // "café"
            h
        };
        // Switch active codepage to CP850
        loader.shared.active_codepage.store(850, Ordering::Relaxed);

        let mut vcpu = MockVcpu::new();
        let esp: u32    = 0x1000;
        let buf_ptr: u32 = 0x2000;
        vcpu.regs.rsp = esp as u64;
        // WinQueryWindowText(hwnd, cchMax=16, pchBuffer)
        write_stack(&loader, esp, &[hwnd, 16, buf_ptr]);
        let result = loader.handle_pmwin_call(&mut vcpu, 0, 841);
        assert!(matches!(result, ApiResult::Normal(4))); // "café" = 4 bytes in CP850

        // CP850: 'c'=0x63, 'a'=0x61, 'f'=0x66, 'é'=0x82
        assert_eq!(loader.guest_read::<u8>(buf_ptr).unwrap(),     b'c');
        assert_eq!(loader.guest_read::<u8>(buf_ptr + 1).unwrap(), b'a');
        assert_eq!(loader.guest_read::<u8>(buf_ptr + 2).unwrap(), b'f');
        assert_eq!(loader.guest_read::<u8>(buf_ptr + 3).unwrap(), 0x82); // é in CP850
        assert_eq!(loader.guest_read::<u8>(buf_ptr + 4).unwrap(), 0x00); // NUL terminator
    }

    /// dispatch_builtin_control #Menu WM_BUTTON1DOWN must toggle the open-item
    /// state when clicking a top-level SUBMENU item, and must post WM_COMMAND
    /// to the client window when a child item is clicked.
    #[test]
    fn test_menu_bar_click_opens_submenu_and_posts_command() {
        use super::super::mutex_ext::MutexExt;
        use super::super::constants::{WM_BUTTON1DOWN, WM_COMMAND, MIS_SUBMENU, MIS_TEXT, MENU_BAR_HEIGHT, CHROME_TITLE_H};
        use super::super::pm_types::MenuItem;

        let loader = Loader::new_mock();

        // Build: frame (cy=480) → menu window (y=440, cy=20) with one SUBMENU "File"
        //        containing two children: "About" (id=201) and "Exit" (id=202).
        //        Menu sits just below the title bar: y = frame_cy - CHROME_TITLE_H - MENU_BAR_HEIGHT.
        //        Also create a client window so frame_to_client resolves.
        let (_frame_hwnd, menu_hwnd, client_hwnd, hmq) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(TEST_CODEPAGE);
            wm.tid_to_hmq.insert(0, hmq);
            let frame = wm.create_window("TestClass".to_string(), 0, hmq);
            let client = wm.create_window("TestClass".to_string(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            let menu = wm.create_window("#Menu".to_string(), frame, hmq);
            // Position frame and menu windows
            let frame_cy = 480i32;
            wm.get_window_mut(frame).unwrap().cy = frame_cy;
            wm.get_window_mut(menu).unwrap().y = frame_cy - CHROME_TITLE_H - MENU_BAR_HEIGHT as i32;
            wm.get_window_mut(menu).unwrap().cy = MENU_BAR_HEIGHT as i32;
            wm.get_window_mut(menu).unwrap().cx = 640;
            wm.get_window_mut(frame).unwrap().menu_hwnd = menu;
            // Load items: "~File" SUBMENU with two children
            let file_item = MenuItem {
                id: 200, style: MIS_TEXT | MIS_SUBMENU, attr: 0,
                text: "~File".to_string(),
                children: vec![
                    MenuItem { id: 201, style: MIS_TEXT, attr: 0, text: "~About".to_string(), children: vec![], image: None },
                    MenuItem { id: 202, style: MIS_TEXT, attr: 0, text: "Exit".to_string(),   children: vec![], image: None },
                ],
                image: None,
            };
            wm.get_window_mut(menu).unwrap().menu_items = vec![file_item];
            (frame, menu, client, hmq)
        };

        // --- Step 1: click on "File" in the menu bar (x=6, y=445) ---
        // item_x starts at x+4 = 0+4 = 4; "File" label_w = 4*8+8 = 40 (stripping ~)
        // click_x=6 is within [4, 44), click_y=445 >= y=440 (menu bar occupies [440,460))
        let mp1_bar: u32 = 6u32 | (445u32 << 16);
        let res = loader.dispatch_builtin_control(menu_hwnd, WM_BUTTON1DOWN, mp1_bar, 0);
        assert!(matches!(res, ApiResult::Normal(0)));
        // open_item should now be 1 (1-based index 0)
        let open_item = loader.shared.window_mgr.lock_or_recover()
            .get_window(menu_hwnd).unwrap()
            .window_ulong.get(&-1).copied().unwrap_or(0);
        assert_eq!(open_item, 1, "open_item should be 1 after clicking File");

        // --- Step 2: click on "Exit" (child index 1) in the dropdown ---
        // Dropdown y range: [y - 2*16, y] = [440 - 32, 440] = [408, 440]
        // Exit is child index 1: item_y range [440 - 32, 440 - 16] = [408, 424]
        // Click at (x=6, y=415) → child_idx = (440-1-415)/16 = 24/16 = 1 → Exit (id=202)
        let mp1_drop: u32 = 6u32 | (415u32 << 16);
        let res = loader.dispatch_builtin_control(menu_hwnd, WM_BUTTON1DOWN, mp1_drop, 0);
        assert!(matches!(res, ApiResult::Normal(0)));
        // Dropdown should be closed
        let open_item = loader.shared.window_mgr.lock_or_recover()
            .get_window(menu_hwnd).unwrap()
            .window_ulong.get(&-1).copied().unwrap_or(0);
        assert_eq!(open_item, 0, "open_item should be 0 after clicking Exit");
        // WM_COMMAND with id=202 should be in the client's message queue
        let mq_arc = loader.shared.window_mgr.lock_or_recover()
            .get_mq(hmq).expect("mq should exist");
        let mq = mq_arc.lock_or_recover();
        let cmd_msg = mq.messages.iter().find(|m| m.msg == WM_COMMAND);
        assert!(cmd_msg.is_some(), "WM_COMMAND should be posted");
        let cmd = cmd_msg.unwrap();
        assert_eq!(cmd.hwnd, client_hwnd);
        // CMDMSG: mp1 = command id, mp2 = MPFROM2SHORT(source, fPointer).
        assert_eq!(cmd.mp1, 202); // Exit
        assert_eq!(cmd.mp2, super::super::constants::CMDSRC_MENU | (1 << 16), "menu, by the pointer");
    }

    // ── Parse robustness tests (T-2) ─────────────────────────────────────────

    /// parse_menu_template with a resource shorter than the 10-byte header
    /// must return an empty Vec without panicking.
    #[test]
    fn test_parse_menu_template_truncated_does_not_panic() {
        let loader = Loader::new_mock();
        let base: u32 = 0x5000;
        // Write only 5 bytes — less than the 10-byte header minimum.
        let data: &[u8] = &[0xFF, 0x01, 0x00, 0x00, 0x00];
        for (i, &b) in data.iter().enumerate() {
            loader.guest_write::<u8>(base + i as u32, b).unwrap();
        }
        let items = loader.parse_menu_template(base, data.len() as u32);
        assert!(items.is_empty(), "truncated header must produce no items");
    }

    /// parse_menu_template with an all-zero resource (item_count=0, no items)
    /// must return an empty Vec without panicking.
    #[test]
    fn test_parse_menu_template_all_zero_does_not_panic() {
        let loader = Loader::new_mock();
        let base: u32 = 0x5100;
        // 16 zero bytes — header is valid (size 16 >= 10) but item_count=0 and
        // every potential item byte is 0x00 (MIS_END sentinel with count fallback).
        let data = [0u8; 16];
        for (i, &b) in data.iter().enumerate() {
            loader.guest_write::<u8>(base + i as u32, b).unwrap();
        }
        let items = loader.parse_menu_template(base, data.len() as u32);
        // With item_count=0 (use_count=false) the loop stops at MIS_END sentinel
        // (style=0 contains bit 15 clear, so !use_count && (0 & MIS_END) == 0 is
        // false, but offset+6 > size=16 after two iterations → early break).
        assert!(items.len() <= 1, "all-zero template must not iterate unboundedly");
    }

    /// parse_menu_template with item_count=0xFFFF but only ~30 bytes of data
    /// must terminate as soon as the offset exceeds the available size,
    /// not iterate 65535 times.
    #[test]
    fn test_parse_menu_template_huge_item_count_bounded_by_size() {
        let loader = Loader::new_mock();
        let base: u32 = 0x5200;
        // Header: total_size=30, codepage=850, reserved=4, item_count=0xFFFF.
        // Item data: 20 bytes of junk after the 10-byte header.
        let mut data = [0u8; 30];
        // u32 total_size = 30
        data[0] = 30;
        // u16 codepage = 850 (0x0352)
        data[4] = 0x52; data[5] = 0x03;
        // u16 reserved = 4
        data[6] = 4;
        // u16 item_count = 0xFFFF
        data[8] = 0xFF; data[9] = 0xFF;
        // Fill item bytes with non-MIS_END style so the loop doesn't early-exit
        // via the sentinel path, only via the size guard.
        for b in &mut data[10..] { *b = 0x01; }
        for (i, &b) in data.iter().enumerate() {
            loader.guest_write::<u8>(base + i as u32, b).unwrap();
        }
        let items = loader.parse_menu_template(base, data.len() as u32);
        // The size guard (offset+6 > 30) caps how many 6-byte items can fit:
        // at most (30 - 10) / 6 = 3 iterations before text-read exhausts the buffer.
        assert!(items.len() <= 4, "huge item_count must be bounded by available data (got {})", items.len());
    }



    /// WinQueryWindowText with ASCII-only text must still work after the
    /// codepage-encode path (ASCII encodes identically in all supported CPs).
    #[test]
    fn test_win_query_window_text_ascii_roundtrip() {
        let loader = Loader::new_mock();
        let hwnd = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let h = wm.create_window("WC_FRAME".to_string(), 0, 1);
            wm.get_window_mut(h).unwrap().text = "Hello".to_string();
            h
        };
        let mut vcpu = MockVcpu::new();
        let esp: u32     = 0x1000;
        let buf_ptr: u32 = 0x2000;
        vcpu.regs.rsp = esp as u64;
        write_stack(&loader, esp, &[hwnd, 16, buf_ptr]);
        let result = loader.handle_pmwin_call(&mut vcpu, 0, 841);
        assert!(matches!(result, ApiResult::Normal(5)));
        assert_eq!(&loader.guest_read::<u8>(buf_ptr).unwrap(),     &b'H');
        assert_eq!(&loader.guest_read::<u8>(buf_ptr + 4).unwrap(), &b'o');
        assert_eq!(loader.guest_read::<u8>(buf_ptr + 5).unwrap(),   0x00);
    }

    /// A std window with one entry field (id 7, text "ab") created through
    /// WinCreateWindow with an explicit owner. Returns `(frame, client, owner, entry)`.
    fn window_with_entry(loader: &Loader) -> (u32, u32, u32, u32) {
        let (frame, client, owner) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(TEST_CODEPAGE);
            wm.tid_to_hmq.insert(0, hmq);
            wm.register_class("App".into(), 0x1234, 0);
            let frame = wm.create_window("App".into(), 0, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            let owner = wm.create_window("App".into(), 0, hmq);
            wm.frame_to_client.insert(frame, client);
            wm.z_push_top(frame);
            wm.focused_hwnd = frame;
            (frame, client, owner)
        };
        // WC_ENTRYFIELD is passed the way the toolkit headers define it: as an atom.
        let (p_class, p_text) = (super::super::constants::WC_ENTRYFIELD_ATOM, 0x13100u32);
        for (i, b) in "ab\0".bytes().enumerate() {
            loader.guest_write::<u8>(p_text + i as u32, b).unwrap();
        }
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        // WinCreateWindow(parent, class, name, style, x, y, cx, cy, owner, behind, id, ctl, pres)
        write_stack(loader, esp, &[client, p_class, p_text, super::super::constants::WS_VISIBLE, 10, 200, 200, 25,
                                   owner, 0, 7, 0, 0]);
        let entry = match loader.handle_pmwin_call(&mut vcpu, 0, 909) {
            ApiResult::Normal(h) => h,
            _ => panic!("WinCreateWindow should return a handle"),
        };
        (frame, client, owner, entry)
    }

    const ORD_WIN_DEF_WINDOW_PROC: u32 = 911;
    const ORD_WIN_GET_MSG: u32 = 915;
    const ORD_WIN_TRANSLATE_ACCEL: u32 = 904;

    /// A key press of F3 as `WM_CHAR` (mp1, mp2).
    fn f3_press() -> (u32, u32) {
        use super::super::constants::*;
        (KC_VIRTUALKEY | KC_SCANCODE | (1 << WM_CHAR_REPEAT_SHIFT) | (0x3D << WM_CHAR_SCANCODE_SHIFT),
         VK_F3 << WM_CHAR_VKEY_SHIFT)
    }

    fn read_qmsg(loader: &Loader, p: u32) -> (u32, u32, u32, u32) {
        let r = |o| loader.guest_read::<u32>(p + o).unwrap();
        (r(0), r(4), r(8), r(12))
    }

    /// WinGetMsg delivers a keystroke that is in the frame's accelerator
    /// table as the command it stands for, addressed to the client.
    #[test]
    fn test_win_get_msg_translates_accelerators() {
        use super::super::constants::*;
        use super::super::pm_types::AccelEntry;
        let loader = Loader::new_mock();
        let (frame, client, _, entry) = window_with_entry(&loader);
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let t = wm.add_accel_table(vec![AccelEntry { flags: AF_VIRTUALKEY, key: VK_F3 as u16, cmd: 77 }]);
            wm.set_window_accel(frame, t);
            // Drop the messages queued while the window was created.
            let hmq = wm.find_hmq_for_hwnd(client).unwrap();
            wm.get_mq(hmq).unwrap().lock_or_recover().messages.clear();
        }
        let (mp1, mp2) = f3_press();
        // Sent to the focused entry field: the accelerator still applies.
        loader.post_to_window(entry, WM_CHAR, mp1, mp2);
        let mut vcpu = MockVcpu::new();
        let pqmsg = 0x13200;
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_GET_MSG, &[0, pqmsg, 0, 0, 0]), 1);
        assert_eq!(read_qmsg(&loader, pqmsg), (client, WM_COMMAND, 77, CMDSRC_ACCELERATOR));
        // The key release is not an accelerator and stays a WM_CHAR.
        loader.post_to_window(entry, WM_CHAR, mp1 | KC_KEYUP, mp2);
        call_pmwin(&loader, &mut vcpu, ORD_WIN_GET_MSG, &[0, pqmsg, 0, 0, 0]);
        assert_eq!(read_qmsg(&loader, pqmsg).1, WM_CHAR);
    }

    const ORD_WIN_PEEK_MSG: u32 = 918;

    /// WinPeekMsg never waits, leaves the message with PM_NOREMOVE, and
    /// both it and WinGetMsg honour the window and message-range filters.
    #[test]
    fn test_win_peek_msg_and_message_filters() {
        use super::super::constants::*;
        let loader = Loader::new_mock();
        let (_, client, owner, entry) = window_with_entry(&loader);
        let hmq = loader.shared.window_mgr.lock_or_recover().find_hmq_for_hwnd(client).unwrap();
        let mq = loader.shared.window_mgr.lock_or_recover().get_mq(hmq).unwrap();
        mq.lock_or_recover().messages.clear();
        let mut vcpu = MockVcpu::new();
        let q = 0x13200;
        let peek = |vcpu: &mut MockVcpu, hwnd: u32, first: u32, last: u32, fl: u32| {
            call_pmwin(&loader, vcpu, ORD_WIN_PEEK_MSG, &[0, q, hwnd, first, last, fl])
        };
        assert_eq!(peek(&mut vcpu, 0, 0, 0, PM_REMOVE), 0, "empty queue: FALSE at once");

        const WM_USER: u32 = 0x1000;
        loader.post_to_window(owner, WM_USER + 1, 11, 0);
        loader.post_to_window(entry, WM_USER + 2, 22, 0);
        loader.post_to_window(client, WM_TIMER, 33, 0);
        assert_eq!(peek(&mut vcpu, 0, 0, 0, PM_NOREMOVE), 1);
        assert_eq!(read_qmsg(&loader, q), (owner, WM_USER + 1, 11, 0));
        assert_eq!(mq.lock_or_recover().messages.len(), 3, "PM_NOREMOVE leaves it");
        // Window filter: the client and its descendants (the entry field).
        assert_eq!(peek(&mut vcpu, client, 0, 0, PM_REMOVE), 1);
        assert_eq!(read_qmsg(&loader, q).0, entry);
        // Range filter.
        assert_eq!(peek(&mut vcpu, 0, WM_TIMER, WM_TIMER, PM_REMOVE), 1);
        assert_eq!(read_qmsg(&loader, q).1, WM_TIMER);
        assert_eq!(peek(&mut vcpu, client, 0, 0, PM_REMOVE), 0, "nothing left for the client");
        // WinGetMsg takes the remaining one.
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_GET_MSG, &[0, q, 0, 0, 0]), 1);
        assert_eq!(read_qmsg(&loader, q).0, owner);
        // WM_QUIT passes any filter and makes WinGetMsg return FALSE.
        loader.post_to_window(owner, WM_QUIT, 0, 0);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_GET_MSG, &[0, q, entry, WM_TIMER, WM_TIMER]), 0);
    }

    /// Keys wait in the system input queue: WinGetMsg / WinPeekMsg deliver
    /// them after the posted messages, to the focus window of the moment
    /// they are read, as accelerators where they are ones.
    #[test]
    fn test_win_get_msg_reads_input_after_posted_messages() {
        use super::super::constants::*;
        use super::super::pm_input::InputEvent;
        use super::super::pm_types::AccelEntry;
        let loader = Loader::new_mock();
        let (frame, client, _, entry) = window_with_entry(&loader);
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.find_hmq_for_hwnd(client).unwrap();
            wm.get_mq(hmq).unwrap().lock_or_recover().messages.clear();
            wm.set_focus(0);
            let t = wm.add_accel_table(vec![AccelEntry { flags: AF_VIRTUALKEY, key: VK_F3 as u16, cmd: 77 }]);
            wm.set_window_accel(frame, t);
            wm.post_input(InputEvent::Key { mp1: 1, mp2: 0 });
            wm.post_input(InputEvent::Char('x'));
            let (mp1, mp2) = f3_press();
            wm.post_input(InputEvent::Key { mp1, mp2 });
        }
        const WM_USER: u32 = 0x1000;
        loader.post_to_window(client, WM_USER, 0, 0);
        let mut vcpu = MockVcpu::new();
        let q = 0x13200;
        let get = |vcpu: &mut MockVcpu| {
            call_pmwin(&loader, vcpu, ORD_WIN_GET_MSG, &[0, q, 0, 0, 0]);
            read_qmsg(&loader, q)
        };
        assert_eq!(get(&mut vcpu).1, WM_USER, "posted messages first");
        // A window filter that the key's target is outside of: not taken.
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_PEEK_MSG, &[0, q, entry, 0, 0, PM_REMOVE]), 0);
        assert_eq!(get(&mut vcpu), (client, WM_CHAR, 1, 0), "no focus window: the client");
        // The first key moved the focus: the next ones go to the entry field.
        // Its notification is a posted message and comes before them.
        loader.shared.window_mgr.lock_or_recover().set_focus(entry);
        assert_eq!(get(&mut vcpu).1, WM_CONTROL);
        assert_eq!(get(&mut vcpu), (entry, WM_CHAR, KC_CHAR | (1 << WM_CHAR_REPEAT_SHIFT), b'x' as u32));
        assert_eq!(get(&mut vcpu), (client, WM_COMMAND, 77, CMDSRC_ACCELERATOR));
        assert_eq!(loader.shared.window_mgr.lock_or_recover().pending_input(), 0);
    }

    #[test]
    fn test_message_range_filter() {
        use super::msg_in_range;
        assert!(msg_in_range(5, 0, 0));
        assert!(msg_in_range(5, 5, 5) && !msg_in_range(6, 5, 5));
        assert!(msg_in_range(3, 2, 4) && !msg_in_range(1, 2, 4));
        // first > last: everything outside the gap between them.
        assert!(msg_in_range(1, 10, 2) && msg_in_range(10, 10, 2) && msg_in_range(50, 10, 2));
        assert!(!msg_in_range(5, 10, 2));
    }

    /// WinDestroyWindow sends WM_DESTROY to the client (the frame and the
    /// entry field have Warpine's default handling), then removes the
    /// windows; WinIsWindow then reports FALSE.
    #[test]
    fn test_win_destroy_window_and_is_window() {
        let loader = Loader::new_mock();
        let (frame, client, _, entry) = window_with_entry(&loader);
        let mut vcpu = MockVcpu::new();
        assert_eq!(call_pmwin(&loader, &mut vcpu, 772, &[0, entry]), 1);
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        write_stack(&loader, esp, &[frame]);
        let ApiResult::DestroyWindows { mut chain, hwnd, wnd_proc } = loader.handle_pmwin_call(&mut vcpu, 0, 728)
            else { panic!("WM_DESTROY goes to the client's procedure") };
        assert_eq!((hwnd, wnd_proc), (client, 0x1234));
        assert_eq!(call_pmwin(&loader, &mut vcpu, 772, &[0, entry]), 1, "still there during WM_DESTROY");
        assert_eq!(loader.advance_destroy(&mut chain), None);
        assert_eq!(chain.result, 1);
        for h in [frame, client, entry] {
            assert_eq!(call_pmwin(&loader, &mut vcpu, 772, &[0, h]), 0);
        }
        assert_eq!(call_pmwin(&loader, &mut vcpu, 728, &[frame]), 0, "already destroyed");
    }

    /// WinTranslateAccel uses the table it is given and the window it names.
    #[test]
    fn test_win_translate_accel_rewrites_the_message() {
        use super::super::constants::*;
        use super::super::pm_types::AccelEntry;
        let loader = Loader::new_mock();
        let haccel = loader.shared.window_mgr.lock_or_recover()
            .add_accel_table(vec![AccelEntry { flags: AF_VIRTUALKEY | AF_HELP, key: VK_F3 as u16, cmd: 5 }]);
        let (mp1, mp2) = f3_press();
        let pqmsg = 0x13200;
        for (o, v) in [(0, 0x55), (4, WM_CHAR), (8, mp1), (12, mp2)] {
            loader.guest_write::<u32>(pqmsg + o, v).unwrap();
        }
        let mut vcpu = MockVcpu::new();
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_TRANSLATE_ACCEL, &[0, 0x66, haccel + 1, pqmsg]), 0,
                   "unknown table");
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_TRANSLATE_ACCEL, &[0, 0x66, haccel, pqmsg]), 1);
        assert_eq!(read_qmsg(&loader, pqmsg), (0x66, WM_HELP, 5, CMDSRC_ACCELERATOR));
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_TRANSLATE_ACCEL, &[0, 0x66, haccel, pqmsg]), 0,
                   "no longer a WM_CHAR");
    }

    /// WinDefWindowProc turns the system command Close into WM_CLOSE.
    #[test]
    fn test_win_def_window_proc_sc_close_posts_wm_close() {
        use super::super::constants::*;
        let loader = Loader::new_mock();
        let (_, client, _, _) = window_with_entry(&loader);
        let hmq = loader.shared.window_mgr.lock_or_recover().find_hmq_for_hwnd(client).unwrap();
        let mq = loader.shared.window_mgr.lock_or_recover().get_mq(hmq).unwrap();
        mq.lock_or_recover().messages.clear();
        let mut vcpu = MockVcpu::new();
        call_pmwin(&loader, &mut vcpu, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_MINIMIZE, 0]);
        assert!(mq.lock_or_recover().messages.is_empty());
        call_pmwin(&loader, &mut vcpu, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_CLOSE, CMDSRC_ACCELERATOR]);
        let msgs: Vec<_> = mq.lock_or_recover().messages.drain(..).map(|m| (m.hwnd, m.msg)).collect();
        assert_eq!(msgs, vec![(client, WM_CLOSE)]);
    }

    /// Call a PMWIN API with `args` on the stack and return its result as is.
    fn call_pmwin_raw(loader: &Loader, ordinal: u32, args: &[u32]) -> ApiResult {
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        write_stack(loader, esp, args);
        loader.handle_pmwin_call(&mut vcpu, 0, ordinal)
    }

    const ORD_WIN_SET_WINDOW_POS: u32 = 875;
    const ORD_WIN_QUERY_WINDOW_USHORT: u32 = 844;
    const ORD_WIN_SET_WINDOW_USHORT: u32 = 879;
    const ORD_WIN_QUERY_WINDOW_ULONG: u32 = 843;

    /// `window_with_entry` whose 640×480 frame at (50, 60) has a sizing
    /// border and minimize / maximize buttons.
    fn minmax_window(loader: &Loader) -> (u32, u32) {
        use super::super::constants::*;
        let (frame, client, _, _) = window_with_entry(loader);
        let mut wm = loader.shared.window_mgr.lock_or_recover();
        let w = wm.get_window_mut(frame).unwrap();
        (w.x, w.y, w.cx, w.cy) = (50, 60, 640, 480);
        w.frame_flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINBUTTON | FCF_MAXBUTTON;
        wm.layout_frame(frame);
        (frame, client)
    }

    /// WinDefWindowProc carries out SC_MAXIMIZE / SC_MINIMIZE / SC_RESTORE
    /// for a frame with minimize / maximize buttons, asking the client with
    /// WM_MINMAXFRAME first; without the buttons nothing happens.
    #[test]
    fn test_win_def_window_proc_minmax_system_commands() {
        use super::super::constants::*;
        use super::super::pm_minmax::WinRect;
        let loader = Loader::new_mock();
        let (frame, client) = minmax_window(&loader);
        let state = || loader.shared.window_mgr.lock_or_recover().minmax_state(frame);
        let ApiResult::MinMaxFrame { wnd_proc, client: to, frame: f, op, target, ret } =
            call_pmwin_raw(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_MAXIMIZE, CMDSRC_MENU])
        else { panic!("WM_MINMAXFRAME expected") };
        assert_eq!((wnd_proc, to, f, op, ret), (0x1234, client, frame, SWP_MAXIMIZE, 0));
        let (dw, dh) = desktop_size();
        assert_eq!(target, WinRect { x: -2, y: -2, cx: dw as i32 + 4, cy: dh as i32 + 4 });
        // Not changed yet: the client may refuse.
        assert_eq!(state(), 0);
        loader.shared.window_mgr.lock_or_recover().finish_minmax(frame, op, target);
        assert_eq!(state(), WS_MAXIMIZED);
        // Already maximized: nothing to do.
        assert!(matches!(call_pmwin_raw(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_MAXIMIZE, 0]),
                         ApiResult::Normal(0)));
        let ApiResult::MinMaxFrame { op, target, .. } =
            call_pmwin_raw(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_RESTORE, 0])
        else { panic!("WM_MINMAXFRAME expected") };
        assert_eq!((op, target), (SWP_RESTORE, WinRect { x: 50, y: 60, cx: 640, cy: 480 }));
        // Without minimize / maximize buttons the commands are ignored.
        loader.shared.window_mgr.lock_or_recover().get_window_mut(frame).unwrap().frame_flags = FCF_TITLEBAR;
        assert!(matches!(call_pmwin_raw(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_MINIMIZE, 0]),
                         ApiResult::Normal(0)));
        // WM_MINMAXFRAME itself: the default answer is FALSE.
        assert!(matches!(call_pmwin_raw(&loader, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_MINMAXFRAME, 0, 0]),
                         ApiResult::Normal(0)));
    }

    /// WinSetWindowPos with SWP_MINIMIZE / SWP_RESTORE (any frame, buttons
    /// or not), the restore and icon positions in the frame's window words,
    /// and WS_MINIMIZED in QWL_STYLE.
    #[test]
    fn test_win_set_window_pos_minimize_and_restore() {
        use super::super::constants::*;
        let loader = Loader::new_mock();
        let (frame, client) = minmax_window(&loader);
        {
            // A client without a window procedure is not asked.
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            wm.get_window_mut(client).unwrap().pfn_wp = 0;
            wm.get_window_mut(frame).unwrap().frame_flags = FCF_TITLEBAR | FCF_SIZEBORDER;
        }
        let mut vcpu = MockVcpu::new();
        let ushort = |vcpu: &mut MockVcpu, i: i32| call_pmwin(&loader, vcpu, ORD_WIN_QUERY_WINDOW_USHORT, &[frame, i as u32]);
        assert_eq!(ushort(&mut vcpu, QWS_XMINIMIZE), 0xFFFF);
        assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 0, 0, 0, 0, SWP_MINIMIZE]), 1);
        let style = call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_WINDOW_ULONG, &[frame, QWL_STYLE as u32]);
        assert_ne!(style & WS_MINIMIZED, 0);
        assert_eq!(style & WS_MAXIMIZED, 0);
        let pos = |l: &Loader| l.shared.window_mgr.lock_or_recover().get_window(frame).map(|w| (w.x, w.y, w.cx, w.cy)).unwrap();
        assert_eq!(pos(&loader), (ICON_AREA_GAP, ICON_AREA_GAP, MINIMIZED_CX, MINIMIZED_CY));
        assert_eq!(ushort(&mut vcpu, QWS_XMINIMIZE), ICON_AREA_GAP as u32);
        assert_eq!((ushort(&mut vcpu, QWS_XRESTORE), ushort(&mut vcpu, QWS_YRESTORE)), (50, 60));
        assert_eq!((ushort(&mut vcpu, QWS_CXRESTORE), ushort(&mut vcpu, QWS_CYRESTORE)), (640, 480));
        // Minimizing again has no effect; mutually exclusive flags neither.
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 0, 0, 0, 0, SWP_MINIMIZE]);
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 0, 0, 0, 0, SWP_MAXIMIZE | SWP_RESTORE]);
        assert_eq!(pos(&loader).2, MINIMIZED_CX);
        // A changed restore width is honoured.
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_USHORT, &[frame, QWS_CXRESTORE as u32, 300]);
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 0, 0, 0, 0, SWP_RESTORE]);
        assert_eq!(pos(&loader), (50, 60, 300, 480));
        let style = call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_WINDOW_ULONG, &[frame, QWL_STYLE as u32]);
        assert_eq!(style & (WS_MINIMIZED | WS_MAXIMIZED), 0);
        // Restoring a normal window only moves it as asked.
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 7, 8, 0, 0, SWP_RESTORE | SWP_MOVE]);
        assert_eq!(pos(&loader), (7, 8, 300, 480));
        // Maximized, then restored to an explicit position and size.
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 0, 0, 0, 0, SWP_MAXIMIZE]);
        assert_eq!(pos(&loader).0, -CHROME_BORDER_W);
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 10, 20, 200, 100, SWP_RESTORE | SWP_MOVE | SWP_SIZE]);
        assert_eq!(pos(&loader), (10, 20, 200, 100));
    }

    /// The messages queued on thread 0's queue, removed from it.
    fn drain_queue(loader: &Loader) -> Vec<(u32, u32, u32, u32)> {
        let wm = loader.shared.window_mgr.lock_or_recover();
        let mq = wm.get_mq(wm.tid_to_hmq[&0]).unwrap();
        let mut mq = mq.lock_or_recover();
        mq.messages.drain(..).map(|m| (m.hwnd, m.msg, m.mp1, m.mp2)).collect()
    }

    /// WinSetWindowPos(SWP_SIZE): a frame's client is told its old and new
    /// size with WM_SIZE (then WM_PAINT); another window is told its own;
    /// a size that does not change sends nothing.
    #[test]
    fn test_win_set_window_pos_size_sends_wm_size() {
        use super::super::constants::*;
        use super::super::pm_minmax::pack_size;
        let loader = Loader::new_mock();
        let (frame, client) = minmax_window(&loader);
        let entry = loader.shared.window_mgr.lock_or_recover()
            .get_window(client).unwrap().children[0];
        drain_queue(&loader);
        let mut vcpu = MockVcpu::new();
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 0, 0, 400, 300, SWP_SIZE]);
        let title = CHROME_TITLE_H;
        // Shrunk, without CS_SIZEREDRAW: nothing uncovered to paint
        // (pm_paint_style.rs).
        assert_eq!(drain_queue(&loader), vec![
            (client, WM_SIZE, pack_size((640, 480 - title)), pack_size((400, 300 - title))),
        ]);
        // Grown: the uncovered band is painted.
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 0, 0, 500, 300, SWP_SIZE]);
        assert_eq!(drain_queue(&loader), vec![
            (client, WM_SIZE, pack_size((400, 300 - title)), pack_size((500, 300 - title))),
            (client, WM_PAINT, 0, 0),
        ]);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().update_rect(client),
                   super::super::pm_rect::Rectl::new(400, 0, 500, 300 - title));
        loader.shared.window_mgr.lock_or_recover().begin_paint(client);
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 0, 0, 400, 300, SWP_SIZE]);
        drain_queue(&loader);
        // Same size again, or a move only: no WM_SIZE.
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 5, 5, 400, 300, SWP_SIZE | SWP_MOVE]);
        assert!(drain_queue(&loader).is_empty());
        // A control: WM_SIZE to itself.
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[entry, 0, 0, 0, 100, 30, SWP_SIZE]);
        assert_eq!(drain_queue(&loader), vec![
            (entry, WM_SIZE, pack_size((200, 25)), pack_size((100, 30))),
            (entry, WM_PAINT, 0, 0),
        ]);
        // A state change sends its own WM_SIZE (after WM_MINMAXFRAME), not this one.
        assert!(matches!(call_pmwin_raw(&loader, ORD_WIN_SET_WINDOW_POS, &[frame, 0, 0, 0, 10, 10, SWP_MAXIMIZE | SWP_SIZE]),
                         ApiResult::MinMaxFrame { .. }));
        assert!(drain_queue(&loader).is_empty());
    }

    const ORD_WIN_QUERY_WINDOW_POS: u32 = 837;

    /// WinQueryWindowPos: fl holds SWP_MOVE | SWP_SIZE, SWP_MINIMIZE or
    /// SWP_MAXIMIZE for a minimized or maximized frame, and the frame's
    /// activation state.
    #[test]
    fn test_win_query_window_pos_reports_the_state() {
        use super::super::constants::*;
        let loader = Loader::new_mock();
        let (frame, client) = minmax_window(&loader);
        loader.shared.window_mgr.lock_or_recover().get_window_mut(client).unwrap().pfn_wp = 0;
        let pswp = 0x13400;
        let mut vcpu = MockVcpu::new();
        let mut fl = |hwnd: u32| {
            assert_eq!(call_pmwin(&loader, &mut vcpu, ORD_WIN_QUERY_WINDOW_POS, &[hwnd, pswp]), 1);
            loader.guest_read::<u32>(pswp).unwrap()
        };
        assert_eq!(fl(frame), SWP_MOVE | SWP_SIZE | SWP_ACTIVATE);
        assert_eq!(fl(client), SWP_MOVE | SWP_SIZE, "not a frame");
        loader.shared.window_mgr.lock_or_recover().set_minmax(frame, SWP_MINIMIZE);
        assert_eq!(fl(frame), SWP_MOVE | SWP_SIZE | SWP_MINIMIZE | SWP_ACTIVATE, "the only frame stays active");
        loader.shared.window_mgr.lock_or_recover().set_minmax(frame, SWP_MAXIMIZE);
        assert_eq!(fl(frame), SWP_MOVE | SWP_SIZE | SWP_MAXIMIZE | SWP_ACTIVATE);
        loader.shared.window_mgr.lock_or_recover().focused_hwnd = 0;
        assert_eq!(fl(frame), SWP_MOVE | SWP_SIZE | SWP_MAXIMIZE | SWP_DEACTIVATE);
        // The whole 36-byte SWP is written: the reserved words are cleared.
        loader.guest_write::<u32>(pswp + 28, 0xDEAD_BEEF).unwrap();
        loader.guest_write::<u32>(pswp + 32, 0xDEAD_BEEF).unwrap();
        fl(frame);
        assert_eq!((loader.guest_read::<u32>(pswp + 28).unwrap(), loader.guest_read::<u32>(pswp + 32).unwrap()), (0, 0));
    }

    /// WinDefWindowProc(WM_SYSCOMMAND(SC_HIDE)) hides the frame; the frame
    /// below it becomes active (WM_ACTIVATE FALSE, then TRUE). WinShowWindow
    /// shows it again without activating it.
    #[test]
    fn test_win_def_window_proc_sc_hide_hides_the_frame() {
        use super::super::constants::*;
        let loader = Loader::new_mock();
        let (frame, client) = minmax_window(&loader);
        let (f2, c2) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.tid_to_hmq[&0];
            let f2 = wm.create_window("App".into(), 0, hmq);
            let c2 = wm.create_window("App".into(), f2, hmq);
            wm.frame_to_client.insert(f2, c2);
            wm.z_push_bottom(f2);
            for h in [frame, f2] { wm.get_window_mut(h).unwrap().visible = true; }
            wm.get_window_mut(frame).unwrap().frame_flags |= FCF_HIDEBUTTON;
            (f2, c2)
        };
        drain_queue(&loader);
        let mut vcpu = MockVcpu::new();
        call_pmwin(&loader, &mut vcpu, ORD_WIN_DEF_WINDOW_PROC, &[client, WM_SYSCOMMAND, SC_HIDE, CMDSRC_MENU]);
        let wm = || loader.shared.window_mgr.lock_or_recover();
        assert!(!wm().get_window(frame).unwrap().visible);
        assert_eq!(wm().focused_hwnd, f2);
        assert_eq!(wm().z_order.last(), Some(&f2));
        assert_eq!(drain_queue(&loader), vec![(client, WM_ACTIVATE, 0, f2), (c2, WM_ACTIVATE, 1, frame)]);
        // WinShowWindow(TRUE): visible again, f2 stays active.
        call_pmwin(&loader, &mut vcpu, 883, &[frame, 1]);
        assert!(wm().get_window(frame).unwrap().visible);
        assert_eq!(wm().focused_hwnd, f2);
        // WinShowWindow(FALSE) on the active frame passes activation on too.
        call_pmwin(&loader, &mut vcpu, 883, &[f2, 0]);
        assert_eq!(wm().focused_hwnd, frame);
        // Hiding a frame that is not active changes no activation.
        drain_queue(&loader);
        call_pmwin(&loader, &mut vcpu, ORD_WIN_SET_WINDOW_POS, &[f2, 0, 0, 0, 0, 0, SWP_SHOW]);
        call_pmwin(&loader, &mut vcpu, ORD_WIN_DEF_WINDOW_PROC, &[c2, WM_SYSCOMMAND, SC_HIDE, 0]);
        assert_eq!(wm().focused_hwnd, frame);
        assert!(drain_queue(&loader).is_empty());
    }

    /// WinSetFocus (860) / WinQueryFocus (817): the focus window defaults to
    /// the active frame's client, can be moved to a control and cleared, and
    /// a bad handle is rejected without changing it.
    #[test]
    fn test_win_set_focus_and_query_focus() {
        let loader = Loader::new_mock();
        let (_frame, client, _owner, entry) = window_with_entry(&loader);
        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        let mut call = |ordinal: u32, args: &[u32]| {
            write_stack(&loader, esp, args);
            match loader.handle_pmwin_call(&mut vcpu, 0, ordinal) {
                ApiResult::Normal(v) => v,
                _ => panic!("expected a plain return value"),
            }
        };
        const HWND_DESKTOP: u32 = 1;
        assert_eq!(call(817, &[HWND_DESKTOP]), client, "default focus: the client");
        assert_eq!(call(860, &[HWND_DESKTOP, entry]), 1);
        assert_eq!(call(817, &[HWND_DESKTOP]), entry);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().key_target(), entry);
        assert_eq!(call(860, &[HWND_DESKTOP, 0xDEAD]), 0, "unknown window");
        assert_eq!(call(817, &[HWND_DESKTOP]), entry, "focus unchanged");
        assert_eq!(call(860, &[HWND_DESKTOP, client]), 1);
        assert_eq!(call(817, &[HWND_DESKTOP]), client);
        assert_eq!(call(860, &[HWND_DESKTOP, entry]), 1);
        assert_eq!(call(860, &[HWND_DESKTOP, 0]), 1, "NULLHANDLE clears the focus");
        assert_eq!(call(817, &[HWND_DESKTOP]), client);
        // A desktop window as the new focus: no window gets it.
        assert_eq!(call(860, &[HWND_DESKTOP, entry]), 1);
        assert_eq!(call(860, &[HWND_DESKTOP, HWND_DESKTOP]), 1, "the desktop clears the focus");
        assert_eq!(call(817, &[HWND_DESKTOP]), client);
    }

    /// WinSetActiveWindow posts WM_ACTIVATE to the clients of the frames
    /// (where clicks and minimizing post it): FALSE to the old active frame's
    /// client, then TRUE to the new one's.
    #[test]
    fn test_win_set_active_window_notifies_the_clients() {
        use super::super::constants::{HWND_DESKTOP, WM_ACTIVATE};
        let loader = Loader::new_mock();
        let (frame, client, _owner, _entry) = window_with_entry(&loader);
        let (frame2, client2, hmq) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = *wm.tid_to_hmq.get(&0).unwrap();
            let frame2 = wm.create_window("App".into(), 0, hmq);
            let client2 = wm.create_window("App".into(), frame2, hmq);
            wm.frame_to_client.insert(frame2, client2);
            wm.get_mq(hmq).unwrap().lock_or_recover().messages.clear();
            (frame2, client2, hmq)
        };
        let mut vcpu = MockVcpu::new();
        assert_eq!(call_pmwin(&loader, &mut vcpu, 851, &[HWND_DESKTOP, frame2]), 1);
        let wm = loader.shared.window_mgr.lock_or_recover();
        assert_eq!(wm.focused_hwnd, frame2);
        let acts: Vec<_> = wm.get_mq(hmq).unwrap().lock_or_recover().messages.iter()
            .filter(|m| m.msg == WM_ACTIVATE).map(|m| (m.hwnd, m.mp1, m.mp2)).collect();
        assert_eq!(acts, vec![(client, 0, frame2), (client2, 1, frame)]);
    }

    /// WinEnableWindow returns TRUE whenever it updates the state, whatever
    /// the state was before (PM Reference: "enabled state successfully
    /// updated"), and FALSE only for a handle that is no window.
    #[test]
    fn test_win_enable_window_returns_success() {
        use super::super::constants::WS_DISABLED;
        let loader = Loader::new_mock();
        let (_frame, _client, _owner, entry) = window_with_entry(&loader);
        let mut vcpu = MockVcpu::new();
        let disabled = || loader.shared.window_mgr.lock_or_recover()
            .get_window(entry).is_some_and(|w| w.style & WS_DISABLED != 0);
        assert_eq!(call_pmwin(&loader, &mut vcpu, 735, &[entry, 1]), 1, "already enabled");
        assert_eq!(call_pmwin(&loader, &mut vcpu, 735, &[entry, 0]), 1);
        assert!(disabled());
        assert_eq!(call_pmwin(&loader, &mut vcpu, 735, &[entry, 0]), 1, "already disabled");
        assert_eq!(call_pmwin(&loader, &mut vcpu, 735, &[entry, 1]), 1);
        assert!(!disabled());
        assert_eq!(call_pmwin(&loader, &mut vcpu, 735, &[0xDEAD, 1]), 0, "no window");
    }

    /// End to end through the PMWIN API: a click and keystrokes dispatched
    /// with WinDispatchMsg (912) edit the entry field, WinQueryWindowText
    /// (841) returns the edited text, EN_CHANGE goes to the owner given to
    /// WinCreateWindow, and WinSetWindowText (877) still replaces the text.
    #[test]
    fn test_entry_field_editing_through_dispatch_and_window_text() {
        use super::super::constants::{WM_CHAR, WM_BUTTON1DOWN, WM_CONTROL, EN_CHANGE,
                                      KC_CHAR, KC_VIRTUALKEY, VK_BACKSPACE, WM_CHAR_VKEY_SHIFT};
        let loader = Loader::new_mock();
        let (_frame, _client, owner, entry) = window_with_entry(&loader);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().get_window(entry).unwrap().owner, owner);

        let mut vcpu = MockVcpu::new();
        let esp: u32 = 0x1000;
        let (qmsg, buf, p_new) = (0x2000u32, 0x2100u32, 0x2200u32);
        vcpu.regs.rsp = esp as u64;
        let mut call = |ordinal: u32, args: &[u32]| {
            write_stack(&loader, esp, args);
            match loader.handle_pmwin_call(&mut vcpu, 0, ordinal) {
                ApiResult::Normal(v) => v,
                _ => panic!("expected a plain return value"),
            }
        };
        let mut dispatch = |msg: u32, mp1: u32, mp2: u32| {
            for (i, v) in [entry, msg, mp1, mp2].into_iter().enumerate() {
                loader.guest_write::<u32>(qmsg + i as u32 * 4, v).unwrap();
            }
            call(912, &[0, qmsg]);
        };
        // Click at the far right (x=190, y=12 inside the field): caret after "ab".
        dispatch(WM_BUTTON1DOWN, 190 | (12 << 16), 0);
        dispatch(WM_CHAR, KC_CHAR, b'c' as u32);
        dispatch(WM_CHAR, KC_CHAR, b'd' as u32);
        dispatch(WM_CHAR, KC_VIRTUALKEY, VK_BACKSPACE << WM_CHAR_VKEY_SHIFT);
        assert_eq!(loader.shared.window_mgr.lock_or_recover().focus_hwnd, entry);

        let text_of = |n: u32| {
            (0..n).map(|i| loader.guest_read::<u8>(buf + i).unwrap() as char).collect::<String>()
        };
        // WinQueryWindowText(hwnd, cchMax, pchBuffer)
        assert_eq!(text_of(call(841, &[entry, 16, buf])), "abc");

        // Three text changes → three EN_CHANGE notifications to the owner.
        let notes: Vec<(u32, u32, u32)> = {
            let wm = loader.shared.window_mgr.lock_or_recover();
            let mq = wm.get_mq(wm.find_hmq_for_hwnd(owner).unwrap()).unwrap();
            let mq = mq.lock_or_recover();
            mq.messages.iter().filter(|m| m.msg == WM_CONTROL && m.mp1 >> 16 == EN_CHANGE)
                .map(|m| (m.hwnd, m.mp1, m.mp2)).collect()
        };
        assert_eq!(notes, vec![(owner, (EN_CHANGE << 16) | 7, entry); 3]);

        // WinSetWindowText with a shorter text; typing continues at its end.
        for (i, b) in "x\0".bytes().enumerate() {
            loader.guest_write::<u8>(p_new + i as u32, b).unwrap();
        }
        call(877, &[entry, p_new]);
        assert_eq!(text_of(call(841, &[entry, 16, buf])), "x");
        for (i, v) in [entry, WM_CHAR, KC_CHAR, b'y' as u32].into_iter().enumerate() {
            loader.guest_write::<u32>(qmsg + i as u32 * 4, v).unwrap();
        }
        call(912, &[0, qmsg]);
        assert_eq!(text_of(call(841, &[entry, 16, buf])), "xy");
    }
}
