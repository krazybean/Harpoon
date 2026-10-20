#!/bin/sh
set -eu
# Fetch Docker's official static macOS CLI so Harpoon.app works on Macs without
# a `docker` install (Homebrew no longer ships Intel bottles). Pinned by sha256.
# HARPOON_ARCH=arm64 (default) keeps the aarch64 build; universal joins both with lipo.
# Output: harpoon/build/docker
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD="$SCRIPT_DIR/build"
CACHE="$BUILD/docker-cli-cache"
OUT="$BUILD/docker"
DOCKER_CLI_VERSION="29.9.1"
SHA_aarch64="7f2c1637313578046e7dd61153e4e4cd1188d83a1e4b71825ab914776c208460"
SHA_x86_64="9fc063bfae58ecf3c46e7528beaad9814968f7e624e9ba04a7f5cba5becad26c"

HARPOON_ARCH="${HARPOON_ARCH:-arm64}"
case "$HARPOON_ARCH" in
  arm64) ARCHES="aarch64" ;;
  universal) ARCHES="aarch64 x86_64" ;;
  *) echo "[docker-cli] FAIL: unsupported HARPOON_ARCH $HARPOON_ARCH (arm64|universal)" >&2; exit 1 ;;
esac

mkdir -p "$CACHE"
SLICES=""
for ARCH in $ARCHES; do
  TGZ="$CACHE/docker-$DOCKER_CLI_VERSION-$ARCH.tgz"
  eval "EXPECTED=\$SHA_$ARCH"
  if [ ! -f "$TGZ" ] || [ "$(shasum -a 256 "$TGZ" | cut -d' ' -f1)" != "$EXPECTED" ]; then
    echo "[docker-cli] downloading Docker CLI $DOCKER_CLI_VERSION ($ARCH)..." >&2
    curl -fsSL -o "$TGZ.part" "https://download.docker.com/mac/static/stable/$ARCH/docker-$DOCKER_CLI_VERSION.tgz"
    mv "$TGZ.part" "$TGZ"
  fi
  ACTUAL="$(shasum -a 256 "$TGZ" | cut -d' ' -f1)"
  [ "$ACTUAL" = "$EXPECTED" ] || { echo "[docker-cli] FAIL: sha256 mismatch for $TGZ ($ACTUAL != $EXPECTED)" >&2; rm -f "$TGZ"; exit 1; }
  rm -rf "$CACHE/$ARCH"
  mkdir -p "$CACHE/$ARCH"
  tar -xzf "$TGZ" -C "$CACHE/$ARCH" docker/docker
  SLICES="$SLICES $CACHE/$ARCH/docker/docker"
done

# shellcheck disable=SC2086
lipo -create $SLICES -output "$OUT"
chmod +x "$OUT"
echo "[docker-cli] $OUT ($DOCKER_CLI_VERSION, $(lipo -archs "$OUT"))" >&2
