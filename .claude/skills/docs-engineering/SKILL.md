---
name: docs-engineering
description: Audit and update all project documentation to stay in sync with the current development status.
---

When performing documentation engineering, always follow these steps:

1. **Survey recent changes** by running `git log --oneline -20` and reading the diff of recent commits (`git diff <prev-tag>..HEAD` or `git show` per commit). Then read the affected source files directly — do not rely solely on commit messages to understand what changed.

2. **Audit** all documentation against the current codebase and development status. The review scope must include — without exception:
   - `README.md` — features list, prerequisites, acknowledgements
   - `CHANGELOG.md` — release notes and version history
   - `CLAUDE.md` — stack, architecture, key gotchas, project conventions
   - `doc/developer_guide.md`, `doc/os2_ordinals.md`, `doc/TODOs.md`, `doc/reference_manual.md`
   - `samples/README.md` — sample scripts and their required features/phases
   - Code comments visible to human developers

3. **Revise and update** any documentation that is stale, incomplete, or inconsistent with the current code. Ensure new features, removed dependencies, behavioral changes, and architectural decisions are reflected accurately. When in doubt, read the source — do not assume.

4. **Remove completed items** from `doc/TODOs.md`. If a summary of completed work is warranted, add a brief note before removing the items.

5. **Commit** documentation changes using the `commit-and-push` skill, grouped by topic. Do not mix unrelated documentation changes in a single commit.
