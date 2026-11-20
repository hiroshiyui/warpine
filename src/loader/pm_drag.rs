// SPDX-License-Identifier: GPL-3.0-or-later
//
//! PMDRAG: direct manipulation (drag and drop) within a process (IBM *PM
//! Programming Reference*, "Direct Manipulation Functions" and the `DM_*`
//! messages; structures and constants from `<pmstddlg.h>`
//! `INCL_WINSTDDRAG`).
//!
//! - **Data:** `DRAGINFO` / `DRAGITEM` / `DRAGTRANSFER` blocks in guest
//!   memory, registered by address ([`DragTable`]); string handles
//!   ([`StrHandles`]); the type and rendering-mechanism-and-format (RMF)
//!   matching of the `DrgVerify*` / `DrgQuery*` calls.
//! - **The drag** (ADR-0045): `DrgDrag` is synchronous. Pressing button 2
//!   and moving the pointer posts `WM_BEGINDRAG` to the window under the
//!   press (the GUI thread, `sdl_renderer.rs`); from then on the GUI
//!   thread feeds pointer moves, button releases and Esc into
//!   [`DragFeed`] instead of routing them. `DrgDrag` waits on the feed
//!   and, for each move, sends `DM_DRAGLEAVE` to the window it left and
//!   `DM_DRAGOVER` to the window under the pointer through call chains
//!   (ADR-0026, continued with `ChainEnd::Next` as in ADR-0044); on the
//!   release it sends `DM_DROP` when the last reply was `DOR_DROP` and
//!   returns the target, else `DM_DRAGLEAVE` and NULLHANDLE.
//! - **Containers** answer as targets through their owner (`CN_DRAGOVER`,
//!   `CN_DRAGLEAVE`, `CN_DROP` with a `CNRDRAGINFO`) and start drags with
//!   `CN_INITDRAG` (`pm_container.rs`).
//!
//! - **Between programs** (ADR-0057): on the session's desktop the window
//!   under the pointer may be another program's. The server names the
//!   frame on top there (`WinCall::FrameAt`), that program the window in
//!   it (`DragTarget`), and the `DM_*` messages go to it as `Drag` calls,
//!   run by the window's thread like a sent message and waited for while
//!   the dragging thread runs the messages sent to it. The `DRAGINFO` and
//!   `DRAGTRANSFER` blocks are then in the session's shared arena
//!   (ADR-0049), given to the target's process, and string handles are
//!   the session's, kept by the desktop server.
//!
//! Messages for another thread's window of this process are made on the
//! dragging thread.
//!
//! - **Files** (ADR-0068): `DrgDragFiles` builds a `DRAGINFO` of
//!   `<DRM_OS2FILE,DRF_UNKNOWN>` items, one per file (container = its
//!   directory, source name = its name), and drags it as `DrgDrag` does.
//!   Host files dropped on the desktop (SDL drop events, or `drop` in a
//!   script) become the same kind of `DRAGINFO`, built on the thread of the
//!   window under the pointer as a [`SentKind::HostDrop`]: `DM_DRAGOVER`,
//!   then `DM_DROP` when it answers `DOR_DROP`, else `DM_DRAGLEAVE`. Only
//!   files inside the process's drives are named; others are left out.
//! - **The operation**: Ctrl, Shift and Ctrl+Shift held during a drag make
//!   `usOperation` `DO_COPY`, `DO_MOVE` and `DO_LINK` (the CUA keys;
//!   [`operation_for`]), and a change of them sends `DM_DRAGOVER` again.
//!
//! [`SentKind::HostDrop`]: super::pm_remote::SentKind::HostDrop

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use super::constants::*;
use super::pm_hook::{CallChain, CallStep, ChainEnd, ChainThen};
use super::pm_types::WindowManager;
use super::shared_arena::in_arena;
use super::{ApiResult, Loader, MutexExt};
use crate::desktop::client::{CallState, CallWaker};
use crate::desktop::proto::{WinCall, WinReply, MAX_STR_TEXT, SERVER_ADDRESS};

/// `DRAGINFO` field offsets and size (`<pmstddlg.h>`).
pub const DI_CB_DRAGINFO: u32 = 0;
pub const DI_CB_DRAGITEM: u32 = 4;
pub const DI_OPERATION: u32 = 6;
pub const DI_HWND_SOURCE: u32 = 8;
pub const DI_X_DROP: u32 = 12;
pub const DI_Y_DROP: u32 = 14;
pub const DI_CDITEM: u32 = 16;
pub const DRAGINFO_SIZE: u32 = 20;
/// `DRAGITEM` field offsets and size.
pub const DIT_HWND_ITEM: u32 = 0;
pub const DIT_ITEM_ID: u32 = 4;
pub const DIT_TYPE: u32 = 8;
pub const DIT_RMF: u32 = 12;
pub const DIT_CONTAINER: u32 = 16;
pub const DIT_SOURCE: u32 = 20;
pub const DIT_TARGET: u32 = 24;
pub const DIT_FS_CONTROL: u32 = 32;
pub const DIT_FS_SUPPORTED_OPS: u32 = 34;
pub const DRAGITEM_SIZE: u32 = 36;
/// `DRAGTRANSFER` size.
pub const DRAGTRANSFER_SIZE: u32 = 28;

/// Most items a `DRAGINFO` holds (Warpine's bound).
pub const MAX_DRAG_ITEMS: u32 = 4096;
/// Most string handles a process holds (Warpine's bound).
pub const MAX_STR_HANDLES: usize = 65536;
/// Longest string a handle holds, in bytes.
pub const MAX_STR_LEN: usize = 4096;
/// Most `DRAGTRANSFER`s one `DrgAllocDragtransfer` makes.
pub const MAX_DRAG_TRANSFERS: u32 = 4096;

pub const DO_DEFAULT: u16 = 0xBFFE;
pub const DO_COPY: u16 = 0x0010;
pub const DO_MOVE: u16 = 0x0020;
pub const DO_LINK: u16 = 0x0018;
pub const DO_COPYABLE: u16 = 0x0001;
pub const DO_MOVEABLE: u16 = 0x0002;
/// `DRAGITEM.fsControl`: the item is a container (a directory).
pub const DC_CONTAINER: u16 = 0x0008;
/// `DRT_UNKNOWN`, the type of a file of no known type.
pub const DRT_UNKNOWN: &[u8] = b"Unknown";
/// The rendering mechanism and format of a file item.
pub const RMF_OS2FILE: &[u8] = b"<DRM_OS2FILE,DRF_UNKNOWN>";
/// What a file item supports: copy and move (unverified; a host file can
/// be copied or moved by the program that takes it).
pub const FILE_OPS: u16 = DO_COPYABLE | DO_MOVEABLE;
/// Most host drops waiting for their window's thread (Warpine's bound).
pub const MAX_HOST_DROPS: usize = 8;

pub const DM_DROP: u32 = 0x032f;
pub const DM_DRAGOVER: u32 = 0x032e;
pub const DM_DRAGLEAVE: u32 = 0x032d;
pub const DM_RENDER: u32 = 0x0329;
pub const DM_RENDERCOMPLETE: u32 = 0x0328;
pub const WM_BEGINDRAG: u32 = 0x0420;
pub const WM_ENDDRAG: u32 = 0x0421;

pub const DOR_NODROP: u32 = 0x0000;
pub const DOR_DROP: u32 = 0x0001;
pub const DOR_NEVERDROP: u32 = 0x0003;

pub const DGS_DRAGINPROGRESS: u32 = 0x0001;
pub const VK_ENDDRAG: u32 = 0x38;

pub const PMERR_NOT_DRAGGING: u32 = 0x1f00;
pub const PMERR_ALREADY_DRAGGING: u32 = 0x1f01;
pub const PMERR_SOURCE_SAME_AS_TARGET: u32 = 0x1502;

/// `DRAGTRANSFER.fsReply` offset.
const DT_FS_REPLY: u32 = 26;
/// The button that ends a `VK_ENDDRAG` drag: the drag button, button 2
/// (unverified: `SV_BEGINDRAG`'s default is not documented).
pub const DRAG_BUTTON: u8 = 2;
/// Pels the pointer moves with the drag button held before `WM_BEGINDRAG`
/// (Warpine's choice).
pub const DRAG_THRESHOLD: i32 = 4;
/// Most `DRAGINFO`s a process holds at once (Warpine's bound).
pub const MAX_DRAG_INFOS: usize = 1024;
/// How long `DrgDrag` waits for input before checking whether the call
/// should stop.
const FEED_WAIT: std::time::Duration = std::time::Duration::from_millis(100);
/// The flags of drag blocks in the session's shared arena: giveable, as
/// `DrgAllocDraginfo`'s are in PM.
const DRAG_SHARED_FLAGS: u32 = PAG_READ | PAG_WRITE | PAG_COMMIT | OBJ_GIVEABLE;

/// String handles (`HSTR`): an application's strings kept by the system,
/// reference counted — adding a string held already returns its handle
/// again (unverified).
#[derive(Debug, Default)]
pub struct StrHandles {
    strings: HashMap<u32, (Vec<u8>, u32)>,
    by_text: HashMap<Vec<u8>, u32>,
    next: u32,
}

impl StrHandles {
    /// The handle of `text` (a new one, or a held one with its count up);
    /// `None` when the table is full or the text too long.
    pub fn add(&mut self, text: &[u8]) -> Option<u32> {
        if text.len() > MAX_STR_LEN { return None; }
        if let Some(&h) = self.by_text.get(text) {
            if let Some(e) = self.strings.get_mut(&h) { e.1 = e.1.saturating_add(1); }
            return Some(h);
        }
        if self.strings.len() >= MAX_STR_HANDLES { return None; }
        let h = loop {
            self.next = self.next.wrapping_add(1).max(1);
            if !self.strings.contains_key(&self.next) { break self.next; }
        };
        self.strings.insert(h, (text.to_vec(), 1));
        self.by_text.insert(text.to_vec(), h);
        Some(h)
    }

    pub fn get(&self, h: u32) -> Option<&[u8]> {
        self.strings.get(&h).map(|e| e.0.as_slice())
    }

    /// Drop one reference to `h`; FALSE when it is no handle.
    pub fn delete(&mut self, h: u32) -> bool {
        let Some(e) = self.strings.get_mut(&h) else { return false };
        e.1 -= 1;
        if e.1 == 0 {
            let (text, _) = self.strings.remove(&h).expect("looked up above");
            self.by_text.remove(&text);
        }
        true
    }

    pub fn len(&self) -> usize { self.strings.len() }
    pub fn is_empty(&self) -> bool { self.strings.is_empty() }
}

/// The types of an item's `hstrType`: comma-separated, the first one the
/// true type; spaces around them are ignored.
pub fn types(s: &str) -> Vec<&str> {
    s.split(',').map(str::trim).filter(|t| !t.is_empty()).collect()
}

/// The (mechanism, format) pairs of a rendering-mechanism-and-format
/// string, in order: `<mech,fmt>` pairs and `(m1,m2)x(f1,f2)` cross
/// products, separated by commas. The first pair is the native RMF.
/// Malformed parts are skipped; at most `MAX_PAIRS` pairs.
pub fn rmf_pairs(s: &str) -> Vec<(String, String)> {
    const MAX_PAIRS: usize = 1024;
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() && out.len() < MAX_PAIRS {
        if let Some(r) = rest.strip_prefix('<') {
            let Some(end) = r.find('>') else { break };
            if let Some((m, f)) = r[..end].split_once(',') {
                out.push((m.trim().to_string(), f.trim().to_string()));
            }
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix('(') {
            let Some(end) = r.find(')') else { break };
            let mechs: Vec<&str> = r[..end].split(',').map(str::trim).filter(|m| !m.is_empty()).collect();
            let after = r[end + 1..].trim_start();
            let Some(after) = after.strip_prefix(['x', 'X']).map(str::trim_start) else { break };
            let Some(after) = after.strip_prefix('(') else { break };
            let Some(fend) = after.find(')') else { break };
            let fmts: Vec<&str> = after[..fend].split(',').map(str::trim).filter(|f| !f.is_empty()).collect();
            for m in &mechs {
                for f in &fmts {
                    if out.len() < MAX_PAIRS { out.push((m.to_string(), f.to_string())); }
                }
            }
            rest = &after[fend + 1..];
        } else {
            // Skip a stray character (the commas between parts).
            let mut chars = rest.chars();
            chars.next();
            rest = chars.as_str();
        }
        rest = rest.trim_start().trim_start_matches(',').trim_start();
    }
    out
}

/// Whether `pairs` hold mechanism `mech` with format `fmt` (`None`: any).
pub fn rmf_has(pairs: &[(String, String)], mech: Option<&str>, fmt: Option<&str>) -> bool {
    pairs.iter().any(|(m, f)| mech.is_none_or(|x| x == m) && fmt.is_none_or(|x| x == f))
}

/// The native RMF of `pairs` as `<mech,fmt>`.
pub fn native_rmf(pairs: &[(String, String)]) -> Option<String> {
    pairs.first().map(|(m, f)| format!("<{m},{f}>"))
}

/// A `DRAGINFO` the process allocated: its item count and whether the
/// current drag uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragInfoBlock {
    pub items: u32,
}

/// A running drag (`DrgDrag`): the source window and `DRAGINFO`, the
/// button whose release drops, the window under the pointer with its last
/// `DM_DRAGOVER` reply (`DOR_*`), whether it said `DOR_NEVERDROP`, the
/// pointer shown over a target that accepts, and the dragging thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragSession {
    pub source: u32,
    pub pdinfo: u32,
    pub button: u8,
    pub target: u32,
    pub reply: u32,
    pub never: bool,
    pub pointer: u32,
    pub tid: u32,
    /// The OS/2 process id of the target's program when it is another
    /// program (0: this one).
    pub target_pid: u32,
    /// The last step is a `DM_DRAGOVER` whose reply is pending.
    awaiting: bool,
    /// A `DrgDragFiles` drag: its `DRAGINFO` and string handles go with it,
    /// and it returns whether the files were dropped.
    files: bool,
}

/// One message of the drag conversation: to a window of this program, in a
/// call chain, or to another program's window, a call waited for.
#[derive(Debug)]
enum DragAction {
    Local(CallStep),
    Remote { hwnd: u32, call: WinCall },
}

/// The messages of one step of the conversation (`DRAG_STAGE_*`) being
/// made: those still to make, the call to another program waited for, the
/// last reply, and the processes the `DRAGINFO` was given to.
#[derive(Debug, Default)]
struct DragRun {
    active: bool,
    stage: u8,
    actions: VecDeque<DragAction>,
    waiting: Option<u32>,
    last: u32,
    given: Vec<u32>,
}

/// The process's drag state: string handles, the `DRAGINFO`s and
/// `DRAGTRANSFER` arrays in guest memory (by address), and the running
/// drag.
#[derive(Debug, Default)]
pub struct DragTable {
    pub strings: StrHandles,
    pub infos: HashMap<u32, DragInfoBlock>,
    /// `DRAGTRANSFER` arrays: address → count.
    pub transfers: HashMap<u32, u32>,
    pub session: Option<DragSession>,
    run: DragRun,
    /// Host files dropped on a window, waiting for its thread
    /// ([`Loader::host_drop_draginfo`]): OS/2 paths, whether each is a
    /// directory.
    pub host_drops: HashMap<u32, Vec<(String, bool)>>,
    next_drop: u32,
}

/// `usOperation` for the modifier keys held: Ctrl copies, Shift moves,
/// both link, none is the default (the CUA keys of OS/2; unverified for
/// PM's own choice when both are held).
pub fn operation_for(ctrl: bool, shift: bool) -> u16 {
    match (ctrl, shift) {
        (true, true) => DO_LINK,
        (true, false) => DO_COPY,
        (false, true) => DO_MOVE,
        (false, false) => DO_DEFAULT,
    }
}

/// An OS/2 path split into its container (the directory, with its
/// trailing backslash) and name; `None` for a path without a name.
pub fn split_file_path(path: &[u8]) -> Option<(&[u8], &[u8])> {
    let cut = path.iter().rposition(|&b| b == b'\\' || b == b'/').map_or(0, |i| i + 1);
    let (dir, name) = path.split_at(cut);
    (!name.is_empty()).then_some((dir, name))
}

/// A file of a file `DRAGINFO`: its OS/2 path (in the thread's codepage),
/// type and target name (`None`: `DRT_UNKNOWN`, the file's name), and
/// whether it is a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileItem {
    pub path: Vec<u8>,
    pub kind: Option<Vec<u8>>,
    pub target: Option<Vec<u8>>,
    pub dir: bool,
}

/// Pointer input for a drag, fed by the GUI thread: after `WM_BEGINDRAG`
/// (`armed`, the drag button) and while `DrgDrag` runs (`active`), the
/// pointer position (desktop coordinates), whether it moved since
/// `DrgDrag` last looked, a button released, and Esc.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DragFeed {
    pub armed: Option<u8>,
    pub active: bool,
    pub pos: (i32, i32),
    pub moved: bool,
    pub released: Option<u8>,
    pub cancelled: bool,
    /// `usOperation` for the keys held at the last move or release (0: not
    /// known yet), so input that came before `DrgDrag` looked keeps the
    /// keys it came with.
    pub op: u16,
}

/// [`DragFeed`] with the condition `DrgDrag` waits on.
#[derive(Debug, Default)]
pub struct DragChannel {
    pub feed: std::sync::Mutex<DragFeed>,
    pub cond: std::sync::Condvar,
    /// `usOperation` for the modifier keys held now ([`operation_for`]),
    /// whether or not a drag runs.
    op: std::sync::atomic::AtomicU32,
}

impl DragChannel {
    /// The modifier keys changed: Ctrl and Shift held or not. A running
    /// drag asks the window under the pointer again with the new
    /// operation.
    pub fn modifiers(&self, ctrl: bool, shift: bool) {
        use std::sync::atomic::Ordering;
        let op = u32::from(operation_for(ctrl, shift));
        if self.op.swap(op, Ordering::Relaxed) == op { return; }
        let mut f = self.feed.lock_or_recover();
        if f.active && f.released.is_none() {
            f.op = op as u16;
            f.moved = true;
            self.cond.notify_all();
        }
    }

    /// `usOperation` for the keys held now.
    pub fn operation(&self) -> u16 {
        match self.op.load(std::sync::atomic::Ordering::Relaxed) {
            0 => DO_DEFAULT,
            op => op as u16,
        }
    }

    /// Whether pointer input belongs to a drag now (the GUI thread does
    /// not route it).
    pub fn active(&self) -> bool {
        self.feed.lock_or_recover().active
    }

    /// The pointer moved to desktop point `pos` (`armed` or `active`).
    pub fn moved(&self, pos: (i32, i32)) {
        let mut f = self.feed.lock_or_recover();
        if f.armed.is_none() && !f.active { return; }
        f.pos = pos;
        f.moved = true;
        f.op = self.operation();
        self.cond.notify_all();
    }

    /// Button `button` came up at `pos`; whether the drag takes it.
    pub fn released(&self, button: u8, pos: (i32, i32)) -> bool {
        let mut f = self.feed.lock_or_recover();
        if f.armed.is_none() && !f.active { return false; }
        if f.pos != pos { f.pos = pos; f.moved = true; }
        f.op = self.operation();
        // Kept while armed: `DrgDrag` may start after the release (the
        // drop then comes at once); the next press resets the channel.
        f.released = Some(button);
        self.cond.notify_all();
        f.active
    }

    /// The drag button went down again: input of an earlier press that
    /// started no drag is forgotten.
    pub fn reset_if_idle(&self) {
        let mut f = self.feed.lock_or_recover();
        if !f.active { *f = DragFeed::default(); }
    }

    /// Esc: the running drag ends without a drop.
    pub fn cancel(&self) -> bool {
        let mut f = self.feed.lock_or_recover();
        if !f.active { return false; }
        f.cancelled = true;
        self.cond.notify_all();
        true
    }

    /// `WM_BEGINDRAG` was posted at `pos` for a press of `button`.
    pub fn arm(&self, button: u8, pos: (i32, i32)) {
        let mut f = self.feed.lock_or_recover();
        if f.active { return; }
        *f = DragFeed { armed: Some(button), pos, moved: true, op: self.operation(), ..DragFeed::default() };
    }
}

/// What `DrgDrag` does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedStep {
    Move((i32, i32)),
    Release(u8),
    Cancel,
    /// The call should stop (thread killed, signal).
    Interrupted,
    /// Another program sent the dragging thread a message.
    Sent(super::pm_remote::SentMessage),
}

/// Which `DM_*` conversation step a [`ChainThen::Drag`] chain made.
pub const DRAG_STAGE_OVER: u8 = 0;
pub const DRAG_STAGE_DROP: u8 = 1;
pub const DRAG_STAGE_END: u8 = 2;

impl DragTable {
    /// Whether a running drag uses `DRAGINFO` `pdinfo`.
    pub fn in_drag(&self, pdinfo: u32) -> bool {
        self.session.is_some_and(|s| s.pdinfo == pdinfo)
    }

    /// The `DRAGINFO` that the item at `pditem` belongs to, with the
    /// item's index.
    pub fn info_of_item(&self, pditem: u32) -> Option<(u32, u32)> {
        self.infos.iter().find_map(|(&base, b)| {
            let first = base.checked_add(DRAGINFO_SIZE)?;
            let off = pditem.checked_sub(first)?;
            (off % DRAGITEM_SIZE == 0 && off / DRAGITEM_SIZE < b.items).then_some((base, off / DRAGITEM_SIZE))
        })
    }
}


impl WindowManager {
    /// The window under desktop point `(x, y)` that pointer input goes to,
    /// with the point in its coordinates: the deepest window there (a
    /// built-in control, an embedded dialog's control, an application's child
    /// window), or the client (dialog) itself; `None` over a frame's border
    /// and title bar or the desktop.
    pub fn window_under(&self, x: i32, y: i32) -> Option<(u32, i32, i32)> {
        let frame = self.z_hit_test(x, y)?;
        let target = self.frame_to_client.get(&frame).copied().unwrap_or(frame);
        let (lx, ly) = self.pointer_local(frame, x, y);
        // The deepest window there, as pointer input is routed (pm_input.rs).
        if let Some(hit) = self.window_under_pointer(frame, lx, ly, &[]) { return Some(hit); }
        let w = self.get_window(target)?;
        (lx >= 0 && ly >= 0 && lx < w.cx && ly < w.cy).then_some((target, lx, ly))
    }
}

impl Loader {
    fn drag_ok(&self, vcpu: u32, ok: bool, code: u32) -> ApiResult {
        if !ok { return self.pm_fail(vcpu, code); }
        ApiResult::Normal(1)
    }

    /// The bytes of the NUL-terminated string at `ptr` (at most
    /// [`MAX_STR_LEN`]); `None` for a NULL pointer.
    fn drag_bytes(&self, ptr: u32) -> Option<Vec<u8>> {
        if ptr == 0 { return None; }
        let mut out = Vec::new();
        for i in 0..MAX_STR_LEN as u32 {
            match self.guest_read::<u8>(ptr.checked_add(i)?)? {
                0 => break,
                b => out.push(b),
            }
        }
        Some(out)
    }

    fn drag_text(&self, ptr: u32) -> Option<String> {
        self.drag_bytes(ptr).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The string of handle `h` as text (empty for none).
    fn hstr_text(&self, h: u32) -> String {
        if let Some(link) = self.shared.desktop_link.get() {
            return match link.call(SERVER_ADDRESS, WinCall::StrName { handle: h }) {
                Some(WinReply::Text(t)) => t,
                _ => String::new(),
            };
        }
        let wm = self.shared.window_mgr.lock_or_recover();
        wm.drag.strings.get(h).map_or(String::new(), |b| String::from_utf8_lossy(b).into_owned())
    }

    /// The bytes of handle `h` in thread `tid`'s codepage (`None`: no
    /// handle). On the session's desktop the session keeps the strings as
    /// text (ADR-0057).
    fn hstr_bytes(&self, tid: u32, h: u32) -> Option<Vec<u8>> {
        if let Some(link) = self.shared.desktop_link.get() {
            return match link.call(SERVER_ADDRESS, WinCall::StrName { handle: h }) {
                Some(WinReply::Text(t)) => Some(super::codepage::cp_encode(&t, self.thread_codepage(tid))),
                _ => None,
            };
        }
        self.shared.window_mgr.lock_or_recover().drag.strings.get(h).map(<[u8]>::to_vec)
    }

    /// A handle for `bytes` (in thread `tid`'s codepage): the session's
    /// on its desktop, else this process's.
    fn hstr_add(&self, tid: u32, bytes: &[u8]) -> Option<u32> {
        if let Some(link) = self.shared.desktop_link.get() {
            let text = super::codepage::cp_decode(bytes, self.thread_codepage(tid));
            if text.len() > MAX_STR_TEXT { return None; }
            return match link.call(SERVER_ADDRESS, WinCall::StrAdd { text }) {
                Some(WinReply::Value(h)) => Some(h),
                _ => None,
            };
        }
        self.shared.window_mgr.lock_or_recover().drag.strings.add(bytes)
    }

    /// Drop a reference to handle `h`; FALSE when it is none.
    fn hstr_delete(&self, h: u32) -> bool {
        if let Some(link) = self.shared.desktop_link.get() {
            return link.call(SERVER_ADDRESS, WinCall::StrDelete { handle: h }) == Some(WinReply::Value(1));
        }
        self.shared.window_mgr.lock_or_recover().drag.strings.delete(h)
    }

    /// `size` bytes for a drag block: in the session's shared arena on the
    /// session's desktop (so that another program can be given it), else
    /// from the process's memory. Zeroed.
    fn drag_block_alloc(&self, size: u32) -> Option<u32> {
        if self.shared.desktop_link.get().is_some() {
            let me = self.shared.process_id();
            let block = self.shared.with_arena_table(|t, _, _| {
                let offset = t.alloc(size, DRAG_SHARED_FLAGS, None, me)?;
                Some((offset, t.at(offset).map_or(size, |b| b.size)))
            }).ok().flatten();
            if let Some((offset, rounded)) = block {
                if let Some(arena) = self.shared.arena.get() { arena.zero(offset, rounded); }
                return Some(super::constants::SHARED_ARENA_BASE + offset);
            }
        }
        let base = self.shared.mem_mgr.lock_or_recover().alloc(size)?;
        if let Some(m) = self.guest_slice_mut(base, size as usize) { m.fill(0); }
        Some(base)
    }

    /// Let a drag block go (an arena block: this process's hold on it).
    fn drag_block_free(&self, p: u32) -> bool {
        if in_arena(p) {
            return self.arena_free(p) == Some(NO_ERROR);
        }
        self.shared.mem_mgr.lock_or_recover().free(p);
        true
    }

    /// Give the arena block at `p`, which this process holds, to process
    /// `pid` of the session.
    fn give_drag_block(&self, p: u32, pid: u32) -> bool {
        if !in_arena(p) { return false; }
        let me = self.shared.process_id();
        if pid == me { return true; }
        let offset = p - super::constants::SHARED_ARENA_BASE;
        self.shared.with_arena_table(|t, live, _| {
            live.contains(&pid) && t.at(offset).is_some_and(|b| b.holders.contains(&me)) && t.hold(offset, pid).is_ok()
        }).unwrap_or(false)
    }

    /// Another program gave this one the `DRAGINFO` at `pdinfo` (a block
    /// of the session's arena this process holds): use it as one of its
    /// own, its items bounded by the block. FALSE when it is not one.
    pub(super) fn accept_draginfo(&self, pdinfo: u32) -> bool {
        if self.shared.window_mgr.lock_or_recover().drag.infos.contains_key(&pdinfo) { return true; }
        if !in_arena(pdinfo) { return false; }
        let me = self.shared.process_id();
        let offset = pdinfo - super::constants::SHARED_ARENA_BASE;
        let Some(size) = self.shared.with_arena_table(|t, _, _| {
            t.at(offset).filter(|b| b.holders.contains(&me)).map(|b| b.size)
        }).ok().flatten() else { return false };
        let n = u32::from(self.guest_read::<u16>(pdinfo + DI_CDITEM).unwrap_or(0));
        if n == 0 || n > MAX_DRAG_ITEMS || DRAGINFO_SIZE + n * DRAGITEM_SIZE > size { return false; }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if wm.drag.infos.len() >= MAX_DRAG_INFOS { return false; }
        wm.drag.infos.insert(pdinfo, DragInfoBlock { items: n });
        true
    }

    /// The `DRAGINFO` an item at `pditem` belongs to (`None`: not an item).
    fn item_ok(&self, pditem: u32) -> bool {
        self.shared.window_mgr.lock_or_recover().drag.info_of_item(pditem).is_some()
    }

    /// The handle in field `off` of the item at `pditem`.
    fn item_hstr(&self, pditem: u32, off: u32) -> u32 {
        self.guest_read::<u32>(pditem.wrapping_add(off)).unwrap_or(0)
    }

    /// `DrgAllocDraginfo(cditem)`: a zeroed `DRAGINFO` with room for
    /// `cditem` items, its sizes, `DO_DEFAULT` and the pointer position.
    pub(crate) fn drg_alloc_draginfo(&self, vcpu: u32, n: u32) -> ApiResult {
        match self.alloc_draginfo(n) {
            Ok(base) => ApiResult::Normal(base),
            Err(code) => self.pm_fail(vcpu, code),
        }
    }

    /// A new `DRAGINFO` for `n` items (see [`Self::drg_alloc_draginfo`]),
    /// or the `PMERR_*` code why not.
    fn alloc_draginfo(&self, n: u32) -> Result<u32, u32> {
        if n == 0 || n > MAX_DRAG_ITEMS || self.shared.window_mgr.lock_or_recover().drag.infos.len() >= MAX_DRAG_INFOS {
            return Err(PMERR_INVALID_PARAMETERS);
        }
        let size = DRAGINFO_SIZE + n * DRAGITEM_SIZE;
        let Some(base) = self.drag_block_alloc(size) else { return Err(PMERR_INSUFFICIENT_MEMORY) };
        let (x, y) = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            wm.drag.infos.insert(base, DragInfoBlock { items: n });
            wm.key_state.pointer
        };
        let _ = self.guest_write::<u32>(base + DI_CB_DRAGINFO, size);
        let _ = self.guest_write::<u16>(base + DI_CB_DRAGITEM, DRAGITEM_SIZE as u16);
        let _ = self.guest_write::<u16>(base + DI_OPERATION, DO_DEFAULT);
        let _ = self.guest_write::<i16>(base + DI_X_DROP, x as i16);
        let _ = self.guest_write::<i16>(base + DI_Y_DROP, y as i16);
        let _ = self.guest_write::<u16>(base + DI_CDITEM, n as u16);
        Ok(base)
    }

    /// A `DRAGINFO` of file items (module comment), its items from window
    /// `hwnd_item`, which is also the source; `None` when it cannot be made
    /// (what was made is let go again).
    pub(super) fn file_draginfo(&self, tid: u32, files: &[FileItem], hwnd_item: u32) -> Option<u32> {
        let n = u32::try_from(files.len()).ok()?;
        let pdinfo = self.alloc_draginfo(n).ok()?;
        let _ = self.guest_write::<u32>(pdinfo + DI_HWND_SOURCE, hwnd_item);
        // One handle per distinct string, as DrgDeleteDraginfoStrHandles
        // deletes each distinct handle once.
        let mut held: HashMap<Vec<u8>, u32> = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            let item = pdinfo + DRAGINFO_SIZE + i as u32 * DRAGITEM_SIZE;
            let Some((dir, name)) = split_file_path(&f.path) else {
                self.discard_draginfo(pdinfo);
                return None;
            };
            let texts: [(u32, &[u8]); 5] = [
                (DIT_TYPE, f.kind.as_deref().unwrap_or(DRT_UNKNOWN)),
                (DIT_RMF, RMF_OS2FILE),
                (DIT_CONTAINER, dir),
                (DIT_SOURCE, name),
                (DIT_TARGET, f.target.as_deref().unwrap_or(name)),
            ];
            for (off, text) in texts {
                let h = match held.get(text) {
                    Some(&h) => h,
                    None => match self.hstr_add(tid, text) {
                        Some(h) => { held.insert(text.to_vec(), h); h }
                        None => {
                            self.discard_draginfo(pdinfo);
                            return None;
                        }
                    },
                };
                let _ = self.guest_write::<u32>(item + off, h);
            }
            let _ = self.guest_write::<u32>(item + DIT_HWND_ITEM, hwnd_item);
            let _ = self.guest_write::<u32>(item + DIT_ITEM_ID, i as u32);
            let _ = self.guest_write::<u16>(item + DIT_FS_CONTROL, if f.dir { DC_CONTAINER } else { 0 });
            let _ = self.guest_write::<u16>(item + DIT_FS_SUPPORTED_OPS, FILE_OPS);
        }
        Some(pdinfo)
    }

    /// Let go of a `DRAGINFO` this process made for files and of its
    /// string handles (if it still has it: the target may have freed it).
    pub(super) fn discard_draginfo(&self, pdinfo: u32) {
        let Some(n) = self.info_items(pdinfo) else { return };
        let mut handles = Vec::new();
        for i in 0..n {
            let item = pdinfo + DRAGINFO_SIZE + i * DRAGITEM_SIZE;
            for off in [DIT_TYPE, DIT_RMF, DIT_CONTAINER, DIT_SOURCE, DIT_TARGET] {
                let h = self.item_hstr(item, off);
                if h != 0 && !handles.contains(&h) { handles.push(h); }
            }
        }
        for h in handles { self.hstr_delete(h); }
        if self.shared.window_mgr.lock_or_recover().drag.infos.remove(&pdinfo).is_some() {
            self.drag_block_free(pdinfo);
        }
    }

    /// `DrgQueryDraginfoPtr(pdinfo)`: the process's `DRAGINFO`s one after
    /// another — NULL gives the first, a `DRAGINFO` the next; NULL after the
    /// last (unverified: the reference's wording is short).
    pub(crate) fn drg_query_draginfo_ptr(&self, prev: u32) -> ApiResult {
        let wm = self.shared.window_mgr.lock_or_recover();
        let next = wm.drag.infos.keys().copied().filter(|&p| prev == 0 || p > prev).min();
        ApiResult::Normal(next.unwrap_or(0))
    }

    /// `DrgQueryDraginfoPtrFromHwnd(hwndSource)`: the `DRAGINFO` whose
    /// source is window `hwnd` (the lowest address when there are several;
    /// unverified), or NULL.
    pub(crate) fn drg_query_draginfo_from_hwnd(&self, hwnd: u32) -> ApiResult {
        if hwnd == 0 { return ApiResult::Normal(0); }
        let mut infos: Vec<u32> = self.shared.window_mgr.lock_or_recover().drag.infos.keys().copied().collect();
        infos.sort_unstable();
        let hit = infos.into_iter().find(|&p| self.guest_read::<u32>(p + DI_HWND_SOURCE) == Some(hwnd));
        ApiResult::Normal(hit.unwrap_or(0))
    }

    /// `DrgReallocDraginfo(pdinfo, cditem)`: a `DRAGINFO` for `cditem`
    /// items with the old one's header and as many of its items as fit; the
    /// old one is freed. Not while it is being dragged (unverified).
    pub(crate) fn drg_realloc_draginfo(&self, vcpu: u32, pdinfo: u32, n: u32) -> ApiResult {
        let (old_n, dragging) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            (wm.drag.infos.get(&pdinfo).map(|b| b.items), wm.drag.in_drag(pdinfo))
        };
        let Some(old_n) = old_n else { return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS) };
        if dragging { return self.pm_fail(vcpu, PMERR_ALREADY_DRAGGING); }
        let new = match self.alloc_draginfo(n) {
            Ok(p) => p,
            Err(code) => return self.pm_fail(vcpu, code),
        };
        for off in [DI_OPERATION, DI_X_DROP, DI_Y_DROP] {
            let v = self.guest_read::<u16>(pdinfo + off).unwrap_or(0);
            let _ = self.guest_write::<u16>(new + off, v);
        }
        let _ = self.guest_write::<u32>(new + DI_HWND_SOURCE, self.guest_read::<u32>(pdinfo + DI_HWND_SOURCE).unwrap_or(0));
        let keep = (old_n.min(n) * DRAGITEM_SIZE) as usize;
        if let Some(items) = self.guest_read_bytes(pdinfo + DRAGINFO_SIZE, keep) {
            let _ = self.guest_write_bytes(new + DRAGINFO_SIZE, &items);
        }
        if self.shared.window_mgr.lock_or_recover().drag.infos.remove(&pdinfo).is_some() {
            self.drag_block_free(pdinfo);
        }
        ApiResult::Normal(new)
    }

    /// `DrgDragFiles(hwnd, apszFiles, apszTypes, apszTargets, cFiles,
    /// hptrDrag, vkTerminate, fSourceRender, ulReserved)`: drag the files
    /// named (fully qualified) from window `hwnd` (module comment); TRUE when
    /// they were dropped. `fSourceRender` is not used: the target reads the
    /// files itself (unverified).
    pub(crate) fn drg_drag_files(&self, vcpu: u32, a: &[u32]) -> ApiResult {
        let (hwnd, files, kinds, targets, n, hptr, vk) = (a[0], a[1], a[2], a[3], a[4], a[5], a[6]);
        if n == 0 || n > MAX_DRAG_ITEMS || files == 0 || drag_button(vk).is_none() {
            return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS);
        }
        let string_at = |array: u32, i: u32| -> Option<Vec<u8>> {
            if array == 0 { return None; }
            self.drag_bytes(self.guest_read::<u32>(array.checked_add(i * 4)?)?)
        };
        let mut items = Vec::new();
        for i in 0..n {
            let Some(path) = string_at(files, i) else { return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS) };
            items.push(FileItem { path, kind: string_at(kinds, i), target: string_at(targets, i), dir: false });
        }
        let Some(pdinfo) = self.file_draginfo(vcpu, &items, hwnd) else {
            return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS);
        };
        let r = self.start_drag(vcpu, hwnd, pdinfo, vk, true, hptr);
        // Refused, or over at once: the DRAGINFO goes (again: no harm).
        if matches!(r, ApiResult::Normal(0)) { self.discard_draginfo(pdinfo); }
        r
    }

    /// `DrgFreeDraginfo(pdinfo)`. Within the process that is dragging it,
    /// before `DrgDrag` returns, it fails with `PMERR_SOURCE_SAME_AS_TARGET`
    /// (the source frees it afterwards). A `DRAGINFO` in the session's
    /// arena is gone once neither the source nor the target holds it.
    pub(crate) fn drg_free_draginfo(&self, vcpu: u32, pdinfo: u32) -> ApiResult {
        let freed = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if wm.drag.in_drag(pdinfo) { drop(wm); return self.pm_fail(vcpu, PMERR_SOURCE_SAME_AS_TARGET); }
            wm.drag.infos.remove(&pdinfo).is_some()
        };
        if freed { self.drag_block_free(pdinfo); }
        self.drag_ok(vcpu, freed, PMERR_INVALID_PARAMETERS)
    }

    /// `DrgAccessDraginfo(pdinfo)`: TRUE for a `DRAGINFO` of this process.
    pub(crate) fn drg_access_draginfo(&self, vcpu: u32, pdinfo: u32) -> ApiResult {
        let known = self.shared.window_mgr.lock_or_recover().drag.infos.contains_key(&pdinfo);
        self.drag_ok(vcpu, known, PMERR_INVALID_PARAMETERS)
    }

    /// The item count of `pdinfo` (`None`: not a `DRAGINFO`).
    fn info_items(&self, pdinfo: u32) -> Option<u32> {
        self.shared.window_mgr.lock_or_recover().drag.infos.get(&pdinfo).map(|b| b.items)
    }

    /// `DrgSetDragitem(pdinfo, pditem, cbBuffer, iItem)` (`set`) and
    /// `DrgQueryDragitem(pdinfo, cbBuffer, pditem, iItem)`: copy an item
    /// (`cbBuffer` bytes at most) in or out.
    pub(crate) fn drg_copy_item(&self, vcpu: u32, pdinfo: u32, buf: u32, cb: u32, index: u32, set: bool) -> ApiResult {
        let Some(n) = self.info_items(pdinfo) else { return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS) };
        if index >= n || cb == 0 || buf == 0 { return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS); }
        let len = cb.min(DRAGITEM_SIZE) as usize;
        let item = pdinfo + DRAGINFO_SIZE + index * DRAGITEM_SIZE;
        let (from, to) = if set { (buf, item) } else { (item, buf) };
        let ok = self.guest_read_bytes(from, len).and_then(|b| self.guest_write_bytes(to, &b)).is_some();
        self.drag_ok(vcpu, ok, PMERR_INVALID_PARAMETERS)
    }

    /// `DrgQueryDragitemPtr(pdinfo, i)`: the address of item `i`, or NULL.
    pub(crate) fn drg_query_dragitem_ptr(&self, vcpu: u32, pdinfo: u32, index: u32) -> ApiResult {
        match self.info_items(pdinfo) {
            Some(n) if index < n => ApiResult::Normal(pdinfo + DRAGINFO_SIZE + index * DRAGITEM_SIZE),
            _ => self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS),
        }
    }

    /// `DrgQueryDragitemCount(pdinfo)`.
    pub(crate) fn drg_query_dragitem_count(&self, vcpu: u32, pdinfo: u32) -> ApiResult {
        match self.info_items(pdinfo) {
            Some(_) => ApiResult::Normal(u32::from(self.guest_read::<u16>(pdinfo + DI_CDITEM).unwrap_or(0))),
            None => self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS),
        }
    }

    /// `DrgAddStrHandle(psz)`.
    pub(crate) fn drg_add_str_handle(&self, vcpu: u32, psz: u32) -> ApiResult {
        let Some(bytes) = self.drag_bytes(psz) else { return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS) };
        match self.hstr_add(vcpu, &bytes) {
            Some(h) => ApiResult::Normal(h),
            None => self.pm_fail(vcpu, PMERR_INSUFFICIENT_MEMORY),
        }
    }

    /// `DrgDeleteStrHandle(hstr)`.
    pub(crate) fn drg_delete_str_handle(&self, vcpu: u32, h: u32) -> ApiResult {
        let ok = self.hstr_delete(h);
        self.drag_ok(vcpu, ok, PMERR_INVALID_PARAMETERS)
    }

    /// `DrgDeleteDraginfoStrHandles(pdinfo)`: delete every distinct handle
    /// of its items (type, RMF, container, source and target names).
    pub(crate) fn drg_delete_draginfo_str_handles(&self, vcpu: u32, pdinfo: u32) -> ApiResult {
        let Some(n) = self.info_items(pdinfo) else { return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS) };
        let mut handles = Vec::new();
        for i in 0..n {
            let item = pdinfo + DRAGINFO_SIZE + i * DRAGITEM_SIZE;
            for off in [DIT_TYPE, DIT_RMF, DIT_CONTAINER, DIT_SOURCE, DIT_TARGET] {
                let h = self.item_hstr(item, off);
                if h != 0 && !handles.contains(&h) { handles.push(h); }
            }
        }
        for h in handles { self.hstr_delete(h); }
        ApiResult::Normal(1)
    }

    /// `DrgQueryStrName(hstr, cbBuffer, pBuffer)`: the bytes copied (a NUL
    /// after them); a bad handle gives an empty string.
    pub(crate) fn drg_query_str_name(&self, tid: u32, h: u32, cb: u32, buf: u32) -> ApiResult {
        if cb == 0 || buf == 0 { return ApiResult::Normal(0); }
        let bytes = self.hstr_bytes(tid, h).unwrap_or_default();
        ApiResult::Normal(self.copy_text_out(&bytes, buf, cb as usize))
    }

    /// `DrgQueryStrNameLen(hstr)`: without the NUL; 0 for a bad handle.
    pub(crate) fn drg_query_str_name_len(&self, tid: u32, h: u32) -> ApiResult {
        ApiResult::Normal(self.hstr_bytes(tid, h).map_or(0, |b| b.len() as u32))
    }

    /// The types of item `pditem` (`None`: not an item).
    fn item_types(&self, pditem: u32) -> Option<Vec<String>> {
        if !self.item_ok(pditem) { return None; }
        Some(types(&self.hstr_text(self.item_hstr(pditem, DIT_TYPE))).into_iter().map(str::to_string).collect())
    }

    fn item_rmf(&self, pditem: u32) -> Option<Vec<(String, String)>> {
        if !self.item_ok(pditem) { return None; }
        Some(rmf_pairs(&self.hstr_text(self.item_hstr(pditem, DIT_RMF))))
    }

    /// `DrgVerifyType(pditem, pszType)`: one of the types listed is the
    /// item's; `DrgVerifyTrueType`: the given type is its first.
    pub(crate) fn drg_verify_type(&self, pditem: u32, psz: u32, true_type: bool) -> ApiResult {
        let (Some(have), Some(want)) = (self.item_types(pditem), self.drag_text(psz)) else { return ApiResult::Normal(0) };
        let ok = if true_type { have.first().is_some_and(|t| *t == want.trim()) }
                 else { types(&want).iter().any(|w| have.iter().any(|h| h == w)) };
        ApiResult::Normal(ok as u32)
    }

    /// `DrgVerifyTypeSet(pditem, pszType, cbMatch, pszMatch)`: as
    /// `DrgVerifyType`, the first matching type copied out.
    pub(crate) fn drg_verify_type_set(&self, pditem: u32, psz: u32, cb: u32, out: u32) -> ApiResult {
        let (Some(have), Some(want)) = (self.item_types(pditem), self.drag_text(psz)) else { return ApiResult::Normal(0) };
        let Some(hit) = types(&want).into_iter().find(|w| have.iter().any(|h| h == w)) else { return ApiResult::Normal(0) };
        if cb != 0 && out != 0 { self.copy_text_out(hit.as_bytes(), out, cb as usize); }
        ApiResult::Normal(1)
    }

    /// `DrgQueryTrueType(pditem, cb, buf)` / `DrgQueryTrueTypeLen(pditem)`.
    pub(crate) fn drg_query_true_type(&self, pditem: u32, cb: u32, buf: u32, len_only: bool) -> ApiResult {
        let Some(t) = self.item_types(pditem).and_then(|t| t.into_iter().next()) else { return ApiResult::Normal(0) };
        if len_only { return ApiResult::Normal(t.len() as u32); }
        if cb == 0 || buf == 0 { return ApiResult::Normal(0); }
        self.copy_text_out(t.as_bytes(), buf, cb as usize);
        ApiResult::Normal(1)
    }

    /// `DrgVerifyRMF(pditem, pszMech, pszFmt)` (NULL: any).
    pub(crate) fn drg_verify_rmf(&self, pditem: u32, mech: u32, fmt: u32) -> ApiResult {
        let Some(pairs) = self.item_rmf(pditem) else { return ApiResult::Normal(0) };
        let (m, f) = (self.drag_text(mech), self.drag_text(fmt));
        ApiResult::Normal(rmf_has(&pairs, m.as_deref(), f.as_deref()) as u32)
    }

    /// `DrgVerifyNativeRMF(pditem, pszRMF)`: the native pair is one the
    /// given RMF string names.
    pub(crate) fn drg_verify_native_rmf(&self, pditem: u32, psz: u32) -> ApiResult {
        let (Some(pairs), Some(want)) = (self.item_rmf(pditem), self.drag_text(psz)) else { return ApiResult::Normal(0) };
        let Some(native) = pairs.first() else { return ApiResult::Normal(0) };
        ApiResult::Normal(rmf_pairs(&want).contains(native) as u32)
    }

    /// `DrgQueryNativeRMF(pditem, cb, buf)` / `DrgQueryNativeRMFLen(pditem)`.
    pub(crate) fn drg_query_native_rmf(&self, pditem: u32, cb: u32, buf: u32, len_only: bool) -> ApiResult {
        let Some(native) = self.item_rmf(pditem).and_then(|p| native_rmf(&p)) else { return ApiResult::Normal(0) };
        if len_only { return ApiResult::Normal(native.len() as u32); }
        if cb == 0 || buf == 0 { return ApiResult::Normal(0); }
        self.copy_text_out(native.as_bytes(), buf, cb as usize);
        ApiResult::Normal(1)
    }

    /// `DrgQueryDraginfoPtrFromDragitem(pditem)`.
    pub(crate) fn drg_info_from_item(&self, pditem: u32) -> ApiResult {
        ApiResult::Normal(self.shared.window_mgr.lock_or_recover().drag.info_of_item(pditem).map_or(0, |(b, _)| b))
    }

    /// `DrgAllocDragtransfer(cdxfer)`: `cdxfer` zeroed `DRAGTRANSFER`s, `cb`
    /// set.
    pub(crate) fn drg_alloc_dragtransfer(&self, vcpu: u32, n: u32) -> ApiResult {
        if n == 0 || n > MAX_DRAG_TRANSFERS { return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS); }
        let size = n * DRAGTRANSFER_SIZE;
        let Some(base) = self.drag_block_alloc(size) else {
            return self.pm_fail(vcpu, PMERR_INSUFFICIENT_MEMORY);
        };
        for i in 0..n { let _ = self.guest_write::<u32>(base + i * DRAGTRANSFER_SIZE, DRAGTRANSFER_SIZE); }
        self.shared.window_mgr.lock_or_recover().drag.transfers.insert(base, n);
        ApiResult::Normal(base)
    }

    /// `DrgFreeDragtransfer(pdxfer)`: the array it starts — this
    /// process's own, or one in the session's arena another program gave
    /// it (`DrgSendTransferMsg`), whose hold it lets go.
    pub(crate) fn drg_free_dragtransfer(&self, vcpu: u32, p: u32) -> ApiResult {
        let own = self.shared.window_mgr.lock_or_recover().drag.transfers.remove(&p).is_some();
        let ok = if own { self.drag_block_free(p) } else { in_arena(p) && self.arena_free(p) == Some(NO_ERROR) };
        self.drag_ok(vcpu, ok, PMERR_INVALID_PARAMETERS)
    }

    /// The start of this process's `DRAGTRANSFER` array that `pdxfer` lies
    /// in.
    fn transfer_block(&self, pdxfer: u32) -> Option<u32> {
        self.shared.window_mgr.lock_or_recover().drag.transfers.iter()
            .find(|&(&b, &n)| pdxfer >= b && pdxfer < b.saturating_add(n * DRAGTRANSFER_SIZE))
            .map(|(&b, _)| b)
    }

    /// `DrgSendTransferMsg` / `DrgPostTransferMsg` (`post`) to window
    /// `hwnd`, mp1 a `DRAGTRANSFER`: for another program's window the
    /// array is given to its process first, then the message is sent or
    /// posted through the desktop server. `None` for this program's
    /// windows, which take the ordinary path.
    pub(crate) fn drg_transfer_to_other(&self, tid: u32, a: &[u32], post: bool) -> Option<ApiResult> {
        let (hwnd, msg, mp1, mp2) = (a[0], a[1], a[2], a[3]);
        if !self.shared.window_mgr.lock_or_recover().is_foreign(hwnd) { return None; }
        if let Some(block) = self.transfer_block(mp1)
            && let Some(info) = self.describe_remote(tid, hwnd) {
            self.give_drag_block(block, info.pid);
        }
        let arg = |off: u64| match off { 4 => hwnd, 8 => msg, 12 => mp1, 16 => mp2, _ => 0 };
        let ordinal = if post { super::pm_remote::ORD_POST_MSG } else { super::pm_remote::ORD_SEND_MSG };
        self.remote_pmwin(tid, ordinal, &arg)
    }

    /// `DrgPushDraginfo(pdinfo, hwndDest)`: give the `DRAGINFO` to the
    /// process of window `hwnd` — another program's, on the session's
    /// desktop; this program's need nothing.
    pub(crate) fn drg_push_draginfo(&self, vcpu: u32, pdinfo: u32, hwnd: u32) -> ApiResult {
        let (known, foreign) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            (wm.drag.infos.contains_key(&pdinfo), wm.is_foreign(hwnd))
        };
        if !known { return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS); }
        if !foreign { return self.drag_ok(vcpu, self.pm_check_hwnd(vcpu, hwnd, false), PMERR_INVALID_HWND); }
        let given = self.describe_remote(vcpu, hwnd).is_some_and(|i| self.give_drag_block(pdinfo, i.pid));
        self.drag_ok(vcpu, given, PMERR_INVALID_PARAMETERS)
    }

    /// `DrgSendTransferMsg` / `DrgPostTransferMsg`: `fsReply` of the
    /// `DRAGTRANSFER` (mp1) set to 0 for the rendering messages, then an
    /// ordinary send or post (one process: nothing to give).
    pub(crate) fn drg_transfer_reset(&self, msg: u32, pdxfer: u32) {
        if matches!(msg, DM_RENDER | DM_RENDERCOMPLETE) || pdxfer != 0 {
            let known = self.shared.window_mgr.lock_or_recover().drag.transfers.keys()
                .any(|&b| pdxfer >= b && pdxfer < b.saturating_add(DRAGTRANSFER_SIZE * MAX_DRAG_TRANSFERS));
            if known { let _ = self.guest_write::<u16>(pdxfer.wrapping_add(DT_FS_REPLY), 0); }
        }
    }

    /// `DrgPostTransferMsg`'s post: `msg` to `hwnd` with mp1 = the
    /// `DRAGTRANSFER`, mp2 = the flags.
    pub(crate) fn drg_post(&self, vcpu: u32, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> ApiResult {
        if !self.pm_check_hwnd(vcpu, hwnd, false) { return ApiResult::Normal(0); }
        let wm = self.shared.window_mgr.lock_or_recover();
        let Some(mq) = wm.find_hmq_for_hwnd(hwnd).and_then(|h| wm.get_mq(h)) else { return ApiResult::Normal(0) };
        let mut q = mq.lock_or_recover();
        q.messages.push_back(super::pm_types::OS2Message { hwnd, msg, mp1, mp2, time: 0, x: 0, y: 0 });
        q.cond.notify_one();
        ApiResult::Normal(1)
    }

    /// `DrgGetPS(hwnd)`: a presentation space for drawing target emphasis,
    /// as `WinGetPS` gives (drawn into the window's frame).
    pub(crate) fn drg_get_ps(&self, vcpu: u32, hwnd: u32) -> ApiResult {
        if !self.pm_check_hwnd(vcpu, hwnd, false) { return ApiResult::Normal(0); }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let hps = wm.create_ps(hwnd);
        if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.origin = super::pm_types::PsOrigin::Cached; }
        ApiResult::Normal(hps)
    }

    /// `DrgReleasePS(hps)`.
    pub(crate) fn drg_release_ps(&self, hps: u32) -> ApiResult {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let ok = wm.ps_map.get(&hps).is_some_and(|ps| ps.origin == super::pm_types::PsOrigin::Cached);
        if ok { wm.remove_ps(hps); }
        ApiResult::Normal(ok as u32)
    }

    /// `DrgQueryDragStatus()`.
    pub(crate) fn drg_query_drag_status(&self) -> ApiResult {
        ApiResult::Normal(if self.shared.window_mgr.lock_or_recover().drag.session.is_some() { DGS_DRAGINPROGRESS } else { 0 })
    }

    /// `DrgSetDragPointer(pdinfo, hptr)`: the pointer shown over a target
    /// that accepts the drop.
    pub(crate) fn drg_set_drag_pointer(&self, vcpu: u32, pdinfo: u32, hptr: u32) -> ApiResult {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        match wm.drag.session.as_mut().filter(|s| s.pdinfo == pdinfo) {
            Some(s) => { s.pointer = hptr; ApiResult::Normal(1) }
            None => { drop(wm); self.pm_fail(vcpu, PMERR_NOT_DRAGGING) }
        }
    }

    /// Show the drag's pointer: the illegal sign unless the window under
    /// it accepts the drop.
    fn drag_feedback(&self, accept: bool, pointer: u32) {
        let shape = {
            let mgr = self.shared.pointers.lock_or_recover();
            if accept {
                let h = if pointer != 0 { pointer } else { mgr.current() };
                mgr.shape(h).unwrap_or(super::pm_pointer::system_shape(SPTR_ARROW))
            } else {
                super::pm_pointer::system_shape(SPTR_ILLEGAL)
            }
        };
        let wm = self.shared.window_mgr.lock_or_recover();
        if let Some(tx) = &wm.gui_tx { let _ = tx.send(crate::gui::GUIMessage::SetPointer(Some(shape))); }
    }

    /// `DrgDrag(hwndSource, pdinfo, pdimg, cdimg, vkTerminate, pRsvd)`:
    /// start the drag and go on with it until the drop (see the module
    /// comment). Returns the target window, or NULLHANDLE.
    pub(crate) fn drg_drag(&self, vcpu: u32, source: u32, pdinfo: u32, vk_terminate: u32) -> ApiResult {
        self.start_drag(vcpu, source, pdinfo, vk_terminate, false, 0)
    }

    /// Start a drag (`DrgDrag`; `files`: `DrgDragFiles`'s, shown with
    /// pointer `pointer` over a target that accepts).
    fn start_drag(&self, vcpu: u32, source: u32, pdinfo: u32, vk_terminate: u32, files: bool, pointer: u32) -> ApiResult {
        let Some(button) = drag_button(vk_terminate) else {
            return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS);
        };
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if !wm.drag.infos.contains_key(&pdinfo) || !wm.is_window(source) {
                drop(wm);
                return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS);
            }
            if wm.drag.session.is_some() { drop(wm); return self.pm_fail(vcpu, PMERR_ALREADY_DRAGGING); }
            // Chains are keyed by vCPU, as WinSendMsg's are.
            wm.drag.session = Some(DragSession { source, pdinfo, button, target: 0, reply: DOR_NODROP, never: false,
                                                 pointer, tid: vcpu, target_pid: 0, awaiting: false, files });
            wm.drag.run = DragRun::default();
            // The drag loop takes the pointer from here on.
            wm.drop_pointer_input();
        }
        if self.guest_read::<u32>(pdinfo + DI_HWND_SOURCE) == Some(0) {
            let _ = self.guest_write::<u32>(pdinfo + DI_HWND_SOURCE, source);
        }
        let at = self.shared.window_mgr.lock_or_recover().key_state.pointer;
        {
            let mut f = self.shared.drag_channel.feed.lock_or_recover();
            // Input since WM_BEGINDRAG stays (a release may have come
            // already); without one the drag starts where the pointer is.
            if f.armed.is_none() {
                *f = DragFeed { pos: at, moved: true, ..DragFeed::default() };
            }
            f.armed = None;
            f.active = true;
        }
        self.drag_continue()
    }

    /// Wait for the next drag input, or a message another program sent
    /// the dragging thread.
    fn drag_wait(&self, tid: u32, button: u8) -> FeedStep {
        let hmq = self.shared.window_mgr.lock_or_recover().thread_queue(tid);
        let ch = &self.shared.drag_channel;
        let mut f = ch.feed.lock_or_recover();
        loop {
            if f.cancelled { f.cancelled = false; return FeedStep::Cancel; }
            if f.moved { f.moved = false; return FeedStep::Move(f.pos); }
            if let Some(b) = f.released.take() {
                if b == button { return FeedStep::Release(b); }
                continue;
            }
            if self.interrupted() { return FeedStep::Interrupted; }
            if let Some(s) = hmq.and_then(|h| self.shared.window_mgr.lock_or_recover().take_sent(h)) {
                return FeedStep::Sent(s);
            }
            f = ch.cond.wait_timeout(f, FEED_WAIT).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// How the window under the pointer takes part: its procedure is
    /// called (a guest window), its owner is asked (a container: `CN_*`), or
    /// it never accepts (other built-in controls, a frame's chrome).
    pub(super) fn drag_step(&self, target: u32, msg: u32, pdinfo: u32, pos: (i32, i32), local: (i32, i32))
        -> Option<CallStep> {
        let (pfn, class, owner_proc, owner, id) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let w = wm.get_window(target)?;
            let owner = wm.notify_target(target);
            (w.pfn_wp, w.class_name.clone(), wm.get_window(owner).map_or(0, |o| o.pfn_wp), owner, w.id)
        };
        let mp2 = if msg == DM_DRAGOVER { (pos.0 as u16 as u32) | ((pos.1 as u16 as u32) << 16) } else { 0 };
        if pfn != 0 {
            return Some(CallStep::send(pfn, target, msg, pdinfo, mp2));
        }
        if class != super::pm_container::CLASS_CONTAINER || owner_proc == 0 || owner == target { return None; }
        let code = match msg {
            DM_DRAGOVER => super::pm_container::CN_DRAGOVER,
            DM_DRAGLEAVE => super::pm_container::CN_DRAGLEAVE,
            _ => super::pm_container::CN_DROP,
        };
        let rec = if msg == DM_DRAGLEAVE { 0 } else { self.container_record_at(target, local) };
        Some(CallStep::Guest {
            proc_addr: owner_proc, args: vec![owner, WM_CONTROL, (code << 16) | (id & 0xFFFF), 0],
            block: vec![pdinfo, rec], block_arg: 3, stop_on_true: false, window: owner,
        })
    }

    /// The window under desktop point `pos` that takes the drag, with the
    /// point in its coordinates and, for another program's window, that
    /// program's process id: on the session's desktop the server names the
    /// frame on top (any program's) and the frame's program the window in
    /// it; else this program's windows. `(0, …)` for none.
    fn drag_target_at(&self, pos: (i32, i32)) -> (u32, (i32, i32), u32) {
        if let Some(link) = self.shared.desktop_link.get()
            && let Some(WinReply::Value(frame)) = link.call(SERVER_ADDRESS, WinCall::FrameAt { x: pos.0, y: pos.1 }) {
            if frame == 0 { return (0, (0, 0), 0); }
            if self.shared.window_mgr.lock_or_recover().is_foreign(frame) {
                let found = link.call(frame, WinCall::DragTarget { x: pos.0, y: pos.1 });
                return match found {
                    Some(WinReply::Windows { pid, hwnds }) if hwnds.len() == 1
                        && super::pm_types::handle_slot(hwnds[0]) == super::pm_types::handle_slot(frame) =>
                        (hwnds[0], (0, 0), pid),
                    _ => (0, (0, 0), 0),
                };
            }
        }
        let under = self.shared.window_mgr.lock_or_recover().window_under(pos.0, pos.1);
        under.map_or((0, (0, 0), 0), |(h, x, y)| (h, (x, y), 0))
    }

    /// Message `msg` of the conversation for the window `target` (of
    /// process `pid`, 0: this one): a call on another program's window,
    /// which is given the `DRAGINFO` first, or a call in a chain; `None`
    /// when the window never accepts.
    fn drag_action(&self, target: u32, pid: u32, msg: u32, pdinfo: u32, pos: (i32, i32), local: (i32, i32))
        -> Option<DragAction> {
        if pid == 0 {
            return self.drag_step(target, msg, pdinfo, pos, local).map(DragAction::Local);
        }
        if msg != DM_DRAGLEAVE {
            let given = self.shared.window_mgr.lock_or_recover().drag.run.given.contains(&pid);
            if !given {
                // A DRAGINFO outside the arena cannot reach another program.
                if !self.give_drag_block(pdinfo, pid) { return None; }
                self.shared.window_mgr.lock_or_recover().drag.run.given.push(pid);
            }
        }
        Some(DragAction::Remote { hwnd: target, call: WinCall::Drag { msg, pdinfo, x: pos.0, y: pos.1 } })
    }

    /// End the drag: no session, input routed again, the pointer back.
    fn drag_end(&self, result: u32) -> ApiResult {
        let session = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            wm.drag.run = DragRun::default();
            wm.drag.session.take()
        };
        *self.shared.drag_channel.feed.lock_or_recover() = DragFeed::default();
        self.drag_feedback(true, 0);
        // DrgDragFiles lets its DRAGINFO go and says whether it dropped.
        if let Some(s) = session.filter(|s| s.files) {
            self.discard_draginfo(s.pdinfo);
            return ApiResult::Normal((result != 0) as u32);
        }
        ApiResult::Normal(result)
    }

    /// Go on with the drag: wait for input and send what it asks for.
    fn drag_continue(&self) -> ApiResult {
        loop {
            let Some(s) = self.shared.window_mgr.lock_or_recover().drag.session else { return ApiResult::Normal(0) };
            match self.drag_wait(s.tid, s.button) {
                FeedStep::Move(pos) => {
                    let (hwnd, local, pid) = self.drag_target_at(pos);
                    let mut actions = Vec::new();
                    let mut sess = s;
                    if hwnd != s.target {
                        if s.target != 0
                            && let Some(a) = self.drag_action(s.target, s.target_pid, DM_DRAGLEAVE, s.pdinfo, pos, (0, 0)) {
                            actions.push(a);
                        }
                        sess.target = hwnd;
                        sess.target_pid = pid;
                        sess.never = false;
                        sess.reply = DOR_NODROP;
                    }
                    sess.awaiting = false;
                    if hwnd != 0 && !sess.never {
                        let _ = self.guest_write::<i16>(s.pdinfo + DI_X_DROP, pos.0 as i16);
                        let _ = self.guest_write::<i16>(s.pdinfo + DI_Y_DROP, pos.1 as i16);
                        let op = match self.shared.drag_channel.feed.lock_or_recover().op {
                            0 => self.shared.drag_channel.operation(),
                            op => op,
                        };
                        let _ = self.guest_write::<u16>(s.pdinfo + DI_OPERATION, op);
                        match self.drag_action(hwnd, pid, DM_DRAGOVER, s.pdinfo, pos, local) {
                            Some(a) => { actions.push(a); sess.awaiting = true; }
                            None => { sess.reply = DOR_NEVERDROP; sess.never = true; }
                        }
                    }
                    self.shared.window_mgr.lock_or_recover().drag.session = Some(sess);
                    if actions.is_empty() {
                        self.drag_feedback(sess.reply == DOR_DROP, sess.pointer);
                        continue;
                    }
                    return self.drag_run(actions, DRAG_STAGE_OVER);
                }
                FeedStep::Release(_) => {
                    let (pos, op) = {
                        let f = self.shared.drag_channel.feed.lock_or_recover();
                        (f.pos, f.op)
                    };
                    if s.target != 0 && s.reply == DOR_DROP {
                        let _ = self.guest_write::<i16>(s.pdinfo + DI_X_DROP, pos.0 as i16);
                        let _ = self.guest_write::<i16>(s.pdinfo + DI_Y_DROP, pos.1 as i16);
                        // The keys held when the button came up.
                        if op != 0 { let _ = self.guest_write::<u16>(s.pdinfo + DI_OPERATION, op); }
                        let local = if s.target_pid != 0 { (0, 0) } else {
                            self.shared.window_mgr.lock_or_recover().window_under(pos.0, pos.1)
                                .filter(|w| w.0 == s.target).map_or((0, 0), |w| (w.1, w.2))
                        };
                        if let Some(a) = self.drag_action(s.target, s.target_pid, DM_DROP, s.pdinfo, pos, local) {
                            return self.drag_run(vec![a], DRAG_STAGE_DROP);
                        }
                    }
                    return self.drag_leave_and_end(s);
                }
                FeedStep::Cancel => return self.drag_leave_and_end(s),
                FeedStep::Interrupted => return self.drag_end(0),
                FeedStep::Sent(m) => return self.run_sent(s.tid, m, super::pm_remote::AfterSent::Drag),
            }
        }
    }

    /// No drop: `DM_DRAGLEAVE` to the window under the pointer, then the
    /// end with NULLHANDLE.
    fn drag_leave_and_end(&self, s: DragSession) -> ApiResult {
        if s.target != 0 && let Some(a) = self.drag_action(s.target, s.target_pid, DM_DRAGLEAVE, s.pdinfo, (0, 0), (0, 0)) {
            return self.drag_run(vec![a], DRAG_STAGE_END);
        }
        self.drag_end(0)
    }

    /// Make the messages of conversation step `stage`.
    fn drag_run(&self, actions: Vec<DragAction>, stage: u8) -> ApiResult {
        {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let run = &mut wm.drag.run;
            (run.active, run.stage, run.actions, run.waiting, run.last) = (true, stage, actions.into(), None, 0);
        }
        self.drag_advance()
    }

    /// Go on with the messages of the current step: a run of messages to
    /// this program's windows is a call chain ([`ChainThen::Drag`]); a
    /// message to another program's window is a call waited for without a
    /// time limit, running the messages sent to the dragging thread
    /// meanwhile. Once all are made, the step ends
    /// ([`Self::drag_step_done`]).
    fn drag_advance(&self) -> ApiResult {
        let Some(tid) = self.shared.window_mgr.lock_or_recover().drag.session.map(|s| s.tid) else {
            return ApiResult::Normal(0);
        };
        let link = self.shared.desktop_link.get();
        let wait = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.thread_queue(tid).and_then(|h| wm.get_mq(h)).map(|mq| {
                let q = mq.lock_or_recover();
                (Arc::clone(&q.cond), Arc::clone(&q.lock))
            })
        };
        let hmq = self.shared.window_mgr.lock_or_recover().thread_queue(tid);
        loop {
            let (waiting, next) = {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let run = &mut wm.drag.run;
                (run.waiting, if run.waiting.is_none() { run.actions.pop_front() } else { None })
            };
            if let Some(id) = waiting {
                let state = link.map_or(CallState::Gone, |l| l.poll_call(id));
                let reply = match state {
                    CallState::Answered(WinReply::Value(v)) => Some(v),
                    // The target's program failed or ended: it takes no drop.
                    CallState::Answered(_) | CallState::Gone => Some(DOR_NEVERDROP),
                    CallState::Waiting => None,
                };
                if let Some(v) = reply {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    (wm.drag.run.waiting, wm.drag.run.last) = (None, v);
                    continue;
                }
                if let Some(s) = hmq.and_then(|h| self.shared.window_mgr.lock_or_recover().take_sent(h)) {
                    return self.run_sent(tid, s, super::pm_remote::AfterSent::Drag);
                }
                if self.interrupted() {
                    if let Some(l) = link { l.abandon_call(id); }
                    return self.drag_end(0);
                }
                if let Some((cond, lock)) = &wait {
                    let g = lock.lock_or_recover();
                    let _ = cond.wait_timeout(g, FEED_WAIT);
                }
                continue;
            }
            match next {
                None => {
                    let (stage, last) = {
                        let mut wm = self.shared.window_mgr.lock_or_recover();
                        wm.drag.run.active = false;
                        (wm.drag.run.stage, wm.drag.run.last)
                    };
                    return self.drag_step_done(stage, last);
                }
                Some(DragAction::Local(step)) => {
                    let mut steps = vec![step];
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    while matches!(wm.drag.run.actions.front(), Some(DragAction::Local(_))) {
                        if let Some(DragAction::Local(step)) = wm.drag.run.actions.pop_front() { steps.push(step); }
                    }
                    drop(wm);
                    return self.start_chain(CallChain::new(tid, steps, false, ChainThen::Drag));
                }
                Some(DragAction::Remote { hwnd, call }) => {
                    // An answer wakes the thread where it waits: its queue.
                    let wake = wait.clone().map(|(cond, lock)| -> CallWaker {
                        Arc::new(move || {
                            let _g = lock.lock_or_recover();
                            cond.notify_all();
                        })
                    });
                    let id = link.and_then(|l| l.begin_call(hwnd, call, wake));
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    match id {
                        Some(id) => wm.drag.run.waiting = Some(id),
                        None => wm.drag.run.last = DOR_NEVERDROP,
                    }
                }
            }
        }
    }

    /// The messages of step `stage` are made, `last` the last reply: after
    /// `DM_DRAGOVER` record it and wait for more input; after the drop
    /// return the target; else end without one.
    fn drag_step_done(&self, stage: u8, last: u32) -> ApiResult {
        match stage {
            DRAG_STAGE_OVER => {
                let s = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let Some(s) = wm.drag.session.as_mut() else { return ApiResult::Normal(0) };
                    if s.awaiting {
                        s.awaiting = false;
                        s.reply = last & 0xFFFF;
                        s.never = s.reply == DOR_NEVERDROP;
                    }
                    *s
                };
                self.drag_feedback(s.reply == DOR_DROP, s.pointer);
                self.drag_continue()
            }
            DRAG_STAGE_DROP => {
                let target = self.shared.window_mgr.lock_or_recover().drag.session.map_or(0, |s| s.target);
                self.drag_end(target)
            }
            _ => self.drag_end(0),
        }
    }

    /// A drag chain returned `last`.
    pub(crate) fn drag_chain_done(&self, last: u32) -> ChainEnd {
        self.shared.window_mgr.lock_or_recover().drag.run.last = last;
        chain_end(self.drag_advance())
    }

    /// The dragging thread ran a message another program sent it: go on
    /// where it was — waiting for a reply, or for input.
    pub(crate) fn drag_after_sent(&self) -> ChainEnd {
        let active = self.shared.window_mgr.lock_or_recover().drag.run.active;
        chain_end(if active { self.drag_advance() } else { self.drag_continue() })
    }
}

/// Which conversation step a [`super::pm_remote::SentKind::HostDrop`] is.
pub const HOST_DROP_OVER: u8 = 0;
pub const HOST_DROP_DROP: u8 = 1;
pub const HOST_DROP_LEAVE: u8 = 2;

impl WindowManager {
    /// Host files (OS/2 paths, whether each is a directory) dropped at
    /// desktop point `(x, y)` (PM coordinates): queued for the thread of
    /// the window under it as a [`super::pm_remote::SentKind::HostDrop`].
    /// FALSE when no window there takes input or too many drops wait.
    pub fn queue_host_drop(&mut self, x: i32, y: i32, files: Vec<(String, bool)>) -> bool {
        use super::pm_remote::{SentKind, SentMessage};
        if files.is_empty() || self.drag.host_drops.len() >= MAX_HOST_DROPS { return false; }
        let Some((hwnd, _, _)) = self.window_under(x, y) else { return false };
        if self.frame_disabled(self.find_frame_for_hwnd(hwnd)) { return false; }
        let Some(hmq) = self.find_hmq_for_hwnd(hwnd) else { return false };
        let drop = loop {
            self.drag.next_drop = self.drag.next_drop.wrapping_add(1).max(1);
            if !self.drag.host_drops.contains_key(&self.drag.next_drop) { break self.drag.next_drop; }
        };
        let s = SentMessage { id: None, hwnd, kind: SentKind::HostDrop { drop, x, y, stage: HOST_DROP_OVER, pdinfo: 0 } };
        if !self.queue_sent(hmq, s) { return false; }
        self.drag.host_drops.insert(drop, files);
        true
    }
}

impl Loader {
    /// The `DRAGINFO` of host drop `drop` for window `hwnd` at desktop
    /// point `pos`, made on the window's thread `tid`; 0 when it is gone or
    /// cannot be made.
    pub(super) fn host_drop_draginfo(&self, tid: u32, drop: u32, pos: (i32, i32)) -> u32 {
        let Some(files) = self.shared.window_mgr.lock_or_recover().drag.host_drops.remove(&drop) else { return 0 };
        let cp = self.thread_codepage(tid);
        let items: Vec<FileItem> = files.into_iter().map(|(path, dir)| FileItem {
            path: super::codepage::cp_encode(&path, cp), kind: None, target: None, dir,
        }).collect();
        // The system is the source: no window of the program's.
        let Some(pdinfo) = self.file_draginfo(tid, &items, 0) else { return 0 };
        let _ = self.guest_write::<i16>(pdinfo + DI_X_DROP, pos.0 as i16);
        let _ = self.guest_write::<i16>(pdinfo + DI_Y_DROP, pos.1 as i16);
        let _ = self.guest_write::<u16>(pdinfo + DI_OPERATION, DO_DEFAULT);
        pdinfo
    }

    /// Step `stage` of a host drop on window `hwnd` was made (`took_part`:
    /// the window was asked), `reply` its answer: after `DM_DRAGOVER` the
    /// drop or the leave follows (queued for the same thread); after the
    /// leave, or when nobody was asked, the `DRAGINFO` goes. After the drop
    /// it is the target's, which frees it.
    pub(super) fn host_drop_step_done(&self, hwnd: u32, stage: u8, pdinfo: u32, pos: (i32, i32), reply: u32, took_part: bool) {
        use super::pm_remote::{SentKind, SentMessage};
        if pdinfo == 0 { return; }
        let next = match stage {
            HOST_DROP_OVER if took_part && reply & 0xFFFF == DOR_DROP => Some(HOST_DROP_DROP),
            HOST_DROP_OVER if took_part => Some(HOST_DROP_LEAVE),
            HOST_DROP_DROP if took_part => return,
            _ => None,
        };
        if let Some(stage) = next {
            let wm = self.shared.window_mgr.lock_or_recover();
            let s = SentMessage { id: None, hwnd, kind: SentKind::HostDrop { drop: 0, x: pos.0, y: pos.1, stage, pdinfo } };
            if wm.find_hmq_for_hwnd(hwnd).is_some_and(|hmq| wm.queue_sent(hmq, s)) { return; }
        }
        self.discard_draginfo(pdinfo);
    }
}

/// The button whose release ends a drag with `vkTerminate` `vk`.
fn drag_button(vk: u32) -> Option<u8> {
    match vk {
        VK_BUTTON1 => Some(1),
        VK_BUTTON2 => Some(2),
        VK_BUTTON3 => Some(3),
        VK_ENDDRAG => Some(DRAG_BUTTON),
        _ => None,
    }
}

/// How a chain ends that goes on with the drag's result `r`.
fn chain_end(r: ApiResult) -> ChainEnd {
    match r {
        ApiResult::CallChain(c) => ChainEnd::Next(c),
        ApiResult::Normal(v) => ChainEnd::Return(v),
        other => { log::warn!("drag: unexpected {other:?}"); ChainEnd::Return(0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::loader::pm_hook::ChainEnd;

    const BUF: u32 = 0x3000;
    const PROC: u32 = 0x0060_0000;

    fn ok(r: ApiResult) -> u32 {
        match r { ApiResult::Normal(v) => v, other => panic!("{other:?}") }
    }

    fn put_str(l: &Loader, at: u32, s: &str) -> u32 {
        l.guest_write_bytes(at, format!("{s}\0").as_bytes()).unwrap();
        at
    }

    /// A frame at (100, 50), 300 × 200, with a guest client procedure and a
    /// container in the client at (10, 10), 100 × 80: (frame, client, cnr).
    fn desk(l: &Loader) -> (u32, u32, u32) {
        let mut wm = l.shared.window_mgr.lock_or_recover();
        let hmq = wm.create_mq(437);
        wm.tid_to_hmq.insert(0, hmq);
        let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
        let client = wm.create_window("App".into(), frame, hmq);
        wm.frame_to_client.insert(frame, client);
        {
            let w = wm.get_window_mut(frame).unwrap();
            (w.x, w.y, w.cx, w.cy, w.visible, w.frame_flags) = (100, 50, 300, 200, true, FCF_TITLEBAR | FCF_SIZEBORDER);
        }
        wm.layout_frame(frame);
        let c = wm.get_window_mut(client).unwrap();
        (c.visible, c.pfn_wp) = (true, PROC);
        let cnr = wm.create_window(super::super::pm_container::CLASS_CONTAINER.into(), client, hmq);
        let w = wm.get_window_mut(cnr).unwrap();
        (w.x, w.y, w.cx, w.cy, w.visible, w.owner, w.id) = (10, 10, 100, 80, true, client, 5);
        wm.build_container(cnr);
        wm.z_push_top(frame);
        (frame, client, cnr)
    }

    #[test]
    fn draginfo_items_and_strings_through_the_api() {
        let l = Loader::new_mock();
        let pd = ok(l.drg_alloc_draginfo(0, 2));
        assert_ne!(pd, 0);
        assert_eq!(l.guest_read::<u32>(pd + DI_CB_DRAGINFO), Some(DRAGINFO_SIZE + 2 * DRAGITEM_SIZE));
        assert_eq!(l.guest_read::<u16>(pd + DI_CB_DRAGITEM), Some(DRAGITEM_SIZE as u16));
        assert_eq!(l.guest_read::<u16>(pd + DI_OPERATION), Some(DO_DEFAULT));
        assert_eq!(ok(l.drg_query_dragitem_count(0, pd)), 2);
        assert_eq!(ok(l.drg_alloc_draginfo(0, 0)), 0);
        // String handles.
        let ht = ok(l.drg_add_str_handle(0, put_str(&l, BUF, "Plain Text,C Code")));
        let hr = ok(l.drg_add_str_handle(0, put_str(&l, BUF + 0x40, "<DRM_OS2FILE,DRF_TEXT>,(DRM_PRINT)x(DRF_TEXT)")));
        let hs = ok(l.drg_add_str_handle(0, put_str(&l, BUF + 0x80, "hello.txt")));
        assert_eq!(ok(l.drg_query_str_name_len(0, hs)), 9);
        assert_eq!(ok(l.drg_query_str_name(0, hs, 6, BUF + 0x100)), 5, "cut to the buffer");
        assert_eq!(l.read_guest_string(BUF + 0x100), "hello");
        assert_eq!(ok(l.drg_query_str_name_len(0, 0x7777)), 0);
        // An item set and read back, through its pointer.
        let item = BUF + 0x200;
        for (o, v) in [(DIT_HWND_ITEM, 0x1234), (DIT_TYPE, ht), (DIT_RMF, hr), (DIT_SOURCE, hs)] {
            l.guest_write::<u32>(item + o, v).unwrap();
        }
        assert_eq!(ok(l.drg_copy_item(0, pd, item, DRAGITEM_SIZE, 1, true)), 1);
        assert_eq!(ok(l.drg_copy_item(0, pd, item, DRAGITEM_SIZE, 2, true)), 0, "no third item");
        let p1 = ok(l.drg_query_dragitem_ptr(0, pd, 1));
        assert_eq!(p1, pd + DRAGINFO_SIZE + DRAGITEM_SIZE);
        assert_eq!(l.guest_read::<u32>(p1 + DIT_SOURCE), Some(hs));
        assert_eq!(ok(l.drg_info_from_item(p1)), pd);
        // Types and RMFs.
        assert_eq!(ok(l.drg_verify_type(p1, put_str(&l, BUF + 0x300, "C Code"), false)), 1);
        assert_eq!(ok(l.drg_verify_type(p1, BUF + 0x300, true)), 0, "not the true type");
        assert_eq!(ok(l.drg_verify_type(p1, put_str(&l, BUF + 0x300, "Plain Text"), true)), 1);
        assert_eq!(ok(l.drg_query_true_type(p1, 0, 0, true)), 10);
        assert_eq!(ok(l.drg_verify_rmf(p1, put_str(&l, BUF + 0x320, "DRM_PRINT"), 0)), 1);
        assert_eq!(ok(l.drg_verify_rmf(p1, BUF + 0x320, put_str(&l, BUF + 0x340, "DRF_UNKNOWN"))), 0);
        assert_eq!(ok(l.drg_verify_native_rmf(p1, put_str(&l, BUF + 0x360, "<DRM_OS2FILE,DRF_TEXT>"))), 1);
        assert_eq!(ok(l.drg_query_native_rmf(p1, 64, BUF + 0x380, false)), 1);
        assert_eq!(l.read_guest_string(BUF + 0x380), "<DRM_OS2FILE,DRF_TEXT>");
        assert_eq!(ok(l.drg_verify_type_set(p1, put_str(&l, BUF + 0x3C0, "Bitmap,C Code"), 32, BUF + 0x3E0)), 1);
        assert_eq!(l.read_guest_string(BUF + 0x3E0), "C Code");
        assert_eq!(ok(l.drg_verify_type(0x5555, BUF + 0x300, false)), 0, "not an item");
        // The item's handles go; the DRAGINFO is freed.
        assert_eq!(ok(l.drg_delete_draginfo_str_handles(0, pd)), 1);
        assert_eq!(ok(l.drg_query_str_name_len(0, hs)), 0);
        assert_eq!(ok(l.drg_free_draginfo(0, pd)), 1);
        assert_eq!(ok(l.drg_access_draginfo(0, pd)), 0);
        assert_eq!(ok(l.drg_free_draginfo(0, pd)), 0);
    }

    #[test]
    fn drag_transfers_are_allocated_and_reset() {
        let l = Loader::new_mock();
        let t = ok(l.drg_alloc_dragtransfer(0, 2));
        assert_eq!(l.guest_read::<u32>(t + DRAGTRANSFER_SIZE), Some(DRAGTRANSFER_SIZE));
        l.guest_write::<u16>(t + DT_FS_REPLY, 7).unwrap();
        l.drg_transfer_reset(DM_RENDER, t);
        assert_eq!(l.guest_read::<u16>(t + DT_FS_REPLY), Some(0));
        assert_eq!(ok(l.drg_free_dragtransfer(0, t)), 1);
        assert_eq!(ok(l.drg_free_dragtransfer(0, t)), 0);
    }

    #[test]
    fn the_window_under_a_point_is_a_control_or_the_client() {
        let l = Loader::new_mock();
        let (_frame, client, cnr) = desk(&l);
        let wm = l.shared.window_mgr.lock_or_recover();
        let (ox, oy) = wm.window_origin(client).unwrap();
        assert_eq!(wm.window_under(ox + 50, oy + 40), Some((cnr, 40, 30)));
        assert_eq!(wm.window_under(ox + 200, oy + 40), Some((client, 200, 40)));
        assert_eq!(wm.window_under(5, 5), None, "the desktop");
    }

    /// Run drag chains, answering each guest call with `answer(msg)`; the
    /// messages made, in order, and the result.
    fn run(l: &Loader, mut r: ApiResult, answer: &dyn Fn(&[u32]) -> u32) -> (Vec<Vec<u32>>, u32) {
        let mut calls = Vec::new();
        loop {
            let mut chain = match r {
                ApiResult::Normal(v) => return (calls, v),
                ApiResult::CallChain(c) => c,
                other => panic!("{other:?}"),
            };
            while let Some(step) = chain.steps.pop_front() {
                let CallStep::Guest { args, block, .. } = step else { panic!("built-in step") };
                let mut seen = args.clone();
                seen.extend(block);
                chain.returned(answer(&seen), false);
                calls.push(seen);
            }
            r = match l.finish_chain(&chain) {
                ChainEnd::Next(c) => ApiResult::CallChain(c),
                ChainEnd::Return(v) => ApiResult::Normal(v),
                other => panic!("{other:?}"),
            };
        }
    }

    #[test]
    fn a_drag_over_the_client_drops_where_it_is_accepted() {
        let l = Loader::new_mock();
        let (frame, client, _cnr) = desk(&l);
        let (ox, oy) = l.shared.window_mgr.lock_or_recover().window_origin(client).unwrap();
        let pd = ok(l.drg_alloc_draginfo(0, 1));
        // WM_BEGINDRAG armed the feed; the pointer moved over the client and
        // the button came up before DrgDrag looked.
        l.shared.drag_channel.arm(DRAG_BUTTON, (ox + 150, oy + 50));
        l.shared.drag_channel.released(DRAG_BUTTON, (ox + 160, oy + 60));
        let (calls, target) = run(&l, l.drg_drag(0, frame, pd, VK_ENDDRAG), &|c| {
            if c[1] == DM_DRAGOVER { DOR_DROP | (0x10 << 16) } else { 0 }
        });
        assert_eq!(target, client);
        let msgs: Vec<u32> = calls.iter().map(|c| c[1]).collect();
        assert_eq!(msgs, vec![DM_DRAGOVER, DM_DROP]);
        assert_eq!(calls[0][3], ((ox + 160) as u32 & 0xFFFF) | (((oy + 60) as u32) << 16), "desktop coordinates");
        assert_eq!(l.guest_read::<i16>(pd + DI_X_DROP), Some((ox + 160) as i16));
        assert_eq!(l.guest_read::<u32>(pd + DI_HWND_SOURCE), Some(frame));
        assert_eq!(ok(l.drg_query_drag_status()), 0, "over");
        assert!(!l.shared.drag_channel.active());
    }

    #[test]
    fn a_refused_drop_leaves_and_returns_nothing() {
        let l = Loader::new_mock();
        let (frame, client, _cnr) = desk(&l);
        let (ox, oy) = l.shared.window_mgr.lock_or_recover().window_origin(client).unwrap();
        let pd = ok(l.drg_alloc_draginfo(0, 1));
        l.shared.drag_channel.arm(DRAG_BUTTON, (ox + 150, oy + 50));
        l.shared.drag_channel.released(DRAG_BUTTON, (ox + 150, oy + 50));
        let (calls, target) = run(&l, l.drg_drag(0, frame, pd, VK_ENDDRAG), &|_| DOR_NODROP);
        assert_eq!(target, 0);
        assert_eq!(calls.iter().map(|c| c[1]).collect::<Vec<_>>(), vec![DM_DRAGOVER, DM_DRAGLEAVE]);
    }

    #[test]
    fn a_container_target_asks_its_owner() {
        let l = Loader::new_mock();
        let (frame, client, cnr) = desk(&l);
        let (ox, oy) = l.shared.window_mgr.lock_or_recover().window_origin(cnr).unwrap();
        let pd = ok(l.drg_alloc_draginfo(0, 1));
        l.shared.drag_channel.arm(DRAG_BUTTON, (ox + 20, oy + 20));
        l.shared.drag_channel.released(DRAG_BUTTON, (ox + 20, oy + 20));
        let (calls, target) = run(&l, l.drg_drag(0, frame, pd, VK_ENDDRAG), &|c| {
            if c[2] >> 16 == super::super::pm_container::CN_DRAGOVER { DOR_DROP } else { 0 }
        });
        assert_eq!(target, cnr);
        use super::super::pm_container::{CN_DRAGOVER, CN_DROP};
        let codes: Vec<(u32, u32, u32)> = calls.iter().map(|c| (c[0], c[1], c[2] >> 16)).collect();
        assert_eq!(codes, vec![(client, WM_CONTROL, CN_DRAGOVER), (client, WM_CONTROL, CN_DROP)]);
        assert_eq!((calls[0][4], calls[0][5]), (pd, 0), "CNRDRAGINFO: no record under the pointer");
    }

    #[test]
    fn the_draginfo_cannot_be_freed_during_the_drag() {
        let l = Loader::new_mock();
        let (frame, client, _cnr) = desk(&l);
        let (ox, oy) = l.shared.window_mgr.lock_or_recover().window_origin(client).unwrap();
        let pd = ok(l.drg_alloc_draginfo(0, 1));
        l.shared.drag_channel.arm(DRAG_BUTTON, (ox + 150, oy + 50));
        let r = l.drg_drag(0, frame, pd, VK_ENDDRAG);
        assert!(matches!(r, ApiResult::CallChain(_)), "DM_DRAGOVER");
        assert_eq!(ok(l.drg_query_drag_status()), DGS_DRAGINPROGRESS);
        assert_eq!(ok(l.drg_free_draginfo(0, pd)), 0, "PMERR_SOURCE_SAME_AS_TARGET");
        assert_eq!(ok(l.drg_drag(0, frame, pd, VK_ENDDRAG)), 0, "already dragging");
        // Esc ends it.
        assert!(l.shared.drag_channel.cancel());
        let ApiResult::CallChain(mut chain) = r else { unreachable!() };
        chain.steps.clear();
        chain.returned(DOR_DROP, false);
        let end = l.finish_chain(&chain);
        let ChainEnd::Next(mut leave) = end else { panic!("{end:?}") };
        leave.steps.clear();
        leave.returned(0, false);
        assert_eq!(l.finish_chain(&leave), ChainEnd::Return(0));
        assert_eq!(ok(l.drg_free_draginfo(0, pd)), 1);
    }

    #[test]
    fn without_the_sessions_desktop_drag_blocks_stay_in_the_process() {
        let l = Loader::new_mock();
        let (frame, client, _cnr) = desk(&l);
        let pd = ok(l.drg_alloc_draginfo(0, 1));
        assert!(!in_arena(pd), "no session: process memory");
        assert!(!l.give_drag_block(pd, 7), "nothing to give");
        assert!(l.accept_draginfo(pd), "its own");
        assert!(!l.accept_draginfo(0x4000), "not a DRAGINFO, not in the arena");
        // DrgPushDraginfo to this program's windows needs nothing.
        assert_eq!(ok(l.drg_push_draginfo(0, pd, client)), 1);
        assert_eq!(ok(l.drg_push_draginfo(0, pd, 0x7777)), 0, "no such window");
        assert_eq!(ok(l.drg_push_draginfo(0, 0x4000, frame)), 0, "not a DRAGINFO");
        // Transfers sent or posted to this program's windows take the
        // ordinary path; a transfer it was never given cannot be freed.
        let t = ok(l.drg_alloc_dragtransfer(0, 1));
        assert_eq!(l.transfer_block(t + 4), Some(t));
        assert!(l.drg_transfer_to_other(0, &[client, DM_RENDER, t, 0], false).is_none());
        assert_eq!(ok(l.drg_free_dragtransfer(0, 0x4000)), 0);
        assert_eq!(ok(l.drg_free_dragtransfer(0, t)), 1);
    }

    #[test]
    fn drag_input_is_fed_only_while_armed_or_active() {
        let ch = DragChannel::default();
        ch.moved((5, 5));
        assert!(!ch.feed.lock_or_recover().moved, "no drag");
        assert!(!ch.released(2, (5, 5)));
        assert!(!ch.cancel());
        ch.arm(2, (1, 1));
        ch.moved((7, 8));
        assert_eq!(ch.feed.lock_or_recover().pos, (7, 8));
        assert!(!ch.released(2, (7, 8)), "not active yet: the release is routed too");
        assert_eq!(ch.feed.lock_or_recover().released, Some(2));
        assert_eq!(ch.feed.lock_or_recover().armed, Some(2), "kept for a DrgDrag that starts late");
        ch.reset_if_idle();
        assert_eq!(*ch.feed.lock_or_recover(), DragFeed::default());
    }

    #[test]
    fn string_handles_are_shared_and_counted() {
        let mut t = StrHandles::default();
        let a = t.add(b"DRT_TEXT").unwrap();
        let b = t.add(b"Plain Text").unwrap();
        assert_ne!(a, b);
        assert_eq!(t.add(b"DRT_TEXT"), Some(a), "the same string, the same handle");
        assert_eq!(t.get(a), Some(&b"DRT_TEXT"[..]));
        assert!(t.delete(a));
        assert_eq!(t.get(a), Some(&b"DRT_TEXT"[..]), "one reference left");
        assert!(t.delete(a));
        assert_eq!(t.get(a), None);
        assert!(!t.delete(a));
        assert!(t.add(&vec![b'x'; MAX_STR_LEN + 1]).is_none());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn types_split_on_commas() {
        assert_eq!(types("Plain Text, C Code,"), vec!["Plain Text", "C Code"]);
        assert!(types("").is_empty());
    }

    #[test]
    fn rmf_strings_expand_to_pairs() {
        let p = rmf_pairs("<DRM_OS2FILE,DRF_TEXT>,(DRM_PRINT,DRM_DISCARD)x(DRF_UNKNOWN, DRF_TEXT)");
        let want: Vec<(String, String)> = [("DRM_OS2FILE", "DRF_TEXT"), ("DRM_PRINT", "DRF_UNKNOWN"), ("DRM_PRINT", "DRF_TEXT"),
                                           ("DRM_DISCARD", "DRF_UNKNOWN"), ("DRM_DISCARD", "DRF_TEXT")]
            .iter().map(|(m, f)| (m.to_string(), f.to_string())).collect();
        assert_eq!(p, want);
        assert_eq!(native_rmf(&p).as_deref(), Some("<DRM_OS2FILE,DRF_TEXT>"));
        assert!(rmf_has(&p, Some("DRM_PRINT"), None));
        assert!(rmf_has(&p, None, Some("DRF_UNKNOWN")));
        assert!(!rmf_has(&p, Some("DRM_OS2FILE"), Some("DRF_UNKNOWN")));
        assert!(rmf_pairs("<broken").is_empty());
        assert_eq!(rmf_pairs("garbage <A,B>").len(), 1);
    }

    #[test]
    fn items_are_found_in_their_draginfo() {
        let mut d = DragTable::default();
        d.infos.insert(0x1000, DragInfoBlock { items: 2 });
        assert_eq!(d.info_of_item(0x1000 + DRAGINFO_SIZE), Some((0x1000, 0)));
        assert_eq!(d.info_of_item(0x1000 + DRAGINFO_SIZE + DRAGITEM_SIZE), Some((0x1000, 1)));
        assert_eq!(d.info_of_item(0x1000 + DRAGINFO_SIZE + 2 * DRAGITEM_SIZE), None);
        assert_eq!(d.info_of_item(0x1000 + DRAGINFO_SIZE + 4), None);
    }

    #[test]
    fn operations_follow_the_cua_keys() {
        assert_eq!(operation_for(false, false), DO_DEFAULT);
        assert_eq!(operation_for(true, false), DO_COPY);
        assert_eq!(operation_for(false, true), DO_MOVE);
        assert_eq!(operation_for(true, true), DO_LINK);
        let ch = DragChannel::default();
        assert_eq!(ch.operation(), DO_DEFAULT, "no key seen yet");
        ch.modifiers(true, false);
        assert_eq!(ch.operation(), DO_COPY);
        assert!(!ch.feed.lock_or_recover().moved, "no drag: nothing to ask again");
        ch.feed.lock_or_recover().active = true;
        ch.modifiers(true, true);
        assert!(ch.feed.lock_or_recover().moved, "a running drag asks again");
    }

    #[test]
    fn file_paths_split_into_container_and_name() {
        assert_eq!(split_file_path(b"C:\\DOCS\\a.txt"), Some((&b"C:\\DOCS\\"[..], &b"a.txt"[..])));
        assert_eq!(split_file_path(b"C:\\a"), Some((&b"C:\\"[..], &b"a"[..])));
        assert_eq!(split_file_path(b"name"), Some((&b""[..], &b"name"[..])));
        assert_eq!(split_file_path(b"C:\\DOCS\\"), None);
    }

    /// The string of handle field `off` of item `i` of `pd`.
    fn item_text(l: &Loader, pd: u32, i: u32, off: u32) -> String {
        l.hstr_text(l.item_hstr(pd + DRAGINFO_SIZE + i * DRAGITEM_SIZE, off))
    }

    fn file(path: &str, kind: Option<&str>, target: Option<&str>, dir: bool) -> FileItem {
        FileItem { path: path.as_bytes().to_vec(), kind: kind.map(|k| k.as_bytes().to_vec()),
                   target: target.map(|t| t.as_bytes().to_vec()), dir }
    }

    #[test]
    fn file_draginfos_name_each_file_and_go_with_their_handles() {
        let l = Loader::new_mock();
        let files = [file("C:\\DOCS\\a.txt", None, None, false), file("C:\\DOCS\\SUB", Some("Plain Text"), Some("b"), true)];
        let pd = l.file_draginfo(0, &files, 0x77).unwrap();
        assert_eq!(l.guest_read::<u32>(pd + DI_HWND_SOURCE), Some(0x77));
        assert_eq!(ok(l.drg_query_dragitem_count(0, pd)), 2);
        let item = |i: u32, off: u32| l.guest_read::<u16>(pd + DRAGINFO_SIZE + i * DRAGITEM_SIZE + off).unwrap();
        assert_eq!(item_text(&l, pd, 0, DIT_TYPE), "Unknown");
        assert_eq!(item_text(&l, pd, 0, DIT_RMF), "<DRM_OS2FILE,DRF_UNKNOWN>");
        assert_eq!(item_text(&l, pd, 0, DIT_CONTAINER), "C:\\DOCS\\");
        assert_eq!(item_text(&l, pd, 0, DIT_SOURCE), "a.txt");
        assert_eq!(item_text(&l, pd, 0, DIT_TARGET), "a.txt");
        assert_eq!((item(0, DIT_FS_CONTROL), item(0, DIT_FS_SUPPORTED_OPS)), (0, FILE_OPS));
        assert_eq!(l.guest_read::<u32>(pd + DRAGINFO_SIZE + DIT_HWND_ITEM), Some(0x77));
        assert_eq!(item_text(&l, pd, 1, DIT_TYPE), "Plain Text");
        assert_eq!(item_text(&l, pd, 1, DIT_TARGET), "b");
        assert_eq!(item(1, DIT_FS_CONTROL), DC_CONTAINER, "a directory");
        assert_eq!(l.guest_read::<u32>(pd + DRAGINFO_SIZE + DRAGITEM_SIZE + DIT_ITEM_ID), Some(1));
        let source = l.item_hstr(pd + DRAGINFO_SIZE, DIT_SOURCE);
        l.discard_draginfo(pd);
        assert_eq!(ok(l.drg_access_draginfo(0, pd)), 0, "gone");
        assert_eq!(ok(l.drg_query_str_name_len(0, source)), 0, "with its handles");
        l.discard_draginfo(pd);
        // A path without a name makes none, and leaves nothing behind.
        assert!(l.file_draginfo(0, &[file("C:\\", None, None, true)], 0).is_none());
        assert!(l.shared.window_mgr.lock_or_recover().drag.infos.is_empty());
    }

    #[test]
    fn draginfos_are_enumerated_found_by_source_and_reallocated() {
        let l = Loader::new_mock();
        let (a, b) = (ok(l.drg_alloc_draginfo(0, 1)), ok(l.drg_alloc_draginfo(0, 2)));
        let (lo, hi) = (a.min(b), a.max(b));
        assert_eq!(ok(l.drg_query_draginfo_ptr(0)), lo);
        assert_eq!(ok(l.drg_query_draginfo_ptr(lo)), hi);
        assert_eq!(ok(l.drg_query_draginfo_ptr(hi)), 0, "after the last");
        l.guest_write::<u32>(b + DI_HWND_SOURCE, 0x55).unwrap();
        assert_eq!(ok(l.drg_query_draginfo_from_hwnd(0x55)), b);
        assert_eq!(ok(l.drg_query_draginfo_from_hwnd(0x66)), 0);
        assert_eq!(ok(l.drg_query_draginfo_from_hwnd(0)), 0);
        // Reallocated: the header and the items that fit come along.
        l.guest_write::<u32>(b + DRAGINFO_SIZE + DIT_ITEM_ID, 9).unwrap();
        l.guest_write::<u16>(b + DI_OPERATION, DO_COPY).unwrap();
        l.guest_write::<i16>(b + DI_X_DROP, 7).unwrap();
        let c = ok(l.drg_realloc_draginfo(0, b, 3));
        assert_ne!(c, 0);
        assert_eq!(ok(l.drg_access_draginfo(0, b)), 0, "the old one is freed");
        assert_eq!(ok(l.drg_query_dragitem_count(0, c)), 3);
        assert_eq!(l.guest_read::<u32>(c + DI_CB_DRAGINFO), Some(DRAGINFO_SIZE + 3 * DRAGITEM_SIZE));
        assert_eq!(l.guest_read::<u32>(c + DRAGINFO_SIZE + DIT_ITEM_ID), Some(9));
        assert_eq!(l.guest_read::<u16>(c + DI_OPERATION), Some(DO_COPY));
        assert_eq!(l.guest_read::<i16>(c + DI_X_DROP), Some(7));
        assert_eq!(l.guest_read::<u32>(c + DI_HWND_SOURCE), Some(0x55));
        let d = ok(l.drg_realloc_draginfo(0, c, 1));
        assert_eq!(ok(l.drg_query_dragitem_count(0, d)), 1);
        assert_eq!(ok(l.drg_realloc_draginfo(0, 0x1234, 1)), 0, "not a DRAGINFO");
        assert_eq!(ok(l.drg_realloc_draginfo(0, d, 0)), 0, "no items");
        assert_eq!(ok(l.drg_access_draginfo(0, d)), 1, "kept when refused");
    }

    #[test]
    fn the_keys_held_choose_the_operation_of_each_dragover() {
        let l = Loader::new_mock();
        let (frame, client, _cnr) = desk(&l);
        let (ox, oy) = l.shared.window_mgr.lock_or_recover().window_origin(client).unwrap();
        let pd = ok(l.drg_alloc_draginfo(0, 1));
        l.shared.drag_channel.modifiers(true, false);
        l.shared.drag_channel.arm(DRAG_BUTTON, (ox + 150, oy + 50));
        l.shared.drag_channel.released(DRAG_BUTTON, (ox + 150, oy + 50));
        let seen = std::cell::Cell::new(0);
        let (_, target) = run(&l, l.drg_drag(0, frame, pd, VK_ENDDRAG), &|c| {
            if c[1] == DM_DRAGOVER {
                seen.set(l.guest_read::<u16>(pd + DI_OPERATION).unwrap());
                DOR_DROP
            } else { 0 }
        });
        assert_eq!(target, client);
        assert_eq!(seen.get(), DO_COPY, "Ctrl held");
        l.shared.drag_channel.modifiers(false, false);
    }

    #[test]
    fn drag_files_drops_named_files_and_says_whether_it_did() {
        let l = Loader::new_mock();
        let (frame, client, _cnr) = desk(&l);
        let (ox, oy) = l.shared.window_mgr.lock_or_recover().window_origin(client).unwrap();
        let names = [put_str(&l, BUF + 0x100, "C:\\X\\one.txt"), put_str(&l, BUF + 0x140, "C:\\two.dat")];
        for (i, n) in names.iter().enumerate() { l.guest_write::<u32>(BUF + i as u32 * 4, *n).unwrap(); }
        let args = |vk: u32| [frame, BUF, 0, 0, 2, 0, vk, 0, 0, 0];
        for (answer, dropped) in [(DOR_DROP, 1), (DOR_NODROP, 0)] {
            l.shared.drag_channel.arm(DRAG_BUTTON, (ox + 150, oy + 50));
            l.shared.drag_channel.released(DRAG_BUTTON, (ox + 150, oy + 50));
            let names_seen = std::cell::RefCell::new(Vec::new());
            let (calls, result) = run(&l, l.drg_drag_files(0, &args(VK_ENDDRAG)), &|c| {
                if c[1] == DM_DRAGOVER {
                    names_seen.borrow_mut().extend([item_text(&l, c[2], 0, DIT_SOURCE), item_text(&l, c[2], 1, DIT_CONTAINER)]);
                    answer
                } else { 0 }
            });
            assert_eq!(result, dropped, "TRUE only when dropped");
            assert_eq!(*names_seen.borrow(), vec!["one.txt".to_string(), "C:\\".to_string()]);
            assert_eq!(ok(l.drg_access_draginfo(0, calls[0][2])), 0, "its DRAGINFO goes with the drag");
        }
        assert_eq!(ok(l.drg_drag_files(0, &args(0x77))), 0, "a bad vkTerminate");
        assert_eq!(ok(l.drg_drag_files(0, &[frame, 0, 0, 0, 2, 0, VK_ENDDRAG, 0, 0, 0])), 0, "no files");
        assert!(l.shared.window_mgr.lock_or_recover().drag.infos.is_empty(), "nothing left behind");
    }

    /// Run the chain of a sent message, answering every call with `answer`:
    /// the calls made (arguments, then the block).
    fn run_sent_chain(l: &Loader, r: ApiResult, answer: u32) -> Vec<Vec<u32>> {
        let ApiResult::CallChain(mut chain) = r else { panic!("{r:?}") };
        let mut calls = Vec::new();
        while let Some(step) = chain.steps.pop_front() {
            let CallStep::Guest { args, block, .. } = step else { panic!("built-in step") };
            calls.push(args.into_iter().chain(block).collect());
            chain.returned(answer, false);
        }
        assert!(matches!(l.finish_chain(&chain), ChainEnd::Rerun(0)));
        calls
    }

    #[test]
    fn host_files_dropped_on_a_window_are_offered_then_dropped_or_left() {
        use super::super::pm_remote::{AfterSent, SentKind};
        let l = Loader::new_mock();
        let (frame, client, _cnr) = desk(&l);
        let (ox, oy) = l.shared.window_mgr.lock_or_recover().window_origin(client).unwrap();
        let next = |l: &Loader| {
            let wm = l.shared.window_mgr.lock_or_recover();
            wm.thread_queue(0).and_then(|h| wm.take_sent(h))
        };
        for (answer, then) in [(DOR_DROP, DM_DROP), (DOR_NODROP, DM_DRAGLEAVE)] {
            let files = vec![("C:\\IN\\a b.txt".to_string(), false)];
            assert!(l.shared.window_mgr.lock_or_recover().queue_host_drop(ox + 150, oy + 50, files));
            let s = next(&l).expect("queued for the window's thread");
            assert!(matches!(s.kind, SentKind::HostDrop { stage: HOST_DROP_OVER, .. }));
            assert_eq!(s.hwnd, client);
            let over = run_sent_chain(&l, l.run_sent(0, s, AfterSent::Rerun(0)), answer);
            assert_eq!((over[0][0], over[0][1]), (client, DM_DRAGOVER));
            let pd = over[0][2];
            assert_eq!(item_text(&l, pd, 0, DIT_CONTAINER), "C:\\IN\\");
            assert_eq!(item_text(&l, pd, 0, DIT_SOURCE), "a b.txt");
            assert_eq!(l.guest_read::<u32>(pd + DI_HWND_SOURCE), Some(0), "the system is the source");
            assert_eq!(l.guest_read::<i16>(pd + DI_X_DROP), Some((ox + 150) as i16));
            let s = next(&l).expect("the next step follows");
            let last = run_sent_chain(&l, l.run_sent(0, s, AfterSent::Rerun(0)), 0);
            assert_eq!((last[0][1], last[0][2]), (then, pd));
            assert!(next(&l).is_none());
            // After the drop the target frees it; after the leave it is gone.
            assert_eq!(ok(l.drg_access_draginfo(0, pd)), (then == DM_DROP) as u32);
        }
        let mut wm = l.shared.window_mgr.lock_or_recover();
        assert!(!wm.queue_host_drop(5, 5, vec![("C:\\a".into(), false)]), "the desktop takes no drop");
        assert!(!wm.queue_host_drop(ox + 150, oy + 50, vec![]), "no files");
        if let Some(w) = wm.get_window_mut(frame) { w.style |= WS_DISABLED; }
        assert!(!wm.queue_host_drop(ox + 150, oy + 50, vec![("C:\\a".into(), false)]), "a disabled frame");
    }
}
