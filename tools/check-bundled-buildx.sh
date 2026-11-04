#!/bin/bash
set -uo pipefail
# Verify the Docker Buildx bundled in Harpoon.app (Contents/Resources/harpoon/bin/docker-buildx).
#   bash tools/check-bundled-buildx.sh APP            pre-release: present, executable, exact architectures,
#                                                     valid signature (ad-hoc or Developer ID), license shipped,
#                                                     runs hermetically; a Developer ID signature must also have
#                                                     the hardened runtime and a secure timestamp
#   bash tools/check-bundled-buildx.sh APP --release  release: additionally Developer ID Application, hardened
#                                                     runtime, secure timestamp, and the SAME TeamIdentifier as
#                                                     Harpoon.app (override: HARPOON_EXPECT_TEAM_ID) — i.e. it
#                                                     was re-signed by Harpoon, not left with Docker Inc.'s signature
# HARPOON_ARCH=universal expects arm64 + x86_64; anything else expects exactly arm64.
# Used by verify-bundle.sh and release/macos/verify-signatures.sh. Runs `docker-buildx version` only
# (no daemon, no build) with an empty HOME and DOCKER_CONFIG.
APP="${1:?usage: check-bundled-buildx.sh APP [--release]}"
MODE="${2:-pre-release}"
case "$MODE" in pre-release|--release) ;; *) echo "[check-buildx] FAIL: unknown mode $MODE" >&2; exit 2 ;; esac
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$APP/Contents/Resources/harpoon/bin/docker-buildx"
LICENSE_FILE="$APP/Contents/Resources/harpoon/share/licenses/docker-buildx/LICENSE"
WANT_VERSION="$(sed -n 's/^BUILDX_VERSION="\(.*\)"$/\1/p' "$REPO_ROOT/harpoon/fetch-buildx.sh")"

FAIL=0
pass() { echo "[check-buildx] PASS: $1" >&2; }
fail() { echo "[check-buildx] FAIL: $1" >&2; FAIL=1; }
finish() {
  if [ "$FAIL" -ne 0 ]; then echo "[check-buildx] FAIL: bundled Docker Buildx verification failed" >&2; exit 1; fi
  echo "[check-buildx] PASS: bundled Docker Buildx verified ($MODE)" >&2; exit 0
}

if [ ! -e "$BIN" ] && [ ! -L "$BIN" ]; then
  fail "bundled Docker Buildx is missing: $BIN — Harpoon does not fall back to Docker Desktop or the legacy builder; run 'sh harpoon/fetch-buildx.sh' and 'bash ui/harpoon-desktop/src-tauri/prepare-bundle.sh', then rebuild"
  finish
fi
if [ -L "$BIN" ] || [ ! -f "$BIN" ]; then fail "docker-buildx must be a regular file, not a symlink or directory"; finish; fi
if [ -x "$BIN" ]; then pass "docker-buildx is executable"; else fail "docker-buildx is not executable (chmod 755 or re-run prepare-bundle.sh)"; finish; fi

# architectures: exact set
ARCHS="$(lipo -archs "$BIN" 2>/dev/null || true)"
if [ "${HARPOON_ARCH:-arm64}" = "universal" ]; then WANT="arm64 x86_64"; else WANT="arm64"; fi
if [ "$(printf '%s\n' $ARCHS | sort | tr '\n' ' ')" = "$(printf '%s\n' $WANT | sort | tr '\n' ' ')" ]; then
  pass "architectures: $ARCHS"
else
  fail "architectures are '${ARCHS:-none}', expected '$WANT'"
fi

# signature
if codesign --verify --strict "$BIN" >/dev/null 2>&1; then pass "code signature valid (strict)"; else fail "code signature missing or invalid"; fi
INFO="$(codesign -dv --verbose=4 "$BIN" 2>&1)"
ADHOC=0; printf '%s\n' "$INFO" | grep -q 'flags=0x[0-9a-f]*(.*adhoc' && ADHOC=1
HARDENED=0; printf '%s\n' "$INFO" | grep -Eq 'flags=0x[0-9a-f]+\(.*runtime' && HARDENED=1
TIMESTAMP=0; printf '%s\n' "$INFO" | grep -q '^Timestamp=' && TIMESTAMP=1
DEVID=0; printf '%s\n' "$INFO" | grep -q '^Authority=Developer ID Application' && DEVID=1
TEAM="$(printf '%s\n' "$INFO" | sed -n 's/^TeamIdentifier=//p')"
if [ "$MODE" = "--release" ]; then
  [ "$ADHOC" -eq 0 ] && pass "not ad-hoc" || fail "docker-buildx is ad-hoc signed"
  [ "$DEVID" -eq 1 ] && pass "Authority is Developer ID Application" || fail "docker-buildx is not signed with a Developer ID Application certificate"
  [ "$HARDENED" -eq 1 ] && pass "hardened runtime" || fail "docker-buildx lacks the hardened runtime (sign with --options runtime)"
  [ "$TIMESTAMP" -eq 1 ] && pass "secure timestamp" || fail "docker-buildx has no secure timestamp (sign with --timestamp)"
  EXPECT="${HARPOON_EXPECT_TEAM_ID:-$(codesign -dv --verbose=4 "$APP" 2>&1 | sed -n 's/^TeamIdentifier=//p')}"
  if [ -z "$EXPECT" ] || [ "$EXPECT" = "not set" ]; then
    fail "cannot determine Harpoon.app's TeamIdentifier to compare with docker-buildx"
  elif [ "$TEAM" = "$EXPECT" ]; then
    pass "TeamIdentifier $TEAM matches Harpoon.app"
  else
    fail "docker-buildx TeamIdentifier '$TEAM' is not Harpoon.app's '$EXPECT' (still carrying the upstream signature?)"
  fi
elif [ "$ADHOC" -eq 0 ] && [ "$DEVID" -eq 1 ]; then
  [ "$HARDENED" -eq 1 ] && pass "Developer ID signature has the hardened runtime" || fail "Developer ID signature lacks the hardened runtime"
  [ "$TIMESTAMP" -eq 1 ] && pass "Developer ID signature has a secure timestamp" || fail "Developer ID signature lacks a secure timestamp"
else
  echo "[check-buildx] INFO: ad-hoc/development signature (release signing: tools/release/macos/verify-signatures.sh)" >&2
fi

# license
if [ -s "$LICENSE_FILE" ] && grep -q "Apache License" "$LICENSE_FILE"; then pass "Apache-2.0 license shipped"; else fail "Apache-2.0 license missing at $LICENSE_FILE"; fi

# hermetic version check (never a build, never a daemon)
if [ "$FAIL" -eq 0 ]; then
  TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
  OUT="$(env -i PATH=/usr/bin:/bin HOME="$TMP" DOCKER_CONFIG="$TMP" DOCKER_HOST=unix:///nonexistent "$BIN" version 2>&1 || true)"
  if printf '%s\n' "$OUT" | grep -q "v$WANT_VERSION"; then pass "runs hermetically: $(printf '%s\n' "$OUT" | head -n 1)"; else fail "'docker-buildx version' did not report v$WANT_VERSION: $OUT"; fi
fi
finish
