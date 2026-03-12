#!/bin/bash
# vendor/setup_watcom.sh
# This script downloads and extracts the Open Watcom v2 snapshot.

# Exit on error
set -e

# Directory for Watcom
WATCOM_DIR="$(pwd)/vendor/watcom"
TAR_FILE="ow-snapshot.tar.xz"
URL="https://github.com/open-watcom/open-watcom-v2/releases/download/Current-build/${TAR_FILE}"

# Create vendor directory if it doesn't exist
mkdir -p vendor

if [ -d "$WATCOM_DIR" ]; then
    echo "Open Watcom v2 already exists in $WATCOM_DIR"
    exit 0
fi

echo "Downloading Open Watcom v2 snapshot from $URL..."
curl -L -O "$URL"

echo "Extracting Open Watcom v2..."
mkdir -p "$WATCOM_DIR"
# The snapshot contains everything; we'll extract it directly to the folder.
tar -xJf "$TAR_FILE" -C "$WATCOM_DIR"

# Clean up tar file
rm "$TAR_FILE"

echo ""
echo "Open Watcom v2 vendored in $WATCOM_DIR"
echo "To use it, set the following environment variables (using 64-bit binaries):"
echo "  export WATCOM=$WATCOM_DIR"
echo "  export PATH=\$WATCOM/binl64:\$PATH"
echo "  export INCLUDE=\$WATCOM/h/os2:\$INCLUDE"
