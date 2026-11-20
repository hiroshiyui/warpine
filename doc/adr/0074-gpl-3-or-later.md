# ADR-0074: License Warpine under GPL-3.0-or-later

- **Status:** Accepted; supersedes ADR-0002 (GPL-3.0-only); ADR-0076 settles its open question on the guest crates (they stay GPL)
- **Date:** 2026-11-20
- **Sources:** `LICENSE` (the GNU GPL, version 3, unchanged); `Cargo.toml`, `crates/*/Cargo.toml`, `samples/rust_hello/Cargo.toml`; `deny.toml`; the SPDX headers of the source files; `README.md`.

## Context

ADR-0002 licensed Warpine under the GNU GPL version 3 only. The project's author chose to allow later versions of the GPL too, so the licence can follow the GPL's own evolution and stays compatible with projects that require "or later".

Two further gaps showed up in an intellectual-property review:
- **Undeclared files.** Many of the project's own files declared no licence at all: sample C, header, resource and C++ binding files, `build.rs`, the guest crates and some scripts.
- **The guest crates.** Their manifests had no `license` field. ADR-0002 put them under the project's licence by default, without saying so in the crates themselves.

Every commit in the history has one human author, the project's author. The other contributors named in commit trailers are AI assistants. So the copyright holder can make this change alone.

## Decision

- **The licence.** Warpine's own code, documentation, samples and guest crates are licensed under the GNU General Public License, version 3 or (at the user's option) any later version: `GPL-3.0-or-later`.
- **Every source file says so.** It carries `SPDX-License-Identifier: GPL-3.0-or-later` in its first lines, in the comment syntax of its language. New files must too.
- **Every manifest says so.** `Cargo.toml`, the guest crates' manifests and `samples/rust_hello/Cargo.toml` declare `license = "GPL-3.0-or-later"`, and the cargo-deny exception for the `warpine` crate allows it.
- **Third-party material keeps its own licence:**
  - IBM Plex: SIL OFL 1.1 (ADR-0012);
  - GNU Unifont: GPL-2.0-or-later with the font embedding exception, or OFL 1.1.

  Both stay compatible with GPL-3.0-or-later, as do the permissive crate licences `deny.toml` allows.
- **What is not covered.** The 4OS2 files still in `samples/4os2` are not Warpine's and are not relicensed. Their removal is planned separately.

## Consequences

- **Distributors** may follow the terms of GPL version 3 or of any later version the Free Software Foundation publishes.
- **The guest crates.** Programs built with them are derived works under the GPL (version 3 or later). Whether these crates should offer a linking exception or a more permissive licence for users' own OS/2 programs was left open here; ADR-0076 keeps them under the GPL.
- **Older history.** Commits before this change carry `GPL-3.0-only` headers. They remain available under those terms.
