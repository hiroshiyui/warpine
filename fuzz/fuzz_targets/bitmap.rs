// SPDX-License-Identifier: GPL-3.0-or-later
//! The bitmap codec (src/loader/bitmap_codec.rs): a program's RT_BITMAP
//! resources and the bitmap data it hands GPI are untrusted. A resource is
//! split into its format and pel data, which is then decoded, uncompressed
//! or RLE, when the bitmap is small enough to keep the run fast.
#![no_main]

use libfuzzer_sys::fuzz_target;

/// What bitmap_codec.rs takes from the rest of Warpine
/// (src/loader/constants.rs, src/gui/surface.rs): the same items, with the
/// same values. The codec is included at the root, so its `super::` is the
/// root too.
mod constants {
    pub const BCA_UNCOMP: u32 = 0;
    pub const BCA_RLE8: u32 = 1;
    pub const BCA_RLE4: u32 = 2;
    pub const BCA_RLE24: u32 = 4;
    pub const BFT_BMAP: u16 = 0x4d42;
    pub const BFT_BITMAPARRAY: u16 = 0x4142;
}
mod gui {
    pub mod surface {
        pub const MAX_SURFACE_DIM: u32 = 8192;
    }
}

#[path = "../../src/loader/bitmap_codec.rs"]
#[allow(dead_code)]
mod bitmap_codec;

/// Bitmaps of more pels are only split, not decoded.
const MAX_DECODED_PELS: u64 = 1 << 20;

fuzz_target!(|data: &[u8]| {
    use bitmap_codec::{bitmap_resource_parts, decode_rows};
    if let Some((fmt, pels)) = bitmap_resource_parts(data)
        && u64::from(fmt.cx) * u64::from(fmt.cy) <= MAX_DECODED_PELS {
        let rows = decode_rows(&fmt, pels, fmt.cy);
        assert_eq!(rows.len(), fmt.cy as usize, "one row per line");
        assert!(rows.iter().all(|r| r.len() == fmt.cx as usize), "cx pels per row");
    }
});
