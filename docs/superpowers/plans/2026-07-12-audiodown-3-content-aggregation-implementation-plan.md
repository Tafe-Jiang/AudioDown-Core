# AudioDown 1.0 Content Aggregation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the phase-three content capability loop: typed content RPC contracts, bounded Core-to-Supervisor invocation, deterministic virtual content plugins, routing and fallback, opaque pagination, canonical-ID deduplication, unified logs, and usable search, discover, album, and track views.

**Architecture:** `audiodown-plugin-api` owns the language-neutral content wire contract. `audiodown-plugin-manager` owns plugin lifecycle, capability checks, active-call leases, and one-plugin invocation. A new dependency-light `audiodown-content` crate owns candidate selection, per-platform fallback, aggregation, cursor envelopes, deduplication, and partial failures. Axum remains a thin adapter over that service. Core sends only typed allowlisted content calls to Supervisor; Supervisor invokes the plugin Unix socket through a fixed script with fixed time and size limits. SQLite remains the source of truth for participation and default-plugin settings. Vue renders only Core-defined models and never accepts plugin HTML or executable UI.

**Tech Stack:** Rust 1.88, Axum, Tokio, SQLx/SQLite, serde, base64url, SHA-256, JSON-RPC 2.0, authenticated Unix sockets, Bollard, Node.js 22 SDK, Vue 3, TypeScript, Vitest, Playwright, Docker Compose.

---

## Delivery Roadmap

1. Foundation and virtual plugin lifecycle - complete.
2. GitHub repository installation and secure Node builds - complete.
3. **Content capabilities and search/discovery aggregation - this plan.**
4. Credential vault, credential plugins, and scoped HTTP proxying.
5. Task engine and Core downloader.
6. Hardening, migration interfaces, diagnostics, and release.

This plan does not add credentials, Cookie handling, HTTP proxying, download planning, download resolution, real file downloads, real platforms, real domains, migration claims, diagnostics, automatic updates, or non-Node runtimes.

## Progress Snapshot

As of 2026-07-12, Tasks 1 through 12 are complete and verified. Tasks 13 and 14 remain pending. The latest `main` push through Task 12 is an explicit intermediate checkpoint requested by the user; phase three is not complete until Task 14 and the clean-checkout verification pass.

## Locked Decisions

- The only phase-three plugin methods are `content.search`, `content.discover`, `content.categories`, `content.album.get`, and `content.tracks.list`.
- Core/Supervisor protocol uses a typed content-method enum. Arbitrary method names, commands, scripts, socket paths, timeouts, container IDs, mounts, or Docker fields never cross the boundary.
- Supervisor applies a fixed 8-second RPC timeout, a 1 MiB response cap, one JSON response line, and strict JSON-RPC identity checks.
- Plugin identity, version, platform, capabilities, and container identity come from trusted installation state, not plugin-supplied routing fields.
- Content calls acquire an active-call lease. Idle reconciliation cannot stop a plugin while a call is in progress.
- Enabled content plugins participate independently in search and discover. The settings are stored separately from global enablement.
- Each platform may have one default content plugin. Candidate order is default first, then ascending priority, then plugin ID.
- Search, discover, and categories aggregate one selected source per platform. A retryable first-page failure may fall back to the next eligible plugin for that platform.
- Explicit `pluginId` filters disable fallback. Explicit `platformId` filters restrict aggregation to that platform.
- Later pages remain pinned to the selected source plugin recorded by the Core cursor. They do not silently switch plugins.
- Album and track calls remain bound to their source plugin because resource IDs are opaque. Cross-plugin resource mapping is deferred until a compatible mapping contract exists.
- Plugin resource IDs and cursors are opaque bounded strings. Core does not parse or infer their format.
- Core cursors are URL-safe base64 encoded, versioned, request-bound, limited to 16 KiB decoded JSON, and contain bounded per-source opaque cursors.
- Deduplication occurs only when resource type and non-empty `canonicalId` both match. Core never infers duplicates from title, author, artwork, or resource ID.
- A single plugin failure produces a safe partial-failure summary and does not discard successful results from other platforms.
- Raw plugin error data is redacted and stored only in structured logs. Public responses expose a standard code, safe summary, plugin ID, and platform ID.
- The UI uses the existing shadcn/Reka visual system. Tabs, select controls, badges, alerts, skeletons, pagination controls, and scroll areas follow the MCP component references already reviewed. React/TSX examples are adapted, not copied.
- The UI contains no remote platform artwork. Virtual fixtures use deterministic local color swatches and text metadata.

## Stable Public HTTP Shape

```text
GET  /api/v1/search?q=&platformId=&pluginId=&cursor=
GET  /api/v1/discover?platformId=&pluginId=&cursor=
GET  /api/v1/categories?platformId=&pluginId=
POST /api/v1/albums/get
POST /api/v1/tracks/list
PATCH /api/v1/plugins/{pluginId}/content-settings
PUT   /api/v1/platforms/{platformId}/default-content-plugin
```

Search and discover responses use:

```json
{
  "items": [],
  "sections": [],
  "nextCursor": null,
  "failures": [],
  "emptyState": null
}
```

Fields not applicable to the operation are empty arrays. The no-plugin case remains HTTP 200 with `emptyState.reason = "NO_CONTENT_PLUGINS"`.

## Execution Rules

- Start every Task with `git status --short --branch`.
- Execute Tasks in order without merging, skipping, or expanding them.
- For every Task: write the failing test, run it and observe the intended failure, implement the minimum code, run the stated passing verification, then commit.
- Use the exact suggested Chinese commit subject and add one concise Chinese commit body.
- Diagnose failures. Do not delete tests, weaken limits, bypass security checks, or accept an unrelated failing baseline.
- Prefer local Rust/Cargo when available; otherwise use the repository's Docker verification pattern with `rust:1.88-bookworm`.
- Do not push intermediate phase-three commits. Push `main` only after Task 14 passes from a clean checkout.

### Task 1: Define Content RPC Contracts and Standard Errors

**Files:**
- Create: `crates/audiodown-plugin-api/src/content.rs`
- Create: `crates/audiodown-plugin-api/src/error.rs`
- Modify: `crates/audiodown-plugin-api/src/lib.rs`
- Modify: `crates/audiodown-plugin-api/src/manifest.rs`
- Test: `crates/audiodown-plugin-api/tests/content_contracts.rs`
- Test: `crates/audiodown-plugin-manager/tests/repository_validation.rs`

- [ ] **Step 1: Write failing contract tests**

Cover strict serde round trips for all five methods, content resource types, discover layouts, bounded opaque IDs/cursors, safe plugin failures, and the standard codes needed by routing:

```text
INVALID_REQUEST
PLUGIN_NOT_FOUND
PLUGIN_DISABLED
PLUGIN_CAPABILITY_MISSING
PLUGIN_UNAVAILABLE
PLUGIN_TIMEOUT
PLUGIN_RESPONSE_INVALID
RESOURCE_NOT_FOUND
RESOURCE_ACCESS_DENIED
RESOURCE_TEMPORARILY_UNAVAILABLE
RATE_LIMITED
PLATFORM_RESPONSE_CHANGED
PLUGIN_INTERNAL_ERROR
```

Add manifest validation tests that reject unknown capability names while retaining the phase-one system methods internally.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-plugin-api --test content_contracts
cargo test -p audiodown-plugin-manager --test repository_validation
```

Expected: FAIL because content contracts and the capability allowlist do not exist.

- [ ] **Step 3: Implement the minimum contract**

Add strict request/result structs for search, discover, categories, album, and tracks. Add `ContentMethod`, `ContentResourceType`, plugin-owned item/detail types, `DiscoverSection`, `DiscoverLayout`, `CategoryItem`, `AlbumDetail`, `TrackItem`, standard plugin error data, and bounded validation helpers. Do not let plugins supply trusted source-plugin identity. Keep download and credential types out of this Task.

- [ ] **Step 4: Run contract and validation checks**

```bash
cargo test -p audiodown-plugin-api
cargo test -p audiodown-plugin-manager --test repository_validation
cargo clippy -p audiodown-plugin-api -p audiodown-plugin-manager --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-api crates/audiodown-plugin-manager/tests/repository_validation.rs
git commit -m "阶段3：定义内容能力协议" \
  -m "定义五类内容调用、标准资源模型和可回退错误契约。"
```

### Task 2: Extend the Node SDK Content Helpers

**Files:**
- Create: `plugin-sdk/node/src/content.js`
- Modify: `plugin-sdk/node/src/index.js`
- Modify: `plugin-sdk/node/src/rpc.js`
- Create: `plugin-sdk/node/test/content.test.js`
- Modify: `plugin-sdk/node/test/rpc.test.js`

- [ ] **Step 1: Write failing Node tests**

Test exported capability constants, strict handler registration, bounded cursor/resource strings, valid result normalization, safe `RpcError` creation, rejection of unknown methods, and conversion of unexpected exceptions to `PLUGIN_INTERNAL_ERROR` without leaking the original message.

- [ ] **Step 2: Run and confirm failure**

```bash
cd plugin-sdk/node && npm test
```

Expected: FAIL because content helpers and typed registration do not exist.

- [ ] **Step 3: Add the minimum SDK helpers**

Expose typed content method constants, validators, and `createContentHandlers`. Keep the generic server transport, but reject handler names outside the built-in system methods and the phase-three allowlist. Preserve the 1 MiB input limit.

- [ ] **Step 4: Run SDK tests**

```bash
cd plugin-sdk/node && npm test
```

- [ ] **Step 5: Commit**

```bash
git add plugin-sdk/node
git commit -m "阶段3：扩展内容插件 SDK" \
  -m "为 Node 插件提供受限内容处理器和安全错误映射。"
```

### Task 3: Add the Typed Supervisor Content RPC Protocol

**Files:**
- Modify: `crates/audiodown-supervisor-protocol/Cargo.toml`
- Modify: `crates/audiodown-supervisor-protocol/src/lib.rs`
- Modify: `crates/audiodown-supervisor-protocol/tests/contracts.rs`
- Modify: `crates/audiodown-server/src/supervisor.rs`
- Modify: `crates/audiodown-server/tests/supervisor_client.rs`
- Modify: `crates/audiodown-supervisor/src/server.rs`
- Modify: `crates/audiodown-supervisor/tests/protocol.rs`

- [ ] **Step 1: Write failing protocol tests**

Require `plugin.rpc` to accept only `PluginRpcRequest { pluginId, method, params }`, reject every other parameter variant, serialize only the five typed methods, enforce request and response size limits, and decode a typed `PluginRpcResult`.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-supervisor-protocol
cargo test -p audiodown-server --test supervisor_client
cargo test -p audiodown-supervisor --test protocol
```

Expected: FAIL because `plugin.rpc` is absent.

- [ ] **Step 3: Implement the protocol and Core client**

Add the enum variant, strict parameter variant, result type, and `SupervisorClient::invoke_plugin`. Reuse authenticated request framing and existing maximum message validation. Do not accept caller-controlled timeout or Docker values. Add a temporary stable `PLUGIN_RPC_UNAVAILABLE` dispatch branch so the new exhaustive method compiles; Task 4 replaces only that branch with real runtime execution.

- [ ] **Step 4: Run protocol checks**

```bash
cargo test -p audiodown-supervisor-protocol
cargo test -p audiodown-server --test supervisor_client
cargo test -p audiodown-supervisor --test protocol
cargo clippy -p audiodown-supervisor-protocol -p audiodown-server --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-supervisor-protocol crates/audiodown-server/src/supervisor.rs crates/audiodown-server/tests/supervisor_client.rs crates/audiodown-supervisor/tests/protocol.rs
git commit -m "阶段3：添加受限插件调用协议" \
  -m "通过认证 Unix 控制面传递类型化内容 RPC，不开放任意方法。"
```

### Task 4: Execute Bounded Content RPC in Supervisor

**Files:**
- Modify: `crates/audiodown-supervisor/src/docker.rs`
- Modify: `crates/audiodown-supervisor/src/server.rs`
- Create: `crates/audiodown-supervisor/tests/content_rpc.rs`
- Modify: `crates/audiodown-supervisor/Cargo.toml`

- [ ] **Step 1: Write failing Docker adapter tests**

Test fixed command construction, trusted managed-container lookup, method mapping, 8-second timeout, one-line JSON-RPC response validation, request ID matching, 1 MiB cap, stderr handling, non-zero exit mapping, malformed JSON rejection, and unavailable-container errors.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-supervisor --test content_rpc
```

Expected: FAIL because the Docker adapter has no generic content invocation.

- [ ] **Step 3: Implement fixed runtime invocation**

Add one fixed Node exec script that connects only to `/tmp/audiodown-rpc.sock`, sends the typed request, accepts exactly one matching response, and exits. Apply outer Tokio timeout and byte caps. Dispatch `plugin.rpc` through the existing trusted install/container lookup.

- [ ] **Step 4: Run Supervisor checks**

```bash
cargo test -p audiodown-supervisor
cargo clippy -p audiodown-supervisor --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-supervisor
git commit -m "阶段3：实现插件内容 RPC 执行" \
  -m "在 Supervisor 中以固定脚本、超时和响应上限调用插件 Unix Socket。"
```

### Task 5: Persist Content Routing Settings

**Files:**
- Create: `migrations/0003_content_routing.sql`
- Create: `crates/audiodown-storage/src/content_routing_repository.rs`
- Modify: `crates/audiodown-storage/src/lib.rs`
- Modify: `crates/audiodown-storage/src/plugin_repository.rs`
- Create: `crates/audiodown-storage/tests/content_routing.rs`
- Modify: `crates/audiodown-storage/tests/storage.rs`

- [ ] **Step 1: Write failing migration and repository tests**

Cover upgrade from the phase-two schema, default search/discover participation, independent participation updates, one default plugin per platform, replacement of a previous default, rejection of wrong-platform or non-content defaults, ordered eligible candidates, and cleanup on uninstall.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-storage --test content_routing
cargo test -p audiodown-storage --test storage
```

Expected: FAIL because migration `0003` and the repository do not exist.

- [ ] **Step 3: Add migration and repository**

Add participation columns and a `platform_content_defaults` table with foreign-key cleanup. Implement transactional setters that verify plugin type and platform. Keep `plugins.priority` as the secondary order and keep SQLite as the only routing source of truth.

- [ ] **Step 4: Run storage checks**

```bash
cargo test -p audiodown-storage
cargo clippy -p audiodown-storage --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add migrations crates/audiodown-storage
git commit -m "阶段3：保存内容路由设置" \
  -m "持久化搜索发现参与开关和每个平台的默认内容插件。"
```

### Task 6: Add Plugin Invocation Leases and Lifecycle Integration

**Files:**
- Modify: `crates/audiodown-plugin-manager/src/service.rs`
- Modify: `crates/audiodown-plugin-manager/tests/lifecycle_service.rs`
- Create: `crates/audiodown-plugin-manager/tests/content_invocation.rs`
- Modify: `crates/audiodown-server/src/plugin_manager_adapters.rs`
- Modify: `crates/audiodown-server/tests/lifecycle_reconciler.rs`

- [ ] **Step 1: Write failing invocation tests**

Cover enabled/content/capability validation, on-demand start before invocation, no redundant start for healthy containers, trusted runtime RPC forwarding, `last_used_at` touch, structured start/result/failure logs, active-call lease protection from idle stop, lease release after timeout/error, and no more than three automatic start retries.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-plugin-manager --test content_invocation
cargo test -p audiodown-plugin-manager --test lifecycle_service
cargo test -p audiodown-server --test lifecycle_reconciler
```

Expected: FAIL because content invocation and active-call leases do not exist.

- [ ] **Step 3: Implement the minimum manager flow**

Extend the state-store port with `touch`, extend the runtime port with typed `invoke`, track active calls per plugin, and expose `invoke_content`. Reuse existing start/health behavior and operation locks only for lifecycle transitions; do not serialize unrelated healthy RPC calls.

- [ ] **Step 4: Run manager and lifecycle checks**

```bash
cargo test -p audiodown-plugin-manager
cargo test -p audiodown-server --test lifecycle_reconciler
cargo clippy -p audiodown-plugin-manager -p audiodown-server --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-manager crates/audiodown-server/src/plugin_manager_adapters.rs crates/audiodown-server/tests/lifecycle_reconciler.rs
git commit -m "阶段3：接通内容插件调用生命周期" \
  -m "按需启动内容插件并用调用租约保护在途 RPC。"
```

### Task 7: Implement Candidate Selection, Aggregation, and Fallback

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/audiodown-content/Cargo.toml`
- Create: `crates/audiodown-content/src/lib.rs`
- Create: `crates/audiodown-content/src/router.rs`
- Create: `crates/audiodown-content/tests/routing.rs`
- Create: `crates/audiodown-content/tests/aggregation.rs`

- [ ] **Step 1: Write failing routing tests**

Cover platform grouping, default-first ordering, priority and plugin-ID tie breaks, search/discover participation, platform/plugin filters, one selected source per platform, retryable first-page fallback, no fallback for explicit plugin filters, no fallback for non-retryable errors, and partial success when one platform fails.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-content
```

Expected: FAIL because the crate is not a workspace member.

- [ ] **Step 3: Implement the dependency-light service**

Define storage and invoker ports, candidate records, trusted `ContentSource`, operation inputs, aggregation responses, and safe failure summaries. Execute independent platform groups concurrently with a small fixed fan-out limit while preserving deterministic output order.

- [ ] **Step 4: Run content routing checks**

```bash
cargo test -p audiodown-content
cargo clippy -p audiodown-content --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/audiodown-content
git commit -m "阶段3：实现内容聚合与回退" \
  -m "按平台选择默认插件并在可重试失败时执行优先级回退。"
```

### Task 8: Add Opaque Cursors and Canonical Deduplication

**Files:**
- Create: `crates/audiodown-content/src/cursor.rs`
- Create: `crates/audiodown-content/src/dedup.rs`
- Modify: `crates/audiodown-content/src/lib.rs`
- Modify: `crates/audiodown-content/src/router.rs`
- Create: `crates/audiodown-content/tests/cursor.rs`
- Create: `crates/audiodown-content/tests/dedup.rs`

- [ ] **Step 1: Write failing pagination and dedup tests**

Cover versioned URL-safe cursors, operation/query/filter binding, selected-plugin pinning, per-source opaque cursor preservation, decoded and encoded size limits, source-count limits, malformed/tampered-shape rejection, end-of-pagination behavior, stable ordering, type-aware canonical deduplication, and preservation of items without canonical IDs.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-content --test cursor
cargo test -p audiodown-content --test dedup
```

Expected: FAIL because cursor and dedup modules do not exist.

- [ ] **Step 3: Implement bounded cursor and dedup logic**

Use URL-safe no-padding base64 JSON with a version and request fingerprint. Treat embedded plugin cursors as opaque strings. Keep the first deterministic item for a duplicate canonical key and merge no untrusted metadata.

- [ ] **Step 4: Run content crate checks**

```bash
cargo test -p audiodown-content
cargo clippy -p audiodown-content --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-content Cargo.lock
git commit -m "阶段3：添加不透明分页与结果去重" \
  -m "封装请求绑定游标并仅依据 canonicalId 执行类型安全去重。"
```

### Task 9: Expose Content APIs and Unified Call Logs

**Files:**
- Modify: `crates/audiodown-content/src/dedup.rs`
- Modify: `crates/audiodown-content/src/lib.rs`
- Modify: `crates/audiodown-content/src/router.rs`
- Modify: `crates/audiodown-content/tests/aggregation.rs`
- Create: `crates/audiodown-server/src/content_adapters.rs`
- Create: `crates/audiodown-server/src/routes/content.rs`
- Modify: `crates/audiodown-server/src/routes/mod.rs`
- Modify: `crates/audiodown-server/src/app.rs`
- Modify: `crates/audiodown-server/src/state.rs`
- Modify: `crates/audiodown-server/src/lib.rs`
- Modify: `crates/audiodown-server/src/main.rs`
- Modify: `crates/audiodown-server/src/routes/plugins.rs`
- Create: `crates/audiodown-server/tests/content_api.rs`
- Modify: `crates/audiodown-server/tests/http_api.rs`

**Plan clarification:** Task 8 establishes the bounded cursor and deduplication primitives. Task 9 connects those primitives to the aggregation service, adds the already-locked categories aggregation path, and then keeps Axum as a thin adapter. These content-crate changes are required by the architecture above and do not add a new capability or phase.

- [ ] **Step 1: Write failing HTTP tests**

Cover all six public content/settings endpoints, query validation, 200 no-plugin empty states, platform/plugin filters, partial failures, cursor continuation, source-bound album/tracks, default/participation setting validation, stable safe API errors, and structured logs containing request ID, plugin ID, platform ID, method, duration, result count, and standard error code without raw plugin data.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-server --test content_api
cargo test -p audiodown-server --test http_api
```

Expected: FAIL because content routes and adapters do not exist.

- [ ] **Step 3: Wire thin adapters**

Construct the content service from SQLite routing and plugin-manager invocation adapters. Add request IDs, safe error mapping, and redacted structured call logs. Preserve existing `/search` and `/discover` paths while replacing their fixed empty handlers.

- [ ] **Step 4: Run server checks**

```bash
cargo test -p audiodown-server
cargo clippy -p audiodown-server --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-server
git commit -m "阶段3：开放内容聚合接口" \
  -m "提供搜索发现分类专辑曲目接口并记录脱敏调用日志。"
```

### Task 10: Build Deterministic Virtual Content Plugins

**Files:**
- Modify: `test-fixtures/repositories/virtual/audiodown-repository.json`
- Modify: `test-fixtures/repositories/virtual/plugins/virtual-content/audiodown-plugin.json`
- Modify: `test-fixtures/repositories/virtual/plugins/virtual-content/src/index.js`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-content-backup/`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-catalog/`
- Modify: `test-fixtures/repositories/virtual/README.md`
- Modify: `crates/audiodown-supervisor/src/docker_build.rs`
- Modify: `crates/audiodown-supervisor/tests/docker_build.rs`
- Modify: `tests/plugin-repository-smoke.sh`
- Create: `tests/virtual-content-contract.sh`
- Modify: `web/tests/plugin-installation-live.spec.ts`

- [ ] **Step 1: Write failing fixture contract test**

Install/run each virtual content plugin through the SDK transport and verify deterministic search, discover layouts, categories, album detail, opaque track pagination, canonical overlap, retryable failure behavior, safe hard failure behavior, and no real host/domain/platform strings.

- [ ] **Step 2: Run and confirm failure**

```bash
./tests/virtual-content-contract.sh
```

Expected: FAIL because the phase-three virtual handlers and fallback fixtures do not exist.

- [ ] **Step 3: Implement virtual-only fixtures**

Add a primary and backup plugin for one virtual platform plus one plugin for a second virtual platform. Use only local deterministic data. Add test-only query switches for timeout/retryable/hard failures without network access.

Keep the Supervisor's trusted runtime SDK context synchronized with the complete phase-three Node SDK file set. The repository smoke and existing live installation test must accept the expanded virtual repository index without weakening their lifecycle or security assertions.

- [ ] **Step 4: Run fixture and repository checks**

```bash
./tests/virtual-content-contract.sh
./tests/plugin-repository-smoke.sh
cargo test -p audiodown-supervisor --test docker_build
cargo clippy -p audiodown-supervisor --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add \
  crates/audiodown-supervisor \
  docs/superpowers/plans/2026-07-12-audiodown-3-content-aggregation-implementation-plan.md \
  test-fixtures/repositories/virtual \
  tests \
  web/tests/plugin-installation-live.spec.ts
git commit -m "阶段3：添加虚拟内容插件集" \
  -m "用确定性虚拟数据覆盖聚合、分页、去重和失败回退场景。"
```

### Task 11: Add Typed Vue Content API and Shared Result Components

**Files:**
- Modify: `crates/audiodown-server/src/routes/plugins.rs`
- Modify: `crates/audiodown-server/tests/content_api.rs`
- Modify: `web/src/api/client.ts`
- Create: `web/src/components/content/ContentSourceBadge.vue`
- Create: `web/src/components/content/ContentItemRow.vue`
- Create: `web/src/components/content/ContentGrid.vue`
- Create: `web/src/components/content/ContentFailureAlert.vue`
- Create: `web/src/components/content/ContentPagination.vue`
- Create: `web/src/components/content/content-components.test.ts`
- Modify: `web/src/components/plugins/PluginSettingsSheet.vue`
- Modify: `web/src/components/plugins/PluginTable.vue`
- Modify: `web/src/components/plugins/plugin-management.test.ts`
- Add as needed: `web/src/components/ui/tabs/`
- Add as needed: `web/src/components/ui/pagination/`

- [ ] **Step 1: Write failing component and API tests**

Test typed envelopes and API error codes, source/version badges, keyboard-accessible rows, loading skeleton stability, partial failure alerts, cursor next/previous controls, long unbroken metadata, plugin capabilities, search/discover participation controls, and default-plugin selection.

- [ ] **Step 2: Run and confirm failure**

```bash
cd web && npm test -- --run src/components/content/content-components.test.ts src/components/plugins/plugin-management.test.ts
```

Expected: FAIL because content types and components do not exist.

- [ ] **Step 3: Implement shared components**

Adapt the reviewed shadcn tabs, pagination, select, badge, alert, skeleton, and scroll-area patterns to Vue/Reka. Keep controls compact, responsive, and consistent with the existing phase-two shell. Do not nest cards or add gradients.

Expose the manifest capabilities and persisted content participation/default state through the existing plugin-list response so the settings UI initializes from Core-owned data. Do not add a second settings source or infer defaults in the browser.

- [ ] **Step 4: Run Vue component checks**

```bash
cargo test -p audiodown-server --test content_api
cd web && npm test -- --run src/components/content/content-components.test.ts src/components/plugins/plugin-management.test.ts
cd web && npm run typecheck
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-server web
git commit -m "阶段3：构建内容结果组件" \
  -m "增加类型化内容 API、来源标识、失败提示和分页控件。"
```

### Task 12: Implement the Search Workspace

**Files:**
- Modify: `web/src/views/SearchView.vue`
- Create: `web/src/views/SearchView.test.ts`
- Modify: `web/src/views/content-empty-states.test.ts`
- Modify: `web/src/views/empty-state.test.ts`
- Modify: `web/tests/fixtures/mock-api.ts`
- Modify: `web/tests/accessibility.spec.ts`
- Modify: `web/tests/visual/ui-visual.spec.ts`
- Add visual baselines under: `web/tests/visual/ui-visual.spec.ts-snapshots/`

- [ ] **Step 1: Write failing search UI tests**

Cover query submission, platform/plugin filters, empty query validation, loading state, aggregated results, source and version display, canonical dedup output, partial failures, no-plugin empty state, cursor pagination, keyboard operation, mobile wrapping, no horizontal overflow, and no serious accessibility violations.

- [ ] **Step 2: Run and confirm failure**

```bash
cd web && npm test -- --run src/views/SearchView.test.ts
npx playwright test tests/accessibility.spec.ts tests/visual/ui-visual.spec.ts --grep "Search"
```

Expected: FAIL because Search still renders only the phase-two empty state.

- [ ] **Step 3: Implement the search workspace**

Use a compact command form with query, platform, and plugin controls; render results as scan-friendly rows/grid based on resource type; keep safe failure summaries visible without replacing successful items.

- [ ] **Step 4: Run search checks**

```bash
cd web && npm test -- --run src/views/SearchView.test.ts
cd web && npm run typecheck
npx playwright test tests/accessibility.spec.ts tests/visual/ui-visual.spec.ts --grep "Search"
```

- [ ] **Step 5: Commit**

```bash
git add web
git commit -m "阶段3：实现聚合搜索界面" \
  -m "展示跨虚拟平台搜索结果、筛选、来源、错误摘要和分页。"
```

### Task 13: Implement Discover, Album, and Track Views

**Files:**
- Modify: `web/src/views/DiscoverView.vue`
- Create: `web/src/views/AlbumView.vue`
- Modify: `web/src/router.ts`
- Create: `web/src/views/DiscoverView.test.ts`
- Create: `web/src/views/AlbumView.test.ts`
- Modify: `web/tests/fixtures/mock-api.ts`
- Modify: `web/tests/accessibility.spec.ts`
- Modify: `web/tests/visual/ui-visual.spec.ts`
- Add visual baselines under: `web/tests/visual/ui-visual.spec.ts-snapshots/`

- [ ] **Step 1: Write failing discover and album tests**

Cover all five standard discover layouts, categories, platform/plugin filters, partial failures, no-plugin empty state, source-bound album navigation, album metadata, paginated tracks, cursor controls, safe not-found errors, keyboard operation, responsive layout, and accessibility.

- [ ] **Step 2: Run and confirm failure**

```bash
cd web && npm test -- --run src/views/DiscoverView.test.ts src/views/AlbumView.test.ts
npx playwright test tests/accessibility.spec.ts tests/visual/ui-visual.spec.ts --grep "Discover|Album"
```

Expected: FAIL because discover is empty-only and album/track views do not exist.

- [ ] **Step 3: Implement views**

Render only Core-defined layouts and local virtual swatches. Use tabs or compact section navigation where useful, not nested cards. Preserve source plugin identity through album and track requests and paginate tracks without interpreting the cursor.

- [ ] **Step 4: Run content UI checks**

```bash
cd web && npm test -- --run
cd web && npm run typecheck
npx playwright test tests/accessibility.spec.ts tests/visual/ui-visual.spec.ts
```

- [ ] **Step 5: Commit**

```bash
git add web
git commit -m "阶段3：实现发现专辑与曲目界面" \
  -m "渲染标准发现布局、专辑详情和不透明游标曲目分页。"
```

### Task 14: Verify the Phase-Three End-to-End Loop

**Files:**
- Create: `tests/content-aggregation-smoke.sh`
- Create: `tests/content-aggregation-security.sh`
- Create: `tests/content-phase3-verification-wiring.sh`
- Create: `web/tests/content-aggregation-live.spec.ts`
- Modify: `scripts/verify.sh`
- Modify: `.github/workflows/ci.yml`
- Create: `docs/phase-3-acceptance.md`

- [ ] **Step 1: Write failing verification wiring and live tests**

Require `verify.sh` and CI to run the content contract, aggregation smoke, security checks, and live Playwright flow. The live loop must install virtual content plugins, set participation/defaults, search across platforms, trigger fallback and partial failure, open an album, paginate tracks, inspect structured logs, and stop plugins.

Security checks must prove content calls cannot:

- invoke methods outside the five-method allowlist;
- exceed request/cursor/response limits;
- select an unmanaged container;
- bypass explicit plugin/platform filters;
- leak raw plugin errors into HTTP or logs;
- race idle shutdown during an active call;
- add public network, Core data, downloads, or Docker Socket access.

- [ ] **Step 2: Run and confirm failure**

```bash
./tests/content-phase3-verification-wiring.sh
./tests/content-aggregation-smoke.sh
./tests/content-aggregation-security.sh
```

Expected: FAIL until scripts, CI wiring, and the full live path exist.

- [ ] **Step 3: Complete verification and acceptance documentation**

Wire the new tests after the phase-two checks. Update the final verify banner to phase three. Document completed capabilities, security boundaries, test commands, and deferred phase-four/five work. Do not claim real platform support.

- [ ] **Step 4: Verify from a clean checkout**

```bash
git status --short --branch
verification_dir="$(mktemp -d /tmp/audiodown-phase3.XXXXXX)"
git clone --no-local . "$verification_dir/repo"
cd "$verification_dir/repo"
./scripts/verify.sh
```

Expected: the complete phase-one through phase-three suite passes, Compose is cleaned up, and no managed plugin containers or test networks remain.

- [ ] **Step 5: Commit**

```bash
git add tests web/tests scripts/verify.sh .github/workflows/ci.yml docs/phase-3-acceptance.md
git commit -m "阶段3：验证内容聚合完整闭环" \
  -m "接入全量验证、CI、实时界面测试和阶段验收记录。"
```

## Phase Completion

After Task 14:

```bash
git status --short --branch
./scripts/verify.sh
git push origin main
```

Confirm:

- `main` is clean and synchronized with `origin/main`;
- GitHub CI succeeds for the pushed commit;
- Core and Supervisor still satisfy all phase-one and phase-two boundaries;
- only Core exposes port `18080`;
- only Supervisor has Docker Socket access;
- virtual content plugins can install, start, handshake, serve all five methods, log, and stop;
- search/discover aggregate across virtual platforms with filters, defaults, priority, fallback, partial failures, opaque cursors, and canonical deduplication;
- album and tracks remain source-plugin-bound;
- no real platform, domain, credential, Cookie, download, archive, updater, private repository, or non-Node runtime behavior was added.
