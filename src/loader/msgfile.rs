// SPDX-License-Identifier: GPL-3.0-or-later
//
// Warpine's message files and bound message segments (ADR-0105).
//
// IBM's MKMSGF format has no complete public description, so Warpine has
// its own, written by `os2msg` and read by `DosGetMessage`. The format is
// Warpine's: nothing here comes from IBM's files.
//
// A message file, little-endian:
//
//   0   magic "WARPMSG\x1A"
//   8   u16 version (1)
//   10  u16 codepage of the texts (0: the reader's)
//   12  3 bytes component ("SYS"), 1 byte 0
//   16  u32 number of the first message
//   20  u32 count of messages
//   24  u32 offset of the index (32)
//   28  u32 size of the file
//   32  index: count × (u32 offset, u32 length) of each message's record,
//       numbered from the first; length 0 is a number with no message
//       records: a type byte (E W I P H ?) and the text
//
// A bound message segment (what `lx-link` puts in a program for the
// message files given to it, and what the SDK's `DosGetMessage` stub
// passes to MSG.6), little-endian, offsets from its start:
//
//   0   magic "WARPMSGS"
//   8   u32 count of files
//   12  count × (u32 name offset, u32 file offset, u32 file size)
//       names: the file's name without directories, upper case, NUL-ended
//
// This module is shared with `os2msg` and `lx-link` (`#[path]`), so it
// depends on nothing else in Warpine.

/// The first bytes of a message file.
pub const FILE_MAGIC: &[u8; 8] = b"WARPMSG\x1A";
/// The first bytes of a bound message segment.
pub const SEGMENT_MAGIC: &[u8; 8] = b"WARPMSGS";
/// The version this module writes and reads.
pub const VERSION: u16 = 1;
/// Size of a message file's header.
pub const HEADER_SIZE: usize = 32;
/// Size of an index entry.
const INDEX_ENTRY: usize = 8;
/// Size of a segment's header and of each of its entries.
const SEGMENT_HEADER: usize = 12;
const SEGMENT_ENTRY: usize = 12;
/// Largest message file or segment taken (a bound on what is read).
pub const MAX_FILE: usize = 16 * 1024 * 1024;
/// Most messages in a file.
pub const MAX_MESSAGES: u32 = 65_535;
/// Most files in a segment.
pub const MAX_SEGMENT_FILES: u32 = 256;
/// Longest file name in a segment.
pub const MAX_NAME: usize = 255;
/// Largest message number (four digits after the component).
pub const MAX_NUMBER: u32 = 9999;

/// What a message is (the letter after its number in the source).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgType {
    /// `E`: an error; shown with its id (`ABC0001: …`).
    Error,
    /// `W`: a warning; shown with its id.
    Warning,
    /// `I`: information.
    Info,
    /// `P`: a prompt; its last line has no line end.
    Prompt,
    /// `H`: help.
    Help,
    /// `?`: a number with no text in use.
    Unused,
}

impl MsgType {
    pub fn from_letter(c: u8) -> Option<Self> {
        Some(match c.to_ascii_uppercase() {
            b'E' => Self::Error,
            b'W' => Self::Warning,
            b'I' => Self::Info,
            b'P' => Self::Prompt,
            b'H' => Self::Help,
            b'?' => Self::Unused,
            _ => return None,
        })
    }

    pub fn letter(self) -> u8 {
        match self {
            Self::Error => b'E',
            Self::Warning => b'W',
            Self::Info => b'I',
            Self::Prompt => b'P',
            Self::Help => b'H',
            Self::Unused => b'?',
        }
    }

    /// Whether `DosGetMessage` puts the message's id before it.
    pub fn shows_id(self) -> bool {
        matches!(self, Self::Error | Self::Warning)
    }
}

/// A message read from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: MsgType,
    pub text: Vec<u8>,
}

/// A parsed message file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgFile {
    pub component: [u8; 3],
    pub codepage: u16,
    pub first: u32,
    /// `messages[i]` is number `first + i`; `None` has no message.
    pub messages: Vec<Option<Message>>,
}

impl MsgFile {
    /// Message `number`, if the file has it.
    pub fn get(&self, number: u32) -> Option<&Message> {
        let i = number.checked_sub(self.first)? as usize;
        self.messages.get(i)?.as_ref()
    }

    /// `ABC0001: `, the id `DosGetMessage` shows before an error or warning.
    pub fn id_prefix(&self, number: u32) -> Vec<u8> {
        let mut id = self.component.to_vec();
        id.extend_from_slice(format!("{number:04}: ").as_bytes());
        id
    }
}

/// Why a file or a source was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub String);

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

fn err<T>(msg: impl Into<String>) -> Result<T, FormatError> { Err(FormatError(msg.into())) }

fn u16_at(b: &[u8], at: usize) -> Option<u16> { Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?)) }
fn u32_at(b: &[u8], at: usize) -> Option<u32> { Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?)) }

/// Read a message file.
pub fn parse(bytes: &[u8]) -> Result<MsgFile, FormatError> {
    if bytes.len() > MAX_FILE { return err("larger than 16 MB"); }
    if bytes.len() < HEADER_SIZE || &bytes[..8] != FILE_MAGIC { return err("not a Warpine message file"); }
    let field = |at| u32_at(bytes, at).ok_or_else(|| FormatError("header cut short".into()));
    let version = u16_at(bytes, 8).unwrap_or(0);
    if version != VERSION { return err(format!("version {version} is not supported")); }
    let codepage = u16_at(bytes, 10).unwrap_or(0);
    let component = [bytes[12], bytes[13], bytes[14]];
    let (first, count, index, size) = (field(16)?, field(20)?, field(24)? as usize, field(28)? as usize);
    if size != bytes.len() { return err("its size does not match its header"); }
    if count > MAX_MESSAGES { return err("too many messages"); }
    if first.checked_add(count).is_none_or(|end| end > MAX_NUMBER + 1) && count > 0 { return err("message numbers past 9999"); }
    let index_end = index.checked_add(count as usize * INDEX_ENTRY).filter(|&e| e <= bytes.len())
        .ok_or_else(|| FormatError("the index lies outside the file".into()))?;
    let mut messages = Vec::with_capacity(count as usize);
    for at in (index..index_end).step_by(INDEX_ENTRY) {
        let (off, len) = (field(at)? as usize, field(at + 4)? as usize);
        if len == 0 { messages.push(None); continue; }
        let record = off.checked_add(len).and_then(|end| bytes.get(off..end))
            .ok_or_else(|| FormatError("a message lies outside the file".into()))?;
        let kind = MsgType::from_letter(record[0]).ok_or_else(|| FormatError("a message has no type".into()))?;
        messages.push(Some(Message { kind, text: record[1..].to_vec() }));
    }
    Ok(MsgFile { component, codepage, first, messages })
}

/// Write a message file.
pub fn write(file: &MsgFile) -> Result<Vec<u8>, FormatError> {
    let count = u32::try_from(file.messages.len()).ok().filter(|&c| c <= MAX_MESSAGES)
        .ok_or_else(|| FormatError("too many messages".into()))?;
    let mut out = Vec::new();
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&file.codepage.to_le_bytes());
    out.extend_from_slice(&file.component);
    out.push(0);
    out.extend_from_slice(&file.first.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]); // the size, below
    let mut records = Vec::new();
    let base = HEADER_SIZE + file.messages.len() * INDEX_ENTRY;
    for m in &file.messages {
        match m {
            Some(m) => {
                let off = base + records.len();
                records.push(m.kind.letter());
                records.extend_from_slice(&m.text);
                out.extend_from_slice(&(off as u32).to_le_bytes());
                out.extend_from_slice(&((1 + m.text.len()) as u32).to_le_bytes());
            }
            None => out.extend_from_slice(&[0; 8]),
        }
    }
    out.extend_from_slice(&records);
    if out.len() > MAX_FILE { return err("larger than 16 MB"); }
    let size = out.len() as u32;
    out[28..32].copy_from_slice(&size.to_le_bytes());
    Ok(out)
}

/// Compile a message source (`os2msg`):
///
/// - lines starting with `;` are comments;
/// - the first other line is the component: three letters or digits;
/// - a message starts with its id, the component, four digits and its
///   type letter, then `:` and its text (`ABC0001E: The file %1 …`);
/// - the lines after it, up to the next id, continue it;
/// - each line ends with CR LF in the message, except a prompt's last;
///   trailing empty lines of a message are dropped;
/// - numbers rise; a number left out has no message.
///
/// The text's bytes are kept as they are (in the codepage the file names).
pub fn compile(source: &[u8], codepage: u16) -> Result<MsgFile, FormatError> {
    let mut component: Option<[u8; 3]> = None;
    let mut entries: Vec<(u32, MsgType, Vec<Vec<u8>>)> = Vec::new();
    for (n, raw) in source.split(|&b| b == b'\n').enumerate() {
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        let at = |m: &str| FormatError(format!("line {}: {m}", n + 1));
        if line.first() == Some(&b';') { continue; }
        let Some(comp) = component else {
            let t = line.trim_ascii();
            if t.is_empty() { continue; }
            if t.len() != 3 || !t.iter().all(u8::is_ascii_alphanumeric) {
                return Err(at("the component must be three letters or digits"));
            }
            component = Some([t[0].to_ascii_uppercase(), t[1].to_ascii_uppercase(), t[2].to_ascii_uppercase()]);
            continue;
        };
        let id = line.get(..9).filter(|id| {
            id[..3].eq_ignore_ascii_case(&comp) && id[3..7].iter().all(u8::is_ascii_digit) && id[8] == b':'
        });
        match id {
            Some(id) => {
                let number: u32 = std::str::from_utf8(&id[3..7]).ok().and_then(|d| d.parse().ok())
                    .ok_or_else(|| at("bad message number"))?;
                let kind = MsgType::from_letter(id[7]).ok_or_else(|| at("the type must be E, W, I, P, H or ?"))?;
                if entries.last().is_some_and(|(last, _, _)| number <= *last) {
                    return Err(at("message numbers must rise"));
                }
                let text = &line[9..];
                let text = text.strip_prefix(b" ").unwrap_or(text);
                entries.push((number, kind, vec![text.to_vec()]));
            }
            None => match entries.last_mut() {
                Some((_, _, lines)) => lines.push(line.to_vec()),
                None if line.trim_ascii().is_empty() => {}
                None => return Err(at("text before the first message id")),
            },
        }
    }
    let Some(component) = component else { return err("no component line") };
    let first = entries.first().map_or(0, |e| e.0);
    let mut messages: Vec<Option<Message>> = Vec::new();
    for (number, kind, mut lines) in entries {
        while lines.len() > 1 && lines.last().is_some_and(|l| l.trim_ascii().is_empty()) { lines.pop(); }
        let mut text = Vec::new();
        let last = lines.len() - 1;
        for (i, l) in lines.into_iter().enumerate() {
            text.extend_from_slice(&l);
            if i < last || kind != MsgType::Prompt { text.extend_from_slice(b"\r\n"); }
        }
        messages.resize((number - first) as usize, None);
        messages.push(Some(Message { kind, text }));
    }
    if messages.len() > MAX_MESSAGES as usize { return err("too many messages"); }
    Ok(MsgFile { component, codepage, first, messages })
}

/// The name a file has in a segment: without directories, upper case.
pub fn segment_name(path: &str) -> String {
    path.rsplit(['\\', '/', ':']).next().unwrap_or(path).to_ascii_uppercase()
}

/// Build a bound message segment from `(name, file bytes)`; each file must
/// be a valid message file.
pub fn build_segment(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, FormatError> {
    if files.len() > MAX_SEGMENT_FILES as usize { return err("too many message files"); }
    let mut out = Vec::new();
    out.extend_from_slice(SEGMENT_MAGIC);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    let mut tail = Vec::new();
    let tail_base = SEGMENT_HEADER + files.len() * SEGMENT_ENTRY;
    for (name, bytes) in files {
        parse(bytes).map_err(|e| FormatError(format!("{name}: {e}")))?;
        let name = segment_name(name);
        if name.is_empty() || name.len() > MAX_NAME || name.contains('\0') { return err(format!("{name}: bad file name")); }
        let name_off = tail_base + tail.len();
        tail.extend_from_slice(name.as_bytes());
        tail.push(0);
        while !tail.len().is_multiple_of(4) { tail.push(0); }
        let file_off = tail_base + tail.len();
        tail.extend_from_slice(bytes);
        while !tail.len().is_multiple_of(4) { tail.push(0); }
        for v in [name_off, file_off, bytes.len()] { out.extend_from_slice(&(v as u32).to_le_bytes()); }
    }
    out.extend_from_slice(&tail);
    if out.len() > MAX_FILE { return err("the message files are larger than 16 MB"); }
    Ok(out)
}

/// A segment's files: (name, offset of the file, its size), from its
/// header and entries (`head`: at least the header and the entries).
/// `None` when `head` is not a segment.
pub fn segment_entries(head: &[u8]) -> Option<Vec<(u32, u32, u32)>> {
    if head.get(..8)? != SEGMENT_MAGIC { return None; }
    let count = u32_at(head, 8)?;
    if count > MAX_SEGMENT_FILES { return None; }
    (0..count as usize).map(|i| {
        let at = SEGMENT_HEADER + i * SEGMENT_ENTRY;
        Some((u32_at(head, at)?, u32_at(head, at + 4)?, u32_at(head, at + 8)?))
    }).collect()
}

/// How many bytes of a segment's start [`segment_entries`] needs, given
/// the count at offset 8.
pub fn segment_head_size(count: u32) -> usize {
    SEGMENT_HEADER + count.min(MAX_SEGMENT_FILES) as usize * SEGMENT_ENTRY
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &[u8] = b"; a test file\r\nABC\r\nABC0001E: The file %1 was not found.\r\nABC0002I: Two\r\nlines.\r\n\r\nABC0005P: Continue (Y/N)?\r\n";

    #[test]
    fn compile_reads_ids_types_continuations_and_gaps() {
        let f = compile(SOURCE, 850).expect("compile");
        assert_eq!(f.component, *b"ABC");
        assert_eq!((f.first, f.messages.len(), f.codepage), (1, 5, 850));
        assert_eq!(f.get(1), Some(&Message { kind: MsgType::Error, text: b"The file %1 was not found.\r\n".to_vec() }));
        assert_eq!(f.get(2).unwrap().text, b"Two\r\nlines.\r\n", "continuation joined, trailing blank line dropped");
        assert_eq!(f.get(3), None);
        assert_eq!(f.get(4), None);
        assert_eq!(f.get(5), Some(&Message { kind: MsgType::Prompt, text: b"Continue (Y/N)?".to_vec() }), "a prompt keeps the cursor on its line");
        assert_eq!(f.get(0), None);
        assert_eq!(f.get(6), None);
        assert_eq!(f.id_prefix(1), b"ABC0001: ");
    }

    #[test]
    fn compile_refuses_bad_sources() {
        for (src, want) in [(&b"AB\n"[..], "component"), (b"ABC\nABC0002E: x\nABC0001E: y\n", "rise"),
                            (b"ABC\nABC0001X: x\n", "type"), (b"ABC\nstray text\n", "before the first"),
                            (b"; only a comment\n", "no component")] {
            let e = compile(src, 0).expect_err("refused");
            assert!(e.0.contains(want), "{e:?} should mention {want}");
        }
    }

    #[test]
    fn files_round_trip_and_bad_ones_are_refused() {
        let f = compile(SOURCE, 437).expect("compile");
        let bytes = write(&f).expect("write");
        assert_eq!(&bytes[..8], FILE_MAGIC);
        assert_eq!(parse(&bytes), Ok(f));
        assert!(parse(b"\xFFMKMSGF\0 an IBM file").is_err());
        let mut cut = bytes.clone();
        cut.truncate(40);
        assert!(parse(&cut).unwrap_err().0.contains("size"));
        let mut wild = bytes.clone();
        let len = wild.len() as u32;
        wild[32..36].copy_from_slice(&(len - 2).to_le_bytes());
        wild[36..40].copy_from_slice(&100u32.to_le_bytes());
        assert!(parse(&wild).unwrap_err().0.contains("outside"), "an index entry past the end");
        let mut many = bytes;
        many[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&many).is_err());
    }

    #[test]
    fn segments_hold_files_by_upper_case_name() {
        let a = write(&compile(SOURCE, 0).unwrap()).unwrap();
        let b = write(&compile(b"XYZ\nXYZ0007I: seven\n", 0).unwrap()).unwrap();
        let seg = build_segment(&[("dir/app.msg".into(), a.clone()), ("C:\\X\\other.MSG".into(), b.clone())]).expect("segment");
        let count = u32::from_le_bytes(seg[8..12].try_into().unwrap());
        let entries = segment_entries(&seg[..segment_head_size(count)]).expect("entries");
        let file = |i: usize| { let (_, off, len) = entries[i]; &seg[off as usize..(off + len) as usize] };
        let name = |i: usize| { let off = entries[i].0 as usize; std::ffi::CStr::from_bytes_until_nul(&seg[off..]).unwrap().to_str().unwrap() };
        assert_eq!((name(0), name(1)), ("APP.MSG", "OTHER.MSG"));
        assert_eq!((file(0), file(1)), (&a[..], &b[..]));
        assert!(segment_entries(b"WARPMSGX\0\0\0\0").is_none());
        assert!(build_segment(&[("bad.msg".into(), b"junk".to_vec())]).is_err());
    }
}
