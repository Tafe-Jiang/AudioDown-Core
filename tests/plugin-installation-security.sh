#!/bin/sh
set -eu

root_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root_dir"

commit_sha="0123456789abcdef0123456789abcdef01234567"
repository_url="https://github.com/example-owner/example-repository"
risk_plugin_id="com.audiodown.virtual.build-risk"
fixture_root="$root_dir/test-fixtures/repositories/virtual"
fixture_dev_token="phase-two-security-token-$$"
compose_project="audiodown-phase2-security-$$"
mock_name="$compose_project-github-mock"
host_port=$((30000 + ($$ % 10000)))
temporary_dir="$(mktemp -d /tmp/audiodown-phase2-security.XXXXXX)"
AUDIODOWN_HOST_DATA_DIR="$temporary_dir/data"
export AUDIODOWN_HOST_DATA_DIR
export AUDIODOWN_DEV_MODE=1
export AUDIODOWN_DEV_TOKEN="$fixture_dev_token"
export AUDIODOWN_GITHUB_API_BASE="http://github-mock:18082"
export AUDIODOWN_GITHUB_ARCHIVE_BASE="http://github-mock:18082"
export AUDIODOWN_PORT="$host_port"

builder_container=""
proxy_container=""
operation_id=""
install_pid=""

fail() {
  printf '%s\n' "PLUGIN_INSTALL_SECURITY: $*" >&2
  exit 1
}

compose() {
  docker compose -p "$compose_project" "$@"
}

remove_fixture_resources() {
  docker ps -aq \
    --filter "label=io.audiodown.plugin-id=$risk_plugin_id" |
    xargs -r docker rm -f >/dev/null 2>&1 || true
  docker images -q \
    --filter "label=io.audiodown.plugin-id=$risk_plugin_id" |
    xargs -r docker image rm -f >/dev/null 2>&1 || true
}

run_security_test() {
  message="$1"
  shift
  if ! "$@"; then
    fail "$message"
  fi
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
    [ "$attempt" -lt 90 ] ||
      fail "Core and Supervisor did not become ready for security assertions"
    attempt=$((attempt + 1))
    sleep 1
  done
}

snapshot_database() {
  snapshot_dir="$1"
  rm -rf "$snapshot_dir"
  mkdir -p "$snapshot_dir"
  docker cp "$core_container:/data/." - |
    tar -xf - -C "$snapshot_dir" ||
    fail "could not snapshot the stopped Core database for host assertions"
  [ -f "$snapshot_dir/audiodown.db" ] ||
    fail "Core database snapshot is missing"
}

cleanup() {
  status=$?
  if [ -n "$install_pid" ]; then
    kill "$install_pid" >/dev/null 2>&1 || true
    wait "$install_pid" >/dev/null 2>&1 || true
  fi
  if [ "${AUDIODOWN_KEEP_CONTAINERS_ON_FAILURE:-0}" != "1" ] ||
    [ "$status" -eq 0 ]; then
    docker rm -f "$mock_name" >/dev/null 2>&1 || true
    if [ -n "$operation_id" ]; then
      docker ps -aq \
        --filter "label=io.audiodown.operation-id=$operation_id" |
        xargs -r docker rm -f >/dev/null 2>&1 || true
      docker network rm \
        "audiodown-build-$operation_id-internal" \
        "audiodown-build-$operation_id-egress" >/dev/null 2>&1 || true
    fi
    remove_fixture_resources
    compose exec -T audiodown \
      chown -R "$(id -u):$(id -g)" /data >/dev/null 2>&1 || true
    compose down --remove-orphans --volumes >/dev/null 2>&1 || true
    rm -rf "$temporary_dir"
  else
    printf '%s\n' \
      "PLUGIN_INSTALL_SECURITY: retained project $compose_project for diagnostics" >&2
  fi
}
trap cleanup EXIT INT TERM

command -v cargo >/dev/null 2>&1 ||
  fail "Cargo is required for focused security tests"
command -v curl >/dev/null 2>&1 ||
  fail "curl is required for Compose security tests"
command -v docker >/dev/null 2>&1 ||
  fail "Docker is required for Compose security tests"
command -v node >/dev/null 2>&1 ||
  fail "Node.js is required for fixture and response validation"
command -v sqlite3 >/dev/null 2>&1 ||
  fail "sqlite3 is required for install side-effect assertions"

remove_fixture_resources

run_security_test \
  "GitHub credential-bearing and non-public repository inputs were not rejected" \
  cargo test -p audiodown-plugin-manager --test github_source \
  rejects_tokens_subpaths_queries_fragments_and_other_hosts -- --exact
run_security_test \
  "repository inspection accepted token or unknown private-repository fields" \
  cargo test -p audiodown-server --test repository_api \
  rejects_unknown_fields_and_urls_over_five_hundred_twelve_bytes -- --exact
run_security_test \
  "non-success or redirected GitHub repository responses were not rejected" \
  cargo test -p audiodown-plugin-manager --test github_client \
  rejects_redirects_and_non_success_responses -- --exact

run_security_test \
  "repository archive traversal paths were not rejected" \
  cargo test -p audiodown-plugin-manager --test archive_safety \
  rejects_parent_and_absolute_paths -- --exact
run_security_test \
  "repository archive links or device entries were not rejected" \
  cargo test -p audiodown-plugin-manager --test archive_safety \
  rejects_links_and_device_entries -- --exact
run_security_test \
  "repository archive duplicate paths were not rejected" \
  cargo test -p audiodown-plugin-manager --test archive_safety \
  rejects_duplicate_normalized_paths -- --exact
run_security_test \
  "repository archive oversized entries were not rejected" \
  cargo test -p audiodown-plugin-manager --test archive_safety \
  rejects_files_larger_than_four_mebibytes -- --exact
run_security_test \
  "repository archive oversized extraction was not rejected" \
  cargo test -p audiodown-plugin-manager --test archive_safety \
  rejects_more_than_sixty_four_mebibytes_extracted -- --exact

run_security_test \
  "malicious build output traversal, links, or duplicate entries were not rejected" \
  cargo test -p audiodown-supervisor --test docker_build \
  downloaded_output_rejects_traversal_hardlinks_and_duplicates -- --exact
run_security_test \
  "malicious build output links, devices, FIFOs, duplicates, or oversize entries were not rejected" \
  cargo test -p audiodown-supervisor --test build_policy \
  build_output_rejects_unsafe_tar_entries -- --exact
run_security_test \
  "build container or proxy policy lost its fixed network and resource restrictions" \
  cargo test -p audiodown-supervisor --test build_policy \
  builder_and_proxy_have_fixed_network_identity_and_resource_limits -- --exact
run_security_test \
  "plugin runtime policy lost its phase-one restrictions" \
  cargo test -p audiodown-supervisor --test policy \
  generated_container_spec_enforces_security_invariants -- --exact
run_security_test \
  "lifecycle scripts could run without an explicit matching grant" \
  cargo test -p audiodown-supervisor --test build_policy \
  npm_command_requires_an_explicit_validated_lifecycle_grant -- --exact
run_security_test \
  "failed builds did not roll back SQLite state and acknowledge cleanup" \
  cargo test -p audiodown-plugin-manager --test install_service \
  rejects_mismatched_artifacts_and_persists_failed_build_logs -- --exact
run_security_test \
  "failed installation rollback left operation-owned prepared files, images, or directories" \
  cargo test -p audiodown-supervisor --test install_operation \
  abort_removes_all_operation_owned_resources -- --exact
run_security_test \
  "Core restart did not reconcile built and finalized install operations" \
  cargo test -p audiodown-plugin-manager --test install_service \
  reconciles_built_and_finalized_operations_after_restart -- --exact
run_security_test \
  "install operations were acknowledged before SQLite and Supervisor state agreed" \
  cargo test -p audiodown-plugin-manager --test install_service \
  never_completes_a_finalized_operation_with_mismatched_artifacts -- --exact
run_security_test \
  "terminal operations did not retain the required acknowledgement semantics" \
  cargo test -p audiodown-supervisor --test install_operation \
  terminal_records_require_ack_and_remain_queryable_for_thirty_minutes -- --exact
run_security_test \
  "uninstall accepted mismatched installation or managed-image labels" \
  cargo test -p audiodown-supervisor --test remove_policy \
  rejects_installation_identity_and_image_label_mismatches -- --exact

mkdir -p "$AUDIODOWN_HOST_DATA_DIR"

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

tar -tzf "$temporary_dir/repository.tar.gz" |
  grep -F 'virtual/audiodown-repository.json' >/dev/null ||
  fail "virtual repository fixture archive is incomplete"

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
    fail "mock GitHub server did not become ready for security assertions"
  attempt=$((attempt + 1))
  sleep 1
done

wait_for_core

core_container="$(compose ps -q audiodown)"
supervisor_container="$(compose ps -q supervisor)"
[ -n "$core_container" ] ||
  fail "Core container is missing during Docker Socket assertion"
[ -n "$supervisor_container" ] ||
  fail "Supervisor container is missing during Docker Socket assertion"

docker inspect "$core_container" "$supervisor_container" |
  node -e '
    let input = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (chunk) => { input += chunk; });
    process.stdin.on("end", () => {
      const [core, supervisor] = JSON.parse(input);
      const hasSocket = (container) =>
        (container.Mounts ?? []).some(
          (mount) =>
            mount.Source === "/var/run/docker.sock" ||
            mount.Destination === "/var/run/docker.sock",
        );
      if (hasSocket(core)) {
        console.error(
          "PLUGIN_INSTALL_SECURITY: Core still mounts the Docker Socket",
        );
        process.exit(1);
      }
      if (!hasSocket(supervisor)) {
        console.error(
          "PLUGIN_INSTALL_SECURITY: Supervisor does not exclusively own the Docker Socket",
        );
        process.exit(1);
      }
    });
  '

inspection_file="$temporary_dir/inspection.json"
curl --fail --silent --show-error \
  --request POST \
  --header "content-type: application/json" \
  --data "{\"url\":\"$repository_url\"}" \
  "http://127.0.0.1:$host_port/api/v1/plugin-repositories/inspect" \
  >"$inspection_file" ||
  fail "virtual public repository inspection failed"

snapshot_id="$(
  node - "$inspection_file" "$commit_sha" "$risk_plugin_id" <<'NODE'
const fs = require("node:fs");
const [file, commitSha, pluginId] = process.argv.slice(2);
const inspection = JSON.parse(fs.readFileSync(file, "utf8"));
if (
  inspection.repository.commitSha !== commitSha ||
  !inspection.plugins.some(
    (plugin) =>
      plugin.pluginId === pluginId &&
      plugin.requiresLifecycleScriptGrant === true,
  )
) {
  process.exit(1);
}
process.stdout.write(inspection.snapshotId);
NODE
)" || fail "virtual build-risk plugin was not present in the locked snapshot"

unknown_status="$(
  curl --silent --show-error \
    --output "$temporary_dir/unknown-install.json" \
    --write-out '%{http_code}' \
    --request POST \
    --header "content-type: application/json" \
    --data '{"allowLifecycleScripts":false,"dockerfile":"forbidden","networkMode":"host"}' \
    "http://127.0.0.1:$host_port/api/v1/plugin-repositories/$snapshot_id/plugins/$risk_plugin_id/install"
)" || fail "Docker-field rejection request could not reach Core"
[ "$unknown_status" = "422" ] ||
  fail "caller-controlled Docker fields were accepted by plugin.install"

denied_status="$(
  curl --silent --show-error \
    --output "$temporary_dir/denied-install.json" \
    --write-out '%{http_code}' \
    --request POST \
    --header "content-type: application/json" \
    --data '{"allowLifecycleScripts":false}' \
    "http://127.0.0.1:$host_port/api/v1/plugin-repositories/$snapshot_id/plugins/$risk_plugin_id/install"
)" || fail "unapproved lifecycle-script request could not reach Core"
[ "$denied_status" = "409" ] ||
  fail "lifecycle-script installation was not rejected without approval"
if ! node - "$temporary_dir/denied-install.json" <<'NODE'
const fs = require("node:fs");
const response = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (response.code !== "RISK_GRANT_REQUIRED") {
  process.exit(1);
}
NODE
then
  fail "unapproved lifecycle-script installation returned the wrong error code"
fi

if find "$AUDIODOWN_HOST_DATA_DIR" -name build-risk-marker.txt -print 2>/dev/null |
  grep -q .; then
  fail "lifecycle script marker was created without approval"
fi
if docker ps -aq \
  --filter "label=io.audiodown.resource-role=plugin-build" |
  grep -q .; then
  fail "build container was created without lifecycle-script approval"
fi
if docker images -q \
  --filter "label=io.audiodown.plugin-id=$risk_plugin_id" |
  grep -q .; then
  fail "managed image was created without lifecycle-script approval"
fi
if find "$AUDIODOWN_HOST_DATA_DIR/plugins/grants" \
  -type f -name '*.json' -print 2>/dev/null |
  grep -q .; then
  fail "risk-grant mirror was created without lifecycle-script approval"
fi
database="$AUDIODOWN_HOST_DATA_DIR/audiodown.db"
[ -f "$database" ] ||
  fail "SQLite database was not created before side-effect assertions"
compose stop audiodown >/dev/null
database_snapshot="$temporary_dir/database-snapshot"
snapshot_database "$database_snapshot"
database="$database_snapshot/audiodown.db"
denied_plugin_count="$(
  sqlite3 -noheader "$database" \
    "SELECT COUNT(*) FROM plugins WHERE plugin_id = '$risk_plugin_id';"
)"
[ "$denied_plugin_count" = "0" ] ||
  fail "unapproved lifecycle-script installation created a SQLite plugin row"
denied_grant_count="$(
  sqlite3 -noheader "$database" \
    "SELECT COUNT(*) FROM plugin_risk_grants WHERE plugin_id = '$risk_plugin_id';"
)"
compose start audiodown >/dev/null
wait_for_core
[ "$denied_grant_count" = "0" ] ||
  fail "unapproved lifecycle-script installation persisted a risk grant"

(
  curl --silent --show-error \
    --output "$temporary_dir/granted-install.json" \
    --write-out '%{http_code}' \
    --request POST \
    --header "content-type: application/json" \
    --header "x-audiodown-dev-token: $fixture_dev_token" \
    --data '{"allowLifecycleScripts":true}' \
    "http://127.0.0.1:$host_port/api/v1/plugin-repositories/$snapshot_id/plugins/$risk_plugin_id/install" \
    >"$temporary_dir/granted-install.status"
) &
install_pid=$!

attempt=1
while [ "$attempt" -le 600 ]; do
  builder_container="$(
    docker ps -q \
      --filter "label=io.audiodown.resource-role=plugin-build" |
      head -n 1
  )"
  proxy_container="$(
    docker ps -q \
      --filter "label=io.audiodown.resource-role=plugin-build-proxy" |
      head -n 1
  )"
  if [ -n "$builder_container" ] && [ -n "$proxy_container" ]; then
    break
  fi
  if ! kill -0 "$install_pid" >/dev/null 2>&1; then
    fail "granted build finished before managed build and proxy containers were observable"
  fi
  [ "$attempt" -lt 600 ] ||
    fail "managed build and proxy labels were not observable during the granted build"
  attempt=$((attempt + 1))
  sleep 0.1
done

operation_id="$(
  docker inspect --format \
    '{{ index .Config.Labels "io.audiodown.operation-id" }}' \
    "$builder_container"
)"
[ -n "$operation_id" ] ||
  fail "build container is missing its operation label"
proxy_operation_id="$(
  docker inspect --format \
    '{{ index .Config.Labels "io.audiodown.operation-id" }}' \
    "$proxy_container"
)"
[ "$proxy_operation_id" = "$operation_id" ] ||
  fail "build proxy operation label does not match the builder"

internal_network="audiodown-build-$operation_id-internal"
egress_network="audiodown-build-$operation_id-egress"

docker inspect "$builder_container" "$proxy_container" |
  node -e '
let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => { input += chunk; });
process.stdin.on("end", () => {
  const [internalNetwork, egressNetwork] = process.argv.slice(1);
  const [builder, proxy] = JSON.parse(input);
  const fail = (message) => {
    console.error(`PLUGIN_INSTALL_SECURITY: ${message}`);
    process.exit(1);
  };
  const hasSocket = (container) =>
    (container.Mounts ?? []).some(
      (mount) =>
        mount.Source === "/var/run/docker.sock" ||
        mount.Destination === "/var/run/docker.sock",
    );
  const hasForbiddenMount = (container) =>
    (container.Mounts ?? []).some((mount) =>
      ["/data", "/downloads"].some(
        (path) =>
          mount.Destination === path ||
          mount.Destination?.startsWith(`${path}/`),
      ),
    );
  const networkNames = (container) =>
    Object.keys(container.NetworkSettings?.Networks ?? {}).sort();

  if (hasSocket(builder)) {
    fail("build container mounts the Docker Socket");
  }
  if (hasForbiddenMount(builder) || (builder.Mounts ?? []).length !== 0) {
    fail("build container mounts Core data, downloads, or host paths");
  }
  if (builder.HostConfig?.Privileged !== false) {
    fail("build container runs in privileged mode");
  }
  if (
    builder.HostConfig?.NetworkMode === "host" ||
    builder.HostConfig?.NetworkMode !== internalNetwork
  ) {
    fail("build container uses Host networking or a non-internal network");
  }
  if (
    JSON.stringify(networkNames(builder)) !==
    JSON.stringify([internalNetwork])
  ) {
    fail("build container joins a network other than its internal build network");
  }
  const builderEnv = new Set(builder.Config?.Env ?? []);
  if (
    !builderEnv.has("HTTP_PROXY=http://audiodown-npm-proxy:18081") ||
    !builderEnv.has("HTTPS_PROXY=http://audiodown-npm-proxy:18081") ||
    !builderEnv.has("NO_PROXY=")
  ) {
    fail("build container is not pinned to the fixed build proxy");
  }
  if (
    builder.HostConfig?.ReadonlyRootfs !== true ||
    !(builder.HostConfig?.CapDrop ?? []).includes("ALL") ||
    !(builder.HostConfig?.SecurityOpt ?? []).includes(
      "no-new-privileges:true",
    )
  ) {
    fail("build container lost its read-only, capability, or privilege restrictions");
  }

  if (hasSocket(proxy)) {
    fail("build proxy mounts the Docker Socket");
  }
  if ((proxy.Mounts ?? []).length !== 0) {
    fail("build proxy has host mounts");
  }
  if (proxy.HostConfig?.Privileged !== false) {
    fail("build proxy runs in privileged mode");
  }
  if (proxy.HostConfig?.NetworkMode === "host") {
    fail("build proxy uses Host networking");
  }
  if (
    JSON.stringify(networkNames(proxy)) !==
    JSON.stringify([egressNetwork, internalNetwork].sort())
  ) {
    fail("build proxy does not join exactly one internal and one egress network");
  }
  if (
    proxy.HostConfig?.ReadonlyRootfs !== true ||
    proxy.HostConfig?.Memory !== 128 * 1024 * 1024 ||
    proxy.HostConfig?.MemorySwap !== 128 * 1024 * 1024 ||
    proxy.HostConfig?.NanoCpus !== 500000000 ||
    proxy.HostConfig?.PidsLimit !== 64
  ) {
    fail("build proxy lost its fixed filesystem or resource limits");
  }
});
' "$internal_network" "$egress_network"

docker network inspect "$internal_network" "$egress_network" |
  node -e '
let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => { input += chunk; });
process.stdin.on("end", () => {
  const [builderId, proxyId] = process.argv.slice(1);
  const [internal, egress] = JSON.parse(input);
  const fail = (message) => {
    console.error(`PLUGIN_INSTALL_SECURITY: ${message}`);
    process.exit(1);
  };
  const ids = (network) =>
    Object.keys(network.Containers ?? {}).map((value) => value.slice(0, 12));
  const builderShort = builderId.slice(0, 12);
  const proxyShort = proxyId.slice(0, 12);
  const internalIds = ids(internal);
  const egressIds = ids(egress);

  if (internal.Internal !== true) {
    fail("operation-scoped build network is not internal");
  }
  if (
    internalIds.length !== 2 ||
    !internalIds.includes(builderShort) ||
    !internalIds.includes(proxyShort)
  ) {
    fail("internal build network contains endpoints other than builder and proxy");
  }
  if (
    egress.Internal !== false ||
    egressIds.length !== 1 ||
    egressIds[0] !== proxyShort
  ) {
    fail("operation-scoped egress network is not exclusive to the build proxy");
  }
});
' "$builder_container" "$proxy_container"

if ! docker exec "$builder_container" node -e '
  const net = require("node:net");
  const socket = net.connect({
    host: "audiodown-npm-proxy",
    port: 18081,
  });
  const timer = setTimeout(() => process.exit(1), 2000);
  socket.once("connect", () => {
    clearTimeout(timer);
    socket.destroy();
    process.exit(0);
  });
  socket.once("error", () => process.exit(1));
'; then
  fail "build container cannot reach its fixed proxy"
fi

wait "$install_pid" ||
  fail "granted lifecycle-script installation request failed"
install_pid=""
granted_status="$(cat "$temporary_dir/granted-install.status")"
[ "$granted_status" = "200" ] ||
  fail "granted lifecycle-script installation did not complete successfully"
if ! node - "$temporary_dir/granted-install.json" "$risk_plugin_id" <<'NODE'
const fs = require("node:fs");
const [file, pluginId] = process.argv.slice(2);
const installed = JSON.parse(fs.readFileSync(file, "utf8"));
if (installed.pluginId !== pluginId || installed.status !== "installed") {
  process.exit(1);
}
NODE
then
  fail "granted lifecycle-script installation returned an invalid plugin record"
fi

if find "$AUDIODOWN_HOST_DATA_DIR/plugins/prepared" \
  -type f -name '*.json' -print 2>/dev/null |
  grep -q .; then
  fail "successful build left a prepared installation file"
fi
if find "$AUDIODOWN_HOST_DATA_DIR/plugins/grants" \
  -type f -name '*.json' -print 2>/dev/null |
  grep -q .; then
  fail "successful build left a lifecycle risk-grant mirror"
fi

mock_requests_before="$(docker logs "$mock_name" 2>&1 | grep -c '^GET ' || true)"
compose restart audiodown >/dev/null
wait_for_core
sleep 3
mock_requests_after="$(docker logs "$mock_name" 2>&1 | grep -c '^GET ' || true)"
[ "$mock_requests_after" = "$mock_requests_before" ] ||
  fail "Core made an automatic repository update request after installation"

curl --fail --silent --show-error --request POST \
  "http://127.0.0.1:$host_port/api/v1/plugins/$risk_plugin_id/start" \
  >"$temporary_dir/runtime-start.json" ||
  fail "installed virtual plugin could not start for runtime security assertions"

plugin_container="$(
  docker ps -q \
    --filter "label=io.audiodown.managed=true" \
    --filter "label=io.audiodown.plugin-id=$risk_plugin_id"
)"
[ -n "$plugin_container" ] ||
  fail "runtime plugin container is missing after start"
docker exec "$plugin_container" \
  sh -c 'test "$(cat /plugin/build-risk-marker.txt)" = approved' ||
  fail "approved lifecycle script output is missing from the managed plugin image"

docker inspect "$plugin_container" |
  node -e '
    let input = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (chunk) => { input += chunk; });
    process.stdin.on("end", () => {
      const [plugin] = JSON.parse(input);
      const fail = (message) => {
        console.error(`PLUGIN_INSTALL_SECURITY: ${message}`);
        process.exit(1);
      };
      const mounts = plugin.Mounts ?? [];
      if (
        mounts.some(
          (mount) =>
            mount.Source === "/var/run/docker.sock" ||
            mount.Destination === "/var/run/docker.sock",
        )
      ) {
        fail("runtime plugin mounts the Docker Socket");
      }
      if (
        mounts.some((mount) =>
          ["/data", "/downloads"].some(
            (path) =>
              mount.Destination === path ||
              mount.Destination?.startsWith(`${path}/`),
          ),
        )
      ) {
        fail("runtime plugin mounts Core data or downloads");
      }
      if (
        plugin.HostConfig?.NetworkMode !== "none" ||
        plugin.Config?.NetworkDisabled !== true
      ) {
        fail("runtime plugin networking is not disabled");
      }
      if (
        plugin.HostConfig?.Privileged !== false ||
        plugin.HostConfig?.ReadonlyRootfs !== true ||
        !(plugin.HostConfig?.CapDrop ?? []).includes("ALL") ||
        !(plugin.HostConfig?.SecurityOpt ?? []).includes(
          "no-new-privileges:true",
        )
      ) {
        fail("runtime plugin lost its phase-one container restrictions");
      }
    });
  '

curl --fail --silent --show-error --request POST \
  "http://127.0.0.1:$host_port/api/v1/plugins/$risk_plugin_id/stop" \
  >/dev/null ||
  fail "virtual plugin could not stop after runtime security assertions"
curl --fail --silent --show-error --request DELETE \
  "http://127.0.0.1:$host_port/api/v1/plugins/$risk_plugin_id" \
  >/dev/null ||
  fail "virtual plugin could not uninstall after security assertions"

if docker images -q \
  --filter "label=io.audiodown.plugin-id=$risk_plugin_id" |
  grep -q .; then
  fail "security test uninstall left the managed plugin image"
fi
[ ! -e "$AUDIODOWN_HOST_DATA_DIR/plugins/installed/$risk_plugin_id" ] ||
  fail "security test uninstall left the managed install directory"
compose stop audiodown >/dev/null
snapshot_database "$database_snapshot"
database="$database_snapshot/audiodown.db"
remaining_plugin_count="$(
  sqlite3 -noheader "$database" \
    "SELECT COUNT(*) FROM plugins WHERE plugin_id = '$risk_plugin_id';"
)"
compose start audiodown >/dev/null
wait_for_core
[ "$remaining_plugin_count" = "0" ] ||
  fail "security test uninstall left the SQLite plugin row"

printf '%s\n' "Plugin installation security assertions passed"
