# ADR-0027: Apply GPI transforms to each primitive when it is called, with an identity fast path

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** batch 5 (GPI state and coordinate spaces, text layout); `src/loader/pm_xform.rs` (`Affine`, `MatrixLf`, `Xform`, `CoordState`), `src/loader/pm_coords.rs`, `src/loader/pm_psstate.rs`, `src/loader/pm_gpi.rs` (`PsTarget::xform`), `src/loader/pm_text.rs`; IBM *Presentation Manager Programming Reference* and *GPI Guide* (coordinate spaces, `GpiSetModelTransformMatrix`, `GpiSetPageViewport`, `GpiConvert`); ADR-0011, ADR-0024

## Context

GPI draws in world coordinates. Model, viewing and default-view transforms, page units and the page viewport map them to device pels. Until batch 5, Warpine had no transforms: world, page and device coordinates were all the pels of the surface. Every drawing primitive, the clip machinery (ADR-0024) and dozens of golden pictures relied on that.

There were two candidate places for the transform. One was the surface rasteriser: give `DrawOp`s world coordinates and a matrix. The other was the primitives: map points before building a `DrawOp`. Some calls take world coordinates and others device coordinates, so either way the mapping depends on the call.

## Decision

- **Each primitive maps its points when it is called.** `ps_target` composes `CoordState::world_to_device()` into `PsTarget::xform`, and the primitive maps its world points before building a `DrawOp`. `DrawOp`s and the surface stay in device pels.
- **Identity fast path:** with the default state, points pass through untouched, with no rounding or clamping, so drawing without transforms is bit-for-bit what it was.
- **Non-identity maps:**
  - Results are rounded half away from zero and clamped to ±2²⁷, the region limit.
  - Matrices are stored as `MATRIXLF` (FIXED / LONG); a composition that overflows fails with `PMERR_COORDINATE_OVERFLOW`.
- **Shapes:**
  - Boxes stay boxes while the map is axis-aligned and become four-point shapes otherwise.
  - Arcs are generated in device space from the mapped parameters.
  - Curves are flattened from mapped control points.
  - Area and path figures are stored in device pels.
- **The current position stays in world coordinates.** Text maps its start point, lays its glyphs out in device pels from there, and maps the end point back. Glyphs, increments and the character box are not transformed; character angle and shear are their own attributes.
- **Which coordinates each call takes:**
  - World: pels, the pattern reference point, clip rectangles, `GpiQueryClipBox`, and `GpiWCBitBlt` / `GpiDrawBits` targets.
  - Device: regions, `GpiBitBlt`, `WinFillRect`, `WinDrawText` and `WinDrawBorder`.
- **`GpiSavePS` saves a clone of the whole presentation space,** so attributes added later are saved and restored without extra code.

## Consequences

- All existing pictures and tests are unchanged, and transformed drawing reuses the existing rasterisers and clipping.
- Some things are approximated: line widths are not scaled, glyphs and images are not rotated, and rotated limits or clip rectangles become their bounding boxes (`doc/TODOs.md`).
- A new primitive must map its points through `PsTarget::xform`, and its documentation must say whether its arguments are world or device coordinates.
