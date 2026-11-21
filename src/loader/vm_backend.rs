// SPDX-License-Identifier: GPL-3.0-or-later
//
// Virtualization backend abstraction.
//
// Defines the `VmBackend` and `VcpuBackend` traits along with the portable
// register/segment types they operate on.  All KVM-specific code lives in
// `kvm_backend.rs`; this module is hypervisor-agnostic.

// ── Exit reason ─────────────────────────────────────────────────────────────

/// Normalized VM-exit reason returned by [`VcpuBackend::run`].
///
/// Covers only the cases that `run_vcpu` actually handles.  Unrecognised
/// exits are reported as [`VmExit::Other`] so the loop can log and terminate
/// without knowing every possible hypervisor-specific exit code.
#[derive(Debug)]
pub enum VmExit {
    /// Software breakpoint (INT 3) or hardware debug event.
    Debug,
    /// Guest executed HLT.
    Hlt,
    /// Guest read from an unmapped MMIO address.
    MmioRead { addr: u64, size: usize },
    /// Guest wrote to an unmapped MMIO address.
    MmioWrite { addr: u64 },
    /// Guest triple-faulted (shutdown condition).
    Shutdown,
    /// [`VcpuKick::kick`] made the vCPU leave the guest (KVM: `KVM_RUN`
    /// returned `EINTR`). Guest state is untouched: the loop looks at its
    /// flags and enters the guest again where it left off.
    Interrupted,
    /// Any other exit reason not handled above.
    Other(String),
}

// ── General-purpose registers ────────────────────────────────────────────────

/// Guest general-purpose register state (32-bit guest; upper 32 bits of each
/// field are zero for a 32-bit OS/2 guest but stored as `u64` for KVM ABI
/// compatibility).
#[derive(Clone, Default, Debug)]
pub struct GuestRegs {
    pub rip:    u64,
    pub rsp:    u64,
    pub rax:    u64,
    pub rbx:    u64,
    pub rcx:    u64,
    pub rdx:    u64,
    pub rsi:    u64,
    pub rdi:    u64,
    pub rbp:    u64,
    pub rflags: u64,
}

// ── Segment descriptor ───────────────────────────────────────────────────────

/// A single x86 segment descriptor in decoded form.
///
/// Field names and semantics mirror `kvm_segment` from `kvm_bindings` so that
/// the KVM backend can map between the two with trivial field copies.
#[derive(Clone, Default, Debug)]
pub struct GuestSegment {
    pub base:     u64,
    pub limit:    u32,
    pub selector: u16,
    pub type_:    u8,
    pub present:  u8,
    pub dpl:      u8,
    pub db:       u8,  // 0 = 16-bit, 1 = 32-bit default operand/stack size
    pub s:        u8,
    pub l:        u8,
    pub g:        u8,
    pub avl:      u8,
    pub unusable: u8,
}

// ── Special registers ────────────────────────────────────────────────────────

/// Guest special-register state: segment registers, descriptor table bases,
/// and the control registers that the loader configures.
#[derive(Clone, Default, Debug)]
pub struct GuestSregs {
    pub cs: GuestSegment,
    pub ds: GuestSegment,
    pub es: GuestSegment,
    pub fs: GuestSegment,
    pub gs: GuestSegment,
    pub ss: GuestSegment,
    /// The task register: the thread's TSS (ADR-0113).
    pub tr: GuestSegment,
    pub gdt_base:  u64,
    pub gdt_limit: u32,
    pub idt_base:  u64,
    pub idt_limit: u32,
    pub cr0: u64,
    pub cr2: u64,
    /// The page directory's guest-physical address when CR0.PG is set
    /// (`paging.rs`).
    pub cr3: u64,
    pub cr4: u64,
}

// ── vCPU kick ────────────────────────────────────────────────────────────────

/// Makes a vCPU that runs guest code leave it, from any host thread.
///
/// A kick is never lost: a vCPU in the guest exits with
/// [`VmExit::Interrupted`] at once; one that is outside the guest returns
/// `Interrupted` from its next [`VcpuBackend::run`]. Several kicks before
/// that exit count as one. Whoever kicks first sets the flag the vCPU
/// loop is to look at (kill, critical section, suspension, process end).
pub trait VcpuKick: Send + Sync {
    fn kick(&self);
}

// ── vCPU trait ───────────────────────────────────────────────────────────────

/// Abstraction over a single virtual CPU.
///
/// Implementors must be `Send` so that `Box<dyn VcpuBackend>` can be moved
/// into a spawned OS/2 thread.
pub trait VcpuBackend: Send {
    /// Execute the guest until the next VM exit and return the exit reason.
    fn run(&mut self) -> Result<VmExit, String>;

    /// Read the current general-purpose register state.
    fn get_regs(&self) -> Result<GuestRegs, String>;

    /// Write the general-purpose register state.
    fn set_regs(&mut self, regs: &GuestRegs) -> Result<(), String>;

    /// Read the current special-register state.
    fn get_sregs(&self) -> Result<GuestSregs, String>;

    /// Write the special-register state.
    fn set_sregs(&mut self, sregs: &GuestSregs) -> Result<(), String>;

    /// Enable guest-mode software-breakpoint (INT 3) interception.
    ///
    /// Must be called once after vCPU creation and before the first `run()`.
    fn enable_software_breakpoints(&mut self) -> Result<(), String>;

    /// Enable or disable single-step execution (TF flag via `KVM_GUESTDBG_SINGLESTEP`).
    ///
    /// When `enabled` is `true` the vCPU will exit after every instruction with
    /// `VmExit::Debug`.  Call with `false` to return to normal execution.
    fn set_single_step(&mut self, enabled: bool) -> Result<(), String>;

    /// Drop an exception, interrupt or NMI the vCPU was about to deliver,
    /// before it is set to run elsewhere after a fault (an exit-list
    /// routine): a fault left half-delivered would hit the new code.
    fn clear_pending_events(&mut self) -> Result<(), String>;

    /// A handle that makes this vCPU leave the guest (see [`VcpuKick`]),
    /// valid for as long as the vCPU exists.
    fn kicker(&self) -> std::sync::Arc<dyn VcpuKick>;
}

// ── VM trait ─────────────────────────────────────────────────────────────────

/// Abstraction over a virtual machine instance.
///
/// A `VmBackend` is a factory for [`VcpuBackend`] instances and owns the
/// registration of the guest physical address space with the hypervisor.
///
/// `Send + Sync` is required so that `Arc<dyn VmBackend>` can be shared
/// between the main thread, vCPU threads, and `dos_create_thread`.
pub trait VmBackend: Send + Sync {
    /// Register a contiguous guest physical memory region with the hypervisor.
    ///
    /// `guest_phys` — guest physical base address (typically 0).
    /// `size`       — region size in bytes.
    /// `host_ptr`   — host virtual address of the backing allocation.
    ///
    /// Called exactly once during loader initialisation after `mmap`.
    fn register_guest_memory(&self, guest_phys: u64, size: u64, host_ptr: u64) -> Result<(), String>;

    /// Create a new virtual CPU with the given numeric ID.
    ///
    /// `id` must be unique within the VM for the lifetime of the vCPU.
    fn create_vcpu(&self, id: u64) -> Result<Box<dyn VcpuBackend>, String>;
}

// ── Mock backend (test-only) ─────────────────────────────────────────────────

/// Test doubles for [`VmBackend`] / [`VcpuBackend`] that require no `/dev/kvm`.
///
/// Import with `use loader::vm_backend::mock::{MockVcpu, MockVmBackend};`
/// inside `#[cfg(test)]` blocks.
#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Kick handle of a [`MockVcpu`]: counts kicks, and the next `run()`
    /// after a kick returns [`VmExit::Interrupted`] like KVM's.
    #[derive(Default)]
    pub struct MockKick {
        pub kicks: AtomicUsize,
        pending: AtomicBool,
    }

    impl VcpuKick for MockKick {
        fn kick(&self) {
            self.kicks.fetch_add(1, Ordering::SeqCst);
            self.pending.store(true, Ordering::SeqCst);
        }
    }

    /// Scriptable vCPU for unit-testing API handlers.
    ///
    /// Register state is directly accessible (`pub regs`, `pub sregs`) so
    /// tests can set up ESP / EIP before invoking a handler.  VM exits are
    /// queued with [`push_exit`]; when the queue is empty `run()` returns
    /// `VmExit::Hlt`.
    pub struct MockVcpu {
        pub regs:  GuestRegs,
        pub sregs: GuestSregs,
        exits:     VecDeque<VmExit>,
        pub kick:  Arc<MockKick>,
    }

    impl Default for MockVcpu {
        fn default() -> Self { Self::new() }
    }

    impl MockVcpu {
        pub fn new() -> Self {
            MockVcpu {
                regs:  GuestRegs::default(),
                sregs: GuestSregs::default(),
                exits: VecDeque::new(),
                kick:  Arc::new(MockKick::default()),
            }
        }

        /// Queue a VM exit to be returned on the next `run()` call.
        pub fn push_exit(&mut self, exit: VmExit) {
            self.exits.push_back(exit);
        }
    }

    impl VcpuBackend for MockVcpu {
        fn run(&mut self) -> Result<VmExit, String> {
            if self.kick.pending.swap(false, Ordering::SeqCst) {
                return Ok(VmExit::Interrupted);
            }
            Ok(self.exits.pop_front().unwrap_or(VmExit::Hlt))
        }
        fn get_regs(&self) -> Result<GuestRegs, String>          { Ok(self.regs.clone()) }
        fn set_regs(&mut self, r: &GuestRegs) -> Result<(), String> { self.regs = r.clone(); Ok(()) }
        fn get_sregs(&self) -> Result<GuestSregs, String>         { Ok(self.sregs.clone()) }
        fn set_sregs(&mut self, s: &GuestSregs) -> Result<(), String> { self.sregs = s.clone(); Ok(()) }
        fn enable_software_breakpoints(&mut self) -> Result<(), String> { Ok(()) }
        fn set_single_step(&mut self, _enabled: bool) -> Result<(), String> { Ok(()) }
        fn clear_pending_events(&mut self) -> Result<(), String> { Ok(()) }
        fn kicker(&self) -> Arc<dyn VcpuKick> { self.kick.clone() }
    }

    /// No-op VM backend for unit tests — no KVM file descriptors opened.
    pub struct MockVmBackend;

    impl VmBackend for MockVmBackend {
        /// No-op: mock tests use `GuestMemory` directly; no hypervisor registration needed.
        fn register_guest_memory(&self, _: u64, _: u64, _: u64) -> Result<(), String> { Ok(()) }
        fn create_vcpu(&self, _id: u64) -> Result<Box<dyn VcpuBackend>, String> {
            Ok(Box::new(MockVcpu::new()))
        }
    }

    #[test]
    fn a_kick_interrupts_the_next_run_once() {
        let mut vcpu = MockVcpu::new();
        vcpu.push_exit(VmExit::Debug);
        let kicker = vcpu.kicker();
        kicker.kick();
        kicker.kick();
        assert!(matches!(vcpu.run(), Ok(VmExit::Interrupted)));
        assert!(matches!(vcpu.run(), Ok(VmExit::Debug)), "kicks before the exit count once");
        assert_eq!(vcpu.kick.kicks.load(Ordering::SeqCst), 2);
    }
}
