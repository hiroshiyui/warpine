#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# ci/run.sh - run CI matrix entries in the pinned image (ADR-0106), on a
# workstation as on the CI runner:
#
#   ci/run.sh watcom gcc clang    # (default: all three)
#   FUZZ_SECONDS=600 ci/run.sh fuzz   # the weekly long fuzz runs
#
# Builds the image if needed (vendor/setup_ci_image.sh; with
# CI_IMAGE_PULL=1 pulls the published one first), then runs
# ci/matrix.sh in it with /dev/kvm, the repository mounted at its own
# path and the caller's user id. Cargo builds into target/ci, and the C
# samples go to samples/build/ci-<cc>, apart from a workstation's own
# builds. The network is on: cargo fetches the crates Cargo.lock pins, and
# setup_watcom.sh the pinned snapshot.
set -euo pipefail
ROOT=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
cd "$ROOT"
# CI_IMAGE_PULL=1 (GitHub's CI): the published image by its pinned digest,
# built here only when there is none (ADR-0106 addendum).
if [ "${CI_IMAGE_PULL:-}" = 1 ]; then
    "$ROOT/vendor/setup_ci_image.sh" --pull
else
    "$ROOT/vendor/setup_ci_image.sh"
fi
IMAGE=$("$ROOT/vendor/setup_ci_image.sh" --print-image)
[ $# -gt 0 ] || set -- watcom gcc clang
for mode in "$@"; do
    echo "##### $mode #####"
    podman run --rm --device /dev/kvm --userns=keep-id --security-opt label=disable \
        -e CI_JOBS="${CI_JOBS:-2}" -e CARGO_TARGET_DIR="$ROOT/target/ci" \
        ${FUZZ_SECONDS:+-e FUZZ_SECONDS="$FUZZ_SECONDS"} \
        -v "$ROOT:$ROOT" -w "$ROOT" "$IMAGE" ci/matrix.sh "$mode"
done
