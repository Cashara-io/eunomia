#!/usr/bin/env bash
# Focused consumer connector checks. Shared-machine build guard, at most two hours per step.
set -euo pipefail
cd "$(dirname "$0")/../../backend"
export CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 RUST_MIN_STACK=8388608
cargo_step() {
  local waited=0 available
  while :; do
    df -h /
    available="$(df -k / | awk 'END {print $4}')"
    if ! pgrep -x cargo && [ "$available" -ge 7340032 ]; then break; fi
    if [ "$waited" -ge 7200 ]; then echo "Build guard: still busy or below 7 GiB after two hours."; return 86; fi
    echo "Build guard: waiting three minutes for Cargo and 7 GiB free."
    sleep 180
    waited=$((waited + 180))
  done
  nice -n 19 cargo "$@"
}
cargo_step test --lib composio
cargo_step test --test composio_flow --test composio_storage
for mutation in owner disconnect; do
  export COMPOSIO_GUARD_MUTATION="$mutation"
  log="$(mktemp)"
  if cargo_step test --test composio_storage > "$log" 2>&1; then cat "$log"; rm "$log"; echo "Guard survived: $mutation"; exit 1; else result=$?; fi
  cat "$log"
  expected="another user changed the credentials"
  [ "$mutation" = owner ] || expected="disconnect retained credentials"
  if [ "$result" -ne 101 ] || ! grep -q "$expected" "$log"; then rm "$log"; echo "Guard did not reach its expected assertion: $mutation"; exit 1; fi
  rm "$log"
  unset COMPOSIO_GUARD_MUTATION
  echo "Guard killed: $mutation"
done
