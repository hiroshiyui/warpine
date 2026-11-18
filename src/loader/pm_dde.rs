// SPDX-License-Identifier: GPL-3.0-only
//
// Dynamic Data Exchange (PMWIN), after the IBM PM Reference ("Dynamic Data
// Exchange Functions" and the `WM_DDE_*` messages): `WinDdeInitiate`
// (718), `WinDdeRespond` (720) and `WinDdePostMsg` (719).
//
// A client starts a conversation with `WinDdeInitiate`: `WM_DDE_INITIATE`
// is sent to every frame window (mp1 the client, mp2 a `DDEINIT` naming
// the application and topic wanted); a server that takes part answers
// with `WinDdeRespond`, which sends `WM_DDE_INITIATEACK` to the client
// (mp1 the server). The two then post each other `WM_DDE_REQUEST` /
// `DATA` / `ADVISE` / `UNADVISE` / `POKE` / `EXECUTE` / `ACK` /
// `TERMINATE` with `WinDdePostMsg` (mp1 the sender, mp2 a `DDESTRUCT` in
// shared memory the program allocated with `DosAllocSharedMem`).
//
// Scope: windows of one Warpine process — the threads of one OS/2 process.
// Every OS/2 process is a Warpine process of its own (ADR-0020), with its
// own windows and memory, so `WinDdeInitiate` reaches the frames of the
// calling process only and conversations with other processes are not
// possible; carrying `DDESTRUCT`s between processes would need a host
// channel and shared memory between Warpine processes (a decision of its
// own).
//
// The `DDEINIT` blocks are giveable shared memory (`DosFreeMem` frees
// them), one per receiving window. Messages go to a frame's client (in
// Warpine the frame shares its client's window procedure).
//
// Unverified (the Reference is silent or the behaviour concerns other
// processes): the receivers free the `DDEINIT` they get (Warpine never
// frees it); `WM_DDE_INITIATE` also reaching the client's own frame;
// `DDEINIT.cb` = `sizeof(DDEINIT)` and `offConvContext` 0 without a
// context; NULL names taken as empty; `WinDdePostMsg` not freeing the
// sender's `DDESTRUCT` (the receiver is in the same address space;
// `DDEPM_NOFREE` thus changes nothing) and accepting a NULL `DDESTRUCT`;
// `DDEPM_RETRY` ignored (queues do not fill up); a message outside
// `WM_DDE_FIRST`..`WM_DDE_LAST` failing with `PMERR_PARAMETER_OUT_OF_RANGE`.

use super::constants::*;
use super::managers::SharedBlock;
use super::mutex_ext::MutexExt;
use super::pm_atom::PMERR_HEAP_OUT_OF_MEMORY;
use super::pm_hook::{CallChain, CallStep, ChainThen};
use super::ApiResult;

// <pmwin.h> INCL_WINDDE.
pub const WM_DDE_FIRST: u32 = 0x00A0;
pub const WM_DDE_INITIATE: u32 = 0x00A0;
pub const WM_DDE_REQUEST: u32 = 0x00A1;
pub const WM_DDE_ACK: u32 = 0x00A2;
pub const WM_DDE_DATA: u32 = 0x00A3;
pub const WM_DDE_ADVISE: u32 = 0x00A4;
pub const WM_DDE_UNADVISE: u32 = 0x00A5;
pub const WM_DDE_POKE: u32 = 0x00A6;
pub const WM_DDE_EXECUTE: u32 = 0x00A7;
pub const WM_DDE_TERMINATE: u32 = 0x00A8;
pub const WM_DDE_INITIATEACK: u32 = 0x00A9;
pub const WM_DDE_LAST: u32 = 0x00AF;
pub const DDEPM_RETRY: u32 = 1;
pub const DDEPM_NOFREE: u32 = 2;

/// `sizeof(DDEINIT)`: cb, pszAppName, pszTopic, offConvContext.
pub const DDEINIT_SIZE: usize = 16;
/// `sizeof(CONVCONTEXT)`: six ULONGs.
pub const CONVCONTEXT_SIZE: usize = 24;
/// `sizeof(DDESTRUCT)`: cbData, fsStatus, usFormat, offszItemName, offabData.
pub const DDESTRUCT_SIZE: usize = 12;
/// Longest application or topic name taken.
pub const MAX_DDE_NAME: usize = 255;
/// Flags of the shared memory holding a `DDEINIT`.
const DDEINIT_FLAGS: u32 = PAG_READ | PAG_WRITE | PAG_COMMIT | OBJ_GIVEABLE | OBJ_GETTABLE;

/// The bytes of a `DDEINIT` placed at guest address `at`: the structure,
/// the conversation context (if any), then the two names.
pub fn ddeinit_bytes(at: u32, app: &[u8], topic: &[u8], context: Option<&[u8; CONVCONTEXT_SIZE]>) -> Vec<u8> {
    let ctx_off = if context.is_some() { DDEINIT_SIZE } else { 0 };
    let app_off = DDEINIT_SIZE + if context.is_some() { CONVCONTEXT_SIZE } else { 0 };
    let topic_off = app_off + app.len() + 1;
    let mut out = Vec::with_capacity(topic_off + topic.len() + 1);
    for v in [DDEINIT_SIZE as u32, at + app_off as u32, at + topic_off as u32, ctx_off as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    if let Some(c) = context {
        out.extend_from_slice(c);
    }
    out.extend_from_slice(app);
    out.push(0);
    out.extend_from_slice(topic);
    out.push(0);
    out
}

impl super::Loader {
    /// A DDE name (NULL: empty), bounded.
    fn dde_name(&self, psz: u32) -> Result<Vec<u8>, u32> {
        if psz == 0 { return Ok(Vec::new()); }
        self.guest_cstr(psz, MAX_DDE_NAME).ok_or(PMERR_INVALID_STRING_PARM)
    }

    /// The `CONVCONTEXT` at `p` (none for NULL).
    fn dde_context(&self, p: u32) -> Result<Option<[u8; CONVCONTEXT_SIZE]>, u32> {
        if p == 0 { return Ok(None); }
        let b = self.guest_read_bytes(p, CONVCONTEXT_SIZE).ok_or(PMERR_INVALID_PARM)?;
        Ok(Some(b.try_into().map_err(|_| PMERR_INVALID_PARM)?))
    }

    /// A new `DDEINIT` in giveable shared memory.
    fn alloc_ddeinit(&self, app: &[u8], topic: &[u8], context: Option<&[u8; CONVCONTEXT_SIZE]>)
        -> Result<u32, u32>
    {
        let size = ddeinit_bytes(0, app, topic, context).len() as u32;
        let at = self.shared.mem_mgr.lock_or_recover().alloc(size).ok_or(PMERR_HEAP_OUT_OF_MEMORY)?;
        if self.guest_write_bytes(at, &ddeinit_bytes(at, app, topic, context)).is_none() {
            self.shared.mem_mgr.lock_or_recover().free(at);
            return Err(PMERR_HEAP_OUT_OF_MEMORY);
        }
        self.shared.shmem_mgr.lock_or_recover()
            .register(SharedBlock { addr: at, size, flags: DDEINIT_FLAGS, name: None });
        Ok(at)
    }

    /// Steps that send `msg` to `hwnd` (after its queue's send hooks).
    fn dde_send_steps(&self, tid: u32, hwnd: u32, msg: u32, mp1: u32, mp2: u32) -> Vec<CallStep> {
        let (pfn, hmq, caller) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let w = wm.get_window(hwnd);
            (w.map_or(0, |w| w.pfn_wp), w.map_or(0, |w| w.hmq), wm.thread_queue(tid).unwrap_or(0))
        };
        let mut steps = Vec::new();
        if hmq != 0 {
            steps.extend(self.send_hook_steps(tid, hmq, [hwnd, msg, mp1, mp2], hmq != caller));
        }
        steps.push(CallStep::send(pfn, hwnd, msg, mp1, mp2));
        steps
    }

    /// `WinDdeInitiate(hwndClient, pszAppName, pszTopicName, pcctxt)` →
    /// BOOL: send `WM_DDE_INITIATE` to the frames of the process.
    pub(super) fn win_dde_initiate(&self, tid: u32, client: u32, psz_app: u32, psz_topic: u32,
                                   pctx: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, client, false) { return ApiResult::Normal(0); }
        let r = (|| {
            let app = self.dde_name(psz_app)?;
            let topic = self.dde_name(psz_topic)?;
            let context = self.dde_context(pctx)?;
            let targets: Vec<u32> = {
                let wm = self.shared.window_mgr.lock_or_recover();
                wm.broadcast_targets(HWND_DESKTOP, false, true).into_iter()
                    .map(|f| wm.frame_msg_target(f))
                    .filter(|&h| wm.get_window(h).is_some_and(|w| w.pfn_wp != 0))
                    .collect()
            };
            let mut steps = Vec::new();
            for h in targets {
                let init = self.alloc_ddeinit(&app, &topic, context.as_ref())?;
                steps.extend(self.dde_send_steps(tid, h, WM_DDE_INITIATE, client, init));
            }
            Ok(steps)
        })();
        match r {
            Ok(steps) if steps.is_empty() => ApiResult::Normal(1),
            Ok(steps) => self.start_chain(CallChain::new(tid, steps, false, ChainThen::Return(1))),
            Err(code) => self.pm_fail(tid, code),
        }
    }

    /// `WinDdeRespond(hwndClient, hwndServer, pszAppName, pszTopicName,
    /// pcctxt)` → MRESULT: send `WM_DDE_INITIATEACK` to the client and
    /// return its result.
    pub(super) fn win_dde_respond(&self, tid: u32, client: u32, server: u32, psz_app: u32,
                                  psz_topic: u32, pctx: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, client, false) || !self.pm_check_hwnd(tid, server, false) {
            return ApiResult::Normal(0);
        }
        let r = (|| {
            let app = self.dde_name(psz_app)?;
            let topic = self.dde_name(psz_topic)?;
            let context = self.dde_context(pctx)?;
            self.alloc_ddeinit(&app, &topic, context.as_ref())
        })();
        match r {
            Ok(init) => {
                let steps = self.dde_send_steps(tid, client, WM_DDE_INITIATEACK, server, init);
                self.start_chain(CallChain::new(tid, steps, false, ChainThen::ReturnLast))
            }
            Err(code) => self.pm_fail(tid, code),
        }
    }

    /// `WinDdePostMsg(hwndTo, hwndFrom, wm, pddest, flOptions)` → BOOL:
    /// post a DDE message.
    pub(super) fn win_dde_post_msg(&self, tid: u32, to: u32, from: u32, msg: u32, pddes: u32,
                                   fl: u32) -> ApiResult {
        if !self.pm_check_hwnd(tid, to, false) || !self.pm_check_hwnd(tid, from, true) {
            return ApiResult::Normal(0);
        }
        if fl & !(DDEPM_RETRY | DDEPM_NOFREE) != 0 {
            return self.pm_fail(tid, PMERR_INVALID_FLAG);
        }
        if !(WM_DDE_FIRST..=WM_DDE_LAST).contains(&msg) {
            return self.pm_fail(tid, PMERR_PARAMETER_OUT_OF_RANGE);
        }
        // The receiver reads the DDESTRUCT: its header must be there.
        if pddes != 0 && self.guest_read_bytes(pddes, DDESTRUCT_SIZE).is_none() {
            return self.pm_fail(tid, PMERR_INVALID_PARM);
        }
        let wm = self.shared.window_mgr.lock_or_recover();
        let Some(mq) = wm.find_hmq_for_hwnd(to).and_then(|h| wm.get_mq(h)) else {
            drop(wm);
            return self.pm_fail(tid, PMERR_INVALID_HWND);
        };
        drop(wm);
        let mut q = mq.lock_or_recover();
        q.messages.push_back(super::pm_types::OS2Message { hwnd: to, msg, mp1: from, mp2: pddes,
                                                           time: 0, x: 0, y: 0 });
        q.cond.notify_one();
        ApiResult::Normal(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;

    const TID: u32 = 0;
    const PROC: u32 = 0x5000;
    const APP: u32 = 0x0010_0000;
    const TOPIC: u32 = 0x0010_0100;
    const CTX: u32 = 0x0010_0200;
    const DDES: u32 = 0x0010_0300;

    fn setup() -> (Loader, u32, u32) {
        let l = Loader::new_mock();
        let (hmq, client) = {
            let mut wm = l.shared.window_mgr.lock_or_recover();
            let hmq = wm.create_mq(437);
            wm.tid_to_hmq.insert(TID, hmq);
            wm.register_class("App".into(), PROC, 0);
            let frame = wm.create_window("App".into(), HWND_DESKTOP, hmq);
            let client = wm.create_window("App".into(), frame, hmq);
            wm.frame_to_client.insert(frame, client);
            (hmq, client)
        };
        (l, hmq, client)
    }

    fn u32_at(b: &[u8], at: usize) -> u32 { u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) }

    #[test]
    fn ddeinit_layout() {
        let b = ddeinit_bytes(0x1000, b"App", b"Topic", None);
        assert_eq!(u32_at(&b, 0), DDEINIT_SIZE as u32);
        assert_eq!(u32_at(&b, 4), 0x1000 + DDEINIT_SIZE as u32);
        assert_eq!(u32_at(&b, 8), 0x1000 + DDEINIT_SIZE as u32 + 4);
        assert_eq!(u32_at(&b, 12), 0);
        assert_eq!(&b[DDEINIT_SIZE..], b"App\0Topic\0");
        let ctx = [7u8; CONVCONTEXT_SIZE];
        let b = ddeinit_bytes(0x1000, b"", b"T", Some(&ctx));
        assert_eq!(u32_at(&b, 12), DDEINIT_SIZE as u32);
        assert_eq!(&b[DDEINIT_SIZE..DDEINIT_SIZE + CONVCONTEXT_SIZE], &ctx);
        assert_eq!(u32_at(&b, 4), 0x1000 + (DDEINIT_SIZE + CONVCONTEXT_SIZE) as u32);
        assert_eq!(&b[DDEINIT_SIZE + CONVCONTEXT_SIZE..], b"\0T\0");
    }

    #[test]
    fn initiate_sends_to_each_frame_client_with_its_own_ddeinit() {
        let (l, _, client) = setup();
        l.guest_write_bytes(APP, b"Srv\0").unwrap();
        l.guest_write_bytes(TOPIC, b"Sys\0").unwrap();
        l.guest_write_bytes(CTX, &[1u8; CONVCONTEXT_SIZE]).unwrap();
        let chain = match l.win_dde_initiate(TID, client, APP, TOPIC, CTX) {
            ApiResult::CallChain(c) => c,
            other => panic!("a call chain, got {other:?}"),
        };
        assert_eq!(chain.steps.len(), 1);
        let CallStep::Guest { proc_addr, args, .. } = &chain.steps[0] else { panic!("a guest call") };
        assert_eq!(*proc_addr, PROC);
        assert_eq!(&args[..3], &[client, WM_DDE_INITIATE, client]);
        let init = args[3];
        let b = l.guest_read_bytes(init, 64).unwrap();
        assert_eq!(u32_at(&b, 12), DDEINIT_SIZE as u32);
        assert_eq!(l.read_guest_string(u32_at(&b, 4)), "Srv");
        assert_eq!(l.read_guest_string(u32_at(&b, 8)), "Sys");
        // Giveable shared memory: DosFreeMem frees it.
        assert_eq!(l.dos_free_mem(init), NO_ERROR);
    }

    #[test]
    fn initiate_and_respond_check_their_windows() {
        let (l, _, client) = setup();
        assert!(matches!(l.win_dde_initiate(TID, 0x7777, 0, 0, 0), ApiResult::Normal(0)));
        assert_eq!(l.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_HWND);
        assert!(matches!(l.win_dde_respond(TID, client, 0x7777, 0, 0, 0), ApiResult::Normal(0)));
        let chain = match l.win_dde_respond(TID, client, client, 0, 0, 0) {
            ApiResult::CallChain(c) => c,
            other => panic!("a call chain, got {other:?}"),
        };
        assert_eq!(chain.then, ChainThen::ReturnLast);
        let CallStep::Guest { args, .. } = &chain.steps[0] else { panic!("a guest call") };
        assert_eq!(&args[..3], &[client, WM_DDE_INITIATEACK, client]);
    }

    #[test]
    fn post_msg_queues_the_message_and_checks_it() {
        let (l, hmq, client) = setup();
        assert!(matches!(l.win_dde_post_msg(TID, client, client, WM_DDE_DATA, DDES, DDEPM_RETRY),
                         ApiResult::Normal(1)));
        let mq = l.shared.window_mgr.lock_or_recover().get_mq(hmq).unwrap();
        let m = mq.lock_or_recover().messages.pop_back().unwrap();
        assert_eq!((m.hwnd, m.msg, m.mp1, m.mp2), (client, WM_DDE_DATA, client, DDES));
        assert!(matches!(l.win_dde_post_msg(TID, client, client, WM_USER, DDES, 0), ApiResult::Normal(0)));
        assert_eq!(l.shared.pm_errors.take(TID) & 0xFFFF, PMERR_PARAMETER_OUT_OF_RANGE);
        assert!(matches!(l.win_dde_post_msg(TID, client, client, WM_DDE_ACK, DDES, 4), ApiResult::Normal(0)));
        assert_eq!(l.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_FLAG);
        let end = l.shared.guest_mem.size() as u32 - 4;
        assert!(matches!(l.win_dde_post_msg(TID, client, client, WM_DDE_ACK, end, 0), ApiResult::Normal(0)));
        assert_eq!(l.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_PARM);
        assert!(matches!(l.win_dde_post_msg(TID, 0x7777, client, WM_DDE_ACK, 0, 0), ApiResult::Normal(0)));
        assert_eq!(l.shared.pm_errors.take(TID) & 0xFFFF, PMERR_INVALID_HWND);
    }
}
