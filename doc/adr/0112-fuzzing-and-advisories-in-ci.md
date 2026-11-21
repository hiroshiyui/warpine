# ADR-0112: Fuzzing the parsers of untrusted input, and dependency advisories in CI

- **Status:** Accepted; amends ADR-0106 (the CI matrix) and ADR-0066 (pinned third-party inputs)
- **Date:** 2026-11-21
- **Sources:**
  - LLVM's libFuzzer documentation (options, corpus and artifact handling) and the `libfuzzer-sys` crate (0.4.13; MIT OR Apache-2.0, with libFuzzer under NCSA).
  - The flags `cargo-fuzz` passes to rustc (`-Cpasses=sancov-module`, the `-sanitizer-coverage-*` options, `-Zsanitizer=address`, `--cfg fuzzing`).
  - cargo-deny's documentation (`check advisories` against the RustSec database) and its release 0.19.4 with its published SHA-256.

## Context

The "Security Facilities" TODOs named two gaps:
- **No fuzzing.** Several parsers read input a hostile party controls:
  - the LX and NE loaders (a program or DLL file);
  - the desktop protocol (the desktop server decodes every program's packets, and each program the server's);
  - message files (ADR-0105).
- **No advisory check in CI.** Advisories in dependencies were checked only at release time. The pre-commit hook runs `cargo deny` for sources, bans and licences, which need no network.

## Decision

1. **`fuzz/`, a workspace of its own,** on the guest crates' dated nightly (`nightly-2026-11-15`). Warpine's build does not depend on it.
   - **The targets include Warpine's modules by `#[path]`** (Warpine has no library crate), as `os2msg` and `lx-link` include `msgfile.rs`:
     - `desktop_proto`: `ClientMsg` / `ServerMsg` decoders, and a decoded message must encode and decode to itself. Two small shims stand in for the constants and handle-slot helpers it takes from the loader;
     - `lx_parse` and `ne_parse`: the executable parsers;
     - `msgfile`: the file and bound-segment readers and the compiler; a parsed file must write back and parse to itself.
   - **Dependencies:** `libfuzzer-sys` pinned exactly, `log` at the version Warpine locks. Transitive crates are held at releases at least two weeks old (`cc` 1.11.1), and `fuzz/deny.toml` allows NCSA besides Warpine's licences.
   - **`fuzz/run.sh <target> [seconds] [corpus]`** builds with libFuzzer's coverage instrumentation and the address sanitizer (the flags `cargo-fuzz` uses, without `cargo-fuzz`) and runs the target.
   - **`fuzz/seed.sh`** seeds the executables and message files from the built samples.
   - Corpus and artifacts stay out of the repository.
2. **CI's watcom entry** (ADR-0106):
   - runs `cargo deny check` in full, advisories included, in the main workspace and in `fuzz/`;
   - then fuzzes each target for a minute (`FUZZ_SECONDS`).

   A crash fails the entry and prints its input as base64, since the workflow uses no third-party action to upload artifacts.
3. **The CI image** gains:
   - `cargo-deny` 0.19.4, its release binary checked against its SHA-256;
   - the fuzz toolchain, through rustup, which checks it against the channel manifest, with `rust-src`: it is also the Rust guest crates' toolchain, and once it is in the image `test_rust_hello` builds `samples/rust_hello` with `-Z build-std` instead of skipping.

   The image's tag names both.

## Warpine's choices (unverified)

- **A minute per target** in CI: enough to catch regressions in the shallow paths, not a substitute for long runs.
- **The shims** in `desktop_proto.rs` copy four constants and a helper; a change to those in Warpine needs the same change there (the fuzz build breaks only when a name changes, not a value).
- **cargo-deny 0.19.4,** the version the pre-commit hook already used, not the newest release (two days old at the time).

## Consequences

- **The first runs found three crashes, fixed with regression tests:**
  - `LxFile::parse` reserved capacity from the header's imported-module count, about 100 GB for a hostile file. Every capacity taken from an LX header is now bounded by what the file could hold (`lx.rs`).
  - `NeFile::parse` panicked on an alignment shift count of 64 or more. Counts of 32 or more are refused.
  - `parse_entry_table` panicked on a bundle header cut after its count, and on ordinals running past 65535. It now stops there.
- After the fixes, two minutes per target found nothing more: 16.7 million inputs for the desktop protocol, 249,000 for LX, 6.2 million for NE and 5.2 million for message files.
- **Later targets:** `os2rc` (resource scripts compiled in memory, reading no file: `compile_in_memory`) and `lx_link` (one input, its kind picked by the first byte, linked in memory with its debug companion: `link_in_memory`). Both include the tool's whole source by `#[path]`. The first `lx_link` run found an object whose section alignment of 2³¹ made the linker allocate 3.6 GB; alignments above 64 KB and common symbols above 256 MB are refused since. On the way, `seed.sh`'s `find -size -1M` turned out to match only empty files, so message files had never been seeded.
- A new advisory against a dependency turns CI red until it is fixed or written off in `deny.toml` with a reason.
- **Found after release:** the image first installed the nightly without `rust-src`. `test_rust_hello`, which had skipped for want of the toolchain, then failed in every matrix entry, from this change until it was fixed (v0.0.9 was released with CI red). The image's tag names the component, so a fixed image is built.
- **Found with it:** `cargo deny`'s advisory check, which CI had never reached, failed in the image: the release binary fetches the advisory database with `git`, which the image lacked. The image installs `git` from the dated Debian snapshot, and its tag carries a revision (`IMAGE_REVISION`) counted up when its package list changes.
