#!/bin/sh
set -eu

root_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
verify_file="$root_dir/scripts/verify.sh"
ci_file="$root_dir/.github/workflows/ci.yml"

fail() {
  printf '%s\n' "PLUGIN_PHASE2_WIRING: $*" >&2
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
  [ -n "$line" ] || fail "$(basename "$file") does not invoke: $pattern"
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

assert_ordered "$verify_file" \
  'cargo test --locked --workspace' \
  'npm test -- --run' \
  'npm run typecheck' \
  'npm run build' \
  'npx playwright test' \
  './tests/plugin-repository-smoke.sh' \
  './tests/plugin-installation-security.sh'

assert_ordered "$ci_file" \
  'cargo test --locked --workspace' \
  'npm test -- --run' \
  'npm run typecheck' \
  'npm run build' \
  'npx playwright test' \
  './tests/plugin-repository-smoke.sh' \
  './tests/plugin-installation-security.sh'

assert_ordered "$ci_file" \
  'docker-integration:' \
  '- name: Prepare embedded UI' \
  'working-directory: web' \
  'npm ci' \
  'npm run build' \
  '- name: Docker integration checks'

for file in "$verify_file" "$ci_file"; do
  grep -F 'tests/plugin-installation-live.spec.ts' "$file" >/dev/null ||
    fail "$(basename "$file") does not declare the live UI-to-Core check"
done

grep -F 'playwright-report' "$ci_file" >/dev/null ||
  fail "ci.yml does not collect the Playwright report on failure"
grep -F 'failed UI screenshots' "$ci_file" >/dev/null ||
  fail "ci.yml does not collect failed UI screenshots on failure"
grep -F 'mock GitHub logs' "$ci_file" >/dev/null ||
  fail "ci.yml does not collect mock GitHub logs on failure"
grep -F 'build proxy logs' "$ci_file" >/dev/null ||
  fail "ci.yml does not collect build proxy logs on failure"
grep -F 'runtime plugin logs' "$ci_file" >/dev/null ||
  fail "ci.yml does not collect runtime plugin logs on failure"
grep -F 'redacted test diagnostics' "$ci_file" >/dev/null ||
  fail "ci.yml does not collect redacted diagnostics on failure"

if grep -Eiq 'docker/(build-)?push-action|docker push|buildx build .*--push' "$ci_file"; then
  fail "ci.yml must not publish container images"
fi

printf '%s\n' "Phase-two verification wiring is complete"
