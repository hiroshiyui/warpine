---
name: docs-engineering
description: Audit and update all project documentation to stay in sync with the current development status.
---

When performing documentation engineering, always follow these steps:

1. **Survey recent changes** — run `git log --oneline -20` to identify what has changed since the last documentation pass, then `git diff <prev-tag>..HEAD` (or `git show` per commit) to read the actual diffs. Read affected source files directly — do not rely on commit messages alone to understand what changed. Note which features, APIs, constants, and behaviors are new, removed, or modified.

2. **Audit all documentation** against the current codebase. Review each of the following without exception:
   - `README.md` — feature list, prerequisites, build instructions, acknowledgements
   - `CHANGELOG.md` — release notes and version history; confirm the latest entry covers all recent commits and nothing is missing
   - `CLAUDE.md` — architecture overview, key constants, project conventions, test counts, and API entry-point counts; all numeric claims must be verified (see step 3)
   - `doc/developer_guide.md` — implementation details, VFS/architecture notes, module descriptions
   - `doc/os2_ordinals.md` — ordinal registry; verify newly implemented APIs are listed and stubs are clearly marked
   - `doc/TODOs.md` — roadmap items; identify what is completed vs. still pending
   - `doc/reference_manual.md` — user-facing OS/2 API reference
   - `samples/README.md` — sample programs and their required phases/features
   - Inline code comments in any modules touched by recent changes — ensure they still accurately describe the current behavior

3. **Verify numeric accuracy** — run `cargo test 2>&1 | tail -10` to get the current test count; inspect `src/loader/api_registry.rs` and sub-dispatchers to count API entry points. Update any stale counts in `CLAUDE.md`, `README.md`, or elsewhere so they match reality exactly.

3a. **Validate the API ordinal table** — run both consistency checks:
   ```bash
   cargo run --bin gen_api -- check          # def-vs-api_trace.rs drift check
   cargo run --bin gen_api -- validate-doc   # def-vs-doc/os2_ordinals.md cross-check
   ```
   - If `check` reports warnings or errors, update `targets/os2api.def` and/or `src/loader/api_trace.rs` to eliminate the drift, then re-run `gen-trace` and paste the output.
   - If `validate-doc` reports **name mismatches** (⚠ section): investigate each one. For mismatches that are genuine errors in `os2api.def` (wrong ordinal or wrong name), fix the def and update `api_trace.rs` accordingly. For mismatches that are merely 16-bit-vs-32-bit naming conventions that the normaliser cannot auto-detect, add a `# NOTE:` comment on the relevant line in `os2api.def` explaining the discrepancy so future passes do not re-investigate.
   - If `validate-doc` reports entries **only in def** (ℹ section): confirm they are intentional (e.g. ordinals documented in the IBM CPP Reference but absent from the Open Watcom lib snapshot). If not intentional, correct the def.
   - Do **not** add new ordinals to `doc/os2_ordinals.md` based solely on Warpine's own assignments — that file is a snapshot of the Open Watcom import library and should only be updated from authoritative OS/2 SDK sources.

4. **Revise stale or incomplete content** — update anything that no longer matches the current code: new features, removed dependencies, behavioral changes, renamed constants, new modules, and architectural decisions. When in doubt, read the source — never assume.

5. **Update `doc/TODOs.md`** — remove completed items. Before removing, if the completed work is non-obvious or architectural, capture a concise summary in `doc/developer_guide.md` or `doc/reference_manual.md` as appropriate so the implementation rationale is preserved.

6. **Commit** documentation changes using the `commit-and-push` skill (which runs `cargo test` and `cargo clippy -- -D warnings` before committing). Group related files per commit — for example, `CLAUDE.md` + `CHANGELOG.md` together when covering the same feature; `doc/TODOs.md` alone for a cleanup pass. Do not mix unrelated documentation changes in a single commit.
