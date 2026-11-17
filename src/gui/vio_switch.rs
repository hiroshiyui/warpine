// SPDX-License-Identifier: GPL-3.0-only

//! Alt+Home: a VIO session switches between its window on the PM desktop and
//! full screen while the program keeps running.
//!
//! IBM documents Alt+Home as the switch between full screen and a window for
//! DOS sessions (the `KBD_ALTHOME_BYPASS` DOS setting: "Off (Alt+Home will
//! cause a switch between full screen and windowed mode)", GG24-3731).
//! Warpine offers the same for window-compatible OS/2 VIO sessions; whether
//! OS/2 itself does so for them is not verified. A program that is not
//! window-compatible stays full screen and gets the key.
//!
//! The renderer that sees Alt+Home sets `SharedState::vio_switch_requested`
//! and its loop returns; `run_vio_session` (`main.rs`) then parks that
//! renderer's SDL window, audio output and GUI script in a [`Parked`] and
//! opens the other display from it. The VIO buffer, the keyboard queue and
//! the guest are untouched. Parked windows are hidden and reused rather than
//! destroyed: their texture creators live for the process (the
//! lifetime-checked textures are `'static`), which would otherwise keep a
//! window alive on every switch.

use std::sync::atomic::Ordering;

use crate::lx::header::AppType;
use crate::loader::SharedState;
use super::audio::SdlAudio;
use super::gui_script::GuiScript;
use super::vio_window::VioDisplay;

/// The display Alt+Home switches a VIO session of type `app_type` to from
/// `current`, or `None` when it stays where it is: a program that is not
/// window-compatible always runs full screen.
pub fn alt_home_target(app_type: AppType, current: VioDisplay) -> Option<VioDisplay> {
    match (app_type, current) {
        (AppType::NotWindowCompatible, _) => None,
        (_, VioDisplay::Window) => Some(VioDisplay::FullScreen),
        (_, VioDisplay::FullScreen) => Some(VioDisplay::Window),
    }
}

/// Whether a key press is Alt+Home: Alt held and Ctrl not (Ctrl+Alt
/// combinations belong to the host); a held key's repeats do not count.
pub fn is_alt_home(kc: sdl3::keyboard::Keycode, keymod: sdl3::keyboard::Mod, repeat: bool) -> bool {
    use sdl3::keyboard::{Keycode, Mod};
    !repeat && kc == Keycode::Home
        && keymod.intersects(Mod::LALTMOD | Mod::RALTMOD)
        && !keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD)
}

/// A key press in a VIO session's display: if it is Alt+Home and the
/// session can switch, ask the GUI loop to return so the other display is
/// shown, and return true — the key is not given to the program.
pub fn request_switch(shared: &SharedState, kc: sdl3::keyboard::Keycode,
                      keymod: sdl3::keyboard::Mod, repeat: bool) -> bool {
    if !is_alt_home(kc, keymod, repeat) || !shared.vio_switchable.load(Ordering::Relaxed) {
        return false;
    }
    shared.vio_switch_requested.store(true, Ordering::Relaxed);
    true
}

/// Whether a switch was asked for and not yet taken.
pub fn switch_pending(shared: &SharedState) -> bool {
    shared.vio_switch_requested.load(Ordering::Relaxed)
}

/// Take a pending switch request, if any.
pub fn take_switch_request(shared: &SharedState) -> bool {
    shared.vio_switch_requested.swap(false, Ordering::Relaxed)
}

/// What one display leaves for the next when a VIO session switches:
/// SDL windows to show again, the audio output (so sounds playing go on and
/// their waiters are not stranded) and the GUI script with its clock.
#[derive(Default)]
pub struct Parked {
    pub(crate) text: Option<super::text_renderer::TextWindow>,
    pub(crate) desktop: Option<super::sdl_renderer::ParkedDesktop>,
    pub(crate) audio: Option<SdlAudio>,
    /// The script being replayed, if the parked display had one; `None`
    /// before the first display opens (it loads the script itself).
    pub(crate) script: Option<Option<GuiScript>>,
}

impl Parked {
    /// The audio output left behind, or a new one.
    pub(crate) fn take_audio(&mut self, sdl: &sdl3::Sdl) -> SdlAudio {
        self.audio.take().unwrap_or_else(|| SdlAudio::new(sdl))
    }

    /// The GUI script left behind, or the one named by the environment for
    /// the first display.
    pub(crate) fn take_script(&mut self) -> Option<GuiScript> {
        self.script.take().unwrap_or_else(GuiScript::from_env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl3::keyboard::{Keycode, Mod};

    #[test]
    fn alt_home_switches_only_a_window_compatible_program() {
        use AppType::*;
        assert_eq!(alt_home_target(WindowCompatible, VioDisplay::Window), Some(VioDisplay::FullScreen));
        assert_eq!(alt_home_target(WindowCompatible, VioDisplay::FullScreen), Some(VioDisplay::Window));
        assert_eq!(alt_home_target(Unspecified, VioDisplay::FullScreen), Some(VioDisplay::Window));
        assert_eq!(alt_home_target(NotWindowCompatible, VioDisplay::FullScreen), None);
        assert_eq!(alt_home_target(NotWindowCompatible, VioDisplay::Window), None);
    }

    #[test]
    fn alt_home_is_alt_and_home_without_ctrl_or_repeat() {
        assert!(is_alt_home(Keycode::Home, Mod::LALTMOD, false));
        assert!(is_alt_home(Keycode::Home, Mod::RALTMOD | Mod::LSHIFTMOD, false));
        assert!(!is_alt_home(Keycode::Home, Mod::LALTMOD, true), "a repeat");
        assert!(!is_alt_home(Keycode::Home, Mod::NOMOD, false));
        assert!(!is_alt_home(Keycode::Home, Mod::LALTMOD | Mod::LCTRLMOD, false));
        assert!(!is_alt_home(Keycode::End, Mod::LALTMOD, false));
    }

    #[test]
    fn a_switch_is_requested_only_in_a_switchable_session() {
        let shared = crate::loader::Loader::new_mock().get_shared();
        // Not switchable: the key is the program's.
        assert!(!request_switch(&shared, Keycode::Home, Mod::LALTMOD, false));
        assert!(!switch_pending(&shared));
        shared.vio_switchable.store(true, Ordering::Relaxed);
        assert!(!request_switch(&shared, Keycode::Home, Mod::NOMOD, false));
        assert!(request_switch(&shared, Keycode::Home, Mod::LALTMOD, false));
        assert!(switch_pending(&shared));
        assert!(take_switch_request(&shared));
        assert!(!take_switch_request(&shared), "taken once");
    }

    #[test]
    fn parked_script_is_handed_on_once_loaded() {
        let mut parked = Parked { script: Some(None), ..Parked::default() };
        // A parked "no script" stays no script, whatever the environment says.
        assert!(parked.take_script().is_none());
        let steps = crate::gui::gui_script::parse("0 quit").unwrap();
        parked.script = Some(Some(GuiScript::new(steps)));
        assert!(parked.take_script().is_some());
    }
}
