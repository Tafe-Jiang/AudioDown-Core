#!/bin/sh
set -eu

compose_file="${COMPOSE_FILE:-docker-compose.yml}"
config_file="$(mktemp /tmp/audiodown-compose-config.XXXXXX)"

cleanup() {
  docker compose -f "$compose_file" down --remove-orphans >/dev/null 2>&1 || true
  rm -f "$config_file"
}
trap cleanup EXIT INT TERM

docker compose -f "$compose_file" config --format json >"$config_file"

node - "$config_file" <<'NODE'
const fs = require("fs");
const config = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const names = Object.keys(config.services).sort();
if (JSON.stringify(names) !== JSON.stringify(["audiodown", "supervisor"])) {
  throw new Error(`expected exactly audiodown and supervisor services, got ${names}`);
}
for (const [name, service] of Object.entries(config.services)) {
  const volumes = service.volumes ?? [];
  const hasDockerSocket = volumes.some((volume) =>
    String(volume.source ?? "").includes("docker.sock"),
  );
  if (name === "supervisor" && !hasDockerSocket) {
    throw new Error("Supervisor must mount the Docker Socket");
  }
  if (name !== "supervisor" && hasDockerSocket) {
    throw new Error(`${name} must not mount the Docker Socket`);
  }
}
NODE

docker compose -f "$compose_file" build
docker compose -f "$compose_file" up -d

attempt=1
while [ "$attempt" -le 60 ]; do
  if curl --fail --silent http://127.0.0.1:18080/healthz >/dev/null; then
    break
  fi
  if [ "$attempt" -eq 60 ]; then
    docker compose -f "$compose_file" logs
    echo "Core health check did not become ready" >&2
    exit 1
  fi
  attempt=$((attempt + 1))
  sleep 1
done

system_json="$(curl --fail --silent http://127.0.0.1:18080/api/v1/system)"
plugins_json="$(curl --fail --silent http://127.0.0.1:18080/api/v1/plugins)"

node - "$system_json" "$plugins_json" <<'NODE'
const system = JSON.parse(process.argv[2]);
const plugins = JSON.parse(process.argv[3]);
if (system.supervisor?.available !== true) {
  throw new Error("Core did not report Supervisor as available");
}
if (!Array.isArray(plugins.items) || plugins.items.length !== 0) {
  throw new Error("initial plugin list must be empty");
}
NODE
