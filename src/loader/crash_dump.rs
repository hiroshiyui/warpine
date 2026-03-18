// SPDX-License-Identifier: GPL-3.0-only
//
// Guest crash-dump facility.
//
// Collects a snapshot of guest CPU state at crash time (exception, triple
// fault, unhandled VMEXIT, KVM error) and writes a human-readable report to
// both stderr and a `warpine-crash-<pid>.txt` file in the current directory.

use super::{GuestRegs, GuestSregs, Loader};
use super::mutex_ext::MutexExt;
use super::vm_backend::VcpuBackend;
use std::fmt::Write as FmtWrite;
use std::io::Write as IoWrite;

// ── Context enum ─────────────────────────────────────────────────────────────

/// Describes the event that triggered the crash dump.
#[derive(Debug)]
pub enum CrashContext {
    /// Guest CPU exception delivered via an IDT handler stub.
    GuestException {
        vector:       u32,
        error_code:   u32,
        fault_eip:    u32,
        fault_cs:     u32,
        fault_eflags: u32,
    },
    /// Guest triple-fault / KVM shutdown condition.
    TripleFault,
    /// An unhandled VMEXIT reason that the loop cannot continue past.
    UnhandledVmexit { description: String },
    /// `vcpu.run()` returned an error from the hypervisor.
    KvmRunError { description: String },
    /// A software breakpoint at an address Warpine did not install.
    UnexpectedBreakpoint,
}

// ── Report struct ─────────────────────────────────────────────────────────────

/// Snapshot of guest CPU state collected at crash time.
pub struct CrashReport {
    pub vcpu_id:     u32,
    pub context:     CrashContext,
    pub regs:        GuestRegs,
    pub sregs:       GuestSregs,
    /// Top 32 dwords from the effective guest stack (ESP / SS.base+SP).
    pub stack_words: Vec<u32>,
    /// Up to 32 bytes at the fault/current instruction pointer (flat address).
    pub code_bytes:  Vec<u8>,
    /// Flat guest address `code_bytes` were read from.
    pub code_addr:   u32,
    pub exe_name:    String,
    pub timestamp:   std::time::SystemTime,
}

// ── Loader methods ────────────────────────────────────────────────────────────

impl Loader {
    /// Collect a [`CrashReport`] snapshot from the current vCPU state.
    pub(crate) fn collect_crash_report(
        &self,
        vcpu: &dyn VcpuBackend,
        vcpu_id: u32,
        context: CrashContext,
    ) -> CrashReport {
        let regs  = vcpu.get_regs().unwrap_or_default();
        let sregs = vcpu.get_sregs().unwrap_or_default();

        // Choose the flat address to disassemble:
        // - GuestException: use fault_eip (adjusted for 16-bit CS if needed)
        // - everything else: use current RIP
        let flat_code_addr: u32 = match &context {
            CrashContext::GuestException { fault_eip, .. } => {
                if sregs.cs.db == 0 {
                    sregs.cs.base as u32 + *fault_eip
                } else {
                    *fault_eip
                }
            }
            _ => regs.rip as u32,
        };

        let mut code_bytes = vec![0u8; 32];
        for (i, b) in code_bytes.iter_mut().enumerate() {
            *b = self.guest_read::<u8>(flat_code_addr.wrapping_add(i as u32))
                     .unwrap_or(0xCC);
        }

        // Effective ESP: for a 16-bit SS (D/B=0) the CPU addresses SS.base+SP.
        let flat_esp: u32 = if sregs.ss.db == 0 {
            sregs.ss.base as u32 + regs.rsp as u16 as u32
        } else {
            regs.rsp as u32
        };

        let mut stack_words = Vec::with_capacity(32);
        for i in 0..32u32 {
            stack_words.push(
                self.guest_read::<u32>(flat_esp.wrapping_add(i * 4))
                    .unwrap_or(0xDEADC0DE),
            );
        }

        let exe_name = self.shared.exe_name.lock_or_recover().clone();

        CrashReport {
            vcpu_id,
            context,
            regs,
            sregs,
            stack_words,
            code_bytes,
            code_addr: flat_code_addr,
            exe_name,
            timestamp: std::time::SystemTime::now(),
        }
    }

    /// Format `report`, print it to stderr, and write it to
    /// `warpine-crash-<pid>.txt` in the current directory.
    ///
    /// Returns the path written (or a diagnostic string on I/O error).
    pub(crate) fn dump_crash_report(&self, report: &CrashReport) -> String {
        let text = format_crash_report(report);
        eprint!("{}", text);

        let pid  = std::process::id();
        let path = format!("warpine-crash-{}.txt", pid);
        match std::fs::File::create(&path) {
            Ok(mut f) => {
                let _ = f.write_all(text.as_bytes());
                eprintln!("[crash dump written to {}]", path);
                path
            }
            Err(e) => {
                eprintln!("[crash dump: failed to write {}: {}]", path, e);
                format!("<failed: {}>", e)
            }
        }
    }
}

// ── Formatting ────────────────────────────────────────────────────────────────

fn format_crash_report(r: &CrashReport) -> String {
    let mut out = String::new();
    let regs  = &r.regs;
    let sregs = &r.sregs;

    // Effective ESP (same logic as collection)
    let flat_esp: u32 = if sregs.ss.db == 0 {
        sregs.ss.base as u32 + regs.rsp as u16 as u32
    } else {
        regs.rsp as u32
    };

    let ts = format_timestamp(r.timestamp);
    let _ = writeln!(out, "╔══════════════════════════════════════════════════════════════╗");
    let _ = writeln!(out, "║         Warpine Guest Crash Dump  {}         ║", ts);
    let _ = writeln!(out, "╚══════════════════════════════════════════════════════════════╝");
    let _ = writeln!(out, "Process : {}", r.exe_name);
    let _ = writeln!(out, "Host PID: {}   VCPU: {}", std::process::id(), r.vcpu_id);
    let _ = writeln!(out);

    // ── Context ───────────────────────────────────────────────────────────────
    match &r.context {
        CrashContext::GuestException { vector, error_code, fault_eip, fault_cs, fault_eflags } => {
            let _ = writeln!(out, "Context : CPU Exception #{} — {}", vector, exception_name(*vector));
            let _ = writeln!(out,
                "Fault   : EIP=0x{:08X}  CS=0x{:04X}  EFLAGS=0x{:08X}  ErrCode=0x{:08X}",
                fault_eip, fault_cs, fault_eflags, error_code);
        }
        CrashContext::TripleFault => {
            let _ = writeln!(out, "Context : Triple Fault (guest shutdown / KVM_EXIT_SHUTDOWN)");
        }
        CrashContext::UnhandledVmexit { description } => {
            let _ = writeln!(out, "Context : Unhandled VMEXIT — {}", description);
        }
        CrashContext::KvmRunError { description } => {
            let _ = writeln!(out, "Context : KVM run error — {}", description);
        }
        CrashContext::UnexpectedBreakpoint => {
            let _ = writeln!(out, "Context : Unexpected breakpoint at EIP=0x{:08X}", regs.rip);
        }
    }
    let _ = writeln!(out);

    // ── General-purpose registers ─────────────────────────────────────────────
    let _ = writeln!(out, "── Registers ───────────────────────────────────────────────────");
    let _ = writeln!(out, "  EIP=0x{:08X}  EFLAGS=0x{:08X}", regs.rip, regs.rflags);
    let _ = writeln!(out, "  EAX=0x{:08X}  EBX=0x{:08X}  ECX=0x{:08X}  EDX=0x{:08X}",
        regs.rax as u32, regs.rbx as u32, regs.rcx as u32, regs.rdx as u32);
    let _ = writeln!(out, "  ESI=0x{:08X}  EDI=0x{:08X}  EBP=0x{:08X}  ESP=0x{:08X}",
        regs.rsi as u32, regs.rdi as u32, regs.rbp as u32, regs.rsp as u32);
    let _ = writeln!(out);

    // ── Segment registers ─────────────────────────────────────────────────────
    let _ = writeln!(out, "── Segments ────────────────────────────────────────────────────");
    for (name, seg) in [
        ("CS", &sregs.cs), ("DS", &sregs.ds), ("SS", &sregs.ss),
        ("ES", &sregs.es), ("FS", &sregs.fs), ("GS", &sregs.gs),
    ] {
        let _ = writeln!(out,
            "  {} sel=0x{:04X} base=0x{:08X} limit=0x{:08X} db={} type=0x{:02X} dpl={}",
            name, seg.selector, seg.base, seg.limit, seg.db, seg.type_, seg.dpl);
    }
    let _ = writeln!(out, "  CR0=0x{:08X}  CR2=0x{:08X}  CR4=0x{:08X}",
        sregs.cr0 as u32, sregs.cr2 as u32, sregs.cr4 as u32);
    let _ = writeln!(out);

    // ── Code bytes at EIP ─────────────────────────────────────────────────────
    let _ = writeln!(out, "── Code at EIP (flat 0x{:08X}) ─────────────────────────────────",
        r.code_addr);
    hex_dump(&mut out, r.code_addr, &r.code_bytes);
    let _ = writeln!(out);

    // ── Stack ─────────────────────────────────────────────────────────────────
    let _ = writeln!(out, "── Stack (32 dwords from ESP flat 0x{:08X}) ─────────────────────",
        flat_esp);
    for (row, chunk) in r.stack_words.chunks(4).enumerate() {
        let addr = flat_esp.wrapping_add(row as u32 * 16);
        let _ = write!(out, "  0x{:08X}: ", addr);
        for w in chunk {
            let _ = write!(out, "{:08X} ", w);
        }
        let _ = writeln!(out);
    }
    let _ = writeln!(out);

    out
}

fn hex_dump(out: &mut String, base_addr: u32, bytes: &[u8]) {
    for (row, chunk) in bytes.chunks(16).enumerate() {
        let addr = base_addr.wrapping_add(row as u32 * 16);
        let _ = write!(out, "  0x{:08X}: ", addr);
        for b in chunk {
            let _ = write!(out, "{:02X} ", b);
        }
        for _ in chunk.len()..16 {
            let _ = write!(out, "   ");
        }
        let _ = write!(out, " |");
        for &b in chunk {
            let c = if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' };
            let _ = write!(out, "{}", c);
        }
        let _ = writeln!(out, "|");
    }
}

fn exception_name(vector: u32) -> &'static str {
    match vector {
        0  => "Divide Error (#DE)",
        1  => "Debug (#DB)",
        2  => "NMI",
        3  => "Breakpoint (#BP)",
        4  => "Overflow (#OF)",
        5  => "Bound Range Exceeded (#BR)",
        6  => "Invalid Opcode (#UD)",
        7  => "Device Not Available (#NM)",
        8  => "Double Fault (#DF)",
        10 => "Invalid TSS (#TS)",
        11 => "Segment Not Present (#NP)",
        12 => "Stack-Segment Fault (#SS)",
        13 => "General Protection Fault (#GP)",
        14 => "Page Fault (#PF)",
        16 => "x87 FP Exception (#MF)",
        17 => "Alignment Check (#AC)",
        18 => "Machine Check (#MC)",
        19 => "SIMD FP Exception (#XM)",
        _  => "Unknown",
    }
}

/// Format a `SystemTime` as `YYYY-MM-DD HH:MM:SS UTC` without external crates.
fn format_timestamp(ts: std::time::SystemTime) -> String {
    let secs = ts.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Compute calendar fields from Unix timestamp (no leap-seconds).
    let s   = secs % 60;
    let m   = (secs / 60) % 60;
    let h   = (secs / 3600) % 24;
    let mut days = secs / 86400;        // days since 1970-01-01
    let mut year = 1970u32;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let days_in_year: u64 = if leap { 366 } else { 365 };
        if days < days_in_year { break; }
        days -= days_in_year;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days: [u64; 12] = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u32;
    for &md in &month_days {
        if days < md { break; }
        days -= md;
        month += 1;
    }
    let day = days + 1;
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", year, month, day, h, m, s)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;

    fn make_report(context: CrashContext) -> CrashReport {
        CrashReport {
            vcpu_id: 0,
            context,
            regs: GuestRegs {
                rip: 0x12345678,
                rsp: 0x001FFFF0,
                rax: 0xDEADBEEF,
                rbx: 0xCAFEBABE,
                rcx: 1,
                rdx: 2,
                rsi: 3,
                rdi: 4,
                rbp: 0x001FFFE0,
                rflags: 0x00000246,
            },
            sregs: GuestSregs::default(),
            stack_words: vec![0xDEADBEEFu32; 32],
            code_bytes: (0u8..32).collect(),
            code_addr: 0x12345678,
            exe_name: "TEST.EXE".to_string(),
            timestamp: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn test_format_exception_report_contains_key_fields() {
        let report = make_report(CrashContext::GuestException {
            vector: 13,
            error_code: 0,
            fault_eip: 0x12345678,
            fault_cs: 0x0008,
            fault_eflags: 0x00010202,
        });
        let text = format_crash_report(&report);
        assert!(text.contains("Exception #13"), "missing exception header");
        assert!(text.contains("General Protection Fault"), "missing exception name");
        assert!(text.contains("0x12345678"), "missing fault EIP");
        assert!(text.contains("EAX=0xDEADBEEF"), "missing EAX");
        assert!(text.contains("EBX=0xCAFEBABE"), "missing EBX");
    }

    #[test]
    fn test_format_triple_fault_report() {
        let text = format_crash_report(&make_report(CrashContext::TripleFault));
        assert!(text.contains("Triple Fault"));
        assert!(text.contains("EIP=0x12345678"));
    }

    #[test]
    fn test_format_unhandled_vmexit_report() {
        let text = format_crash_report(&make_report(CrashContext::UnhandledVmexit {
            description: "ExitReason(99)".to_string(),
        }));
        assert!(text.contains("ExitReason(99)"));
    }

    #[test]
    fn test_format_kvm_run_error() {
        let text = format_crash_report(&make_report(CrashContext::KvmRunError {
            description: "ENXIO".to_string(),
        }));
        assert!(text.contains("ENXIO"));
    }

    #[test]
    fn test_format_unexpected_breakpoint() {
        let text = format_crash_report(&make_report(CrashContext::UnexpectedBreakpoint));
        assert!(text.contains("Unexpected breakpoint"));
        assert!(text.contains("0x12345678"));
    }

    #[test]
    fn test_hex_dump_ascii_printable() {
        let mut out = String::new();
        let bytes = b"Hello, World!   "; // exactly 16 bytes
        hex_dump(&mut out, 0x1000, bytes);
        assert!(out.contains("0x00001000"));
        assert!(out.contains("48 65 6C 6C 6F")); // "Hello"
        assert!(out.contains("|Hello, World!   |"));
    }

    #[test]
    fn test_hex_dump_non_printable_replaced() {
        let mut out = String::new();
        hex_dump(&mut out, 0, &[0x00, 0x01, 0x1F, 0x7F, 0x80, 0xFF]);
        // Non-printable bytes should appear as '.' in the ASCII column
        assert!(out.contains("|......|"));
    }

    #[test]
    fn test_exception_name_all_known() {
        for &v in &[0u32, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 16, 17, 18, 19] {
            assert_ne!(exception_name(v), "Unknown", "vector {} should be named", v);
        }
    }

    #[test]
    fn test_exception_name_unknown() {
        assert_eq!(exception_name(255), "Unknown");
        assert_eq!(exception_name(9),   "Unknown");
    }

    #[test]
    fn test_format_timestamp_epoch() {
        let ts = std::time::SystemTime::UNIX_EPOCH;
        assert_eq!(format_timestamp(ts), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn test_format_timestamp_known_date() {
        // 2024-03-18 12:14:56 UTC = 1710764096
        let ts = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(1710764096);
        let s = format_timestamp(ts);
        assert_eq!(s, "2024-03-18 12:14:56 UTC");
    }

    #[test]
    fn test_dump_crash_report_creates_file() {
        let loader = Loader::new_mock();
        let report = make_report(CrashContext::TripleFault);
        let path = loader.dump_crash_report(&report);
        assert!(path.ends_with(".txt"), "expected .txt, got: {}", path);
        let p = std::path::Path::new(&path);
        assert!(p.exists(), "crash file not created: {}", path);
        let content = std::fs::read_to_string(p).unwrap();
        assert!(content.contains("Triple Fault"));
        assert!(content.contains("TEST.EXE"));
        let _ = std::fs::remove_file(p);
    }
}
