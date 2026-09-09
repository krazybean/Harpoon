#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP=$(mktemp -d /tmp/harpoon-initramfs-builder.XXXXXX)
trap 'rm -rf "$TMP"' EXIT
ROOT="$TMP/repo"
OUT="$ROOT/assets/guest/harpoon-initramfs.cpio.gz"
BUILDER="$ROOT/tools/guest-builder/build-initramfs.sh"

fail() { echo "initramfs-builder FAIL: $*" >&2; exit 1; }
mkdir -p "$ROOT/tools/guest-builder/src" "$ROOT/assets/guest"
for path in tools/guest-builder/build-initramfs.sh tools/guest-builder/src/init tools/guest-builder/src/harpoon-mgmt tools/guest-builder/src/harpoon-mgmt-wrapper tools/guest-builder/required-modules.txt tools/guest-builder/required-kernel-features.txt tools/guest-builder/kernel-feature-modules.txt; do
  mkdir -p "$ROOT/$(dirname "$path")"
  cp "$REPO_ROOT/$path" "$ROOT/$path"
done
printf 'test root\n' > "$ROOT/assets/guest/harpoon-root.img"

HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --fingerprint > "$TMP/original"
: > "$OUT"
cp "$TMP/original" "$OUT.inputs"
HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --check
printf '\n# stale\n' >> "$ROOT/tools/guest-builder/src/init"
! HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --check
cp "$REPO_ROOT/tools/guest-builder/src/init" "$ROOT/tools/guest-builder/src/init"
HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --fingerprint > "$TMP/restored"
cmp -s "$TMP/original" "$TMP/restored" || fail "restored source fingerprint changed"
touch "$ROOT/tools/guest-builder/src/init"
HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --check
printf '\n# stale\n' >> "$ROOT/tools/guest-builder/required-modules.txt"
! HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --check
cp "$REPO_ROOT/tools/guest-builder/required-modules.txt" "$ROOT/tools/guest-builder/required-modules.txt"
cp "$TMP/original" "$OUT.inputs"
rm "$OUT.inputs"
! HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --check
printf 'malformed\n' > "$OUT.inputs"
! HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --check
cp "$TMP/original" "$OUT.inputs"
rm "$OUT"
! HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --check

FAKE="$TMP/bin"
mkdir -p "$FAKE"
cat > "$FAKE/docker" <<'EOF'
#!/bin/bash
printf '%s|%s|%s\n' "${DOCKER_HOST:-}" "${DOCKER_CONTEXT:-}" "$*" >> "$DOCKER_LOG"
case " $* " in
  *' info '*) [ "${1:-}" = "--context" ] && [ "${2:-}" = "harpoon" ] && exit 0; [ -n "${DOCKER_HOST:-}" ] && exit 0; [ "${1:-}" = "--context" ] && exit 0; exit 1 ;;
  *' run '*) : > "$HARPOON_INITRAMFS_OUT"; exit 0 ;;
esac
EOF
chmod +x "$FAKE/docker"
for mode in override host caller fallback; do
  rm -rf "$OUT" "$OUT.inputs" "$TMP/$mode.log" "$TMP/$mode-config"
  case "$mode" in
    override) PATH="$FAKE:$PATH" DOCKER_LOG="$TMP/$mode.log" DOCKER_CONFIG="$TMP/$mode-config" HARPOON_INITRAMFS_OUT="$OUT" DOCKER_HOST=unix:///ignored HARPOON_BUILD_DOCKER_CONTEXT=chosen "$BUILDER" ;;
    host) PATH="$FAKE:$PATH" DOCKER_LOG="$TMP/$mode.log" DOCKER_CONFIG="$TMP/$mode-config" HARPOON_INITRAMFS_OUT="$OUT" DOCKER_HOST=unix:///chosen "$BUILDER" ;;
    caller) PATH="$FAKE:$PATH" DOCKER_LOG="$TMP/$mode.log" DOCKER_CONFIG="$TMP/$mode-config" HARPOON_INITRAMFS_OUT="$OUT" DOCKER_CONTEXT=chosen "$BUILDER" ;;
    fallback) PATH="$FAKE:$PATH" DOCKER_LOG="$TMP/$mode.log" DOCKER_CONFIG="$TMP/$mode-config" HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" ;;
  esac
  case "$mode" in override|caller) grep -q -- '--context chosen' "$TMP/$mode.log" || fail "$mode context not selected" ;; esac
  [ "$mode" != host ] || grep -q '^unix:///chosen||' "$TMP/$mode.log" || fail "explicit endpoint not selected"
  [ "$mode" != fallback ] || grep -q -- '--context harpoon' "$TMP/$mode.log" || fail "harpoon fallback not selected"
  grep -Eq '(^|[| ])run ' "$TMP/$mode.log" || fail "$mode build did not use selected engine"
  [ ! -e "$TMP/$mode-config" ] || fail "$mode mutated Docker config"
done
echo "initramfs-builder PASS"
