#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vendor/setup_compilers.sh - the pinned GCC and clang that build the C
# samples (ADR-0101).
#
#   ./vendor/setup_compilers.sh            # once, from the repository root
#   ./vendor/setup_compilers.sh --refresh  # rebuild the image
#
# Builds a local Podman image, localhost/warpine-cc:<SNAPSHOT>, holding
# Debian's gcc, g++, clang, binutils and make as they were at one dated
# snapshot.debian.org timestamp, on a Debian base image named by its
# digest. vendor/pinned/gcc, g++, clang and clang++ then run those
# compilers in it:
#
#   CC_CMD=vendor/pinned/gcc OUT_TAG=gcc-pinned samples/build_sdk.sh gcc
#
# Supply chain: the base image is pulled by its sha256 digest, never by a
# moving tag; the packages come from the snapshot archive at a fixed
# timestamp, and apt checks them against the Release file signed with the
# Debian archive keys already in that image. To move to newer compilers,
# change BASE_DIGEST (podman pull docker.io/library/debian:<suite>-slim;
# podman image inspect --format '{{.Digest}}' ...) and SNAPSHOT together,
# then run --refresh and rebuild the samples.

set -euo pipefail

SUITE="trixie"
BASE_IMAGE="docker.io/library/debian"
BASE_DIGEST="sha256:7378260c0a098d27ac206225db5e7d92b8c7c84ea7afccc86eeb513756bbd392"
SNAPSHOT="20261115T000000Z"
IMAGE="localhost/warpine-cc:${SUITE}-${SNAPSHOT}"

command -v podman >/dev/null || { echo "setup_compilers.sh: podman is not installed" >&2; exit 1; }

if [ "${1:-}" != "--refresh" ] && podman image exists "$IMAGE"; then
    echo "The pinned compilers are already in $IMAGE"
    exit 0
fi

CONTEXT=$(mktemp -d)
trap 'rm -rf -- "$CONTEXT"' EXIT
cat > "$CONTEXT/Containerfile" <<CONTAINERFILE
FROM ${BASE_IMAGE}@${BASE_DIGEST}
RUN rm -f /etc/apt/sources.list.d/debian.sources \\
 && echo 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${SNAPSHOT} ${SUITE} main' > /etc/apt/sources.list \\
 && apt-get -o Acquire::Retries=3 update \\
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends gcc g++ clang binutils make \\
 && rm -rf /var/lib/apt/lists/*
CONTAINERFILE

echo "Building $IMAGE (Debian $SUITE at $SNAPSHOT on $BASE_DIGEST)"
podman build --pull=missing -t "$IMAGE" "$CONTEXT"
podman run --rm --network=none "$IMAGE" sh -c 'gcc --version | head -1; g++ --version | head -1; clang --version | head -1'
