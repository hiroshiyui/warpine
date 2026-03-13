#!/bin/bash
# Fetch 4OS2 source code for building under Warpine.
#
# 4OS2 is Copyright (c) 1989-2002 J.P. Software, Inc.
# Licensed under a permissive BSD-like license — see license.txt in the
# fetched source tree for the full terms.
#
# Source: https://github.com/StevenLevine/4os2
# Pinned to commit 7e56d1c for reproducible builds.

set -euo pipefail

REPO_URL="https://github.com/StevenLevine/4os2.git"
COMMIT="7e56d1c"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Check if source already fetched
if [ -d "$SCRIPT_DIR/c" ] && [ -f "$SCRIPT_DIR/license.txt" ]; then
    echo "4OS2 source already present. To re-fetch, run: $0 --force"
    if [ "${1:-}" != "--force" ]; then
        exit 0
    fi
    echo "Re-fetching..."
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Cloning 4OS2 source (commit $COMMIT)..."
git clone --quiet "$REPO_URL" "$TMPDIR/4os2"
cd "$TMPDIR/4os2"
git checkout --quiet "$COMMIT"

echo "Copying source files..."
# Copy source directories and essential files; skip .git
for item in a c h license.txt 4os2.rc 4os2.ico 4os2.txt 4os2h.txt \
            4os2.ipf 4os2.wlk 4os2.wis 4os2.ini.sample \
            4os2Alias.sample 4start.cmd.sample 4start.cmd.sample2 \
            cmds.err batch.err; do
    if [ -e "$TMPDIR/4os2/$item" ]; then
        cp -r "$TMPDIR/4os2/$item" "$SCRIPT_DIR/$item"
    fi
done

echo "Done. 4OS2 source is ready in $SCRIPT_DIR"
echo "Build with: make -C $SCRIPT_DIR"
