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

echo "Applying warpine patches..."
# Apply patches for warpine compatibility (see patches/README.md)
if [ -d "$SCRIPT_DIR/patches" ]; then
    # bsesub.h — diff patch against vendor/watcom/h/os2/bsesub.h
    # Replaces APIENTRY16 with _System to eliminate 16-bit VIO/KBD thunks
    if [ -f "$SCRIPT_DIR/patches/bsesub.h.patch" ]; then
        WATCOM_BSESUB="$SCRIPT_DIR/../../vendor/watcom/h/os2/bsesub.h"
        if [ -f "$WATCOM_BSESUB" ]; then
            cp "$WATCOM_BSESUB" "$SCRIPT_DIR/h/bsesub.h"
            patch -s -p2 "$SCRIPT_DIR/h/bsesub.h" < "$SCRIPT_DIR/patches/bsesub.h.patch"
            echo "  Applied: h/bsesub.h (APIENTRY16 → _System)"
        else
            echo "  Warning: vendor/watcom/h/os2/bsesub.h not found, skipping patch"
        fi
    fi
    # viodirect.h — new file (APIENTRY16/_Seg16 overrides)
    if [ -f "$SCRIPT_DIR/patches/viodirect.h" ]; then
        cp "$SCRIPT_DIR/patches/viodirect.h" "$SCRIPT_DIR/h/viodirect.h"
        echo "  Applied: h/viodirect.h"
    fi
    # viowrap.c — new file (32-bit VIO/KBD import pragmas)
    if [ -f "$SCRIPT_DIR/patches/viowrap.c" ]; then
        cp "$SCRIPT_DIR/patches/viowrap.c" "$SCRIPT_DIR/c/viowrap.c"
        echo "  Applied: c/viowrap.c"
    fi
    # crt0.c — new file (minimal C runtime startup, replaces Watcom's __OS2Main)
    if [ -f "$SCRIPT_DIR/patches/crt0.c" ]; then
        cp "$SCRIPT_DIR/patches/crt0.c" "$SCRIPT_DIR/c/crt0.c"
        echo "  Applied: c/crt0.c"
    fi
    # os2init.c — diff patch (DosGetInfoSeg → DosGetInfoBlocks)
    if [ -f "$SCRIPT_DIR/patches/os2init.c.patch" ]; then
        patch -s -p1 -d "$SCRIPT_DIR" < "$SCRIPT_DIR/patches/os2init.c.patch"
        echo "  Applied: c/os2init.c (DosGetInfoSeg replacement)"
    fi
    # os2calls.c — diff patch (direct DosFindFirst/DosFindNext, getline rename)
    if [ -f "$SCRIPT_DIR/patches/os2calls.c.patch" ]; then
        patch -s -p1 -d "$SCRIPT_DIR" < "$SCRIPT_DIR/patches/os2calls.c.patch"
        echo "  Applied: c/os2calls.c (DosFindFirst/DosFindNext fixes)"
    fi
fi

echo "Done. 4OS2 source is ready in $SCRIPT_DIR"
echo "Build with: make -C $SCRIPT_DIR"
