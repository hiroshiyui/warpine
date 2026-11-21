#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# fuzz/seed.sh - starting inputs for the fuzz targets, from the repository's
# own built samples (Open Watcom's, and samples/build/*): LX programs and
# DLLs for lx_parse, NE programs for ne_parse, message files for msgfile.
# desktop_proto starts from nothing; libFuzzer finds its tags quickly.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"
mkdir -p corpus/lx_parse corpus/ne_parse corpus/msgfile corpus/desktop_proto
n=0
while IFS= read -r -d '' f; do
    case $(head -c 2 "$f" | od -An -c | tr -d ' ') in
        MZ) cp -- "$f" "corpus/lx_parse/$(sha256sum -- "$f" | cut -c1-16)"
            cp -- "$f" "corpus/ne_parse/$(sha256sum -- "$f" | cut -c1-16)"
            n=$((n + 1)) ;;
    esac
done < <(find ../samples -type f \( -name '*.exe' -o -name '*.dll' \) -size -2M -print0)
while IFS= read -r -d '' f; do
    cp -- "$f" "corpus/msgfile/$(sha256sum -- "$f" | cut -c1-16)"
done < <(find ../samples ../target -type f -name '*.msg' -size -1M -print0 2>/dev/null)
echo "fuzz/seed.sh: $n programs and DLLs as seeds"
