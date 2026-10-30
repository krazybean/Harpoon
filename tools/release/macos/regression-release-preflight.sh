#!/bin/bash
set -euo pipefail

# Safe release regression: exercise only the preflight branch with mocked
# macOS/credential commands; it never builds, signs, notarizes, or publishes.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
TMP="$(mktemp -d /tmp/harpoon-release-preflight.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin"

for command in swiftc codesign cargo npm docker xcrun spctl otool shasum stat; do
  cat > "$TMP/bin/$command" <<'EOF'
#!/bin/sh
exit 0
EOF
  chmod +x "$TMP/bin/$command"
done
cat > "$TMP/bin/security" <<'EOF'
#!/bin/sh
printf '  1) ABCD1234 "Developer ID Application: Test (TEAMID)"\n'
EOF
cat > "$TMP/bin/node" <<'EOF'
#!/bin/sh
printf '2.0.0-rc.1\n'
EOF
chmod +x "$TMP/bin/security" "$TMP/bin/node"
cat > "$TMP/bin/git" <<'EOF'
#!/bin/sh
case "$*" in
  *' status --porcelain'*) exit 0 ;;
  *' status --short'*) exit 0 ;;
  *) exec /usr/bin/git "$@" ;;
esac
EOF
chmod +x "$TMP/bin/git"

output=$(
  cd "$REPO_ROOT"
  PATH="$TMP/bin:$PATH" HARPOON_SIGN_IDENTITY='Developer ID Application: Test (TEAMID)' \
    bash "$SCRIPT_DIR/release.sh" --preflight 2>&1
)
grep -q 'PREFLIGHT PASS' <<<"$output"
echo 'release-preflight PASS'
