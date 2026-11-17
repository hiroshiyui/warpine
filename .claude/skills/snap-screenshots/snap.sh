#!/usr/bin/env bash
# Capture one screenshot of a PM sample running under Warpine, with no display.
#
#   snap.sh <name> <sample.exe> [script-steps] [out-dir]
#
#   name          output file stem; writes <out-dir>/<name>.png
#   sample.exe    OS/2 PM binary, path relative to the repository root
#   script-steps  GUI script lines separated by ';' that run BEFORE the capture,
#                 e.g. "1500 click 52 62;2500 click 62 80". Times are
#                 milliseconds since start. Default: none.
#   out-dir       default: doc/screenshots
#
# The capture is taken 1.5 s after the last step (2.5 s after start when there
# are no steps), then the session quits. The desktop background is trimmed
# away so the image shows the application's windows with a small margin.
#
# Environment: WARPINE_BIN overrides the binary (default target/debug/warpine).
set -euo pipefail

name=${1:?usage: snap.sh <name> <sample.exe> [script-steps] [out-dir]}
sample=${2:?sample path missing}
steps=${3:-}
out_dir=${4:-doc/screenshots}
bin=${WARPINE_BIN:-target/debug/warpine}

[ -x "$bin" ]    || { echo "snap: $bin not found; run cargo build first" >&2; exit 2; }
[ -f "$sample" ] || { echo "snap: $sample not found; build the sample first (make -C $(dirname "$sample"))" >&2; exit 2; }
[ -e /dev/kvm ]  || { echo "snap: /dev/kvm not available" >&2; exit 2; }
command -v magick >/dev/null || command -v convert >/dev/null \
                 || { echo "snap: ImageMagick (magick/convert) not found" >&2; exit 2; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Build the session script: the caller's steps, then dump and quit.
last_ms=1000
: > "$work/session.script"
if [ -n "$steps" ]; then
    while IFS= read -r line; do
        line=$(echo "$line" | sed 's/^ *//; s/ *$//')
        [ -z "$line" ] && continue
        echo "$line" >> "$work/session.script"
        ms=${line%% *}
        [ "$ms" -gt "$last_ms" ] 2>/dev/null && last_ms=$ms
    done < <(echo "$steps" | tr ';' '\n')
fi
echo "$((last_ms + 1500)) dump $work/desktop.ppm" >> "$work/session.script"
echo "$((last_ms + 1600)) quit"                   >> "$work/session.script"

# Pin the US country and codepage: Warpine follows the host locale otherwise.
SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy RUST_LOG=error WARPINE_COUNTRY=1 WARPINE_CODEPAGE=437 \
WARPINE_GUI_SCRIPT="$work/session.script" \
    timeout 60 "$bin" "$sample" >/dev/null 2>"$work/stderr.log" || {
        echo "snap: warpine exited with an error for $sample:" >&2
        tail -5 "$work/stderr.log" >&2
        exit 1
    }
[ -s "$work/desktop.ppm" ] || { echo "snap: no desktop dump was written for $sample" >&2; exit 1; }

mkdir -p "$out_dir"
im=$(command -v magick || command -v convert)
# Trim the uniform desktop background, then put back a 12-px margin of it.
bg=$("$im" "$work/desktop.ppm" -format '%[pixel:p{0,0}]' info:)
"$im" "$work/desktop.ppm" -trim +repage -bordercolor "$bg" -border 12 \
      -strip "$out_dir/$name.png"
command -v optipng >/dev/null && optipng -quiet "$out_dir/$name.png" || true

echo "$out_dir/$name.png $("$im" "$out_dir/$name.png" -format '%wx%h' info:)"
