#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# fuzz/seed.sh - starting inputs for the fuzz targets, from the repository's
# own built samples (Open Watcom's, and samples/build/*): LX programs and
# DLLs for lx_parse, NE programs for ne_parse, message files for msgfile,
# resource scripts for os2rc, and lx-link's inputs (ELF objects, resource
# files, DLLs, message files) for lx_link, each behind the byte that picks
# its kind (fuzz_targets/lx_link.rs). desktop_proto starts from nothing;
# libFuzzer finds its tags quickly.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"
mkdir -p corpus/lx_parse corpus/ne_parse corpus/msgfile corpus/desktop_proto corpus/os2rc corpus/lx_link
n=0
while IFS= read -r -d '' f; do
    case $(head -c 2 "$f" | od -An -c | tr -d ' ') in
        MZ) cp -- "$f" "corpus/lx_parse/$(sha256sum -- "$f" | cut -c1-16)"
            cp -- "$f" "corpus/ne_parse/$(sha256sum -- "$f" | cut -c1-16)"
            n=$((n + 1)) ;;
    esac
done < <(find ../samples -type f \( -name '*.exe' -o -name '*.dll' \) -size -2048k -print0)
while IFS= read -r -d '' f; do
    cp -- "$f" "corpus/msgfile/$(sha256sum -- "$f" | cut -c1-16)"
done < <(find ../samples ../target -type f -name '*.msg' -size -1024k -print0 2>/dev/null)
while IFS= read -r -d '' f; do
    cp -- "$f" "corpus/os2rc/$(sha256sum -- "$f" | cut -c1-16)"
done < <(find ../samples ../sdk -type f -name '*.rc' -size -1024k -print0)
# lx_link: the first byte picks the input's name: 0 .o, 1 .res, 2 .dll,
# 3 .msg.
m=0
while IFS= read -r -d '' f; do
    case $f in
        *.o) kind='\x00' ;;
        *.res) kind='\x01' ;;
        *.dll) kind='\x02' ;;
        *.msg) kind='\x03' ;;
        *) continue ;;
    esac
    { printf "$kind"; cat -- "$f"; } > "corpus/lx_link/$(sha256sum -- "$f" | cut -c1-16)"
    m=$((m + 1))
done < <(find ../samples ../sdk -type f \( -name '*.o' -o -name '*.res' -o -name '*.dll' -o -name '*.msg' \) -size -1024k -print0 2>/dev/null)
echo "fuzz/seed.sh: $n programs and DLLs, $m lx-link inputs as seeds"
