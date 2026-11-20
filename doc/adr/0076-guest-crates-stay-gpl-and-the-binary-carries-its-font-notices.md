# ADR-0076: The guest crates stay GPL; the binary carries its font notices

- **Status:** Accepted; settles the guest-crate question ADR-0074 left open
- **Date:** 2026-11-20
- **Sources:**
  - SIL Open Font License 1.1, condition 2 (`vendor/ibm-plex/LICENSE.txt`, `vendor/unifont/OFL-1.1.txt`);
  - GNU Unifont 17.0.03's README, sections "Unifont AUTHORS" and "LICENSE", from the release tarball whose SHA-256 `vendor/unifont/README.md` pins;
  - the code: `src/main.rs` (`license_notices`, `--licenses`), `src/gui/font.rs` and `build.rs` (the fonts built into the binary);
  - the guest crates' manifests: `crates/*/Cargo.toml`.

## Context

An intellectual-property review left two licensing questions open after ADR-0074.

1. **The guest crates.** `warpine-os2-sys`, `warpine-os2-rt` and `warpine-os2` are linked statically into every OS/2 program built with them. Under GPL-3.0-or-later, each such program is itself covered by the GPL. ADR-0074 asked whether they should offer a linking exception or a more permissive licence. The options weighed were:
   - MPL-2.0 (file-level copyleft);
   - MIT OR Apache-2.0;
   - LGPL-3.0-or-later;
   - keeping the GPL.
2. **The fonts in the binary.** Every `warpine` binary embeds the IBM Plex fonts (`include_bytes!` in `font.rs`) and glyph tables built from GNU Unifont (`build.rs`).
   - The OFL lets the fonts be redistributed, bundled or embedded "provided that each copy contains the above copyright notice and this license", as text files, headers or metadata fields "easily viewed by the user".
   - Unifont's other licence, the GPL version 2 or later with the font embedding exception, also expects its notices to travel with it.
   - The repository carried IBM Plex's notice, but not Unifont's notice or licence text. A binary passed on alone, by anyone who builds Warpine, carried no notices at all.

## Decision

- **The guest crates stay GPL-3.0-or-later.** Programs built with them are covered by the GPL (version 3 or later). This keeps the platform's own runtime and what is built on it Free Software, in line with the project's goal. A program that must not be GPL can still be written against the OS/2 API with another toolchain (Open Watcom's headers and libraries); Warpine runs it either way.
- **The binary carries its notices.** `warpine --licenses` prints, from text compiled into the binary (`include_str!`):
  - Warpine's licence notice;
  - IBM Plex's copyright notice and the OFL;
  - GNU Unifont's authors and licence statement (GPL-2.0-or-later with the font embedding exception), and the OFL under which it is also available.
- **The repository carries Unifont's notices.** `vendor/unifont/LICENSE.txt` holds the "Unifont AUTHORS" and "LICENSE" sections of the release's README, copied unchanged. `vendor/unifont/OFL-1.1.txt` is the release's `OFL-1.1.txt`, unchanged.

## Consequences

- **Every copy of the binary** shows the fonts' notices on request, whoever builds or passes it on. A future binary release should also ship `LICENSE` and the two `vendor/*/` licence files next to it.
- **Guest programs.** People who build OS/2 programs with Warpine's Rust crates distribute them under the GPL. Changing this later needs a new ADR. The project's copyright holder can do it, as for ADR-0074.
- **Updating Unifont** now also means re-copying those two README sections and `OFL-1.1.txt` from the new release (`vendor/unifont/README.md`).
- **Tests:**
  - `license_notices_carry_every_font_notice_and_licence` (`main.rs`);
  - `test_licenses_switch_prints_the_built_in_font_notices` (`tests/integration.rs`).
