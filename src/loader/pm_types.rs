// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::AtomicBool;
use std::thread;
use crate::gui::font::FontSpec;

/// Type alias for the timer map to reduce type complexity.
pub type TimerMap = HashMap<(u32, u32), (Arc<AtomicBool>, Option<thread::JoinHandle<()>>)>;


pub struct OS2Message {
    pub hwnd: u32,
    pub msg: u32,
    pub mp1: u32,
    pub mp2: u32,
    pub time: u32,
    pub x: i16,
    pub y: i16,
}

#[allow(non_camel_case_types)]
pub struct PM_MsgQueue {
    pub messages: VecDeque<OS2Message>,
    pub cond: Arc<Condvar>,
    pub lock: Arc<Mutex<bool>>,
}

pub struct WindowClass {
    pub name: String,
    pub pfn_wp: u32,
    pub style: u32,
}

pub struct OS2Window {
    pub handle: u32,
    pub class_name: String,
    pub pfn_wp: u32,
    pub parent: u32,
    pub hmq: u32,
    pub text: String,
    pub window_ulong: HashMap<i32, u32>,
    pub window_ushort: HashMap<i32, u16>,
    pub id: u32,
    pub children: Vec<u32>,
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub visible: bool,
    /// Raw flStyle value from WinCreateWindow / WinCreateStdWindow.
    pub style: u32,
    /// Items stored by WC_LISTBOX on LM_INSERTITEM.
    pub listbox_items: Vec<String>,
    /// Selected WC_LISTBOX item (single selection); `None` = no selection.
    pub listbox_sel: Option<usize>,
    /// Owner window from `WinCreateWindow` (receives WM_CONTROL notifications);
    /// 0 = none, in which case the parent is notified.
    pub owner: u32,
    /// Caret position of a WC_ENTRYFIELD / WC_MLE, in characters (not bytes)
    /// from the start of `text`. May exceed the text length after the guest
    /// replaces the text; readers clamp it.
    pub edit_caret: usize,
    /// Horizontal scroll of a WC_ENTRYFIELD / WC_MLE: the first visible
    /// character column of the caret's line.
    pub edit_scroll: usize,
    /// Vertical scroll: first visible line (WC_MLE) or item (WC_LISTBOX).
    pub top_row: usize,
    /// Items parsed from a MENUTEMPLATE binary resource (WC_MENU windows only).
    pub menu_items: Vec<MenuItem>,
    /// HWND of the menu attached to this frame window via WinSetMenu / WinLoadMenu.
    pub menu_hwnd: u32,
    /// Set by WinDismissDlg; inspected by the DlgRunLoop to end the modal loop.
    pub dialog_dismissed: bool,
    /// The result code passed to WinDismissDlg; returned by WinDlgBox.
    pub dialog_result: u32,
    /// `flCreateFlags` from `WinCreateStdWindow` (FCF_* bits).
    /// Zero for non-frame windows (dialogs, controls).
    pub frame_flags: u32,
}

pub struct PresentationSpace {
    pub hps: u32,
    pub hwnd: u32,
    /// Foreground (pen/text) colour, stored as 0x00RRGGBB.
    pub color: u32,
    /// Background colour, stored as 0x00RRGGBB.
    pub back_color: u32,
    /// Foreground mix mode (FM_* — e.g. FM_OVERPAINT = 6).
    pub mix_mode: u32,
    /// Background mix mode (BM_* — e.g. BM_OVERPAINT = 6).
    pub back_mix: u32,
    /// Current logical character set (LCID) selected via GpiSetCharSet.
    pub char_set: u32,
    /// Character box in world units (cx, cy); 0 means use default font metrics.
    pub char_box: (i32, i32),
    /// Logical fonts created by `GpiCreateLogFont`, keyed by LCID.
    pub log_fonts: HashMap<u32, FontSpec>,
    pub current_pos: (i32, i32),
}

impl PresentationSpace {
    /// The font text is currently drawn and measured with: the logical font
    /// selected by `GpiSetCharSet` (the system font for LCID 0 or an unknown
    /// LCID), resized by a `GpiSetCharBox` character box when one is set.
    pub fn current_font(&self) -> FontSpec {
        let mut spec = self.log_fonts.get(&self.char_set).copied().unwrap_or(FontSpec::SYSTEM);
        if self.char_box.1 > 0 {
            spec.px = crate::gui::font::clamp_px(self.char_box.1 as u32);
        }
        spec
    }
}

pub struct AccelEntry {
    pub flags: u16,
    pub key: u16,
    pub cmd: u16,
}

/// A single item from a parsed OS/2 MENUTEMPLATE binary resource.
///
/// Populated by `Loader::parse_menu_items` and stored in
/// `OS2Window::menu_items` for WC_MENU windows created by `WinLoadMenu`.
#[derive(Clone)]
pub struct MenuItem {
    pub id: u16,
    /// `afStyle` field from the binary resource (MIS_* flags, bit 15 stripped).
    pub style: u16,
    /// `afAttribute` field (MIA_* flags).
    pub attr: u16,
    pub text: String,
    /// Nested items for `MIS_SUBMENU` entries.
    pub children: Vec<MenuItem>,
}

/// Strip the `~` mnemonic marker from a menu item's text for display.
pub fn menu_label(text: &str) -> String {
    text.chars().filter(|&c| c != '~').collect()
}

/// Pixel width a top-level item occupies in the menu bar: its label in the
/// system font plus padding.
pub fn menu_bar_item_width(text: &str) -> i32 {
    use super::constants::MENU_BAR_ITEM_PAD;
    use crate::gui::font::{text_width, FontSpec};
    text_width(&menu_label(text), FontSpec::SYSTEM) + MENU_BAR_ITEM_PAD
}

pub struct WindowManager {
    classes: HashMap<String, WindowClass>,
    windows: HashMap<u32, OS2Window>,
    pub ps_map: HashMap<u32, PresentationSpace>,
    pub msg_queues: HashMap<u32, Arc<Mutex<PM_MsgQueue>>>,
    pub frame_to_client: HashMap<u32, u32>,
    pub tid_to_hmq: HashMap<u32, u32>,
    pub gui_tx: Option<crate::gui::GUISender>,
    pub timers: TimerMap,
    pub clipboard: HashMap<u32, u32>,
    pub clipboard_open: bool,
    /// Currently captured window handle (0 = none).
    pub capture_hwnd: u32,
    /// Text-format clipboard content, kept in sync with the host SDL2 clipboard.
    pub clipboard_text: String,
    accel_tables: HashMap<u32, Vec<AccelEntry>>,
    window_accel: HashMap<u32, u32>, // hwnd → haccel
    next_hwnd: u32,
    next_hps: u32,
    next_hmq: u32,
    next_haccel: u32,
    /// Back-to-front Z-order stack of top-level frame window handles.
    /// Index 0 = bottom-most; last index = top-most (foreground).
    /// Only frame windows (those present in `frame_to_client`) are tracked here.
    pub z_order: Vec<u32>,
    /// The currently active (focused) frame window handle; 0 = none.
    pub focused_hwnd: u32,
    /// The PM focus window: the window that receives keyboard input. 0 means
    /// "no explicit focus", in which case keys go to the active frame's
    /// client. Set by `WinSetFocus` and by clicking a built-in text control.
    pub focus_hwnd: u32,
    /// VDR-B4: set of HWNDs whose visual state has changed since the last composite.
    /// Any non-empty dirty set causes the next compositor tick to recomposite the
    /// full desktop; an empty set allows the tick to be skipped entirely.
    pub dirty: HashSet<u32>,
}

impl Default for WindowManager {
    fn default() -> Self { Self::new() }
}

impl WindowManager {
    pub fn new() -> Self {
        WindowManager {
            classes: HashMap::new(),
            windows: HashMap::new(),
            ps_map: HashMap::new(),
            msg_queues: HashMap::new(),
            frame_to_client: HashMap::new(),
            tid_to_hmq: HashMap::new(),
            gui_tx: None,
            timers: HashMap::new(),
            clipboard: HashMap::new(),
            clipboard_open: false,
            capture_hwnd: 0,
            clipboard_text: String::new(),
            accel_tables: HashMap::new(),
            window_accel: HashMap::new(),
            next_hwnd: 0x1000,
            next_hps: 0x2000,
            next_hmq: 0x3000,
            next_haccel: 0x4000,
            z_order: Vec::new(),
            focused_hwnd: 0,
            focus_hwnd: 0,
            dirty: HashSet::new(),
        }
    }
    pub fn register_class(&mut self, name: String, pfn_wp: u32, style: u32) {
        self.classes.insert(name.clone(), WindowClass { name, pfn_wp, style });
    }
    pub fn get_class(&self, name: &str) -> Option<&WindowClass> {
        self.classes.get(name)
    }
    pub fn create_window(&mut self, class_name: String, parent: u32, hmq: u32) -> u32 {
        let h = self.next_hwnd;
        let pfn_wp = self.classes.get(&class_name).map(|c| c.pfn_wp).unwrap_or(0);
        self.windows.insert(h, OS2Window {
            handle: h, class_name, pfn_wp, parent, hmq,
            text: String::new(),
            window_ulong: HashMap::new(),
            window_ushort: HashMap::new(),
            id: 0,
            children: Vec::new(),
            x: 0, y: 0, cx: 0, cy: 0,
            visible: false,
            style: 0,
            listbox_items: Vec::new(),
            listbox_sel: None,
            owner: 0,
            edit_caret: 0,
            edit_scroll: 0,
            top_row: 0,
            menu_items: Vec::new(),
            menu_hwnd: 0,
            dialog_dismissed: false,
            dialog_result: 0,
            frame_flags: 0,
        });
        // Register as child of parent
        if parent != 0
            && let Some(parent_win) = self.windows.get_mut(&parent) {
                parent_win.children.push(h);
        }
        self.next_hwnd += 1;
        h
    }
    pub fn get_window(&self, h: u32) -> Option<&OS2Window> {
        self.windows.get(&h)
    }
    pub fn get_window_mut(&mut self, h: u32) -> Option<&mut OS2Window> {
        self.windows.get_mut(&h)
    }
    pub fn get_ps_hwnd(&self, hps: u32) -> u32 {
        self.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0)
    }
    pub fn create_ps(&mut self, hwnd: u32) -> u32 {
        let h = self.next_hps;
        self.ps_map.insert(h, PresentationSpace {
            hps: h,
            hwnd,
            color: 0x00000000,     // CLR_BLACK — default foreground
            back_color: 0x00FFFFFF, // CLR_WHITE — default background
            mix_mode: 6,            // FM_OVERPAINT
            back_mix: 6,            // BM_OVERPAINT
            char_set: 0,
            char_box: (0, 0),
            log_fonts: HashMap::new(),
            current_pos: (0, 0),
        });
        self.next_hps += 1;
        h
    }
    pub fn create_mq(&mut self) -> u32 {
        let h = self.next_hmq;
        self.msg_queues.insert(h, Arc::new(Mutex::new(PM_MsgQueue {
            messages: VecDeque::new(),
            cond: Arc::new(Condvar::new()),
            lock: Arc::new(Mutex::new(false)),
        })));
        self.next_hmq += 1;
        h
    }
    pub fn get_mq(&self, h: u32) -> Option<Arc<Mutex<PM_MsgQueue>>> {
        self.msg_queues.get(&h).cloned()
    }
    pub fn find_hmq_for_hwnd(&self, hwnd: u32) -> Option<u32> {
        if let Some(win) = self.windows.get(&hwnd)
            && win.hmq != 0 {
                return Some(win.hmq);
        }
        // Search through tid_to_hmq for a match
        self.tid_to_hmq.values().find(|&&hmq| self.msg_queues.contains_key(&hmq)).copied()
    }
    /// Reverse lookup: given a client hwnd, find the frame hwnd.
    /// Returns the client hwnd itself if no mapping exists.
    pub fn client_to_frame(&self, client_hwnd: u32) -> u32 {
        self.frame_to_client.iter()
            .find(|&(_, &client)| client == client_hwnd)
            .map(|(&frame, _)| frame)
            .unwrap_or(client_hwnd)
    }

    /// Stop all running timers and join their threads.
    pub fn stop_all_timers(&mut self) {
        for (_, (running, handle)) in self.timers.drain() {
            running.store(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(h) = handle {
                let _ = h.join();
            }
        }
    }

    pub fn add_accel_table(&mut self, entries: Vec<AccelEntry>) -> u32 {
        let h = self.next_haccel;
        self.accel_tables.insert(h, entries);
        self.next_haccel += 1;
        h
    }

    pub fn set_window_accel(&mut self, hwnd: u32, haccel: u32) {
        if haccel == 0 {
            self.window_accel.remove(&hwnd);
        } else {
            self.window_accel.insert(hwnd, haccel);
        }
    }

    pub fn translate_accel(&self, hwnd: u32, key: u16, flags: u16) -> Option<u16> {
        let haccel = self.window_accel.get(&hwnd)?;
        let entries = self.accel_tables.get(haccel)?;
        for entry in entries {
            if entry.key == key && (entry.flags & flags) == entry.flags {
                return Some(entry.cmd);
            }
        }
        None
    }

    /// Walk up the parent chain to find the top-level frame window for `hwnd`.
    ///
    /// Stops when the parent is in `frame_to_client` (i.e. it is a frame) or
    /// when there is no parent.  Returns the frame hwnd, or `hwnd` itself if
    /// no frame ancestor is found.
    pub fn find_frame_for_hwnd(&self, hwnd: u32) -> u32 {
        let mut h = hwnd;
        loop {
            let parent = match self.windows.get(&h) { Some(w) => w.parent, None => return h };
            if parent == 0 { return h; }
            if self.frame_to_client.contains_key(&parent) { return parent; }
            h = parent;
        }
    }

    /// Compute the absolute OS/2-space rectangle of `hwnd` relative to the
    /// root frame's client area.
    ///
    /// OS/2 PM uses a bottom-left origin, so `(x, y)` is the bottom-left corner
    /// of `hwnd` in frame-client coordinates.  The caller can pass these
    /// directly as `(x1=x, y1=y, x2=x+cx, y2=y+cy)` to `GUIMessage::DrawBox`.
    ///
    /// Returns `(x, y, cx, cy)`.
    pub fn get_abs_rect_in_frame(&self, hwnd: u32) -> (i32, i32, i32, i32) {
        let (cx, cy) = self.windows.get(&hwnd)
            .map(|w| (w.cx, w.cy)).unwrap_or((0, 0));
        let mut ax = 0i32;
        let mut ay = 0i32;
        let mut h = hwnd;
        while let Some(win) = self.windows.get(&h) {
            ax += win.x;
            ay += win.y;
            let parent = win.parent;
            if parent == 0 { break; }
            if self.frame_to_client.contains_key(&parent) { break; }
            h = parent;
        }
        (ax, ay, cx, cy)
    }

    pub fn find_child_by_id(&self, parent: u32, id: u32) -> Option<u32> {
        if let Some(win) = self.windows.get(&parent) {
            for &child_hwnd in &win.children {
                if let Some(child) = self.windows.get(&child_hwnd)
                    && child.id == id {
                        return Some(child_hwnd);
                }
            }
        }
        None
    }

    /// Visible built-in controls (`WC_*` windows without a guest window
    /// procedure) below `hwnd`, in paint order: parents before children,
    /// siblings in creation order.
    pub fn builtin_control_descendants(&self, hwnd: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack: Vec<u32> = self.windows.get(&hwnd)
            .map(|w| w.children.iter().rev().copied().collect()).unwrap_or_default();
        while let Some(h) = stack.pop() {
            let Some(w) = self.windows.get(&h) else { continue };
            if !w.visible { continue; }
            if w.pfn_wp == 0 && w.class_name.starts_with("WC_") {
                out.push(h);
            }
            stack.extend(w.children.iter().rev().copied());
        }
        out
    }

    /// The window keyboard input is delivered to: the focus window while it
    /// exists, is visible and belongs to the active frame; otherwise the
    /// active frame itself (whose client `push_msg` resolves).
    pub fn key_target(&self) -> u32 {
        match self.windows.get(&self.focus_hwnd) {
            Some(w) if w.visible && self.focus_hwnd != 0
                && self.find_frame_for_hwnd(self.focus_hwnd) == self.focused_hwnd => self.focus_hwnd,
            _ => self.focused_hwnd,
        }
    }

    /// The window the owner notifications (WM_CONTROL) of `hwnd` go to: its
    /// owner, or its parent when it was created without one.
    pub fn notify_target(&self, hwnd: u32) -> u32 {
        self.windows.get(&hwnd).map_or(0, |w| if w.owner != 0 { w.owner } else { w.parent })
    }

    /// Hit-test point `(x, y)` (client coordinates of `frame`, origin at the
    /// bottom-left) against the frame's built-in controls. Returns the
    /// top-most enabled control under the point together with the point in
    /// that control's own coordinates. Static text is transparent to the
    /// pointer, and windows without a client (dialogs) have no routed controls.
    pub fn control_at(&self, frame: u32, x: i32, y: i32) -> Option<(u32, i32, i32)> {
        use super::constants::WS_DISABLED;
        let &client = self.frame_to_client.get(&frame)?;
        self.builtin_control_descendants(client).into_iter().rev().find_map(|h| {
            let w = self.windows.get(&h)?;
            if w.class_name == "WC_STATIC" || w.style & WS_DISABLED != 0 { return None; }
            let (ax, ay, cx, cy) = self.get_abs_rect_in_frame(h);
            (x >= ax && x < ax + cx && y >= ay && y < ay + cy).then_some((h, x - ax, y - ay))
        })
    }

    // ── Frame layout helpers ─────────────────────────────────────────────────

    /// Height of the chrome stacked above a frame's client area: the title bar
    /// (when `FCF_TITLEBAR` is set) plus the menu bar (when a menu is attached).
    pub fn frame_chrome_top_h(&self, frame: u32) -> i32 {
        use super::constants::{CHROME_TITLE_H, FCF_TITLEBAR, MENU_BAR_HEIGHT};
        self.windows.get(&frame).map_or(0, |w| {
            let title_h = if w.frame_flags & FCF_TITLEBAR != 0 { CHROME_TITLE_H } else { 0 };
            let menu_h = if w.menu_hwnd != 0 { MENU_BAR_HEIGHT as i32 } else { 0 };
            title_h + menu_h
        })
    }

    /// Re-derive the menu-bar and client rectangles of `frame` from its current
    /// size: the menu bar sits directly below the title bar and the client
    /// fills what is left underneath. Returns the client `(cx, cy)`.
    pub fn layout_frame(&mut self, frame: u32) -> (i32, i32) {
        use super::constants::MENU_BAR_HEIGHT;
        let Some((frame_cx, frame_cy, menu_hwnd)) =
            self.windows.get(&frame).map(|w| (w.cx, w.cy, w.menu_hwnd)) else { return (0, 0) };
        let top_h = self.frame_chrome_top_h(frame);
        let client_cy = (frame_cy - top_h).max(0);
        if menu_hwnd != 0 && let Some(menu) = self.windows.get_mut(&menu_hwnd) {
            menu.x = 0;
            menu.y = client_cy;
            menu.cx = frame_cx;
            menu.cy = MENU_BAR_HEIGHT as i32;
        }
        if let Some(&client) = self.frame_to_client.get(&frame)
            && let Some(cw) = self.windows.get_mut(&client) {
            cw.cx = frame_cx;
            cw.cy = client_cy;
        }
        self.mark_dirty(frame);
        (frame_cx, client_cy)
    }

    /// Attach `menu` as the menu bar of `frame` (0 detaches) and re-layout the
    /// frame so the client area no longer overlaps the bar. Returns the
    /// previously attached menu handle.
    pub fn attach_menu(&mut self, frame: u32, menu: u32) -> u32 {
        let Some(win) = self.windows.get_mut(&frame) else { return 0 };
        let prev = win.menu_hwnd;
        win.menu_hwnd = menu;
        self.layout_frame(frame);
        prev
    }

    // ── Z-order helpers ──────────────────────────────────────────────────────

    /// VDR-B4: mark an HWND (frame or client) as visually dirty.
    /// The compositor will recomposite the full desktop on its next tick.
    pub fn mark_dirty(&mut self, hwnd: u32) {
        self.dirty.insert(hwnd);
    }

    /// VDR-B4: mark every frame in the Z-order dirty.
    /// Called after Z-order changes, which affect all overlapping windows.
    pub fn mark_all_dirty(&mut self) {
        self.dirty.extend(self.z_order.iter().copied());
    }

    /// Register a frame window into the Z-order stack (top-most position).
    /// Only frame windows (present in `frame_to_client`) should be registered.
    /// Safe to call multiple times — duplicates are ignored.
    pub fn z_push_top(&mut self, frame_hwnd: u32) {
        if !self.z_order.contains(&frame_hwnd) {
            self.z_order.push(frame_hwnd);
        } else {
            // Already present — bring to top.
            self.z_order.retain(|&h| h != frame_hwnd);
            self.z_order.push(frame_hwnd);
        }
        self.mark_all_dirty();
    }

    /// Remove a frame window from the Z-order stack (called on WinDestroyWindow).
    pub fn z_remove(&mut self, frame_hwnd: u32) {
        self.z_order.retain(|&h| h != frame_hwnd);
        if self.focused_hwnd == frame_hwnd {
            // Focus falls to the new top-most window, if any.
            self.focused_hwnd = self.z_order.last().copied().unwrap_or(0);
        }
        self.mark_all_dirty();
    }

    /// Move a frame to the bottom of the Z-order stack.
    pub fn z_push_bottom(&mut self, frame_hwnd: u32) {
        self.z_order.retain(|&h| h != frame_hwnd);
        self.z_order.insert(0, frame_hwnd);
        self.mark_all_dirty();
    }

    /// Insert `frame_hwnd` directly below `behind_hwnd` in the Z-stack.
    /// If `behind_hwnd` is not found, falls back to `z_push_top`.
    pub fn z_insert_behind(&mut self, frame_hwnd: u32, behind_hwnd: u32) {
        self.z_order.retain(|&h| h != frame_hwnd);
        if let Some(pos) = self.z_order.iter().position(|&h| h == behind_hwnd) {
            self.z_order.insert(pos, frame_hwnd);
        } else {
            self.z_order.push(frame_hwnd);
        }
        self.mark_all_dirty();
    }

    /// Hit-test desktop-absolute point `(px, py)` against the Z-order stack,
    /// returning the top-most visible frame whose bounding box contains the point.
    /// `(px, py)` uses OS/2 coordinates (y=0 at bottom).
    pub fn z_hit_test(&self, px: i32, py: i32) -> Option<u32> {
        // Iterate top-to-bottom (reverse of z_order).
        for &hwnd in self.z_order.iter().rev() {
            if let Some(win) = self.windows.get(&hwnd) {
                if !win.visible { continue; }
                if px >= win.x && px < win.x + win.cx
                    && py >= win.y && py < win.y + win.cy
                {
                    return Some(hwnd);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::WindowManager;

    fn make_frame(wm: &mut WindowManager) -> u32 {
        let h_frame = wm.create_window("TestFrame".to_string(), 0, 0);
        let h_client = wm.create_window("TestClient".to_string(), h_frame, 0);
        wm.frame_to_client.insert(h_frame, h_client);
        h_frame
    }

    /// A std window with a button, an overlapping entry field created later,
    /// a static label and a disabled list box.
    #[test]
    fn control_at_finds_topmost_enabled_control_in_local_coords() {
        use super::super::constants::WS_DISABLED;
        let mut wm = WindowManager::new();
        let frame = wm.create_window("App".into(), 0, 0);
        let client = wm.create_window("App".into(), frame, 0);
        wm.frame_to_client.insert(frame, client);
        let add = |wm: &mut WindowManager, class: &str, rect: (i32, i32, i32, i32), style: u32| {
            let h = wm.create_window(class.into(), client, 0);
            let w = wm.get_window_mut(h).unwrap();
            (w.x, w.y, w.cx, w.cy) = rect;
            w.visible = true;
            w.style = style;
            h
        };
        let button = add(&mut wm, "WC_BUTTON", (10, 10, 100, 30), 0);
        let entry = add(&mut wm, "WC_ENTRYFIELD", (50, 20, 100, 30), 0);
        let _label = add(&mut wm, "WC_STATIC", (200, 10, 50, 20), 0);
        let _disabled = add(&mut wm, "WC_LISTBOX", (300, 10, 50, 50), WS_DISABLED);

        assert_eq!(wm.control_at(frame, 12, 15), Some((button, 2, 5)));
        // Overlap: the later sibling is painted on top, so it wins.
        assert_eq!(wm.control_at(frame, 60, 25), Some((entry, 10, 5)));
        // Edges: left/bottom inclusive, right/top exclusive.
        assert_eq!(wm.control_at(frame, 10, 10), Some((button, 0, 0)));
        assert_eq!(wm.control_at(frame, 150, 49), None);
        assert_eq!(wm.control_at(frame, 149, 49), Some((entry, 99, 29)));
        // Static text and disabled controls are transparent; so is empty space.
        assert_eq!(wm.control_at(frame, 210, 15), None);
        assert_eq!(wm.control_at(frame, 310, 15), None);
        assert_eq!(wm.control_at(frame, 400, 400), None);
        assert_eq!(wm.control_at(frame, -5, -5), None);
        // Not a std frame (no client): nothing is routed.
        assert_eq!(wm.control_at(client, 12, 15), None);
        assert_eq!(wm.control_at(0xDEAD, 12, 15), None);
    }

    #[test]
    fn key_target_prefers_valid_focus_window_of_active_frame() {
        let mut wm = WindowManager::new();
        let f1 = wm.create_window("App".into(), 0, 0);
        let c1 = wm.create_window("App".into(), f1, 0);
        wm.frame_to_client.insert(f1, c1);
        let f2 = wm.create_window("App".into(), 0, 0);
        let c2 = wm.create_window("App".into(), f2, 0);
        wm.frame_to_client.insert(f2, c2);
        let entry = wm.create_window("WC_ENTRYFIELD".into(), c1, 0);
        wm.get_window_mut(entry).unwrap().visible = true;
        wm.focused_hwnd = f1;

        assert_eq!(wm.key_target(), f1, "no focus window: the active frame");
        wm.focus_hwnd = entry;
        assert_eq!(wm.key_target(), entry);
        // The focus window belongs to another frame than the active one.
        wm.focused_hwnd = f2;
        assert_eq!(wm.key_target(), f2);
        wm.focused_hwnd = f1;
        // Hidden or destroyed focus windows do not swallow keys.
        wm.get_window_mut(entry).unwrap().visible = false;
        assert_eq!(wm.key_target(), f1);
        wm.focus_hwnd = 0xDEAD;
        assert_eq!(wm.key_target(), f1);
    }

    #[test]
    fn notify_target_is_owner_else_parent() {
        let mut wm = WindowManager::new();
        let parent = wm.create_window("App".into(), 0, 0);
        let other = wm.create_window("App".into(), 0, 0);
        let ctl = wm.create_window("WC_LISTBOX".into(), parent, 0);
        assert_eq!(wm.notify_target(ctl), parent);
        wm.get_window_mut(ctl).unwrap().owner = other;
        assert_eq!(wm.notify_target(ctl), other);
        assert_eq!(wm.notify_target(0xDEAD), 0);
    }

    #[test]
    fn builtin_control_descendants_lists_visible_controls_in_paint_order() {
        let mut wm = WindowManager::new();
        let client = wm.create_window("AppClient".to_string(), 0, 0);
        let mk = |wm: &mut WindowManager, class: &str, parent: u32, visible: bool, pfn: u32| {
            let h = wm.create_window(class.to_string(), parent, 0);
            let w = wm.get_window_mut(h).unwrap();
            w.visible = visible;
            w.pfn_wp = pfn;
            h
        };
        let button = mk(&mut wm, "WC_BUTTON", client, true, 0);
        let hidden = mk(&mut wm, "WC_LISTBOX", client, false, 0);
        let custom = mk(&mut wm, "AppChild", client, true, 0x1000); // guest-drawn child
        let nested = mk(&mut wm, "WC_STATIC", custom, true, 0);
        let entry = mk(&mut wm, "WC_ENTRYFIELD", client, true, 0);
        let _under_hidden = mk(&mut wm, "WC_STATIC", hidden, true, 0);

        assert_eq!(wm.builtin_control_descendants(client), vec![button, nested, entry]);
        assert!(wm.builtin_control_descendants(button).is_empty());
        assert!(wm.builtin_control_descendants(0xDEAD).is_empty());
    }

    #[test]
    fn layout_frame_puts_client_below_title_bar() {
        use crate::loader::constants::{CHROME_TITLE_H, FCF_TITLEBAR};
        let mut wm = WindowManager::new();
        let frame = make_frame(&mut wm);
        {
            let w = wm.get_window_mut(frame).unwrap();
            w.cx = 640; w.cy = 480; w.frame_flags = FCF_TITLEBAR;
        }
        assert_eq!(wm.frame_chrome_top_h(frame), CHROME_TITLE_H);
        assert_eq!(wm.layout_frame(frame), (640, 480 - CHROME_TITLE_H));
        let client = wm.frame_to_client[&frame];
        assert_eq!(wm.get_window(client).unwrap().cy, 480 - CHROME_TITLE_H);
    }

    #[test]
    fn attach_menu_shrinks_client_and_places_bar_above_it() {
        use crate::loader::constants::{CHROME_TITLE_H, FCF_TITLEBAR, MENU_BAR_HEIGHT};
        let mut wm = WindowManager::new();
        let frame = make_frame(&mut wm);
        {
            let w = wm.get_window_mut(frame).unwrap();
            w.cx = 640; w.cy = 480; w.frame_flags = FCF_TITLEBAR;
        }
        wm.layout_frame(frame);
        wm.dirty.clear();
        let menu = wm.create_window("#Menu".to_string(), frame, 0);

        assert_eq!(wm.attach_menu(frame, menu), 0, "no previous menu");
        let client_cy = 480 - CHROME_TITLE_H - MENU_BAR_HEIGHT as i32;
        assert_eq!(wm.frame_chrome_top_h(frame), CHROME_TITLE_H + MENU_BAR_HEIGHT as i32);
        let client = wm.frame_to_client[&frame];
        assert_eq!(wm.get_window(client).unwrap().cy, client_cy);
        // The bar occupies the strip directly above the client, so they never overlap.
        let m = wm.get_window(menu).unwrap();
        assert_eq!((m.x, m.y, m.cx, m.cy), (0, client_cy, 640, MENU_BAR_HEIGHT as i32));
        assert!(wm.dirty.contains(&frame), "attaching a menu must request a recomposite");

        // Detaching gives the strip back to the client.
        assert_eq!(wm.attach_menu(frame, 0), menu);
        assert_eq!(wm.get_window(client).unwrap().cy, 480 - CHROME_TITLE_H);
    }

    #[test]
    fn menu_bar_item_width_ignores_mnemonic_marker() {
        use super::{menu_bar_item_width, menu_label};
        assert_eq!(menu_label("~File"), "File");
        assert_eq!(menu_bar_item_width("~File"), menu_bar_item_width("File"));
        // Label width in the proportional system font, plus the fixed padding.
        let label_w = crate::gui::font::text_width("File", crate::gui::font::FontSpec::SYSTEM);
        assert_eq!(menu_bar_item_width("File"), label_w + 8);
        assert!(menu_bar_item_width("WWWW") > menu_bar_item_width("iiii"));
    }

    #[test]
    fn z_push_top_adds_and_deduplicates() {
        let mut wm = WindowManager::new();
        let f1 = make_frame(&mut wm);
        let f2 = make_frame(&mut wm);
        let f3 = make_frame(&mut wm);

        wm.z_push_top(f1);
        wm.z_push_top(f2);
        wm.z_push_top(f3);
        assert_eq!(wm.z_order, vec![f1, f2, f3]);

        // Pushing f1 again brings it to top.
        wm.z_push_top(f1);
        assert_eq!(wm.z_order, vec![f2, f3, f1]);
    }

    #[test]
    fn z_push_bottom_places_at_index_zero() {
        let mut wm = WindowManager::new();
        let f1 = make_frame(&mut wm);
        let f2 = make_frame(&mut wm);
        let f3 = make_frame(&mut wm);

        wm.z_push_top(f1);
        wm.z_push_top(f2);
        wm.z_push_top(f3);
        // Stack: [f1, f2, f3]

        wm.z_push_bottom(f3);
        assert_eq!(wm.z_order, vec![f3, f1, f2]);
    }

    #[test]
    fn z_insert_behind_positions_correctly() {
        let mut wm = WindowManager::new();
        let f1 = make_frame(&mut wm);
        let f2 = make_frame(&mut wm);
        let f3 = make_frame(&mut wm);

        wm.z_push_top(f1);
        wm.z_push_top(f2);
        // Stack: [f1, f2]

        // Insert f3 behind (below) f2.
        wm.z_insert_behind(f3, f2);
        assert_eq!(wm.z_order, vec![f1, f3, f2]);
    }

    #[test]
    fn z_remove_cleans_up_focus() {
        let mut wm = WindowManager::new();
        let f1 = make_frame(&mut wm);
        let f2 = make_frame(&mut wm);

        wm.z_push_top(f1);
        wm.z_push_top(f2);
        wm.focused_hwnd = f2;

        wm.z_remove(f2);
        // f2 gone; focus falls back to new top (f1).
        assert_eq!(wm.focused_hwnd, f1);
        assert!(!wm.z_order.contains(&f2));
    }

    #[test]
    fn z_hit_test_returns_top_most_visible() {
        let mut wm = WindowManager::new();
        let f1 = make_frame(&mut wm);
        let f2 = make_frame(&mut wm);

        // f1: covers (0,0)-(100,100), f2: covers (50,50)-(150,150)
        {
            let w = wm.get_window_mut(f1).unwrap();
            w.x = 0; w.y = 0; w.cx = 100; w.cy = 100; w.visible = true;
        }
        {
            let w = wm.get_window_mut(f2).unwrap();
            w.x = 50; w.y = 50; w.cx = 100; w.cy = 100; w.visible = true;
        }

        wm.z_push_top(f1);
        wm.z_push_top(f2); // f2 on top

        // In overlap region (60,60): f2 is on top.
        assert_eq!(wm.z_hit_test(60, 60), Some(f2));
        // In f1-only region (10,10): f1.
        assert_eq!(wm.z_hit_test(10, 10), Some(f1));
        // Outside both.
        assert_eq!(wm.z_hit_test(200, 200), None);
    }

    #[test]
    fn z_hit_test_ignores_invisible_windows() {
        let mut wm = WindowManager::new();
        let f1 = make_frame(&mut wm);
        let f2 = make_frame(&mut wm);

        for &h in &[f1, f2] {
            let w = wm.get_window_mut(h).unwrap();
            w.x = 0; w.y = 0; w.cx = 100; w.cy = 100; w.visible = true;
        }
        wm.z_push_top(f1);
        wm.z_push_top(f2);

        // Hide the top window — hit should fall through to f1.
        wm.get_window_mut(f2).unwrap().visible = false;
        assert_eq!(wm.z_hit_test(50, 50), Some(f1));
    }

    #[test]
    fn mark_dirty_inserts_hwnd() {
        let mut wm = WindowManager::new();
        assert!(wm.dirty.is_empty());
        wm.mark_dirty(0x1000);
        assert!(wm.dirty.contains(&0x1000));
        assert_eq!(wm.dirty.len(), 1);
    }

    #[test]
    fn mark_all_dirty_covers_z_order() {
        let mut wm = WindowManager::new();
        let f1 = make_frame(&mut wm);
        let f2 = make_frame(&mut wm);
        // Use a raw z_order push to avoid the side-effect of mark_all_dirty.
        wm.z_order.push(f1);
        wm.z_order.push(f2);
        wm.dirty.clear();

        wm.mark_all_dirty();
        assert!(wm.dirty.contains(&f1));
        assert!(wm.dirty.contains(&f2));
    }

    #[test]
    fn z_push_top_marks_dirty() {
        let mut wm = WindowManager::new();
        let f1 = make_frame(&mut wm);
        wm.dirty.clear();
        wm.z_push_top(f1);
        assert!(!wm.dirty.is_empty(), "z_push_top must mark dirty");
    }

    #[test]
    fn z_remove_marks_dirty() {
        let mut wm = WindowManager::new();
        let f1 = make_frame(&mut wm);
        let f2 = make_frame(&mut wm);
        wm.z_order.push(f1);
        wm.z_order.push(f2);
        wm.dirty.clear();
        wm.z_remove(f1);
        assert!(!wm.dirty.is_empty(), "z_remove must mark dirty");
    }
}
