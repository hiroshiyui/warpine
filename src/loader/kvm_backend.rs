// SPDX-License-Identifier: GPL-3.0-only
//
// KVM implementation of VmBackend / VcpuBackend.
//
// This is the ONLY file in the codebase that imports `kvm_ioctls` or
// `kvm_bindings`.  All other modules interact with KVM exclusively through
// the `VmBackend` / `VcpuBackend` traits defined in `vm_backend.rs`.

use kvm_ioctls::{Kvm, VmFd, VcpuFd};
use kvm_bindings::{
    kvm_userspace_memory_region, kvm_segment,
    kvm_guest_debug, KVM_GUESTDBG_ENABLE, KVM_GUESTDBG_USE_SW_BP,
};
use std::sync::Arc;
use super::vm_backend::{GuestRegs, GuestSegment, GuestSregs, VcpuBackend, VmBackend, VmExit};

// ── VM backend ───────────────────────────────────────────────────────────────

/// KVM-backed virtual machine.
///
/// Holds the `/dev/kvm` handle (`_kvm`) and the VM file descriptor (`vm`).
/// `_kvm` must outlive `vm`; keeping both in the same struct guarantees
/// the correct drop order.
pub struct KvmVmBackend {
    _kvm: Kvm,
    vm:   Arc<VmFd>,
}

impl KvmVmBackend {
    /// Open `/dev/kvm` and create a new VM.
    pub fn new() -> Self {
        let kvm = Kvm::new().expect("Failed to open /dev/kvm");
        let vm  = Arc::new(kvm.create_vm().expect("Failed to create KVM VM"));
        KvmVmBackend { _kvm: kvm, vm }
    }
}

impl VmBackend for KvmVmBackend {
    fn register_guest_memory(&self, guest_phys: u64, size: u64, host_ptr: u64) -> Result<(), String> {
        let region = kvm_userspace_memory_region {
            slot:             0,
            guest_phys_addr:  guest_phys,
            memory_size:      size,
            userspace_addr:   host_ptr,
            flags:            0,
        };
        unsafe { self.vm.set_user_memory_region(region) }.map_err(|e| e.to_string())
    }

    fn create_vcpu(&self, id: u64) -> Result<Box<dyn VcpuBackend>, String> {
        let fd = self.vm.create_vcpu(id).map_err(|e| e.to_string())?;
        Ok(Box::new(KvmVcpu { fd }))
    }
}

// ── vCPU backend ─────────────────────────────────────────────────────────────

struct KvmVcpu {
    fd: VcpuFd,
}

impl VcpuBackend for KvmVcpu {
    fn run(&mut self) -> Result<VmExit, String> {
        use kvm_ioctls::VcpuExit;
        match self.fd.run().map_err(|e| e.to_string())? {
            VcpuExit::Debug(_) => Ok(VmExit::Debug),
            VcpuExit::Hlt     => Ok(VmExit::Hlt),
            VcpuExit::MmioRead(addr, data) => {
                // Fill the KVM-owned read buffer with zeros before returning;
                // the caller only sees the normalised exit variant.
                for b in data.iter_mut() { *b = 0; }
                Ok(VmExit::MmioRead { addr, size: data.len() })
            }
            VcpuExit::MmioWrite(addr, _data) => Ok(VmExit::MmioWrite { addr }),
            VcpuExit::Shutdown => Ok(VmExit::Shutdown),
            other => Ok(VmExit::Other(format!("{:?}", other))),
        }
    }

    fn get_regs(&self) -> Result<GuestRegs, String> {
        let r = self.fd.get_regs().map_err(|e| e.to_string())?;
        Ok(GuestRegs {
            rip: r.rip, rsp: r.rsp, rax: r.rax, rbx: r.rbx,
            rcx: r.rcx, rdx: r.rdx, rsi: r.rsi, rdi: r.rdi,
            rbp: r.rbp, rflags: r.rflags,
        })
    }

    fn set_regs(&mut self, g: &GuestRegs) -> Result<(), String> {
        // Read first to preserve fields not covered by GuestRegs (r8–r15).
        // For a 32-bit OS/2 guest these are always zero, but being correct
        // costs nothing.
        let mut r = self.fd.get_regs().map_err(|e| e.to_string())?;
        r.rip = g.rip; r.rsp = g.rsp; r.rax = g.rax; r.rbx = g.rbx;
        r.rcx = g.rcx; r.rdx = g.rdx; r.rsi = g.rsi; r.rdi = g.rdi;
        r.rbp = g.rbp; r.rflags = g.rflags;
        self.fd.set_regs(&r).map_err(|e| e.to_string())
    }

    fn get_sregs(&self) -> Result<GuestSregs, String> {
        let s = self.fd.get_sregs().map_err(|e| e.to_string())?;
        Ok(GuestSregs {
            cs: kseg_to_guest(s.cs), ds: kseg_to_guest(s.ds),
            es: kseg_to_guest(s.es), fs: kseg_to_guest(s.fs),
            gs: kseg_to_guest(s.gs), ss: kseg_to_guest(s.ss),
            gdt_base:  s.gdt.base,  gdt_limit: s.gdt.limit as u32,
            idt_base:  s.idt.base,  idt_limit: s.idt.limit as u32,
            cr0: s.cr0, cr2: s.cr2, cr4: s.cr4,
        })
    }

    fn set_sregs(&mut self, g: &GuestSregs) -> Result<(), String> {
        // Read current sregs first so that KVM-internal fields (EFER,
        // interrupt shadow, APIC base, etc.) are preserved.
        let mut s = self.fd.get_sregs().map_err(|e| e.to_string())?;
        s.cs = guest_to_kseg(&g.cs); s.ds = guest_to_kseg(&g.ds);
        s.es = guest_to_kseg(&g.es); s.fs = guest_to_kseg(&g.fs);
        s.gs = guest_to_kseg(&g.gs); s.ss = guest_to_kseg(&g.ss);
        s.gdt.base  = g.gdt_base;  s.gdt.limit = g.gdt_limit as u16;
        s.idt.base  = g.idt_base;  s.idt.limit = g.idt_limit as u16;
        s.cr0 = g.cr0; s.cr4 = g.cr4;
        self.fd.set_sregs(&s).map_err(|e| e.to_string())
    }

    fn enable_software_breakpoints(&mut self) -> Result<(), String> {
        let dbg = kvm_guest_debug {
            control: KVM_GUESTDBG_ENABLE | KVM_GUESTDBG_USE_SW_BP,
            ..Default::default()
        };
        self.fd.set_guest_debug(&dbg).map_err(|e| e.to_string())
    }
}

// ── Segment conversion helpers ───────────────────────────────────────────────

fn kseg_to_guest(k: kvm_segment) -> GuestSegment {
    GuestSegment {
        base: k.base, limit: k.limit, selector: k.selector,
        type_: k.type_, present: k.present, dpl: k.dpl,
        db: k.db, s: k.s, l: k.l, g: k.g, avl: k.avl, unusable: k.unusable,
    }
}

fn guest_to_kseg(g: &GuestSegment) -> kvm_segment {
    kvm_segment {
        base: g.base, limit: g.limit, selector: g.selector,
        type_: g.type_, present: g.present, dpl: g.dpl,
        db: g.db, s: g.s, l: g.l, g: g.g, avl: g.avl,
        unusable: g.unusable, padding: 0,
    }
}
