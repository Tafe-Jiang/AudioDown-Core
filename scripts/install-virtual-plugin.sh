#!/bin/sh
set -eu

plugin_id="com.audiodown.virtual.content"
image_id="audiodown/plugin-virtual:dev"
fixture_manifest="test-fixtures/plugins/virtual/audiodown-plugin.json"
install_dir="/data/plugins/installed/$plugin_id"
installed_manifest="$install_dir/audiodown-plugin.json"
dev_token="${AUDIODOWN_DEV_TOKEN:-}"

if [ -z "$dev_token" ]; then
  echo "AUDIODOWN_DEV_TOKEN must be set" >&2
  exit 1
fi

temporary_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$temporary_dir"
}
trap cleanup EXIT INT TERM

docker build \
  --file docker/plugin-runtime/node22.Dockerfile \
  --tag "$image_id" \
  .

installation_id="$(
  docker compose exec -T supervisor \
    sh -c 'cat /data/plugins/installation-id'
)"
installation_id="$(printf '%s' "$installation_id" | tr -d '\r\n')"
if [ -z "$installation_id" ]; then
  echo "Supervisor installation ID is unavailable" >&2
  exit 1
fi

manifest_hash="$(
  node -e '
    const fs = require("node:fs");
    const crypto = require("node:crypto");
    const bytes = fs.readFileSync(process.argv[1]);
    process.stdout.write(crypto.createHash("sha256").update(bytes).digest("hex"));
  ' "$fixture_manifest"
)"

docker compose exec -T supervisor sh -c "
  set -eu
  umask 077
  mkdir -p '$install_dir'
  temporary='$install_dir/.manifest.tmp'
  cat > \"\$temporary\"
  mv \"\$temporary\" '$installed_manifest'
" < "$fixture_manifest"

record_file="$temporary_dir/install.json"
node -e '
  const fs = require("node:fs");
  const [
    output,
    pluginId,
    imageId,
    manifestPath,
    manifestHash,
    installationId,
  ] = process.argv.slice(1);
  fs.writeFileSync(output, `${JSON.stringify({
    pluginId,
    imageId,
    manifestPath,
    manifestHash,
    installationId,
    memoryBytes: 134217728,
    nanoCpus: 500000000,
    pidsLimit: 64,
    runMode: "on_demand",
  })}\n`, { mode: 0o600 });
' \
  "$record_file" \
  "$plugin_id" \
  "$image_id" \
  "$installed_manifest" \
  "$manifest_hash" \
  "$installation_id"

docker compose exec -T supervisor sh -c "
  set -eu
  umask 077
  temporary='$install_dir/.install.tmp'
  cat > \"\$temporary\"
  mv \"\$temporary\" '$install_dir/install.json'
" < "$record_file"

request_file="$temporary_dir/register.json"
node -e '
  const fs = require("node:fs");
  const [output, manifestPath, manifestHash, imageId] = process.argv.slice(1);
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  fs.writeFileSync(output, JSON.stringify({ manifest, manifestHash, imageId }));
' "$request_file" "$fixture_manifest" "$manifest_hash" "$image_id"

curl --fail --silent --show-error \
  --request POST \
  --header "content-type: application/json" \
  --header "x-audiodown-dev-token: $dev_token" \
  --data-binary "@$request_file" \
  http://127.0.0.1:18080/api/v1/dev/plugins/register-fixture \
  >/dev/null

printf '%s\n' "Installed virtual plugin fixture: $plugin_id"
