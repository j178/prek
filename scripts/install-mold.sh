#!/usr/bin/env bash
# Install mold linker and make it the default linker.
#
# Retries on transient HTTP errors (e.g., 500) that the `rui314/setup-mold`
# GitHub Action does not handle.

set -euo pipefail

MOLD_VERSION="${MOLD_VERSION:-2.42.0}"

arch="$(uname -m)"

# Release assets are mutable, so new versions require reviewed SHA-256 digests.
case "${MOLD_VERSION}:${arch}" in
    2.42.0:aarch64)
        checksum="3c9a0a3624aac8a2007569ae50c33b3129a0f0ae8bcc974aeee2f8939d295190"
        ;;
    2.42.0:x86_64)
        checksum="f5ed2f6e31d1ada4f07fe766fe0de7a73104d1c5cdc59086fcecc16a43720b6d"
        ;;
    *)
        echo "No trusted mold checksum for version ${MOLD_VERSION} (${arch})" >&2
        exit 1
        ;;
esac

url="https://github.com/rui314/mold/releases/download/v${MOLD_VERSION}/mold-${MOLD_VERSION}-${arch}-linux.tar.gz"

echo "Installing mold ${MOLD_VERSION} (${arch})..."

archive="$(mktemp)"
trap 'rm -f "$archive"' EXIT

wget -O "$archive" \
    --timeout=10 \
    --tries=5 \
    --waitretry=3 \
    --retry-connrefused \
    --retry-on-http-error=429,500,502,503,504 \
    --progress=dot:mega \
    "$url"

printf '%s  %s\n' "$checksum" "$archive" | sha256sum -c -

if [ "$(whoami)" = root ]; then
    SUDO=""
else
    SUDO="sudo"
fi

$SUDO tar -C /usr/local --strip-components=1 --no-overwrite-dir -xzf "$archive"

# Make mold the default linker
current_ld="$(realpath /usr/bin/ld)"
if [ "$current_ld" != /usr/local/bin/mold ]; then
    $SUDO ln -sf /usr/local/bin/mold "$current_ld"
fi

echo "mold ${MOLD_VERSION} installed successfully"
mold --version
