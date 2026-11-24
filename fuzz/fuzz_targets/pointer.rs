// SPDX-License-Identifier: GPL-3.0-or-later
//! The pointer decoder (src/loader/pointer_codec.rs, on bitmap_codec.rs):
//! a program's RT_POINTER resources are untrusted. A pointer or icon file,
//! or a bitmap array of them, becomes an RGBA image with a hot spot.
#![no_main]

use libfuzzer_sys::fuzz_target;

/// What the codecs take from the rest of Warpine (src/loader/constants.rs,
/// src/gui/surface.rs, src/gui/pointer.rs): the same items, with the same
/// values. Both are included at the root, so their `super::` and `crate::`
/// are the root too.
mod constants {
    pub const BCA_UNCOMP: u32 = 0;
    pub const BCA_RLE8: u32 = 1;
    pub const BCA_RLE4: u32 = 2;
    pub const BCA_RLE24: u32 = 4;
    pub const BFT_BMAP: u16 = 0x4d42;
    pub const BFT_BITMAPARRAY: u16 = 0x4142;
    pub const POINTER_SIZE: u32 = 32;
    pub const MAX_POINTER_DIM: u32 = 256;
}
mod gui {
    pub mod surface {
        pub const MAX_SURFACE_DIM: u32 = 8192;
    }
    pub mod pointer {
        #[derive(Debug)]
        pub struct PointerImage {
            pub width: u32,
            pub height: u32,
            pub hot_x: i32,
            pub hot_y: i32,
            pub rgba: Vec<u8>,
        }
    }
}

#[path = "../../src/loader/bitmap_codec.rs"]
#[allow(dead_code)]
mod bitmap_codec;
#[path = "../../src/loader/pointer_codec.rs"]
#[allow(dead_code)]
mod pointer_codec;

fuzz_target!(|data: &[u8]| {
    if let Some(img) = pointer_codec::decode_pointer_resource(data) {
        assert!(img.width > 0 && img.height > 0 && img.width <= 256 && img.height <= 256);
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize, "four bytes per pel");
        assert!((0..img.width as i32).contains(&img.hot_x) && (0..img.height as i32).contains(&img.hot_y));
    }
});
