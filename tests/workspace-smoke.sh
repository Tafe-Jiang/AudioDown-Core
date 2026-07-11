#!/bin/sh
set -eu

docker run --rm \
  -v "$(pwd):/workspace" \
  -w /workspace \
  rust:1.88-bookworm \
  cargo metadata --no-deps --format-version 1 >/tmp/audiodown-metadata.json

grep -q 'audiodown-server' /tmp/audiodown-metadata.json
grep -q 'audiodown-supervisor' /tmp/audiodown-metadata.json
