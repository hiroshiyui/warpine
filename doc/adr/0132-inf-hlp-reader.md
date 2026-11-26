# ADR-0132: Reading INF / HLP help files: Open Watcom's wipfc as specification and oracle

- **Status:** Accepted
- **Date:** 2026-11-27
- **Sources:** Open Watcom's IPF compiler `wipfc` (`vendor/watcom/binl64/wipfc`, and its source in the Open Watcom repository, Sybase Open Watcom Public License); community write-ups of the INF format and NewView's documentation, each used after a licence check; a local corpus from the Hobbes OS/2 archive (`doc/references/ipf-corpus/`, gitignored)

## Context

OS/2's help files (INF books and HLP help libraries, compiled from IPF source) are what the Help Manager and `VIEW.EXE` display, and what `CMDREF.INF` and other references come in. IBM never published the format, so it sat among the blockers (TODOs): no help viewer, no F1 panels, and questions such as `THREADS=` left unanswered because `CMDREF.INF` cannot be read.

Warpine's policy takes behaviour from public documentation only and uses no IBM binaries.

## Decision

- **The specification is a public writer.** Open Watcom ships `wipfc`, an IPF compiler with open source. How it writes each INF / HLP structure is the public specification Warpine follows; community write-ups and NewView's documentation fill gaps, each only after its licence is checked.
- **The oracle is files Warpine makes.** Test files are compiled by the pinned `wipfc` from IPF sources written for Warpine, as `os2rc` is checked against Open Watcom's `wrc` (ADR-0099). They are ours to commit, and every byte has a known source.
- **A local cross-check corpus.** Real-world help files compiled by IBM's IPFC in the 1990s, with their IPF sources where the package shipped them, from the Hobbes archive (third-party packages, not IBM's), in `doc/references/ipf-corpus/` (gitignored). They test that the reader also takes files from the other writer. Their licences vary, so none is committed; IBM's own INF files are not used.
- **Parser first.** A pure parser of guest- and file-supplied data, in its own module with no `Loader` (like `ea_list.rs`), with a fuzz target (ADR-0112). The viewer, the Help Manager's panels and `Ddf*` come after it, each in its own step.

## Consequences

- The IPF format leaves the blockers: the help viewer, F1 help and reading `CMDREF.INF` become ordinary work.
- What `wipfc` does not write (features only IBM's compiler used) shows up in the corpus first; such parts are read where a public source describes them, else skipped and recorded.
- Unverified: wherever `wipfc` and IBM's compiler differ, Warpine follows what it can document and records the choice.

## Addendum (2026-11-27): the header and the table of contents

`src/loader/ipf_file.rs` reads the header (155 bytes, `header.hpp`'s order) and the table of contents (each entry through the offset table, an extended entry's optional parts skipped by its flags, `page.cpp`'s order), and `warpine --inf-toc` lists them. Checked against a book `wipfc` compiles from `tests/ipf/book.ipf` (integration test) and, locally, against IBM-compiled files of the corpus: `texi2ipf.INF` (15 entries), `listpm.HLP` (35) and `OCL.INF` (205) list as expected. Next: the dictionary and the cells, then panel text.
