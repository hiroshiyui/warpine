#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# ci/matrix.sh - one entry of the CI matrix (ADR-0106), run in the image
# vendor/setup_ci_image.sh builds (ci/run.sh starts it there):
#
#   ci/matrix.sh watcom   # Open Watcom's samples: every test, clippy, the generators, cargo deny, fuzzing
#   ci/matrix.sh gcc      # every C sample from the pinned GCC, the integration tests on them
#   ci/matrix.sh clang    # the same with the pinned clang
#   ci/matrix.sh fuzz     # every fuzz target for FUZZ_SECONDS (default 600), seeded from Open Watcom's and GCC's samples
#
# CI_JOBS (default 2) sets cargo's jobs and the test threads. Each Warpine
# a test starts runs under the tests' own timeouts; the entry fails if any
# is left when the tests end.
set -euo pipefail

MODE=${1:?usage: ci/matrix.sh watcom|gcc|clang|fuzz}
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

# Every fuzz target for $1 seconds, seeded from the samples built (fuzz/).
# A crash prints its input to rerun.
fuzz_all() {
    step "fuzzing, $1 s per target"
    fuzz/seed.sh
    for target in desktop_proto lx_parse ne_parse msgfile os2rc somidl lx_link bitmap res_templates pointer wav config_sys profile resolver wps_setup fea2_list; do
        if ! FUZZ_TARGET_DIR="$CARGO_TARGET_DIR/fuzz" fuzz/run.sh "$target" "$1" >"$CARGO_TARGET_DIR/fuzz-$target.log" 2>&1; then
            tail -40 "$CARGO_TARGET_DIR/fuzz-$target.log"
            for crash in fuzz/artifacts/"$target"/*; do
                [ -f "$crash" ] && { echo "input $crash (base64):"; base64 -w 76 "$crash"; }
            done
            echo "ci/matrix.sh: fuzz target $target failed" >&2
            exit 1
        fi
        grep -h "^Done" "$CARGO_TARGET_DIR/fuzz-$target.log" || true
    done
}

# The integration tests (`cargo test --test integration ...`). On GitHub's
# runners only, a run that failed because guests triple-faulted (some of
# them cannot run PAE guests, ADR-0131 addendum) is a warning on the run's
# page, not a failure; any other failure still fails the job.
integration_tests() {
    local log="$CARGO_TARGET_DIR/integration.log"
    if "$@" 2>&1 | tee "$log"; then return 0; fi
    if [ -n "${GITHUB_ACTIONS:-}" ] && grep -q "Triple Fault (guest shutdown" "$log"; then
        echo "::warning::Guests triple-faulted on this runner (it cannot run PAE guests, ADR-0131); the integration tests' failures are not counted"
        return 0
    fi
    return 1
}

step "build"
cargo build --bins

case "$MODE" in
    watcom)
        step "Open Watcom (pinned snapshot) and its samples"
        ./vendor/setup_watcom.sh
        make -C samples
        step "unit tests"
        cargo test --bins -- --test-threads="$JOBS"
        step "integration tests"
        integration_tests cargo test --test integration -- --test-threads="$JOBS"
        step "clippy"
        cargo clippy --all-targets -- -D warnings
        step "generators"
        cargo run -q --bin gen_api -- check
        cargo run -q --bin gen_api -- validate-doc
        cargo run -q --bin gen_api -- check-headers
        cargo run -q --bin gen_arch -- check
        # The whole dependency policy of deny.toml, advisories included
        # (the pre-commit hook checks only what needs no network).
        step "dependency policy (cargo deny, with advisories)"
        cargo deny --log-level error check
        (cd fuzz && cargo deny --log-level error check)
        # The parsers that read untrusted input, for a minute each.
        fuzz_all "${FUZZ_SECONDS:-60}"
        ;;
    fuzz)
        # The weekly long runs (.github/workflows/fuzz.yml): seeds from
        # Open Watcom's and GCC's samples, then every target for
        # FUZZ_SECONDS (default ten minutes).
        step "seeds: Open Watcom's and GCC's samples"
        ./vendor/setup_watcom.sh
        make -C samples
        OUT_TAG=ci-gcc samples/build_sdk.sh gcc
        fuzz_all "${FUZZ_SECONDS:-600}"
        ;;
    gcc|clang)
        step "every C sample from $MODE ($("$MODE" --version | head -1))"
        # Under their own tag, apart from a workstation's own builds.
        OUT_TAG="ci-$MODE" samples/build_sdk.sh "$MODE"
        step "integration tests on the $MODE builds"
        WARPINE_SAMPLES="ci-$MODE" integration_tests cargo test --test integration -- --test-threads="$JOBS"
        ;;
    *)
        echo "ci/matrix.sh: unknown entry $MODE (watcom, gcc, clang or fuzz)" >&2
        exit 2
        ;;
esac

LEFT=$(pgrep -c -x warpine || true)
if [ "$LEFT" != "0" ]; then
    echo "ci/matrix.sh: $LEFT Warpine processes are left after the tests" >&2
    exit 1
fi
echo "ci/matrix.sh $MODE: passed"
