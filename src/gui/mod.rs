// SPDX-License-Identifier: GPL-3.0-only

pub mod message;
pub mod renderer;
pub mod render_utils;
pub mod headless;
pub mod sdl2_renderer;
pub mod text_renderer;

pub use message::{GUIMessage, GUISender, create_gui_channel};
pub use renderer::{PmRenderer, run_pm_loop};
pub use render_utils::{flip_y, text_screen_y, glyph_index,
    render_text_to_buffer, render_rect_to_buffer, render_line_to_buffer};
pub use headless::HeadlessRenderer;
pub use sdl2_renderer::{Sdl2Renderer, push_msg, sdl_scancode_to_os2, sdl_keycode_to_vk};
pub use text_renderer::{
    CGA_PALETTE, VgaTextBuffer, TextModeRenderer, HeadlessTextRenderer,
    Sdl2TextRenderer, run_text_loop, get_cp437_glyph,
};
