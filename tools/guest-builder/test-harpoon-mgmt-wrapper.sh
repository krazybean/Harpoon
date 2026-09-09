#!/bin/sh
set -eu
WRAPPER="$(dirname "$0")/src/harpoon-mgmt-wrapper"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
run() { HARPOON_MGMT_MARKER_PATH="$TMP/markers" HARPOON_MGMT_TARGET="$1" "$WRAPPER" >/dev/null 2>&1 || true; }
check() { grep -q "$2" "$TMP/markers" && echo "wrapper $1 PASS" || { echo "wrapper $1 FAIL" >&2; exit 1; }; }

cat > "$TMP/ok" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$TMP/ok"
run "$TMP/ok"
check success WRAPPER_START
check success 'WRAPPER_TARGET exists=yes regular=yes executable=yes'
check success 'WRAPPER_INTERPRETER exists=yes executable=yes'

: > "$TMP/markers"
run "$TMP/missing"
check missing-target 'WRAPPER_TARGET exists=no'

cat > "$TMP/nonexec" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod 0644 "$TMP/nonexec"
: > "$TMP/markers"
run "$TMP/nonexec"
check non-executable 'WRAPPER_TARGET exists=yes regular=yes executable=no'

cat > "$TMP/nointerpreter" <<'EOF'
#!/no/such/interpreter
EOF
chmod +x "$TMP/nointerpreter"
: > "$TMP/markers"
run "$TMP/nointerpreter"
check missing-interpreter 'WRAPPER_INTERPRETER exists=no executable=no'
