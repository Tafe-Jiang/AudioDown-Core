#!/bin/sh
set -eu

secret_file=/run/audiodown-secrets/proxy-token
attempt=0
while [ ! -f "$secret_file" ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 200 ]; then
    echo 'Plugin proxy token was not delivered' >&2
    exit 1
  fi
  sleep 0.05
done

AUDIODOWN_PROXY_TOKEN="$(cat "$secret_file")"
rm -f "$secret_file"
[ -n "$AUDIODOWN_PROXY_TOKEN" ] || exit 1
export AUDIODOWN_PROXY_TOKEN
unset secret_file attempt
exec "$@"
