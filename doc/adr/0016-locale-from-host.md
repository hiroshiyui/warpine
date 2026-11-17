# ADR-0016: Take the country and codepage from the host locale

- **Status:** Accepted
- **Date:** 2026-11-17
- **Sources:** commit ce2f9c3 ("take the country and codepage from the host locale"); the maintainer's choice "Follow the host locale" (2026-11-17); `src/loader/locale.rs`

## Context

OS/2 programs read the country (date, time, currency formats) and the process codepage at start. With a fixed US / 437 default, users outside the US saw US formats, and NLS code paths for other countries went untested.

## Decision

The country code and codepage come from the host locale (`resolve_country`); `WARPINE_COUNTRY` and `WARPINE_CODEPAGE` override them. The process, VIO and PM-face codepages start from it. Tests and screenshots pin the US country and codepage 437 (`PINNED_LOCALE`, `Loader::new_mock`) so results do not depend on the machine.

## Consequences

- Programs see the user's country formats and a matching codepage.
- Behaviour can differ between machines; tests must pin the locale.
