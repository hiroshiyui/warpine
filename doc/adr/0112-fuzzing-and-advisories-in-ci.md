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
- **Later still:** `bitmap` fuzzes the bitmap codec, taken out of `pm_bitmap.rs` into `bitmap_codec.rs` so that it builds alone (two minutes, 1.9 million inputs, nothing found). A weekly workflow (`fuzz.yml`, `ci/matrix.sh fuzz`) runs every target for ten minutes, past the push CI's minute.
- A new advisory against a dependency turns CI red until it is fixed or written off in `deny.toml` with a reason.
- **Found after release:** the image first installed the nightly without `rust-src`. `test_rust_hello`, which had skipped for want of the toolchain, then failed in every matrix entry, from this change until it was fixed (v0.0.9 was released with CI red). The image's tag names the component, so a fixed image is built.
- **Found with it:** `cargo deny`'s advisory check, which CI had never reached, failed in the image: the release binary fetches the advisory database with `git`, which the image lacked. The image installs `git` from the dated Debian snapshot, and its tag carries a revision (`IMAGE_REVISION`) counted up when its package list changes.

## Addendum (2026-11-24): the resource parsers

`res_templates` fuzzes the parsers of the resources a running program hands Warpine — dialog templates, menu templates, accelerator tables, `RT_STRING` / `RT_MESSAGE` bundles and `PRESPARAMS` blocks — moved into `src/loader/res_templates.rs` so that they build alone (code page decoding and class-atom names come in as functions; the old modules re-export). The menu parser now reads a byte slice instead of guest memory and stops nesting at `MAX_MENU_DEPTH` (16): a hostile template could recurse into submenus once per 16 bytes. Two minutes, 1.76 million inputs, nothing found.

Later the same day `pointer` (the `RT_POINTER` decoder, `pointer_codec.rs`) and `wav` (the waveaudio device's WAV parser, `wav.rs`) followed, each taken out of its module the same way: a minute each, 0.87 and 25.8 million inputs, nothing found.

## Addendum (2026-11-25): the files a program can write

Two parsers read files on drives a program can write, and had no target: `C:\CONFIG.SYS` and the `Prf*` profiles. Their pure parts move into modules of their own, which the old ones re-export: `config_sys_parse.rs` (the statements, the environment, the `THREADS=` limit; `config_sys.rs` keeps the read through the VFS) and `profile_text.rs` (the model and Warpine's text format; `profile.rs` keeps loading and saving). `config_sys` checks the bounds (LIBPATH entries, one environment block, the limit's range); `profile` checks that a parsed profile written back parses to the same profile. A minute each, 1.27 and 3.25 million inputs, nothing found; `seed.sh` writes a CONFIG.SYS and a profile.

Warpine reads no font or help file a program supplies (its fonts are built in, `GpiLoadFonts` is a stub, the Help Manager records a library's name only), so there is no such reader to fuzz yet.

## Addendum (2026-11-26): EA lists

`DosSetPathInfo` level 2 now sets extended attributes, so a program's `FEA2LIST` is parsed (`src/loader/ea_list.rs`, which builds alone). The `fea2_list` target checks that parsing never panics, that every entry it gives fits inside `cbList` with a non-empty name without NULs, and that an error names an offset inside the list. It ran 32 million inputs in 90 seconds without finding anything. `seed.sh` writes one list.

Later the same day the target grew with the level-3 queries: the `GEA2LIST` they take (`parse_gea2_list`, the same checks) and the lists Warpine builds (`build_fea2_list` must parse back to the same entries and state its own length; `build_dena2` must stay in the room it is given). 15 million inputs in two minutes found nothing.

## Addendum (2026-11-26): Workplace setup strings

`WinCreateObject` and `WinSetObjectData` (ADR-0130) bring a parser of setup strings from the program (`KEY=value;…`, `^;` a semicolon) and the joining of queued strings, in `src/loader/wps_setup.rs`, which builds alone. The `wps_setup` target's first byte splits the input into a key and a string. It checks that scanning never panics, that an escaped value scans back, that the next queued item is still found and that a later occurrence wins, and that a title is always followed safely. Within its first minute it found two problems:
- `append` took a value ending in an escaped `^;` for the end of an item, joining it to the next one;
- a value ending in `^` cannot be followed by another item at all, so `WinCreateObject` drops a title's trailing carets.

After the fixes it ran 1.79 million inputs in two minutes without finding anything. `seed.sh` writes one setup string.

## Addendum (2026-11-25): the resolver

The TCP/IP databases and resolver (v0.0.18) brought parsers of a DNS answer from the network, of the host's `/etc/hosts`, `/etc/networks` and `/etc/resolv.conf`, and of names and address strings from the program. `resolver.rs` was already pure; the address and RFC 1035 name parsers move out of `tcpip_util.rs` into `inet_names.rs` so that both build alone. The `resolver` target's first byte picks the input: a DNS message (every name in it expanded and skipped: `dn_skipname` must agree with `dn_expand`, no name over 255 bytes), each of the three files (within their bounds), or a string (a packed name expands back to itself, a parsed address prints and parses back, a query is no answer). In its first minute it found `expand_name` computing the length a name takes with `unwrap_or` / `get_or_insert`, whose argument is evaluated even when a pointer had already set it: past a backward pointer it underflowed, a panic in debug builds. With `unwrap_or_else` / `get_or_insert_with`, two minutes, 3.48 million inputs, nothing found; `seed.sh` writes one input of each kind.

## Addendum (2026-11-27): INF / HLP files

The INF / HLP reader (ADR-0132) parses files a program or a user supplies, so `src/loader/ipf_file.rs` builds alone and has an `ipf_file` target: parsing the header and table of contents never panics, gives as many entries as the header names, and keeps each within its size byte. `seed.sh` compiles Warpine's own `tests/ipf/book.ipf` with Open Watcom's `wipfc` as the seed. 24 million inputs in two minutes found nothing.
