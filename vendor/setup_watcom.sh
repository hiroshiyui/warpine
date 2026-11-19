#!/bin/bash
# vendor/setup_watcom.sh
# This script downloads and extracts the Open Watcom v2 snapshot.
#
#   ./vendor/setup_watcom.sh            # once, from the repository root
#   ./vendor/setup_watcom.sh --refresh  # replace the snapshot with the current one
#
# Snapshots built before 2026-08-11 have a broken 32-bit OS/2 start-up
# (Open Watcom issue / pull request #1649: `__OS2Main` was declared "no
# return", so it reads the entry stack one slot low — the command line from
# [esp+12], where the environment is). Programs built with them see an empty
# argv and no environment, and fail at start-up on OS/2 itself. The script
# warns about such a snapshot; rebuild the samples after --refresh.
#
# Supply chain: the script downloads one pinned, dated snapshot (not the
# moving "Current-build") over HTTPS only and refuses it unless its SHA-256
# matches the digest below, which GitHub records for that release asset.
# To move to a newer snapshot, change SNAPSHOT_TAG and SNAPSHOT_SHA256
# together (gh api repos/open-watcom/open-watcom-v2/releases/tags/<tag>
# --jq '.assets[] | select(.name=="ow-snapshot.tar.xz") | .digest'),
# then run --refresh and rebuild the samples.

# Exit on error
set -e

# Directory for Watcom
WATCOM_DIR="$(pwd)/vendor/watcom"
TAR_FILE="ow-snapshot.tar.xz"
SNAPSHOT_TAG="2026-11-01-Build"
SNAPSHOT_SHA256="89e90b7544cf3ea4a38124ec4c104a5027f31101405d7115c3f833daa465bb48"
URL="https://github.com/open-watcom/open-watcom-v2/releases/download/${SNAPSHOT_TAG}/${TAR_FILE}"
# The fix of #1649 (commit ce4a86f, 2026-08-11): libraries built after it.
FIXED_STARTUP_DATE="2026-08-12"

# Warn when the snapshot's OS/2 C library predates the start-up fix.
check_startup() {
    local lib="$WATCOM_DIR/lib386/os2/clib3r.lib"
    if [ -f "$lib" ] && [ -z "$(find "$lib" -newermt "$FIXED_STARTUP_DATE")" ]; then
        echo "WARNING: this Open Watcom snapshot predates $FIXED_STARTUP_DATE: its OS/2 start-up"
        echo "         code misreads the entry stack (argv and environment are lost, #1649)."
        echo "         Run ./vendor/setup_watcom.sh --refresh, then rebuild the samples."
    fi
}

# Create vendor directory if it doesn't exist
mkdir -p vendor

REFRESH=0
if [ "$1" = "--refresh" ] && [ -d "$WATCOM_DIR" ]; then
    REFRESH=1
    echo "Replacing the Open Watcom v2 snapshot in $WATCOM_DIR"
fi

if [ "$REFRESH" = 0 ] && [ -d "$WATCOM_DIR" ]; then
    echo "Open Watcom v2 already exists in $WATCOM_DIR"
    check_startup
    exit 0
fi

echo "Downloading Open Watcom v2 snapshot $SNAPSHOT_TAG from $URL..."
DOWNLOAD="vendor/$TAR_FILE.part"
rm -f "$DOWNLOAD"
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fL -o "$DOWNLOAD" "$URL"

echo "Checking its SHA-256..."
if ! echo "$SNAPSHOT_SHA256  $DOWNLOAD" | sha256sum -c --status; then
    echo "error: $TAR_FILE does not match the pinned SHA-256 $SNAPSHOT_SHA256; refusing it." >&2
    rm -f "$DOWNLOAD"
    exit 1
fi

echo "Extracting Open Watcom v2..."
# Extract next to the final place first, so a failed download or extraction
# leaves an existing snapshot untouched (set -e stops before the swap).
# --no-same-owner: files belong to the user, whatever the archive says;
# GNU tar also drops leading "/" and refuses ".." in member names.
rm -rf "$WATCOM_DIR.new"
mkdir -p "$WATCOM_DIR.new"
tar --no-same-owner -xJf "$DOWNLOAD" -C "$WATCOM_DIR.new"
[ -x "$WATCOM_DIR.new/binl64/wcl386" ] || { echo "error: the snapshot has no binl64/wcl386"; exit 1; }

# Clean up the archive
rm "$DOWNLOAD"
# Swap: the old snapshot (on --refresh) out, the new one in.
if [ -d "$WATCOM_DIR" ]; then
    rm -rf "$WATCOM_DIR.old"
    mv "$WATCOM_DIR" "$WATCOM_DIR.old"
fi
mv "$WATCOM_DIR.new" "$WATCOM_DIR"
rm -rf "$WATCOM_DIR.old"
check_startup

echo ""
echo "Open Watcom v2 vendored in $WATCOM_DIR"
echo "To use it, set the following environment variables (using 64-bit binaries):"
echo "  export WATCOM=$WATCOM_DIR"
echo "  export PATH=\$WATCOM/binl64:\$PATH"
echo "  export INCLUDE=\$WATCOM/h/os2:\$INCLUDE"
