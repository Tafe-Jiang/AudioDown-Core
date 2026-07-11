#!/bin/sh
set -eu

plugin_id="com.audiodown.virtual.content"
export AUDIODOWN_DEV_MODE=1
export AUDIODOWN_DEV_TOKEN="${AUDIODOWN_DEV_TOKEN:-virtual-fixture-dev-token}"
owns_data_dir=0
if [ -z "${AUDIODOWN_HOST_DATA_DIR:-}" ]; then
  AUDIODOWN_HOST_DATA_DIR="$(mktemp -d /tmp/audiodown-virtual-data.XXXXXX)"
  export AUDIODOWN_HOST_DATA_DIR
  owns_data_dir=1
fi

cleanup() {
  status=$?
  if [ "${AUDIODOWN_KEEP_CONTAINERS_ON_FAILURE:-0}" != "1" ] || [ "$status" -eq 0 ]; then
    docker ps -aq \
      --filter "label=io.audiodown.managed=true" \
      --filter "label=io.audiodown.plugin-id=$plugin_id" \
      | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker compose down --remove-orphans >/dev/null 2>&1 || true
  fi
  if [ "$owns_data_dir" -eq 1 ]; then
    rm -rf "$AUDIODOWN_HOST_DATA_DIR"
  fi
}
trap cleanup EXIT INT TERM

docker compose up -d --build

attempt=1
while [ "$attempt" -le 60 ]; do
  system_json="$(curl --silent http://127.0.0.1:18080/api/v1/system || true)"
  if node -e '
    try {
      const system = JSON.parse(process.argv[1]);
      process.exit(system.supervisor?.available === true ? 0 : 1);
    } catch {
      process.exit(1);
    }
  ' "$system_json"; then
    break
  fi
  if [ "$attempt" -eq 60 ]; then
    docker compose logs
    echo "Core and Supervisor did not become ready" >&2
    exit 1
  fi
  attempt=$((attempt + 1))
  sleep 1
done

./scripts/install-virtual-plugin.sh

plugins_json="$(curl --fail --silent http://127.0.0.1:18080/api/v1/plugins)"
node - "$plugins_json" "$plugin_id" <<'NODE'
const plugins = JSON.parse(process.argv[2]);
const pluginId = process.argv[3];
const plugin = plugins.items.find((item) => item.pluginId === pluginId);
if (!plugin || plugin.status !== "installed") {
  throw new Error("virtual plugin was not registered as installed");
}
NODE

start_json="$(curl --fail --silent -X POST \
  "http://127.0.0.1:18080/api/v1/plugins/$plugin_id/start")"
node - "$start_json" "$plugin_id" <<'NODE'
const state = JSON.parse(process.argv[2]);
if (state.pluginId !== process.argv[3] || state.status !== "healthy") {
  throw new Error(`unexpected start state: ${JSON.stringify(state)}`);
}
NODE

inspect_json="$(curl --fail --silent \
  "http://127.0.0.1:18080/api/v1/plugins/$plugin_id/runtime")"
node - "$inspect_json" "$plugin_id" <<'NODE'
const state = JSON.parse(process.argv[2]);
if (state.pluginId !== process.argv[3] || state.status !== "healthy") {
  throw new Error(`unexpected inspect state: ${JSON.stringify(state)}`);
}
NODE

logs_json="$(curl --fail --silent \
  "http://127.0.0.1:18080/api/v1/logs?pluginId=$plugin_id")"
node - "$logs_json" "$plugin_id" <<'NODE'
const logs = JSON.parse(process.argv[2]);
const pluginId = process.argv[3];
if (!logs.items.some((entry) => entry.pluginId === pluginId)) {
  throw new Error("virtual plugin log attribution is missing");
}
NODE

stop_json="$(curl --fail --silent -X POST \
  "http://127.0.0.1:18080/api/v1/plugins/$plugin_id/stop")"
node - "$stop_json" "$plugin_id" <<'NODE'
const state = JSON.parse(process.argv[2]);
if (state.pluginId !== process.argv[3] || state.status !== "stopped") {
  throw new Error(`unexpected stop state: ${JSON.stringify(state)}`);
}
NODE

printf '%s\n' "installed -> starting -> healthy -> stopped"
