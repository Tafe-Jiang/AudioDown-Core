#!/bin/sh
set -eu

root_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root_dir"

plugin_ids="
com.audiodown.virtual.content
com.audiodown.virtual.build-risk
"
verify_id="audiodown-verify-$$"
rust_registry_volume="$verify_id-registry"
rust_target_volume="$verify_id-target"
AUDIODOWN_HOST_DATA_DIR="$(mktemp -d /tmp/audiodown-verify-data.XXXXXX)"
export AUDIODOWN_HOST_DATA_DIR

cleanup() {
  for plugin_id in $plugin_ids; do
    docker ps -aq \
      --filter "label=io.audiodown.managed=true" \
      --filter "label=io.audiodown.plugin-id=$plugin_id" \
      | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker images -q \
      --filter "label=io.audiodown.plugin-id=$plugin_id" \
      | xargs -r docker image rm -f >/dev/null 2>&1 || true
  done
  docker ps -aq \
    --filter "label=io.audiodown.resource-role=plugin-build" \
    | xargs -r docker rm -f >/dev/null 2>&1 || true
  docker ps -aq \
    --filter "label=io.audiodown.resource-role=plugin-build-proxy" \
    | xargs -r docker rm -f >/dev/null 2>&1 || true
  docker compose exec -T audiodown \
    chown -R "$(id -u):$(id -g)" /data >/dev/null 2>&1 || true
  docker compose down --remove-orphans >/dev/null 2>&1 || true
  docker volume rm "$rust_registry_volume" "$rust_target_volume" >/dev/null 2>&1 || true
  rm -rf "$AUDIODOWN_HOST_DATA_DIR"
}
trap cleanup EXIT INT TERM

run_cargo() {
  docker volume create "$rust_registry_volume" >/dev/null
  docker volume create "$rust_target_volume" >/dev/null
  docker run --rm \
    -e CARGO_TARGET_DIR=/target \
    -v "$root_dir:/workspace:ro" \
    -v "$rust_registry_volume:/usr/local/cargo/registry" \
    -v "$rust_target_volume:/target" \
    -w /workspace \
    rust:1.88-bookworm \
    cargo "$@"
}

# The server embeds web/dist at compile time, so a clean checkout needs this
# prerequisite before Rust compiles. The ordered Vue checks still run below.
printf '%s\n' "Preparing embedded Vue assets"
(
  cd web
  npm ci
  npm run build
)

printf '%s\n' "Checking Rust formatting"
run_cargo fmt --all -- --check

printf '%s\n' "Running Rust workspace tests"
run_cargo test --locked --workspace

printf '%s\n' "Running Rust clippy"
run_cargo clippy --locked --workspace --all-targets -- -D warnings

printf '%s\n' "Running Node SDK tests"
(
  cd plugin-sdk/node
  npm ci
  npm test
)

printf '%s\n' "Running Vue tests"
(
  cd web
  npm test -- --run
)

printf '%s\n' "Running Vue typecheck"
(
  cd web
  npm run typecheck
)

printf '%s\n' "Building Vue production assets"
(
  cd web
  npm run build
)

printf '%s\n' "Running MCP UI Playwright accessibility, responsive, and visual tests"
docker run --rm --ipc=host \
  -v "$root_dir/web:/app" \
  -w /app \
  mcr.microsoft.com/playwright:v1.61.1-noble \
  sh -lc 'npm ci && npx playwright test'

printf '%s\n' "Validating Compose configuration"
docker compose config --quiet

printf '%s\n' "Running Compose smoke test"
./tests/compose-smoke.sh

printf '%s\n' "Running virtual plugin smoke test"
./tests/virtual-plugin-smoke.sh

printf '%s\n' "Running security boundary checks"
./tests/security-boundary.sh

printf '%s\n' "Running repository installation smoke including tests/plugin-installation-live.spec.ts"
./tests/plugin-repository-smoke.sh

printf '%s\n' "Running plugin installation security checks"
./tests/plugin-installation-security.sh

printf '%s\n' "AudioDown phase-two verification passed"
