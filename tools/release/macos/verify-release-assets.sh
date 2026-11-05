#!/bin/bash
set -euo pipefail
# A GitHub release is not successful until its DMG is actually attached and is the file that was built. Release notes
# that name a DMG and its checksum prove nothing about distribution (v2.0.0-rc.2 was published with no assets at all).
# Run this after uploading, with the DMG that tools/release/macos/release.sh produced; it changes nothing on GitHub.
#
#   tools/release/macos/verify-release-assets.sh v2.0.0-rc.3 dist/v2.0.0-rc.3/Harpoon-2.0.0-rc.3-universal.dmg [OWNER/REPO]
#
# Fails (exit 1, with the reason) unless the release has an asset with the DMG's file name, in state `uploaded`, of the
# same size, whose SHA-256 equals the local file's. The digest is GitHub's own; when GitHub reports none, the asset is
# downloaded and hashed.
TAG="${1:?usage: $0 TAG DMG [OWNER/REPO]}"
DMG="${2:?usage: $0 TAG DMG [OWNER/REPO]}"
REPO="${3:-}"
fail() { echo "[release-assets] FAIL: $*" >&2; exit 1; }
[ -f "$DMG" ] || fail "the local DMG $DMG does not exist"
command -v gh >/dev/null 2>&1 || fail "gh is required"
command -v node >/dev/null 2>&1 || fail "node is required"
REPO_ARGS=(); [ -z "$REPO" ] || REPO_ARGS=(--repo "$REPO")
NAME="$(basename "$DMG")"
LOCAL_SHA="$(shasum -a 256 "$DMG" | cut -d' ' -f1)"
LOCAL_SIZE="$(stat -f%z "$DMG" 2>/dev/null || stat -c%s "$DMG")"

JSON="$(gh release view "$TAG" ${REPO_ARGS[@]+"${REPO_ARGS[@]}"} --json assets,isDraft 2>&1)" || fail "release $TAG cannot be read: $JSON"
# one line: COUNT<TAB>FOUND(0|1)<TAB>STATE<TAB>SIZE<TAB>DIGEST ; the asset names go to stderr
FIELDS="$(printf '%s' "$JSON" | node -e '
  const j = JSON.parse(require("fs").readFileSync(0, "utf8")); const want = process.argv[1];
  const a = (j.assets || []).find((x) => x.name === want);
  console.error("assets on the release: " + ((j.assets || []).map((x) => x.name).join(", ") || "(none)"));
  console.log([(j.assets || []).length, a ? 1 : 0, a ? a.state : "", a ? a.size : "", a ? (a.digest || "") : ""].join("\t"));
' "$NAME")" || fail "the release description could not be parsed"
IFS=$'\t' read -r COUNT FOUND STATE SIZE DIGEST <<<"$FIELDS"

[ "$COUNT" != 0 ] || fail "release $TAG has no assets: nothing is distributed, whatever its notes say"
[ "$FOUND" = 1 ] || fail "release $TAG has no asset named $NAME"
[ "$STATE" = uploaded ] || fail "asset $NAME is in state '$STATE', not 'uploaded'"
[ "$SIZE" = "$LOCAL_SIZE" ] || fail "asset $NAME is $SIZE bytes; the built DMG is $LOCAL_SIZE"
if [ -z "$DIGEST" ]; then
  TMP="$(mktemp -d /tmp/harpoon-release-assets.XXXXXX)"; trap 'rm -rf "$TMP"' EXIT
  gh release download "$TAG" ${REPO_ARGS[@]+"${REPO_ARGS[@]}"} --pattern "$NAME" --dir "$TMP" >/dev/null 2>&1 || fail "asset $NAME could not be downloaded to verify it"
  DIGEST="sha256:$(shasum -a 256 "$TMP/$NAME" | cut -d' ' -f1)"
fi
[ "$DIGEST" = "sha256:$LOCAL_SHA" ] || fail "asset $NAME has digest $DIGEST; the built DMG is sha256:$LOCAL_SHA"
echo "[release-assets] PASS: $TAG carries $NAME ($SIZE bytes, sha256 $LOCAL_SHA)" >&2
