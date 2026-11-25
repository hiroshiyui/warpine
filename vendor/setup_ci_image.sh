#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vendor/setup_ci_image.sh - the image the CI matrix runs in (ADR-0106).
#
#   ./vendor/setup_ci_image.sh            # once, from the repository root
#   ./vendor/setup_ci_image.sh --refresh  # rebuild the image
#   ./vendor/setup_ci_image.sh --print-image  # its name
#   ./vendor/setup_ci_image.sh --pull     # the published image, else build
#   ./vendor/setup_ci_image.sh --publish  # build and push it (ci-image.yml)
#
# Published image (ADR-0106 addendum): ci-image.yml pushes the image to
# $REGISTRY under the same tag; ci/image.digest pins each tag to the
# digest it was published with. --pull takes the image by that digest
# (so a replaced image fails the pull) and builds it here when the tag
# has no pin yet or the pull fails. GitHub's CI pulls; a workstation
# builds unless CI_IMAGE_PULL=1. --publish pushes nothing when the tag is
# already published, and prints the line for ci/image.digest.
#
# Other localhost/warpine-ci tags (images of earlier pins) are removed
# once the current image exists; KEEP_OLD_CI_IMAGES=1 keeps them.
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
# The fuzz targets' nightly comes from rustup like the stable toolchain.
# cargo-deny (the dependency policy of deny.toml, advisories included) is
# its release's static musl binary, by version and SHA-256; move
# CARGO_DENY_VERSION and CARGO_DENY_SHA256 together (the release's
# cargo-deny-<version>-x86_64-unknown-linux-musl.tar.gz.sha256).

set -euo pipefail

ROOT=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
setting() { sed -n "s/^$1=\"\(.*\)\"$/\1/p" "$ROOT/vendor/setup_compilers.sh"; }
SUITE=$(setting SUITE)
BASE_IMAGE=$(setting BASE_IMAGE)
BASE_DIGEST=$(setting BASE_DIGEST)
SNAPSHOT=$(setting SNAPSHOT)
RUST_TOOLCHAIN=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$ROOT/rust-toolchain.toml")
# The dated nightly of the fuzz targets (fuzz/rust-toolchain.toml) and of
# the Rust guest crates: rust-src for the guests' `-Z build-std`
# (test_rust_hello builds samples/rust_hello whenever the toolchain is
# installed).
FUZZ_TOOLCHAIN=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$ROOT/fuzz/rust-toolchain.toml")
FUZZ_COMPONENTS="rust-src"
RUSTUP_VERSION="1.29.3"
RUSTUP_SHA256="b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55"
CARGO_DENY_VERSION="0.19.4"
CARGO_DENY_SHA256="3bd58b784e83715b86ddbc9deac591890372ec77fda5741bb0826970b958506f"
# Counted up whenever what the image holds changes in a way the names
# above do not show (its package list): a new tag builds a new image.
# 2: git, which cargo-deny runs to fetch the advisory database.
IMAGE_REVISION=2
IMAGE="localhost/warpine-ci:${SUITE}-${SNAPSHOT}-rust${RUST_TOOLCHAIN}-deny${CARGO_DENY_VERSION}-fuzz${FUZZ_TOOLCHAIN}-${FUZZ_COMPONENTS}-r${IMAGE_REVISION}"

TAG=${IMAGE#localhost/warpine-ci:}
REGISTRY=${WARPINE_CI_REGISTRY:-ghcr.io/hiroshiyui/warpine-ci}
MODE=${1:-}

if [ "$MODE" = "--print-image" ]; then
    echo "$IMAGE"
    exit 0
fi

command -v podman >/dev/null || { echo "setup_ci_image.sh: podman is not installed" >&2; exit 1; }
[ -n "$SUITE" ] && [ -n "$BASE_DIGEST" ] && [ -n "$SNAPSHOT" ] && [ -n "$RUST_TOOLCHAIN" ] && [ -n "$FUZZ_TOOLCHAIN" ] \
    || { echo "setup_ci_image.sh: cannot read the pins" >&2; exit 1; }

# Remove the warpine-ci images of other tags: each change of the pins
# leaves the previous image behind (gigabytes each); they can all be
# built again from this script. KEEP_OLD_CI_IMAGES=1 keeps them.
remove_stale_images() {
    [ "${KEEP_OLD_CI_IMAGES:-}" = "1" ] && return 0
    local old
    podman images --format '{{.Repository}}:{{.Tag}}' localhost/warpine-ci | while read -r old; do
        [ "$old" = "$IMAGE" ] && continue
        echo "Removing the stale CI image $old"
        podman rmi "$old" >/dev/null || echo "setup_ci_image.sh: could not remove $old (in use?)" >&2
    done
}

# The digest ci/image.digest pins this tag to (empty: none).
pinned_digest() {
    awk -v t="$TAG" '$1 == t && $2 ~ /^sha256:[0-9a-f]+$/ && length($2) == 71 { print $2 }' "$ROOT/ci/image.digest" 2>/dev/null
}

# Pull the published image by its pinned digest, three tries, and name it
# $IMAGE; fails (for a build here) when there is no pin or no image.
pull_pinned() {
    local digest i
    digest=$(pinned_digest)
    if [ -z "$digest" ]; then
        echo "setup_ci_image.sh: ci/image.digest pins no image for $TAG: building it" >&2
        return 1
    fi
    for i in 1 2 3; do
        if podman pull -q "$REGISTRY@$digest" >/dev/null; then
            podman tag "$REGISTRY@$digest" "$IMAGE"
            echo "Pulled $IMAGE ($REGISTRY@$digest)"
            return 0
        fi
        sleep $((i * 15))
    done
    echo "setup_ci_image.sh: cannot pull $REGISTRY@$digest: building it" >&2
    return 1
}

# The line for ci/image.digest, also to the workflow's summary.
report_published() {
    echo "Published: $REGISTRY:$TAG"
    echo "ci/image.digest line: $TAG $1"
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
        printf '### CI image\n\n`%s:%s`\n\nPin it in `ci/image.digest`:\n\n```\n%s %s\n```\n' \
            "$REGISTRY" "$TAG" "$TAG" "$1" >> "$GITHUB_STEP_SUMMARY"
    fi
}

if [ "$MODE" = "--publish" ] && podman pull -q "$REGISTRY:$TAG" >/dev/null 2>&1; then
    # Published already: a rebuild would give another digest than the pin.
    report_published "$(podman image inspect --format '{{.Digest}}' "$REGISTRY:$TAG")"
    exit 0
fi

if [ "$MODE" != "--refresh" ] && podman image exists "$IMAGE"; then
    echo "The CI image is already $IMAGE"
    remove_stale_images
    [ "$MODE" = "--publish" ] || exit 0
elif [ "$MODE" = "--pull" ] && pull_pinned; then
    remove_stale_images
    exit 0
else

CONTEXT=$(mktemp -d)
trap 'rm -rf -- "$CONTEXT"' EXIT
cat > "$CONTEXT/Containerfile" <<CONTAINERFILE
FROM ${BASE_IMAGE}@${BASE_DIGEST}
RUN rm -f /etc/apt/sources.list.d/debian.sources \\
 && { echo 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${SNAPSHOT} ${SUITE} main'; \\
      echo 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${SNAPSHOT} ${SUITE}-updates main'; \\
      echo 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/${SNAPSHOT} ${SUITE}-security main'; } > /etc/apt/sources.list \\
 && apt-get -o Acquire::Retries=5 update \\
 && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=5 install -y --no-install-recommends \\
      gcc g++ clang binutils make libsdl3-dev pkg-config gdb ca-certificates curl xz-utils procps git \\
 && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:\$PATH
RUN curl --retry 5 --retry-all-errors --proto '=https' --tlsv1.2 -fsSL -o /tmp/rustup-init \\
      https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/x86_64-unknown-linux-gnu/rustup-init \\
 && echo '${RUSTUP_SHA256}  /tmp/rustup-init' | sha256sum -c - \\
 && chmod +x /tmp/rustup-init \\
 && /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain ${RUST_TOOLCHAIN} -c clippy,rustfmt \\
 && rm /tmp/rustup-init \\
 && /opt/cargo/bin/rustup toolchain install --no-self-update ${FUZZ_TOOLCHAIN} --profile minimal -c ${FUZZ_COMPONENTS} \\
 && curl --retry 5 --retry-all-errors --proto '=https' --tlsv1.2 -fsSL -o /tmp/cargo-deny.tar.gz \\
      https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl.tar.gz \\
 && echo '${CARGO_DENY_SHA256}  /tmp/cargo-deny.tar.gz' | sha256sum -c - \\
 && tar -xzf /tmp/cargo-deny.tar.gz -C /tmp \\
 && install -m 0755 /tmp/cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl/cargo-deny /opt/cargo/bin/cargo-deny \\
 && rm -rf /tmp/cargo-deny.tar.gz /tmp/cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl \\
 && chmod -R a+rwX /opt/rustup /opt/cargo
CONTAINERFILE

echo "Building $IMAGE (Debian $SUITE at $SNAPSHOT on $BASE_DIGEST, Rust $RUST_TOOLCHAIN)"
podman build --pull=missing -t "$IMAGE" "$CONTEXT"
podman run --rm --network=none "$IMAGE" sh -c 'gcc --version | head -1; clang --version | head -1; rustc --version; pkg-config --modversion sdl3'
remove_stale_images
fi

if [ "$MODE" = "--publish" ]; then
    DIGEST_FILE=$(mktemp)
    for i in 1 2 3 0; do
        [ "$i" != 0 ] || { echo "setup_ci_image.sh: cannot push $REGISTRY:$TAG" >&2; exit 1; }
        podman push --digestfile "$DIGEST_FILE" "$IMAGE" "docker://$REGISTRY:$TAG" && break
        sleep $((i * 15))
    done
    report_published "$(cat "$DIGEST_FILE")"
    rm -f -- "$DIGEST_FILE"
fi
