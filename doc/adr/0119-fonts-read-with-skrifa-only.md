# ADR-0119: Fonts read with `skrifa` only

- **Status:** Accepted
- **Date:** 2026-11-23
- **Amends:** ADR-0047 (light hinting with `skrifa`): what it left on `ab_glyph` moves to `skrifa` too.

## Context

PM text came from two font libraries: `skrifa` drew hinted Latin glyphs (ADR-0047), and `ab_glyph` did everything else — glyph lookup, advances and metrics, the unhinted CJK glyphs and the glyphs drawn under a transform (`glyph_transform.rs`). `ab_glyph` reads fonts with `ttf-parser`, which RustSec reports as unmaintained (RUSTSEC-2026-0192); `deny.toml` carried an ignore for it with a written reason. One parser is less supply-chain surface than two.

## Decision

1. **`gui/face.rs`** gives the font code the subset of `ab_glyph`'s calls it used, read with `skrifa` and computed with `ab_glyph`'s arithmetic: the scale factor `px / (ascent − descent)`; advances from `hmtx`; outlines collected through `skrifa`'s pen as lines, quadratic and cubic curves in font units, each contour closed back to its start, bounded by all their points; pixel bounds rounded with the subpixel fraction as `ab_glyph` rounds them; coverage from `ab_glyph_rasterizer`, which `ab_glyph` used itself and which needs no font parser.
2. `ab_glyph` is dropped; `ttf-parser` leaves the dependency tree, and the advisory ignore leaves `deny.toml`.
3. The test of this decision is the golden pictures: every one of them — Latin, CJK, transformed and rotated text — stays pel for pel the same.

## Alternatives considered

- **Keeping `ab_glyph` with the advisory ignored.** It parses only the embedded IBM Plex fonts, so the risk was small, but it stays a second parser to watch.
- **`swash` or another full text stack.** More capability than Warpine uses, and its rasteriser would change pels.

## Consequences

- 80 crates instead of 83; `cargo audit` and `cargo deny` report nothing.
- Every glyph path now runs on one parser; `face.rs` unit tests check contour closing, point bounds and pixel bounds.
