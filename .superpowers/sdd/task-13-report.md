# Post-`a57cdd8` State Consistency Repair RED/GREEN (2026-07-16)

## RED

- Added plugin-manager regressions proving that process-boundary cleanup must
  skip `Installing` records without changing their install operation IDs, and
  that a disabled plugin remains `Disabled` after inspection confirms no
  runtime is left. Both focused regressions failed against `a57cdd8`: cleanup
  contacted the runtime and changed the installing record, while inspection
  persisted the raw `Stopped` status.
- Updated the Supervisor adapter regressions to require both inspect errors and
  non-Healthy inspect results to return the final confirmed `Stopped` state.
  Against `a57cdd8`, confirmed cleanup revoked the generation but returned the
  stale error or pre-cleanup `Unhealthy` state.

## GREEN

- `cargo test -p audiodown-plugin-manager --test management_service --locked`
  -> exit 0; all 14 tests passed.
- `cargo test -p audiodown-server --test supervisor_client --locked` -> exit 0;
  all 16 tests passed, including cleanup-failure generation preservation.
- Full plugin-manager, Server, and Supervisor test suites passed after the
  repair; Gateway remained at 9 passing tests.
- Plugin-manager, Server, and Supervisor Clippy with `-D warnings`, workspace
  check, formatting, diff checks, and Compose configuration all passed.
- `./tests/security-boundary.sh` -> exit 0 and printed
  `Security boundary assertions passed` after rebuilding the changed Core and
  Supervisor images.
- Final residue checks found no Compose services, no containers labeled
  `io.audiodown.managed=true`, and no AudioDown-named containers.

The process-boundary cleanup report now counts skipped installations separately.
Confirmed inspect cleanup is represented as the final stopped/disabled state;
cleanup failure still returns an error and preserves the current proxy-token
generation. Task 13 remains pending independent approval, so Task 14 has not
started.

# Final-Gate Lifecycle RED (2026-07-16)

- Added plugin-manager regressions for paired stop after start/inspect failure,
  non-Healthy inspection cleanup, failed cleanup preservation, and full runtime
  cleanup continuation/aggregation.
- `cargo test -p audiodown-plugin-manager --test management_service --locked`
  -> exit 101 at compilation because `PluginManagerService::cleanup_all_runtimes`
  does not exist. The transaction assertions are therefore pending the minimum
  production API and will be rerun after implementation.

## Final-Gate Lifecycle GREEN (2026-07-16)

- `cargo test -p audiodown-plugin-manager --test management_service --locked`
  -> exit 0; all 12 tests passed, including inspect-error/non-Healthy rollback,
  cleanup failure state, log-persistence state, and continued aggregate cleanup.
- `cargo test -p audiodown-plugin-manager --test lifecycle_service --locked`
  -> exit 0; all 4 tests passed.
- `cargo test -p audiodown-server --test supervisor_client --locked` -> exit 0;
  all 15 tests passed, including exact generation revoke after confirmed inspect
  cleanup and preservation when cleanup fails.
- `cargo test -p audiodown-server --locked --quiet` -> exit 0, including four
  ordered-shutdown unit tests and the complete Server integration suite.
- `cargo test -p audiodown-supervisor --locked --quiet` -> exit 0.
- `cargo test -p audiodown-proxy-gateway --locked --quiet` -> exit 0; all 9
  Gateway tests passed.
- Plugin-manager, Server, and Supervisor Clippy with `-D warnings` -> exit 0.

- `cargo check --workspace --all-targets --locked` -> exit 0.
- `cargo fmt --all -- --check` -> exit 0.
- `git diff --check` -> exit 0 before the final shell-only noise cleanup; the
  final diff check is rerun immediately before commit.
- `docker compose config` -> exit 0.
- `./tests/security-boundary.sh` -> exit 0 twice after image construction. The
  script proved that restarting only Core while Supervisor remained running
  removed the previous plugin container, fixed Gateway container, and internal
  network before the new Core served requests. The final cached rerun printed
  `Security boundary assertions passed` with no `/proc` race warning.
- The first long image-build attempts encountered transient
  `static.rust-lang.org` TLS EOF/crates.io throughput retries. One overlapping
  Compose retry then lost its one-shot helper container. After an explicit
  `docker compose down --remove-orphans`, two complete cached runs passed with
  exit code 0; no security assertion was weakened.
- Final cleanup check: `docker compose ps -a` was empty and
  `docker ps -aq --filter label=io.audiodown.managed=true` returned no
  containers.

# Task 13 Report

## Third Review Keep-alive Repair RED (2026-07-16, after `d17c569`)

- `cargo test -p audiodown-proxy-gateway --test relay closes_client_requested_keep_alive_after_one_relay --locked -- --nocapture --test-threads=1` -> exit 101. The valid relay returned 200, but the client-requested keep-alive connection remained open beyond the 300ms hard timeout: `client-requested keep-alive connection must close after one response: Elapsed(())`.

## Third Review Keep-alive Repair GREEN (2026-07-16)

- `cargo test -p audiodown-proxy-gateway --test relay closes_client_requested_keep_alive_after_one_relay --locked -- --nocapture --test-threads=1` -> exit 0; the raw TCP regression passed.
- `cargo test -p audiodown-proxy-gateway --locked` -> exit 0; all 9 Gateway tests passed.
- `cargo clippy -p audiodown-proxy-gateway --all-targets --locked -- -D warnings` -> exit 0.
- `cargo check --workspace --all-targets --locked` -> exit 0.
- `cargo fmt --all -- --check` -> exit 0.
- `git diff --check` -> exit 0.

The Gateway now disables HTTP/1 keep-alive and emits `Connection: close` on
normal and safe JSON error responses. The connection-level permit is therefore
released after one request/response connection completes. Task 13 remains
pending independent controller approval; Task 14 was not entered.

## Second Re-review Repair RED/GREEN (2026-07-15, after `8f44eb3`)

### Baseline

- Initial `git status --short --branch`: `## main...origin/main [ahead 1]` with only the intentionally local `.superpowers/sdd/task-13-report.md` modification.
- Second repair baseline HEAD: `8f44eb373df6ab368c72418fd3ff33bedb17a45d`; `origin/main`: `71512bd1af21b64bdc549664e32052b1661e675a`.

### Atomic secret publish RED

- `cargo test -p audiodown-supervisor --test policy proxy_token_publish --locked -- --nocapture` -> exit 101. The delayed/chunked writer and short-input cleanup regressions could not import `audiodown_supervisor::docker::proxy_token_publish_command`. The current delivery command writes directly to the final `proxy-token` path, so it has no production contract for a permission-restricted temporary file, exact byte-length validation, atomic rename, or failure cleanup.

### Atomic secret publish GREEN

- `cargo test -p audiodown-supervisor --test policy proxy_token_publish --locked -- --nocapture` -> exit 0; 2 tests passed in 0.04s. During delayed chunked stdin, only the mode-restricted temporary path existed; the final path appeared only after exact byte-length validation and same-directory atomic rename. Short input failed and removed both the temporary and final paths.

### Connection-level Gateway limits RED

- `cargo test -p audiodown-proxy-gateway --test relay connection_level_limit --locked -- --nocapture --test-threads=1` -> exit 101; both targeted tests failed. The incomplete-header connection remained open beyond the 300ms test deadline (`Elapsed(())`), and with `max_concurrency=1` an incomplete first connection did not consume the handler semaphore, so a second complete request still received an HTTP response instead of being rejected before header parsing.

### Connection-level Gateway limits GREEN

- `cargo test -p audiodown-proxy-gateway --test relay connection_level_limit --locked -- --nocapture --test-threads=1` -> exit 0; 2 tests passed in 0.12s. The accept loop now acquires one bounded permit for the complete HTTP connection lifetime, saturated connections are closed before handler dispatch, and Hyper's HTTP/1 parser applies the server deadline while reading headers. Existing handler body/server timeouts and strict backend EOF checks remain in place.

### Existing image compatibility RED

- `cargo test -p audiodown-supervisor --test policy plugin_container_uses_fixed_inline_bootstrap_for_existing_images --locked -- --nocapture` -> exit 101. The new metadata regression expected a fixed `/bin/sh -c` inline entrypoint but the current entrypoint contained only `/usr/local/bin/audiodown-plugin-bootstrap`, producing `range end index 2 out of range for slice of length 1`.
- `./tests/security-boundary.sh` -> exit 1 after the real plugin installed and started. The new assertion printed `SECURITY_BOUNDARY: plugin startup must not depend on a bootstrap file in the attested image`, proving the current image still contains and requires the 8f44eb3 bootstrap asset. The cleanup trap removed the validation resources.

### Existing image compatibility GREEN

- `cargo test -p audiodown-supervisor --test policy plugin_container_uses_fixed_inline_bootstrap_for_existing_images --locked -- --nocapture` -> exit 0; the metadata regression passed. Supervisor now supplies a fixed token-free `/bin/sh -c` entrypoint script and preserves the policy-derived plugin command as Docker `Cmd`; image label, digest, and attestation checks were not changed.
- `./tests/security-boundary.sh` -> exit 0; printed `Security boundary assertions passed`. The rebuilt real plugin image contains no `/usr/local/bin/audiodown-plugin-bootstrap`, yet the Supervisor inline wrapper waited for the atomically published final file, removed both final/temporary secret paths, exported the token only to the plugin process, completed the handshake, and passed the existing network/cleanup boundaries.

### Focused regression follow-up

- The first complete `cargo test -p audiodown-proxy-gateway --locked` run exposed a connection-saturation compatibility regression: the accept-level rejection reset the socket while the existing test required the fixed safe 503 response. The production accept path was corrected to perform a short bounded drain and return a fixed token-free 503 without spawning an unbounded task; the new saturation regression was strengthened to require that response.
- `cargo test -p audiodown-proxy-gateway --locked` -> exit 0; all 8 Gateway tests passed, including the original handler saturation assertion and both new connection-level regressions.
- `cargo test -p audiodown-supervisor --test policy proxy_token_publish --locked -- --nocapture` -> exit 0; 2 tests passed in 0.03s after strengthening the delayed-chunk regression to verify temporary-file mode `0600` directly.

### Second repair final verification

- `cargo test -p audiodown-proxy-gateway --locked` -> exit 0; all 8 tests passed.
- `cargo test -p audiodown-supervisor-protocol --locked` -> exit 0; all 10 tests passed.
- `cargo test -p audiodown-server --test supervisor_client --locked` -> exit 0; all 13 tests passed in 2.22s with no hang.
- `cargo test -p audiodown-supervisor --locked` -> exit 0; all Supervisor unit, integration, and doc tests passed, including 9 policy tests.
- `cargo test -p audiodown-server --locked` -> exit 0; the complete Server suite passed, including 13 Supervisor client tests in 2.21s.
- `cargo clippy -p audiodown-proxy-gateway --all-targets --locked -- -D warnings` -> exit 0.
- `cargo clippy -p audiodown-supervisor --all-targets --locked -- -D warnings` -> exit 0.
- `cargo clippy -p audiodown-server --all-targets --locked -- -D warnings` -> exit 0.
- `cargo check --workspace --all-targets --locked` -> exit 0.
- `cargo fmt --all -- --check` -> exit 0.
- `git diff --check` -> exit 0.
- `docker compose config` -> exit 0.
- Final `./tests/security-boundary.sh` -> exit 0; printed `Security boundary assertions passed` after rebuilding the final Gateway connection-limit code and the wrapper-free plugin image.
- After removing `audiodown/plugin-gateway:1.0.0-alpha.1`, ordinary `docker compose up -d --build` rebuilt all default images, started Core and Supervisor, and recreated the one-shot Gateway image helper successfully. Cleanup removed all validation containers and the Compose network; the command printed `Clean Gateway image Compose build/start/cleanup passed`.
- Final residue checks: `docker compose ps -a` was empty, the `io.audiodown.managed=true` container query was empty, and no Cargo test or Supervisor process remained.

## Review Repair RED/GREEN (2026-07-15)

### Lifecycle RED

- `cargo test -p audiodown-server --test supervisor_client --locked` -> FAIL. `serializes_same_plugin_starts_before_contacting_supervisor` observed a second Supervisor connection before the first start completed; `keeps_generation_when_stop_cleanup_fails` found the generation token had already been revoked after Docker cleanup failed. The ambiguous/failed-start fixtures also waited for a cleanup-confirmation stop request that the current runtime never sent.
- `cargo test -p audiodown-supervisor --test protocol --locked` -> exit 101. The keyed `PluginLifecycleLocks` contract did not exist, so Supervisor could not serialize start/stop/remove per plugin.

The first lifecycle RED command left PID 65792 (`cargo`) and PID 65872 (`supervisor_client`) alive after the tool-side interruption. At that RED revision, failed/ambiguous-start fixtures intentionally waited for a cleanup-confirmation stop that production did not yet send, so the orphaned test process could not finish. Both processes were terminated after inspection. Five lifecycle tests then passed individually under a 15-second alarm with `--nocapture --test-threads=1` (slowest 2.10s), followed by the complete 13-test client file in 5.19s and the Supervisor keyed-lock test in 0.04s under the same hard timeout. This confirmed the hang was an unreaped RED process, not a remaining lock-order deadlock.

### Gateway RED

- `cargo test -p audiodown-proxy-gateway --locked` -> exit 101. The test-only short-limit contract could not import `GatewayLimits` or `serve_with_limits`; production had no body/server timeout or concurrency-limit configuration, and the delayed trailing-byte regression was therefore unsupported.

### Docker cleanup and token bootstrap RED

- `cargo test -p audiodown-supervisor --test policy --locked` -> exit 101. Ownership-only discovery/health helpers, three-resource startup discovery, aggregated reconcile results, fixed bootstrap metadata, and tmpfs secret constants did not exist. Production still serialized the proxy token into Docker `Config.Env`.

### Compose and runtime boundary RED

- `./tests/security-boundary.sh` -> exit 1 before build. The default `docker compose config` omitted `plugin-gateway-image` because it was profile-only, proving that ordinary `docker compose up -d --build` could not prepare the fixed Gateway image.

### Repair GREEN

- `cargo test -p audiodown-proxy-gateway --locked` -> exit 0; all 6 tests passed, including body/server timeout, concurrency saturation, and delayed trailing-byte rejection.
- `cargo test -p audiodown-supervisor-protocol --locked` -> exit 0; all 10 tests passed.
- `cargo test -p audiodown-server --test supervisor_client --locked` -> exit 0; all 13 tests passed in 5.19s. The five lifecycle regressions also passed individually with `--nocapture --test-threads=1` under a 15-second alarm; the slowest completed in 2.10s.
- `cargo test -p audiodown-supervisor --locked` -> exit 0; all unit, integration, and doc tests passed, including the keyed lifecycle-lock test in 0.04s under a 15-second alarm.
- `cargo test -p audiodown-server --locked` -> exit 0; the complete Server suite passed.
- `cargo clippy -p audiodown-proxy-gateway --all-targets --locked -- -D warnings` -> exit 0.
- `cargo clippy -p audiodown-supervisor --all-targets --locked -- -D warnings` -> exit 0.
- `cargo clippy -p audiodown-server --all-targets --locked -- -D warnings` -> exit 0.
- `cargo check --workspace --all-targets --locked` -> exit 0.
- `cargo fmt --all -- --check` -> exit 0.
- `git diff --check` -> exit 0.
- `docker compose config` -> exit 0.
- `./tests/security-boundary.sh` -> exit 0; printed `Security boundary assertions passed`. The real Docker checks confirmed no token in `Config.Env`, deletion of the one-time tmpfs secret file, token delivery only to the plugin process, ownership-only cleanup of a malformed network, continued reconciliation after one cleanup failure, and aggregated retry cleanup.
- From a removed `audiodown/plugin-gateway:1.0.0-alpha.1` image, ordinary `docker compose up -d --build` rebuilt the Gateway, Supervisor, and Core images and started Core/Supervisor successfully. The one-shot Gateway builder exposed no ports or mounts. `docker compose down --remove-orphans` then removed all validation containers and the Compose network; `docker compose ps -a` and the managed-container query were empty. The command printed `Clean Gateway image Compose build/start/cleanup passed`.

Token bootstrap implementation note: an attempted Docker archive/copy delivery was rejected because `ReadonlyRootfs` prevents the archive API even when the destination is a tmpfs mount. The final fixed path creates an exec whose metadata contains only the secret byte length, streams the token over exec stdin into the dedicated tmpfs file, and uses the repository-owned wrapper to read and delete the file before exporting the token solely to the plugin process. The Node SDK was not changed.

## Baseline

- Initial `git status --short --branch`: `## main...origin/main`; the controller-owned Task 13 design-correction edit in the implementation plan was preserved.
- Repair baseline HEAD and `origin/main`: `71512bd`.

## RED

Task 13 tests and necessary crate scaffolding were written before production implementation. Step 2 produced the expected failures:

- `cargo test -p audiodown-proxy-gateway` -> exit 101. After correcting a test-only missing `Future` import, the rerun failed because the fixed Gateway library, `serve`, frame limit, and safe error type did not exist.
- `cargo test -p audiodown-supervisor-protocol` -> exit 101. `PluginStartRequest`, redacted `ProxyToken`, and the typed `SupervisorParams::Start` variant did not exist.
- `cargo test -p audiodown-server --test supervisor_client` -> exit 101. The trusted start method, shared-registry runtime constructor, and generation registration/revocation integration did not exist.
- `cargo test -p audiodown-supervisor --test policy --test protocol` -> exit 101. The fixed Gateway configuration, per-plugin internal network/runtime policy, fixed constants, redacted token type, and typed start params did not exist.
- `./tests/security-boundary.sh` -> exit 1. Compose had no repository-built `plugin-gateway-image` service, so the fixed Gateway/runtime boundary was absent.

These failures were specific to the fixed Gateway, trusted start protocol, generation-bound token lifecycle, and derived Supervisor runtime policy required by Task 13.

## GREEN

- Added the repository-owned `audiodown-proxy-gateway` binary and image. The production binary binds only `0.0.0.0:18081`, connects only to `/run/audiodown-proxy/core.sock`, and relays one JSON frame with the exact one-MiB/newline contract. It emits no request, URL, header, body, or token logs and returns fixed safe errors.
- Added trusted typed start parameters containing only `pluginId` and a bounded proxy token with redacted `Debug`; the existing top-level Supervisor control token remains unchanged. Public Axum plugin start inputs remain path-only.
- Injected the Task 12 `ProxyTokenRegistry` into `SupervisorPluginRuntime`. Each start registers a fresh generation before the Supervisor request; replacement, failed start, stop, remove/uninstall, unhealthy inspection, and Core shutdown revoke the applicable generation or all generations.
- Added deployment-owned fixed Gateway image/volume configuration and deterministic 12-hex plugin, Gateway, and internal-network names. Plugin and Gateway containers share only that internal non-attachable network; only the Gateway mounts the proxy volume read-only, and only the plugin receives the fixed Gateway URL and generation token.
- Added paired Docker cleanup in plugin -> Gateway -> network order for replacement, failure, stop, and remove, plus Supervisor startup reconciliation for stale managed Gateway/network resources. Label and installation ownership checks remain mandatory.
- Added the Compose proxy volume with `name: "${AUDIODOWN_PROXY_VOLUME:-audiodown-proxy}"`, mounted only into Core. Supervisor receives only its name through deployment configuration; Core still has no Docker Socket and still publishes only port 18080.
- Expanded the real Docker boundary test to verify image, alias, network membership, no public/private/Core/other-plugin reachability, mounts, environment, ports, Host PID/network, sandbox limits, stop cleanup, and startup reconciliation.

## Verification

All required Task 13 checks passed:

- `cargo test -p audiodown-proxy-gateway` -> exit 0; 3 relay/frame tests passed.
- `cargo test -p audiodown-supervisor-protocol` -> exit 0; 10 contract tests passed.
- `cargo test -p audiodown-server --test supervisor_client` -> exit 0; 10 client/generation tests passed.
- `cargo test -p audiodown-supervisor` -> exit 0; all Supervisor unit, integration, and doc tests passed.
- `cargo clippy -p audiodown-proxy-gateway --all-targets -- -D warnings` -> exit 0.
- `./tests/security-boundary.sh` -> exit 0; printed `Security boundary assertions passed`, including real stop and Supervisor-startup reconciliation cleanup.
- `docker compose config` -> exit 0.
- `cargo fmt --all -- --check` -> exit 0 after applying standard formatting within Task 13-owned Rust files.
- `cargo check --workspace --all-targets --locked` -> exit 0.
- `git diff --check` -> exit 0.

Additional regression coverage:

- `cargo test -p audiodown-server --locked` -> exit 0; the complete server suite passed, including Core shutdown token revocation and uninstall cleanup coverage.

Docker validation diagnostics:

- Two early security-boundary attempts stopped during image construction because `static.rust-lang.org` returned transient TLS handshake EOF errors; no container or security assertion ran in those attempts. A later Docker retry downloaded successfully without changing or bypassing any assertion.
- The first runtime assertion attempt reported `virtual plugin must use only its derived internal network`. Docker inspection showed the exact derived `HostConfig.NetworkMode`, one matching runtime network, and `internal=true`; Docker omitted the false-valued `Config.NetworkDisabled` field from JSON. The test was corrected to reject explicit `true` and require exactly one matching network. Its cleanup trap was also corrected to remove fixture-managed networks after failed runs. The complete script then passed repeatedly.

## Self-review

- Secrets: `ProxyToken` and all containing typed params render only `[REDACTED]`; the Gateway has no tracing/log calls; Supervisor and Gateway errors do not contain request data or container environment values. Canary tests and searches found no token output path.
- Public input: no Axum route/body changed. Caller-controlled image, command, path, mount, volume, network, alias, or runtime token fields are rejected by strict authenticated Supervisor params, and the Gateway image/volume come only from deployment configuration.
- Lifecycle: Core registers before each runtime start and revokes exact/current generations on replacement, failure, stop, remove/uninstall, unhealthy reconciliation, and shutdown. Docker cleanup is plugin -> Gateway -> network and continues attempting later resources after an earlier failure.
- Ownership: Supervisor remains the only Docker Socket owner and removes only resources with matching managed, installation, plugin, and resource labels. Core and plugin containers do not mount the Docker Socket; Supervisor does not mount the proxy volume.
- Boundary: plugin and Gateway have one internal non-attachable peer network, no public ports or external network, no Host PID/network, no Core data/download/control-token access, read-only root filesystems, dropped capabilities, no-new-privileges, and fixed CPU/memory/PID/tmpfs limits.
- Scope: no real platform, domain, Cookie acquisition, credential flow, download behavior, or Task 14 invocation behavior was added. All modified/untracked paths are within the authorized Task 13 whitelist, including the controller-owned plan correction.
