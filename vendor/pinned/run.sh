#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vendor/pinned/run.sh - run a pinned compiler (ADR-0101). gcc and clang in
# this directory are links to it; it runs the command it is called as in
# the image vendor/setup_compilers.sh builds, with the repository mounted
# at its own path, the current directory kept, no network, and the
# caller's user id, so the outputs belong to the caller.
set -euo pipefail
ROOT=$(cd "$(dirname "$(readlink -f "$0")")/../.." && pwd)
IMAGE=$(sed -n 's/^IMAGE="\(.*\)"$/\1/p' "$ROOT/vendor/setup_compilers.sh")
SUITE=$(sed -n 's/^SUITE="\(.*\)"$/\1/p' "$ROOT/vendor/setup_compilers.sh")
SNAPSHOT=$(sed -n 's/^SNAPSHOT="\(.*\)"$/\1/p' "$ROOT/vendor/setup_compilers.sh")
IMAGE=${IMAGE//\$\{SUITE\}/$SUITE}
IMAGE=${IMAGE//\$\{SNAPSHOT\}/$SNAPSHOT}
case "$PWD" in
    "$ROOT"|"$ROOT"/*) ;;
    *) echo "pinned $(basename "$0"): run it inside $ROOT" >&2; exit 1 ;;
esac
exec podman run --rm --network=none --userns=keep-id \
    -v "$ROOT:$ROOT" -w "$PWD" "$IMAGE" "$(basename "$0")" "$@"
