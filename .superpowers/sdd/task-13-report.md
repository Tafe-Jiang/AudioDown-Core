# Task 13 Report

## Baseline

- Initial `git status --short --branch`: `## main...origin/main`; the controller-owned Task 13 design-correction edit in the implementation plan was preserved.
- HEAD and `origin/main`: `2c1c0903a466c793fb5cb38fb8946ede143d513b`.

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
