// SPDX-License-Identifier: GPL-3.0-only
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
//! Only windows of this process take part: there is one OS/2 program per
//! host process (ADR-0020) until the session desktop server exists.
//! Messages for another thread's window are made on the dragging thread.

use std::collections::HashMap;

use super::constants::*;
use super::pm_hook::{CallChain, CallStep, ChainEnd, ChainThen};
use super::pm_types::WindowManager;
use super::{ApiResult, Loader, MutexExt};

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
pub const DIT_TYPE: u32 = 8;
pub const DIT_RMF: u32 = 12;
pub const DIT_CONTAINER: u32 = 16;
pub const DIT_SOURCE: u32 = 20;
pub const DIT_TARGET: u32 = 24;
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

pub const DM_DROP: u32 = 0x032f;
pub const DM_DRAGOVER: u32 = 0x032e;
pub const DM_DRAGLEAVE: u32 = 0x032d;
pub const DM_RENDER: u32 = 0x0329;
pub const DM_RENDERCOMPLETE: u32 = 0x0328;
pub const WM_BEGINDRAG: u32 = 0x0420;

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
    /// The last chain ends with a `DM_DRAGOVER` whose reply is pending.
    awaiting: bool,
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
}

/// [`DragFeed`] with the condition `DrgDrag` waits on.
#[derive(Debug, Default)]
pub struct DragChannel {
    pub feed: std::sync::Mutex<DragFeed>,
    pub cond: std::sync::Condvar,
}

impl DragChannel {
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
        self.cond.notify_all();
    }

    /// Button `button` came up at `pos`; whether the drag takes it.
    pub fn released(&self, button: u8, pos: (i32, i32)) -> bool {
        let mut f = self.feed.lock_or_recover();
        if f.armed.is_none() && !f.active { return false; }
        if f.pos != pos { f.pos = pos; f.moved = true; }
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
        *f = DragFeed { armed: Some(button), pos, moved: true, ..DragFeed::default() };
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
    /// with the point in its coordinates: a built-in control, a dialog
    /// embedded in the frame, or the client (dialog) itself; `None` over a
    /// frame's border and title bar or the desktop.
    pub fn window_under(&self, x: i32, y: i32) -> Option<(u32, i32, i32)> {
        let frame = self.z_hit_test(x, y)?;
        let target = self.frame_to_client.get(&frame).copied().unwrap_or(frame);
        let (ox, oy) = self.window_origin(target)?;
        let (lx, ly) = (x - ox, y - oy);
        // As the renderer routes button 1: from message coordinates to the
        // frame surface (a client lies at its place in it; a dialog's
        // surface starts inside its left border).
        let (sx, sy) = match self.get_window(frame) {
            Some(w) if !self.frame_to_client.contains_key(&frame) => (lx - frame_border_width(w.frame_flags), ly),
            _ => {
                let (cx, cy, _, _) = self.client_view(frame).unwrap_or_default();
                (lx.saturating_add(cx), ly.saturating_add(cy))
            }
        };
        if let Some(hit) = self.control_at(frame, sx, sy) { return Some(hit); }
        if let Some(root) = self.control_root(frame) && let Some(hit) = self.embedded_dialog_at(root, sx, sy) {
            return Some(hit);
        }
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

    /// The string of handle `h` (empty for none).
    fn hstr_text(&self, h: u32) -> String {
        let wm = self.shared.window_mgr.lock_or_recover();
        wm.drag.strings.get(h).map_or(String::new(), |b| String::from_utf8_lossy(b).into_owned())
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
        if n == 0 || n > MAX_DRAG_ITEMS || self.shared.window_mgr.lock_or_recover().drag.infos.len() >= MAX_DRAG_INFOS {
            return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS);
        }
        let size = DRAGINFO_SIZE + n * DRAGITEM_SIZE;
        let Some(base) = self.shared.mem_mgr.lock_or_recover().alloc(size) else {
            return self.pm_fail(vcpu, PMERR_INSUFFICIENT_MEMORY);
        };
        if let Some(m) = self.guest_slice_mut(base, size as usize) { m.fill(0); }
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
        ApiResult::Normal(base)
    }

    /// `DrgFreeDraginfo(pdinfo)`. Within the process that is dragging it,
    /// before `DrgDrag` returns, it fails with `PMERR_SOURCE_SAME_AS_TARGET`
    /// (the source frees it afterwards).
    pub(crate) fn drg_free_draginfo(&self, vcpu: u32, pdinfo: u32) -> ApiResult {
        let freed = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            if wm.drag.in_drag(pdinfo) { drop(wm); return self.pm_fail(vcpu, PMERR_SOURCE_SAME_AS_TARGET); }
            wm.drag.infos.remove(&pdinfo).is_some()
        };
        if freed { self.shared.mem_mgr.lock_or_recover().free(pdinfo); }
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
        match self.shared.window_mgr.lock_or_recover().drag.strings.add(&bytes) {
            Some(h) => ApiResult::Normal(h),
            None => self.pm_fail(vcpu, PMERR_INSUFFICIENT_MEMORY),
        }
    }

    /// `DrgDeleteStrHandle(hstr)`.
    pub(crate) fn drg_delete_str_handle(&self, vcpu: u32, h: u32) -> ApiResult {
        let ok = self.shared.window_mgr.lock_or_recover().drag.strings.delete(h);
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
        let mut wm = self.shared.window_mgr.lock_or_recover();
        for h in handles { wm.drag.strings.delete(h); }
        ApiResult::Normal(1)
    }

    /// `DrgQueryStrName(hstr, cbBuffer, pBuffer)`: the bytes copied (a NUL
    /// after them); a bad handle gives an empty string.
    pub(crate) fn drg_query_str_name(&self, h: u32, cb: u32, buf: u32) -> ApiResult {
        if cb == 0 || buf == 0 { return ApiResult::Normal(0); }
        let bytes = self.shared.window_mgr.lock_or_recover().drag.strings.get(h).map(<[u8]>::to_vec).unwrap_or_default();
        ApiResult::Normal(self.copy_text_out(&bytes, buf, cb as usize))
    }

    /// `DrgQueryStrNameLen(hstr)`: without the NUL; 0 for a bad handle.
    pub(crate) fn drg_query_str_name_len(&self, h: u32) -> ApiResult {
        ApiResult::Normal(self.shared.window_mgr.lock_or_recover().drag.strings.get(h).map_or(0, |b| b.len() as u32))
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
        let Some(base) = self.shared.mem_mgr.lock_or_recover().alloc(size) else {
            return self.pm_fail(vcpu, PMERR_INSUFFICIENT_MEMORY);
        };
        if let Some(m) = self.guest_slice_mut(base, size as usize) { m.fill(0); }
        for i in 0..n { let _ = self.guest_write::<u32>(base + i * DRAGTRANSFER_SIZE, DRAGTRANSFER_SIZE); }
        self.shared.window_mgr.lock_or_recover().drag.transfers.insert(base, n);
        ApiResult::Normal(base)
    }

    /// `DrgFreeDragtransfer(pdxfer)`: the array it starts.
    pub(crate) fn drg_free_dragtransfer(&self, vcpu: u32, p: u32) -> ApiResult {
        let ok = self.shared.window_mgr.lock_or_recover().drag.transfers.remove(&p).is_some();
        if ok { self.shared.mem_mgr.lock_or_recover().free(p); }
        self.drag_ok(vcpu, ok, PMERR_INVALID_PARAMETERS)
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
        let button = match vk_terminate {
            VK_BUTTON1 => 1,
            VK_BUTTON2 => 2,
            VK_BUTTON3 => 3,
            VK_ENDDRAG => DRAG_BUTTON,
            _ => return self.pm_fail(vcpu, PMERR_INVALID_PARAMETERS),
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
                                                 pointer: 0, tid: vcpu, awaiting: false });
        }
        if self.guest_read::<u32>(pdinfo + DI_HWND_SOURCE) == Some(0) {
            let _ = self.guest_write::<u32>(pdinfo + DI_HWND_SOURCE, source);
        }
        let pointer = self.shared.window_mgr.lock_or_recover().key_state.pointer;
        {
            let mut f = self.shared.drag_channel.feed.lock_or_recover();
            // Input since WM_BEGINDRAG stays (a release may have come
            // already); without one the drag starts where the pointer is.
            if f.armed.is_none() {
                *f = DragFeed { pos: pointer, moved: true, ..DragFeed::default() };
            }
            f.armed = None;
            f.active = true;
        }
        self.drag_continue()
    }

    /// Wait for the next drag input.
    fn drag_wait(&self, button: u8) -> FeedStep {
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
            f = ch.cond.wait_timeout(f, FEED_WAIT).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// How the window under the pointer takes part: its procedure is
    /// called (a guest window), its owner is asked (a container: `CN_*`), or
    /// it never accepts (other built-in controls, a frame's chrome).
    fn drag_step(&self, target: u32, msg: u32, pdinfo: u32, pos: (i32, i32), local: (i32, i32)) -> Option<CallStep> {
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

    fn drag_chain(&self, tid: u32, steps: Vec<CallStep>, stage: u8) -> ApiResult {
        self.start_chain(CallChain::new(tid, steps, false, ChainThen::Drag { stage }))
    }

    /// End the drag: no session, input routed again, the pointer back.
    fn drag_end(&self, result: u32) -> ApiResult {
        self.shared.window_mgr.lock_or_recover().drag.session = None;
        *self.shared.drag_channel.feed.lock_or_recover() = DragFeed::default();
        self.drag_feedback(true, 0);
        ApiResult::Normal(result)
    }

    /// Go on with the drag: wait for input and send what it asks for.
    fn drag_continue(&self) -> ApiResult {
        loop {
            let Some(s) = self.shared.window_mgr.lock_or_recover().drag.session else { return ApiResult::Normal(0) };
            match self.drag_wait(s.button) {
                FeedStep::Move(pos) => {
                    let under = self.shared.window_mgr.lock_or_recover().window_under(pos.0, pos.1);
                    let (hwnd, local) = under.map_or((0, (0, 0)), |(h, x, y)| (h, (x, y)));
                    let mut steps = Vec::new();
                    let mut sess = s;
                    if hwnd != s.target {
                        if s.target != 0 && let Some(step) = self.drag_step(s.target, DM_DRAGLEAVE, s.pdinfo, pos, (0, 0)) {
                            steps.push(step);
                        }
                        sess.target = hwnd;
                        sess.never = false;
                        sess.reply = DOR_NODROP;
                    }
                    sess.awaiting = false;
                    if hwnd != 0 && !sess.never {
                        let _ = self.guest_write::<i16>(s.pdinfo + DI_X_DROP, pos.0 as i16);
                        let _ = self.guest_write::<i16>(s.pdinfo + DI_Y_DROP, pos.1 as i16);
                        match self.drag_step(hwnd, DM_DRAGOVER, s.pdinfo, pos, local) {
                            Some(step) => { steps.push(step); sess.awaiting = true; }
                            None => { sess.reply = DOR_NEVERDROP; sess.never = true; }
                        }
                    }
                    self.shared.window_mgr.lock_or_recover().drag.session = Some(sess);
                    if steps.is_empty() {
                        self.drag_feedback(sess.reply == DOR_DROP, sess.pointer);
                        continue;
                    }
                    return self.drag_chain(s.tid, steps, DRAG_STAGE_OVER);
                }
                FeedStep::Release(_) => {
                    let pos = self.shared.drag_channel.feed.lock_or_recover().pos;
                    if s.target != 0 && s.reply == DOR_DROP {
                        let _ = self.guest_write::<i16>(s.pdinfo + DI_X_DROP, pos.0 as i16);
                        let _ = self.guest_write::<i16>(s.pdinfo + DI_Y_DROP, pos.1 as i16);
                        let local = self.shared.window_mgr.lock_or_recover().window_under(pos.0, pos.1)
                            .filter(|w| w.0 == s.target).map_or((0, 0), |w| (w.1, w.2));
                        if let Some(step) = self.drag_step(s.target, DM_DROP, s.pdinfo, pos, local) {
                            return self.drag_chain(s.tid, vec![step], DRAG_STAGE_DROP);
                        }
                    }
                    return self.drag_leave_and_end(s);
                }
                FeedStep::Cancel => return self.drag_leave_and_end(s),
                FeedStep::Interrupted => return self.drag_end(0),
            }
        }
    }

    /// No drop: `DM_DRAGLEAVE` to the window under the pointer, then the
    /// end with NULLHANDLE.
    fn drag_leave_and_end(&self, s: DragSession) -> ApiResult {
        if s.target != 0 && let Some(step) = self.drag_step(s.target, DM_DRAGLEAVE, s.pdinfo, (0, 0), (0, 0)) {
            return self.drag_chain(s.tid, vec![step], DRAG_STAGE_END);
        }
        self.drag_end(0)
    }

    /// A drag chain of `stage` returned `last` (the `DM_DRAGOVER` reply).
    pub(crate) fn drag_chain_done(&self, stage: u8, last: u32) -> ChainEnd {
        let result = match stage {
            DRAG_STAGE_OVER => {
                let s = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let Some(s) = wm.drag.session.as_mut() else { return ChainEnd::Return(0) };
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
        };
        match result {
            ApiResult::CallChain(c) => ChainEnd::Next(c),
            ApiResult::Normal(v) => ChainEnd::Return(v),
            other => { log::warn!("drag: unexpected {other:?}"); ChainEnd::Return(0) }
        }
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
        assert_eq!(ok(l.drg_query_str_name_len(hs)), 9);
        assert_eq!(ok(l.drg_query_str_name(hs, 6, BUF + 0x100)), 5, "cut to the buffer");
        assert_eq!(l.read_guest_string(BUF + 0x100), "hello");
        assert_eq!(ok(l.drg_query_str_name_len(0x7777)), 0);
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
        assert_eq!(ok(l.drg_query_str_name_len(hs)), 0);
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
}
