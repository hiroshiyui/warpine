#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# fuzz/seed.sh - starting inputs for the fuzz targets, from the repository's
# own built samples (Open Watcom's, and samples/build/*): LX programs and
# DLLs for lx_parse, NE programs for ne_parse, message files for msgfile,
# resource scripts for os2rc, IDL files for somidl, and lx-link's inputs (ELF objects, resource
# files, DLLs, message files) for lx_link, each behind the byte that picks
# its kind (fuzz_targets/lx_link.rs), bitmap files for bitmap, and the
# resource files behind each of res_templates' five kinds, pointer and
# icon files for pointer, WAV files for wav, and a CONFIG.SYS and a
# profile written here for config_sys and profile, the resolver's
# inputs, setup strings for wps_setup, and an EA list for fea2_list.
# desktop_proto starts from nothing;
# libFuzzer finds its tags quickly.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"
mkdir -p corpus/lx_parse corpus/ne_parse corpus/msgfile corpus/desktop_proto corpus/os2rc corpus/somidl corpus/lx_link corpus/bitmap corpus/res_templates corpus/pointer corpus/wav corpus/config_sys corpus/profile corpus/resolver corpus/wps_setup corpus/fea2_list
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
# somidl: IDL files, Open Watcom's SOM samples among them (local only:
# the corpus is not committed).
while IFS= read -r -d '' f; do
    cp -- "$f" "corpus/somidl/$(sha256sum -- "$f" | cut -c1-16)"
done < <(find ../samples ../sdk ../vendor/watcom/samples -type f -name '*.idl' -size -1024k -print0 2>/dev/null)
while IFS= read -r -d '' f; do
    cp -- "$f" "corpus/bitmap/$(sha256sum -- "$f" | cut -c1-16)"
done < <(find ../samples ../sdk -type f -iname '*.bmp' -size -1024k -print0)
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
# res_templates: each resource file behind each parser's byte (0 dialog,
# 1 menu, 2 accelerators, 3 string bundle, 4 PRESPARAMS).
while IFS= read -r -d '' f; do
    for kind in 0 1 2 3 4; do
        { printf "\\x0$kind"; cat -- "$f"; } > "corpus/res_templates/$kind-$(sha256sum -- "$f" | cut -c1-16)"
    done
done < <(find ../samples ../sdk -type f -name '*.res' -size -256k -print0 2>/dev/null)
while IFS= read -r -d '' f; do
    cp -- "$f" "corpus/pointer/$(sha256sum -- "$f" | cut -c1-16)"
done < <(find ../samples ../sdk -type f \( -iname '*.ptr' -o -iname '*.ico' \) -size -1024k -print0)
while IFS= read -r -d '' f; do
    cp -- "$f" "corpus/wav/$(sha256sum -- "$f" | cut -c1-16)"
done < <(find ../samples -type f -iname '*.wav' -size -1024k -print0)
# No sample ships a WAV file: a minimal one (8 kHz, 8-bit mono, 8 frames).
printf 'RIFF\x2c\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x40\x1f\x00\x00\x40\x1f\x00\x00\x01\x00\x08\x00data\x08\x00\x00\x00\x80\x80\x80\x80\x80\x80\x80\x80' > corpus/wav/minimal
# CONFIG.SYS and profile seeds written here: the statements Warpine reads,
# and a profile with escapes.
printf 'REM seed\r\nTHREADS=512\r\nLIBPATH=.;C:\\OS2\\DLL;C:\\LIB\r\nSET PATH=C:\\BIN;C:\\\r\nSET TMP=\r\n' > corpus/config_sys/seed
printf '; warpine-profile 1\n["App"]\n"Key" = "value\\0"\n"Blob" = "\\x01\\xff\\"\\\\"\n' > corpus/profile/seed
# resolver: a DNS answer with a compression pointer (kind 0), hosts,
# networks and resolv.conf lines (1-3), and a name (4).
printf '\x00\x12\x34\x81\x80\x00\x01\x00\x01\x00\x00\x00\x00\x03www\x07example\x03com\x00\x00\x01\x00\x01\xc0\x0c\x00\x01\x00\x01\x00\x00\x0e\x10\x00\x04\x5d\xb8\xd8\x22' > corpus/resolver/answer
printf '\x01127.0.0.1 localhost lh # c\n::1 ip6-localhost\n10.0.0.2\tbox\n' > corpus/resolver/hosts
printf '\x02loopback 127\nlink-local 169.254.0.0\n' > corpus/resolver/networks
printf '\x03nameserver 192.0.2.53\nsearch example.com lan\ndomain example.org\n' > corpus/resolver/resolv_conf
printf '\x04www.example.com.' > corpus/resolver/name
printf '\x08OBJECTIDTITLE=Lunch^;today;TEXT=soup;OBJECTID=<WP_C>;NODELETE=YES' > corpus/wps_setup/create
printf '\x1c\x00\x00\x00\x00\x00\x00\x00\x00\x04\x03\x00MINE\x00abc\x00\x00\x00\x00\x00\x00\x00' > corpus/fea2_list/one
echo "fuzz/seed.sh: $n programs and DLLs, $m lx-link inputs as seeds"
