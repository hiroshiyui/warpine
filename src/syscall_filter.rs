// SPDX-License-Identifier: GPL-3.0-or-later
//! Seccomp filters per process role (ADR-0124): a third wall behind
//! Warpine's own checks (ADR-0066) and Landlock (ADR-0111), should a guest
//! ever run code in Warpine's process.
//!
//! The filter is a deny list: Warpine, SDL and the display and sound
//! libraries make far more system calls than an allow list could follow,
//! but none of them needs the calls that reach the kernel's most exposed
//! or most powerful corners. Every role refuses:
//! - the 32-bit and x32 system-call ABIs (the process is killed);
//! - debugging and reading other processes (`ptrace`,
//!   `process_vm_readv` / `writev`, `pidfd_getfd`);
//! - new namespaces (`unshare`, `setns`, `clone` with a `CLONE_NEW*`
//!   flag; `clone3`, whose flags a filter cannot read, answers `ENOSYS`, so
//!   the C library falls back to `clone`);
//! - kernel attack surface nothing here uses: `bpf`, `perf_event_open`,
//!   `userfaultfd`, `io_uring_*`, the key ring, `fanotify_init`;
//! - administration: modules, `kexec`, mounts, `reboot`, swap, clocks,
//!   `iopl` / `ioperm`, `syslog`, `acct`, `quotactl`, `open_by_handle_at`;
//! - injecting input into a terminal (`ioctl` `TIOCSTI`, `TIOCLINUX`).
//!
//! The desktop server also refuses `execve` / `execveat` (it starts no
//! process) and the KVM ioctls (it runs no guest code).
//!
//! A refused call fails with `EPERM`, so a library that tries one sees an
//! error, not a crash. The filter covers every thread (`TSYNC`) and is
//! inherited by the processes a filtered one starts. `WARPINE_SANDBOX=off`
//! leaves it out, as it does Landlock.

#![forbid(unsafe_code)]

use seccompiler::sock_filter;

/// Which process the filter is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// An OS/2 program (LX, NE) or CMD.EXE: runs guest code under KVM and
    /// starts child processes.
    Guest,
    /// The session's desktop server (ADR-0052): composites and reads input;
    /// no guest code, no children.
    DesktopServer,
}

// ── seccomp and BPF constants (linux/seccomp.h, linux/filter.h, linux/audit.h)
const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const RET_ALLOW: u32 = 0x7FFF_0000;
const AUDIT_ARCH_X86_64: u32 = 62 | 0x8000_0000 | 0x4000_0000;
/// System-call numbers with this bit set are the x32 ABI's.
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// Instruction codes: class | size | mode, or class | operation | source.
const BPF_LD_W_ABS: u16 = 0x20; // BPF_LD (0) | BPF_W (0) | BPF_ABS (0x20)
const BPF_JMP_JEQ_K: u16 = 0x15; // BPF_JMP (5) | BPF_JEQ (0x10) | BPF_K (0)
const BPF_JMP_JGE_K: u16 = 0x35; // BPF_JMP | BPF_JGE (0x30)
const BPF_JMP_JSET_K: u16 = 0x45; // BPF_JMP | BPF_JSET (0x40)
const BPF_ALU_AND_K: u16 = 0x54; // BPF_ALU (4) | BPF_AND (0x50)
const BPF_RET_K: u16 = 0x06; // BPF_RET (6) | BPF_K

/// Offsets in `struct seccomp_data`.
const DATA_NR: u32 = 0;
const DATA_ARCH: u32 = 4;
/// The low 32 bits of argument `n` (little-endian).
const fn data_arg(n: u32) -> u32 { 16 + 8 * n }

/// The `CLONE_NEW*` flags `clone` takes (`CLONE_NEWTIME` shares bits with
/// the exit signal there, and only `clone3` and `unshare` take it).
const CLONE_NEW_ANY: u32 = (libc::CLONE_NEWNS | libc::CLONE_NEWCGROUP | libc::CLONE_NEWUTS | libc::CLONE_NEWIPC
    | libc::CLONE_NEWUSER | libc::CLONE_NEWPID | libc::CLONE_NEWNET) as u32;
/// Terminal ioctls that type into, or drive, a terminal.
const TIOCSTI: u32 = libc::TIOCSTI as u32;
const TIOCLINUX: u32 = 0x541C;
/// The KVM ioctls' type (`_IO(KVMIO, n)`, KVMIO = 0xAE), bits 8–15.
const IOCTL_TYPE_MASK: u32 = 0xFF00;
const KVM_IOCTL_TYPE: u32 = 0xAE00;

/// The system calls every role refuses with `EPERM`.
const DENIED: &[i64] = &[
    // Other processes' memory and debugging.
    libc::SYS_ptrace, libc::SYS_process_vm_readv, libc::SYS_process_vm_writev, libc::SYS_pidfd_getfd,
    // Namespaces.
    libc::SYS_unshare, libc::SYS_setns,
    // Kernel attack surface.
    libc::SYS_bpf, libc::SYS_perf_event_open, libc::SYS_userfaultfd,
    libc::SYS_io_uring_setup, libc::SYS_io_uring_enter, libc::SYS_io_uring_register,
    libc::SYS_keyctl, libc::SYS_add_key, libc::SYS_request_key, libc::SYS_fanotify_init,
    // Administration.
    libc::SYS_init_module, libc::SYS_finit_module, libc::SYS_delete_module,
    libc::SYS_kexec_load, libc::SYS_kexec_file_load, libc::SYS_reboot,
    libc::SYS_mount, libc::SYS_umount2, libc::SYS_pivot_root, libc::SYS_chroot,
    libc::SYS_move_mount, libc::SYS_open_tree, libc::SYS_fsopen, libc::SYS_fsconfig,
    libc::SYS_fsmount, libc::SYS_fspick, libc::SYS_mount_setattr,
    libc::SYS_swapon, libc::SYS_swapoff, libc::SYS_acct, libc::SYS_quotactl,
    libc::SYS_settimeofday, libc::SYS_clock_settime, libc::SYS_clock_adjtime, libc::SYS_adjtimex,
    libc::SYS_iopl, libc::SYS_ioperm, libc::SYS_syslog, libc::SYS_vhangup,
    libc::SYS_sethostname, libc::SYS_setdomainname, libc::SYS_open_by_handle_at,
];

/// The system calls only the desktop server refuses.
const DENIED_DESKTOP_SERVER: &[i64] = &[libc::SYS_execve, libc::SYS_execveat];

fn stmt(code: u16, k: u32) -> sock_filter { sock_filter { code, jt: 0, jf: 0, k } }
fn jump(code: u16, k: u32, jt: u8, jf: u8) -> sock_filter { sock_filter { code, jt, jf, k } }
fn nr(syscall: i64) -> u32 { u32::try_from(syscall).expect("system-call numbers are small") }

/// The BPF program for `role`.
pub fn program(role: Role) -> Vec<sock_filter> {
    let eperm = RET_ERRNO | libc::EPERM as u32;
    let enosys = RET_ERRNO | libc::ENOSYS as u32;
    let mut p = vec![
        // Only the x86-64 ABI: `int 0x80` reports the i386 one.
        stmt(BPF_LD_W_ABS, DATA_ARCH),
        jump(BPF_JMP_JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
        stmt(BPF_RET_K, RET_KILL_PROCESS),
        stmt(BPF_LD_W_ABS, DATA_NR),
        // No x32 system calls.
        jump(BPF_JMP_JGE_K, X32_SYSCALL_BIT, 0, 1),
        stmt(BPF_RET_K, RET_KILL_PROCESS),
    ];
    let denied = DENIED.iter().chain(if role == Role::DesktopServer { DENIED_DESKTOP_SERVER } else { &[] });
    for &call in denied {
        p.push(jump(BPF_JMP_JEQ_K, nr(call), 0, 1));
        p.push(stmt(BPF_RET_K, eperm));
    }
    p.push(jump(BPF_JMP_JEQ_K, nr(libc::SYS_clone3), 0, 1));
    p.push(stmt(BPF_RET_K, enosys));
    // clone: refused with a namespace flag. The block ends the program
    // either way, as the accumulator no longer holds the number.
    p.extend([
        jump(BPF_JMP_JEQ_K, nr(libc::SYS_clone), 0, 4),
        stmt(BPF_LD_W_ABS, data_arg(0)),
        jump(BPF_JMP_JSET_K, CLONE_NEW_ANY, 0, 1),
        stmt(BPF_RET_K, eperm),
        stmt(BPF_RET_K, RET_ALLOW),
    ]);
    // ioctl: the terminal-injection requests; the KVM ones in the server.
    // Each check jumps to the block's last instruction, `EPERM`.
    let mut ioctl = vec![stmt(BPF_LD_W_ABS, data_arg(1)), jump(BPF_JMP_JEQ_K, TIOCSTI, 0, 0),
                         jump(BPF_JMP_JEQ_K, TIOCLINUX, 0, 0)];
    if role == Role::DesktopServer {
        ioctl.extend([stmt(BPF_ALU_AND_K, IOCTL_TYPE_MASK), jump(BPF_JMP_JEQ_K, KVM_IOCTL_TYPE, 0, 0)]);
    }
    ioctl.push(stmt(BPF_RET_K, RET_ALLOW));
    ioctl.push(stmt(BPF_RET_K, eperm));
    let deny_at = ioctl.len() - 1;
    for (i, ins) in ioctl.iter_mut().enumerate() {
        if ins.code == BPF_JMP_JEQ_K {
            ins.jt = u8::try_from(deny_at - i - 1).expect("a short block");
        }
    }
    p.push(jump(BPF_JMP_JEQ_K, nr(libc::SYS_ioctl), 0, u8::try_from(ioctl.len()).expect("a short block")));
    p.extend(ioctl);
    p.push(stmt(BPF_RET_K, RET_ALLOW));
    p
}

/// `warpine --syscall-check <role>`: install the role's filter, then report.
pub const CHECK_SWITCH: &str = "--syscall-check";

/// After [`apply`]: the kernel's seccomp state of this process, whether a
/// thread still starts (`clone3` refused, `clone` used instead) and whether
/// a process still starts (`execve`), one `name: value` line each.
pub fn check_report() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| status.lines().find_map(|l| l.strip_prefix(name)).map(str::trim).unwrap_or("?").to_string();
    let thread = std::thread::spawn(|| ()).join().is_ok();
    let exec = match std::process::Command::new("/bin/true").status() {
        Ok(_) => "ok".to_string(),
        Err(e) if e.raw_os_error() == Some(libc::EPERM) => "EPERM".to_string(),
        Err(e) => e.to_string(),
    };
    format!("seccomp: {}\nfilters: {}\nthread: {}\nexec: {exec}\n",
            field("Seccomp:"), field("Seccomp_filters:"), if thread { "ok" } else { "failed" })
}

/// What [`apply`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The filter is installed on every thread.
    Filtered,
    /// Not an x86-64 host: no filter is written for others.
    Unsupported,
}

/// Install the filter for `role` on every thread of this process.
pub fn apply(role: Role) -> Result<Outcome, String> {
    if !cfg!(target_arch = "x86_64") {
        return Ok(Outcome::Unsupported);
    }
    seccompiler::apply_filter_all_threads(&program(role)).map_err(|e| e.to_string())?;
    Ok(Outcome::Filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A classic-BPF interpreter for the instructions [`program`] uses, over
    /// a `seccomp_data` of `arch`, `nr` and `args`.
    fn run(p: &[sock_filter], arch: u32, nr: i64, args: [u64; 6]) -> u32 {
        let mut data = [0u8; 64];
        data[0..4].copy_from_slice(&(nr as u32).to_le_bytes());
        data[4..8].copy_from_slice(&arch.to_le_bytes());
        for (i, a) in args.iter().enumerate() {
            data[16 + 8 * i..24 + 8 * i].copy_from_slice(&a.to_le_bytes());
        }
        let (mut pc, mut acc) = (0usize, 0u32);
        loop {
            let ins = &p[pc];
            pc += 1;
            match ins.code {
                BPF_LD_W_ABS => {
                    let at = ins.k as usize;
                    acc = u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
                }
                BPF_ALU_AND_K => acc &= ins.k,
                BPF_RET_K => return ins.k,
                code => {
                    let taken = match code {
                        BPF_JMP_JEQ_K => acc == ins.k,
                        BPF_JMP_JGE_K => acc >= ins.k,
                        BPF_JMP_JSET_K => acc & ins.k != 0,
                        _ => panic!("unexpected instruction {code:#x}"),
                    };
                    pc += usize::from(if taken { ins.jt } else { ins.jf });
                }
            }
            assert!(pc < p.len(), "the program ran off its end");
        }
    }

    const EPERM: u32 = RET_ERRNO | libc::EPERM as u32;
    const ENOSYS: u32 = RET_ERRNO | libc::ENOSYS as u32;

    fn call(role: Role, nr: i64, args: [u64; 6]) -> u32 { run(&program(role), AUDIT_ARCH_X86_64, nr, args) }

    #[test]
    fn ordinary_calls_pass() {
        for role in [Role::Guest, Role::DesktopServer] {
            for nr in [libc::SYS_read, libc::SYS_write, libc::SYS_mmap, libc::SYS_openat, libc::SYS_futex,
                       libc::SYS_memfd_create, libc::SYS_socket, libc::SYS_exit_group] {
                assert_eq!(call(role, nr, [0; 6]), RET_ALLOW, "{role:?} {nr}");
            }
        }
    }

    #[test]
    fn the_denied_calls_fail_with_eperm_in_every_role() {
        for role in [Role::Guest, Role::DesktopServer] {
            for &nr in DENIED {
                assert_eq!(call(role, nr, [0; 6]), EPERM, "{role:?} {nr}");
            }
        }
    }

    #[test]
    fn other_abis_kill_the_process() {
        const AUDIT_ARCH_I386: u32 = 3 | 0x4000_0000;
        let p = program(Role::Guest);
        assert_eq!(run(&p, AUDIT_ARCH_I386, libc::SYS_read, [0; 6]), RET_KILL_PROCESS);
        assert_eq!(run(&p, AUDIT_ARCH_X86_64, i64::from(X32_SYSCALL_BIT) + 101, [0; 6]), RET_KILL_PROCESS);
    }

    #[test]
    fn clone_is_refused_only_with_a_namespace_flag() {
        let thread = (libc::CLONE_VM | libc::CLONE_FS | libc::CLONE_FILES | libc::CLONE_SIGHAND
            | libc::CLONE_THREAD | libc::CLONE_SYSVSEM | libc::CLONE_SETTLS) as u64;
        let spawn = (libc::CLONE_VM | libc::CLONE_VFORK | libc::SIGCHLD) as u64;
        for role in [Role::Guest, Role::DesktopServer] {
            assert_eq!(call(role, libc::SYS_clone, [thread, 0, 0, 0, 0, 0]), RET_ALLOW);
            assert_eq!(call(role, libc::SYS_clone, [spawn, 0, 0, 0, 0, 0]), RET_ALLOW);
            for flag in [libc::CLONE_NEWUSER, libc::CLONE_NEWNS, libc::CLONE_NEWNET, libc::CLONE_NEWPID] {
                assert_eq!(call(role, libc::SYS_clone, [spawn | flag as u64, 0, 0, 0, 0, 0]), EPERM);
            }
            assert_eq!(call(role, libc::SYS_clone3, [0; 6]), ENOSYS);
        }
    }

    #[test]
    fn terminal_injection_is_refused_and_kvm_only_in_the_server() {
        const KVM_CREATE_VM: u64 = 0xAE01;
        const KVM_RUN: u64 = 0xAE80;
        const TCGETS: u64 = 0x5401;
        for role in [Role::Guest, Role::DesktopServer] {
            assert_eq!(call(role, libc::SYS_ioctl, [0, u64::from(TIOCSTI), 0, 0, 0, 0]), EPERM);
            assert_eq!(call(role, libc::SYS_ioctl, [0, u64::from(TIOCLINUX), 0, 0, 0, 0]), EPERM);
            assert_eq!(call(role, libc::SYS_ioctl, [0, TCGETS, 0, 0, 0, 0]), RET_ALLOW);
            // The kernel reads the request as 32 bits: high bits change nothing.
            assert_eq!(call(role, libc::SYS_ioctl, [0, (1 << 32) | u64::from(TIOCSTI), 0, 0, 0, 0]), EPERM);
        }
        for request in [KVM_CREATE_VM, KVM_RUN] {
            assert_eq!(call(Role::Guest, libc::SYS_ioctl, [3, request, 0, 0, 0, 0]), RET_ALLOW);
            assert_eq!(call(Role::DesktopServer, libc::SYS_ioctl, [3, request, 0, 0, 0, 0]), EPERM);
        }
    }

    #[test]
    fn only_the_desktop_server_refuses_exec() {
        for nr in [libc::SYS_execve, libc::SYS_execveat] {
            assert_eq!(call(Role::Guest, nr, [0; 6]), RET_ALLOW);
            assert_eq!(call(Role::DesktopServer, nr, [0; 6]), EPERM);
        }
    }

    #[test]
    fn jumps_stay_inside_the_program() {
        for role in [Role::Guest, Role::DesktopServer] {
            let p = program(role);
            assert!(p.len() < usize::from(u16::MAX));
            for (i, ins) in p.iter().enumerate() {
                if ins.code & 0x07 == 0x05 {
                    assert!(i + 1 + usize::from(ins.jt.max(ins.jf)) < p.len(), "{role:?} at {i}");
                }
            }
            assert_eq!(p.last().unwrap().code, BPF_RET_K);
        }
    }
}
