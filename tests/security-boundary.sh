#!/bin/sh
set -eu

plugin_id="com.audiodown.virtual.content"
export AUDIODOWN_DEV_MODE=1
export AUDIODOWN_DEV_TOKEN="${AUDIODOWN_DEV_TOKEN:-virtual-fixture-dev-token}"
owns_data_dir=0
if [ -z "${AUDIODOWN_HOST_DATA_DIR:-}" ]; then
  AUDIODOWN_HOST_DATA_DIR="$(mktemp -d /tmp/audiodown-security-data.XXXXXX)"
  export AUDIODOWN_HOST_DATA_DIR
  owns_data_dir=1
fi

fail() {
  echo "SECURITY_BOUNDARY: $1" >&2
  exit 1
}

cleanup() {
  status=$?
  if [ "${AUDIODOWN_KEEP_CONTAINERS_ON_FAILURE:-0}" != "1" ] || [ "$status" -eq 0 ]; then
    docker ps -aq \
      --filter "label=io.audiodown.managed=true" \
      --filter "label=io.audiodown.plugin-id=$plugin_id" \
      | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker compose exec -T audiodown \
      chown -R "$(id -u):$(id -g)" /data >/dev/null 2>&1 || true
    docker compose down --remove-orphans >/dev/null 2>&1 || true
    if [ "$owns_data_dir" -eq 1 ]; then
      rm -rf "$AUDIODOWN_HOST_DATA_DIR"
    fi
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
    fail "Core and Supervisor did not become ready"
  fi
  attempt=$((attempt + 1))
  sleep 1
done

./scripts/install-virtual-plugin.sh
curl --fail --silent --request POST \
  "http://127.0.0.1:18080/api/v1/plugins/$plugin_id/start" \
  >/dev/null

core_container="$(docker compose ps -q audiodown)"
supervisor_container="$(docker compose ps -q supervisor)"
plugin_container="$(
  docker ps -q \
    --filter "label=io.audiodown.managed=true" \
    --filter "label=io.audiodown.plugin-id=$plugin_id"
)"

[ -n "$core_container" ] || fail "Core container is missing"
[ -n "$supervisor_container" ] || fail "Supervisor container is missing"
[ -n "$plugin_container" ] || fail "virtual plugin container is missing"

docker inspect "$core_container" "$supervisor_container" "$plugin_container" |
  node -e '
    let input = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (chunk) => { input += chunk; });
    process.stdin.on("end", () => {
      const [core, supervisor, plugin] = JSON.parse(input);
      const fail = (message) => {
        console.error(`SECURITY_BOUNDARY: ${message}`);
        process.exit(1);
      };
      const hasDockerSocket = (container) =>
        (container.Mounts ?? []).some(
          (mount) =>
            mount.Source === "/var/run/docker.sock" ||
            mount.Destination === "/var/run/docker.sock",
        );

      if (hasDockerSocket(core)) {
        fail("Core container must not mount the Docker Socket");
      }
      if (!hasDockerSocket(supervisor)) {
        fail("Supervisor container must mount the Docker Socket");
      }
      if ((plugin.HostConfig?.PortBindings &&
          Object.keys(plugin.HostConfig.PortBindings).length > 0) ||
          (plugin.NetworkSettings?.Ports &&
          Object.values(plugin.NetworkSettings.Ports).some(Boolean))) {
        fail("virtual plugin must not publish public ports");
      }
      if (plugin.HostConfig?.Privileged !== false) {
        fail("virtual plugin must not run privileged");
      }
      if (plugin.HostConfig?.ReadonlyRootfs !== true) {
        fail("virtual plugin root filesystem must be read-only");
      }
      const capDrop = (plugin.HostConfig?.CapDrop ?? []).map((value) =>
        value.toUpperCase(),
      );
      if (!capDrop.includes("ALL")) {
        fail("virtual plugin must drop all Linux capabilities");
      }
      if (!plugin.HostConfig?.SecurityOpt?.includes("no-new-privileges:true")) {
        fail("virtual plugin must enable no-new-privileges");
      }
      if ((plugin.HostConfig?.Memory ?? 0) < 134217728) {
        fail("virtual plugin memory limit is missing");
      }
      if ((plugin.HostConfig?.NanoCpus ?? 0) < 500000000) {
        fail("virtual plugin CPU limit is missing");
      }
      if ((plugin.HostConfig?.PidsLimit ?? 0) < 1 ||
          (plugin.HostConfig?.PidsLimit ?? 0) > 64) {
        fail("virtual plugin PID limit is missing");
      }
      if (plugin.HostConfig?.NetworkMode !== "none" ||
          plugin.Config?.NetworkDisabled !== true) {
        fail("virtual plugin must use disabled networking");
      }
      if (hasDockerSocket(plugin)) {
        fail("virtual plugin must not mount the Docker Socket");
      }
      const forbiddenMount = (plugin.Mounts ?? []).find((mount) =>
        ["/data", "/downloads"].some(
          (path) =>
            mount.Destination === path ||
            mount.Destination?.startsWith(`${path}/`),
        ),
      );
      if (forbiddenMount) {
        fail("virtual plugin must not mount Core data or downloads");
      }
    });
  '

if docker exec "$plugin_container" sh -c 'test -r /data/audiodown.db'; then
  fail "virtual plugin can read the Core SQLite database"
fi
if docker exec "$plugin_container" sh -c 'test -e /var/run/docker.sock'; then
  fail "virtual plugin can inspect the Docker Socket"
fi
if ! docker exec "$plugin_container" node -e '
  require("node:dns").lookup("example.com", (error) => {
    process.exit(error ? 0 : 1);
  });
'; then
  fail "virtual plugin can resolve a public test endpoint"
fi
if ! docker exec "$plugin_container" node -e '
  const net = require("node:net");
  const socket = net.connect({ host: "192.0.2.1", port: 80 });
  const blocked = () => {
    socket.destroy();
    process.exit(0);
  };
  socket.setTimeout(1000, blocked);
  socket.once("error", blocked);
  socket.once("connect", () => process.exit(1));
'; then
  fail "virtual plugin can connect to a public test endpoint"
fi

printf '%s\n' "Security boundary assertions passed"
