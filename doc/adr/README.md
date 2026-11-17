# Architecture Decision Records

Why Warpine is built the way it is. Each record states one decision, its context and its consequences, in the format of Michael Nygard's "Documenting Architecture Decisions" (ADR-0001).

To add one: copy the layout of an existing record, take the next number, and add it to this list. A changed decision gets a new record that supersedes the old one; the old record is kept.

| ADR | Decision | Status | Date |
|---|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted | 2026-11-18 |
| [0002](0002-gpl-3-only.md) | License Warpine under GPL-3.0-only | Accepted | 2026-03-12 |
| [0003](0003-kvm-native-execution.md) | Run OS/2 code natively on KVM | Accepted | 2026-03-12 |
| [0004](0004-near-clean-room-policy.md) | Implement OS/2 from public documentation only (near-clean-room) | Accepted | 2026-03-18 |
| [0005](0005-api-thunks-and-module-slots.md) | Dispatch API calls through INT 3 thunks with one ordinal slot per built-in module | Accepted | 2026-03-14 |
| [0006](0006-ordinal-sources-of-truth.md) | Use the Open Watcom import library and toolkit headers as the sources of ordinals and constants | Accepted | 2026-03-14 |
| [0007](0007-threads-on-vcpus.md) | Run each OS/2 thread on its own host thread and KVM vCPU | Accepted | 2026-03-12 |
| [0008](0008-unicode-inside.md) | Keep text as Unicode inside Warpine and convert at the codepage boundary | Accepted | 2026-03-17 |
| [0009](0009-drives-on-host-directories.md) | Map OS/2 drives to host directories, with C: in the user's data directory | Accepted | 2026-03-14 |
| [0010](0010-sdl3-safe-only.md) | Use SDL3 through the safe sdl3-rs API only | Accepted | 2026-11-16 |
| [0011](0011-own-desktop-compositor.md) | Draw the PM desktop in one host window with Warpine's own compositor | Accepted | 2026-04-02 |
| [0012](0012-fonts-plex-and-unifont.md) | Draw PM text with IBM Plex and text mode with GNU Unifont | Accepted | 2026-11-15 |
| [0013](0013-vio-in-pm-window.md) | Run text-mode (VIO) programs in a PM window by default | Accepted | 2026-11-17 |
| [0014](0014-builtin-cmd.md) | Provide CMD.EXE as a built-in Rust shell | Accepted | 2026-04-01 |
| [0015](0015-rust-guest-toolchain.md) | Build Rust guest programs with Warpine's own ELF-to-LX linker | Accepted | 2026-04-01 |
| [0016](0016-locale-from-host.md) | Take the country and codepage from the host locale | Accepted | 2026-11-17 |
| [0017](0017-profile-text-format.md) | Store OS/2 profiles (INI files) in Warpine's own text format | Accepted | 2026-11-18 |
| [0018](0018-testing-oracle-and-goldens.md) | Test against Open Watcom sample programs and golden screen signatures | Accepted | 2026-11-15 |
| [0019](0019-quality-gates.md) | Gate every commit on tests and lint-free code with a pinned toolchain | Accepted | 2026-03-31 |
| [0020](0020-one-process-per-host-process.md) | Run each OS/2 process as its own host process | Accepted | 2026-03-13 |
