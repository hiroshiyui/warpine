# ADR-0024: Represent regions as y-bands of x spans and clip by saving and restoring pels

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** commit d3c5c4d; `src/gui/region.rs`, `src/gui/surface.rs` (`Surface::draw_clipped`), `src/loader/pm_region.rs`, `src/loader/pm_area.rs`; IBM *Presentation Manager Programming Reference* (GPI regions, clipping, paths); ADR-0011

## Context

GPI regions (`HRGN`) are sets of pels built from rectangles, combined with OR / AND / XOR / DIFF, queried as rectangle lists, painted, framed and used as clip regions; clip paths and `GpiPathToRegion` turn arbitrary shapes into them; windows have update regions. Every drawing call through a presentation space must respect the clip: lines, areas with patterns and mixes, text, bit blits, `WinFillRect`, `WinDrawText`, `WinDrawBorder`.

Warpine's drawing primitives (`DrawOp`, ADR-0011) clip only to the surface. Teaching each primitive to clip against an arbitrary region would touch every rasteriser and every future one.

## Decision

- **A region is a canonical list of y-bands, each a sorted list of disjoint x spans** (`gui/region.rs`), exclusive at the right and top. Canonical form (adjacent equal bands merged, empty bands dropped) makes equality, `RGN_RECT` / `RGN_COMPLEX` and rectangle enumeration straightforward. Shapes become regions by scan conversion with the area fill rules.
- **Clipping wraps the unchanged draw** (`Surface::draw_clipped`): the area an operation can touch is saved, the operation applied, and the pels outside the clip put back. An unclipped draw, or one whose clip holds the whole area, costs nothing extra; one whose clip misses it draws nothing.
- Regions are bounded: coordinates within ±2²⁷ (the range of a `GPIF_LONG` presentation space, so arithmetic cannot overflow), a maximum number of rectangles per region and of regions per process; a result past a bound fails with the documented `PMERR_*` instead of growing host memory.

## Consequences

- Every present and future `DrawOp` is clipped pel-exactly, mixes and patterns included, without knowing about regions.
- A clipped draw costs a save and a restore of its bounding area, which is fine for GUI drawing but would be slow for many tiny draws through a complex clip.
- Window update areas remain rectangles internally; the region calls on windows (`WinInvalidateRegion`, `WinQueryUpdateRegion` …) convert. True update regions and clipping the `WinBeginPaint` presentation space to them are follow-ups (`doc/TODOs.md`).
- Elliptic, rounded-rectangle and polygon region constructors can be added as scan conversions on the same representation.
