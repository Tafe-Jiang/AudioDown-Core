#!/bin/sh
set -eu

root_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root_dir"

fail() {
  printf '%s\n' "CONTENT_AGGREGATION_SECURITY: $*" >&2
  exit 1
}

run_check() {
  message="$1"
  shift
  if ! "$@"; then
    fail "$message"
  fi
}

command -v cargo >/dev/null 2>&1 ||
  fail "Cargo is required for focused content security tests"
command -v docker >/dev/null 2>&1 ||
  fail "Docker is required for runtime boundary checks"
command -v node >/dev/null 2>&1 ||
  fail "Node.js is required for SDK security tests"

run_check \
  "content contracts accepted unknown methods or oversized opaque values" \
  cargo test -p audiodown-plugin-api --test content_contracts
run_check \
  "Supervisor protocol accepted arbitrary plugin RPC fields or oversized responses" \
  cargo test -p audiodown-supervisor-protocol --test contracts
run_check \
  "Supervisor content execution lost trusted-container, response, or method checks" \
  cargo test -p audiodown-supervisor --test content_rpc
run_check \
  "Core cursor validation accepted malformed, oversized, or request-mismatched cursors" \
  cargo test -p audiodown-content --test cursor
run_check \
  "content routing bypassed explicit filters, defaults, or source pinning" \
  cargo test -p audiodown-content --test routing
run_check \
  "Core API exposed raw failures or lost filter and partial-result behavior" \
  cargo test -p audiodown-server --test content_api
run_check \
  "active content calls could race idle plugin shutdown" \
  cargo test -p audiodown-plugin-manager --test content_invocation \
  active_call_prevents_idle_stop_until_the_lease_is_released -- --exact
run_check \
  "plugin runtime policy permits forbidden networking or mounts" \
  cargo test -p audiodown-supervisor --test policy

run_check \
  "Node SDK accepted non-allowlisted content handlers" \
  sh -c 'cd plugin-sdk/node && npm test'

run_check \
  "live plugin container escaped the runtime security boundary" \
  ./tests/security-boundary.sh

printf '%s\n' "Content aggregation security assertions passed"
