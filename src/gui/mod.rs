// SPDX-License-Identifier: GPL-3.0-only

pub mod font;
pub mod damage;
pub mod frame_stats;
pub mod gui_script;
pub mod message;
pub mod renderer;
pub mod surface;
pub mod headless;
pub mod sdl2_renderer;
pub mod text_renderer;

pub use message::{GUIMessage, GUISender, GUIReceiver, GuiWake, create_gui_channel};
pub use renderer::{PmRenderer, run_pm_loop};
pub use surface::{DrawOp, Mix, SharedSurface, Surface};
pub use font::{CjkRegion, FontSpec, FontFamily};
pub use headless::HeadlessRenderer;
pub use sdl2_renderer::{Sdl2Renderer, push_msg, sdl_scancode_to_os2, sdl_keycode_to_vk};
pub use text_renderer::{
    CGA_PALETTE, VgaTextBuffer, TextModeRenderer, HeadlessTextRenderer,
    Sdl2TextRenderer, run_text_loop, get_glyph_for_char,
};
