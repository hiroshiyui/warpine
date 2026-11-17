# ADR-0015: Build Rust guest programs with Warpine's own ELF-to-LX linker

- **Status:** Accepted
- **Date:** 2026-04-01
- **Sources:** commits 0cf6252 (plan), f55deeb (lx-link); `src/bin/lx_link.rs`; `targets/i686-warpine-os2.json`; `crates/warpine-os2*`; `CLAUDE.md` (Rust Guest Toolchain)

## Context

New OS/2 programs, tests and samples are easier to write in Rust than in C, but no Rust toolchain produces LX executables.

## Decision

Rust guest programs are compiled for a custom `i686-warpine-os2` target (`no_std`, `panic=abort`, only `R_386_32` relocations) and linked by `lx-link` (`src/bin/lx_link.rs`), which reads ELF objects and archives, resolves OS/2 imports through `targets/os2api.def` (ADR-0006) and writes an MZ + LX executable. The guest crates `warpine-os2-sys`, `warpine-os2-rt` and `warpine-os2` provide raw bindings, the runtime and safe wrappers; they build with `cargo +nightly` and are not part of the host workspace.

## Consequences

- Guest programs can be written in Rust; `samples/rust_hello` runs end to end.
- `lx-link` and the target spec are maintained by Warpine.
- C guests built with GCC or clang through `lx-link` are planned (`doc/TODOs.md`).
