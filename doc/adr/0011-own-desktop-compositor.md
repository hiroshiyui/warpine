# ADR-0011: Draw the PM desktop in one host window with Warpine's own compositor

- **Status:** Accepted
- **Date:** 2026-04-02 (single surface); 2026-11-16 (compositor trait, layered compositor)
- **Sources:** commits 086ae2d, a559b70 (VDR), cd76808 (layered compositor); developer guide (Rendering Pipeline); `src/gui/surface.rs`, `src/gui/sdl_renderer/compositor.rs`

## Context

PM programs need overlapping frames, chrome, a Z-order, focus and read-back of drawn pels (`GpiQueryPel`). One host window per PM window would leave Z-order, chrome and read-back to the host window manager, which differs between desktops.

## Decision

Warpine shows one SDL desktop window and draws the PM desktop in it itself (the Virtual Desktop Renderer).

- Each PM frame has a `Surface` shared between the API threads and the GUI thread. GPI, `WinFillRect` and built-in controls rasterise on the calling API thread (`Loader::draw`), so read-back sees what was drawn; the lock order is window manager, then surface.
- Window chrome (title bar, borders, buttons, menu bar, pull-downs) is drawn by the compositor, not into client surfaces.
- Compositing sits behind the `Compositor` trait: `SoftwareCompositor` (damage-tracked CPU path, default on the dummy and offscreen drivers, reference for golden tests) and `LayeredCompositor` (one texture per frame, SDL's accelerated renderer, default on real displays); `WARPINE_COMPOSITOR` overrides.

## Consequences

- PM behaviour (Z-order, activation, chrome, min / max, system menu) is under Warpine's control and testable headless.
- Warpine must implement what a window manager would otherwise provide.
- Drawing performance depends on Warpine's damage tracking and compositor.
