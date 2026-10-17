#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
TMP="$(mktemp -d /tmp/harpoon-docker-diagnostics.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

SELECTED="$TMP/selected/docker"
STALE="$TMP/stale/docker"
DECOY="$TMP/decoy/docker"
mkdir -p "$(dirname "$SELECTED")" "$(dirname "$STALE")" "$(dirname "$DECOY")" "$TMP/fixture-docker"

cat > "$STALE" <<'EOF'
this is not a runnable Docker executable
EOF

cat > "$SELECTED" <<'EOF'
#!/bin/sh
case " $* " in
  *" --version "*) echo 'Docker version 28.3.3'; exit 0 ;;
  *" compose version "*) echo 'Docker Compose version v5.1.0'; exit 0 ;;
  *" context inspect harpoon "*) printf '%s\n' '[{"Endpoints":{"docker":{"Host":"unix:///tmp/harpoon-docker.sock"}}}]'; exit 0 ;;
  *" version "*) echo 'Server: Docker Engine - 28.3.3'; exit 0 ;;
esac
exit 1
EOF
cat > "$DECOY" <<'EOF'
#!/bin/sh
echo 'decoy Docker CLI must not be selected' >&2
exit 1
EOF
chmod +x "$STALE" "$SELECTED" "$DECOY"

HARPOON_TEST_TMPDIR="$TMP/state" HOME="$TMP" DOCKER_CONFIG="$TMP/fixture-docker" \
  PATH="$TMP/stale:$TMP/selected:$TMP/decoy:/usr/bin:/bin" \
  "$ROOT/harpoon/build/harpoon" doctor > "$TMP/doctor.out" 2>&1 || true

grep -q "Docker CLI .* $SELECTED" "$TMP/doctor.out"
grep -q "Docker Compose plugin .* PASS Docker Compose version v5.1.0" "$TMP/doctor.out"
grep -q "Harpoon context .* PASS unix:///tmp/harpoon-docker.sock" "$TMP/doctor.out"
! grep -q "decoy Docker CLI" "$TMP/doctor.out"
echo "docker diagnostics fixture PASS"
