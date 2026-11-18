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
        _ => return None,
    })
}

/// The SDL event for protocol input `input`.
pub fn to_event(input: &Input) -> Event {
    let (timestamp, window_id, which) = (0, 0, 0);
    match input {
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
    }
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
        ];
        for input in events {
            assert_eq!(to_input(&to_event(&input)), Some(input.clone()), "{input:?}");
        }
        assert_eq!(to_input(&Event::Quit { timestamp: 0 }), None);
    }

    #[test]
    fn system_pointers_have_stable_indices() {
        for (i, p) in SYSTEM_POINTERS.iter().enumerate() {
            assert_eq!(pointer_index(*p) as usize, i);
        }
    }
}
