#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# ci/matrix.sh - one entry of the CI matrix (ADR-0106), run in the image
# vendor/setup_ci_image.sh builds (ci/run.sh starts it there):
#
#   ci/matrix.sh watcom   # Open Watcom's samples: every test, clippy, the generators
#   ci/matrix.sh gcc      # every C sample from the pinned GCC, the integration tests on them
#   ci/matrix.sh clang    # the same with the pinned clang
#
# CI_JOBS (default 2) sets cargo's jobs and the test threads. Each Warpine
# a test starts runs under the tests' own timeouts; the entry fails if any
# is left when the tests end.
set -euo pipefail

MODE=${1:?usage: ci/matrix.sh watcom|gcc|clang}
JOBS=${CI_JOBS:-2}
ROOT=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
cd "$ROOT"

[ -c /dev/kvm ] || { echo "ci/matrix.sh: /dev/kvm is missing; the tests would skip" >&2; exit 1; }

export CARGO_BUILD_JOBS=$JOBS CARGO_TERM_COLOR=never
export SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy
# Drive C and the session directories: a fresh place, never a home's.
SCRATCH=$(mktemp -d)
trap 'rm -rf -- "$SCRATCH"' EXIT
chmod 700 "$SCRATCH"
export XDG_DATA_HOME="$SCRATCH" XDG_RUNTIME_DIR="$SCRATCH"

step() { echo; echo "=== $* ==="; }

step "build"
cargo build --bins

case "$MODE" in
    watcom)
        step "Open Watcom (pinned snapshot) and its samples"
        ./vendor/setup_watcom.sh
        make -C samples
        step "unit and integration tests"
        cargo test -- --test-threads="$JOBS"
        step "clippy"
        cargo clippy --all-targets -- -D warnings
        step "generators"
        cargo run -q --bin gen_api -- check
        cargo run -q --bin gen_api -- validate-doc
        cargo run -q --bin gen_api -- check-headers
        cargo run -q --bin gen_arch -- check
        ;;
    gcc|clang)
        step "every C sample from $MODE ($("$MODE" --version | head -1))"
        # Under their own tag, apart from a workstation's own builds.
        OUT_TAG="ci-$MODE" samples/build_sdk.sh "$MODE"
        step "integration tests on the $MODE builds"
        WARPINE_SAMPLES="ci-$MODE" cargo test --test integration -- --test-threads="$JOBS"
        ;;
    *)
        echo "ci/matrix.sh: unknown entry $MODE (watcom, gcc or clang)" >&2
        exit 2
        ;;
esac

LEFT=$(pgrep -c -x warpine || true)
if [ "$LEFT" != "0" ]; then
    echo "ci/matrix.sh: $LEFT Warpine processes are left after the tests" >&2
    exit 1
fi
echo "ci/matrix.sh $MODE: passed"
