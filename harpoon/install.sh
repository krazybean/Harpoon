#!/bin/sh
set -eu
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# Determine source: if run from staged dist, SCRIPT_DIR is dist/harpoon-*/ and contains bin/lib
# If run from repo, SCRIPT_DIR is harpoon/
if [ -d "$SCRIPT_DIR/bin" ] && [ -d "$SCRIPT_DIR/lib" ]; then
  SRC="$SCRIPT_DIR"
else
  SRC="$(cd "$SCRIPT_DIR/../dist/harpoon-0.1.0-darwin-arm64" && pwd 2>/dev/null || cd "$SCRIPT_DIR/../dist/harpoon-0.1.0-dev-darwin-arm64" && pwd 2>/dev/null || echo "$SCRIPT_DIR/../dist/harpoon-0.1.0-darwin-arm64")"
fi
if [ ! -d "$SRC/bin" ]; then echo "[install] staged bin not found at $SRC/bin" >&2; exit 1; fi
PREFIX="/usr/local"
if [ "$(uname -m)" != "arm64" ]; then echo "[install] only arm64 supported" >&2; exit 1; fi
# Check if Harpoon is running
if [ -f /tmp/harpoon.lock ] && /usr/bin/lsof /tmp/harpoon.lock >/dev/null 2>&1; then
  if harpoon status 2>&1 | grep -q "running" 2>/dev/null || [ -S /tmp/harpoon-docker.sock ]; then
    echo "[install] Harpoon is running. Please run 'harpoon stop' before installing." >&2
    exit 1
  fi
fi
echo "[install] installing to $PREFIX..." >&2
# need privilege for /usr/local
if [ ! -w "$PREFIX/bin" ] && [ ! -w "$PREFIX" ]; then
  echo "[install] need sudo for $PREFIX" >&2
  SUDO="sudo"
else
  SUDO=""
fi
$SUDO mkdir -p "$PREFIX/bin" "$PREFIX/lib/harpoon"
$SUDO cp "$SRC/bin/harpoon" "$PREFIX/bin/harpoon"
$SUDO chmod +x "$PREFIX/bin/harpoon"
$SUDO cp "$SRC/lib/harpoon/Image-virt" "$PREFIX/lib/harpoon/Image-virt"
$SUDO cp "$SRC/lib/harpoon/harpoon-initramfs.cpio.gz" "$PREFIX/lib/harpoon/harpoon-initramfs.cpio.gz"
# clone-aware for sparse root image
if $SUDO cp -c "$SRC/lib/harpoon/harpoon-root.img" "$PREFIX/lib/harpoon/harpoon-root.img" 2>/dev/null; then :; elif $SUDO ditto "$SRC/lib/harpoon/harpoon-root.img" "$PREFIX/lib/harpoon/harpoon-root.img" 2>/dev/null; then :; else $SUDO cp "$SRC/lib/harpoon/harpoon-root.img" "$PREFIX/lib/harpoon/harpoon-root.img"; fi
$SUDO chmod 644 "$PREFIX/lib/harpoon/"*
# verify
if ! codesign --verify --verbose "$PREFIX/bin/harpoon" 2>&1 | grep -q "valid on disk"; then echo "[install] warning: installed binary not valid" >&2; fi
if ! codesign -d --entitlements :- "$PREFIX/bin/harpoon" 2>&1 | grep -q "com.apple.security.virtualization"; then echo "[install] warning: entitlement missing" >&2; fi
echo "[install] installed:" >&2
ls -lh "$PREFIX/bin/harpoon" "$PREFIX/lib/harpoon/"* 2>&1 | tail -n 10 >&2
echo "[install] done. Try: harpoon version; harpoon doctor" >&2
echo "[install] user data at ~/Library/Application Support/Harpoon/ (preserved)" >&2
