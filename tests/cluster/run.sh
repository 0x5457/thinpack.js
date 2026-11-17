#!/usr/bin/env bash
# Runs the ported Node.js cluster tests against the Thinpack runtime.
#
# Usage: tests/cluster/run.sh [test-file...]
# The runtime defaults to target-core/debug/thinpack-rt (an unpacked build); set
# THINPACK_RT to another build. Tests that need handle passing, dgram, unix socket
# sharing, advanced serialization or Node's private cluster internals are not ported.
set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
rt=${THINPACK_RT:-$root/target-core/debug/thinpack-rt}

if [ ! -x "$rt" ]; then
  echo "runtime not found: $rt (set THINPACK_RT)" >&2
  exit 2
fi

if [ "$#" -gt 0 ]; then
  tests=()
  for test in "$@"; do
    case "$test" in
      */*) tests+=("$test") ;;
      *) tests+=("$here/parallel/$test") ;;
    esac
  done
else
  tests=("$here"/parallel/test-cluster-*.js)
fi

failed=0
passed=0
for test in "${tests[@]}"; do
  name=$(basename "$test")
  output=$(cd "$root" && timeout 60 "$rt" "$test" 2>&1)
  status=$?
  if [ "$status" -eq 0 ]; then
    passed=$((passed + 1))
    printf 'ok   %s\n' "$name"
  else
    failed=$((failed + 1))
    printf 'FAIL %s (exit %d)\n' "$name" "$status"
    printf '%s\n' "$output" | sed 's/^/     /' | head -40
  fi
done

printf '\n%d passed, %d failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ]
