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
grep -q 'PACKAGES="docker-engine docker-cli containerd runc socat python3 ca-certificates alpine-release e2fsprogs' "$INIT" || fail "target package closure omits alpine-release"
grep -q 'if ! write_manifest; then' "$INIT" || fail "manifest generation lacks explicit failure routing"
grep -q 'fail manifest' "$INIT" || fail "manifest generation does not emit the FAIL marker"

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

apk_line=$(grep -n '/sbin/apk --root "$ROOT" .* add' "$INIT" | cut -d: -f1)
prep_line=$(grep -n 'bb cp -a /etc/apk/keys/. "$ROOT' "$INIT" | cut -d: -f1)
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

# Missing release metadata must use the production manifest failure route,
# rather than letting PID 1 die under set -e.
sed -n '/^write_manifest() {/,/^}/p' "$INIT" > "$TMP/manifest.sh"
cat > "$TMP/manifest-failure.sh" <<EOF
#!/bin/sh
set -eu
ROOT="$TMP/target"
bb() { "\$@"; }
fail() { printf 'HARPOON_ROOT_BUILDER_FAIL %s\\n' "\$*" > "$TMP/fail-marker"; }
. "$TMP/manifest.sh"
if ! write_manifest; then
  fail manifest
fi
EOF
chmod +x "$TMP/manifest-failure.sh"
mkdir -p "$TMP/target/usr/local/share/harpoon-runtime" "$TMP/target/etc/apk"
printf '%s\n' 'main' > "$TMP/target/etc/apk/repositories"
sh "$TMP/manifest-failure.sh"
[ "$(cat "$TMP/fail-marker")" = 'HARPOON_ROOT_BUILDER_FAIL manifest' ] || fail "missing release did not route through FAIL marker"

# Exercise the exact production teardown function and its ordering.
sed -n '/^finalize_target() {/,/^if ! finalize_target; then/p' "$INIT" | sed '$d' > "$TMP/finalize.sh"
cat > "$TMP/finalize-test.sh" <<EOF
#!/bin/sh
set -eu
ROOT="$TMP/target"
TARGET=/dev/vda
MARKER="$TMP/marker"
bb() {
  case "\$1" in
    umount) printf '%s %s\\n' "\$1" "\$2" >> "$TMP/events" ;;
    sync) printf '%s\\n' "\$1" >> "$TMP/events" ;;
    cat) cat ;;
  esac
}
fail() { printf 'HARPOON_ROOT_BUILDER_FAIL %s\\n' "\$*" > "$TMP/fail-marker"; return 1; }
. "$TMP/finalize.sh"
finalize_target
EOF
mkdir -p "$TMP/bin" "$TMP/target"
: > "$TMP/events"
printf '#!/bin/sh\nprintf "clean\\n"\nexit 0\n' > "$TMP/bin/e2fsck"
chmod +x "$TMP/bin/e2fsck"
PATH="$TMP/bin:$PATH" sh "$TMP/finalize-test.sh"
sed -n '1p' "$TMP/events" | grep -Fq "umount $TMP/target/sys" || fail "sys unmount ordering changed"
sed -n '2p' "$TMP/events" | grep -Fq "umount $TMP/target/proc" || fail "proc unmount ordering changed"
sed -n '3p' "$TMP/events" | grep -Fq "umount $TMP/target/dev" || fail "dev unmount ordering changed"
sed -n '4p' "$TMP/events" | grep -Fxq "sync" || fail "sync ordering changed"
sed -n '5p' "$TMP/events" | grep -Fq "umount $TMP/target" || fail "root unmount ordering changed"

# A non-zero e2fsck status must route to the explicit FAIL marker.
printf '#!/bin/sh\nexit 4\n' > "$TMP/bin/e2fsck"
sed 's/^finalize_target$/! finalize_target/' "$TMP/finalize-test.sh" > "$TMP/finalize-failure-test.sh"
PATH="$TMP/bin:$PATH" sh "$TMP/finalize-failure-test.sh"
[ "$(cat "$TMP/fail-marker")" = 'HARPOON_ROOT_BUILDER_FAIL e2fsck-4' ] || fail "e2fsck failure did not route through FAIL marker"

echo "root-builder-init PASS"
