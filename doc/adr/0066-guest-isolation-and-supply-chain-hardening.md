# ADR-0066: Guests cannot touch the host environment or host Unix sockets; downloads and toolchains are pinned and verified; a cargo-deny policy guards dependencies

- **Status:** Accepted; its 4OS2 pin is moot since ADR-0075 removed 4OS2
- **Date:** 2026-11-19
- **Sources:**
  - Guest isolation:
    - `src/loader/cmd.rs` (the shell's own environment);
    - `src/loader/tcpip.rs` (`check_socket_kind`, `check_sockaddr_in`, `read_sockaddr_in`);
    - `src/loader/vcpu.rs` (`guest_stack_ok`, `MIN_GUEST_ESP`);
    - `src/loader/ne_exec.rs`;
    - `src/loader/pm_container.rs` (`MAX_DETAIL_WIDTH`).
  - Supply chain:
    - `Cargo.toml` (`object` features), `deny.toml`, `clippy.toml`, `githooks/pre-commit`;
    - `vendor/setup_watcom.sh`, `samples/4os2/fetch_source.sh`;
    - `vendor/unifont/README.md`;
    - the `rust-toolchain.toml` files of `crates/*` and `samples/rust_hello`;
    - `src/bin/lx_link.rs` (`zeroed_section`).
  - A security audit on 2026-11-19 (`cargo audit`, `cargo deny`, a review of every `unsafe` block and guest-input path).

## Context

A security audit found two ways out of the guest sandbox:

1. **Changing the host environment.** The built-in CMD.EXE runs inside the Warpine process. Its `SET` changed that process's environment with `std::env::set_var`. A guest could set `LD_PRELOAD` to a library it wrote, and every program Warpine started afterwards would load it as native code. `SET` alone also listed the host's environment to the guest: tokens, agent sockets, paths.
2. **Host Unix sockets.** `socket()` passed the guest's address family to the host, and `bind` / `connect` / `sendto` passed its raw address bytes. A guest could reach the host's Unix sockets:
   - the session D-Bus, which can start units;
   - X11;
   - `docker.sock`;
   - Warpine's own desktop and pipe sockets.

   `bind` could also create socket files outside the drives.

It also found smaller problems:
- **A guest stack pointer too low** for the frames the host builds below it made the host panic. That crashed only the guest's own process.
- **Huge `FIELDINFO.cxWidth` values** overflowed the container's column arithmetic.

And it found supply-chain weaknesses:
- **A vulnerable decompressor.** `ruzstd` (RUSTSEC-2026-0344), a decompression bomb, came in through `object`'s default features.
- **An unverified compiler download.** The Open Watcom download was the moving `Current-build`, unverified.
- **A weak pin.** The 4OS2 source was pinned by a 7-character prefix.
- **An unrecorded font.** The vendored Unifont had no recorded source and matched no published release.
- **A floating toolchain.** The Rust guest toolchain was the floating `nightly`.
- **No policy.** There was no dependency policy, and build outputs (`.o`, `.res`) were tracked.

## Decision

### Guest isolation

- **The shell keeps its own OS/2 environment.**
  - It starts from the caller's environment block, or, on the terminal path, from Warpine's default OS/2 environment.
  - It is changed only by `SET`, listed only by `SET`, and given to the programs it starts through `WARPINE_OS2ENV`.
  - It is bounded: 1024 variables, 64 KB.
  - A `.CMD` script shares it, so its `SET`s stay, as in OS/2.
- **Warpine never changes its process environment.** `clippy.toml` forbids `std::env::set_var` / `remove_var` throughout the crate, so this cannot come back unnoticed.
  - Child processes keep the host variables the user started Warpine with. Stripping `LD_*` would protect nothing once guests cannot set them, and would break users who need `LD_LIBRARY_PATH`.
- **Guests get Internet sockets only.**
  - `socket()` accepts `AF_INET` with stream, datagram or raw type, and no Linux flag bits.
  - `bind` / `connect` / `sendto` accept an `AF_INET` `sockaddr_in` of at least 16 bytes, and pass exactly 16 bytes to the host.
  - `AF_UNIX`, netlink and the rest are refused with `SOCEAFNOSUPPORT`.
- **The guest's stack pointer is checked where the host builds frames below it.** A 32-bit API call, or a hardware exception about to call a handler, needs an ESP between `MIN_GUEST_ESP` (8 KB, more than the largest host frame) and the top of guest memory.
  - At API entry, outside that range the process ends as an unhandled exception ends it.
  - On an exception, no handler is called, and the process ends with a crash report.
  - NE address sums wrap explicitly, and are read through the bounded accessors.
- **Container columns are bounded.**
  - A column's width is bounded before it is signed.
  - Column positions and pane contents stop at `MAX_DETAIL_WIDTH` (2²⁴ pels).

### Supply chain

- **Dependencies.**
  - `object` is built with `read_core`, `elf`, `archive` and `std` only. Its decompressors (`ruzstd`, `flate2`) leave the build.
  - `lx_link` refuses compressed or unreadable sections instead of linking zeros, and bounds uninitialized ones to 256 MB.
  - `rustix` is updated (RUSTSEC-2026-0486).
- **The `deny.toml` policy.**
  - Dependencies come from crates.io only; no other registry, no git dependencies.
  - Licences are a fixed list compatible with GPL-3.0-only.
  - Yanked crates and advisories are denied. Any ignored advisory needs a written reason; today only `ttf-parser`'s "unmaintained" is ignored, since it parses only our own embedded fonts.
- **When the checks run.**
  - The pre-commit hook runs the offline checks (sources, bans, licences).
  - Releases run `cargo audit` and the full `cargo deny check`.
- **Downloads.**
  - `vendor/setup_watcom.sh` downloads a dated snapshot (`2026-11-01-Build`) over HTTPS only, checks its SHA-256 (the digest GitHub records for the asset) before extracting, and extracts with `--no-same-owner`. Moving to a newer snapshot means changing the tag and the digest together.
  - `samples/4os2/fetch_source.sh` checks out the full 40-character commit, detached, and refuses the tree unless `HEAD` is that commit. It clones over HTTPS only.
- **Assets and toolchains.**
  - `vendor/unifont/unifont.hex` is now GNU Unifont 17.0.03's plane-0 font, unmodified. It comes from the release tarball, whose signature by the Unifont maintainer was verified against the GNU keyring. `vendor/unifont/README.md` records the source, the hashes and how to update it.
  - The Rust guest crates and `samples/rust_hello` pin `nightly-2026-11-15`. The docs build with `cargo build`, since `cargo +nightly` would bypass the pin.
- **Build outputs.** The `.o` / `.res` files that were tracked are removed from git; `.gitignore` already covered them.

## Consequences

- **Isolation.** A guest can no longer run native code through the environment, read the host's environment, or talk to host Unix sockets.
- **OS/2 programs.**
  - OS/2 programs using Internet sockets are unaffected.
  - Programs using OS/2's local sockets (`AF_OS2` / `AF_UNIX`, rare) now fail; supporting them would need a Warpine-side namespace, not host paths.
- **Changed glyphs.** The Unifont change alters 41 BMP glyphs, mostly Devanagari and five CJK ideographs, to the released 17.0.03 shapes.
- **Upkeep.**
  - Moving to a newer Open Watcom snapshot, Unifont or nightly is a deliberate edit with a new hash or date.
  - A dependency from outside crates.io, or with a new licence, fails the hook until the policy is changed on purpose.
- **Tests:** unit tests for:
  - the shell's environment;
  - the socket checks, including a refused `AF_UNIX` socket and `connect`;
  - the stack bound, with an API call at a tiny ESP ending the process;
  - huge column widths;
  - `lx_link`'s section bound.
