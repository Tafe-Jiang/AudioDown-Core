#!/bin/sh
set -eu

root_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root_dir"

commit_sha="0123456789abcdef0123456789abcdef01234567"
repository_url="https://github.com/example-owner/example-repository"
content_plugin_id="com.audiodown.virtual.content"
risk_plugin_id="com.audiodown.virtual.build-risk"
fixture_root="$root_dir/test-fixtures/repositories/virtual"
fixture_dev_token="phase-two-fixture-token-$$"
compose_project="audiodown-phase2-$$"
mock_name="$compose_project-github-mock"
host_port=$((20000 + ($$ % 10000)))
temporary_dir="$(mktemp -d /tmp/audiodown-phase2-smoke.XXXXXX)"
AUDIODOWN_HOST_DATA_DIR="$temporary_dir/data"
export AUDIODOWN_HOST_DATA_DIR
export AUDIODOWN_DEV_MODE=1
export AUDIODOWN_DEV_TOKEN="$fixture_dev_token"
export AUDIODOWN_GITHUB_API_BASE="http://github-mock:18082"
export AUDIODOWN_GITHUB_ARCHIVE_BASE="http://github-mock:18082"
export AUDIODOWN_PORT="$host_port"

compose() {
  docker compose -p "$compose_project" "$@"
}

wait_for_core() {
  attempt=1
  while [ "$attempt" -le 90 ]; do
    system_json="$(curl --silent "http://127.0.0.1:$host_port/api/v1/system" || true)"
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
    [ "$attempt" -lt 90 ] || fail "Core and Supervisor did not become ready"
    attempt=$((attempt + 1))
    sleep 1
  done
}

fail() {
  printf '%s\n' "PLUGIN_REPOSITORY_SMOKE: $*" >&2
  exit 1
}

cleanup() {
  status=$?
  if [ "${AUDIODOWN_KEEP_CONTAINERS_ON_FAILURE:-0}" != "1" ] || [ "$status" -eq 0 ]; then
    docker rm -f "$mock_name" >/dev/null 2>&1 || true
    for plugin_id in "$content_plugin_id" "$risk_plugin_id"; do
      docker ps -aq \
        --filter "label=io.audiodown.managed=true" \
        --filter "label=io.audiodown.plugin-id=$plugin_id" \
        | xargs -r docker rm -f >/dev/null 2>&1 || true
    done
    compose exec -T audiodown \
      chown -R "$(id -u):$(id -g)" /data >/dev/null 2>&1 || true
    compose down --remove-orphans --volumes >/dev/null 2>&1 || true
    rm -rf "$temporary_dir"
  else
    printf '%s\n' "PLUGIN_REPOSITORY_SMOKE: retained project $compose_project for diagnostics" >&2
  fi
}
trap cleanup EXIT INT TERM

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v node >/dev/null 2>&1 || fail "Node.js is required"
command -v sqlite3 >/dev/null 2>&1 || fail "sqlite3 is required"

mkdir -p "$AUDIODOWN_HOST_DATA_DIR"

node - "$fixture_root" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");

const root = process.argv[2];
const repository = JSON.parse(
  fs.readFileSync(path.join(root, "audiodown-repository.json"), "utf8"),
);
if (
  repository.schemaVersion !== "1.0" ||
  repository.repository.id !== "example.plugins"
) {
  throw new Error("invalid repository index fixture");
}

const expected = new Map([
  ["plugins/virtual-content", {
    id: "com.audiodown.virtual.content",
    lifecycle: false,
  }],
  ["plugins/virtual-build-risk", {
    id: "com.audiodown.virtual.build-risk",
    lifecycle: true,
  }],
]);
if (repository.plugins.length !== expected.size) {
  throw new Error("repository fixture must contain exactly two plugins");
}

for (const pluginReference of repository.plugins) {
  const expectation = expected.get(pluginReference.path);
  if (!expectation) {
    throw new Error(`unexpected plugin path: ${pluginReference.path}`);
  }
  const pluginRoot = path.join(root, pluginReference.path);
  const manifest = JSON.parse(
    fs.readFileSync(path.join(pluginRoot, "audiodown-plugin.json"), "utf8"),
  );
  const packageJson = JSON.parse(
    fs.readFileSync(path.join(pluginRoot, "package.json"), "utf8"),
  );
  const lockfile = JSON.parse(
    fs.readFileSync(path.join(pluginRoot, "package-lock.json"), "utf8"),
  );
  if (
    manifest.id !== expectation.id ||
    manifest.runtime.type !== "nodejs" ||
    manifest.runtime.version !== "22" ||
    manifest.runtime.entry !== "src/index.js" ||
    manifest.network.allowedHosts.length !== 0 ||
    manifest.build.npmLifecycleScripts.required !== expectation.lifecycle
  ) {
    throw new Error(`invalid manifest fixture: ${manifest.id}`);
  }
  if (
    packageJson.name !== lockfile.name ||
    packageJson.version !== lockfile.version ||
    lockfile.lockfileVersion !== 3 ||
    lockfile.packages[""].name !== packageJson.name ||
    lockfile.packages[""].version !== packageJson.version ||
    typeof packageJson.scripts?.build !== "string"
  ) {
    throw new Error(`invalid package fixture: ${manifest.id}`);
  }
  const hasPreinstall = typeof packageJson.scripts?.preinstall === "string";
  if (hasPreinstall !== expectation.lifecycle) {
    throw new Error(`lifecycle declaration mismatch: ${manifest.id}`);
  }
}
NODE

for plugin_path in virtual-content virtual-build-risk; do
  docker run --rm \
    -v "$fixture_root/plugins/$plugin_path:/fixture:ro" \
    node:22-bookworm-slim \
    sh -lc '
      set -eu
      cp -a /fixture /tmp/plugin
      cd /tmp/plugin
      npm ci --omit=dev --ignore-scripts --no-audit --no-fund
      test ! -e build-risk-marker.txt
      npm run build
    '
done

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

archive_hash="$(shasum -a 256 "$temporary_dir/repository.tar.gz" | awk '{print $1}')"
[ "${#archive_hash}" -eq 64 ] || fail "fixture archive hash is invalid"
tar -tzf "$temporary_dir/repository.tar.gz" |
  grep -F 'virtual/audiodown-repository.json' >/dev/null ||
  fail "fixture archive does not contain the repository index"

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
  [ "$attempt" -lt 30 ] || fail "mock GitHub server did not become ready"
  attempt=$((attempt + 1))
  sleep 1
done

docker exec "$mock_name" node --input-type=module - "$archive_hash" <<'NODE'
const crypto = require("node:crypto");

const expectedHash = process.argv[2];
const base = "http://127.0.0.1:18082";
const repository = await fetch(
  `${base}/repos/example-owner/example-repository`,
).then((response) => response.json());
const commit = await fetch(
  `${base}/repos/example-owner/example-repository/commits/main`,
).then((response) => response.json());
const archive = Buffer.from(
  await fetch(
    `${base}/example-owner/example-repository/tar.gz/${commit.sha}`,
  ).then((response) => response.arrayBuffer()),
);
const rejected = await fetch(`${base}/unexpected`);

if (
  repository.default_branch !== "main" ||
  commit.sha !== "0123456789abcdef0123456789abcdef01234567" ||
  crypto.createHash("sha256").update(archive).digest("hex") !== expectedHash ||
  rejected.status !== 404
) {
  throw new Error("mock GitHub route self-check failed");
}
NODE

wait_for_core

inspection_file="$temporary_dir/inspection.json"
curl --fail --silent --show-error \
  --request POST \
  --header "content-type: application/json" \
  --data "{\"url\":\"$repository_url\"}" \
  "http://127.0.0.1:$host_port/api/v1/plugin-repositories/inspect" \
  >"$inspection_file"

snapshot_id="$(
  node - "$inspection_file" "$commit_sha" "$content_plugin_id" "$risk_plugin_id" <<'NODE'
const fs = require("node:fs");
const [file, commitSha, contentPluginId, riskPluginId] = process.argv.slice(2);
const inspection = JSON.parse(fs.readFileSync(file, "utf8"));
if (
  inspection.repository.id !== "example.plugins" ||
  inspection.repository.commitSha !== commitSha ||
  inspection.repository.sourceUrl !==
    "https://github.com/example-owner/example-repository" ||
  !inspection.plugins.some(
    (plugin) =>
      plugin.pluginId === contentPluginId &&
      plugin.requiresLifecycleScriptGrant === false,
  ) ||
  !inspection.plugins.some(
    (plugin) =>
      plugin.pluginId === riskPluginId &&
      plugin.requiresLifecycleScriptGrant === true,
  )
) {
  throw new Error("unexpected repository inspection response");
}
process.stdout.write(inspection.snapshotId);
NODE
)"

install_file="$temporary_dir/content-install.json"
curl --fail --silent --show-error \
  --request POST \
  --header "content-type: application/json" \
  --data '{"allowLifecycleScripts":false}' \
  "http://127.0.0.1:$host_port/api/v1/plugin-repositories/$snapshot_id/plugins/$content_plugin_id/install" \
  >"$install_file"

node - "$install_file" "$content_plugin_id" "$commit_sha" <<'NODE'
const fs = require("node:fs");
const [file, pluginId, commitSha] = process.argv.slice(2);
const installed = JSON.parse(fs.readFileSync(file, "utf8"));
if (
  installed.pluginId !== pluginId ||
  installed.status !== "installed" ||
  installed.commitSha !== commitSha
) {
  throw new Error("content plugin installation response is invalid");
}
NODE

database="$AUDIODOWN_HOST_DATA_DIR/audiodown.db"
attempt=1
while [ "$attempt" -le 30 ] && [ ! -f "$database" ]; do
  attempt=$((attempt + 1))
  sleep 1
done
[ -f "$database" ] || fail "SQLite database was not created"

compose stop audiodown >/dev/null
record="$(
  sqlite3 -noheader "$database" "
    SELECT source_ref, commit_sha, image_id, status
    FROM plugins
    WHERE plugin_id = '$content_plugin_id';
  "
)"
compose start audiodown >/dev/null
wait_for_core
source_ref="$(printf '%s' "$record" | cut -d'|' -f1)"
stored_commit="$(printf '%s' "$record" | cut -d'|' -f2)"
content_image_id="$(printf '%s' "$record" | cut -d'|' -f3)"
stored_status="$(printf '%s' "$record" | cut -d'|' -f4)"
[ "$source_ref" = "$repository_url" ] || fail "SQLite source URL is incorrect"
[ "$stored_commit" = "$commit_sha" ] || fail "SQLite commit SHA is incorrect"
[ -n "$content_image_id" ] || fail "SQLite image ID is missing"
[ "$stored_status" = "installed" ] || fail "SQLite plugin state is not installed"
docker image inspect "$content_image_id" >/dev/null ||
  fail "managed plugin image does not exist"

start_file="$temporary_dir/content-start.json"
curl --fail --silent --show-error --request POST \
  "http://127.0.0.1:$host_port/api/v1/plugins/$content_plugin_id/start" \
  >"$start_file"
node - "$start_file" "$content_plugin_id" <<'NODE'
const fs = require("node:fs");
const [file, pluginId] = process.argv.slice(2);
const state = JSON.parse(fs.readFileSync(file, "utf8"));
if (state.pluginId !== pluginId || state.status !== "healthy") {
  throw new Error("content plugin did not complete its handshake");
}
NODE

curl --fail --silent --show-error \
  --request PATCH \
  --header "content-type: application/json" \
  --data '{"enabled":true,"runMode":"always","priority":321}' \
  "http://127.0.0.1:$host_port/api/v1/plugins/$content_plugin_id" \
  >/dev/null
curl --fail --silent --show-error \
  --request PATCH \
  --header "content-type: application/json" \
  --data '{"enabled":true,"runMode":"on_demand","priority":123}' \
  "http://127.0.0.1:$host_port/api/v1/plugins/$content_plugin_id" \
  >/dev/null

curl --fail --silent --show-error --request POST \
  "http://127.0.0.1:$host_port/api/v1/plugins/$content_plugin_id/stop" \
  >/dev/null
curl --fail --silent --show-error --request DELETE \
  "http://127.0.0.1:$host_port/api/v1/plugins/$content_plugin_id" \
  >/dev/null

[ ! -e "$AUDIODOWN_HOST_DATA_DIR/plugins/installed/$content_plugin_id" ] ||
  fail "content plugin install directory remains after uninstall"
plugins_json="$(curl --fail --silent "http://127.0.0.1:$host_port/api/v1/plugins")"
node - "$plugins_json" "$content_plugin_id" <<'NODE'
const plugins = JSON.parse(process.argv[2]);
const pluginId = process.argv[3];
if (plugins.items.some((plugin) => plugin.pluginId === pluginId)) {
  throw new Error("content plugin API row remains after uninstall");
}
NODE
if docker image inspect "$content_image_id" >/dev/null 2>&1; then
  fail "content plugin image remains after uninstall"
fi
if docker ps -aq \
  --filter "label=io.audiodown.plugin-id=$content_plugin_id" |
  grep -q .; then
  fail "content plugin container remains after uninstall"
fi

docker run --rm --ipc=host \
  --network "${compose_project}_default" \
  -e AUDIODOWN_LIVE_BASE_URL="http://audiodown:18080" \
  -e AUDIODOWN_LIVE_DEV_TOKEN="$fixture_dev_token" \
  -v "$root_dir/web:/app" \
  -w /app \
  mcr.microsoft.com/playwright:v1.61.1-noble \
  sh -lc 'npm ci && npx playwright test tests/plugin-installation-live.spec.ts'

compose stop audiodown >/dev/null
plugin_count="$(sqlite3 -noheader "$database" 'SELECT COUNT(*) FROM plugins;')"
compose start audiodown >/dev/null
wait_for_core
[ "$plugin_count" = "0" ] || fail "live UI cleanup left plugin rows"
[ ! -e "$AUDIODOWN_HOST_DATA_DIR/plugins/installed/$risk_plugin_id" ] ||
  fail "live UI cleanup left the risk plugin directory"
if docker images -q \
  --filter "label=io.audiodown.plugin-id=$risk_plugin_id" |
  grep -q .; then
  fail "live UI cleanup left the risk plugin image"
fi
if docker ps -aq \
  --filter "label=io.audiodown.plugin-id=$risk_plugin_id" |
  grep -q .; then
  fail "live UI cleanup left the risk plugin container"
fi

health_json="$(curl --fail --silent "http://127.0.0.1:$host_port/healthz")"
system_json="$(curl --fail --silent "http://127.0.0.1:$host_port/api/v1/system")"
node - "$health_json" "$system_json" <<'NODE'
const health = JSON.parse(process.argv[2]);
const system = JSON.parse(process.argv[3]);
if (
  health.ok !== true ||
  health.service !== "audiodown-core" ||
  system.supervisor?.available !== true ||
  system.pluginCount !== 0
) {
  throw new Error("Core or Supervisor is unhealthy after repository smoke");
}
NODE

printf '%s\n' "Repository install -> handshake -> settings -> uninstall -> live UI cleanup passed"
