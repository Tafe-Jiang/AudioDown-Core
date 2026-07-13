#!/bin/sh
set -eu

root_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
verify_file="$root_dir/scripts/verify.sh"
ci_file="$root_dir/.github/workflows/ci.yml"

fail() {
  printf '%s\n' "CONTENT_PHASE3_WIRING: $*" >&2
  exit 1
}

line_number_after() {
  file="$1"
  pattern="$2"
  minimum_line="$3"
  line="$(
    awk -v pattern="$pattern" -v minimum_line="$minimum_line" \
      'NR > minimum_line && index($0, pattern) { print NR; exit }' "$file"
  )"
  [ -n "$line" ] ||
    fail "$(basename "$file") does not invoke in order: $pattern"
  printf '%s\n' "$line"
}

assert_ordered() {
  file="$1"
  shift
  previous=0

  for pattern in "$@"; do
    current="$(line_number_after "$file" "$pattern" "$previous")"
    previous="$current"
  done
}

for file in "$verify_file" "$ci_file"; do
  assert_ordered "$file" \
    './tests/plugin-installation-security.sh' \
    './tests/virtual-content-contract.sh' \
    './tests/content-aggregation-smoke.sh' \
    './tests/content-aggregation-security.sh'

  grep -F 'web/tests/content-aggregation-live.spec.ts' "$file" >/dev/null ||
    fail "$(basename "$file") does not declare web/tests/content-aggregation-live.spec.ts"
done

for artifact in \
  'artifacts/content' \
  'artifacts/runtime-plugin' \
  'artifacts/playwright' \
  'artifacts/diagnostics'; do
  grep -F "$artifact" "$ci_file" >/dev/null ||
    fail "ci.yml does not retain failure artifact content: $artifact"
done

grep -F 'if: failure()' "$ci_file" >/dev/null ||
  fail "ci.yml does not upload artifacts only after failure"
grep -F 'actions/upload-artifact' "$ci_file" >/dev/null ||
  fail "ci.yml does not upload phase-three failure artifacts"

for file in "$verify_file" "$ci_file"; do
  if grep -Eiq \
    'docker/(build-)?push-action|docker[[:space:]]+push|buildx[[:space:]]+build.*--push' \
    "$file"; then
    fail "$(basename "$file") must not publish container images"
  fi
done

printf '%s\n' "Phase-three content verification wiring is complete"
