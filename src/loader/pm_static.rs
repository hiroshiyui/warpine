// SPDX-License-Identifier: GPL-3.0-only

//! Static controls (`WC_STATIC`): the `SS_*` styles of `<pmwin.h>`.
//!
//! * `SS_TEXT`: the window text, erased to the background first (text drawn
//!   again over itself would grow bolder), aligned by the `DT_*` bits of
//!   the style — `DT_LEFT` / `DT_CENTER` / `DT_RIGHT`, `DT_TOP` /
//!   `DT_VCENTER` / `DT_BOTTOM` — and drawn grey with `DT_HALFTONE`. A `~`
//!   marks a mnemonic, as in dialogs. One line (`DT_WORDBREAK` is not
//!   applied yet).
//! * `SS_GROUPBOX`: a frame round the control with the text in its top
//!   edge; the inside is left alone (the controls in it show through).
//! * `SS_FGNDRECT` / `SS_BKGNDRECT` / `SS_HALFTONERECT` fill the control
//!   with the foreground colour, the background colour or a 50 % halftone of
//!   the foreground over the background; `SS_FGNDFRAME` / `SS_BKGNDFRAME` /
//!   `SS_HALFTONEFRAME` draw only a one-pel frame in them. The foreground is
//!   `PP_FOREGROUNDCOLOR`, else `SYSCLR_WINDOWFRAME`; the background is
//!   `PP_BACKGROUNDCOLOR`, else what is behind the control (the dialog face
//!   or a client's white). Unverified: PM's exact default colours.
//! * `SS_ICON`: the text "#id" names an `RT_POINTER` resource of the
//!   executable; `SS_SYSICON`: "#id" names a system pointer (`SPTR_ICON*`
//!   have pictures). `SS_BITMAP`: "#id" names an `RT_BITMAP` resource. The
//!   picture is drawn at the control's top-left corner, clipped to the
//!   control; with `SS_AUTOSIZE`, or when the control has no size, the
//!   control first takes the picture's size, its top-left corner staying
//!   (unverified: where PM anchors it, and whether it centres an icon).
//!   Resources come from the executable even for a dialog loaded from a DLL.

use super::constants::*;
use super::managers::EXE_RESOURCE_MODULE;
use super::mutex_ext::MutexExt;
use super::pm_presparam::{PP_BACKGROUNDCOLOR, PP_FOREGROUNDCOLOR};
use super::{control_label, Loader, Painter, CONTROL_BORDER_COLOR, CONTROL_LINE_H, CONTROL_TEXT_INSET};
use crate::gui::surface::{AreaFill, DrawOp, Mix, Pattern};

// ── <pmwin.h> ────────────────────────────────────────────────────────────────

pub const SS_ICON: u32 = 0x0003;
pub const SS_BITMAP: u32 = 0x0004;
pub const SS_FGNDRECT: u32 = 0x0005;
pub const SS_HALFTONERECT: u32 = 0x0006;
pub const SS_BKGNDRECT: u32 = 0x0007;
pub const SS_FGNDFRAME: u32 = 0x0008;
pub const SS_HALFTONEFRAME: u32 = 0x0009;
pub const SS_BKGNDFRAME: u32 = 0x000A;
pub const SS_SYSICON: u32 = 0x000B;
pub const SS_AUTOSIZE: u32 = 0x0040;

/// The bits of the style that hold the `SS_*` kind (the kinds run from 1 to
/// `SS_SYSICON`; `SS_AUTOSIZE` and the `DT_*` bits lie above).
const SS_KIND_MASK: u32 = 0x003F;
/// Grey of `DT_HALFTONE` text.
const HALFTONE_TEXT_COLOR: u32 = 0x0080_8080;
/// The 50 % checkerboard of the halftone styles.
const HALFTONE_ROWS: [u8; 8] = [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55];
/// Largest picture a static control draws (pels per side).
const MAX_PICTURE_DIM: u32 = 1024;

/// What a static control of `style` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticKind {
    Text,
    GroupBox,
    Icon,
    SysIcon,
    Bitmap,
    /// A filled rectangle or a frame: which colour, whether only the frame.
    Rect { color: RectColor, frame: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RectColor {
    Foreground,
    Background,
    Halftone,
}

/// The kind of a static control's `style`; an unknown kind shows its text.
pub fn static_kind(style: u32) -> StaticKind {
    use RectColor::*;
    match style & SS_KIND_MASK {
        SS_GROUPBOX => StaticKind::GroupBox,
        SS_ICON => StaticKind::Icon,
        SS_SYSICON => StaticKind::SysIcon,
        SS_BITMAP => StaticKind::Bitmap,
        SS_FGNDRECT => StaticKind::Rect { color: Foreground, frame: false },
        SS_BKGNDRECT => StaticKind::Rect { color: Background, frame: false },
        SS_HALFTONERECT => StaticKind::Rect { color: Halftone, frame: false },
        SS_FGNDFRAME => StaticKind::Rect { color: Foreground, frame: true },
        SS_BKGNDFRAME => StaticKind::Rect { color: Background, frame: true },
        SS_HALFTONEFRAME => StaticKind::Rect { color: Halftone, frame: true },
        _ => StaticKind::Text,
    }
}

/// The resource id of a static control's text "#123"; `None` otherwise.
pub fn resource_id(text: &str) -> Option<u16> {
    text.trim().strip_prefix('#')?.parse().ok()
}

/// Where a `w`-pel-wide text line goes in a control `(x, y, cx, cy)` for
/// the `DT_*` bits of `style`: the bottom-left of its `line_h`-high line
/// box. `DT_TOP | DT_LEFT` (0) is the top-left corner.
pub fn text_origin(style: u32, (x, y, cx, cy): (i32, i32, i32, i32), w: i32, line_h: i32) -> (i32, i32) {
    let tx = if style & DT_RIGHT != 0 {
        x + (cx - w).max(0)
    } else if style & DT_CENTER != 0 {
        x + (cx - w).max(0) / 2
    } else {
        x
    };
    let ty = if style & DT_BOTTOM != 0 {
        y
    } else if style & DT_VCENTER != 0 {
        y + (cy - line_h).max(0) / 2
    } else {
        y + (cy - line_h).max(0)
    };
    (tx, ty)
}

/// A picture's pels, top row first (`None` = transparent).
struct Picture {
    w: u32,
    h: u32,
    pels: Vec<Option<u32>>,
}

impl Loader {
    /// Paint static control `hwnd` at `rect` (frame surface coordinates).
    pub(super) fn paint_static(&self, p: &Painter, hwnd: u32, rect: (i32, i32, i32, i32), text: &str) {
        let style = self.shared.window_mgr.lock_or_recover().get_window(hwnd).map_or(0, |w| w.style);
        let (x, y, cx, cy) = rect;
        let bg = self.control_background(hwnd);
        match static_kind(style) {
            StaticKind::Text => {
                p.fill((x, y, x + cx, y + cy), bg);
                let (shown, _) = super::pm_dialog::mnemonic_label(text);
                let w = crate::gui::font::text_width(crate::gui::font::fit_text(&shown, cx, p.font()), p.font());
                let (tx, ty) = text_origin(style, rect, w, CONTROL_LINE_H);
                if style & DT_HALFTONE != 0 {
                    let grey = Painter { fg: HALFTONE_TEXT_COLOR, ..Painter::new(self, p.frame, hwnd) };
                    control_label(&grey, tx, ty, text, cx);
                } else {
                    control_label(p, tx, ty, text, cx);
                }
            }
            StaticKind::GroupBox => self.paint_group_box(p, hwnd, rect, text, bg),
            StaticKind::Rect { color, frame } => {
                let fg = self.static_foreground(hwnd);
                let back = self.shared.window_mgr.lock_or_recover()
                    .pres_color(hwnd, PP_BACKGROUNDCOLOR, false).unwrap_or(bg);
                let pattern = match color {
                    RectColor::Foreground => Pattern::solid(fg),
                    RectColor::Background => Pattern::solid(back),
                    RectColor::Halftone => Pattern { rows: HALFTONE_ROWS, fg, bg: back, origin: (x, y) },
                };
                let (x2, y2) = (x + cx - 1, y + cy - 1);
                if cx <= 0 || cy <= 0 { return; }
                let fill = |r: (i32, i32, i32, i32)| p.op(DrawOp::Shape {
                    points: vec![(r.0, r.1), (r.2, r.1), (r.2, r.3), (r.0, r.3)], closed: true,
                    fill: Some(AreaFill { pattern, mix: Mix::Overpaint, back_mix: Some(Mix::Overpaint) }),
                    stroke: None,
                });
                if frame {
                    for r in [(x, y, x2, y), (x, y2, x2, y2), (x, y, x, y2), (x2, y, x2, y2)] {
                        fill(r);
                    }
                } else {
                    fill((x, y, x2, y2));
                }
            }
            kind @ (StaticKind::Icon | StaticKind::SysIcon | StaticKind::Bitmap) => {
                let picture = resource_id(text).and_then(|id| self.static_picture(kind, id));
                let Some(pic) = picture else {
                    p.fill((x, y, x + cx, y + cy), bg);
                    return;
                };
                let rect = self.autosize_static(hwnd, style, rect, (pic.w as i32, pic.h as i32));
                let (x, y, cx, cy) = rect;
                p.fill((x, y, x + cx - 1, y + cy - 1), bg);
                // Top-left corner, clipped to the control.
                let (w, h) = (pic.w.min(cx.max(0) as u32), pic.h.min(cy.max(0) as u32));
                if w == 0 || h == 0 { return; }
                let src: Vec<Option<u32>> = (0..h).rev()
                    .flat_map(|row| pic.pels[(row * pic.w) as usize..][..w as usize].iter().copied())
                    .collect();
                p.op(DrawOp::Blit {
                    x, y: y + cy - h as i32, w, h, src: Some(src),
                    rop: ROP_SRCCOPY as u8, pattern: Pattern::solid(0),
                });
            }
        }
    }

    /// The foreground of a rectangle or frame style.
    fn static_foreground(&self, hwnd: u32) -> u32 {
        let wm = self.shared.window_mgr.lock_or_recover();
        wm.pres_color(hwnd, PP_FOREGROUNDCOLOR, false)
            .or_else(|| wm.sys_colors.get(SYSCLR_WINDOWFRAME))
            .unwrap_or(0)
    }

    /// `SS_GROUPBOX`: a frame whose top edge runs through the middle of the
    /// text line, the text over it at the left.
    fn paint_group_box(&self, p: &Painter, hwnd: u32, (x, y, cx, cy): (i32, i32, i32, i32), text: &str, bg: u32) {
        if cx <= 0 || cy <= 0 { return; }
        let frame_color = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.pres_color(hwnd, super::pm_presparam::PP_BORDERCOLOR, false).unwrap_or(CONTROL_BORDER_COLOR)
        };
        let line_y = y + (cy - CONTROL_LINE_H).max(0);
        let top = (line_y + CONTROL_LINE_H / 2).min(y + cy - 1);
        let (x2, y2) = (x + cx - 1, y);
        // The text band is erased; the inside is left to what is in it.
        p.fill((x, line_y, x2, y + cy - 1), bg);
        for (a, b) in [((x, y2), (x2, y2)), ((x, y2), (x, top)), ((x2, y2), (x2, top)), ((x, top), (x2, top))] {
            p.op(DrawOp::Line { x1: a.0, y1: a.1, x2: b.0, y2: b.1, color: frame_color, mix: Mix::Overpaint,
                                style: crate::gui::surface::SOLID_LINE });
        }
        let (shown, _) = super::pm_dialog::mnemonic_label(text);
        if shown.is_empty() { return; }
        let max_w = cx - 2 * CONTROL_TEXT_INSET;
        let w = crate::gui::font::text_width(crate::gui::font::fit_text(&shown, max_w, p.font()), p.font());
        let tx = x + CONTROL_TEXT_INSET;
        p.fill((tx - 1, line_y, tx + w, y + cy - 1), bg);
        control_label(p, tx, line_y, text, max_w);
    }

    /// With `SS_AUTOSIZE`, or when the control has no size, it takes the
    /// picture's size `(w, h)` with its top-left corner in place. Returns
    /// the rectangle to paint.
    fn autosize_static(&self, hwnd: u32, style: u32, (x, y, cx, cy): (i32, i32, i32, i32), (w, h): (i32, i32))
        -> (i32, i32, i32, i32) {
        if style & SS_AUTOSIZE == 0 && cx > 0 && cy > 0 || (cx, cy) == (w, h) {
            return (x, y, cx, cy);
        }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if let Some(win) = wm.get_window_mut(hwnd) {
            win.y += win.cy - h;
            (win.cx, win.cy) = (w, h);
        }
        (x, y + cy - h, w, h)
    }

    /// The picture of an icon / system icon / bitmap static control.
    fn static_picture(&self, kind: StaticKind, id: u16) -> Option<Picture> {
        match kind {
            StaticKind::Icon => {
                let res = self.shared.resource_mgr.lock_or_recover()
                    .find(EXE_RESOURCE_MODULE, crate::lx::header::RT_POINTER, id)?;
                let bytes = self.guest_read_bytes(res.0, res.1 as usize)?;
                let img = super::pm_pointer::decode_pointer_resource(&bytes)?;
                let pels = img.rgba.as_chunks::<4>().0.iter()
                    .map(|c| (c[3] != 0).then(|| (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32))
                    .collect();
                Some(Picture { w: img.width, h: img.height, pels })
            }
            StaticKind::SysIcon => {
                let pels = super::pm_msgbox::icon_pels(id as u32)?;
                let n = super::pm_msgbox::MSGBOX_ICON_SIZE as u32;
                // icon_pels is top row first, as a Picture.
                Some(Picture { w: n, h: n, pels })
            }
            StaticKind::Bitmap => {
                let res = self.shared.resource_mgr.lock_or_recover()
                    .find(EXE_RESOURCE_MODULE, crate::lx::header::RT_BITMAP, id)?;
                let bytes = self.guest_read_bytes(res.0, res.1 as usize)?;
                let (_, s) = super::pm_bitmap::parse_bitmap_resource(&bytes)?;
                let (w, h) = (s.width.min(MAX_PICTURE_DIM), s.height.min(MAX_PICTURE_DIM));
                let mut pels = Vec::with_capacity((w * h) as usize);
                for row in (0..h as i32).rev() {
                    let r = s.row_pels(row)?;
                    pels.extend(r[..w as usize].iter().map(|&c| Some(c)));
                }
                Some(Picture { w, h, pels })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::surface::DrawOp;

    #[test]
    fn kinds_come_from_the_low_style_bits() {
        assert_eq!(static_kind(SS_TEXT | DT_CENTER | WS_VISIBLE), StaticKind::Text);
        assert_eq!(static_kind(SS_GROUPBOX), StaticKind::GroupBox);
        assert_eq!(static_kind(SS_ICON | SS_AUTOSIZE), StaticKind::Icon);
        assert_eq!(static_kind(SS_SYSICON), StaticKind::SysIcon);
        assert_eq!(static_kind(SS_BITMAP), StaticKind::Bitmap);
        assert_eq!(static_kind(SS_HALFTONEFRAME), StaticKind::Rect { color: RectColor::Halftone, frame: true });
        assert_eq!(static_kind(SS_BKGNDRECT), StaticKind::Rect { color: RectColor::Background, frame: false });
        assert_eq!(static_kind(0), StaticKind::Text);
        assert_eq!(static_kind(0x0030), StaticKind::Text);
    }

    #[test]
    fn resource_ids_are_hash_numbers() {
        assert_eq!(resource_id("#42"), Some(42));
        assert_eq!(resource_id(" #7 "), Some(7));
        assert_eq!(resource_id("42"), None);
        assert_eq!(resource_id("#x"), None);
        assert_eq!(resource_id("#70000"), None);
    }

    #[test]
    fn text_is_aligned_by_the_dt_bits() {
        let r = (10, 20, 100, 40);
        assert_eq!(text_origin(0, r, 30, 16), (10, 44), "top left");
        assert_eq!(text_origin(DT_CENTER | DT_VCENTER, r, 30, 16), (45, 32));
        assert_eq!(text_origin(DT_RIGHT | DT_BOTTOM, r, 30, 16), (80, 20));
        assert_eq!(text_origin(DT_RIGHT, r, 300, 16), (10, 44), "too wide: from the left");
    }

    fn static_ops(style: u32, text: &str, pres: &[(u32, u32)]) -> (Vec<DrawOp>, u32) {
        let loader = Loader::new_mock();
        let (frame, ctl) = {
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let frame = wm.create_window(super::super::pm_types::WindowManager::DIALOG_CLASS.into(), 0, 0);
            let w = wm.get_window_mut(frame).unwrap();
            (w.cx, w.cy, w.visible) = (200, 100, true);
            let ctl = wm.create_window("WC_STATIC".into(), frame, 0);
            let w = wm.get_window_mut(ctl).unwrap();
            (w.x, w.y, w.cx, w.cy, w.style, w.text, w.owner, w.visible) =
                (10, 10, 50, 30, style, text.into(), frame, true);
            for &(id, v) in pres { w.pres_params.set(id, &v.to_le_bytes()); }
            (frame, ctl)
        };
        let s = loader.shared.window_mgr.lock_or_recover().frame_surface(frame).unwrap();
        s.lock_or_recover().record = Some(Vec::new());
        loader.dispatch_builtin_control(ctl, WM_PAINT, 0, 0);
        let ops = s.lock_or_recover().record.take().unwrap();
        (ops, ctl)
    }

    #[test]
    fn rectangles_and_frames_use_the_colours() {
        let (ops, _) = static_ops(SS_FGNDRECT, "", &[(PP_FOREGROUNDCOLOR, 0x00AB_CDEF)]);
        assert!(ops.iter().any(|o| matches!(o, DrawOp::Shape { fill: Some(f), points, .. }
            if f.pattern.fg == 0x00AB_CDEF && f.pattern.is_solid() && points.len() == 4)), "{ops:?}");
        let (ops, _) = static_ops(SS_HALFTONEFRAME, "", &[(PP_BACKGROUNDCOLOR, 0x0011_2233)]);
        let shapes: Vec<_> = ops.iter().filter_map(|o| match o {
            DrawOp::Shape { fill: Some(f), .. } => Some(*f), _ => None }).collect();
        assert_eq!(shapes.len(), 4, "a frame is four edges");
        assert!(shapes.iter().all(|f| f.pattern.bg == 0x0011_2233 && !f.pattern.is_solid()));
    }

    #[test]
    fn text_takes_the_presentation_colours() {
        let (ops, _) = static_ops(SS_TEXT | DT_CENTER, "Hi", &[(PP_FOREGROUNDCOLOR, 0x00FF_0000),
                                                               (PP_BACKGROUNDCOLOR, 0x0000_FF00)]);
        assert!(ops.iter().any(|o| matches!(o, DrawOp::Box { color: 0x0000_FF00, fill: true, .. })));
        let text = ops.iter().find_map(|o| match o { DrawOp::Text { x, color, .. } => Some((*x, *color)), _ => None });
        let (x, color) = text.expect("text drawn");
        assert_eq!(color, 0x00FF_0000);
        assert!(x > 10, "centred");
    }

    #[test]
    fn a_font_presentation_parameter_changes_the_face() {
        let loader_ops = {
            let loader = Loader::new_mock();
            let mut wm = loader.shared.window_mgr.lock_or_recover();
            let frame = wm.create_window(super::super::pm_types::WindowManager::DIALOG_CLASS.into(), 0, 0);
            let ctl = wm.create_window("WC_STATIC".into(), frame, 0);
            let w = wm.get_window_mut(ctl).unwrap();
            (w.cx, w.cy, w.style, w.text, w.visible) = (80, 30, SS_TEXT, "Mono".into(), true);
            w.pres_params.set(super::super::pm_presparam::PP_FONTNAMESIZE, b"12.Courier\0");
            let s = wm.frame_surface(frame).unwrap();
            drop(wm);
            s.lock_or_recover().record = Some(Vec::new());
            loader.dispatch_builtin_control(ctl, WM_PAINT, 0, 0);
            s.lock_or_recover().record.take().unwrap()
        };
        assert!(loader_ops.iter().any(|o| matches!(o, DrawOp::TextFont { font, text, .. }
            if font.family == crate::gui::font::FontFamily::Mono && font.px == 20 && text == "Mono")), "{loader_ops:?}");
    }

    #[test]
    fn a_group_box_leaves_its_inside_alone() {
        let (ops, _) = static_ops(SS_GROUPBOX, "Group", &[]);
        let lines = ops.iter().filter(|o| matches!(o, DrawOp::Line { .. })).count();
        assert_eq!(lines, 4);
        // No fill reaches below the text band.
        assert!(!ops.iter().any(|o| matches!(o, DrawOp::Box { fill: true, y1, .. } if *y1 < 10 + 30 - CONTROL_LINE_H)));
    }

    #[test]
    fn system_icons_are_drawn_and_size_the_control() {
        let (ops, ctl) = static_ops(SS_SYSICON | SS_AUTOSIZE, &format!("#{SPTR_ICONINFORMATION}"), &[]);
        let n = super::super::pm_msgbox::MSGBOX_ICON_SIZE as u32;
        assert!(ops.iter().any(|o| matches!(o, DrawOp::Blit { w, h, .. } if *w == n && *h == n)), "{ops:?}");
        // Upright: the warning triangle's apex is in the top row (the blit
        // takes the bottom row first).
        let (ops, _) = static_ops(SS_SYSICON | SS_AUTOSIZE, &format!("#{SPTR_ICONWARNING}"), &[]);
        let src = ops.iter().find_map(|o| match o { DrawOp::Blit { src: Some(s), .. } => Some(s.clone()), _ => None })
            .expect("a blit");
        assert_eq!(src.len(), (n * n) as usize);
        let row = |r: usize| src[r * n as usize..][..n as usize].iter().filter(|p| p.is_some()).count();
        assert!(row(0) > row(n as usize - 1), "base {} at the bottom, apex {} at the top", row(0), row(n as usize - 1));
        // A missing icon draws only the background.
        let (ops, _) = static_ops(SS_ICON, "#999", &[]);
        assert!(!ops.iter().any(|o| matches!(o, DrawOp::Blit { .. })));
        let _ = ctl;
    }
}
