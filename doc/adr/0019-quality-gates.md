# ADR-0019: Gate every commit on tests and lint-free code with a pinned toolchain

- **Status:** Accepted
- **Date:** 2026-03-31 (pre-commit hook); 2026-11-15 (pinned toolchain)
- **Sources:** commits 46c8460 ("enforce cargo test in pre-commit hook"), 24ddc05 ("pin Rust 1.100.0 via rust-toolchain.toml"); `githooks/pre-commit`; `rust-toolchain.toml`; `CLAUDE.md`

## Context

Warpine is security-sensitive (it runs untrusted guest code under a hypervisor) and changes quickly. New Rust releases add lints and change behaviour.

## Decision

- The pre-commit hook (`githooks/pre-commit`) runs `cargo test` and `cargo clippy --all-targets -- -D warnings`; a commit with a failing test or any warning is refused. Hooks are never bypassed.
- The Rust toolchain is pinned in `rust-toolchain.toml` (1.100.0, with clippy and rustfmt); guest crates use `cargo +nightly`, which overrides the pin.
- Commits follow Conventional Commits and are made by topic.

## Consequences

- `main` always builds, passes its tests and is lint-free.
- Toolchain upgrades are deliberate changes that fix new lints in the same step.
- Commits take longer because the full test suite runs each time.
