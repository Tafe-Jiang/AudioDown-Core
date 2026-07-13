# AudioDown Phase 3 Acceptance

## Scope

Phase three completes the virtual content-plugin aggregation loop. AudioDown-Core
remains an empty core: it contains no real platform names, domains, interfaces,
credentials, Cookie acquisition, or real resource downloads.

## Accepted Capabilities

- The Node SDK and Rust wire contract expose only `content.search`,
  `content.discover`, `content.categories`, `content.album.get`, and
  `content.tracks.list`.
- Core invokes content plugins through the authenticated Supervisor control
  socket with typed methods, fixed timeouts, bounded messages, and trusted
  managed-container lookup.
- SQLite stores search/discover participation, priority, and one default
  content plugin per virtual platform.
- Search, discover, and categories aggregate deterministic virtual plugins by
  platform. Explicit platform and plugin filters are enforced.
- Retryable first-page failures can fall back to the next eligible plugin.
  One failed source does not discard successful results from other platforms.
- Core-owned opaque cursors pin later pages to the selected source and bind the
  cursor to its operation, query, and filters.
- Canonical deduplication applies only to matching resource types with a
  non-empty `canonicalId`.
- Album and track requests remain bound to the source plugin. Track pagination
  treats plugin resource IDs and cursors as opaque values.
- The Vue UI presents empty states, aggregated search, five discover layouts,
  source metadata, partial errors, album details, and track pagination.
- Structured logs record all five content methods without exposing raw plugin
  error details.

## Security Boundaries

- Arbitrary plugin methods, commands, container IDs, socket paths, timeouts,
  Docker fields, and unmanaged containers cannot be selected by content calls.
- Request, opaque ID, cursor, Supervisor frame, and plugin response limits are
  enforced.
- Explicit platform/plugin filters and later-page source pinning cannot be
  bypassed by plugin output.
- Active-call leases prevent idle reconciliation from stopping a plugin during
  a content call.
- Plugin containers have networking disabled, publish no ports, run
  unprivileged with a read-only root filesystem, and receive no Core data,
  downloads, or Docker Socket mounts.
- Only Supervisor receives the Docker Socket. Only Core publishes port 18080.
- Public HTTP failures and persisted structured logs use safe standard error
  codes and redacted summaries.

## Verification

Run the complete phase-one through phase-three suite:

```bash
./scripts/verify.sh
```

Focused phase-three checks:

```bash
./tests/content-phase3-verification-wiring.sh
./tests/virtual-content-contract.sh
./tests/content-aggregation-smoke.sh
./tests/content-aggregation-security.sh
```

The aggregation smoke installs three virtual content plugins from the local
mock public repository, starts them on demand, exercises aggregation, filters,
fallback, partial failures, all discover layouts, source-bound album and track
pagination, structured logs, and plugin stop behavior through the real Core UI.

## Deferred Work

Phase four owns the credential vault, virtual credential plugins, temporary
Cookie Jar sessions, scoped credential promotion, and credential-aware HTTP
proxying. Phase five owns download planning, opaque resource references, task
persistence, and the Core downloader. No part of either phase is implemented
or implied by this acceptance.

Real platform plugins, real platform domains, real Cookie acquisition, real
platform downloads, private repositories, non-Node runtimes, plugin-provided
Dockerfiles, archive organization, format conversion, and Core automatic
updates remain out of scope.
