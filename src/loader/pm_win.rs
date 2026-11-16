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
fn bound_window_geometry(x: i32, y: i32, cx: i32, cy: i32) -> (i32, i32, i32, i32) {
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
use crate::gui::surface::{DrawOp, Mix};
use super::pm_types::{DeviceContext, PsOrigin, TimerStop};
use crate::lx::header::{RT_STRING, RT_MENU, RT_DIALOG, RT_ACCELTABLE};
use super::pm_types::MenuItem;

/// Parsed metadata for a single DLGITEMTEMPLATE entry.
#[derive(Clone)]
struct DlgItemInfo {
    x: i16, y: i16, cx: i16, cy: i16,
    id: u16, fl_style: u32,
    class_name: String,
    text: String,
}

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
                // The queue codepage starts as the process codepage.
                let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
                let hmq = wm.create_mq(cp);
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
                let pfc_flags_ptr = read_stack(12);
                let psz_class_name_ptr = read_stack(16);
                let psz_title_ptr = read_stack(20);
                let _client_style = read_stack(24);
                let _hmod = read_stack(28);
                let id_resources = read_stack(32);
                let phwnd_client_ptr = read_stack(36);
                let class_name = self.read_guest_string(psz_class_name_ptr);
                let title = if psz_title_ptr != 0 { self.read_guest_string(psz_title_ptr) } else { "Warpine Window".to_string() };
                // VDR-C1: read the flCreateFlags (FCF_* bits) from the PULONG argument.
                let fl_create_flags = if pfc_flags_ptr != 0 {
                    self.guest_read::<u32>(pfc_flags_ptr).unwrap_or(0)
                } else { 0 };
                debug!("  [VCPU {}] WinCreateStdWindow: class='{}', title='{}', parent=0x{:08X}, style=0x{:08X}, fcf=0x{:08X}", vcpu_id, class_name, title, parent, style, fl_create_flags);

                // FCF_MENU: the frame loads its menu bar from the RT_MENU resource
                // whose id is the idResources argument. A missing resource simply
                // leaves the frame without a menu.
                let fcf_menu_items = if fl_create_flags & FCF_MENU != 0 {
                    self.load_menu_resource(id_resources, vcpu_id)
                } else { None };

                let (h_frame, h_client, pfn_wp_client) = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    let h_frame = wm.create_window(class_name.clone(), parent, hmq);
                    let h_client = wm.create_window(class_name.clone(), h_frame, hmq);
                    wm.frame_to_client.insert(h_frame, h_client);
                    // Initialise both windows to the default SDL2 window size so that
                    // WinQueryWindowRect returns correct dimensions before the first resize.
                    // Default placement: cascade from the desktop's top-left corner
                    // so successive windows do not stack exactly on top of each other.
                    let (def_x, def_y) = default_frame_pos(
                        wm.frame_to_client.len() - 1, FRAME_DEFAULT_CX, FRAME_DEFAULT_CY, desktop_size());
                    if let Some(win) = wm.get_window_mut(h_frame)  {
                        win.x = def_x; win.y = def_y;
                        win.cx = FRAME_DEFAULT_CX; win.cy = FRAME_DEFAULT_CY;
                        win.frame_flags = fl_create_flags;
                        win.visible = style & WS_VISIBLE != 0;
                        win.text = title.clone();
                    }
                    if let Some(items) = fcf_menu_items {
                        let h_menu = wm.create_window("#Menu".to_string(), h_frame, 0);
                        if let Some(win) = wm.get_window_mut(h_menu) { win.menu_items = items; }
                        if let Some(win) = wm.get_window_mut(h_frame) { win.menu_hwnd = h_menu; }
                    }
                    // Client fills the frame below the title bar and menu bar.
                    wm.layout_frame(h_frame);
                    // VDR-B1/E1: register in Z-order stack (top-most) and set focus.
                    wm.z_push_top(h_frame);
                    wm.focused_hwnd = h_frame;

                    // The frame is composited once it has a surface.
                    wm.frame_surface(h_frame);

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
                let mp1 = read_stack(12);
                let _mp2 = read_stack(16);

                match msg {
                    WM_CLOSE => { self.post_wm_quit(hwnd); }
                    WM_BUTTON1DOWN | WM_BUTTON1UP => {
                        // If this frame has a menu, delegate mouse events to the menu
                        // control handler so it can handle menu-bar clicks and dropdown
                        // item selection.
                        //
                        // push_msg() translates frame_hwnd → client_hwnd via frame_to_client,
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
                        if menu_hwnd != 0 && msg == WM_BUTTON1DOWN {
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
                // The whole window is the update region.
                let hwnd = read_stack(4);
                let hps_in = read_stack(8);
                let prcl = read_stack(12);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let hps = if hps_in != 0 && wm.ps_map.contains_key(&hps_in) {
                    hps_in
                } else {
                    let h = wm.create_ps(hwnd);
                    if let Some(ps) = wm.ps_map.get_mut(&h) { ps.origin = PsOrigin::Paint; }
                    h
                };
                let (cx, cy) = wm.get_window(hwnd).map_or((0, 0), |w| (w.cx, w.cy));
                drop(wm);
                if prcl != 0 {
                    for (i, v) in [0, 0, cx, cy].into_iter().enumerate() {
                        self.guest_write::<i32>(prcl + 4 * i as u32, v);
                    }
                }
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
                    wm.ps_map.remove(&hps);
                }
                ApiResult::Normal(1)
            }
            757 => {
                // WinGetPS(HWND hwnd) → HPS (freed by WinReleasePS)
                let hwnd = read_stack(4);
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
                if ok { wm.ps_map.remove(&hps); }
                ApiResult::Normal(ok as u32)
            }
            794 => {
                // WinOpenWindowDC(HWND hwnd) → HDC (one per window)
                let hwnd = read_stack(4);
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
                wm.mark_dirty(hwnd); // VDR-B4
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
                // WinDestroyWindow(HWND hwnd)
                let hwnd = read_stack(4);
                {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    // If this is a frame, clean up Z-order and focus.
                    if wm.frame_to_client.contains_key(&hwnd) {
                        wm.z_remove(hwnd);
                        wm.frame_to_client.remove(&hwnd);
                        wm.surfaces.remove(&hwnd);
                    }
                }
                ApiResult::Normal(1)
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
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let h = wm.create_window("#Menu".to_string(), hwnd_parent, 0);
                if let Some(win) = wm.get_window_mut(h) {
                    win.menu_items = items;
                }
                ApiResult::Normal(h)
            }
            910 => {
                // WinDefDlgProc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2)
                let hwnd = read_stack(4);
                let msg  = read_stack(8);
                let mp1  = read_stack(12);
                let mp2  = read_stack(16);
                match msg {
                    WM_INITDLG => {
                        // Return FALSE (0): accept the focus set in mp1 as-is.
                        ApiResult::Normal(0)
                    }
                    WM_BUTTON1DOWN => {
                        // Route through the built-in #Dialog handler so it can
                        // hit-test child WC_BUTTON controls and post WM_COMMAND.
                        self.dispatch_builtin_control(hwnd, WM_BUTTON1DOWN, mp1, mp2)
                    }
                    WM_COMMAND => {
                        // If the command is DID_OK or DID_CANCEL, dismiss.
                        let id = mp1 & 0xFFFF;
                        if id == DID_OK || id == DID_CANCEL {
                            let mut wm = self.shared.window_mgr.lock_or_recover();
                            if let Some(win) = wm.get_window_mut(hwnd) {
                                win.dialog_dismissed = true;
                                win.dialog_result    = id;
                            }
                            let hmq = wm.find_hmq_for_hwnd(hwnd);
                            if let Some(hmq) = hmq
                                && let Some(mq_arc) = wm.get_mq(hmq) {
                                    mq_arc.lock_or_recover().cond.notify_one();
                            }
                        }
                        ApiResult::Normal(0)
                    }
                    WM_CLOSE => {
                        let mut wm = self.shared.window_mgr.lock_or_recover();
                        if let Some(win) = wm.get_window_mut(hwnd) {
                            win.dialog_dismissed = true;
                            win.dialog_result    = DID_CANCEL;
                        }
                        let hmq = wm.find_hmq_for_hwnd(hwnd);
                        if let Some(hmq) = hmq
                            && let Some(mq_arc) = wm.get_mq(hmq) {
                                mq_arc.lock_or_recover().cond.notify_one();
                        }
                        ApiResult::Normal(0)
                    }
                    _ => ApiResult::Normal(0)
                }
            }
            729 => {
                // WinDismissDlg(HWND hwndDlg, ULONG usResult)
                let hwnd   = read_stack(4);
                let result = read_stack(8);
                debug!("  [VCPU {}] WinDismissDlg hwnd={} result={}", vcpu_id, hwnd, result);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    win.dialog_dismissed = true;
                    win.dialog_result    = result;
                }
                // Wake the DlgRunLoop condvar so it notices the dismissal.
                let hmq = wm.find_hmq_for_hwnd(hwnd);
                if let Some(hmq) = hmq
                    && let Some(mq_arc) = wm.get_mq(hmq) {
                        mq_arc.lock_or_recover().cond.notify_one();
                }
                ApiResult::Normal(1)
            }
            923 => {
                // WinDlgBox(hwndParent, hwndOwner, pfnDlgProc, hmod, idDlg, pCreateParams)
                let hwnd_parent      = read_stack(4);
                let _hwnd_owner      = read_stack(8);
                let pfn_dlg_proc     = read_stack(12);
                let _hmod            = read_stack(16);
                let id_dlg           = read_stack(20);
                let p_create_params  = read_stack(24);
                debug!("  [VCPU {}] WinDlgBox: parent={} idDlg={} pfnDlgProc=0x{:08X}",
                       vcpu_id, hwnd_parent, id_dlg, pfn_dlg_proc);

                if pfn_dlg_proc == 0 {
                    return ApiResult::Normal(DID_ERROR);
                }

                // Parse the dialog template resource.
                let res = self.shared.resource_mgr.lock_or_recover()
                    .find(RT_DIALOG, id_dlg as u16);

                let (hwnd_dlg, hmq) = if let Some((guest_addr, size)) = res {
                    match self.parse_dlg_template(guest_addr, size) {
                        Some((dx, dy, dcx, dcy, title, items)) => {
                            debug!("  [VCPU {}] WinDlgBox: parsed {} items for idDlg={}",
                                   vcpu_id, items.len(), id_dlg);
                            self.create_dialog_from_template(
                                hwnd_parent, pfn_dlg_proc, vcpu_id,
                                title, dx, dy, dcx, dcy, items,
                            )
                        }
                        None => {
                            warn!("  [VCPU {}] WinDlgBox: failed to parse template for idDlg={}", vcpu_id, id_dlg);
                            let mut wm = self.shared.window_mgr.lock_or_recover();
                            let hmq2 = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                            let h = wm.create_window("#Dialog".to_string(), hwnd_parent, hmq2);
                            if let Some(win) = wm.get_window_mut(h) { win.pfn_wp = pfn_dlg_proc; }
                            (h, hmq2)
                        }
                    }
                } else {
                    debug!("  [VCPU {}] WinDlgBox: no RT_DIALOG resource for idDlg={}", vcpu_id, id_dlg);
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let hmq2 = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    let h = wm.create_window("#Dialog".to_string(), hwnd_parent, hmq2);
                    if let Some(win) = wm.get_window_mut(h) { win.pfn_wp = pfn_dlg_proc; }
                    (h, hmq2)
                };

                // Post WM_INITDLG (mp1 = first child hwnd, mp2 = pCreateParams).
                let first_child = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    wm.get_window(hwnd_dlg)
                        .and_then(|w| w.children.first().copied())
                        .unwrap_or(0)
                };
                if hmq != 0 {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    if let Some(mq_arc) = wm.get_mq(hmq) {
                        let mut mq = mq_arc.lock_or_recover();
                        mq.messages.push_back(OS2Message {
                            hwnd: hwnd_dlg, msg: WM_INITDLG,
                            mp1: first_child, mp2: p_create_params,
                            time: 0, x: 0, y: 0,
                        });
                        // WM_PAINT renders the dialog after WM_INITDLG is processed by the dialog proc.
                        mq.messages.push_back(OS2Message {
                            hwnd: hwnd_dlg, msg: WM_PAINT,
                            mp1: 0, mp2: 0, time: 0, x: 0, y: 0,
                        });
                        mq.cond.notify_one();
                    }
                }

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
                let frame_hwnd = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    wm.client_to_frame(wm.get_ps_hwnd(hps))
                };
                // RECTL is exclusive at the top and right.
                self.draw(frame_hwnd, DrawOp::Box {
                    x1: x_left, y1: y_bottom, x2: x_right - 1, y2: y_top - 1,
                    color, fill: true, mix: Mix::Overpaint,
                });
                ApiResult::Normal(1)
            }
            765 => {
                // WinInvalidateRect(HWND hwnd, PRECTL prcl, BOOL fIncludeChildren)
                let hwnd = read_stack(4);
                // Post WM_PAINT to trigger repaint
                let mut wm = self.shared.window_mgr.lock_or_recover();
                wm.mark_dirty(hwnd); // VDR-B4
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
                // WinLoadDlg(hwndParent, hwndOwner, pfnDlgProc, hmod, idDlg, pCreateParams)
                let hwnd_parent      = read_stack(4);
                let _hwnd_owner      = read_stack(8);
                let pfn_dlg_proc     = read_stack(12);
                let _hmod            = read_stack(16);
                let id_dlg           = read_stack(20);
                let p_create_params  = read_stack(24);
                debug!("  [VCPU {}] WinLoadDlg: parent={} idDlg={} pfnDlgProc=0x{:08X}",
                       vcpu_id, hwnd_parent, id_dlg, pfn_dlg_proc);

                let res = self.shared.resource_mgr.lock_or_recover()
                    .find(RT_DIALOG, id_dlg as u16);

                let (hwnd_dlg, hmq) = if let Some((guest_addr, size)) = res {
                    match self.parse_dlg_template(guest_addr, size) {
                        Some((dx, dy, dcx, dcy, title, items)) => {
                            self.create_dialog_from_template(
                                hwnd_parent, pfn_dlg_proc, vcpu_id,
                                title, dx, dy, dcx, dcy, items,
                            )
                        }
                        None => {
                            let mut wm = self.shared.window_mgr.lock_or_recover();
                            let hmq2 = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                            let h = wm.create_window("#Dialog".to_string(), hwnd_parent, hmq2);
                            if let Some(win) = wm.get_window_mut(h) { win.pfn_wp = pfn_dlg_proc; }
                            (h, hmq2)
                        }
                    }
                } else {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let hmq2 = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
                    let h = wm.create_window("#Dialog".to_string(), hwnd_parent, hmq2);
                    if let Some(win) = wm.get_window_mut(h) { win.pfn_wp = pfn_dlg_proc; }
                    (h, hmq2)
                };

                // Post WM_INITDLG so the first WinProcessDlg call dispatches it.
                let first_child = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    wm.get_window(hwnd_dlg)
                        .and_then(|w| w.children.first().copied())
                        .unwrap_or(0)
                };
                if hmq != 0 {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    if let Some(mq_arc) = wm.get_mq(hmq) {
                        let mut mq = mq_arc.lock_or_recover();
                        mq.messages.push_back(OS2Message {
                            hwnd: hwnd_dlg, msg: WM_INITDLG,
                            mp1: first_child, mp2: p_create_params,
                            time: 0, x: 0, y: 0,
                        });
                        // WM_PAINT renders the dialog after WM_INITDLG is processed.
                        mq.messages.push_back(OS2Message {
                            hwnd: hwnd_dlg, msg: WM_PAINT,
                            mp1: 0, mp2: 0, time: 0, x: 0, y: 0,
                        });
                        mq.cond.notify_one();
                    }
                }
                ApiResult::Normal(hwnd_dlg)
            }
            778 => {
                // WinLoadMenu(HWND hwndFrame, HMODULE hmod, ULONG idMenu)
                let hwnd_frame = read_stack(4);
                let _hmod      = read_stack(8);
                let id_menu    = read_stack(12);
                debug!("  [VCPU {}] WinLoadMenu hwnd_frame={} id={}", vcpu_id, hwnd_frame, id_menu);

                let items = self.load_menu_resource(id_menu, vcpu_id).unwrap_or_default();
                let h = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let h = wm.create_window("#Menu".to_string(), hwnd_frame, 0);
                    if let Some(win) = wm.get_window_mut(h) { win.menu_items = items; }
                    h
                };
                // Associate the menu with the frame; the compositor draws the bar.
                self.attach_menu_to_frame(hwnd_frame, h);
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
            851 => {
                // WinSetActiveWindow(HWND hwndDesktop, HWND hwnd) -> BOOL
                // VDR-E2: set focused_hwnd and raise to top of Z-order;
                // post WM_ACTIVATE to both the old and new active windows (VDR-D4).
                let _hwnd_desktop = read_stack(4);
                let hwnd = read_stack(8);
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
                    let wm = self.shared.window_mgr.lock_or_recover();
                    // Post WM_ACTIVATE to old frame (mp1=0 deactivating, mp2=new frame).
                    if prev != 0
                        && let Some(mq_arc) = wm.find_hmq_for_hwnd(prev).and_then(|h| wm.get_mq(h)) {
                        let mut mq = mq_arc.lock_or_recover();
                        mq.messages.push_back(OS2Message {
                            hwnd: prev, msg: WM_ACTIVATE, mp1: 0, mp2: new_frame,
                            time: 0, x: 0, y: 0,
                        });
                        mq.cond.notify_one();
                    }
                    // Post WM_ACTIVATE to new frame (mp1=1 activating, mp2=old frame).
                    if let Some(mq_arc) = wm.find_hmq_for_hwnd(new_frame).and_then(|h| wm.get_mq(h)) {
                        let mut mq = mq_arc.lock_or_recover();
                        mq.messages.push_back(OS2Message {
                            hwnd: new_frame, msg: WM_ACTIVATE, mp1: 1, mp2: prev,
                            time: 0, x: 0, y: 0,
                        });
                        mq.cond.notify_one();
                    }
                }
                ApiResult::Normal(if new_frame != 0 { 1 } else { 0 })
            }
            860 => {
                // WinSetFocus(HWND hwndDesktop, HWND hwndNewFocus) -> BOOL
                // The focus window receives keyboard input; NULLHANDLE means
                // no window has the focus (keys go to the active frame's client).
                let _hwnd_desktop = read_stack(4);
                let hwnd = read_stack(8);
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
                // Stub
                debug!("  [VCPU {}] WinPopupMenu (stub)", vcpu_id);
                ApiResult::Normal(1)
            }
            796 => {
                // WinProcessDlg(HWND hwndDlg) — run the modal dialog loop
                let hwnd_dlg = read_stack(4);
                debug!("  [VCPU {}] WinProcessDlg hwnd={}", vcpu_id, hwnd_dlg);
                let (pfn_wp, hmq) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let pfn = wm.get_window(hwnd_dlg).map(|w| w.pfn_wp).unwrap_or(0);
                    let hmq = wm.find_hmq_for_hwnd(hwnd_dlg).unwrap_or(0);
                    (pfn, hmq)
                };
                if pfn_wp == 0 || hmq == 0 {
                    return ApiResult::Normal(DID_CANCEL);
                }
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
                    20 => desktop_size().0,   // SV_CXSCREEN
                    21 => desktop_size().1,   // SV_CYSCREEN
                    22 => desktop_size().0,   // SV_CXFULLSCREEN
                    23 => desktop_size().1.saturating_sub(CHROME_TITLE_H as u32), // SV_CYFULLSCREEN (minus title bar)
                    24 => 20,    // SV_CYTITLEBAR
                    27 => 1,     // SV_CXSIZEBORDER
                    28 => 1,     // SV_CYSIZEBORDER
                    _ => 0,
                };
                ApiResult::Normal(result)
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
                let hwnd_dlg = read_stack(4);
                let id_item  = read_stack(8);
                let msg      = read_stack(12);
                let mp1      = read_stack(16);
                let mp2      = read_stack(20);
                let (child_hwnd, pfn_wp) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    if let Some(ch) = wm.find_child_by_id(hwnd_dlg, id_item) {
                        (ch, wm.get_window(ch).map(|w| w.pfn_wp).unwrap_or(0))
                    } else { (0, 0) }
                };
                if child_hwnd == 0 { return ApiResult::Normal(0); }
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
                let hwnd         = read_stack(4);
                let hwnd_behind  = read_stack(8);
                let (x, y, cx, cy) = bound_window_geometry(
                    read_stack(12) as i32, read_stack(16) as i32,
                    read_stack(20) as i32, read_stack(24) as i32);
                let fl = read_stack(28);
                debug!("  [VCPU {}] WinSetWindowPos hwnd={} behind={} x={} y={} cx={} cy={} fl=0x{:04X}",
                       vcpu_id, hwnd, hwnd_behind, x, y, cx, cy, fl);

                let mut wm = self.shared.window_mgr.lock_or_recover();

                // Update the OS2Window position/size state.
                if let Some(win) = wm.get_window_mut(hwnd) {
                    if fl & SWP_MOVE != 0 {
                        win.x = x;
                        win.y = y;
                    }
                    if fl & SWP_SIZE != 0 {
                        win.cx = cx;
                        win.cy = cy;
                    }
                    if fl & SWP_SHOW != 0 { win.visible = true; }
                    if fl & SWP_HIDE != 0 { win.visible = false; }
                }

                // VDR-B2: SWP_ZORDER — update the Z-order stack.
                if fl & SWP_ZORDER != 0 && wm.frame_to_client.contains_key(&hwnd) {
                    match hwnd_behind {
                        HWND_TOP | HWND_FLOAT => wm.z_push_top(hwnd),
                        HWND_BOTTOM           => wm.z_push_bottom(hwnd),
                        0                     => {} // no-op
                        behind                => wm.z_insert_behind(hwnd, behind),
                    }
                }

                // VDR-E1: SWP_ACTIVATE — update focused_hwnd.
                if fl & SWP_ACTIVATE != 0 && wm.frame_to_client.contains_key(&hwnd) {
                    wm.focused_hwnd = hwnd;
                    wm.z_push_top(hwnd);
                }

                // VDR-B4: any geometry/visibility/z-order/activation change → dirty.
                if fl & (SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_HIDE | SWP_ACTIVATE) != 0 {
                    wm.mark_dirty(hwnd);
                }
                // (SWP_ZORDER dirtiness is handled transitively via z_push_top/bottom/insert_behind)

                // A resized frame re-lays out its menu bar and client; the frame's
                // FrameBuffer holds client content only, so size it to the client.
                let (fb_cx, fb_cy) = if fl & SWP_SIZE != 0 && wm.frame_to_client.contains_key(&hwnd) {
                    wm.layout_frame(hwnd)
                } else { (cx, cy) };

                // Position and visibility are read by the compositor from
                // the window; only the surface has to follow a size change.
                if fl & SWP_SIZE != 0 {
                    wm.resize_surface(hwnd, fb_cx, fb_cy);
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
                wm.mark_dirty(hwnd); // VDR-B4: title bar text changed
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
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let frame_hwnd = wm.client_to_frame(hwnd);
                wm.mark_dirty(frame_hwnd);
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
                let (x, y, cx, cy)  = bound_window_geometry(
                    read_stack(20) as i32, read_stack(24) as i32,
                    read_stack(28) as i32, read_stack(32) as i32);
                let hwnd_owner      = read_stack(36);
                // hwndInsertBehind (+40) — ignored for now
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
                        win.owner = hwnd_owner;
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
            929 => {
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

    // ── Binary resource parser helpers ───────────────────────────────────────────

    /// Read a NUL-terminated guest string starting at `base + *offset`, advancing
    /// `*offset` past the terminating NUL.  Stops at `base + max_size`.
    fn read_nul_str_at(&self, base: u32, offset: &mut u32, max_size: u32) -> String {
        let mut s = String::new();
        loop {
            if *offset >= max_size { break; }
            let b = self.guest_read::<u8>(base + *offset).unwrap_or(0);
            *offset += 1;
            if b == 0 { break; }
            s.push(b as char);
        }
        s
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
    fn parse_menu_items(&self, base: u32, size: u32, offset: &mut u32, count: usize) -> Vec<MenuItem> {
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
                self.read_nul_str_at(base, offset, size)
            } else {
                String::new()
            };

            // Recurse into submenu: wrc prepends a 10-byte child-block header
            // (u32 total_size + u16 codepage + u16 reserved + u16 child_count).
            let children = if style & MIS_SUBMENU != 0 {
                if *offset + 10 <= size {
                    let child_count = self.guest_read::<u16>(base + *offset + 8).unwrap_or(0) as usize;
                    *offset += 10;
                    self.parse_menu_items(base, size, offset, child_count)
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            items.push(MenuItem { id, style, attr, text, children });
            if use_count { remaining -= 1; }
            if is_last { break; }
        }
        items
    }

    /// Parse a MENUTEMPLATE binary resource at `guest_addr` (size `size`).
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
        let mut offset = 10u32;
        self.parse_menu_items(guest_addr, size, &mut offset, item_count)
    }

    /// Parse a DLGTEMPLATE binary resource at `guest_addr` (size `size`).
    ///
    /// Handles the Open Watcom wrc OS/2 DLGTEMPLATE binary layout:
    ///   Bytes  0- 9: Template header (u32 total_size, u16 codepage, u16 offLastItem, u16 cTopItems)
    ///   Bytes 10-19: Dialog sub-header ×5 u16  (child_count at [16,17])
    ///   Bytes 20-29: Dialog metadata   ×5 u16  (title_offset at [24,25])
    ///   Bytes 30-39: Dialog position   i16 x,y,cx,cy + u16 id
    ///   Bytes 40-49: Dialog variable data (classname, etc.)
    ///   Bytes 50+:   Child items — each: 10-byte pre-header (class_ord u16 at [0], text_off u16 at
    ///                [4]) + 10-byte position (i16 x,y,cx,cy + u16 id) + variable data
    ///   Bytes [title_off]+: NUL-terminated strings (title then item texts indexed by text_off)
    ///
    /// Returns `(x, y, cx, cy, title, items)` or `None` on parse failure.
    fn parse_dlg_template(
        &self,
        guest_addr: u32,
        size: u32,
    ) -> Option<(i16, i16, i16, i16, String, Vec<DlgItemInfo>)> {
        if size < 40 { return None; }

        let c_items   = self.guest_read::<u16>(guest_addr + 16).unwrap_or(0) as usize;
        let title_off = self.guest_read::<u16>(guest_addr + 24).unwrap_or(0) as u32;
        let x  = self.guest_read::<i16>(guest_addr + 30).unwrap_or(0);
        let y  = self.guest_read::<i16>(guest_addr + 32).unwrap_or(0);
        let cx = self.guest_read::<i16>(guest_addr + 34).unwrap_or(0);
        let cy = self.guest_read::<i16>(guest_addr + 36).unwrap_or(0);

        // Dialog title sits in the string pool starting at title_off.
        let title = if title_off > 0 && title_off < size {
            let mut toff = title_off;
            self.read_nul_str_at(guest_addr, &mut toff, size)
        } else {
            String::new()
        };

        // Children start at byte 50; string pool begins at title_off.
        let items_end = if title_off > 0 && title_off < size { title_off } else { size };
        let mut off: u32 = 50;
        let mut items = Vec::with_capacity(c_items);
        for i in 0..c_items {
            if off + 20 > items_end { break; }
            // Pre-header (10 bytes): class ordinal at [0,1], text_off at [4,5]
            let class_ord = self.guest_read::<u16>(guest_addr + off).unwrap_or(0);
            let text_off  = self.guest_read::<u16>(guest_addr + off + 4).unwrap_or(0) as u32;
            off += 10;
            // Position block (10 bytes): i16 x, y, cx, cy then u16 id
            let ix  = self.guest_read::<i16>(guest_addr + off).unwrap_or(0);
            let iy  = self.guest_read::<i16>(guest_addr + off + 2).unwrap_or(0);
            let icx = self.guest_read::<i16>(guest_addr + off + 4).unwrap_or(0);
            let icy = self.guest_read::<i16>(guest_addr + off + 6).unwrap_or(0);
            let id  = self.guest_read::<u16>(guest_addr + off + 8).unwrap_or(0);
            off += 10;
            // Skip variable data: scan (2 bytes at a time) for the next item's class ordinal
            // (valid wrc class ordinals are 1–10) or advance to items_end for the last item.
            if i + 1 < c_items {
                while off + 2 <= items_end {
                    let maybe_class = self.guest_read::<u16>(guest_addr + off).unwrap_or(0);
                    if (1..=10).contains(&maybe_class) { break; }
                    off += 2;
                }
            } else {
                off = items_end;
            }
            let class_name = match class_ord {
                3 => "WC_BUTTON",
                5 => "WC_STATIC",
                6 => "WC_ENTRYFIELD",
                7 => "WC_LISTBOX",
                _ => "WC_STATIC",
            }.to_string();
            let text = if text_off > 0 && text_off < size {
                let mut toff = text_off;
                self.read_nul_str_at(guest_addr, &mut toff, size)
            } else {
                String::new()
            };
            items.push(DlgItemInfo {
                x: ix, y: iy, cx: icx, cy: icy,
                id,
                fl_style: super::constants::WS_VISIBLE,
                class_name,
                text,
            });
        }
        Some((x, y, cx, cy, title, items))
    }

    /// Parse the RT_MENU resource `id_menu` into a menu item tree.
    /// Returns `None` when the executable has no such resource.
    fn load_menu_resource(&self, id_menu: u32, vcpu_id: u32) -> Option<Vec<MenuItem>> {
        let res = self.shared.resource_mgr.lock_or_recover().find(RT_MENU, id_menu as u16);
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

    /// WinSetCp: set the codepage of message queue `hmq`. Fails for a
    /// codepage warpine cannot convert or an unknown queue, changing nothing.
    ///
    /// The value is per queue and independent of the process codepage
    /// (`DosSetProcessCp`), as in PM. PM text conversion still follows the
    /// process codepage, so this only affects what `WinQueryCp` reports.
    fn win_set_cp(&self, hmq: u32, cp: u32) -> bool {
        if !super::stubs::is_supported_codepage(cp) { return false; }
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
    fn attach_menu_to_frame(&self, hwnd_frame: u32, hwnd_menu: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let client = wm.frame_to_client.get(&hwnd_frame).copied();
        let old_size = client.and_then(|c| wm.get_window(c)).map(|w| (w.cx, w.cy));
        let prev = wm.attach_menu(hwnd_frame, hwnd_menu);
        let new_size = client.and_then(|c| wm.get_window(c)).map(|w| (w.cx, w.cy));
        if let (Some(client), Some((cx, cy))) = (client, new_size)
            && old_size != new_size
        {
            wm.resize_surface(hwnd_frame, cx, cy);
            if let Some(mq_arc) = wm.find_hmq_for_hwnd(client).and_then(|h| wm.get_mq(h)) {
                let mut mq = mq_arc.lock_or_recover();
                let mp2 = ((cy as u32 & 0xFFFF) << 16) | (cx as u32 & 0xFFFF);
                for (msg, mp2) in [(WM_SIZE, mp2), (WM_PAINT, 0)] {
                    mq.messages.push_back(OS2Message {
                        hwnd: client, msg, mp1: 0, mp2, time: 0, x: 0, y: 0,
                    });
                }
                mq.cond.notify_one();
            }
        }
        prev
    }

    /// Create a dialog window with child controls from parsed template data.
    ///
    /// Returns `(hwnd_dlg, hmq)`.
    #[allow(clippy::too_many_arguments)]
    fn create_dialog_from_template(
        &self,
        hwnd_parent: u32,
        pfn_dlg_proc: u32,
        vcpu_id: u32,
        title: String,
        dlg_x: i16, dlg_y: i16, dlg_cx: i16, dlg_cy: i16,
        items: Vec<DlgItemInfo>,
    ) -> (u32, u32) {
        use super::constants::{dlg_units_to_px_x, dlg_units_to_px_y};
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let hmq = wm.tid_to_hmq.get(&vcpu_id).copied().unwrap_or(0);
        // Template coordinates are relative to the dialog's parent: the desktop
        // for HWND_DESKTOP, otherwise the parent window's frame.
        let (origin_x, origin_y) = if hwnd_parent == HWND_DESKTOP {
            (0, 0)
        } else {
            let frame = wm.find_frame_for_hwnd(hwnd_parent);
            wm.get_window(frame).map_or((0, 0), |w| (w.x, w.y))
        };
        let h = wm.create_window("#Dialog".to_string(), hwnd_parent, hmq);
        if let Some(win) = wm.get_window_mut(h) {
            // Dialogs get the standard frame chrome (title bar, system menu,
            // dialog border) drawn by the compositor.
            win.frame_flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_DLGBORDER;
            win.text    = title.clone();
            win.pfn_wp  = pfn_dlg_proc;
            // DLGTEMPLATE geometry is in dialog units, not pixels.
            win.x       = origin_x + dlg_units_to_px_x(dlg_x);
            win.y       = origin_y + dlg_units_to_px_y(dlg_y);
            win.cx      = dlg_units_to_px_x(dlg_cx);
            win.cy      = dlg_units_to_px_y(dlg_cy);
            win.visible = true; // dialogs are always shown when created
        }
        wm.z_push_top(h);
        // A new dialog is the active window; z_remove hands focus back to the
        // window underneath when it is dismissed.
        wm.focused_hwnd = h;
        for item in items {
            let class_name = if !item.class_name.is_empty() {
                item.class_name.clone()
            } else {
                "WC_STATIC".to_string()
            };
            let ch = wm.create_window(class_name, h, hmq);
            if let Some(cwin) = wm.get_window_mut(ch) {
                cwin.text    = item.text;
                cwin.x       = dlg_units_to_px_x(item.x);
                cwin.y       = dlg_units_to_px_y(item.y);
                cwin.cx      = dlg_units_to_px_x(item.cx);
                cwin.cy      = dlg_units_to_px_y(item.cy);
                cwin.id      = item.id as u32;
                cwin.style   = item.fl_style;
                cwin.visible = item.fl_style & super::constants::WS_VISIBLE != 0;
            }
        }
        // VDR-A5: dialogs are pure Z-stack entries; no CreateWindow / ResizeWindow
        // needed — the compositor reads position from OS2Window and the FrameBuffer
        // is allocated lazily on the first draw call (sized from win.cx × win.cy).
        (h, hmq)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Loader, ApiResult};
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::mutex_ext::MutexExt;
    use std::sync::atomic::Ordering;

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
        loader.shared.resource_mgr.lock_or_recover().add(crate::lx::header::RT_MENU, 7, base, data.len() as u32);
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
    /// children must be scaled to pixels (2× with the 8×16 system font).
    #[test]
    fn test_create_dialog_scales_dialog_units_to_pixels() {
        use super::super::mutex_ext::MutexExt;
        use super::DlgItemInfo;
        let loader = Loader::new_mock();
        {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(TEST_CODEPAGE);
            wm.tid_to_hmq.insert(0, hmq);
        }
        let items = vec![DlgItemInfo {
            x: 70, y: 10, cx: 40, cy: 14, id: 1, fl_style: 0,
            class_name: "WC_BUTTON".to_string(), text: "OK".to_string(),
        }];
        use super::super::constants::{dlg_units_to_px_x as dx, dlg_units_to_px_y as dy,
                                      HWND_DESKTOP, FCF_TITLEBAR, FCF_DLGBORDER};
        let (hwnd_dlg, _) = loader.create_dialog_from_template(
            HWND_DESKTOP, 0, 0, "About".to_string(), 40, 40, 180, 80, items.clone());
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
        let (hwnd_dlg, _) = loader.create_dialog_from_template(
            client, 0, 0, "About".to_string(), 40, 40, 180, 80, items);
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
        let esp: u32 = 0x1000;
        vcpu.regs.rsp = esp as u64;
        write_stack(loader, esp, args);
        match loader.handle_pmwin_call(vcpu, 0, ordinal) {
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

    /// The codepage belongs to the queue: setting one leaves another unchanged.
    #[test]
    fn test_win_set_cp_is_per_queue() {
        use std::sync::atomic::Ordering;
        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(437, Ordering::Relaxed);
        let mut vcpu = MockVcpu::new();
        let hmq_a = call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
        let hmq_b = call_pmwin(&loader, &mut vcpu, ORD_WIN_CREATE_MSG_QUEUE, &[0, 0]);
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
                    MenuItem { id: 201, style: MIS_TEXT, attr: 0, text: "~About".to_string(), children: vec![] },
                    MenuItem { id: 202, style: MIS_TEXT, attr: 0, text: "Exit".to_string(),   children: vec![] },
                ],
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
        assert_eq!(cmd.mp1 & 0xFFFF, 202); // id=202 (Exit)
        assert_eq!((cmd.mp1 >> 16) & 0xFFFF, 2); // CMDSRC_MENU=2
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

    /// parse_dlg_template with a resource shorter than the 40-byte minimum
    /// must return None without panicking.
    #[test]
    fn test_parse_dlg_template_truncated_does_not_panic() {
        let loader = Loader::new_mock();
        let base: u32 = 0x5300;
        // Write only 20 bytes — less than the 40-byte minimum.
        let data = [0u8; 20];
        for (i, &b) in data.iter().enumerate() {
            loader.guest_write::<u8>(base + i as u32, b).unwrap();
        }
        let result = loader.parse_dlg_template(base, data.len() as u32);
        assert!(result.is_none(), "truncated dlg template must return None");
    }

    /// parse_dlg_template with an all-zero 64-byte resource (c_items=0)
    /// must return Some with an empty item list, not panic.
    #[test]
    fn test_parse_dlg_template_all_zero_does_not_panic() {
        let loader = Loader::new_mock();
        let base: u32 = 0x5400;
        // 64 zero bytes — size >= 40 so the header check passes; c_items=0 so
        // the child loop body never executes.
        let data = [0u8; 64];
        for (i, &b) in data.iter().enumerate() {
            loader.guest_write::<u8>(base + i as u32, b).unwrap();
        }
        let result = loader.parse_dlg_template(base, data.len() as u32);
        // size=64 >= 40, so result is Some; c_items=0 → empty items vec.
        assert!(result.is_some(), "all-zero dlg template (size>=40) should return Some");
        let (_, _, _, _, title, items) = result.unwrap();
        assert!(items.is_empty(), "c_items=0 must produce no child items");
        assert!(title.is_empty(), "title_off=0 must produce empty title");
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
            mq.messages.iter().filter(|m| m.msg == WM_CONTROL)
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
