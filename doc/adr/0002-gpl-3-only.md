# ADR-0002: License Warpine under GPL-3.0-only

- **Status:** Accepted
- **Date:** 2026-03-12
- **Sources:** commit e01df5f ("legal: adopt GPLv3 license"); `LICENSE`; `CLAUDE.md`

## Context

Warpine is a compatibility layer, comparable to WINE. Its source and any derived work should stay open.

## Decision

Warpine is licensed under the GNU General Public License, version 3 only (`GPL-3.0-only`). Source files carry `SPDX-License-Identifier: GPL-3.0-only`, and new ones should too.

## Consequences

- Derived works distributed to others must be published under the same licence.
- Vendored third-party material keeps its own licence and must be compatible: IBM Plex (SIL OFL 1.1, ADR-0012), GNU Unifont.
- Sample programs and guest crates in this repository are under the same licence unless they say otherwise.
