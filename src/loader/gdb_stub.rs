// SPDX-License-Identifier: GPL-3.0-only
//
// GDB Remote Serial Protocol stub for Warpine.
//
// Thread model:
//   • vCPU thread  — runs the guest; pauses when GDB requests a stop.
//   • GDB thread   — accepts one TCP connection, drives the gdbstub RSP loop.
//
// Synchronisation via GdbState:
//   vCPU pauses  → notify stopped/stop_cond  → GDB reads regs/memory
//   GDB 'c'/'si' → send resume_cmd/resume_cond → vCPU wakes and runs
//
// Guest memory is only accessed from the GDB thread when the vCPU is paused
// (gdbstub only calls read_addrs/write_addrs in the Idle state, before the
// next resume command), so no additional locking is needed for those reads.

use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gdbstub::common::Signal;
use gdbstub::conn::ConnectionExt;
use gdbstub::stub::run_blocking::{BlockingEventLoop, Event, WaitForStopReasonError};
use gdbstub::stub::{DisconnectReason, GdbStub, SingleThreadStopReason};
use gdbstub::target::ext::base::singlethread::{
    SingleThreadBase, SingleThreadResume, SingleThreadResumeOps,
    SingleThreadSingleStep, SingleThreadSingleStepOps,
};
use gdbstub::target::ext::breakpoints::{
    Breakpoints, BreakpointsOps, SwBreakpoint, SwBreakpointOps,
};
use gdbstub::target::{Target, TargetError, TargetResult};
use gdbstub_arch::x86::X86_SSE;
use gdbstub_arch::x86::reg::{X86CoreRegs, X86SegmentRegs};
use log::{info, warn};

use super::vm_backend::{GuestRegs, GuestSregs};

// ── Stop reason ───────────────────────────────────────────────────────────────

/// Why the vCPU stopped — set by the vCPU thread, consumed by the GDB thread.
pub enum GdbVcpuStopReason {
    /// Initial pause at entry point (before the first instruction executes).
    Initial,
    /// Hit a GDB software breakpoint (INT 3 written into guest memory).
    SwBreakpoint,
    /// Hit a hardware execute breakpoint (DR0-DR3).
    HwBreakpoint,
    /// Single-step completed.
    SingleStep,
    /// Stop requested by the GDB client (Ctrl-C / interrupt packet).
    Interrupt,
    /// Guest raised a CPU exception (vector number).
    Exception(u8),
}

/// Complete vCPU state snapshot taken when the CPU stops.
pub struct GdbStopInfo {
    pub reason: GdbVcpuStopReason,
    pub regs:   GuestRegs,
    pub sregs:  GuestSregs,
}

impl GdbStopInfo {
    /// Map the stop reason to the gdbstub `SingleThreadStopReason` wire type.
    fn to_stop_reason(&self) -> SingleThreadStopReason<u32> {
        match self.reason {
            GdbVcpuStopReason::Initial      => SingleThreadStopReason::Signal(Signal::SIGTRAP),
            GdbVcpuStopReason::SwBreakpoint => SingleThreadStopReason::SwBreak(()),
            GdbVcpuStopReason::HwBreakpoint => SingleThreadStopReason::HwBreak(()),
            GdbVcpuStopReason::SingleStep   => SingleThreadStopReason::DoneStep,
            GdbVcpuStopReason::Interrupt    => SingleThreadStopReason::Signal(Signal::SIGINT),
            GdbVcpuStopReason::Exception(v) => {
                let sig = match v {
                    0 | 4 => Signal::SIGFPE,   // #DE, #OF
                    5     => Signal::SIGSEGV,   // #BR
                    6     => Signal::SIGILL,    // #UD invalid opcode
                    11 | 12 => Signal::SIGBUS,  // #NP, #SS
                    13 | 14 => Signal::SIGSEGV, // #GP, #PF
                    _     => Signal::SIGTRAP,
                };
                SingleThreadStopReason::Signal(sig)
            }
        }
    }
}

// ── Resume command ────────────────────────────────────────────────────────────

/// Command sent by the GDB thread to the vCPU thread.
pub enum GdbResumeCmd {
    Continue,
    Step,
    Kill,
}

// ── Shared state ──────────────────────────────────────────────────────────────

/// Synchronisation channel between the vCPU thread and the GDB stub thread.
pub struct GdbState {
    /// Set by the GDB thread to request a vCPU pause at the next safe point.
    pub stop_requested: AtomicBool,

    /// Written by the vCPU when it stops; the GDB thread reads it.
    pub stopped:   Mutex<Option<GdbStopInfo>>,
    pub stop_cond: Condvar,

    /// Written by the GDB thread to command a resume; the vCPU reads it.
    pub resume_cmd:  Mutex<Option<GdbResumeCmd>>,
    pub resume_cond: Condvar,

    /// Software breakpoints: guest flat address → original byte saved before
    /// the INT 3 (0xCC) was patched in.
    pub sw_breakpoints: Mutex<HashMap<u32, u8>>,

    /// Hardware execute breakpoints (DR0-DR3).  `None` = slot unused.
    pub hw_breakpoints: Mutex<[Option<u32>; 4]>,
}

impl Default for GdbState {
    fn default() -> Self { Self::new() }
}

impl GdbState {
    pub fn new() -> Self {
        GdbState {
            stop_requested: AtomicBool::new(false),
            stopped:        Mutex::new(None),
            stop_cond:      Condvar::new(),
            resume_cmd:     Mutex::new(None),
            resume_cond:    Condvar::new(),
            sw_breakpoints: Mutex::new(HashMap::new()),
            hw_breakpoints: Mutex::new([None; 4]),
        }
    }

    /// Called by the vCPU thread when it pauses.  Stores the stop info and
    /// wakes any thread waiting on `stop_cond`.
    pub fn notify_stopped(&self, info: GdbStopInfo) {
        *self.stopped.lock().unwrap() = Some(info);
        self.stop_cond.notify_all();
    }

    /// Called by the vCPU thread — blocks until the GDB thread sends a resume
    /// command, then returns it.
    pub fn wait_for_resume(&self) -> GdbResumeCmd {
        let mut cmd = self.resume_cmd.lock().unwrap();
        loop {
            if let Some(c) = cmd.take() {
                return c;
            }
            cmd = self.resume_cond.wait(cmd).unwrap();
        }
    }

    /// Called by the GDB thread — posts a resume command and wakes the vCPU.
    pub fn send_resume(&self, cmd: GdbResumeCmd) {
        *self.resume_cmd.lock().unwrap() = Some(cmd);
        self.resume_cond.notify_all();
    }
}

// ── GDB Target ────────────────────────────────────────────────────────────────

/// Implements `gdbstub::target::Target` for the Warpine KVM guest.
///
/// Owned exclusively by the GDB stub thread.  Guest memory is accessed only
/// while the vCPU is paused (gdbstub guarantees this through the
/// Idle/Running state machine).
pub struct WarpineTarget {
    gdb_state: Arc<GdbState>,
    /// Reference to the whole shared state for guest memory access.
    shared:    Arc<super::SharedState>,
    /// Last register snapshot received from the vCPU.
    cached_regs:  GuestRegs,
    cached_sregs: GuestSregs,
}

impl WarpineTarget {
    pub fn new(gdb_state: Arc<GdbState>, shared: Arc<super::SharedState>) -> Self {
        WarpineTarget {
            gdb_state,
            shared,
            cached_regs:  GuestRegs::default(),
            cached_sregs: GuestSregs::default(),
        }
    }

    fn update_cache(&mut self, info: &GdbStopInfo) {
        self.cached_regs  = info.regs.clone();
        self.cached_sregs = info.sregs.clone();
    }
}

// ── Target trait ─────────────────────────────────────────────────────────────

impl Target for WarpineTarget {
    type Arch  = X86_SSE;
    type Error = &'static str;

    fn base_ops(&mut self) -> gdbstub::target::ext::base::BaseOps<'_, Self::Arch, Self::Error> {
        gdbstub::target::ext::base::BaseOps::SingleThread(self)
    }

    fn support_breakpoints(&mut self) -> Option<BreakpointsOps<'_, Self>> {
        Some(self)
    }
}

// ── SingleThreadBase ──────────────────────────────────────────────────────────

impl SingleThreadBase for WarpineTarget {
    /// Populate `regs` from the cached register snapshot.
    fn read_registers(&mut self, regs: &mut X86CoreRegs) -> TargetResult<(), Self> {
        let r = &self.cached_regs;
        let s = &self.cached_sregs;
        regs.eax    = r.rax as u32;
        regs.ecx    = r.rcx as u32;
        regs.edx    = r.rdx as u32;
        regs.ebx    = r.rbx as u32;
        regs.esp    = r.rsp as u32;
        regs.ebp    = r.rbp as u32;
        regs.esi    = r.rsi as u32;
        regs.edi    = r.rdi as u32;
        regs.eip    = r.rip as u32;
        regs.eflags = r.rflags as u32;
        regs.segments = X86SegmentRegs {
            cs: s.cs.selector as u32,
            ss: s.ss.selector as u32,
            ds: s.ds.selector as u32,
            es: s.es.selector as u32,
            fs: s.fs.selector as u32,
            gs: s.gs.selector as u32,
        };
        Ok(())
    }

    /// Update the cached register state (applied on next resume).
    fn write_registers(&mut self, regs: &X86CoreRegs) -> TargetResult<(), Self> {
        self.cached_regs.rax    = regs.eax as u64;
        self.cached_regs.rcx    = regs.ecx as u64;
        self.cached_regs.rdx    = regs.edx as u64;
        self.cached_regs.rbx    = regs.ebx as u64;
        self.cached_regs.rsp    = regs.esp as u64;
        self.cached_regs.rbp    = regs.ebp as u64;
        self.cached_regs.rsi    = regs.esi as u64;
        self.cached_regs.rdi    = regs.edi as u64;
        self.cached_regs.rip    = regs.eip as u64;
        self.cached_regs.rflags = regs.eflags as u64;
        Ok(())
    }

    /// Read guest physical memory.  Returns the number of bytes read (may be
    /// less than `data.len()` if the range extends past the end of guest RAM).
    fn read_addrs(&mut self, start_addr: u32, data: &mut [u8]) -> TargetResult<usize, Self> {
        let mut n = 0usize;
        for (i, byte) in data.iter_mut().enumerate() {
            match self.shared.guest_mem.read::<u8>(start_addr.wrapping_add(i as u32)) {
                Some(b) => { *byte = b; n += 1; }
                None    => break,
            }
        }
        Ok(n)
    }

    /// Write to guest physical memory.
    fn write_addrs(&mut self, start_addr: u32, data: &[u8]) -> TargetResult<(), Self> {
        for (i, &byte) in data.iter().enumerate() {
            if self.shared.guest_mem.write::<u8>(
                start_addr.wrapping_add(i as u32), byte,
            ).is_none() {
                return Err(TargetError::NonFatal);
            }
        }
        Ok(())
    }

    fn support_resume(&mut self) -> Option<SingleThreadResumeOps<'_, Self>> {
        Some(self)
    }
}

// ── SingleThreadResume ────────────────────────────────────────────────────────

impl SingleThreadResume for WarpineTarget {
    /// Signal the vCPU to continue running (gdbstub calls this for 'c').
    fn resume(&mut self, _signal: Option<Signal>) -> Result<(), Self::Error> {
        self.gdb_state.send_resume(GdbResumeCmd::Continue);
        Ok(())
    }

    fn support_single_step(&mut self) -> Option<SingleThreadSingleStepOps<'_, Self>> {
        Some(self)
    }
}

// ── SingleThreadSingleStep ────────────────────────────────────────────────────

impl SingleThreadSingleStep for WarpineTarget {
    /// Signal the vCPU to execute exactly one instruction (gdbstub calls this
    /// for 'si').
    fn step(&mut self, _signal: Option<Signal>) -> Result<(), Self::Error> {
        self.gdb_state.send_resume(GdbResumeCmd::Step);
        Ok(())
    }
}

// ── Breakpoints ───────────────────────────────────────────────────────────────

impl Breakpoints for WarpineTarget {
    fn support_sw_breakpoint(&mut self) -> Option<SwBreakpointOps<'_, Self>> {
        Some(self)
    }
}

impl SwBreakpoint for WarpineTarget {
    fn add_sw_breakpoint(&mut self, addr: u32, _kind: usize) -> TargetResult<bool, Self> {
        let mut bps = self.gdb_state.sw_breakpoints.lock().unwrap();
        if bps.contains_key(&addr) {
            return Ok(true); // already installed
        }
        let orig = match self.shared.guest_mem.read::<u8>(addr) {
            Some(b) => b,
            None    => return Ok(false),
        };
        if self.shared.guest_mem.write::<u8>(addr, 0xCC).is_none() {
            return Ok(false);
        }
        bps.insert(addr, orig);
        Ok(true)
    }

    fn remove_sw_breakpoint(&mut self, addr: u32, _kind: usize) -> TargetResult<bool, Self> {
        let mut bps = self.gdb_state.sw_breakpoints.lock().unwrap();
        if let Some(orig) = bps.remove(&addr) {
            self.shared.guest_mem.write::<u8>(addr, orig);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

// ── Blocking event loop ───────────────────────────────────────────────────────

/// Drives the gdbstub `run_blocking` event loop.
///
/// After `resume()`/`step()` is called on the target, gdbstub transitions to
/// the `Running` state and calls `wait_for_stop_reason`.  We poll the vCPU
/// stop condvar with a short timeout, checking the TCP connection for incoming
/// bytes (e.g. Ctrl-C) between each poll.
pub struct GdbBlockingEventLoop;

impl BlockingEventLoop for GdbBlockingEventLoop {
    type Target     = WarpineTarget;
    type Connection = TcpStream;
    type StopReason = SingleThreadStopReason<u32>;

    fn wait_for_stop_reason(
        target: &mut WarpineTarget,
        conn:   &mut TcpStream,
    ) -> Result<
        Event<SingleThreadStopReason<u32>>,
        WaitForStopReasonError<
            <WarpineTarget as Target>::Error,
            <TcpStream as gdbstub::conn::Connection>::Error,
        >,
    > {
        loop {
            // Wait up to 10 ms for the vCPU to report a stop.
            {
                let (mut stopped, _) = target
                    .gdb_state
                    .stop_cond
                    .wait_timeout(
                        target.gdb_state.stopped.lock().unwrap(),
                        Duration::from_millis(10),
                    )
                    .unwrap();
                if let Some(info) = stopped.take() {
                    drop(stopped); // release the lock before &mut self call
                    let reason = info.to_stop_reason();
                    target.update_cache(&info);
                    return Ok(Event::TargetStopped(reason));
                }
            }

            // Check for incoming GDB bytes (Ctrl-C = 0x03) without blocking.
            match conn.peek() {
                Ok(Some(_)) => {
                    let b = conn.read()
                        .map_err(WaitForStopReasonError::Connection)?;
                    return Ok(Event::IncomingData(b));
                }
                Ok(None) => {} // no data pending
                Err(e)   => return Err(WaitForStopReasonError::Connection(e)),
            }
        }
    }

    /// Called when the GDB client sends a Ctrl-C interrupt packet.
    /// Requests the vCPU to stop and waits up to 2 s for it.
    fn on_interrupt(
        target: &mut WarpineTarget,
    ) -> Result<Option<SingleThreadStopReason<u32>>, <WarpineTarget as Target>::Error> {
        target.gdb_state.stop_requested.store(true, Ordering::Relaxed);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            let (mut stopped, _) = target
                .gdb_state
                .stop_cond
                .wait_timeout(target.gdb_state.stopped.lock().unwrap(), remaining)
                .unwrap();
            if let Some(info) = stopped.take() {
                drop(stopped); // release the lock before &mut self call
                target.update_cache(&info);
                return Ok(Some(SingleThreadStopReason::Signal(Signal::SIGINT)));
            }
        }
        warn!("GDB: interrupt timed out — vCPU did not stop within 2 s");
        Ok(None)
    }
}

// ── TCP listener ──────────────────────────────────────────────────────────────

/// Spawn a background thread that:
///   1. Listens for one incoming GDB TCP connection on `127.0.0.1:<port>`.
///   2. Runs the full GDB RSP session via `GdbStub::run_blocking`.
///
/// The vCPU must already be paused (waiting on `GdbState::resume_cond`) when
/// this is called, so the first GDB `c` command starts a clean execution.
pub fn launch_gdb_stub(shared: Arc<super::SharedState>, port: u16) {
    std::thread::spawn(move || {
        let gdb_state = match shared.gdb_state.as_ref() {
            Some(g) => g.clone(),
            None    => { warn!("GDB: gdb_state not set in SharedState"); return; }
        };

        let addr = format!("127.0.0.1:{}", port);
        let listener = match TcpListener::bind(&addr) {
            Ok(l)  => { info!("GDB: listening on {} — waiting for client", addr); l }
            Err(e) => { warn!("GDB: bind failed on {}: {}", addr, e); return; }
        };

        let (stream, peer) = match listener.accept() {
            Ok(p)  => p,
            Err(e) => { warn!("GDB: accept error: {}", e); return; }
        };
        info!("GDB: client connected from {}", peer);
        let _ = stream.set_nodelay(true);

        let mut target = WarpineTarget::new(gdb_state, shared);
        let gdb = GdbStub::new(stream);

        match gdb.run_blocking::<GdbBlockingEventLoop>(&mut target) {
            Ok(DisconnectReason::Disconnect) => info!("GDB: client disconnected"),
            Ok(DisconnectReason::Kill)       => {
                info!("GDB: session killed");
                target.gdb_state.send_resume(GdbResumeCmd::Kill);
            }
            Ok(reason) => info!("GDB: session ended ({:?})", reason),
            Err(e)     => warn!("GDB: session error: {}", e),
        }
    });
}
