// SPDX-License-Identifier: GPL-3.0-or-later

//! Themes: how Warpine draws what it draws itself — frame chrome now,
//! controls and menus later (ADR-0118).
//!
//! A theme changes pels only, never geometry a program can see: window
//! sizes, client rectangles, `SV_*` metrics, frame insets and hit areas are
//! the same in every theme. Two exist:
//!
//! * `classic` — the look Warpine has always had: bevelled borders and
//!   title-bar buttons, square frames. It is the default for now, and the
//!   look the golden pictures hold.
//! * `modern` — flat chrome: a thin edge line instead of a bevelled
//!   border, flat title-bar pictograms in the title's text colour, rounded
//!   outer corners and a soft shadow under each frame
//!   ([`super::frame_shape`]). A maximized frame stays square and casts no
//!   shadow.
//!
//! `WARPINE_THEME=classic|modern` picks one for the session; the desktop
//! server needs no theme of its own (a client marks the pictures it wants
//! shaped, `Item::rounded`).

use std::sync::OnceLock;

/// Environment variable naming the theme.
pub const THEME_ENV: &str = "WARPINE_THEME";

/// The themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Classic,
    Modern,
}

/// The modern edge line around a frame (inside its border).
pub const MODERN_EDGE_COLOR: u32 = 0x0080_8080;
/// Corner radius of a modern control's box (buttons, fields, lists).
pub const CONTROL_RADIUS: i32 = 4;
/// The modern accent: the focus ring and a focused field's border.
pub const MODERN_ACCENT_COLOR: u32 = 0x0030_6FD0;
/// The face of a modern push button under the pointer.
pub const MODERN_HOVER_FACE: u32 = 0x00E6_E3DC;

/// How far row `k` (0 = the outermost) of a box with corners of radius
/// `r` is inset at each end; 0 from row `r` on.
pub fn corner_inset(r: i32, k: i32) -> i32 {
    if k >= r { return 0; }
    let dy = r as f32 - k as f32 - 0.5;
    let dx = ((r * r) as f32 - dy * dy).max(0.0).sqrt();
    (r as f32 - dx).round() as i32
}

impl Theme {
    /// The theme a `WARPINE_THEME` value names; classic for anything else.
    pub fn from_name(name: Option<&str>) -> Theme {
        match name.map(str::trim) {
            Some(n) if n.eq_ignore_ascii_case("modern") => Theme::Modern,
            _ => Theme::Classic,
        }
    }

    /// Whether frames are drawn with rounded corners and a shadow.
    pub fn rounded_frames(self) -> bool {
        self == Theme::Modern
    }

    /// Whether borders and title-bar buttons are bevelled.
    pub fn bevelled(self) -> bool {
        self == Theme::Classic
    }
}

/// The session's theme, read once from [`THEME_ENV`].
pub fn current() -> Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    *THEME.get_or_init(|| Theme::from_name(std::env::var(THEME_ENV).ok().as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_comes_from_its_name_and_defaults_to_classic() {
        assert_eq!(Theme::from_name(Some("modern")), Theme::Modern);
        assert_eq!(Theme::from_name(Some(" Modern ")), Theme::Modern);
        assert_eq!(Theme::from_name(Some("classic")), Theme::Classic);
        assert_eq!(Theme::from_name(Some("neon")), Theme::Classic);
        assert_eq!(Theme::from_name(None), Theme::Classic);
        assert!(Theme::Modern.rounded_frames() && !Theme::Modern.bevelled());
        assert!(!Theme::Classic.rounded_frames() && Theme::Classic.bevelled());
    }

    #[test]
    fn control_corners_are_inset_only_near_the_ends() {
        let r = CONTROL_RADIUS;
        let insets: Vec<i32> = (0..=r).map(|k| corner_inset(r, k)).collect();
        assert!(insets[0] > 0, "the outermost row is inset: {insets:?}");
        assert!(insets.windows(2).all(|w| w[0] >= w[1]), "insets shrink inwards: {insets:?}");
        assert_eq!(insets[r as usize], 0);
        assert_eq!(corner_inset(0, 0), 0, "no radius, no inset");
    }
}
