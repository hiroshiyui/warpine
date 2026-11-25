#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# ci/check_desktop.sh - check Warpine's confinement on a real desktop.
#
#   ci/check_desktop.sh            # from the repository, in a desktop session
#
# CI tests the Landlock sandbox (ADR-0111) and the seccomp deny lists
# (ADR-0124) only with SDL's dummy drivers. This runs a few samples on the
# session's real display and sound (Wayland or X11, PipeWire or
# PulseAudio, the GPU's drivers) twice: confined, and with
# WARPINE_SANDBOX=off (which lifts both). Both layers refuse rather than
# kill (Landlock EACCES, seccomp EPERM), so a path they block shows up as a
# quieter run: a missing driver, a warning, a check that fails. Each check
# compares the two runs; with strace installed it also lists the calls only
# the confined run had refused.
#
# Windows open and sounds play for a few seconds each; every sample ends
# itself through a GUI script. The logs and target/desktop-check/summary.txt
# stay for a report. Needs KVM and the Open Watcom samples (make -C samples).
set -uo pipefail
ROOT=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
cd "$ROOT"
OUT="$ROOT/target/desktop-check"
mkdir -p "$OUT"
SUMMARY="$OUT/summary.txt"

if [ "${SDL_VIDEODRIVER:-}" = dummy ] || [ "${SDL_AUDIODRIVER:-}" = dummy ]; then
    echo "check_desktop.sh: SDL_VIDEODRIVER / SDL_AUDIODRIVER is dummy: this checks no real desktop" >&2
fi
[ -e /dev/kvm ] || { echo "check_desktop.sh: /dev/kvm is missing" >&2; exit 1; }
cargo build -q || exit 1
WARPINE="$ROOT/target/debug/warpine"
STRACE=$(command -v strace || true)

# name | sample | GUI script | marker of a complete run
CHECKS=(
    "pm-window|samples/pm_bench/pm_bench.exe|5000 quit|pm_bench: done"
    "pm-audio|samples/pm_audio_test/pm_audio_test.exe|15000 quit|pm_audio_test: done"
    "vio-window|samples/hello/hello.exe|3000 quit|Hello"
    "vio-audio|samples/audio_test/audio_test.exe|10000 quit|Failed: 0"
)

# One run of a check: its log, and (with strace) its refused calls.
run() {
    local name=$1 sample=$2 steps=$3 mode=$4
    local log="$OUT/$name.$mode.log" script="$OUT/$name.script"
    printf '%s\n' "$steps" > "$script"
    # The desktop server (its own process, its own seccomp role) logs to
    # a file of its own.
    rm -f "$OUT/$name.$mode.server.log"
    local -a env=(RUST_LOG=info NO_COLOR=1 WARPINE_GUI_SCRIPT="$script" WARPINE_DESKTOP_LOG="$OUT/$name.$mode.server.log")
    [ "$mode" = open ] && env+=(WARPINE_SANDBOX=off)
    if [ -n "$STRACE" ]; then
        env "${env[@]}" timeout -k 10 90 "$STRACE" -f -qq -Z -e trace=%file,%network,%desc,%ipc \
            -o "$OUT/$name.$mode.strace" "$WARPINE" "$sample" < /dev/null > "$log" 2>&1
    else
        env "${env[@]}" timeout -k 10 90 "$WARPINE" "$sample" < /dev/null > "$log" 2>&1
    fi
    echo $? > "$OUT/$name.$mode.rc"
}

# A run's log and its desktop server's.
logs() { cat "$OUT/$1.$2.log" "$OUT/$1.$2.server.log" 2>/dev/null; }

# The refused calls of a strace log, one per line: call, first path, error.
refusals() {
    [ -f "$1" ] || return 0
    grep -E ' = -1 (EACCES|EPERM) ' "$1" \
        | sed -E 's/^[0-9]+ +//; s/^([a-z0-9_]+)\(([^"]*"([^"]*)")?.* = -1 ([A-Z]+).*/\1 \3 \4/' \
        | sort -u
}

{
    echo "Warpine desktop check, $(date -u +%Y-%m-%dT%H:%MZ), $(git rev-parse --short HEAD)"
    echo "session: ${XDG_SESSION_TYPE:-?}, WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-}, DISPLAY=${DISPLAY:-}"
    echo "sound server: $(pactl info 2>/dev/null | sed -n 's/^Server Name: //p' || true)"
    echo "kernel: $(uname -r); strace: ${STRACE:-none}"
    echo
} > "$SUMMARY"

failed=0
for check in "${CHECKS[@]}"; do
    IFS='|' read -r name sample steps marker <<< "$check"
    if [ ! -f "$sample" ]; then
        echo "$name: SKIPPED ($sample not built: make -C samples)" | tee -a "$SUMMARY"
        continue
    fi
    echo "== $name ($sample)"
    run "$name" "$sample" "$steps" open
    run "$name" "$sample" "$steps" confined
    verdict=PASS
    for mode in open confined; do
        log="$OUT/$name.$mode.log"
        got=$(grep -c -- "$marker" "$log")
        drivers=$(logs "$name" "$mode" | grep -oE '(video|audio) driver [a-z0-9_]+' | sort -u | paste -sd, -)
        warns=$(logs "$name" "$mode" | grep -cE '\b(WARN|ERROR)\b')
        fails=$(grep -cE '\bFAIL' "$log")
        echo "  $mode: exit $(cat "$OUT/$name.$mode.rc"), marker ${got}x, $fails FAIL lines, drivers [$drivers], $warns warnings/errors" | tee -a "$SUMMARY.tmp"
    done
    o_drv=$(logs "$name" open | grep -oE '(video|audio) driver [a-z0-9_]+' | sort -u)
    c_drv=$(logs "$name" confined | grep -oE '(video|audio) driver [a-z0-9_]+' | sort -u)
    [ "$o_drv" = "$c_drv" ] || verdict=DIFFERENT
    grep -q -- "$marker" "$OUT/$name.confined.log" || verdict=DIFFERENT
    grep -qE '\bFAIL' "$OUT/$name.confined.log" && verdict=DIFFERENT
    # Warnings only the confined run had.
    diff <(logs "$name" open | grep -E '\b(WARN|ERROR)\b' | sed -E 's/\x1b\[[0-9;]*m//g; s/^[0-9T:.Z-]+ +//' | sort -u) \
         <(logs "$name" confined | grep -E '\b(WARN|ERROR)\b' | sed -E 's/\x1b\[[0-9;]*m//g; s/^[0-9T:.Z-]+ +//' | sort -u) \
        | sed -n 's/^> /  warning when confined: /p' > "$OUT/$name.warnings"
    [ -s "$OUT/$name.warnings" ] && verdict=DIFFERENT
    if [ -n "$STRACE" ]; then
        refusals "$OUT/$name.open.strace" > "$OUT/$name.open.refused"
        refusals "$OUT/$name.confined.strace" > "$OUT/$name.confined.refused"
        comm -13 "$OUT/$name.open.refused" "$OUT/$name.confined.refused" > "$OUT/$name.blocked"
        [ -s "$OUT/$name.blocked" ] && verdict=DIFFERENT
    fi
    [ "$verdict" = PASS ] || failed=1
    {
        echo "$name: $verdict"
        cat "$SUMMARY.tmp"
        if [ -s "$OUT/$name.blocked" ]; then
            echo "  refused only when confined:"
            sed 's/^/    /' "$OUT/$name.blocked" | head -40
        fi
        head -20 "$OUT/$name.warnings"
    } >> "$SUMMARY"
    rm -f "$SUMMARY.tmp"
    echo "  $verdict"
done

echo
cat "$SUMMARY"
echo
echo "Logs in $OUT; please send summary.txt (and any *.blocked file)."
exit $failed
