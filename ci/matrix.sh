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
    for target in desktop_proto lx_parse ne_parse msgfile os2rc somidl lx_link bitmap res_templates pointer wav config_sys profile resolver wps_setup fea2_list ipf_file; do
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

# The host's processor and KVM settings, for runners that cannot run PAE
# guests (ADR-0131 addendum): is nested paging (npt / ept) on?
step "host"
grep -m1 "model name" /proc/cpuinfo || true
uname -r
for p in /sys/module/kvm_amd/parameters/npt /sys/module/kvm_amd/parameters/nested \
         /sys/module/kvm_intel/parameters/ept /sys/module/kvm_intel/parameters/nested; do
    [ -r "$p" ] && echo "$p: $(cat "$p")"
done
true

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
    probe)
        # A quick check of PAE guests on this runner (ADR-0131 addendum,
        # .github/workflows/pae-probe.yml): one and several threads, and
        # no warning in place of a failure.
        step "Open Watcom and the probe's samples"
        ./vendor/setup_watcom.sh
        for s in hello nx_test tlm_test ring_test mem_protect_test stack_grow_test; do make -C "samples/$s"; done
        step "PAE probe"
        cargo test --test integration -- --test-threads="$JOBS" test_hello_world test_no_execute_memory \
            test_thread_local_memory every_thread_has_a_tss_and_the_flat_segments \
            page_states_of_allocated_memory_hold thread_stacks_grow_and_run_out
        ;;
    gcc|clang)
        step "every C sample from $MODE ($("$MODE" --version | head -1))"
        # Under their own tag, apart from a workstation's own builds.
        OUT_TAG="ci-$MODE" samples/build_sdk.sh "$MODE"
        step "integration tests on the $MODE builds"
        WARPINE_SAMPLES="ci-$MODE" cargo test --test integration -- --test-threads="$JOBS"
        ;;
    *)
        echo "ci/matrix.sh: unknown entry $MODE (watcom, gcc, clang, fuzz or probe)" >&2
        exit 2
        ;;
esac

LEFT=$(pgrep -c -x warpine || true)
if [ "$LEFT" != "0" ]; then
    echo "ci/matrix.sh: $LEFT Warpine processes are left after the tests" >&2
    exit 1
fi
echo "ci/matrix.sh $MODE: passed"
