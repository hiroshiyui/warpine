# ADR-0047: Light hinting of Latin text with skrifa, and a contrast curve

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** `src/gui/font.rs` (`rasterize_hinted`, `hint_face`, `PathPen`, `contrast_coverage`, `pen_phase`), `src/gui/glyph_transform.rs`; `Cargo.toml` (`skrifa`, `ab_glyph_rasterizer`); FreeType's `FT_LOAD_TARGET_LIGHT` as the model; the comparison renderings made with FreeType when this was decided

## Context

PM text is IBM Plex, rasterised by `ab_glyph`, which does not hint. At the sizes PM uses (the 16-px system font has an em of about 12.3 px), the baseline, x-height and horizontal strokes land between pixel rows. They smear over two rows of grey, and the text looks blurry. Plain linear coverage also makes thin stems light grey.

Glyphs are placed at quarter-pixel pen positions so that the spacing follows the font's advances. Full (Windows-like) hinting would also move glyphs horizontally and change their advances, and with them every measured text width (`GpiQueryTextBox`, control layouts, `fit_text`).

Comparison renderings of the same text with FreeType at Warpine's size were made in five modes:
1. no hinting;
2. no hinting with higher contrast;
3. light hinting;
4. light hinting with higher contrast;
5. full hinting.

Mode 4 was chosen.

## Decision

- **Light, vertical-only hinting of the Latin faces through `skrifa`.**
  - `skrifa` is the Google Fontations scaler: pure Rust, `#![forbid(unsafe_code)]`, MIT OR Apache-2.0.
  - Its autohinter runs with `Target::Smooth { mode: Light, preserve_linear_metrics: true }`. This snaps vertical metrics and stems to the pixel grid and leaves horizontal positions and advances alone, like FreeType's `FT_LOAD_TARGET_LIGHT`.
  - Each face's `GlyphStyles` are computed once. Hinting instances are cached per thread by face and size.
  - The hinted outline from `skrifa`'s pen is filled by `ab_glyph_rasterizer` at the glyph's quarter-pixel phase.
  - Advances still come from `ab_glyph`, unhinted, so measurement and layout do not change.
- **The CJK faces stay unhinted.** The autohinter's per-font analysis of tens of thousands of glyphs would cost too much, and hinting matters less for those shapes. Transformed glyphs (rotated or sheared GPI text) cannot be hinted. Both use `ab_glyph`'s rasterizer, as before.
- **A contrast curve** `c^(1 / 1.45)` is applied to the coverage of every outline glyph (`TEXT_CONTRAST`, `contrast_coverage`), so anti-aliased edges read as ink.
- **Fallback.** If hinting fails for a glyph (no `skrifa` face, a size out of range, a draw error), the unhinted `ab_glyph` raster is used.

## Consequences

- Text is crisper and darker. Every golden picture with text, and the screenshots, were regenerated in the same change.
- Text widths, `GpiQueryTextBox` and layouts are unchanged.
- Upright and transformed Latin text differ slightly, since only the first is hinted.
- New dependencies:
  - `skrifa` and `read-fonts`, which forbid unsafe code;
  - `font-types`, which has a few `bytemuck` casts;
  - `yoke`, `zerofrom` and `smallvec`;
  - `ab_glyph_rasterizer`, used directly now (it was already a dependency of `ab_glyph`).
- The first text at a new size builds a hinting instance. The first use of each Latin face computes its glyph styles once per process.
- Not done:
  - GPOS kerning (see the TODOs), which `skrifa` does not apply either; it needs a shaper;
  - hinting the CJK faces;
  - a gamma-correct (linear-light) blend.
