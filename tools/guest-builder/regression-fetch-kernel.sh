#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP=$(mktemp -d /tmp/harpoon-fetch-kernel.XXXXXX)
trap 'rm -rf "$TMP"' EXIT
ROOT="$TMP/repo"
mkdir -p "$ROOT/tools/guest-builder" "$ROOT/assets/guest/.cache" "$TMP/bin"
cp "$REPO_ROOT/tools/guest-builder/fetch-kernel.sh" "$ROOT/tools/guest-builder/fetch-kernel.sh"
printf stale > "$ROOT/assets/guest/.cache/vmlinuz-virt"
printf stale > "$ROOT/assets/guest/.cache/initramfs-virt"
touch -d '2030-01-01' "$ROOT/assets/guest/Image-virt" 2>/dev/null || touch "$ROOT/assets/guest/Image-virt"

cat > "$TMP/bin/curl" <<'EOF'
#!/bin/bash
set -euo pipefail
out=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    *) url="$1"; shift ;;
  esac
done
if [ "${url##*/}" = vmlinuz-virt ]; then
  printf 'canonical-%s' "${url##*/}" | gzip -c > "$out"
else
  printf 'canonical-%s' "${url##*/}" > "$out"
fi
printf '%s\n' "${url##*/}" >> "$CURL_LOG"
EOF
cat > "$TMP/bin/shasum" <<'EOF'
#!/bin/bash
set -euo pipefail
case "$3" in
  *vmlinuz-virt*)
    if grep -q stale "$3"; then printf '%s  %s\n' stale "$3"; else printf '%s  %s\n' dd37c4460f933ec909ff022c34944ff7e1a5300aff99750e3bdc02cd4074c332 "$3"; fi ;;
  *initramfs-virt*)
    if grep -q stale "$3"; then printf '%s  %s\n' stale "$3"; else printf '%s  %s\n' 384f2d828bbbf237bc57d03c0b5bb371482f01868b5f5fea151a2022554b889c "$3"; fi ;;
  *) exit 1 ;;
esac
EOF
chmod +x "$TMP/bin/curl" "$TMP/bin/shasum"

CURL_LOG="$TMP/curl.log" PATH="$TMP/bin:$PATH" bash "$ROOT/tools/guest-builder/fetch-kernel.sh" >/dev/null
[ "$(wc -l < "$TMP/curl.log" | tr -d ' ')" = 2 ] || { echo 'fetch-kernel FAIL: stale cache was not refreshed' >&2; exit 1; }
grep -q '^vmlinuz-virt$' "$TMP/curl.log"
grep -q '^initramfs-virt$' "$TMP/curl.log"

CURL_LOG="$TMP/curl-second.log" PATH="$TMP/bin:$PATH" bash "$ROOT/tools/guest-builder/fetch-kernel.sh" >/dev/null
[ ! -s "$TMP/curl-second.log" ] || { echo 'fetch-kernel FAIL: matching cache was downloaded again' >&2; exit 1; }
echo "fetch-kernel PASS"
