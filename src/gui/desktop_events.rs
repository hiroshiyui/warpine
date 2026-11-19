// SPDX-License-Identifier: GPL-3.0-only

//! SDL events ↔ desktop protocol input (ADR-0052). The desktop server turns
//! the host's (or a GUI script's) SDL input events into
//! [`proto::Input`](crate::desktop::proto::Input) for the client it routes
//! them to; the client turns them back into SDL events and handles them
//! exactly as it handles its own window's events.

use sdl3::event::Event;
use sdl3::keyboard::{Keycode, Mod, Scancode};
use sdl3::mouse::{MouseButton, MouseState};

use crate::desktop::proto::{Input, MAX_COORD};

use super::pointer::SystemPointer;

/// Desktop coordinates of an SDL event, as whole pels within the protocol's
/// range.
fn pel(v: f32) -> i32 {
    (v as i32).clamp(-MAX_COORD, MAX_COORD)
}

/// The protocol input for an SDL input event (`None`: not input).
pub fn to_input(event: &Event) -> Option<Input> {
    Some(match event {
        Event::MouseMotion { mousestate, x, y, xrel, yrel, .. } => Input::Motion {
            x: pel(*x), y: pel(*y), dx: pel(*xrel), dy: pel(*yrel), buttons: mousestate.to_sdl_state(),
        },
        Event::MouseButtonDown { mouse_btn, clicks, x, y, .. } => Input::Button {
            down: true, button: *mouse_btn as u8, clicks: *clicks, x: pel(*x), y: pel(*y),
        },
        Event::MouseButtonUp { mouse_btn, clicks, x, y, .. } => Input::Button {
            down: false, button: *mouse_btn as u8, clicks: *clicks, x: pel(*x), y: pel(*y),
        },
        Event::KeyDown { keycode, scancode, keymod, repeat, .. } => Input::Key {
            down: true, scancode: scancode.map_or(0, |s| s as i32), keycode: keycode.map_or(0, |k| k as u32),
            mods: keymod.bits(), repeat: *repeat,
        },
        Event::KeyUp { keycode, scancode, keymod, repeat, .. } => Input::Key {
            down: false, scancode: scancode.map_or(0, |s| s as i32), keycode: keycode.map_or(0, |k| k as u32),
            mods: keymod.bits(), repeat: *repeat,
        },
        Event::TextInput { text, .. } => Input::Text(text.clone()),
        Event::MouseWheel { y, integer_y, direction, mouse_x, mouse_y, .. } => Input::Wheel {
            x: pel(*mouse_x), y: pel(*mouse_y), steps: wheel_steps(*y, *integer_y, *direction),
        },
        _ => return None,
    })
}

/// The SDL event for protocol input `input`; `None` for a drop of host
/// files, which has no SDL event with a position (the client hands it to
/// the program at once, `SdlRenderer::host_drop`).
pub fn to_event(input: &Input) -> Option<Event> {
    let (timestamp, window_id, which) = (0, 0, 0);
    Some(match input {
        Input::Motion { x, y, dx, dy, buttons } => Event::MouseMotion {
            timestamp, window_id, which, mousestate: MouseState::from_sdl_state(*buttons),
            x: *x as f32, y: *y as f32, xrel: *dx as f32, yrel: *dy as f32,
        },
        Input::Button { down: true, button, clicks, x, y } => Event::MouseButtonDown {
            timestamp, window_id, which, mouse_btn: MouseButton::from_ll(*button), clicks: *clicks,
            x: *x as f32, y: *y as f32,
        },
        Input::Button { down: false, button, clicks, x, y } => Event::MouseButtonUp {
            timestamp, window_id, which, mouse_btn: MouseButton::from_ll(*button), clicks: *clicks,
            x: *x as f32, y: *y as f32,
        },
        Input::Key { down, scancode, keycode, mods, repeat } => {
            let (keycode, scancode) = (Keycode::from_u32(*keycode), Scancode::from_i32(*scancode));
            let keymod = Mod::from_bits_truncate(*mods);
            if *down {
                Event::KeyDown { timestamp, window_id, keycode, scancode, keymod, repeat: *repeat, which, raw: 0 }
            } else {
                Event::KeyUp { timestamp, window_id, keycode, scancode, keymod, repeat: *repeat, which, raw: 0 }
            }
        }
        Input::Text(text) => Event::TextInput { timestamp, window_id, text: text.clone() },
        Input::Wheel { x, y, steps } => Event::MouseWheel {
            timestamp, window_id, which, x: 0.0, y: *steps as f32,
            direction: sdl3::mouse::MouseWheelDirection::Normal,
            mouse_x: *x as f32, mouse_y: *y as f32, integer_x: 0, integer_y: *steps,
        },
        Input::Drop { .. } => return None,
    })
}

/// The drop of host files `paths` at desktop point `(x, y)`, within the
/// protocol's limits: paths longer than `MAX_DROP_PATH` are left out (a cut
/// path would name another file), at most `MAX_DROP_FILES` are kept;
/// `None` when none is left.
pub fn drop_input(x: i32, y: i32, paths: Vec<String>) -> Option<Input> {
    use crate::desktop::proto::{MAX_DROP_FILES, MAX_DROP_PATH};
    let paths: Vec<String> = paths.into_iter().filter(|p| !p.is_empty() && p.len() <= MAX_DROP_PATH)
        .take(MAX_DROP_FILES).collect();
    (!paths.is_empty()).then_some(Input::Drop { x: pel(x as f32), y: pel(y as f32), paths })
}

/// The notches of an SDL wheel event, up positive (a flipped wheel turned
/// back), within `MAX_WHEEL_STEPS`; a fractional turn counts as one notch.
pub fn wheel_steps(y: f32, integer_y: i32, direction: sdl3::mouse::MouseWheelDirection) -> i32 {
    let n = if integer_y != 0 { integer_y } else if y > 0.0 { 1 } else if y < 0.0 { -1 } else { 0 };
    let n = if matches!(direction, sdl3::mouse::MouseWheelDirection::Flipped) { -n } else { n };
    n.clamp(-crate::desktop::proto::MAX_WHEEL_STEPS, crate::desktop::proto::MAX_WHEEL_STEPS)
}

/// The system pointers by protocol index.
pub const SYSTEM_POINTERS: [SystemPointer; 9] = [
    SystemPointer::Arrow, SystemPointer::Text, SystemPointer::Wait, SystemPointer::Move,
    SystemPointer::SizeNwse, SystemPointer::SizeNesw, SystemPointer::SizeWe, SystemPointer::SizeNs,
    SystemPointer::Illegal,
];

/// The protocol index of system pointer `p`.
pub fn pointer_index(p: SystemPointer) -> u8 {
    SYSTEM_POINTERS.iter().position(|&q| q == p).unwrap_or(0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_events_survive_the_trip_through_the_protocol() {
        let events = [
            Input::Motion { x: 10, y: 20, dx: -1, dy: 2, buttons: 1 },
            Input::Button { down: true, button: MouseButton::Right as u8, clicks: 2, x: 3, y: 4 },
            Input::Button { down: false, button: MouseButton::Left as u8, clicks: 1, x: 3, y: 4 },
            Input::Key { down: true, scancode: Scancode::A as i32, keycode: Keycode::A as u32,
                         mods: Mod::LSHIFTMOD.bits(), repeat: false },
            Input::Key { down: false, scancode: Scancode::F10 as i32, keycode: Keycode::F10 as u32,
                         mods: 0, repeat: true },
            Input::Text("é日".into()),
            Input::Wheel { x: 5, y: 6, steps: -2 },
        ];
        for input in events {
            assert_eq!(to_event(&input).and_then(|e| to_input(&e)), Some(input.clone()), "{input:?}");
        }
        assert_eq!(to_event(&Input::Drop { x: 0, y: 0, paths: vec!["/a".into()] }), None);
        assert_eq!(to_input(&Event::Quit { timestamp: 0 }), None);
    }

    #[test]
    fn drops_keep_to_the_protocol_limits() {
        use crate::desktop::proto::{MAX_DROP_FILES, MAX_DROP_PATH};
        assert_eq!(drop_input(1, 2, vec![]), None);
        assert_eq!(drop_input(1, 2, vec!["x".repeat(MAX_DROP_PATH + 1)]), None, "a cut path would name another file");
        let Some(Input::Drop { x: 1, y: 2, paths }) = drop_input(1, 2, vec!["/a".into(); MAX_DROP_FILES + 3]) else { panic!() };
        assert_eq!(paths.len(), MAX_DROP_FILES);
    }

    #[test]
    fn system_pointers_have_stable_indices() {
        for (i, p) in SYSTEM_POINTERS.iter().enumerate() {
            assert_eq!(pointer_index(*p) as usize, i);
        }
    }

    #[test]
    fn wheel_notches_count_up_positive() {
        use sdl3::mouse::MouseWheelDirection::{Flipped, Normal};
        assert_eq!(wheel_steps(2.0, 2, Normal), 2);
        assert_eq!(wheel_steps(-1.0, -1, Flipped), 1, "a flipped wheel turned back");
        assert_eq!(wheel_steps(0.25, 0, Normal), 1, "a fractional turn is a notch");
        assert_eq!(wheel_steps(0.0, 0, Normal), 0);
        assert_eq!(wheel_steps(500.0, 500, Normal), crate::desktop::proto::MAX_WHEEL_STEPS);
    }
}
