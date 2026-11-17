# ADR-0021: Generate the architecture diagram from a checked layer map

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** the maintainer's request for an architecture diagram generated from a reliable, trustworthy source, and the choice of the hybrid source (2026-11-18); `doc/architecture/layers.toml`; `src/bin/gen_arch.rs`

## Context

Warpine had no architecture diagram. A hand-drawn one drifts from the code as soon as a module is added, renamed or moved, and readers cannot tell. A diagram derived from the code alone (module declarations and `use` dependencies) cannot drift, but shows about a hundred files and their imports rather than the layers, the guest / host boundary and the host services that explain the system.

## Decision

The diagram has two sources, and a tool keeps them consistent:

- `doc/architecture/layers.toml`, written and reviewed by hand: zones (guest, Warpine host process, host system, build tools), the layers in each with the modules that belong to them, external parts, and the flows between them. It is a small subset of TOML, parsed by `gen_arch` itself, so Warpine takes on no new dependency.
- The list of modules under `src/`, which `gen_arch` reads itself.

`gen_arch` (`src/bin/gen_arch.rs`) validates the map against `src/` — every module in exactly one layer, no module named that does not exist, every flow between known nodes — and writes `doc/architecture/warpine.dot`, from which Graphviz renders `warpine.svg` and `warpine.png` (`cargo run --bin gen_arch -- render`; `check` validates and compares). All four files are committed. The unit test `layer_map_matches_src_and_the_committed_diagram` runs the same validation and fails when the committed DOT is not what the map and `src/` generate, so `cargo test` — and with it the pre-commit hook (ADR-0019) — rejects a module added without a layer or a map edited without rendering.

The diagram is shown at the head of `doc/reference_manual.md`, and the `/docs-engineering` pass renders and checks it.

## Consequences

- The diagram cannot silently miss, invent or misplace a module; its grouping and arrows remain a reviewed judgement, like the rest of the documentation.
- Adding, renaming or removing a module under `src/` requires updating `layers.toml` and running `render` in the same change.
- Rendering needs Graphviz (`dot`); validation and the test do not, so `cargo test` works without it. The committed SVG / PNG can be stale only if the DOT was edited by hand, which its header forbids.
- Flows are not checked against the code's real call graph; they describe the architecture, not every import.
