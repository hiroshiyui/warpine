#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vendor/setup_ci_image.sh - the image the CI matrix runs in (ADR-0106).
#
#   ./vendor/setup_ci_image.sh            # once, from the repository root
#   ./vendor/setup_ci_image.sh --refresh  # rebuild the image
#   ./vendor/setup_ci_image.sh --print-image  # its name
#
# Builds localhost/warpine-ci:<SUITE>-<SNAPSHOT>: the Debian base and the
# snapshot.debian.org timestamp of vendor/setup_compilers.sh (the same
# pinned gcc, g++ and clang), plus what building and testing Warpine
# needs (SDL3, pkg-config, GDB, curl for Open Watcom's snapshot), and
# Rust: rustup-init of a fixed version checked against its SHA-256, which
# installs the toolchain rust-toolchain.toml names. ci/matrix.sh runs in it.
#
# Supply chain: the base image by digest, the packages from the dated
# snapshot of the suite and of its updates and security suites (the base
# image already carries their updates; apt checks every package against
# the archive keys in the image),
# rustup-init by version and SHA-256; rustup checks the toolchain against
# the channel manifest's hashes. Move RUSTUP_VERSION and RUSTUP_SHA256
# together (https://static.rust-lang.org/rustup/archive/<version>/
# x86_64-unknown-linux-gnu/rustup-init.sha256).

set -euo pipefail

ROOT=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
setting() { sed -n "s/^$1=\"\(.*\)\"$/\1/p" "$ROOT/vendor/setup_compilers.sh"; }
SUITE=$(setting SUITE)
BASE_IMAGE=$(setting BASE_IMAGE)
BASE_DIGEST=$(setting BASE_DIGEST)
SNAPSHOT=$(setting SNAPSHOT)
RUST_TOOLCHAIN=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$ROOT/rust-toolchain.toml")
RUSTUP_VERSION="1.29.3"
RUSTUP_SHA256="b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55"
IMAGE="localhost/warpine-ci:${SUITE}-${SNAPSHOT}-rust${RUST_TOOLCHAIN}"

if [ "${1:-}" = "--print-image" ]; then
    echo "$IMAGE"
    exit 0
fi

command -v podman >/dev/null || { echo "setup_ci_image.sh: podman is not installed" >&2; exit 1; }
[ -n "$SUITE" ] && [ -n "$BASE_DIGEST" ] && [ -n "$SNAPSHOT" ] && [ -n "$RUST_TOOLCHAIN" ] \
    || { echo "setup_ci_image.sh: cannot read the pins" >&2; exit 1; }

if [ "${1:-}" != "--refresh" ] && podman image exists "$IMAGE"; then
    echo "The CI image is already $IMAGE"
    exit 0
fi

CONTEXT=$(mktemp -d)
trap 'rm -rf -- "$CONTEXT"' EXIT
cat > "$CONTEXT/Containerfile" <<CONTAINERFILE
FROM ${BASE_IMAGE}@${BASE_DIGEST}
RUN rm -f /etc/apt/sources.list.d/debian.sources \\
 && { echo 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${SNAPSHOT} ${SUITE} main'; \\
      echo 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${SNAPSHOT} ${SUITE}-updates main'; \\
      echo 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/${SNAPSHOT} ${SUITE}-security main'; } > /etc/apt/sources.list \\
 && apt-get -o Acquire::Retries=3 update \\
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \\
      gcc g++ clang binutils make libsdl3-dev pkg-config gdb ca-certificates curl xz-utils procps \\
 && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:\$PATH
RUN curl --proto '=https' --tlsv1.2 -fsSL -o /tmp/rustup-init \\
      https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/x86_64-unknown-linux-gnu/rustup-init \\
 && echo '${RUSTUP_SHA256}  /tmp/rustup-init' | sha256sum -c - \\
 && chmod +x /tmp/rustup-init \\
 && /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain ${RUST_TOOLCHAIN} -c clippy,rustfmt \\
 && rm /tmp/rustup-init \\
 && chmod -R a+rwX /opt/rustup /opt/cargo
CONTAINERFILE

echo "Building $IMAGE (Debian $SUITE at $SNAPSHOT on $BASE_DIGEST, Rust $RUST_TOOLCHAIN)"
podman build --pull=missing -t "$IMAGE" "$CONTEXT"
podman run --rm --network=none "$IMAGE" sh -c 'gcc --version | head -1; clang --version | head -1; rustc --version; pkg-config --modversion sdl3'
