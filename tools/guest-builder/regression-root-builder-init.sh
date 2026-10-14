#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INIT="$REPO_ROOT/tools/guest-builder/src/alpine-root-builder-init"
TMP="$(mktemp -d /tmp/harpoon-root-builder-init.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "root-builder-init FAIL: $*" >&2; exit 1; }

grep -q '/sbin/apk --initdb add --no-cache e2fsprogs e2fsprogs-extra' "$INIT" || fail "bootstrap does not initialize apk database"
! grep -q '/sbin/apk update' "$INIT" || fail "bootstrap updates before database initialization"
grep -q 'bb mkdir -p /bin /sbin /usr/bin /usr/sbin /usr/local/bin /usr/local/sbin' "$INIT" || fail "BusyBox destinations are not prepared"

# Execute the production writer with host-side fixtures so this checks the
# generated file format, not just the source spelling.
sed -n '/^write_repositories() {/,/^}/p' "$INIT" > "$TMP/write-repositories.sh"
source "$TMP/write-repositories.sh"
REPO_MAIN=https://dl-cdn.alpinelinux.org/alpine/v3.22/main
REPO_COMMUNITY=https://dl-cdn.alpinelinux.org/alpine/v3.22/community
bb() { "$@"; }
write_repositories "$TMP/repositories" "$REPO_MAIN" "$REPO_COMMUNITY"
[ "$(wc -l < "$TMP/repositories" | tr -d ' ')" = 2 ] || fail "APK repositories are not newline-delimited"
! grep -Fq "$REPO_MAIN $REPO_COMMUNITY" "$TMP/repositories" || fail "APK repositories collapsed onto one line"
[ "$(tr '\n' ' ' < "$TMP/repositories")" = "$REPO_MAIN $REPO_COMMUNITY " ] || fail "APK repository set changed"

apk_line=$(grep -n '/sbin/apk --root .* add' "$INIT" | cut -d: -f1)
prep_line=$(grep -n 'bb cp -a /etc/apk/keys' "$INIT" | cut -d: -f1)
[ "$prep_line" -lt "$apk_line" ] || fail "target APK keys are copied too late"

# Execute the exact target-side verification body with a fake target PATH. This
# catches accidental builder command -v/Python lookups without booting VZ.
body="$TMP/verify.sh"
sed -n "/^bb chroot \"\$ROOT\" \/bin\/sh -c '/,/^' || fail target-runtime\$/p" "$INIT" \
  | sed "1d;\$d; s#\"/usr/local#\"$TMP/target/usr/local#g; s#PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin#PATH=$TMP/target/bin#g" > "$body"
mkdir -p "$TMP/target/usr/local/share/harpoon-runtime" "$TMP/target/bin"
for bin in dockerd docker containerd runc socat python3 resize2fs ip iptables nft modprobe; do
  printf '#!/bin/sh\nexit 0\n' > "$TMP/target/bin/$bin"
  chmod +x "$TMP/target/bin/$bin"
done
sh "$body"
for bin in dockerd docker containerd runc socat python3 resize2fs ip iptables nft modprobe; do
  path="$TMP/target/usr/local/share/harpoon-runtime/$bin.path"
  [ "$(cat "$path")" = "$TMP/target/bin/$bin" ] || fail "verification consulted builder PATH for $bin"
done

echo "root-builder-init PASS"
