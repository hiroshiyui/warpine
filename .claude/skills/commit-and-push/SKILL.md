---
name: commit-and-push
description: Stage, commit, and push changes to the remote repository with a well-formed commit message.
---

When committing and pushing changes, always follow these steps:

1. **Run tests** with `cargo test` and ensure all tests pass. If any test fails, stop and report the failures to the user — do not proceed with staging or committing until the tests are green.

2. **Stage** all relevant changes with `git add`. Be deliberate — stage only files related to the current topic. Never blindly stage everything with `git add -A` if unrelated changes are present.

3. **Commit** with a clear, concise message following the [Conventional Commits](https://www.conventionalcommits.org/) standard (e.g., `feat(loader): implement DosSetFilePtr`). The message should explain *why* the change was made, not just *what* changed. Append a co-author trailer:
   ```
   Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
   ```
   Pass the full message via a heredoc to preserve formatting. Never use `--no-verify` to skip hooks unless the user explicitly requests it.

4. **Push** the committed changes to the current branch on the remote repository.

5. **Verify** that the push succeeded and the remote is in sync with the local branch.
