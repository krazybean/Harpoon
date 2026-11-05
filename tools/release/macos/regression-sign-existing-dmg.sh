#!/bin/bash
set -euo pipefail
# Exercises tools/release/macos/sign-existing-dmg.sh on a small synthetic DMG with a stand-in signer (ad-hoc, no
# Developer ID, no Keychain) and a stand-in verifier. It proves the shadow-file plumbing and its failure handling; it never
# touches a real release, contacts Apple, or uses a signing identity.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TMP="$(mktemp -d /tmp/harpoon-sign-dmg-regression.XXXXXX)"
test_cleanup() { { mount | grep -F "$TMP" | awk '{print $3}' | while read -r m; do hdiutil detach -force "$m" >/dev/null 2>&1 || true; done; } || true; rm -rf "$TMP"; }
trap test_cleanup EXIT

# shellcheck disable=SC1091
source "$SCRIPT_DIR/sign-existing-dmg.sh"

fail() { echo "FAIL: $*" >&2; exit 1; }
mounts() { mount | grep -cF "$TMP" || true; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

# ---- a synthetic input: Harpoon.app with the four code files, a seal file, and bulky resources ---------------------
SRC="$TMP/src"; APP="$SRC/Harpoon.app/Contents"
mkdir -p "$APP/MacOS" "$APP/Resources/harpoon/bin" "$APP/Resources/harpoon/lib" "$APP/_CodeSignature"
for f in MacOS/harpoon-desktop Resources/harpoon/bin/harpoon Resources/harpoon/bin/docker Resources/harpoon/bin/docker-buildx; do
  cp /usr/bin/true "$APP/$f"; chmod u+w "$APP/$f"
done
echo "seal v1" > "$APP/_CodeSignature/CodeResources"
head -c 4194304 /dev/urandom > "$APP/Resources/harpoon/lib/Image-virt"
dd if=/dev/zero of="$APP/Resources/harpoon/lib/harpoon-root.img" bs=1m count=16 2>/dev/null
ln -s /Applications "$SRC/Applications"
mkdir -p "$TMP/in" "$TMP/out"
hdiutil create -size 96m -fs HFS+ -volname Harpoon -srcfolder "$SRC" -ov -format UDZO "$TMP/in/Harpoon-test.dmg" >/dev/null
IN="$TMP/in/Harpoon-test.dmg"; IN_SHA="$(sha "$IN")"

# ---- stand-ins -----------------------------------------------------------------------------------------------------
GOOD_SIGNER_CALLS=0
app_signer() { # ad-hoc re-sign the four code files and update the seal, as the real signer changes exactly those
  local a="$1" f
  for f in MacOS/harpoon-desktop Resources/harpoon/bin/harpoon Resources/harpoon/bin/docker Resources/harpoon/bin/docker-buildx; do
    codesign --force --sign - "$a/Contents/$f" >/dev/null 2>&1
  done
  echo "seal v2" > "$a/Contents/_CodeSignature/CodeResources"
}
app_verifier() { codesign --verify "$1/Contents/MacOS/harpoon-desktop" >/dev/null 2>&1; }
free_kib() { echo 999999999; } # plenty, unless a test lowers it

run() { # run OUT_DIR EXPECT STAGE : in a subshell, with the real cleanup trap; prints the failure message
  ( trap cleanup EXIT; sign_dmg_run "$IN" "$1" "$TMP/out" "$2" "$3" ) 2>"$TMP/err" && return 0 || return 1
}
assert_clean() { # nothing mounted, no temp directory left, input unchanged
  [ "$(mounts)" = 0 ] || fail "$1: a volume is still mounted"
  [ -z "$(ls -d "$TMP/out"/sign-dmg.* 2>/dev/null)" ] || fail "$1: a temp directory is left"
  [ "$(sha "$IN")" = "$IN_SHA" ] || fail "$1: the input changed"
}

# 1. success: output exists, input untouched, only code and seal differ, shadow gone
run "$TMP/out" "$IN_SHA" sign || { cat "$TMP/err" >&2; fail "the good run failed"; }
OUT="$TMP/out/Harpoon-test-signed.dmg"
[ -f "$OUT" ] || fail "no output"
assert_clean "success"
[ -z "$(ls "$TMP/out"/*.shadow 2>/dev/null)" ] || fail "a shadow file is left"
mkdir -p "$TMP/m1" "$TMP/m2"
hdiutil attach -readonly -nobrowse -noverify -mountpoint "$TMP/m1" "$IN" >/dev/null
hdiutil attach -readonly -nobrowse -noverify -mountpoint "$TMP/m2" "$OUT" >/dev/null
[ "$(cat "$TMP/m2/Harpoon.app/Contents/_CodeSignature/CodeResources")" = "seal v2" ] || fail "the seal was not updated"
cmp -s "$TMP/m1/Harpoon.app/Contents/Resources/harpoon/lib/Image-virt" "$TMP/m2/Harpoon.app/Contents/Resources/harpoon/lib/Image-virt" || fail "a resource changed"
cmp -s "$TMP/m1/Harpoon.app/Contents/Resources/harpoon/lib/harpoon-root.img" "$TMP/m2/Harpoon.app/Contents/Resources/harpoon/lib/harpoon-root.img" || fail "the root image changed"
cmp -s "$TMP/m1/Harpoon.app/Contents/MacOS/harpoon-desktop" "$TMP/m2/Harpoon.app/Contents/MacOS/harpoon-desktop" && fail "the code was not re-signed"
[ -L "$TMP/m2/Applications" ] || fail "the Applications link is gone"
hdiutil detach "$TMP/m1" >/dev/null; hdiutil detach "$TMP/m2" >/dev/null
echo "ok: success path"

# 2. an output that exists is never overwritten
before="$(sha "$OUT")"; run "$TMP/out" "$IN_SHA" sign && fail "overwrote an existing output"
grep -q "never overwritten" "$TMP/err" || fail "wrong message for an existing output"; [ "$(sha "$OUT")" = "$before" ] || fail "the existing output changed"
assert_clean "existing output"; echo "ok: existing output refused"
rm -f "$OUT"

# 3. a wrong expected checksum is refused before anything is mounted
run "$TMP/out" "0000000000000000000000000000000000000000000000000000000000000000" sign && fail "accepted a wrong checksum"
grep -q "does not equal the expected" "$TMP/err" || fail "wrong message for a checksum mismatch"; assert_clean "checksum"; [ ! -e "$OUT" ] || fail "an output exists"
echo "ok: checksum mismatch refused"

# 4. insufficient headroom is refused before anything is mounted
free_kib() { echo 1000; }; run "$TMP/out" "$IN_SHA" sign && fail "ran without headroom"
free_kib() { echo 999999999; }
grep -q "not enough free space" "$TMP/err" || fail "wrong message for low headroom"; assert_clean "headroom"; [ ! -e "$OUT" ] || fail "an output exists"
echo "ok: low headroom refused"

# 5. a signer that fails leaves no output, no mount, no shadow, and the input intact
good_signer="$(declare -f app_signer)"
app_signer() { return 1; }; run "$TMP/out" "$IN_SHA" sign && fail "a failing signer succeeded"
eval "$good_signer"; assert_clean "failing signer"; [ ! -e "$OUT" ] || fail "an output exists after a failed signer"
echo "ok: signer failure cleaned up"

# 6. a signer that changes a file that is not code is caught after conversion, and the output is removed
app_signer() { echo tampered > "$1/Contents/Resources/harpoon/lib/Image-virt"; echo "seal v2" > "$1/Contents/_CodeSignature/CodeResources"; }
run "$TMP/out" "$IN_SHA" sign && fail "a change to a resource was accepted"
eval "$good_signer"
grep -q "signing changed a file that is not code" "$TMP/err" || { cat "$TMP/err" >&2; fail "wrong message for a changed resource"; }
assert_clean "changed resource"; [ ! -e "$OUT" ] || fail "an output exists after a changed resource"
echo "ok: a changed resource is caught and the output removed"

# 7. a dry run mounts and writes nothing
run "$TMP/out" "$IN_SHA" dry || { cat "$TMP/err" >&2; fail "the dry run failed"; }
assert_clean "dry run"; [ ! -e "$OUT" ] || fail "a dry run wrote an output"
echo "ok: dry run"

# 8. an existing shadow file is never reused
SHADOW_PROBE="$TMP/probe.shadow"; : > "$SHADOW_PROBE"
( trap cleanup EXIT; attach shadow "$IN" "$TMP/probe-mnt" "$SHADOW_PROBE" ) 2>"$TMP/err" && fail "reused a shadow"
grep -q "never reused" "$TMP/err" || fail "wrong message for an existing shadow"; assert_clean "existing shadow"
echo "ok: an existing shadow is refused"

# 9. through the command line: a dry run needs no identity and writes nothing; signing without one is refused up front
env -u HARPOON_SIGN_IDENTITY bash "$SCRIPT_DIR/sign-existing-dmg.sh" --dry-run "$IN" --out-dir "$TMP/out" --expect-sha256 "$IN_SHA" 2>"$TMP/err" || { cat "$TMP/err" >&2; fail "the command-line dry run failed"; }
assert_clean "cli dry run"; [ ! -e "$OUT" ] || fail "the command-line dry run wrote an output"
env -u HARPOON_SIGN_IDENTITY bash "$SCRIPT_DIR/sign-existing-dmg.sh" "$IN" --out-dir "$TMP/out" 2>"$TMP/err" && fail "signed without an identity"
grep -q "HARPOON_SIGN_IDENTITY is not set" "$TMP/err" || fail "wrong message without an identity"; assert_clean "no identity"
echo "ok: command line"

echo "sign-existing-dmg regression PASS"
