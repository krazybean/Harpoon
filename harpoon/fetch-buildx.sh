#!/bin/sh
set -eu
# Fetch and verify the pinned upstream Docker Buildx so Harpoon-managed builds never depend on
# Docker Desktop, Homebrew, or the user's Docker configuration. Fails closed: any problem leaves
# no output file.
#
#   sh harpoon/fetch-buildx.sh                      arm64 (HARPOON_ARCH=arm64, default)
#   HARPOON_ARCH=universal sh harpoon/fetch-buildx.sh   arm64 + x86_64 joined with lipo
#   sh harpoon/fetch-buildx.sh verify-signature FILE arm64|x86_64
#
# Output: harpoon/build/docker-buildx  (never committed; harpoon/build/ is ignored)
#
# Checks, in order, before anything is repackaged (per slice):
#   1. SHA-256 equals the pin below
#   2. SHA-256 equals the entry in the release's checksums-signed.txt (Darwin digests are NOT in
#      checksums.txt). Skipped only with HARPOON_BUILDX_OFFLINE=1 and a previously cached copy.
#   3. Upstream code signature: valid (--strict), Authority "Developer ID Application: Docker Inc
#      (9BNSXJN65R)", hardened runtime, secure timestamp
#   4. Mach-O architecture is exactly the requested one
# The pins cannot be overridden from the environment. HARPOON_BUILDX_BASE_URL and
# HARPOON_BUILDX_CACHE / HARPOON_BUILDX_OUT exist so tests can point at local files or a scratch dir.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD="$SCRIPT_DIR/build"
CACHE="${HARPOON_BUILDX_CACHE:-$BUILD/buildx-cache}"
OUT="${HARPOON_BUILDX_OUT:-$BUILD/docker-buildx}"

BUILDX_VERSION="0.38.1"
SHA_arm64="6f20ba2437865311ef99015bf5d1039c3c8e4d2177c1c64aa41b134b5ecd3012"
SHA_amd64="76e7a809cc98d34a4c1b7db23feac673def5b8dcdddd9e713ef26598689aa796"
UPSTREAM_TEAM_ID="9BNSXJN65R"
UPSTREAM_AUTHORITY="Developer ID Application: Docker Inc ($UPSTREAM_TEAM_ID)"
BASE_URL="${HARPOON_BUILDX_BASE_URL:-https://github.com/docker/buildx/releases/download/v$BUILDX_VERSION}"

fail() { echo "[buildx] FAIL: $*" >&2; exit 1; }
sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }

# $1 file, $2 lipo arch name (arm64|x86_64)
verify_signature() {
  _f="$1"; _arch="$2"
  [ -f "$_f" ] || fail "no file to verify at $_f"
  codesign --verify --strict "$_f" >/dev/null 2>&1 || fail "upstream code signature of $_f is not valid"
  _info="$(codesign -dv --verbose=4 "$_f" 2>&1)" || fail "cannot read the code signature of $_f"
  printf '%s\n' "$_info" | grep -Fqx "Authority=$UPSTREAM_AUTHORITY" \
    || fail "$_f is not signed by $UPSTREAM_AUTHORITY"
  printf '%s\n' "$_info" | grep -Fqx "TeamIdentifier=$UPSTREAM_TEAM_ID" \
    || fail "$_f has an unexpected TeamIdentifier"
  printf '%s\n' "$_info" | grep -Eq '^CodeDirectory .*flags=0x[0-9a-f]+\(.*runtime' \
    || fail "$_f was not signed with the hardened runtime"
  printf '%s\n' "$_info" | grep -q '^Timestamp=' || fail "$_f has no secure timestamp"
  [ "$(lipo -archs "$_f" 2>/dev/null)" = "$_arch" ] || fail "$_f is not exactly $_arch (found: $(lipo -archs "$_f" 2>/dev/null || echo none))"
}

if [ "${1:-}" = "verify-signature" ]; then
  [ $# -eq 3 ] || fail "usage: fetch-buildx.sh verify-signature FILE arm64|x86_64"
  verify_signature "$2" "$3"
  echo "[buildx] signature OK: $2 ($3)" >&2
  exit 0
fi
[ $# -eq 0 ] || fail "unknown argument $1"

HARPOON_ARCH="${HARPOON_ARCH:-arm64}"
case "$HARPOON_ARCH" in
  arm64) ARCHES="arm64" ;;
  universal) ARCHES="arm64 amd64" ;;
  *) fail "unsupported HARPOON_ARCH $HARPOON_ARCH (arm64|universal)" ;;
esac

# Never leave a previous (possibly different-arch or unverified) result behind.
rm -f "$OUT" "$OUT.part"
mkdir -p "$CACHE" "$(dirname "$OUT")"

SIGNED_SUMS="$CACHE/checksums-signed-$BUILDX_VERSION.txt"
if [ ! -f "$SIGNED_SUMS" ]; then
  [ "${HARPOON_BUILDX_OFFLINE:-0}" = "1" ] && fail "offline mode but no cached checksums-signed.txt for $BUILDX_VERSION"
  echo "[buildx] fetching checksums-signed.txt..." >&2
  curl -fsSL -o "$SIGNED_SUMS.part" "$BASE_URL/checksums-signed.txt" \
    || { rm -f "$SIGNED_SUMS.part"; fail "could not download checksums-signed.txt from $BASE_URL"; }
  mv "$SIGNED_SUMS.part" "$SIGNED_SUMS"
fi

SLICES=""
for A in $ARCHES; do
  NAME="buildx-v$BUILDX_VERSION.darwin-$A"
  BIN="$CACHE/$NAME"
  eval "PIN=\$SHA_$A"
  if [ ! -f "$BIN" ] || [ "$(sha256_of "$BIN")" != "$PIN" ]; then
    rm -f "$BIN"
    echo "[buildx] downloading $NAME..." >&2
    curl -fsSL -o "$BIN.part" "$BASE_URL/$NAME" || { rm -f "$BIN.part"; fail "could not download $NAME from $BASE_URL (missing architecture slice)"; }
    mv "$BIN.part" "$BIN"
  fi
  ACTUAL="$(sha256_of "$BIN")"
  [ "$ACTUAL" = "$PIN" ] || { rm -f "$BIN"; fail "sha256 mismatch for $NAME ($ACTUAL != pinned $PIN)"; }
  LISTED="$(awk -v n="*$NAME" '$2 == n {print $1}' "$SIGNED_SUMS")"
  [ -n "$LISTED" ] || fail "$NAME is not listed in checksums-signed.txt"
  [ "$LISTED" = "$PIN" ] || fail "checksums-signed.txt disagrees with the pin for $NAME ($LISTED != $PIN)"
  case "$A" in arm64) LIPO_ARCH=arm64 ;; amd64) LIPO_ARCH=x86_64 ;; esac
  verify_signature "$BIN" "$LIPO_ARCH"
  SLICES="$SLICES $BIN"
done

# shellcheck disable=SC2086
lipo -create $SLICES -output "$OUT.part" || { rm -f "$OUT.part"; fail "lipo failed"; }
case "$HARPOON_ARCH" in arm64) WANT="arm64" ;; universal) WANT="x86_64 arm64" ;; esac
GOT="$(lipo -archs "$OUT.part")"
[ "$GOT" = "$WANT" ] || { rm -f "$OUT.part"; fail "assembled binary has architectures '$GOT', expected '$WANT'"; }
chmod 755 "$OUT.part"
mv "$OUT.part" "$OUT"
echo "[buildx] $OUT (v$BUILDX_VERSION, $GOT)" >&2
