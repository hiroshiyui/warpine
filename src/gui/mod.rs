// SPDX-License-Identifier: GPL-3.0-only

// The SDL front end uses only the safe sdl3-rs API (doc/TODOs.md, SDL3
// migration): no `unsafe` anywhere in the GUI.
#![forbid(unsafe_code)]

pub mod font;
pub mod damage;
pub mod frame_stats;
pub mod gui_script;
pub mod message;
pub mod pointer;
pub mod renderer;
pub mod surface;
pub mod audio;
pub mod headless;
pub mod sdl_renderer;
pub mod text_renderer;

pub use message::{GUIMessage, GUISender, GUIReceiver, GuiWake, create_gui_channel};
pub use renderer::{PmRenderer, run_pm_loop};
pub use surface::{DrawOp, Mix, SharedSurface, Surface};
pub use font::{CjkRegion, FontSpec, FontFamily};
pub use headless::HeadlessRenderer;
pub use sdl_renderer::{SdlRenderer, push_msg, sdl_scancode_to_os2, sdl_keycode_to_vk};
pub use text_renderer::{
    CGA_PALETTE, VgaTextBuffer, TextModeRenderer, HeadlessTextRenderer,
    SdlTextRenderer, run_text_loop, get_glyph_for_char,
};
