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
for path in tools/guest-builder/build-initramfs.sh tools/guest-builder/rebuild-initramfs-container.sh tools/guest-builder/build-engine.sh tools/guest-builder/src/init tools/guest-builder/src/harpoon-mgmt tools/guest-builder/src/harpoon-mgmt-wrapper tools/guest-builder/required-modules.txt tools/guest-builder/required-kernel-features.txt tools/guest-builder/kernel-feature-modules.txt; do
  mkdir -p "$ROOT/$(dirname "$path")"
  cp "$REPO_ROOT/$path" "$ROOT/$path"
done
printf 'test root\n' > "$ROOT/assets/guest/harpoon-root.img"

bash -n "$BUILDER"
sh -n "$ROOT/tools/guest-builder/rebuild-initramfs-container.sh"

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
printf '\n# stale\n' >> "$ROOT/tools/guest-builder/rebuild-initramfs-container.sh"
! HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" --check
cp "$REPO_ROOT/tools/guest-builder/rebuild-initramfs-container.sh" "$ROOT/tools/guest-builder/rebuild-initramfs-container.sh"
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
    fallback) ! PATH="$FAKE:$PATH" DOCKER_LOG="$TMP/$mode.log" DOCKER_CONFIG="$TMP/$mode-config" HARPOON_INITRAMFS_OUT="$OUT" "$BUILDER" ;;
  esac
  case "$mode" in override|caller) grep -q -- '--context chosen' "$TMP/$mode.log" || fail "$mode context not selected" ;; esac
  [ "$mode" != host ] || grep -q '^unix:///chosen||' "$TMP/$mode.log" || fail "explicit endpoint not selected"
  [ "$mode" != fallback ] || ! grep -q -- ' run ' "$TMP/$mode.log" || fail "implicit Harpoon fallback was selected"
  [ "$mode" = fallback ] || grep -Eq '(^|[| ])run ' "$TMP/$mode.log" || fail "$mode build did not use selected engine"
  [ ! -e "$TMP/$mode-config" ] || fail "$mode mutated Docker config"
done

# Exercise the production extraction helpers: record exact source/destination
# commands and require rdump to create the extracted subtree.
sed -n '/^debugfs_extract_file() {/,/^debugfs_extract_file \/usr\/bin\/python3.12/p' "$ROOT/tools/guest-builder/rebuild-initramfs-container.sh" | sed '$d' > "$TMP/debugfs-extract.sh"
cat >> "$TMP/debugfs-extract.sh" <<EOF
#!/bin/bash
set -euo pipefail
ROOT_IMG="$ROOT/assets/guest/harpoon-root.img"
STAGING="$TMP/staging"
DEBUGFS_LOG="$TMP/debugfs.log"
mkdir -p "\$STAGING"
debugfs() {
  printf '%s\n' "\$2" >> "\$DEBUGFS_LOG"
  case "\$2" in
    "dump /usr/bin/python3.12 "*) destination="\${2#dump /usr/bin/python3.12 }"; mkdir -p "\$(dirname "\$destination")"; : > "\$destination" ;;
    "dump /usr/lib/libpython3.12.so.1.0 "*) destination="\${2#dump /usr/lib/libpython3.12.so.1.0 }"; mkdir -p "\$(dirname "\$destination")"; : > "\$destination" ;;
    "dump /lib/ld-musl-aarch64.so.1 "*) destination="\${2#dump /lib/ld-musl-aarch64.so.1 }"; mkdir -p "\$(dirname "\$destination")"; : > "\$destination" ;;
    "rdump /usr/lib/python3.12 "*) destination="\${2#rdump /usr/lib/python3.12 }"; mkdir -p "\$destination/python3.12"; : > "\$destination/python3.12/os.py" ;;
    *) echo "unexpected debugfs command: \$2" >&2; return 1 ;;
  esac
}
debugfs_extract_file /usr/bin/python3.12 "\$STAGING/usr/bin/python3.12"
debugfs_extract_file /usr/lib/libpython3.12.so.1.0 "\$STAGING/usr/lib/libpython3.12.so.1.0"
debugfs_extract_file /lib/ld-musl-aarch64.so.1 "\$STAGING/lib/ld-musl-aarch64.so.1"
debugfs_extract_tree /usr/lib/python3.12 "\$STAGING/usr/lib" "\$STAGING/usr/lib/python3.12"
EOF
chmod +x "$TMP/debugfs-extract.sh"
"$TMP/debugfs-extract.sh"
for command in \
  "dump /usr/bin/python3.12 $TMP/staging/usr/bin/python3.12" \
  "dump /usr/lib/libpython3.12.so.1.0 $TMP/staging/usr/lib/libpython3.12.so.1.0" \
  "dump /lib/ld-musl-aarch64.so.1 $TMP/staging/lib/ld-musl-aarch64.so.1" \
  "rdump /usr/lib/python3.12 $TMP/staging/usr/lib"; do
  grep -Fqx "$command" "$TMP/debugfs.log" || fail "missing debugfs command: $command"
done
[ -f "$TMP/staging/usr/lib/python3.12/os.py" ] || fail "rdump subtree was not extracted"

# The module selector must ignore a firmware sibling and enforce the pinned KVER.
{
  printf 'MODLOOP_TMP="%s"\n' "$TMP"
  sed -n '/^# Find the one kernel version dir with module metadata;/,/^echo "\[rebuild\] kernel \$KVER"/p' "$ROOT/tools/guest-builder/rebuild-initramfs-container.sh"
} > "$TMP/select-kver.sh"
mkdir -p "$TMP/modloop/modules/firmware" "$TMP/modloop/modules/6.12.110-0-virt"
: > "$TMP/modloop/modules/6.12.110-0-virt/modules.dep"
sh "$TMP/select-kver.sh" 2>"$TMP/select-kver.log"
grep -q 'kernel 6.12.110-0-virt' "$TMP/select-kver.log" || fail "firmware sibling changed KVER selection"
rm -rf "$TMP/modloop/modules/6.12.110-0-virt"
! sh "$TMP/select-kver.sh" 2>"$TMP/select-kver-zero.log"
grep -q 'no kernel module directory with modules.dep' "$TMP/select-kver-zero.log" || fail "zero KVER candidates were accepted"
mkdir -p "$TMP/modloop/modules/6.12.110-0-virt" "$TMP/modloop/modules/6.12.111-0-virt"
: > "$TMP/modloop/modules/6.12.110-0-virt/modules.dep"
: > "$TMP/modloop/modules/6.12.111-0-virt/modules.dep"
! sh "$TMP/select-kver.sh" 2>"$TMP/select-kver-multiple.log"
grep -q 'multiple kernel module directories with modules.dep' "$TMP/select-kver-multiple.log" || fail "multiple KVER candidates were accepted"

# Exercise the production extraction error path and report the actual command.
sed -n '/^debugfs_extract_file() {/,/^debugfs_extract_tree()/p' "$ROOT/tools/guest-builder/rebuild-initramfs-container.sh" | sed '$d' > "$TMP/debugfs-failure.sh"
cat >> "$TMP/debugfs-failure.sh" <<EOF
#!/bin/bash
set -euo pipefail
ROOT_IMG="$ROOT/assets/guest/harpoon-root.img"
debugfs() { echo "corrupt image" >&2; return 1; }
if debugfs_extract_file /usr/bin/python3.12 "$TMP/missing"; then
  exit 1
fi
EOF
chmod +x "$TMP/debugfs-failure.sh"
! "$TMP/debugfs-failure.sh" 2>"$TMP/debugfs-extract-error"
grep -q 'debugfs extraction failed' "$TMP/debugfs-extract-error"

# A successful debugfs process can still report corruption on stderr; detect it.
sed -n '/^debugfs_extract_tree() {/,/^debugfs_extract_tree \/usr\/lib\/python3.12/p' "$ROOT/tools/guest-builder/rebuild-initramfs-container.sh" | sed '$d' > "$TMP/debugfs-regex.sh"
cat >> "$TMP/debugfs-regex.sh" <<EOF
#!/bin/sh
set -euo pipefail
ROOT_IMG="$ROOT/assets/guest/harpoon-root.img"
debugfs() { echo 'error: checksum mismatch' >&2; return 0; }
if debugfs_extract_tree /usr/lib/python3.12 "$TMP/staging" "$TMP/staging/python3.12"; then
  exit 1
fi
EOF
chmod +x "$TMP/debugfs-regex.sh"
! "$TMP/debugfs-regex.sh" 2>"$TMP/debugfs-regex-error"
grep -q 'could not read canonical root image' "$TMP/debugfs-regex-error"
echo "initramfs-builder PASS"
