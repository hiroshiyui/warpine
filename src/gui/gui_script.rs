// SPDX-License-Identifier: GPL-3.0-or-later

//! Scripted GUI sessions for automated testing.
//!
//! When `WARPINE_GUI_SCRIPT` names a script file, the PM renderer replays it
//! against the virtual desktop: it injects mouse and keyboard input, writes
//! the composited desktop to image files, and can end the session. Combined
//! with `SDL_VIDEODRIVER=dummy` this drives a PM application with no display,
//! which is how the GUI regression tests in `tests/integration.rs` work.
//!
//! This module is free of SDL types. A script line is parsed into an
//! [`Action`] (what the user does), which is expanded into primitive
//! [`Input`]s (what the input devices report); the renderer translates those
//! into SDL events.
//!
//! # Script format
//!
//! One step per line, `<milliseconds since start> <action> [arguments]`.
//! Blank lines and lines starting with `#` are ignored.
//!
//! ```text
//! # open the File menu, pick the first item, capture, exit
//! 800  click 52 62
//! 1600 click 62 80
//! 2600 dump  /tmp/about.ppm
//! 2700 quit
//! ```
//!
//! Coordinates are desktop pixels, origin top-left.
//!
//! * `click X Y` — left button press and release at `(X, Y)`.
//! * `[MOD+]...click X Y` — the same with modifier keys held, e.g.
//!   `shift+click 10 20`: the modifiers go down before the press and up
//!   after the release.
//! * `rclick X Y` — right button press and release at `(X, Y)`.
//! * `wheel X Y N` — the mouse wheel turned `N` notches (up positive,
//!   within ±64) with the pointer at `(X, Y)`.
//! * `dblclick X Y` — two left clicks at `(X, Y)`, the second press
//!   reported as the second click of a double click.
//! * `drag X1 Y1 X2 Y2` — left button down at `(X1, Y1)`, [`DRAG_MOVES`]
//!   evenly spaced mouse moves ending at `(X2, Y2)`, button up there. The
//!   events are [`DRAG_STEP_MS`] apart, so a drag takes
//!   `(DRAG_MOVES + 1) * DRAG_STEP_MS` = 100 ms.
//! * `rdrag X1 Y1 X2 Y2` — the same with the right button, OS/2's drag
//!   button; `[MOD+]...rdrag` holds modifier keys until the release (`ctrl+rdrag`
//!   copies in a PM drag).
//! * `drop X Y PATH` — host file `PATH` (the rest of the line) dropped on
//!   the desktop at `(X, Y)`, as a file manager drops it.
//! * `key [MOD+]...NAME` — press and release one key. `NAME` is one of
//!   `enter`, `tab`, `esc`, `backspace`, `delete`, `insert`, `left`, `right`,
//!   `up`, `down`, `home`, `end`, `pageup`, `pagedown`, `space`, `f1`..`f12`
//!   (case-insensitive), or a single printable ASCII character (`key a`).
//!   Modifiers are `shift+`, `ctrl+` and `alt+`, e.g. `key alt+f4`,
//!   `key ctrl+shift+tab`. A character that needs Shift on a US keyboard
//!   (`key A`, `key ?`) implies `shift+`. Modifier keys are pressed before
//!   and released after the key, as on a real keyboard.
//! * `text STRING` — type the rest of the line, one character every
//!   [`TEXT_CHAR_MS`]. An ASCII character is the US-layout keystroke that
//!   produces it plus the text-input event carrying the character; any other
//!   character arrives as a text-input event alone, the way an input method
//!   commits it. Leading and trailing blanks are not part of the string (use
//!   `key space`).
//! * `dump PATH` — write the desktop as a binary PPM (P6) image. The path is
//!   the rest of the line.
//! * `quit` — end the session as if the desktop window had been closed.
//!
//! Steps never interleave: a step whose time falls inside a preceding `drag`
//! or `text` starts when that one has finished.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Environment variable naming the script file.
pub const SCRIPT_ENV: &str = "WARPINE_GUI_SCRIPT";

/// Number of mouse-move events a `drag` generates between press and release.
pub const DRAG_MOVES: u64 = 4;
/// Milliseconds between the successive events of a `drag`.
pub const DRAG_STEP_MS: u64 = 20;
/// Milliseconds between the characters of a `text`.
pub const TEXT_CHAR_MS: u64 = 10;
/// Highest function key number accepted by `key fN`.
pub const MAX_FUNCTION_KEY: u8 = 12;

/// A key on a US keyboard, named by what it does rather than by any
/// toolkit's key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Enter, Tab, Esc, Backspace, Delete, Insert,
    Left, Right, Up, Down, Home, End, PageUp, PageDown, Space,
    /// Function key `F1`..`F12`.
    F(u8),
    /// A printable key, identified by the character it types unshifted:
    /// a lowercase letter, a digit, or one of `` ` - = [ ] \ ; ' , . / ``.
    Char(char),
    /// Modifier keys; these appear in [`Input`]s only, never in an [`Action`].
    Shift, Ctrl, Alt,
}

/// Modifier keys held down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button { Left, Right }

/// Names accepted by `key`, lowercase. `f1`..`f12` and single characters are
/// handled by [`key_from_name`].
pub const KEY_NAMES: &[(&str, Key)] = &[
    ("enter", Key::Enter), ("return", Key::Enter),
    ("tab", Key::Tab),
    ("esc", Key::Esc), ("escape", Key::Esc),
    ("backspace", Key::Backspace),
    ("delete", Key::Delete), ("del", Key::Delete),
    ("insert", Key::Insert), ("ins", Key::Insert),
    ("left", Key::Left), ("right", Key::Right), ("up", Key::Up), ("down", Key::Down),
    ("home", Key::Home), ("end", Key::End),
    ("pageup", Key::PageUp), ("pgup", Key::PageUp),
    ("pagedown", Key::PageDown), ("pgdn", Key::PageDown),
    ("space", Key::Space),
];

/// Characters typed with Shift on a US keyboard; the unshifted character of
/// the same key is at the same index in [`US_UNSHIFTED`].
const US_SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
const US_UNSHIFTED: &str = "`1234567890-=[]\\;',./";

/// The US-layout keystroke that types `c`: the key and whether Shift is held.
/// `None` for anything but printable ASCII.
pub fn key_for_char(c: char) -> Option<(Key, bool)> {
    if c == ' ' {
        Some((Key::Space, false))
    } else if c.is_ascii_uppercase() {
        Some((Key::Char(c.to_ascii_lowercase()), true))
    } else if let Some(i) = US_SHIFTED.find(c) {
        // Both tables are ASCII, so byte index == character index.
        Some((Key::Char(US_UNSHIFTED.as_bytes()[i] as char), true))
    } else if c.is_ascii_graphic() {
        Some((Key::Char(c), false))
    } else {
        None
    }
}

/// Resolve a key name as written in a script (without modifier prefixes) to
/// the key and whether typing it needs Shift.
pub fn key_from_name(name: &str) -> Option<(Key, bool)> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return key_for_char(c);
    }
    let lower = name.to_ascii_lowercase();
    if let Some(&(_, key)) = KEY_NAMES.iter().find(|(n, _)| *n == lower) {
        return Some((key, false));
    }
    // `f1`..`f12`, written without sign or leading zero.
    let digits = lower.strip_prefix('f')?;
    if digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u8 = digits.parse().ok()?;
    (1..=MAX_FUNCTION_KEY).contains(&n).then_some((Key::F(n), false))
}

/// Parse the argument of `key`: optional `shift+` / `ctrl+` / `alt+` prefixes
/// and a key name.
pub fn parse_key_spec(spec: &str) -> Option<(Key, Mods)> {
    let mut mods = Mods::default();
    let mut name = spec;
    // A lone remaining character is the key itself, so `ctrl++` is Ctrl and `+`.
    while name.chars().count() > 1 {
        let Some((prefix, rest)) = name.split_once('+') else { break };
        match prefix.to_ascii_lowercase().as_str() {
            "shift" => mods.shift = true,
            "ctrl" => mods.ctrl = true,
            "alt" => mods.alt = true,
            _ => return None,
        }
        name = rest;
    }
    let (key, shifted) = key_from_name(name)?;
    mods.shift |= shifted;
    Some((key, mods))
}

/// Parse `shift+ctrl` style modifier names joined by `+`.
fn parse_mods(spec: &str) -> Option<Mods> {
    let mut mods = Mods::default();
    for name in spec.split('+') {
        match name.to_ascii_lowercase().as_str() {
            "shift" => mods.shift = true,
            "ctrl" => mods.ctrl = true,
            "alt" => mods.alt = true,
            _ => return None,
        }
    }
    Some(mods)
}

/// One scripted action: what the user does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Click { x: i32, y: i32 },
    /// A left click with modifier keys held (`shift+click X Y`).
    ModClick { x: i32, y: i32, mods: Mods },
    RClick { x: i32, y: i32 },
    /// The wheel turned `steps` notches at `(x, y)` (`wheel X Y N`).
    Wheel { x: i32, y: i32, steps: i32 },
    /// Two left clicks in quick succession (`dblclick X Y`).
    DblClick { x: i32, y: i32 },
    Drag { x1: i32, y1: i32, x2: i32, y2: i32 },
    /// A drag with the right button (`rdrag X1 Y1 X2 Y2`): the OS/2 drag
    /// button.
    RDrag { x1: i32, y1: i32, x2: i32, y2: i32 },
    /// A button-2 drag with modifier keys held throughout (`ctrl+rdrag …`).
    ModRDrag { x1: i32, y1: i32, x2: i32, y2: i32, mods: Mods },
    /// A host file dropped on the desktop at `(x, y)` (`drop X Y PATH`),
    /// as a file manager drops it.
    Drop { x: i32, y: i32, path: PathBuf },
    Key { key: Key, mods: Mods },
    Text(String),
    Dump(PathBuf),
    Quit,
}

/// One primitive event of a session: what an input device reports, or a
/// request to the renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    MouseDown { button: Button, x: i32, y: i32 },
    /// The second press of a double click.
    MouseDoubleDown { button: Button, x: i32, y: i32 },
    /// Pointer moved to `(x, y)` by `(dx, dy)` with `button` held.
    MouseDrag { button: Button, x: i32, y: i32, dx: i32, dy: i32 },
    MouseUp { button: Button, x: i32, y: i32 },
    /// `mods` is the modifier state including the effect of this event, so
    /// pressing Shift reports `shift: true` and releasing it `shift: false`.
    KeyDown { key: Key, mods: Mods },
    KeyUp { key: Key, mods: Mods },
    /// Text committed by the keyboard or an input method.
    TextInput(String),
    /// The wheel turned `steps` notches (up positive) at `(x, y)`.
    Wheel { x: i32, y: i32, steps: i32 },
    /// Host file `path` dropped at `(x, y)`.
    Drop { x: i32, y: i32, path: String },
    Dump(PathBuf),
    Quit,
}

/// Append the events of one keystroke: modifiers down, key down, key up,
/// modifiers up in reverse order. `between` goes between key down and key up.
fn push_keystroke(out: &mut Vec<(u64, Input)>, at: u64, key: Key, mods: Mods, between: Option<Input>) {
    push_with_mods(out, at, mods, |out| {
        out.push((at, Input::KeyDown { key, mods }));
        out.extend(between.map(|i| (at, i)));
        out.push((at, Input::KeyUp { key, mods }));
    });
}

/// Append the modifier keys of `mods` going down, the events `inner` adds,
/// then the modifiers going up in reverse order.
fn push_with_mods(out: &mut Vec<(u64, Input)>, at: u64, mods: Mods,
                  inner: impl FnOnce(&mut Vec<(u64, Input)>)) {
    push_with_mods_until(out, at, at, mods, inner);
}

/// [`push_with_mods`] with the modifiers going up at `end` ms.
fn push_with_mods_until(out: &mut Vec<(u64, Input)>, at: u64, end: u64, mods: Mods,
                        inner: impl FnOnce(&mut Vec<(u64, Input)>)) {
    let held: Vec<Key> = [(mods.ctrl, Key::Ctrl), (mods.alt, Key::Alt), (mods.shift, Key::Shift)]
        .into_iter().filter_map(|(on, k)| on.then_some(k)).collect();
    let set = |m: &mut Mods, k: Key, on: bool| match k {
        Key::Ctrl => m.ctrl = on,
        Key::Alt => m.alt = on,
        _ => m.shift = on,
    };
    let mut state = Mods::default();
    for &k in &held {
        set(&mut state, k, true);
        out.push((at, Input::KeyDown { key: k, mods: state }));
    }
    inner(out);
    for &k in held.iter().rev() {
        set(&mut state, k, false);
        out.push((end, Input::KeyUp { key: k, mods: state }));
    }
}

/// Append a drag with `button` from `(x1, y1)` to `(x2, y2)`: the press,
/// `DRAG_MOVES` moves `DRAG_STEP_MS` apart, the release.
fn push_drag(out: &mut Vec<(u64, Input)>, at: u64, button: Button, (x1, y1): (i32, i32), (x2, y2): (i32, i32)) {
    out.push((at, Input::MouseDown { button, x: x1, y: y1 }));
    let n = DRAG_MOVES as i64;
    // 64-bit so that extreme coordinates cannot overflow.
    let lerp = |a: i32, b: i32, i: i64| (a as i64 + (b as i64 - a as i64) * i / n) as i32;
    for i in 1..=n {
        let (px, py) = (lerp(x1, x2, i - 1), lerp(y1, y2, i - 1));
        let (x, y) = (lerp(x1, x2, i), lerp(y1, y2, i));
        out.push((at + i as u64 * DRAG_STEP_MS, Input::MouseDrag {
            button, x, y, dx: x.wrapping_sub(px), dy: y.wrapping_sub(py) }));
    }
    out.push((at + (DRAG_MOVES + 1) * DRAG_STEP_MS, Input::MouseUp { button, x: x2, y: y2 }));
}

impl Action {
    /// The primitive events this action consists of, starting at `at` ms,
    /// each with the time it becomes due. Times never decrease.
    pub fn expand(&self, at: u64) -> Vec<(u64, Input)> {
        let mut out = Vec::new();
        let mut press_release = |button, x, y| {
            out.push((at, Input::MouseDown { button, x, y }));
            out.push((at, Input::MouseUp { button, x, y }));
        };
        match self {
            &Action::Click { x, y } => press_release(Button::Left, x, y),
            &Action::ModClick { x, y, mods } => push_with_mods(&mut out, at, mods, |out| {
                out.push((at, Input::MouseDown { button: Button::Left, x, y }));
                out.push((at, Input::MouseUp { button: Button::Left, x, y }));
            }),
            &Action::RClick { x, y } => press_release(Button::Right, x, y),
            &Action::Wheel { x, y, steps } => out.push((at, Input::Wheel { x, y, steps })),
            &Action::DblClick { x, y } => {
                let button = Button::Left;
                out.push((at, Input::MouseDown { button, x, y }));
                out.push((at, Input::MouseUp { button, x, y }));
                out.push((at, Input::MouseDoubleDown { button, x, y }));
                out.push((at, Input::MouseUp { button, x, y }));
            }
            &Action::Drag { x1, y1, x2, y2 } => push_drag(&mut out, at, Button::Left, (x1, y1), (x2, y2)),
            &Action::RDrag { x1, y1, x2, y2 } => push_drag(&mut out, at, Button::Right, (x1, y1), (x2, y2)),
            &Action::ModRDrag { x1, y1, x2, y2, mods } => {
                // The keys stay down until the button comes up.
                let end = at + (DRAG_MOVES + 1) * DRAG_STEP_MS;
                push_with_mods_until(&mut out, at, end, mods, |out| {
                    push_drag(out, at, Button::Right, (x1, y1), (x2, y2));
                });
            }
            Action::Drop { x, y, path } => {
                out.push((at, Input::Drop { x: *x, y: *y, path: path.to_string_lossy().into_owned() }));
            }
            &Action::Key { key, mods } => push_keystroke(&mut out, at, key, mods, None),
            Action::Text(text) => {
                for (i, c) in text.chars().enumerate() {
                    let t = at + i as u64 * TEXT_CHAR_MS;
                    let typed = Input::TextInput(c.to_string());
                    match key_for_char(c) {
                        Some((key, shift)) => {
                            let mods = Mods { shift, ..Mods::default() };
                            push_keystroke(&mut out, t, key, mods, Some(typed));
                        }
                        // No key types this on a US keyboard: an input
                        // method commits it without key events.
                        None => out.push((t, typed)),
                    }
                }
            }
            Action::Dump(path) => out.push((at, Input::Dump(path.clone()))),
            Action::Quit => out.push((at, Input::Quit)),
        }
        out
    }
}

/// An [`Action`] and the time, in milliseconds since the session started, at
/// which it becomes due.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub at_ms: u64,
    pub action: Action,
}

/// Split the first whitespace-delimited word off `s`; the rest is trimmed.
fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (&s[..end], s[end..].trim())
}

/// Parse exactly `N` integer arguments.
fn coords<const N: usize>(args: &str) -> Option<[i32; N]> {
    let mut out = [0; N];
    let mut words = args.split_whitespace();
    for slot in &mut out {
        *slot = words.next()?.parse().ok()?;
    }
    words.next().is_none().then_some(out)
}

/// Parse a script. Steps are returned ordered by time; steps with equal times
/// keep their order in the file. Errors name the offending line.
pub fn parse(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let err = |what: &str| format!("line {}: {what}: `{line}`", idx + 1);
        let (time, rest) = split_word(line);
        let at_ms = time.parse::<u64>().map_err(|_| err("expected a time in milliseconds"))?;
        let (name, args) = split_word(rest);
        let action = match name {
            "click" => coords(args).map(|[x, y]| Action::Click { x, y })
                .ok_or_else(|| err("click needs X and Y"))?,
            _ if name.ends_with("+click") => {
                let mods = parse_mods(&name[..name.len() - "+click".len()])
                    .ok_or_else(|| err("unknown modifier (expected shift, ctrl or alt)"))?;
                coords(args).map(|[x, y]| Action::ModClick { x, y, mods })
                    .ok_or_else(|| err("click needs X and Y"))?
            }
            "rclick" => coords(args).map(|[x, y]| Action::RClick { x, y })
                .ok_or_else(|| err("rclick needs X and Y"))?,
            "wheel" => coords(args).filter(|[_, _, n]| (-64..=64).contains(n))
                .map(|[x, y, steps]| Action::Wheel { x, y, steps })
                .ok_or_else(|| err("wheel needs X Y and notches from -64 to 64"))?,
            "dblclick" => coords(args).map(|[x, y]| Action::DblClick { x, y })
                .ok_or_else(|| err("dblclick needs X and Y"))?,
            "drag" => coords(args).map(|[x1, y1, x2, y2]| Action::Drag { x1, y1, x2, y2 })
                .ok_or_else(|| err("drag needs X1 Y1 X2 Y2"))?,
            "rdrag" => coords(args).map(|[x1, y1, x2, y2]| Action::RDrag { x1, y1, x2, y2 })
                .ok_or_else(|| err("rdrag needs X1 Y1 X2 Y2"))?,
            _ if name.ends_with("+rdrag") => {
                let mods = parse_mods(&name[..name.len() - "+rdrag".len()])
                    .ok_or_else(|| err("unknown modifier (expected shift, ctrl or alt)"))?;
                coords(args).map(|[x1, y1, x2, y2]| Action::ModRDrag { x1, y1, x2, y2, mods })
                    .ok_or_else(|| err("rdrag needs X1 Y1 X2 Y2"))?
            }
            // The path is the rest of the line, so it may contain spaces.
            "drop" => {
                let (x, rest) = split_word(args);
                let (y, path) = split_word(rest);
                match (x.parse(), y.parse()) {
                    (Ok(x), Ok(y)) if !path.is_empty() => Action::Drop { x, y, path: PathBuf::from(path) },
                    _ => return Err(err("drop needs X Y and a host file path")),
                }
            }
            "key" => {
                if args.is_empty() || args.contains(char::is_whitespace) {
                    return Err(err("key needs one key name"));
                }
                let (key, mods) = parse_key_spec(args).ok_or_else(|| err(
                    "unknown key (expected [shift+|ctrl+|alt+]NAME, NAME a key name such as \
                     enter, esc, left, f1 or one printable ASCII character)"))?;
                Action::Key { key, mods }
            }
            // The text and the path are the rest of the line, so they may
            // contain spaces.
            "text" if args.is_empty() => return Err(err("text needs a string")),
            "text" => Action::Text(args.to_string()),
            "dump" if args.is_empty() => return Err(err("dump needs a file path")),
            "dump" => Action::Dump(PathBuf::from(args)),
            "quit" if args.is_empty() => Action::Quit,
            "quit" => return Err(err("unexpected extra arguments")),
            _ => return Err(err(
                "unknown action (expected click, MOD+click, rclick, dblclick, drag, key, text, dump or quit)")),
        };
        steps.push(Step { at_ms, action });
    }
    steps.sort_by_key(|s| s.at_ms);
    Ok(steps)
}

/// A script being replayed.
pub struct GuiScript {
    /// Primitive events with the time each becomes due, in order.
    pending: VecDeque<(u64, Input)>,
    /// Set on the first poll, so start-up time before the event loop runs
    /// does not eat into the script's timeline.
    started: Option<Instant>,
}

impl GuiScript {
    /// `steps` must be ordered by time, as [`parse`] returns them.
    pub fn new(steps: Vec<Step>) -> Self {
        let mut pending = VecDeque::new();
        // A multi-event step (drag, text) runs to completion before the next
        // step starts, so their events never interleave.
        let mut busy_until = 0;
        for step in steps {
            let events = step.action.expand(step.at_ms.max(busy_until));
            busy_until = events.last().map_or(busy_until, |e| e.0);
            pending.extend(events);
        }
        GuiScript { pending, started: None }
    }

    /// Load the script named by [`SCRIPT_ENV`], if set. A script that cannot
    /// be read or parsed is a fatal configuration error for a test run, so it
    /// is reported and the process exits instead of silently running without it.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os(SCRIPT_ENV)?;
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| parse(&text));
        match loaded {
            Ok(steps) => Some(GuiScript::new(steps)),
            Err(e) => {
                eprintln!("warpine: {SCRIPT_ENV}={}: {e}", Path::new(&path).display());
                std::process::exit(2);
            }
        }
    }

    /// Events that have become due, in order.
    pub fn due(&mut self) -> Vec<Input> {
        let elapsed = self.started.get_or_insert_with(Instant::now).elapsed().as_millis() as u64;
        self.due_at(elapsed)
    }

    /// When the next event becomes due, so the GUI loop can sleep until
    /// then; `None` when the script is exhausted. Before the first
    /// [`due`](Self::due) the clock has not started, so the answer is "now".
    pub fn next_due(&self) -> Option<Instant> {
        let at_ms = self.pending.front()?.0;
        Some(match self.started {
            Some(start) => start + std::time::Duration::from_millis(at_ms),
            None => Instant::now(),
        })
    }

    fn due_at(&mut self, elapsed_ms: u64) -> Vec<Input> {
        let mut out = Vec::new();
        while self.pending.front().is_some_and(|e| e.0 <= elapsed_ms) {
            out.extend(self.pending.pop_front().map(|e| e.1));
        }
        out
    }
}

/// Encode `width` × `height` 0x00RRGGBB pixels as a binary PPM (P6) image.
pub fn encode_ppm(pixels: &[u32], width: u32, height: u32) -> Vec<u8> {
    let count = (width as usize * height as usize).min(pixels.len());
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    out.reserve(count * 3);
    for &p in &pixels[..count] {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    // Pad a short buffer so the file always matches its header.
    out.resize(out.len() + (width as usize * height as usize - count) * 3, 0);
    out
}

/// Write a desktop buffer to `path` as a PPM image.
pub fn write_ppm(path: &Path, pixels: &[u32], width: u32, height: u32) -> std::io::Result<()> {
    std::fs::File::create(path)?.write_all(&encode_ppm(pixels, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, ctrl: bool, alt: bool) -> Mods { Mods { shift, ctrl, alt } }

    #[test]
    fn next_due_follows_the_script_clock() {
        let mut script = GuiScript::new(parse("100 click 1 2\n500 quit\n").unwrap());
        // Not started: due now, so the first poll runs at once.
        let before = Instant::now();
        assert!(script.next_due().unwrap() >= before);

        let start = Instant::now();
        script.started = Some(start);
        assert_eq!(script.next_due(), Some(start + std::time::Duration::from_millis(100)));
        assert_eq!(script.due_at(100).len(), 2, "press and release");
        assert_eq!(script.next_due(), Some(start + std::time::Duration::from_millis(500)));
        script.due_at(500);
        assert_eq!(script.next_due(), None, "exhausted");
    }

    #[test]
    fn rdrag_holds_the_right_button() {
        let steps = parse("100 rdrag 10 20 50 60\n").unwrap();
        let events = steps[0].action.expand(100);
        assert_eq!(events[0].1, Input::MouseDown { button: Button::Right, x: 10, y: 20 });
        assert!(events[1..events.len() - 1].iter().all(|(_, e)| matches!(e, Input::MouseDrag { button: Button::Right, .. })));
        assert_eq!(events.last().unwrap().1, Input::MouseUp { button: Button::Right, x: 50, y: 60 });
    }

    #[test]
    fn parse_reads_steps_and_orders_them_by_time() {
        let steps = parse("\
            # comment\n\
            \n\
            2600 dump  /tmp/my shots/about.ppm \n\
            800  click 52 62\n\
            2700 quit\n\
            800  click 1 2\n").unwrap();
        assert_eq!(steps, vec![
            Step { at_ms: 800, action: Action::Click { x: 52, y: 62 } },
            Step { at_ms: 800, action: Action::Click { x: 1, y: 2 } },
            Step { at_ms: 2600, action: Action::Dump(PathBuf::from("/tmp/my shots/about.ppm")) },
            Step { at_ms: 2700, action: Action::Quit },
        ]);
        assert_eq!(parse("").unwrap(), vec![]);
    }

    #[test]
    fn parse_reads_the_mouse_and_keyboard_actions() {
        let actions: Vec<Action> = parse("\
            1 rclick 300 -4\n\
            2 drag 100 42 400 300\n\
            3 key enter\n\
            4 key alt+f4\n\
            5 text Hello, wörld  日本 \n\
            6   dump   /tmp/a b.ppm\n").unwrap().into_iter().map(|s| s.action).collect();
        assert_eq!(actions, vec![
            Action::RClick { x: 300, y: -4 },
            Action::Drag { x1: 100, y1: 42, x2: 400, y2: 300 },
            Action::Key { key: Key::Enter, mods: Mods::default() },
            Action::Key { key: Key::F(4), mods: mods(false, false, true) },
            Action::Text("Hello, wörld  日本".into()),
            // Runs of blanks before the action do not leak into its argument.
            Action::Dump(PathBuf::from("/tmp/a b.ppm")),
        ]);
    }

    #[test]
    fn parse_rejects_malformed_lines_and_names_the_line() {
        for bad in ["soon click 1 2", "10 click 1", "10 click a b", "10 click 1 2 3",
                    "10 dump", "10 quit now", "10 drag 1 2", "10",
                    "10 rclick", "10 rclick 1", "10 rclick 1 2 3", "10 rclick x 2",
                    "10 drag", "10 drag 1 2 3", "10 drag 1 2 3 4 5", "10 drag 1 2 3 x",
                    "10 key", "10 key nosuchkey", "10 key a b", "10 key super+a",
                    "10 key shift+", "10 key f0", "10 key f13", "10 key f01", "10 key é",
                    "10 key ab", "10 key shift+ctrl",
                    "10 text", "10 text   ", "10 doubleclick 1 2", "-5 quit",
                    "10 shift+click 1", "10 super+click 1 2", "10 +click 1 2", "10 shift+rclick 1 2"] {
            let err = parse(&format!("5 quit\n{bad}\n")).unwrap_err();
            assert!(err.starts_with("line 2:"), "{bad:?} → {err}");
            assert!(err.contains(bad.trim()), "the line is quoted: {err}");
        }
        // The message says what is wrong, not only where.
        assert!(parse("1 key bogus").unwrap_err().contains("unknown key"));
        assert!(parse("1 key").unwrap_err().contains("key needs one key name"));
        assert!(parse("1 drag 1 2").unwrap_err().contains("drag needs X1 Y1 X2 Y2"));
        assert!(parse("1 rclick 1").unwrap_err().contains("rclick needs X and Y"));
        assert!(parse("1 text").unwrap_err().contains("text needs a string"));
        assert!(parse("1 fly").unwrap_err().contains("rclick, dblclick, drag, key, text"));
        assert!(parse("1 super+click 1 2").unwrap_err().contains("unknown modifier"));
    }

    #[test]
    fn modifier_clicks_hold_the_keys_around_the_click() {
        assert_eq!(parse("10 shift+click 3 4\n20 Ctrl+SHIFT+click 5 6").unwrap(), vec![
            Step { at_ms: 10, action: Action::ModClick { x: 3, y: 4, mods: mods(true, false, false) } },
            Step { at_ms: 20, action: Action::ModClick { x: 5, y: 6, mods: mods(true, true, false) } },
        ]);
        let (shift, none) = (mods(true, false, false), Mods::default());
        assert_eq!(Action::ModClick { x: 3, y: 4, mods: shift }.expand(50), vec![
            (50, Input::KeyDown { key: Key::Shift, mods: shift }),
            (50, Input::MouseDown { button: Button::Left, x: 3, y: 4 }),
            (50, Input::MouseUp { button: Button::Left, x: 3, y: 4 }),
            (50, Input::KeyUp { key: Key::Shift, mods: none }),
        ]);
    }

    #[test]
    fn key_spec_combines_modifiers_with_names_and_characters() {
        let k = parse_key_spec;
        assert_eq!(k("tab"), Some((Key::Tab, Mods::default())));
        assert_eq!(k("shift+tab"), Some((Key::Tab, mods(true, false, false))));
        assert_eq!(k("Ctrl+ALT+Delete"), Some((Key::Delete, mods(false, true, true))));
        assert_eq!(k("alt+shift+ctrl+f12"), Some((Key::F(12), mods(true, true, true))));
        assert_eq!(k("a"), Some((Key::Char('a'), Mods::default())));
        assert_eq!(k("ctrl+c"), Some((Key::Char('c'), mods(false, true, false))));
        // Characters that need Shift on a US keyboard imply it.
        assert_eq!(k("A"), Some((Key::Char('a'), mods(true, false, false))));
        assert_eq!(k("?"), Some((Key::Char('/'), mods(true, false, false))));
        assert_eq!(k("alt+!"), Some((Key::Char('1'), mods(true, false, true))));
        // `+` is a key too, alone or after a modifier.
        assert_eq!(k("+"), Some((Key::Char('='), mods(true, false, false))));
        assert_eq!(k("ctrl++"), Some((Key::Char('='), mods(true, true, false))));
        assert_eq!(k("ctrl+-"), Some((Key::Char('-'), mods(false, true, false))));
        for bad in ["", "shift", "shift+", "meta+a", "a+b", "tab+shift", "ctrl+nosuch", "ctrl"] {
            assert_eq!(k(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn key_name_table_covers_every_documented_key() {
        for (name, key) in [
            ("enter", Key::Enter), ("tab", Key::Tab), ("esc", Key::Esc),
            ("backspace", Key::Backspace), ("delete", Key::Delete), ("insert", Key::Insert),
            ("left", Key::Left), ("right", Key::Right), ("up", Key::Up), ("down", Key::Down),
            ("home", Key::Home), ("end", Key::End), ("pageup", Key::PageUp),
            ("pagedown", Key::PageDown), ("space", Key::Space),
            ("return", Key::Enter), ("escape", Key::Esc), ("del", Key::Delete),
            ("pgup", Key::PageUp), ("pgdn", Key::PageDown), ("ins", Key::Insert),
            ("PageDown", Key::PageDown), ("ESC", Key::Esc), ("F5", Key::F(5)),
        ] {
            assert_eq!(key_from_name(name), Some((key, false)), "{name}");
        }
        for n in 1..=MAX_FUNCTION_KEY {
            assert_eq!(key_from_name(&format!("f{n}")), Some((Key::F(n), false)));
        }
        for bad in ["f0", "f13", "f1x", "f+1", "f01", "f256", "entr", "", "ß", "日", "shift"] {
            assert_eq!(key_from_name(bad), None, "{bad:?}");
        }
        // Table names are lowercase and unique, or lookups would miss them.
        for (i, (name, _)) in KEY_NAMES.iter().enumerate() {
            assert_eq!(*name, name.to_ascii_lowercase());
            assert!(KEY_NAMES[..i].iter().all(|(n, _)| n != name), "duplicate {name}");
        }
    }

    #[test]
    fn key_for_char_maps_all_printable_ascii_to_a_us_keystroke() {
        assert_eq!(US_SHIFTED.len(), US_UNSHIFTED.len());
        assert_eq!(key_for_char('a'), Some((Key::Char('a'), false)));
        assert_eq!(key_for_char('Z'), Some((Key::Char('z'), true)));
        assert_eq!(key_for_char('7'), Some((Key::Char('7'), false)));
        assert_eq!(key_for_char('&'), Some((Key::Char('7'), true)));
        assert_eq!(key_for_char('"'), Some((Key::Char('\''), true)));
        assert_eq!(key_for_char('\\'), Some((Key::Char('\\'), false)));
        assert_eq!(key_for_char(' '), Some((Key::Space, false)));
        for c in ' '..='~' {
            match key_for_char(c).unwrap_or_else(|| panic!("{c:?} has no keystroke")) {
                (Key::Space, false) => assert_eq!(c, ' '),
                // The key is always named by an unshifted character.
                (Key::Char(k), _) => assert_eq!(key_for_char(k), Some((Key::Char(k), false)), "{c:?}"),
                other => panic!("{c:?} → {other:?}"),
            }
        }
        for c in ['\t', '\n', '\x7f', 'é', '日'] {
            assert_eq!(key_for_char(c), None, "{c:?}");
        }
    }

    #[test]
    fn clicks_expand_to_press_and_release_of_the_right_button() {
        assert_eq!(Action::Click { x: 3, y: 4 }.expand(50), vec![
            (50, Input::MouseDown { button: Button::Left, x: 3, y: 4 }),
            (50, Input::MouseUp { button: Button::Left, x: 3, y: 4 }),
        ]);
        assert_eq!(Action::RClick { x: 3, y: 4 }.expand(50), vec![
            (50, Input::MouseDown { button: Button::Right, x: 3, y: 4 }),
            (50, Input::MouseUp { button: Button::Right, x: 3, y: 4 }),
        ]);
    }

    #[test]
    fn double_clicks_expand_to_a_click_then_a_double_press() {
        assert_eq!(parse("5 dblclick 3 4").unwrap()[0].action, Action::DblClick { x: 3, y: 4 });
        assert!(parse("5 dblclick 3").is_err());
        let button = Button::Left;
        assert_eq!(Action::DblClick { x: 3, y: 4 }.expand(50), vec![
            (50, Input::MouseDown { button, x: 3, y: 4 }),
            (50, Input::MouseUp { button, x: 3, y: 4 }),
            (50, Input::MouseDoubleDown { button, x: 3, y: 4 }),
            (50, Input::MouseUp { button, x: 3, y: 4 }),
        ]);
    }

    #[test]
    fn drag_expands_to_press_spaced_moves_and_release() {
        let events = Action::Drag { x1: 100, y1: 40, x2: 300, y2: 140 }.expand(1000);
        assert_eq!(events, vec![
            (1000, Input::MouseDown { button: Button::Left, x: 100, y: 40 }),
            (1020, Input::MouseDrag { button: Button::Left, x: 150, y: 65, dx: 50, dy: 25 }),
            (1040, Input::MouseDrag { button: Button::Left, x: 200, y: 90, dx: 50, dy: 25 }),
            (1060, Input::MouseDrag { button: Button::Left, x: 250, y: 115, dx: 50, dy: 25 }),
            (1080, Input::MouseDrag { button: Button::Left, x: 300, y: 140, dx: 50, dy: 25 }),
            (1100, Input::MouseUp { button: Button::Left, x: 300, y: 140 }),
        ]);
        // The last move always lands exactly on the target, also when the
        // distance does not divide evenly or runs backwards.
        let events = Action::Drag { x1: 10, y1: 10, x2: 3, y2: 11 }.expand(0);
        assert_eq!(events[DRAG_MOVES as usize], (DRAG_MOVES * DRAG_STEP_MS,
            Input::MouseDrag { button: Button::Left, x: 3, y: 11, dx: -2, dy: 1 }));
        // Extreme coordinates must not overflow.
        let events = Action::Drag { x1: i32::MIN, y1: 0, x2: i32::MAX, y2: 0 }.expand(0);
        assert_eq!(events.len(), DRAG_MOVES as usize + 2);
    }

    #[test]
    fn key_expands_to_modifiers_around_the_keystroke() {
        let plain = Mods::default();
        assert_eq!(Action::Key { key: Key::Enter, mods: plain }.expand(7), vec![
            (7, Input::KeyDown { key: Key::Enter, mods: plain }),
            (7, Input::KeyUp { key: Key::Enter, mods: plain }),
        ]);
        let alt = mods(false, false, true);
        assert_eq!(Action::Key { key: Key::F(4), mods: alt }.expand(7), vec![
            (7, Input::KeyDown { key: Key::Alt, mods: alt }),
            (7, Input::KeyDown { key: Key::F(4), mods: alt }),
            (7, Input::KeyUp { key: Key::F(4), mods: alt }),
            (7, Input::KeyUp { key: Key::Alt, mods: plain }),
        ]);
        // Several modifiers go down in one order and come up in the reverse,
        // each event reporting the state it leaves behind.
        let all = mods(true, true, true);
        let events: Vec<Input> = Action::Key { key: Key::Tab, mods: all }.expand(0)
            .into_iter().map(|e| e.1).collect();
        assert_eq!(events, vec![
            Input::KeyDown { key: Key::Ctrl, mods: mods(false, true, false) },
            Input::KeyDown { key: Key::Alt, mods: mods(false, true, true) },
            Input::KeyDown { key: Key::Shift, mods: all },
            Input::KeyDown { key: Key::Tab, mods: all },
            Input::KeyUp { key: Key::Tab, mods: all },
            Input::KeyUp { key: Key::Shift, mods: mods(false, true, true) },
            Input::KeyUp { key: Key::Alt, mods: mods(false, true, false) },
            Input::KeyUp { key: Key::Ctrl, mods: plain },
        ]);
    }

    #[test]
    fn text_expands_to_keystrokes_with_text_input_and_bare_text_for_non_ascii() {
        let plain = Mods::default();
        let shift = mods(true, false, false);
        assert_eq!(Action::Text("aB 日".into()).expand(100), vec![
            (100, Input::KeyDown { key: Key::Char('a'), mods: plain }),
            (100, Input::TextInput("a".into())),
            (100, Input::KeyUp { key: Key::Char('a'), mods: plain }),
            (110, Input::KeyDown { key: Key::Shift, mods: shift }),
            (110, Input::KeyDown { key: Key::Char('b'), mods: shift }),
            (110, Input::TextInput("B".into())),
            (110, Input::KeyUp { key: Key::Char('b'), mods: shift }),
            (110, Input::KeyUp { key: Key::Shift, mods: plain }),
            (120, Input::KeyDown { key: Key::Space, mods: plain }),
            (120, Input::TextInput(" ".into())),
            (120, Input::KeyUp { key: Key::Space, mods: plain }),
            // Non-ASCII: committed text only, the whole character in one event.
            (130, Input::TextInput("日".into())),
        ]);
    }

    #[test]
    fn due_releases_each_event_once_when_its_time_has_come() {
        let mut script = GuiScript::new(parse("100 rclick 1 1\n200 quit\n200 click 2 2\n").unwrap());
        assert!(script.due_at(99).is_empty());
        assert_eq!(script.due_at(100), vec![
            Input::MouseDown { button: Button::Right, x: 1, y: 1 },
            Input::MouseUp { button: Button::Right, x: 1, y: 1 },
        ]);
        assert!(script.due_at(150).is_empty());
        assert_eq!(script.due_at(5000), vec![
            Input::Quit,
            Input::MouseDown { button: Button::Left, x: 2, y: 2 },
            Input::MouseUp { button: Button::Left, x: 2, y: 2 },
        ]);
        assert!(script.due_at(9000).is_empty());
    }

    #[test]
    fn a_drag_is_released_over_several_polls_and_is_never_interleaved() {
        // The click is scheduled in the middle of the drag; it must wait.
        let mut script = GuiScript::new(parse("100 drag 0 0 40 0\n150 click 9 9\n").unwrap());
        assert_eq!(script.due_at(100), vec![Input::MouseDown { button: Button::Left, x: 0, y: 0 }]);
        assert_eq!(script.due_at(120), vec![Input::MouseDrag { button: Button::Left, x: 10, y: 0, dx: 10, dy: 0 }]);
        assert_eq!(script.due_at(165), vec![
            Input::MouseDrag { button: Button::Left, x: 20, y: 0, dx: 10, dy: 0 },
            Input::MouseDrag { button: Button::Left, x: 30, y: 0, dx: 10, dy: 0 },
        ]);
        // A late poll still delivers everything, in order.
        assert_eq!(script.due_at(10_000), vec![
            Input::MouseDrag { button: Button::Left, x: 40, y: 0, dx: 10, dy: 0 },
            Input::MouseUp { button: Button::Left, x: 40, y: 0 },
            Input::MouseDown { button: Button::Left, x: 9, y: 9 },
            Input::MouseUp { button: Button::Left, x: 9, y: 9 },
        ]);
    }

    #[test]
    fn encode_ppm_writes_header_and_rgb_triples() {
        let ppm = encode_ppm(&[0x00FF_8040, 0xFF00_0001], 2, 1);
        assert_eq!(ppm, b"P6\n2 1\n255\n\xFF\x80\x40\x00\x00\x01");
        // A buffer shorter than the header claims is padded, never over-read.
        assert_eq!(encode_ppm(&[0x0012_3456], 2, 1).len(), b"P6\n2 1\n255\n".len() + 6);
    }

    #[test]
    fn wheel_steps_are_parsed_and_expanded() {
        let s = parse("10 wheel 100 200 -2\n").unwrap();
        assert_eq!(s[0].action, Action::Wheel { x: 100, y: 200, steps: -2 });
        assert_eq!(s[0].action.expand(10), vec![(10, Input::Wheel { x: 100, y: 200, steps: -2 })]);
        assert!(parse("1 wheel 1 2").unwrap_err().contains("wheel needs X Y"));
        assert!(parse("1 wheel 1 2 65").is_err(), "at most 64 notches");
    }

    #[test]
    fn drops_and_modified_drags_are_parsed_and_expanded() {
        let s = parse("10 drop 5 6 /tmp/a b.txt\n").unwrap();
        assert_eq!(s[0].action, Action::Drop { x: 5, y: 6, path: PathBuf::from("/tmp/a b.txt") });
        assert_eq!(s[0].action.expand(10), vec![(10, Input::Drop { x: 5, y: 6, path: "/tmp/a b.txt".into() })]);
        assert!(parse("1 drop 1 2").unwrap_err().contains("drop needs X Y"));
        assert!(parse("1 drop x 2 /a").is_err());

        let s = parse("100 ctrl+rdrag 1 2 9 2\n").unwrap();
        let ev = s[0].action.expand(100);
        assert!(matches!(ev[0], (100, Input::KeyDown { key: Key::Ctrl, .. })));
        assert!(matches!(ev[1], (100, Input::MouseDown { button: Button::Right, x: 1, y: 2 })));
        let n = ev.len();
        assert!(matches!(ev[n - 2], (_, Input::MouseUp { button: Button::Right, x: 9, y: 2 })));
        assert!(matches!(ev[n - 1], (_, Input::KeyUp { key: Key::Ctrl, .. })), "Ctrl held until the release");
        assert!(ev.windows(2).all(|w| w[0].0 <= w[1].0), "times never decrease");
        assert!(parse("1 hyper+rdrag 1 2 3 4").is_err());
    }
}
