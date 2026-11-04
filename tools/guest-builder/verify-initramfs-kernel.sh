#!/bin/bash
set -euo pipefail

ARCHIVE=${1:?usage: $0 ARCHIVE EXPECTED_KERNEL}
EXPECTED_KERNEL=${2:?usage: $0 ARCHIVE EXPECTED_KERNEL}
[ -f "$ARCHIVE" ] || { echo "[verify-initramfs-kernel] FAIL: missing $ARCHIVE" >&2; exit 1; }
LISTING=$(gzip -dc "$ARCHIVE" | cpio -it 2>/dev/null)
VERSIONS=$(printf '%s\n' "$LISTING" | sed 's#^\./##' | awk -F/ '$1 == "lib" && $2 == "modules" && $4 == "modules.dep" {print $3}' | LC_ALL=C sort -u)
if [ "$(printf '%s\n' "$VERSIONS" | sed '/^$/d' | wc -l | tr -d ' ')" != "1" ]; then
  echo "[verify-initramfs-kernel] FAIL: expected one module release, found: ${VERSIONS:-none}" >&2
  exit 1
fi
[ "$VERSIONS" = "$EXPECTED_KERNEL" ] || { echo "[verify-initramfs-kernel] FAIL: module release $VERSIONS != kernel $EXPECTED_KERNEL" >&2; exit 1; }
echo "[verify-initramfs-kernel] PASS: modules match kernel $EXPECTED_KERNEL" >&2
