#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
VM="$ROOT/harpoon/Sources/VMManager.swift"
MAIN="$ROOT/harpoon/Sources/main.swift"
BRIDGES="$ROOT/harpoon/Sources/Bridges.swift"

grep -q 'VZVirtualMachineDelegate' "$VM"
grep -q 'guestDidStop' "$VM"
grep -q 'HARPOON_VM_STOP_CALLBACK' "$VM"
grep -q 'explicitStopInProgress' "$VM"
grep -q 'HARPOON_VM_UNEXPECTED_STOP' "$MAIN"
grep -q 'restart=disabled' "$MAIN"
grep -q 'guard !shuttingDown, !manager.unexpectedStopHandled' "$MAIN"
grep -q 'manager.markExplicitStop()' "$MAIN"
grep -q 'guard !explicitStopInProgress' "$VM"
grep -q 'HARPOON_SOCKET_REMOVE_SKIP' "$BRIDGES"
grep -q 'fstat(fd' "$BRIDGES"
printf '%s\n' 'VM lifecycle offline regression PASS'
