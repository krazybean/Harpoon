#!/bin/bash
set -euo pipefail
# Sign the app inside an already-built, already-verified universal DMG (for rc.3: the one CI produced) without
# rebuilding anything. The DMG is a read-only compressed image, so it is mounted *writable through a shadow file*
# (hdiutil -shadow: copy-on-write, only the blocks signing changes are stored), the existing Developer ID signer runs on
# the mounted app, and the image is converted back to UDZO with the same shadow. The input file is never modified.
#
#   tools/release/macos/sign-existing-dmg.sh INPUT.dmg [--out-dir DIR] [--work-dir DIR] [--expect-sha256 HEX]
#                                            [--dry-run] [--sign-dmg] [--submit-to-apple]
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

say() { echo "[sign-dmg] $*" >&2; }
die() { say "FAIL: $*"; exit 1; }

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
    if mount | grep -qF " on $real "; then hdiutil detach "$m" >/dev/null 2>&1 || hdiutil detach -force "$m" >/dev/null 2>&1 || say "WARNING: could not detach $m"; fi
  done
  for f in ${TEMP_FILES[@]+"${TEMP_FILES[@]}"}; do rm -f -- "$f"; done
  for f in ${PARTIAL_FILES[@]+"${PARTIAL_FILES[@]}"}; do rm -f -- "$f"; done
  for (( i=${#OWN_DIRS[@]}-1; i>=0; i-- )); do rmdir -- "${OWN_DIRS[$i]}" 2>/dev/null || true; done
  return $status
}

sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
bytes() { stat -f%z "$1" 2>/dev/null || stat -c%s "$1"; }

attach() { # attach MODE(shadow|readonly) IMAGE MOUNTPOINT [SHADOW]
  local mode="$1" image="$2" mp="$3" shadow="${4:-}"
  mkdir -p "$mp" || die "cannot create $mp"; OWN_DIRS+=("$mp")
  if [ "$mode" = shadow ]; then
    [ ! -e "$shadow" ] || die "the shadow file $shadow already exists; a shadow is never reused (it is not checked against its image)"
    TEMP_FILES+=("$shadow")
    hdiutil attach -shadow "$shadow" -nobrowse -noautoopen -noverify -mountpoint "$mp" "$image" >/dev/null || die "could not mount $image writable through a shadow"
  else
    hdiutil attach -readonly -nobrowse -noautoopen -noverify -mountpoint "$mp" "$image" >/dev/null || die "could not mount $image read-only"
  fi
  MOUNTS+=("$mp")
}
detach() { # detach MOUNTPOINT : plain detach, never forced here (a busy volume is a failure to look at)
  hdiutil detach "$1" >/dev/null || die "could not detach $1 (something is using it)"
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
  hdiutil verify "$input" >/dev/null 2>&1 || die "the input image does not verify (hdiutil verify)"

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
  mdutil -i off "$mnt" >/dev/null 2>&1 || true
  say "signing Harpoon.app inside the image"
  app_signer "$mnt/Harpoon.app" || die "signing the app inside the image failed"
  app_verifier "$mnt/Harpoon.app" || die "the freshly signed app does not verify"
  local j; for j in "${VOLUME_JUNK[@]}"; do rm -rf -- "${mnt:?}/$j"; done
  sync || true
  detach "$mnt"
  local sh_bytes; sh_bytes="$(bytes "$shadow")"
  say "shadow holds $(( sh_bytes / 1048576 )) MiB of changed blocks"

  [ "$(sha256 "$input")" = "$in_sha" ] || die "the input image changed while it was being signed"
  require_headroom "$out_dir" $(( in_bytes + SLACK_BYTES )) "converting the signed image"
  PARTIAL_FILES+=("$out")
  hdiutil convert "$input" -format UDZO -shadow "$shadow" -o "$out" >/dev/null || die "hdiutil convert failed"
  hdiutil verify "$out" >/dev/null 2>&1 || die "the signed image does not verify (hdiutil verify)"
  [ "$(sha256 "$input")" = "$in_sha" ] || die "the input image changed during conversion"

  say "comparing the signed image with the input: only code signatures may differ"
  local a="$tmp/in" b="$tmp/out" line rel allowed d ok
  attach readonly "$input" "$a"; attach readonly "$out" "$b"
  local diffs; diffs="$(diff -rq "$a" "$b" -x .fseventsd -x .Trashes -x .Spotlight-V100 -x .TemporaryItems || true)"
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    case "$line" in
      "Files $a/Harpoon.app/"*" and "*" differ") rel="${line#Files $a/Harpoon.app/}"; rel="${rel%% and *}" ;;
      *) die "unexpected difference after signing: $line" ;;
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

main() {
  local input="" out_dir="" work="" expect="" stage=sign
  while [ $# -gt 0 ]; do
    case "$1" in
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
