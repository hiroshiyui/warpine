#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# fuzz/run.sh <target> [seconds] [corpus dir] - build a fuzz target with
# libFuzzer's coverage instrumentation and the address sanitizer, then run
# it for `seconds` (default 60). The flags are those cargo-fuzz passes,
# without needing cargo-fuzz itself. Seeds: the corpus directory (default
# fuzz/corpus/<target>, not in the repository), which fuzz/seed.sh fills
# from the built samples. A crash leaves its input in fuzz/artifacts/.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"
target=${1:?usage: fuzz/run.sh desktop_proto|lx_parse|ne_parse|msgfile|os2rc|somidl|lx_link|bitmap|res_templates|pointer|wav|config_sys|profile|resolver [seconds] [corpus]}
seconds=${2:-60}
corpus=${3:-corpus/$target}
mkdir -p "$corpus" "artifacts/$target"
export RUSTFLAGS="-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=4 \
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters -Cllvm-args=-sanitizer-coverage-pc-table \
-Cllvm-args=-sanitizer-coverage-trace-compares --cfg fuzzing -Zsanitizer=address -Cdebug-assertions"
export CARGO_TARGET_DIR=${FUZZ_TARGET_DIR:-target}
cargo build --release --locked --bin "$target" --target x86_64-unknown-linux-gnu
"$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/$target" \
    -max_total_time="$seconds" -rss_limit_mb=2048 -max_len=1048576 \
    -artifact_prefix="artifacts/$target/" "$corpus"
