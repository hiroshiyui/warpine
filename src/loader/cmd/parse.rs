// SPDX-License-Identifier: GPL-3.0-or-later
//
// The built-in CMD.EXE's command-line parsing: the /C and /K switches,
// tokens, pipelines and redirections.

/// Parsed command line with pipeline segments and I/O redirections.
pub struct ParsedCommand {
    /// Pipeline segments (commands split at unquoted `|`)
    pub segments: Vec<String>,
    /// `< file` stdin redirect
    pub stdin_file: Option<String>,
    /// `> file` or `>> file` stdout redirect: (path, append)
    pub stdout_redir: Option<(String, bool)>,
}

/// Parse `/C` / `/K` flags and extract the optional initial command.
/// Returns `(run_once, initial_command)`.
///
/// `/C <cmd>` — run command then exit (`run_once = true`)
/// `/K <cmd>` — run command then stay interactive (`run_once = false`)
/// No flag     — start interactive shell immediately
pub fn parse_shell_flags(args: &[String]) -> (bool, Option<String>) {
    // args[0] is the program name; args[1] is the optional argument string.
    let arg_str = match args.get(1) {
        Some(s) if !s.is_empty() => s.trim(),
        _ => return (false, None),
    };

    let upper = arg_str.to_uppercase();
    if upper.starts_with("/C ") || upper == "/C" {
        let cmd = arg_str[2..].trim().to_string();
        (true, if cmd.is_empty() { None } else { Some(cmd) })
    } else if upper.starts_with("/K ") || upper == "/K" {
        let cmd = arg_str[2..].trim().to_string();
        (false, if cmd.is_empty() { None } else { Some(cmd) })
    } else {
        // Treat anything else as a command to run interactively (/K semantics)
        (false, Some(arg_str.to_string()))
    }
}

/// Tokenize a command line, respecting double-quoted strings.
pub fn tokenize(line: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut in_quote = false;
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            in_quote = !in_quote;
            if start.is_none() { start = Some(i); }
        } else if b == b' ' && !in_quote {
            if let Some(s) = start.take() {
                tokens.push(&line[s..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
        i += 1;
    }
    if let Some(s) = start {
        tokens.push(&line[s..]);
    }
    tokens
}

/// Parse a command line into segments (pipeline), stdout redirect, and stdin redirect.
/// OS/2's escape character: `^&` is a plain `&`, outside quotes.
const ESCAPE_CHAR: char = '^';

/// How a command of a line is joined to the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Chain {
    /// The first command, or one after `&`: it always runs.
    Then,
    /// After `&&`: it runs when the one before returned 0.
    And,
    /// After `||`: it runs when the one before returned non-zero.
    Or,
}

/// Split a command line at the `&`, `&&` and `||` outside quotes and
/// parentheses and not escaped by `^` (a single `|` is a pipe and stays). Each command is
/// trimmed; empty ones are dropped. Commands run left to right, each
/// looking at the return code the line has so far.
pub(super) fn split_chain(line: &str) -> Vec<(Chain, String)> {
    let mut out = Vec::new();
    let mut link = Chain::Then;
    let mut current = String::new();
    let mut in_quote = false;
    let mut depth = 0usize;
    let mut chars = line.chars().peekable();
    let mut finish = |link: Chain, current: &mut String| {
        let command = current.trim();
        if !command.is_empty() { out.push((link, command.to_string())); }
        current.clear();
    };
    while let Some(ch) = chars.next() {
        match ch {
            '"' => { in_quote = !in_quote; current.push(ch); }
            ESCAPE_CHAR if !in_quote => {
                current.push(ch);
                if let Some(next) = chars.next() { current.push(next); }
            }
            '(' if !in_quote => { depth += 1; current.push(ch); }
            ')' if !in_quote => { depth = depth.saturating_sub(1); current.push(ch); }
            _ if depth > 0 => current.push(ch),
            '&' if !in_quote => {
                finish(link, &mut current);
                link = if chars.next_if_eq(&'&').is_some() { Chain::And } else { Chain::Then };
            }
            '|' if !in_quote && chars.peek() == Some(&'|') => {
                chars.next();
                finish(link, &mut current);
                link = Chain::Or;
            }
            _ => current.push(ch),
        }
    }
    finish(link, &mut current);
    out
}

pub fn parse_command(line: &str) -> ParsedCommand {
    let mut segments: Vec<String> = Vec::new();
    let mut stdin_file: Option<String> = None;
    let mut stdout_redir: Option<(String, bool)> = None;

    let mut current = String::new();
    let mut in_quote = false;
    // Inside parentheses `|`, `<` and `>` belong to the group.
    let mut depth = 0usize;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        if ch == '"' {
            in_quote = !in_quote;
            current.push(ch);
            i += 1;
            continue;
        }
        // `^` outside quotes: the next character is taken as it is (in a
        // group, it stays for the group's own parsing).
        if ch == ESCAPE_CHAR && !in_quote && i + 1 < chars.len() {
            if depth > 0 { current.push(ch); }
            current.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if !in_quote && ch == '(' { depth += 1; }
        if !in_quote && ch == ')' { depth = depth.saturating_sub(1); current.push(ch); i += 1; continue; }
        if in_quote || depth > 0 {
            current.push(ch);
            i += 1;
            continue;
        }
        if ch == '|' {
            segments.push(current.trim().to_string());
            current = String::new();
            i += 1;
            continue;
        }
        if ch == '>' {
            let append = i + 1 < chars.len() && chars[i + 1] == '>';
            if append { i += 1; }
            i += 1;
            while i < chars.len() && chars[i] == ' ' { i += 1; }
            let mut fname = String::new();
            while i < chars.len() && !matches!(chars[i], ' ' | '<' | '>' | '|') {
                fname.push(chars[i]);
                i += 1;
            }
            if !fname.is_empty() { stdout_redir = Some((fname, append)); }
            continue;
        }
        if ch == '<' {
            i += 1;
            while i < chars.len() && chars[i] == ' ' { i += 1; }
            let mut fname = String::new();
            while i < chars.len() && !matches!(chars[i], ' ' | '<' | '>' | '|') {
                fname.push(chars[i]);
                i += 1;
            }
            if !fname.is_empty() { stdin_file = Some(fname); }
            continue;
        }
        current.push(ch);
        i += 1;
    }
    let last = current.trim().to_string();
    if !last.is_empty() || segments.is_empty() {
        segments.push(last);
    }

    ParsedCommand { segments, stdin_file, stdout_redir }
}

/// The condition of an `IF` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum IfCondition {
    /// `ERRORLEVEL n`: the return code is `n` or more.
    ErrorLevel(u32),
    /// `EXIST name`: a file or directory matches `name` (wildcards allowed).
    Exist(String),
    /// `a==b`: the strings are equal, case and quotes included.
    Equal(String, String),
}

/// An `IF [NOT] condition command`, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IfCommand {
    pub not: bool,
    pub condition: IfCondition,
    pub command: String,
}

/// The first word of `s` (up to a blank) and the rest, trimmed at its
/// start; quotes keep blanks inside a word.
pub(super) fn first_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    let mut in_quote = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            c if c.is_whitespace() && !in_quote => return (&s[..i], s[i..].trim_start()),
            _ => {}
        }
    }
    (s, "")
}

/// Parse what follows `IF` (`[NOT] ERRORLEVEL n cmd`, `[NOT] EXIST name
/// cmd`, `[NOT] a==b cmd`, blanks around `==` allowed). `None` for a
/// syntax error: no condition or no command.
pub(super) fn parse_if(rest: &str) -> Option<IfCommand> {
    let (mut word, mut rest) = first_word(rest);
    let not = word.eq_ignore_ascii_case("NOT");
    if not { (word, rest) = first_word(rest); }
    let (condition, command) = if word.eq_ignore_ascii_case("ERRORLEVEL") {
        let (n, rest) = first_word(rest);
        (IfCondition::ErrorLevel(n.parse().ok()?), rest)
    } else if word.eq_ignore_ascii_case("EXIST") {
        let (name, rest) = first_word(rest);
        if name.is_empty() { return None; }
        (IfCondition::Exist(name.to_string()), rest)
    } else {
        // `a==b`, `a== b`, `a ==b`, `a == b`.
        let (left, rest) = match word.split_once("==") {
            Some((l, r)) if !r.is_empty() => return parse_if_tail(not, l, r, rest),
            Some((l, _)) => (l.to_string(), rest),
            None => {
                let rest = rest.strip_prefix("==")?;
                (word.to_string(), rest)
            }
        };
        let (right, rest) = first_word(rest);
        return parse_if_tail(not, &left, right, rest);
    };
    (!command.is_empty()).then(|| IfCommand { not, condition, command: command.to_string() })
}

fn parse_if_tail(not: bool, left: &str, right: &str, command: &str) -> Option<IfCommand> {
    if right.is_empty() || command.is_empty() { return None; }
    Some(IfCommand { not, condition: IfCondition::Equal(left.to_string(), right.to_string()), command: command.to_string() })
}

/// A `FOR %v IN (set) DO command`, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ForCommand {
    /// The variable's letter (`%v`; a script writes `%%v`, which
    /// expansion has made `%v`).
    pub var: char,
    /// The set's items, split at blanks, commas and semicolons.
    pub items: Vec<String>,
    pub command: String,
}

/// Parse what follows `FOR`. `None` for a syntax error.
pub(super) fn parse_for(rest: &str) -> Option<ForCommand> {
    let (var, rest) = first_word(rest);
    let mut chars = var.chars();
    let (Some('%'), Some(letter), None) = (chars.next(), chars.next(), chars.next()) else { return None };
    if !letter.is_ascii_alphabetic() { return None; }
    let (word, rest) = first_word(rest);
    if !word.eq_ignore_ascii_case("IN") { return None; }
    let rest = rest.strip_prefix('(')?;
    let (set, rest) = rest.split_once(')')?;
    let (word, command) = first_word(rest);
    if !word.eq_ignore_ascii_case("DO") || command.is_empty() { return None; }
    let items = set.split([' ', '\t', ',', ';']).filter(|s| !s.is_empty()).map(str::to_string).collect();
    Some(ForCommand { var: letter, items, command: command.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    use split_chain as chain;

    /// The commands of `links` as `split_chain` returns them.
    fn want(links: &[(Chain, &str)]) -> Vec<(Chain, String)> {
        links.iter().map(|&(c, s)| (c, s.to_string())).collect()
    }

    /// `&`, `&&` and `||` split a line; pipes, quotes and `^` do not.
    #[test]
    fn commands_split_at_and_or_and_then() {
        assert_eq!(chain("a & b && c || d"), want(&[(Chain::Then, "a"), (Chain::Then, "b"), (Chain::And, "c"), (Chain::Or, "d")]));
        assert_eq!(chain("ver | find \"x\" && echo y"), want(&[(Chain::Then, "ver | find \"x\""), (Chain::And, "echo y")]));
        assert_eq!(chain("echo \"a && b\" & echo c"), want(&[(Chain::Then, "echo \"a && b\""), (Chain::Then, "echo c")]));
        assert_eq!(chain("echo a^&b"), want(&[(Chain::Then, "echo a^&b")]), "escaped for parse_command");
        assert_eq!(chain(" & a &&  & "), want(&[(Chain::Then, "a")]), "empty commands dropped");
        assert_eq!(chain("a&&b"), want(&[(Chain::Then, "a"), (Chain::And, "b")]));
        assert!(chain("").is_empty());
    }

    /// `IF`'s three conditions, with `NOT` and blanks around `==`.
    #[test]
    fn if_commands_parse() {
        let parsed = |s: &str| parse_if(s).map(|c| (c.not, c.condition, c.command));
        assert_eq!(parsed("ERRORLEVEL 2 echo high"), Some((false, IfCondition::ErrorLevel(2), "echo high".into())));
        assert_eq!(parsed("not exist C:\\X.TXT goto make"),
                   Some((true, IfCondition::Exist("C:\\X.TXT".into()), "goto make".into())));
        let eq = |l: &str, r: &str| IfCondition::Equal(l.into(), r.into());
        assert_eq!(parsed("\"%1\"==\"\" goto usage"), Some((false, eq("\"%1\"", "\"\""), "goto usage".into())));
        assert_eq!(parsed("a == b echo x"), Some((false, eq("a", "b"), "echo x".into())));
        assert_eq!(parsed("a== b echo x"), Some((false, eq("a", "b"), "echo x".into())));
        assert_eq!(parsed("a ==b echo x"), Some((false, eq("a", "b"), "echo x".into())));
        assert_eq!(parsed("\"a b\"==\"a b\" echo same"), Some((false, eq("\"a b\"", "\"a b\""), "echo same".into())));
        for bad in ["", "ERRORLEVEL x echo", "ERRORLEVEL 1", "EXIST", "a==b", "a b c", "NOT"] {
            assert_eq!(parse_if(bad), None, "{bad:?}");
        }
    }

    /// `FOR %v IN (set) DO command`: the set splits at blanks, commas and
    /// semicolons.
    #[test]
    fn for_commands_parse() {
        let f = parse_for("%f in (a.txt, b.txt;c) do echo %f").unwrap();
        assert_eq!((f.var, f.items.as_slice(), f.command.as_str()), ('f', &["a.txt".to_string(), "b.txt".into(), "c".into()][..], "echo %f"));
        assert_eq!(parse_for("%X IN () DO echo").map(|f| f.items.len()), Some(0));
        for bad in ["", "%f in (a) echo", "f in (a) do x", "%ff in (a) do x", "%1 in (a) do x", "%f on (a) do x", "%f in a do x", "%f in (a) do"] {
            assert_eq!(parse_for(bad), None, "{bad:?}");
        }
    }

    /// Parentheses group commands: `&`, `|` and redirections inside them
    /// belong to the group.
    #[test]
    fn parentheses_group_commands() {
        assert_eq!(split_chain("(a & b) && c"), want(&[(Chain::Then, "(a & b)"), (Chain::And, "c")]));
        assert_eq!(split_chain("x || (a && (b & c))"), want(&[(Chain::Then, "x"), (Chain::Or, "(a && (b & c))")]));
        let p = parse_command("(echo a > in.txt & ver | find x) > out.txt");
        assert_eq!(p.segments, vec!["(echo a > in.txt & ver | find x)"]);
        assert_eq!(p.stdout_redir, Some(("out.txt".to_string(), false)));
        assert_eq!(parse_command("(echo a^&b) | more").segments, vec!["(echo a^&b)", "more"], "^ kept for the group");
        assert_eq!(split_chain("echo a) & echo b"), want(&[(Chain::Then, "echo a)"), (Chain::Then, "echo b")]), "a lone ) is text");
    }

    /// `^` takes the next character as it is, outside quotes only.
    #[test]
    fn caret_escapes_the_next_character() {
        let p = parse_command("echo a^|b ^> c");
        assert_eq!(p.segments, vec!["echo a|b > c"]);
        assert!(p.stdout_redir.is_none());
        assert_eq!(parse_command("echo \"^|\"").segments, vec!["echo \"^|\""]);
        assert_eq!(parse_command("echo a^&b").segments, vec!["echo a&b"]);
        assert_eq!(parse_command("echo end^").segments, vec!["echo end^"], "a trailing ^ stays");
    }

    // ── parse_shell_flags ────────────────────────────────────────────────────

    #[test]
    fn test_parse_flags_no_args() {
        let args: Vec<String> = vec![];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(!once);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_parse_flags_only_program_name() {
        let args = vec!["CMD.EXE".into()];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(!once);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_parse_flags_slash_c_with_command() {
        let args = vec!["CMD.EXE".into(), "/C dir c:\\".into()];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(once);
        assert_eq!(cmd.as_deref(), Some("dir c:\\"));
    }

    #[test]
    fn test_parse_flags_slash_c_alone() {
        let args = vec!["CMD.EXE".into(), "/C".into()];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(once);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_parse_flags_slash_k_with_command() {
        let args = vec!["CMD.EXE".into(), "/K echo hello".into()];
        let (once, cmd) = parse_shell_flags(&args);
        assert!(!once);
        assert_eq!(cmd.as_deref(), Some("echo hello"));
    }

    // ── tokenize ────────────────────────────────────────────────────────────

    #[test]
    fn test_tokenize_simple() {
        assert_eq!(tokenize("dir c:\\"), vec!["dir", "c:\\"]);
    }

    #[test]
    fn test_tokenize_empty() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn test_tokenize_quoted() {
        let t = tokenize(r#"echo "hello world""#);
        assert_eq!(t, vec!["echo", r#""hello world""#]);
    }

    #[test]
    fn test_tokenize_multiple_spaces() {
        assert_eq!(tokenize("cd  c:\\os2"), vec!["cd", "c:\\os2"]);
    }

    // ── parse_command ─────────────────────────────────────────────────────────

    #[test]
    fn test_parse_command_no_redirections() {
        let p = parse_command("echo hello world");
        assert_eq!(p.segments, vec!["echo hello world"]);
        assert!(p.stdin_file.is_none());
        assert!(p.stdout_redir.is_none());
    }

    #[test]
    fn test_parse_command_stdout_redirect() {
        let p = parse_command("echo hello > out.txt");
        assert_eq!(p.segments, vec!["echo hello"]);
        assert_eq!(p.stdout_redir, Some(("out.txt".to_string(), false)));
    }

    #[test]
    fn test_parse_command_append_redirect() {
        let p = parse_command("echo hello >> log.txt");
        assert_eq!(p.segments, vec!["echo hello"]);
        assert_eq!(p.stdout_redir, Some(("log.txt".to_string(), true)));
    }

    #[test]
    fn test_parse_command_stdin_redirect() {
        let p = parse_command("type < input.txt");
        assert_eq!(p.segments, vec!["type"]);
        assert_eq!(p.stdin_file, Some("input.txt".to_string()));
    }

    #[test]
    fn test_parse_command_pipe() {
        let p = parse_command("echo hello | ver");
        assert_eq!(p.segments, vec!["echo hello", "ver"]);
        assert!(p.stdin_file.is_none());
        assert!(p.stdout_redir.is_none());
    }

    #[test]
    fn test_parse_command_pipe_three_stages() {
        let p = parse_command("echo a | echo b | echo c");
        assert_eq!(p.segments.len(), 3);
    }

    #[test]
    fn test_parse_command_quoted_pipe_not_split() {
        let p = parse_command(r#"echo "a|b""#);
        assert_eq!(p.segments.len(), 1, "pipe inside quotes should not split");
    }

    #[test]
    fn test_parse_command_empty() {
        let p = parse_command("   ");
        // Should produce one empty segment
        assert_eq!(p.segments, vec![""]);
        assert!(p.stdin_file.is_none());
        assert!(p.stdout_redir.is_none());
    }
}
