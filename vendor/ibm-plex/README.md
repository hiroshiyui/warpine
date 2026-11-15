# IBM Plex (vendored)

IBM Plex is the typeface of Warpine's Presentation Manager text: window
chrome, menus, dialogs, built-in controls and GPI text drawn by guest
applications. The font files in this directory are embedded in the `warpine`
binary by `src/gui/font.rs` (`include_bytes!`), so rendering is identical on
every host and no fonts need to be installed.

## Source

- Repository: <https://github.com/IBM/plex>
- Commit: `bc6a7e4b37634eb79785d59781893db63ee3edcb`
- Paths: `packages/plex-{sans,mono,serif}/fonts/complete/ttf/` and
  `packages/plex-sans-{jp,kr,tc,sc}/fonts/complete/ttf/unhinted/`

The files are unmodified copies.

| Files | Use |
|---|---|
| `IBMPlexSans-{Regular,Bold,Italic,BoldItalic}.ttf` | Default PM font (System Proportional, WarpSans, Helv, …) |
| `IBMPlexMono-{Regular,Bold,Italic,BoldItalic}.ttf` | Fixed-pitch faces (Courier, System Monospaced, System VIO, …) |
| `IBMPlexSerif-{Regular,Bold,Italic,BoldItalic}.ttf` | Serif faces (Tms Rmn, Times New Roman, …) |
| `IBMPlexSans{JP,KR,TC,SC}-Regular.ttf` | CJK text; the face order follows the active DBCS codepage |

Characters found in none of these faces fall back to the GNU Unifont bitmap
glyphs in `vendor/unifont`.

## Licence

Copyright © 2017 IBM Corp. with Reserved Font Name "Plex". Licensed under the
SIL Open Font License, Version 1.1 — see [`LICENSE.txt`](LICENSE.txt). The
fonts are bundled with Warpine as a separate work and are not covered by
Warpine's GPL-3.0-only licence.

## Updating

Replace the `.ttf` files with the same-named files from a newer IBM/plex
commit, update the commit hash above, and run `cargo test` (the
`gui::font` tests check that every face parses and that the expected glyph
coverage is present).
