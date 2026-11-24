// SPDX-License-Identifier: GPL-3.0-or-later
//
// PM profiles (`Prf*` of PMSHAPI): the pure model and Warpine's own file
// format.
//
// A profile holds applications; an application holds keys; a key holds a
// value of bytes. Names are compared case-sensitively ("The search is
// case-dependent", PM Reference, PrfQueryProfileString) and kept in the
// order they were first written. Names are the guest's bytes (its codepage),
// never decoded, so a profile written under one codepage reads back the same.
//
// IBM does not document the binary layout of OS2.INI, so Warpine does not
// reproduce it. A profile is stored as text instead:
//
// ```text
// ; warpine-profile 1
// ["My App"]
// "Window" = "120 80 400 300\0"
// "Blob" = "\x01\x02\xff"
// ```
//
// Every name and value is a quoted byte string: printable ASCII stands for
// itself, `\\`, `\"`, `\0`, `\t`, `\n`, `\r` and `\xHH` for everything else,
// so any byte sequence round-trips and the file stays ASCII. The first line
// identifies the format; a file without it (for example a real OS/2 binary
// INI file) is reported as corrupt rather than misread.


/// First line of every profile file Warpine writes.
pub const PROFILE_MAGIC: &str = "; warpine-profile 1";

/// Largest value a key may hold: "This length should not exceed 64K bytes"
/// (PM Reference, PrfWriteProfileData).
pub const MAX_PROFILE_VALUE: usize = 64 * 1024;

/// Largest profile file Warpine reads (host memory bound).
pub const MAX_PROFILE_FILE: usize = 16 * 1024 * 1024;

/// One application: its name and its keys in insertion order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct App {
    name: Vec<u8>,
    keys: Vec<(Vec<u8>, Vec<u8>)>,
}

/// A profile in memory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    apps: Vec<App>,
}

/// Why a profile file could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line number.
    pub line: usize,
    pub reason: &'static str,
}

impl Profile {
    fn app(&self, app: &[u8]) -> Option<&App> {
        self.apps.iter().find(|a| a.name == app)
    }

    /// The value of `key` of application `app`.
    pub fn get(&self, app: &[u8], key: &[u8]) -> Option<&[u8]> {
        self.app(app)?.keys.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_slice())
    }

    /// Whether application `app` exists.
    pub fn has_app(&self, app: &[u8]) -> bool {
        self.app(app).is_some()
    }

    /// Set `key` of `app` to `value`, creating either as needed.
    pub fn set(&mut self, app: &[u8], key: &[u8], value: &[u8]) {
        let idx = match self.apps.iter().position(|a| a.name == app) {
            Some(i) => i,
            None => {
                self.apps.push(App { name: app.to_vec(), keys: Vec::new() });
                self.apps.len() - 1
            }
        };
        let keys = &mut self.apps[idx].keys;
        match keys.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value.to_vec(),
            None => keys.push((key.to_vec(), value.to_vec())),
        }
    }

    /// Delete `key` of `app`; an application left without keys goes too.
    /// Returns whether the key existed.
    pub fn delete_key(&mut self, app: &[u8], key: &[u8]) -> bool {
        let Some(idx) = self.apps.iter().position(|a| a.name == app) else { return false };
        let keys = &mut self.apps[idx].keys;
        let before = keys.len();
        keys.retain(|(k, _)| k != key);
        let found = keys.len() != before;
        if keys.is_empty() {
            self.apps.remove(idx);
        }
        found
    }

    /// Delete application `app` with all its keys. Returns whether it existed.
    pub fn delete_app(&mut self, app: &[u8]) -> bool {
        let before = self.apps.len();
        self.apps.retain(|a| a.name != app);
        self.apps.len() != before
    }

    /// The application names.
    pub fn app_names(&self) -> impl Iterator<Item = &[u8]> {
        self.apps.iter().map(|a| a.name.as_slice())
    }

    /// The key names of `app`, or `None` when it does not exist.
    pub fn key_names(&self, app: &[u8]) -> Option<Vec<&[u8]>> {
        self.app(app).map(|a| a.keys.iter().map(|(k, _)| k.as_slice()).collect())
    }

    /// Write the profile in Warpine's text format.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(PROFILE_MAGIC.as_bytes());
        out.push(b'\n');
        for app in &self.apps {
            out.push(b'[');
            quote(&app.name, &mut out);
            out.extend_from_slice(b"]\n");
            for (k, v) in &app.keys {
                quote(k, &mut out);
                out.extend_from_slice(b" = ");
                quote(v, &mut out);
                out.push(b'\n');
            }
        }
        out
    }

    /// Read a profile in Warpine's text format. An empty file is an empty
    /// profile; anything else must start with [`PROFILE_MAGIC`].
    pub fn parse(text: &[u8]) -> Result<Profile, ParseError> {
        let mut profile = Profile::default();
        if text.is_empty() {
            return Ok(profile);
        }
        let mut lines = text.split(|&b| b == b'\n').enumerate();
        let first = lines.next().map(|(_, l)| trim(l)).unwrap_or(&[]);
        if first != PROFILE_MAGIC.as_bytes() {
            return Err(ParseError { line: 1, reason: "not a Warpine profile" });
        }
        let mut current: Option<Vec<u8>> = None;
        for (i, raw) in lines {
            let line_no = i + 1;
            let err = |reason| ParseError { line: line_no, reason };
            let line = trim(raw);
            if line.is_empty() || line[0] == b';' {
                continue;
            }
            if line[0] == b'[' {
                let (name, rest) = unquote(trim(&line[1..])).ok_or(err("bad application name"))?;
                if trim(rest) != b"]" {
                    return Err(err("expected `]`"));
                }
                if name.contains(&0) {
                    return Err(err("NUL in a name"));
                }
                current = Some(name);
                continue;
            }
            let app = current.as_ref().ok_or(err("key outside an application"))?;
            let (key, rest) = unquote(line).ok_or(err("bad key name"))?;
            let rest = trim(rest);
            let rest = rest.strip_prefix(b"=").ok_or(err("expected `=`"))?;
            let (value, tail) = unquote(trim(rest)).ok_or(err("bad value"))?;
            if !trim(tail).is_empty() {
                return Err(err("text after the value"));
            }
            if key.contains(&0) {
                return Err(err("NUL in a name"));
            }
            if value.len() > MAX_PROFILE_VALUE {
                return Err(err("value too long"));
            }
            profile.set(app, &key, &value);
        }
        Ok(profile)
    }
}

/// `bytes` without leading and trailing blanks (and a trailing CR).
fn trim(bytes: &[u8]) -> &[u8] {
    let is_blank = |b: &u8| matches!(*b, b' ' | b'\t' | b'\r');
    let start = bytes.iter().position(|b| !is_blank(b)).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|b| !is_blank(b)).map_or(start, |e| e + 1);
    &bytes[start..end]
}

/// Append `bytes` as a quoted, escaped string.
fn quote(bytes: &[u8], out: &mut Vec<u8>) {
    out.push(b'"');
    for &b in bytes {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'"' => out.extend_from_slice(b"\\\""),
            0 => out.extend_from_slice(b"\\0"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0x20..=0x7E => out.push(b),
            _ => out.extend_from_slice(format!("\\x{b:02x}").as_bytes()),
        }
    }
    out.push(b'"');
}

/// Parse a quoted string at the start of `s`: the bytes and the rest.
fn unquote(s: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    let mut rest = s.strip_prefix(b"\"")?;
    let mut out = Vec::new();
    loop {
        let (&b, tail) = rest.split_first()?;
        rest = tail;
        match b {
            b'"' => return Some((out, rest)),
            b'\\' => {
                let (&e, tail) = rest.split_first()?;
                rest = tail;
                out.push(match e {
                    b'\\' => b'\\',
                    b'"' => b'"',
                    b'0' => 0,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b'x' => {
                        let hex = rest.get(..2)?;
                        rest = &rest[2..];
                        u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?
                    }
                    _ => return None,
                });
            }
            _ => out.push(b),
        }
    }
}

/// A list of names as the enumerating `Prf*` calls return it: every name
/// NUL-terminated, the last followed by a second NUL (an empty list is a
/// single NUL).
pub fn name_list<'a>(names: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    for n in names {
        out.extend_from_slice(n);
        out.push(0);
    }
    out.push(0);
    out
}

/// A stored value as a string: the bytes before the first NUL.
pub fn value_as_str(value: &[u8]) -> &[u8] {
    value.iter().position(|&b| b == 0).map_or(value, |n| &value[..n])
}

/// `PrfQueryProfileInt`'s conversion: an optional leading `-` and the
/// digits that follow, up to the first other character ("123xyz" is 123,
/// "-100.10" is -100); no digits is 0. Overflow wraps like a 32-bit LONG.
pub fn parse_profile_int(value: &[u8]) -> i32 {
    let s = value_as_str(value);
    let (neg, digits) = match s.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, s),
    };
    let mut n: i32 = 0;
    for &d in digits.iter().take_while(|b| b.is_ascii_digit()) {
        n = n.wrapping_mul(10).wrapping_add((d - b'0') as i32);
    }
    if neg { n.wrapping_neg() } else { n }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Profile {
        let mut p = Profile::default();
        p.set(b"App", b"Key", b"value\0");
        p.set(b"App", b"Blob", &[0, 1, 0xFF, b'"', b'\\', b'\n']);
        p.set(b"Other \"quoted\"", b"k", b"");
        p
    }

    #[test]
    fn set_get_and_replace() {
        let mut p = sample();
        assert_eq!(p.get(b"App", b"Key"), Some(&b"value\0"[..]));
        assert_eq!(p.get(b"App", b"key"), None, "names are case-sensitive");
        assert_eq!(p.get(b"app", b"Key"), None);
        p.set(b"App", b"Key", b"new");
        assert_eq!(p.get(b"App", b"Key"), Some(&b"new"[..]));
        assert_eq!(p.key_names(b"App").unwrap(), vec![&b"Key"[..], &b"Blob"[..]]);
    }

    #[test]
    fn deleting_keys_and_applications() {
        let mut p = sample();
        assert!(p.delete_key(b"App", b"Key"));
        assert!(!p.delete_key(b"App", b"Key"));
        assert!(p.has_app(b"App"));
        assert!(p.delete_key(b"App", b"Blob"));
        assert!(!p.has_app(b"App"), "an application without keys is removed");
        assert!(p.delete_app(b"Other \"quoted\""));
        assert!(!p.delete_app(b"Other \"quoted\""));
        assert_eq!(p.app_names().count(), 0);
    }

    #[test]
    fn serialization_round_trips_any_bytes() {
        let mut p = sample();
        let all: Vec<u8> = (0..=255).collect();
        p.set(b"\x80\xFE app", b"all", &all);
        let text = p.serialize();
        assert!(text.iter().all(|b| b.is_ascii()), "the file is ASCII");
        assert!(text.starts_with(PROFILE_MAGIC.as_bytes()));
        assert_eq!(Profile::parse(&text).unwrap(), p);
    }

    #[test]
    fn serialized_text_is_readable() {
        let mut p = Profile::default();
        p.set(b"My App", b"Window", b"120 80\0");
        let text = String::from_utf8(p.serialize()).unwrap();
        assert_eq!(text, format!("{PROFILE_MAGIC}\n[\"My App\"]\n\"Window\" = \"120 80\\0\"\n"));
    }

    #[test]
    fn parse_accepts_comments_blank_lines_and_crlf() {
        let text = format!("{PROFILE_MAGIC}\r\n\r\n; note\r\n[ \"A\" ]\r\n  \"k\"=\"v\\x41\"  \r\n");
        let p = Profile::parse(text.as_bytes()).unwrap();
        assert_eq!(p.get(b"A", b"k"), Some(&b"vA"[..]));
        assert_eq!(Profile::parse(b"").unwrap(), Profile::default());
    }

    #[test]
    fn parse_rejects_foreign_or_broken_files() {
        // A binary INI file (or anything else) is not misread.
        assert_eq!(Profile::parse(b"\xFF\xFF\xFF\xFF\x14\x00").unwrap_err().line, 1);
        let bad = |body: &str| Profile::parse(format!("{PROFILE_MAGIC}\n{body}").as_bytes());
        assert_eq!(bad("\"k\" = \"v\"").unwrap_err().reason, "key outside an application");
        assert!(bad("[\"A\"\n").is_err());
        assert!(bad("[\"A\"]\n\"k\" \"v\"").is_err());
        assert!(bad("[\"A\"]\n\"k\" = \"v").is_err());
        assert!(bad("[\"A\"]\n\"k\" = \"\\q\"").is_err());
        assert!(bad("[\"A\"]\n\"k\" = \"\\x4\"").is_err());
        assert!(bad("[\"A\"]\n\"k\" = \"v\" x").is_err());
        assert!(bad("[\"A\\0\"]").is_err(), "a name cannot hold NUL");
        let long = format!("[\"A\"]\n\"k\" = \"{}\"", "a".repeat(MAX_PROFILE_VALUE + 1));
        assert_eq!(bad(&long).unwrap_err().reason, "value too long");
    }

    #[test]
    fn name_lists_are_double_nul_terminated() {
        assert_eq!(name_list([&b"A"[..], &b"BC"[..]]), b"A\0BC\0\0");
        assert_eq!(name_list(std::iter::empty::<&[u8]>()), b"\0");
    }

    #[test]
    fn profile_int_conversion() {
        assert_eq!(parse_profile_int(b"123xyz\0"), 123);
        assert_eq!(parse_profile_int(b"100,000"), 100);
        assert_eq!(parse_profile_int(b"-100.10,000x"), -100);
        assert_eq!(parse_profile_int(b"-123\0"), -123);
        assert_eq!(parse_profile_int(b"abc"), 0);
        assert_eq!(parse_profile_int(b" 12"), 0, "digits must come first");
        assert_eq!(parse_profile_int(b"+12"), 0, "only a minus sign is accepted");
        assert_eq!(parse_profile_int(b""), 0);
        assert_eq!(parse_profile_int(b"12\x0034"), 12, "the value ends at NUL");
        assert_eq!(parse_profile_int(b"4294967297"), 1, "overflow wraps");
    }
}
