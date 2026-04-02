// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 Presentation Manager PMWIN API handler methods.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use super::vm_backend::VcpuBackend;
use log::{debug, info, warn};

use super::constants::*;

// ── Built-in class atom → canonical class name ────────────────────────────────

/// Resolve a WinCreateWindow `pszClass` argument.
///
/// OS/2 PM encodes built-in class atoms as `MAKEINTATOM(n) = 0xFFFF0000 | n`.
/// If `ptr` carries the `0xFFFF` high word (or is a small bare atom < 0x10000),
/// we map it to the canonical class-name string.  Otherwise `string` contains
/// the already-read guest class-name string.
fn resolve_class_atom(ptr: u32, string: String) -> String {
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
use crate::lx::header::{RT_STRING, RT_MENU, RT_ACCELTABLE};

impl super::Loader {
    pub(crate) fn handle_pmwin_call(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, ordinal: u32) -> ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| -> u32 { self.guest_read::<u32>((esp + off) as u32).expect("Stack read OOB") };

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
                let hmq = wm.create_mq();
                wm.tid_to_hmq.insert(vcpu_id, hmq);
                ApiResult::Normal(hmq)
            }
            726 => {
                // WinDestroyMsgQueue
                debug!("  [VCPU {}] WinDestroyMsgQueue called.", vcpu_id);
                ApiResult::Normal(1) // TRUE
            }
            926 => {
                // WinRegisterClass
                let _hab = read_stack(4);
                let psz_class_name_ptr = read_stack(8);
                let pfn_wp = read_stack(12);
                let style = read_stack(16);
                let _cb_window_data = read_stack(20);
                let name = self.read_guest_string(psz_class_name_ptr);
                debug!("  [VCPU {}] WinRegisterClass: name='{}', pfn_wp=0x{:08X}", vcpu_id, name, pfn_wp);
                self.shared.window_mgr.lock_or_recover().register_class(name, pfn_wp, style);
                ApiResult::Normal(1) // TRUE
            }
            908 => {
                // WinCreateStdWindow
                let parent = read_stack(4);
                let style = read_stack(8);
                let _pfc_flags_ptr = read_stack(12);
                let psz_class_name_ptr = read_stack(16);
                let psz_title_ptr = read_stack(20);
                let _client_style = read_stack(24);
                let _hmod = read_stack(28);
                let _id = read_stack(32);
                let phwnd_client_ptr = read_stack(36);
                let class_name = self.read_guest_string(psz_class_name_ptr);
                let title = if psz_title_ptr != 0 { self.read_guest_string(psz_title_ptr) } else { "Warpine Window".to_string() };
                debug!("  [VCPU {}] WinCreateStdWindow: class='{}', title='{}', parent=0x{:08X}, style=0x{:08X}", vcpu_id, class_name, title, parent, style);

                let (h_frame, h_client, pfn_wp_client) = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    let h_frame = wm.create_window(class_name.clone(), parent, hmq);
                    let h_client = wm.create_window(class_name.clone(), h_frame, hmq);
                    wm.frame_to_client.insert(h_frame, h_client);
                    // Initialise both windows to the default SDL2 window size so that
                    // WinQueryWindowRect returns correct dimensions before the first resize.
                    if let Some(win) = wm.get_window_mut(h_frame)  { win.cx = 640; win.cy = 480; }
                    if let Some(win) = wm.get_window_mut(h_client) { win.cx = 640; win.cy = 480; }

                    if let Some(ref sender) = wm.gui_tx {
                        let _ = sender.send(GUIMessage::CreateWindow { class: class_name, title, handle: h_frame });
                    }

                    // Post initial WM_PAINT so the guest paints on creation
                    let hmq2 = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    if let Some(mq_arc) = wm.get_mq(hmq2) {
                        let mut mq = mq_arc.lock_or_recover();
                        mq.messages.push_back(OS2Message {
                            hwnd: h_client, msg: WM_PAINT, mp1: 0, mp2: 0, time: 0, x: 0, y: 0,
                        });
                        mq.cond.notify_one();
                    }

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
                // WinGetMsg
                let _hab = read_stack(4);
                let pqmsg_ptr = read_stack(8);
                let _hwnd = read_stack(12);
                let _first = read_stack(16);
                let _last = read_stack(20);

                // Find the message queue for this thread
                let (_hmq, mq_arc) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    let mq = wm.get_mq(hmq);
                    (hmq, mq)
                };

                if let Some(mq_arc) = mq_arc {
                    // Get the condvar/lock for blocking wait
                    let (cond, wait_lock) = {
                        let mq = mq_arc.lock_or_recover();
                        (Arc::clone(&mq.cond), Arc::clone(&mq.lock))
                    };
                    loop {
                        if self.shutting_down() { return ApiResult::Normal(0); }
                        {
                            let mut mq = mq_arc.lock_or_recover();
                            if let Some(msg) = mq.messages.pop_front() {
                                if pqmsg_ptr != 0 {
                                    self.guest_write::<u32>(pqmsg_ptr, msg.hwnd);
                                    self.guest_write::<u32>(pqmsg_ptr + 4, msg.msg);
                                    self.guest_write::<u32>(pqmsg_ptr + 8, msg.mp1);
                                    self.guest_write::<u32>(pqmsg_ptr + 12, msg.mp2);
                                    self.guest_write::<u32>(pqmsg_ptr + 16, msg.time);
                                    self.guest_write::<i16>(pqmsg_ptr + 20, msg.x);
                                    self.guest_write::<i16>(pqmsg_ptr + 22, msg.y);
                                }
                                if msg.msg == WM_QUIT { return ApiResult::Normal(0); }
                                return ApiResult::Normal(1);
                            }
                        }
                        // Block on condvar instead of spinning
                        let guard = wait_lock.lock_or_recover();
                        let _ = cond.wait_timeout(guard, std::time::Duration::from_millis(100)).unwrap();
                    }
                }
                ApiResult::Normal(0)
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
                self.dispatch_builtin_control(hwnd, msg, mp1, mp2)
            }
            919 => {
                // WinPostMsg
                let hwnd = read_stack(4);
                let msg = read_stack(8);
                let mp1 = read_stack(12);
                let mp2 = read_stack(16);
                let wm = self.shared.window_mgr.lock_or_recover();
                let hmq = wm.find_hmq_for_hwnd(hwnd);
                if let Some(hmq) = hmq
                    && let Some(mq_arc) = wm.get_mq(hmq) {
                        let mut mq = mq_arc.lock_or_recover();
                        mq.messages.push_back(OS2Message {
                            hwnd, msg, mp1, mp2, time: 0, x: 0, y: 0,
                        });
                        // Wake WinGetMsg if it's waiting on the condvar
                        mq.cond.notify_one();
                }
                ApiResult::Normal(1)
            }
            920 => {
                // WinSendMsg - synchronous, needs callback
                let hwnd = read_stack(4);
                let msg = read_stack(8);
                let mp1 = read_stack(12);
                let mp2 = read_stack(16);

                let pfn_wp = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    wm.get_window(hwnd).map(|w| w.pfn_wp).unwrap_or(0)
                };

                if pfn_wp != 0 {
                    return ApiResult::Callback {
                        wnd_proc: pfn_wp,
                        hwnd,
                        msg,
                        mp1,
                        mp2,
                    };
                }
                // No guest window proc — route to built-in control handler
                // (handles LM_INSERTITEM, LM_QUERYITEMCOUNT, etc.)
                self.dispatch_builtin_control(hwnd, msg, mp1, mp2)
            }
            911 => {
                // WinDefWindowProc
                let hwnd = read_stack(4);
                let msg = read_stack(8);
                let _mp1 = read_stack(12);
                let _mp2 = read_stack(16);

                if msg == WM_CLOSE {
                    // Post WM_QUIT to the message queue
                    self.post_wm_quit(hwnd);
                }
                ApiResult::Normal(0)
            }
            703 => {
                // WinBeginPaint
                let hwnd = read_stack(4);
                let _hps = read_stack(8);
                let _prcl_ptr = read_stack(12);
                let hps = self.shared.window_mgr.lock_or_recover().create_ps(hwnd);
                ApiResult::Normal(hps)
            }
            738 => {
                // WinEndPaint
                let hps = read_stack(4);
                let wm = self.shared.window_mgr.lock_or_recover();
                let ps_hwnd = wm.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0);
                let frame_hwnd = wm.client_to_frame(ps_hwnd);
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::PresentBuffer { handle: frame_hwnd });
                }
                ApiResult::Normal(1)
            }
            753 => {
                // WinGetLastError
                ApiResult::Normal(0)
            }
            789 => {
                // WinMessageBox(HWND hwndParent, HWND hwndOwner, PCSZ pszText,
                //               PCSZ pszCaption, ULONG idWindow, ULONG flStyle)
                //
                // Shows a modal dialog via the GUI thread and blocks the vCPU
                // thread until the user dismisses it.  In headless mode the GUI
                // thread replies immediately with MBID_OK.
                let _hwnd_parent    = read_stack(4);
                let _hwnd_owner     = read_stack(8);
                let psz_text_ptr    = read_stack(12);
                let psz_caption_ptr = read_stack(16);
                let _id             = read_stack(20);
                let style           = read_stack(24);

                let text    = self.read_guest_string(psz_text_ptr);
                let caption = self.read_guest_string(psz_caption_ptr);
                info!("  [VCPU {}] WinMessageBox '{}' : '{}'", vcpu_id, caption, text);

                // Grab the sender while holding the lock for the shortest time.
                let gui_tx = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    wm.gui_tx.clone()
                };

                if let Some(ref sender) = gui_tx {
                    // Rendezvous channel: capacity 1 so the send in the GUI
                    // thread never blocks even if we time out.
                    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel::<u32>(1);
                    if sender.send(GUIMessage::ShowMessageBox {
                        caption, text, style, reply_tx,
                    }).is_ok() {
                        // Block until user clicks a button (or fall back to OK on error).
                        let mbid = reply_rx.recv().unwrap_or(MBID_OK);
                        return ApiResult::Normal(mbid);
                    }
                }
                ApiResult::Normal(MBID_OK)
            }
            883 => {
                // WinShowWindow(HWND hwnd, BOOL fShow)
                let hwnd = read_stack(4);
                let show = read_stack(8) != 0;
                debug!("  [VCPU {}] WinShowWindow hwnd={} show={}", vcpu_id, hwnd, show);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    win.visible = show;
                }
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::ShowWindow { handle: hwnd, show });
                }
                ApiResult::Normal(1)
            }
            840 => {
                // WinQueryWindowRect(HWND hwnd, PRECTL prcl)
                let hwnd = read_stack(4);
                let prcl_ptr = read_stack(8);
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
                // WinDestroyWindow
                ApiResult::Normal(1)
            }
            884 => {
                // WinStartTimer(HAB hab, HWND hwnd, ULONG idTimer, ULONG dtTimeout)
                let _hab = read_stack(4);
                let hwnd = read_stack(8);
                let id_timer = read_stack(12);
                let dt_timeout = read_stack(16);
                debug!("  [VCPU {}] WinStartTimer: hwnd={}, id={}, timeout={}ms", vcpu_id, hwnd, id_timer, dt_timeout);

                let running = Arc::new(AtomicBool::new(true));
                let running_clone = running.clone();
                let shared = Arc::clone(&self.shared);

                {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    // Stop any existing timer with the same id
                    if let Some((old_flag, old_handle)) = wm.timers.remove(&(hwnd, id_timer)) {
                        old_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                        if let Some(h) = old_handle { let _ = h.join(); }
                    }
                }

                let timeout = std::time::Duration::from_millis(dt_timeout as u64);
                let join_handle = thread::spawn(move || {
                    while running_clone.load(std::sync::atomic::Ordering::Relaxed)
                        && !shared.exit_requested.load(std::sync::atomic::Ordering::Relaxed) {
                        thread::sleep(timeout);
                        if !running_clone.load(std::sync::atomic::Ordering::Relaxed) { break; }
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
                    wm.timers.insert((hwnd, id_timer), (running, Some(join_handle)));
                }
                ApiResult::Normal(id_timer) // Return the timer ID
            }
            885 => {
                // WinStopTimer(HAB hab, HWND hwnd, ULONG idTimer)
                let _hab = read_stack(4);
                let hwnd = read_stack(8);
                let id_timer = read_stack(12);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some((running, handle)) = wm.timers.remove(&(hwnd, id_timer)) {
                    running.store(false, std::sync::atomic::Ordering::Relaxed);
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
            907 => {
                // WinCreateMenu(HWND hwndParent, PVOID pvmt)
                // Stub - return a fake menu handle
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let h = wm.create_window("#Menu".to_string(), read_stack(4), 0);
                ApiResult::Normal(h)
            }
            910 => {
                // WinDefDlgProc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2)
                // Default dialog procedure - delegate to WinDefWindowProc behavior
                let _hwnd = read_stack(4);
                let msg = read_stack(8);
                let _mp1 = read_stack(12);
                let _mp2 = read_stack(16);
                if msg == WM_CLOSE {
                    self.post_wm_quit(_hwnd);
                }
                ApiResult::Normal(0)
            }
            729 => {
                // WinDismissDlg(HWND hwndDlg, ULONG usResult)
                let hwnd = read_stack(4);
                let _result = read_stack(8);
                // Post WM_QUIT to dismiss the dialog's message loop
                self.post_wm_quit(hwnd);
                ApiResult::Normal(1)
            }
            923 => {
                // WinDlgBox(HWND hwndParent, HWND hwndOwner, PFNWP pfnDlgProc, HMODULE hmod, ULONG idDlg, PVOID pCreateParams)
                let _hwnd_parent = read_stack(4);
                let _hwnd_owner = read_stack(8);
                let _pfn_dlg_proc = read_stack(12);
                let _hmod = read_stack(16);
                let id_dlg = read_stack(20);
                debug!("  [VCPU {}] WinDlgBox: idDlg={} (dialog template parsing deferred)", vcpu_id, id_dlg);
                ApiResult::Normal(1) // DID_OK
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
                let wm = self.shared.window_mgr.lock_or_recover();
                let ps_hwnd = wm.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0);
                let frame_hwnd = wm.client_to_frame(ps_hwnd);
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::DrawBox {
                        handle: frame_hwnd,
                        x1: x_left, y1: y_bottom, x2: x_right, y2: y_top,
                        color, fill: true,
                    });
                }
                ApiResult::Normal(1)
            }
            765 => {
                // WinInvalidateRect(HWND hwnd, PRECTL prcl, BOOL fIncludeChildren)
                let hwnd = read_stack(4);
                // Post WM_PAINT to trigger repaint
                let wm = self.shared.window_mgr.lock_or_recover();
                let target = wm.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
                let hmq = wm.find_hmq_for_hwnd(target);
                if let Some(hmq) = hmq
                    && let Some(mq_arc) = wm.get_mq(hmq) {
                        let mut mq = mq_arc.lock_or_recover();
                        mq.messages.push_back(OS2Message {
                            hwnd: target, msg: WM_PAINT, mp1: 0, mp2: 0,
                            time: 0, x: 0, y: 0,
                        });
                        mq.cond.notify_one();
                }
                ApiResult::Normal(1)
            }
            776 => {
                // WinLoadAccelTable(HAB hab, HMODULE hmod, ULONG idAccelTable)
                let _hab = read_stack(4);
                let _hmod = read_stack(8);
                let id = read_stack(12);
                debug!("  [VCPU {}] WinLoadAccelTable id={}", vcpu_id, id);
                let res_mgr = self.shared.resource_mgr.lock_or_recover();
                if let Some((guest_addr, size)) = res_mgr.find(RT_ACCELTABLE, id as u16) {
                    drop(res_mgr);
                    // Parse binary accelerator table: u16 count at offset 2, then entries of 6 bytes each
                    if size >= 4 {
                        let count = self.guest_read::<u16>(guest_addr + 2).unwrap_or(0) as usize;
                        let mut entries = Vec::new();
                        for i in 0..count {
                            let entry_off = guest_addr + 4 + (i as u32) * 6;
                            if entry_off + 6 > guest_addr + size { break; }
                            let flags = self.guest_read::<u16>(entry_off).unwrap_or(0);
                            let key = self.guest_read::<u16>(entry_off + 2).unwrap_or(0);
                            let cmd = self.guest_read::<u16>(entry_off + 4).unwrap_or(0);
                            entries.push(super::pm_types::AccelEntry { flags, key, cmd });
                        }
                        let mut wm = self.shared.window_mgr.lock_or_recover();
                        let haccel = wm.add_accel_table(entries);
                        ApiResult::Normal(haccel)
                    } else {
                        ApiResult::Normal(0)
                    }
                } else {
                    debug!("  [VCPU {}] WinLoadAccelTable: resource not found for id={}", vcpu_id, id);
                    ApiResult::Normal(0)
                }
            }
            924 => {
                // WinLoadDlg(HWND hwndParent, HWND hwndOwner, PFNWP pfnDlgProc, HMODULE hmod, ULONG idDlg, PVOID pCreateParams)
                let hwnd_parent = read_stack(4);
                let _hwnd_owner = read_stack(8);
                let _pfn_dlg_proc = read_stack(12);
                let _hmod = read_stack(16);
                let id_dlg = read_stack(20);
                debug!("  [VCPU {}] WinLoadDlg: parent={} idDlg={} (dialog template parsing deferred)", vcpu_id, hwnd_parent, id_dlg);
                // Create a placeholder dialog window
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                let h = wm.create_window("#Dialog".to_string(), hwnd_parent, hmq);
                ApiResult::Normal(h)
            }
            778 => {
                // WinLoadMenu(HWND hwndFrame, HMODULE hmod, ULONG idMenu)
                let hwnd_frame = read_stack(4);
                let _hmod = read_stack(8);
                let id_menu = read_stack(12);
                debug!("  [VCPU {}] WinLoadMenu hwnd_frame={} id={}", vcpu_id, hwnd_frame, id_menu);
                let res_mgr = self.shared.resource_mgr.lock_or_recover();
                let has_resource = res_mgr.find(RT_MENU, id_menu as u16).is_some();
                drop(res_mgr);
                // Create a menu window regardless; actual template parsing deferred
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let h = wm.create_window("#Menu".to_string(), hwnd_frame, 0);
                if has_resource {
                    debug!("  [VCPU {}] WinLoadMenu: created menu hwnd={} from resource", vcpu_id, h);
                } else {
                    debug!("  [VCPU {}] WinLoadMenu: created empty menu hwnd={} (no resource found)", vcpu_id, h);
                }
                ApiResult::Normal(h)
            }
            781 => {
                // WinLoadString(HAB hab, HMODULE hmod, ULONG id, LONG cchMax, PSZ pszBuffer)
                let _hab = read_stack(4);
                let _hmod = read_stack(8);
                let id = read_stack(12);
                let cch_max = read_stack(16) as i32;
                let psz_buffer = read_stack(20);
                debug!("  [VCPU {}] WinLoadString id={} cchMax={}", vcpu_id, id, cch_max);

                // OS/2 string tables are grouped in bundles of 16.
                // Resource ID = (string_id / 16) + 1, index within bundle = string_id % 16
                let bundle_id = (id / 16) + 1;
                let string_index = (id % 16) as usize;
                let res_mgr = self.shared.resource_mgr.lock_or_recover();
                if let Some((guest_addr, size)) = res_mgr.find(RT_STRING, bundle_id as u16) {
                    drop(res_mgr);
                    // Parse bundle: sequential length-prefixed strings (1-byte length + data)
                    let mut offset = 0u32;
                    for idx in 0..16 {
                        if offset >= size { break; }
                        let len = self.guest_read::<u8>(guest_addr + offset).unwrap_or(0) as u32;
                        offset += 1;
                        if idx == string_index {
                            let copy_len = len.min((cch_max.max(0) as u32).saturating_sub(1));
                            if psz_buffer != 0 && copy_len > 0 {
                                for i in 0..copy_len {
                                    let b = self.guest_read::<u8>(guest_addr + offset + i).unwrap_or(0);
                                    self.guest_write::<u8>(psz_buffer + i, b);
                                }
                                self.guest_write::<u8>(psz_buffer + copy_len, 0);
                            } else if psz_buffer != 0 {
                                self.guest_write::<u8>(psz_buffer, 0);
                            }
                            return ApiResult::Normal(copy_len);
                        }
                        offset += len;
                    }
                    // String index not found in bundle
                    if psz_buffer != 0 {
                        self.guest_write::<u8>(psz_buffer, 0);
                    }
                    ApiResult::Normal(0)
                } else {
                    drop(res_mgr);
                    debug!("  [VCPU {}] WinLoadString: string bundle {} not found", vcpu_id, bundle_id);
                    if psz_buffer != 0 {
                        self.guest_write::<u8>(psz_buffer, 0);
                    }
                    ApiResult::Normal(0)
                }
            }
            793 => {
                // WinOpenClipbrd(HAB hab)
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.clipboard_open = true;
                ApiResult::Normal(1)
            }
            937 => {
                // WinPopupMenu(HWND hwndParent, HWND hwndOwner, HWND hwndMenu, LONG x, LONG y, LONG idItem, ULONG fs)
                // Stub
                debug!("  [VCPU {}] WinPopupMenu (stub)", vcpu_id);
                ApiResult::Normal(1)
            }
            796 => {
                // WinProcessDlg(HWND hwndDlg)
                // Stub - return DID_OK (1)
                ApiResult::Normal(1)
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
                let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
                let wm = self.shared.window_mgr.lock_or_recover();
                let text = wm.find_child_by_id(hwnd_dlg, id_item)
                    .and_then(|h| wm.get_window(h))
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
                // WinQuerySysPointer(HWND hwndDesktop, LONG iptr, BOOL fLoad)
                // Return a fake pointer handle
                ApiResult::Normal(MOCK_HPOINTER)
            }
            829 => {
                // WinQuerySysValue(HWND hwndDesktop, LONG iSysValue)
                let _hwnd = read_stack(4);
                let sys_val = read_stack(8) as i32;
                let result = match sys_val {
                    20 => 640,   // SV_CXSCREEN
                    21 => 480,   // SV_CYSCREEN
                    22 => 640,   // SV_CXFULLSCREEN
                    23 => 460,   // SV_CYFULLSCREEN (minus title bar)
                    24 => 20,    // SV_CYTITLEBAR
                    27 => 1,     // SV_CXSIZEBORDER
                    28 => 1,     // SV_CYSIZEBORDER
                    _ => 0,
                };
                ApiResult::Normal(result as u32)
            }
            841 => {
                // WinQueryWindowText(HWND hwnd, LONG cchBufferMax, PCH pchBuffer)
                let hwnd = read_stack(4);
                let cch_max = read_stack(8) as usize;
                let buffer_ptr = read_stack(12);
                let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
                let wm = self.shared.window_mgr.lock_or_recover();
                let text = wm.get_window(hwnd).map(|w| w.text.clone()).unwrap_or_default();
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
                let wm = self.shared.window_mgr.lock_or_recover();
                let result = match code {
                    5 => { // QW_PARENT
                        wm.get_window(hwnd).map(|w| w.parent).unwrap_or(0)
                    }
                    6 => { // QW_OWNER
                        wm.get_window(hwnd).map(|w| w.parent).unwrap_or(0)
                    }
                    _ => 0,
                };
                ApiResult::Normal(result)
            }
            843 => {
                // WinQueryWindowULong(HWND hwnd, LONG index)
                let hwnd = read_stack(4);
                let index = read_stack(8) as i32;
                let wm = self.shared.window_mgr.lock_or_recover();
                let val = wm.get_window(hwnd)
                    .and_then(|w| w.window_ulong.get(&index))
                    .copied()
                    .unwrap_or(0);
                ApiResult::Normal(val)
            }
            844 => {
                // WinQueryWindowUShort(HWND hwnd, LONG index)
                let hwnd = read_stack(4);
                let index = read_stack(8) as i32;
                let wm = self.shared.window_mgr.lock_or_recover();
                let val = wm.get_window(hwnd)
                    .and_then(|w| w.window_ushort.get(&index))
                    .copied()
                    .unwrap_or(0);
                ApiResult::Normal(val as u32)
            }
            903 => {
                // WinSendDlgItemMsg(HWND hwndDlg, ULONG idItem, ULONG msg, MPARAM mp1, MPARAM mp2)
                // Stub - would need to find child and dispatch
                ApiResult::Normal(0)
            }
            850 => {
                // WinSetAccelTable(HAB hab, HACCEL haccel, HWND hwnd)
                let _hab = read_stack(4);
                let haccel = read_stack(8);
                let hwnd = read_stack(12);
                debug!("  [VCPU {}] WinSetAccelTable haccel={} hwnd={}", vcpu_id, haccel, hwnd);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.set_window_accel(hwnd, haccel);
                ApiResult::Normal(1)
            }
            852 => {
                // WinSetCapture(HWND hwndDesktop, HWND hwnd)
                let _hwnd_desktop = read_stack(4);
                let hwnd = read_stack(8);
                debug!("  [VCPU {}] WinSetCapture hwnd={}", vcpu_id, hwnd);
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
                    let text = self.read_guest_string(data);
                    debug!("  [VCPU {}] WinSetClipbrdData CF_TEXT: {:?}", vcpu_id, &text);
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
                let text = self.read_guest_string(psz_text);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(child_hwnd) = wm.find_child_by_id(hwnd_dlg, id_item)
                    && let Some(win) = wm.get_window_mut(child_hwnd) {
                        win.text = text;
                }
                ApiResult::Normal(1)
            }
            866 => {
                // WinSetPointer(HWND hwndDesktop, HPOINTER hptrNew)
                // Stub - cursor changing not supported
                ApiResult::Normal(1)
            }
            875 => {
                // WinSetWindowPos(HWND hwnd, HWND hwndInsertBehind, LONG x, LONG y, LONG cx, LONG cy, ULONG fl)
                let hwnd = read_stack(4);
                let _hwnd_behind = read_stack(8);
                let x = read_stack(12) as i32;
                let y = read_stack(16) as i32;
                let cx = read_stack(20) as i32;
                let cy = read_stack(24) as i32;
                let fl = read_stack(28);
                debug!("  [VCPU {}] WinSetWindowPos hwnd={} x={} y={} cx={} cy={} fl=0x{:04X}", vcpu_id, hwnd, x, y, cx, cy, fl);

                let mut wm = self.shared.window_mgr.lock_or_recover();

                // Update the OS2Window position/size state
                if let Some(win) = wm.get_window_mut(hwnd) {
                    if fl & SWP_MOVE != 0 {
                        win.x = x;
                        win.y = y;
                    }
                    if fl & SWP_SIZE != 0 {
                        win.cx = cx;
                        win.cy = cy;
                    }
                    if fl & SWP_SHOW != 0 {
                        win.visible = true;
                    }
                    if fl & SWP_HIDE != 0 {
                        win.visible = false;
                    }
                }

                // Send GUI messages for the actual window operations
                if let Some(ref sender) = wm.gui_tx {
                    if fl & SWP_SIZE != 0 {
                        let _ = sender.send(GUIMessage::ResizeWindow {
                            handle: hwnd, width: cx as u32, height: cy as u32,
                        });
                    }
                    if fl & SWP_MOVE != 0 {
                        let _ = sender.send(GUIMessage::MoveWindow {
                            handle: hwnd, x, y,
                        });
                    }
                    if fl & SWP_SHOW != 0 {
                        let _ = sender.send(GUIMessage::ShowWindow { handle: hwnd, show: true });
                    }
                    if fl & SWP_HIDE != 0 {
                        let _ = sender.send(GUIMessage::ShowWindow { handle: hwnd, show: false });
                    }
                }

                ApiResult::Normal(1)
            }
            877 => {
                // WinSetWindowText(HWND hwnd, PCSZ pszText)
                let hwnd = read_stack(4);
                let psz_text = read_stack(8);
                let text = self.read_guest_string(psz_text);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    win.text = text;
                }
                ApiResult::Normal(1)
            }
            878 => {
                // WinSetWindowULong(HWND hwnd, LONG index, ULONG ul)
                let hwnd = read_stack(4);
                let index = read_stack(8) as i32;
                let value = read_stack(12);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    win.window_ulong.insert(index, value);
                }
                ApiResult::Normal(1)
            }
            879 => {
                // WinSetWindowUShort(HWND hwnd, LONG index, USHORT us)
                let hwnd = read_stack(4);
                let index = read_stack(8) as i32;
                let value = read_stack(12) as u16;
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    win.window_ushort.insert(index, value);
                }
                ApiResult::Normal(1)
            }
            904 => {
                // WinTranslateAccel(HAB hab, HWND hwnd, HACCEL haccel, PQMSG pqmsg)
                let _hab = read_stack(4);
                let hwnd = read_stack(8);
                let _haccel = read_stack(12);
                let pqmsg = read_stack(16);
                if pqmsg == 0 { return ApiResult::Normal(0); }
                let msg = self.guest_read::<u32>(pqmsg + 4).unwrap_or(0);
                if msg == WM_CHAR {
                    let mp1 = self.guest_read::<u32>(pqmsg + 8).unwrap_or(0);
                    let mp2 = self.guest_read::<u32>(pqmsg + 12).unwrap_or(0);
                    let flags = (mp1 & 0xFFFF) as u16;
                    let key = (mp2 & 0xFFFF) as u16;
                    let wm = self.shared.window_mgr.lock_or_recover();
                    if let Some(cmd) = wm.translate_accel(hwnd, key, flags) {
                        drop(wm);
                        // Rewrite message as WM_COMMAND
                        self.guest_write::<u32>(pqmsg + 4, WM_COMMAND);
                        self.guest_write::<u32>(pqmsg + 8, cmd as u32); // mp1 = command id
                        self.guest_write::<u32>(pqmsg + 12, 0);         // mp2 = 0
                        return ApiResult::Normal(1); // TRUE - translated
                    }
                }
                ApiResult::Normal(0) // FALSE - not translated
            }
            892 => {
                // WinUpdateWindow(HWND hwnd)
                // Trigger a present buffer
                let hwnd = read_stack(4);
                let wm = self.shared.window_mgr.lock_or_recover();
                let frame_hwnd = wm.client_to_frame(hwnd);
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::PresentBuffer { handle: frame_hwnd });
                }
                ApiResult::Normal(1)
            }
            899 => {
                // WinWindowFromID(HWND hwndParent, ULONG id)
                let hwnd_parent = read_stack(4);
                let id = read_stack(8);
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
                let x               = read_stack(20) as i32;
                let y               = read_stack(24) as i32;
                let cx              = read_stack(28) as i32;
                let cy              = read_stack(32) as i32;
                // hwndOwner (+36), hwndInsertBehind (+40) — ignored for now
                let id              = read_stack(44);
                // pCtlData (+48), pPresParams (+52) — ignored

                // MAKEINTATOM atoms have high word 0xFFFF; also accept bare
                // small integers (< 0x10000) for robustness.
                let class_name = if (psz_class & 0xFFFF_0000) == 0xFFFF_0000 || psz_class < 0x10000 {
                    resolve_class_atom(psz_class, String::new())
                } else {
                    resolve_class_atom(psz_class, self.read_guest_string(psz_class))
                };
                let text = if psz_name != 0 { self.read_guest_string(psz_name) } else { String::new() };
                debug!("  [VCPU {}] WinCreateWindow class='{}' text='{}' parent={} ({},{}) {}x{} id={} style=0x{:08X}",
                       vcpu_id, class_name, text, hwnd_parent, x, y, cx, cy, id, fl_style);

                let hwnd = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    let h = wm.create_window(class_name.clone(), hwnd_parent, hmq);
                    if let Some(win) = wm.get_window_mut(h) {
                        win.text = text;
                        win.x = x; win.y = y;
                        win.cx = cx; win.cy = cy;
                        win.id = id;
                        win.style = fl_style;
                        win.visible = fl_style & WS_VISIBLE != 0;
                    }
                    h
                };

                // Post WM_PAINT so the control draws itself on creation.
                if fl_style & WS_VISIBLE != 0 {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    if let Some(mq_arc) = wm.get_mq(hmq) {
                        let mut mq = mq_arc.lock_or_recover();
                        mq.messages.push_back(OS2Message {
                            hwnd, msg: WM_PAINT, mp1: 0, mp2: 0, time: 0, x: 0, y: 0,
                        });
                        mq.cond.notify_one();
                    }
                }
                ApiResult::Normal(hwnd)
            }
            895 => {
                // WinSubclassWindow(HWND hwnd, PFNWP pfnwp) → PFNWP (old proc)
                //
                // Replace hwnd's window procedure with pfnwp and return the
                // previous one.  The caller is responsible for chaining.
                let hwnd   = read_stack(4);
                let pfn_wp = read_stack(8);
                debug!("  [VCPU {}] WinSubclassWindow hwnd={} pfn_wp=0x{:08X}", vcpu_id, hwnd, pfn_wp);
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
                // Toggles the WS_DISABLED style bit.  Returns the previous
                // enabled state (TRUE = was enabled before this call).
                let hwnd    = read_stack(4);
                let enable  = read_stack(8) != 0;
                debug!("  [VCPU {}] WinEnableWindow hwnd={} enable={}", vcpu_id, hwnd, enable);

                let was_enabled = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    if let Some(win) = wm.get_window_mut(hwnd) {
                        let prev = win.style & WS_DISABLED == 0; // was enabled?
                        if enable {
                            win.style &= !WS_DISABLED;
                        } else {
                            win.style |= WS_DISABLED;
                        }
                        prev
                    } else {
                        true // non-existent window: treat as already enabled
                    }
                };

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
                ApiResult::Normal(was_enabled as u32)
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
                let wm = self.shared.window_mgr.lock_or_recover();
                let enabled = wm.get_window(hwnd)
                    .map(|w| w.style & WS_DISABLED == 0)
                    .unwrap_or(false);
                ApiResult::Normal(enabled as u32)
            }
            837 => {
                // WinQueryWindowPos(HWND hwnd, PSWP pswp) -> BOOL
                //
                // Fills in a SWP structure (28 bytes) with the window's current
                // position and size.
                //
                //   LONG   fl;               +0  SWP_MOVE | SWP_SIZE
                //   LONG   cy;               +4  height
                //   LONG   cx;               +8  width
                //   LONG   y;                +12 y position
                //   LONG   x;                +16 x position
                //   HWND   hwndInsertBehind; +20 (always 0 here)
                //   HWND   hwnd;             +24 window handle
                let hwnd     = read_stack(4);
                let pswp_ptr = read_stack(8);
                if pswp_ptr == 0 {
                    return ApiResult::Normal(0); // FALSE — null pointer
                }
                let wm = self.shared.window_mgr.lock_or_recover();
                let (x, y, cx, cy) = wm.get_window(hwnd)
                    .map(|w| (w.x, w.y, w.cx, w.cy))
                    .unwrap_or((0, 0, 0, 0));
                drop(wm);
                // fl: report both position and size are valid
                self.guest_write::<u32>(pswp_ptr,      SWP_MOVE | SWP_SIZE); // fl
                self.guest_write::<i32>(pswp_ptr +  4, cy);                  // cy
                self.guest_write::<i32>(pswp_ptr +  8, cx);                  // cx
                self.guest_write::<i32>(pswp_ptr + 12, y);                   // y
                self.guest_write::<i32>(pswp_ptr + 16, x);                   // x
                self.guest_write::<u32>(pswp_ptr + 20, 0);                   // hwndInsertBehind
                self.guest_write::<u32>(pswp_ptr + 24, hwnd);                // hwnd
                ApiResult::Normal(1) // TRUE
            }
            _ => {
                warn!("Warning: Unknown PMWIN Ordinal {} on VCPU {}", ordinal, vcpu_id);
                ApiResult::Normal(0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Loader, ApiResult};
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::mutex_ext::MutexExt;
    use std::sync::atomic::Ordering;

    fn write_stack(loader: &Loader, esp: u32, args: &[u32]) {
        for (i, &a) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, a).unwrap();
        }
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
}
