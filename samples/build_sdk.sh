#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# samples/build_sdk.sh - build the C samples with GCC or clang and
# Warpine's SDK instead of Open Watcom (ADR-0101).
#
#   samples/build_sdk.sh [gcc|clang] [sample...]
#
# Each sample's programs and DLLs go to samples/build/<cc>/<sample>/ under
# the names Open Watcom gives them, with the sample's data files beside
# them, so the integration tests can run either build
# (WARPINE_SAMPLES=gcc|clang cargo test --test integration). The Open
# Watcom binaries in samples/<sample>/ stay the regression build.
#
# A sample is one .c file making one program of the same name, with its
# .rc if it has one; the samples that are not are listed in build_special.
# Samples that cannot be built this way are in SKIP.
#
# CC_CMD overrides the compiler command, and OUT_TAG the output's name
# (samples/build/<OUT_TAG>, sdk/build/<OUT_TAG>): the pinned compilers of
# vendor/setup_compilers.sh are
#   CC_CMD=vendor/pinned/gcc OUT_TAG=gcc-pinned samples/build_sdk.sh gcc

set -euo pipefail
shopt -s nullglob

CC_NAME=${1:-gcc}
shift || true
case "$CC_NAME" in
    gcc|clang) ;;
    *) echo "build_sdk.sh: the compiler is gcc or clang, not '$CC_NAME'" >&2; exit 2 ;;
esac
CC_CMD=${CC_CMD:-$CC_NAME}
# A path is made absolute: the compiler runs in each sample's directory.
case "$CC_CMD" in */*) CC_CMD=$(cd "$(dirname "$CC_CMD")" && pwd)/$(basename "$CC_CMD") ;; esac
OUT_TAG=${OUT_TAG:-$CC_NAME}

ROOT=$(cd "$(dirname "$0")/.." && pwd)
SAMPLES=$ROOT/samples
SDK=$ROOT/sdk
OUT_ROOT=$SAMPLES/build/$OUT_TAG
LX_LINK=${LX_LINK:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/lx_link}
OS2RC=${OS2RC:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/os2rc}

# Not built here: the 16-bit NE program, the SDK's own samples (their
# Makefiles build them with either compiler), the Rust guest, the
# samples only Open Watcom builds (shapes; wcxx_throw_test, Open Watcom
# C++), and the SOM samples whose classes come from the SOM compiler's
# C++ and IDL output.
SKIP=" ne_hello ne_ring_test c_hello c_sdk_test c_socket_test c_pm_test c_gdb_test c_msg_test c_som_test som_idl_test cxx_test wcxx_throw_test rust_hello shapes som_test build "

for tool in "$LX_LINK" "$OS2RC"; do
    [ -x "$tool" ] || { echo "build_sdk.sh: $tool is missing (cargo build --bin lx_link --bin os2rc)" >&2; exit 1; }
done

# The runtime, built with the same compiler command.
make -s -C "$SDK" CC="$CC_CMD" OS2_BUILD="$SDK/build/$OUT_TAG"
RT=$SDK/build/$OUT_TAG
CRT0=$RT/crt0.o
DLL0=$RT/dll0.o
LIBC=$RT/libos2c.a

GCC_INCLUDE=$("$CC_CMD" -m32 -print-file-name=include)
CFLAGS=(-m32 -O2 -ffreestanding -fno-pic -fno-stack-protector
        -fasynchronous-unwind-tables -ffunction-sections -fdata-sections
        -nostdinc -isystem "$SDK/include" -isystem "$GCC_INCLUDE"
        -D_System= -D__syscall= -D_Optlink= -std=gnu11 -w)
if [ "$CC_NAME" = clang ]; then
    CFLAGS+=(--target=i386-unknown-none -mstack-alignment=4)
else
    CFLAGS+=(-mincoming-stack-boundary=2)
fi

# cc <out.o> <src.c> [flags...]: compile in the source's directory.
cc() {
    local out=$1 src=$2
    shift 2
    (cd "$(dirname "$src")" && "$CC_CMD" "${CFLAGS[@]}" -I . "$@" -c "$(basename "$src")" -o "$out")
}

# rc <out.res> <src.rc>: compile a resource script in its directory.
rc() {
    (cd "$(dirname "$2")" && "$OS2RC" -I . -I "$SDK/include" "$(basename "$2")" -o "$1")
}

# program <out.exe> <objects and .res / .dll inputs...>
program() {
    local out=$1
    shift
    "$LX_LINK" --gc-sections "$CRT0" "$@" "$LIBC" -o "$out"
}

# dll <out.dll> <exports> <objects...>: a DLL with the runtime and
# INITINSTANCE / TERMINSTANCE start-up.
dll() {
    local out=$1 exports=$2
    shift 2
    local args=(--dll --initinstance --terminstance)
    [ -n "$exports" ] && args+=(--export "$exports")
    "$LX_LINK" --gc-sections "${args[@]}" "$DLL0" "$@" "$LIBC" -o "$out"
}

# The sample's data files (anything that is not a source or a build
# product) go beside the programs.
copy_data() {
    local src=$1 out=$2 f
    for f in "$src"/*; do
        [ -f "$f" ] || continue
        case "$f" in
            *.c|*.h|*.rc|*.obj|*.o|*.exe|*.dll|*.lnk|*.lib|*.res|*.err|*.map|*/Makefile|*.mk) continue ;;
        esac
        cp -p "$f" "$out/"
    done
}

build_special() {
    local name=$1 src=$SAMPLES/$1 out=$2 v
    case "$name" in
        dll_noinit)
            # No runtime and no initialization routine, as with Open Watcom.
            cc "$out/noinit.o" "$src/noinit.c"
            "$LX_LINK" --dll --export NoInitAdd.1,NoInitTwice.2,NoInitAnswer.3 "$out/noinit.o" -o "$out/noinit.dll"
            ;;
        dllinit_test)
            for v in goodinit badinit dllinit badstart; do cc "$out/$v.o" "$src/$v.c"; done
            dll "$out/goodinit.dll" GoodValue "$out/goodinit.o"
            dll "$out/badinit.dll" BadValue "$out/badinit.o"
            program "$out/dllinit.exe" "$out/dllinit.o" "$out/goodinit.dll" "$out/badinit.dll"
            program "$out/badstart.exe" "$out/badstart.o" "$out/badinit.dll"
            ;;
        exitlist_test)
            cc "$out/exitlist_test.o" "$src/exitlist_test.c"
            program "$out/exitlist_test.exe" "$out/exitlist_test.o"
            for v in thread:THREAD_EXIT mainlast:MAIN_LAST nomain:NO_MAIN hlt:HLT_EXIT; do
                cc "$out/exitlist_${v%%:*}.o" "$src/exitlist_test.c" "-D${v#*:}"
                program "$out/exitlist_${v%%:*}.exe" "$out/exitlist_${v%%:*}.o"
            done
            ;;
        paging_abuse_test)
            for v in zero:ZERO cr3:CR3 remap:REMAP off:PAGING_OFF past:PAST; do
                cc "$out/pgabuse_${v%%:*}.o" "$src/paging_abuse_test.c" "-D${v#*:}"
                program "$out/pgabuse_${v%%:*}.exe" "$out/pgabuse_${v%%:*}.o"
            done
            ;;
        thread_exit_test)
            cc "$out/thread_exit_test.o" "$src/thread_exit_test.c"
            program "$out/thread_exit_test.exe" "$out/thread_exit_test.o"
            cc "$out/thread_last_test.o" "$src/thread_exit_test.c" -DLAST_THREAD
            program "$out/thread_last_test.exe" "$out/thread_last_test.o"
            ;;
        pm_bitmap_test)
            cc "$out/pm_bitmap_test.o" "$src/pm_bitmap_test.c"
            rc "$out/pm_bitmap_test.res" "$src/pm_bitmap_test.rc"
            program "$out/pm_bitmap_test.exe" "$out/pm_bitmap_test.o" "$out/pm_bitmap_test.res"
            cc "$out/bmpres.o" "$src/bmpres.c"
            rc "$out/bmpres.res" "$src/bmpres.rc"
            dll "$out/bmpres.dll" "" "$out/bmpres.o" "$out/bmpres.res"
            ;;
        som_build_test)
            cc "$out/bird.o" "$src/bird.c" -I "$SAMPLES/som_test/include"
            dll "$out/bird.dll" SOMInitModule,BirdNewClass,BirdClassData,BirdCClassData "$out/bird.o"
            cc "$out/somb.o" "$src/somb.c" -I "$SAMPLES/som_test/include"
            program "$out/somb.exe" "$out/somb.o"
            ;;
        som_layout_test|som_ordinal_test)
            local c=("$src"/*.c)
            local base
            base=$(basename "${c[0]}" .c)
            cc "$out/$base.o" "${c[0]}" -I "$SAMPLES/som_test/include"
            program "$out/$base.exe" "$out/$base.o"
            ;;
        *) return 1 ;;
    esac
}

build_one() {
    local name=$1 src=$SAMPLES/$1 out=$OUT_ROOT/$1
    mkdir -p "$out"
    # Inputs the sample's Makefile generates first (not in the repository).
    if [ -f "$src/make_ptr.py" ]; then
        python3 -I "$src/make_ptr.py" "$src"
    fi
    copy_data "$src" "$out"
    build_special "$name" "$out" && return 0
    local c=("$src"/*.c)
    if [ ${#c[@]} -ne 1 ]; then
        echo "build_sdk.sh: $name has ${#c[@]} C files and no rule in build_special" >&2
        return 1
    fi
    local base
    base=$(basename "${c[0]}" .c)
    cc "$out/$base.o" "${c[0]}"
    local inputs=("$out/$base.o")
    if [ -f "$src/$base.rc" ]; then
        rc "$out/$base.res" "$src/$base.rc"
        inputs+=("$out/$base.res")
    fi
    program "$out/$base.exe" "${inputs[@]}"
}

names=("$@")
if [ ${#names[@]} -eq 0 ]; then
    for d in "$SAMPLES"/*/; do
        d=$(basename "$d")
        [ -f "$SAMPLES/$d/Makefile" ] || continue
        case "$SKIP" in *" $d "*) continue ;; esac
        names+=("$d")
    done
fi

failed=()
for n in "${names[@]}"; do
    if build_one "$n"; then :; else failed+=("$n"); fi
done
echo "build_sdk.sh: $OUT_TAG built $(( ${#names[@]} - ${#failed[@]} )) of ${#names[@]} samples into ${OUT_ROOT#"$ROOT"/}"
if [ ${#failed[@]} -gt 0 ]; then
    echo "build_sdk.sh: failed: ${failed[*]}" >&2
    exit 1
fi
