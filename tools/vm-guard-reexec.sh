# Sourced FIRST by every script that boots a VM: re-runs the script under tools/vm-test-guard.py (deadline, resource supervision,
# owned-process cleanup) unless it already is. Override VMGUARD_DEADLINE (seconds) / VMGUARD_NAMESPACE (production|private) before sourcing.
# VMGUARD_DRYRUN=1 only reports that the script is guarded, without running it (used by the inventory test).
if [ -z "${HARPOON_VMGUARD_ACTIVE:-}" ]; then
  _vmg_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
  _vmg=
  for _c in "$_vmg_dir" "$_vmg_dir/../tools" "$_vmg_dir/../../tools"; do
    [ -f "$_c/vm-test-guard.py" ] && { _vmg="$_c/vm-test-guard.py"; break; }
  done
  [ -n "$_vmg" ] || { echo "vm-guard-reexec: tools/vm-test-guard.py not found from $_vmg_dir" >&2; exit 70; }
  if [ "${VMGUARD_DRYRUN:-}" = 1 ]; then echo "VMGUARD_WOULD_GUARD $0"; exit 0; fi
  if [ -n "${BASH_VERSION:-}" ]; then _vmg_sh=bash; else _vmg_sh=sh; fi
  exec python3 "$_vmg" --deadline "${VMGUARD_DEADLINE:-1800}" --namespace "${VMGUARD_NAMESPACE:-production}" -- "$_vmg_sh" "$0" "$@"
fi
