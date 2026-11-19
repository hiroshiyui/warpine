// SPDX-License-Identifier: GPL-3.0-only

//! The desktop protocol (ADR-0052): the messages a Warpine process (client)
//! and the desktop server exchange over their `SOCK_SEQPACKET` socket.
//!
//! Every message is one packet: a `u16` tag followed by fixed-layout
//! little-endian fields. Decoding is strict and bounded — a short packet,
//! trailing bytes, an unknown tag, a count or length over its limit or text
//! that is not UTF-8 is an error, and the receiver drops the connection.
//! The server treats every client packet as hostile; the client checks the
//! server's packets the same way.

/// Protocol version, checked in `Hello` / `Welcome`.
pub const PROTOCOL_VERSION: u32 = 3;
/// Largest packet either side sends or accepts.
pub const MAX_MESSAGE: usize = 128 * 1024;
/// Most items (frame pictures and pull-downs) in one `Update`.
pub const MAX_ITEMS: usize = 512;
/// Most painted parts of one item.
pub const MAX_PARTS: usize = 8;
/// Most damaged rectangles in one `Update`.
pub const MAX_DAMAGE: usize = 256;
/// Most frames named as raised (or as lowered) in one `Update`.
pub const MAX_RAISED: usize = 64;
/// Longest window class name, in bytes.
pub const MAX_NAME: usize = 256;
/// Longest text (clipboard, typed text), in bytes.
pub const MAX_TEXT: usize = 64 * 1024;
/// Largest pointer image side sent through the socket.
pub const MAX_POINTER_SIDE: u32 = 128;
/// Every coordinate and size lies within ±`MAX_COORD`, so sums of a few of
/// them cannot overflow an `i32`.
pub const MAX_COORD: i32 = 1 << 24;

/// A rectangle in pels: `x`, `y` of its top-left corner, width and height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Area {
        Area { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y
            && (px as i64) < self.x as i64 + self.w as i64
            && (py as i64) < self.y as i64 + self.h as i64
    }
}

/// One picture of a client on the desktop: a frame (with its chrome) or an
/// open pull-down, held in pixmap `pixmap` and shown with its top-left pel
/// at desktop point `(x, y)` (top-left origin). Only the `parts` (pixmap
/// coordinates) are painted; the rest shows what lies below.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Item {
    /// The frame window it belongs to (a pull-down: its frame).
    pub frame: u32,
    pub pixmap: u32,
    pub x: i32,
    pub y: i32,
    pub parts: Vec<Area>,
}

/// What a client shows now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Update {
    /// Its active frame, 0 if none.
    pub active: u32,
    /// Frames brought to the top since the last update, in that order.
    pub raised: Vec<u32>,
    /// Frames sent to the bottom since the last update, in that order.
    pub lowered: Vec<u32>,
    /// Its pictures, back to front.
    pub items: Vec<Item>,
    /// Pixmap areas whose pels changed since the last update.
    pub damage: Vec<(u32, Area)>,
    /// An open pop-up menu, drawn over everything; while it is open the
    /// client gets all input.
    pub overlay: Option<Item>,
}

/// The mouse pointer a client asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pointer {
    Hidden,
    /// A system pointer, `gui::pointer::SystemPointer` by index.
    System(u8),
    /// An RGBA image with its hot spot.
    Image { width: u32, height: u32, hot_x: i32, hot_y: i32, rgba: Vec<u8> },
}

/// A call on a window of another program (ADR-0052 phase B), made by the
/// caller's API thread and answered by the program the window belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WinCall {
    /// `WinPostMsg`: queue the message for the window.
    Post { msg: u32, mp1: u32, mp2: u32 },
    /// Describe the window ([`WindowInfo`]).
    Describe,
    /// Read a window word of `size` (2 or 4) bytes at `index`.
    Word { index: i32, size: u8 },
    /// `WinQueryWindow(hwnd, code)`.
    Query { code: i32 },
    /// `WinSetWindowPos`.
    SetPos { behind: u32, x: i32, y: i32, cx: i32, cy: i32, fl: u32 },
    /// `WinShowWindow`.
    Show { show: bool },
    /// `WinSetActiveWindow`.
    Activate,
    /// `WinSendMsg`: the window's thread runs its window procedure; the
    /// answer is the procedure's result. Waits for as long as that takes.
    Send { msg: u32, mp1: u32, mp2: u32 },
    /// `WinSetWindowText`.
    SetText { text: String },
}

impl WinCall {
    /// Whether the server gives up on the call after `CALL_EXPIRY`: every
    /// call but `Send`, whose window procedure may run for any time.
    pub fn expires(&self) -> bool {
        !matches!(self, WinCall::Send { .. })
    }
}

/// What a program says about one of its windows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowInfo {
    /// OS/2 process and thread of the window.
    pub pid: u32,
    pub tid: u32,
    pub parent: u32,
    pub owner: u32,
    /// `QWL_STYLE`, `WS_VISIBLE` as the window is now.
    pub style: u32,
    pub id: u32,
    /// Position (PM coordinates, relative to the parent) and size.
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    /// `SWP_*` flags `WinQueryWindowPos` reports.
    pub swp_fl: u32,
    /// `WinIsWindowVisible`: it and its ancestors are visible.
    pub showing: bool,
    pub class: String,
    pub text: String,
}

/// The answer to a [`WinCall`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WinReply {
    /// No such window, or the call failed (the caller reports
    /// `PMERR_INVALID_HWND` or FALSE).
    Failed,
    /// The call's value (BOOL, handle, window word).
    Value(u32),
    Info(Box<WindowInfo>),
}

/// Client → server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    Hello { version: u32 },
    /// A new pixmap; the packet carries its memfd.
    NewPixmap { id: u32, width: u32, height: u32 },
    DropPixmap { id: u32 },
    Update(Update),
    /// Input event `token` was taken; `active` is the client's active
    /// frame now.
    Ack { token: u64, active: u32 },
    Clipboard(String),
    /// Keep the pointer while a button is not held (a drag).
    Capture(bool),
    Pointer(Pointer),
    WarpPointer { x: i32, y: i32 },
    /// Where the host input method's window goes: a desktop rectangle and
    /// the caret offset in it.
    TextArea { area: Area, cursor: i32 },
    /// Everything up to `Flush { token }` is shown.
    FlushDone { token: u64 },
    /// Make `call` on window `hwnd` of another program; the answer comes
    /// back as `Answered { id }`.
    Call { id: u32, hwnd: u32, call: WinCall },
    /// The answer to `Called { id }`.
    Answer { id: u64, reply: WinReply },
}

/// One input event, in desktop coordinates (top-left origin).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// The pointer moved by `(dx, dy)` to `(x, y)`; `buttons` the SDL
    /// button mask.
    Motion { x: i32, y: i32, dx: i32, dy: i32, buttons: u32 },
    /// SDL button number, its click count.
    Button { down: bool, button: u8, clicks: u8, x: i32, y: i32 },
    /// SDL scancode, keycode and modifier bits.
    Key { down: bool, scancode: i32, keycode: u32, mods: u16, repeat: bool },
    Text(String),
}

/// Server → client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMsg {
    Welcome { version: u32, slot: u32, width: u32, height: u32 },
    /// Handle this input and acknowledge `token` once it is taken.
    Input { token: u64, input: Input },
    /// Frame `frame` is no longer the active window: `new_active`, another
    /// client's, is.
    Deactivate { frame: u32, new_active: u32 },
    Resized { width: u32, height: u32 },
    Clipboard(String),
    /// Send what you show now, then `FlushDone { token }`.
    Flush { token: u64 },
    /// The desktop is closing: end.
    Quit,
    /// Another program makes `call` on this program's window `hwnd`:
    /// answer with `Answer { id }`.
    Called { id: u64, hwnd: u32, call: WinCall },
    /// The answer to this program's `Call { id }`.
    Answered { id: u32, reply: WinReply },
    /// The session's active frame is now `frame` (0: none).
    Active { frame: u32 },
}

/// Why a packet was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Short,
    Trailing,
    UnknownTag(u16),
    TooMany,
    BadText,
    BadValue,
    TooLarge,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for DecodeError {}

// ── encoding ────────────────────────────────────────────────────────────────

struct Writer(Vec<u8>);

impl Writer {
    fn new(tag: u16) -> Writer {
        let mut w = Writer(Vec::with_capacity(64));
        w.u16(tag);
        w
    }
    fn u8(&mut self, v: u8) { self.0.push(v); }
    fn u16(&mut self, v: u16) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn u32(&mut self, v: u32) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn i32(&mut self, v: i32) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn u64(&mut self, v: u64) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn bool(&mut self, v: bool) { self.u8(v as u8); }
    /// Text cut to `MAX_TEXT` bytes at a character boundary.
    fn text(&mut self, s: &str) {
        self.text_max(s, MAX_TEXT);
    }
    /// Text cut to `max` bytes at a character boundary.
    fn text_max(&mut self, s: &str, max: usize) {
        let mut end = s.len().min(max);
        while !s.is_char_boundary(end) { end -= 1; }
        self.u32(end as u32);
        self.0.extend_from_slice(&s.as_bytes()[..end]);
    }
    fn area(&mut self, a: &Area) {
        self.i32(a.x); self.i32(a.y); self.i32(a.w); self.i32(a.h);
    }
    fn item(&mut self, it: &Item) {
        self.u32(it.frame); self.u32(it.pixmap); self.i32(it.x); self.i32(it.y);
        let parts = &it.parts[..it.parts.len().min(MAX_PARTS)];
        self.u16(parts.len() as u16);
        for p in parts { self.area(p); }
    }
    fn handles(&mut self, hs: &[u32]) {
        let hs = &hs[..hs.len().min(MAX_RAISED)];
        self.u16(hs.len() as u16);
        for h in hs { self.u32(*h); }
    }
    fn call(&mut self, c: &WinCall) {
        match c {
            WinCall::Post { msg, mp1, mp2 } => { self.u8(tag::CALL_POST); self.u32(*msg); self.u32(*mp1); self.u32(*mp2); }
            WinCall::Describe => self.u8(tag::CALL_DESCRIBE),
            WinCall::Word { index, size } => { self.u8(tag::CALL_WORD); self.i32(*index); self.u8(*size); }
            WinCall::Query { code } => { self.u8(tag::CALL_QUERY); self.i32(*code); }
            WinCall::SetPos { behind, x, y, cx, cy, fl } => {
                self.u8(tag::CALL_SETPOS);
                self.u32(*behind); self.i32(*x); self.i32(*y); self.i32(*cx); self.i32(*cy); self.u32(*fl);
            }
            WinCall::Show { show } => { self.u8(tag::CALL_SHOW); self.bool(*show); }
            WinCall::Activate => self.u8(tag::CALL_ACTIVATE),
            WinCall::Send { msg, mp1, mp2 } => { self.u8(tag::CALL_SEND); self.u32(*msg); self.u32(*mp1); self.u32(*mp2); }
            WinCall::SetText { text } => { self.u8(tag::CALL_SETTEXT); self.text(text); }
        }
    }
    fn reply(&mut self, r: &WinReply) {
        match r {
            WinReply::Failed => self.u8(tag::REPLY_FAILED),
            WinReply::Value(v) => { self.u8(tag::REPLY_VALUE); self.u32(*v); }
            WinReply::Info(i) => {
                self.u8(tag::REPLY_INFO);
                for v in [i.pid, i.tid, i.parent, i.owner, i.style, i.id] { self.u32(v); }
                for v in [i.x, i.y, i.cx, i.cy] { self.i32(v); }
                self.u32(i.swp_fl);
                self.bool(i.showing);
                self.text_max(&i.class, MAX_NAME);
                self.text(&i.text);
            }
        }
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Short)?;
        let s = self.buf.get(self.pos..end).ok_or(DecodeError::Short)?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> { Ok(self.take(1)?[0]) }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2 bytes")))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }
    fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::BadValue),
        }
    }
    fn text(&mut self) -> Result<String, DecodeError> {
        self.text_max(MAX_TEXT)
    }
    fn text_max(&mut self, max: usize) -> Result<String, DecodeError> {
        let n = self.u32()? as usize;
        if n > max { return Err(DecodeError::TooLarge); }
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| DecodeError::BadText)
    }
    fn count(&mut self, max: usize) -> Result<usize, DecodeError> {
        let n = self.u16()? as usize;
        if n > max { return Err(DecodeError::TooMany); }
        Ok(n)
    }
    /// A coordinate within ±`MAX_COORD`.
    fn coord(&mut self) -> Result<i32, DecodeError> {
        let v = self.i32()?;
        if !(-MAX_COORD..=MAX_COORD).contains(&v) { return Err(DecodeError::BadValue); }
        Ok(v)
    }
    fn area(&mut self) -> Result<Area, DecodeError> {
        let a = Area { x: self.coord()?, y: self.coord()?, w: self.coord()?, h: self.coord()? };
        if a.w < 0 || a.h < 0 { return Err(DecodeError::BadValue); }
        Ok(a)
    }
    fn item(&mut self) -> Result<Item, DecodeError> {
        let (frame, pixmap, x, y) = (self.u32()?, self.u32()?, self.coord()?, self.coord()?);
        let n = self.count(MAX_PARTS)?;
        let parts = (0..n).map(|_| self.area()).collect::<Result<_, _>>()?;
        Ok(Item { frame, pixmap, x, y, parts })
    }
    fn handles(&mut self) -> Result<Vec<u32>, DecodeError> {
        let n = self.count(MAX_RAISED)?;
        (0..n).map(|_| self.u32()).collect()
    }
    fn call(&mut self) -> Result<WinCall, DecodeError> {
        Ok(match self.u8()? {
            tag::CALL_POST => WinCall::Post { msg: self.u32()?, mp1: self.u32()?, mp2: self.u32()? },
            tag::CALL_DESCRIBE => WinCall::Describe,
            tag::CALL_WORD => {
                let index = self.i32()?;
                let size = self.u8()?;
                if size != 2 && size != 4 { return Err(DecodeError::BadValue); }
                WinCall::Word { index, size }
            }
            tag::CALL_QUERY => WinCall::Query { code: self.i32()? },
            tag::CALL_SETPOS => WinCall::SetPos { behind: self.u32()?, x: self.coord()?, y: self.coord()?,
                                                  cx: self.coord()?, cy: self.coord()?, fl: self.u32()? },
            tag::CALL_SHOW => WinCall::Show { show: self.bool()? },
            tag::CALL_ACTIVATE => WinCall::Activate,
            tag::CALL_SEND => WinCall::Send { msg: self.u32()?, mp1: self.u32()?, mp2: self.u32()? },
            tag::CALL_SETTEXT => WinCall::SetText { text: self.text()? },
            _ => return Err(DecodeError::BadValue),
        })
    }
    fn reply(&mut self) -> Result<WinReply, DecodeError> {
        Ok(match self.u8()? {
            tag::REPLY_FAILED => WinReply::Failed,
            tag::REPLY_VALUE => WinReply::Value(self.u32()?),
            tag::REPLY_INFO => {
                let mut i = WindowInfo {
                    pid: self.u32()?, tid: self.u32()?, parent: self.u32()?, owner: self.u32()?,
                    style: self.u32()?, id: self.u32()?, ..WindowInfo::default()
                };
                (i.x, i.y, i.cx, i.cy) = (self.coord()?, self.coord()?, self.coord()?, self.coord()?);
                i.swp_fl = self.u32()?;
                i.showing = self.bool()?;
                i.class = self.text_max(MAX_NAME)?;
                i.text = self.text()?;
                WinReply::Info(Box::new(i))
            }
            _ => return Err(DecodeError::BadValue),
        })
    }
    fn end(&self) -> Result<(), DecodeError> {
        if self.pos == self.buf.len() { Ok(()) } else { Err(DecodeError::Trailing) }
    }
}

fn finish(w: Writer) -> Vec<u8> {
    debug_assert!(w.0.len() <= MAX_MESSAGE, "desktop message over MAX_MESSAGE");
    w.0
}

/// Start reading a packet: its tag and a reader for the rest.
fn open(buf: &[u8]) -> Result<(u16, Reader<'_>), DecodeError> {
    if buf.len() > MAX_MESSAGE { return Err(DecodeError::TooLarge); }
    let mut r = Reader { buf, pos: 0 };
    let tag = r.u16()?;
    Ok((tag, r))
}

mod tag {
    pub const HELLO: u16 = 1;
    pub const NEW_PIXMAP: u16 = 2;
    pub const DROP_PIXMAP: u16 = 3;
    pub const UPDATE: u16 = 4;
    pub const ACK: u16 = 5;
    pub const CLIPBOARD: u16 = 6;
    pub const CAPTURE: u16 = 7;
    pub const POINTER: u16 = 8;
    pub const WARP: u16 = 9;
    pub const TEXT_AREA: u16 = 10;
    pub const FLUSH_DONE: u16 = 11;
    pub const CALL: u16 = 12;
    pub const ANSWER: u16 = 13;

    pub const WELCOME: u16 = 100;
    pub const INPUT: u16 = 101;
    pub const DEACTIVATE: u16 = 102;
    pub const RESIZED: u16 = 103;
    pub const S_CLIPBOARD: u16 = 104;
    pub const FLUSH: u16 = 105;
    pub const QUIT: u16 = 106;
    pub const CALLED: u16 = 107;
    pub const ANSWERED: u16 = 108;
    pub const ACTIVE: u16 = 109;

    pub const CALL_POST: u8 = 1;
    pub const CALL_DESCRIBE: u8 = 2;
    pub const CALL_WORD: u8 = 3;
    pub const CALL_QUERY: u8 = 4;
    pub const CALL_SETPOS: u8 = 5;
    pub const CALL_ACTIVATE: u8 = 6;
    pub const CALL_SHOW: u8 = 7;
    pub const CALL_SEND: u8 = 8;
    pub const CALL_SETTEXT: u8 = 9;

    pub const REPLY_FAILED: u8 = 0;
    pub const REPLY_VALUE: u8 = 1;
    pub const REPLY_INFO: u8 = 2;

    pub const IN_MOTION: u8 = 1;
    pub const IN_BUTTON: u8 = 2;
    pub const IN_KEY: u8 = 3;
    pub const IN_TEXT: u8 = 4;

    pub const PTR_HIDDEN: u8 = 0;
    pub const PTR_SYSTEM: u8 = 1;
    pub const PTR_IMAGE: u8 = 2;
}

impl ClientMsg {
    /// The packet for this message. Lists over their limits are cut (the
    /// sender keeps within them; see `RemoteDesktop`).
    pub fn encode(&self) -> Vec<u8> {
        let mut w;
        match self {
            ClientMsg::Hello { version } => { w = Writer::new(tag::HELLO); w.u32(*version); }
            ClientMsg::NewPixmap { id, width, height } => {
                w = Writer::new(tag::NEW_PIXMAP); w.u32(*id); w.u32(*width); w.u32(*height);
            }
            ClientMsg::DropPixmap { id } => { w = Writer::new(tag::DROP_PIXMAP); w.u32(*id); }
            ClientMsg::Update(u) => {
                w = Writer::new(tag::UPDATE);
                w.u32(u.active);
                w.handles(&u.raised);
                w.handles(&u.lowered);
                let items = &u.items[..u.items.len().min(MAX_ITEMS)];
                w.u16(items.len() as u16);
                for it in items { w.item(it); }
                let damage = &u.damage[..u.damage.len().min(MAX_DAMAGE)];
                w.u16(damage.len() as u16);
                for (id, a) in damage { w.u32(*id); w.area(a); }
                match &u.overlay {
                    None => w.bool(false),
                    Some(it) => { w.bool(true); w.item(it); }
                }
            }
            ClientMsg::Ack { token, active } => { w = Writer::new(tag::ACK); w.u64(*token); w.u32(*active); }
            ClientMsg::Clipboard(s) => { w = Writer::new(tag::CLIPBOARD); w.text(s); }
            ClientMsg::Capture(on) => { w = Writer::new(tag::CAPTURE); w.bool(*on); }
            ClientMsg::Pointer(p) => {
                w = Writer::new(tag::POINTER);
                match p {
                    Pointer::Hidden => w.u8(tag::PTR_HIDDEN),
                    Pointer::System(n) => { w.u8(tag::PTR_SYSTEM); w.u8(*n); }
                    Pointer::Image { width, height, hot_x, hot_y, rgba } => {
                        w.u8(tag::PTR_IMAGE);
                        w.u32(*width); w.u32(*height); w.i32(*hot_x); w.i32(*hot_y);
                        w.u32(rgba.len() as u32);
                        w.0.extend_from_slice(rgba);
                    }
                }
            }
            ClientMsg::WarpPointer { x, y } => { w = Writer::new(tag::WARP); w.i32(*x); w.i32(*y); }
            ClientMsg::TextArea { area, cursor } => { w = Writer::new(tag::TEXT_AREA); w.area(area); w.i32(*cursor); }
            ClientMsg::FlushDone { token } => { w = Writer::new(tag::FLUSH_DONE); w.u64(*token); }
            ClientMsg::Call { id, hwnd, call } => { w = Writer::new(tag::CALL); w.u32(*id); w.u32(*hwnd); w.call(call); }
            ClientMsg::Answer { id, reply } => { w = Writer::new(tag::ANSWER); w.u64(*id); w.reply(reply); }
        }
        finish(w)
    }

    pub fn decode(buf: &[u8]) -> Result<ClientMsg, DecodeError> {
        let (t, mut r) = open(buf)?;
        let m = match t {
            tag::HELLO => ClientMsg::Hello { version: r.u32()? },
            tag::NEW_PIXMAP => ClientMsg::NewPixmap { id: r.u32()?, width: r.u32()?, height: r.u32()? },
            tag::DROP_PIXMAP => ClientMsg::DropPixmap { id: r.u32()? },
            tag::UPDATE => {
                let active = r.u32()?;
                let raised = r.handles()?;
                let lowered = r.handles()?;
                let n = r.count(MAX_ITEMS)?;
                let items = (0..n).map(|_| r.item()).collect::<Result<_, _>>()?;
                let n = r.count(MAX_DAMAGE)?;
                let damage = (0..n).map(|_| Ok((r.u32()?, r.area()?))).collect::<Result<_, _>>()?;
                let overlay = if r.bool()? { Some(r.item()?) } else { None };
                ClientMsg::Update(Update { active, raised, lowered, items, damage, overlay })
            }
            tag::ACK => ClientMsg::Ack { token: r.u64()?, active: r.u32()? },
            tag::CLIPBOARD => ClientMsg::Clipboard(r.text()?),
            tag::CAPTURE => ClientMsg::Capture(r.bool()?),
            tag::POINTER => ClientMsg::Pointer(match r.u8()? {
                tag::PTR_HIDDEN => Pointer::Hidden,
                tag::PTR_SYSTEM => Pointer::System(r.u8()?),
                tag::PTR_IMAGE => {
                    let (width, height, hot_x, hot_y) = (r.u32()?, r.u32()?, r.i32()?, r.i32()?);
                    if width == 0 || height == 0 || width > MAX_POINTER_SIDE || height > MAX_POINTER_SIDE {
                        return Err(DecodeError::BadValue);
                    }
                    let n = r.u32()? as usize;
                    if n != width as usize * height as usize * 4 { return Err(DecodeError::BadValue); }
                    Pointer::Image { width, height, hot_x, hot_y, rgba: r.take(n)?.to_vec() }
                }
                _ => return Err(DecodeError::BadValue),
            }),
            tag::WARP => ClientMsg::WarpPointer { x: r.coord()?, y: r.coord()? },
            tag::TEXT_AREA => ClientMsg::TextArea { area: r.area()?, cursor: r.coord()? },
            tag::FLUSH_DONE => ClientMsg::FlushDone { token: r.u64()? },
            tag::CALL => ClientMsg::Call { id: r.u32()?, hwnd: r.u32()?, call: r.call()? },
            tag::ANSWER => ClientMsg::Answer { id: r.u64()?, reply: r.reply()? },
            other => return Err(DecodeError::UnknownTag(other)),
        };
        r.end()?;
        Ok(m)
    }
}

impl ServerMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w;
        match self {
            ServerMsg::Welcome { version, slot, width, height } => {
                w = Writer::new(tag::WELCOME); w.u32(*version); w.u32(*slot); w.u32(*width); w.u32(*height);
            }
            ServerMsg::Input { token, input } => {
                w = Writer::new(tag::INPUT);
                w.u64(*token);
                match input {
                    Input::Motion { x, y, dx, dy, buttons } => {
                        w.u8(tag::IN_MOTION); w.i32(*x); w.i32(*y); w.i32(*dx); w.i32(*dy); w.u32(*buttons);
                    }
                    Input::Button { down, button, clicks, x, y } => {
                        w.u8(tag::IN_BUTTON); w.bool(*down); w.u8(*button); w.u8(*clicks); w.i32(*x); w.i32(*y);
                    }
                    Input::Key { down, scancode, keycode, mods, repeat } => {
                        w.u8(tag::IN_KEY); w.bool(*down); w.i32(*scancode); w.u32(*keycode); w.u16(*mods);
                        w.bool(*repeat);
                    }
                    Input::Text(s) => { w.u8(tag::IN_TEXT); w.text(s); }
                }
            }
            ServerMsg::Deactivate { frame, new_active } => {
                w = Writer::new(tag::DEACTIVATE); w.u32(*frame); w.u32(*new_active);
            }
            ServerMsg::Resized { width, height } => { w = Writer::new(tag::RESIZED); w.u32(*width); w.u32(*height); }
            ServerMsg::Clipboard(s) => { w = Writer::new(tag::S_CLIPBOARD); w.text(s); }
            ServerMsg::Flush { token } => { w = Writer::new(tag::FLUSH); w.u64(*token); }
            ServerMsg::Quit => w = Writer::new(tag::QUIT),
            ServerMsg::Called { id, hwnd, call } => { w = Writer::new(tag::CALLED); w.u64(*id); w.u32(*hwnd); w.call(call); }
            ServerMsg::Answered { id, reply } => { w = Writer::new(tag::ANSWERED); w.u32(*id); w.reply(reply); }
            ServerMsg::Active { frame } => { w = Writer::new(tag::ACTIVE); w.u32(*frame); }
        }
        finish(w)
    }

    pub fn decode(buf: &[u8]) -> Result<ServerMsg, DecodeError> {
        let (t, mut r) = open(buf)?;
        let m = match t {
            tag::WELCOME => ServerMsg::Welcome { version: r.u32()?, slot: r.u32()?, width: r.u32()?, height: r.u32()? },
            tag::INPUT => {
                let token = r.u64()?;
                let input = match r.u8()? {
                    tag::IN_MOTION => Input::Motion { x: r.coord()?, y: r.coord()?, dx: r.coord()?,
                                                      dy: r.coord()?, buttons: r.u32()? },
                    tag::IN_BUTTON => Input::Button { down: r.bool()?, button: r.u8()?, clicks: r.u8()?,
                                                      x: r.coord()?, y: r.coord()? },
                    tag::IN_KEY => Input::Key { down: r.bool()?, scancode: r.i32()?, keycode: r.u32()?,
                                                mods: r.u16()?, repeat: r.bool()? },
                    tag::IN_TEXT => Input::Text(r.text()?),
                    _ => return Err(DecodeError::BadValue),
                };
                ServerMsg::Input { token, input }
            }
            tag::DEACTIVATE => ServerMsg::Deactivate { frame: r.u32()?, new_active: r.u32()? },
            tag::RESIZED => ServerMsg::Resized { width: r.u32()?, height: r.u32()? },
            tag::S_CLIPBOARD => ServerMsg::Clipboard(r.text()?),
            tag::FLUSH => ServerMsg::Flush { token: r.u64()? },
            tag::QUIT => ServerMsg::Quit,
            tag::CALLED => ServerMsg::Called { id: r.u64()?, hwnd: r.u32()?, call: r.call()? },
            tag::ANSWERED => ServerMsg::Answered { id: r.u32()?, reply: r.reply()? },
            tag::ACTIVE => ServerMsg::Active { frame: r.u32()? },
            other => return Err(DecodeError::UnknownTag(other)),
        };
        r.end()?;
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> WindowInfo {
        WindowInfo { pid: 3, tid: 1, parent: 1, owner: 0, style: 0x8000_0000, id: 0x8008, x: -5, y: 6, cx: 7,
                     cy: 8, swp_fl: 3, showing: true, class: "#1".into(), text: "Programm B — 日本".into() }
    }

    fn client_msgs() -> Vec<ClientMsg> {
        let item = Item { frame: 0x10_1000, pixmap: 3, x: -5, y: 40, parts: vec![Area::new(0, 0, 10, 20)] };
        vec![
            ClientMsg::Hello { version: PROTOCOL_VERSION },
            ClientMsg::NewPixmap { id: 7, width: 640, height: 480 },
            ClientMsg::DropPixmap { id: 7 },
            ClientMsg::Update(Update {
                active: 0x10_1000, raised: vec![0x10_1000], lowered: vec![0x10_1001], items: vec![item.clone()],
                damage: vec![(3, Area::new(1, 2, 3, 4))], overlay: Some(item),
            }),
            ClientMsg::Update(Update::default()),
            ClientMsg::Ack { token: u64::MAX, active: 0 },
            ClientMsg::Clipboard("Grüße 日本".into()),
            ClientMsg::Capture(true),
            ClientMsg::Pointer(Pointer::Hidden),
            ClientMsg::Pointer(Pointer::System(3)),
            ClientMsg::Pointer(Pointer::Image { width: 2, height: 1, hot_x: 0, hot_y: 0, rgba: vec![1; 8] }),
            ClientMsg::WarpPointer { x: 1, y: -1 },
            ClientMsg::TextArea { area: Area::new(1, 2, 3, 4), cursor: 5 },
            ClientMsg::FlushDone { token: 9 },
            ClientMsg::Call { id: 1, hwnd: 0x20_1000, call: WinCall::Post { msg: 0x1001, mp1: 2, mp2: 3 } },
            ClientMsg::Call { id: 2, hwnd: 0x20_1000, call: WinCall::Describe },
            ClientMsg::Call { id: 3, hwnd: 0x20_1000, call: WinCall::Word { index: -2, size: 2 } },
            ClientMsg::Call { id: 4, hwnd: 0x20_1000, call: WinCall::Query { code: 5 } },
            ClientMsg::Call { id: 5, hwnd: 0x20_1000, call: WinCall::SetPos { behind: 3, x: -1, y: 2, cx: 30, cy: 40,
                                                                             fl: 0x8B } },
            ClientMsg::Call { id: 6, hwnd: 0x20_1000, call: WinCall::Activate },
            ClientMsg::Call { id: 6, hwnd: 0x20_1000, call: WinCall::Show { show: false } },
            ClientMsg::Call { id: 7, hwnd: 0x20_1000, call: WinCall::Send { msg: 0x1002, mp1: 4, mp2: 5 } },
            ClientMsg::Call { id: 8, hwnd: 0x20_1000, call: WinCall::SetText { text: "Größe".into() } },
            ClientMsg::Answer { id: 7, reply: WinReply::Failed },
            ClientMsg::Answer { id: 8, reply: WinReply::Value(9) },
            ClientMsg::Answer { id: 9, reply: WinReply::Info(Box::new(info())) },
        ]
    }

    fn server_msgs() -> Vec<ServerMsg> {
        vec![
            ServerMsg::Welcome { version: 1, slot: 2, width: 1024, height: 768 },
            ServerMsg::Input { token: 1, input: Input::Motion { x: 1, y: 2, dx: -1, dy: 0, buttons: 1 } },
            ServerMsg::Input { token: 2, input: Input::Button { down: true, button: 1, clicks: 2, x: 3, y: 4 } },
            ServerMsg::Input { token: 3, input: Input::Key { down: false, scancode: 4, keycode: 97, mods: 1,
                                                              repeat: true } },
            ServerMsg::Input { token: 4, input: Input::Text("é".into()) },
            ServerMsg::Deactivate { frame: 5, new_active: 6 },
            ServerMsg::Resized { width: 800, height: 600 },
            ServerMsg::Clipboard(String::new()),
            ServerMsg::Flush { token: 8 },
            ServerMsg::Quit,
            ServerMsg::Called { id: u64::MAX, hwnd: 0x10_1000, call: WinCall::Describe },
            ServerMsg::Answered { id: 3, reply: WinReply::Info(Box::new(info())) },
            ServerMsg::Answered { id: 4, reply: WinReply::Value(0) },
            ServerMsg::Active { frame: 0x30_1000 },
        ]
    }

    #[test]
    fn every_message_survives_a_round_trip() {
        for m in client_msgs() {
            assert_eq!(ClientMsg::decode(&m.encode()), Ok(m.clone()), "{m:?}");
        }
        for m in server_msgs() {
            assert_eq!(ServerMsg::decode(&m.encode()), Ok(m.clone()), "{m:?}");
        }
    }

    #[test]
    fn every_cut_packet_and_every_extended_one_is_refused() {
        for m in client_msgs() {
            let p = m.encode();
            for n in 0..p.len() {
                assert!(ClientMsg::decode(&p[..n]).is_err(), "{m:?} cut at {n}");
            }
            let mut longer = p.clone();
            longer.push(0);
            assert_eq!(ClientMsg::decode(&longer), Err(DecodeError::Trailing));
        }
        for m in server_msgs() {
            let p = m.encode();
            for n in 0..p.len() {
                assert!(ServerMsg::decode(&p[..n]).is_err(), "{m:?} cut at {n}");
            }
        }
    }

    #[test]
    fn hostile_values_are_refused() {
        // Unknown tags, either way round.
        assert_eq!(ClientMsg::decode(&ServerMsg::Quit.encode()), Err(DecodeError::UnknownTag(106)));
        assert_eq!(ServerMsg::decode(&[1, 0, 1, 0, 0, 0]), Err(DecodeError::UnknownTag(1)));
        // A count over its limit.
        let mut p = vec![4, 0]; // UPDATE
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&((MAX_RAISED + 1) as u16).to_le_bytes());
        assert_eq!(ClientMsg::decode(&p), Err(DecodeError::TooMany));
        // Text that is not UTF-8, and text over its limit.
        let mut p = vec![6, 0, 1, 0, 0, 0, 0xFF];
        assert_eq!(ClientMsg::decode(&p), Err(DecodeError::BadText));
        p.truncate(2);
        p.extend_from_slice(&((MAX_TEXT + 1) as u32).to_le_bytes());
        assert_eq!(ClientMsg::decode(&p), Err(DecodeError::TooLarge));
        // A boolean other than 0 / 1, a negative rectangle, an oversized pointer.
        assert_eq!(ClientMsg::decode(&[7, 0, 2]), Err(DecodeError::BadValue));
        let mut p = ClientMsg::TextArea { area: Area::new(0, 0, 1, 1), cursor: 0 }.encode();
        p[10..14].copy_from_slice(&(-1i32).to_le_bytes());
        assert_eq!(ClientMsg::decode(&p), Err(DecodeError::BadValue));
        let big = ClientMsg::Pointer(Pointer::Image { width: MAX_POINTER_SIDE + 1, height: 1, hot_x: 0, hot_y: 0,
                                                       rgba: vec![0; (MAX_POINTER_SIDE as usize + 1) * 4] });
        assert_eq!(ClientMsg::decode(&big.encode()), Err(DecodeError::BadValue));
        // Coordinates out of range.
        let far = ClientMsg::WarpPointer { x: MAX_COORD + 1, y: 0 };
        assert_eq!(ClientMsg::decode(&far.encode()), Err(DecodeError::BadValue));
        let item = Item { frame: 1, pixmap: 1, x: i32::MIN, y: 0, parts: vec![] };
        let u = ClientMsg::Update(Update { items: vec![item], ..Update::default() });
        assert_eq!(ClientMsg::decode(&u.encode()), Err(DecodeError::BadValue));
        // A window word of an odd size, an unknown call, an overlong class name.
        let mut p = ClientMsg::Call { id: 1, hwnd: 2, call: WinCall::Word { index: 0, size: 4 } }.encode();
        *p.last_mut().unwrap() = 3;
        assert_eq!(ClientMsg::decode(&p), Err(DecodeError::BadValue));
        p[10] = 99;
        assert_eq!(ClientMsg::decode(&p), Err(DecodeError::BadValue));
        let long = WindowInfo { class: "c".repeat(MAX_NAME + 1), ..info() };
        let mut w = Writer::new(tag::ANSWER);
        w.u64(1);
        w.u8(tag::REPLY_INFO);
        for v in [0u32; 6] { w.u32(v); }
        for v in [0i32; 4] { w.i32(v); }
        w.u32(0);
        w.bool(false);
        w.text(&long.class);
        w.text("");
        assert_eq!(ClientMsg::decode(&w.0), Err(DecodeError::TooLarge));
        // ... which the encoder cuts instead.
        let a = ClientMsg::Answer { id: 1, reply: WinReply::Info(Box::new(long)) };
        let Ok(ClientMsg::Answer { reply: WinReply::Info(back), .. }) = ClientMsg::decode(&a.encode()) else { panic!() };
        assert_eq!(back.class.len(), MAX_NAME);
        // A packet over the size limit.
        assert_eq!(ClientMsg::decode(&vec![0u8; MAX_MESSAGE + 1]), Err(DecodeError::TooLarge));
    }

    #[test]
    fn long_text_is_cut_at_a_character_boundary() {
        let s = "é".repeat(MAX_TEXT); // two bytes each
        let ClientMsg::Clipboard(back) = ClientMsg::decode(&ClientMsg::Clipboard(s).encode()).unwrap() else {
            panic!()
        };
        assert_eq!(back.len(), MAX_TEXT);
        assert!(back.chars().all(|c| c == 'é'));
    }

    #[test]
    fn the_largest_update_fits_in_a_packet() {
        let item = Item { frame: 1, pixmap: 1, x: 0, y: 0, parts: vec![Area::default(); MAX_PARTS] };
        let u = Update { active: 1, raised: vec![1; MAX_RAISED], lowered: vec![1; MAX_RAISED],
                         items: vec![item.clone(); MAX_ITEMS],
                         damage: vec![(1, Area::default()); MAX_DAMAGE], overlay: Some(item) };
        assert!(ClientMsg::Update(u).encode().len() <= MAX_MESSAGE);
        let p = ClientMsg::Pointer(Pointer::Image { width: MAX_POINTER_SIDE, height: MAX_POINTER_SIDE, hot_x: 0,
                                                     hot_y: 0, rgba: vec![0; (MAX_POINTER_SIDE * MAX_POINTER_SIDE * 4) as usize] });
        assert!(p.encode().len() <= MAX_MESSAGE);
        assert!(ClientMsg::Clipboard("x".repeat(MAX_TEXT)).encode().len() <= MAX_MESSAGE);
        let i = WindowInfo { class: "c".repeat(MAX_NAME), text: "t".repeat(MAX_TEXT), ..info() };
        assert!(ClientMsg::Answer { id: 1, reply: WinReply::Info(Box::new(i.clone())) }.encode().len() <= MAX_MESSAGE);
        assert!(ServerMsg::Answered { id: 1, reply: WinReply::Info(Box::new(i)) }.encode().len() <= MAX_MESSAGE);
    }
}
