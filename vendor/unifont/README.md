# GNU Unifont (vendored)

`unifont.hex` holds the 8×16 and 16×16 glyphs of Unicode's Basic
Multilingual Plane. `build.rs` turns it into the VIO text-mode font tables,
and it is the last fallback of the PM fonts (`src/gui/font.rs`).

## Source

- Release: GNU Unifont 17.0.03, <https://ftp.gnu.org/gnu/unifont/unifont-17.0.03/>
- File: `unifont-17.0.03/font/precompiled/unifont-17.0.03.hex` from
  `unifont-17.0.03.tar.gz`, unmodified.
- `unifont-17.0.03.tar.gz` SHA-256:
  `9a26aa9adfa8eb1f91b0cd9b83e7f95ea9e14c6e85be71aa3ab0df5cb4e69c35`.
  It is signed (`unifont-17.0.03.tar.gz.sig`) by Paul Hardy, the Unifont
  maintainer: RSA key `95D2 E9AB 8740 D804 6387 FD15 1A09 227B 1F43 5A33`,
  from the GNU keyring (<https://ftp.gnu.org/gnu/gnu-keyring.gpg>). The
  signature was checked on 2026-11-19; the key has expired since it signed.
- `unifont.hex` SHA-256:
  `30fe137f60dd95e8be1d499853e24dbbb35621bcd5d8328d8f310c4e2c5ab274`.

To update it, download the new release's tarball and its `.sig`, verify
the signature against the GNU keyring, copy
`font/precompiled/unifont-<version>.hex` here unchanged, and update this file.

## Licence

GNU Unifont is dual-licensed under the GNU General Public License,
version 2 or later, with the GNU font embedding exception, and the SIL Open
Font License 1.1.

- `LICENSE.txt`: the "Unifont AUTHORS" and "LICENSE" sections of the
  release's README, unchanged.
- `OFL-1.1.txt`: the release's `OFL-1.1.txt`, unchanged.

Both are compiled into the `warpine` binary and printed by
`warpine --licenses` (ADR-0076). Copy them again from each new release.
