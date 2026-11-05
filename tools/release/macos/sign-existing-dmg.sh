#!/bin/bash
set -euo pipefail
# Sign the app inside an already-built, already-verified universal DMG (for rc.3: the one CI produced) without
# rebuilding anything. The DMG is a read-only compressed image, so it is mounted *writable through a shadow file*
# (hdiutil -shadow: copy-on-write, only the blocks signing changes are stored), the existing Developer ID signer runs on
# the mounted app, and the image is converted back to UDZO with the same shadow. The input file is never modified.
#
#   tools/release/macos/sign-existing-dmg.sh INPUT.dmg [--out-dir DIR] [--work-dir DIR] [--expect-sha256 HEX]
#                                            [--dry-run] [--sign-dmg] [--submit-to-apple]
#   tools/release/macos/sign-existing-dmg.sh --resume-from-signed SIGNED.dmg --expect-sha256 HEX --out-dir DIR
#                                            (--dry-run | --sign-dmg | --submit-to-apple)   # see "--resume-from-signed" below
#
# Stages (each needs the one before it, and only the flag you pass goes further):
#   (default)           check the input, sign the app inside, convert, and prove nothing but the code signatures changed.
#                       Writes INPUT-signed.dmg. Signing uses the Developer ID key (HARPOON_SIGN_IDENTITY); nothing is sent to Apple.
#   --dry-run           check the input, tools, identity and disk headroom; print the plan; sign nothing.
#   --sign-dmg          also sign the DMG itself (codesign --timestamp), required before notarization.
#   --submit-to-apple   also submit the DMG for notarization (tools/release/macos/notarize.sh, which waits and staples),
#                       then validate, and write SHA256SUMS and SIGNING-PROVENANCE.txt. This is the only step that
#                       contacts Apple. Implies --sign-dmg.
#
# Safety: the free space of the work volume must stay above GUARD_GIB (16) after each major operation, with FLOOR_GIB (15)
# as the absolute floor; nothing here deletes anything but the temporary files this run created (a mount it made, its
# shadow file, a partial output); the output is a separately named file and is never overwritten.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
GUARD_GIB="${GUARD_GIB:-16}"
FLOOR_GIB=15
SHADOW_ESTIMATE_BYTES=$((600 * 1024 * 1024)) # measured binaries total about 262 MB; twice that, rounded up
SLACK_BYTES=$((150 * 1024 * 1024))
# What signing may change, relative to Harpoon.app. Anything else that differs after conversion is a failure.
CODE_FILES=(
  "Contents/MacOS/harpoon-desktop"
  "Contents/Resources/harpoon/bin/harpoon"
  "Contents/Resources/harpoon/bin/docker"
  "Contents/Resources/harpoon/bin/docker-buildx"
  "Contents/_CodeSignature/CodeResources"
)
# Metadata a writable mount may create, ignored when comparing and removed before the image is closed.
VOLUME_JUNK=(.fseventsd .Trashes .Spotlight-V100 .TemporaryItems)

say() { echo "[sign-dmg $(date -u +%H:%M:%S)] $*" >&2; }
die() { say "FAIL: $*"; exit 1; }

# Every command that can block on the disk-image machinery runs under a watchdog, so a hang is a failure with a message that
# names the command and how long it was given, instead of a job that silently runs out its timeout. Only the command's own
# process is terminated. Plain shell variables (not environment switches): a longer limit for a real 5 GB image, a short one in tests.
HDIUTIL_TIMEOUT="${HDIUTIL_TIMEOUT:-1800}"
run_bounded() { # run_bounded SECONDS COMMAND... : returns the command's status, or 124 after SECONDS
  local secs="$1" start rc=0 pid watchdog flag; shift
  start="$(date +%s)"; flag="$(mktemp "${TMPDIR:-/tmp}/bounded.XXXXXX")"; rm -f "$flag"
  say "RUN (limit ${secs}s): $*"
  "$@" & pid=$!
  # the watchdog ends its own sleep when told to stop, so nothing lingers and the flag is only written on a real timeout
  ( sp=""; trap 'kill "$sp" 2>/dev/null; exit 0' TERM; sleep "$secs" & sp=$!; wait "$sp"
    : > "$flag"; pkill -TERM -P "$pid" 2>/dev/null; kill -TERM "$pid" 2>/dev/null; sleep 3
    pkill -KILL -P "$pid" 2>/dev/null; kill -KILL "$pid" 2>/dev/null ) >/dev/null 2>&1 & watchdog=$!
  wait "$pid" || rc=$?
  kill "$watchdog" 2>/dev/null || true; wait "$watchdog" 2>/dev/null || true
  if [ -e "$flag" ]; then rm -f "$flag"; say "TIMEOUT after ${secs}s: $*"; return 124; fi
  say "done rc=$rc in $(( $(date +%s) - start ))s: $1 ${2:-}"
  return "$rc"
}

# ---- the pieces the regression test replaces (shell functions, not environment switches) --------------------------
free_kib() { df -k "$1" | awk 'NR==2{print $4}'; }
app_signer() { HARPOON_ARCH=universal bash "$SCRIPT_DIR/sign-app.sh" "$1"; }
app_verifier() {
  HARPOON_ARCH=universal bash "$SCRIPT_DIR/verify-signatures.sh" "$1" &&
    HARPOON_ARCH=universal bash "$REPO_ROOT/tools/verify-bundle.sh" "$1"
}
dmg_signer() { codesign --force --sign "${HARPOON_SIGN_IDENTITY:?HARPOON_SIGN_IDENTITY is not set}" --timestamp "$1"; }
notarizer() { bash "$SCRIPT_DIR/notarize.sh" "$1"; }

# ---- headroom ------------------------------------------------------------------------------------------------------
# After consuming NEED bytes on the volume holding DIR, free space must still be above the guard (and the floor).
require_headroom() {
  local dir="$1" need="$2" what="$3" free_b guard floor
  free_b=$(( $(free_kib "$dir") * 1024 ))
  guard=$(( GUARD_GIB * 1073741824 )); floor=$(( FLOOR_GIB * 1073741824 ))
  if [ $(( free_b - need )) -lt "$guard" ] || [ $(( free_b - need )) -lt "$floor" ]; then
    die "not enough free space for $what: $(( free_b / 1048576 )) MiB free, needs $(( need / 1048576 )) MiB, and free space must stay above ${GUARD_GIB} GiB"
  fi
  say "headroom OK for $what: $(( free_b / 1048576 )) MiB free, needs about $(( need / 1048576 )) MiB, guard ${GUARD_GIB} GiB"
}

# ---- cleanup: only what this run created ---------------------------------------------------------------------------
MOUNTS=()        # mount points this run attached (and nothing else)
TEMP_FILES=()    # temporary files this run created (the shadow): always removed
PARTIAL_FILES=() # an output being written: removed only if the run fails before it is complete
OWN_DIRS=()      # temporary directories this run created: removed (innermost first) when empty
cleanup() {
  local status=$? m f i
  for m in ${MOUNTS[@]+"${MOUNTS[@]}"}; do
    # `mount` prints the physical path (/private/tmp for /tmp), so compare against the resolved one
    local real; real="$(cd "$m" 2>/dev/null && pwd -P || echo "$m")"
    if mount | grep -qF " on $real "; then run_bounded 120 hdiutil detach "$m" >/dev/null 2>&1 || run_bounded 120 hdiutil detach -force "$m" >/dev/null 2>&1 || say "WARNING: could not detach $m"; fi
  done
  for f in ${TEMP_FILES[@]+"${TEMP_FILES[@]}"}; do rm -f -- "$f"; done
  for f in ${PARTIAL_FILES[@]+"${PARTIAL_FILES[@]}"}; do rm -f -- "$f"; done
  for (( i=${#OWN_DIRS[@]}-1; i>=0; i-- )); do rmdir -- "${OWN_DIRS[$i]}" 2>/dev/null || true; done
  return $status
}

sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
bytes() { stat -f%z "$1" 2>/dev/null || stat -c%s "$1"; }

# Compare two mounted trees WITHOUT following symlinks (the image has `Applications -> /Applications`; `diff -r` walks into
# the host's real /Applications, which is slow, wrong and, on a runner full of apps, endless). Prints the relative paths whose
# content, type or link target differ; returns 2 when the two trees do not list the same paths.
list_tree() { ( cd "$1" && find . -mindepth 1 \( -name .fseventsd -o -name .Trashes -o -name .Spotlight-V100 -o -name .TemporaryItems \) -prune -o -print | LC_ALL=C sort ); }
tree_differences() {
  local a="$1" b="$2" la lb rel
  la="$(list_tree "$a")"; lb="$(list_tree "$b")"
  if [ "$la" != "$lb" ]; then say "the two images do not list the same paths: $(diff <(echo "$la") <(echo "$lb") | head -5 | tr '\n' ' ')"; return 2; fi
  while IFS= read -r rel; do
    [ -n "$rel" ] || continue
    if [ -L "$a/$rel" ] || [ -L "$b/$rel" ]; then
      if [ -L "$a/$rel" ] && [ -L "$b/$rel" ] && [ "$(readlink "$a/$rel")" = "$(readlink "$b/$rel")" ]; then :; else echo "$rel"; fi
    elif [ -f "$a/$rel" ] && [ -f "$b/$rel" ]; then cmp -s "$a/$rel" "$b/$rel" || echo "$rel"
    elif [ -d "$a/$rel" ] && [ -d "$b/$rel" ]; then :
    else echo "$rel"; fi
  done <<<"$la"
}

attach() { # attach MODE(shadow|readonly) IMAGE MOUNTPOINT [SHADOW]
  local mode="$1" image="$2" mp="$3" shadow="${4:-}"
  mkdir -p "$mp" || die "cannot create $mp"; OWN_DIRS+=("$mp")
  if [ "$mode" = shadow ]; then
    [ ! -e "$shadow" ] || die "the shadow file $shadow already exists; a shadow is never reused (it is not checked against its image)"
    TEMP_FILES+=("$shadow")
    run_bounded "$HDIUTIL_TIMEOUT" hdiutil attach -shadow "$shadow" -nobrowse -noautoopen -noverify -mountpoint "$mp" "$image" >/dev/null || die "could not mount $image writable through a shadow"
  else
    run_bounded "$HDIUTIL_TIMEOUT" hdiutil attach -readonly -nobrowse -noautoopen -noverify -mountpoint "$mp" "$image" >/dev/null || die "could not mount $image read-only"
  fi
  MOUNTS+=("$mp")
}
detach() { # detach MOUNTPOINT : plain detach, never forced here (a busy volume is a failure to look at)
  run_bounded 300 hdiutil detach "$1" >/dev/null || die "could not detach $1 (something is using it)"
}

# ---- the work ------------------------------------------------------------------------------------------------------
sign_dmg_run() { # sign_dmg_run INPUT OUT_DIR WORK_DIR EXPECT_SHA256 STAGE(sign|dmg|apple|dry)
  local input="$1" out_dir="$2" work="$3" expect="$4" stage="$5"
  [ -f "$input" ] || die "input $input not found"
  local base out in_sha in_bytes
  base="$(basename "$input" .dmg)"; out="$out_dir/$base-signed.dmg"
  [ ! -e "$out" ] || die "$out already exists; the output is never overwritten"
  in_bytes="$(bytes "$input")"

  say "input: $input ($(( in_bytes / 1048576 )) MiB)"
  in_sha="$(sha256 "$input")"
  if [ -n "$expect" ]; then
    [ "$in_sha" = "$expect" ] || die "input sha256 $in_sha does not equal the expected $expect"
    say "input sha256 matches the expected value"
  else
    say "input sha256 $in_sha (no expected value given: it is recorded, not verified)"
  fi
  run_bounded "$HDIUTIL_TIMEOUT" hdiutil verify "$input" >/dev/null 2>&1 || die "the input image does not verify (hdiutil verify)"

  require_headroom "$work" $(( SHADOW_ESTIMATE_BYTES + SLACK_BYTES )) "the shadow file"
  require_headroom "$out_dir" $(( in_bytes + SLACK_BYTES )) "the signed image"
  if [ "$stage" = dry ]; then
    say "plan: mount writable via a shadow in $work; sign Harpoon.app inside; convert to $out; verify that only code signatures changed"
    say "dry run: nothing was mounted, signed or written"
    return 0
  fi

  local tmp mnt shadow
  tmp="$(mktemp -d "$work/sign-dmg.XXXXXX")" || die "cannot create a temporary directory in $work"; OWN_DIRS+=("$tmp")
  mnt="$tmp/mnt"; shadow="$tmp/work.shadow"

  say "mounting writable through $shadow"
  attach shadow "$input" "$mnt" "$shadow"
  [ -d "$mnt/Harpoon.app" ] || die "no Harpoon.app at the root of the image"
  run_bounded 60 mdutil -i off "$mnt" >/dev/null 2>&1 || true
  say "signing Harpoon.app inside the image"
  app_signer "$mnt/Harpoon.app" || die "signing the app inside the image failed"
  app_verifier "$mnt/Harpoon.app" || die "the freshly signed app does not verify"
  local j; for j in "${VOLUME_JUNK[@]}"; do rm -rf -- "${mnt:?}/$j"; done
  run_bounded 300 sync || true
  detach "$mnt"
  local sh_bytes; sh_bytes="$(bytes "$shadow")"
  say "shadow holds $(( sh_bytes / 1048576 )) MiB of changed blocks"

  [ "$(sha256 "$input")" = "$in_sha" ] || die "the input image changed while it was being signed"
  require_headroom "$out_dir" $(( in_bytes + SLACK_BYTES )) "converting the signed image"
  PARTIAL_FILES+=("$out")
  run_bounded "$HDIUTIL_TIMEOUT" hdiutil convert "$input" -format UDZO -shadow "$shadow" -o "$out" >/dev/null || die "hdiutil convert failed"
  run_bounded "$HDIUTIL_TIMEOUT" hdiutil verify "$out" >/dev/null 2>&1 || die "the signed image does not verify (hdiutil verify)"
  [ "$(sha256 "$input")" = "$in_sha" ] || die "the input image changed during conversion"

  say "comparing the signed image with the input: only code signatures may differ"
  local a="$tmp/in" b="$tmp/out" line rel allowed ok
  attach readonly "$input" "$a"; attach readonly "$out" "$b"
  local diffs rc=0
  diffs="$(run_bounded "$HDIUTIL_TIMEOUT" tree_differences "$a" "$b")" || rc=$?
  [ "$rc" = 0 ] || die "comparing the images failed, found different contents, or timed out (status $rc)"
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    case "$line" in
      ./Harpoon.app/*) rel="${line#./Harpoon.app/}" ;;
      *) die "signing changed a path outside Harpoon.app: $line" ;;
    esac
    ok=0; for allowed in "${CODE_FILES[@]}"; do [ "$rel" = "$allowed" ] && ok=1; done
    [ "$ok" = 1 ] || die "signing changed a file that is not code: $rel"
  done <<<"$diffs"
  say "only signature-bearing files differ ($(grep -c . <<<"$diffs") of them)"
  app_verifier "$b/Harpoon.app" >/dev/null 2>&1 || die "the signed image's app does not verify"
  detach "$a"; detach "$b"
  rm -f -- "$shadow"
  PARTIAL_FILES=() # complete and verified: from here on it is kept, whatever happens next
  say "signed (not notarized) image: $out"

  if [ "$stage" = sign ]; then return 0; fi

  say "signing the image itself"
  dmg_signer "$out" || die "signing the DMG failed"
  if [ "$stage" = dmg ]; then return 0; fi

  # --submit-to-apple: the only step that contacts Apple. A clone keeps the pre-staple file for provenance at no extra space.
  require_headroom "$out_dir" $(( SLACK_BYTES )) "stapling"
  local pre="$out_dir/$base-signed-prestaple.dmg"
  cp -c "$out" "$pre" 2>/dev/null || cp "$out" "$pre"
  say "submitting to Apple (notarize.sh waits for the result and staples)"
  notarizer "$out" || die "notarization failed; $pre is the signed, unstapled image"
  xcrun stapler validate "$out" >/dev/null 2>&1 || die "the stapled ticket does not validate"
  spctl --assess --type install --verbose=4 "$out" || die "Gatekeeper does not accept the DMG"
  local final; final="$(sha256 "$out")"
  ( cd "$out_dir" && shasum -a 256 "$(basename "$out")" > SHA256SUMS )
  {
    echo "input $(basename "$input") sha256 $in_sha ($in_bytes bytes)"
    echo "signed-unstapled $(basename "$pre") sha256 $(sha256 "$pre")"
    echo "final $(basename "$out") sha256 $final ($(bytes "$out") bytes)"
    echo "identity ${HARPOON_SIGN_IDENTITY:-unknown}"
    echo "date $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "macOS $(sw_vers -productVersion 2>/dev/null || echo unknown)"
  } > "$out_dir/SIGNING-PROVENANCE.txt"
  say "final: $out sha256 $final"
}

# =====================================================================================================================
# --resume-from-signed: continue from an image whose app was ALREADY signed by an earlier run (the default stage above).
# Nothing is mounted writable and nothing in the app is signed again. The signed image is only ever read; the distribution
# image is a separate file (a clone of it, so it costs no extra space until it is stapled). Stages, each explicit:
#   --dry-run          verify the signed image completely and stop (writes nothing)
#   --sign-dmg         also create the distribution image and sign it (codesign --timestamp)
#   --submit-to-apple  also notarize and staple it, verify the result, and write SHA256SUMS and SIGNING-PROVENANCE.txt
# Progress is recorded beside the output in <output>.state, so an interrupted or rejected notarization is recovered, never
# repeated blindly: a recorded submission is polled, not resubmitted; a rejection is reported and never retried
# automatically; an accepted but unstapled image is stapled. The output is never overwritten: an existing output is accepted
# only when its state file shows it came from this exact input.
#
# The expectations below are plain assignments (not read from the environment). The regression replaces them with ad-hoc
# equivalents so the same checks run on a stand-in signed with `codesign -s -`.
REQUIRED_AUTHORITY="Authority=Developer ID Application:"
RELEASE_GRADE=1                       # 1: also require a team identifier and a secure timestamp on every executable
REQUIRED_ARCHS="arm64 x86_64"
NOTARY_PROFILE="loom-notary"
NOTARY_WAIT_SECONDS=3600
NOTARY_POLL_SECONDS=20

# ---- stand-ins for Apple and the disk-image tools (shell functions, replaced by the regression) ------------------------
dmg_signature_state() { local o; o="$(codesign -dv "$1" 2>&1 || true)"; if grep -q "not signed at all" <<<"$o"; then echo unsigned; else echo signed; fi; } # captured first: grep -q ending a pipeline can fail it under pipefail
dmg_signature_ok() { # the distribution image's own signature, as a release requires
  local info; codesign --verify --strict "$1" >/dev/null 2>&1 || return 1
  info="$(codesign -dvv "$1" 2>&1)"; grep -qF "$REQUIRED_AUTHORITY" <<<"$info" || return 1
  [ "$RELEASE_GRADE" = 0 ] || grep -q "^Timestamp=" <<<"$info"
}
staple_validate() { xcrun stapler validate "$1" >/dev/null 2>&1; }
notary_submit() { local id; id="$(xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --no-wait --output-format plist | plutil -extract id raw -o - -)" && [ -n "$id" ] && echo "$id"; }
notary_status() { xcrun notarytool info "$1" --keychain-profile "$NOTARY_PROFILE" --output-format plist | plutil -extract status raw -o - -; }
notary_log() { xcrun notarytool log "$1" --keychain-profile "$NOTARY_PROFILE"; }
notary_staple() { xcrun stapler staple "$1"; }
gatekeeper_assess() { spctl --assess --type install --verbose=4 "$1"; }

sorted_words() { tr ' ' '\n' | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//'; }

# Every executable in the app is signed as a release requires. Prints the first problem and returns 1.
check_signed_app() { # check_signed_app APP
  local app="$1" f path info a ents
  for f in "${CODE_FILES[@]}"; do
    path="$app/$f"; [ -f "$path" ] || { say "missing from the app: $f"; return 1; }
    case "$f" in */CodeResources) continue ;; esac
    codesign --verify --strict "$path" >/dev/null 2>&1 || { say "$f: unsigned, or its signature is invalid"; return 1; }
    info="$(codesign -dvv "$path" 2>&1)"
    grep -qF "$REQUIRED_AUTHORITY" <<<"$info" || { say "$f: not signed by the required authority ($REQUIRED_AUTHORITY)"; return 1; }
    grep -Eq 'flags=0x[0-9a-f]+[(][^)]*runtime' <<<"$info" || { say "$f: the hardened runtime is not enabled"; return 1; }
    if [ "$RELEASE_GRADE" != 0 ]; then
      grep -q "^Timestamp=" <<<"$info" || { say "$f: no secure timestamp"; return 1; }
      grep -q "^TeamIdentifier=" <<<"$info" && ! grep -q "^TeamIdentifier=not set" <<<"$info" || { say "$f: no team identifier"; return 1; }
    fi
    [ "$(lipo -archs "$path" | sorted_words)" = "$(echo "$REQUIRED_ARCHS" | sorted_words)" ] || { say "$f: architectures are '$(lipo -archs "$path")', expected '$REQUIRED_ARCHS'"; return 1; }
    for a in $REQUIRED_ARCHS; do codesign --verify --strict -a "$a" "$path" >/dev/null 2>&1 || { say "$f: the $a slice does not verify"; return 1; }; done
    ents="$(codesign -d --entitlements :- "$path" 2>&1 | grep -c "com.apple.security.virtualization" || true)"
    case "$f" in
      Contents/Resources/harpoon/bin/harpoon) [ "$ents" != 0 ] || { say "$f: the virtualization entitlement is missing"; return 1; } ;;
      *) [ "$ents" = 0 ] || { say "$f: carries the virtualization entitlement, which only the Swift runtime may have"; return 1; } ;;
    esac
  done
  codesign --verify --deep --strict "$app" >/dev/null 2>&1 || { say "the app bundle does not verify (deep, strict)"; return 1; }
}

# The recorded progress of one output: atomic key=value updates, mode 600.
state_get() { sed -n "s/^$2=//p" "$1" 2>/dev/null | tail -n 1; }
state_set() { # state_set FILE KEY VALUE
  local f="$1" k="$2" v="$3" tmp
  tmp="$(mktemp "$f.XXXXXX")" || die "cannot write $f"
  { { [ -f "$f" ] && grep -v "^$k=" "$f"; } || true; echo "$k=$v"; } > "$tmp"
  chmod 600 "$tmp"; mv -f "$tmp" "$f"
}

# Mount IMAGE read-only, run check_signed_app and app_verifier on its app, require that it equals REFERENCE (the mounted
# signed input) path for path and byte for byte, then detach. Used on the final distribution image.
verify_image_app() { # verify_image_app IMAGE WORKTMP LABEL REFERENCE_MOUNT
  local image="$1" tmp="$2" label="$3" ref="$4" mp="$2/mnt-$3" diffs
  attach readonly "$image" "$mp"
  [ -d "$mp/Harpoon.app" ] || die "$label: no Harpoon.app at the root of the image"
  check_signed_app "$mp/Harpoon.app" || die "$label: the app is not a correctly Developer ID-signed app (details above)"
  app_verifier "$mp/Harpoon.app" >/dev/null 2>&1 || die "$label: bundle or signature verification failed (verify-bundle.sh / verify-signatures.sh)"
  diffs="$(run_bounded "$HDIUTIL_TIMEOUT" tree_differences "$ref" "$mp")" || die "$label: its contents do not match the signed input"
  [ -z "$diffs" ] || die "$label: its contents differ from the signed input: $(echo "$diffs" | head -3 | tr '\n' ' ')"
  detach "$mp"
}

resume_run() { # resume_run INPUT OUT_DIR WORK EXPECT STAGE(verify|dmg|apple)
  local input="$1" out_dir="$2" work="$3" expect="$4" stage="$5"
  local in_sha in_bytes base out state tmp in_mp id status waited
  # ---- paths and names: regular files, real directories, no symlinks, an output that is not the input
  [ -f "$input" ] && [ ! -L "$input" ] || die "the input must be a regular file, not a symlink: $input"
  [ -d "$out_dir" ] && [ ! -L "$out_dir" ] || die "--out-dir must be a real directory, not a symlink: $out_dir"
  [ -d "$work" ] && [ ! -L "$work" ] || die "--work-dir must be a real directory, not a symlink: $work"
  [[ "$expect" =~ ^[0-9a-f]{64}$ ]] || die "--resume-from-signed needs --expect-sha256 with the signed input's exact SHA-256 (64 lowercase hex characters)"
  base="$(basename "$input" .dmg)"
  case "$base" in *-signed) out="$out_dir/${base%-signed}.dmg" ;; *) out="$out_dir/$base-dist.dmg" ;; esac
  state="$out.state"
  [ "$(cd -P "$(dirname "$input")" && pwd)/$(basename "$input")" != "$(cd -P "$out_dir" && pwd)/$(basename "$out")" ] || die "the distribution image would overwrite the signed input; use a different --out-dir"
  [ ! -L "$out" ] && [ ! -L "$state" ] || die "$out or its state file is a symlink: refusing"

  # ---- the signed input: exact checksum, integrity, an app that is correctly signed, and an image that is not yet signed or notarized
  in_bytes="$(bytes "$input")"; in_sha="$(sha256 "$input")"
  [ "$in_sha" = "$expect" ] || die "input sha256 $in_sha does not equal the expected $expect"
  say "signed input: $input ($(( in_bytes / 1048576 )) MiB), sha256 matches the expected value"
  run_bounded "$HDIUTIL_TIMEOUT" hdiutil verify "$input" >/dev/null 2>&1 || die "the input image does not verify (hdiutil verify)"
  if [ "$(dmg_signature_state "$input")" != unsigned ] || staple_validate "$input"; then
    die "the input image itself is already signed or notarized, so there is nothing to resume from it. If it is the distribution image, verify it instead; if it came from an earlier run, use that run's output and its .state file"
  fi
  tmp="$(mktemp -d "$work/resume-dmg.XXXXXX")" || die "cannot create a temporary directory in $work"; OWN_DIRS+=("$tmp")
  in_mp="$tmp/mnt-input"
  attach readonly "$input" "$in_mp"
  [ -d "$in_mp/Harpoon.app" ] || die "no Harpoon.app at the root of the signed input"
  check_signed_app "$in_mp/Harpoon.app" || die "the signed input's app is not correctly Developer ID-signed (details above); re-run the application-signing stage instead of resuming"
  app_verifier "$in_mp/Harpoon.app" >/dev/null 2>&1 || die "the signed input does not pass verify-bundle.sh / verify-signatures.sh"
  say "the signed input passes: every executable signed (hardened runtime, timestamps, entitlements, both architectures) and the bundle verifies"
  if [ "$stage" = verify ]; then say "verification only: nothing was written"; return 0; fi

  # ---- an existing output is accepted only when its state shows it was made from this exact input
  if [ -e "$out" ] || [ -e "$state" ]; then
    [ -f "$out" ] && [ -f "$state" ] || die "$out and $state must both exist or both be absent; they are never overwritten or guessed at"
    [ "$(state_get "$state" input_sha256)" = "$in_sha" ] || die "$out exists and its state does not show it came from this input: never overwritten"
    say "resuming the recorded output: stage $(state_get "$state" stage)"
  else
    require_headroom "$out_dir" $(( in_bytes + SLACK_BYTES )) "the distribution image (a clone normally costs nothing; a copy needs the whole size)"
    PARTIAL_FILES+=("$out" "$state")
    cp -c "$input" "$out" 2>/dev/null || cp "$input" "$out" || die "cannot create the distribution image"
    state_set "$state" input_sha256 "$in_sha"; state_set "$state" stage cloned
  fi
  local st; st="$(state_get "$state" stage)"
  case "$st" in rejected) die "Apple rejected submission $(state_get "$state" submission_id) earlier: see \`xcrun notarytool log $(state_get "$state" submission_id) --keychain-profile $NOTARY_PROFILE\`. Nothing is resubmitted automatically; once the cause is fixed, move $out and $state aside and run again" ;; esac

  # ---- sign the distribution image (once)
  if [ "$st" = cloned ]; then
    say "signing the distribution image"
    dmg_signer "$out" || die "signing the distribution image failed"
    dmg_signature_ok "$out" || die "the distribution image's signature does not verify as a release requires"
    state_set "$state" signed_sha256 "$(sha256 "$out")"; state_set "$state" stage dmg-signed
    PARTIAL_FILES=() # signed and recorded: from here on the output is kept whatever happens next
    st=dmg-signed
  fi
  [ "$(sha256 "$input")" = "$in_sha" ] || die "the signed input changed during the run"
  if [ "$stage" = dmg ]; then say "distribution image signed (not notarized): $out"; return 0; fi

  # ---- notarize: a recorded submission is polled, never resubmitted
  if [ "$st" = dmg-signed ] || [ "$st" = submitted ]; then
    id="$(state_get "$state" submission_id)"
    if [ -z "$id" ]; then
      require_headroom "$out_dir" "$SLACK_BYTES" "stapling"
      say "submitting to Apple (profile $NOTARY_PROFILE)"
      id="$(notary_submit "$out")" || die "submission to Apple failed; nothing was recorded, so a rerun submits again"
      state_set "$state" submission_id "$id"; state_set "$state" stage submitted
    else say "submission $id was already recorded: polling it, not resubmitting"; fi
    waited=0
    while :; do
      status="$(notary_status "$id" 2>/dev/null || echo unknown)"; say "notarization $id: $status"
      case "$status" in
        Accepted) break ;;
        Invalid|Rejected) state_set "$state" stage rejected; notary_log "$id" > "$out_dir/notary-$id.log" 2>/dev/null || true
          die "Apple rejected submission $id ($status). The log is in $out_dir/notary-$id.log (or \`xcrun notarytool log $id --keychain-profile $NOTARY_PROFILE\`). It is not resubmitted automatically" ;;
      esac
      [ "$waited" -lt "$NOTARY_WAIT_SECONDS" ] || die "submission $id is still '$status' after ${NOTARY_WAIT_SECONDS}s. Run the same command again later: it will keep polling this submission"
      sleep "$NOTARY_POLL_SECONDS"; waited=$(( waited + NOTARY_POLL_SECONDS ))
    done
    state_set "$state" stage accepted; st=accepted
  fi
  if [ "$st" = accepted ]; then
    say "stapling"
    notary_staple "$out" || die "stapling failed; the image is still signed and accepted, run the same command again to retry the staple"
    staple_validate "$out" || die "the stapled ticket does not validate"
    state_set "$state" stapled_sha256 "$(sha256 "$out")"; state_set "$state" stage stapled; st=stapled
  fi

  # ---- verify the final artifact completely, then (and only then) write the checksums and provenance
  say "verifying the final distribution image"
  run_bounded "$HDIUTIL_TIMEOUT" hdiutil verify "$out" >/dev/null 2>&1 || die "the final image does not verify (hdiutil verify)"
  dmg_signature_ok "$out" || die "the final image's signature does not verify"
  staple_validate "$out" || die "the final image is not stapled"
  gatekeeper_assess "$out" || die "Gatekeeper does not accept the final image"
  verify_image_app "$out" "$tmp" final "$in_mp"
  [ "$(sha256 "$input")" = "$in_sha" ] || die "the signed input changed during the run"
  local final id_rec sums prov; final="$(sha256 "$out")"; id_rec="$(state_get "$state" submission_id)"
  sums="$out_dir/SHA256SUMS"; prov="$out_dir/SIGNING-PROVENANCE.txt"
  local line="$final  $(basename "$out")"
  if [ -e "$sums" ]; then [ "$(cat "$sums")" = "$line" ] || die "$sums exists and does not describe this image: never overwritten"
  else printf '%s\n' "$line" > "$sums.tmp.$$" && mv "$sums.tmp.$$" "$sums"; fi
  if [ ! -e "$prov" ]; then
    {
      echo "signed input      $(basename "$input") sha256 $in_sha ($in_bytes bytes)"
      echo "final artifact    $(basename "$out") sha256 $final ($(bytes "$out") bytes)"
      echo "notarization      submission $id_rec accepted; stapled"
      echo "identity          ${HARPOON_SIGN_IDENTITY:-unknown}"
      echo "tooling commit    $(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
      echo "date              $(date -u +%Y-%m-%dT%H:%M:%SZ)"
      echo "macOS             $(sw_vers -productVersion 2>/dev/null || echo unknown)"
    } > "$prov.tmp.$$" && mv "$prov.tmp.$$" "$prov"
  fi
  state_set "$state" final_sha256 "$final"; state_set "$state" stage finalized
  say "final: $out sha256 $final"
}


main() {
  local input="" out_dir="" work="" expect="" stage=sign resume=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --resume-from-signed) resume=1; shift ;;
      --out-dir) out_dir="$2"; shift 2 ;;
      --work-dir) work="$2"; shift 2 ;;
      --expect-sha256) expect="$2"; shift 2 ;;
      --dry-run) stage=dry; shift ;;
      --sign-dmg) [ "$stage" = apple ] || stage=dmg; shift ;;
      --submit-to-apple) stage=apple; shift ;;
      -*) die "unknown option $1" ;;
      *) [ -z "$input" ] || die "one input only"; input="$1"; shift ;;
    esac
  done
  [ -n "$input" ] || die "usage: $0 INPUT.dmg [options]"
  out_dir="${out_dir:-$(dirname "$input")}"; work="${work:-$out_dir}"
  [ -d "$out_dir" ] && [ -d "$work" ] || die "--out-dir and --work-dir must exist"
  if [ "$resume" = 1 ]; then
    # no sibling-checksum fallback here: resuming needs the exact checksum of the signed input given explicitly
    case "$stage" in
      dry) stage=verify ;; dmg|apple) ;;
      *) die "--resume-from-signed needs one stage: --dry-run (verify only), --sign-dmg, or --submit-to-apple" ;;
    esac
    for t in hdiutil shasum diff codesign lipo; do command -v "$t" >/dev/null || die "$t is required"; done
    if [ "$stage" != verify ]; then
      [ -n "${HARPOON_SIGN_IDENTITY:-}" ] || die "HARPOON_SIGN_IDENTITY is not set (Developer ID Application)"
      security find-identity -v -p codesigning 2>&1 | grep -F "$HARPOON_SIGN_IDENTITY" | grep -q "Developer ID Application" || die "identity is not a Developer ID Application identity: $HARPOON_SIGN_IDENTITY"
    fi
    trap cleanup EXIT
    resume_run "$input" "$out_dir" "$work" "$expect" "$stage"
    return
  fi
  if [ -z "$expect" ] && [ -f "$(dirname "$input")/SHA256SUMS" ]; then
    expect="$(awk -v n="$(basename "$input")" '$2==n{print $1}' "$(dirname "$input")/SHA256SUMS")"
    [ -z "$expect" ] || say "expected sha256 taken from $(dirname "$input")/SHA256SUMS"
  fi
  for t in hdiutil shasum diff codesign; do command -v "$t" >/dev/null || die "$t is required"; done
  if [ "$stage" != dry ] || [ -n "${HARPOON_SIGN_IDENTITY:-}" ]; then
    [ -n "${HARPOON_SIGN_IDENTITY:-}" ] || die "HARPOON_SIGN_IDENTITY is not set (Developer ID Application)"
    security find-identity -v -p codesigning 2>&1 | grep -F "$HARPOON_SIGN_IDENTITY" | grep -q "Developer ID Application" || die "identity is not a Developer ID Application identity: $HARPOON_SIGN_IDENTITY"
  fi
  trap cleanup EXIT
  sign_dmg_run "$input" "$out_dir" "$work" "$expect" "$stage"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then main "$@"; fi
