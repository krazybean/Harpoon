#!/bin/bash
set -euo pipefail
# Exercises tools/release/macos/verify-release-assets.sh against a mocked `gh`; it contacts nothing and publishes nothing.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TMP="$(mktemp -d /tmp/harpoon-release-assets-regression.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin"
DMG="$TMP/Harpoon-9.9.9-universal.dmg"; printf 'a dmg\n' > "$DMG"
SHA="$(shasum -a 256 "$DMG" | cut -d' ' -f1)"; SIZE="$(stat -f%z "$DMG" 2>/dev/null || stat -c%s "$DMG")"
cat > "$TMP/bin/gh" <<'EOF'
#!/bin/sh
# gh release view TAG --json ... prints $MOCK_JSON ; gh release download ... copies $MOCK_FILE into --dir
case "$2" in
  view) printf '%s\n' "$MOCK_JSON" ;;
  download) while [ $# -gt 0 ]; do [ "$1" = --dir ] && cp "$MOCK_FILE" "$2/$(basename "$MOCK_FILE")"; shift; done ;;
esac
EOF
chmod +x "$TMP/bin/gh"
run() { PATH="$TMP/bin:$PATH" MOCK_JSON="$1" MOCK_FILE="$DMG" bash "$SCRIPT_DIR/verify-release-assets.sh" v9.9.9 "$DMG" 2>&1; }
asset() { printf '{"name":"Harpoon-9.9.9-universal.dmg","state":"%s","size":%s,"digest":"%s"}' "$1" "$2" "$3"; }
expect_pass() { run "{\"isDraft\":false,\"assets\":[$1]}" | grep -q 'PASS' || { echo "FAIL: $2 should pass" >&2; exit 1; }; }
expect_fail() { out="$(run "{\"isDraft\":false,\"assets\":[$1]}" || true)"; echo "$out" | grep -q "FAIL.*$3" || { echo "FAIL: $2 should fail with '$3': $out" >&2; exit 1; }; }

expect_pass "$(asset uploaded "$SIZE" "sha256:$SHA")" "the matching asset"
expect_pass "$(asset uploaded "$SIZE" "")" "an asset GitHub reports no digest for, verified by download"
expect_fail "" "a release with no assets" "no assets"
expect_fail '{"name":"other.dmg","state":"uploaded","size":1,"digest":""}' "a release without the DMG" "no asset named"
expect_fail "$(asset starter "$SIZE" "sha256:$SHA")" "an asset that is not uploaded" "state"
expect_fail "$(asset uploaded 1 "sha256:$SHA")" "an asset of another size" "bytes"
expect_fail "$(asset uploaded "$SIZE" "sha256:$(printf 0%.0s $(seq 64))")" "an asset with another digest" "digest"
echo "release-assets PASS"
