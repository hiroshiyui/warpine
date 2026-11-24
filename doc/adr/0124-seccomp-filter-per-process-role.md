# ADR-0124: A seccomp deny list per process role, behind Landlock

- **Status:** Accepted; adds a third layer to ADR-0066 and ADR-0111
- **Date:** 2026-11-24
- **Sources:**
  - The Linux kernel's seccomp documentation (`Documentation/userspace-api/seccomp_filter.rst`) and `seccomp(2)`: classic-BPF filters over `struct seccomp_data`, `SECCOMP_RET_ERRNO` / `KILL_PROCESS` / `ALLOW`, `SECCOMP_FILTER_FLAG_TSYNC`, `no_new_privs`, inheritance across `fork` and `execve`, the architecture check and the x32 bit.
  - `clone(2)` and `clone3(2)`: `clone3` passes its flags in memory a filter cannot read; the C library falls back to `clone` when `clone3` answers `ENOSYS`.
  - `ioctl_tty(2)` (`TIOCSTI`, `TIOCLINUX`) and `linux/kvm.h` (`KVMIO` = 0xAE).
  - The `seccompiler` crate (0.5.0, Apache-2.0 OR BSD-3-Clause, rust-vmm), whose `apply_filter_all_threads` installs a program.

## Context

Landlock (ADR-0111) keeps a compromised Warpine process away from the user's files, but not from the rest of the kernel: the process could still debug and read other processes of the user, enter new user namespaces (a common first step of kernel exploits), or reach interfaces with a long record of kernel bugs (`bpf`, `io_uring`, `userfaultfd`, `perf_event_open`). It could also type into the terminal it was started from (`TIOCSTI`). The "Security Facilities" TODOs asked for seccomp filters per process role.

Warpine has two kinds of process: guest processes (LX and NE programs, CMD.EXE), which run guest code under KVM and start child processes, and the session's desktop server, which composites and reads input and does neither.

## Decision

1. **A deny list, not an allow list.** Warpine, SDL3 and the display, GPU and sound libraries it loads make many system calls, which differ between desktops, drivers and library versions; an allow list would break on systems the test suite never sees. The filter refuses the calls nothing in Warpine needs and that carry most of the risk (`src/syscall_filter.rs`, `DENIED`):
   - other processes: `ptrace`, `process_vm_readv` / `writev`, `pidfd_getfd`;
   - namespaces: `unshare`, `setns`, and `clone` with a `CLONE_NEW*` flag; `clone3` answers `ENOSYS`;
   - kernel attack surface: `bpf`, `perf_event_open`, `userfaultfd`, `io_uring_*`, `keyctl` / `add_key` / `request_key`, `fanotify_init`;
   - administration: modules, `kexec`, `reboot`, mounts (old and new API), `chroot` / `pivot_root`, swap, `acct`, `quotactl`, clocks, `iopl` / `ioperm`, `syslog`, `vhangup`, host and domain names, `open_by_handle_at`;
   - `ioctl` `TIOCSTI` and `TIOCLINUX` (typing into a terminal).
2. **Roles** (`syscall_filter::Role`):
   - `Guest`: the list above;
   - `DesktopServer`: also `execve` / `execveat` and every KVM ioctl (type 0xAE).
3. **Other ABIs kill the process:** an architecture other than x86-64 (`int 0x80` reports i386) or an x32 system call, which could otherwise reach the denied calls under other numbers.
4. **Refused calls fail with `EPERM`**, so a library that tries one gets an error it can handle.
5. **The program is written by hand** as classic BPF (a few instructions per denied call), in safe Rust; `seccompiler` installs it with `TSYNC` on every thread, so a thread started before the filter is covered too. The filter is inherited by the processes a filtered one starts.
6. **When:** right after Landlock, in `sandbox::confine_process`, before any guest code, in every role. `WARPINE_SANDBOX=off` leaves both out; a failure to install it is a warning. Non-x86-64 hosts get no filter (Warpine needs x86-64 KVM anyway).
7. **`warpine --syscall-check guest|desktop-server`** installs a role's filter and reports the kernel's seccomp state, whether a thread starts and whether a process starts.

## Warpine's choices (unverified)

- **The deny list** is Warpine's own selection, after the kernel documentation and common container practice; it was checked against the test suite on SDL's dummy drivers only. A real display, GPU or sound stack that needs a refused call gets `EPERM`; `WARPINE_SANDBOX=off` is the way out, and such a call should be reported.
- **`clone3` answering `ENOSYS`** relies on the C library's fallback to `clone` (glibc 2.34 and later, musl's `clone` path).
- **The desktop server refusing `execve`** assumes SDL starts no helper processes there (none do on the dummy drivers).
- **`personality`, `kcmp`, `mbind` / `migrate_pages` / `move_pages`** stay allowed: they need privileges or reach only the caller.

## Consequences

- A compromised Warpine process can no longer debug the user's other processes, enter new namespaces, use the kernel interfaces above or type into its terminal; the desktop server cannot start programs or KVM guests.
- **New dependency:** `seccompiler` (Apache-2.0 OR BSD-3-Clause), checked by `deny.toml`; the unsafe `seccomp` call stays inside it, `syscall_filter.rs` is `#![forbid(unsafe_code)]`.
- **Tests:**
  - `syscall_filter.rs`: a BPF interpreter runs the program for each role over the denied calls, ordinary calls, other ABIs, `clone` flags, `clone3`, the terminal and KVM ioctls (a 32-bit request with high bits set too), `execve`, and checks the jumps stay inside the program;
  - `integration.rs`: `the_syscall_filter_is_installed_per_role`; every other integration test runs filtered.
