#!/bin/bash
set -uo pipefail
# Offline packaging regression for the bundled Docker Buildx (Milestone B2). No VM, no engine, no
# builds, no Docker config. Needs network only to fetch the two pinned upstream binaries into
# harpoon/build/buildx-cache the first time; tests that need them are SKIPPED (and counted)
# when they are neither cached nor downloadable.
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FETCH="$REPO_ROOT/harpoon/fetch-buildx.sh"
CHECK="$REPO_ROOT/tools/check-bundled-buildx.sh"
REAL_CACHE="$REPO_ROOT/harpoon/build/buildx-cache"
V="$(sed -n 's/^BUILDX_VERSION="\(.*\)"$/\1/p' "$FETCH")"
ARM="buildx-v$V.darwin-arm64"; AMD="buildx-v$V.darwin-amd64"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
PASS=0; FAILN=0; SKIP=0
ok() { echo "ok   - $1"; PASS=$((PASS+1)); }
bad() { echo "FAIL - $1"; FAILN=$((FAILN+1)); }
skip() { echo "SKIP - $1"; SKIP=$((SKIP+1)); }
# expect NAME CMD... : must exit non-zero and stderr must contain $EXPECT_MSG (if set)
expect_fail() { local name="$1"; shift; local out; if out="$("$@" 2>&1)"; then bad "$name (exited 0)"; else if [ -n "${EXPECT_MSG:-}" ] && ! printf '%s' "$out" | grep -qF "$EXPECT_MSG"; then bad "$name (message lacks '$EXPECT_MSG'): $out"; else ok "$name"; fi; fi; EXPECT_MSG=; }
expect_pass() { local name="$1"; shift; local out; if out="$("$@" 2>&1)"; then ok "$name"; else bad "$name: $out"; fi; }

# ---- real upstream files (cached or downloaded once through the script itself)
HAVE_REAL=0
if [ -f "$REAL_CACHE/$ARM" ] && [ -f "$REAL_CACHE/$AMD" ]; then HAVE_REAL=1
elif sh "$FETCH" >/dev/null 2>&1 && HARPOON_ARCH=universal HARPOON_BUILDX_OUT="$T/prefetch" sh "$FETCH" >/dev/null 2>&1; then HAVE_REAL=1; fi
SUMS="$REAL_CACHE/checksums-signed-$V.txt"
[ -f "$SUMS" ] || HAVE_REAL=0

run_fetch() { # CACHE BASE_URL OUT [ENV...]
  local cache="$1" base="$2" out="$3"; shift 3
  env "$@" HARPOON_BUILDX_CACHE="$cache" HARPOON_BUILDX_BASE_URL="$base" HARPOON_BUILDX_OUT="$out" sh "$FETCH"
}

# ---- 1. wrong checksum fails closed (no real files needed)
mkdir -p "$T/bad" "$T/c1"
if [ "$HAVE_REAL" = 1 ]; then cp "$SUMS" "$T/bad/checksums-signed.txt"; else
  printf '%s *%s\n%s *%s\n' 6f20ba2437865311ef99015bf5d1039c3c8e4d2177c1c64aa41b134b5ecd3012 "$ARM" 76e7a809cc98d34a4c1b7db23feac673def5b8dcdddd9e713ef26598689aa796 "$AMD" > "$T/bad/checksums-signed.txt"; fi
echo "not a buildx binary" > "$T/bad/$ARM"; echo "x" > "$T/bad/$AMD"
echo stale > "$T/out1"
EXPECT_MSG="sha256 mismatch" expect_fail "wrong checksum fails closed" run_fetch "$T/c1" "file://$T/bad" "$T/out1"
[ ! -e "$T/out1" ] && ok "a failed fetch leaves no output (stale file removed)" || bad "stale output survived a failed fetch"
[ ! -e "$T/c1/$ARM" ] && ok "the mismatching download is not kept in the cache" || bad "mismatching download kept"

# ---- 2. tampered cache entry with an unreachable source
if [ "$HAVE_REAL" = 1 ]; then
  mkdir -p "$T/c2"; cp "$REAL_CACHE/$ARM" "$T/c2/$ARM"; cp "$SUMS" "$T/c2/"; printf 'x' >> "$T/c2/$ARM"
  EXPECT_MSG="could not download" expect_fail "tampered cached binary is rejected, not trusted" run_fetch "$T/c2" "file://$T/nowhere" "$T/out2"
  [ ! -e "$T/out2" ] && ok "no output after tampered cache" || bad "output after tampered cache"
else skip "tampered cache (upstream binaries unavailable)"; fi

# ---- 3. checksums-signed.txt must agree with the pin and list the file
if [ "$HAVE_REAL" = 1 ]; then
  mkdir -p "$T/c3" "$T/src3"; cp "$REAL_CACHE/$ARM" "$T/c3/"
  sed "s/^6f20ba24/00000000/" "$SUMS" > "$T/c3/checksums-signed-$V.txt"
  EXPECT_MSG="disagrees" expect_fail "checksums-signed.txt disagreeing with the pin fails" run_fetch "$T/c3" "file://$T/nowhere" "$T/out3" HARPOON_BUILDX_OFFLINE=1
  grep -v "darwin-arm64" "$SUMS" > "$T/c3/checksums-signed-$V.txt"
  EXPECT_MSG="not listed" expect_fail "binary missing from checksums-signed.txt fails" run_fetch "$T/c3" "file://$T/nowhere" "$T/out3" HARPOON_BUILDX_OFFLINE=1
  mkdir -p "$T/c3b"; cp "$REAL_CACHE/$ARM" "$T/c3b/"
  EXPECT_MSG="no cached checksums-signed" expect_fail "offline mode without cached checksums fails" run_fetch "$T/c3b" "file://$T/nowhere" "$T/out3b" HARPOON_BUILDX_OFFLINE=1
else skip "checksums-signed.txt cross-check (upstream binaries unavailable)"; fi

# ---- 4. missing architecture fails closed (universal with only arm64 reachable)
if [ "$HAVE_REAL" = 1 ]; then
  mkdir -p "$T/src4" "$T/c4"; cp "$REAL_CACHE/$ARM" "$SUMS" "$T/src4/"; cp "$SUMS" "$T/src4/checksums-signed.txt"
  EXPECT_MSG="missing architecture slice" expect_fail "universal without the x86_64 slice fails closed" run_fetch "$T/c4" "file://$T/src4" "$T/out4" HARPOON_ARCH=universal
  [ ! -e "$T/out4" ] && ok "no partial universal output" || bad "partial output survived"
else skip "missing architecture (upstream binaries unavailable)"; fi
EXPECT_MSG="unsupported HARPOON_ARCH" expect_fail "unknown HARPOON_ARCH is rejected" run_fetch "$T/c1" "file://$T/bad" "$T/out5" HARPOON_ARCH=x86_64

# ---- 5. correct architecture is bundled
if [ "$HAVE_REAL" = 1 ]; then
  run_fetch "$REAL_CACHE" "file://$T/nowhere" "$T/arm" HARPOON_BUILDX_OFFLINE=1 >/dev/null 2>&1
  [ "$(lipo -archs "$T/arm" 2>/dev/null)" = "arm64" ] && ok "arm64 build bundles exactly arm64" || bad "arm64 output wrong: $(lipo -archs "$T/arm" 2>&1)"
  [ -x "$T/arm" ] && ok "output is executable" || bad "output not executable"
  cdhash() { codesign -dvvv "$1" 2>&1 | sed -n 's/^CDHash=//p'; }
  [ -n "$(cdhash "$T/arm")" ] && [ "$(cdhash "$T/arm")" = "$(cdhash "$REAL_CACHE/$ARM")" ] && ok "arm64 output carries the verified upstream code (same CDHash)" || bad "arm64 output's CDHash differs from upstream"
  run_fetch "$REAL_CACHE" "file://$T/nowhere" "$T/uni" HARPOON_BUILDX_OFFLINE=1 HARPOON_ARCH=universal >/dev/null 2>&1
  [ "$(lipo -archs "$T/uni" 2>/dev/null | tr ' ' '\n' | sort | tr '\n' ' ')" = "arm64 x86_64 " ] && ok "universal build bundles arm64 + x86_64" || bad "universal output wrong"
  codesign --verify --strict "$T/uni" >/dev/null 2>&1 && ok "lipo'd universal keeps a valid upstream signature" || bad "universal signature invalid"
else skip "correct architecture bundled (upstream binaries unavailable)"; fi

# ---- 6. upstream signature check (independent of the checksum)
mkdir -p "$T/sig"
cp /bin/ls "$T/sig/adhoc"; codesign --force --sign - "$T/sig/adhoc" >/dev/null 2>&1
lipo -archs "$T/sig/adhoc" | grep -q arm64 && {
  EXPECT_MSG="is not signed by" expect_fail "an ad-hoc/foreign signature is not accepted as upstream" sh "$FETCH" verify-signature "$T/sig/adhoc" arm64
}
if [ "$HAVE_REAL" = 1 ]; then
  expect_pass "the real upstream arm64 signature verifies" sh "$FETCH" verify-signature "$REAL_CACHE/$ARM" arm64
  EXPECT_MSG="is not exactly" expect_fail "architecture mismatch is rejected" sh "$FETCH" verify-signature "$REAL_CACHE/$ARM" x86_64
  cp "$REAL_CACHE/$ARM" "$T/sig/unsigned"; codesign --remove-signature "$T/sig/unsigned" >/dev/null 2>&1
  EXPECT_MSG="not valid" expect_fail "an unsigned binary is rejected" sh "$FETCH" verify-signature "$T/sig/unsigned" arm64
  cp "$REAL_CACHE/$ARM" "$T/sig/tampered"; printf 'x' >> "$T/sig/tampered"
  EXPECT_MSG="not valid" expect_fail "a signed binary modified after signing is rejected" sh "$FETCH" verify-signature "$T/sig/tampered" arm64
else skip "real-signature tests (upstream binaries unavailable)"; fi

# ---- 7. the bundle checker (pre-release and release), against fake Harpoon.app trees
mkapp() { # NAME -> prints dir; has license, no binary
  local a="$T/$1/Harpoon.app"; mkdir -p "$a/Contents/Resources/harpoon/bin" "$a/Contents/Resources/harpoon/share/licenses/docker-buildx"
  cp "$REPO_ROOT/harpoon/third-party/docker-buildx/LICENSE" "$a/Contents/Resources/harpoon/share/licenses/docker-buildx/"; echo "$a"; }
put_real() { cp "$REAL_CACHE/$ARM" "$1"; chmod 755 "$1"; }
BX() { echo "$1/Contents/Resources/harpoon/bin/docker-buildx"; }

A="$(mkapp missing)"
EXPECT_MSG="bundled Docker Buildx is missing" expect_fail "missing Buildx gives an actionable error" bash "$CHECK" "$A"
EXPECT_MSG="fetch-buildx.sh" expect_fail "…that says how to fix it" bash "$CHECK" "$A"
EXPECT_MSG="does not fall back to Docker Desktop" expect_fail "…and says there is no fallback" bash "$CHECK" "$A"

if [ "$HAVE_REAL" = 1 ]; then
  A="$(mkapp noexec)"; cp "$REAL_CACHE/$ARM" "$(BX "$A")"; chmod 644 "$(BX "$A")"
  EXPECT_MSG="not executable" expect_fail "non-executable Buildx fails" bash "$CHECK" "$A"

  A="$(mkapp symlink)"; ln -s "$REAL_CACHE/$ARM" "$(BX "$A")"
  EXPECT_MSG="regular file" expect_fail "a symlinked Buildx fails" bash "$CHECK" "$A"

  A="$(mkapp upstream)"; put_real "$(BX "$A")"; chmod 755 "$(BX "$A")"
  expect_pass "unmodified upstream binary passes pre-release checks (Developer ID, hardened runtime, timestamp)" bash "$CHECK" "$A"
  EXPECT_MSG="not Harpoon.app's" expect_fail "release mode rejects the upstream (Docker Inc.) signature as not Harpoon's" env HARPOON_EXPECT_TEAM_ID=ABCDE12345 bash "$CHECK" "$A" --release
  expect_pass "release mode passes when the team matches (Developer-ID workflow, simulated team)" env HARPOON_EXPECT_TEAM_ID=9BNSXJN65R bash "$CHECK" "$A" --release
  EXPECT_MSG="TeamIdentifier" expect_fail "release mode cannot pass without knowing Harpoon's team" bash "$CHECK" "$A" --release
  EXPECT_MSG="architectures are 'arm64', expected 'arm64 x86_64'" expect_fail "universal build rejects an arm64-only binary" env HARPOON_ARCH=universal bash "$CHECK" "$A"

  A="$(mkapp adhoc)"; put_real "$(BX "$A")"; codesign --force --sign - "$(BX "$A")" >/dev/null 2>&1
  expect_pass "ad-hoc re-signed binary passes pre-release checks" bash "$CHECK" "$A"
  EXPECT_MSG="ad-hoc" expect_fail "release mode rejects an ad-hoc binary" env HARPOON_EXPECT_TEAM_ID=9BNSXJN65R bash "$CHECK" "$A" --release

  A="$(mkapp tampered)"; put_real "$(BX "$A")"; printf 'x' >> "$(BX "$A")"
  EXPECT_MSG="signature missing or invalid" expect_fail "binary modified after signing fails" bash "$CHECK" "$A"

  A="$(mkapp unsigned)"; put_real "$(BX "$A")"; codesign --remove-signature "$(BX "$A")" >/dev/null 2>&1
  EXPECT_MSG="signature missing or invalid" expect_fail "unsigned binary fails" bash "$CHECK" "$A"

  A="$(mkapp nolicense)"; put_real "$(BX "$A")"; rm -r "$A/Contents/Resources/harpoon/share"
  EXPECT_MSG="license missing" expect_fail "missing Apache-2.0 license fails" bash "$CHECK" "$A"

  printf '#include <stdio.h>\nint main(void){puts("not buildx");return 0;}\n' > "$T/fake.c"
  A="$(mkapp wrongprog)"; cc -arch arm64 -o "$(BX "$A")" "$T/fake.c" 2>/dev/null && codesign --force --sign - "$(BX "$A")" >/dev/null 2>&1
  EXPECT_MSG="did not report" expect_fail "an arm64 program that is not Buildx is not accepted" bash "$CHECK" "$A"

  A="$(mkapp x86only)"; lipo -thin x86_64 "$REAL_CACHE/$AMD" -output "$(BX "$A")" 2>/dev/null || cp "$REAL_CACHE/$AMD" "$(BX "$A")"; chmod 755 "$(BX "$A")"
  EXPECT_MSG="architectures are 'x86_64', expected 'arm64'" expect_fail "an x86_64-only binary is rejected for an arm64 bundle" bash "$CHECK" "$A"
else skip "bundle checker with real binary (upstream binaries unavailable)"; fi

# ---- 8. signing scripts refuse a bundle without Buildx; Developer ID path needs an identity
A="$(mkapp signmissing)"; mkdir -p "$A/Contents/MacOS"; cp /bin/ls "$A/Contents/Resources/harpoon/bin/harpoon"
EXPECT_MSG="docker-buildx not found" expect_fail "ad-hoc sign-app refuses a bundle without Buildx" sh "$REPO_ROOT/ui/harpoon-desktop/scripts/sign-app.sh" "$A"
EXPECT_MSG="HARPOON_SIGN_IDENTITY not set" expect_fail "Developer ID sign-app still requires an identity" env -u HARPOON_SIGN_IDENTITY bash "$REPO_ROOT/tools/release/macos/sign-app.sh" "$A"

# ---- 9. no Buildx binary is tracked in git
# Judge the FILENAME (last path component) only: a directory called docker-buildx/ legitimately
# holds the tracked license and notice. Rejects a file named docker-buildx or buildx-v*.darwin-*
# (with or without a suffix such as .part), wherever it sits.
tracked_buildx_binaries() { awk -F/ '$NF == "docker-buildx" || $NF ~ /^buildx-v[0-9][0-9.]*\.darwin-/ || $NF ~ /^docker-buildx\.(part|bak|old)$/ {print}'; }
# predicate self-tests
printf '%s\n' harpoon/third-party/docker-buildx/LICENSE harpoon/third-party/docker-buildx/NOTICE.md harpoon/fetch-buildx.sh tools/check-bundled-buildx.sh | tracked_buildx_binaries | grep -q . \
  && bad "predicate wrongly flags the license/notice/scripts" || ok "predicate accepts the license, notice and scripts"
for f in bin/docker-buildx harpoon/build/docker-buildx harpoon/third-party/docker-buildx/docker-buildx buildx-v0.38.1.darwin-arm64 x/buildx-v0.38.1.darwin-amd64 a/b/buildx-v9.9.9.darwin-arm64.part Resources/harpoon/bin/docker-buildx; do
  [ -n "$(printf '%s\n' "$f" | tracked_buildx_binaries)" ] && ok "predicate rejects tracked $f" || bad "predicate misses tracked $f"
done
# the real tree
TRACKED="$(git -C "$REPO_ROOT" ls-files)"
printf '%s\n' "$TRACKED" | grep -qx 'harpoon/third-party/docker-buildx/LICENSE' && ok "the license is tracked" || bad "the license is not tracked"
printf '%s\n' "$TRACKED" | grep -qx 'harpoon/third-party/docker-buildx/NOTICE.md' && ok "the notice is tracked" || bad "the notice is not tracked"
FOUND="$(printf '%s\n' "$TRACKED" | tracked_buildx_binaries)"
[ -z "$FOUND" ] && ok "no Buildx binary is tracked in git" || bad "a Buildx binary is tracked in git: $FOUND"
git -C "$REPO_ROOT" check-ignore harpoon/build/docker-buildx harpoon/build/buildx-cache/"$ARM" >/dev/null && ok "harpoon/build/ is git-ignored (binary and cache)" || bad "harpoon/build/ is not ignored"
[ -z "$(printf '%s\n' "$TRACKED" | grep '^harpoon/build/')" ] && ok "nothing under harpoon/build/ is tracked" || bad "harpoon/build/ has tracked files"

echo "buildx packaging regression: $PASS passed, $FAILN failed, $SKIP skipped"
[ "$FAILN" -eq 0 ]
