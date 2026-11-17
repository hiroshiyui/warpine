// SPDX-License-Identifier: GPL-3.0-only

use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use sdl3::event::Event;
use sdl3::keyboard::{Keycode, Mod as KeyMod, Scancode};
use log::debug;
use crate::loader::{SharedState, OS2Message, MutexExt,
    WM_CLOSE, WM_MOUSEMOVE,
    WM_BUTTON1DOWN, WM_BUTTON1UP, WM_BUTTON2DOWN, WM_BUTTON2UP, WM_BUTTON3DOWN, WM_BUTTON3UP,
    WM_BUTTON1DBLCLK, WM_BUTTON2DBLCLK, WM_BUTTON3DBLCLK,
    KC_CHAR, KC_VIRTUALKEY, KC_SCANCODE, KC_SHIFT, KC_CTRL, KC_ALT, KC_KEYUP,
    VK_BACKSPACE, VK_TAB, VK_BACKTAB, VK_NEWLINE, VK_ESC, VK_SPACE,
    VK_PAGEUP, VK_PAGEDOWN, VK_END, VK_HOME, VK_LEFT, VK_UP, VK_RIGHT, VK_DOWN,
    VK_INSERT, VK_DELETE, VK_SCRLLOCK, VK_NUMLOCK, VK_ENTER,
    VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6,
    VK_F7, VK_F8, VK_F9, VK_F10, VK_F11, VK_F12,
    VK_SHIFT, VK_CTRL, VK_ALT, VK_ALTGRAF, VK_CAPSLOCK, VK_PAUSE, VK_PRINTSCRN,
    CF_TEXT, KC_PREVDOWN, WM_CHAR_REPEAT_SHIFT, WM_CHAR_SCANCODE_SHIFT, WM_CHAR_VKEY_SHIFT,
    FCF_TITLEBAR, FCF_SYSMENU, FCF_MINBUTTON, FCF_MAXBUTTON, FCF_HIDEBUTTON,
    FCF_SIZEBORDER, FCF_BORDER,
    WM_ACTIVATE, WM_SYSCOMMAND, SC_MINIMIZE, SC_MAXIMIZE, SC_RESTORE, SC_CLOSE, SC_HIDE,
    WS_MINIMIZED, WS_MAXIMIZED, CMDSRC_MENU, CMDSRC_OTHER};
use crate::loader::constants::{
    MENU_BAR_HEIGHT, MENU_BAR_ITEM_INSET, MENU_BAR_ITEM_PAD, MENU_DROPDOWN_W, MENU_DROPDOWN_ITEM_H,
    MIS_SEPARATOR, MIS_SYSCOMMAND, MIS_HELP, SC_MOVE, SC_SIZE, WM_COMMAND, WM_HELP};
use super::frame_track::{KeyTrack, TrackKey, TrackOutcome};
use super::system_menu::{can_track, frame_system_menu_items, menu_popup_items};
use crate::loader::pm_minmax::WinRect;
use super::popup_menu::{PopupAction, PopupKey, PopupMenu};
use super::vio_window::system_menu_items;
use crate::loader::pm_types::{MenuItem, WindowManager, menu_label, menu_bar_item_width, owner_menu_item_w};
use crate::loader::pm_input::InputEvent;
use std::time::{Duration, Instant};
use super::damage::{Damage, Rect, blit_clipped, fill_clipped};
use super::cursor::CursorMark;

mod compositor;
use compositor::{Compositor, CompositorKind, Overlay, Scene, SceneFrame};
use super::frame_stats::FrameStats;
use super::message::{GUIMessage, GuiWake};
use super::renderer::PmRenderer;
use super::font::{self, CjkRegion, FontSpec};
use super::gui_script::{self, GuiScript};
use super::surface::{SharedSurface, Surface};

// ── VDR-A: Single-surface desktop renderer ────────────────────────────────

/// Desktop background colour (OS/2-style teal).
const DESKTOP_BG: u32 = 0x00408040;

// ── Event loop pacing ─────────────────────────────────────────────────────

/// Minimum time between composites (~60 Hz). Presents arriving faster are
/// coalesced into the next frame.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// Longest the loop blocks with nothing to do. Bounds how long it takes to
/// notice state changes that arrive without a wake-up, such as
/// `exit_requested` set by the guest thread.
const IDLE_WAIT: Duration = Duration::from_millis(100);
/// Half period of the caret blink until the guest sets `SV_CURSORRATE`.
const CARET_BLINK: Duration = Duration::from_millis(crate::loader::constants::CARET_BLINK_MS);
/// Interval of the drag scroll timer (`WM_TIMER(TID_SCROLL)`).
const DRAG_SCROLL: Duration = Duration::from_millis(crate::loader::constants::DRAG_SCROLL_MS);

/// When the drag scroll timer fires next: off while no control is held,
/// armed one interval after a control is first seen held, rearmed after
/// each tick. Returns the new deadline and whether a tick is due now.
fn drag_scroll_step(held: bool, next: Option<Instant>, now: Instant) -> (Option<Instant>, bool) {
    match (held, next) {
        (false, _) => (None, false),
        (true, None) => (Some(now + DRAG_SCROLL), false),
        (true, Some(t)) if now >= t => (Some(now + DRAG_SCROLL), true),
        (true, Some(t)) => (Some(t), false),
    }
}

/// How long the GUI loop may block before it has work again.
///
/// `busy` means a composite is owed (windows are dirty or a full redraw is
/// pending): the loop then wakes for the next frame. `timed_next` is when
/// the next timed event is due (scripted step, caret blink). Input and
/// wake-ups end the wait early.
fn wait_timeout(now: Instant, last_composite: Instant, busy: bool,
                timed_next: Option<Instant>) -> Duration {
    let mut deadline = now + IDLE_WAIT;
    if busy {
        deadline = deadline.min(last_composite + FRAME_INTERVAL);
    }
    if let Some(t) = timed_next {
        deadline = deadline.min(t);
    }
    deadline.saturating_duration_since(now)
}

// ── VDR-C: Window chrome constants ────────────────────────────────────────

use crate::loader::constants::{CHROME_BORDER_W, CHROME_TITLE_H};

/// Width of each title-bar button (close/min/max/sysmenu), in desktop pixels.
const CHROME_BTN_W: i32   = 16;
/// Offset of the system-menu icon from the frame's left edge.
const SYSMENU_ICON_X: i32 = 3;
/// Gap between adjacent title-bar buttons.
const CHROME_BTN_GAP: i32 = 2;
/// Right margin before the close button.
const CHROME_BTN_MARGIN: i32 = 2;
/// Resize-hit zone width at each window edge (VDR-C5).
const RESIZE_HIT_W: i32 = 6;

/// Active (focused) title bar colour — OS/2 navy blue.
const CHROME_TITLE_ACTIVE: u32   = 0x0000_0080;
/// Inactive title bar colour — dark gray.
const CHROME_TITLE_INACTIVE: u32 = 0x0040_4040;
/// Title bar text colour.
const CHROME_TEXT_COLOR: u32     = 0x00FF_FFFF;

/// Draw text into a desktop-space pixel buffer.
///
/// `sx`/`sy` are screen coordinates (y=0 at top, x=0 at left) of the top-left
/// corner of the text's line box, which is `spec.px` pixels tall.
#[allow(clippy::too_many_arguments)]
fn draw_text_screen(buf: &mut [u32], buf_w: usize, buf_h: usize,
                    sx: i32, sy: i32, text: &str, color: u32, spec: FontSpec) {
    let baseline = sy + font::metrics(spec).ascent;
    font::draw_text(buf, buf_w, buf_h, sx, baseline, text, color, spec);
}

// ── Frame chrome ─────────────────────────────────────────────────────────────

/// Face colour of raised chrome elements (buttons, sizing border).
const CHROME_FACE_COLOR: u32   = 0x00CC_CCCC;
/// Lit (top/left) edge of a raised bevel.
const CHROME_LIGHT_COLOR: u32  = 0x00FF_FFFF;
/// Shaded (bottom/right) edge of a raised bevel.
const CHROME_SHADOW_COLOR: u32 = 0x0080_8080;
/// Outermost dark edge and button glyphs.
const CHROME_DARK_COLOR: u32   = 0x0000_0000;
/// Vertical inset of the title-bar buttons inside the title bar.
const CHROME_BTN_INSET_Y: i32  = 2;

/// Geometry and state of one frame's chrome, in SDL screen space.
struct FrameChrome<'a> {
    x: i32,
    /// Screen y of the frame's top edge.
    top: i32,
    cx: i32,
    cy: i32,
    frame_flags: u32,
    title: &'a str,
    focused: bool,
    /// Regional face of CJK characters in the title (the frame's queue
    /// codepage).
    cjk: CjkRegion,
    /// Title-bar colours (the system colours).
    colors: TitleColors,
    /// `WS_MINIMIZED` / `WS_MAXIMIZED` of the frame: a minimized frame's
    /// title bar has no buttons, a maximized frame's maximize button is a
    /// restore button.
    state: u32,
}

/// The colours of a title bar: `SYSCLR_ACTIVETITLE` / `SYSCLR_INACTIVETITLE`
/// and their text colours, which `WinSetSysColors` changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TitleColors {
    active: u32,
    inactive: u32,
    active_text: u32,
    inactive_text: u32,
}

impl Default for TitleColors {
    fn default() -> Self {
        TitleColors { active: CHROME_TITLE_ACTIVE, inactive: CHROME_TITLE_INACTIVE,
                      active_text: CHROME_TEXT_COLOR, inactive_text: CHROME_TEXT_COLOR }
    }
}

impl TitleColors {
    /// The title-bar colours of the system colour table.
    fn of(table: &crate::loader::pm_syscolor::SysColorTable) -> Self {
        use crate::loader::constants::{SYSCLR_ACTIVETITLE, SYSCLR_ACTIVETITLETEXT, SYSCLR_INACTIVETITLE,
                                       SYSCLR_INACTIVETITLETEXT};
        let d = TitleColors::default();
        TitleColors {
            active: table.get(SYSCLR_ACTIVETITLE).unwrap_or(d.active),
            inactive: table.get(SYSCLR_INACTIVETITLE).unwrap_or(d.inactive),
            active_text: table.get(SYSCLR_ACTIVETITLETEXT).unwrap_or(d.active_text),
            inactive_text: table.get(SYSCLR_INACTIVETITLETEXT).unwrap_or(d.inactive_text),
        }
    }
}

/// Which pictogram a title-bar button carries.
#[derive(Clone, Copy)]
enum ButtonGlyph { Close, Maximize, Restore, Minimize, Hide, SysMenu }

/// The frame flags a frame in `state` is drawn and hit-tested with: a
/// minimized frame is a bordered title bar whatever its own flags.
fn display_flags(frame_flags: u32, state: u32) -> u32 {
    if state & WS_MINIMIZED != 0 { FCF_TITLEBAR | FCF_BORDER } else { frame_flags }
}

/// Fill `[x0,x1) × [y0,y1)` with `face` and give it a 1-px raised bevel:
/// `light` along the top and left edges, `shadow` along the bottom and right.
#[allow(clippy::too_many_arguments)]
fn draw_raised_box(buf: &mut [u32], buf_w: usize, x0: i32, y0: i32, x1: i32, y1: i32,
                   face: u32, light: u32, shadow: u32) {
    draw_filled_rect(buf, buf_w, x0, y0, x1, y1, face);
    draw_filled_rect(buf, buf_w, x0, y0, x1, y0 + 1, light);
    draw_filled_rect(buf, buf_w, x0, y0, x0 + 1, y1, light);
    draw_filled_rect(buf, buf_w, x0, y1 - 1, x1, y1, shadow);
    draw_filled_rect(buf, buf_w, x1 - 1, y0, x1, y1, shadow);
}

/// Draw one raised title-bar button with its pictogram. `(x, y)` is the
/// button's top-left corner; it is `CHROME_BTN_W` wide and `h` tall.
fn draw_chrome_button(buf: &mut [u32], buf_w: usize, x: i32, y: i32, h: i32, glyph: ButtonGlyph) {
    let w = CHROME_BTN_W;
    draw_raised_box(buf, buf_w, x, y, x + w, y + h,
                    CHROME_FACE_COLOR, CHROME_LIGHT_COLOR, CHROME_SHADOW_COLOR);
    // Pictogram box: the button interior minus a 4-px margin.
    let (gx0, gy0, gx1, gy1) = (x + 4, y + 4, x + w - 4, y + h - 4);
    let ink = CHROME_DARK_COLOR;
    match glyph {
        ButtonGlyph::Close => {
            // Two 2-px-wide diagonals.
            let n = (gx1 - gx0).min(gy1 - gy0);
            for i in 0..n {
                draw_filled_rect(buf, buf_w, gx0 + i, gy0 + i, gx0 + i + 2, gy0 + i + 1, ink);
                draw_filled_rect(buf, buf_w, gx1 - i - 2, gy0 + i, gx1 - i, gy0 + i + 1, ink);
            }
        }
        ButtonGlyph::Maximize => {
            // Window outline with a thick top edge.
            draw_filled_rect(buf, buf_w, gx0, gy0, gx1, gy0 + 2, ink);
            draw_filled_rect(buf, buf_w, gx0, gy1 - 1, gx1, gy1, ink);
            draw_filled_rect(buf, buf_w, gx0, gy0, gx0 + 1, gy1, ink);
            draw_filled_rect(buf, buf_w, gx1 - 1, gy0, gx1, gy1, ink);
        }
        ButtonGlyph::Restore => {
            // Two overlapping window outlines.
            let (dx, dy) = ((gx1 - gx0) / 3, (gy1 - gy0) / 3);
            for (x0, y0, x1, y1) in [(gx0 + dx, gy0, gx1, gy1 - dy), (gx0, gy0 + dy, gx1 - dx, gy1)] {
                draw_filled_rect(buf, buf_w, x0, y0, x1, y0 + 2, ink);
                draw_filled_rect(buf, buf_w, x0, y1 - 1, x1, y1, ink);
                draw_filled_rect(buf, buf_w, x0, y0, x0 + 1, y1, ink);
                draw_filled_rect(buf, buf_w, x1 - 1, y0, x1, y1, ink);
            }
            // The front window hides the back one's lines.
            draw_filled_rect(buf, buf_w, gx0 + 1, gy0 + dy + 2, gx1 - dx - 1, gy1 - 1, CHROME_FACE_COLOR);
        }
        ButtonGlyph::Minimize => {
            // Bar along the bottom.
            draw_filled_rect(buf, buf_w, gx0, gy1 - 2, gx1, gy1, ink);
        }
        ButtonGlyph::Hide => {
            // A small hollow square in the middle: the window shrunk to
            // nothing (Warpine's own pictogram).
            let (cx, cy) = ((gx0 + gx1) / 2, (gy0 + gy1) / 2);
            let (x0, y0, x1, y1) = (cx - 2, cy - 2, cx + 3, cy + 3);
            draw_filled_rect(buf, buf_w, x0, y0, x1, y0 + 1, ink);
            draw_filled_rect(buf, buf_w, x0, y1 - 1, x1, y1, ink);
            draw_filled_rect(buf, buf_w, x0, y0, x0 + 1, y1, ink);
            draw_filled_rect(buf, buf_w, x1 - 1, y0, x1, y1, ink);
        }
        ButtonGlyph::SysMenu => {
            // Small raised plate with a horizontal slot.
            let my = (gy0 + gy1) / 2;
            draw_raised_box(buf, buf_w, gx0 - 1, my - 2, gx1 + 1, my + 2,
                            CHROME_LIGHT_COLOR, CHROME_DARK_COLOR, CHROME_DARK_COLOR);
        }
    }
}

/// The button left of the maximize button: the hide button for
/// `FCF_HIDEBUTTON` (`FCF_HIDEMAX` pairs it with the maximize button as
/// `FCF_MINMAX` pairs the minimize button), else the minimize button for
/// `FCF_MINBUTTON`. A frame asking for both gets the hide button; what PM
/// does then is not documented.
fn min_slot_glyph(frame_flags: u32) -> Option<ButtonGlyph> {
    if frame_flags & FCF_HIDEBUTTON != 0 {
        Some(ButtonGlyph::Hide)
    } else if frame_flags & FCF_MINBUTTON != 0 {
        Some(ButtonGlyph::Minimize)
    } else {
        None
    }
}

/// Draw a frame's title bar and border into the desktop buffer.
#[cfg(test)]
fn draw_frame_chrome(buf: &mut [u32], buf_w: usize, c: &FrameChrome) {
    draw_title_bar(buf, buf_w, c);
    let all = Rect::at(0, 0, buf_w as i32, (buf.len() / buf_w.max(1)) as i32);
    draw_frame_border(buf, buf_w, c, all);
}

/// Height of a frame's title bar: `CHROME_TITLE_H`, or 0 without `FCF_TITLEBAR`.
fn title_bar_h(frame_flags: u32) -> i32 {
    if frame_flags & FCF_TITLEBAR != 0 { CHROME_TITLE_H } else { 0 }
}

/// Draw a frame's title bar (with system-menu, minimise, maximise and close
/// buttons). Draws nothing without `FCF_TITLEBAR`.
///
/// Button positions mirror `hit_test_chrome`: the close button sits
/// `CHROME_BTN_MARGIN` from the right edge, then maximise and minimise to its
/// left with `CHROME_BTN_GAP` between them; the system menu is 3 px from the
/// left edge.
fn draw_title_bar(buf: &mut [u32], buf_w: usize, c: &FrameChrome) {
    let buf_h = buf.len() / buf_w.max(1);
    let title_h = title_bar_h(c.frame_flags);

    if title_h > 0 {
        let title_color = if c.focused { c.colors.active } else { c.colors.inactive };
        let text_color = if c.focused { c.colors.active_text } else { c.colors.inactive_text };
        draw_filled_rect(buf, buf_w, c.x, c.top, c.x + c.cx, c.top + title_h, title_color);
        // Hairline separating the title bar from the menu bar / client below.
        draw_filled_rect(buf, buf_w, c.x, c.top + title_h - 1, c.x + c.cx, c.top + title_h,
                         CHROME_SHADOW_COLOR);

        let btn_y = c.top + CHROME_BTN_INSET_Y;
        let btn_h = title_h - 2 * CHROME_BTN_INSET_Y - 1;
        let mut text_left = c.x + 4;
        if c.state & WS_MINIMIZED != 0 {
            // A minimized frame: its title only.
            let text_y = c.top + (title_h - GLYPH_H) / 2;
            let title = font::fit_text(c.title, c.cx - 8, FontSpec::SYSTEM_BOLD);
            draw_text_screen(buf, buf_w, buf_h, text_left, text_y, title,
                             text_color, FontSpec::SYSTEM_BOLD);
            return;
        }
        if c.frame_flags & FCF_SYSMENU != 0 {
            draw_chrome_button(buf, buf_w, c.x + SYSMENU_ICON_X, btn_y, btn_h, ButtonGlyph::SysMenu);
            text_left = c.x + SYSMENU_ICON_X + CHROME_BTN_W + 4;
        }

        // Right-side buttons: close, then maximise and minimise when present.
        let mut btn_x = c.x + c.cx - CHROME_BTN_MARGIN - CHROME_BTN_W;
        draw_chrome_button(buf, buf_w, btn_x, btn_y, btn_h, ButtonGlyph::Close);
        if c.frame_flags & FCF_MAXBUTTON != 0 {
            btn_x -= CHROME_BTN_W + CHROME_BTN_GAP;
            let glyph = if c.state & WS_MAXIMIZED != 0 { ButtonGlyph::Restore } else { ButtonGlyph::Maximize };
            draw_chrome_button(buf, buf_w, btn_x, btn_y, btn_h, glyph);
        }
        if let Some(glyph) = min_slot_glyph(c.frame_flags) {
            btn_x -= CHROME_BTN_W + CHROME_BTN_GAP;
            draw_chrome_button(buf, buf_w, btn_x, btn_y, btn_h, glyph);
        }

        // Title text, clipped so it never runs under the buttons.
        let text_y = c.top + (title_h - GLYPH_H) / 2;
        let avail = btn_x - 4 - text_left;
        let face = FontSpec { cjk: c.cjk, ..FontSpec::SYSTEM_BOLD };
        let title = font::fit_text(c.title, avail.max(0), face);
        draw_text_screen(buf, buf_w, buf_h, text_left, text_y, title, text_color, face);
    }
}

use crate::loader::constants::frame_border_width as frame_border_px;

/// Draw a frame's border, only inside `clip`. A sizing border is a raised
/// bevel; a plain or dialog border is a dark outline with a face-coloured
/// inner ring. Draws nothing for a frame without a border.
fn draw_frame_border(buf: &mut [u32], buf_w: usize, c: &FrameChrome, clip: Rect) {
    if frame_border_px(c.frame_flags) == 0 { return; }
    let (outer_lt, outer_rb) = if c.frame_flags & FCF_SIZEBORDER != 0 {
        (CHROME_LIGHT_COLOR, CHROME_SHADOW_COLOR)
    } else {
        (CHROME_SHADOW_COLOR, CHROME_SHADOW_COLOR)
    };
    let rings = [(outer_lt, outer_rb), (CHROME_FACE_COLOR, CHROME_FACE_COLOR)];
    for (i, &(lt, rb)) in rings.iter().enumerate().take(CHROME_BORDER_W as usize) {
        let i = i as i32;
        let (x0, y0, x1, y1) = (c.x + i, c.top + i, c.x + c.cx - i, c.top + c.cy - i);
        fill_clipped(buf, buf_w, Rect::new(x0, y0, x1, y0 + 1), clip, lt);
        fill_clipped(buf, buf_w, Rect::new(x0, y0, x0 + 1, y1), clip, lt);
        fill_clipped(buf, buf_w, Rect::new(x0, y1 - 1, x1, y1), clip, rb);
        fill_clipped(buf, buf_w, Rect::new(x1 - 1, y0, x1, y1), clip, rb);
    }
}

/// Line height of the PM system font (pixels).
const GLYPH_H: i32 = font::SYSTEM_FONT_PX as i32;
use super::popup_menu::{MENU_BG_COLOR, MENU_EDGE_COLOR, MENU_TEXT_COLOR, MENU_HILITE_COLOR,
                         MENU_HILITE_TEXT};

/// Draw a frame's menu bar and, when `open_item` (1-based) is non-zero, the
/// pull-down of that top-level item, into a desktop-space pixel buffer.
///
/// `(x0, bar_top)` is the bar's top-left corner in SDL screen space and
/// `bar_w` its width. Item geometry mirrors the `#Menu` hit-testing in
/// `Loader::dispatch_builtin_control` (same insets, widths and row height).
#[cfg(test)]
fn draw_menu_chrome(buf: &mut [u32], buf_w: usize, x0: i32, bar_top: i32, bar_w: i32,
                    items: &[MenuItem], open_item: u32) {
    let open_x = draw_menu_bar(buf, buf_w, x0, bar_top, bar_w, items, open_item, CjkRegion::Auto);
    if let (Some(drop_x), Some(parent)) = (open_x, items.get(open_item.wrapping_sub(1) as usize)) {
        draw_menu_pulldown(buf, buf_w, drop_x, bar_top + MENU_BAR_HEIGHT as i32, parent, CjkRegion::Auto);
    }
}

/// Draw a menu bar, highlighting the open item (1-based; 0 = none).
/// Returns the x of the open item, where its pull-down belongs.
#[allow(clippy::too_many_arguments)]
fn draw_menu_bar(buf: &mut [u32], buf_w: usize, x0: i32, bar_top: i32, bar_w: i32,
                 items: &[MenuItem], open_item: u32, cjk: CjkRegion) -> Option<i32> {
    let buf_h = buf.len() / buf_w.max(1);
    let bar_h = MENU_BAR_HEIGHT as i32;
    let bar_bot = bar_top + bar_h;
    draw_filled_rect(buf, buf_w, x0, bar_top, x0 + bar_w, bar_bot, MENU_BG_COLOR);
    draw_filled_rect(buf, buf_w, x0, bar_bot - 1, x0 + bar_w, bar_bot, MENU_EDGE_COLOR);

    let text_y = bar_top + (bar_h - GLYPH_H) / 2;
    let mut item_x = x0 + MENU_BAR_ITEM_INSET;
    let mut open_x = None;
    for (i, item) in items.iter().enumerate() {
        if item.style & MIS_SEPARATOR != 0 { continue; }
        let label_w = menu_bar_item_width(&item.text);
        let is_open = open_item > 0 && i == (open_item - 1) as usize;
        if is_open {
            draw_filled_rect(buf, buf_w, item_x, bar_top, item_x + label_w, bar_bot, MENU_HILITE_COLOR);
            open_x = Some(item_x);
        }
        draw_text_screen(buf, buf_w, buf_h, item_x + MENU_BAR_ITEM_PAD / 2, text_y,
                         &menu_label(&item.text),
                         if is_open { MENU_HILITE_TEXT } else { MENU_TEXT_COLOR },
                         FontSpec { cjk, ..FontSpec::SYSTEM });
        item_x += label_w;
    }
    open_x
}

/// Height of the pull-down listing `parent`'s children.
fn pulldown_h(parent: &MenuItem) -> i32 {
    parent.children.len() as i32 * MENU_DROPDOWN_ITEM_H
}

/// Draw the pull-down of `parent` with its top-left corner at
/// `(drop_x, bar_bot)`; it is `MENU_DROPDOWN_W` × `pulldown_h(parent)`.
fn draw_menu_pulldown(buf: &mut [u32], buf_w: usize, drop_x: i32, bar_bot: i32, parent: &MenuItem,
                      cjk: CjkRegion) {
    let buf_h = buf.len() / buf_w.max(1);
    let drop_h = pulldown_h(parent);
    let (dx1, dy1) = (drop_x + MENU_DROPDOWN_W, bar_bot + drop_h);
    draw_filled_rect(buf, buf_w, drop_x, bar_bot, dx1, dy1, MENU_EDGE_COLOR);
    draw_filled_rect(buf, buf_w, drop_x + 1, bar_bot, dx1 - 1, dy1 - 1, MENU_BG_COLOR);
    for (ci, child) in parent.children.iter().enumerate() {
        let row_top = bar_bot + ci as i32 * MENU_DROPDOWN_ITEM_H;
        if child.style & MIS_SEPARATOR != 0 {
            let sep_y = row_top + MENU_DROPDOWN_ITEM_H / 2;
            draw_filled_rect(buf, buf_w, drop_x + 2, sep_y, dx1 - 2, sep_y + 1, MENU_EDGE_COLOR);
        } else if let Some(image) = &child.image {
            // MIS_OWNERDRAW: what the owner drew (pm_owner.rs), rows top down.
            let w = owner_menu_item_w() as usize;
            for (r, row) in image.chunks(w).enumerate() {
                let y = row_top + r as i32;
                if y < 0 || y as usize >= buf_h { continue; }
                for (c, &p) in row.iter().enumerate() {
                    let x = drop_x + 1 + c as i32;
                    if x >= 0 && (x as usize) < buf_w {
                        buf[y as usize * buf_w + x as usize] = p;
                    }
                }
            }
        } else {
            draw_text_screen(buf, buf_w, buf_h, drop_x + 4, row_top,
                             &menu_label(&child.text), MENU_TEXT_COLOR, FontSpec { cjk, ..FontSpec::SYSTEM });
        }
    }
}

/// Draw a filled axis-aligned rectangle into a desktop-space pixel buffer.
///
/// Coordinates are in SDL screen space (y=0 at top). Clips to buffer bounds.
fn draw_filled_rect(buf: &mut [u32], buf_w: usize,
                    x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
    let r = Rect::new(x0, y0, x1, y1);
    fill_clipped(buf, buf_w, r, r, color);
}

// ── Compositor: chrome cache, frame placement, damage ─────────────────────

/// An open pull-down, pre-rendered (`MENU_DROPDOWN_W` × `h`).
struct Pulldown {
    /// Offset of its left edge from the menu bar's left edge.
    dx: i32,
    h: i32,
    pixels: Vec<u32>,
}

/// A frame's menu bar as last rendered.
struct MenuChrome {
    items: Vec<MenuItem>,
    /// 1-based open item; 0 = none.
    open_item: u32,
    bar_w: i32,
    /// `bar_w` × `MENU_BAR_HEIGHT`.
    bar_px: Vec<u32>,
    pulldown: Option<Pulldown>,
}

/// Pre-rendered chrome of one frame: title bar, menu bar and open pull-down.
///
/// Chrome changes only when a frame's title, width, flags, focus or menu do,
/// so it is rendered into pixmaps once and copied into the desktop on each
/// composite, instead of rasterising title and menu text every time. The
/// window state is compared with the cached copy under the window-manager
/// lock without allocating; only a change copies it, and the pixmaps are
/// re-rendered after the lock is released.
struct ChromeCache {
    title: String,
    cx: i32,
    frame_flags: u32,
    /// `WS_MINIMIZED` / `WS_MAXIMIZED`.
    state: u32,
    focused: bool,
    /// Regional face of CJK text in the title and menus.
    cjk: CjkRegion,
    /// Title-bar colours.
    colors: TitleColors,
    /// `cx` × `title_bar_h(frame_flags)`.
    title_px: Vec<u32>,
    menu: Option<MenuChrome>,
    /// The pixmaps do not match the fields yet.
    stale: bool,
}

impl ChromeCache {
    fn new() -> Self {
        ChromeCache { title: String::new(), cx: 0, frame_flags: 0, state: 0, focused: false, cjk: CjkRegion::Auto,
                      colors: TitleColors::default(), title_px: Vec::new(), menu: None, stale: true }
    }

    /// Bring the cached state up to date; returns whether it changed (or
    /// was never rendered), in which case the frame's chrome must be redrawn.
    #[allow(clippy::too_many_arguments)]
    fn sync(&mut self, title: &str, cx: i32, frame_flags: u32, state: u32, focused: bool,
            menu: Option<(&[MenuItem], u32)>, bar_w: i32) -> bool {
        let mut changed = self.stale;
        if self.title != title {
            self.title.clear();
            self.title.push_str(title);
            changed = true;
        }
        if (self.cx, self.frame_flags, self.state, self.focused) != (cx, frame_flags, state, focused) {
            (self.cx, self.frame_flags, self.state, self.focused) = (cx, frame_flags, state, focused);
            changed = true;
        }
        match (menu, &self.menu) {
            (None, None) => {}
            (Some((items, open)), Some(m))
                if m.open_item == open && m.bar_w == bar_w && m.items == items => {}
            (menu, _) => {
                self.menu = menu.map(|(items, open_item)| MenuChrome {
                    items: items.to_vec(), open_item, bar_w, bar_px: Vec::new(), pulldown: None,
                });
                changed = true;
            }
        }
        self.stale = changed;
        changed
    }

    /// Use `cjk` for CJK text from now on; returns whether it changed (the
    /// chrome must then be redrawn, as after [`ChromeCache::sync`]).
    fn set_cjk(&mut self, cjk: CjkRegion) -> bool {
        let changed = self.cjk != cjk;
        self.cjk = cjk;
        self.stale |= changed;
        changed
    }

    /// Draw the title bar in `colors` from now on; returns whether they
    /// changed (the chrome must then be redrawn).
    fn set_colors(&mut self, colors: TitleColors) -> bool {
        let changed = self.colors != colors;
        self.colors = colors;
        self.stale |= changed;
        changed
    }

    /// Re-render the pixmaps if the state changed since the last render.
    fn render(&mut self) {
        if !std::mem::take(&mut self.stale) { return; }
        let (w, th) = (self.cx.max(0) as usize, title_bar_h(self.frame_flags));
        self.title_px = vec![0; w * th as usize];
        if !self.title_px.is_empty() {
            draw_title_bar(&mut self.title_px, w, &FrameChrome { colors: self.colors,
                x: 0, top: 0, cx: self.cx, cy: th, frame_flags: self.frame_flags,
                title: &self.title, focused: self.focused, state: self.state, cjk: self.cjk,
            });
        }
        let Some(m) = &mut self.menu else { return };
        let bw = m.bar_w.max(0) as usize;
        m.bar_px = vec![0; bw * MENU_BAR_HEIGHT as usize];
        let open_x = if bw > 0 {
            draw_menu_bar(&mut m.bar_px, bw, 0, 0, m.bar_w, &m.items, m.open_item, self.cjk)
        } else { None };
        m.pulldown = match (open_x, m.items.get(m.open_item.wrapping_sub(1) as usize)) {
            (Some(dx), Some(parent)) if pulldown_h(parent) > 0 => {
                let h = pulldown_h(parent);
                let mut pixels = vec![0; MENU_DROPDOWN_W as usize * h as usize];
                draw_menu_pulldown(&mut pixels, MENU_DROPDOWN_W as usize, 0, 0, parent, self.cjk);
                Some(Pulldown { dx, h, pixels })
            }
            _ => None,
        };
    }
}

/// Where the parts of one frame land on the desktop (screen coordinates).
#[derive(Debug, PartialEq)]
struct FramePlacement {
    hwnd: u32,
    frame_flags: u32,
    /// The whole frame window.
    frame: Rect,
    /// Title bar (empty without `FCF_TITLEBAR`).
    title: Rect,
    /// Menu bar (empty without a menu).
    menu_bar: Rect,
    /// Open pull-down (empty when none is open); may extend past the frame.
    pulldown: Rect,
    /// Client area, where the frame buffer is drawn (clamped to its size).
    client: Rect,
    /// The frame buffer fills `client`, so every pixel of `frame` is painted
    /// and frames below are hidden.
    opaque: bool,
}

impl FramePlacement {
    /// Everything the frame paints: the frame plus an open pull-down.
    fn extent(&self) -> Rect {
        self.frame.union(&self.pulldown)
    }
}

/// Lay out a frame at OS/2 position `(x, y)` (bottom-left origin) with size
/// `cx` × `cy` on a desktop `desktop_h` pixels tall. `pulldown` is the open
/// pull-down's `(offset from the menu bar's left edge, height)`; `fb` the
/// frame buffer's size.
#[allow(clippy::too_many_arguments)]
fn place_frame(hwnd: u32, x: i32, y: i32, cx: i32, cy: i32, frame_flags: u32, has_menu: bool,
               pulldown: Option<(i32, i32)>, fb: (u32, u32), desktop_h: i32) -> FramePlacement {
    let top = desktop_h - y - cy;
    let th = title_bar_h(frame_flags);
    let bw = frame_border_px(frame_flags);
    let mh = if has_menu { MENU_BAR_HEIGHT as i32 } else { 0 };
    let client = Rect::new(x + bw, top + th + mh, x + cx - bw, top + cy);
    FramePlacement {
        hwnd,
        frame_flags,
        frame: Rect::at(x, top, cx, cy),
        title: Rect::at(x, top, cx, th),
        menu_bar: if has_menu { Rect::at(x + bw, top + th, cx - 2 * bw, mh) } else { Rect::EMPTY },
        pulldown: pulldown
            .map(|(dx, h)| Rect::at(x + bw + dx, top + th + mh, MENU_DROPDOWN_W, h))
            .unwrap_or(Rect::EMPTY),
        client,
        opaque: fb.0 as i32 >= client.width() && fb.1 as i32 >= client.height(),
    }
}

/// Draw one frame — client content, title bar, border, menu bar and
/// pull-down, in that order — into the desktop buffer, only inside `clip`.
fn draw_placed_frame(buf: &mut [u32], buf_w: usize, p: &FramePlacement,
                     fb: &Surface, chrome: &ChromeCache, clip: Rect) {
    let blit = |buf: &mut [u32], px: &[u32], src_w: i32, at: Rect| {
        blit_clipped(buf, buf_w, px, src_w.max(1) as usize, at.x0, at.y0, at.width(), at.height(), clip);
    };
    blit(buf, &fb.buffer, fb.width as i32, p.client);
    blit(buf, &chrome.title_px, chrome.cx, p.title);
    draw_frame_border(buf, buf_w, &FrameChrome { colors: Default::default(),
        x: p.frame.x0, top: p.frame.y0, cx: p.frame.width(), cy: p.frame.height(),
        frame_flags: p.frame_flags, title: "", focused: false, state: 0, cjk: CjkRegion::Auto,
    }, clip);
    if let Some(m) = &chrome.menu {
        blit(buf, &m.bar_px, m.bar_w, p.menu_bar);
        if let Some(pd) = &m.pulldown {
            blit(buf, &pd.pixels, MENU_DROPDOWN_W, p.pulldown);
        }
    }
}

/// Longest parent chain followed when mapping a dirty window to its frame.
const MAX_PARENT_DEPTH: usize = 64;

/// The frame in the Z-order that `hwnd` is drawn as part of: `hwnd` itself,
/// the frame of a client, or the frame above a control. `None` for windows
/// the compositor does not draw.
fn owning_frame(wm: &WindowManager, hwnd: u32) -> Option<u32> {
    let mut h = hwnd;
    for _ in 0..MAX_PARENT_DEPTH {
        if wm.z_order.contains(&h) { return Some(h); }
        let frame = wm.client_to_frame(h);
        if frame != h && wm.z_order.contains(&frame) { return Some(frame); }
        h = wm.get_window(h)?.parent;
        if h == 0 { return None; }
    }
    None
}

/// Active window-drag state (VDR-D3).
struct DragState {
    /// HWND of the frame being dragged.
    hwnd: u32,
    /// Horizontal offset (SDL pixels) from the frame's left edge to the press point.
    anchor_x: i32,
    /// Vertical offset (SDL pixels) from the top of the frame in SDL coords to the press point.
    anchor_y_sdl: i32,
}

/// Which edge or corner is being resized (VDR-C5).
#[derive(Clone, Copy, PartialEq)]
enum ResizeEdge {
    N, S, E, W, NE, NW, SE, SW,
}

/// Active resize-drag state (VDR-C5).
struct ResizeState {
    hwnd:    u32,
    edge:    ResizeEdge,
    /// SDL cursor position at press.
    press_x: i32,
    press_y: i32,
    /// Window geometry at press time (OS/2 coords).
    orig_x: i32, orig_y: i32, orig_cx: i32, orig_cy: i32,
}

/// What an open pop-up menu belongs to: where its choice goes.
enum PopupOwner {
    /// The VIO window's system menu (`vio_command`).
    Vio,
    /// A PM frame's system menu: `WM_SYSCOMMAND(SC_*)` to the frame.
    Frame(u32),
    /// A `WinPopupMenu` menu: notifications go to `owner`; `styles` holds
    /// the items' `MIS_*` styles by id (`MIS_SYSCOMMAND` / `MIS_HELP`).
    Menu { owner: u32, styles: HashMap<u32, u16> },
}

/// A keyboard move or size of a PM frame in progress (`frame_track.rs`).
struct FrameTrack {
    frame: u32,
    track: super::frame_track::KeyTrack,
    /// The client's size when tracking started (for `WM_SIZE` at the end).
    client: (i32, i32),
}

/// SDL-backed Presentation Manager renderer.
///
/// VDR-A: one SDL window (the desktop); each PM frame/dialog is drawn into
/// its surface (`WindowManager::surfaces`) by the API threads.
/// `PresentBuffer` composites all visible frames onto the desktop surface using
/// window positions from `SharedState::window_mgr`.
///
/// Created on the main thread; must stay on the main thread for the duration
/// of `run_pm_loop`.
pub struct SdlRenderer {
    video:       sdl3::VideoSubsystem,
    event:       sdl3::EventSubsystem,
    event_pump:  sdl3::EventPump,
    /// The desktop window and how frames are put on it.
    desktop:     Box<dyn Compositor>,
    /// Frame → pre-rendered title bar, menu bar and pull-down.
    chrome: HashMap<u32, ChromeCache>,
    /// Frame → desktop area it covered at the last composite; what must be
    /// redrawn when it moves, changes or disappears.
    placed: HashMap<u32, Rect>,
    /// Redraw and upload the whole desktop at the next composite (start-up,
    /// host window resized or exposed).
    force_full: bool,
    /// Last seen host clipboard text — used to detect changes.
    cached_clipboard: String,
    /// Whether the host clipboard has been read at least once. After that it
    /// is re-read only on SDL clipboard-update and focus-gained events.
    clipboard_synced: bool,
    /// Wait in SDL's own event wait, woken by a pushed [`WakeEvent`];
    /// false if the video driver cannot block (see `driver_can_wait`) or
    /// the event type could not be registered.
    native_wait: bool,
    /// Mouse capture during drags (`SetMouseCapture`).
    mouse: sdl3::mouse::MouseUtil,
    /// The host cursor showing the PM pointer (`SetPointer`).
    pointer: super::pointer::HostPointer,
    /// A VIO program's text screen in a frame on the desktop (a windowed
    /// VIO session, `vio_window.rs`); `None` for PM programs.
    vio: Option<super::vio_window::VioWindow>,
    /// An open pop-up menu (a system menu or a `WinPopupMenu` menu), drawn over
    /// everything as an overlay; it takes the mouse and the keyboard.
    popup: Option<PopupMenu>,
    /// What the open pop-up menu belongs to.
    popup_owner: PopupOwner,
    /// A keyboard move or size of a PM frame (Alt+F7 / Alt+F8, system menu
    /// Move / Size); it takes the arrows, Enter and Esc.
    frame_track: Option<FrameTrack>,
    /// Desktop area the overlay covered at the last composite.
    overlay_placed: Rect,
    /// The overlay opened, closed or changed its look since then.
    overlay_changed: bool,
    /// Counts overlay changes (the layered compositor re-uploads on a change).
    overlay_generation: u64,
    /// The PM text cursor shown on each frame at the last composite.
    cursor_marks: HashMap<u32, CursorMark>,
    /// Ignore the next button release: its press closed a pop-up menu.
    swallow_release: bool,
    /// Shift / Ctrl / Alt as `KC_*` flags, from the last key event; mouse
    /// messages report them in `fsInp` (high word of mp2).
    key_mods: u32,
    /// MMPM/2 and DosBeep audio, played on this (main) thread.
    audio: super::audio::SdlAudio,
    /// The loop's wake-up once installed; audio commands use it.
    wake_slot: Arc<std::sync::OnceLock<Arc<GuiWake>>>,
    /// The channel's wake-up, for drivers where the loop blocks on it.
    wake: Option<Arc<GuiWake>>,
    /// Events received while waiting, handled on the next `poll_events`.
    pending_events: Vec<Event>,
    /// Loop and composite timings, logged periodically at debug level.
    stats: FrameStats,
    /// Active drag operation (VDR-D3); `None` when no drag is in progress.
    drag_state: Option<DragState>,
    /// Active resize operation (VDR-C5); `None` when no resize is in progress.
    resize_state: Option<ResizeState>,
    /// When the compositor last actually composited (frame pacing).
    last_composite: Instant,
    /// Scripted session replayed against the desktop (`WARPINE_GUI_SCRIPT`).
    script: Option<GuiScript>,
    /// When the caret of the focused text control blinks next; `None` in a
    /// scripted session, whose captures must not depend on the blink phase.
    caret_next: Option<Instant>,
    /// When the drag scroll timer fires next (`None` while no control is
    /// held or in a scripted session).
    drag_scroll_next: Option<Instant>,
    /// The host text-input area last given to SDL (`SDL_SetTextInputArea`):
    /// screen rectangle and caret offset.
    ime_area: Option<((i32, i32, i32, i32), i32)>,
}

/// The desktop window and its compositor while a VIO session is full screen
/// (Alt+Home): hidden, to be shown again by the next [`SdlRenderer::open`].
/// Kept rather than dropped because the compositor's process-lifetime
/// texture creator would keep the window alive anyway.
pub struct ParkedDesktop(Box<dyn Compositor>);

impl SdlRenderer {
    /// Create an `SdlRenderer` from an existing SDL context.
    ///
    /// Reads `WARPINE_DESKTOP_W` / `WARPINE_DESKTOP_H` env vars for the desktop
    /// size; defaults to 1024×768.
    ///
    /// `sdl` must outlive this renderer.  Typical usage:
    /// ```rust,ignore
    /// let sdl = sdl3::init().unwrap();
    /// let mut renderer = SdlRenderer::new(&sdl, &shared.audio);
    /// run_pm_loop(&mut renderer, shared, rx);
    /// ```
    /// `audio` gets its output attached here, before the guest starts, so
    /// early sounds are not lost; commands wake the loop once
    /// `install_waker` has run.
    pub fn new(sdl: &sdl3::Sdl, audio: &super::audio::AudioService) -> Self {
        Self::open(sdl, audio, &mut super::vio_switch::Parked::default())
    }

    /// Show a VIO program in a window on the desktop titled `title`, drawn
    /// from the VIO buffer on every pass. The frame is created now, at the
    /// size of the VIO buffer — or, when the session comes back from full
    /// screen (Alt+Home) and the frame still exists, that frame is brought
    /// to the top and activated again.
    pub fn attach_vio_window(&mut self, shared: &SharedState, title: &str) {
        let (cols, rows) = {
            let con = shared.console_mgr.lock_or_recover();
            (con.cols, con.rows)
        };
        let frame = {
            let mut wm = shared.window_mgr.lock_or_recover();
            let existing = wm.vio_frame;
            if existing != 0 && wm.get_window(existing).is_some() {
                wm.z_push_top(existing);
                wm.focused_hwnd = existing;
                existing
            } else {
                wm.create_vio_frame(title, cols, rows)
            }
        };
        self.vio = Some(super::vio_window::VioWindow::new(frame));
    }

    /// Hide the desktop and leave its window, audio output and GUI script in
    /// `parked` for the next display of a VIO session (Alt+Home).
    pub fn park(self, parked: &mut super::vio_switch::Parked) {
        let SdlRenderer { desktop, audio, script, .. } = self;
        desktop.window().clone().hide();
        parked.desktop = Some(ParkedDesktop(desktop));
        parked.audio = Some(audio);
        parked.script = Some(script);
    }

    /// Whether the active window is the VIO window, which takes the keys.
    fn vio_active(&self, shared: &SharedState) -> bool {
        self.vio.as_ref().is_some_and(|v| shared.window_mgr.lock_or_recover().focused_hwnd == v.frame())
    }

    /// Whether the keyboard goes to the renderer itself rather than to PM:
    /// an open pop-up menu, a keyboard move or size, the active VIO window.
    fn takes_keys(&self, shared: &SharedState) -> bool {
        self.popup.is_some() || self.frame_track.is_some() || self.vio_active(shared)
    }

    /// Open the VIO window's system menu below its system-menu icon, from
    /// the keyboard (`keyboard`: the first item is highlighted) or a click.
    fn open_vio_menu(&mut self, shared: &SharedState, keyboard: bool) {
        let Some(vio) = &self.vio else { return };
        let (dw, dh) = self.desktop.size();
        let (items, x, top) = {
            let wm = shared.window_mgr.lock_or_recover();
            let Some(f) = wm.get_window(vio.frame()) else { return };
            let state = f.style & (WS_MINIMIZED | WS_MAXIMIZED);
            let items = system_menu_items(state, vio.marking(), vio.mark().is_some(),
                                          !wm.clipboard_text.is_empty());
            (items, f.x + SYSMENU_ICON_X, dh as i32 - f.y - f.cy + CHROME_TITLE_H)
        };
        self.show_popup(PopupMenu::new(items, x, top, (dw as i32, dh as i32), keyboard), PopupOwner::Vio);
    }

    /// Show `menu`, belonging to `owner`, as the pop-up menu (replacing any
    /// open one). A keyboard move or size in progress ends where it is.
    fn show_popup(&mut self, menu: PopupMenu, owner: PopupOwner) {
        self.popup = Some(menu);
        self.popup_owner = owner;
        self.overlay_changed = true;
    }

    /// Open the system menu of PM frame `frame` below its system-menu icon,
    /// from the keyboard (`keyboard`: the first item is highlighted) or a
    /// click (`system_menu.rs`). Nothing for a frame without a system menu
    /// or a hidden one.
    fn open_frame_menu(&mut self, shared: &SharedState, frame: u32, keyboard: bool) {
        if self.vio.as_ref().is_some_and(|v| v.frame() == frame) {
            return self.open_vio_menu(shared, keyboard);
        }
        let (dw, dh) = self.desktop.size();
        let (items, x, top) = {
            let wm = shared.window_mgr.lock_or_recover();
            let Some(f) = wm.get_window(frame).filter(|f| f.visible && f.frame_flags & FCF_SYSMENU != 0)
                else { return };
            let state = f.style & (WS_MINIMIZED | WS_MAXIMIZED);
            (frame_system_menu_items(f.frame_flags, state), f.x + SYSMENU_ICON_X,
             dh as i32 - f.y - f.cy + title_bar_h(f.frame_flags))
        };
        self.end_frame_track(shared);
        self.show_popup(PopupMenu::new(items, x, top, (dw as i32, dh as i32), keyboard), PopupOwner::Frame(frame));
    }

    /// Show the pop-up menu `req` asks for (`WinPopupMenu`): its bottom-left
    /// corner at `(x, y)`, or with item `item`'s row there
    /// (`PU_POSITIONONITEM`), kept on the desktop.
    fn open_popup_request(&mut self, req: super::message::PopupRequest) {
        let (dw, dh) = self.desktop.size();
        let (dw, dh) = (dw as i32, dh as i32);
        let items = menu_popup_items(&req.items);
        let styles = req.items.iter().map(|m| (m.id as u32, m.style)).collect();
        // The menu's height, then where its top goes (screen rows from the top).
        let probe = PopupMenu::new(items.clone(), 0, 0, (dw, dh), false);
        let screen_y = dh - 1 - req.y;
        let top = match req.item.filter(|_| req.position_on_item).and_then(|id| probe.item_middle(id)) {
            Some(middle) => screen_y - middle,
            None => screen_y + 1 - probe.rect().height(),
        };
        let mut menu = PopupMenu::new(items, req.x, top, (dw, dh), false);
        if let Some(id) = req.item {
            menu.highlight(id);
        }
        self.show_popup(menu, PopupOwner::Menu { owner: req.owner, styles });
    }

    /// Close the pop-up menu, if one is open.
    fn close_popup(&mut self) {
        if self.popup.take().is_some() {
            self.overlay_changed = true;
        }
    }

    /// Carry out what an input did to the pop-up menu, chosen with the
    /// mouse (`pointer`) or the keyboard. Returns whether Warpine keeps
    /// running.
    fn popup_action(&mut self, shared: &SharedState, action: PopupAction, pointer: bool) -> bool {
        match action {
            PopupAction::Stay => {
                self.overlay_changed = true;
                true
            }
            PopupAction::Close => {
                self.close_popup();
                true
            }
            PopupAction::Choose(id) => {
                self.close_popup();
                // mp2 = MPFROM2SHORT(CMDSRC_MENU, fPointer).
                let mp2 = CMDSRC_MENU | (pointer as u32) << 16;
                match std::mem::replace(&mut self.popup_owner, PopupOwner::Vio) {
                    PopupOwner::Vio => return self.vio_command(shared, id),
                    // The frame's client (or the dialog) may handle the
                    // command itself; WinDefWindowProc carries it out.
                    PopupOwner::Frame(frame) => push_msg(shared, frame, WM_SYSCOMMAND, id, mp2),
                    PopupOwner::Menu { owner, styles } => {
                        let style = styles.get(&id).copied().unwrap_or(0);
                        let msg = if style & MIS_SYSCOMMAND != 0 { WM_SYSCOMMAND }
                            else if style & MIS_HELP != 0 { WM_HELP } else { WM_COMMAND };
                        push_msg(shared, owner, msg, id, mp2);
                    }
                }
                true
            }
        }
    }

    /// A key while a pop-up menu is open: it works the menu (Alt+Space and
    /// Shift+Esc close it). Returns whether Warpine keeps running.
    fn popup_key(&mut self, shared: &SharedState, kc: Keycode, keymod: KeyMod) -> bool {
        let Some(popup) = self.popup.as_mut() else { return true };
        let key = match kc {
            _ if is_system_menu_key(kc, keymod) => Some(PopupKey::Esc),
            Keycode::Up => Some(PopupKey::Up),
            Keycode::Down => Some(PopupKey::Down),
            Keycode::Home => Some(PopupKey::Home),
            Keycode::End => Some(PopupKey::End),
            Keycode::Return | Keycode::KpEnter => Some(PopupKey::Enter),
            Keycode::Escape => Some(PopupKey::Esc),
            kc => char::from_u32(sdl_keycode_to_char(kc)).filter(char::is_ascii_alphanumeric).map(PopupKey::Char),
        };
        let Some(key) = key else { return true };
        let action = popup.key(key);
        self.popup_action(shared, action, false)
    }

    /// Start a keyboard move (`size` false) or size of PM frame `frame`
    /// (`GUIMessage::TrackFrame`), if its state and sizing border allow it.
    fn start_frame_track(&mut self, shared: &SharedState, frame: u32, size: bool) {
        self.end_frame_track(shared);
        let wm = shared.window_mgr.lock_or_recover();
        let Some(f) = wm.get_window(frame) else { return };
        if !can_track(f.frame_flags, f.style & (WS_MINIMIZED | WS_MAXIMIZED), size) { return; }
        let orig = WinRect { x: f.x, y: f.y, cx: f.cx, cy: f.cy };
        let client = wm.get_window(wm.frame_msg_target(frame)).map_or((0, 0), |c| (c.cx, c.cy));
        let step = super::frame_track::FRAME_TRACK_STEP;
        self.frame_track = Some(FrameTrack { frame, track: KeyTrack::new(orig, size, (step, step)), client });
    }

    /// Work the keyboard move or size with `key`.
    fn frame_track_key(&mut self, shared: &SharedState, key: TrackKey) {
        let Some(ft) = self.frame_track.as_mut() else { return };
        let frame = ft.frame;
        let outcome = {
            let mut wm = shared.window_mgr.lock_or_recover();
            let Some(f) = wm.get_window(frame) else {
                drop(wm);
                self.frame_track = None;
                return;
            };
            let current = WinRect { x: f.x, y: f.y, cx: f.cx, cy: f.cy };
            let outcome = ft.track.key(current, key, |cx, cy| (cx.max(MIN_RESIZE), cy.max(MIN_RESIZE)));
            match outcome {
                TrackOutcome::Continue(Some(r)) | TrackOutcome::Cancel(r) => {
                    if ft.track.size { set_frame_rect(&mut wm, frame, r); } else { wm.move_frame(frame, r.x, r.y); }
                }
                TrackOutcome::Continue(None) | TrackOutcome::Accept => {}
            }
            outcome
        };
        if !matches!(outcome, TrackOutcome::Continue(_)) {
            self.end_frame_track(shared);
        }
    }

    /// End a keyboard move or size where the frame is: the client gets
    /// `WM_SIZE` when its size changed.
    fn end_frame_track(&mut self, shared: &SharedState) {
        let Some(ft) = self.frame_track.take() else { return };
        let mut wm = shared.window_mgr.lock_or_recover();
        let client = wm.frame_msg_target(ft.frame);
        let now = wm.get_window(client).map_or(ft.client, |c| (c.cx, c.cy));
        if now != ft.client { wm.post_size(client, ft.client, now); }
    }

    /// Button `button` went down at screen `(x, y)` while a pop-up menu is
    /// open. Button 1 on the menu works it; any other press closes it and
    /// is otherwise ignored (Warpine's choice), except a double click on
    /// a system-menu icon, which goes on to close the window (`None`:
    /// handle the click as usual). Returns whether Warpine keeps running.
    fn popup_press(&mut self, shared: &SharedState, button: sdl3::mouse::MouseButton, clicks: u8,
                   x: i32, y: i32) -> Option<bool> {
        let popup = self.popup.as_mut()?;
        let left = button == sdl3::mouse::MouseButton::Left;
        if left && popup.contains(x, y) {
            let action = popup.press(x, y);
            return Some(self.popup_action(shared, action, true));
        }
        self.close_popup();
        let dh = self.desktop.size().1 as i32;
        let on_icon = {
            let wm = shared.window_mgr.lock_or_recover();
            wm.z_hit_test(x, dh - 1 - y).and_then(|h| wm.get_window(h)).is_some_and(|f| {
                let state = f.style & (WS_MINIMIZED | WS_MAXIMIZED);
                frame_chrome_hit(f.x, f.y, f.cx, f.cy, f.frame_flags, state, x, dh - 1 - y) == ChromeHit::SysMenu
            })
        };
        if left && on_icon && button_down_msg(button, clicks) == Some(WM_BUTTON1DBLCLK) {
            return None;
        }
        self.swallow_release = true;
        Some(true)
    }

    /// Carry out system-menu command `id` (`SC_*` or a `vio_window::CMD_*`)
    /// for the VIO window. Returns whether Warpine keeps running (Close
    /// ends the program).
    fn vio_command(&mut self, shared: &SharedState, id: u32) -> bool {
        use super::vio_window::{CMD_COPY, CMD_COPY_ALL, CMD_MARK, CMD_PASTE};
        let Some(vio) = self.vio.as_mut() else { return true };
        let mut copied = None;
        match id {
            SC_CLOSE => {
                vio.close(shared);
                return false;
            }
            SC_RESTORE | SC_MINIMIZE | SC_MAXIMIZE => {
                if let Some(op) = crate::loader::pm_minmax::syscommand_state(id) {
                    shared.window_mgr.lock_or_recover().vio_minmax(op);
                }
            }
            SC_MOVE | SC_SIZE => { vio.start_track(shared, id == SC_SIZE); }
            CMD_MARK => {
                let on = !vio.marking();
                vio.set_marking(shared, on);
            }
            CMD_COPY => copied = vio.copy_mark(shared),
            CMD_COPY_ALL => copied = Some(vio.copy_all(shared)),
            CMD_PASTE => {
                let text = shared.window_mgr.lock_or_recover().clipboard_text.clone();
                vio.paste(shared, &text);
            }
            _ => {}
        }
        vio.update(shared);
        if let Some(text) = copied {
            self.set_clipboard_text(shared, text);
        }
        true
    }

    /// Put `text` on the PM clipboard (`CF_TEXT`) and the host clipboard,
    /// as an entry field's Copy does (`pm_controls.rs`), from this thread.
    fn set_clipboard_text(&mut self, shared: &SharedState, text: String) {
        {
            let mut wm = shared.window_mgr.lock_or_recover();
            wm.clipboard_text = text.clone();
            // A guest CF_TEXT handle no longer holds the clipboard text.
            wm.clipboard.insert(CF_TEXT, 0);
        }
        if let Err(e) = self.video.clipboard().set_clipboard_text(&text) {
            log::warn!("[GUI] cannot set the host clipboard: {e}");
        }
        self.cached_clipboard = text;
    }

    /// A key pressed while a pop-up menu is open, a PM frame is moved or
    /// sized with the keyboard, or the VIO window is active. An open menu
    /// takes the keys; a keyboard move or size takes the arrows, Enter and
    /// Esc (other keys are ignored meanwhile, Warpine's choice). Returns
    /// whether Warpine keeps running.
    fn window_key(&mut self, shared: &SharedState, kc: Keycode, sc: Scancode, keymod: KeyMod) -> bool {
        if self.popup.is_some() {
            return self.popup_key(shared, kc, keymod);
        }
        if self.frame_track.is_some() {
            if let Some(key) = track_key_of(kc) {
                self.frame_track_key(shared, key);
            }
            return true;
        }
        self.vio_key(shared, kc, sc, keymod)
    }

    /// A key pressed while the VIO window is active. A keyboard move or
    /// size takes the arrows, Enter and Esc; Alt+Space and Shift+Esc open
    /// the system menu (whether a VIO window keeps them from the program
    /// is not documented); Alt+F4 / F5 / F7 / F8 / F9 / F10 run the system
    /// command the menu shows next to its item (unverified for a VIO
    /// window); in Mark mode Esc leaves it and other keys are ignored
    /// (Warpine's choice). Every other key goes to the program. Returns
    /// whether Warpine keeps running.
    fn vio_key(&mut self, shared: &SharedState, kc: Keycode, sc: Scancode, keymod: KeyMod) -> bool {
        let alt = keymod.intersects(KeyMod::LALTMOD | KeyMod::RALTMOD);
        let ctrl = keymod.intersects(KeyMod::LCTRLMOD | KeyMod::RCTRLMOD);
        let Some(vio) = self.vio.as_mut() else { return true };
        if vio.tracking() {
            if let Some(key) = track_key_of(kc) {
                vio.track_key(shared, key);
                vio.update(shared);
            }
            return true;
        }
        if is_system_menu_key(kc, keymod) {
            self.open_vio_menu(shared, true);
            return true;
        }
        if alt && !ctrl {
            let command = match kc {
                Keycode::F4 => Some(SC_CLOSE),
                Keycode::F5 => Some(SC_RESTORE),
                Keycode::F7 => Some(SC_MOVE),
                Keycode::F8 => Some(SC_SIZE),
                Keycode::F9 => Some(SC_MINIMIZE),
                Keycode::F10 => Some(SC_MAXIMIZE),
                _ => None,
            };
            if let Some(id) = command {
                return self.vio_command(shared, id);
            }
        }
        if vio.marking() {
            if kc == Keycode::Escape {
                vio.set_marking(shared, false);
                vio.update(shared);
            }
            return true;
        }
        super::text_renderer::kbd_key_down(shared, kc, sc, keymod);
        true
    }

    /// Create an `SdlRenderer`, reusing what `parked` holds: the desktop
    /// window, audio output and GUI script of an earlier display of a VIO
    /// session (Alt+Home). See [`new`](Self::new).
    pub fn open(sdl: &sdl3::Sdl, audio: &super::audio::AudioService,
                parked: &mut super::vio_switch::Parked) -> Self {
        /// SDL accepts a custom event type once per process; whether that
        /// worked is remembered for every later renderer.
        static WAKE_EVENT_REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let (dw, dh) = crate::loader::constants::desktop_size();
        let video      = sdl.video().expect("SDL video subsystem init failed");
        let event      = sdl.event().expect("SDL event subsystem init failed");
        let event_pump = sdl.event_pump().expect("SDL event pump init failed");
        let real_display = driver_can_wait(video.current_video_driver());
        let desktop = match parked.desktop.take() {
            Some(ParkedDesktop(desktop)) => {
                let mut window = desktop.window().clone();
                window.show();
                window.raise();
                desktop
            }
            None => {
                let kind = CompositorKind::choose(
                    std::env::var(compositor::COMPOSITOR_ENV).ok().as_deref(), real_display);
                compositor::create(&video, dw, dh, kind)
            }
        };
        // Typed characters are taken from SDL TextInput events (see
        // `build_wm_char`); SDL3 generates them per window, once started.
        video.text_input().start(desktop.window());
        // Block in SDL's own wait only where the video driver can really
        // block on input; the headless drivers have no input to wait for and
        // SDL would poll internally instead.
        let native_wait = real_display
            && *WAKE_EVENT_REGISTERED.get_or_init(|| event.register_custom_event::<WakeEvent>()
                .map_err(|e| log::warn!("[GUI] no SDL user event for wake-ups ({e})"))
                .is_ok());
        let now = Instant::now();
        let wake_slot: Arc<std::sync::OnceLock<Arc<GuiWake>>> = Arc::default();
        let mut renderer = SdlRenderer {
            video,
            event,
            event_pump,
            desktop,
            chrome: HashMap::new(),
            placed: HashMap::new(),
            force_full: true,
            cached_clipboard: String::new(),
            clipboard_synced: false,
            native_wait,
            mouse: sdl.mouse(),
            pointer: super::pointer::HostPointer::new(sdl.mouse()),
            audio: {
                let mut a = parked.take_audio(sdl);
                let slot = wake_slot.clone();
                a.attach(audio, Box::new(move || {
                    if let Some(w) = slot.get() { w.wake(); }
                }));
                a
            },
            wake_slot,
            wake: None,
            pending_events: Vec::new(),
            stats: FrameStats::new(now),
            drag_state: None,
            resize_state: None,
            key_mods: 0,
            // Long enough ago that the first frame is composited at once.
            last_composite: now.checked_sub(FRAME_INTERVAL).unwrap_or(now),
            script: parked.take_script().inspect(|_| {
                log::info!("[GUI] GUI script active: host mouse and keyboard input is ignored");
            }),
            caret_next: None,
            drag_scroll_next: None,
            vio: None,
            popup: None,
            popup_owner: PopupOwner::Vio,
            frame_track: None,
            overlay_placed: Rect::EMPTY,
            overlay_changed: false,
            overlay_generation: 0,
            cursor_marks: HashMap::new(),
            swallow_release: false,
            ime_area: None,
        };
        if renderer.script.is_none() {
            renderer.caret_next = Some(now + CARET_BLINK);
        }
        renderer
    }

    /// Copy the host clipboard into `SharedState` so `WinQueryClipbrdData`
    /// can read it. Called at start-up and when SDL reports a clipboard
    /// change or the desktop window gains focus — not on every loop
    /// iteration, since reading it allocates and on X11 can mean a round
    /// trip to the display server.
    fn sync_host_clipboard(&mut self, shared: &Arc<SharedState>) {
        self.clipboard_synced = true;
        if let Ok(host_text) = self.video.clipboard().clipboard_text()
            && host_text != self.cached_clipboard {
                self.cached_clipboard = host_text.clone();
                let mut wm = shared.window_mgr.lock_or_recover();
                wm.clipboard_text = host_text;
                wm.clipboard.insert(CF_TEXT, 0); // invalidate stale guest pointer
        }
    }

    /// Redraw the damaged parts of the desktop and present them.
    ///
    /// The desktop buffer keeps its pixels between composites. Damage is
    /// worked out from the windows marked dirty since the last composite
    /// (`WindowManager::dirty`, mapped to their frames) and from frames whose
    /// on-screen extent or chrome changed; each damaged rectangle is filled
    /// with the desktop background and the frames that reach into it are
    /// drawn back-to-front, skipping frames an opaque frame above hides
    /// there. Only the damaged rectangles are uploaded to the texture.
    ///
    /// Frames are placed at their OS/2 screen position converted to SDL
    /// coordinates (`top = desktop_height - win.y - win.cy`), with the client
    /// frame buffer below the title bar and menu bar (VDR-C1, C2, E3).
    fn composite_and_present(&mut self, shared: &Arc<SharedState>) {
        let started = Instant::now();
        let (dw, dh) = self.desktop.size();
        let (dw, dh) = (dw as i32, dh as i32);
        let mut damage = Damage::new(dw, dh);

        // Snapshot geometry and bring the chrome cache up to date under the
        // lock; rendering happens after it is released.
        struct Snap { hwnd: u32, x: i32, y: i32, cx: i32, cy: i32, frame_flags: u32, has_menu: bool,
                      surface: SharedSurface }
        let mut dirty_frames: HashSet<u32> = HashSet::new();
        let mut chrome_changed: HashSet<u32> = HashSet::new();
        let mut cursor_marks: HashMap<u32, CursorMark> = HashMap::new();
        let snaps: Vec<Snap> = {
            let mut wm = shared.window_mgr.lock_or_recover();
            if wm.dirty.is_empty() && !self.force_full && !self.overlay_changed {
                return;
            }
            let dirty = std::mem::take(&mut wm.dirty);
            dirty_frames.extend(dirty.into_iter().filter_map(|h| owning_frame(&wm, h)));
            // PM text cursors: a frame whose cursor appeared, moved, changed
            // or went is redrawn.
            for (frame, mark) in wm.cursor_marks() {
                cursor_marks.entry(frame).or_insert(mark);
            }
            for frame in cursor_marks.keys().chain(self.cursor_marks.keys()) {
                if cursor_marks.get(frame) != self.cursor_marks.get(frame) {
                    dirty_frames.insert(*frame);
                }
            }
            let focused = wm.focused_hwnd;
            let title_colors = TitleColors::of(&wm.sys_colors);
            let chrome = &mut self.chrome;
            wm.z_order.iter().filter_map(|&hwnd| {
                let w = wm.get_window(hwnd)?;
                // Frames are shown once they have a surface.
                let surface = wm.surfaces.get(&hwnd).filter(|_| w.visible)?.clone();
                // A minimized frame shows neither its own chrome nor its menu.
                let state = w.style & (WS_MINIMIZED | WS_MAXIMIZED);
                let frame_flags = display_flags(w.frame_flags, state);
                let menu = wm.get_window(w.menu_hwnd).filter(|_| w.menu_hwnd != 0 && state & WS_MINIMIZED == 0).map(|m| {
                    (m.menu_items.as_slice(), m.window_ulong.get(&-1).copied().unwrap_or(0))
                });
                let bar_w = w.cx - 2 * frame_border_px(frame_flags);
                // Title and menu text follow the frame's queue codepage.
                let cjk = wm.queue_codepage(hwnd).map_or(CjkRegion::Auto, CjkRegion::from_codepage);
                let cache = chrome.entry(hwnd).or_insert_with(ChromeCache::new);
                let cjk_changed = cache.set_cjk(cjk) | cache.set_colors(title_colors);
                if cache.sync(&w.text, w.cx, frame_flags, state, hwnd == focused, menu, bar_w) || cjk_changed {
                    dirty_frames.insert(hwnd);
                    chrome_changed.insert(hwnd);
                }
                Some(Snap { hwnd, x: w.x, y: w.y, cx: w.cx, cy: w.cy,
                            frame_flags, has_menu: menu.is_some(), surface })
            }).collect()
        };
        self.chrome.retain(|h, _| snaps.iter().any(|s| s.hwnd == *h));
        for c in self.chrome.values_mut() {
            c.render();
        }

        let placements: Vec<FramePlacement> = snaps.iter().map(|s| {
            let fb = {
                let fb = s.surface.lock_or_recover();
                (fb.width, fb.height)
            };
            let pulldown = self.chrome[&s.hwnd].menu.as_ref()
                .and_then(|m| m.pulldown.as_ref().map(|p| (p.dx, p.h)));
            place_frame(s.hwnd, s.x, s.y, s.cx, s.cy, s.frame_flags, s.has_menu, pulldown,
                        fb, dh)
        }).collect();

        // Damage: whatever dirty or changed frames cover now or covered
        // before, plus the old extent of frames that are gone or hidden.
        let full = std::mem::take(&mut self.force_full);
        if full {
            damage.add_all();
        }
        let mut placed = HashMap::with_capacity(placements.len());
        for p in &placements {
            let now = p.extent();
            let before = self.placed.get(&p.hwnd).copied().unwrap_or(Rect::EMPTY);
            if dirty_frames.contains(&p.hwnd) || now != before {
                damage.add(before);
                damage.add(now);
            }
            placed.insert(p.hwnd, now);
        }
        for (hwnd, before) in &self.placed {
            if !placed.contains_key(hwnd) {
                damage.add(*before);
            }
        }
        self.placed = placed;
        // The pop-up menu: where it was and where it is now.
        let overlay_rect = self.popup.as_ref().map_or(Rect::EMPTY, PopupMenu::rect);
        if std::mem::take(&mut self.overlay_changed) {
            self.overlay_generation += 1;
            damage.add(self.overlay_placed);
            damage.add(overlay_rect);
        }
        self.overlay_placed = overlay_rect;
        if damage.is_empty() {
            return;
        }

        let frames = placements.iter().zip(&snaps).map(|(p, s)| SceneFrame {
            placement: p, surface: &s.surface, chrome: &self.chrome[&p.hwnd],
            cursor: cursor_marks.get(&p.hwnd).copied(),
        }).collect();
        let overlay = self.popup.as_mut().map(|p| Overlay {
            rect: overlay_rect, pixels: p.pixels(), generation: self.overlay_generation,
        });
        let present = self.desktop.compose(&Scene {
            frames, damage: &damage, chrome_changed: &chrome_changed, full, overlay,
        });
        self.cursor_marks = cursor_marks;
        let finished = Instant::now();
        let damaged: i64 = damage.rects().iter().map(|r| r.width() as i64 * r.height() as i64).sum();
        self.stats.record_composite(finished - started, present,
                                    damaged as f64 / (dw as f64 * dh as f64).max(1.0));
        self.last_composite = finished;
    }

    /// Handle a single SDL event, posting OS/2 messages to `shared` as needed.
    /// Returns `false` when the loop should return: the application exits,
    /// or (Alt+Home) a VIO session switches to full screen.
    fn handle_sdl_event(&mut self, event: Event, shared: &Arc<SharedState>) -> bool {
        let dh = self.desktop.size().1;
        // The physical keyboard and the pointer position, whoever gets the
        // input (WinGetPhysKeyState, WinQueryPointerPos).
        track_physical_input(shared, &event, dh as i32);
        // A windowed VIO program's mouse (MOUCALLS) sees the pointer over
        // its cells; PM handling (activation, chrome, drags) goes on below.
        // Not while the window uses the mouse itself: an open system menu,
        // Mark mode, a scroll-bar drag, or a frame drag or resize.
        if let Some(vio) = &self.vio && self.drag_state.is_none() && self.resize_state.is_none()
            && self.popup.is_none() && !vio.marking() && !vio.dragging() {
            vio_pointer_event(vio, shared, dh as i32, &event);
        }
        match event {
            Event::RenderTargetsReset { .. } | Event::RenderDeviceReset { .. } => {
                // The renderer may have lost its textures.
                self.force_full = true;
            }
            Event::Quit { .. } => {
                shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                let focused = shared.window_mgr.lock_or_recover().focused_hwnd;
                if focused != 0 {
                    push_msg(shared, focused, WM_CLOSE, 0, 0);
                }
                return false;
            }
            // Events of another SDL window — the full-screen text display of
            // a VIO session, still queued after Alt+Home — are not the
            // desktop's: its size change must not resize the desktop.
            Event::Window { window_id, .. } if window_id != self.desktop.window().id() => {}
            Event::Window { win_event, .. } => {
                use sdl3::event::WindowEvent;
                match win_event {
                    WindowEvent::CloseRequested => {
                        shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                        let focused = shared.window_mgr.lock_or_recover().focused_hwnd;
                        if focused != 0 {
                            push_msg(shared, focused, WM_CLOSE, 0, 0);
                        }
                        return false;
                    }
                    WindowEvent::Resized(w, h) => {
                        let (w, h) = (w as u32, h as u32);
                        // Only the view of the desktop changes: PM windows
                        // keep their size and position, as on an OS/2
                        // desktop. (Positions count from the bottom edge, so
                        // a change of height moves them on the screen.)
                        self.desktop.resize(w, h);
                        self.force_full = true;
                    }
                    WindowEvent::Exposed => {
                        // Re-upload everything: the host discarded our pixels.
                        self.force_full = true;
                        self.composite_and_present(shared);
                    }
                    WindowEvent::FocusGained => {
                        // The user may have copied something in another program.
                        self.sync_host_clipboard(shared);
                    }
                    _ => {}
                }
            }
            Event::ClipboardUpdate { .. } => {
                self.sync_host_clipboard(shared);
            }
            // Keyboard input goes to the system input queue; the thread
            // that reads it delivers it to the PM focus window of that
            // moment (a control that was clicked or given the focus with
            // WinSetFocus), else to the active frame's client.
            // Keys for a windowed VIO program go to its keyboard queue
            // (KbdCharIn), as in the text window.
            // The window's own keys (its system menu, Mark mode, a keyboard
            // move or size) come first; Alt+Home sends the session full
            // screen (the loop returns; input after it in this pass is dropped).
            // A pop-up menu or a keyboard move / size of a PM frame takes
            // the keyboard too.
            Event::KeyDown { keycode: Some(kc), scancode: Some(sc), keymod, repeat, .. }
                if self.takes_keys(shared) => {
                if self.popup.is_none() && self.frame_track.is_none()
                    && super::vio_switch::request_switch(shared, kc, keymod, repeat) {
                    return false;
                }
                return self.window_key(shared, kc, sc, keymod);
            }
            Event::KeyUp { .. } | Event::TextEditing { .. } if self.takes_keys(shared) => {}
            Event::TextInput { text, .. } if self.takes_keys(shared) => {
                if self.popup.is_none() && self.frame_track.is_none() && self.vio_active(shared)
                    && self.vio.as_ref().is_some_and(|v| !v.marking() && !v.tracking()) {
                    super::text_renderer::kbd_text_input(shared, &text);
                }
            }
            Event::KeyDown { keycode, scancode, keymod, repeat, .. } => {
                self.key_mods = kc_modifiers(keymod);
                let (mp1, mp2) = build_wm_char(keycode, scancode, keymod, repeat, false);
                shared.window_mgr.lock_or_recover().post_input(InputEvent::Key { mp1, mp2 });
            }
            Event::KeyUp { keycode, scancode, keymod, .. } => {
                self.key_mods = kc_modifiers(keymod);
                let (mp1, mp2) = build_wm_char(keycode, scancode, keymod, false, true);
                shared.window_mgr.lock_or_recover().post_input(InputEvent::Key { mp1, mp2 });
            }
            // Typed text: the final characters after keyboard layout, Shift,
            // dead keys and input-method composition. KeyDown above carries no
            // printable character, so nothing is delivered twice. Each
            // character is encoded in the codepage of the queue that reads it.
            Event::TextInput { text, .. } => {
                let mut wm = shared.window_mgr.lock_or_recover();
                for ch in text.chars().filter(|ch| !ch.is_control()) {
                    wm.post_input(InputEvent::Char(ch));
                }
            }
            Event::MouseMotion { x, y, .. } => {
                // SDL3 reports float pixels; PM works in whole pels.
                let (x, y) = (x as i32, y as i32);
                // A pop-up menu takes the pointer: the item under it lights up.
                if let Some(p) = self.popup.as_mut() {
                    self.overlay_changed |= p.hover(x, y);
                    return true;
                }
                // The VIO window's own drags (marking, a scroll-bar thumb).
                if let Some(vio) = self.vio.as_mut() && vio.dragging() {
                    vio.drag_to(shared, x, (dh as i32 - 1) - y);
                    vio.update(shared);
                    return true;
                }
                // VDR-C5: if a resize is active, update window geometry.
                if let Some(ref rs) = self.resize_state {
                    let hwnd = rs.hwnd;
                    let orig = (rs.orig_x, rs.orig_y, rs.orig_cx, rs.orig_cy);
                    let (dx, dy) = (x - rs.press_x, y - rs.press_y);
                    let edge = rs.edge;
                    let mut wm = shared.window_mgr.lock_or_recover();
                    // The VIO frame shows at most its whole screen.
                    let is_vio = wm.vio_frame == hwnd;
                    let rect = resized_rect(orig, edge, dx, dy, |cx, cy| {
                        wm.vio_clamp_size(hwnd, cx, cy).unwrap_or((cx.max(MIN_RESIZE), cy.max(MIN_RESIZE)))
                    });
                    wm.mark_dirty(hwnd); // VDR-B4
                    if let Some(win) = wm.get_window_mut(hwnd) {
                        (win.x, win.y, win.cx, win.cy) = rect;
                    }
                    if is_vio {
                        // Lay the VIO window out at once: its view and
                        // scroll bars follow the drag.
                        let (scx, scy) = wm.layout_frame(hwnd);
                        wm.resize_surface(hwnd, scx, scy);
                        drop(wm);
                        if let Some(vio) = self.vio.as_mut() { vio.update(shared); }
                    }
                // VDR-D3: if a drag is active, move the window.
                } else if let Some(ref ds) = self.drag_state {
                    let hwnd = ds.hwnd;
                    let anchor_x = ds.anchor_x;
                    let anchor_y_sdl = ds.anchor_y_sdl;
                    let mut wm = shared.window_mgr.lock_or_recover();
                    let cy = wm.get_window(hwnd).map_or(0, |w| w.cy);
                    let new_top_y_sdl = y - anchor_y_sdl;
                    wm.move_frame(hwnd, x - anchor_x, dh as i32 - new_top_y_sdl - cy);
                } else if let Some((control, cx, cy)) = pressed_control_release(shared, x, (dh as i32 - 1) - y) {
                    // A built-in control held with button 1 follows the
                    // pointer (a text control extends its selection).
                    let mp1 = ((cx as u32) & 0xFFFF) | ((cy as u32 & 0xFFFF) << 16);
                    push_input(shared, control, WM_MOUSEMOVE, mp1, 0);
                } else {
                    let os2_y = (dh as i32 - 1) - y;
                    // VDR-D6: captured window overrides hit-test.
                    let capture = shared.window_mgr.lock_or_recover().capture_hwnd;
                    let target = if capture != 0 {
                        Some(capture)
                    } else {
                        let wm = shared.window_mgr.lock_or_recover();
                        wm.z_hit_test(x, os2_y)
                    };
                    if let Some(hwnd) = target {
                        let (local_x, local_y) = {
                            let wm = shared.window_mgr.lock_or_recover();
                            pointer_local(&wm, hwnd, x, os2_y)
                        };
                        let mp1 = ((local_x as u32) & 0xFFFF) | ((local_y as u32 & 0xFFFF) << 16);
                        push_input(shared, hwnd, WM_MOUSEMOVE, mp1, 0);
                    }
                }
            }
            Event::MouseButtonDown { mouse_btn, clicks, x, y, .. } => {
                // SDL3 reports float pixels; PM works in whole pels.
                let (x, y) = (x as i32, y as i32);
                let os2_y = (dh as i32 - 1) - y;
                // An open pop-up menu takes the click.
                if self.popup.is_some()
                    && let Some(keep_running) = self.popup_press(shared, mouse_btn, clicks, x, y) {
                    return keep_running;
                }
                // A click ends a keyboard move or size where the frame is
                // (Warpine's choice: PM lets the mouse take the tracking over).
                self.end_frame_track(shared);
                // VDR-D6: captured window overrides hit-test.
                let capture = shared.window_mgr.lock_or_recover().capture_hwnd;
                let target = if capture != 0 {
                    Some(capture)
                } else {
                    let wm = shared.window_mgr.lock_or_recover();
                    wm.z_hit_test(x, os2_y)
                };
                if let Some(hwnd) = target {
                    // The owner of a modal dialog is disabled: it takes no
                    // clicks, cannot be activated, moved or resized.
                    if clicks_blocked(shared, hwnd) {
                        return true;
                    }
                    // VDR-D4: activate window on click; post WM_ACTIVATE to both windows.
                    let prev_focused = {
                        let mut wm = shared.window_mgr.lock_or_recover();
                        let frame = wm.client_to_frame(hwnd);
                        let frame = if wm.is_frame(frame) { frame } else { hwnd };
                        let prev = wm.focused_hwnd;
                        if wm.is_frame(frame) && prev != frame {
                            // Keys typed before the click were meant for
                            // the old active window.
                            wm.pin_pending_input();
                            wm.focused_hwnd = frame;
                            wm.z_push_top(frame);
                            prev
                        } else { 0 }
                    };
                    if prev_focused != 0 {
                        // Notify old frame it lost focus.
                        push_msg(shared, prev_focused, WM_ACTIVATE, 0, hwnd);
                        // Notify new frame it gained focus (WA_CLICK = 2).
                        let new_focused = shared.window_mgr.lock_or_recover().focused_hwnd;
                        push_msg(shared, new_focused, WM_ACTIVATE, 2, prev_focused);
                    }
                    // VDR-D2/C3/C4: chrome hit-testing (title bar + buttons).
                    let (chrome_hit, state) = {
                        let wm = shared.window_mgr.lock_or_recover();
                        wm.get_window(hwnd).map_or((ChromeHit::None, 0), |win| {
                            let state = win.style & (WS_MINIMIZED | WS_MAXIMIZED);
                            (frame_chrome_hit(win.x, win.y, win.cx, win.cy, win.frame_flags, state, x, os2_y),
                             state)
                        })
                    };
                    let double = mouse_btn == sdl3::mouse::MouseButton::Left
                        && button_down_msg(mouse_btn, clicks) == Some(WM_BUTTON1DBLCLK);
                    // System commands from the chrome: MPFROM2SHORT(source, fPointer).
                    let syscommand = |sc: u32, source: u32| push_msg(shared, hwnd, WM_SYSCOMMAND, sc, source | (1 << 16));
                    // The VIO window has no message queue: its system commands
                    // are carried out here (`vio_command`); closing it ends the program.
                    let is_vio = shared.window_mgr.lock_or_recover().is_vio_window(hwnd);
                    match chrome_hit {
                        // The VIO window's system commands are carried out
                        // here (it has no message queue to post them to).
                        ChromeHit::Close | ChromeHit::SysMenu if is_vio
                            && (matches!(chrome_hit, ChromeHit::Close) || double) => {
                            return self.vio_command(shared, SC_CLOSE);
                        }
                        ChromeHit::SysMenu if is_vio => {
                            // A single click opens the system menu.
                            self.open_vio_menu(shared, false);
                            self.swallow_release = true;
                        }
                        ChromeHit::TitleBar if is_vio && double => {
                            return self.vio_command(shared, if state != 0 { SC_RESTORE } else { SC_MAXIMIZE });
                        }
                        ChromeHit::Minimize if is_vio => return self.vio_command(shared, SC_MINIMIZE),
                        ChromeHit::Maximize if is_vio => {
                            let sc = if state & WS_MAXIMIZED != 0 { SC_RESTORE } else { SC_MAXIMIZE };
                            return self.vio_command(shared, sc);
                        }
                        ChromeHit::Close => {
                            push_msg(shared, hwnd, WM_CLOSE, 0, 0);
                        }
                        ChromeHit::SysMenu if double => {
                            // Double click on the system-menu icon: Close.
                            syscommand(SC_CLOSE, CMDSRC_MENU);
                        }
                        ChromeHit::TitleBar if double => {
                            // Double click on the title bar (or on a minimized
                            // frame): restore a minimized or maximized frame,
                            // maximize any other.
                            syscommand(if state != 0 { SC_RESTORE } else { SC_MAXIMIZE }, CMDSRC_OTHER);
                        }
                        // A single click on the icon opens the system menu
                        // (its release is not the menu's).
                        ChromeHit::SysMenu => {
                            self.open_frame_menu(shared, hwnd, false);
                            self.swallow_release = self.popup.is_some();
                        }
                        // A maximized frame is not dragged.
                        ChromeHit::TitleBar if state & WS_MAXIMIZED != 0 => {}
                        ChromeHit::Minimize => {
                            // VDR-C4: post WM_SYSCOMMAND(SC_MINIMIZE) to the client.
                            syscommand(SC_MINIMIZE, CMDSRC_MENU);
                        }
                        ChromeHit::Hide => {
                            // FCF_HIDEBUTTON: WM_SYSCOMMAND(SC_HIDE) to the client.
                            syscommand(SC_HIDE, CMDSRC_MENU);
                        }
                        ChromeHit::Maximize => {
                            // VDR-C4: WM_SYSCOMMAND(SC_MAXIMIZE) to the client;
                            // on a maximized frame the button restores it.
                            let sc = if state & WS_MAXIMIZED != 0 { SC_RESTORE } else { SC_MAXIMIZE };
                            syscommand(sc, CMDSRC_MENU);
                        }
                        ChromeHit::TitleBar => {
                            // VDR-D3: start drag — record how far into the title bar the user clicked.
                            let (anchor_x, dst_top_y) = {
                                let wm = shared.window_mgr.lock_or_recover();
                                wm.get_window(hwnd)
                                    .map(|w| (x - w.x, dh as i32 - w.y - w.cy))
                                    .unwrap_or((0, 0))
                            };
                            self.drag_state = Some(DragState {
                                hwnd, anchor_x, anchor_y_sdl: y - dst_top_y,
                            });
                        }
                        ChromeHit::None => {
                            // VDR-C5: check resize edge before forwarding to app.
                            let resize_edge = {
                                let wm = shared.window_mgr.lock_or_recover();
                                wm.get_window(hwnd).and_then(|win| {
                                    // Minimized and maximized frames are not resized.
                                    if win.style & (WS_MINIMIZED | WS_MAXIMIZED) != 0 { return None; }
                                    resize_hit_test(win.x, win.y, win.cx, win.cy,
                                                    win.frame_flags, x, os2_y)
                                })
                            };
                            if let Some(edge) = resize_edge {
                                let (orig_x, orig_y, orig_cx, orig_cy) = {
                                    let wm = shared.window_mgr.lock_or_recover();
                                    wm.get_window(hwnd)
                                        .map(|w| (w.x, w.y, w.cx, w.cy))
                                        .unwrap_or((0, 0, 0, 0))
                                };
                                self.resize_state = Some(ResizeState {
                                    hwnd, edge,
                                    press_x: x, press_y: y,
                                    orig_x, orig_y, orig_cx, orig_cy,
                                });
                            } else if is_vio && mouse_btn == sdl3::mouse::MouseButton::Left
                                && let Some(vio) = self.vio.as_mut() && vio.press(shared, x, os2_y) {
                                // The VIO window's scroll bars, and marking.
                                vio.update(shared);
                            } else {
                            let (local_x, local_y) = {
                                let wm = shared.window_mgr.lock_or_recover();
                                pointer_local(&wm, hwnd, x, os2_y)
                            };
                            use sdl3::mouse::MouseButton;
                            let msg = button_down_msg(mouse_btn, clicks);
                            // Button 1 goes to the built-in control under the
                            // pointer, if any; a click elsewhere takes the
                            // keyboard focus away from the focused control.
                            let (hwnd, local_x, local_y) = if mouse_btn == MouseButton::Left {
                                let (target, lost_focus) = route_button1(shared, hwnd, local_x, local_y, true);
                                if lost_focus != 0 {
                                    // Repaint the control without its caret.
                                    shared.window_mgr.lock_or_recover().invalidate_window_only(lost_focus);
                                }
                                target
                            } else {
                                (hwnd, local_x, local_y)
                            };
                            let mp1 = ((local_x as u32) & 0xFFFF) | ((local_y as u32 & 0xFFFF) << 16);
                            if let Some(m) = msg { push_input(shared, hwnd, m, mp1, mouse_mp2(self.key_mods)); }
                            }
                        }
                    }
                }
            }
            Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                // SDL3 reports float pixels; PM works in whole pels.
                let (x, y) = (x as i32, y as i32);
                let left = mouse_btn == sdl3::mouse::MouseButton::Left;
                // A pop-up menu chooses the item the button comes up over.
                if left && let Some(p) = self.popup.as_mut() && p.is_pressed() {
                    let action = p.release(x, y);
                    return self.popup_action(shared, action, true);
                }
                // The release of a click that closed a pop-up menu.
                if std::mem::take(&mut self.swallow_release) {
                    return true;
                }
                if left && let Some(vio) = self.vio.as_mut() && vio.release() {
                    vio.update(shared);
                    return true;
                }
                // VDR-C5: end resize on button release.
                if let Some(rs) = self.resize_state.take() {
                    let hwnd = rs.hwnd;
                    // Re-lay out the menu bar and client for the new frame size; the
                    // surface and WM_SIZE both describe the client area.
                    {
                        let mut wm = shared.window_mgr.lock_or_recover();
                        let client = wm.frame_msg_target(hwnd);
                        let old = wm.get_window(client).map_or((0, 0), |w| (w.cx, w.cy));
                        let new = wm.layout_frame(hwnd);
                        wm.resize_surface(hwnd, new.0, new.1);
                        // Notify the app (old and new size) so it repaints.
                        if old != new { wm.post_size(client, old, new); }
                    }
                    self.composite_and_present(shared);
                    return true;
                }
                // VDR-D3: end drag on button release, regardless of which button.
                if self.drag_state.is_some() {
                    self.drag_state = None;
                    // Trigger one final composite to show the window at its resting position.
                    self.composite_and_present(shared);
                    return true;
                }
                let _ = (x, y); // suppress unused-variable warnings for x/y used above
                let os2_y = (dh as i32 - 1) - y;
                // A built-in button held down gets the release wherever the
                // pointer is, so it pops up (and is clicked only over itself).
                if mouse_btn == sdl3::mouse::MouseButton::Left
                    && let Some((button, bx, by)) = pressed_control_release(shared, x, os2_y)
                {
                    let mp1 = ((bx as u32) & 0xFFFF) | ((by as u32 & 0xFFFF) << 16);
                    push_input(shared, button, WM_BUTTON1UP, mp1, mouse_mp2(self.key_mods));
                    return true;
                }
                // VDR-D6: captured window overrides hit-test.
                let capture = shared.window_mgr.lock_or_recover().capture_hwnd;
                let target = if capture != 0 {
                    Some(capture)
                } else {
                    let wm = shared.window_mgr.lock_or_recover();
                    wm.z_hit_test(x, os2_y)
                };
                if let Some(hwnd) = target {
                    if clicks_blocked(shared, hwnd) {
                        return true;
                    }
                    let (local_x, local_y) = {
                        let wm = shared.window_mgr.lock_or_recover();
                        pointer_local(&wm, hwnd, x, os2_y)
                    };
                    use sdl3::mouse::MouseButton;
                    let (hwnd, local_x, local_y) = if mouse_btn == MouseButton::Left && capture == 0 {
                        route_button1(shared, hwnd, local_x, local_y, false).0
                    } else {
                        (hwnd, local_x, local_y)
                    };
                    let mp1 = ((local_x as u32) & 0xFFFF) | ((local_y as u32 & 0xFFFF) << 16);
                    let msg = match mouse_btn {
                        MouseButton::Left   => Some(WM_BUTTON1UP),
                        MouseButton::Right  => Some(WM_BUTTON2UP),
                        MouseButton::Middle => Some(WM_BUTTON3UP),
                        _ => None,
                    };
                    if let Some(m) = msg { push_input(shared, hwnd, m, mp1, mouse_mp2(self.key_mods)); }
                }
            }
            _ => {}
        }
        true
    }
}

impl PmRenderer for SdlRenderer {
    fn handle_message(&mut self, msg: GUIMessage, shared: &Arc<SharedState>) {
        match msg {
            GUIMessage::SystemMenu { frame, keyboard } => self.open_frame_menu(shared, frame, keyboard),
            GUIMessage::TrackFrame { frame, size } => {
                self.close_popup();
                self.start_frame_track(shared, frame, size);
            }
            GUIMessage::PopupMenu(req) => {
                self.end_frame_track(shared);
                self.open_popup_request(req);
            }
            GUIMessage::SetClipboardText(text) => {
                let _ = self.video.clipboard().set_clipboard_text(&text);
                self.cached_clipboard = text;
            }
            GUIMessage::SetMouseCapture(hwnd) => {
                // hwnd == 0 releases capture; any other value acquires it.
                self.mouse.capture(hwnd != 0);
            }
            GUIMessage::SetPointer(shape) => self.pointer.apply(shape),
            GUIMessage::SetPointerPos { x, y } => {
                // A scripted session does not follow the host pointer, so
                // it does not move it either (WinSetPointerPos has already
                // set the PM pointer position).
                if self.script.is_none() {
                    let dh = self.desktop.size().1 as i32;
                    self.mouse.warp_mouse_in_window(self.desktop.window(), x as f32, (dh - 1 - y) as f32);
                }
            }
        }
    }

    fn poll_events(&mut self, shared: &Arc<SharedState>) -> bool {
        self.stats.record_iteration();
        // Audio commands from the vCPU threads (they wake this loop too).
        self.audio.pump(&shared.audio);
        // A windowed VIO program: show what it wrote since the last pass.
        if let Some(vio) = self.vio.as_mut() {
            vio.update(shared);
        }
        // Read the host clipboard once at start-up; after that SDL tells us
        // when it changes (ClipboardUpdate) and we re-read on focus gain.
        if !self.clipboard_synced {
            self.sync_host_clipboard(shared);
        }

        // A scripted session must not depend on where the host pointer is
        // or what is typed: a stray motion event during a scripted drag
        // would move the window off the scripted path. Its input is added
        // below, after this filter.
        let scripted_session = self.script.is_some();
        let mut events: Vec<_> = self.pending_events.drain(..)
            .chain(self.event_pump.poll_iter())
            .filter(|e| !is_wake_event(e) && !(scripted_session && is_host_input(e)))
            .collect();

        // Scripted session: turn due steps into input events and captures.
        let scripted = self.script.as_mut().map(GuiScript::due).unwrap_or_default();
        for input in scripted {
            match input {
                gui_script::Input::Dump(path) => {
                    // Input scripted before the capture must have taken
                    // effect by the time the desktop is read.
                    for event in events.drain(..) {
                        if !self.handle_sdl_event(event, shared) {
                            return false;
                        }
                    }
                    // Capture what is on screen now, including changes that
                    // have not been composited yet.
                    shared.window_mgr.lock_or_recover().mark_all_dirty();
                    self.composite_and_present(shared);
                    let (pixels, w, h) = self.desktop.snapshot();
                    if let Err(e) = gui_script::write_ppm(&path, &pixels, w, h) {
                        eprintln!("warpine: cannot write {}: {e}", path.display());
                    }
                }
                input => events.extend(script_input_to_event(&input)),
            }
        }

        for event in events {
            if !self.handle_sdl_event(event, shared) {
                return false;
            }
        }

        // Caret blink: the focused text control toggles its caret.
        if let Some(next) = self.caret_next {
            let now = Instant::now();
            if now >= next {
                let mut wm = shared.window_mgr.lock_or_recover();
                wm.post_caret_blink();
                // CURSOR_FLASH cursors blink at the same rate.
                wm.flash_cursors();
                // SV_CURSORRATE (WinSetSysValue) sets the interval.
                self.caret_next = Some(now + Duration::from_millis(wm.cursor_rate_ms as u64));
            }
        }

        // Drag scroll: an entry field, MLE or list box held with the pointer
        // resting outside it keeps scrolling. Not in scripted sessions,
        // which must not depend on timing.
        if self.script.is_none() {
            let held = shared.window_mgr.lock_or_recover().pressed_control != 0;
            let (next, due) = drag_scroll_step(held, self.drag_scroll_next, Instant::now());
            self.drag_scroll_next = next;
            if due {
                shared.window_mgr.lock_or_recover().post_drag_scroll();
            }
        }

        // Host input method: its window goes where the focused text
        // control's caret is.
        let area = shared.window_mgr.lock_or_recover().text_input_area(self.desktop.size().1 as i32);
        if area.is_some() && area != self.ime_area {
            self.ime_area = area;
            if let Some(((x, y, w, h), cursor)) = area {
                let rect = sdl3::rect::Rect::new(x, y, w.max(1) as u32, h.max(1) as u32);
                self.video.text_input().set_rect(self.desktop.window(), rect, cursor);
            }
        }

        // Frame pacing: composite at most once per FRAME_INTERVAL.
        // `composite_and_present` itself returns early when nothing is dirty
        // and no drag or resize is in progress.
        if self.last_composite.elapsed() >= FRAME_INTERVAL {
            self.composite_and_present(shared);
        }

        if let Some(report) = self.stats.take_report(Instant::now()) {
            debug!("[GUI] {report}");
        }
        true
    }

    fn install_waker(&mut self, wake: Arc<GuiWake>) {
        self.wake = Some(wake.clone());
        let _ = self.wake_slot.set(wake.clone());
        if !self.native_wait { return; }
        let sender = self.event.event_sender();
        wake.set_hook(Box::new(move || {
            // SDL_PushEvent is thread-safe. A failure (queue full, SDL shut
            // down) only costs latency: the loop still wakes by IDLE_WAIT.
            let _ = sender.push_custom_event(WakeEvent);
        }));
    }

    fn wait_for_work(&mut self, shared: &Arc<SharedState>) {
        // A composite is owed only for dirty windows (drags and resizes mark
        // theirs on every mouse move) or a pending full redraw. A drag with
        // the mouse at rest owes nothing: the compositor would return early
        // without advancing `last_composite`, and the zero timeout that
        // follows would spin the loop.
        let busy = self.force_full || self.overlay_changed
            || !shared.window_mgr.lock_or_recover().dirty.is_empty();
        let script_next = self.script.as_ref().and_then(GuiScript::next_due);
        let timed_next = script_next.into_iter().chain(self.caret_next).chain(self.drag_scroll_next).min();
        let mut timeout = wait_timeout(Instant::now(), self.last_composite, busy, timed_next);
        // Notice finished audio promptly (MCI_WAIT, MM_MCINOTIFY, DosBeep).
        if self.audio.busy() {
            timeout = timeout.min(super::audio::AUDIO_PUMP_INTERVAL);
        }
        // A VIO program writes its screen without waking the loop.
        if self.vio.is_some() {
            timeout = timeout.min(super::vio_window::VIO_POLL);
        }
        if timeout.is_zero() {
            return;
        }
        match (self.native_wait, &self.wake) {
            // Native wait: input and wake-ups (pushed as user events) both
            // end it.
            (true, _) => {
                if let Some(event) = self.event_pump.wait_event_timeout(timeout.max(Duration::from_millis(1))) {
                    self.pending_events.push(event);
                }
            }
            // Headless driver: no real input, so wake-ups and the deadline
            // (which includes the next scripted step) are all there is.
            (false, Some(wake)) => wake.wait(timeout),
            (false, None) => std::thread::sleep(timeout.min(super::renderer::POLL_PERIOD)),
        }
    }
}

/// Pass a mouse event to the windowed VIO program's mouse queue.
fn vio_pointer_event(vio: &super::vio_window::VioWindow, shared: &SharedState, desk_h: i32, event: &Event) {
    match *event {
        Event::MouseMotion { x, y, .. } => vio.pointer_event(shared, desk_h, x as i32, y as i32, None),
        Event::MouseButtonDown { mouse_btn, x, y, .. } | Event::MouseButtonUp { mouse_btn, x, y, .. } => {
            if let Some(b) = mou_button(mouse_btn) {
                let down = matches!(event, Event::MouseButtonDown { .. });
                vio.pointer_event(shared, desk_h, x as i32, y as i32, Some((b, down)));
            }
        }
        _ => {}
    }
}

/// The OS/2 mouse button of an SDL button: 1 left, 2 right, 3 middle.
pub(crate) fn mou_button(button: sdl3::mouse::MouseButton) -> Option<crate::loader::moucalls::MouButton> {
    use crate::loader::moucalls::MouButton;
    use sdl3::mouse::MouseButton;
    match button {
        MouseButton::Left => Some(MouButton::B1),
        MouseButton::Right => Some(MouButton::B2),
        MouseButton::Middle => Some(MouButton::B3),
        _ => None,
    }
}

/// Mouse, keyboard and text input from the host. Ignored while a GUI script
/// runs, so a scripted session behaves the same on a real display as on
/// the headless drivers; window events (resize, expose, close) still count.
pub(crate) fn is_host_input(event: &Event) -> bool {
    matches!(event,
        Event::MouseMotion { .. } | Event::MouseButtonDown { .. } | Event::MouseButtonUp { .. }
        | Event::MouseWheel { .. } | Event::KeyDown { .. } | Event::KeyUp { .. }
        | Event::TextInput { .. } | Event::TextEditing { .. })
}

/// SDL video drivers with no input source: SDL cannot block on them and
/// would poll inside its wait, so the loop waits on the channel instead.
const NON_WAITING_DRIVERS: &[&str] = &["dummy", "offscreen"];

/// Whether SDL's event wait can block on `driver` (as named by
/// `SDL_GetCurrentVideoDriver`).
fn driver_can_wait(driver: &str) -> bool {
    !NON_WAITING_DRIVERS.iter().any(|d| d.eq_ignore_ascii_case(driver))
}

/// Whether `kc` with `keymod` is a key that opens (or closes) a system
/// menu: Alt+Space or Shift+Esc (IBM OS/2 1.2 Quick Reference).
fn is_system_menu_key(kc: Keycode, keymod: KeyMod) -> bool {
    let alt = keymod.intersects(KeyMod::LALTMOD | KeyMod::RALTMOD);
    let shift = keymod.intersects(KeyMod::LSHIFTMOD | KeyMod::RSHIFTMOD);
    (alt && kc == Keycode::Space) || (shift && kc == Keycode::Escape)
}

/// The keyboard move / size key `kc` stands for, if any.
fn track_key_of(kc: Keycode) -> Option<TrackKey> {
    match kc {
        Keycode::Left => Some(TrackKey::Left),
        Keycode::Right => Some(TrackKey::Right),
        Keycode::Up => Some(TrackKey::Up),
        Keycode::Down => Some(TrackKey::Down),
        Keycode::Return | Keycode::KpEnter => Some(TrackKey::Enter),
        Keycode::Escape => Some(TrackKey::Esc),
        _ => None,
    }
}

/// Give PM frame `frame` the position and size `r` (a keyboard size): its
/// menu bar, client and surface are laid out again at once.
fn set_frame_rect(wm: &mut WindowManager, frame: u32, r: WinRect) {
    let Some(w) = wm.get_window_mut(frame) else { return };
    (w.x, w.y, w.cx, w.cy) = (r.x, r.y, r.cx, r.cy);
    let (cx, cy) = wm.layout_frame(frame);
    wm.resize_surface(frame, cx, cy);
}

/// The user event the wake hook pushes: zero-sized, so pushing it
/// allocates nothing.
struct WakeEvent;

/// Whether `event` is a [`WakeEvent`]. Warpine pushes no other user events.
fn is_wake_event(event: &Event) -> bool {
    event.is_user_event() && event.as_user_event_type::<WakeEvent>().is_some()
}

// ── Internal helpers ───────────────────────────────────────────────────────

/// Result of a chrome hit-test on a mouse click.
#[derive(Debug, PartialEq)]
enum ChromeHit {
    /// Click is on the close (×) button — send WM_CLOSE.
    Close,
    /// Click is on the system-menu icon (VDR-C3): a click opens the system
    /// menu (`system_menu.rs`); a double click closes the window.
    SysMenu,
    /// Click is on the minimize button (VDR-C4) — send WM_SYSCOMMAND(SC_MINIMIZE).
    Minimize,
    /// Click is on the maximize button (VDR-C4) — send WM_SYSCOMMAND(SC_MAXIMIZE).
    Maximize,
    /// Click is on the hide button (`FCF_HIDEBUTTON`) — send WM_SYSCOMMAND(SC_HIDE).
    Hide,
    /// Click is in the title bar but not on any button — activate / drag.
    TitleBar,
    /// Click is in the client area — forward to the app.
    None,
}

/// Perform a chrome hit-test for a click at SDL `(x, _)` / OS/2 `os2_y` on `win`.
///
/// Returns the hit target.  `dst_top_y` is the SDL y of the top of the frame's
/// title bar.  Call only when the title bar is enabled (`FCF_TITLEBAR`).
fn chrome_hit_test(win_x: i32, win_cx: i32, win_fcf: u32,
                   x: i32, os2_y: i32, win_y: i32, win_cy: i32) -> ChromeHit {
    // Title bar occupies OS/2 y in [win_y + win_cy - CHROME_TITLE_H, win_y + win_cy).
    let title_bot_os2 = win_y + win_cy - CHROME_TITLE_H;
    if os2_y < title_bot_os2 {
        return ChromeHit::None;
    }
    // Right-side buttons: close is rightmost, then max (if present), then min.
    let mut btn_right = win_x + win_cx - CHROME_BTN_MARGIN;
    let close_x0 = btn_right - CHROME_BTN_W;
    if x >= close_x0 && x < btn_right {
        return ChromeHit::Close;
    }
    btn_right = close_x0 - CHROME_BTN_GAP;
    if win_fcf & FCF_MAXBUTTON != 0 {
        let max_x0 = btn_right - CHROME_BTN_W;
        if x >= max_x0 && x < btn_right {
            return ChromeHit::Maximize;
        }
        btn_right = max_x0 - CHROME_BTN_GAP;
    }
    if let Some(glyph) = min_slot_glyph(win_fcf) {
        let min_x0 = btn_right - CHROME_BTN_W;
        if x >= min_x0 && x < btn_right {
            return if matches!(glyph, ButtonGlyph::Hide) { ChromeHit::Hide } else { ChromeHit::Minimize };
        }
    }
    // Left-side: system menu icon.
    if win_fcf & FCF_SYSMENU != 0 {
        let sysmenu_x1 = win_x + SYSMENU_ICON_X + CHROME_BTN_W;
        if x >= win_x + SYSMENU_ICON_X && x < sysmenu_x1 {
            return ChromeHit::SysMenu;
        }
    }
    ChromeHit::TitleBar
}

/// Chrome hit-test of a click at `(x, os2_y)` on a frame at `(win_x, win_y)`
/// of size `win_cx` × `win_cy` in `state` (`WS_MINIMIZED` / `WS_MAXIMIZED`):
/// a minimized frame is all title bar; other frames are tested by
/// [`chrome_hit_test`] when they have a title bar.
#[allow(clippy::too_many_arguments)]
fn frame_chrome_hit(win_x: i32, win_y: i32, win_cx: i32, win_cy: i32, frame_flags: u32, state: u32,
                    x: i32, os2_y: i32) -> ChromeHit {
    if state & WS_MINIMIZED != 0 {
        let inside = x >= win_x && x < win_x + win_cx && os2_y >= win_y && os2_y < win_y + win_cy;
        return if inside { ChromeHit::TitleBar } else { ChromeHit::None };
    }
    if frame_flags & FCF_TITLEBAR == 0 {
        return ChromeHit::None;
    }
    chrome_hit_test(win_x, win_cx, frame_flags, x, os2_y, win_y, win_cy)
}

/// Test if a click at `(x, os2_y)` is on a resize edge of the window (VDR-C5).
///
/// Returns `Some(edge)` when `FCF_SIZEBORDER` is set and the click is within
/// `RESIZE_HIT_W` pixels of an edge.  Does NOT include the title-bar area for
/// N/NE/NW edges — those are handled by the chrome hit-test first.
fn resize_hit_test(win_x: i32, win_y: i32, win_cx: i32, win_cy: i32,
                   win_fcf: u32, x: i32, os2_y: i32) -> Option<ResizeEdge> {
    if win_fcf & FCF_SIZEBORDER == 0 { return None; }
    let near_left   = x   >= win_x            && x   < win_x            + RESIZE_HIT_W;
    let near_right  = x   >= win_x + win_cx - RESIZE_HIT_W && x < win_x + win_cx;
    let near_bottom = os2_y >= win_y            && os2_y < win_y            + RESIZE_HIT_W;
    let near_top    = os2_y >= win_y + win_cy - RESIZE_HIT_W && os2_y < win_y + win_cy;
    match (near_left, near_right, near_bottom, near_top) {
        (true,  false, false, false) => Some(ResizeEdge::W),
        (false, true,  false, false) => Some(ResizeEdge::E),
        (false, false, true,  false) => Some(ResizeEdge::S),
        (false, false, false, true ) => Some(ResizeEdge::N),
        (true,  false, true,  false) => Some(ResizeEdge::SW),
        (false, true,  true,  false) => Some(ResizeEdge::SE),
        (true,  false, false, true ) => Some(ResizeEdge::NW),
        (false, true,  false, true ) => Some(ResizeEdge::NE),
        _ => None,
    }
}

/// Smallest frame width and height a sizing-border drag leaves.
const MIN_RESIZE: i32 = 32;

/// A frame at `orig` (`x, y, cx, cy`, OS/2 coordinates, bottom-left origin)
/// after its `edge` was dragged by `(dx, dy)` desktop pels (`dy` positive
/// downwards, as SDL counts): the dragged edges follow the pointer, the
/// opposite ones stay, and `clamp` limits the size.
fn resized_rect(orig: (i32, i32, i32, i32), edge: ResizeEdge, dx: i32, dy: i32,
                clamp: impl Fn(i32, i32) -> (i32, i32)) -> (i32, i32, i32, i32) {
    use ResizeEdge::*;
    let (x, y, cx, cy) = orig;
    let left = matches!(edge, W | NW | SW);
    let right = matches!(edge, E | NE | SE);
    let top = matches!(edge, N | NE | NW);
    let bottom = matches!(edge, S | SE | SW);
    let ncx = if right { cx + dx } else if left { cx - dx } else { cx };
    // Dragging the bottom edge down or the top edge up makes it taller.
    let ncy = if bottom { cy + dy } else if top { cy - dy } else { cy };
    let (ncx, ncy) = clamp(ncx, ncy);
    // PM y is the bottom edge: it moves when the bottom edge is dragged.
    (if left { x + cx - ncx } else { x }, if bottom { y + cy - ncy } else { y }, ncx, ncy)
}

/// Decide which window receives a mouse-button-1 message aimed at `frame`.
///
/// `(x, y)` is the pointer relative to the frame's bottom-left corner. If a
/// built-in control lies under the pointer the message goes to that control,
/// with the position relative to the control; otherwise it goes to the frame
/// (whose client the input queue delivers to) unchanged.
///
/// With `press` set, a click that is not on a control ends that frame's
/// control focus: the second return value is the control that lost the focus
/// (0 if none) so the caller can have it repainted without its caret.
fn route_button1(shared: &Arc<SharedState>, frame: u32, x: i32, y: i32, press: bool)
    -> ((u32, i32, i32), u32)
{
    let mut wm = shared.window_mgr.lock_or_recover();
    // `(x, y)` is in message coordinates (`pointer_local`): a standard
    // frame's client, whose buffer is the surface; a dialog's surface
    // starts inside its left border.
    let border = match wm.get_window(frame) {
        Some(w) if !wm.frame_to_client.contains_key(&frame) => frame_border_px(w.frame_flags),
        _ => 0,
    };
    if let Some(hit) = wm.control_at(frame, x - border, y) {
        return (hit, 0);
    }
    let focus = wm.focus_hwnd;
    let lost = if press && focus != 0 && wm.frame_to_client.contains_key(&frame)
        && wm.find_frame_for_hwnd(focus) == frame
    {
        // Keys typed before the click were meant for the control.
        wm.pin_pending_input();
        wm.set_focus(0);
        focus
    } else {
        0
    };
    ((frame, x, y), lost)
}

/// Desktop point `(x, y)` (bottom-left origin) in the coordinates of the
/// window that gets the pointer messages addressed to `hwnd`: a standard
/// frame's client (inside the frame's left border), else `hwnd` itself (a
/// dialog, a window holding the capture).
fn pointer_local(wm: &WindowManager, hwnd: u32, x: i32, y: i32) -> (i32, i32) {
    let target = wm.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
    let (ox, oy) = wm.window_origin(target).unwrap_or((0, 0));
    (x - ox, y - oy)
}

/// The PM message for a mouse button press: `WM_BUTTONnDOWN`, or
/// `WM_BUTTONnDBLCLK` for the second press of a double click (SDL counts
/// the presses of a multi-click; every second one completes a double click).
fn button_down_msg(button: sdl3::mouse::MouseButton, clicks: u8) -> Option<u32> {
    use sdl3::mouse::MouseButton;
    let double = clicks >= 2 && clicks.is_multiple_of(2);
    match button {
        MouseButton::Left   => Some(if double { WM_BUTTON1DBLCLK } else { WM_BUTTON1DOWN }),
        MouseButton::Right  => Some(if double { WM_BUTTON2DBLCLK } else { WM_BUTTON2DOWN }),
        MouseButton::Middle => Some(if double { WM_BUTTON3DBLCLK } else { WM_BUTTON3DOWN }),
        _ => None,
    }
}

/// Whether mouse clicks on window `hwnd` are ignored because its frame is
/// disabled (the owner of a modal dialog).
fn clicks_blocked(shared: &Arc<SharedState>, hwnd: u32) -> bool {
    let wm = shared.window_mgr.lock_or_recover();
    wm.frame_disabled(wm.find_frame_for_hwnd(hwnd))
}

/// The built-in control held down with mouse button 1, if any, and the
/// desktop point `(x, os2_y)` (bottom-left origin) relative to that control.
fn pressed_control_release(shared: &Arc<SharedState>, x: i32, os2_y: i32) -> Option<(u32, i32, i32)> {
    let wm = shared.window_mgr.lock_or_recover();
    let button = wm.pressed_control;
    if button == 0 { return None; }
    let frame = wm.find_frame_for_hwnd(button);
    let f = wm.get_window(frame)?;
    // Frame-relative, then relative to the frame's surface, then to the button.
    let (sx, sy) = (x - f.x - frame_border_px(f.frame_flags), os2_y - f.y);
    let (bx, by, _, _) = wm.get_abs_rect_in_frame(button);
    Some((button, sx - bx, sy - by))
}

/// Record what `event` says about the physical keyboard (keys by scan code)
/// and the pointer position (desktop coordinates, bottom-left origin) on a
/// desktop `desk_h` pixels tall.
fn track_physical_input(shared: &Arc<SharedState>, event: &Event, desk_h: i32) {
    let key = |sc: Option<Scancode>, down: bool| sc.map(|sc| (sdl_scancode_to_os2(sc), down));
    let (key, pointer) = match *event {
        Event::KeyDown { scancode, .. } => (key(scancode, true), None),
        Event::KeyUp { scancode, .. } => (key(scancode, false), None),
        Event::MouseMotion { x, y, .. } | Event::MouseButtonDown { x, y, .. }
        | Event::MouseButtonUp { x, y, .. } => (None, Some((x as i32, desk_h - 1 - y as i32))),
        _ => return,
    };
    let mut wm = shared.window_mgr.lock_or_recover();
    if let Some((sc, down)) = key.filter(|&(sc, _)| sc != 0) {
        wm.key_state.physical_key(sc, down);
    }
    if let Some(p) = pointer {
        wm.key_state.pointer = p;
    }
}

/// Queue pointer message `msg` for window `hwnd` (a frame stands for its
/// client) in the system input queue, behind the keys typed before it.
fn push_input(shared: &Arc<SharedState>, hwnd: u32, msg: u32, mp1: u32, mp2: u32) {
    shared.window_mgr.lock_or_recover().post_input(InputEvent::Window { hwnd, msg, mp1, mp2 });
}

/// Post an OS/2 message to the queue associated with `hwnd`.
///
/// Looks up the client window via `frame_to_client` first; falls back to
/// `hwnd` itself.  Notifies the condvar so `WinGetMsg` wakes up.
pub fn push_msg(shared: &SharedState, hwnd: u32, msg: u32, mp1: u32, mp2: u32) {
    let wm = shared.window_mgr.lock_or_recover();
    let target = wm.frame_to_client.get(&hwnd).copied().unwrap_or(hwnd);
    let hmq = wm.find_hmq_for_hwnd(target).or_else(|| wm.find_hmq_for_hwnd(hwnd));
    if let Some(hmq) = hmq
        && let Some(mq_arc) = wm.get_mq(hmq) {
            let mut mq = mq_arc.lock_or_recover();
            mq.messages.push_back(OS2Message {
                hwnd: target, msg, mp1, mp2, time: 0, x: 0, y: 0,
            });
            mq.cond.notify_one();
    }
}

/// Map SDL hardware Scancode → IBM PC Set-1 make code (OS/2 scan code field).
///
/// Extended keys (with E0 prefix on real hardware) share the same Set-1 make
/// code in OS/2; the distinction is handled via VK_ virtual-key codes instead.
pub fn sdl_scancode_to_os2(sc: Scancode) -> u8 {
    match sc {
        Scancode::Escape       => 0x01,
        Scancode::_1         => 0x02,
        Scancode::_2         => 0x03,
        Scancode::_3         => 0x04,
        Scancode::_4         => 0x05,
        Scancode::_5         => 0x06,
        Scancode::_6         => 0x07,
        Scancode::_7         => 0x08,
        Scancode::_8         => 0x09,
        Scancode::_9         => 0x0A,
        Scancode::_0         => 0x0B,
        Scancode::Minus        => 0x0C,
        Scancode::Equals       => 0x0D,
        Scancode::Backspace    => 0x0E,
        Scancode::Tab          => 0x0F,
        Scancode::Q            => 0x10,
        Scancode::W            => 0x11,
        Scancode::E            => 0x12,
        Scancode::R            => 0x13,
        Scancode::T            => 0x14,
        Scancode::Y            => 0x15,
        Scancode::U            => 0x16,
        Scancode::I            => 0x17,
        Scancode::O            => 0x18,
        Scancode::P            => 0x19,
        Scancode::LeftBracket  => 0x1A,
        Scancode::RightBracket => 0x1B,
        Scancode::Return       => 0x1C,
        Scancode::LCtrl        => 0x1D,
        Scancode::RCtrl        => 0x1D, // E0-1D
        Scancode::A            => 0x1E,
        Scancode::S            => 0x1F,
        Scancode::D            => 0x20,
        Scancode::F            => 0x21,
        Scancode::G            => 0x22,
        Scancode::H            => 0x23,
        Scancode::J            => 0x24,
        Scancode::K            => 0x25,
        Scancode::L            => 0x26,
        Scancode::Semicolon    => 0x27,
        Scancode::Apostrophe   => 0x28,
        Scancode::Grave        => 0x29,
        Scancode::LShift       => 0x2A,
        Scancode::Backslash    => 0x2B,
        Scancode::Z            => 0x2C,
        Scancode::X            => 0x2D,
        Scancode::C            => 0x2E,
        Scancode::V            => 0x2F,
        Scancode::B            => 0x30,
        Scancode::N            => 0x31,
        Scancode::M            => 0x32,
        Scancode::Comma        => 0x33,
        Scancode::Period       => 0x34,
        Scancode::Slash        => 0x35,
        Scancode::KpDivide     => 0x35, // E0-35
        Scancode::RShift       => 0x36,
        Scancode::KpMultiply   => 0x37,
        Scancode::PrintScreen  => 0x37, // E0-37
        Scancode::LAlt         => 0x38,
        Scancode::RAlt         => 0x38, // E0-38
        Scancode::Space        => 0x39,
        Scancode::CapsLock     => 0x3A,
        Scancode::F1           => 0x3B,
        Scancode::F2           => 0x3C,
        Scancode::F3           => 0x3D,
        Scancode::F4           => 0x3E,
        Scancode::F5           => 0x3F,
        Scancode::F6           => 0x40,
        Scancode::F7           => 0x41,
        Scancode::F8           => 0x42,
        Scancode::F9           => 0x43,
        Scancode::F10          => 0x44,
        Scancode::NumLockClear => 0x45,
        Scancode::ScrollLock   => 0x46,
        Scancode::Kp7          => 0x47,
        Scancode::Home         => 0x47, // E0-47
        Scancode::Kp8          => 0x48,
        Scancode::Up           => 0x48, // E0-48
        Scancode::Kp9          => 0x49,
        Scancode::PageUp       => 0x49, // E0-49
        Scancode::KpMinus      => 0x4A,
        Scancode::Kp4          => 0x4B,
        Scancode::Left         => 0x4B, // E0-4B
        Scancode::Kp5          => 0x4C,
        Scancode::Kp6          => 0x4D,
        Scancode::Right        => 0x4D, // E0-4D
        Scancode::KpPlus       => 0x4E,
        Scancode::Kp1          => 0x4F,
        Scancode::End          => 0x4F, // E0-4F
        Scancode::Kp2          => 0x50,
        Scancode::Down         => 0x50, // E0-50
        Scancode::Kp3          => 0x51,
        Scancode::PageDown     => 0x51, // E0-51
        Scancode::Kp0          => 0x52,
        Scancode::Insert       => 0x52, // E0-52
        Scancode::KpPeriod     => 0x53,
        Scancode::Delete       => 0x53, // E0-53
        Scancode::KpEnter      => 0x1C, // E0-1C
        Scancode::F11          => 0x57,
        Scancode::F12          => 0x58,
        _                      => 0x00,
    }
}

/// Map an SDL Keycode to the OS/2 VK_* virtual key code.
/// Returns 0 for keys that have no VK_ mapping (regular printable characters).
pub fn sdl_keycode_to_vk(kc: Keycode) -> u32 {
    match kc {
        Keycode::Backspace => VK_BACKSPACE,
        Keycode::Tab       => VK_TAB,
        Keycode::Return    => VK_NEWLINE,
        Keycode::KpEnter   => VK_ENTER,
        Keycode::Escape    => VK_ESC,
        Keycode::Space     => VK_SPACE,
        Keycode::PageUp    => VK_PAGEUP,
        Keycode::PageDown  => VK_PAGEDOWN,
        Keycode::End       => VK_END,
        Keycode::Home      => VK_HOME,
        Keycode::Left      => VK_LEFT,
        Keycode::Up        => VK_UP,
        Keycode::Right     => VK_RIGHT,
        Keycode::Down      => VK_DOWN,
        Keycode::Insert    => VK_INSERT,
        Keycode::Delete    => VK_DELETE,
        Keycode::ScrollLock => VK_SCRLLOCK,
        Keycode::NumLockClear => VK_NUMLOCK,
        Keycode::F1        => VK_F1,
        Keycode::F2        => VK_F2,
        Keycode::F3        => VK_F3,
        Keycode::F4        => VK_F4,
        Keycode::F5        => VK_F5,
        Keycode::F6        => VK_F6,
        Keycode::F7        => VK_F7,
        Keycode::F8        => VK_F8,
        Keycode::F9        => VK_F9,
        Keycode::F10       => VK_F10,
        Keycode::F11       => VK_F11,
        Keycode::F12       => VK_F12,
        // The shift keys are virtual keys of their own (WM_CHAR with
        // KC_VIRTUALKEY when pressed alone; WinGetKeyState).
        Keycode::LShift | Keycode::RShift => VK_SHIFT,
        Keycode::LCtrl | Keycode::RCtrl   => VK_CTRL,
        Keycode::LAlt      => VK_ALT,
        Keycode::RAlt      => VK_ALTGRAF,
        Keycode::CapsLock  => VK_CAPSLOCK,
        Keycode::Pause     => VK_PAUSE,
        Keycode::PrintScreen => VK_PRINTSCRN,
        _                  => 0,
    }
}

/// Build WM_CHAR MP1 and MP2 from SDL key event fields.
///
/// Packing follows CHRMSG in `<pmwin.h>`:
///
/// MP1 layout:
///   bits 15–0  : KC_* flags (`fs`)
///   bits 23–16 : repeat count (`cRepeat`, always 1)
///   bits 31–24 : hardware scan code (`scancode`)
///
/// MP2 layout:
///   bits 15–0  : character code (`chr`, 0 if none)
///   bits 31–16 : virtual key code (`vkey`, 0 if none)
///
/// A key event only carries a character when no `TextInput` event will: for
/// control characters (Enter, Tab, Backspace, Esc, Delete) and for Ctrl/Alt
/// chords, which report the key's unshifted character for accelerators.
/// Printable characters are delivered by `pm_input::char_message` instead, with
/// the keyboard layout and Shift state applied, so they never arrive twice.
fn build_wm_char(
    keycode:  Option<Keycode>,
    scancode: Option<Scancode>,
    keymod:   KeyMod,
    repeat:   bool,
    key_up:   bool,
) -> (u32, u32) {
    let sc  = scancode.map(sdl_scancode_to_os2).unwrap_or(0);
    let vk  = keycode.map(sdl_keycode_to_vk).unwrap_or(0);
    let chord = keymod.intersects(
        KeyMod::LCTRLMOD | KeyMod::RCTRLMOD | KeyMod::LALTMOD | KeyMod::RALTMOD);
    let ch  = keycode.map(sdl_keycode_to_char)
        .filter(|&c| chord || !is_text_input_char(c)).unwrap_or(0);

    let mut flags = KC_SCANCODE | kc_modifiers(keymod);
    if key_up  { flags |= KC_KEYUP;   }
    if repeat  { flags |= KC_PREVDOWN; }

    // PM reports Shift+Tab as its own virtual key.
    let vk = if vk == VK_TAB && flags & KC_SHIFT != 0 { VK_BACKTAB } else { vk };
    if vk != 0 { flags |= KC_VIRTUALKEY; }
    if ch != 0 { flags |= KC_CHAR;       }

    let mp1 = flags | (1 << WM_CHAR_REPEAT_SHIFT) | ((sc as u32) << WM_CHAR_SCANCODE_SHIFT);
    let mp2 = ch | (vk << WM_CHAR_VKEY_SHIFT);
    (mp1, mp2)
}

// ── Scripted sessions: gui_script::Input → SDL events ──────────────────────

/// SDL button-state bit for the left mouse button (`SDL_BUTTON_LMASK`), the
/// state a real mouse reports in motion events while dragging.
const SDL_LEFT_BUTTON_MASK: u32 = 1;

/// SDL key code and hardware scan code a US keyboard reports for a script key.
/// `None` for a `Key::Char` that is not an unshifted printable key or an
/// `F` number without a key; the script parser never produces either.
fn script_key_to_sdl(key: gui_script::Key) -> Option<(Keycode, Scancode)> {
    use gui_script::Key;
    Some(match key {
        Key::Enter     => (Keycode::Return,    Scancode::Return),
        Key::Tab       => (Keycode::Tab,       Scancode::Tab),
        Key::Esc       => (Keycode::Escape,    Scancode::Escape),
        Key::Backspace => (Keycode::Backspace, Scancode::Backspace),
        Key::Delete    => (Keycode::Delete,    Scancode::Delete),
        Key::Insert    => (Keycode::Insert,    Scancode::Insert),
        Key::Left      => (Keycode::Left,      Scancode::Left),
        Key::Right     => (Keycode::Right,     Scancode::Right),
        Key::Up        => (Keycode::Up,        Scancode::Up),
        Key::Down      => (Keycode::Down,      Scancode::Down),
        Key::Home      => (Keycode::Home,      Scancode::Home),
        Key::End       => (Keycode::End,       Scancode::End),
        Key::PageUp    => (Keycode::PageUp,    Scancode::PageUp),
        Key::PageDown  => (Keycode::PageDown,  Scancode::PageDown),
        Key::Space     => (Keycode::Space,     Scancode::Space),
        Key::Shift     => (Keycode::LShift,    Scancode::LShift),
        Key::Ctrl      => (Keycode::LCtrl,     Scancode::LCtrl),
        Key::Alt       => (Keycode::LAlt,      Scancode::LAlt),
        Key::F(n) => match n {
            1  => (Keycode::F1,  Scancode::F1),
            2  => (Keycode::F2,  Scancode::F2),
            3  => (Keycode::F3,  Scancode::F3),
            4  => (Keycode::F4,  Scancode::F4),
            5  => (Keycode::F5,  Scancode::F5),
            6  => (Keycode::F6,  Scancode::F6),
            7  => (Keycode::F7,  Scancode::F7),
            8  => (Keycode::F8,  Scancode::F8),
            9  => (Keycode::F9,  Scancode::F9),
            10 => (Keycode::F10, Scancode::F10),
            11 => (Keycode::F11, Scancode::F11),
            12 => (Keycode::F12, Scancode::F12),
            _  => return None,
        },
        Key::Char(c) => {
            let scancode = match c {
                'a' => Scancode::A, 'b' => Scancode::B, 'c' => Scancode::C, 'd' => Scancode::D,
                'e' => Scancode::E, 'f' => Scancode::F, 'g' => Scancode::G, 'h' => Scancode::H,
                'i' => Scancode::I, 'j' => Scancode::J, 'k' => Scancode::K, 'l' => Scancode::L,
                'm' => Scancode::M, 'n' => Scancode::N, 'o' => Scancode::O, 'p' => Scancode::P,
                'q' => Scancode::Q, 'r' => Scancode::R, 's' => Scancode::S, 't' => Scancode::T,
                'u' => Scancode::U, 'v' => Scancode::V, 'w' => Scancode::W, 'x' => Scancode::X,
                'y' => Scancode::Y, 'z' => Scancode::Z,
                '1' => Scancode::_1, '2' => Scancode::_2, '3' => Scancode::_3,
                '4' => Scancode::_4, '5' => Scancode::_5, '6' => Scancode::_6,
                '7' => Scancode::_7, '8' => Scancode::_8, '9' => Scancode::_9,
                '0' => Scancode::_0,
                '`' => Scancode::Grave,       '-' => Scancode::Minus,
                '=' => Scancode::Equals,      '[' => Scancode::LeftBracket,
                ']' => Scancode::RightBracket, '\\' => Scancode::Backslash,
                ';' => Scancode::Semicolon,   '\'' => Scancode::Apostrophe,
                ',' => Scancode::Comma,       '.' => Scancode::Period,
                '/' => Scancode::Slash,
                _ => return None,
            };
            // SDL key codes of printable keys are their unshifted ASCII codes.
            (Keycode::from_u32(c as u32)?, scancode)
        }
    })
}

/// SDL modifier state for script modifiers (left-hand keys, as
/// [`script_key_to_sdl`] presses).
fn script_mods_to_sdl(mods: gui_script::Mods) -> KeyMod {
    let mut keymod = KeyMod::NOMOD;
    if mods.shift { keymod |= KeyMod::LSHIFTMOD; }
    if mods.ctrl  { keymod |= KeyMod::LCTRLMOD; }
    if mods.alt   { keymod |= KeyMod::LALTMOD; }
    keymod
}

/// The SDL event a real mouse or keyboard produces for one scripted input, so
/// that scripted sessions exercise the same `handle_sdl_event` paths as a
/// user. `None` for `Dump`, which is a request to the renderer rather than
/// input, and for a key SDL has no code for.
pub(crate) fn script_input_to_event(input: &gui_script::Input) -> Option<Event> {
    use gui_script::{Button, Input};
    use sdl3::mouse::{MouseButton, MouseState};
    let sdl_button = |b: &Button| match b {
        Button::Left  => MouseButton::Left,
        Button::Right => MouseButton::Right,
    };
    let (timestamp, window_id, which) = (0, 0, 0);
    Some(match input {
        Input::MouseDown { button, x, y } => Event::MouseButtonDown {
            timestamp, window_id, which, mouse_btn: sdl_button(button), clicks: 1, x: *x as f32, y: *y as f32 },
        Input::MouseDoubleDown { button, x, y } => Event::MouseButtonDown {
            timestamp, window_id, which, mouse_btn: sdl_button(button), clicks: 2, x: *x as f32, y: *y as f32 },
        Input::MouseUp { button, x, y } => Event::MouseButtonUp {
            timestamp, window_id, which, mouse_btn: sdl_button(button), clicks: 1, x: *x as f32, y: *y as f32 },
        Input::MouseDrag { x, y, dx, dy } => Event::MouseMotion {
            timestamp, window_id, which,
            mousestate: MouseState::from_sdl_state(SDL_LEFT_BUTTON_MASK),
            x: *x as f32, y: *y as f32, xrel: *dx as f32, yrel: *dy as f32 },
        Input::KeyDown { key, mods } => {
            let (keycode, scancode) = script_key_to_sdl(*key)?;
            Event::KeyDown { timestamp, window_id, keycode: Some(keycode),
                             scancode: Some(scancode), keymod: script_mods_to_sdl(*mods),
                             repeat: false, which, raw: 0 }
        }
        Input::KeyUp { key, mods } => {
            let (keycode, scancode) = script_key_to_sdl(*key)?;
            Event::KeyUp { timestamp, window_id, keycode: Some(keycode),
                           scancode: Some(scancode), keymod: script_mods_to_sdl(*mods),
                           repeat: false, which, raw: 0 }
        }
        Input::TextInput(text) => Event::TextInput { timestamp, window_id, text: text.clone() },
        Input::Quit => Event::Quit { timestamp },
        Input::Dump(_) => return None,
    })
}

/// Whether a key's character is printable, i.e. SDL reports it through a
/// `TextInput` event rather than only as a key press.
fn is_text_input_char(ch: u32) -> bool {
    const SPACE: u32 = 0x20;
    const DEL: u32 = 0x7F;
    ch >= SPACE && ch != DEL
}

/// SDL modifier state → `KC_SHIFT` / `KC_CTRL` / `KC_ALT`.
fn kc_modifiers(keymod: KeyMod) -> u32 {
    let mut flags = 0;
    if keymod.intersects(KeyMod::LSHIFTMOD | KeyMod::RSHIFTMOD) { flags |= KC_SHIFT; }
    if keymod.intersects(KeyMod::LCTRLMOD  | KeyMod::RCTRLMOD)  { flags |= KC_CTRL;  }
    if keymod.intersects(KeyMod::LALTMOD   | KeyMod::RALTMOD)   { flags |= KC_ALT;   }
    flags
}

/// mp2 of a mouse button message (MSEMSG): `codeHitTest` (low word,
/// `HT_NORMAL` = 0) and `fsInp`, the `KC_*` modifier flags (high word).
fn mouse_mp2(key_mods: u32) -> u32 {
    key_mods << 16
}

/// Map an SDL Keycode to the unshifted OS/2 character code.
fn sdl_keycode_to_char(kc: Keycode) -> u32 {
    let name = kc.name();
    let b = name.as_bytes();
    if b.len() == 1 && b[0].is_ascii() {
        // Letter keys arrive as uppercase names; return lowercase (unshifted).
        let c = if b[0].is_ascii_uppercase() { b[0] + 32 } else { b[0] };
        return c as u32;
    }
    match kc {
        Keycode::Space     => b' ' as u32,
        Keycode::Return    => b'\r' as u32,
        Keycode::Tab       => b'\t' as u32,
        Keycode::Backspace => 8,
        Keycode::Escape    => 27,
        Keycode::Delete    => 127,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{sdl_keycode_to_char, sdl_keycode_to_vk, sdl_scancode_to_os2, build_wm_char,
        kc_modifiers, mouse_mp2};
    use super::{wait_timeout, is_wake_event, driver_can_wait, Event, Duration, Instant,
        FRAME_INTERVAL, IDLE_WAIT};

    #[test]
    fn headless_video_drivers_do_not_use_the_sdl_wait() {
        assert!(!driver_can_wait("dummy"));
        assert!(!driver_can_wait("OFFSCREEN"));
        for real in ["x11", "wayland", "kmsdrm", "windows", "cocoa"] {
            assert!(driver_can_wait(real), "{real}");
        }
    }
    use super::{route_button1, CHROME_BORDER_W,
                SharedState, MutexExt};
    use std::sync::Arc;
    use super::{draw_frame_chrome, FrameChrome, CHROME_TITLE_H, CHROME_BTN_W, CHROME_BTN_GAP,
                CHROME_BTN_MARGIN, CHROME_BTN_INSET_Y, CHROME_TITLE_ACTIVE, CHROME_TITLE_INACTIVE,
                CHROME_LIGHT_COLOR, CHROME_SHADOW_COLOR, CHROME_FACE_COLOR, CHROME_DARK_COLOR,
                FCF_TITLEBAR, FCF_SYSMENU, FCF_MINBUTTON, FCF_MAXBUTTON, FCF_SIZEBORDER};
    use crate::loader::constants::{FCF_BORDER, WS_MINIMIZED, WS_MAXIMIZED};
    use super::{display_flags, frame_chrome_hit, ChromeHit, CHROME_TEXT_COLOR};
    use super::{draw_menu_chrome, menu_bar_item_width, MenuItem,
                MENU_BG_COLOR, MENU_EDGE_COLOR, MENU_HILITE_COLOR,
                MENU_BAR_HEIGHT, MENU_BAR_ITEM_INSET, MENU_DROPDOWN_W, MENU_DROPDOWN_ITEM_H, MIS_SEPARATOR};
    use crate::loader::constants::{MIS_TEXT, MIS_SUBMENU};
    use sdl3::keyboard::{Keycode, Mod as KeyMod, Scancode};

    fn menu_item(text: &str, style: u16, children: Vec<MenuItem>) -> MenuItem {
        MenuItem { id: 0, style, attr: 0, text: text.to_string(), children, image: None }
    }

    #[test]
    fn dragged_edges_follow_the_pointer_and_the_others_stay() {
        use super::ResizeEdge::*;
        let orig = (100, 200, 400, 300); // x, y (bottom), cx, cy
        let free = |cx: i32, cy: i32| (cx.max(super::MIN_RESIZE), cy.max(super::MIN_RESIZE));
        let r = |edge, dx, dy| super::resized_rect(orig, edge, dx, dy, free);
        assert_eq!(r(E, 10, 99), (100, 200, 410, 300), "right edge: wider");
        assert_eq!(r(W, 10, 0), (110, 200, 390, 300), "left edge moved right: narrower");
        // SDL dy > 0 is downwards: the bottom edge goes down (y smaller,
        // taller), the top edge comes down (shorter, bottom unchanged).
        assert_eq!(r(S, 0, 20), (100, 180, 400, 320));
        assert_eq!(r(N, 0, 20), (100, 200, 400, 280));
        assert_eq!(r(N, 0, -20), (100, 200, 400, 320), "top edge up: taller");
        assert_eq!(r(SE, 5, 5), (100, 195, 405, 305));
        assert_eq!(r(NW, -5, -5), (95, 200, 405, 305));
        assert_eq!(r(E, -1000, 0).2, super::MIN_RESIZE, "never below the minimum");
        // Limits: the size stays inside them, the fixed edges stay put.
        let limits = |cx: i32, cy: i32| (cx.clamp(160, 420), cy.clamp(70, 330));
        assert_eq!(super::resized_rect(orig, SW, -100, 100, limits), (80, 170, 420, 330));
        assert_eq!(super::resized_rect(orig, NE, -1000, 1000, limits), (100, 200, 160, 70));
    }

    /// The bar is drawn at the given screen position; with no open item nothing
    /// is painted below it.
    #[test]
    fn idle_loop_waits_the_full_idle_period() {
        let now = Instant::now();
        let long_ago = now - Duration::from_secs(1);
        assert_eq!(wait_timeout(now, long_ago, false, None), IDLE_WAIT);
    }

    #[test]
    fn owed_composite_waits_only_until_the_next_frame() {
        let now = Instant::now();
        // Composited 10 ms ago: the next frame is 6 ms away.
        let t = wait_timeout(now, now - Duration::from_millis(10), true, None);
        assert_eq!(t, FRAME_INTERVAL - Duration::from_millis(10));
        // Frame already overdue: do not block at all.
        assert!(wait_timeout(now, now - FRAME_INTERVAL * 2, true, None).is_zero());
    }

    #[test]
    fn scripted_step_shortens_the_wait() {
        let now = Instant::now();
        let long_ago = now - Duration::from_secs(1);
        let step = now + Duration::from_millis(30);
        assert_eq!(wait_timeout(now, long_ago, false, Some(step)), Duration::from_millis(30));
        // A step already due wins over everything.
        assert!(wait_timeout(now, now, true, Some(now - Duration::from_millis(1))).is_zero());
    }

    #[test]
    fn drag_scroll_timer_runs_only_while_a_control_is_held() {
        use super::{drag_scroll_step, DRAG_SCROLL};
        let now = Instant::now();
        assert_eq!(drag_scroll_step(false, None, now), (None, false));
        // Armed when the hold is first seen, no tick yet.
        let (next, due) = drag_scroll_step(true, None, now);
        assert_eq!((next, due), (Some(now + DRAG_SCROLL), false));
        // Not yet due: unchanged; due: a tick and rearmed from now.
        assert_eq!(drag_scroll_step(true, next, now + DRAG_SCROLL / 2), (next, false));
        let later = now + DRAG_SCROLL * 3;
        assert_eq!(drag_scroll_step(true, next, later), (Some(later + DRAG_SCROLL), true));
        // Released: off.
        assert_eq!(drag_scroll_step(false, next, later), (None, false));
    }

    /// Input events are never mistaken for wake-ups. (A real `WakeEvent`
    /// needs an initialised SDL event subsystem to register its type; the
    /// GUI integration tests exercise that path.)
    #[test]
    fn input_events_are_not_wake_events() {
        assert!(!is_wake_event(&Event::Quit { timestamp: 0 }));
        assert!(!is_wake_event(&Event::TextInput { timestamp: 0, window_id: 0, text: "a".into() }));
    }

    #[test]
    fn draw_menu_chrome_closed_paints_only_the_bar() {
        const W: usize = 200;
        let mut buf = vec![0xFFFF_FFFFu32; W * 100];
        let items = vec![menu_item("~File", MIS_TEXT | MIS_SUBMENU,
                                   vec![menu_item("~About", MIS_TEXT, vec![])])];
        draw_menu_chrome(&mut buf, W, 0, 10, W as i32, &items, 0);
        let px = |x: usize, y: usize| buf[y * W + x];
        assert_eq!(px(150, 9), 0xFFFF_FFFF, "row above the bar untouched");
        assert_eq!(px(150, 10), MENU_BG_COLOR);
        assert_eq!(px(150, 10 + MENU_BAR_HEIGHT as usize - 1), MENU_EDGE_COLOR, "bottom rule");
        assert_eq!(px(10, 10 + MENU_BAR_HEIGHT as usize), 0xFFFF_FFFF, "no pull-down when closed");
    }

    /// An open item is highlighted and its pull-down appears directly below the
    /// bar, one row per child, with a rule for separators.
    #[test]
    fn draw_menu_chrome_open_paints_highlight_and_pulldown() {
        const W: usize = 200;
        let mut buf = vec![0xFFFF_FFFFu32; W * 100];
        let items = vec![menu_item("~File", MIS_TEXT | MIS_SUBMENU, vec![
            menu_item("~About", MIS_TEXT, vec![]),
            menu_item("", MIS_SEPARATOR, vec![]),
            menu_item("E~xit", MIS_TEXT, vec![]),
        ])];
        draw_menu_chrome(&mut buf, W, 0, 10, W as i32, &items, 1);
        let px = |x: i32, y: i32| buf[y as usize * W + x as usize];
        let bar_bot = 10 + MENU_BAR_HEIGHT as i32;
        // Highlight spans the item's width at the bar's top row.
        assert_eq!(px(MENU_BAR_ITEM_INSET, 10), MENU_HILITE_COLOR);
        assert_eq!(px(MENU_BAR_ITEM_INSET + menu_bar_item_width("File"), 10), MENU_BG_COLOR);
        // Pull-down: 3 rows, left edge is the border, interior is menu background.
        let drop_bot = bar_bot + 3 * MENU_DROPDOWN_ITEM_H;
        assert_eq!(px(MENU_BAR_ITEM_INSET, bar_bot + 1), MENU_EDGE_COLOR);
        assert_eq!(px(MENU_BAR_ITEM_INSET + MENU_DROPDOWN_W - 2, bar_bot + 1), MENU_BG_COLOR);
        assert_eq!(px(MENU_BAR_ITEM_INSET + MENU_DROPDOWN_W, bar_bot + 1), 0xFFFF_FFFF, "right of pull-down");
        assert_eq!(px(MENU_BAR_ITEM_INSET + 10, drop_bot), 0xFFFF_FFFF, "below pull-down");
        // Separator rule in the middle of the second row.
        let sep_y = bar_bot + MENU_DROPDOWN_ITEM_H + MENU_DROPDOWN_ITEM_H / 2;
        assert_eq!(px(MENU_BAR_ITEM_INSET + 60, sep_y), MENU_EDGE_COLOR);
    }

    /// Chrome for a 200×100 frame at (10, 10) in a 240×140 buffer.
    fn chrome_buf(frame_flags: u32, focused: bool) -> Vec<u32> {
        let mut buf = vec![0x00FF_00FFu32; 240 * 140];
        draw_frame_chrome(&mut buf, 240, &FrameChrome { colors: Default::default(),
            x: 10, top: 10, cx: 200, cy: 100, frame_flags, title: "Title", focused, cjk: super::CjkRegion::Auto, state: 0,
        });
        buf
    }

    /// The title bar is navy when focused and grey otherwise, and each button
    /// is a raised box (lit top-left corner, shaded bottom-right) exactly where
    /// the hit-test expects it.
    #[test]
    fn frame_chrome_draws_title_bar_and_raised_buttons() {
        let flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_MINBUTTON | FCF_MAXBUTTON;
        let buf = chrome_buf(flags, true);
        let px = |x: i32, y: i32| buf[y as usize * 240 + x as usize];
        // Title bar fill between the system menu and the title text start.
        assert_eq!(px(10 + 3 + CHROME_BTN_W + 1, 12), CHROME_TITLE_ACTIVE);
        assert_eq!(px(100, 10 + CHROME_TITLE_H), 0x00FF_00FF, "nothing drawn below the title bar");
        // Close, maximise, minimise from the right edge; system menu on the left.
        let close_x = 10 + 200 - CHROME_BTN_MARGIN - CHROME_BTN_W;
        let max_x = close_x - CHROME_BTN_W - CHROME_BTN_GAP;
        let min_x = max_x - CHROME_BTN_W - CHROME_BTN_GAP;
        let btn_y = 10 + CHROME_BTN_INSET_Y;
        let btn_bot = 10 + CHROME_TITLE_H - CHROME_BTN_INSET_Y - 1;
        for x in [close_x, max_x, min_x, 10 + 3] {
            assert_eq!(px(x, btn_y), CHROME_LIGHT_COLOR, "lit corner at x={x}");
            assert_eq!(px(x + CHROME_BTN_W - 1, btn_bot - 1), CHROME_SHADOW_COLOR, "shaded corner at x={x}");
            assert_eq!(px(x + 2, btn_y + 2), CHROME_FACE_COLOR, "face at x={x}");
        }
        // Each button carries a dark pictogram.
        for x in [close_x, max_x, min_x] {
            let dark = (0..CHROME_BTN_W).flat_map(|dx| (btn_y..btn_bot).map(move |y| (x + dx, y)))
                .filter(|&(x, y)| px(x, y) == CHROME_DARK_COLOR).count();
            assert!(dark >= 4, "button at x={x} has no pictogram");
        }
        // Inactive frames use the inactive title colour.
        let buf = chrome_buf(flags, false);
        assert_eq!(buf[12 * 240 + (10 + 3 + CHROME_BTN_W + 1) as usize], CHROME_TITLE_INACTIVE);
    }

    /// Without FCF_MINBUTTON / FCF_MAXBUTTON only the close button is drawn,
    /// and a long title is clipped before it.
    #[test]
    fn frame_chrome_omits_absent_buttons_and_clips_title() {
        let mut buf = vec![0u32; 240 * 140];
        draw_frame_chrome(&mut buf, 240, &FrameChrome { colors: Default::default(),
            x: 10, top: 10, cx: 200, cy: 100, frame_flags: FCF_TITLEBAR,
            title: "A very long window title that cannot possibly fit in the bar", focused: true, cjk: super::CjkRegion::Auto, state: 0,
        });
        let px = |x: i32, y: i32| buf[y as usize * 240 + x as usize];
        let close_x = 10 + 200 - CHROME_BTN_MARGIN - CHROME_BTN_W;
        let max_x = close_x - CHROME_BTN_W - CHROME_BTN_GAP;
        assert_eq!(px(close_x, 10 + CHROME_BTN_INSET_Y), CHROME_LIGHT_COLOR);
        assert_eq!(px(max_x, 10 + CHROME_BTN_INSET_Y), CHROME_TITLE_ACTIVE, "no maximise button");
        // No title pixels in the 4-px gap before the close button or on it.
        for y in 10..10 + CHROME_TITLE_H - 1 {
            for x in close_x - 4..close_x {
                assert_eq!(px(x, y), CHROME_TITLE_ACTIVE, "title text ran into the buttons at ({x},{y})");
            }
        }
    }

    /// A maximized frame's maximize button shows the restore pictogram; the
    /// other buttons stay as they are.
    #[test]
    fn frame_chrome_maximized_shows_a_restore_button() {
        let flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_MINBUTTON | FCF_MAXBUTTON;
        let draw = |state: u32| {
            let mut buf = vec![0u32; 240 * 140];
            draw_frame_chrome(&mut buf, 240, &FrameChrome { colors: Default::default(),
                x: 10, top: 10, cx: 200, cy: 100, frame_flags: flags, title: "T", focused: true, state, cjk: super::CjkRegion::Auto,
            });
            buf
        };
        let (normal, maximized) = (draw(0), draw(WS_MAXIMIZED));
        let close_x = 10 + 200 - CHROME_BTN_MARGIN - CHROME_BTN_W;
        let max_x = close_x - CHROME_BTN_W - CHROME_BTN_GAP;
        let button = |buf: &[u32], x0: i32| -> Vec<u32> {
            (10..10 + CHROME_TITLE_H).flat_map(|y| (x0..x0 + CHROME_BTN_W).map(move |x| (x, y)))
                .map(|(x, y)| buf[y as usize * 240 + x as usize]).collect()
        };
        assert_ne!(button(&normal, max_x), button(&maximized, max_x), "restore pictogram");
        assert_eq!(button(&normal, close_x), button(&maximized, close_x));
        assert_eq!(button(&normal, max_x - CHROME_BTN_W - CHROME_BTN_GAP),
                   button(&maximized, max_x - CHROME_BTN_W - CHROME_BTN_GAP));
    }

    /// FCF_HIDEBUTTON puts a hide button where the minimize button would be
    /// (it wins when both are asked for), with its own pictogram, and a
    /// click on it is a hide-button click.
    #[test]
    fn hide_button_takes_the_minimize_slot() {
        use crate::loader::constants::{FCF_HIDEBUTTON, FCF_HIDEMAX, FCF_MINMAX};
        let draw = |flags: u32| {
            let mut buf = vec![0u32; 240 * 140];
            draw_frame_chrome(&mut buf, 240, &FrameChrome { colors: Default::default(),
                x: 10, top: 10, cx: 200, cy: 100, frame_flags: FCF_TITLEBAR | flags, title: "T",
                focused: true, state: 0, cjk: super::CjkRegion::Auto,
            });
            buf
        };
        let close_x = 10 + 200 - CHROME_BTN_MARGIN - CHROME_BTN_W;
        let slot_x = close_x - 2 * (CHROME_BTN_W + CHROME_BTN_GAP);
        let button = |buf: &[u32]| -> Vec<u32> {
            (10..10 + CHROME_TITLE_H).flat_map(|y| (slot_x..slot_x + CHROME_BTN_W).map(move |x| (x, y)))
                .map(|(x, y)| buf[y as usize * 240 + x as usize]).collect()
        };
        let (hide, min, both) = (draw(FCF_HIDEMAX), draw(FCF_MINMAX), draw(FCF_MINMAX | FCF_HIDEBUTTON));
        assert_eq!(hide[(10 + CHROME_BTN_INSET_Y) as usize * 240 + slot_x as usize], CHROME_LIGHT_COLOR, "a button");
        assert!(button(&hide).contains(&CHROME_DARK_COLOR), "with a pictogram");
        assert_ne!(button(&hide), button(&min), "not the minimize pictogram");
        assert_eq!(button(&both), button(&hide));
        // Hit-testing (frame at the origin, 200 × 100; title bar at the top).
        let x = slot_x - 10 + 2;
        assert_eq!(frame_chrome_hit(0, 0, 200, 100, FCF_TITLEBAR | FCF_HIDEMAX, 0, x, 95), ChromeHit::Hide);
        assert_eq!(frame_chrome_hit(0, 0, 200, 100, FCF_TITLEBAR | FCF_MINMAX, 0, x, 95), ChromeHit::Minimize);
        // Without a maximize button the hide button sits next to close.
        let x = close_x - 10 - CHROME_BTN_GAP - 2;
        assert_eq!(frame_chrome_hit(0, 0, 200, 100, FCF_TITLEBAR | FCF_HIDEBUTTON, 0, x, 95), ChromeHit::Hide);
    }

    /// A minimized frame is drawn as a bordered title bar with its title and
    /// no buttons, and every click on it is a title-bar click.
    #[test]
    fn minimized_frame_is_a_title_bar_without_buttons() {
        use crate::loader::constants::{ICON_AREA_GAP, MINIMIZED_CX, MINIMIZED_CY};
        let flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINBUTTON | FCF_MAXBUTTON;
        let shown = display_flags(flags, WS_MINIMIZED);
        assert_eq!(shown, FCF_TITLEBAR | FCF_BORDER);
        assert_eq!(display_flags(flags, WS_MAXIMIZED), flags);
        let mut buf = vec![0u32; 240 * 40];
        draw_frame_chrome(&mut buf, 240, &FrameChrome { colors: Default::default(),
            x: 0, top: 0, cx: MINIMIZED_CX, cy: MINIMIZED_CY, frame_flags: shown,
            title: "Iconic", focused: true, state: WS_MINIMIZED, cjk: super::CjkRegion::Auto,
        });
        let px = |x: i32, y: i32| buf[y as usize * 240 + x as usize];
        let close_x = MINIMIZED_CX - CHROME_BTN_MARGIN - CHROME_BTN_W;
        assert_eq!(px(close_x + 1, CHROME_BTN_INSET_Y + 1), CHROME_TITLE_ACTIVE, "no close button");
        assert_eq!(px(5, CHROME_BTN_INSET_Y + 1), CHROME_TITLE_ACTIVE, "no system-menu button");
        assert!((0..MINIMIZED_CX).any(|x| (0..CHROME_TITLE_H).any(|y| px(x, y) == CHROME_TEXT_COLOR)),
                "the title is drawn");
        // Hit-testing: anywhere on the icon is the title bar.
        let (wx, wy) = (ICON_AREA_GAP, ICON_AREA_GAP);
        let hit = |x, y| frame_chrome_hit(wx, wy, MINIMIZED_CX, MINIMIZED_CY, flags, WS_MINIMIZED, x, y);
        assert_eq!(hit(wx + MINIMIZED_CX - 5, wy + MINIMIZED_CY - 5), ChromeHit::TitleBar, "where close was");
        assert_eq!(hit(wx + 5, wy), ChromeHit::TitleBar, "the bottom border");
        assert_eq!(hit(wx + MINIMIZED_CX, wy), ChromeHit::None);
        // A normal frame keeps its buttons; without a title bar it has none.
        let close = frame_chrome_hit(0, 0, 200, 100, flags, 0, 200 - CHROME_BTN_MARGIN - 4, 95);
        assert_eq!(close, ChromeHit::Close);
        assert_eq!(frame_chrome_hit(0, 0, 200, 100, FCF_SIZEBORDER, 0, 195, 95), ChromeHit::None);
        assert_eq!(frame_chrome_hit(0, 0, 200, 100, flags, WS_MAXIMIZED, 10, 95), ChromeHit::SysMenu);
    }

    /// A sizing border is a raised bevel; a plain border is a uniform outline;
    /// no border flag draws nothing at the frame edge.
    #[test]
    fn frame_chrome_border_styles() {
        let edge = |flags: u32| {
            let buf = chrome_buf(flags, true);
            // top-left outer, bottom-right outer, inner ring (left side, mid-height)
            (buf[10 * 240 + 10], buf[109 * 240 + 209], buf[60 * 240 + 11])
        };
        assert_eq!(edge(FCF_SIZEBORDER), (CHROME_LIGHT_COLOR, CHROME_SHADOW_COLOR, CHROME_FACE_COLOR));
        assert_eq!(edge(FCF_BORDER), (CHROME_SHADOW_COLOR, CHROME_SHADOW_COLOR, CHROME_FACE_COLOR));
        assert_eq!(edge(0), (0x00FF_00FF, 0x00FF_00FF, 0x00FF_00FF));
    }

    #[test]
    fn test_sdl_keycode_to_char_printable() {
        // SDL printable keycodes equal their ASCII value directly
        assert_eq!(sdl_keycode_to_char(Keycode::A), b'a' as u32);
        assert_eq!(sdl_keycode_to_char(Keycode::Space), b' ' as u32);
        assert_eq!(sdl_keycode_to_char(Keycode::_1), b'1' as u32);
    }

    #[test]
    fn test_sdl_keycode_to_char_special_returns_zero() {
        // Non-printable keys (arrows, F-keys) have SDLK_SCANCODE_MASK set
        assert_eq!(sdl_keycode_to_char(Keycode::Up), 0);
        assert_eq!(sdl_keycode_to_char(Keycode::F1), 0);
        assert_eq!(sdl_keycode_to_char(Keycode::Return), b'\r' as u32);
    }

    #[test]
    fn scancode_alpha_keys_are_set1() {
        assert_eq!(sdl_scancode_to_os2(Scancode::A), 0x1E);
        assert_eq!(sdl_scancode_to_os2(Scancode::Z), 0x2C);
        assert_eq!(sdl_scancode_to_os2(Scancode::Q), 0x10);
    }

    #[test]
    fn scancode_function_keys() {
        assert_eq!(sdl_scancode_to_os2(Scancode::F1),  0x3B);
        assert_eq!(sdl_scancode_to_os2(Scancode::F10), 0x44);
        assert_eq!(sdl_scancode_to_os2(Scancode::F11), 0x57);
        assert_eq!(sdl_scancode_to_os2(Scancode::F12), 0x58);
    }

    #[test]
    fn scancode_extended_nav_keys() {
        assert_eq!(sdl_scancode_to_os2(Scancode::Up),       0x48);
        assert_eq!(sdl_scancode_to_os2(Scancode::Down),     0x50);
        assert_eq!(sdl_scancode_to_os2(Scancode::Left),     0x4B);
        assert_eq!(sdl_scancode_to_os2(Scancode::Right),    0x4D);
        assert_eq!(sdl_scancode_to_os2(Scancode::Home),     0x47);
        assert_eq!(sdl_scancode_to_os2(Scancode::End),      0x4F);
        assert_eq!(sdl_scancode_to_os2(Scancode::PageUp),   0x49);
        assert_eq!(sdl_scancode_to_os2(Scancode::PageDown), 0x51);
        assert_eq!(sdl_scancode_to_os2(Scancode::Insert),   0x52);
        assert_eq!(sdl_scancode_to_os2(Scancode::Delete),   0x53);
    }

    #[test]
    fn vk_mapping_for_special_keys() {
        use crate::loader::*;
        assert_eq!(sdl_keycode_to_vk(Keycode::F1),        VK_F1);
        assert_eq!(sdl_keycode_to_vk(Keycode::F12),       VK_F12);
        assert_eq!(sdl_keycode_to_vk(Keycode::Up),        VK_UP);
        assert_eq!(sdl_keycode_to_vk(Keycode::Return),    VK_NEWLINE);
        assert_eq!(sdl_keycode_to_vk(Keycode::KpEnter),   VK_ENTER);
        assert_eq!(sdl_keycode_to_vk(Keycode::Escape),    VK_ESC);
        assert_eq!(sdl_keycode_to_vk(Keycode::Backspace), VK_BACKSPACE);
    }

    /// The shift keys have virtual keys of their own (WinGetKeyState), so
    /// their WM_CHAR carries KC_VIRTUALKEY.
    #[test]
    fn vk_mapping_for_shift_keys() {
        use crate::loader::constants::*;
        for (kc, vk) in [(Keycode::LShift, VK_SHIFT), (Keycode::RShift, VK_SHIFT), (Keycode::LCtrl, VK_CTRL),
                         (Keycode::RCtrl, VK_CTRL), (Keycode::LAlt, VK_ALT), (Keycode::RAlt, VK_ALTGRAF),
                         (Keycode::CapsLock, VK_CAPSLOCK), (Keycode::Pause, VK_PAUSE),
                         (Keycode::PrintScreen, VK_PRINTSCRN)] {
            assert_eq!(sdl_keycode_to_vk(kc), vk, "{kc:?}");
        }
        let (mp1, mp2) = build_wm_char(Some(Keycode::LShift), Some(Scancode::LShift), KeyMod::LSHIFTMOD, false, false);
        assert!(mp1 & KC_VIRTUALKEY != 0);
        assert_eq!(mp2 >> WM_CHAR_VKEY_SHIFT, VK_SHIFT);
        assert_eq!(mp1 >> WM_CHAR_SCANCODE_SHIFT, 0x2A);
    }

    #[test]
    fn vk_mapping_returns_zero_for_printable() {
        assert_eq!(sdl_keycode_to_vk(Keycode::A), 0);
        assert_eq!(sdl_keycode_to_vk(Keycode::_5), 0);
    }

    #[test]
    fn build_wm_char_regular_key() {
        use crate::loader::*;
        // 'A' key down, no modifiers
        let (mp1, mp2) = build_wm_char(
            Some(Keycode::A), Some(Scancode::A),
            KeyMod::NOMOD, false, false,
        );
        let flags = mp1 & 0xFFFF;
        let sc = mp1 >> WM_CHAR_SCANCODE_SHIFT;
        assert_eq!((mp1 >> WM_CHAR_REPEAT_SHIFT) & 0xFF, 1, "repeat count is 1");
        let ch = mp2 & 0xFFFF;
        let vk = mp2 >> 16;
        assert!(flags & KC_CHAR == 0,     "printable characters come from TextInput");
        assert!(flags & KC_SCANCODE != 0, "KC_SCANCODE always set");
        assert!(flags & KC_VIRTUALKEY == 0, "KC_VIRTUALKEY not set for plain char");
        assert_eq!(sc, 0x1E, "scan code for A is 0x1E");
        assert_eq!(ch, 0);
        assert_eq!(vk, 0);
    }

    #[test]
    fn build_wm_char_function_key() {
        use crate::loader::*;
        let (mp1, mp2) = build_wm_char(
            Some(Keycode::F1), Some(Scancode::F1),
            KeyMod::NOMOD, false, false,
        );
        let flags = mp1 & 0xFFFF;
        let sc = mp1 >> WM_CHAR_SCANCODE_SHIFT;
        let vk = mp2 >> 16;
        assert!(flags & KC_VIRTUALKEY != 0, "KC_VIRTUALKEY set for F1");
        assert!(flags & KC_SCANCODE != 0,   "KC_SCANCODE always set");
        assert!(flags & KC_CHAR == 0,       "KC_CHAR not set for F1");
        assert_eq!(sc, 0x3B, "scan code for F1");
        assert_eq!(vk, VK_F1);
    }

    #[test]
    fn every_second_press_of_a_multi_click_is_a_double_click() {
        use sdl3::mouse::MouseButton;
        use crate::loader::*;
        assert_eq!(super::button_down_msg(MouseButton::Left, 1), Some(WM_BUTTON1DOWN));
        assert_eq!(super::button_down_msg(MouseButton::Left, 2), Some(WM_BUTTON1DBLCLK));
        assert_eq!(super::button_down_msg(MouseButton::Left, 3), Some(WM_BUTTON1DOWN));
        assert_eq!(super::button_down_msg(MouseButton::Left, 4), Some(WM_BUTTON1DBLCLK));
        assert_eq!(super::button_down_msg(MouseButton::Right, 2), Some(WM_BUTTON2DBLCLK));
        assert_eq!(super::button_down_msg(MouseButton::Middle, 1), Some(WM_BUTTON3DOWN));
        assert_eq!(super::button_down_msg(MouseButton::X1, 1), None);
    }

    #[test]
    fn mouse_messages_report_the_modifier_keys_in_fsinp() {
        use crate::loader::*;
        assert_eq!(kc_modifiers(KeyMod::NOMOD), 0);
        assert_eq!(kc_modifiers(KeyMod::RSHIFTMOD | KeyMod::LCTRLMOD), KC_SHIFT | KC_CTRL);
        assert_eq!(kc_modifiers(KeyMod::RALTMOD | KeyMod::CAPSMOD), KC_ALT);
        // MSEMSG: codeHitTest (HT_NORMAL = 0) in the low word, fsInp high.
        assert_eq!(mouse_mp2(KC_SHIFT), KC_SHIFT << 16);
        assert_eq!(mouse_mp2(0), 0);
    }

    #[test]
    fn build_wm_char_shift_modifier() {
        use crate::loader::*;
        let (mp1, mp2) = build_wm_char(
            Some(Keycode::A), Some(Scancode::A),
            KeyMod::LSHIFTMOD, false, false,
        );
        let flags = mp1 & 0xFFFF;
        assert!(flags & KC_SHIFT != 0, "KC_SHIFT set when shift held");
        assert_eq!(mp2 & 0xFFFF, 0, "the shifted character comes from TextInput");
    }

    #[test]
    fn build_wm_char_keyup_flag() {
        use crate::loader::*;
        let (mp1, _) = build_wm_char(
            Some(Keycode::A), Some(Scancode::A),
            KeyMod::NOMOD, false, true,
        );
        let flags = mp1 & 0xFFFF;
        assert!(flags & KC_KEYUP != 0, "KC_KEYUP set on key release");
    }

    /// What `build_wm_char` produces is what the built-in edit controls and
    /// `WinTranslateAccel` decode (CHRMSG packing). Printable keys carry no
    /// character on key-down; editing keys and control characters do.
    #[test]
    fn build_wm_char_round_trips_through_the_edit_key_decoder() {
        use crate::loader::pm_edit::{key_from_wm_char, EditKey};
        let key = |kc, sc, km, up| {
            let (mp1, mp2) = build_wm_char(Some(kc), Some(sc), km, false, up);
            key_from_wm_char(mp1, mp2, 437)
        };
        for km in [KeyMod::NOMOD, KeyMod::RSHIFTMOD, KeyMod::CAPSMOD] {
            assert_eq!(key(Keycode::A, Scancode::A, km, false), None);
            assert_eq!(key(Keycode::_1, Scancode::_1, km, false), None);
            assert_eq!(key(Keycode::Space, Scancode::Space, km, false), None);
        }
        assert_eq!(key(Keycode::Backspace, Scancode::Backspace, KeyMod::NOMOD, false), Some((EditKey::Backspace, false)));
        assert_eq!(key(Keycode::Delete, Scancode::Delete, KeyMod::NOMOD, false), Some((EditKey::Delete, false)));
        assert_eq!(key(Keycode::Return, Scancode::Return, KeyMod::NOMOD, false), Some((EditKey::Enter, false)));
        assert_eq!(key(Keycode::Home, Scancode::Home, KeyMod::LSHIFTMOD, false), Some((EditKey::Home, false)));
        assert_eq!(key(Keycode::Left, Scancode::Left, KeyMod::NOMOD, true), Some((EditKey::Left, true)));
        assert_eq!(key(Keycode::Tab, Scancode::Tab, KeyMod::NOMOD, false), None);
        assert_eq!(key(Keycode::C, Scancode::C, KeyMod::LCTRLMOD, false), None);
    }

    #[test]
    fn build_wm_char_keeps_characters_that_text_input_will_not_send() {
        use crate::loader::*;
        let chr = |kc, sc, km| {
            let (mp1, mp2) = build_wm_char(Some(kc), Some(sc), km, false, false);
            ((mp1 & KC_CHAR != 0), mp2 & 0xFFFF)
        };
        // Control characters.
        assert_eq!(chr(Keycode::Return, Scancode::Return, KeyMod::NOMOD), (true, b'\r' as u32));
        assert_eq!(chr(Keycode::Tab, Scancode::Tab, KeyMod::NOMOD), (true, b'\t' as u32));
        assert_eq!(chr(Keycode::Backspace, Scancode::Backspace, KeyMod::NOMOD), (true, 8));
        assert_eq!(chr(Keycode::Escape, Scancode::Escape, KeyMod::NOMOD), (true, 27));
        // Ctrl / Alt chords report the key's character for accelerators.
        assert_eq!(chr(Keycode::C, Scancode::C, KeyMod::LCTRLMOD), (true, b'c' as u32));
        assert_eq!(chr(Keycode::F, Scancode::F, KeyMod::RALTMOD), (true, b'f' as u32));
        // Printable keys, shifted or not, do not.
        assert_eq!(chr(Keycode::C, Scancode::C, KeyMod::NOMOD), (false, 0));
        assert_eq!(chr(Keycode::Slash, Scancode::Slash, KeyMod::LSHIFTMOD), (false, 0));
        assert_eq!(chr(Keycode::Space, Scancode::Space, KeyMod::NOMOD), (false, 0));
    }

    #[test]
    fn build_wm_char_reports_shift_tab_as_backtab() {
        use crate::loader::*;
        let vk = |km| build_wm_char(Some(Keycode::Tab), Some(Scancode::Tab), km, false, false).1 >> WM_CHAR_VKEY_SHIFT;
        assert_eq!(vk(KeyMod::NOMOD), VK_TAB);
        assert_eq!(vk(KeyMod::LSHIFTMOD), VK_BACKTAB);
        assert_eq!(vk(KeyMod::RSHIFTMOD), VK_BACKTAB);
    }

    /// A std window with one entry field; returns `(shared, frame, client, entry)`.
    fn routed_window(frame_flags: u32) -> (Arc<SharedState>, u32, u32, u32) {
        let loader = crate::loader::Loader::new_mock();
        let shared = loader.shared.clone();
        let mut wm = shared.window_mgr.lock_or_recover();
        let frame = wm.create_window("App".into(), 0, 0);
        let client = wm.create_window("App".into(), frame, 0);
        wm.frame_to_client.insert(frame, client);
        wm.get_window_mut(frame).unwrap().frame_flags = frame_flags;
        let entry = wm.create_window("WC_ENTRYFIELD".into(), client, 0);
        let w = wm.get_window_mut(entry).unwrap();
        (w.x, w.y, w.cx, w.cy, w.visible) = (10, 200, 200, 25, true);
        drop(wm);
        (shared, frame, client, entry)
    }

    #[test]
    fn button1_is_routed_to_the_control_under_the_pointer() {
        let (shared, frame, _client, entry) = routed_window(0);
        // On the control: control-relative coordinates, focus untouched here
        // (the control takes the focus when it handles the message).
        assert_eq!(route_button1(&shared, frame, 15, 210, true), ((entry, 5, 10), 0));
        assert_eq!(route_button1(&shared, frame, 15, 210, false), ((entry, 5, 10), 0));
        // Off the control: the frame gets it unchanged.
        assert_eq!(route_button1(&shared, frame, 300, 100, true), ((frame, 300, 100), 0));
    }

    #[test]
    fn clicks_on_a_disabled_frame_are_blocked() {
        let (shared, frame, client, entry) = routed_window(0);
        assert!(!super::clicks_blocked(&shared, frame));
        shared.window_mgr.lock_or_recover().get_window_mut(frame).unwrap().style |= crate::loader::WS_DISABLED;
        for h in [frame, client, entry] {
            assert!(super::clicks_blocked(&shared, h), "{h:#x}");
        }
    }

    #[test]
    fn a_held_button_gets_the_release_relative_to_itself() {
        let (shared, frame, _client, entry) = routed_window(FCF_SIZEBORDER);
        assert_eq!(super::pressed_control_release(&shared, 0, 0), None, "no button held");
        {
            let mut wm = shared.window_mgr.lock_or_recover();
            let f = wm.get_window_mut(frame).unwrap();
            (f.x, f.y) = (100, 50);
            wm.pressed_control = entry;
        }
        // Desktop point → frame (100, 50) → surface (inside the border) → control (10, 200).
        let (x, y) = (100 + CHROME_BORDER_W + 10 + 3, 50 + 200 + 4);
        assert_eq!(super::pressed_control_release(&shared, x, y), Some((entry, 3, 4)));
        // Far away the position is outside the button, so it is not clicked.
        let (_, bx, by) = super::pressed_control_release(&shared, 0, 0).unwrap();
        assert!(bx < 0 && by < 0);
    }

    #[test]
    fn button1_accounts_for_the_frame_border() {
        let (shared, frame, _client, entry) = routed_window(FCF_SIZEBORDER);
        // Client coordinates: the control's left edge is at 10, whatever the
        // border (the client lies inside it).
        assert_eq!(route_button1(&shared, frame, 10, 200, true), ((entry, 0, 0), 0));
        assert_eq!(route_button1(&shared, frame, 9, 200, true), ((frame, 9, 200), 0));
        // A desktop point inside the border is client x 0.
        let (x, y) = {
            let mut wm = shared.window_mgr.lock_or_recover();
            let f = wm.get_window_mut(frame).unwrap();
            (f.x, f.y) = (100, 50);
            super::pointer_local(&wm, frame, 100 + CHROME_BORDER_W + 10, 50 + 200)
        };
        assert_eq!((x, y), (10, 200));
        assert_eq!(route_button1(&shared, frame, x, y, true), ((entry, 0, 0), 0));
    }

    #[test]
    fn click_outside_controls_ends_control_focus() {
        let (shared, frame, _client, entry) = routed_window(0);
        shared.window_mgr.lock_or_recover().focus_hwnd = entry;
        // Releasing the button elsewhere does not change the focus.
        assert_eq!(route_button1(&shared, frame, 300, 100, false).1, 0);
        assert_eq!(shared.window_mgr.lock_or_recover().focus_hwnd, entry);
        // Pressing it does, and reports the control to repaint.
        assert_eq!(route_button1(&shared, frame, 300, 100, true), ((frame, 300, 100), entry));
        assert_eq!(shared.window_mgr.lock_or_recover().focus_hwnd, 0);
        assert_eq!(route_button1(&shared, frame, 300, 100, true).1, 0);
        // A click in another frame leaves this frame's focus control alone.
        shared.window_mgr.lock_or_recover().focus_hwnd = entry;
        let other = {
            let mut wm = shared.window_mgr.lock_or_recover();
            let f = wm.create_window("App".into(), 0, 0);
            let c = wm.create_window("App".into(), f, 0);
            wm.frame_to_client.insert(f, c);
            f
        };
        assert_eq!(route_button1(&shared, other, 300, 100, true), ((other, 300, 100), 0));
        assert_eq!(shared.window_mgr.lock_or_recover().focus_hwnd, entry);
    }
}

#[cfg(test)]
mod script_tests {
    use super::*;
    use super::gui_script::{Input, Key, Mods};
    use sdl3::mouse::MouseButton;

    #[test]
    fn host_input_is_told_apart_from_window_events() {
        for line in ["0 click 5 6", "0 rclick 5 6", "0 drag 1 2 3 4", "0 key ctrl+a", "0 text hi"] {
            for event in script_events(line) {
                assert!(is_host_input(&event), "{line}: {event:?} is input");
            }
        }
        use sdl3::event::WindowEvent;
        let window = |win_event| Event::Window { timestamp: 0, window_id: 0, win_event };
        assert!(!is_host_input(&Event::Quit { timestamp: 0 }));
        assert!(!is_host_input(&window(WindowEvent::Exposed)));
        assert!(!is_host_input(&window(WindowEvent::Resized(10, 10))));
        assert!(!is_host_input(&Event::ClipboardUpdate { timestamp: 0 }));
    }

    /// The SDL events one script line turns into.
    fn script_events(line: &str) -> Vec<Event> {
        gui_script::parse(line).expect("script line parses").iter()
            .flat_map(|step| step.action.expand(step.at_ms))
            .filter_map(|(_, input)| script_input_to_event(&input))
            .collect()
    }

    /// The `WM_CHAR` parameters `handle_sdl_event` posts for a key event:
    /// `(KC_* flags, scan code, character, virtual key)`.
    fn wm_char(event: &Event) -> (u32, u32, u32, u32) {
        let (mp1, mp2) = match event {
            Event::KeyDown { keycode, scancode, keymod, repeat, .. } =>
                build_wm_char(*keycode, *scancode, *keymod, *repeat, false),
            Event::KeyUp { keycode, scancode, keymod, .. } =>
                build_wm_char(*keycode, *scancode, *keymod, false, true),
            other => panic!("not a key event: {other:?}"),
        };
        // CHRMSG layout: flags in the low word of mp1, scan code in its top
        // byte; character in the low word of mp2, virtual key in the high word.
        (mp1 & 0xFFFF, mp1 >> crate::loader::constants::WM_CHAR_SCANCODE_SHIFT,
         mp2 & 0xFFFF, mp2 >> crate::loader::constants::WM_CHAR_VKEY_SHIFT)
    }

    /// Every key a script can name has an SDL key code and scan code that
    /// agree with each other and that the WM_CHAR tables know.
    #[test]
    fn every_script_key_maps_to_a_consistent_sdl_key() {
        let named = gui_script::KEY_NAMES.iter().map(|&(_, key)| key);
        let function = (1..=gui_script::MAX_FUNCTION_KEY).map(Key::F);
        let printable = (' '..='~').map(|c| gui_script::key_for_char(c).expect("printable").0);
        let modifiers = [Key::Shift, Key::Ctrl, Key::Alt];
        for key in named.chain(function).chain(printable).chain(modifiers) {
            let (keycode, scancode) = script_key_to_sdl(key)
                .unwrap_or_else(|| panic!("{key:?} has no SDL key"));
            let no_mods = sdl3::sys::keycode::SDL_Keymod(KeyMod::NOMOD.bits());
            assert_eq!(Keycode::from_scancode(scancode, no_mods, false), Some(keycode),
                "{key:?}: scan code and key code name different keys");
            assert_ne!(sdl_scancode_to_os2(scancode), 0, "{key:?} has no OS/2 scan code");
            match key {
                Key::Char(c) => assert_eq!(sdl_keycode_to_char(keycode), c as u32, "{key:?}"),
                Key::Shift | Key::Ctrl | Key::Alt => {}
                Key::F(_) | Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End
                | Key::PageUp | Key::PageDown | Key::Insert =>
                    assert_ne!(sdl_keycode_to_vk(keycode), 0, "{key:?} has no VK_ code"),
                Key::Enter | Key::Tab | Key::Esc | Key::Backspace | Key::Delete | Key::Space => {
                    assert_ne!(sdl_keycode_to_vk(keycode), 0, "{key:?} has no VK_ code");
                    assert_ne!(sdl_keycode_to_char(keycode), 0, "{key:?} has no character");
                }
            }
        }
        // Keys the parser cannot produce are refused rather than guessed.
        for key in [Key::F(0), Key::F(13), Key::Char('A'), Key::Char('!'), Key::Char('é'), Key::Char(' ')] {
            assert_eq!(script_key_to_sdl(key), None, "{key:?}");
        }
    }

    #[test]
    fn script_mods_map_to_left_hand_sdl_modifiers() {
        assert_eq!(script_mods_to_sdl(Mods::default()), KeyMod::NOMOD);
        assert_eq!(script_mods_to_sdl(Mods { shift: true, ctrl: false, alt: false }), KeyMod::LSHIFTMOD);
        assert_eq!(script_mods_to_sdl(Mods { shift: true, ctrl: true, alt: true }),
                   KeyMod::LSHIFTMOD | KeyMod::LCTRLMOD | KeyMod::LALTMOD);
    }

    /// `key esc` / `key enter` reach the application as the WM_CHAR pair a
    /// real key press and release produces.
    #[test]
    fn plain_key_becomes_wm_char_down_and_up() {
        let events = script_events("0 key esc");
        assert_eq!(events.len(), 2);
        let (flags, scan, ch, vk) = wm_char(&events[0]);
        assert_eq!((scan, ch, vk), (0x01, 27, VK_ESC));
        assert_eq!(flags, KC_SCANCODE | KC_VIRTUALKEY | KC_CHAR);
        let (flags, scan, ch, vk) = wm_char(&events[1]);
        assert_eq!((scan, ch, vk), (0x01, 27, VK_ESC));
        assert_eq!(flags, KC_SCANCODE | KC_VIRTUALKEY | KC_CHAR | KC_KEYUP);

        let (_, scan, ch, vk) = wm_char(&script_events("0 key enter")[0]);
        assert_eq!((scan, ch, vk), (0x1C, b'\r' as u32, VK_NEWLINE));
        // A printable key carries no character on its key press: the character
        // arrives separately through SDL text input (see the `text` action).
        let (flags, _, ch, vk) = wm_char(&script_events("0 key a")[0]);
        assert_eq!((flags, ch, vk), (KC_SCANCODE, 0, 0));
    }

    /// `key alt+f4`: Alt goes down first and up last, and F4 carries KC_ALT.
    #[test]
    fn modified_key_holds_the_modifier_around_the_key() {
        let events = script_events("0 key alt+f4");
        let keys: Vec<(bool, Option<Keycode>, KeyMod)> = events.iter().map(|e| match e {
            Event::KeyDown { keycode, keymod, .. } => (true, *keycode, *keymod),
            Event::KeyUp { keycode, keymod, .. } => (false, *keycode, *keymod),
            other => panic!("unexpected {other:?}"),
        }).collect();
        assert_eq!(keys, vec![
            (true, Some(Keycode::LAlt), KeyMod::LALTMOD),
            (true, Some(Keycode::F4), KeyMod::LALTMOD),
            (false, Some(Keycode::F4), KeyMod::LALTMOD),
            (false, Some(Keycode::LAlt), KeyMod::NOMOD),
        ]);
        let (flags, scan, ch, vk) = wm_char(&events[1]);
        assert_eq!((scan, ch, vk), (0x3E, 0, VK_F4));
        assert_eq!(flags, KC_SCANCODE | KC_VIRTUALKEY | KC_ALT);
        assert_eq!(wm_char(&events[2]).0, KC_SCANCODE | KC_VIRTUALKEY | KC_ALT | KC_KEYUP);

        let (flags, _, _, vk) = wm_char(&script_events("0 key shift+tab")[1]);
        assert_eq!(vk, VK_BACKTAB, "PM reports Shift+Tab as VK_BACKTAB");
        assert_ne!(flags & KC_SHIFT, 0);
        let (flags, _, ch, _) = wm_char(&script_events("0 key ctrl+c")[1]);
        assert_eq!(ch, b'c' as u32);
        assert_ne!(flags & KC_CTRL, 0);
    }

    /// `text` produces what a keyboard does: key down, the typed text, key
    /// up for ASCII; committed text alone for anything else.
    #[test]
    fn text_becomes_keystrokes_and_text_input_events() {
        let events = script_events("0 text aé日");
        assert_eq!(events.len(), 5);
        assert!(matches!(&events[0], Event::KeyDown { keycode: Some(k), scancode: Some(Scancode::A),
            keymod, repeat: false, .. } if *k == Keycode::A && *keymod == KeyMod::NOMOD));
        assert!(matches!(&events[1], Event::TextInput { text, .. } if text == "a"));
        assert!(matches!(&events[2], Event::KeyUp { keycode: Some(k), .. } if *k == Keycode::A));
        assert!(matches!(&events[3], Event::TextInput { text, .. } if text == "é"));
        assert!(matches!(&events[4], Event::TextInput { text, .. } if text == "日"));

        // A shifted character presses Shift around the key that carries it.
        let events = script_events("0 text ?");
        assert_eq!(events.len(), 5);
        assert!(matches!(&events[0], Event::KeyDown { keycode: Some(k), .. } if *k == Keycode::LShift));
        assert!(matches!(&events[1], Event::KeyDown { scancode: Some(Scancode::Slash), keymod, .. }
            if *keymod == KeyMod::LSHIFTMOD));
        assert!(matches!(&events[2], Event::TextInput { text, .. } if text == "?"));
        assert!(matches!(&events[4], Event::KeyUp { keycode: Some(k), keymod, .. }
            if *k == Keycode::LShift && *keymod == KeyMod::NOMOD));
    }

    /// `drag` is a left press, one motion event per step with the button
    /// held, and a release: the sequence `DragState`/`ResizeState` consume.
    #[test]
    fn drag_becomes_press_motions_and_release() {
        let events = script_events("0 drag 100 40 300 240");
        assert_eq!(events.len(), gui_script::DRAG_MOVES as usize + 2);
        assert!(matches!(events[0],
            Event::MouseButtonDown { mouse_btn: MouseButton::Left, x: 100.0, y: 40.0, .. }));
        let mut last = (100.0, 40.0);
        for event in &events[1..events.len() - 1] {
            let Event::MouseMotion { mousestate, x, y, xrel, yrel, .. } = event else {
                panic!("expected motion, got {event:?}");
            };
            assert!(mousestate.left() && !mousestate.right(), "left button held while dragging");
            assert_eq!((last.0 + xrel, last.1 + yrel), (*x, *y), "relative motion matches position");
            assert!(*x > last.0 && *y > last.1, "motions advance towards the target");
            last = (*x, *y);
        }
        assert_eq!(last, (300.0, 240.0), "the last motion reaches the target");
        assert!(matches!(events[events.len() - 1],
            Event::MouseButtonUp { mouse_btn: MouseButton::Left, x: 300.0, y: 240.0, .. }));
    }

    #[test]
    fn clicks_use_the_scripted_button_and_dump_is_not_an_event() {
        let events = script_events("0 rclick 7 9");
        assert!(matches!(events[..], [
            Event::MouseButtonDown { mouse_btn: MouseButton::Right, clicks: 1, x: 7.0, y: 9.0, .. },
            Event::MouseButtonUp { mouse_btn: MouseButton::Right, clicks: 1, x: 7.0, y: 9.0, .. },
        ]));
        let events = script_events("0 click 7 9");
        assert!(matches!(events[..], [
            Event::MouseButtonDown { mouse_btn: MouseButton::Left, .. },
            Event::MouseButtonUp { mouse_btn: MouseButton::Left, .. },
        ]));
        assert!(matches!(script_events("0 quit")[..], [Event::Quit { .. }]));
        assert!(script_input_to_event(&Input::Dump("x.ppm".into())).is_none());
        assert!(script_events("0 dump x.ppm").is_empty());
    }
}

#[cfg(test)]
mod compositor_tests {
    use super::*;

    #[test]
    fn place_frame_lays_out_chrome_and_client() {
        let flags = FCF_TITLEBAR | FCF_SIZEBORDER;
        // 200×150 frame at OS/2 (10, 20) on a 600-px-tall desktop: top = 430.
        // The client reaches the frame's bottom edge (the bottom border is
        // drawn over it), so a covering frame buffer is 150 - title - menu tall.
        let fb_h = (150 - CHROME_TITLE_H - MENU_BAR_HEIGHT as i32) as u32;
        let p = place_frame(7, 10, 20, 200, 150, flags, true, Some((30, 48)), (196, fb_h), 600);
        let mh = MENU_BAR_HEIGHT as i32;
        assert_eq!(p.frame, Rect::at(10, 430, 200, 150));
        assert_eq!(p.title, Rect::at(10, 430, 200, CHROME_TITLE_H));
        assert_eq!(p.menu_bar, Rect::at(12, 450, 196, mh));
        assert_eq!(p.client, Rect::new(12, 450 + mh, 208, 580));
        assert_eq!(p.pulldown, Rect::at(42, 450 + mh, MENU_DROPDOWN_W, 48));
        assert_eq!(p.extent(), p.frame.union(&p.pulldown));
        assert!(p.opaque, "a frame buffer the size of the client covers it");

        let small = place_frame(7, 10, 20, 200, 150, flags, true, None, (100, fb_h), 600);
        assert!(!small.opaque, "a narrower frame buffer leaves desktop showing");
        assert!(small.pulldown.is_empty());
        assert_eq!(small.extent(), small.frame);

        let bare = place_frame(7, 0, 0, 50, 40, 0, false, None, (50, 40), 40);
        assert!(bare.title.is_empty() && bare.menu_bar.is_empty());
        assert_eq!(bare.client, Rect::at(0, 0, 50, 40));
    }

    fn items() -> Vec<MenuItem> {
        let child = MenuItem { id: 11, style: 0, attr: 0, text: "~Open".into(), children: vec![], image: None };
        vec![MenuItem { id: 1, style: 0, attr: 0, text: "~File".into(), children: vec![child.clone(), child], image: None }]
    }

    #[test]
    fn chrome_cache_reports_only_real_changes() {
        let mut c = ChromeCache::new();
        let items = items();
        assert!(c.sync("App", 200, FCF_TITLEBAR, 0, true, Some((&items, 0)), 196), "first sync renders");
        c.render();
        assert!(!c.sync("App", 200, FCF_TITLEBAR, 0, true, Some((&items, 0)), 196), "unchanged");
        for changed in [
            c.sync("App 2", 200, FCF_TITLEBAR, 0, true, Some((&items, 0)), 196),
            c.sync("App 2", 220, FCF_TITLEBAR, 0, true, Some((&items, 0)), 216),
            c.sync("App 2", 220, FCF_TITLEBAR, 0, false, Some((&items, 0)), 216),
            c.sync("App 2", 220, FCF_TITLEBAR, 0, false, Some((&items, 1)), 216),
            c.sync("App 2", 220, FCF_TITLEBAR, 0, false, None, 216),
        ] {
            assert!(changed);
            c.render();
        }
        assert!(!c.sync("App 2", 220, FCF_TITLEBAR, 0, false, None, 216));
    }

    /// The title and menus are drawn in the CJK face of the frame's queue
    /// codepage: changing it re-renders the chrome with other glyphs (直 and
    /// 骨 have different forms in Japanese and Simplified Chinese).
    #[test]
    fn chrome_cache_follows_the_cjk_face() {
        let mut c = ChromeCache::new();
        let items = vec![MenuItem { id: 1, style: 0, attr: 0, text: "直骨".into(), children: vec![], image: None }];
        assert!(!c.set_cjk(CjkRegion::Auto), "unchanged");
        c.set_cjk(CjkRegion::Japanese);
        c.sync("直骨", 200, FCF_TITLEBAR, 0, true, Some((&items, 0)), 196);
        c.render();
        let (title, bar) = (c.title_px.clone(), c.menu.as_ref().unwrap().bar_px.clone());
        assert!(c.set_cjk(CjkRegion::SimplifiedChinese), "a new face");
        assert!(!c.sync("直骨", 200, FCF_TITLEBAR, 0, true, Some((&items, 0)), 196) || c.stale);
        c.render();
        assert_ne!(c.title_px, title, "title redrawn in the Chinese face");
        assert_ne!(c.menu.as_ref().unwrap().bar_px, bar, "menu bar redrawn in the Chinese face");
    }

    /// WinSetSysColors: the title bar follows the system colour table.
    #[test]
    fn chrome_cache_follows_the_system_title_colours() {
        use crate::loader::constants::{SYSCLR_ACTIVETITLE, SYSCLR_INACTIVETITLE};
        use crate::loader::pm_syscolor::SysColorTable;
        let table = SysColorTable::default();
        assert_eq!(TitleColors::of(&table), TitleColors::default(), "defaults match the drawing");
        let mut c = ChromeCache::new();
        c.sync("App", 200, FCF_TITLEBAR, 0, true, None, 196);
        assert!(!c.set_colors(TitleColors::of(&table)), "unchanged");
        c.render();
        assert_eq!(c.title_px[200 * 5 + 100], CHROME_TITLE_ACTIVE);
        table.set(SYSCLR_ACTIVETITLE, 0x00_80_00_00);
        table.set(SYSCLR_INACTIVETITLE, 0x00_00_80_00);
        assert!(c.set_colors(TitleColors::of(&table)), "changed: redraw");
        c.render();
        assert_eq!(c.title_px[200 * 5 + 100], 0x00_80_00_00, "active title in the new colour");
        c.sync("App", 200, FCF_TITLEBAR, 0, false, None, 196);
        c.render();
        assert_eq!(c.title_px[200 * 5 + 100], 0x00_00_80_00, "inactive title in the new colour");
    }

    #[test]
    fn chrome_cache_renders_title_menu_and_open_pulldown() {
        let mut c = ChromeCache::new();
        let items = items();
        c.sync("App", 200, FCF_TITLEBAR, 0, true, Some((&items, 1)), 196);
        c.render();
        assert_eq!(c.title_px.len(), 200 * CHROME_TITLE_H as usize);
        assert_eq!(c.title_px[200 * 5 + 100], CHROME_TITLE_ACTIVE, "title bar background");
        let m = c.menu.as_ref().unwrap();
        assert_eq!(m.bar_px.len(), 196 * MENU_BAR_HEIGHT as usize);
        let pd = m.pulldown.as_ref().expect("item 1 is open");
        assert_eq!((pd.dx, pd.h), (MENU_BAR_ITEM_INSET, 2 * MENU_DROPDOWN_ITEM_H));
        assert_eq!(pd.pixels.len(), (MENU_DROPDOWN_W * pd.h) as usize);
        // Closing the menu drops the pull-down.
        c.sync("App", 200, FCF_TITLEBAR, 0, true, Some((&items, 0)), 196);
        c.render();
        assert!(c.menu.as_ref().unwrap().pulldown.is_none());
    }

    #[test]
    fn dirty_windows_map_to_the_frame_that_draws_them() {
        let mut wm = WindowManager::new();
        let frame = wm.create_window("Frame".into(), 0, 0);
        let client = wm.create_window("Client".into(), frame, 0);
        wm.frame_to_client.insert(frame, client);
        let button = wm.create_window("WC_BUTTON".into(), client, 0);
        let loose = wm.create_window("Object".into(), 0, 0);
        wm.z_order.push(frame);
        assert_eq!(owning_frame(&wm, frame), Some(frame));
        assert_eq!(owning_frame(&wm, client), Some(frame));
        assert_eq!(owning_frame(&wm, button), Some(frame));
        assert_eq!(owning_frame(&wm, loose), None, "not drawn by the compositor");
        assert_eq!(owning_frame(&wm, 0xDEAD), None, "unknown window");
    }

    #[test]
    fn placed_frame_is_drawn_only_inside_the_clip() {
        let (dw, dh) = (64usize, 48usize);
        let mut buf = vec![DESKTOP_BG; dw * dh];
        let mut fb = Surface::new(40, 20);
        fb.buffer.fill(0x00_1234);
        let mut chrome = ChromeCache::new();
        chrome.sync("T", 44, FCF_TITLEBAR | crate::loader::constants::FCF_BORDER, 0, true, None, 40);
        chrome.render();
        let p = place_frame(1, 4, 4, 44, 42, FCF_TITLEBAR | crate::loader::constants::FCF_BORDER, false, None, (40, 20), dh as i32);
        let clip = Rect::new(0, 0, 30, 30);
        draw_placed_frame(&mut buf, dw, &p, &fb, &chrome, clip);
        let px = |x: usize, y: usize| buf[y * dw + x];
        // Frame top is 48 - 4 - 42 = 2; (22, 5) is title bar right of the text.
        assert_eq!(px(22, 5), CHROME_TITLE_ACTIVE, "title bar inside the clip");
        assert_eq!(px(10, 25), 0x00_1234, "client inside the clip");
        assert_eq!(px(40, 25), DESKTOP_BG, "client outside the clip is untouched");
        assert_eq!(px(10, 35), DESKTOP_BG, "below the clip is untouched");
    }
}
