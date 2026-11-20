// SPDX-License-Identifier: GPL-3.0-or-later
//
// The host mouse cursor that shows the PM pointer.
//
// API threads decide which pointer PM shows (`loader::pm_pointer`) and send
// its shape with `GUIMessage::SetPointer`; SDL cursors are not `Send`, so
// they are created and kept here, on the GUI thread. A system pointer
// (`SPTR_*`) becomes the nearest SDL system cursor; a pointer loaded from
// a resource becomes an SDL colour cursor built from its RGBA image.

use std::collections::HashMap;
use std::sync::Arc;

use log::debug;
use sdl3::mouse::{Cursor, MouseUtil, SystemCursor};
use sdl3::pixels::PixelFormat;

/// The PM system pointers that have an SDL system cursor of the same kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemPointer {
    Arrow,
    /// The I-beam (`SPTR_TEXT`).
    Text,
    Wait,
    /// `SPTR_SIZE` and `SPTR_MOVE` (four-headed arrows).
    Move,
    SizeNwse,
    SizeNesw,
    SizeWe,
    SizeNs,
    Illegal,
}

impl SystemPointer {
    /// The SDL system cursor shown for it.
    pub fn sdl_cursor(self) -> SystemCursor {
        match self {
            SystemPointer::Arrow => SystemCursor::Arrow,
            SystemPointer::Text => SystemCursor::IBeam,
            SystemPointer::Wait => SystemCursor::Wait,
            SystemPointer::Move => SystemCursor::SizeAll,
            SystemPointer::SizeNwse => SystemCursor::SizeNWSE,
            SystemPointer::SizeNesw => SystemCursor::SizeNESW,
            SystemPointer::SizeWe => SystemCursor::SizeWE,
            SystemPointer::SizeNs => SystemCursor::SizeNS,
            SystemPointer::Illegal => SystemCursor::No,
        }
    }
}

/// A pointer image: `width` × `height` RGBA pels (red, green, blue, alpha
/// bytes), top row first; the hot spot counts from the top-left pel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PointerImage {
    pub width: u32,
    pub height: u32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub rgba: Vec<u8>,
}

/// What the host cursor shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointerShape {
    System(SystemPointer),
    Image(Arc<PointerImage>),
}

/// The host cursor, owned by the GUI thread.
pub struct HostPointer {
    mouse: MouseUtil,
    /// System cursors created so far (SDL keeps showing a cursor only while
    /// it exists).
    system: HashMap<SystemPointer, Cursor>,
    /// The colour cursor of the image shown now.
    image: Option<Cursor>,
    /// The shape last applied; `None` = hidden.
    shown: Option<PointerShape>,
}

impl HostPointer {
    pub fn new(mouse: MouseUtil) -> Self {
        HostPointer { mouse, system: HashMap::new(), image: None,
                      shown: Some(PointerShape::System(SystemPointer::Arrow)) }
    }

    /// Show `shape`, or hide the cursor for `None`. A cursor SDL cannot
    /// create (the `dummy` video driver has none) leaves the old one: the
    /// program does not notice.
    pub fn apply(&mut self, shape: Option<PointerShape>) {
        if shape == self.shown { return; }
        match &shape {
            None => self.mouse.show_cursor(false),
            Some(PointerShape::System(sp)) => {
                if !self.system.contains_key(sp) {
                    match Cursor::from_system(sp.sdl_cursor()) {
                        Ok(c) => { self.system.insert(*sp, c); }
                        Err(e) => debug!("no SDL system cursor for {sp:?}: {e}"),
                    }
                }
                if let Some(c) = self.system.get(sp) { c.set(); }
                self.image = None;
                self.mouse.show_cursor(true);
            }
            Some(PointerShape::Image(img)) => {
                match color_cursor(img) {
                    Ok(c) => { c.set(); self.image = Some(c); }
                    Err(e) => debug!("no SDL colour cursor: {e}"),
                }
                self.mouse.show_cursor(true);
            }
        }
        self.shown = shape;
    }
}

/// An SDL colour cursor of `img`.
fn color_cursor(img: &PointerImage) -> Result<Cursor, sdl3::Error> {
    let mut surface = sdl3::surface::Surface::new(img.width, img.height, PixelFormat::RGBA32)?;
    let pitch = surface.pitch() as usize;
    let row = img.width as usize * 4;
    surface.with_lock_mut(|pixels| {
        for (y, src) in img.rgba.chunks_exact(row).take(img.height as usize).enumerate() {
            if let Some(dst) = pixels.get_mut(y * pitch..y * pitch + row) {
                dst.copy_from_slice(src);
            }
        }
    });
    Cursor::from_surface(surface, img.hot_x, img.hot_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_pointers_map_to_sdl_cursors_of_the_same_kind() {
        assert_eq!(SystemPointer::Arrow.sdl_cursor() as u32, SystemCursor::Arrow as u32);
        assert_eq!(SystemPointer::Text.sdl_cursor() as u32, SystemCursor::IBeam as u32);
        assert_eq!(SystemPointer::Wait.sdl_cursor() as u32, SystemCursor::Wait as u32);
        assert_eq!(SystemPointer::Move.sdl_cursor() as u32, SystemCursor::SizeAll as u32);
        assert_eq!(SystemPointer::SizeNwse.sdl_cursor() as u32, SystemCursor::SizeNWSE as u32);
        assert_eq!(SystemPointer::SizeNesw.sdl_cursor() as u32, SystemCursor::SizeNESW as u32);
        assert_eq!(SystemPointer::SizeWe.sdl_cursor() as u32, SystemCursor::SizeWE as u32);
        assert_eq!(SystemPointer::SizeNs.sdl_cursor() as u32, SystemCursor::SizeNS as u32);
        assert_eq!(SystemPointer::Illegal.sdl_cursor() as u32, SystemCursor::No as u32);
    }
}
