// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, Condvar};
use std::thread;
use crate::gui::font::FontSpec;
use crate::gui::surface::{SharedSurface, Surface};
use super::mutex_ext::MutexExt;

/// Stop signal of a `WinStartTimer` thread. The thread sleeps on it, so
/// stopping the timer wakes the thread at once instead of after the rest of
/// its period (a `WinStopTimer` that joined a thread sleeping a whole period
/// blocked the caller for up to that period).
#[derive(Default)]
pub struct TimerStop {
    stopped: Mutex<bool>,
    cond: Condvar,
}

impl TimerStop {
    pub fn stop(&self) {
        *self.stopped.lock_or_recover() = true;
        self.cond.notify_all();
    }

    pub fn is_stopped(&self) -> bool {
        *self.stopped.lock_or_recover()
    }

    /// Sleep for `period` unless stopped first; whether it was stopped.
    pub fn sleep(&self, period: std::time::Duration) -> bool {
        let guard = self.stopped.lock_or_recover();
        let (guard, _) = self.cond.wait_timeout_while(guard, period, |stopped| !*stopped)
            .unwrap_or_else(|e| e.into_inner());
        *guard
    }
}

/// Type alias for the timer map to reduce type complexity.
pub type TimerMap = HashMap<(u32, u32), (Arc<TimerStop>, Option<thread::JoinHandle<()>>)>;


#[derive(Debug, Clone, PartialEq)]
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
    /// Queue codepage (WinSetCp / WinQueryCp). PM keeps one per message
    /// queue; it starts as the process codepage when the queue is created.
    pub codepage: u32,
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
    /// Selected WC_LISTBOX items, cursor and range anchor.
    pub listbox_sel: super::pm_list::ListSelection,
    /// Owner window from `WinCreateWindow` (receives WM_CONTROL notifications);
    /// 0 = none, in which case the parent is notified.
    pub owner: u32,
    /// Caret position of a WC_ENTRYFIELD / WC_MLE, in characters (not bytes)
    /// from the start of `text`. May exceed the text length after the guest
    /// replaces the text; readers clamp it.
    pub edit_caret: usize,
    /// Selection anchor of a WC_ENTRYFIELD / WC_MLE: the selection runs from
    /// here to `edit_caret`; equal to it when nothing is selected.
    pub edit_anchor: usize,
    /// The caret of a focused WC_ENTRYFIELD / WC_MLE is in the off phase of
    /// its blink (toggled by `WM_TIMER(TID_CURSOR)`; input shows it again).
    pub caret_off: bool,
    /// Horizontal scroll of a WC_ENTRYFIELD / WC_MLE: the first visible
    /// character column of the caret's line.
    pub edit_scroll: usize,
    /// Vertical scroll: first visible line (WC_MLE) or item (WC_LISTBOX).
    pub top_row: usize,
    /// Items parsed from a MENUTEMPLATE binary resource (WC_MENU windows only).
    pub menu_items: Vec<MenuItem>,
    /// HWND of the menu attached to this frame window (WinLoadMenu / FCF_MENU).
    pub menu_hwnd: u32,
    /// Set by WinDismissDlg; inspected by the DlgRunLoop to end the modal loop.
    pub dialog_dismissed: bool,
    /// The result code passed to WinDismissDlg; returned by WinDlgBox.
    pub dialog_result: u32,
    /// While a modal dialog runs: the owner frame it disabled (0 = none),
    /// enabled again when the dialog ends.
    pub modal_owner: u32,
    /// Destroy the dialog when its modal loop ends (WinDlgBox; a dialog run
    /// by WinProcessDlg stays until the program destroys it).
    pub destroy_on_dismiss: bool,
    /// `flCreateFlags` from `WinCreateStdWindow` (FCF_* bits).
    /// Zero for non-frame windows (dialogs, controls).
    pub frame_flags: u32,
}

/// Map a PM colour value to 0x00RRGGBB with the default colour table: the
/// negative `CLR_*` specials, the 16 indices `CLR_BACKGROUND`..`CLR_PALEGRAY`,
/// and — leniently, since applications often skip `GpiCreateLogColorTable` —
/// any value ≥ 16 as a direct RGB value.
pub fn default_color(clr: u32) -> u32 {
    // Cast to signed so that CLR_BLACK=-1, CLR_WHITE=-2, CLR_DEFAULT=-3 match.
    match clr as i32 {
        -5 | -4     => 0x00000000, // CLR_FALSE / CLR_TRUE — treat as black
        -3          => 0x00000000, // CLR_DEFAULT — default foreground (black)
        -2          => 0x00FFFFFF, // CLR_WHITE
        -1          => 0x00000000, // CLR_BLACK
         0          => 0x00FFFFFF, // CLR_BACKGROUND — page background (white)
         1          => 0x000000FF, // CLR_BLUE
         2          => 0x00FF0000, // CLR_RED
         3          => 0x00FF00FF, // CLR_PINK (magenta)
         4          => 0x0000FF00, // CLR_GREEN
         5          => 0x0000FFFF, // CLR_CYAN
         6          => 0x00FFFF00, // CLR_YELLOW
         7          => 0x00808080, // CLR_NEUTRAL (medium grey)
         8          => 0x00404040, // CLR_DARKGRAY
         9          => 0x00000080, // CLR_DARKBLUE
        10          => 0x00800000, // CLR_DARKRED
        11          => 0x00800080, // CLR_DARKPINK
        12          => 0x00008000, // CLR_DARKGREEN
        13          => 0x00008080, // CLR_DARKCYAN
        14          => 0x00804000, // CLR_BROWN
        15          => 0x00C0C0C0, // CLR_PALEGRAY
        n if n >= 16 => clr & 0x00FF_FFFF, // Direct 0x00RRGGBB — mask alpha
        _           => 0x00808080, // Unknown — grey
    }
}

/// Highest index of the default colour table (`CLR_PALEGRAY`).
pub const MAX_DEFAULT_COLOR_INDEX: u32 = 15;

/// Where a presentation space draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawTarget {
    /// Nothing (a presentation space with neither a window nor a bitmap).
    None,
    /// The surface of this frame window, composited onto the desktop.
    Frame(u32),
    /// This bitmap (selected into a memory presentation space).
    Bitmap(u32),
}

/// A bitmap created by `GpiCreateBitmap` / `GpiLoadBitmap`. Pixels are kept
/// as true colour whatever the format it was created with; `bit_count` and
/// `planes` are what the application asked for and are reported back.
pub struct Bitmap {
    pub surface: SharedSurface,
    pub bit_count: u16,
    pub planes: u16,
}

/// A device context opened with `DevOpenDC` / `WinOpenWindowDC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceContext {
    /// `OD_MEMORY`: a presentation space associated with it draws into the
    /// bitmap selected with `GpiSetBitmap`.
    Memory,
    /// `OD_INFO`: for `DevQueryCaps` only.
    Info,
    /// The window DC of this window: presentation spaces associated with it
    /// draw into the window.
    Window(u32),
}

/// Who created a presentation space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PsOrigin {
    /// `GpiCreatePS` (or internal): lives until `GpiDestroyPS`.
    Created,
    /// `WinBeginPaint` without a PS: freed by `WinEndPaint`.
    Paint,
    /// `WinGetPS`: freed by `WinReleasePS`.
    Cached,
}

pub struct PresentationSpace {
    pub hps: u32,
    pub hwnd: u32,
    /// Foreground (pen/text) colour, resolved to 0x00RRGGBB.
    pub color: u32,
    /// Background colour, resolved to 0x00RRGGBB.
    pub back_color: u32,
    /// The colour values as set (`GpiSetColor` / `GpiSetBackColor`): an
    /// index, a `CLR_*` special or an RGB value, reported back by the query
    /// functions and re-resolved when the colour table changes.
    pub color_attr: u32,
    pub back_color_attr: u32,
    /// `GpiCreateLogColorTable(LCOLF_RGB)`: colour values are RGB.
    pub rgb_mode: bool,
    /// Logical colour table entries loaded with `LCOLF_CONSECRGB` /
    /// `LCOLF_INDRGB`, overriding the default table (index → RGB).
    pub color_table: std::collections::BTreeMap<u32, u32>,
    /// `LINETYPE_*` set by `GpiSetLineType`.
    pub line_type: u32,
    /// Cosmetic line width (FIXED multiple of one pel; `LINEWIDTH_DEFAULT`
    /// = normal), set by `GpiSetLineWidth`.
    pub line_width: u32,
    /// Geometric line width (`GpiSetLineWidthGeom`), in world units. Only
    /// stroked paths use it, which Warpine does not have; it is stored and
    /// reported back.
    pub line_width_geom: i32,
    /// Arc parameters (`GpiSetArcParams`).
    pub arc_params: super::pm_shapes::ArcParams,
    /// Area pattern symbol (`PATSYM_*`, `GpiSetPattern`).
    pub pattern: u32,
    /// Pattern reference point (`GpiSetPatternRefPoint`).
    pub pattern_ref: (i32, i32),
    /// Associated device context (0 = none).
    pub hdc: u32,
    /// Bitmap selected with `GpiSetBitmap` (0 = none).
    pub bitmap: u32,
    /// Who created it, and so who frees it.
    pub origin: PsOrigin,
    /// Foreground mix mode (`FM_*`, set by `GpiSetMix`).
    pub mix_mode: u32,
    /// Background mix mode (`BM_*`, set by `GpiSetBackMix`).
    pub back_mix: u32,
    /// Current logical character set (LCID) selected via GpiSetCharSet.
    pub char_set: u32,
    /// Character box in world units (cx, cy); 0 means use default font metrics.
    pub char_box: (i32, i32),
    /// Logical fonts created by `GpiCreateLogFont`, keyed by LCID.
    pub log_fonts: HashMap<u32, FontSpec>,
    pub current_pos: (i32, i32),
    /// Codepage of the text this PS draws (`GpiSetCp` / `GpiQueryCp`):
    /// its window's message-queue codepage when created, 0 = not known
    /// (the process codepage is used).
    pub codepage: u32,
}

impl PresentationSpace {
    /// Resolve a colour value against this presentation space's colour table.
    pub fn resolve_color(&self, value: u32) -> u32 {
        if (value as i32) < 0 {
            default_color(value)
        } else if self.rgb_mode {
            value & 0x00FF_FFFF
        } else {
            self.color_table.get(&value).copied().unwrap_or_else(|| default_color(value))
        }
    }

    /// Set the foreground colour value and resolve it.
    pub fn set_color(&mut self, value: u32) {
        self.color_attr = value;
        self.color = self.resolve_color(value);
    }

    /// Set the background colour value and resolve it.
    pub fn set_back_color(&mut self, value: u32) {
        self.back_color_attr = value;
        self.back_color = self.resolve_color(value);
    }

    /// Re-resolve the current colours after the colour table changed.
    pub fn refresh_colors(&mut self) {
        self.set_color(self.color_attr);
        self.set_back_color(self.back_color_attr);
    }

    /// The entries of the colour table in force, lowest index first: the
    /// loaded entries over the 16 default ones. Empty in RGB mode.
    pub fn color_entries(&self) -> Vec<(u32, u32)> {
        if self.rgb_mode { return Vec::new(); }
        let mut all: std::collections::BTreeMap<u32, u32> =
            (0..=MAX_DEFAULT_COLOR_INDEX).map(|i| (i, default_color(i))).collect();
        all.extend(self.color_table.iter().map(|(&i, &rgb)| (i, rgb)));
        all.into_iter().collect()
    }

    /// The colour value `GpiQueryPel` reports for a pel of colour `rgb`:
    /// the RGB value in RGB mode; otherwise the lowest index of the colour
    /// table holding exactly that colour (`CLR_BLACK` for black, which the
    /// default table has no index for), or the RGB value itself when no entry
    /// matches (Warpine accepts RGB values in index mode too).
    pub fn value_of_pel(&self, rgb: u32) -> u32 {
        if self.rgb_mode { return rgb; }
        self.color_entries().into_iter().find(|&(_, c)| c == rgb).map(|(i, _)| i)
            .unwrap_or(if rgb == default_color(super::constants::CLR_BLACK as u32) {
                super::constants::CLR_BLACK as u32
            } else { rgb })
    }

    /// `GpiQueryColorIndex`: the index whose colour is nearest `rgb` (exact
    /// matches first); `rgb` itself in RGB mode.
    pub fn nearest_index(&self, rgb: u32) -> u32 {
        if self.rgb_mode { return rgb & 0x00FF_FFFF; }
        let dist = |c: u32| -> i64 {
            (0..3).map(|i| {
                let d = ((c >> (8 * i)) & 0xFF) as i64 - ((rgb >> (8 * i)) & 0xFF) as i64;
                d * d
            }).sum()
        };
        self.color_entries().into_iter().min_by_key(|&(i, c)| (dist(c), i)).map_or(0, |(i, _)| i)
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccelEntry {
    pub flags: u16,
    pub key: u16,
    pub cmd: u16,
}

/// A single item from a parsed OS/2 MENUTEMPLATE binary resource.
///
/// Populated by `Loader::parse_menu_items` and stored in
/// `OS2Window::menu_items` for WC_MENU windows created by `WinLoadMenu`.
#[derive(Clone, Debug, PartialEq)]
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
    /// Text-format clipboard content, kept in sync with the host SDL clipboard.
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
    /// The built-in control held down with mouse button 1 (0 = none): it
    /// receives the pointer's moves and the release wherever the pointer
    /// is — a button pops up, a text control extends its selection.
    pub pressed_control: u32,
    /// VDR-B4: set of HWNDs whose visual state has changed since the last composite.
    /// Any non-empty dirty set causes the next compositor tick to recomposite the
    /// full desktop; an empty set allows the tick to be skipped entirely.
    pub dirty: HashSet<u32>,
    /// Frame (or dialog) window → the surface its client area is drawn into.
    /// API threads draw into it directly; the compositor reads it. See
    /// `gui::surface` for the locking rule.
    pub surfaces: HashMap<u32, SharedSurface>,
    /// Bitmaps (`GpiCreateBitmap`, `GpiLoadBitmap`) by handle.
    pub bitmaps: HashMap<u32, Bitmap>,
    /// Device contexts (`DevOpenDC`, `WinOpenWindowDC`) by handle.
    pub dcs: HashMap<u32, DeviceContext>,
    next_hbm: u32,
    next_hdc: u32,
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
            pressed_control: 0,
            dirty: HashSet::new(),
            surfaces: HashMap::new(),
            bitmaps: HashMap::new(),
            dcs: HashMap::new(),
            next_hbm: 0x7000,
            next_hdc: 0x6000,
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
            listbox_sel: Default::default(),
            owner: 0,
            edit_caret: 0,
            edit_anchor: 0,
            caret_off: false,
            edit_scroll: 0,
            top_row: 0,
            menu_items: Vec::new(),
            menu_hwnd: 0,
            dialog_dismissed: false,
            dialog_result: 0,
            modal_owner: 0,
            destroy_on_dismiss: false,
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
            color: default_color(super::constants::CLR_DEFAULT as u32),
            back_color: default_color(super::constants::CLR_BACKGROUND),
            color_attr: super::constants::CLR_DEFAULT as u32,
            back_color_attr: super::constants::CLR_BACKGROUND,
            rgb_mode: false,
            color_table: std::collections::BTreeMap::new(),
            line_type: super::constants::LINETYPE_DEFAULT,
            line_width: super::constants::LINEWIDTH_DEFAULT,
            line_width_geom: 0,
            arc_params: super::pm_shapes::ArcParams::default(),
            pattern: super::constants::PATSYM_DEFAULT,
            pattern_ref: (0, 0),
            hdc: 0,
            bitmap: 0,
            origin: PsOrigin::Created,
            mix_mode: super::constants::FM_OVERPAINT,
            back_mix: super::constants::BM_LEAVEALONE,
            char_set: 0,
            char_box: (0, 0),
            log_fonts: HashMap::new(),
            current_pos: (0, 0),
            codepage: self.queue_codepage(hwnd).unwrap_or(0),
        });
        self.next_hps += 1;
        h
    }
    /// Create a message queue whose codepage starts as `codepage` (the
    /// process codepage at the time of the call).
    pub fn create_mq(&mut self, codepage: u32) -> u32 {
        let h = self.next_hmq;
        self.msg_queues.insert(h, Arc::new(Mutex::new(PM_MsgQueue {
            messages: VecDeque::new(),
            cond: Arc::new(Condvar::new()),
            lock: Arc::new(Mutex::new(false)),
            codepage,
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
        for (_, (stop, handle)) in self.timers.drain() {
            stop.stop();
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

    /// Class name of dialog windows created from a `DLGTEMPLATE`.
    pub const DIALOG_CLASS: &'static str = "#Dialog";

    /// Whether `hwnd` is a dialog: a frame whose controls are its own
    /// children (it has no client window) and which has its own surface.
    pub fn is_dialog(&self, hwnd: u32) -> bool {
        self.windows.get(&hwnd).is_some_and(|w| w.class_name == Self::DIALOG_CLASS)
    }

    /// Whether `hwnd` is a frame with its own surface: a standard frame
    /// (which has a client) or a dialog.
    pub fn is_frame(&self, hwnd: u32) -> bool {
        self.frame_to_client.contains_key(&hwnd) || self.is_dialog(hwnd)
    }

    /// The window under which `frame`'s controls live: a standard frame's
    /// client, or the dialog itself.
    pub fn control_root(&self, frame: u32) -> Option<u32> {
        match self.frame_to_client.get(&frame) {
            Some(&client) => Some(client),
            None => self.is_dialog(frame).then_some(frame),
        }
    }

    /// Walk up the parent chain to find the frame window `hwnd` draws into:
    /// the nearest dialog, or the frame of the nearest client window.
    ///
    /// Returns `hwnd` itself when there is no frame above it (a frame, or a
    /// window whose parent is the desktop or another pseudo window).
    pub fn find_frame_for_hwnd(&self, hwnd: u32) -> u32 {
        let mut h = hwnd;
        loop {
            if self.is_dialog(h) { return h; }
            let parent = match self.windows.get(&h) { Some(w) => w.parent, None => return h };
            if self.frame_to_client.contains_key(&parent) { return parent; }
            if !self.windows.contains_key(&parent) { return h; }
            h = parent;
        }
    }

    /// Compute the rectangle of `hwnd` in the coordinates of its frame's
    /// surface, bottom-left origin: relative to the client area of a
    /// standard frame, or to a dialog's surface, which starts inside the
    /// dialog's left border (control positions are relative to the dialog
    /// window, border included, as in PM).
    ///
    /// The caller can pass these directly as `(x1=x, y1=y, x2=x+cx, y2=y+cy)`.
    ///
    /// Returns `(x, y, cx, cy)`.
    pub fn get_abs_rect_in_frame(&self, hwnd: u32) -> (i32, i32, i32, i32) {
        let (cx, cy) = self.windows.get(&hwnd)
            .map(|w| (w.cx, w.cy)).unwrap_or((0, 0));
        let mut ax = 0i32;
        let mut ay = 0i32;
        let mut h = hwnd;
        while let Some(win) = self.windows.get(&h) {
            if self.is_dialog(h) {
                if h != hwnd {
                    ax -= super::constants::frame_border_width(win.frame_flags);
                }
                break;
            }
            ax += win.x;
            ay += win.y;
            let parent = win.parent;
            if parent == 0 { break; }
            if self.frame_to_client.contains_key(&parent) { break; }
            h = parent;
        }
        (ax, ay, cx, cy)
    }

    /// WinQueryWindow: the window related to `hwnd` by `code` (`QW_*`), or 0.
    ///
    /// A parent's `children` list runs bottom to top (a new window goes on
    /// top of its siblings), so `QW_TOP` is its last entry and `QW_NEXT`
    /// (the next window down) the entry before `hwnd`.
    pub fn query_window(&self, hwnd: u32, code: i32) -> u32 {
        use super::constants::{QW_BOTTOM, QW_NEXT, QW_OWNER, QW_PARENT, QW_PREV, QW_TOP};
        let Some(w) = self.windows.get(&hwnd) else { return 0 };
        let siblings = || self.windows.get(&w.parent).map(|p| p.children.as_slice()).unwrap_or(&[]);
        let children = w.children.as_slice();
        let pos = || siblings().iter().position(|&h| h == hwnd);
        match code {
            QW_PARENT => w.parent,
            QW_OWNER => w.owner,
            QW_TOP => children.last().copied().unwrap_or(0),
            QW_BOTTOM => children.first().copied().unwrap_or(0),
            QW_NEXT => pos().and_then(|i| i.checked_sub(1)).map_or(0, |i| siblings()[i]),
            QW_PREV => pos().and_then(|i| siblings().get(i + 1).copied()).unwrap_or(0),
            _ => 0,
        }
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
            // A hidden window hides its children; a dialog owned by this
            // window draws its controls into its own surface.
            if !w.visible || self.is_dialog(h) { continue; }
            if w.pfn_wp == 0 && w.class_name.starts_with("WC_") {
                out.push(h);
            }
            stack.extend(w.children.iter().rev().copied());
        }
        out
    }

    /// Whether `hwnd` is a push button (a `WC_BUTTON` of kind `BS_PUSHBUTTON`).
    pub fn is_push_button(&self, hwnd: u32) -> bool {
        use super::constants::{BS_PRIMARYSTYLES, BS_PUSHBUTTON};
        self.windows.get(&hwnd).is_some_and(|w| {
            w.class_name == "WC_BUTTON" && w.style & BS_PRIMARYSTYLES == BS_PUSHBUTTON
        })
    }

    /// Whether `hwnd` can hold the keyboard focus: an enabled, visible
    /// entry field, MLE, list box or button.
    pub fn can_take_focus(&self, hwnd: u32) -> bool {
        use super::constants::WS_DISABLED;
        self.windows.get(&hwnd).is_some_and(|w| {
            w.visible && w.style & WS_DISABLED == 0
                && matches!(w.class_name.as_str(), "WC_ENTRYFIELD" | "WC_MLE" | "WC_LISTBOX" | "WC_BUTTON")
        })
    }

    /// The controls of `dialog` that Tab visits, in order: `WS_TABSTOP`
    /// controls that can take the focus.
    pub fn tab_stops(&self, dialog: u32) -> Vec<u32> {
        use super::constants::WS_TABSTOP;
        self.builtin_control_descendants(dialog).into_iter().filter(|&h| {
            self.can_take_focus(h) && self.windows[&h].style & WS_TABSTOP != 0
        }).collect()
    }

    /// The tab stop after the focus window (before it when `backward`),
    /// wrapping around; the first (last) tab stop when the focus is
    /// elsewhere. `None` when `dialog` has no tab stops.
    pub fn next_tab_stop(&self, dialog: u32, backward: bool) -> Option<u32> {
        let stops = self.tab_stops(dialog);
        let n = stops.len();
        if n == 0 { return None; }
        let i = match stops.iter().position(|&h| h == self.focus_hwnd) {
            Some(i) if backward => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
            None if backward => n - 1,
            None => 0,
        };
        Some(stops[i])
    }

    /// The control after the focus window (before it when `backward`) in
    /// its group, wrapping around: the controls of `dialog` from one
    /// `WS_GROUP` control up to the next, those that can take the focus.
    /// `None` when the focus is not in `dialog` or alone in its group.
    pub fn next_in_group(&self, dialog: u32, backward: bool) -> Option<u32> {
        use super::constants::WS_GROUP;
        let controls = self.builtin_control_descendants(dialog);
        let at = controls.iter().position(|&h| h == self.focus_hwnd)?;
        let starts_group = |h: &u32| self.windows[h].style & WS_GROUP != 0;
        let start = controls[..=at].iter().rposition(starts_group).unwrap_or(0);
        let end = controls[at + 1..].iter().position(starts_group).map_or(controls.len(), |i| at + 1 + i);
        let group: Vec<u32> = controls[start..end].iter().copied()
            .filter(|&h| h == self.focus_hwnd || self.can_take_focus(h)).collect();
        let i = group.iter().position(|&h| h == self.focus_hwnd)?;
        let n = group.len();
        if n < 2 { return None; }
        Some(group[if backward { (i + n - 1) % n } else { (i + 1) % n }])
    }

    /// The control that first gets the focus in `dialog`: `preferred` (the
    /// template's choice) if it can take the focus, else the first tab stop,
    /// else the first control that can take the focus.
    pub fn initial_dialog_focus(&self, dialog: u32, preferred: Option<u32>) -> Option<u32> {
        preferred.filter(|&h| self.can_take_focus(h))
            .or_else(|| self.tab_stops(dialog).first().copied())
            .or_else(|| self.builtin_control_descendants(dialog).into_iter().find(|&h| self.can_take_focus(h)))
    }

    /// The default push button (`BS_DEFAULT`) of `dialog`, if it has one.
    pub fn default_button(&self, dialog: u32) -> Option<u32> {
        use super::constants::{BS_DEFAULT, WS_DISABLED};
        self.builtin_control_descendants(dialog).into_iter().find(|&h| {
            let w = &self.windows[&h];
            self.is_push_button(h) && w.style & BS_DEFAULT != 0 && w.style & WS_DISABLED == 0
        })
    }

    /// Accelerator table `haccel`.
    pub fn accel_table(&self, haccel: u32) -> Option<&Vec<AccelEntry>> {
        self.accel_tables.get(&haccel)
    }

    /// The accelerator table set for `hwnd` (0 = the queue's table).
    pub fn accel_table_of(&self, hwnd: u32) -> Option<&Vec<AccelEntry>> {
        self.accel_tables.get(self.window_accel.get(&hwnd)?)
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

    /// Post `WM_CONTROL` to the owner of control `hwnd`:
    /// mp1 = MPFROM2SHORT(control id, notify code), mp2 = `data`.
    pub fn notify_owner(&self, hwnd: u32, code: u32, data: u32) {
        use super::constants::WM_CONTROL;
        let target = self.notify_target(hwnd);
        let Some(id) = self.windows.get(&hwnd).map(|w| w.id) else { return };
        if target == 0 || target == hwnd {
            return;
        }
        if let Some(mq_arc) = self.find_hmq_for_hwnd(target).and_then(|hmq| self.get_mq(hmq)) {
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(OS2Message {
                hwnd: target, msg: WM_CONTROL, mp1: (code << 16) | (id & 0xFFFF), mp2: data,
                time: 0, x: 0, y: 0,
            });
            mq.cond.notify_one();
        }
    }

    /// Tell the owner of control `hwnd` that it gained (`gained`) or lost
    /// the focus: `EN_SETFOCUS` / `EN_KILLFOCUS` for an entry field,
    /// `MLN_*` for an MLE, `LN_*` for a list box (mp2 = the control). Other
    /// windows have no such notification.
    pub fn notify_focus(&self, hwnd: u32, gained: bool) {
        use super::constants::*;
        let Some(class) = self.windows.get(&hwnd).map(|w| w.class_name.as_str()) else { return };
        let code = match (class, gained) {
            ("WC_ENTRYFIELD", true) => EN_SETFOCUS,
            ("WC_ENTRYFIELD", false) => EN_KILLFOCUS,
            ("WC_MLE", true) => MLN_SETFOCUS,
            ("WC_MLE", false) => MLN_KILLFOCUS,
            ("WC_LISTBOX", true) => LN_SETFOCUS,
            ("WC_LISTBOX", false) => LN_KILLFOCUS,
            _ => return,
        };
        self.notify_owner(hwnd, code, hwnd);
    }

    /// Make `hwnd` the focus window (0 = none), notifying the owners of the
    /// control that lost it and the one that gained it. Returns the
    /// previous focus window.
    pub fn set_focus(&mut self, hwnd: u32) -> u32 {
        let old = std::mem::replace(&mut self.focus_hwnd, hwnd);
        // A control taking the focus shows its caret at once.
        if let Some(w) = self.windows.get_mut(&hwnd) {
            w.caret_off = false;
        }
        if old != hwnd {
            self.notify_focus(old, false);
            self.notify_focus(hwnd, true);
        }
        old
    }

    /// One step of the caret blink: post `WM_TIMER(TID_CURSOR)` to the focus
    /// window when it is an enabled entry field or MLE, unless one is still
    /// waiting in its queue (timer messages do not pile up behind a busy
    /// application). Returns the window posted to.
    pub fn post_caret_blink(&self) -> Option<u32> {
        use super::constants::{TID_CURSOR, WM_TIMER, WS_DISABLED};
        let hwnd = self.focus_hwnd;
        let w = self.windows.get(&hwnd)?;
        if !matches!(w.class_name.as_str(), "WC_ENTRYFIELD" | "WC_MLE") || w.style & WS_DISABLED != 0 {
            return None;
        }
        let mq_arc = self.find_hmq_for_hwnd(hwnd).and_then(|hmq| self.get_mq(hmq))?;
        let mut mq = mq_arc.lock_or_recover();
        let pending = mq.messages.iter()
            .any(|m| m.hwnd == hwnd && m.msg == WM_TIMER && m.mp1 == TID_CURSOR);
        if !pending {
            mq.messages.push_back(OS2Message {
                hwnd, msg: WM_TIMER, mp1: TID_CURSOR, mp2: 0, time: 0, x: 0, y: 0,
            });
            mq.cond.notify_one();
        }
        Some(hwnd)
    }

    /// Hit-test point `(x, y)` (client coordinates of `frame`, origin at the
    /// bottom-left) against the frame's built-in controls. Returns the
    /// top-most enabled control under the point together with the point in
    /// that control's own coordinates. Static text is transparent to the
    /// pointer.
    pub fn control_at(&self, frame: u32, x: i32, y: i32) -> Option<(u32, i32, i32)> {
        use super::constants::WS_DISABLED;
        let root = self.control_root(frame)?;
        self.builtin_control_descendants(root).into_iter().rev().find_map(|h| {
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

    // ── Drawing surfaces ─────────────────────────────────────────────────────

    /// The surface `frame` is drawn into, created on first use (dialogs are
    /// first drawn without having been created as standard windows). A new
    /// surface covers the frame below its title bar and menu bar, or the
    /// whole desktop while the window has no size yet. `None` for handles
    /// that are not windows.
    pub fn frame_surface(&mut self, frame: u32) -> Option<SharedSurface> {
        if let Some(s) = self.surfaces.get(&frame) {
            return Some(s.clone());
        }
        let win = self.windows.get(&frame)?;
        let (w, h) = if win.cx > 0 && win.cy > 0 {
            (win.cx as u32, (win.cy - self.frame_chrome_top_h(frame)).max(1) as u32)
        } else {
            super::constants::desktop_size()
        };
        let s = Surface::shared(w, h);
        self.surfaces.insert(frame, s.clone());
        Some(s)
    }

    /// Where presentation space `hps` draws: its selected bitmap, else the
    /// frame of its window.
    pub fn ps_draw_target(&self, hps: u32) -> Option<DrawTarget> {
        let ps = self.ps_map.get(&hps)?;
        Some(if ps.bitmap != 0 {
            DrawTarget::Bitmap(ps.bitmap)
        } else if ps.hwnd != 0 {
            DrawTarget::Frame(self.client_to_frame(ps.hwnd))
        } else {
            DrawTarget::None
        })
    }

    /// The surface behind `target` (a frame's is created on first use).
    pub fn target_surface(&mut self, target: DrawTarget) -> Option<SharedSurface> {
        match target {
            DrawTarget::None => None,
            DrawTarget::Frame(frame) => self.frame_surface(frame),
            DrawTarget::Bitmap(hbm) => self.bitmaps.get(&hbm).map(|b| b.surface.clone()),
        }
    }

    /// Register a bitmap of `width` × `height` pels and return its handle.
    pub fn create_bitmap(&mut self, surface: Surface, bit_count: u16, planes: u16) -> u32 {
        let h = self.next_hbm;
        self.next_hbm += 1;
        self.bitmaps.insert(h, Bitmap {
            surface: Arc::new(Mutex::new(surface)), bit_count, planes,
        });
        h
    }

    /// The presentation space `hbm` is selected into, if any.
    pub fn bitmap_owner(&self, hbm: u32) -> Option<u32> {
        self.ps_map.values().find(|ps| ps.bitmap == hbm).map(|ps| ps.hps)
    }

    /// Register a device context and return its handle.
    pub fn open_dc(&mut self, dc: DeviceContext) -> u32 {
        let h = self.next_hdc;
        self.next_hdc += 1;
        self.dcs.insert(h, dc);
        h
    }

    /// Resize the surface of `hwnd`, if it has one, keeping its content.
    pub fn resize_surface(&self, hwnd: u32, width: i32, height: i32) {
        if let Some(s) = self.surfaces.get(&hwnd) {
            s.lock_or_recover().resize(width.max(1) as u32, height.max(1) as u32);
        }
    }

    /// VDR-B4: mark an HWND (frame or client) as visually dirty.
    /// The compositor will recomposite the full desktop on its next frame;
    /// the GUI thread is woken, since it blocks while nothing is dirty.
    pub fn mark_dirty(&mut self, hwnd: u32) {
        self.dirty.insert(hwnd);
        self.wake_gui();
    }

    /// VDR-B4: mark every frame in the Z-order dirty.
    /// Called after Z-order changes, which affect all overlapping windows.
    pub fn mark_all_dirty(&mut self) {
        self.dirty.extend(self.z_order.iter().copied());
        self.wake_gui();
    }

    /// Wake the GUI thread (no-op before the GUI channel exists).
    fn wake_gui(&self) {
        if let Some(tx) = &self.gui_tx {
            tx.wake();
        }
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

    /// The codepage of the message queue `hwnd` belongs to (`WinSetCp`), or
    /// `None` for an unknown window or one without a queue.
    pub fn queue_codepage(&self, hwnd: u32) -> Option<u32> {
        let hmq = self.windows.get(&hwnd)?.hmq;
        Some(self.msg_queues.get(&hmq)?.lock_or_recover().codepage)
    }

    /// Whether `hwnd` is `ancestor` or lies below it in the window tree.
    pub fn is_same_or_descendant(&self, hwnd: u32, ancestor: u32) -> bool {
        let mut h = hwnd;
        // A tree is never deeper than the number of windows.
        for _ in 0..=self.windows.len() {
            if h == ancestor { return true; }
            match self.windows.get(&h) {
                Some(w) if w.parent != 0 => h = w.parent,
                _ => return false,
            }
        }
        false
    }

    /// Whether `hwnd` is an existing window (`WinIsWindow`).
    pub fn is_window(&self, hwnd: u32) -> bool {
        self.windows.contains_key(&hwnd)
    }

    /// Whether input to frame `frame` is blocked: it is disabled, as the
    /// owner of a modal dialog is.
    pub fn frame_disabled(&self, frame: u32) -> bool {
        use super::constants::WS_DISABLED;
        self.windows.get(&frame).is_some_and(|w| w.style & WS_DISABLED != 0)
    }

    /// Destroy `hwnd` and all its descendants (`WinDestroyWindow`): they are
    /// removed from the window tree, the Z-order and the frame map, lose
    /// their surfaces, accelerator tables and timers, and stop being the
    /// focus, capture or pressed button. Returns `false` for an unknown
    /// window. The window procedures are not called (no `WM_DESTROY`).
    pub fn destroy_window(&mut self, hwnd: u32) -> bool {
        let Some(parent) = self.windows.get(&hwnd).map(|w| w.parent) else { return false };
        if let Some(p) = self.windows.get_mut(&parent) {
            p.children.retain(|&c| c != hwnd);
        }
        let mut doomed = vec![hwnd];
        let mut i = 0;
        while i < doomed.len() {
            if let Some(w) = self.windows.get(&doomed[i]) {
                doomed.extend(w.children.iter().copied());
            }
            i += 1;
        }
        for &h in &doomed {
            if self.z_order.contains(&h) {
                self.z_remove(h);
            }
            self.windows.remove(&h);
            self.frame_to_client.remove(&h);
            self.surfaces.remove(&h);
            self.window_accel.remove(&h);
            self.dirty.remove(&h);
            for slot in [&mut self.focus_hwnd, &mut self.capture_hwnd, &mut self.pressed_control] {
                if *slot == h { *slot = 0; }
            }
        }
        // Stop the timers of the destroyed windows; their threads end on
        // their own (joining here could wait on this lock).
        self.timers.retain(|&(h, _), (stop, _)| {
            let keep = !doomed.contains(&h);
            if !keep { stop.stop(); }
            keep
        });
        self.mark_all_dirty();
        true
    }

    /// Start the modal loop of `dialog`: its owner's frame is disabled while
    /// it runs (if it was enabled), and `destroy` says whether the dialog
    /// goes away when the loop ends.
    pub fn begin_modal_dialog(&mut self, dialog: u32, destroy: bool) {
        use super::constants::WS_DISABLED;
        let Some(owner) = self.windows.get(&dialog).map(|w| w.owner) else { return };
        let frame = if self.windows.contains_key(&owner) { self.find_frame_for_hwnd(owner) } else { 0 };
        let disable = frame != 0 && frame != dialog && !self.frame_disabled(frame);
        if disable && let Some(f) = self.windows.get_mut(&frame) {
            f.style |= WS_DISABLED;
        }
        if let Some(d) = self.windows.get_mut(&dialog) {
            d.modal_owner = if disable { frame } else { 0 };
            d.destroy_on_dismiss = destroy;
        }
    }

    /// End the modal loop of `dialog`: take it off the screen
    /// (`close_dialog`), enable the owner frame it disabled, and destroy it
    /// when it was started that way. Returns the dialog result
    /// (`DID_CANCEL` when the dialog no longer exists).
    pub fn finish_modal_dialog(&mut self, dialog: u32) -> u32 {
        use super::constants::{DID_CANCEL, WS_DISABLED};
        let Some((result, owner, destroy, dismissed)) = self.windows.get(&dialog)
            .map(|w| (w.dialog_result, w.modal_owner, w.destroy_on_dismiss, w.dialog_dismissed))
            else { return DID_CANCEL };
        self.close_dialog(dialog);
        if owner != 0 && let Some(f) = self.windows.get_mut(&owner) {
            f.style &= !WS_DISABLED;
        }
        if destroy {
            self.destroy_window(dialog);
        } else if let Some(d) = self.windows.get_mut(&dialog) {
            d.modal_owner = 0;
        }
        if dismissed { result } else { DID_CANCEL }
    }

    /// Take a dismissed dialog off the screen: out of the Z-order (the
    /// window below becomes active) and, if one of its controls has the
    /// focus, the focus is released.
    pub fn close_dialog(&mut self, dialog: u32) {
        self.z_remove(dialog);
        if self.find_frame_for_hwnd(self.focus_hwnd) == dialog {
            self.focus_hwnd = 0;
        }
        if self.find_frame_for_hwnd(self.pressed_control) == dialog {
            self.pressed_control = 0;
        }
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
    /// Stopping a timer wakes its sleeping thread at once (WinStopTimer used
    /// to block for the rest of the period).
    #[test]
    fn timer_stop_interrupts_the_sleep() {
        use std::time::{Duration, Instant};
        let stop = std::sync::Arc::new(super::TimerStop::default());
        assert!(!stop.sleep(Duration::from_millis(5)), "times out when not stopped");
        let s2 = stop.clone();
        let t = std::thread::spawn(move || s2.sleep(Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(20));
        let start = Instant::now();
        stop.stop();
        assert!(t.join().unwrap(), "reports the stop");
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(stop.is_stopped() && stop.sleep(Duration::from_secs(60)), "stays stopped");
    }

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
    fn query_window_follows_parent_owner_and_sibling_order() {
        use crate::loader::constants::{QW_BOTTOM, QW_NEXT, QW_OWNER, QW_PARENT, QW_PREV, QW_TOP};
        let mut wm = WindowManager::new();
        let parent = wm.create_window("App".to_string(), 0, 0);
        let owner = wm.create_window("App".to_string(), 0, 0);
        // Created bottom to top: `last` is the top-most sibling.
        let first = wm.create_window("WC_BUTTON".to_string(), parent, 0);
        let middle = wm.create_window("WC_BUTTON".to_string(), parent, 0);
        let last = wm.create_window("WC_BUTTON".to_string(), parent, 0);
        wm.get_window_mut(middle).unwrap().owner = owner;

        assert_eq!(wm.query_window(middle, QW_PARENT), parent);
        assert_eq!(wm.query_window(middle, QW_OWNER), owner);
        assert_eq!(wm.query_window(first, QW_OWNER), 0, "no owner");
        assert_eq!(wm.query_window(parent, QW_TOP), last);
        assert_eq!(wm.query_window(parent, QW_BOTTOM), first);
        assert_eq!(wm.query_window(middle, QW_NEXT), first, "next = one lower");
        assert_eq!(wm.query_window(middle, QW_PREV), last, "previous = one higher");
        assert_eq!(wm.query_window(first, QW_NEXT), 0, "bottom has no next");
        assert_eq!(wm.query_window(last, QW_PREV), 0, "top has no previous");
        assert_eq!(wm.query_window(first, QW_TOP), 0, "no children");
        assert_eq!(wm.query_window(middle, 99), 0, "unknown code");
        assert_eq!(wm.query_window(0xDEAD, QW_PARENT), 0, "unknown window");
    }

    #[test]
    fn destroy_window_removes_the_whole_subtree() {
        let mut wm = WindowManager::new();
        let frame = wm.create_window("App".into(), 1, 0);
        let client = wm.create_window("App".into(), frame, 0);
        let button = wm.create_window("WC_BUTTON".into(), client, 0);
        let other = wm.create_window("App".into(), 1, 0);
        wm.frame_to_client.insert(frame, client);
        wm.z_push_top(other);
        wm.z_push_top(frame);
        wm.focused_hwnd = frame;
        wm.frame_surface(frame);
        (wm.focus_hwnd, wm.pressed_control, wm.capture_hwnd) = (button, button, client);
        let t = wm.add_accel_table(Vec::new());
        wm.set_window_accel(frame, t);

        assert!(wm.destroy_window(frame));
        for h in [frame, client, button] {
            assert!(!wm.is_window(h));
        }
        assert!(wm.is_window(other));
        assert!(!wm.z_order.contains(&frame) && !wm.frame_to_client.contains_key(&frame));
        assert!(!wm.surfaces.contains_key(&frame) && wm.accel_table_of(frame).is_none());
        assert_eq!((wm.focus_hwnd, wm.pressed_control, wm.capture_hwnd), (0, 0, 0));
        assert_eq!(wm.focused_hwnd, other, "the next window is active");
        assert!(!wm.destroy_window(frame), "already gone");

        // A child is unlinked from its parent.
        let child = wm.create_window("WC_BUTTON".into(), other, 0);
        assert!(wm.destroy_window(child));
        assert!(wm.get_window(other).unwrap().children.is_empty());
    }

    #[test]
    fn queue_codepage_follows_the_window_queue() {
        let mut wm = WindowManager::new();
        let hmq = wm.create_mq(850);
        let w = wm.create_window("A".into(), 0, hmq);
        let orphan = wm.create_window("B".into(), 0, 0);
        assert_eq!(wm.queue_codepage(w), Some(850));
        wm.get_mq(hmq).unwrap().lock().unwrap().codepage = 1004;
        assert_eq!(wm.queue_codepage(w), Some(1004));
        assert_eq!(wm.queue_codepage(orphan), None);
        assert_eq!(wm.queue_codepage(0xDEAD), None);
        // A PS created for the window starts with its queue's codepage.
        let hps = wm.create_ps(w);
        assert_eq!(wm.ps_map[&hps].codepage, 1004);
        let hps_orphan = wm.create_ps(orphan);
        assert_eq!(wm.ps_map[&hps_orphan].codepage, 0);
    }

    #[test]
    fn descendants_are_found_through_parents() {
        let mut wm = WindowManager::new();
        let a = wm.create_window("A".into(), 0, 0);
        let b = wm.create_window("B".into(), a, 0);
        let c = wm.create_window("C".into(), b, 0);
        assert!(wm.is_same_or_descendant(c, a));
        assert!(wm.is_same_or_descendant(a, a));
        assert!(!wm.is_same_or_descendant(a, c));
        assert!(!wm.is_same_or_descendant(0xDEAD, a));
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

    /// The GUI thread blocks while nothing is dirty, so marking a window
    /// dirty (e.g. from WinSetWindowPos on the guest thread) must wake it.
    #[test]
    fn marking_dirty_wakes_the_gui_thread() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        let (tx, rx) = crate::gui::create_gui_channel();
        let wake = rx.wake_handle();
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        wake.set_hook(Box::new(move || { c.fetch_add(1, Ordering::Relaxed); }));

        let mut wm = WindowManager::new();
        wm.mark_dirty(0x1000); // no channel yet: must not panic
        wm.gui_tx = Some(tx);
        wm.mark_dirty(0x1000);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        wake.rearm();
        wm.mark_all_dirty();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
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
