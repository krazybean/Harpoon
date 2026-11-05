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
cat > "$APP/Info.plist" <<'P'
<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleExecutable</key><string>harpoon-desktop</string><key>CFBundleIdentifier</key><string>com.example.stand-in</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>
P
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
app_verifier() { codesign --verify --ignore-resources "$1/Contents/MacOS/harpoon-desktop" >/dev/null 2>&1; } # the stand-in app has a fake seal in the default-stage scenarios
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

# =====================================================================================================================
# --resume-from-signed: stand-ins signed ad-hoc with the hardened runtime, the runtime-only entitlement, and the checks'
# expectations switched to match (plain variables, as the script documents). Apple and Gatekeeper are stand-ins.
# =====================================================================================================================
REQUIRED_AUTHORITY="Signature=adhoc"; RELEASE_GRADE=0; REQUIRED_ARCHS="$(lipo -archs /usr/bin/true)"
cat > "$TMP/virt.plist" <<'P'
<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>com.apple.security.virtualization</key><true/></dict></plist>
P
release_signer() { # what the real signer produces, ad hoc: hardened runtime everywhere, the virtualization entitlement on the runtime only
  local a="$1" f
  codesign --force --sign - --options runtime --entitlements "$TMP/virt.plist" "$a/Contents/Resources/harpoon/bin/harpoon" >/dev/null 2>&1
  for f in MacOS/harpoon-desktop Resources/harpoon/bin/docker Resources/harpoon/bin/docker-buildx; do codesign --force --sign - --options runtime "$a/Contents/$f" >/dev/null 2>&1; done
  codesign --force --sign - --options runtime "$a" >/dev/null 2>&1   # the bundle last, which writes a real CodeResources
}
scenario "13. set-up: a signed intermediate, made by the default stage with the stand-in release signer"
mkdir -p "$TMP/rs-in" "$TMP/rw"
app_signer() { release_signer "$1"; }
run "$TMP/rs-in" "$IN_SHA" sign || { cat "$TMP/err" >&2; fail "could not make the signed intermediate"; }
SIGNED="$TMP/rs-in/Harpoon-test-signed.dmg"; SIGNED_SHA="$(sha "$SIGNED")"
[ -f "$SIGNED" ] || fail "no signed intermediate"
echo "ok: signed intermediate"

# from here on the application signer must never be called, and nothing may be mounted writable
app_signer() { : > "$TMP/app-signer-called"; return 1; }
real_hdiutil="$(command -v hdiutil)"
hdiutil() { echo "$*" >> "$TMP/hdiutil.calls"; "$real_hdiutil" "$@"; }
dmg_signer() { codesign --force --sign - "$1" >/dev/null 2>&1; }
gatekeeper_assess() { return 0; }
notary_submit() { local n; n="$(cat "$TMP/submits" 2>/dev/null || echo 0)"; echo $(( n + 1 )) > "$TMP/submits"; echo "SUB-$(( n + 1 ))"; }
notary_status() { cat "$TMP/notary-status"; }
notary_log() { echo '{"issues":["stand-in rejection"]}'; }
notary_staple() { : > "$1.stapled"; echo x >> "$TMP/staples"; }
staple_validate() { [ -e "$1.stapled" ]; }
submits() { cat "$TMP/submits" 2>/dev/null || echo 0; }
resume() { # resume OUT_DIR EXPECT STAGE [INPUT]
  mkdir -p "$1"; ( trap cleanup EXIT; resume_run "${4:-$SIGNED}" "$1" "$TMP/rw" "$2" "$3" ) 2>"$TMP/err" && return 0 || return 1
}
assert_resume_clean() { # nothing mounted, no temp directory, the signed input unchanged, the app signer never called, nothing writable
  [ "$(mounts)" = 0 ] || fail "$1: a volume is still mounted"
  [ -z "$(ls -d "$TMP/rw"/resume-dmg.* 2>/dev/null)" ] || fail "$1: a temp directory is left"
  [ "$(sha "$SIGNED")" = "$SIGNED_SHA" ] || fail "$1: the signed input changed"
  [ ! -e "$TMP/app-signer-called" ] || fail "$1: the application signer was called"
  ! grep -qE -- "-shadow|convert" "$TMP/hdiutil.calls" 2>/dev/null || fail "$1: a writable shadow or a conversion was used"
}
DIST="$TMP/rs-dist"

scenario "14. resume, verify only: the signed input is checked completely and nothing is written"
resume "$DIST" "$SIGNED_SHA" verify || { cat "$TMP/err" >&2; fail "verification of a good signed input failed"; }
grep -q "verification only" "$TMP/err" || fail "no verification message"; [ -z "$(ls -A "$DIST")" ] || fail "verification wrote files"
assert_resume_clean "verify only"; echo "ok: verify only"

scenario "15. resume, sign the image: a separate distribution image, the signed input byte-identical, no application signing"
resume "$DIST" "$SIGNED_SHA" dmg || { cat "$TMP/err" >&2; fail "creating the distribution image failed"; }
OUT="$DIST/Harpoon-test.dmg"; [ -f "$OUT" ] && [ "$OUT" != "$SIGNED" ] || fail "no separate distribution image"
[ "$(dmg_signature_state "$OUT")" = signed ] || fail "the distribution image is not signed"
[ "$(dmg_signature_state "$SIGNED")" = unsigned ] || fail "the signed input's image was signed"
[ "$(state_get "$OUT.state" stage)" = dmg-signed ] && [ "$(state_get "$OUT.state" input_sha256)" = "$SIGNED_SHA" ] || fail "the state does not record the stage and input"
assert_resume_clean "sign image"; echo "ok: distribution image"
OUT_SHA="$(sha "$OUT")"; resume "$DIST" "$SIGNED_SHA" dmg || { cat "$TMP/err" >&2; fail "an identical rerun failed"; }
[ "$(sha "$OUT")" = "$OUT_SHA" ] || fail "an identical rerun changed the output"; assert_resume_clean "idempotent rerun"; echo "ok: rerun is idempotent"

scenario "16. a wrong or missing checksum is refused before anything is written"
resume "$TMP/rs-w" "0000000000000000000000000000000000000000000000000000000000000000" dmg && fail "accepted a wrong checksum"
grep -q "does not equal the expected" "$TMP/err" || fail "wrong message for a wrong checksum"; [ -z "$(ls -A "$TMP/rs-w")" ] || fail "files were written"
resume "$TMP/rs-w" "" dmg && fail "accepted a missing checksum"; grep -q "needs --expect-sha256" "$TMP/err" || fail "wrong message for a missing checksum"
assert_resume_clean "checksum"; echo "ok: checksum required and checked"

scenario "17. an unsigned app and an invalid signature are refused"
resume "$TMP/rs-u" "$IN_SHA" dmg "$IN" && fail "resumed from an image whose app is unsigned"
grep -q "unsigned, or its signature is invalid" "$TMP/err" || { cat "$TMP/err" >&2; fail "wrong message for an unsigned app"; }; [ -z "$(ls -A "$TMP/rs-u")" ] || fail "files written for an unsigned app"
mkdir -p "$TMP/bad-m" "$TMP/bad-src"; "$real_hdiutil" attach -readonly -nobrowse -noverify -mountpoint "$TMP/bad-m" "$SIGNED" >/dev/null 2>&1
cp -R "$TMP/bad-m/Harpoon.app" "$TMP/bad-src/" ; ln -s /Applications "$TMP/bad-src/Applications"; "$real_hdiutil" detach "$TMP/bad-m" >/dev/null 2>&1
chmod -R u+w "$TMP/bad-src"; printf 'X' | dd of="$TMP/bad-src/Harpoon.app/Contents/Resources/harpoon/bin/docker" bs=1 seek=200 conv=notrunc 2>/dev/null
"$real_hdiutil" create -size 96m -fs HFS+ -volname Harpoon -srcfolder "$TMP/bad-src" -ov -format UDZO "$TMP/bad-signature.dmg" >/dev/null 2>&1
resume "$TMP/rs-i" "$(sha "$TMP/bad-signature.dmg")" dmg "$TMP/bad-signature.dmg" && fail "resumed from an image with an invalid signature"
grep -q "unsigned, or its signature is invalid" "$TMP/err" || { cat "$TMP/err" >&2; fail "wrong message for an invalid signature"; }; [ -z "$(ls -A "$TMP/rs-i")" ] || fail "files written for an invalid signature"
assert_resume_clean "bad signatures"; echo "ok: unsigned and invalid signatures refused"

scenario "18. an image that is already signed is not a resume point"
cp -c "$SIGNED" "$TMP/already.dmg" 2>/dev/null || cp "$SIGNED" "$TMP/already.dmg"; codesign --force --sign - "$TMP/already.dmg" >/dev/null 2>&1
resume "$TMP/rs-a" "$(sha "$TMP/already.dmg")" dmg "$TMP/already.dmg" && fail "resumed from an already signed image"
grep -q "already signed or notarized" "$TMP/err" || { cat "$TMP/err" >&2; fail "wrong message for an already signed image"; }; [ -z "$(ls -A "$TMP/rs-a")" ] || fail "files written"
assert_resume_clean "already signed"; echo "ok: already signed refused"

scenario "19. an existing output is never overwritten unless its state shows it came from this input"
mkdir -p "$TMP/rs-e"; echo "someone else's file" > "$TMP/rs-e/Harpoon-test.dmg"; before="$(sha "$TMP/rs-e/Harpoon-test.dmg")"
resume "$TMP/rs-e" "$SIGNED_SHA" dmg && fail "overwrote a foreign output"; grep -q "must both exist or both be absent" "$TMP/err" || fail "wrong message for a foreign output"
echo "input_sha256=$IN_SHA" > "$TMP/rs-e/Harpoon-test.dmg.state"
resume "$TMP/rs-e" "$SIGNED_SHA" dmg && fail "overwrote an output made from another input"; grep -q "never overwritten" "$TMP/err" || fail "wrong message for another input's output"
[ "$(sha "$TMP/rs-e/Harpoon-test.dmg")" = "$before" ] || fail "the foreign output changed"; assert_resume_clean "existing output"; echo "ok: existing output protected"

scenario "20. symlinks and paths: a symlinked input, output directory, output or state file is refused"
ln -s "$SIGNED" "$TMP/link-in.dmg"; resume "$TMP/rs-s1" "$SIGNED_SHA" dmg "$TMP/link-in.dmg" && fail "accepted a symlinked input"
grep -q "regular file, not a symlink" "$TMP/err" || fail "wrong message for a symlinked input"
mkdir -p "$TMP/real-out"; ln -s "$TMP/real-out" "$TMP/link-out"; resume "$TMP/link-out" "$SIGNED_SHA" dmg && fail "accepted a symlinked output directory"
grep -q "real directory, not a symlink" "$TMP/err" || fail "wrong message for a symlinked output directory"; [ -z "$(ls -A "$TMP/real-out")" ] || fail "wrote through a symlink"
mkdir -p "$TMP/rs-s2"; ln -s "$TMP/elsewhere" "$TMP/rs-s2/Harpoon-test.dmg.state"; resume "$TMP/rs-s2" "$SIGNED_SHA" dmg && fail "accepted a symlinked state file"
grep -q "is a symlink: refusing" "$TMP/err" || fail "wrong message for a symlinked state file"; [ ! -e "$TMP/elsewhere" ] || fail "wrote through a symlinked state file"
assert_resume_clean "symlinks"; echo "ok: symlink and path safeguards"

scenario "21. a failing signer leaves no output, no state, no mount, and the input intact"
dmg_signer() { return 1; }; resume "$TMP/rs-f" "$SIGNED_SHA" dmg && fail "a failing signer succeeded"
dmg_signer() { codesign --force --sign - "$1" >/dev/null 2>&1; }
[ -z "$(ls -A "$TMP/rs-f")" ] || fail "files left after a failed signing: $(ls -A "$TMP/rs-f")"; assert_resume_clean "failing signer"; echo "ok: failure cleaned up"

scenario "22. interrupted notarization: the recorded submission is polled, never resubmitted; then it completes"
NOTARY_WAIT_SECONDS=2; NOTARY_POLL_SECONDS=1; rm -f "$TMP/submits" "$TMP/staples"; echo "In Progress" > "$TMP/notary-status"
resume "$TMP/rs-n" "$SIGNED_SHA" apple && fail "completed while Apple was still working"
grep -q "still 'In Progress'" "$TMP/err" || { cat "$TMP/err" >&2; fail "wrong message while in progress"; }
N="$TMP/rs-n/Harpoon-test.dmg"; [ "$(state_get "$N.state" submission_id)" = "SUB-1" ] && [ "$(state_get "$N.state" stage)" = submitted ] || fail "the submission was not recorded"
[ ! -e "$TMP/rs-n/SHA256SUMS" ] && [ ! -e "$TMP/rs-n/SIGNING-PROVENANCE.txt" ] || fail "checksums were written before notarization finished"
echo "Accepted" > "$TMP/notary-status"; resume "$TMP/rs-n" "$SIGNED_SHA" apple || { cat "$TMP/err" >&2; fail "the resumed notarization failed"; }
[ "$(submits)" = 1 ] || fail "the image was submitted again ($(submits) submissions)"; [ "$(wc -l < "$TMP/staples" | tr -d ' ')" = 1 ] || fail "stapled more or less than once"
FINAL="$(sha "$N")"; grep -qx "$FINAL  Harpoon-test.dmg" "$TMP/rs-n/SHA256SUMS" || fail "SHA256SUMS does not describe the final image"
grep -q "signed input      Harpoon-test-signed.dmg sha256 $SIGNED_SHA" "$TMP/rs-n/SIGNING-PROVENANCE.txt" && grep -q "final artifact    Harpoon-test.dmg sha256 $FINAL" "$TMP/rs-n/SIGNING-PROVENANCE.txt" || fail "provenance lacks the input and final hashes"
[ "$FINAL" != "$SIGNED_SHA" ] || fail "the final hash equals the input hash"; [ "$(state_get "$N.state" stage)" = finalized ] || fail "not finalized"
sums_before="$(sha "$TMP/rs-n/SHA256SUMS")"; prov_before="$(sha "$TMP/rs-n/SIGNING-PROVENANCE.txt")"
resume "$TMP/rs-n" "$SIGNED_SHA" apple || { cat "$TMP/err" >&2; fail "rerunning a finished notarization failed"; }
[ "$(sha "$N")" = "$FINAL" ] && [ "$(sha "$TMP/rs-n/SHA256SUMS")" = "$sums_before" ] && [ "$(sha "$TMP/rs-n/SIGNING-PROVENANCE.txt")" = "$prov_before" ] && [ "$(submits)" = 1 ] || fail "a rerun of a finished notarization changed something"
assert_resume_clean "notarization"; echo "ok: interrupted notarization recovered"

scenario "23. a rejected notarization is reported, kept, and never retried automatically"
rm -f "$TMP/submits"; echo "Invalid" > "$TMP/notary-status"
resume "$TMP/rs-r" "$SIGNED_SHA" apple && fail "completed despite a rejection"
grep -q "Apple rejected submission SUB-1" "$TMP/err" || { cat "$TMP/err" >&2; fail "wrong message for a rejection"; }
[ -s "$TMP/rs-r/notary-SUB-1.log" ] || fail "the rejection log was not saved"; [ ! -e "$TMP/rs-r/SHA256SUMS" ] || fail "checksums written for a rejected image"
[ "$(state_get "$TMP/rs-r/Harpoon-test.dmg.state" stage)" = rejected ] || fail "the rejection was not recorded"
resume "$TMP/rs-r" "$SIGNED_SHA" apple && fail "retried a rejected notarization"; [ "$(submits)" = 1 ] || fail "resubmitted after a rejection"
grep -q "Nothing is resubmitted automatically" "$TMP/err" || fail "wrong message on the rerun"; [ -f "$TMP/rs-r/Harpoon-test.dmg" ] || fail "the rejected image was removed"
assert_resume_clean "rejection"; echo "ok: rejection handled"

scenario "24. through the command line: resuming needs an explicit stage"
mkdir -p "$TMP/rs-c"
env -u HARPOON_SIGN_IDENTITY bash "$SCRIPT_DIR/sign-existing-dmg.sh" --resume-from-signed "$SIGNED" --expect-sha256 "$SIGNED_SHA" --out-dir "$TMP/rs-c" 2>"$TMP/err" && fail "resumed without a stage"
grep -q "needs one stage" "$TMP/err" || fail "wrong message without a stage"
env -u HARPOON_SIGN_IDENTITY bash "$SCRIPT_DIR/sign-existing-dmg.sh" --resume-from-signed "$SIGNED" --expect-sha256 "$SIGNED_SHA" --out-dir "$TMP/rs-c" --work-dir "$TMP/rw" --dry-run 2>"$TMP/err" && fail "the real checks accepted an ad-hoc stand-in" || true
grep -q "not signed by the required authority" "$TMP/err" || { cat "$TMP/err" >&2; fail "the real (Developer ID) check did not run by default"; }
echo "ok: command line"


echo "sign-existing-dmg regression PASS"
