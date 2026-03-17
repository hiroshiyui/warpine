// SPDX-License-Identifier: GPL-3.0-only
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
    pub gdt_base:  u64,
    pub gdt_limit: u32,
    pub idt_base:  u64,
    pub idt_limit: u32,
    pub cr0: u64,
    pub cr2: u64,
    pub cr4: u64,
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
