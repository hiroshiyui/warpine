// SPDX-License-Identifier: GPL-3.0-or-later
//
// The built-in CMD.EXE's PROMPT (ADR-0080): the `$` codes of OS/2's
// CMD.EXE, turned into the text the shell writes before each command.

use crate::loader::clock::LocalTime;

/// What the default prompt is when `PROMPT` is not set: the current drive
/// and directory in brackets.
pub(super) const DEFAULT_PROMPT: &str = "[$P] ";

/// What a prompt's `$` codes show.
pub(super) struct PromptContext<'a> {
    /// The current drive and directory (`C:\SUB`).
    pub path: &'a str,
    /// The return code of the last command.
    pub last_rc: u32,
    /// The session clock's local date and time, and its hundredths.
    pub now: Option<(LocalTime, u8)>,
    /// What VER writes.
    pub version: &'a str,
}

/// The days of the week, Sunday first (`LocalTime::weekday`).
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// `template` with its `$` codes replaced (letters without case):
///
/// | Code | Text | Code | Text |
/// |------|------|------|------|
/// | `$$` | `$` | `$L` | `<` |
/// | `$_` | CR LF | `$N` | the current drive's letter |
/// | `$A` | `&` | `$P` | the current drive and directory |
/// | `$B` | `\|` | `$Q` | `=` |
/// | `$C` | `(` | `$R` | the last return code |
/// | `$D` | the date (`Thu 11-20-2026`) | `$S` | a blank |
/// | `$E` | ESC (for ANSI sequences) | `$T` | the time (`13:05:09.42`) |
/// | `$F` | `)` | `$V` | the version line VER writes |
/// | `$G` | `>` | `$H` | a backspace |
///
/// `$I` (OS/2's help line) and any other code show nothing.
pub(super) fn render_prompt(template: &str, ctx: &PromptContext) -> String {
    let mut out = String::with_capacity(template.len() + ctx.path.len());
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let Some(code) = chars.next() else { break };
        match code.to_ascii_uppercase() {
            '$' => out.push('$'),
            '_' => out.push_str("\r\n"),
            'A' => out.push('&'),
            'B' => out.push('|'),
            'C' => out.push('('),
            'D' => if let Some((t, _)) = ctx.now {
                let day = WEEKDAYS.get(t.weekday as usize).copied().unwrap_or("");
                out.push_str(&format!("{day} {:02}-{:02}-{:04}", t.month, t.day, t.year));
            },
            'E' => out.push('\u{1b}'),
            'F' => out.push(')'),
            'G' => out.push('>'),
            'H' => out.push('\u{8}'),
            'L' => out.push('<'),
            'N' => out.push(ctx.path.chars().next().unwrap_or('?')),
            'P' => out.push_str(ctx.path),
            'Q' => out.push('='),
            'R' => out.push_str(&ctx.last_rc.to_string()),
            'S' => out.push(' '),
            'T' => if let Some((t, hundredths)) = ctx.now {
                out.push_str(&format!("{:02}:{:02}:{:02}.{:02}", t.hours, t.minutes, t.seconds, hundredths));
            },
            'V' => out.push_str(ctx.version),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(now: Option<(LocalTime, u8)>) -> PromptContext<'static> {
        PromptContext { path: "C:\\SUB", last_rc: 3, now, version: "Warpine 0.0.6" }
    }

    /// Every `$` code, its letter in either case.
    #[test]
    fn dollar_codes_render() {
        let mut t = LocalTime::new(2026, 11, 20, 13, 5, 9);
        t.weekday = 4;
        let c = ctx(Some((t, 42)));
        assert_eq!(render_prompt(DEFAULT_PROMPT, &c), "[C:\\SUB] ");
        assert_eq!(render_prompt("$p$g", &c), "C:\\SUB>");
        assert_eq!(render_prompt("$N$Q$R$S$A$B$C$F$L$$", &c), "C=3 &|()<$");
        assert_eq!(render_prompt("$D $T", &c), "Thu 11-20-2026 13:05:09.42");
        assert_eq!(render_prompt("$e[32m$p$e[0m$_>", &c), "\u{1b}[32mC:\\SUB\u{1b}[0m\r\n>");
        assert_eq!(render_prompt("$v$h", &c), "Warpine 0.0.6\u{8}");
        assert_eq!(render_prompt("$i$x[$", &c), "[", "unknown codes and a trailing $ show nothing");
        assert_eq!(render_prompt("$d$t.", &ctx(None)), ".", "no clock, no date");
        assert_eq!(render_prompt("ünï $p", &c), "ünï C:\\SUB");
    }
}
