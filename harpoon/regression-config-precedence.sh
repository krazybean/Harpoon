#!/bin/sh
# cpus and memory resolve as: compiled default < config file < environment < CLI flag. No VM is started: every case either
# reads `status --json` of a stopped engine or is a `start` that is made to stop in argument validation (the kernel is a
# missing file, so a start that got past the cpus and memory checks fails with "kernel not found", never boots). Everything
# runs in a private HARPOON_TEST_TMPDIR namespace, never against a serving engine. Usage: sh harpoon/regression-config-precedence.sh [harpoon-binary]
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
bin=${1:-${HARPOON_BIN:-$ROOT/harpoon/build/harpoon}}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HARPOON_TEST_TMPDIR="$tmp"
export HARPOON_KERNEL="$tmp/no-such-kernel"
unset HARPOON_CPUS HARPOON_MEMORY_MIB
fail() { echo "config precedence FAIL: $*" >&2; exit 1; }

field() { "$bin" status --json | plutil -extract "$1" raw -; }
expect() { [ "$2" = "$3" ] || fail "$1: expected $3, got $2"; }
config() { printf '%s\n' "$1" > "$tmp/config.json"; }
# a `start` that must stop at validation; prints its output
start() { set +e; out=$("$bin" start "$@" 2>&1); code=$?; set -e; [ "$code" = 2 ] || fail "start $*: exit $code, expected 2 ($out)"; printf '%s' "$out"; }
rejects() { case "$1" in *"$2"*) ;; *) fail "$3: expected '$2' in: $1" ;; esac; }
passes_limits() { case "$1" in *"must be"*) fail "$2: rejected a value that should have been replaced: $1" ;; *"kernel not found"*) ;; *) fail "$2: unexpected output: $1" ;; esac; }

# resolved values, from a stopped engine
rm -f "$tmp/config.json"
expect "default cpus" "$(field cpus)" 2
expect "default memory" "$(field memoryMiB)" 4096
config '{"cpus":3,"memory":2048}'
expect "config cpus" "$(field cpus)" 3
expect "config memory" "$(field memoryMiB)" 2048
expect "environment beats config: cpus" "$(HARPOON_CPUS=4 field cpus)" 4
expect "environment beats config: memory" "$(HARPOON_MEMORY_MIB=3072 field memoryMiB)" 3072

# the CLI layer, through which of two conflicting values the validation complains about
config '{"cpus":2,"memory":2048}'
rejects "$(HARPOON_MEMORY_MIB=100 start)" "got 100" "an invalid environment value must win over a valid config value"
config '{"cpus":2,"memory":100}'
passes_limits "$(HARPOON_MEMORY_MIB=2048 start)" "a valid environment value must win over an invalid config value"
config '{"cpus":2,"memory":2048}'
rejects "$(HARPOON_MEMORY_MIB=2048 start --memory 100)" "got 100" "an invalid CLI value must win over a valid environment value"
passes_limits "$(HARPOON_MEMORY_MIB=100 start --memory 1024)" "a valid CLI value must win over an invalid environment value"
rejects "$(HARPOON_CPUS=2 start --cpus 99)" "got 99" "an invalid CLI cpus value must win over a valid environment value"
passes_limits "$(HARPOON_CPUS=99 start --cpus 2)" "a valid CLI cpus value must win over an invalid environment value"
config '{"cpus":99,"memory":2048}'
passes_limits "$(HARPOON_CPUS=2 start)" "a valid environment cpus value must win over an invalid config value"

# nothing was started
expect "state after the cases" "$(field state)" stopped

# the help, the usage line and the guide say the same order
old1='CLI flag > user config > environment'
old2='CLI > config > environment'
for f in harpoon/Sources/HarpoonCLI.swift harpoon/Sources/main.swift docs/configuration.md; do
  ! grep -q -e "$old1" -e "$old2" "$ROOT/$f" || fail "$f still states the old order"
done
grep -q 'CLI flag > environment (HARPOON_CPUS, HARPOON_MEMORY_MIB) > user config > defaults' "$ROOT/harpoon/Sources/HarpoonCLI.swift" || fail "help does not state the order"
grep -q 'precedence: CLI > environment > config > defaults' "$ROOT/harpoon/Sources/main.swift" || fail "usage does not state the order"
grep -q 'CLI flag > environment > user config > compiled default' "$ROOT/docs/configuration.md" || fail "the configuration guide does not state the order"
"$bin" help 2>&1 | grep -q 'Precedence (cpus, memory): CLI flag > environment' || fail "the built binary's help does not state the order"
echo "config precedence regression PASS"
