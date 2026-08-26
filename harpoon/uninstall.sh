#!/bin/sh
set -eu
PREFIX="/usr/local"
PURGE=0
if [ "${1:-}" = "--purge" ]; then PURGE=1; fi
echo "[uninstall] checking Harpoon status..." >&2
if harpoon status 2>&1 | grep -q "running" 2>/dev/null; then
  echo "[uninstall] Harpoon is running. Run 'harpoon stop' first." >&2
  exit 1
fi
if [ ! -w "$PREFIX/bin" ] && [ -f "$PREFIX/bin/harpoon" ]; then SUDO="sudo"; else SUDO=""; fi
if [ -f "$PREFIX/bin/harpoon" ]; then
  $SUDO rm -f "$PREFIX/bin/harpoon"
  echo "[uninstall] removed $PREFIX/bin/harpoon" >&2
fi
if [ -d "$PREFIX/lib/harpoon" ]; then
  $SUDO rm -rf "$PREFIX/lib/harpoon"
  echo "[uninstall] removed $PREFIX/lib/harpoon" >&2
fi
# never remove user data by default
USER_DATA="$HOME/Library/Application Support/Harpoon"
if [ $PURGE -eq 1 ]; then
  echo "[uninstall] --purge: removing $USER_DATA" >&2
  # strict validation: only remove if path is exactly that
  case "$USER_DATA" in
    "$HOME/Library/Application Support/Harpoon") rm -rf "$USER_DATA" 2>/dev/null || true; echo "[uninstall] purged user data" >&2 ;;
    *) echo "[uninstall] refusing to purge unexpected path $USER_DATA" >&2; exit 1 ;;
  esac
  # also check fallback
  rm -rf /tmp/harpoon-runtime 2>/dev/null || true
else
  echo "[uninstall] preserved user data at $USER_DATA (use --purge to remove)" >&2
fi
# Docker context: only remove if owned
if command -v docker >/dev/null 2>&1; then
  if docker context inspect harpoon 2>&1 | grep -q "unix:///tmp/harpoon-docker.sock"; then
    echo "[uninstall] removing harpoon Docker context" >&2
    docker context rm harpoon 2>&1 | tail -n 3 || true
    # if current was harpoon, switch to default
    if [ "$(docker context show 2>&1 | tr -d '\n')" = "harpoon" ]; then docker context use default 2>&1 | tail -n 3 || true; fi
  else
    echo "[uninstall] harpoon context not owned, not removing" >&2
  fi
fi
echo "[uninstall] done" >&2
