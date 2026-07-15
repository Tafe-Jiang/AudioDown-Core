#!/bin/sh
set -eu

plugin_id="com.audiodown.virtual.content"
gateway_image="audiodown/plugin-gateway:1.0.0-alpha.1"
export AUDIODOWN_DEV_MODE=1
export AUDIODOWN_DEV_TOKEN="${AUDIODOWN_DEV_TOKEN:-virtual-fixture-dev-token}"
owns_data_dir=0
other_container=""
other_network=""
plugin_only_container=""
malformed_network=""
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
    if [ -n "$other_container" ]; then
      docker rm -f "$other_container" >/dev/null 2>&1 || true
    fi
    if [ -n "$other_network" ]; then
      docker network rm "$other_network" >/dev/null 2>&1 || true
    fi
    if [ -n "$plugin_only_container" ]; then
      docker rm -f "$plugin_only_container" >/dev/null 2>&1 || true
    fi
    if [ -n "$malformed_network" ]; then
      docker network rm "$malformed_network" >/dev/null 2>&1 || true
    fi
    docker ps -aq \
      --filter "label=io.audiodown.managed=true" \
      --filter "label=io.audiodown.plugin-id=$plugin_id" \
      | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker network ls -q \
      --filter "label=io.audiodown.managed=true" \
      --filter "label=io.audiodown.plugin-id=$plugin_id" \
      | xargs -r docker network rm >/dev/null 2>&1 || true
    docker compose exec -T audiodown \
      chown -R "$(id -u):$(id -g)" /data >/dev/null 2>&1 || true
    docker compose down --remove-orphans >/dev/null 2>&1 || true
    if [ "$owns_data_dir" -eq 1 ]; then
      rm -rf "$AUDIODOWN_HOST_DATA_DIR"
    fi
  fi
}
trap cleanup EXIT INT TERM

compose_json="$(docker compose config --format json)"
node -e '
  const config = JSON.parse(process.argv[1]);
  const helper = config.services?.["plugin-gateway-image"];
  if (!helper) throw new Error("default Compose config must include the Gateway image builder");
  if (!helper.build) throw new Error("Gateway image helper must build repository code");
  if ((helper.ports ?? []).length !== 0) throw new Error("Gateway image helper must not publish ports");
  if ((helper.volumes ?? []).length !== 0) throw new Error("Gateway image helper must not mount sensitive data");
  const dependency = config.services?.supervisor?.depends_on?.["plugin-gateway-image"];
  if (dependency?.condition !== "service_completed_successfully") {
    throw new Error("Supervisor must wait for the fixed Gateway image build helper");
  }
' "$compose_json"
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
    --filter "label=io.audiodown.plugin-id=$plugin_id" \
    --filter "label=io.audiodown.resource=plugin"
)"
gateway_container="$(
  docker ps -q \
    --filter "label=io.audiodown.managed=true" \
    --filter "label=io.audiodown.plugin-id=$plugin_id" \
    --filter "label=io.audiodown.resource=gateway"
)"

[ -n "$core_container" ] || fail "Core container is missing"
[ -n "$supervisor_container" ] || fail "Supervisor container is missing"
[ -n "$plugin_container" ] || fail "virtual plugin container is missing"
[ -n "$gateway_container" ] || fail "fixed plugin Gateway container is missing"

[ "$(docker inspect --format '{{.Config.Image}}' "$gateway_container")" = "$gateway_image" ] ||
  fail "Gateway does not use the fixed repository image"

network_name="$(
  docker inspect "$plugin_container" --format '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{end}}'
)"
[ -n "$network_name" ] || fail "virtual plugin internal network is missing"
gateway_network="$(
  docker inspect "$gateway_container" --format '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{end}}'
)"
[ "$gateway_network" = "$network_name" ] || fail "plugin and Gateway do not share one network"
network_json="$(docker network inspect "$network_name")"
expected_proxy_volume="${AUDIODOWN_PROXY_VOLUME:-audiodown-proxy}"
installation_id="$(
  docker inspect "$plugin_container" \
    --format '{{index .Config.Labels "io.audiodown.installation"}}'
)"

docker inspect "$core_container" "$supervisor_container" "$plugin_container" "$gateway_container" |
  node -e '
    let input = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (chunk) => { input += chunk; });
    process.stdin.on("end", () => {
      const [core, supervisor, plugin, gateway] = JSON.parse(input);
      const networkName = process.argv[1];
      const expectedProxyVolume = process.argv[2];
      const [network] = JSON.parse(process.argv[3]);
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
      const hasPublishedPorts = (container) =>
        (container.HostConfig?.PortBindings &&
          Object.keys(container.HostConfig.PortBindings).length > 0) ||
        (container.NetworkSettings?.Ports &&
          Object.values(container.NetworkSettings.Ports).some(Boolean));
      const assertSandboxed = (container, label) => {
        if (hasPublishedPorts(container)) {
          fail(`${label} must not publish public ports`);
        }
        if (container.HostConfig?.Privileged !== false) {
          fail(`${label} must not run privileged`);
        }
        if (container.HostConfig?.PidMode === "host") {
          fail(`${label} must not use the host PID namespace`);
        }
        if (container.HostConfig?.ReadonlyRootfs !== true) {
          fail(`${label} root filesystem must be read-only`);
        }
        const capDrop = (container.HostConfig?.CapDrop ?? []).map((value) =>
          value.toUpperCase(),
        );
        if (!capDrop.includes("ALL")) {
          fail(`${label} must drop all Linux capabilities`);
        }
        if (!container.HostConfig?.SecurityOpt?.includes("no-new-privileges:true")) {
          fail(`${label} must enable no-new-privileges`);
        }
        if ((container.HostConfig?.Memory ?? 0) < 1 ||
            (container.HostConfig?.NanoCpus ?? 0) < 1 ||
            (container.HostConfig?.PidsLimit ?? 0) < 1) {
          fail(`${label} resource limits are missing`);
        }
        const runtimeNetworks = Object.keys(
          container.NetworkSettings?.Networks ?? {},
        );
        if (container.HostConfig?.NetworkMode !== networkName ||
            container.Config?.NetworkDisabled === true ||
            runtimeNetworks.length !== 1 || runtimeNetworks[0] !== networkName) {
          fail(`${label} must use only its derived internal network`);
        }
        if (hasDockerSocket(container)) {
          fail(`${label} must not mount the Docker Socket`);
        }
      };

      if (hasDockerSocket(core)) {
        fail("Core container must not mount the Docker Socket");
      }
      if (!hasDockerSocket(supervisor)) {
        fail("Supervisor container must mount the Docker Socket");
      }
      assertSandboxed(plugin, "virtual plugin");
      assertSandboxed(gateway, "fixed Gateway");
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
      const forbiddenPluginMount = (plugin.Mounts ?? []).find((mount) =>
        ["/data", "/downloads"].some(
          (path) =>
            mount.Destination === path ||
            mount.Destination?.startsWith(`${path}/`),
        ),
      );
      if (forbiddenPluginMount || (plugin.Mounts ?? []).length !== 0) {
        fail("virtual plugin must not mount Core data or downloads");
      }
      const proxyMount = (gateway.Mounts ?? []).find(
        (mount) => mount.Destination === "/run/audiodown-proxy",
      );
      if (!proxyMount || proxyMount.Name !== expectedProxyVolume || proxyMount.RW !== false ||
          (gateway.Mounts ?? []).length !== 1) {
        fail("Gateway alone must mount the configured proxy volume read-only");
      }
      if ((core.Mounts ?? []).filter(
        (mount) => mount.Destination === "/run/audiodown-proxy",
      ).length !== 1) {
        fail("Core must mount the dedicated proxy volume");
      }
      if ((supervisor.Mounts ?? []).some(
        (mount) => mount.Destination === "/run/audiodown-proxy",
      )) {
        fail("Supervisor must receive the proxy volume name only through configuration");
      }

      const pluginEnv = new Map((plugin.Config?.Env ?? []).map((entry) => {
        const index = entry.indexOf("=");
        return [entry.slice(0, index), entry.slice(index + 1)];
      }));
      if (pluginEnv.get("AUDIODOWN_PROXY_URL") !== "http://audiodown-gateway:18081") {
        fail("plugin must receive the fixed Gateway URL");
      }
      if (pluginEnv.has("AUDIODOWN_PROXY_TOKEN")) {
        fail("plugin proxy token must not persist in Docker Config.Env");
      }
      const gatewayEnv = gateway.Config?.Env ?? [];
      if (gatewayEnv.some((entry) =>
        entry.startsWith("AUDIODOWN_PROXY_TOKEN=") ||
        entry.startsWith("AUDIODOWN_CORE_TOKEN=") ||
        entry.startsWith("AUDIODOWN_DEV_TOKEN="))) {
        fail("Gateway must not receive proxy or Core control tokens");
      }

      if (network.Internal !== true || network.Attachable !== false) {
        fail("plugin network must be internal and non-attachable");
      }
      if (network.Labels?.["io.audiodown.managed"] !== "true" ||
          network.Labels?.["io.audiodown.plugin-id"] !== "com.audiodown.virtual.content" ||
          network.Labels?.["io.audiodown.resource"] !== "network") {
        fail("plugin network managed labels are missing");
      }
      const members = Object.keys(network.Containers ?? {}).sort();
      const expectedMembers = [plugin.Id, gateway.Id].sort();
      if (JSON.stringify(members) !== JSON.stringify(expectedMembers)) {
        fail("plugin network must contain only the plugin and its fixed Gateway");
      }
      const gatewayEndpoint = gateway.NetworkSettings?.Networks?.[networkName];
      if (!(gatewayEndpoint?.Aliases ?? []).includes("audiodown-gateway")) {
        fail("Gateway fixed network alias is missing");
      }
    });
  ' "$network_name" "$expected_proxy_volume" "$network_json"

if docker exec "$plugin_container" sh -c 'test -e /run/audiodown-secrets/proxy-token'; then
  fail "plugin one-time proxy token file was not removed"
fi
if ! docker exec "$plugin_container" sh -c '
  for environment in /proc/[0-9]*/environ; do
    tr "\0" "\n" <"$environment" 2>/dev/null || true
  done | grep -q "^AUDIODOWN_PROXY_TOKEN=."
'; then
  fail "plugin process did not receive the runtime proxy token"
fi

if docker exec "$plugin_container" sh -c 'test -r /data/audiodown.db'; then
  fail "virtual plugin can read the Core SQLite database"
fi
if docker exec "$plugin_container" sh -c 'test -e /var/run/docker.sock'; then
  fail "virtual plugin can inspect the Docker Socket"
fi
if ! docker exec "$plugin_container" node -e '
  const net = require("node:net");
  const socket = net.connect({ host: "audiodown-gateway", port: 18081 });
  const timer = setTimeout(() => process.exit(1), 1000);
  socket.once("error", () => process.exit(1));
  socket.once("connect", () => {
    clearTimeout(timer);
    socket.destroy();
    process.exit(0);
  });
'; then
  fail "virtual plugin cannot reach its fixed Gateway"
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
if ! docker exec "$plugin_container" node -e '
  const net = require("node:net");
  const socket = net.connect({ host: "172.17.0.1", port: 18080 });
  const blocked = () => {
    socket.destroy();
    process.exit(0);
  };
  socket.setTimeout(1000, blocked);
  socket.once("error", blocked);
  socket.once("connect", () => process.exit(1));
'; then
  fail "virtual plugin can connect to a private Docker endpoint"
fi
if ! docker exec "$plugin_container" node -e '
  const net = require("node:net");
  const socket = net.connect({ host: "audiodown", port: 18080 });
  const blocked = () => {
    socket.destroy();
    process.exit(0);
  };
  socket.setTimeout(1000, blocked);
  socket.once("error", blocked);
  socket.once("connect", () => process.exit(1));
'; then
  fail "virtual plugin can reach Core Web"
fi

other_network="audiodown-security-other-$$"
docker network create --internal "$other_network" >/dev/null
plugin_image="$(docker inspect --format '{{.Config.Image}}' "$plugin_container")"
other_container="$(
  docker run -d --rm \
    --network "$other_network" \
    --network-alias other-plugin \
    --entrypoint node \
    "$plugin_image" \
    -e 'require("node:net").createServer(() => {}).listen(19090); setInterval(() => {}, 1000)'
)"
if ! docker exec "$plugin_container" node -e '
  const net = require("node:net");
  const socket = net.connect({ host: "other-plugin", port: 19090 });
  const blocked = () => {
    socket.destroy();
    process.exit(0);
  };
  socket.setTimeout(1000, blocked);
  socket.once("error", blocked);
  socket.once("connect", () => process.exit(1));
'; then
  fail "virtual plugin can reach another plugin network"
fi

curl --fail --silent --request POST \
  "http://127.0.0.1:18080/api/v1/plugins/$plugin_id/stop" \
  >/dev/null
attempt=1
while [ "$attempt" -le 30 ]; do
  runtime_containers="$(
    docker ps -aq \
      --filter "label=io.audiodown.managed=true" \
      --filter "label=io.audiodown.plugin-id=$plugin_id"
  )"
  if [ -z "$runtime_containers" ] &&
    ! docker network inspect "$network_name" >/dev/null 2>&1; then
    break
  fi
  if [ "$attempt" -eq 30 ]; then
    fail "plugin stop did not remove the plugin, Gateway, and internal network"
  fi
  attempt=$((attempt + 1))
  sleep 1
done

curl --fail --silent --request POST \
  "http://127.0.0.1:18080/api/v1/plugins/$plugin_id/start" \
  >/dev/null
reconcile_network="$(
  plugin_container="$(
    docker ps -q \
      --filter "label=io.audiodown.managed=true" \
      --filter "label=io.audiodown.plugin-id=$plugin_id" \
      --filter "label=io.audiodown.resource=plugin"
  )"
  docker inspect "$plugin_container" \
    --format '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{end}}'
)"
[ -n "$reconcile_network" ] || fail "restart fixture internal network is missing"
docker restart "$supervisor_container" >/dev/null

attempt=1
while [ "$attempt" -le 60 ]; do
  system_json="$(curl --silent http://127.0.0.1:18080/api/v1/system || true)"
  runtime_containers="$(
    docker ps -aq \
      --filter "label=io.audiodown.managed=true" \
      --filter "label=io.audiodown.plugin-id=$plugin_id"
  )"
  if node -e '
    try {
      const system = JSON.parse(process.argv[1]);
      process.exit(system.supervisor?.available === true ? 0 : 1);
    } catch {
      process.exit(1);
    }
  ' "$system_json" && [ -z "$runtime_containers" ] &&
    ! docker network inspect "$reconcile_network" >/dev/null 2>&1; then
    break
  fi
  if [ "$attempt" -eq 60 ]; then
    fail "Supervisor startup reconcile did not remove stale runtime resources"
  fi
  attempt=$((attempt + 1))
  sleep 1
done

plugin_only_id="com.audiodown.virtual.orphan"
plugin_only_container="$(
  docker run -d \
    --network none \
    --label io.audiodown.managed=true \
    --label "io.audiodown.installation=$installation_id" \
    --label "io.audiodown.plugin-id=$plugin_only_id" \
    --label io.audiodown.resource=plugin \
    --entrypoint node \
    "$plugin_image" \
    -e 'setInterval(() => {}, 1000)'
)"
malformed_network="$network_name"
docker network create \
  --attachable \
  --label io.audiodown.managed=true \
  --label "io.audiodown.installation=$installation_id" \
  --label "io.audiodown.plugin-id=$plugin_id" \
  --label io.audiodown.resource=network \
  "$malformed_network" >/dev/null
docker network connect "$malformed_network" "$other_container"
docker restart "$supervisor_container" >/dev/null

attempt=1
while [ "$attempt" -le 30 ]; do
  if ! docker inspect "$plugin_only_container" >/dev/null 2>&1; then
    plugin_only_container=""
    break
  fi
  if [ "$attempt" -eq 30 ]; then
    fail "startup reconcile stopped after one owned resource cleanup failure"
  fi
  attempt=$((attempt + 1))
  sleep 1
done

docker network disconnect "$malformed_network" "$other_container"
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
  ' "$system_json" && ! docker network inspect "$malformed_network" >/dev/null 2>&1; then
    malformed_network=""
    break
  fi
  if [ "$attempt" -eq 60 ]; then
    fail "Supervisor did not aggregate, retry, and remove the unhealthy owned network"
  fi
  attempt=$((attempt + 1))
  sleep 1
done

printf '%s\n' "Security boundary assertions passed"
