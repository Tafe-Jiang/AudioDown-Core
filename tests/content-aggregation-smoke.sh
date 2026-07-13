#!/bin/sh
set -eu

root_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root_dir"

commit_sha="0123456789abcdef0123456789abcdef01234567"
repository_url="https://github.com/example-owner/example-repository"
content_plugin_id="com.audiodown.virtual.content"
backup_plugin_id="com.audiodown.virtual.content-backup"
catalog_plugin_id="com.audiodown.catalog.content"
fixture_root="$root_dir/test-fixtures/repositories/virtual"
compose_project="audiodown-phase3-$$"
mock_name="$compose_project-github-mock"
host_port=$((41000 + ($$ % 10000)))
temporary_dir="$(mktemp -d /tmp/audiodown-phase3-smoke.XXXXXX)"
AUDIODOWN_HOST_DATA_DIR="$temporary_dir/data"
export AUDIODOWN_HOST_DATA_DIR
export AUDIODOWN_DEV_MODE=1
export AUDIODOWN_DEV_TOKEN="phase-three-fixture-token-$$"
export AUDIODOWN_GITHUB_API_BASE="http://github-mock:18082"
export AUDIODOWN_GITHUB_ARCHIVE_BASE="http://github-mock:18082"
export AUDIODOWN_PORT="$host_port"

fail() {
  printf '%s\n' "CONTENT_AGGREGATION_SMOKE: $*" >&2
  exit 1
}

compose() {
  docker compose -p "$compose_project" "$@"
}

cleanup() {
  status=$?
  if [ "${AUDIODOWN_KEEP_CONTAINERS_ON_FAILURE:-0}" != "1" ] ||
    [ "$status" -eq 0 ]; then
    docker rm -f "$mock_name" >/dev/null 2>&1 || true
    for plugin_id in \
      "$content_plugin_id" \
      "$backup_plugin_id" \
      "$catalog_plugin_id"; do
      docker ps -aq \
        --filter "label=io.audiodown.managed=true" \
        --filter "label=io.audiodown.plugin-id=$plugin_id" |
        xargs -r docker rm -f >/dev/null 2>&1 || true
      docker images -q \
        --filter "label=io.audiodown.plugin-id=$plugin_id" |
        xargs -r docker image rm -f >/dev/null 2>&1 || true
    done
    compose exec -T audiodown \
      chown -R "$(id -u):$(id -g)" /data >/dev/null 2>&1 || true
    compose down --remove-orphans --volumes >/dev/null 2>&1 || true
    rm -rf "$temporary_dir"
  else
    printf '%s\n' \
      "CONTENT_AGGREGATION_SMOKE: retained project $compose_project for diagnostics" >&2
  fi
}
trap cleanup EXIT INT TERM

wait_for_core() {
  attempt=1
  while [ "$attempt" -le 90 ]; do
    system_json="$(
      curl --silent "http://127.0.0.1:$host_port/api/v1/system" || true
    )"
    if node -e '
      try {
        const system = JSON.parse(process.argv[1]);
        process.exit(system.supervisor?.available === true ? 0 : 1);
      } catch {
        process.exit(1);
      }
    ' "$system_json"; then
      return
    fi
    [ "$attempt" -lt 90 ] ||
      fail "Core and Supervisor did not become ready"
    attempt=$((attempt + 1))
    sleep 1
  done
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v docker >/dev/null 2>&1 || fail "Docker is required"
command -v node >/dev/null 2>&1 || fail "Node.js is required"

mkdir -p "$AUDIODOWN_HOST_DATA_DIR"

./tests/virtual-content-contract.sh

docker run --rm \
  -v "$fixture_root:/source/virtual:ro" \
  -v "$temporary_dir:/output" \
  node:22-bookworm-slim \
  sh -lc '
    set -eu
    tar --sort=name --mtime="@0" --owner=0 --group=0 --numeric-owner \
      -C /source -cf - virtual |
      gzip -n > /output/repository.tar.gz
    chmod 0644 /output/repository.tar.gz
  '

compose up -d --build

docker run -d --rm \
  --name "$mock_name" \
  --network "${compose_project}_default" \
  --network-alias github-mock \
  -e AUDIODOWN_FIXTURE_ARCHIVE=/fixture/repository.tar.gz \
  -v "$temporary_dir/repository.tar.gz:/fixture/repository.tar.gz:ro" \
  -v "$root_dir/test-fixtures/github-mock/server.js:/fixture/server.js:ro" \
  node:22-bookworm-slim \
  node /fixture/server.js >/dev/null

attempt=1
while [ "$attempt" -le 30 ]; do
  if docker exec "$mock_name" node -e \
    "fetch('http://127.0.0.1:18082/repos/example-owner/example-repository').then(r => { if (!r.ok) process.exit(1) })" \
    >/dev/null 2>&1; then
    break
  fi
  [ "$attempt" -lt 30 ] ||
    fail "mock GitHub server did not become ready"
  attempt=$((attempt + 1))
  sleep 1
done

wait_for_core

inspection_file="$temporary_dir/inspection.json"
curl --fail --silent --show-error \
  --request POST \
  --header "content-type: application/json" \
  --data "{\"url\":\"$repository_url\"}" \
  "http://127.0.0.1:$host_port/api/v1/plugin-repositories/inspect" \
  >"$inspection_file"

snapshot_id="$(
  node - "$inspection_file" \
    "$commit_sha" \
    "$content_plugin_id" \
    "$backup_plugin_id" \
    "$catalog_plugin_id" <<'NODE'
const fs = require("node:fs");
const [
  file,
  commitSha,
  contentPluginId,
  backupPluginId,
  catalogPluginId,
] = process.argv.slice(2);
const inspection = JSON.parse(fs.readFileSync(file, "utf8"));
const expectedIds = [contentPluginId, backupPluginId, catalogPluginId];
if (
  inspection.repository.commitSha !== commitSha ||
  inspection.repository.sourceUrl !==
    "https://github.com/example-owner/example-repository" ||
  !expectedIds.every((pluginId) =>
    inspection.plugins.some(
      (plugin) =>
        plugin.pluginId === pluginId &&
        plugin.pluginType === "content" &&
        plugin.requiresLifecycleScriptGrant === false,
    ),
  )
) {
  throw new Error("content repository inspection response is invalid");
}
process.stdout.write(inspection.snapshotId);
NODE
)"

for plugin_id in \
  "$content_plugin_id" \
  "$backup_plugin_id" \
  "$catalog_plugin_id"; do
  install_file="$temporary_dir/install-$plugin_id.json"
  curl --fail --silent --show-error \
    --request POST \
    --header "content-type: application/json" \
    --data '{"allowLifecycleScripts":false}' \
    "http://127.0.0.1:$host_port/api/v1/plugin-repositories/$snapshot_id/plugins/$plugin_id/install" \
    >"$install_file"
  node - "$install_file" "$plugin_id" "$commit_sha" <<'NODE'
const fs = require("node:fs");
const [file, pluginId, commitSha] = process.argv.slice(2);
const installed = JSON.parse(fs.readFileSync(file, "utf8"));
if (
  installed.pluginId !== pluginId ||
  installed.status !== "installed" ||
  installed.commitSha !== commitSha
) {
  throw new Error(`content plugin installation failed: ${pluginId}`);
}
NODE
done

plugins_json="$(
  curl --fail --silent "http://127.0.0.1:$host_port/api/v1/plugins"
)"
node - "$plugins_json" \
  "$content_plugin_id" \
  "$backup_plugin_id" \
  "$catalog_plugin_id" <<'NODE'
const plugins = JSON.parse(process.argv[2]);
const expectedIds = process.argv.slice(3);
const methods = [
  "content.search",
  "content.discover",
  "content.categories",
  "content.album.get",
  "content.tracks.list",
];
for (const pluginId of expectedIds) {
  const plugin = plugins.items.find((candidate) => candidate.pluginId === pluginId);
  if (
    !plugin ||
    plugin.pluginType !== "content" ||
    plugin.enabled !== true ||
    !methods.every((method) => plugin.capabilities.includes(method))
  ) {
    throw new Error(`installed content plugin is incomplete: ${pluginId}`);
  }
}
NODE

docker run --rm --ipc=host \
  --network "${compose_project}_default" \
  -e AUDIODOWN_LIVE_BASE_URL="http://audiodown:18080" \
  -v "$root_dir/web:/app" \
  -w /app \
  mcr.microsoft.com/playwright:v1.61.1-noble \
  sh -lc \
  'npm ci && npx playwright test tests/content-aggregation-live.spec.ts'

for plugin_id in \
  "$content_plugin_id" \
  "$backup_plugin_id" \
  "$catalog_plugin_id"; do
  if docker ps -q \
    --filter "label=io.audiodown.managed=true" \
    --filter "label=io.audiodown.plugin-id=$plugin_id" |
    grep -q .; then
    fail "live content test left plugin running: $plugin_id"
  fi
done

health_json="$(curl --fail --silent "http://127.0.0.1:$host_port/healthz")"
logs_json="$(
  curl --fail --silent "http://127.0.0.1:$host_port/api/v1/logs?limit=200"
)"
node - "$health_json" "$logs_json" <<'NODE'
const health = JSON.parse(process.argv[2]);
const logs = JSON.parse(process.argv[3]);
const methods = new Set(
  logs.items
    .filter((item) => item.component === "content-api")
    .map((item) => item.context?.method)
    .filter(Boolean),
);
for (const method of [
  "content.search",
  "content.discover",
  "content.categories",
  "content.album.get",
  "content.tracks.list",
]) {
  if (!methods.has(method)) {
    throw new Error(`missing structured content log: ${method}`);
  }
}
if (
  health.ok !== true ||
  health.service !== "audiodown-core" ||
  JSON.stringify(logs).includes("raw-plugin-secret")
) {
  throw new Error("content smoke health or redaction assertion failed");
}
NODE

printf '%s\n' \
  "Content install -> aggregation -> fallback -> album pagination -> logs -> stop passed"
