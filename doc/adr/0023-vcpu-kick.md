# ADR-0023: Kick vCPUs out of guest code with `immediate_exit` and a real-time signal

- **Status:** Accepted; supersedes the consequence of ADR-0003 and ADR-0007 that a thread running guest code cannot be stopped
- **Date:** 2026-11-18
- **Sources:** commit 925680e; `src/loader/host_signal.rs`, `src/loader/kvm_backend.rs` (`KvmKick`), `src/loader/vm_backend.rs` (`VcpuKick`, `VmExit::Interrupted`), `src/loader/threads.rs` (`kick`, `kick_all_but`), `src/loader/signals.rs`; Linux KVM API documentation (`kvm_run.immediate_exit`)

## Context

Each OS/2 thread runs on its own host thread and KVM vCPU (ADR-0007). Warpine only regained control of a thread when its guest code called an API or faulted, so a thread spinning in its own code could not be stopped: `DosKillThread`, critical sections, exit lists, process end, `DosSuspendThread` and signal exceptions (which interrupt thread 1 wherever it is) all waited for the thread's next call, possibly for ever.

The Linux KVM API documents the way out: "In the common scenario where a signal is used to 'kick' a VCPU out of KVM_RUN", `kvm_run.immediate_exit` is polled when `KVM_RUN` starts, and a signal interrupts a running `KVM_RUN` with `EINTR`.

## Decision

- Every vCPU has a kick handle (`VcpuKick`, behind the `VmBackend` traits so tests have a mock). A kick sets the vCPU's `immediate_exit` byte (so a vCPU about to enter the guest exits at once) and then sends the real-time signal `SIGRTMIN+2` with `tgkill` to the vCPU's host thread if it is inside `KVM_RUN`. The byte is set from the kicking thread, not from the signal handler, so the handler does nothing and needs no thread-local state; it is installed with `SA_RESTART`.
- `KVM_RUN` returning `EINTR` becomes `VmExit::Interrupted`; the vCPU loop clears the byte and re-checks its kill, suspend, critical-section, exit-list and signal flags without touching the guest registers, then resumes the guest.
- Two further real-time signals (`SIGRTMIN+3` / `+4`) carry `XCPT_SIGNAL_INTR` / `XCPT_SIGNAL_BREAK` between Warpine processes; their handler stores the signal in an atomic and writes a byte to a pipe (both async-signal-safe), and a listener thread hands it to the process.
- All `unsafe` for this sits in `host_signal.rs` and `kvm_backend.rs`, each block with its justification; the `kvm_run` mapping is only touched while the kick's lock shows the vCPU still exists.

## Consequences

- `DosKillThread`, `DosEnterCritSec`, exit lists and process end now reach a thread spinning in guest code; `DosSuspendThread` / `DosResumeThread` and signal exceptions are possible.
- A thread blocked inside an API call is still not interrupted: the call finishes first (signals reach thread 1 after it). Interrupting blocking calls is a follow-up (`doc/TODOs.md`).
- Warpine claims three real-time signals; nothing else in the process may use them.
- Kicks are cheap and idempotent: several kicks before the vCPU looks give one `Interrupted`.
