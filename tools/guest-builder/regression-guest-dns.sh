#!/bin/bash
# Guest DNS survives DHCP renewal. Runs the guest's real busybox udhcpc script (fixtures/udhcpc-default.script) and the real
# DNS functions from src/init in a sandbox; only `ip` and `nslookup` are stubbed (the host has no Linux network stack).
#   bash tools/guest-builder/regression-guest-dns.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
INIT="$HERE/src/init"
FIX="$HERE/fixtures"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/harpoon-guest-dns.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
fail() { echo "guest-dns FAIL: $*" >&2; exit 1; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

# ---- the fixtures are the guest's own files -------------------------------------------------------------------------------
[ "$(sha "$FIX/udhcpc-default.script")" = 1cf94e3918ce389c722f4408490ae7334321a5ca2aec2c945b0c47662aa274a6 ] || fail "udhcpc-default.script is not the pinned file"
[ "$(sha "$FIX/udhcpc.conf")" = 5b372209e01cda07c87b8afa6d1ab3d7e8daf18a1f2332d744203a6dc289eb1a ] || fail "udhcpc.conf is not the pinned file"

# the supported mechanism: the script sources udhcpc.conf and resolvconf() honours NO_DNS for the interface
grep -q '^\[ -f "\$UDHCPC_CONF" \] && \. "\$UDHCPC_CONF"' "$FIX/udhcpc-default.script" || fail "script no longer sources udhcpc.conf"
grep -q 'for i in \$NO_DNS; do' "$FIX/udhcpc-default.script" || fail "script no longer has NO_DNS"
grep -q 'NO_DNS=' "$FIX/udhcpc.conf" || fail "stock udhcpc.conf no longer documents NO_DNS"

# ---- sandbox -----------------------------------------------------------------------------------------------------------------
ROOT="$TMP/root"; STUBS="$TMP/stubs"; STATE="$TMP/state"; CONSOLE="$TMP/console"
mkdir -p "$STUBS"

# The only edits to the real script: its three absolute locations point into the sandbox (it hard-codes /etc and PATH).
sed -e "s#^UDHCPC=\"/etc/udhcpc\"\$#UDHCPC=\"$ROOT/etc/udhcpc\"#" \
    -e "s#^RESOLV_CONF=\"/etc/resolv.conf\"\$#RESOLV_CONF=\"$ROOT/etc/resolv.conf\"#" \
    -e "s#^export PATH=/usr/bin:/bin:/usr/sbin:/sbin\$#export PATH=\"$STUBS:\$PATH\"#" \
    "$FIX/udhcpc-default.script" > "$TMP/default.script"
[ "$(diff "$FIX/udhcpc-default.script" "$TMP/default.script" | grep -c '^>')" = 3 ] || fail "sandboxing changed other than the three locations"

cat > "$STUBS/ip" <<'STUB'
#!/bin/sh
# Linux `ip` for the commands default.script uses: a tiny model of "address present" and "default route present".
echo "$*" >> "$STATE/ip.log"
case "$*" in
  "-4 addr show dev"*) [ -f "$STATE/addr" ] && echo "    inet 192.168.64.3/24 brd 192.168.64.255 scope global eth0" ;;
  "-4 addr flush"*) rm -f "$STATE/addr" ;;
  "-4 addr add"*) : > "$STATE/addr" ;;
  "-4 link show dev"*) echo "2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500" ;;
  "-4 route show") [ -f "$STATE/route" ] && echo "default via 192.168.64.1 dev eth0 metric 202" ;;
  "-4 route del"*) exit 1 ;;
  "-4 route add"*) : > "$STATE/route" ;;
esac
exit 0
STUB
cat > "$STUBS/nslookup" <<'STUB'
#!/bin/sh
# Resolves unless the first nameserver in resolv.conf is the dead gateway (or NSL_ALWAYS_FAIL is set).
first=$(grep -m1 '^nameserver' "$NSL_RESOLV" | cut -d' ' -f2)
if [ -n "${NSL_ALWAYS_FAIL:-}" ] || [ "$first" = "${NSL_DEAD:-192.168.64.1}" ]; then
  echo "nslookup: read: Connection refused"; exit 1
fi
printf 'Server:\t\t%s\nAddress:\t%s:53\n\nName:\t%s\nAddress: 151.101.130.132\n' "$first" "$first" "$1"
STUB
chmod +x "$STUBS/ip" "$STUBS/nslookup"
export STATE NSL_RESOLV="$ROOT/etc/resolv.conf"
export PATH="$STUBS:$PATH"   # init's own nslookup/ip calls must hit the stubs, never the host network
[ "$(command -v nslookup)" = "$STUBS/nslookup" ] || fail "stub nslookup is not first on PATH"

fresh() {  # a guest at boot: stock udhcpc.conf, no resolv.conf, no address, no route
  rm -rf "$ROOT" "$STATE"; mkdir -p "$ROOT/etc/udhcpc" "$ROOT/tmp" "$STATE"
  cp "$FIX/udhcpc.conf" "$ROOT/etc/udhcpc/udhcpc.conf"; : > "$CONSOLE"
  unset NSL_ALWAYS_FAIL NSL_DEAD
}
dhcp() {  # dhcp EVENT DNS [INTERFACE]: what udhcpc does (background job: stdin is /dev/null)
  env interface="${3:-eth0}" ip=192.168.64.3 mask=24 broadcast=192.168.64.255 subnet=255.255.255.0 router=192.168.64.1 dns="$2" domain=lan \
    bash "$TMP/default.script" "$1" < /dev/null >> "$TMP/script.out" 2>&1
}
# the production functions, with their file locations pointed into the sandbox
sed -n '/^# BEGIN harpoon-guest-dns/,/^# END harpoon-guest-dns/p' "$INIT" > "$TMP/dns-functions.sh"
[ -s "$TMP/dns-functions.sh" ] && grep -q '^harpoon_dns_select()' "$TMP/dns-functions.sh" || fail "cannot extract the DNS functions from init"
load() {  # load CMDLINE-TEXT
  printf '%s\n' "$1" > "$TMP/cmdline"
  export HARPOON_CONSOLE="$CONSOLE" HARPOON_RESOLV="$ROOT/etc/resolv.conf" HARPOON_CMDLINE="$TMP/cmdline" HARPOON_TMP="$ROOT/tmp" UDHCPC_CONF="$ROOT/etc/udhcpc/udhcpc.conf"
  # shellcheck disable=SC1091
  . "$TMP/dns-functions.sh"
}
resolv() { cat "$ROOT/etc/resolv.conf"; }
CONF="$ROOT/etc/udhcpc/udhcpc.conf"
DEAD_DNS=192.168.64.1
HOST_CMDLINE='console=hvc0 harpoon.dns=8.8.8.8,8.8.4.4'
HOST_RESOLV=$'nameserver 8.8.8.8\nnameserver 8.8.4.4'

# ---- 1. DHCP DNS works: it stays preferred and renewals keep following the server ----------------------------------------
( fresh; load "$HOST_CMDLINE"
  harpoon_dns_release
  dhcp bound 192.168.64.53
  harpoon_dns_select eth0 || fail "1: select failed with working DHCP DNS"
  cmp -s "$CONF" "$FIX/udhcpc.conf" || fail "1: udhcpc.conf was changed although DHCP DNS works"
  grep -q HARPOON_DNS_OK "$CONSOLE" && ! grep -q -E "OVERRIDE|HOLD" "$CONSOLE" || fail "1: console shows an override"
  [ "$(resolv)" = $'search lan\nnameserver 192.168.64.53' ] || fail "1: resolv.conf is not the DHCP one"
  dhcp renew 192.168.64.54
  [ "$(resolv)" = $'search lan\nnameserver 192.168.64.54' ] || fail "1: a renewal did not follow the DHCP server"
  for _ in 1 2 3; do dhcp renew 192.168.64.54; done
  [ "$(resolv)" = $'search lan\nnameserver 192.168.64.54' ] || fail "1: repeated renewals changed resolv.conf"
) || exit 1
echo "ok 1 working DHCP DNS is kept and followed"

# ---- 2. DHCP DNS unusable: the host resolvers are selected and kept through repeated renewals ------------------------------
( fresh; load "$HOST_CMDLINE"
  harpoon_dns_release
  dhcp bound "$DEAD_DNS"
  [ "$(grep -c nameserver "$ROOT/etc/resolv.conf")" = 1 ] && grep -q "$DEAD_DNS" "$ROOT/etc/resolv.conf" || fail "2: setup: DHCP did not install the dead DNS"
  harpoon_dns_select eth0 || fail "2: select failed although the host DNS works"
  [ "$(resolv)" = "$HOST_RESOLV" ] || fail "2: host resolvers not installed: $(resolv)"
  for m in HARPOON_DNS_DHCP_FAILED HARPOON_DNS_HOST HARPOON_DNS_OVERRIDE_APPLIED 'HARPOON_DNS_HOLD eth0' HARPOON_DNS_OK; do grep -q "$m" "$CONSOLE" || fail "2: console lacks $m"; done
  [ "$(grep -c '^# BEGIN harpoon-dns-hold' "$CONF")" = 1 ] && grep -q '^NO_DNS="\$NO_DNS eth0"$' "$CONF" || fail "2: no single hold block"
  for _ in 1 2 3 4 5; do dhcp renew "$DEAD_DNS"; done
  dhcp bound "$DEAD_DNS"; dhcp renew "$DEAD_DNS"
  [ "$(resolv)" = "$HOST_RESOLV" ] || fail "2: renewals replaced the selected resolver with: $(resolv)"
) || exit 1
echo "ok 2 fallback DNS survives repeated renewals"

# ---- 3. control: without the hold the real script does put the dead DNS back (the original defect) -----------------------
( fresh; load "$HOST_CMDLINE"
  harpoon_dns_hold() { :; }   # what init did before the fix
  dhcp bound "$DEAD_DNS"
  harpoon_dns_select eth0 || fail "3: select failed"
  [ "$(resolv)" = "$HOST_RESOLV" ] || fail "3: host resolvers not installed"
  dhcp renew "$DEAD_DNS"
  grep -q "$DEAD_DNS" "$ROOT/etc/resolv.conf" || fail "3: control did not reproduce the defect: the test cannot detect it"
) || exit 1
echo "ok 3 control: the unprotected renewal reproduces the defect"

# ---- 4. address, route and lease handling are identical with and without the hold ------------------------------------------
lifecycle() {  # lifecycle MODE: same event sequence; records the ip commands
  fresh; load "$HOST_CMDLINE"
  [ "$1" = off ] && harpoon_dns_hold() { :; }
  dhcp bound "$DEAD_DNS"; harpoon_dns_select eth0
  : > "$STATE/ip.log"
  dhcp renew "$DEAD_DNS"                        # healthy renewal
  rm -f "$STATE/addr"; dhcp renew "$DEAD_DNS"   # the address was lost: renewal must restore it
  [ -f "$STATE/addr" ] || fail "4($1): renewal did not restore the address"
  rm -f "$STATE/route"; dhcp renew "$DEAD_DNS"  # the default route was lost: renewal must restore it
  [ -f "$STATE/route" ] || fail "4($1): renewal did not restore the default route"
  dhcp deconfig "$DEAD_DNS"; [ ! -f "$STATE/addr" ] || fail "4($1): deconfig did not release the address"
  dhcp bound "$DEAD_DNS"; [ -f "$STATE/addr" ] && [ -f "$STATE/route" ] || fail "4($1): bound did not configure address and route"
  cp "$STATE/ip.log" "$TMP/ip.$1.log"
}
( lifecycle on ) || exit 1
( lifecycle off ) || exit 1
[ -s "$TMP/ip.on.log" ] || fail "4: no ip commands were recorded"
cmp -s "$TMP/ip.on.log" "$TMP/ip.off.log" || { diff "$TMP/ip.on.log" "$TMP/ip.off.log" >&2 || true; fail "4: the hold changed address/route/lease handling"; }
echo "ok 4 address, route and lease handling are unchanged ($(wc -l < "$TMP/ip.on.log" | tr -d ' ') ip commands identical)"

# ---- 5. the next boot starts from "DHCP DNS preferred" again ---------------------------------------------------------------
( fresh; load "$HOST_CMDLINE"
  printf '# local line before\n' >> "$CONF"
  dhcp bound "$DEAD_DNS"; harpoon_dns_select eth0
  printf '# local line after\n' >> "$CONF"
  harpoon_dns_hold eth0                             # twice: still one block
  [ "$(grep -c '^# BEGIN harpoon-dns-hold' "$CONF")" = 1 ] || fail "5: hold is not idempotent"
  grep -q '^# local line before$' "$CONF" && grep -q '^# local line after$' "$CONF" || fail "5: surrounding content lost"
  harpoon_dns_release                               # the next boot, before udhcpc starts
  ! grep -q 'harpoon-dns-hold\|NO_DNS="\$NO_DNS' "$CONF" || fail "5: release left the hold"
  grep -q '^# local line before$' "$CONF" && grep -q '^# local line after$' "$CONF" || fail "5: release removed other content"
  dhcp renew 192.168.64.53
  [ "$(resolv)" = $'search lan\nnameserver 192.168.64.53' ] || fail "5: after release a renewal does not follow DHCP again"
  harpoon_dns_release; harpoon_dns_release           # no block: no-op
  # a file without a final newline, and a missing file
  fresh; load "$HOST_CMDLINE"; printf 'X=1' > "$CONF"; harpoon_dns_hold eth0
  grep -qx 'X=1' "$CONF" && grep -qx '# BEGIN harpoon-dns-hold' "$CONF" || fail "5: hold glued onto the last line"
  rm -f "$CONF"; harpoon_dns_release; harpoon_dns_hold eth0; [ -f "$CONF" ] || fail "5: hold did not create udhcpc.conf"
  # a damaged block (begin without end) is left alone, not swallowed to the end of the file
  fresh; load "$HOST_CMDLINE"; printf '# BEGIN harpoon-dns-hold\nNO_DNS="x"\nUSER_SETTING=1\n' > "$CONF"; cp "$CONF" "$TMP/damaged"
  harpoon_dns_release && fail "5: damaged block was not reported"
  cmp -s "$CONF" "$TMP/damaged" || fail "5: damaged block was modified"
) || exit 1
echo "ok 5 release/hold are idempotent, preserve other content, and the next boot prefers DHCP DNS"

# ---- 6. the hold is per interface -------------------------------------------------------------------------------------------
( fresh; load "$HOST_CMDLINE"
  dhcp bound "$DEAD_DNS"; harpoon_dns_select eth0
  dhcp renew 192.168.64.99 eth1
  [ "$(resolv)" = $'search lan\nnameserver 192.168.64.99' ] || fail "6: eth0's hold blocked another interface"
) || exit 1
echo "ok 6 the hold applies to the held interface only"

# ---- 7. no host DNS on the kernel command line, and nothing resolving -------------------------------------------------------
( fresh; load 'console=hvc0'
  dhcp bound "$DEAD_DNS"
  # the public fallback must also not be the dead gateway
  harpoon_dns_select eth0 || fail "7: select failed"
  [ "$(resolv)" = $'nameserver 1.1.1.1\nnameserver 8.8.8.8' ] || fail "7: public fallback not installed"
  grep -q HARPOON_DNS_HOST_MISSING "$CONSOLE" || fail "7: console lacks HOST_MISSING"
  for _ in 1 2 3; do dhcp renew "$DEAD_DNS"; done
  [ "$(resolv)" = $'nameserver 1.1.1.1\nnameserver 8.8.8.8' ] || fail "7: renewal replaced the public fallback"
) || exit 1
( fresh; load "$HOST_CMDLINE"; export NSL_ALWAYS_FAIL=1
  dhcp bound "$DEAD_DNS"
  harpoon_dns_select eth0 && fail "7: select succeeded although nothing resolves"
  grep -q 'HARPOON_DNS_FAILED' "$CONSOLE" || fail "7: failure not reported"
) || exit 1
echo "ok 7 missing host DNS uses the public fallback (also held); total failure is reported"

# ---- 8. wiring in init -------------------------------------------------------------------------------------------------------
rel=$(grep -n '^harpoon_dns_release ||' "$INIT" | head -1 | cut -d: -f1)
dh=$(grep -n '^udhcpc -i "\$IFACE"' "$INIT" | head -1 | cut -d: -f1)
sel=$(grep -n 'harpoon_dns_select "\$IFACE"' "$INIT" | head -1 | cut -d: -f1)
[ -n "$rel" ] && [ -n "$dh" ] && [ -n "$sel" ] && [ "$rel" -lt "$dh" ] && [ "$dh" -lt "$sel" ] || fail "8: init does not release before udhcpc starts and select after"
[ "$(grep -c 'harpoon_dns_hold "\$1"' "$INIT")" = 1 ] || fail "8: the hold must be applied in exactly one place (the fallback)"
awk '/^harpoon_dns_select\(\)/,/^}/' "$INIT" | awk '/DNS_DHCP_FAILED/{seen=1} /harpoon_dns_hold "\$1"/{ if(!seen){print "before"; exit 1} }' || fail "8: hold is reachable without DHCP DNS having failed"
sh -n "$INIT" || fail "8: init has a syntax error"
echo "ok 8 init wiring"
echo "guest dns regression: PASS"
