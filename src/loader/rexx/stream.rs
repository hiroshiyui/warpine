// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX streams (ADR-0084): CHARIN, CHAROUT, CHARS, LINEIN, LINEOUT, LINES
// and STREAM. A stream is a file, named by its OS/2 path and reached
// through the host (the shell's VFS, so it stays inside the drives), or
// one of the standard streams: STDIN (the default input, the shell's),
// STDOUT (the default output, as SAY) and STDERR.
//
// A file stream opens on first use: for reading, or for reading and
// writing (created when missing) once it is written to. It has a read
// position (from its start) and a write position (from its end until
// moved). Lines end with CR LF when written; LF or CR LF when read. A
// read past the end, or a failed open, read or write, raises NOTREADY.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use super::builtins::{bad, count, opt, option_arg, req, whole_arg, whole_out, Args};
use super::condition::Cond;
use super::interp::{Interp, OpenFor, RexxError, ERROR_PATH_NOT_FOUND, MAX_STRING, R};
use super::program::Program;

/// The most file streams a program has open at once.
pub const MAX_STREAMS: usize = 64;
/// How much is read from a file at a time.
const CHUNK: usize = 4096;
const CRLF: &[u8] = b"\r\n";
/// OS/2's "too many open files".
const ERROR_TOO_MANY_OPEN_FILES: u32 = 4;
/// The OS/2 attribute of a directory.
const ATTR_DIRECTORY: u32 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State { Ready, NotReady, Error }

impl State {
    fn word(self) -> &'static str {
        match self { State::Ready => "READY", State::NotReady => "NOTREADY", State::Error => "ERROR" }
    }
}

/// An open (or failed) file stream.
struct Stream {
    /// The host's handle; `None` when it could not be opened.
    handle: Option<u32>,
    writable: bool,
    read_pos: u64,
    /// `None`: at the end of the file.
    write_pos: Option<u64>,
    state: State,
    /// Why it is not ready: `EOF` or an OS/2 error code.
    reason: Vec<u8>,
}

/// A program's streams.
#[derive(Default)]
pub struct Streams {
    /// File streams by their full name in upper case.
    files: HashMap<Vec<u8>, Stream>,
    /// The default input read ahead (CHARIN, CHARS, LINES), its lines
    /// ending with CR LF.
    input: VecDeque<u8>,
    input_ended: bool,
}

/// What a stream name stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which { StdIn, StdOut, StdErr, File }

/// The stream `name` names; an empty one is the default input or output.
fn which(name: &[u8], output: bool) -> Which {
    let upper = name.trim_ascii().to_ascii_uppercase();
    match upper.strip_suffix(b":").unwrap_or(&upper) {
        b"" if output => Which::StdOut,
        b"" | b"STDIN" => Which::StdIn,
        b"STDOUT" => Which::StdOut,
        b"STDERR" => Which::StdErr,
        _ => Which::File,
    }
}

/// How a read ended.
enum Read { Data(Vec<u8>), End, Failed(u32) }

/// An OS/2 FDATE / FTIME pair as (year, month, day, hours, minutes,
/// seconds).
fn unpack_date_time(date: u16, time: u16) -> (u32, u32, u32, u32, u32, u32) {
    (1980 + (date >> 9) as u32, ((date >> 5) & 0x0F) as u32, (date & 0x1F) as u32,
     (time >> 11) as u32, ((time >> 5) & 0x3F) as u32, ((time & 0x1F) * 2) as u32)
}

impl Interp<'_> {
    // ── The default input ───────────────────────────────────────────────

    /// A line of the default input (PULL with an empty queue, PARSE
    /// EXTERNAL, LINEIN): what was read ahead first, then the host's.
    pub fn read_input_line(&mut self) -> Option<Vec<u8>> {
        let input = &mut self.streams.input;
        if !input.is_empty() {
            let end = input.iter().position(|&b| b == b'\n').map_or(input.len(), |p| p + 1);
            let mut line: Vec<u8> = input.drain(..end).collect();
            if line.last() == Some(&b'\n') { line.pop(); }
            if line.last() == Some(&b'\r') { line.pop(); }
            return Some(line);
        }
        if self.streams.input_ended { return None; }
        let line = self.host.read_line();
        if line.is_none() { self.streams.input_ended = true; }
        line
    }

    /// Read the default input ahead until `n` bytes wait (or it ends).
    fn fill_input(&mut self, n: usize) {
        while self.streams.input.len() < n && !self.streams.input_ended {
            match self.host.read_line() {
                Some(line) => {
                    self.streams.input.extend(line);
                    self.streams.input.extend(CRLF);
                }
                None => self.streams.input_ended = true,
            }
        }
    }

    // ── File streams ────────────────────────────────────────────────────

    /// The key of file stream `name`, opened as needed (for writing too
    /// when `write`). Err: the OS/2 error; the stream is not ready.
    fn file_stream(&mut self, name: &[u8], write: bool) -> Result<Vec<u8>, u32> {
        let full = self.host.full_name(name.trim_ascii()).ok_or(ERROR_PATH_NOT_FOUND)?;
        let key = full.to_ascii_uppercase();
        let existing = self.streams.files.get(&key).map(|s| (s.handle, s.writable));
        match existing {
            Some((Some(_), writable)) if writable || !write => return Ok(key),
            None if self.streams.files.len() >= MAX_STREAMS => return Err(ERROR_TOO_MANY_OPEN_FILES),
            _ => {}
        }
        if let Some((Some(h), _)) = existing { self.host.file_close(h); }
        let opened = self.host.file_open(&full, if write { OpenFor::Write } else { OpenFor::Read });
        let stream = self.streams.files.entry(key.clone()).or_insert_with(|| Stream {
            handle: None, writable: false, read_pos: 0, write_pos: None, state: State::Ready, reason: Vec::new(),
        });
        match opened {
            Ok(h) => {
                stream.handle = Some(h);
                stream.writable = write;
                stream.state = State::Ready;
                stream.reason.clear();
                Ok(key)
            }
            Err(code) => {
                stream.handle = None;
                stream.state = State::NotReady;
                stream.reason = code.to_string().into_bytes();
                Err(code)
            }
        }
    }

    fn handle_of(&self, key: &[u8]) -> u32 {
        self.streams.files.get(key).and_then(|s| s.handle).unwrap_or(u32::MAX)
    }

    /// Close file stream `name`, if open.
    fn close_stream(&mut self, name: &[u8]) {
        let Some(full) = self.host.full_name(name.trim_ascii()) else { return };
        if let Some(Stream { handle: Some(h), .. }) = self.streams.files.remove(&full.to_ascii_uppercase()) {
            self.host.file_close(h);
        }
    }

    /// Close every stream (at the program's end).
    pub fn close_streams(&mut self) {
        let handles: Vec<u32> = self.streams.files.drain().filter_map(|(_, s)| s.handle).collect();
        for h in handles { self.host.file_close(h); }
    }

    /// Mark a stream (`key`, a file's; `None`, a standard one) not ready
    /// or in error, and raise NOTREADY with its name as the description.
    fn not_ready(&mut self, key: Option<&[u8]>, name: &[u8], state: State, reason: &[u8]) -> R<()> {
        if let Some(s) = key.and_then(|k| self.streams.files.get_mut(k)) {
            s.state = state;
            s.reason = reason.to_vec();
        }
        self.raise(Cond::NotReady, name.to_vec())
    }

    fn set_ready(&mut self, key: &[u8]) {
        if let Some(s) = self.streams.files.get_mut(key) {
            s.state = State::Ready;
            s.reason.clear();
        }
    }

    /// Read up to `max` bytes from `pos`; stops at a line end when
    /// `line`.
    fn read_at(&mut self, handle: u32, mut pos: u64, max: usize, line: bool) -> (Read, u64) {
        let mut out = Vec::new();
        let mut buf = vec![0u8; CHUNK];
        let mut any = false;
        while out.len() < max {
            let want = CHUNK.min(max - out.len());
            let n = match self.host.file_read(handle, pos, &mut buf[..want]) {
                Ok(n) => n,
                Err(code) => return (Read::Failed(code), pos),
            };
            if n == 0 { break; }
            any = true;
            if line && let Some(p) = buf[..n].iter().position(|&b| b == b'\n') {
                out.extend_from_slice(&buf[..p]);
                pos += p as u64 + 1;
                if out.last() == Some(&b'\r') { out.pop(); }
                return (Read::Data(out), pos);
            }
            out.extend_from_slice(&buf[..n]);
            pos += n as u64;
        }
        if line && out.last() == Some(&b'\r') { out.pop(); }
        if any { (Read::Data(out), pos) } else { (Read::End, pos) }
    }

    /// Where line `n` (from 1) of a file starts, or `None` past its end.
    fn line_start(&mut self, handle: u32, n: u64) -> Option<u64> {
        let (mut pos, mut line) = (0u64, 1u64);
        let mut buf = vec![0u8; CHUNK];
        while line < n {
            let got = self.host.file_read(handle, pos, &mut buf).ok()?;
            if got == 0 { return None; }
            for &b in &buf[..got] {
                pos += 1;
                if b == b'\n' {
                    line += 1;
                    if line == n { return Some(pos); }
                }
            }
        }
        Some(pos)
    }

    /// The lines between `from` and `to` (a last line without its line
    /// end counts).
    fn count_lines(&mut self, handle: u32, from: u64, to: u64) -> u64 {
        let (mut pos, mut lines, mut last) = (from, 0u64, b'\n');
        let mut buf = vec![0u8; CHUNK];
        while pos < to {
            let want = CHUNK.min((to - pos) as usize);
            let Ok(got) = self.host.file_read(handle, pos, &mut buf[..want]) else { break };
            if got == 0 { break; }
            lines += buf[..got].iter().filter(|&&b| b == b'\n').count() as u64;
            last = buf[got - 1];
            pos += got as u64;
        }
        if pos > from && last != b'\n' { lines += 1; }
        lines
    }

    fn file_size_of(&mut self, key: &[u8]) -> u64 {
        let h = self.handle_of(key);
        self.host.file_size(h).unwrap_or(0)
    }

    // ── The functions ───────────────────────────────────────────────────

    /// LINEIN: the next line (`count` 1) of stream `name`, from line
    /// `line` when given.
    pub fn linein(&mut self, name: &[u8], line: Option<u64>, count: u64) -> R<Vec<u8>> {
        match which(name, false) {
            Which::StdIn => {
                if count == 0 { return Ok(Vec::new()); }
                match self.read_input_line() {
                    Some(l) => Ok(l),
                    None => { self.not_ready(None, b"STDIN", State::NotReady, b"EOF")?; Ok(Vec::new()) }
                }
            }
            Which::StdOut | Which::StdErr => { self.not_ready(None, name, State::Error, b"")?; Ok(Vec::new()) }
            Which::File => {
                let key = match self.file_stream(name, false) {
                    Ok(k) => k,
                    Err(code) => { self.not_ready(None, name, State::NotReady, code.to_string().as_bytes())?; return Ok(Vec::new()); }
                };
                let h = self.handle_of(&key);
                if let Some(n) = line {
                    match self.line_start(h, n) {
                        Some(pos) => if let Some(s) = self.streams.files.get_mut(&key) { s.read_pos = pos },
                        None => { self.not_ready(Some(&key), name, State::NotReady, b"EOF")?; return Ok(Vec::new()); }
                    }
                }
                if count == 0 { return Ok(Vec::new()); }
                let pos = self.streams.files.get(&key).map_or(0, |s| s.read_pos);
                let (read, pos) = self.read_at(h, pos, MAX_STRING, true);
                if let Some(s) = self.streams.files.get_mut(&key) { s.read_pos = pos; }
                match read {
                    Read::Data(l) => { self.set_ready(&key); Ok(l) }
                    Read::End => { self.not_ready(Some(&key), name, State::NotReady, b"EOF")?; Ok(Vec::new()) }
                    Read::Failed(code) => { self.not_ready(Some(&key), name, State::Error, code.to_string().as_bytes())?; Ok(Vec::new()) }
                }
            }
        }
    }

    /// Write `data` to file stream `name` at character `start` (from 1)
    /// or its write position. Returns whether it was written.
    fn write_file(&mut self, name: &[u8], data: Option<&[u8]>, start: Option<u64>, line: Option<u64>) -> R<bool> {
        let key = match self.file_stream(name, true) {
            Ok(k) => k,
            Err(code) => { self.not_ready(None, name, State::NotReady, code.to_string().as_bytes())?; return Ok(false); }
        };
        let h = self.handle_of(&key);
        if let Some(n) = line {
            let Some(pos) = self.line_start(h, n) else {
                self.not_ready(Some(&key), name, State::NotReady, b"EOF")?;
                return Ok(false);
            };
            if let Some(s) = self.streams.files.get_mut(&key) { s.write_pos = Some(pos); }
        }
        if let Some(n) = start && let Some(s) = self.streams.files.get_mut(&key) { s.write_pos = Some(n - 1); }
        let Some(data) = data else { return Ok(true) };
        let pos = match self.streams.files.get(&key).and_then(|s| s.write_pos) {
            Some(p) => p,
            None => self.host.file_size(h).unwrap_or(0),
        };
        match self.host.file_write(h, pos, data) {
            Ok(()) => {
                if let Some(s) = self.streams.files.get_mut(&key) { s.write_pos = Some(pos + data.len() as u64); }
                self.set_ready(&key);
                Ok(true)
            }
            Err(code) => { self.not_ready(Some(&key), name, State::Error, code.to_string().as_bytes())?; Ok(false) }
        }
    }

    /// LINEOUT: write `string` and a line end; no string and no line
    /// closes the stream. Returns the lines not written (0 or 1).
    pub fn lineout(&mut self, name: &[u8], string: Option<&[u8]>, line: Option<u64>) -> R<u64> {
        match which(name, true) {
            Which::StdOut => { if let Some(s) = string { self.host.say(s); } Ok(0) }
            Which::StdErr => {
                if let Some(s) = string { self.host.write_err(&[s, CRLF].concat()); }
                Ok(0)
            }
            Which::StdIn => { self.not_ready(None, name, State::Error, b"")?; Ok(1) }
            Which::File if string.is_none() && line.is_none() => { self.close_stream(name); Ok(0) }
            Which::File => {
                let data = string.map(|s| [s, CRLF].concat());
                Ok(if self.write_file(name, data.as_deref(), None, line)? { 0 } else { 1 })
            }
        }
    }

    /// CHARIN: `length` characters of stream `name`, from `start`.
    pub fn charin(&mut self, name: &[u8], start: Option<u64>, length: usize) -> R<Vec<u8>> {
        match which(name, false) {
            Which::StdIn => {
                self.fill_input(length);
                let n = length.min(self.streams.input.len());
                let out: Vec<u8> = self.streams.input.drain(..n).collect();
                if out.len() < length { self.not_ready(None, b"STDIN", State::NotReady, b"EOF")?; }
                Ok(out)
            }
            Which::StdOut | Which::StdErr => { self.not_ready(None, name, State::Error, b"")?; Ok(Vec::new()) }
            Which::File => {
                let key = match self.file_stream(name, false) {
                    Ok(k) => k,
                    Err(code) => { self.not_ready(None, name, State::NotReady, code.to_string().as_bytes())?; return Ok(Vec::new()); }
                };
                if let Some(n) = start && let Some(s) = self.streams.files.get_mut(&key) { s.read_pos = n - 1; }
                if length == 0 { return Ok(Vec::new()); }
                let h = self.handle_of(&key);
                let pos = self.streams.files.get(&key).map_or(0, |s| s.read_pos);
                let (read, pos) = self.read_at(h, pos, length, false);
                if let Some(s) = self.streams.files.get_mut(&key) { s.read_pos = pos; }
                match read {
                    Read::Data(d) if d.len() == length => { self.set_ready(&key); Ok(d) }
                    Read::Data(d) => { self.not_ready(Some(&key), name, State::NotReady, b"EOF")?; Ok(d) }
                    Read::End => { self.not_ready(Some(&key), name, State::NotReady, b"EOF")?; Ok(Vec::new()) }
                    Read::Failed(code) => { self.not_ready(Some(&key), name, State::Error, code.to_string().as_bytes())?; Ok(Vec::new()) }
                }
            }
        }
    }

    /// CHAROUT: write `string` as it is; neither string nor start closes
    /// the stream. Returns the characters not written.
    pub fn charout(&mut self, name: &[u8], string: Option<&[u8]>, start: Option<u64>) -> R<u64> {
        let len = string.map_or(0, |s| s.len() as u64);
        match which(name, true) {
            Which::StdOut => { if let Some(s) = string { self.host.write_out(s); } Ok(0) }
            Which::StdErr => { if let Some(s) = string { self.host.write_err(s); } Ok(0) }
            Which::StdIn => { self.not_ready(None, name, State::Error, b"")?; Ok(len) }
            Which::File if string.is_none() && start.is_none() => { self.close_stream(name); Ok(0) }
            Which::File => Ok(if self.write_file(name, string, start, None)? { 0 } else { len }),
        }
    }

    /// CHARS: the characters left to read.
    pub fn chars(&mut self, name: &[u8]) -> u64 {
        match which(name, false) {
            Which::StdIn => { self.fill_input(1); self.streams.input.len() as u64 }
            Which::StdOut | Which::StdErr => 0,
            Which::File => {
                let Ok(key) = self.file_stream(name, false) else { return 0 };
                let size = self.file_size_of(&key);
                size.saturating_sub(self.streams.files.get(&key).map_or(0, |s| s.read_pos))
            }
        }
    }

    /// LINES: whether lines are left to read (1 or 0), or, with `count`,
    /// how many.
    pub fn lines(&mut self, name: &[u8], count: bool) -> u64 {
        match which(name, false) {
            Which::StdIn => { self.fill_input(1); u64::from(!self.streams.input.is_empty()) }
            Which::StdOut | Which::StdErr => 0,
            Which::File => {
                let Ok(key) = self.file_stream(name, false) else { return 0 };
                let size = self.file_size_of(&key);
                let pos = self.streams.files.get(&key).map_or(0, |s| s.read_pos);
                if !count { return u64::from(pos < size); }
                let h = self.handle_of(&key);
                self.count_lines(h, pos, size)
            }
        }
    }

    /// STREAM(name, 'S' | 'D'): the stream's state, or with its reason.
    fn stream_state(&mut self, name: &[u8], describe: bool) -> Vec<u8> {
        let (state, reason) = match which(name, false) {
            Which::File => {
                let key = self.host.full_name(name.trim_ascii()).map(|f| f.to_ascii_uppercase());
                match key.and_then(|k| self.streams.files.get(&k)) {
                    Some(s) => (s.state.word(), s.reason.clone()),
                    None => ("UNKNOWN", Vec::new()),
                }
            }
            _ => ("READY", Vec::new()),
        };
        let mut out = state.as_bytes().to_vec();
        if describe {
            out.push(b':');
            out.extend(reason);
        }
        out
    }

    /// STREAM(name, 'C', command).
    fn stream_command(&mut self, fname: &[u8], name: &[u8], command: &[u8]) -> R<Vec<u8>> {
        let words: Vec<Vec<u8>> = command.split(|&b| b == b' ').filter(|w| !w.is_empty()).map(<[u8]>::to_ascii_uppercase).collect();
        let word = |k: usize| words.get(k).map_or(&b""[..], Vec::as_slice);
        let file = which(name, false) == Which::File;
        match word(0) {
            b"OPEN" => {
                let (mut mode, mut replace) = (None, false);
                for w in &words[1..] {
                    match w.as_slice() {
                        b"READ" => mode = Some(false),
                        b"WRITE" | b"BOTH" => mode = Some(true),
                        b"REPLACE" => replace = true,
                        b"APPEND" => replace = false,
                        _ => return Err(bad(fname, "an unknown OPEN option")),
                    }
                }
                if !file { return Ok(b"READY:".to_vec()); }
                self.close_stream(name);
                let write = mode.unwrap_or(true);
                if replace && write {
                    // Empty it first; then it opens as any written stream.
                    match self.host.full_name(name.trim_ascii()).ok_or(ERROR_PATH_NOT_FOUND)
                        .and_then(|full| self.host.file_open(&full, OpenFor::Replace)) {
                        Ok(h) => self.host.file_close(h),
                        Err(code) => return Ok(format!("NOTREADY:{code}").into_bytes()),
                    }
                }
                Ok(match self.file_stream(name, write) {
                    Ok(_) => b"READY:".to_vec(),
                    Err(code) => format!("NOTREADY:{code}").into_bytes(),
                })
            }
            b"CLOSE" => { if file { self.close_stream(name); } Ok(b"READY:".to_vec()) }
            b"FLUSH" => Ok(b"READY:".to_vec()),
            b"SEEK" | b"POSITION" => self.seek(fname, name, &words[1..]),
            b"QUERY" => self.query(fname, name, &words[1..]),
            _ => Err(bad(fname, "an unknown stream command")),
        }
    }

    /// `SEEK [= < + -]n [READ | WRITE] [CHAR | LINE]`: move the read and
    /// write positions (both when neither is named, unverified); returns
    /// the new position. Lines only from the start (`=`).
    fn seek(&mut self, fname: &[u8], name: &[u8], words: &[Vec<u8>]) -> R<Vec<u8>> {
        let Some(offset) = words.first() else { return Err(bad(fname, "SEEK needs a position")) };
        let (how, digits) = match offset.first() {
            Some(c @ (b'=' | b'<' | b'+' | b'-')) => (*c, &offset[1..]),
            _ => (b'=', &offset[..]),
        };
        let n = self.whole(digits).ok().filter(|&n| n >= 0).ok_or_else(|| bad(fname, "a position must be a whole number"))? as u64;
        let (mut read, mut write, mut lines) = (false, false, false);
        for w in &words[1..] {
            match w.as_slice() {
                b"READ" => read = true,
                b"WRITE" => write = true,
                b"CHAR" => lines = false,
                b"LINE" => lines = true,
                _ => return Err(bad(fname, "an unknown SEEK option")),
            }
        }
        if !read && !write { (read, write) = (true, true); }
        if which(name, false) != Which::File { return Ok(Vec::new()); }
        let Ok(key) = self.file_stream(name, write) else { return Ok(Vec::new()) };
        let h = self.handle_of(&key);
        let size = self.file_size_of(&key);
        let current = self.streams.files.get(&key).map_or(0, |s| if read { s.read_pos } else { s.write_pos.unwrap_or(size) });
        let target = match (how, lines) {
            (b'=', true) => match self.line_start(h, n.max(1)) { Some(p) => p, None => return Ok(Vec::new()) },
            (_, true) => return Err(bad(fname, "lines are sought only from the start")),
            (b'=', false) => n.saturating_sub(1),
            (b'<', false) => size.saturating_sub(n),
            (b'+', false) => current.saturating_add(n),
            _ => current.saturating_sub(n),
        };
        if let Some(s) = self.streams.files.get_mut(&key) {
            if read { s.read_pos = target; }
            if write { s.write_pos = Some(target); }
        }
        Ok(whole_out(target as i64 + 1))
    }

    /// `QUERY EXISTS | SIZE | DATETIME | TIMESTAMP | HANDLE | STREAMTYPE
    /// | SEEK | POSITION …`.
    fn query(&mut self, fname: &[u8], name: &[u8], words: &[Vec<u8>]) -> R<Vec<u8>> {
        let file = which(name, false) == Which::File;
        let what = words.first().map_or(&b""[..], Vec::as_slice);
        let info = if file { self.host.file_info(name.trim_ascii()).filter(|i| i.attributes & ATTR_DIRECTORY == 0) } else { None };
        let key = if file { self.host.full_name(name.trim_ascii()).map(|f| f.to_ascii_uppercase()) } else { None };
        Ok(match what {
            b"EXISTS" => info.map(|i| i.name).unwrap_or_default(),
            b"SIZE" => info.map(|i| whole_out(i.size as i64)).unwrap_or_default(),
            b"DATETIME" => info.map(|i| {
                let (y, mo, d, h, mi, s) = unpack_date_time(i.date, i.time);
                format!("{mo:02}-{d:02}-{:02} {h:02}:{mi:02}:{s:02}", y % 100).into_bytes()
            }).unwrap_or_default(),
            b"TIMESTAMP" => info.map(|i| {
                let (y, mo, d, h, mi, s) = unpack_date_time(i.date, i.time);
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}").into_bytes()
            }).unwrap_or_default(),
            b"HANDLE" => key.and_then(|k| self.streams.files.get(&k)?.handle).map(|h| whole_out(h as i64)).unwrap_or_default(),
            b"STREAMTYPE" => match (file, info.is_some()) {
                (false, _) => b"TRANSIENT".to_vec(),
                (true, true) => b"PERSISTENT".to_vec(),
                (true, false) => b"UNKNOWN".to_vec(),
            },
            b"SEEK" | b"POSITION" => {
                let write = words.iter().any(|w| w == b"WRITE");
                let lines = words.iter().any(|w| w == b"LINE");
                let Some(key) = key.filter(|k| self.streams.files.get(k).is_some_and(|s| s.handle.is_some())) else { return Ok(Vec::new()) };
                let size = self.file_size_of(&key);
                let pos = self.streams.files.get(&key).map_or(0, |s| if write { s.write_pos.unwrap_or(size) } else { s.read_pos });
                if lines {
                    let h = self.handle_of(&key);
                    whole_out(self.count_lines(h, 0, pos) as i64 + 1)
                } else {
                    whole_out(pos as i64 + 1)
                }
            }
            _ => return Err(bad(fname, "an unknown QUERY")),
        })
    }
}

// ── The built-in functions ───────────────────────────────────────────────

/// A position argument (from 1), when given.
fn position_arg(i: &Interp, n: &[u8], a: Args, k: usize) -> R<Option<u64>> {
    Ok(match opt(a, k) { Some(_) => Some(whole_arg(i, n, a, k, 1, 1)? as u64), None => None })
}

/// A stream name argument, which may be omitted.
fn name_arg(a: Args<'_>) -> &[u8] { opt(a, 0).unwrap_or_default() }

pub(super) fn charin(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 3)?;
    let start = position_arg(i, n, a, 1)?;
    let length = whole_arg(i, n, a, 2, 0, 1)?;
    if length as u64 > MAX_STRING as u64 { return Err(RexxError::with(5, "a string is too long")); }
    if start.is_some() && which(name_arg(a), false) != Which::File { return Err(bad(n, "a standard stream has no positions")); }
    i.charin(name_arg(a), start, length as usize)
}

pub(super) fn charout(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 3)?;
    let start = position_arg(i, n, a, 2)?;
    i.charout(name_arg(a), opt(a, 1), start).map(|left| whole_out(left as i64))
}

pub(super) fn chars(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 1)?;
    Ok(whole_out(i.chars(name_arg(a)) as i64))
}

pub(super) fn linein(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 3)?;
    let line = position_arg(i, n, a, 1)?;
    let lines = whole_arg(i, n, a, 2, 0, 1)?;
    if lines > 1 { return Err(bad(n, "the count must be 0 or 1")); }
    if line.is_some() && which(name_arg(a), false) != Which::File { return Err(bad(n, "a standard stream has no positions")); }
    i.linein(name_arg(a), line, lines as u64)
}

pub(super) fn lineout(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 3)?;
    let line = position_arg(i, n, a, 2)?;
    i.lineout(name_arg(a), opt(a, 1), line).map(|left| whole_out(left as i64))
}

pub(super) fn lines(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 2)?;
    let count_them = option_arg(n, a, 1, b'N', b"NC")? == b'C';
    Ok(whole_out(i.lines(name_arg(a), count_them) as i64))
}

pub(super) fn stream(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 3)?;
    let name = req(a, 0);
    match option_arg(n, a, 1, b'S', b"SDC")? {
        b'C' => match opt(a, 2) {
            Some(command) => i.stream_command(n, name, command),
            None => Err(bad(n, "a command is needed")),
        },
        op => {
            if opt(a, 2).is_some() { return Err(bad(n, "a command only goes with 'C'")); }
            Ok(i.stream_state(name, op == b'D'))
        }
    }
}
