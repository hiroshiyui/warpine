// SPDX-License-Identifier: GPL-3.0-or-later
//
//! The drag image: the pictures `DrgDrag` and `DrgSetDragImage` are given
//! (`DRAGIMAGE` array, `<pmstddlg.h>`), drawn as the host pointer while
//! the drag runs (ADR-0045 addendum).
//!
//! - Each `DRAGIMAGE` is one layer: an icon (`DRG_ICON`, an `HPOINTER`) or
//!   a bitmap (`DRG_BITMAP`, an `HBITMAP`), stretched to `sizlStretch`
//!   with `DRG_STRETCH`, its lower-left corner `cxOffset` / `cyOffset`
//!   pels from the pointer's hot spot (PM coordinates, y up).
//! - The layers are composed into one pointer picture, the first at the
//!   bottom. Over a window that refuses the drop the picture gets the
//!   "no" sign over the hot spot.
//! - Warpine's choices (unverified): polygons (`DRG_POLYGON`) and
//!   `DRG_TRANSPARENT` are not drawn; a picture is clipped to
//!   [`IMAGE_MAX`] pels around the hot spot; a pointer set with
//!   `DrgSetDragPointer` replaces the picture over a target that accepts.

use std::sync::Arc;

use crate::gui::pointer::{PointerImage, PointerShape};

use super::{Loader, MutexExt};

/// `DRAGIMAGE` fields and size.
pub const DIM_HIMAGE: u32 = 4;
pub const DIM_CX_STRETCH: u32 = 8;
pub const DIM_CY_STRETCH: u32 = 12;
pub const DIM_FL: u32 = 16;
pub const DIM_CX_OFFSET: u32 = 20;
pub const DIM_CY_OFFSET: u32 = 22;
pub const DRAGIMAGE_SIZE: u32 = 24;

/// `DRAGIMAGE.fl` flags (`<pmstddlg.h>`).
pub const DRG_ICON: u32 = 0x0001;
pub const DRG_BITMAP: u32 = 0x0002;
pub const DRG_POLYGON: u32 = 0x0004;
pub const DRG_STRETCH: u32 = 0x0008;

/// The most `DRAGIMAGE`s read of one array (Warpine's bound).
pub const MAX_DRAG_IMAGES: u32 = 64;
/// The widest and tallest drag picture, in pels, centred on the hot spot.
pub const IMAGE_MAX: i32 = 256;
/// The "no" sign drawn over a refused drag: its radius and line width.
pub const NO_SIGN_RADIUS: i32 = 9;
pub const NO_SIGN_WIDTH: i32 = 2;
/// The "no" sign's colour (RGB).
pub const NO_SIGN_COLOR: u32 = 0xD0_00_00;

/// One picture of a drag image: `w` × `h` pels, rows from the top
/// (`None`: transparent), its lower-left corner `(dx, dy)` from the hot
/// spot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLayer {
    pub w: i32,
    pub h: i32,
    pub pels: Vec<Option<u32>>,
    pub dx: i32,
    pub dy: i32,
}

impl ImageLayer {
    /// The layer stretched to `w` × `h` (nearest pel).
    pub fn stretched(&self, w: i32, h: i32) -> ImageLayer {
        let (w, h) = (w.clamp(1, IMAGE_MAX), h.clamp(1, IMAGE_MAX));
        let pels = (0..h).flat_map(|r| (0..w).map(move |c| (r, c))).map(|(r, c)| {
            let (sx, sy) = (c * self.w / w, r * self.h / h);
            self.pels.get((sy * self.w + sx) as usize).copied().flatten()
        }).collect();
        ImageLayer { w, h, pels, dx: self.dx, dy: self.dy }
    }
}

/// The drag pictures: shown over a target that accepts, and with the "no"
/// sign over one that refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragImage {
    pub accepted: PointerShape,
    pub refused: PointerShape,
}

/// Compose `layers` into one pointer picture, the hot spot where the
/// layers' offsets start; with `refused`, the "no" sign over the hot spot.
/// `None` without a layer.
pub fn compose(layers: &[ImageLayer], refused: bool) -> Option<PointerImage> {
    if layers.is_empty() { return None; }
    let r = if refused { NO_SIGN_RADIUS } else { 0 };
    let half = IMAGE_MAX / 2;
    let x0 = layers.iter().map(|l| l.dx).min()?.min(-r).max(-half);
    let y0 = layers.iter().map(|l| l.dy).min()?.min(-r).max(-half);
    let x1 = layers.iter().map(|l| l.dx.saturating_add(l.w)).max()?.max(r + 1).min(half);
    let y1 = layers.iter().map(|l| l.dy.saturating_add(l.h)).max()?.max(r + 1).min(half);
    let (w, h) = (x1 - x0, y1 - y0);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let mut put = |x: i32, y: i32, c: u32| {
        if x < x0 || x >= x1 || y < y0 || y >= y1 { return; }
        let i = (((y1 - 1 - y) * w + (x - x0)) * 4) as usize;
        rgba[i..i + 4].copy_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8, 0xFF]);
    };
    for l in layers {
        for row in 0..l.h {
            for col in 0..l.w {
                if let Some(c) = l.pels.get((row * l.w + col) as usize).copied().flatten() {
                    put(l.dx + col, l.dy + l.h - 1 - row, c);
                }
            }
        }
    }
    if refused {
        // A ring and its falling diagonal.
        let (outer, inner) = (r * r, (r - NO_SIGN_WIDTH) * (r - NO_SIGN_WIDTH));
        for y in -r..=r {
            for x in -r..=r {
                let d = x * x + y * y;
                if (d <= outer && d > inner) || (d <= outer && (x + y).abs() < NO_SIGN_WIDTH) {
                    put(x, y, NO_SIGN_COLOR);
                }
            }
        }
    }
    Some(PointerImage { width: w as u32, height: h as u32, hot_x: -x0, hot_y: y1 - 1, rgba })
}

impl Loader {
    /// The drag image of the `cdimg` `DRAGIMAGE`s at `pdimg`; `None` when
    /// none of them is a picture Warpine draws.
    pub(super) fn drag_image(&self, pdimg: u32, cdimg: u32) -> Option<DragImage> {
        if pdimg == 0 { return None; }
        let layers: Vec<ImageLayer> = (0..cdimg.min(MAX_DRAG_IMAGES))
            .filter_map(|i| self.drag_image_layer(pdimg.checked_add(i * DRAGIMAGE_SIZE)?))
            .collect();
        let shape = |refused| compose(&layers, refused).map(|img| PointerShape::Image(Arc::new(img)));
        Some(DragImage { accepted: shape(false)?, refused: shape(true)? })
    }

    /// The layer of the `DRAGIMAGE` at `at`, if it is a picture.
    fn drag_image_layer(&self, at: u32) -> Option<ImageLayer> {
        let h = self.guest_read::<u32>(at + DIM_HIMAGE)?;
        let fl = self.guest_read::<u32>(at + DIM_FL)?;
        let dx = self.guest_read::<i16>(at + DIM_CX_OFFSET)? as i32;
        let dy = self.guest_read::<i16>(at + DIM_CY_OFFSET)? as i32;
        let (w, ht, pels) = if fl & DRG_POLYGON != 0 {
            return None;
        } else if fl & DRG_BITMAP != 0 {
            self.bitmap_layer_pels(h)?
        } else if fl & DRG_ICON != 0 {
            self.icon_layer_pels(h)?
        } else {
            return None;
        };
        let layer = ImageLayer { w, h: ht, pels, dx, dy };
        if fl & DRG_STRETCH != 0 {
            let cx = self.guest_read::<i32>(at + DIM_CX_STRETCH)?;
            let cy = self.guest_read::<i32>(at + DIM_CY_STRETCH)?;
            if cx > 0 && cy > 0 { return Some(layer.stretched(cx, cy)); }
        }
        Some(layer)
    }

    /// Bitmap `hbm`'s pels, rows from the top, clipped to [`IMAGE_MAX`].
    fn bitmap_layer_pels(&self, hbm: u32) -> Option<(i32, i32, Vec<Option<u32>>)> {
        let surface = self.shared.window_mgr.lock_or_recover().bitmaps.get(&hbm).map(|b| b.surface.clone())?;
        let s = surface.lock_or_recover();
        let (w, h) = ((s.width as i32).min(IMAGE_MAX), (s.height as i32).min(IMAGE_MAX));
        let pels = (0..h).flat_map(|r| (0..w).map(move |c| (r, c))).map(|(r, c)| s.pel(c, h - 1 - r)).collect();
        Some((w, h, pels))
    }

    /// Icon (pointer) `hptr`'s pels, rows from the top: a created pointer's
    /// picture, or Warpine's pictogram of a system icon.
    fn icon_layer_pels(&self, hptr: u32) -> Option<(i32, i32, Vec<Option<u32>>)> {
        let shape = self.shared.pointers.lock_or_recover().shape(hptr)?;
        match shape {
            PointerShape::Image(img) => {
                let (w, h) = ((img.width as i32).min(IMAGE_MAX), (img.height as i32).min(IMAGE_MAX));
                let iw = img.width as usize;
                let pels = (0..h as usize).flat_map(|r| (0..w as usize).map(move |c| (r, c))).map(|(r, c)| {
                    let i = (r * iw + c) * 4;
                    img.rgba.get(i..i + 4).and_then(|p| (p[3] >= 0x80)
                        .then(|| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32))
                }).collect();
                Some((w, h, pels))
            }
            PointerShape::System(_) => {
                let sptr = hptr.wrapping_sub(super::pm_pointer::PointerManager::system_handle(0));
                let n = super::pm_msgbox::MSGBOX_ICON_SIZE;
                Some((n, n, super::pm_msgbox::icon_pels(sptr)?))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: i32, h: i32, c: u32, dx: i32, dy: i32) -> ImageLayer {
        ImageLayer { w, h, pels: vec![Some(c); (w * h) as usize], dx, dy }
    }

    fn pel(img: &PointerImage, x: i32, y: i32) -> Option<u32> {
        // (x, y) from the hot spot, y up.
        let (col, row) = (img.hot_x + x, img.hot_y - y);
        let i = ((row * img.width as i32 + col) * 4) as usize;
        let p = img.rgba.get(i..i + 4)?;
        (p[3] != 0).then(|| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32)
    }

    #[test]
    fn layers_sit_at_their_offsets_from_the_hot_spot() {
        let img = compose(&[solid(4, 4, 0x0000FF, -2, -2), solid(2, 2, 0x00FF00, 4, 4)], false).unwrap();
        assert_eq!((img.width, img.height), (8, 8));
        assert_eq!((img.hot_x, img.hot_y), (2, 5));
        assert_eq!(pel(&img, -2, -2), Some(0x0000FF));
        assert_eq!(pel(&img, 1, 1), Some(0x0000FF));
        assert_eq!(pel(&img, 2, 2), None);
        assert_eq!(pel(&img, 5, 5), Some(0x00FF00));
    }

    #[test]
    fn later_layers_cover_earlier_ones_and_rows_run_from_the_top() {
        let mut top_red = solid(2, 2, 0x0000FF, 0, 0);
        top_red.pels[0] = Some(0xFF0000); // the top-left pel
        let img = compose(&[solid(2, 2, 0x00FF00, 0, 0), top_red], false).unwrap();
        assert_eq!(pel(&img, 0, 1), Some(0xFF0000));
        assert_eq!(pel(&img, 0, 0), Some(0x0000FF));
    }

    #[test]
    fn a_refused_drag_shows_the_no_sign_over_the_hot_spot() {
        let layers = [solid(4, 4, 0x0000FF, 20, 20)];
        let ok = compose(&layers, false).unwrap();
        let no = compose(&layers, true).unwrap();
        assert_eq!(pel(&ok, 0, 0), None);
        assert_eq!(pel(&no, 0, 0), Some(NO_SIGN_COLOR), "the diagonal crosses the hot spot");
        assert_eq!(pel(&no, NO_SIGN_RADIUS, 0), Some(NO_SIGN_COLOR), "the ring");
        assert_eq!(pel(&no, 21, 21), Some(0x0000FF), "the picture stays");
    }

    #[test]
    fn pictures_are_clipped_around_the_hot_spot_and_stretched() {
        let img = compose(&[solid(1000, 10, 1, 0, 0)], false).unwrap();
        assert_eq!(img.width as i32, IMAGE_MAX / 2);
        let s = solid(2, 2, 7, 3, 4).stretched(6, 4);
        assert_eq!((s.w, s.h, s.dx, s.dy, s.pels.len()), (6, 4, 3, 4, 24));
        assert!(compose(&[], false).is_none());
    }
}
