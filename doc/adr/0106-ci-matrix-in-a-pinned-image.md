# ADR-0106: CI: the test matrix in a pinned image, on GitHub's runners and on a workstation alike

- **Status:** Accepted; builds on ADR-0101 (pinned compilers), ADR-0066 (pinned third-party inputs); amended by ADR-0112 (the watcom entry also runs `cargo deny check` with advisories, and fuzzes; the image gains cargo-deny and the fuzz toolchain)
- **Date:** 2026-11-21
- **Sources:**
  - GitHub's documentation of Actions on Linux runners: KVM on the standard runners (a udev rule gives the runner's user `/dev/kvm`), Podman on `ubuntu-24.04`.
  - Debian's snapshot archive, including `debian-security`.
  - rustup's release archive and its published SHA-256 files.

## Context

The project had no CI. The pre-commit hook runs the unit and integration tests against Open Watcom's sample builds. Since ADR-0101 the integration suite can also run against every C sample built by GCC and by clang (`WARPINE_SAMPLES`), but only by hand.

The tests need KVM, SDL3, GDB, the pinned Open Watcom snapshot and the pinned compilers. Third-party inputs must be pinned and verified (ADR-0066), and that includes the CI's own.

## Decision

1. **One script per matrix entry, run in a pinned image** (`ci/matrix.sh watcom|gcc|clang`):
   - **`watcom`:** fetch the pinned Open Watcom snapshot, build its samples, run every test, clippy, and the `gen_api` / `gen_arch` checks;
   - **`gcc` / `clang`:** build every C sample with that compiler (`OUT_TAG=ci-<cc>`), and run the integration tests on those builds (`WARPINE_SAMPLES=ci-<cc>`; `sample_path` takes the `ci-` prefix);
   - every entry requires `/dev/kvm` and fails when a Warpine process is left after the tests.
2. **The image** (`vendor/setup_ci_image.sh`, `localhost/warpine-ci:<suite>-<snapshot>-rust<version>`):
   - the Debian base digest and snapshot timestamp of `vendor/setup_compilers.sh`, so the same GCC 14.2 and clang 19.1.7;
   - the snapshot's updates and security suites too, since the base image already carries their updates;
   - SDL3, pkg-config, GDB, curl, xz;
   - `rustup-init` 1.29.3 checked against its SHA-256, installing the toolchain `rust-toolchain.toml` names.
3. **`ci/run.sh [entries]`** builds the image if needed and runs the entries in it:
   - with `/dev/kvm`, the repository at its own path and the caller's user id;
   - with cargo's output in `target/ci`, apart from a workstation's own builds (`os2.mk` and `build_sdk.sh` follow `CARGO_TARGET_DIR`);
   - with the network on, for the crates `Cargo.lock` pins and the Open Watcom snapshot.
4. **The workflow** (`.github/workflows/ci.yml`) runs the three entries on `ubuntu-24.04`:
   - on pushes to `main`, pull requests, and by hand;
   - with read-only permissions and no third-party actions: the checkout is plain `git fetch` of the commit;
   - it gives the runner's user `/dev/kvm` with a udev rule, then calls `ci/run.sh <entry>`.

## Warpine's choices (unverified)

- **Untested on GitHub:** whether GitHub's runners give enough memory and time for the three entries (150 minutes each). The entries were run locally with `ci/run.sh` in the same image.
- **No caching** between runs: each builds the image and the crates from scratch.
- **The pre-commit hook stays** as it is; CI adds the GCC and clang builds and a clean environment.

## Consequences

- The GCC and clang sample builds are tested on every change, in an environment pinned like the compilers themselves. `ci/run.sh` reproduces a CI failure on a workstation.
- **Problems found while setting it up:**
  - `samples/build_sdk.sh` reported `cxx_test` as a failure (fixed; see ADR-0107);
  - the tool paths in `os2.mk`, `build_sdk.sh` and `samples/c_hello` ignored `CARGO_TARGET_DIR`;
  - `WARPINE_COUNTRY` set only the country code, while the date and number formats followed the host's `LANG`, so `pm_container_test`'s golden depended on the developer's locale. A country now decides its formats, and the golden is recorded with country 1's;
  - GDB 16 (the image's) reads the symbols of the DLLs in its first library list only after `sharedlibrary` (ADR-0104).
- **The first run on GitHub** failed the `gcc` and `clang` entries, on gaps a workstation's leftovers had hidden:
  - `pm_pointer_test`'s pointer files are written by `make_ptr.py`, which only its Open Watcom Makefile ran; `build_sdk.sh` now runs it too;
  - `test_ne_hello` and `test_som_samples_run_on_the_som_runtime` ran Open Watcom builds that a `gcc` / `clang` entry does not have; they now skip without them, like the other tests.

  The `gcc` entry was then run from a fresh worktree with `ci/run.sh`, which reproduces a clean checkout.
- `.gitignore` ignores the debug companions (`*.dbg`) and the message files built in `samples/c_msg_test`.
