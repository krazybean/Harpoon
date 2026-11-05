#!/bin/bash
set -euo pipefail
# Exercises tools/release/macos/sign-existing-dmg.sh on a small synthetic DMG with a stand-in signer (ad-hoc, no
# Developer ID, no Keychain) and a stand-in verifier. It proves the shadow-file plumbing and its failure handling; it never
# touches a real release, contacts Apple, or uses a signing identity.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TMP="$(mktemp -d /tmp/harpoon-sign-dmg-regression.XXXXXX)"
OVERALL_SECONDS=480 # the CI job allows 10 minutes; fail with diagnostics well before it is killed silently
WATCHDOG=""

ts() { date -u +%H:%M:%S; }
scenario() { echo "[regression $(ts)] === scenario: $* ===" >&2; CURRENT_SCENARIO="$*"; }
CURRENT_SCENARIO="(setup)"

# Process and mount state, for a run that failed or was cut short. Read-only; limited to this test's own paths.
diagnostics() {
  echo "[regression $(ts)] --- diagnostics (last scenario: $CURRENT_SCENARIO) ---" >&2
  { sw_vers; df -h /tmp | tail -2; } 2>&1 | sed 's/^/  /' >&2 || true
  echo "  mounts under $TMP:" >&2; { mount | grep -F "$TMP" || echo "  (none)"; } 2>&1 | sed 's/^/    /' >&2 || true
  echo "  files under $TMP:" >&2; { ls -la "$TMP" "$TMP/out" 2>&1 | head -30; } | sed 's/^/    /' >&2 || true
  echo "  descendants of this test:" >&2; { ps -axo pid,ppid,etime,stat,command | awk -v me="$$" 'NR==1 || $2==me' ; } 2>&1 | sed 's/^/    /' >&2 || true
  echo "  disk-image processes on the host:" >&2; { ps -axo pid,ppid,etime,stat,command | grep -E "hdiutil|diskimage|mdutil|codesign" | grep -v grep | head -20; } 2>&1 | sed 's/^/    /' >&2 || true
  echo "  hdiutil info (bounded):" >&2
  { hdiutil info & hp=$!; ( sleep 20; kill "$hp" 2>/dev/null ) >/dev/null 2>&1 & hw=$!; wait "$hp" 2>/dev/null; kill "$hw" 2>/dev/null; } 2>&1 | grep -E "image-path|/dev/disk|mount" | head -20 | sed 's/^/    /' >&2 || true
}
# Terminate only what belongs to this test: its own child processes and the volumes mounted under its own directory.
test_cleanup() {
  local status=$?
  [ -z "$WATCHDOG" ] || { kill "$WATCHDOG" 2>/dev/null; wait "$WATCHDOG" 2>/dev/null; } || true
  if [ "$status" -ne 0 ]; then diagnostics; fi
  { pkill -TERM -P $$ 2>/dev/null || true; } >/dev/null 2>&1
  { mount | grep -F "$TMP" | awk '{print $3}' | while read -r m; do hdiutil detach -force "$m" >/dev/null 2>&1 || true; done; } || true
  rm -rf "$TMP"
  exit "$status"
}
trap test_cleanup EXIT
trap 'echo "[regression $(ts)] OVERALL TIMEOUT after ${OVERALL_SECONDS}s" >&2; exit 124' TERM
MAIN_PID=$$
( sp=""; trap 'kill "$sp" 2>/dev/null; exit 0' TERM; sleep "$OVERALL_SECONDS" & sp=$!; wait "$sp"; kill -TERM "$MAIN_PID" 2>/dev/null ) >/dev/null 2>&1 & WATCHDOG=$!

# shellcheck disable=SC1091
source "$SCRIPT_DIR/sign-existing-dmg.sh"
HDIUTIL_TIMEOUT=120 # a synthetic image needs seconds; a hang should fail fast, with the command named

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
scenario "build the synthetic input image"
run_bounded 120 hdiutil create -size 96m -fs HFS+ -volname Harpoon -srcfolder "$SRC" -ov -format UDZO "$TMP/in/Harpoon-test.dmg" >/dev/null || { echo "FAIL: could not create the synthetic image" >&2; exit 1; }
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

scenario "1. success: output exists, input untouched, only code and seal differ, shadow gone"
# 1. success: output exists, input untouched, only code and seal differ, shadow gone
run "$TMP/out" "$IN_SHA" sign || { cat "$TMP/err" >&2; fail "the good run failed"; }
OUT="$TMP/out/Harpoon-test-signed.dmg"
[ -f "$OUT" ] || fail "no output"
assert_clean "success"
[ -z "$(ls "$TMP/out"/*.shadow 2>/dev/null)" ] || fail "a shadow file is left"
mkdir -p "$TMP/m1" "$TMP/m2"
run_bounded 120 hdiutil attach -readonly -nobrowse -noverify -mountpoint "$TMP/m1" "$IN" >/dev/null || fail "could not mount the input"
run_bounded 120 hdiutil attach -readonly -nobrowse -noverify -mountpoint "$TMP/m2" "$OUT" >/dev/null || fail "could not mount the output"
[ "$(cat "$TMP/m2/Harpoon.app/Contents/_CodeSignature/CodeResources")" = "seal v2" ] || fail "the seal was not updated"
cmp -s "$TMP/m1/Harpoon.app/Contents/Resources/harpoon/lib/Image-virt" "$TMP/m2/Harpoon.app/Contents/Resources/harpoon/lib/Image-virt" || fail "a resource changed"
cmp -s "$TMP/m1/Harpoon.app/Contents/Resources/harpoon/lib/harpoon-root.img" "$TMP/m2/Harpoon.app/Contents/Resources/harpoon/lib/harpoon-root.img" || fail "the root image changed"
cmp -s "$TMP/m1/Harpoon.app/Contents/MacOS/harpoon-desktop" "$TMP/m2/Harpoon.app/Contents/MacOS/harpoon-desktop" && fail "the code was not re-signed"
[ -L "$TMP/m2/Applications" ] || fail "the Applications link is gone"
run_bounded 120 hdiutil detach "$TMP/m1" >/dev/null || fail "could not detach m1"; run_bounded 120 hdiutil detach "$TMP/m2" >/dev/null || fail "could not detach m2"
echo "ok: success path"

scenario "2. an output that exists is never overwritten"
# 2. an output that exists is never overwritten
before="$(sha "$OUT")"; run "$TMP/out" "$IN_SHA" sign && fail "overwrote an existing output"
grep -q "never overwritten" "$TMP/err" || fail "wrong message for an existing output"; [ "$(sha "$OUT")" = "$before" ] || fail "the existing output changed"
assert_clean "existing output"; echo "ok: existing output refused"
rm -f "$OUT"

scenario "3. a wrong expected checksum is refused before anything is mounted"
# 3. a wrong expected checksum is refused before anything is mounted
run "$TMP/out" "0000000000000000000000000000000000000000000000000000000000000000" sign && fail "accepted a wrong checksum"
grep -q "does not equal the expected" "$TMP/err" || fail "wrong message for a checksum mismatch"; assert_clean "checksum"; [ ! -e "$OUT" ] || fail "an output exists"
echo "ok: checksum mismatch refused"

scenario "4. insufficient headroom is refused before anything is mounted"
# 4. insufficient headroom is refused before anything is mounted
free_kib() { echo 1000; }; run "$TMP/out" "$IN_SHA" sign && fail "ran without headroom"
free_kib() { echo 999999999; }
grep -q "not enough free space" "$TMP/err" || fail "wrong message for low headroom"; assert_clean "headroom"; [ ! -e "$OUT" ] || fail "an output exists"
echo "ok: low headroom refused"

scenario "5. a signer that fails leaves no output, no mount, no shadow, and the input intact"
# 5. a signer that fails leaves no output, no mount, no shadow, and the input intact
good_signer="$(declare -f app_signer)"
app_signer() { return 1; }; run "$TMP/out" "$IN_SHA" sign && fail "a failing signer succeeded"
eval "$good_signer"; assert_clean "failing signer"; [ ! -e "$OUT" ] || fail "an output exists after a failed signer"
echo "ok: signer failure cleaned up"

scenario "6. a signer that changes a file that is not code is caught after conversion, and the output is removed"
# 6. a signer that changes a file that is not code is caught after conversion, and the output is removed
app_signer() { echo tampered > "$1/Contents/Resources/harpoon/lib/Image-virt"; echo "seal v2" > "$1/Contents/_CodeSignature/CodeResources"; }
run "$TMP/out" "$IN_SHA" sign && fail "a change to a resource was accepted"
eval "$good_signer"
grep -q "signing changed a file that is not code" "$TMP/err" || { cat "$TMP/err" >&2; fail "wrong message for a changed resource"; }
assert_clean "changed resource"; [ ! -e "$OUT" ] || fail "an output exists after a changed resource"
echo "ok: a changed resource is caught and the output removed"

scenario "7. a dry run mounts and writes nothing"
# 7. a dry run mounts and writes nothing
run "$TMP/out" "$IN_SHA" dry || { cat "$TMP/err" >&2; fail "the dry run failed"; }
assert_clean "dry run"; [ ! -e "$OUT" ] || fail "a dry run wrote an output"
echo "ok: dry run"

scenario "8. an existing shadow file is never reused"
# 8. an existing shadow file is never reused
SHADOW_PROBE="$TMP/probe.shadow"; : > "$SHADOW_PROBE"
( trap cleanup EXIT; attach shadow "$IN" "$TMP/probe-mnt" "$SHADOW_PROBE" ) 2>"$TMP/err" && fail "reused a shadow"
grep -q "never reused" "$TMP/err" || fail "wrong message for an existing shadow"; assert_clean "existing shadow"
echo "ok: an existing shadow is refused"

scenario "9. through the command line: a dry run needs no identity and writes nothing; signing without one is refused up front"
# 9. through the command line: a dry run needs no identity and writes nothing; signing without one is refused up front
env -u HARPOON_SIGN_IDENTITY bash "$SCRIPT_DIR/sign-existing-dmg.sh" --dry-run "$IN" --out-dir "$TMP/out" --expect-sha256 "$IN_SHA" 2>"$TMP/err" || { cat "$TMP/err" >&2; fail "the command-line dry run failed"; }
assert_clean "cli dry run"; [ ! -e "$OUT" ] || fail "the command-line dry run wrote an output"
env -u HARPOON_SIGN_IDENTITY bash "$SCRIPT_DIR/sign-existing-dmg.sh" "$IN" --out-dir "$TMP/out" 2>"$TMP/err" && fail "signed without an identity"
grep -q "HARPOON_SIGN_IDENTITY is not set" "$TMP/err" || fail "wrong message without an identity"; assert_clean "no identity"
echo "ok: command line"

scenario "10. the watchdog: a command that blocks is ended after its limit, with its own process only"
( run_bounded 2 sleep 30 ) 2>"$TMP/err" && fail "a blocked command was not ended"
[ "$?" = 0 ] || true
grep -q "TIMEOUT after 2s: sleep 30" "$TMP/err" || { cat "$TMP/err" >&2; fail "the timeout was not reported with the command"; }
[ -z "$(pgrep -fx 'sleep 30' || true)" ] || fail "the blocked command is still running"
rc=0; ( run_bounded 5 true ) 2>/dev/null || rc=$?; [ "$rc" = 0 ] || fail "a quick command was reported as failed"
rc=0; ( run_bounded 5 sh -c 'exit 3' ) 2>/dev/null || rc=$?; [ "$rc" = 3 ] || fail "a failing command's status was lost ($rc)"
echo "ok: watchdog"

scenario "11. a hung disk-image command fails the run with a message, and nothing is left mounted or running"
real_hdiutil="$(command -v hdiutil)"
hdiutil() { if [ "$1" = attach ]; then sleep 120; else "$real_hdiutil" "$@"; fi; } # attach never returns
HDIUTIL_TIMEOUT=20; started="$(date +%s)" # long enough for the real verify that precedes the attach
run "$TMP/out" "$IN_SHA" sign && fail "a hung attach succeeded"
elapsed=$(( $(date +%s) - started )); unset -f hdiutil; HDIUTIL_TIMEOUT=120
[ "$elapsed" -lt 100 ] || fail "the hung attach took ${elapsed}s to fail"
grep -q "TIMEOUT after 20s: hdiutil attach" "$TMP/err" || { cat "$TMP/err" >&2; fail "the hung attach was not named in the failure"; }
[ -z "$(pgrep -fx 'sleep 120' || true)" ] || fail "the stub that blocked the attach is still running"
assert_clean "hung attach"; [ ! -e "$OUT" ] || fail "an output exists after a hung attach"
echo "ok: hung attach is bounded"

scenario "12. comparing two images never follows a symlink out of the image (the real one has Applications -> /Applications)"
mkdir -p "$TMP/ta/x" "$TMP/tb/x"
for t in ta tb; do ln -s / "$TMP/$t/Applications"; ln -s . "$TMP/$t/x/loop"; echo same > "$TMP/$t/x/f"; done
echo one > "$TMP/ta/x/changed"; echo two > "$TMP/tb/x/changed"
started="$(date +%s)"; out12="$(run_bounded 60 tree_differences "$TMP/ta" "$TMP/tb" 2>/dev/null)" || fail "the comparison failed or ran into a symlink"
[ "$out12" = "./x/changed" ] || fail "wrong differences: [$out12]"
[ $(( $(date +%s) - started )) -lt 30 ] || fail "the comparison followed a symlink"
rm "$TMP/tb/Applications"; ln -s /tmp "$TMP/tb/Applications"
out12="$(run_bounded 60 tree_differences "$TMP/ta" "$TMP/tb" 2>/dev/null)" || fail "a changed link target made the comparison fail"
echo "$out12" | grep -qx "./Applications" || fail "a changed link target was not reported"
mkdir "$TMP/tb/extra"; rc=0; run_bounded 60 tree_differences "$TMP/ta" "$TMP/tb" >/dev/null 2>&1 || rc=$?
[ "$rc" = 2 ] || fail "a different set of paths was not refused (status $rc)"
echo "ok: symlinks are not followed"

echo "sign-existing-dmg regression PASS"
