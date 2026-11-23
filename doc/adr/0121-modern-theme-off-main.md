# ADR-0121: The modern theme leaves `main` for a feature branch

- **Status:** Accepted
- **Date:** 2026-11-23
- **Withdraws:** ADR-0118

## Context

ADR-0118 added themes: `classic`, the default, and an experimental `modern` one with rounded, shadowed frames, menus and controls, chosen with `WARPINE_THEME`. Warpine still lacks PM widgets of its own (the help viewer, parts of containers, notebooks, drag images, sliders); every one of them would need drawing twice, and a theme only some widgets follow looks unfinished. The user decided that the PM widgets come first, and the modern theme leaves `main`.

## Decision

1. The modern theme is kept, unchanged, on the branch `feature/modern-theme` (from `703782d`, the last commit with it), to come back when the widgets are done.
2. `main` has one look, the former `classic`: `gui/theme.rs`, `gui/frame_shape.rs`, every modern drawing path (frames, title bars, controls, menus, cascades, scroll bars, sliders, the circular slider, the push-button hover), the rounded-picture shaping of the three compositors and the modern golden pictures are removed; `WARPINE_THEME` is no longer read.
3. The desktop protocol drops `Item::rounded`; its version becomes 12 (11 was the theme's), so a client and a server of different builds refuse each other.

## Consequences

- Pels on `main` are the classic ones; no classic golden picture changes.
- The cascades, `WM_MEASUREITEM` and the circular slider stay, drawn in the classic look.
- Bringing the theme back means rebasing `feature/modern-theme` on `main` and drawing the widgets added meanwhile in both looks.
