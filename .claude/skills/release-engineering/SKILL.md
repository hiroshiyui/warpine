---
name: release-engineering
description: Manage the full software release process, including version bumps, changelogs, Git tags, and GitHub releases.
---

When performing release engineering, always follow these steps:

1. **Verify the build is clean** — run `cargo clean && cargo build 2>&1` to confirm a from-scratch build passes with no errors or unexpected warnings. Then run `cargo test 2>&1` to confirm all tests pass. Do not proceed if either fails.

2. **Determine the release type** — review all unreleased commits since the last tag (`git log $(git describe --tags --abbrev=0)..HEAD --oneline`) and classify the release as `major`, `minor`, or `patch` following [Semantic Versioning](https://semver.org/). Present the recommendation to the user and confirm before proceeding.

3. **Update the version** — bump the `version` field in `Cargo.toml` to match the new release version. Run `cargo check` to regenerate `Cargo.lock`.

4. **Update `CHANGELOG.md`** — add a new version entry at the top following the [Keep a Changelog](https://keepachangelog.com/) format. Group changes under `Added`, `Changed`, `Fixed`, `Removed`, or `Security` as appropriate. Include all notable changes since the previous release.

5. **Commit the release** — stage `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` together and commit with the message `chore: release vX.Y.Z` (plus co-author trailer per the `commit-and-push` skill).

6. **Tag the release** — create an annotated Git tag (`git tag -a vX.Y.Z -m "vX.Y.Z"`) and push both the commit and the tag to the remote (`git push && git push --tags`).

7. **Create a GitHub release** — use `gh release create vX.Y.Z --title "vX.Y.Z" --notes "..."` with the corresponding `CHANGELOG.md` section as the release notes. Use `--notes` (not `--body`) for the release description.
