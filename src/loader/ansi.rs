// SPDX-License-Identifier: GPL-3.0-only
//
// ANSI escape-sequence parser for VIO TTY output.
//
// OS/2 interprets ANSI escape sequences in text written with `VioWrtTTY`
// (and so `DosWrite` to the console) while ANSI support is on — the default,
// switched by `VioSetAnsi` and the `ANSI ON` / `ANSI OFF` command. The
// sequences OS/2 documents (OS/2 FAQ 5.12 "ANSI Escape Sequences", REXX Tips
// & Tricks "ANSI ESC Sequences"):
//
//   ESC[#;#H / ESC[#;#f  cursor position        ESC[#A/B/C/D  cursor up/down/forward/back
//   ESC[s / ESC[u        save / restore cursor  ESC[6n        report the cursor position
//   ESC[2J               clear screen, home     ESC[K         erase to end of line
//   ESC[#;...;#m         graphics rendition     ESC[=#h / l   set / reset a mode (7 = wrap)
//   ESC[#;...;#p         keyboard reassignment
//
// This module is only the byte-level state machine: it turns a byte stream
// into text bytes and complete control sequences, keeping its state between
// calls so a sequence split across two writes still works. `VioManager`
// (console.rs) carries the sequences out on the screen buffer.

/// The escape character that starts every sequence.
pub const ESC: u8 = 0x1B;
/// Second byte of a control sequence (Control Sequence Introducer `ESC[`).
const CSI_INTRODUCER: u8 = b'[';
/// Parameter separator.
const PARAM_SEPARATOR: u8 = b';';
/// Quote that brackets a string parameter (keyboard reassignment).
const STRING_QUOTE: u8 = b'"';
/// At most this many parameters are kept; further ones are dropped.
pub const MAX_PARAMS: usize = 16;
/// A sequence longer than this (quoted strings included) is abandoned, so a
/// stray `ESC[` cannot swallow the rest of the output.
pub const MAX_SEQUENCE_LEN: usize = 256;
/// Numeric parameters saturate here (screen coordinates are far smaller).
pub const MAX_PARAM_VALUE: u16 = 9999;

/// A complete control sequence `ESC[` [private] params final.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Csi {
    /// `=` or `?` directly after `ESC[` (mode set / reset), else `None`.
    pub private: Option<u8>,
    /// Numeric parameters; `None` where a parameter was left empty.
    pub params: Vec<Option<u16>>,
    /// The final byte (`A`, `H`, `m`, …).
    pub final_byte: u8,
}

impl Csi {
    /// Parameter `i`, or `default` when it is missing or empty.
    pub fn param(&self, i: usize, default: u16) -> u16 {
        self.params.get(i).copied().flatten().unwrap_or(default)
    }

    /// Parameter `i` as a count or coordinate: missing, empty or 0 mean 1.
    pub fn count(&self, i: usize) -> u16 {
        self.param(i, 1).max(1)
    }
}

/// What one input byte amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feed {
    /// Part of an unfinished sequence; nothing to do yet.
    Pending,
    /// An ordinary byte (printable or control) to write as text.
    Text(u8),
    /// An `ESC` that did not start a control sequence: write the `ESC` as a
    /// character, then the byte that followed it.
    EscThen(u8),
    /// A complete control sequence.
    Sequence(Csi),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// After `ESC`.
    Escape,
    /// Inside `ESC[` …; `in_string` while between `"` quotes.
    Csi { in_string: bool },
}

/// Byte-level ANSI parser; keeps its state across writes.
#[derive(Debug, Clone, Default)]
pub struct AnsiParser {
    state: State,
    private: Option<u8>,
    params: Vec<Option<u16>>,
    current: Option<u16>,
    len: usize,
}

impl AnsiParser {
    pub fn new() -> Self { Self::default() }

    /// Whether a sequence is under way.
    pub fn in_sequence(&self) -> bool { self.state != State::Ground }

    /// Forget an unfinished sequence (`VioSetAnsi`).
    pub fn reset(&mut self) { *self = Self::default(); }

    fn begin_csi(&mut self) {
        self.state = State::Csi { in_string: false };
        self.private = None;
        self.params.clear();
        self.current = None;
        self.len = 2;
    }

    fn push_param(&mut self) {
        if self.params.len() < MAX_PARAMS {
            self.params.push(self.current);
        }
        self.current = None;
    }

    /// Feed one byte.
    pub fn feed(&mut self, b: u8) -> Feed {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Escape;
                    Feed::Pending
                } else {
                    Feed::Text(b)
                }
            }
            State::Escape => {
                if b == CSI_INTRODUCER {
                    self.begin_csi();
                    Feed::Pending
                } else if b == ESC {
                    // The first ESC was a lone character; the second may
                    // start a sequence.
                    Feed::Text(ESC)
                } else {
                    self.state = State::Ground;
                    Feed::EscThen(b)
                }
            }
            State::Csi { in_string } => {
                self.len += 1;
                if self.len > MAX_SEQUENCE_LEN {
                    self.reset();
                    return Feed::Pending;
                }
                if in_string {
                    if b == STRING_QUOTE {
                        self.state = State::Csi { in_string: false };
                    }
                    return Feed::Pending;
                }
                match b {
                    b'0'..=b'9' => {
                        let digit = (b - b'0') as u16;
                        let v = self.current.unwrap_or(0).saturating_mul(10).saturating_add(digit);
                        self.current = Some(v.min(MAX_PARAM_VALUE));
                        Feed::Pending
                    }
                    PARAM_SEPARATOR => { self.push_param(); Feed::Pending }
                    b'=' | b'?' if self.len == 3 => { self.private = Some(b); Feed::Pending }
                    STRING_QUOTE => {
                        self.state = State::Csi { in_string: true };
                        Feed::Pending
                    }
                    // Intermediate bytes: accepted and ignored.
                    0x20..=0x2F => Feed::Pending,
                    0x40..=0x7E => {
                        if self.current.is_some() || !self.params.is_empty() {
                            self.push_param();
                        }
                        let csi = Csi {
                            private: self.private,
                            params: std::mem::take(&mut self.params),
                            final_byte: b,
                        };
                        self.reset();
                        Feed::Sequence(csi)
                    }
                    ESC => {
                        // A new sequence interrupts the unfinished one.
                        self.reset();
                        self.state = State::Escape;
                        Feed::Pending
                    }
                    // Anything else (a control character, a stray byte)
                    // abandons the sequence; the byte is written as text.
                    _ => {
                        self.reset();
                        Feed::Text(b)
                    }
                }
            }
        }
    }
}

/// ANSI colour number (30–37 / 40–47 minus the base) → CGA colour index of
/// the VIO attribute (black, red, green, yellow/brown, blue, magenta, cyan, white).
pub const ANSI_TO_CGA: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];

/// SGR parameters OS/2 documents.
pub const SGR_RESET: u16 = 0;
pub const SGR_BOLD: u16 = 1;
pub const SGR_NORMAL: u16 = 2;
pub const SGR_UNDERLINE: u16 = 4;
pub const SGR_BLINK: u16 = 5;
pub const SGR_REVERSE: u16 = 7;
pub const SGR_CONCEALED: u16 = 8;
pub const SGR_FG_FIRST: u16 = 30;
pub const SGR_FG_LAST: u16 = 37;
pub const SGR_BG_FIRST: u16 = 40;
pub const SGR_BG_LAST: u16 = 47;

/// VIO attribute bits.
const ATTR_FG_MASK: u8 = 0x07;
const ATTR_INTENSITY: u8 = 0x08;
const ATTR_BG_SHIFT: u8 = 4;
const ATTR_BLINK: u8 = 0x80;
/// The default attribute: light grey on black.
pub const DEFAULT_ATTR: u8 = 0x07;

/// The graphics rendition set by SGR sequences, from which the VIO
/// attribute of TTY output is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rendition {
    pub fg: u8,
    pub bg: u8,
    pub bold: bool,
    pub blink: bool,
    pub reverse: bool,
    pub concealed: bool,
}

impl Default for Rendition {
    fn default() -> Self { Self::from_attr(DEFAULT_ATTR) }
}

impl Rendition {
    /// The rendition that produces `attr`.
    pub fn from_attr(attr: u8) -> Self {
        Rendition {
            fg: attr & ATTR_FG_MASK,
            bg: (attr >> ATTR_BG_SHIFT) & ATTR_FG_MASK,
            bold: attr & ATTR_INTENSITY != 0,
            blink: attr & ATTR_BLINK != 0,
            reverse: false,
            concealed: false,
        }
    }

    /// The VIO attribute byte.
    pub fn attr(&self) -> u8 {
        let (mut fg, bg) = if self.reverse { (self.bg, self.fg) } else { (self.fg, self.bg) };
        if self.concealed { fg = bg; }
        let mut a = fg | (bg << ATTR_BG_SHIFT);
        if self.bold { a |= ATTR_INTENSITY; }
        if self.blink { a |= ATTR_BLINK; }
        a
    }

    /// Apply the parameters of an `ESC[...m` sequence (none = reset).
    pub fn apply_sgr(&mut self, params: &[Option<u16>]) {
        if params.is_empty() {
            *self = Self::default();
            return;
        }
        for p in params {
            match p.unwrap_or(SGR_RESET) {
                SGR_RESET => *self = Self::default(),
                SGR_BOLD => self.bold = true,
                SGR_NORMAL => self.bold = false,
                // Underline exists on monochrome displays only; a colour
                // attribute has no underline bit.
                SGR_UNDERLINE => {}
                SGR_BLINK => self.blink = true,
                SGR_REVERSE => self.reverse = true,
                SGR_CONCEALED => self.concealed = true,
                n @ SGR_FG_FIRST..=SGR_FG_LAST => self.fg = ANSI_TO_CGA[(n - SGR_FG_FIRST) as usize],
                n @ SGR_BG_FIRST..=SGR_BG_LAST => self.bg = ANSI_TO_CGA[(n - SGR_BG_FIRST) as usize],
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(p: &mut AnsiParser, bytes: &[u8]) -> Vec<Feed> {
        bytes.iter().map(|&b| p.feed(b)).filter(|f| *f != Feed::Pending).collect()
    }

    fn csi(private: Option<u8>, params: &[Option<u16>], final_byte: u8) -> Feed {
        Feed::Sequence(Csi { private, params: params.to_vec(), final_byte })
    }

    #[test]
    fn plain_text_passes_through() {
        let mut p = AnsiParser::new();
        assert_eq!(feed_all(&mut p, b"ab\r\n"),
                   vec![Feed::Text(b'a'), Feed::Text(b'b'), Feed::Text(b'\r'), Feed::Text(b'\n')]);
    }

    #[test]
    fn clear_screen_sequence() {
        let mut p = AnsiParser::new();
        assert_eq!(feed_all(&mut p, b"\x1b[2J"), vec![csi(None, &[Some(2)], b'J')]);
        assert!(!p.in_sequence());
    }

    #[test]
    fn cursor_position_with_two_and_empty_params() {
        let mut p = AnsiParser::new();
        assert_eq!(feed_all(&mut p, b"\x1b[5;10H"), vec![csi(None, &[Some(5), Some(10)], b'H')]);
        assert_eq!(feed_all(&mut p, b"\x1b[;7f"), vec![csi(None, &[None, Some(7)], b'f')]);
        assert_eq!(feed_all(&mut p, b"\x1b[H"), vec![csi(None, &[], b'H')]);
    }

    #[test]
    fn sequence_split_across_writes() {
        let mut p = AnsiParser::new();
        assert_eq!(feed_all(&mut p, b"x\x1b"), vec![Feed::Text(b'x')]);
        assert!(p.in_sequence());
        assert_eq!(feed_all(&mut p, b"[1;3"), vec![]);
        assert_eq!(feed_all(&mut p, b"1my"), vec![csi(None, &[Some(1), Some(31)], b'm'), Feed::Text(b'y')]);
    }

    #[test]
    fn mode_set_and_reset_keep_the_private_marker() {
        let mut p = AnsiParser::new();
        assert_eq!(feed_all(&mut p, b"\x1b[=7l\x1b[?7h"),
                   vec![csi(Some(b'='), &[Some(7)], b'l'), csi(Some(b'?'), &[Some(7)], b'h')]);
    }

    #[test]
    fn lone_escape_is_text() {
        let mut p = AnsiParser::new();
        assert_eq!(feed_all(&mut p, b"\x1bA"), vec![Feed::EscThen(b'A')]);
        // ESC ESC [ K: the first ESC is a character, the second starts a sequence.
        assert_eq!(feed_all(&mut p, b"\x1b\x1b[K"), vec![Feed::Text(ESC), csi(None, &[], b'K')]);
    }

    #[test]
    fn control_character_abandons_a_sequence() {
        let mut p = AnsiParser::new();
        assert_eq!(feed_all(&mut p, b"\x1b[12\nz"), vec![Feed::Text(b'\n'), Feed::Text(b'z')]);
    }

    #[test]
    fn quoted_strings_are_skipped() {
        let mut p = AnsiParser::new();
        assert_eq!(feed_all(&mut p, b"\x1b[0;68;\"dir;m\";13p"),
                   vec![csi(None, &[Some(0), Some(68), None, Some(13)], b'p')]);
    }

    #[test]
    fn parameters_and_length_are_bounded() {
        let mut p = AnsiParser::new();
        assert_eq!(feed_all(&mut p, b"\x1b[99999999A"), vec![csi(None, &[Some(MAX_PARAM_VALUE)], b'A')]);
        let many: Vec<u8> = b"\x1b[".iter().copied()
            .chain(std::iter::repeat_n(b"1;".iter().copied(), 40).flatten())
            .chain(*b"m").collect();
        match &feed_all(&mut p, &many)[..] {
            [Feed::Sequence(c)] => assert_eq!(c.params.len(), MAX_PARAMS),
            other => panic!("{other:?}"),
        }
        // An endless quoted string is abandoned, and later text comes through.
        let mut long = b"\x1b[\"".to_vec();
        long.extend(std::iter::repeat_n(b'a', MAX_SEQUENCE_LEN));
        long.extend(b"\"pQ");
        let out = feed_all(&mut p, &long);
        assert_eq!(out.last(), Some(&Feed::Text(b'Q')));
    }

    #[test]
    fn count_treats_zero_and_missing_as_one() {
        let c = Csi { private: None, params: vec![Some(0), None, Some(4)], final_byte: b'A' };
        assert_eq!((c.count(0), c.count(1), c.count(2), c.count(3)), (1, 1, 4, 1));
        assert_eq!(c.param(3, 9), 9);
    }

    #[test]
    fn sgr_colours_map_to_cga_attributes() {
        let mut r = Rendition::default();
        assert_eq!(r.attr(), 0x07);
        r.apply_sgr(&[Some(31)]); // red
        assert_eq!(r.attr(), 0x04);
        r.apply_sgr(&[Some(1), Some(44)]); // bright, blue background
        assert_eq!(r.attr(), 0x1C);
        r.apply_sgr(&[Some(33), Some(46)]); // yellow (brown + bright) on cyan
        assert_eq!(r.attr(), 0x3E);
        r.apply_sgr(&[Some(5)]);
        assert_eq!(r.attr(), 0xBE);
        r.apply_sgr(&[Some(2)]); // normal intensity
        assert_eq!(r.attr(), 0xB6);
        r.apply_sgr(&[]); // ESC[m = reset
        assert_eq!(r.attr(), 0x07);
    }

    #[test]
    fn sgr_reverse_and_concealed() {
        let mut r = Rendition::default();
        r.apply_sgr(&[Some(32), Some(41), Some(7)]); // green on red, reversed
        assert_eq!(r.attr(), 0x24);
        r.apply_sgr(&[Some(0), Some(37), Some(44), Some(8)]); // concealed: fg = bg
        assert_eq!(r.attr(), 0x11);
        r.apply_sgr(&[Some(4), Some(99)]); // underline and unknown codes: no change
        assert_eq!(r.attr(), 0x11);
    }

    #[test]
    fn rendition_round_trips_an_attribute() {
        for a in [0x07u8, 0x1F, 0x4E, 0x8A, 0x70] {
            assert_eq!(Rendition::from_attr(a).attr(), a);
        }
    }
}
